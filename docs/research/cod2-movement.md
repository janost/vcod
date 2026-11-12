# CoD2 1.3 movement against CoD1

What Call of Duty 2 patch 1.3's `bg_pmove`, `bg_slidemove`, `bg_jump` and
`bg_mantle` do differently from the CoD1 movement vcod runs (`pmove.rs`,
`cod11-mantle.md`, `cod15-movement.md`, `coduo-movement.md`).

In short, CoD2 starts from 1.5's movement (the jump penalty, one stance at
a time, the halved standing lean, the 9-unit ground snap) and adds:

- a mantle;
- an inertia clamp;
- a jump that never rises past its height;
- a re-ordered clip plane solver;
- a trace with no end point.

The usercmd lost `upmove` and `wbuttons`: jump, stance and lean are bits
of `buttons`. The tunables CoD1 kept in the playerstate are rodata
immediates or dvars now, and the dvars reach clients.

## Sources and conventions

- `cod2_lnxded`: the Linux 1.3 dedicated server, game module linked in.
  Addresses are its VAs (`.text` 0x804a6c0, `.rodata` 0x813e340 at file
  offset 0xf6340, `.data` 0x817d3a0). Names come from the project's symbol
  map (transferred from the Mac build, or from zk_libcod's index) or, where
  the map has none, from the matching Mac function or from what the
  function does. Every such name is INFERRED.
- The Mac 1.3 MP binary (i386 slice) carries full symbols and STABS. Its
  addresses read "Mac 0x...". It is an optimized build that inlines
  `PmoveSingle`, `PM_WalkMove`, `PM_CheckJump` and most helpers into
  `Pmove` (Mac 0x70c7e, 10398 bytes), so the pairing below is by callee,
  constant and string, not by name.
- `CoD2MP_s.exe`: the Windows 1.3 client. CoD2 has no cgame DLL; the
  client's pmove is in the exe.
- Floats were read from the file's bytes, not from the decompiler's
  printing. A "rodata 0x..." value is a float32 at that VA; an "immediate"
  is a `mov $0x...` in the named function.
- Every claim carries its own label. VERIFIED: read out of a binary (bytes,
  immediates, strings, STABS) or an asset. INFERRED: read off control flow.

Contents: 1 function map · 2 stored floats · 3 structures · 4 the chop · 5
friction and moves · 6 jump · 7 ground, slide, step · 8 fall · 9 mantle ·
10 stance and view · 11 ladder · 12 flags and events · 13 tunables · 14
open · 15 what a port needs · 16 vcod's port · 17 the A/B gate · 18 client
prediction · 19 breath hold · 20 other bodies in the prediction.

## 1. Function map

| Function | `cod2_lnxded` | Mac | How named |
|---|---|---|---|
| `Pmove` | 0x80e9464 | 0x70c7e | zk |
| `PmoveSingle` | 0x80e8934 | inlined | its only caller is `Pmove`'s chop loop |
| `PM_playerTrace` | 0x80e1360 | 0x6cd96 | zk |
| `PM_AddEvent` / `PM_AddTouchEnt` | 0x80e1452 / 0x80e1474 | 0x6b6ac / 0x6b65e | zk |
| `PM_ClipVelocity` | 0x80e14d4 | 0x6b8ec | zk |
| `PM_GetEffectiveStance` | 0x80e1532 | 0x6b6ce | zk |
| `PM_Friction` | 0x80e156e | 0x6bc62 | reads `friction` and `stopspeed` |
| inertia test / clamp | 0x80e170c / 0x80e17fe | inlined | the `inertiaDebug` strings |
| `PM_Accelerate` | 0x80e188e | 0x6b970 | calls the clamp |
| `PM_CmdScale` (3 axes, 2 axes, walk) | 0x80e1a02 / 0x80e1b1e / 0x80e1c58 | inlined | the 127 divisor |
| `PM_DamageScale_Walk` | 0x80e1f20 | 0x2aa6d88 (stub) | the `player_dmgtimer_*` dvars |
| `PM_SetMovementDir` | 0x80e1f8c | 0x6ceb8 | writes `movementDir` |
| `PM_GroundSurfaceType` | 0x80e2220 | 0x6b6ee | zk |
| fly move (spectator) | 0x80e225e | inlined | `pm_type` 4 arm |
| `PM_AirMove` | 0x80e23f8 | 0x6d060 | |
| `PM_WalkMove` | 0x80e25a6 | inlined | calls `Jump_Check` |
| dead move | 0x80e2a22 | inlined | `pm_type` 6 |
| noclip / UFO move | 0x80e2aa2 / 0x80e2cec | inlined | `pm_type` 2 / 3 |
| `PM_CrashLand` | 0x80e30c4 | inlined | the `bg_fallDamage*` string |
| correct all-solid / ground trace missed | 0x80e34c6 / 0x80e36aa | inlined | |
| `PM_GroundTrace` | 0x80e38a2 | 0x6fbe8 | |
| `PM_GetViewHeightLerpTime` | 0x80e3c46 | 0x6b70a | |
| `PM_ViewHeightAdjust` | 0x80e3eb0 | 0x6d7aa | |
| `PM_CheckDuck` | 0x80e4644 | 0x6e08e | raises events 140-142 |
| `PM_FootstepEvent` / `PM_ShouldMakeFootsteps` / `PM_Footsteps` | 0x80e5402 / 0x80e55ca / 0x80e572c | 0x6eab2 / 0x6b8a2 / 0x6ed5c | zk, strings |
| foliage sound | 0x80e6784 | inlined | the `bg_foliagesnd_*` dvars, event 139 |
| `PM_DropTimers` | 0x80e6936 | inlined | |
| `PM_UpdateLean` | 0x80e69f4 | 0x6bdce | |
| `BG_CheckProneTurned` | 0x80e6dac | 0x6b790 | |
| `PM_UpdateViewAngles` / `PM_UpdatePronePitch` | 0x80e6eb8 / 0x80e7c00 | 0x6c082 / 0x6cafe | |
| `PM_SetProneMovementOverride` | 0x80e7f44 | 0x6b730 | |
| `PM_CheckLadderMove` / `PM_LadderMove` | 0x80e805a / 0x80e847e | inlined | the 8 / 30 reaches |
| `BG_GetSpeed` | 0x80e9528 | 0x6b746 | zk |
| `PM_VerifyPronePosition` | 0x80e9a7c | inlined | zk |
| `PM_PermuteRestrictiveClipPlanes` | 0x80e9b7c | inlined | zk |
| `PM_SlideMove` / `PM_StepSlideMove` | 0x80e9c42 / 0x80ea3f4 | 0x9d318 / 0x9dc9c | zk / callers |
| `BG_CheckProneValid` | 0x80e01f4 (`BG_CheckProne` 0x80e0e58) | 0x6a534 | |
| `Jump_RegisterDvars` .. `Jump_Check` | 0x80dc60c .. 0x80dcca8 | 0x9ccf0 .. 0x9cfb2 | section 6 |
| `Mantle_RegisterDvars` .. `Mantle_CapView` | 0x80dcf8c .. 0x80de47a | 0x9e618 .. 0x9f6c6 | section 9 |
| `BG_RegisterDvars` | 0x80de830 | 0x694d4 | strings |
| `ClientEndFrame` | 0x80f7516 | | map |

The addresses and the Mac symbols are VERIFIED. Every row named off a
callee, a constant or a string, and every "inlined" pairing, is INFERRED.

## 2. Codegen: stored floats

- `cod2_lnxded`'s `.comment` lists gcc 3.3.6 (159 objects) and gcc 3.4.5
  (4). VERIFIED.
- It is an unoptimized build: every function keeps a frame pointer, every
  local lives in memory, and the vector helpers are out-of-line copies, one
  per translation unit (`VectorCopy` at 0x80e97fe in `bg_pmove`'s and
  0x80eaede in `bg_slidemove`'s are the same instructions). VERIFIED.
- Every assignment to a float variable stores it as float32:
  `PM_ClipVelocity` stores the dot (0x80e14ec) before it reads it back. A
  dot product's sum stays on the x87 stack until its caller stores it
  (`DotProduct` 0x80e9980). `sqrt` is libc's, called on the float widened
  to double and stored back to float (0x80e9590). VERIFIED.
- So the server is built like 1.5b and UO (`cod15-movement.md` 7), not like
  1.1d: vcod's `Arith::Stored` model, applied to every line of pmove rather
  than to the three stores 1.5 needed. INFERRED.
- The Windows client is an optimized MSVC build that keeps intermediates
  on the stack. Its `PM_CrashLand` (`CoD2MP_s.exe` 0x516a20, the
  `bg_fallDamageMaxHeight` string pushed at 0x516ac7) runs an inline
  `fsqrt` and keeps the impact speed on the stack where the server stores
  it. VERIFIED. A client replay of a server capture will correct in the
  last bits, as CoD1's does (`cod15-movement.md` 8). INFERRED.

## 3. Structures

### usercmd_s (STABS, 28 bytes)

`serverTime` 0, `buttons` 4, `weapon` 8, `offHandIndex` 9, `angles[3]`
0xc, `forwardmove` 0x18, `rightmove` 0x19. VERIFIED. There is no `upmove`
and no `wbuttons`.

The `buttons` bits, INFERRED off the tests in `Jump_Check` 0x80dcca8,
`PM_CheckDuck`, `PM_UpdateLean` and `PM_UpdateAimDownSightFlag` 0x80ecb42:

| Bit | Key |
|---|---|
| 0x1 | attack |
| 0x40 / 0x80 | lean left / right |
| 0x100 | prone |
| 0x200 | crouch |
| 0x400 | jump |
| 0x800 | set by the client while shellshocked (`CL_SetUserCmdInShellshock`, Mac 0x17a28c) |
| 0x1000 | sight |

1.1's held-jump latch (`pm_flags` 0x8 writing 20 into `upmove`,
`pmove::chop`) has nothing to write. `Pmove` 0x80e9464 copies `cmd` into
`oldcmd` after each chopped step, and `Jump_Check` jumps on the 0x400 edge
against `oldcmd`. INFERRED.

### pmove_t (STABS, 248 bytes)

`ps` 0, `cmd` 4, `oldcmd` 0x20, `tracemask` 0x3c, `numtouch` 0x40,
`touchents[32]` 0x44, `mins` 0xc4, `maxs` 0xd0, `xyspeed` 0xdc,
`proneChange` 0xe0, `handler` 0xe4 (a byte), `mantleStarted` 0xe5,
`mantleEndPos` 0xe8, `mantleDuration` 0xf4. VERIFIED. `handler` indexes a
table of trace functions at `.data` 0x81869c0, three pointers a row;
`PM_playerTrace` calls through it. VERIFIED.

### pml_t (STABS, 124 bytes)

`forward/right/up` 0..0x24, `frametime` 0x24, `msec` 0x28, `walking`
0x2c, `groundPlane` 0x30, `almostGroundPlane` 0x34 (new), `groundTrace`
0x38, `impactSpeed` 0x5c, `previous_origin` 0x60, `previous_velocity`
0x6c, `previous_waterlevel` 0x78. VERIFIED.

### trace_t (STABS, 36 bytes)

`fraction` 0, `normal` 4, `surfaceFlags` 0x10, `contents` 0x14,
`material` 0x18, `entityNum` 0x1c (16 bits), `partName` 0x1e,
`partGroup` 0x20, `allsolid` 0x22, `startsolid` 0x23. VERIFIED.

There is no `endpos`. Every pmove caller rebuilds the point as
`Vec3Lerp(start, end, fraction)` (0x80e129e: `a + (b - a) * f` per
component, stored): `PM_SlideMove` (0x80e9c42) and the step's down pass
(0x80ea3f4). VERIFIED, the calls. 1.1's rule "a trace ends where
`SV_Trace` puts it" (`cod11-movers.md` 21) has no CoD2 counterpart: the
origin after a hit is the lerp, in stored float32. INFERRED.

### playerState_s (STABS, 9896 bytes)

Against 1.1's layout (offsets from `fields_v1.rs`'s netfields):

| Field | CoD2 | 1.1 | Note |
|---|---|---|---|
| `oldVelocity[2]` | 0x2c | none | new; the inertia clamp's (section 5) |
| `weaponRestrictKickTime` | 0x40 | none | new |
| `foliageSoundTime` | 0x44 | 0x38 | |
| `gravity` / `leanf` / `speed` | 0x48 / 0x4c / 0x50 | 0x3c / 0x40 / 0x44 | |
| `delta_angles` / `groundEntityNum` / `vLadderVec` | 0x54 / 0x60 / 0x64 | 0x48 / 0x54 / 0x58 | |
| `jumpTime` / `jumpOriginZ` | 0x70 / 0x74 | 0x64 / 0x68 (`fJumpPeak`) | |
| `legsAnimDuration`, `torsoAnimDuration` | 0x88, 0x8c | none | new |
| `damageTimer`, `damageDuration`, `flinchYaw` | 0x90, 0x94, 0x98 | none | new; the damage slowdown |
| `eFlags` | 0xa0 | 0x80 | |
| `adsDelayTime` | 0xe0 | none | new |
| `viewHeightTarget` .. `viewHeightLerpPosAdj` | 0xf4 .. 0x108 | 0xcc .. 0xe0 | |
| `viewAngleClampBase/Range` | 0x10c / 0x114 | none | new; the turret's |
| `ammo[128]`, `ammoclip[128]` | 0x144, 0x344 | | |
| `mins` / `maxs` | 0x56c / 0x578 | 0x324 / 0x330 | |
| `proneViewHeight` .. `deadViewHeight` | none | 0x33c .. 0x348 | dropped: immediates now |
| `walkSpeedScale` .. `leanSpeedScale` | none | 0x34c .. 0x364 | dropped: rodata and dvars |
| `friction` | none | 0x37c | dropped: the `friction` dvar |
| `holdBreathScale`, `holdBreathTimer` | 0x5b4, 0x5b8 | none | new: the breath hold (19) |
| `mantleState` {yaw, timer, transIndex, flags} | 0x5bc | none | new |
| `aimSpreadScale` | 0x5d4 | 0x3d8 | |

VERIFIED, both layouts. The playerstate netfield table (0x8142520, 105
entries) sends `jumpOriginZ`, every `mantleState` field, `damageTimer`,
`damageDuration`, and `pm_flags` in 27 bits. It does not send
`oldVelocity`. VERIFIED (`tools/re/netfields.py`). So a client predicting
from a snapshot starts every replay with whatever `oldVelocity` its own
copy holds. INFERRED.

## 4. PmoveSingle and the chop

`Pmove` (0x80e9464):

- A cmd behind `commandTime` does nothing. One more than 1000 ms ahead
  pulls `commandTime` to 1000 short. Steps are at most 66 ms (immediate
  0x42). VERIFIED, the immediates. This is 1.1's chop (`pmove::chop`)
  without the jump latch. INFERRED.
- `numtouch` is cleared once per `Pmove`, not per step, and `oldcmd` takes
  the cmd after every step. VERIFIED, the store at 0x80e94a9 and the copy
  loop after the step's call. The touch list is the whole cmd's, as 1.5's
  and UO's is. INFERRED.

`PmoveSingle` (0x80e8934), in order. INFERRED, the call order, unless a
step says otherwise:

1. Anim conditions. `pm_flags` 0x8000 keeps `buttons & 0x2300` and zeroes
   the move keys and the velocity (a frozen player); `buttons` 0x40000
   keeps `& 0x43300` and zeroes the keys. VERIFIED, the masks.
2. `pm_flags` 0x10000 cleared. A dead player (`pm_type` > 5) drops
   `CONTENTS_BODY` (0x2000000) from the trace mask. VERIFIED, the masks.
3. Prone: a move key that grew against `oldcmd` interrupts the weapon and
   leaves the sight (`PM_InteruptWeaponWithProneMove` 0x80ed2de, then
   `PM_ExitAimDownSight` 0x80ecc72). Prone and sighted (`pm_flags` 0x40)
   zeroes the move keys: no crawling on the sight.
4. `eFlags` 0x200000 follows `buttons` 0x40000. `eFlags` 0x40 (firing) is
   set off attack with ammo in weapon state 0 or 3. VERIFIED, the masks.
5. `msec = clamp(serverTime - commandTime, 1, 200)`, `frametime = msec *
   0.001` (rodata 0x815516c). VERIFIED.
6. `PM_AdjustAimSpreadScale`, `PM_UpdateViewAngles`, `AngleVectors`.
   `pm_flags` 0x80 follows a negative `forwardmove`.
7. The switch on `pm_type` (0 normal, 1 linked, 2 noclip, 3 UFO, 4
   spectator, 5 intermission, 6 dead, 7 dead linked; names zk_libcod's).
   In the default arm:
   - a gunner (`eFlags & 0x300`) runs only the duck, ladder flag, weapon
     and footsteps;
   - a mantling player (`pm_flags` 4) runs `PM_CheckDuck` and
     `Mantle_Move` (section 9 has the whole order);
   - everyone else runs `PM_CheckDuck`, `PM_GroundTrace`, `Mantle_Check`,
     the sight flag, the prone pitch, the dead move, `PM_CheckLadderMove`,
     `PM_DropTimers`, the ladder, walk or air move, `PM_GroundTrace` again,
     `PM_Footsteps`, `PM_Weapon` and the foliage sound.
8. The tail. When `|displacement|^2 / frametime^2` is under 0.25 (rodata
   0x8155170) times the squared speed, the velocity becomes the
   displacement over the frame (1.1's "tail of the default arm").
   `oldVelocity += (velocity - oldVelocity) * min(frametime, 1)` in two
   dimensions. The velocity is snapped with libc `rint` per component
   (0x80d5e94). VERIFIED, the calls. `rint` keeps the sign of a zero,
   where 1.1d's `fistp`/`fild` snap turns -0.0 into 0.0 (`pmove.rs`, the
   snap). INFERRED.

## 5. Friction, accelerate, the walk and the air

- `PM_Friction` (0x80e156e). VERIFIED, the constants; INFERRED, the
  shape:
  - below 1 unit/s all three axes are cleared (`VectorClear` 0x80e97b0),
    as 1.5 does; 1.1 clears x and y only;
  - walking, not on slick ground (`groundTrace.surfaceFlags & 2`) and
    without `pm_flags` 0x400, the control is `max(speed, stopspeed)`,
    times 0.3 (rodata 0x8154ee4) under `pm_flags` 0x200, or times
    `Jump_ReduceFriction` under 0x80000; the drop is `control * friction *
    frametime`;
  - a spectator adds `5 * speed * frametime` (rodata 0x8154ee8).

  `friction` 5.5 and `stopspeed` 100 are dvars (`BG_RegisterDvars`;
  VERIFIED, the defaults). 1.1's are rodata with the same values. There is
  no water term: CoD2 MP has no water level. INFERRED.
- `PM_Accelerate` (0x80e188e), off a ladder: `add = wishspeed - v .
  wishdir`; the step is `min(accel * frametime * max(wishspeed,
  stopspeed), add)`. So 1.1's walk accel floor of 100
  (`WALK_ACCEL_FLOOR`) is the `stopspeed` dvar. INFERRED. The step then
  goes through the inertia clamp.
- The inertia clamp (0x80e17fe, 0x80e170c), new. Unless `pm_type` is 2, a
  step over `inertiaMax` (50) with `|oldVelocity|^2` at least 0.0001
  (double, rodata 0x8154f68) is cut to `inertiaMax` when the flat new
  velocity, normalized, dotted with the flat `oldVelocity`, normalized,
  is below `inertiaAngle` (0). VERIFIED, the dvar defaults and constant.
  In effect a turn of more than 90 degrees in one step accelerates at most
  50 units. INFERRED.
- `PM_Accelerate` on a ladder (`pm_flags` 0x20) is Q2's: the velocity
  moves toward `wishdir * wishspeed` by `accel * frametime * wishspeed` at
  most, with no floor. INFERRED. 1.1's ladder accelerates along the wish
  by a rate floored at 100 (`pmove.rs`, the ladder accelerate).
- The cmd scales (0x80e1a02, 0x80e1b1e, 0x80e1c58) are `ps.speed * max /
  (127 * len)` (rodata 0x8154f70, 0x8154f80, 0x8154f90: 127). VERIFIED.
  Then:
  - `pm_flags` 0x100 (the sighted walk) or a non-zero `leanf`: x0.4
    (rodata 0x8154f74/84/94). VERIFIED, the value. One constant now serves
    1.1's walk and lean scales. INFERRED.
  - Noclip x3, UFO x6 (rodata 0x8154f98, 0x8154f9c), spectator x
    `player_spectateSpeedScale` (2.0). VERIFIED.
  - `ClientEndFrame` writes `ps.speed` from `g_speed` (190) and
    `ps.gravity` from `(int)g_gravity` for everyone (0x80f7516, a `fldcw |
    0xc00` truncation). VERIFIED. The spectator's own think (0x80f4eb2,
    `ClientThink_real`'s `sessionState` 2 arm; name INFERRED) stores
    `ps.speed` 400, or 0 when `G_ClientCanSpectateTeam` refuses, right
    before its `Pmove`, under `pm.tracemask` 0x800811. VERIFIED, the
    immediates. So a free spectator's cmds scale off 400, twice that with
    the stock dvar. INFERRED.
  - The spectator's rise is the lean bits. The fly move (0x80e225e) adds
    `127 * scale(0, 0, 127)` to the wish's z under `buttons` 0x80, takes
    it away under 0x40, and accelerates at 8 (immediate 0x41000000).
    VERIFIED, the masks and immediates.
  - A living player's `pm.tracemask` is 0x2810011, a dead one's 0x810011
    (0x80f5d2e, `ClientThink_real` by its calls; its `pm_type < 6` test).
    VERIFIED, the immediates.
  - Walk scale only: back keys x `player_backSpeedScale` (0.7), strafe
    keys x `player_strafeSpeedScale` (0.8), dvars where 1.1 read the
    playerstate. VERIFIED.
  - Walk scale only: prone x0.15 (rodata 0x8154fa0), crouch x0.65
    (0x8154fa4), the stance read off `viewHeightTarget` by
    `PM_GetEffectiveStance` (40 crouch, 11 prone; immediates at 0x80e153b,
    0x80e1550). VERIFIED, the constants. While the eye runs between 40 and
    11 the scale blends the two by the lerp's fraction (0x80e3dae).
    INFERRED.
  - Walk scale only: the held weapon's `moveSpeedScale` (`WeaponDef`
    +0x264, STABS) when positive, and `buttons` 0x800 (shellshock) x0.4.
    VERIFIED.
  - Retail `moveSpeedScale` values (iw_13.iwd, `weapons/mp/*`): 0.9 bar,
    bren, mp44; 1.0 the rifles, shotgun, snipers, rockets, frags; 1.05 the
    SMGs; 1.1 m1carbine and the pistols; 1.2 the smoke grenades. No
    ADS-move or stance-move key exists in the weapon files. VERIFIED.
  - No sprint: no `sprint`, `fatigue` or `stamina` string in the server or
    the Mac symbols. VERIFIED.
- `PM_WalkMove` (0x80e25a6). VERIFIED, the constants; INFERRED, the
  order:
  - the jump slowdown and `Jump_Check` first (a jump hands over to the air
    move), then friction;
  - the walk scale times `PM_DamageScale_Walk` (`1 - damageTimer *
    minScale / maxTime`, 1 with the stock `player_dmgtimer_minScale` 0);
    `damageTimer` counts down by `frametime * 1000` (rodata 0x8154fb4);
  - both axes are clipped to the ground plane before the wish is built;
  - accel 19 prone, 12 crouch, 9 standing (immediates 0x80e2842,
    0x80e2858, 0x80e2865), 1 on slick ground or under `pm_flags` 0x400,
    and x0.25 (rodata 0x8154fb8) under 0x200.

  Slick ground and 0x400 also pull gravity on the ground; the speed is
  kept across the ground clip, and the step slide runs only when the flat
  velocity is non-zero. INFERRED. These are 1.1's numbers (`PM_ACCELERATE`
  and kin), with 1.1's 0x100 and 0x200 timers now 0x200 and 0x400
  (section 12). INFERRED.
- `PM_AirMove` (0x80e23f8): friction, the flat 2-axis scale, accel 1
  (immediate 0x3f800000), the ground-plane clip, the step slide with
  gravity, `PM_SetMovementDir`. As 1.1. INFERRED.
- The dead move (0x80e2a22) drops 20 a frame (rodata 0x8154fbc), as 1.1
  (`DEAD_SPEED_DROP`). VERIFIED.

## 6. Jump

- `Jump_Check` (0x80dcca8): the 0x400 edge, 500 ms since `jumpTime`,
  `pm_flags` 0x1000 and 0x4 clear, `pm_type` under 6, standing. A held key
  takes 0x400 off the cmd. INFERRED. The 500 is an immediate. VERIFIED.
- The takeoff (0x80dca40): `vz^2 = 2 * gravity * jump_height` (39),
  divided by the 1.5 curve while `pm_flags` 0x80000 stands with `pm_time`
  at most 1800; `jumpOriginZ` = origin z (1.5's meaning), `pm_flags |=
  0x80000`, `pm_time` 0, `groundEntityNum` 1023, `aimSpreadScale +=
  jump_spreadAdd` (64) capped at 255 (rodata 0x8153340). INFERRED, the
  shape; VERIFIED, the constants.
- The penalty is 1.5's on a new bit. `Jump_ApplySlowdown` (0x80dc818), on
  the walk with 0x80000 set: `pm_time` over 1800 clears the jump and
  scales the velocity by 0.65; at `pm_time` 0 a landing under
  `jumpOriginZ + 18` (rodata 0x8153320) starts 1800 ms at 0.65, a higher
  one 1200 ms at 0.5 (immediates 0x708, 0x4b0, 0x3f266666, 0x3f000000).
  VERIFIED. The friction curve and the jump curve are `1 + t * 1.5 / 1700`
  under 1700 ms, else 2.5 (rodata 0x8153324-0x815333c). VERIFIED.
  `jump_slowdownEnable` 0 makes every scale 1 and keeps the timers.
  INFERRED. vcod's `JUMP_PENALTY_*` constants carry the same numbers.
- `Jump_ActivateSlowdown` (0x80dc7ee), from `PM_CheckDuck`'s prone entry,
  starts 1800 ms only when `pm_time` is 0. INFERRED.
- The jump never rises past its height (new). `PM_StepSlideMove`'s
  airborne step under 0x80000 climbs at most `min(jump_stepSize 18,
  jumpOriginZ + jump_height - start z)` (`Jump_GetStepHeight` 0x80dc73a).
  A step ending at or over `jumpOriginZ + jump_height` is reverted
  (`Jump_IsPlayerAboveMax` 0x80dc7c8). `Jump_ClampVelocity` (0x80dc95c)
  holds a rising vz to `sqrt(2 * g * remaining)`, zero within 0.1 (rodata
  0x8153330). INFERRED. 1.5 had the step's reach; the speed clamp is
  CoD2's.
- Off a ladder: vz x0.75 (rodata 0x8153344), the flat forward reflected
  off `vLadderVec` when facing it (-2, rodata 0x8153348), scaled to
  `jump_ladderPushVel` (128), `pm_flags` 0x20 cleared. INFERRED, the
  shape; VERIFIED, the constants. 1.1's push-off is 128 too
  (`LADDER_PUSHOFF_SPEED`).
- Events: the jump raises 70 + surface (91, `EV_JUMP_WOOD`, on a ladder),
  and anim event 4 when backing, else 3. INFERRED.

## 7. Ground, slide and step

- `PM_GroundTrace` (0x80e38a2), as 1.1. VERIFIED, the constants;
  INFERRED, the shape:
  - origin +0.25 to -0.25 (rodata 0x8155050), or 0 to -1 on a turret;
  - an all-solid start retries 0.001 down (rodata 0x8155054) and, still
    solid, leaves the player airborne;
  - a miss runs the missed-ground arm (0x80e36aa);
  - a rising body off the ladder whose `velocity . normal` exceeds 10
    (rodata 0x8155058) leaves the ground (Q3's kick-off);
  - a normal under 0.7 (rodata 0x815505c) is a steep plane that clears the
    jump;
  - a landing from the air runs `PM_CrashLand`.
- `PM_ClipVelocity` (0x80e14d4) is not Q3's: `backoff = dot - |dot| *
  0.001` (rodata 0x8154ee0, `fabs` at 0x80e9576), `out = in - normal *
  backoff`. VERIFIED. Into a plane that is Q3's `dot * 1.001` up to
  rounding; out of one it is `dot * 0.999` where Q3 divides by 1.001.
  INFERRED. vcod-common's `clip_velocity` and its `OVERCLIP` 1.001 are
  1.1's; `vcod_cod2::pmove` has its own.
- `PM_playerTrace` (0x80e1360): a trace that starts in a body
  (`startsolid`, `contents & 0x2000000`) touches it, drops `CONTENTS_BODY`
  from `pm->tracemask` and traces again. VERIFIED. 1.5's and UO's rule
  (`movetrace::Pm`).
- `PM_SlideMove` (0x80e9c42), as 1.1: four bumps, gravity averaged over
  the frame with the end velocity stored (rodata 0x8155318, 0.5), the
  ground plane and the normalized velocity as the first planes,
  `allsolid` kills vz, more than 7 planes clears the velocity, a running
  `pm_time` hands back the entry velocity. INFERRED. What changed:
  - the origin after a hit is `Vec3Lerp(origin, end, fraction)` (no
    `endpos`, section 3). VERIFIED, the call.
  - a plane within 0.999 (rodata 0x815531c) of one already met clips the
    velocity against it, then adds the normal; 1.1 adds the normal only.
    INFERRED.
  - a new plane goes into `PM_PermuteRestrictiveClipPlanes` (0x80e9b7c),
    an insertion sort of the planes by `velocity . plane`, stored as
    float32, ascending, which returns the smallest. Under 0.1 (rodata
    0x8155320) the velocity is clipped against the most restrictive plane
    first, then against each later plane in that order that it still
    meets under 0.1, with the crease and the three-plane stop as Q3's.
    1.1 clips against the first plane, in hit order, that it meets under
    0.1. INFERRED.
  - `pml.impactSpeed` keeps the largest `-dot`. INFERRED.
- `PM_StepSlideMove` (0x80ea3f4), 1.5's shape. The step is 18, or 10 with
  `pm_flags` 1 (immediates at 0x80ea4fe and 0x80ea4f1). VERIFIED. The
  rest INFERRED:
  - a grounded frame always takes the down pass; an airborne one only on
    the jump's step or climbing a ladder;
  - the step-up is `(step + 1) * fraction - 1`, nothing under 1;
  - the down pass reaches the step plus 9 (rodata 0x8155324) on a ground
    plane;
  - a down pass that meets a client (entity under 64) keeps the plain
    slide;
  - the revert compares the two flat displacements along the velocity
    with 0.001 slack (rodata 0x815532c);
  - new: a down pass that lands on a normal under 0.3 (rodata 0x8155328)
    keeps the plain slide.

  1.1d's step reads no 0.3: its float loads at 0x34fbc-0x357a0 are 10, 18,
  -0.001, 0.001, 0.5, 1.001, 0.1, 0, 0.8 and 0.19999999. VERIFIED.
- The step's tail. Over half a unit (rodata 0x8155330) of eye movement:
  `EV_STEP_VIEW` (143) with the rounded step clamped to -16..24 plus 128,
  the velocity times `0.19999999 + 0.8 * (1 - |dz| / step)` (rodata
  0x8155338 = 0x3e4ccccc, 0x8155334), and a footstep when the step is over
  3 on the ground (`bobCycle += 7 + 1.25 * min(|step| / 2, 4)`, rodata
  0x8155340, 0x815533c). VERIFIED, the constants. As 1.1, whose base is
  the same 0x3e4ccccc (1.1d rodata 0x70f14, `fadds` at 0x35776). VERIFIED.
  vcod's `STEP_SCALE_BASE` is 0x3e4ccccc in both `pmove.rs` and
  `vcod_cod2::pmove::slide`. Trap: `0.2f32` is 0x3e4ccccd, one bit above.

## 8. Fall damage and landing

`PM_CrashLand` (0x80e30c4) is 1.1's (`pmove::crash_land`) with the water
level gone. VERIFIED, the constants; INFERRED, the shape:

- the impact speed solved off `previous_velocity` and the fall, squared
  back over `2g` (rodata 0x8155024 0.5, 0x8155028 4);
- damage 0 at or under `bg_fallDamageMinHeight` (256), on `SURF_NODAMAGE`
  (`surfaceFlags & 1`) or dead, 100 at or over `bg_fallDamageMaxHeight`
  (480), linear and truncated between (rodata 0x815502c 100); bounds out
  of order print `bg_fallDamageMaxHeight must be greater than
  bg_fallDamageMinHeight` and take none;
- the land bob `4 + 4 * (h - 12) / 26`, at most 24 (rodata 0x8155030,
  0x8155034), with anim event 5;
- no damage: a walk step event under 8 (rodata 0x8155044), a run step
  under 12, else the land event and the velocity x0.67 (immediate
  0x3f2b851f);
- damage under 100 off slick ground: `pm_time = min(35 * damage + 500,
  2000)`, `pm_flags |= 0x200`, the velocity x0.5 at 500 ms down to x0.2
  at 1500 ms and past (rodata 0x8155038, 0x815503c, 0x8155040; immediates
  0x3f000000, 0x3e4ccccd); otherwise x0.67; then `EV_LANDING_PAIN` +
  surface with the damage as parm.

1.1's halving at water level 2 and its no-landing under water are gone.
INFERRED.

## 9. Mantle

New in CoD2 and in `bg_`, so the client predicts it. VERIFIED: the Mac
client calls `Mantle_CreateAnims` (Mac 0x9e84c) and its `PM_Weapon` calls
`Mantle_IsWeaponInactive`. The Linux functions carry no symbols; each is
paired with the Mac name by its strings, dvars and callees. INFERRED.

| Function | `cod2_lnxded` | Mac |
|---|---|---|
| `Mantle_Check` | 0x80de088 | 0x9f6c6 |
| the surface probe | 0x80dddfe | inlined |
| `Mantle_CheckLedge` | 0x80dd65a | 0x9ef56 |
| the over probe / the start | 0x80dd3dc / 0x80dd552 | inlined |
| the nearest row | 0x80dd35c | inlined |
| `Mantle_GetAnimDelta` | 0x80dd216 | 0x9eb68 |
| `Mantle_Move` | 0x80de260 | 0x9ecdc |
| `Mantle_CapView` | 0x80de47a | 0x9e758 |
| `Mantle_ClearHint` / `Mantle_IsWeaponInactive` | 0x80de5ba / 0x80de5d4 | 0x9e748 / 0x9e814 |
| `Mantle_CreateAnims` | 0x80ddaa4 | 0x9e84c |
| `XAnimGetAbsDelta` / `XAnimGetLengthMsec` | 0x80c177c / 0x80c0d6c | |

**The dvars.** `mantle_enable` 1, `mantle_debug` 0, `mantle_check_range`
20, `mantle_check_radius` 0.1 (0x3dcccccd), `mantle_check_angle` 60,
`mantle_view_yawcap` 60, all flag 0x1180. VERIFIED
(`Mantle_RegisterDvars` 0x80dcf8c, the immediates).

**The state.** `mantleState` (ps 0x5bc) is {yaw, timer, transIndex,
flags}. The flags are VERIFIED masks; the names are INFERRED from what
reads them:

| Flag | Name | Meaning |
|---|---|---|
| 1 | over | the wall is `MANTLE_OVER` |
| 2 | crouch | the standing box does not fit at the ledge |
| 4 | stand | it fits where the mantle ends |
| 8 | hint | a mantle is available |

`PmoveSingle` clears 8 before its `pm_type` switch every frame
(0x80de5ba). VERIFIED, the call.

**The call order** (`PmoveSingle` 0x80e8934, the default arm). VERIFIED,
the calls:

- off a mantle, `PM_CheckDuck` and `PM_GroundTrace`, then `Mantle_Check`
  either way;
- then, while `pm_flags` 4 is set (a mantle under way or started this
  frame): `0x80e8026` (off the ladder), ground 1023, `pml.walking` and
  `pml.groundPlane` 0, the sight flag, the sight walk, `PM_CheckDuck`,
  `Mantle_Move` and `PM_Weapon`, and nothing else;
- no timers, no footsteps, no foliage and no tail, so the velocity is not
  `rint`-snapped and `oldVelocity` holds;
- `PM_UpdateViewAngles` (0x80e6eb8) calls `Mantle_CapView` instead of the
  ladder cap, the prone arm and the lean while `pm_flags` 4 is set.
  VERIFIED, the branch.

**`Mantle_Check`** (0x80de088) clears 8, then refuses on `mantle_enable`
0, `pm_type` over 5, `pm_flags` 4, `eFlags` 0xc or `weaponstate` 17..22,
each with its `Mantle Failed:` string. VERIFIED, the tests and strings.

- The surface (0x80dddfe). The box (-r, -r, `mins.z`)..(r, r, `maxs.z`),
  r `mantle_check_radius`, goes from the origin `maxs.x - r` back along
  the view's flattened, normalized forward to `mantle_check_range +
  maxs.x - r` ahead (the sum passed as a float), under contents
  0x1000000. It fails when it starts solid, at fraction 1, with no surface
  bit of 0x6000000 (`MANTLE_ON` 0x2000000, `MANTLE_OVER` 0x4000000), on a
  negated normal whose flat length (`Vec3Normalize`) is under 1e-4 (rodata
  0x8153814), or when `acos` of the stored dot of the forward and that
  normal, times 180 / pi (the double at rodata 0x8153818), passes
  `mantle_check_angle`. VERIFIED, the calls and constants.
- `MANTLE_OVER` on the struck side sets flag 1 in the results, which start
  at the origin with that normal. VERIFIED.
- `Mantle_CheckLedge` (0x80dd65a) runs at 60, 40 and 20 up (immediates)
  until one returns true. With r = `maxs.x`, a box (-r, -r, 0)..(r, r, 2r)
  goes from the start raised by the height 16 along the normal (it must
  reach clear), then from there down to the start's z + 18 (rodata
  0x81535a4; it fails starting solid or at fraction 1). A floor normal z
  under 0.7 (rodata 0x81535a8) fails. The ledge is the down trace's lerp,
  and a box topped at 50 must fit there. Then the hint (8) goes on the
  playerstate and the results. Without `buttons` 0x400 it stops there: the
  key is read held, not on its edge. VERIFIED, the calls, immediates and
  tests.
- Every mantle trace calls `pm->handler`'s capsule trace directly
  (0x80e1308), not `PM_playerTrace`, under `pm->tracemask`. VERIFIED.
- The over probe (0x80dd3dc). Under flag 1 the player's box topped at 50
  is traced from 31 past the ledge along the normal (immediate
  0x41f80000) to 18 below (rodata 0x815348c). Starting solid or stopping
  short clears flag 1 and the mantle ends on the ledge; else it ends where
  the trace does. Without flag 1 it ends on the ledge. VERIFIED.
- Unless `eFlags` 4 is set, the player's box starting solid on the ledge
  sets 2, and not starting solid at the end sets 4. VERIFIED.
- The start (0x80dd552). VERIFIED:
  - `yaw` is `vectoyaw` of the normal, `timer` 0;
  - `transIndex` is the row of `.data` 0x8184d00 whose height is nearest
    the ledge's rise over the start (the rise stored as a float, the first
    row on a tie: 0x80dd35c, `ja`);
  - the origin is the end less the anims' whole offset;
  - then `pm_flags` 4, `eFlags` 0x4000, and `pmove_t`'s `mantleEndPos`,
    `mantleDuration` and `mantleStarted`.

  The stored rise decides ties: `mantle` starts at z 28.125002 under a
  ledge at 76.125, the rise rounds to 48, between the 51 and 45 rows, and
  retail takes 51. VERIFIED, the capture.

**The anims.** The rows at `.data` 0x8184d00 are {up anim, over anim,
height}: (1, 8, 57), (2, 8, 51), (3, 9, 45), (4, 9, 39), (5, 9, 33), (6,
10, 27), (7, 10, 21), over the names at 0x8184d60: `mp_mantle_root`,
`mp_mantle_up_57` .. `_21`, `mp_mantle_over_high`, `mp_mantle_over_mid`,
`player_mantle_over_low`. VERIFIED. `Mantle_CreateAnims` errors at load
unless every up anim's root ends at (16, 0, height) and every over anim's
at (31, 0, -18), within 1. VERIFIED (strings, rodata
0x8153678..0x8153698). The stock anims (xanim v14 with a root track, in
`iw_13.iwd`) pass. VERIFIED (`mantle::tests`).

- `XAnimGetLengthMsec` (0x80c0d6c): `numframes / framerate` times 1000.0,
  truncated. That is `vcod_common::xanim`'s `length_ms`, 833 for
  `mp_mantle_up_57`. VERIFIED.
- `XAnimGetAbsDelta`'s translation (0x80c177c, 0x80bdc5a): the last key at
  time 1 or for a one-key track. Else the frame position `t * numframes`
  stored, its whole part truncated (`fistp` under 0xc00, 0x80bd0a6), the
  key pair around it (indexed directly when the track has a key a frame,
  else found in its frame list: 0x80bd4c4, 0x80bd876), `frac` stored and
  `a + (b - a) * frac` stored (0x80c36fe). VERIFIED, the disassembly.
- `Mantle_GetAnimDelta` (0x80dd216) at `t` ms: within the up anim, its
  offset at `t / up_len` (a float division); past it, the up anim's whole
  offset plus the over anim's at `(t - up_len) / over_len`. The over anim
  counts only under flag 1. Then `VectorAngleMultiply` (0x80a7670): the
  yaw times pi / 180 (a double) stored, libm `sin` and `cos` of it stored,
  `x' = x cos - y sin`, `y' = y cos + x sin`. VERIFIED, the disassembly.

**`Mantle_Move`** (0x80de260). VERIFIED, the calls and masks:

- clears 8, raises `EV_STANCE_FORCE_CROUCH` (141) every frame under flag
  2;
- steps `timer` by `msec` up to the up and over lengths, moves the origin
  by the offset between the old and new times, and sets the velocity to
  that offset times `1 / (0.001 * (new - old))` (rodata 0x8153928, the
  factor passed as a float);
- sets the torso and legs anim (`BG_AnimScriptAnimation`, the up or over
  slot plus 20);
- at the end clears `pm_flags` 4 and `mantleStarted`; under flag 1 runs
  `BG_AnimScriptEvent`; under flag 4 raises `EV_STANCE_FORCE_STAND` (140)
  and clears `eFlags` 0x4000.

Without flag 4, `eFlags` keeps 0x4000 after the mantle: `mantle_crouch`
(17.1) ends with `eFlags` 0x4002. VERIFIED.

**The stance the client keeps.** `CG_EntityEvent` (Mac 0x1e4650), for an
event on its own client's entity, sets `cl_stance` (`legacyHacks` +8) to
0 on `EV_STANCE_FORCE_STAND` (0x8c), 1 on `EV_STANCE_FORCE_CROUCH` (0x8d)
and 2 on `EV_STANCE_FORCE_PRONE` (0x8e), each only while `legacyHacks`
+0xc is 0. For anyone else's it prints `Event %s just for client %i was
sent to other clients`. VERIFIED, the decompile's three cases. With
`cl_stance` 1 every later cmd carries the crouch bit 0x200
(cod2-protocol.md, `CL_KeyMove`), so a player a mantle forced to crouch
stays crouched until he stands up himself. INFERRED. vcod's bots do the
same and stand once a standing box fits on the ground
(`Cod2Bots::forced_crouch`, bot-objectives.md 11.4).

**`Mantle_CapView`** (0x80de47a): when `AngleDelta(yaw, viewangles[1])`
is outside the cap, it is wrapped into the cap by whole caps,
`delta_angles[1]` grows by its shorts (182.04445, truncated, masked), and
the view yaw becomes `AngleNormalize360Accurate(yaw - cap)` for a positive
difference, `yaw + cap` otherwise. VERIFIED, the decompile. `mantle_turn`
turns the view 90 off the wall during the climb, and retail holds it at
59.99 (`delta_angles[1]` 0xeaab). VERIFIED, the capture.

**The weapon.** `Mantle_IsWeaponInactive` (0x80de5d4) holds under
`mantle_enable` and `pm_flags` 4 when the row's over anim is not 10, so
the two low rows (27, 21) keep the gun up and the rest put it away
(`cod2-combat.md` 1.7). VERIFIED, the test and the captures.

**The block.** lnxded `ClientThink_real`, after `Pmove`: when `pmove_t`'s
`mantleStarted` is set it calls 0x80f5c34 with `mantleEndPos` and
`mantleDuration`. That `G_Spawn`s an entity with `parent` the player and
`r.ownerNum` his number, contents and clipmask 0x10000, `svFlags` 0x21,
`eType` 5, the player's `r.mins`/`r.maxs`, set at the end position and
linked. Its think (handler 0x13, `G_FreeEntity` 0x811ee50) is due
`level.time + mantleDuration + g_mantleBlockTimeBuffer`. VERIFIED,
`Dvar_RegisterInt("g_mantleBlockTimeBuffer", 500, 0, 60000, 0x1080)`.

- Its role is a body box at the ledge that keeps other players out of the
  end spot while the climb runs. INFERRED.
- A living player's `pm.tracemask` is 0x2810011, a dead one's 0x810011
  (lnxded `ClientThink_real` 0x80f5d2e), so every living move meets the
  block's 0x10000. `SV_Trace` (0x809d884) passes an entity whose
  `r.ownerNum` is the mover, so the climber goes through his own.
  VERIFIED.
- `SV_ClipHandleForEntity` (0x809c544) gives a non-brush entity
  `CM_TempBoxModel` of its box, whose sides are all axial and so meet the
  mover's capsule as its box. INFERRED, the shape.
- `svFlags` 0x21 carries Q3's `SVF_NOCLIENT` bit, so it is never sent.
  INFERRED.

vcod-server spawns and frees it on that clock (`Cod2Level::
spawn_mantle_block`, `run_mantle_blocks`), so later entity numbers match,
and hands each cmd's pmove the blocks as `movetrace::ClipBox`es
(`MoveWorld::with_clip_boxes`), the climber's own passed. No capture has a
second player walk into one. The bots' walks and the client's prediction
do not see it. `vcod_cod2::pmove::mantle::end_and_duration` gives the
duration, the two anims' whole length. INFERRED.

**The hint.** Mac `CG_DrawMantleHint` 0x18ae44: the HUD's owner draw 80
draws only under `cg_drawMantleHint` (`Dvar_RegisterBool`, 1, flags
0x1001, in `CG_Init` 0x145214) and with `mantleState.flags & 8`; the
layout is cod2-hud.md 9's. VERIFIED. vcod's client registers the dvar
archived and the HUD reads it.

**vcod** (`vcod_cod2::pmove::mantle`) ports all of the above except the
`pmove_t` outputs (no caller reads them) and `mantle_debug`. The torso and
legs anim and the anim script event go through `pmove::anim`
(`cod2-player-anims.md` 9). The anims load once per process
(`mantle::anims`) into `Rules::mantle_anims`; without them no mantle
starts. `mantle::weapon_inactive` takes `mantle_enable` as on.
`examples/mantle_spots.rs` finds capture spots: it sets a body down in
front of every flagged axial brush face of a map and runs the `mantle`
script's cmds through the pmove.

## 10. Stance, eye, prone, lean, view

- `pm_flags` 1 prone and 2 crouch, as 1.1. The keys are `buttons` 0x100
  and 0x200. `eFlags` 0x8 prone, 0x4 crouch (1.1: 0x40, 0x20). VERIFIED,
  the immediates in `PM_CheckDuck` 0x80e4644. A gunner's stance is
  `eFlags` 0x100 prone, 0x200 crouch, 0x300 standing (1.1: 0x4000, 0x8000,
  0xc000). INFERRED.
- The box: maxs z 30 prone and 50 crouched (immediates 0x80e49c7,
  0x80e4ac9), `ps.maxs` standing; the stock bounds (-15,-15,0)..(15,15,70)
  at `.data` 0x8187ba0 and 0x8187bac. VERIFIED. A spectator's box is
  (-8,-8,-8)..(8,8,16) with its eye at 0. INFERRED.
- The eye targets 60, 40, 11, and 8 dead (immediates), one stance at a
  time while no lerp runs, and the stance follows the eye's target, as
  1.5. INFERRED. `PM_GetViewHeightLerpTime` (0x80e3c46): 400 into prone and
  up to the crouch, 200 down to the crouch and to standing (1.1: 150 down
  to the crouch). VERIFIED. The curve tables at `.data` 0x8186780,
  0x8186800, 0x8186880, 0x8186960 match 1.1's and 1.5's byte for byte;
  there is no dive curve. VERIFIED. The dead eye moves at 180/s (rodata
  0x8155060). VERIFIED.
- No prone dive: a prone press in the air is dropped. A refused press on
  the ground sets `pm_flags` 0x10000 (1.1: 0x8000) and raises 140 or 141;
  prone entry starts the jump slowdown (section 6). INFERRED. Rising
  traces mask bodies out (`tracemask & 0xfdffffff`). VERIFIED.
- The prone body is longer. VERIFIED, the constants:
  - `PM_CheckDuck`, `PM_UpdatePronePitch` and `PM_VerifyPronePosition`
    pass 66 (immediate 0x42840000 at 0x80e4953; rodata 0x815511c); the
    swing tests 45 (rodata 0x8155118); `BG_CheckProneTurned` blends `66 *
    (1 - f) + 45 * f`, `f = |dyaw| / 240` (rodata 0x8155114). 1.1 uses 54
    (`PRONE_BODY_LENGTH`), 1.5 60.
  - `BG_CheckProneValid` (0x80e01f4) samples the hips 48 out (immediate
    0x42400000) with a sill limit `48 + 0.7 h` (rodata 0x8154e00).
    `vcod_cod2::pmove::stance` uses 48; vcod-common's CoD1 `pmove.rs` uses
    24 for both.
  - The yaw cap is `bg_prone_yawcap` (85) with the 5-degree soft edge
    (rodata 0x815512c) and 55 deg/s swing (0x8155130), the refused-turn
    pull 0.98 (0x815513c), the torso pitch ±45 at 70 deg/s (0x8155138,
    0x8155148), a slope steeper than 0.7 (0x8155144) forcing a crouch
    (141).
- `PM_UpdatePronePitch` (0x80e7c00) runs each prone frame after the sight
  flag and before the dead move. VERIFIED (the call at 0x80e925d, after
  the sight flag's at 0x80e9248).
  - Off the ground (`groundEntityNum` 1023) it asks `BG_CheckProneValid`
    with the body's heading, height 30, length 66, already prone, not on
    the ground, and the ground normal when `pml.groundPlane` is set, its
    three outputs into ps 0x5a8..0x5b0. A refusal raises 141 and sets
    `pm_flags` 0x10000. VERIFIED, the arguments and the stores.
  - On the ground a plane whose normal z is under 0.7 raises 141 and sets
    no flag. VERIFIED.
  - Then `proneDirectionPitch` (ps 0x588) eases toward
    `PitchForYawOnNormal(proneDirection, normal)` and `proneTorsoPitch`
    (ps 0x58c) toward the same along the view's yaw (ps 0xec), each 0
    without a ground plane: `AngleDelta(target, cur)`, a step of `70 *
    frametime` when the delta is larger, else the delta, through
    `AngleNormalize180Accurate`. VERIFIED. The compare and the step keep
    `70 * frametime` on the x87 stack. INFERRED, the `longdouble` product
    in the export.
  - vcod: `pmove::stance::update_prone_pitch`, tested in
    `the_prone_pitches_ease_onto_the_ground_at_70_a_second` and
    `a_prone_body_on_a_steep_plane_is_forced_to_crouch`. Trap: setting
    both pitches only on the first prone frame leaves the torso's ±45 view
    pitch cap at the slope the body lay down on.
- The pitch cap is `player_view_pitch_up` / `_down` (85) times 182.04445
  (rodata 0x8155124), truncated (`fldcw | 0xc00`, 0x80e6f87-0x80e6f92):
  15473 shorts where 1.1 caps at 16000. VERIFIED. The ladder yaw cap is
  `bg_ladder_yawcap` (100) about `vectoyaw(vLadderVec) + 180`. INFERRED.
  1.1's is 100 (`LADDER_YAWCAP`).
- `PM_UpdateLean` (0x80e69f4): the keys count with `pm_flags` 0x8000
  clear, `pm_type` under 6 and on the ground or linked; a gunner's are
  zeroed. INFERRED. The scale is 0.25 prone (immediate 0x80e6aa4) and 0.5
  otherwise (0x80e6ab1), standing included, as 1.5. The ramps are 350 out
  and 280 back (rodata 0x8155110, 0x815510c). `AddLeanToPosition` takes 16
  and 20 (immediates 0x80e6c95, 0x80e6c8c). The wall box is ±8 under
  0x2810011. VERIFIED.

## 11. Ladder

`PM_CheckLadderMove` (0x80e805a) is called after the dead move and before
`PM_DropTimers`; `pm_flags` 0x20 then picks `PM_LadderMove` (0x80e847e)
over the walk and the air move (0x80e8934's default arm). VERIFIED, the
call sites.

- The check. VERIFIED, the constants and offsets; INFERRED, the shape:
  - a walking frame clears `pm_flags` 0x40000; nothing more runs while
    `pm_time` is set and a 0x200 or 0x400 timer runs off the ladder;
  - the reach is 8 walking and 30 airborne (immediates 0x41000000,
    0x41f00000), along `-vLadderVec` (ps+0x64) when already on the ladder
    and off the ground, else along the flat view normalized;
  - a dead player (`pm_type` 6, 7) is put off the ground and off the
    ladder; 0x40000 set, `PM_GetEffectiveStance` 1 (prone) or a jump
    within 300 ms (immediate) leaves it;
  - the probe is the cmd's box shrunk 6 a side (rodata 0x815514c), its
    floor 8 and its top at least 8, traced under `pm->tracemask` through
    `PM_playerTrace`; a hit with `surfaceFlags & 8` that is airborne or
    has `forwardmove > 0` keeps a held ladder at once;
  - a fresh grab copies the trace's normal into `vLadderVec` and probes
    again along its negation, and takes the ladder only if that meets
    ladder too;
  - anything else leaves it: 0x20 cleared, 0x40000 set if it was held
    (0x80e8026).
- The move. VERIFIED, the constants and calls:
  - `Jump_Check` first; a jump runs `PM_AirMove` and nothing else;
  - the climb `up = (0.25 + forward.z) * 2.5` (rodata 0x8155150,
    0x8155154) clamped to ±1;
  - `forward` and `right` are flattened and normalized, then `right` is
    laid on the ladder's plane (`ProjectPointOnPlane` 0x80aa0e4, the dot
    cast to a float);
  - the wish is `fm * 0.5 * up * scale` up (rodata 0x815515c) plus `rm *
    0.2 * scale` along that right (0x8155160), `scale` the 2-axis cmd
    scale (0x80e1b1e), into `PM_Accelerate` at 9 (immediate).
- `PM_Accelerate` with 0x20 set is Q2's: `push = wishdir * wishspeed -
  velocity` normalized (length kept), `canpush = accel * frametime *
  wishspeed`, the velocity moved `min(canpush, |push|)` along it; no
  `stopspeed` floor and no inertia clamp. INFERRED, the shape off
  0x80e188e.
- The bleeds. VERIFIED, the constants; INFERRED, the shape:
  - no forward key: vz bleeds toward 0 by `gravity * frametime`, never
    past it;
  - no strafe key: the velocity along the flat right (a `Vec2Normalize`d
    copy) is taken away and given back less `16 * along * frametime`
    (rodata 0x8155164), that drop at least a unit (`1 - 2 * signbit`,
    0x80e9646), all of it gone once the drop exceeds it;
  - airborne: the flat velocity into `vLadderVec` is taken away, and when
    the flat speed squared (stored) is no more than vz squared (kept), 50
    more pushes into it (immediate 0xc2480000);
  - then `PM_StepSlideMove` without gravity, and `movementDir` is
    `AngleDelta(vectoyaw(vLadderVec) + 180, yaw)` truncated, capped at 75
    (immediate 0x4b).

  1.1's ladder press is 500 or 250 every airborne frame (`pmove.rs`);
  CoD2's is the 50, and only on a mostly vertical velocity.
- The push-off (0x80dcb48), from `Jump_Check` after the takeoff and the
  ladder's jump event (91): vz x0.75 (rodata 0x8153344); the flat forward,
  normalized, reflected off `vLadderVec` (`f - 2 (f . n) n`, rodata
  0x8153348, normalized again) when `n . forward` is negative; the flat
  velocity that times `jump_ladderPushVel` (128); `pm_flags` 0x20 cleared
  without 0x40000. VERIFIED, the constants; INFERRED, the shape.
- `PM_UpdateViewAngles` (0x80e6eb8), not a gunner, not mantling: on the
  ladder off the ground with `bg_ladder_yawcap` (100, `BG_RegisterDvars`
  0x80de830) non-zero, a view more than the cap off
  `vectoyaw(vLadderVec) + 180` (rodata 0x8155128) pushes the excess into
  `delta_angles[1]` and puts the yaw at the cap
  (`AngleNormalize360Accurate`). VERIFIED, the constants; INFERRED, the
  shape.
- `PM_Footsteps` (0x80e572c). Airborne on the ladder, nothing within 300
  ms of a jump; else the bob cycle runs at `0.45 * vz / 95.25`, sighted or
  leaning `0.35 * vz / (0.4 * 95.25)` (rodata 0x81550d0, 0x81550cc,
  0x81550c8), truncated and masked to a byte, and each quarter crossed is a
  rung footstep. `PM_FootstepEvent` (0x80e5402) off the ground makes one
  only on the ladder: the probe box 31 into `vLadderVec` (immediate
  0xc1f80000) under the trace mask less `CONTENTS_BODY` and
  `CONTENTS_PLAYERCLIP` (`& 0xfdfeffff`), the struck material (bits
  20-24) plus 1, material 21 (immediate 0x15) on a miss or material 0.
  VERIFIED, the constants; INFERRED, the shape.
- mp_toujane's ladders are player-clip brushes (contents 0x28010000,
  material `ladder`, surface flags 0x440b8), so the footstep's probe
  passes through them to the wall behind: the `ladder` capture's rung
  steps are event 17 (material 16). VERIFIED, the brush and its material
  off the `.d3dbsp`, the events off the capture.
- vcod: `vcod_cod2::pmove`'s `ladder.rs`. The `ladder` capture (17.1)
  replays it, with the weapon half, in every row of both runs. VERIFIED
  (`moves_ab`). The gun goes down on the rungs and comes back off them
  (`PM_Weapon`'s, `cod2-combat.md` 1.7). `game::cod2_ladder`
  (`crates/server/src/game/cod2_ladder.rs`) replays the capture's cmds
  through `Cod2Level::client_think` and fails on the first snapshot that
  holds the ladder without the check. VERIFIED. Trap: without the check
  vcod-server never sets 0x20 and a client on it cannot climb.

## 12. Flags and events

`pm_flags` (ps+0xc). Meanings are INFERRED from their readers and writers;
the 1.1 column is vcod's `net/cod1/flags.rs` and `pmove.rs`.

| Bit | CoD2 | 1.1 |
|---|---|---|
| 0x1, 0x2 | prone, crouch | same |
| 0x4 | mantling | the prone dive |
| 0x10 | offhand in use | ladder |
| 0x20 | ladder | |
| 0x40 | sighted (`PM_ExitAimDownSight` clears it) | backwards run |
| 0x80 | backwards run | |
| 0x100 | sighted walk (scale 0.4) | the 0x100 timer (landing stun) |
| 0x200 | landing stun timer (friction 0.3, accel 0.25) | damage knockback timer |
| 0x400 | knockback timer (no friction, accel 1, gravity on the ground) | |
| 0x800 | sighted while prone (move keys zeroed) | respawned |
| 0x1000 | fire latch after a spawn | |
| 0x2000 | | 1.5's jump penalty |
| 0x8000 | frozen | prone refused |
| 0x10000 | prone refused this cmd | |
| 0x20000 | binoculars | |
| 0x40000 | just left a ladder | |
| 0x80000 | jump slowdown timer | |

`PM_DropTimers` (0x80e6936) clears 0x200, 0x400 and 0x80000 when
`pm_time` runs out (mask 0xfff7f9ff). VERIFIED. `eFlags`: 0x4 crouch, 0x8
prone, 0x40 firing, 0x4000 mantling, 0x200000 talk, 0x300 the gunner's
stance. INFERRED.

Events: the name table at `.data` 0x8184da0 holds 199 names (strings at
rodata 0x8153940 on). VERIFIED. The movement block is 1.1's numbering:
footsteps run 1, walk 24, prone 47 (23 surfaces each), jump 70, landing
93, landing pain 116, `EV_FOLIAGE_SOUND` 139,
`EV_STANCE_FORCE_STAND/CROUCH/PRONE` 140-142, `EV_STEP_VIEW` 143.
VERIFIED. The tables part from 144 (`EV_ITEM_PICKUP`, 1.1's 146): CoD2 has
no water touch, water leave or quiet pickup. VERIFIED.

`PM_FoliageSounds` (lnxded 0x80e6784, inlined in the Mac's `PmoveSingle`
at 0x72260) runs after `PM_Weapon` in the default arm. VERIFIED (the call
at 0x80e9345, after `PM_Weapon`'s at 0x80e933a).

- With `pm->xyspeed` (pmove_t +0xdc) under `bg_foliagesnd_minspeed` (40)
  it zeroes `foliageSoundTime` (ps 0x44) once
  `bg_foliagesnd_resetinterval` (500) has passed since it. VERIFIED.
- At 40 or more the interval is `slow + (fast - slow) * min((xyspeed -
  40) / (180 - 40), 1)`, `bg_foliagesnd_slowinterval` 1500 and
  `_fastinterval` 500, truncated to an int. VERIFIED (the `fistpl` under
  `fnstcw | 0xc00` at 0x80e6843..0x80e6855; the Mac's `(int)` cast).
- Once `foliageSoundTime + interval` is before the cmd's time it
  position-tests the pmove box scaled by 0.75 (immediate 0x3f400000), its
  top by 0.9 (rodata 0x8155108), at the origin under mask 2 (foliage)
  through `PM_playerTrace`. A start in solid raises `EV_FOLIAGE_SOUND`
  (139) and sets the time. VERIFIED.
- Stock maps carry foliage brushes on 13 of 15 (mp_brecourt 666, none on
  mp_downtown and mp_leningrad). VERIFIED
  (`foliage_brushes_answer_the_foliage_sound_test`).

vcod: `pmove::foliage_sound`, after `PM_Footsteps` in `pmove_single`. vcod
runs `PM_Weapon` after the whole step (`weapon::armed_step`) rather than
inside each chopped step, so a shot's events land after the rustle in the
ring where retail's land before it. INFERRED, the call order. The client
counts 139 among its predicted events (`cod2_predict::predicted_event`).

## 13. Tunables

The `bg_`, `jump_` and `mantle_` dvars register with flags 0x1180
(`bg_fallDamage*` 0x1188). VERIFIED (`BG_RegisterDvars` 0x80de830,
`Jump_RegisterDvars` 0x80dc60c, `Mantle_RegisterDvars` 0x80dcf8c). The 0x100
bit marks a dvar whose server value overrides the client's. INFERRED. So
the client's prediction runs on the server's values, as 1.5's bob cvars do
through 140/204, and a server that sets `jump_height` or `friction`
changes every client's predictor. vcod's predictor runs the defaults
(section 18).

| Constant | CoD2 | Where | 1.1 | Label |
|---|---|---|---|---|
| gravity, speed | `(int)g_gravity` 800, `g_speed` 190 | `ClientEndFrame` 0x80f7516 | same | VERIFIED |
| friction / stopspeed | 5.5 / 100 | dvars | 5.5 / 100 rodata | VERIFIED |
| walk accel floor | `stopspeed` | `PM_Accelerate` | 100 rodata | INFERRED |
| accel stand / crouch / prone / slick | 9 / 12 / 19 / 1 | immediates 0x80e2842.. | same | VERIFIED |
| air accel | 1 | immediate | same | VERIFIED |
| spectator friction | 5 | rodata 0x8154ee8 | 5 | VERIFIED |
| knockback friction / accel | 0.3 / 0.25 | rodata 0x8154ee4 / 0x8154fb8 | same, on 0x100 | VERIFIED |
| inertia max / angle | 50 / 0 | dvars | none | VERIFIED |
| crouch / prone scale | 0.65 / 0.15 | rodata 0x8154fa4 / 0x8154fa0 | ps fields, same values | VERIFIED |
| sighted walk = lean scale | 0.4 | rodata 0x8154f94 | 0.4 / 0.4 ps fields | VERIFIED |
| back / strafe scale | 0.7 / 0.8 | dvars | ps fields, same values | VERIFIED |
| spectator / noclip / UFO scale | 2.0 / 3 / 6 | dvar, rodata 0x8154f98, 0x8154f9c | speed 400 | VERIFIED |
| jump height | 39 | dvar `jump_height` | 39 rodata | VERIFIED |
| jump step | 18 | dvar `jump_stepSize` | | VERIFIED |
| jump spread add / cap | 64 / 255 | dvar, rodata 0x8153340 | 64 / 255 | VERIFIED |
| jump re-arm | 500 ms | immediate | 500 | VERIFIED |
| penalty 1800 / 1200, high 18, 0.65 / 0.5 | as 1.5 | 0x80dc818, rodata 0x8153320 | none (1.5: same) | VERIFIED |
| ladder push-off | 128 | dvar `jump_ladderPushVel` | 128 rodata | VERIFIED |
| step / prone step | 18 / 10 | immediates | same | VERIFIED |
| ground snap | 9 | rodata 0x8155324 | half a step (1.5: 9) | VERIFIED |
| step-down normal floor | 0.3 | rodata 0x8155328 | none | VERIFIED |
| min walk normal | 0.7 | rodata 0x815505c | 0.7 | VERIFIED |
| clip | `dot - abs(dot) * 0.001` | rodata 0x8154ee0 | 1.001 overclip | VERIFIED |
| same-plane | 0.999 | rodata 0x815531c | 0.999 | VERIFIED |
| max clip planes | 8 | `PM_SlideMove` | 8 | INFERRED |
| frame clamp | 66 ms | immediate | same | VERIFIED |
| box | (-15,-15,0)..(15,15,70), 50, 30 | .data 0x8187ba0, immediates | same | VERIFIED |
| eye | 60 / 40 / 11 / dead 8 | immediates | ps fields, same values | VERIFIED |
| eye lerps | 400 / 200 down to crouch / 400 / 200 | 0x80e3c46 | 150 down to crouch | VERIFIED |
| prone body / swing / divisor | 66 / 45 / 240 | rodata 0x815511c / 0x8155118 / 0x8155114 | 54 (1.5: 60) | VERIFIED |
| prone yaw cap / ladder yaw cap | 85 / 100 | dvars | 85 / 100 | VERIFIED |
| view pitch cap | 85 deg (15473) | dvars, rodata 0x8155124 | 16000 | VERIFIED |
| lean ramps / roll / dist | 350, 280 / 16 / 20 | rodata 0x8155110, 0x815510c, immediates | same | VERIFIED |
| lean scale stand / prone | 0.5 / 0.25 | immediates | 1 / 0.25 (1.5: 0.5) | VERIFIED |
| fall damage min / max | 256 / 480 | dvars | same | VERIFIED |
| landing stun | `35 d + 500`, 2000, 0.5 to 0.2 | 0x80e30c4 | same, on 0x100 | VERIFIED |
| landing damp | 0.67 at 12 | immediate, rodata 0x8155030 | same | VERIFIED |
| ladder reach / shrink / climb / strafe / friction | 8, 30 / 6 / 0.5 / 0.2 / 16 | 0x80e805a, 0x80e847e | same | VERIFIED |
| mantle range / radius / angle / yaw cap | 20 / 0.1 / 60 / 60 | dvars | none | VERIFIED |
| bob standing / ducked / prone / max | 0.007 / 0.0075 / 0.03 / 8 | dvars `bg_bob*` | cgame `cg_bob*`, same | VERIFIED |

A VERIFIED row rests on the dvar default (`BG_RegisterDvars`'s
immediates), the rodata VA or the immediate it names; the 1.1 column is
vcod's `pmove.rs` and `bsp-ibsp59-format.md`. The walk accel floor is read
off `PM_Accelerate`'s use of `stopspeed` and the plane count off
`PM_SlideMove`'s `> 7` test, both control flow.

## 14. Open

- `BG_CheckProneValid`'s sample order against 1.5b's was not diffed.
- Whether the Windows client keeps or stores in each function (section 2
  checked `PM_CrashLand` only), and what its predictor does with the
  unsent `oldVelocity`.
- The `almostGroundPlane` field's readers.
- 17.3's remaining gap: `EV_RESET_ADS` rising out of prone.

## 15. What a CoD2 `MoveRules` needs

Ranked by how much an A/B replay would miss without it:

1. **The usercmd.** Jump, stance and lean as `buttons` bits, the jump on
   the edge against `oldcmd`, no `upmove` latch. Nothing replays without
   it.
2. **Stored floats everywhere** on the server (an `Arith` that stores
   every assignment, not 1.5's three spots), and the MSVC keep on the
   client. Every bit of every origin rides on it.
3. **No trace `endpos`.** Origins after a hit are `start + (end - start) *
   fraction`, stored; collision's `cm_endpos` and the movers'
   `sv_trace_endpos` do not apply.
4. **`PM_ClipVelocity`'s `dot - |dot| * 0.001`.** Every ground clip and
   every slide plane.
5. **`PM_SlideMove`'s plane order** (`PM_PermuteRestrictiveClipPlanes`)
   and the clip-then-nudge on a repeated plane: any corner or crease.
6. **The inertia clamp and `oldVelocity`'s lerp**: any strafe reversal
   over 50 units a step.
7. **The jump**: 1.5's penalty on 0x80000, the step capped at the jump
   height, the vz clamp, prone entry only at `pm_time` 0, the ladder
   push-off.
8. **The step slide's 0.3 floor** on the down pass, and 1.5's 9-unit snap.
9. **The stance machine**: 1.5's one-at-a-time eye with 1.5's lerp times,
   the 66/45/240 prone body, the 48 hips sample, the 0.5 standing lean,
   the 15473 pitch cap.
10. **The cmd scales**: dvar back and strafe, one 0.4 for sight and lean,
    the crouch-prone blend over the eye lerp, the weapon scale, the damage
    timer, spectator x2 on 190, no water.
11. **The mantle**: a whole subsystem, but only on brushes flagged for it
    and only on a jump press.
12. **The flag renumbering** (`pm_flags`, `eFlags`) and the event table
    past 143: wire mapping, not movement, as 1.5's and UO's are.
13. **Dvar-driven tunables** synced from the server: the rules read
    values, not constants.
14. **The velocity snap's `rint`** (signed zero) and the ladder's Q2
    accelerate.

## 16. vcod's CoD2 pmove

`vcod_cod2::pmove` is a path of its own, not a `MoveRules` arm in
vcod-common's `pmove.rs`: items 1 to 5 above reach every function the CoD1
code shares between its rule sets, so CoD1's code stays as it is and the
two share only `MoveWorld` and the collision world. `player_step` runs one
`UserCmd2` through `Pmove`'s chop on a `PlayerState` holding the retail
fields pmove reads, over a `MoveWorld` built from a CoD2 `MapWorld`
(`vcod_cod2::world::from_bytes`), under `Rules` holding the dvars at
their defaults. Every float assignment is stored as `cod2_lnxded` stores
it (section 2); the collision world takes `Arith::Stored`.

Ported, by section 15's rank:

| Rank | What |
|---|---|
| 1 | the usercmd buttons |
| 2 | the stored floats on the server (the client's MSVC keep is not modelled) |
| 3 | the lerp placement |
| 4 | the clip |
| 5 | the plane permutation and the nudge |
| 6 | the inertia clamp and `oldVelocity` |
| 7 | the jump with its slowdown, step cap and vz clamp |
| 8 | the 0.3 floor and the 9-unit snap |
| 9 | the stance machine with its curves, the 66/45/240 prone fits, `BG_CheckProneValid`'s 48-unit hips, the prone swing and caps, the 0.5 lean, the 15473 pitch cap |
| 10 | the cmd scales |
| 12 | the flag numbering |
| 14 | the `rint` snap |

Also ported: the ground trace, the all-solid nudges, the crash land, the
noclip and spectator moves, `PM_Footsteps`' bob and footstep events
(17.2), the ladder with its push-off, yaw cap and rung bob (section 11),
the mantle (section 9), `PM_UpdatePronePitch` (10), the foliage sound
(12), the turret view clamp and the firing flag (`cod2-turrets.md` 5).
The legs and torso picks are `pmove::anim` (`cod2-player-anims.md` 9);
`PM_Weapon`'s torso events run through it when the caller hands
`WeaponCmd::anim` the script, as vcod-server does.

The weapon half (`PM_Weapon`, the sight flag and lerp, the aim spread, the
breath hold) is `vcod_cod2::pmove::weapon`, run beside `player_step` by
`weapon::armed_step` (`cod2-combat.md` 1). The server, the client's
predictor and `moves_ab` run it, the last with the probe's kar98k at
retail's index 3.

INFERRED choices where the binary's callee was not read:

- the prone move interrupt (`PM_InteruptWeaponWithProneMove`) is taken to
  interrupt;
- `PM_DamageScale_Walk` is taken as 1 (the stock
  `player_dmgtimer_minScale` 0);
- UFO flies as noclip at its x6.

Section 17 is the A/B gate against retail.

## 17. The A/B gate

### 17.1 The captures

The captures are retail `cod2_lnxded` 1.3 inside `unshare -rn` on
`mp_toujane`, the dvars at their defaults (a pure server), under a probe
gametype (`crates/cod2/tests/fixtures/moves/probe_moves.gsc`). It spawns
each client at `probe_x/y/z` facing `probe_yaw` with the kar98k
(`moveSpeedScale` 1, or the `probe_weapon` dvar's weapon) and takes no
damage. A headless client (`crates/cod2/examples/move_probe.rs`) joins,
waits for the spawn and plays a script: a cmd every 11 or 12 ms and a
packet every 33 to 36 ms carrying the new cmds and the previous packet's
(retail's `com_maxfps` 85, `cl_maxpackets` 30, `cl_packetdup` 1
defaults), the view held at the spawn's. `tools/run_probe_cod2.sh` runs
both; each fixture's header holds its command line. Every result in this
section is VERIFIED off those captures unless labelled.

The fixtures log each cmd as sent and each snapshot's changed playerstate
netfields as raw words. Every snapshot's `commandTime` equalled the
`serverTime` of a cmd sent: the server ran each cmd whole and clamped
none. VERIFIED.

| Capture | Map, spot | Script |
|---|---|---|
| `walk` | mp_toujane | a run each way with stops |
| `jump` | mp_toujane | jumps in place, running and on a held key |
| `stance` | mp_toujane | crouch, crawl, prone, a prone turn and back up |
| `reverse` | mp_toujane | hard strafe and run reversals |
| `lean` | mp_toujane | leans standing, moving and crouched |
| `wall` | mp_toujane | forward at a wall at 45 degrees |
| `slope` | mp_toujane | up a terrain slope |
| `steps` | mp_toujane | onto a kerb along a wall on a slope |
| `ledge` | mp_toujane | off a roof |
| `mantle` | mp_toujane | a jump at a low wall flagged `MANTLE_OVER` |
| `ladder` | mp_toujane (821, 789) | up the ladder from the ground, a hang, a strafe on the rungs, down, up, the push-off, back on from the ground and over the top; view pitched 30 up and 60 down |
| `mantle_turn` | mp_farmhouse (-614, 1140) | a `MANTLE_ON` ledge on the 27 row (flag 4, the gun kept up), the view turned 90 off the wall during the climb |
| `mantle_crouch` | mp_leningrad (15, 282) | the 39 row with flag 2 alone (141 every frame, the gun put away, `eFlags` 0x4000 kept after) |
| `walk_bar` | mp_toujane | `walk` with the bar (`moveSpeedScale` 0.9) |
| `stance_colt` | mp_toujane | `stance` with the colt (1.1) |

The spots were picked by running vcod's pmove from every spawn; the
mantle ones by `examples/mantle_spots.rs`. The first nine never mantle.
`mantle` mantles on retail at 6950 (`pm_flags` 4, `eFlags` 0x4000,
`mantleState.transIndex` 1), and `mantle_turn` and `mantle_crouch` each
on their jump cmd. VERIFIED.

`walk_bar` and `stance_colt` hand their weapon out in the kar98k's place
at the same index 3 through `probe_weapon`. Replayed at the held weapon's
scale both match retail on every row their kar98k twins do; replayed at
no scale `walk_bar` goes wrong on its first running row. VERIFIED. That
scale is what `PM_CmdScale_Walk` (0x80e1c58) reads off
`BG_GetWeaponDef(ps->weapon)` +0x264 each step. VERIFIED. vcod-server's
`client_think` and the client's predictor pass the same
(`weapon::move_scale`, `Cod2Session::move_scale`).

Two wire facts the comparison follows. A playerstate float goes integral
or raw with no zero flag, so a -0 the server holds reads as +0 unless the
client's base was +0: the gate compares a float's zero without its sign.
VERIFIED (`read_ps_value`, cod2-protocol.md 5). An angle is the 16-bit
short the wire carries. VERIFIED.

### 17.2 What the captures fixed

- **`oldcmd` is the cmd as the client sent it.** `ClientThink`
  (0x80f65f8, Mac 0x1a46c2) copies `pers.cmd` (client +0x26c8) to
  `pers.oldcmd` (+0x26e4) before `SV_GetUsercmd` fills the next, and
  `ClientThink_real` (0x80f5d2e, Mac 0x1a3d00) builds `pm.oldcmd` from that
  copy. VERIFIED, the copies and the stores into `pmove_t`. `Pmove`'s own
  `oldcmd = cmd` between chopped steps carries `PM_CheckJump`'s cleared
  jump bit, but it never leaves the `pmove_t`. Trap: handing back the
  step's copy makes a held jump key jump again on its next cmd; retail's
  `jump` capture lands once and holds (`pm_time` 1737 at 11250).
  VERIFIED.
- **The lean's step is one x87 expression.** `PM_UpdateLean` (0x80e69f4)
  computes `leanf -+ msec / 350 * scale` on the stack and stores once
  (`flds`/`fdivrp`/`fmuls`/`fsubp`/`fstps` at 0x80e6adf). Storing the
  step first is a bit off on 7 rows of `lean`. VERIFIED.
- **`PM_Footsteps` (0x80e572c), less its anims.**
  - `xyspeed` is `I_sqrt` of `vy*vy + vx*vx`. VERIFIED.
  - Off the ground (not linked) it returns when the stance matches
    `pm_flags` & 3; under `player_moveThreshhold` (10) or linked the cycle
    stops, and under 1 it is zeroed. INFERRED.
  - The rate divides `xyspeed` by the speed the keys ask for: `ps.speed`,
    times `0.75 * (strafe - 1) + 1` on a strafe alone, the back scale on a
    back key alone, `0.5 * (that + 1)` and `0.5 * (back + 1)` on a
    diagonal, 0.4 sighted or leaning, and the crouch (0.65) or prone
    (0.15) share, blended over the eye's lerp between the two. INFERRED,
    the selection.
  - The rate's constant is 0.24 / 0.25 prone, 0.315 / 0.34 crouched,
    0.325 / 0.36 running back and 0.305 / 0.335 otherwise (sighted / not;
    rodata 0x81550c8..0x8155100). VERIFIED, the constants.
  - The cycle steps by `msec * rate`, truncated (`fistp` under control
    word 0xc00), mod 256. VERIFIED.
  - With a move key held a footstep goes out when bit 7 of `cycle + 64`
    changes, on the ground, standing, out of the sighted walk and at
    `player_footstepsThreshhold` (0) or over (`PM_ShouldMakeFootsteps`
    0x80e55ca). The event is the ground's surface type (0 silent) past the
    run (1), walk (24, sighted or leaning) or prone (47) block
    (0x80e2fb0). INFERRED.
  - The step's footstep (`PM_StepSlideMove` 0x80ea3f4, its tail) is the
    same event after its `7 + 1.25 * n` bob (rodata 0x8155340,
    0x815533c; VERIFIED), behind the same `PM_ShouldMakeFootsteps`.
    INFERRED.
  - `PmoveSingle` calls it after the second ground trace in the default
    arm and after `PM_DropTimers` in the linked and turret arms
    (0x80e8934). VERIFIED, the call sites.

Matching rows per capture, before and after these fixes (the before
column is the first port with the gate's own parse and zero rules already
right):

| Capture | Rows | Free run | Resynced |
|---|---|---|---|
| walk | 261 | 19 -> 118 | 133 -> 260 |
| jump | 171 | 49 -> 171 | 129 -> 171 |
| stance | 333 | 45 -> 115 | 277 -> 332 |
| reverse | 169 | 15 -> 169 | 83 -> 169 |
| lean | 161 | 15 -> 161 | 136 -> 161 |
| wall | 169 | 20 -> 169 | 60 -> 169 |
| slope | 169 | 19 -> 24 | 59 -> 167 |
| steps | 169 | 3 -> 3 | 61 -> 163 |
| ledge | 169 | 20 -> 97 | 85 -> 167 |
| mantle | 105 | 19 -> 33 -> 105 | 67 -> 75 -> 105 |
| mantle_turn | 104 | 104 | 104 |
| mantle_crouch | 105 | 105 | 105 |
| ladder | 211 | 211 | 211 |

- `mantle`'s third column is the mantle's port (section 9). All three
  mantle captures match every row in both runs, `mantleState`'s four
  fields and the weapon's included. VERIFIED.
- `ladder` came with the ladder (section 11), so it has no before column;
  without it 0x20 was never set and `origin[2]` was wrong in 105 resynced
  rows. With the weapon half (`cod2-combat.md` 1.7) both runs match every
  row, the putaway and the raises included. Replayed through
  `Cod2Level::client_think` (`crates/server/src/game/cod2_ladder.rs`),
  which compares origins and the ladder flag only, it stays within 0.01 of
  retail's origin and holds the ladder exactly when retail did over the
  whole capture. VERIFIED.
- The free run starts at the spawn's first snapshot and counts rows up to
  its first divergence; the resynced run starts each row from retail's
  last. VERIFIED (`crates/cod2/tests/moves_ab.rs`).

### 17.3 The gaps

- **Terrain fractions and the crease on a slope (closed).** 11
  `origin[2]` rows on curved floors and one `steps` row sliding 0.035
  further along a wall up a slope were the clip map's capsule arithmetic,
  not pmove. Once the clip stored the closing distance and its square
  roots as floats (`cod2-d3dbsp-format.md`, "The trace A/B"), every one of
  those rows matches retail bit for bit. VERIFIED (`moves_ab`, both runs).
- **Weapon events (open).** `stance` and `stance_colt` get `EV_RESET_ADS`
  (149) as the body rises out of prone, which `pm_weapon` beside the move
  does not raise; the ring slot after it shifts (`moves_ab`'s `GAPS`, one
  row each of `eventSequence` and `events[1]`). VERIFIED. `ladder`'s
  `EV_PUTAWAY_WEAPON` (155) as the body takes the ladder and
  `EV_RAISE_WEAPON` (154) onto the empty hands 211 ms later (not as it
  leaves, which raises none) match through the ladder arm of `PM_Weapon`
  (`cod2-combat.md` 1.7). VERIFIED.
- **The mantle (closed).** With section 9 ported every row of `mantle`
  matches, and the gate compares `mantleState.*` too. VERIFIED
  (`moves_ab`, both runs).

## 18. The client's prediction

`CG_PredictPlayerState` (Mac 0x1ddafa) is Q3's. VERIFIED, the decompile's
calls and constants:

- It copies the current snapshot's playerstate and replays every cmd from
  `CL_GetCurrentCmdNumber() - 127` whose time is past its `commandTime`
  through `Pmove`, each with the cmd before it as `oldcmd`. VERIFIED.
- It interpolates instead while a demo plays, under `cg_nopredict` or
  `cg_synchronousClients`, or while the snapshot's `pm_flags` carry
  0x400000 (`snap + 0x1a & 0x40`, the playerstate starting at snap + 0xc).
  VERIFIED, the tests. That 0x400000 is the follow flag is INFERRED:
  `CG_CalcMuzzlePoint` (Mac 0x1d7f72) takes the view origin for the
  playerstate's own client under 0xc00000, and a live player's snapshots
  carry 0x800000 alone (`tests/fixtures/moves/`). VERIFIED, the captures'
  flags.
- The trace mask is 0x810011, widened to 0x2810011 under `pm_type` 6 and
  narrowed by 0x2010000 for a spectator (4). VERIFIED, the immediates (the
  decompile misnames the widened constant as a string address).
- When the replay reaches last frame's predicted `commandTime`, the origin
  there against last frame's (both carried by the ground mover) is the
  correction. One longer than 0.1 (Mac rodata 0x326388) decays what is
  left of the last by `(cg_errordecay - (time - predictedErrorTime)) /
  cg_errordecay`, adds itself, and restamps the time. VERIFIED.
  `cg_errordecay` registers at 100. VERIFIED. `CG_CalcViewValues` (Mac
  0x1d3b28) adds the error times the same fraction to the eye while it is
  inside (0, 1) and drops it otherwise. VERIFIED.
- The eye. VERIFIED, the functions and their rodata (0x32618c 255,
  0x340c08 pi, 0x340c78 2 pi, 0x3261c0 4, 0x340c80 pi / 2, 0x3262dc 0.2,
  0x326234 0.75):
  - `viewHeightCurrent` over the origin;
  - the vertical bob `min(speed * amplitude, bg_bobMax) * (sin(2c) + 0.2
    sin(4c + pi/2)) * 0.75` added to z, and the horizontal `min(...) *
    sin(c)` along the view's right;
  - `c` is the low byte of `bobCycle` over 255 times 2 pi plus 2 pi
    (`BG_GetBobCycle` Mac 0x7a088), `speed` the flat speed off a ladder
    (`BG_GetSpeed` Mac 0x6b746), and the amplitude prone at
    `viewHeightTarget` 11, ducked at 40 (`BG_GetVerticalBobFactor` Mac
    0x7a0c8, `BG_GetHorizontalBobFactor` Mac 0x7a1b2).
- After the bob, the landing dip, then the step: `CG_EntityEvent` (Mac
  0x1e4650) takes an `EV_STEP_VIEW` whose `clientNum` is the predicted
  playerstate's, outside a demo (`cg` +8) and with `cg_nopredict` and
  `cg_synchronousClients` off, as `cg + 0x284d0 = (100 - dt) * change /
  100 * 0.9 + (parm - 128)` while `dt = cg.time - cg + 0x284d4` is under
  100, else `parm - 128`, clamped to -16..24 (0x326470 24, 0x326630 -16),
  and stamps `cg + 0x284d4` with `cg.time` (0x326350 100, 0x326370 0.9).
  VERIFIED, the stores and the rodata. `CG_CalcViewValues` restamps a
  time ahead of the clock, then while `dt` is under 100 adds `(100 - dt)
  * change / -100` to the eye's z (0x326620 -100). VERIFIED. A step up
  sinks the eye by its size and lifts it back over 100 ms. INFERRED, the
  sign.
- Then the lean: `AddLeanToPosition` (Mac 0x2923c, called with 16 and 20)
  eases `leanf` to `(2 - |leanf|) * leanf` (0x3261e4 2) and moves the eye
  20 times that along the right of the yaw rolled 16 times it. VERIFIED.
  The eye's z is held at least 8 over the origin (0x3262f8). VERIFIED.

vcod (`crates/client/src/play/cod2_predict.rs`):

- replays the newest snapshot's playerstate (`PlayerState::from_wire`)
  with every kept cmd past its `commandTime` through `player_step`, or
  `weapon::armed_step` with the level's weapon table, over the map's
  collision world, and eases corrections as above;
- draws the eye above plus the landing dip and the step
  (`first_person_eye`'s `dips`, `play::cod2_view::Land::eye` and
  `Step::eye`, the step taken in `Cod2Session::recoil_frame` off the
  frame's own events, none in a demo); the damage kick, the sight's sway and
  bob, the camera shake and the shellshock kick are the view's angles
  (`play::cod2_view`);
- runs the Linux server's stored floats (`Arith::Stored`, section 2), not
  the Windows client's MSVC keep: what the server computes, so a
  prediction can be exact. Retail's own client corrects in the last bits.
  INFERRED;
- starts each replay's `oldVelocity` (not on the wire) from the value its
  last replay reached at the snapshot's `commandTime`. Retail starts from
  the snapshot copy's, which nothing writes. INFERRED. Over the captures
  of 17.1 both starts predict the same origins.

Measured:

- Every capture of 17.1 replayed as the client would have run it (the cmds
  up to the next snapshot's `commandTime` on top of each snapshot)
  predicts the next snapshot's origin bit for bit in every row, the three
  mantles included. VERIFIED (`retail_captures_predict_bit_for_bit`).
- Live, `vcod --net-probe 127.0.0.1:28970 --game cod2 --probe-team allies`
  against retail `cod2_lnxded` on mp_toujane (loopback, 40 s, the probe's
  scripted walk, strafe, jump, crouch, fire, sight and switch loop): 4587
  predicted frames, no miss, 19 corrections past 0.1, all in the two
  seconds after the first lap's jump mantled onto a ledge (`pm_flags`
  0x4), the longest 11.7 units; none in the second lap, whose jump did not
  mantle. VERIFIED. That run predates the mantle's port.
- Against `vcod-server --game cod2` (34 s, the same loop): 3956 frames, no
  correction at all. VERIFIED.

The mantle is predicted once `Cod2Session` hands the predictor the anims
(`mantle::anims`, when the map's collision is built), the weapon with its
events once it hands the level's weapon table (cod2-hud.md 18.1), and
other bodies as section 20 clips them. Not predicted: brush models, item
pickups and the server's `bg_*` dvar values (the replay runs the
defaults).

## 19. The breath hold

`PM_Weapon`'s caller (0x80f066e) runs the sight lerp (0x80ecc9c) and then
the breath hold (0x80ed084) on every cmd that passes its three gates
(`pm_flags & 0x1000` clear, `pm_type` under 6, `eFlags & 0x300` clear).
VERIFIED, the calls and their order. vcod:
`pmove::weapon::update_hold_breath`, called from `pm_weapon` after
`update_sight_lerp`.

- The dvars (`Dvar_RegisterFloat`, flags 0x1180, 90983..). VERIFIED:

  | Dvar | Default |
  |---|---|
  | `player_breath_hold_time` | 4.5 |
  | `player_breath_gasp_time` | 1 |
  | `player_breath_fire_delay` | 0 |
  | `player_breath_gasp_scale` | 4.5 |
  | `player_breath_hold_lerp` | 4 |
  | `player_breath_gasp_lerp` | 6 |

  The times go to ms as `(int)(1000.0 * value)` under a truncating control
  word (`fldcw` around `fistpl` at 0x80ed0ce, 0x80ed0e7; 1000.0 at rodata
  0x8155728). VERIFIED. vcod reads the defaults only (`Breath::DEFAULT`).
- The breath weapon is the viewmodel weapon (0x80ebd22: the binoculars
  while `pm_flags & 0x20000`, the offhand while 0x10, else `weapon`) with
  `fWeaponPosFrac` exactly 1, `adsOverlayReticle` (def+0x278) non-zero
  and `weaponClass` (def+0x7c) not 9 (`item`). VERIFIED.
- The timer. VERIFIED:
  - with a hold time under 1 ms the step clears `pm_flags` 0x4000, sets
    `holdBreathScale` (ps+0x5b4) to 1 and `holdBreathTimer` (ps+0x5b8) to
    0;
  - otherwise, off a breath weapon or without the cmd's 0x8000
    (`+holdbreath`), 0x4000 is cleared; with both and a timer of 0 it is
    set;
  - the timer adds `pml.msec` while 0x4000 is set and subtracts it
    otherwise, floored at 0;
  - a held timer past the hold time is set to hold plus gasp time (5500)
    and 0x4000 cleared: the gasp.

  So a key held through the gasp holds again only once the timer is back
  to 0. INFERRED.
- The scale tracks a goal through `DiffTrack` (0x80a3cea: `cur + rate *
  (goal - cur) * frametime`, landing on the goal once the gap is 0.001 or
  less, rodata 0x814d024, or the step would pass it). Held: target 0 at
  `hold_lerp`. Else `1 + (gasp_scale - 1) * timer / (hold + gasp)` at
  `gasp_lerp`, the ratio stored as a float first (`fstps` at 0x80ed26a).
  The goal is `(target - 1) * fWeaponPosFrac + 1`. VERIFIED.
- A fresh spawn's scale is `ClientSpawn`'s 0 and climbs to 1 at the gasp
  rate: the captures of 17 read 0.519 on their first snapshot. VERIFIED
  live.
- A shot (0x80ef58a, after its fire event) calls 0x80ecf9c: down a breath
  weapon's sight, a timer under the hold time takes `fire_delay` ms more,
  capped at the hold time, and 0x4000 is cleared. VERIFIED. vcod:
  `breath_on_fire`.

The capture `breath` (`crates/cod2/tests/fixtures/combat/breath.txt`,
`kar98k_sniper_mp` on mp_toujane, recipe in its header) holds a 2 s hold
let go, the timer back to 0, a hold broken by a shot and taken up again
once the timer ran out, and a 5.5 s hold through the gasp. `moves_ab`
replays its 411 rows with `holdBreathScale`, `holdBreathTimer` and
`pm_flags` compared bit for bit, free run and resynced alike, and the
earlier captures' rest climb with them. VERIFIED. The scale multiplies
the scope's idle sway, which vcod's CoD2 view draws
(`play::cod2_view`, `hold_breath_scale`).

## 20. Other bodies in the prediction

`CG_BuildSolidList` (Mac 0x1dea06) runs from `CG_SetNextSnap` (Mac
0x1dee08), after the new snapshot's entities are copied into their
`nextState`. VERIFIED, the call. It walks that snapshot's entities
(`cg.nextSnap`, `cg` +0x24) with no teleport test. VERIFIED, the load.

- An entity whose `nextState.solid` is 0xffffff (a brush model) is skipped
  when its `eFlags & 1` is set or `CM_ContentsOfModel` of its `index` is
  0. VERIFIED, the tests.
- Of the rest, `eType` 3 (an item) goes to the trigger list and any other
  with a non-zero `solid` to the solid list. VERIFIED.

`CG_PredictPlayerState` traces through `CG_TraceCapsule` (Mac 0x1de958):
`CM_BoxTrace` against the world, then, unless that stopped at 0,
`CG_ClipMoveToEntities` (Mac 0x1de5b8) over the solid list. VERIFIED.

- It passes over the entity numbered as the skip argument, the predicted
  playerstate's `clientNum`. VERIFIED, the compare; INFERRED that the
  argument is `clientNum`, as Q3's.
- A brush model is traced at its lerped origin and angles (centity
  +0x1ec, +0x1f8). VERIFIED.
- Any other entity clips with contents 0x2000000 when its `eType` is 1 (a
  player) and 1 otherwise, against the trace's mask. VERIFIED.
- Its box is unpacked from `solid` as `x = solid & 0xff`, `zd = (solid >>
  8) & 0xff`, `zu = (solid >> 16) & 0xff`: mins `(1 - x, 1 - x, 1 - (zd -
  1))`, maxs `(x - 1, x - 1, zu - 32 - 1)`, the 1 being the float at
  rodata 0x326188. VERIFIED, the arithmetic and the constant. That box
  goes to `CM_TempBoxModel` (Mac 0x63f14) and is traced at the lerped
  origin with angles 0. VERIFIED.

`SV_LinkEntity` (lnxded 0x809c63a) packs `solid` from `r.maxs[0]`, `1 -
r.mins[2]` and `32 + r.maxs[2]` (rodata 0x814c678), each truncated
(`fldcw` with 0xc00 at 0x809c69f) and clamped to 1..255, when `r.contents
& 0x2000001` is set, else 0. VERIFIED. So the client's box is the server's
shrunk by a unit on every side: a standing player's 0x66010f
(cod2-protocol.md 15.6) unpacks to (-14,-14,1)..(14,14,69) against the
server's (-15,-15,0)..(15,15,70). VERIFIED, the two formulas.

The server clips another client through `SV_ClipHandleForEntity` (lnxded
0x809c544), which hands a non-brush entity's `r.mins` and `r.maxs` to
`CM_TempBoxModel` (0x80592e2) unshrunk. VERIFIED. `CM_Trace` (Mac
0x64426) traces the temp box model (handle 0x3ff) as a capsule: radius
the smaller of its half width and half height, end spheres through
`CM_TraceSphereThroughSphere` (Mac 0x63bec), a side cylinder between, and
the 0.125 back-off of rodata 0x3261c8. VERIFIED, the structure and the
floats. That is the construction vcod's `movetrace` capsule clips 1.1's
bodies with (cod11-player-clip.md 2.2). INFERRED, that the two agree in
every case.

vcod: `play::cod2_predict::solid_bodies` builds the list off the frame's
`snap` (`cg.nextSnap`) at `Frame::entity_origin`, where the frame draws
each entity (`CG_CalcEntityLerpPositions` runs before
`CG_PredictPlayerState` in `CG_DrawActiveFrame`, Mac 0x1d4a50; VERIFIED,
the call order). It passes over the own `clientNum` and unpacks `solid`
through `body_of_solid`; the predictor clips them as movetrace bodies
beside the world. Brush models are left out: the client's collision world
does not move its submodels.

Measured live on loopback with
`crates/cod2/tests/fixtures/moves/probe_pair.gsc` (`probe_moves.gsc` with
client 0 set `probe_dy` 120 further along y, run as gametype
`probe_pair`, on vcod-server through `--gametype-script`) on mp_toujane at
(-299, 997, 43) yaw 90: a `VCOD_PROBE_STILL` probe stands as client 0 and
a `--probe-team allies` probe runs its scripted loop into him, pressing
into his body for about three seconds. VERIFIED, each count below off the
probe's summary.

| Server | Bodies clipped | Corrections past 0.1 | Longest |
|---|---|---|---|
| retail `cod2_lnxded` | no (`VCOD_PROBE_NO_BODIES`) | 67 | 7.60 |
| retail `cod2_lnxded` | yes | 66 | 1.09 |
| `vcod-server --game cod2` | no | 65 | 6.08 |
| `vcod-server --game cod2` | yes | 66 | 1.07 |

With the bodies every correction while pressing is 1.0, the unit the
cgame's box gives away, on both servers. VERIFIED. So vcod-server stops a
run into a player where retail's does, to the probe's resolution.
VERIFIED. Retail's own client would correct the same unit on every
snapshot. INFERRED, from the same rule.
