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

- Gated on: `pm_flags & 0x20` clear and `cmd.forwardmove != 0`. Bit 0x20 is
  the ADS (aim-down-sight) flag, not a timer: `PM_UpdateAimDownSightFlag`
  (@0x37230) sets it while the ADS button is held - unconditionally when not
  prone, behind idle checks when prone - and clears it otherwise, with a second
  unconditional clearer at 0x3ABDC. So holding ADS blocks the ground jump; no
  stance does. There is NO time comparison anywhere in this block:
  `ps.jumpTime` (ps+0x64) is written only by the ladder push-off stamp
  (@0x33964) and the steep-slope mover (@0x2F279).
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
    (composition @0x2EC7E..0x2ED36: dot of vLadderVec with the FULL pitched
    pml.forward gates reflection @0x2ECAA-0x2ECD3; the reflected vector is
    built from a flattened forward copy whose z lane is a literal zero,
    normalized in 3D @0x2ED36, then x/y alone scale by 128 @0x2ED5A - so the
    horizontal push is exactly 128 at any pitch) and vertical velocity is
    first scaled by 0.75 (0x708D0);
  - off a ladder the direction is the horizontal forward from pml.
- Clears `PMF_LADDER`, fires `EV_JUMP_*` (base 70 + surface index; hardcoded
  83 = `EV_JUMP_METAL` when leaving a ladder, 0x2ED86), adds 64 to
  `aimSpreadScale`, fires anim event JUMP (3) or JUMPBK (4) depending on the
  sign of forwardmove, then callers store `cmd.serverTime` into
  `ps.jumpTime`.

Note vcod implements the stance-dependent ground jump described above
(heights 34/24, `sqrt(2*height*gravity)`, forwardmove gate); its old flat
`JUMP_VELOCITY = 250.0` constant is gone. The ladder push-off path is not
ported; see port notes for what landed.

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
- Wall glue, misread earlier as a backwards hop: while walking on a ladder
  (gate reads `[pm+0x2C]`, the static pmove_t's walking flag @0x33CF1), the
  velocity's component along the ladder plane is stripped and then
  K * vLadderVec is ADDED with K selected between **-500.0** (@0x70CD4) and
  **-250.0** (@0x70CD8) by the SIGN of the climb-rate slot
  (`forwardmove*0.5*upscale*cmdScale + sidemove*0.2*cmdScale*right.z`, where
  right.z is compile-time zeroed @0x339c6): positive climb rate glues at
  -500, otherwise -250 (selector @0x33D2E-0x33D43, applied before the step
  slide). The constants are negative - this presses you INTO the wall while
  climbing or hanging, it never hops off.
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
| 0x20 | ADS held (blocks ground jump); set by PM_UpdateAimDownSightFlag | tested at 0x31CCB |

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

Status after the pmove work landed on this branch:

1. SHIPPED - stance-dependent ground jump: `vz = sqrt(2 * height * gravity)`,
   height 34 standing / 24 crouched-prone, forwardmove gate, horizontal
   velocity kept.
2. SHIPPED - friction 5.5, accelerate 9, stopspeed 100, stance accelerates
   12 ducked / 19 prone. One labeled deviation: vcod scales the stopspeed
   control floor by stance (flat 100 verifiably stalls prone under the Q3
   accelerate shape; retail's compensation was not recoverable from the x87
   flow).
3. SHIPPED, with a correction to this document: step height is 10 while
   PRONE - the chooser @0x35034 tests pm_flags bit 0x1, not ladder state as
   this section previously claimed. vcod implements prone.
4. SHIPPED - ladder detection/movement: brush flag 0x8, probe reach 30/8,
   probe bbox shrunk horizontally with top lowered by the probe distance,
   `-vLadderVec` stick-with-wall direction when airborne, ProjectPointOnPlane
   wishdir at 0.5 scale with 16.0 friction and command scaling, the 300 ms
   re-grab lock after a jump, the PM_Jump push-off body (`sqrt(78*g)` then
   x0.75 vertical, flattened-reflection horizontal reset to exactly 128), and
   the wall glue (negative-magnitude K*vLadderVec by climb-rate sign).
   NOT ported: the +/-75 degree yaw lock (its ps+0x7C consumer is unverified)
   and climb anim events (walk mode has no event consumer yet). vcod models
   jumpTime as a dt-advanced ms counter instead of cmd.serverTime.
5. Stands as the negative result: no mantle exists in retail 1.1. If
   ledge-climbing is wanted as a feature it would be a vcod extension with
   no retail counterpart - decide its constants, don't dig for them in the
   binary.
