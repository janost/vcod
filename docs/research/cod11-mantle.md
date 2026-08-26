# CoD 1.1 mantling and the pmove ledge mechanics that actually exist

**Verdict up front: CoD 1.1 multiplayer has no mantle.** No ledge-detection probe,
no climb impulse, no mantle flag, no mantle anim path exists in the server-side
pmove. The mechanic the task brief describes does not appear in any 1.1 module.
What does exist is a ladder system (probe + climb + push-off), a stance-dependent
jump, an 18-unit step-up, and a steep-slope slide - together they produce every
"climb onto a ledge" behaviour a player sees in game. This document maps those
mechanics with addresses so nobody searches for mantle again, and ends with
port notes for `crates/common/src/pmove.rs`.

Everything below is INFERRED from static analysis of the binaries unless marked
otherwise; nothing here was verified against live server behaviour.

## Sources

Primary evidence is the retail 1.1d Linux dedicated server's game module,
`private/server/main/game.mp.i386.so` (md5 `de8947beb6f86fbfb46f5adfaab3d3ed`,
ELF32 i386). Its `.dynsym` exports every internal `PM_*`/`BG_*` function name
and most globals (`pm`, `pml`, the `pm_*` tuning floats), which makes it the
highest-signal CoD 1.1 game-module source available; `game_mp_x86.dll` is
stripped and MSVC-compiled. The first LOAD segment maps at vaddr 0, so text and
rodata VAs equal file offsets; `.data` VAs are file offset + 0x1000.

Cross-checks to the Windows module used by other research docs:
`main/game_mp_x86.dll` 1.1 (md5 `25e2fcfe02ca0c46f4e9ad2530d50691`) carries the
same constant pool - e.g. the pair `39.0`/`78.0` sits adjacent in its rodata at
file offsets `0x5fa90`/`0x5fa94`, matching the Linux jump code - but instruction
patterns differ enough (GCC vs MSVC) that byte-level matches fail. Function-level
findings are stated against the Linux binary only.

Struct layouts come from the disassembly itself, cross-checked against the
CoDExtended `shared.h` playerState/usercmd definitions
(`private/reference/CoDExtended/src/shared.h`). Warning for future readers:
CoDExtended offsets match the *internal* server struct, while netfield-table
offsets (see `nf_mp.txt`, `docs/protocol-1.1.md`) index a different layout -
e.g. `groundEntityNum` lives at ps+0x54 internally (the code writes 1023 there)
but at wire offset 124. Never mix the two.

Disassembly listings: `private/ghidra/gamemp.asm` (objdump of the .so). Call
targets in that listing need the `.rel.text` PC32 relocations resolved manually;
relocation entries also carry names for data references (`pml`, `pm`,
`bg_ladder_yawcap`, ...).

## Search log: where mantle was looked for and what was found instead

The brief's hypothesis - mantle logic inside game_mp's compiled-in bg_pmove near
the waterjump/ladder dispatch - is correct about *location* and wrong about
*existence*. What I searched:

- **Symbols.** The full `.dynsym` export list (1004 functions) contains no
  function or data symbol matching mantle/climb/vault/ledge beyond the ladder
  set (`PM_LadderMove`, `pm_ladderScale`, `pm_ladderPushOff`,
  `pm_ladderJumpTime`, `pm_ladderfriction`). Every pmove tunable is enumerated
  below under "Tunables"; none is mantle-related.
- **Strings.** `strings` over `game.mp.i386.so`, both `game_mp_x86.dll`s
  (1.1/1.5), `cgame_mp_x86.dll` 1.1, `CoDMP.exe`, `CoDSP.exe`, `cgamex86.dll`
  1.1/1.5: zero hits for "mantl" in all of them. "climb" hits only as anim
  state tokens (`CLIMBUP`, `CLIMBDOWN`, `CLIMBMOUNT`, `CLIMBDISMOUNT`) in the
  `mp/playeranim.script` token table shared with AI animation parsing.
- **Decompilations** (`private/ghidra/cgmp11.c`, `cod11.c`, `cod15.c`,
  `CoDMP.exe.c`; note `cod11.c`/`cod15.c` are singleplayer cgamex86.dll, not
  game_mp): no climb/mantle-labelled references. cgame_mp knows exactly one
  ladder string, `"hintLadder"` (use-button hint).
- **Trace sites.** Every indirect call through the four trace hooks in
  `pmove_t` (`pm+0xE8` trace, `+0xEC`, `+0xF0`, `+0xF4`, read via
  `PM_SlideMove` at 0x34988) inside the pmove region 0x2E400-0x35000 was
  inspected. They implement: ground stick/snap (fn at 0x30214), ground trace +
  slope/water categorisation (fn at 0x30474), three sight-trace checks (fn at
  0x30778), stance-change headroom probes plus jump plus ground snap (fn at
  0x316F4), the prone debug visualiser gated on `g_debugProneCheck` (code at
  0x32FA1), and ladder detection (fn at 0x336E8). None probes forward-then-up
  over a ledge; the geometry of each is summarised below.
- **Anim selection.** `mp/playeranim.script` (in pak4.pk3, extracted) defines
  `climbup`/`climbdown` movement types mapping to `pb_climbup`/`pb_climbdown`.
  In the binary these are MOVETYPE condition values 16 and 17 (name table at
  .data 0x7A3E0..0x7A470: UNUSED..TURNLEFT=15, CLIMBUP, CLIMBDOWN), selected
  only in one branch, decoded under "Ladders" below. There is no second
  selection site that could serve a mantle.

Negative result is high-confidence: the server module is authoritative for
movement, its pmove region is fully mapped, and nothing in it boosts or
teleports a player over a ledge except ladders.

## Frame flow of the normal-move path

`PmoveSingle` (0x33DFC) dispatches on `ps.pm_type` via a jump table at rodata
0x70CE8; pm_type 0 (`PM_NORMAL`) falls through to the default branch, which
runs, in order:

| step | callee | role |
|---|---|---|
| pre | fn 0x30778 | pre-move bookkeeping |
| | saves old water level into pml | drives EV_WATER_TOUCH/LEAVE at frame end (events 144/145, fired at 0x3435F/0x3438B) |
| | fn 0x316F4 | spectator bbox setup, stance transitions, viewheight lerp, **ground jump**, ground snap |
| | fn 0x30474 | ground trace, walking/steep-slope categorisation, landing events |
| | `PM_UpdateAimDownSightFlag` etc. | weapon/stance updates |
| | fn 0x336E8 | **ladder probe**, sets/clears `PMF_LADDER` + `vLadderVec` |
| move | dispatch at 0x34305 | `pm_flags & PMF_LADDER` -> `PM_LadderMove` (0x33944); steep-slope flag -> fn 0x2F258; otherwise walk/air mover fn 0x2F03C |
| post | fns 0x30474, 0x30778 again | re-categorise |

The walk/air mover (0x2F03C) is Q3-shaped: wishdir from cmd and yaw, wishspeed
from `BG_GetSpeed` scaled by `ps.speed`-related scales, accelerate
(`pm_accelerate` = 9.0 ground / 1.0 air), then `PM_StepSlideMove(gravity)`.

## Jumps (there are two)

**Ground jump**, inside fn 0x316F4 at 0x31CC0:

- Gated on: no `pm_flags & 0x20` timer bit, `cmd.forwardmove != 0`.
- Jump height depends on stance: standing (`(pm_flags & 3) == 0`, flag derived
  at 0x317E8) gives height 34 units, crouched/prone gives 24. Vertical velocity
  is `sqrt(2 * height * gravity)` with gravity read from `ps.gravity` (int,
  ps+0x3C): 233.2 standing / 196.0 crouched at gravity 800 (constants 34.0 and
  24.0 at 0x70BE8/0x70BEC).
- Sets `groundEntityNum = 1023`, zeroes the pml walking and steep-slope flags,
  writes 255 into `ps.aimSpreadScale` (ps+0x3D8).

**Ladder/waterjump-path jump** ("PM_Jump", exported-callable body at 0x2EB98),
called from `PM_LadderMove` and the steep-slope mover only:

- Requires `cmd.serverTime - ps.jumpTime > 499` (500 ms cooldown, compare at
  0x2EBB3), not prone, not blocked by two higher pm_flags bits (0x800/0x2000),
  and `cmd.upmove > 9`.
- Vertical velocity `sqrt(gravity * 78)` = ~250 at gravity 800 (constants 78.0
  and 39.0 at 0x708C8/0x708CC); sets `ps.fJumpOriginZ` (ps+0x68) =
  origin.z + 39.
- Horizontal velocity is *reset* to 128 units along a direction vector
  (`pm_ladderPushOff` = 128.0, rodata 0x7082C, applied at 0x2ED5A):
  - on a ladder the direction is the push-off away from the ladder plane
    (reflection-style composition of wish-forward with `-vLadderVec`,
    0x2ECA2..0x2ED36) and vertical velocity is first scaled by 0.75
    (0x708D0);
  - off a ladder the direction is the horizontal forward from pml.
- Clears `PMF_LADDER`, fires `EV_JUMP_*` (base 70 + surface index; hardcoded
  83 = `EV_JUMP_METAL` when leaving a ladder, 0x2ED86), adds 64 to
  `aimSpreadScale`, fires anim event JUMP (3) or JUMPBK (4) depending on the
  sign of forwardmove, then callers store `cmd.serverTime` into
  `ps.jumpTime`.

Note vcod's current `JUMP_VELOCITY = 250.0` corresponds to neither path
exactly; see port notes.

## Ladders

**Detection**, fn at 0x336E8, called once per frame before the move dispatch:

1. Skip if `ps.pm_time != 0`.
2. Probe distance: 30 units normally, 8 while the steep-slope flag is set
   (constants 30.0/8.0 at 0x70CAC/0x70CA8).
3. Direction: `-vLadderVec` if already on a ladder and airborne (sticking with
   the wall you left), else the horizontal forward.
4. Gates: clears `PMF_LADDER` first; skip when within `pm_ladderJumpTime` =
   300 ms (int at rodata 0x70830; bytes 2c 01 00 00 - an earlier read of
   this file said 299) of `ps.jumpTime` (compare 0x12B at 0x33822);
   on a steep slope additionally require `forwardmove > 0`.
5. Geometry: box trace from origin, mins/maxs taken from `pm->mins/maxs`
   (which PmoveSingle refreshes each frame from `ps.mins/maxs`, ps+0x324/0x330)
   shrunk horizontally by 6..8 units per side (constants 6/8 at 0x70CB0/0x70CA8)
   and with the top lowered by the probe distance; trace length = distance from
   step 2 along the direction (trace call at 0x338F4).
6. Accept when the trace hits (`fraction < 1`) **and** the hit surface flags
   have bit 0x8 set (Q3's `SURF_LADDER` value; test of the trace_t surfaceFlags
   byte at 0x33908). Store the plane normal into `ps.vLadderVec` (ps+0x58) and
   set `PMF_LADDER` (pm_flags bit 0x10).

So ladders are brushes flagged at the material level, not entities.

**Movement**, `PM_LadderMove` (exported, 0x33944):

- First calls the PM_Jump body above; if it jumped (push-off), runs the normal
  mover and stores `jumpTime`, done.
- Otherwise builds wish velocity from forward/rightmove projected onto the
  ladder plane via `ProjectPointOnPlane`, applies `pm_ladderfriction` (16.0)
  and `pm_ladderScale` (0.5) speed scaling, moves with the normal mover, and
  clamps vertical velocity against the ladder top/bottom.
- Pressing back (+up?) pushes off with velocity -250 or -500 along the view
  forward (constants at 0x70CD4/0x70CD8, applied around 0x33D47) - the
  backwards dismount hop.
- Yaw lock: `AngleDelta(vectoyaw(vLadderVec), viewYaw)` clamped to ±75 degrees
  (0x4B; cvar-backed float `bg_ladder_yawcap`, .bss 0x16E9C0) is stored to
  ps+0x7C (a movement-direction field; exact client-side use unverified).
- Leaving a ladder fires event 0x90 (EV_WATER_TOUCH per the EV table - the
  pairing looks odd; flagged unverified).

**Animation**: the movement-anim update (fn 0x322C8) selects, when airborne on
a ladder and outside the 300 ms post-jump lockout, MOVETYPE CLIMBUP (value 16)
or CLIMBDOWN (17) by the sign of `velocity.z` scaled against speed scales
(decision block 0x323CE..0x3242E, threshold constant 95.25 at 0x70C10 and
factor 0.45 at 0x70C18). These condition values resolve to `pb_climbup` /
`pb_climbdown` through mp/playeranim.script. Mount/dismount anims come from anim
events 8/9 (`CLIMBMOUNT`/`CLIMBDISMOUNT` in the same name table). This is the
only place those values are ever produced - there is no non-ladder route to
`pb_climbup`.

## Step-up and steep slopes

- `PM_StepSlideMove` (exported, 0x34FBC): step height 18 units, reduced to 10
  while PRONE (constants 18.0/10.0 at 0x70EEC/0x70EE8; the chooser at 0x35045
  tests pm_flags bit 0x1 = PRONE - an earlier read of this file said
  walking && !on-ladder, contradicted by its own bytes). Uses `ps.fJumpOriginZ` (compared ±0.001) as
  part of the step decision.
- The ground-trace function (0x30474) classifies the ground contact: impact
  velocity along the normal > 10 leaves the ground and fires the JUMP/JUMPBK
  anim events; normal.z >= 0.7 marks walking; anything flatter sets the pml
  steep-slope flag (pml+0x2C), which routes the next move through the
  steep-slope mover fn 0x2F258 (clip velocity to the slope plane with 1.001
  overclip, then accelerate) and shortens the ladder probe to 8 units.
- Stance changes trace headroom with the new bbox at the current origin
  (probes at 0x319ED/0x31A65/0x31AC3/0x31B47) and fire EV_STANCE_FORCE_*
  (140/141/142) when forced; viewheight lerps run through
  `PM_GetEffectiveStance` (0x34554) and `PM_GetViewHeightLerpTime` (0x345B8,
  200 ms stand/crouch family plus `bg_duck2prone_time`/`bg_prone2duck_time`).

## State reference (observed pm_flags bits, internal ps+0xC)

| bit | meaning | evidence |
|---|---|---|
| 0x01 | PRONE | tested everywhere stance matters (e.g. 0x3455A) |
| 0x02 | CROUCH | 0x34590 pattern |
| 0x04 | set by the ground-jump gate; read by viewheight-lerp timing | 0x31CD5, 0x345C9 |
| 0x10 | LADDER | set at 0x33937, cleared at 0x3377C/0x2ED7A |
| 0x80 | affects ladder-anim speed-scale choice | 0x323D7 |
| 0x20 | jump-inhibiting timer bit | tested at 0x31CCB |

(`PMF_PRONE`, `PMF_CROUCH`, `PMF_LADDER`, `PMF_SLIDING=0x100` agree with
CoDExtended's `shared.h`; bits 0x04/0x20/0x80/0x800/0x2000 are INFERRED from
usage sites only.)

## Tunables (all read directly from rodata, VERIFIED values)

| symbol | address | value |
|---|---|---|
| pm_stopspeed | 0x70824 | 100.0 |
| pm_ladderScale | 0x70828 | 0.5 |
| pm_ladderPushOff | 0x7082C | 128.0 |
| pm_ladderJumpTime | 0x70830 | 300 (ms, int) |
| pm_waterSwimScale | 0x70834 | 0.5 |
| pm_waterWadeScale | 0x70838 | 0.7 |
| pm_prone_accelerate | 0x7083C | 19.0 |
| pm_ducked_accelerate | 0x70840 | 12.0 |
| pm_accelerate | 0x70844 | 9.0 |
| pm_airaccelerate | 0x70848 | 1.0 |
| pm_wateraccelerate | 0x7084C | 4.0 |
| pm_flyaccelerate | 0x70850 | 8.0 |
| pm_friction | 0x70854 | 5.5 |
| pm_waterfriction | 0x70858 | 1.0 |
| pm_ladderfriction | 0x7085C | 16.0 |
| pm_spectatorfriction | 0x70860 | 5.0 |

Other constants seen in the move paths: 0.25 down-trace, MIN_WALK_NORMAL 0.7,
impact threshold 10, viewheight lerp 180 deg/s-ish factors, spectator bbox
(-8,-8,-8)..(8,8,16), ladder probe shrink 6/8, ladder probe distance 30/8,
step 18/10.

## Port notes (for crates/common/src/pmove.rs)

If the goal is "feel like retail 1.1", in priority order:

1. Replace `JUMP_VELOCITY = 250.0` with the stance-dependent form:
   `vz = sqrt(2 * height * gravity)`, height 34 standing / 24 crouched-prone;
   keep horizontal velocity. Gate on forwardmove != 0. This is the biggest
   behavioural divergence found.
2. Friction/accelerate constants differ from the Q3-derived ones currently in
   the file: friction 5.5 (not 6), accelerate 9 (not 10), stopspeed 100 (not
   60), plus stance-specific accelerate (prone 19, ducked 12).
3. Step-up: keep 18, but drop to 10 while PRONE.
4. Ladders, if implemented: brush surfaces with surface flag 0x8; probe =
   shrunken bbox (top lowered by probe length) pushed 30 units along horizontal
   forward (8 while sliding, -lastNormal when already on a ladder and
   airborne); store plane normal as `vLadderVec`; move with wishdir projected
   onto the ladder plane at 0.5 scale and 16.0 friction; jump-off = vz *
   sqrt(78*g) * 0.75 with horizontal := 128 * reflected dir; backward dismount
   -250/-500 along view forward; lock yaw to ±75° of the ladder yaw; play
   pb_climbup/pb_climbdown by velocity.z sign while airborne on the ladder.
   500 ms (ground jump) and 300 ms (ladder re-grab/push-off) timers on
   `ps.jumpTime` are part of the feel.
5. Mantle: do not add one. If ledge-climbing is wanted as a feature, it would be
   a vcod extension with no retail counterpart - decide its constants, don't
   dig for them in the binary; this document is the negative result.
