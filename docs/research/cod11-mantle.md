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
| move | dispatch at 0x34305 | `pm_flags & PMF_LADDER` -> `PM_LadderMove` (0x33944); `pml.walking` (pml+0x2c) set -> `PM_WalkMove` (0x2F258); otherwise `PM_AirMove` (0x2F03C) |
| post | fns 0x30474, 0x30778 again | re-categorise |

Both movers are Q3-shaped: wishdir from cmd and yaw, wishspeed from the cmd
scale ("The wish speed" below), accelerate (`pm_accelerate` = 9.0 ground /
1.0 air), then `PM_StepSlideMove(gravity)`. An earlier read of this table
had the two movers swapped and 0x2F258 down as a steep-slope mover:
VERIFIED, 0x2F258 opens with a call to 0x2eb98 whose non-zero return goes to
0x2F03C (0x2f26a), Q3's `PM_CheckJump` then `PM_AirMove` at the top of
`PM_WalkMove`, and the dispatch tests `pml+0x2c` (0x34312), the `walking`
slot of the `pml_t` layout the rest of this module matches.

## Jumps (there are two)

**Ground jump**, inside fn 0x316F4 at 0x31CC0:

- Gated on: `pm_flags & 0x20` clear. A second gate read off this block as
  `cmd.forwardmove != 0` was a misread, and the code that ported it kept a
  player standing still from ever jumping. VERIFIED live 2026-09-01: a probe
  holding `upmove` 127 with `forwardmove` 0 against the retail server leaves
  the ground (`groundEntityNum` 1023, `velocity[2]` 197 one frame later,
  `aimSpreadScale` 255 as below), so whatever that comparison reads, it is not
  forwardmove. Bit 0x20 is
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
  0x2EBB3), not prone, not crouched (@0x2EBF5), not blocked by two higher pm_flags bits (0x800/0x2000),
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
`JUMP_VELOCITY = 250.0` constant is gone. The ladder push-off path is
ported too - port note 4 lists exactly what landed.

## Prone: the fit check, the body swing and the yaw cap

Read out of `game.mp.i386.so` on 2026-09-01 with
`tools/re/annotate_func.py`, which resolves the PIC relocations; without
them every call in this code disassembles as a placeholder and every
tunable as `ds:0xc`. Cvar defaults are the retail server's own console
output (`bg_prone_yawcap` and friends, dedicated server, stock config).

Three mechanisms, and vcod had none of them.

### The fit check, `BG_CheckProneValid` (0x2d428)

`BG_CheckProne` (0x2e358) is a 98-byte forwarder to it, and the engine
calls the pair to ask "can a body lie here facing this yaw". Signature, from
the forwarder and the call site at 0x33174:

    BG_CheckProneValid(ignoreEnt, origin, halfWidth, 30.0, yaw,
                       &outDir, &outPitchA, &outPitchB, ..., trace, mask...)

The trace function arrives as an argument (`call [ebp+0x34]`, 0x2d664) with
the Q3 argument order `trace(results, start, mins, maxs, end, ignoreEnt,
mask)`. The content mask is `0x820011`, or `0x810011` when the caller's last
flag is clear (0x2d4c2, 0x2d4d2).

The first and decisive trace (0x2d57a-0x2d664): a box of mins `(-6,-6,-6)` to
maxs `(6,6,6)` swept from the origin **54 units along yaw - 180 degrees**,
that is, straight backwards from the facing (constants at rodata 0x70174,
0x70178, 0x7017c, 0x70180). That is the space the body needs behind you when
you lie down, and it is why standing with your back to a wall refuses the
prone. Seven further traces follow, using 22, 2.5, 54, 1.5 and 5, and feed the
two `vectopitch` calls and the `AngleSubtract` at 0x2dd63-0x2ddbc, which
produce `proneDirectionPitch` (ps+0x36c) and `proneTorsoPitch` (ps+0x370) --
the pitch the body takes on sloped ground. INFERRED, from the call sequence:
those seven are ground sampling along the body rather than further clearance
tests. They are not read out field by field here, because both outputs are
animation inputs: they change how a prone body is drawn, not whether it may
lie down or where it may look.

VERIFIED live 2026-09-01, independently of the disassembly: the same input
held for the same three seconds went prone at one spawn and was refused at
another on the same map.

### The body swing and the yaw cap, `PM_UpdateViewAngles` (0x32d7c)

The prone branch runs at 0x3301f-0x33235. Tunables are cvars, read from the
retail server's console: `bg_prone_yawcap` 85, `bg_prone_softyawedge` 1,
`bg_duck2prone_time` 400, `bg_prone2duck_time` 400, `bg_viewheight_prone` 11.

    delta = AngleNormalize180(AngleDelta(viewangles[1], ps->proneDirection))

- **Soft edge** (0x33092): with `bg_prone_softyawedge` set, the body only
  starts to turn once `|delta|` passes `bg_prone_yawcap - 5`, that is 80
  degrees.
- **The swing** (0x330dd-0x3311e): the body turns toward the view at
  `pml.frametime * 55.0` per frame (rate at rodata 0x70c88), snapping the rest
  of the way when one frame would cover it. VERIFIED live: `ps.proneDirection`
  ramped 0 -> 10.12 -> 21.12 -> 30.36 over ~195 ms snapshots, which is 54
  degrees per second against the 55 in rodata.
- **The candidate is validated** (0x33181): the new direction goes through
  `BG_CheckProne` before it is taken. Valid, and `ps->proneDirection` is
  written (0x33193); invalid, and past `yawcap + 0.1` (rodata 0x70c90) it sets
  `pm_flags` bit 0x8000 instead (0x331c4). So the body only swings where it
  fits, and a blocked swing is announced rather than forced.
- **The hard cap** (0x331c8-0x33235): whatever the swing did, a `|delta|`
  past 85 degrees is pushed back into the cone by adding the excess to
  `ps->delta_angles[1]`, in ANGLE2SHORT units (rodata 0x70c7c = 182.0444):

      ps->delta_angles[1] += ANGLE2SHORT(delta - copysign(yawcap, delta))

  VERIFIED live: a probe holding a yaw 150 degrees off its prone direction had
  `delta_angles[1]` rewritten by the server every frame, while one holding 60
  degrees off did not.

`ps->proneDirection` is ps+0x368 internally, wire offset 872. The only two
writers in the module are this function and the static prone-entry routine
around 0x31c60, which also writes both pitch fields through
`PitchForYawOnNormal` and `AngleDelta`.

### Why it matters to a server

All three are `bg_`/`PM_` code, so a retail client predicts them. A server
that does none of them disagrees with its own clients every frame a player is
prone: the client swings the body and clamps the view locally, the server's
snapshot says otherwise, and the view is yanked on going prone and blocked
when crawling. That is the shape of the bug this section was written for.

Because the refusal depends on where a player spawns, the A/B gate
`crates/server/tests/playerstate_motion_ab.rs` skips a prone pose retail
refused rather than pinning it, and fails if a capture refuses every one.

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
- Wall glue, misread earlier as a backwards hop: while NOT walking - i.e.
  airborne on the ladder (the gate at @0x33CF1 runs it only when the static
  pml_t's walking flag is zero), the
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

### The ground snap

`PM_StepSlideMove` is not Q3's. Q3 and RTCW return the moment `PM_SlideMove`
reports the move went through unobstructed, and their push-down pass only
undoes the step-up they just took. CoD's runs a down pass on every grounded
frame and reaches past the step.

VERIFIED: at 0x350EC, on the path taken when `PM_SlideMove` returned 0, the
function compares `ps->groundEntityNum` (ps+0x54) against 0x3FF and jumps into
the body at 0x35116 when the two differ; the equal case returns unless
`pm_flags & 0x10` is set and `velocity[2] > 0`. INFERRED: a player standing on
something therefore takes the down pass whether or not its move was blocked,
and an airborne one takes it only on a ladder going up.

VERIFIED: the push-down point is built at 0x352D8 as `origin[2] - stepUp`,
where `stepUp` is the up-trace's fraction times `stepSize + 1`, and at 0x352E8
a further `stepSize * 0.5` (0x70EF8 holds 0.5) is subtracted from it when the
flag at pml+0x30 is set and `pm_flags & 0x10` is clear (0x34FDF-0x34FF1).
INFERRED: pml+0x30 is `groundPlane`, from its position in the Q3 `pml_t`
layout the rest of this module matches, so the extra half step is taken for a
player on a ground plane that is not on a ladder.

VERIFIED: at 0x35380 the trace fraction is compared against 1.0; the
`fraction < 1.0` arm writes `trace.endpos` into the origin and calls
`PM_ClipVelocity` with the 1.001 overclip at 0x70EFC, and the other arm
(0x353D0) subtracts `stepUp` from `origin[2]` and nothing else. INFERRED: so
the extra half step pulls the player onto whatever is under it and is not
itself a fall.

An earlier read of this file inferred from the reach that the down pass is
what keeps a walker on the ground at a crest. It is not, and vcod snapped a
walker onto every floor within 9 units on every grounded frame on that
reading until 2026-09-07. VERIFIED: at 0x35166 the slide's return value is
tested and an unobstructed slide jumps to 0x352a0, past the up trace and
the second slide, so an unobstructed grounded frame runs only the down
trace (0x352c0-0x35335, with `stepUp` still 0) and then the revert test at
0x353f7 below, which compares the down pass's result against the plain
slide's along the velocity in x and y, equal by construction, and restores
the plain slide's origin and velocity. VERIFIED by capture
(`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-run3.txt`,
`commandTime` 256091): walking off a 4-degree grade onto a floor 8 units
lower, retail keeps `origin[2]` -23.875 for that cmd and falls under gravity
over the next three snapshots (-24.83, -27.69, -32.45), where vcod's
unconditional pass put it 8 units down in one 9 ms cmd, on the ground and
with the step's speed scale applied. INFERRED: the down pass therefore
survives only after a step-up that gained horizontal distance, which is
what the `stepUp + 9` reach is for, and a walker on a crest at a
high-fps client's cmd rate stays grounded because its rise per cmd is under
the 0.25 units of the ground trace, not because anything pulls it down.

VERIFIED: between the second `PM_SlideMove` call (0x35296) and the down trace
(0x35335) the function tests two things and nothing else: the flag in `ebx`
at 0x352a4 and `stepUp` against zero at 0x352a8-0x352ba (`fldz`, `fld
[ebp-0xc0]`, `fucompp`), skipping to 0x353f7 only when both are clear.
INFERRED: the velocity is never read on that path, so a grounded frame takes
the down pass whatever its velocity is, including one a wall or a seam bevel
just redirected into the ground plane. vcod used to re-run the ground
trace's kickoff test (`velocity.z > 0 && velocity . normal > 10`) against the
current velocity before it snapped, and a walker rubbing a wall while
climbing a slope, whose slide keeps the climb's upward component and loses
the forward one, failed that test every frame: the snap was refused, the
ground trace after the move read the same test and dropped the player, and
the sight ramp reversed with it. That gate is gone with the unconditional
pass; what is left of it is the waterjump exclusion (its launch is set
inside the move and the down pass's clip would take it away).

VERIFIED: at 0x3533a a 16-bit word at +0x28 of the down trace is compared
against 0x3f, and the at-or-below arm (0x35341-0x35377) writes the origin
and velocity saved at 0x35116 back over the playerstate and returns.
INFERRED: by its range that word is the hit entity number, so a down pass
that lands on a client is undone whole, and the tail never runs for it.

VERIFIED: at 0x353f7-0x3544e the function compares
`v . (down_o - start_o) + 0.001` (0x70ef4) against `v . (origin - start_o)`,
with `v` the velocity as it stands after the down pass and both
displacements taken in x and y (`down_o - start_o` is formed at
0x35146-0x35160, right after the slots are saved), and the greater-former
arm (0x3546e-0x35499) writes `down_o` and `down_v` back over the
playerstate. INFERRED: this is the "did the flat slide get further" test,
measured along the velocity rather than by length, and what it restores is
the flat slide's state from before any down pass, so a reverted step ends
the move unsnapped and the ground trace after it decides; and since the
0.001 is on the plain slide's side, a step whose slide got exactly as far as
the plain one is reverted too. vcod's `step_slide_move` runs the same
test on every frame now, with `STEP_REVERT_EPS` the 0.001; until 2026-09-07
it compared squared horizontal lengths, only after a step-up, and let an
unobstructed frame keep its down pass.

### What the collider does to a walker on a terrain seam

Everything under this heading is a measurement of vcod, not of retail:
retail's terrain collision is not read, and the numbers are from
`crates/common/src/collision.rs` walking `mp_carentan`'s own mesh.

VERIFIED: a box resting on one facet of a terrain mesh is inside the
neighbouring facet's box-expanded slab wherever the two meet at a convex
seam, by half the box width times the grade change, and beside an 8-unit
kerb it is inside the kerb top's slab. The soup clip used to read a start
inside a slab as `allsolid` the way `CM_TraceThroughBrush` reads a start
inside a brush, so the 0.25-unit ground trace failed on the seam and the
9-unit snap did nothing there. Q3's patch facets never report a start as
solid (`CM_TraceThroughPatchCollide`, `cm_patch.c`), and the soup clip now
does the same: a start within 8 units behind a face is a fraction-0 contact
with that face, deeper than that the triangle does not clip the trace.

VERIFIED: a box touching two surfaces clips both at fraction 0, and the one
reported used to be whichever the BVH walk reached first, so the ground trace
and the snap trace at the same origin could name different normals; the
kickoff test then read a seam bevel's normal where the snap had clipped the
velocity into the facet's. The contact reported is now the one with the
largest unclamped enter fraction, which the two traces agree on.

VERIFIED: walked offline at 8 ms with the sight held from 300 sloped spots
around `mp_carentan`'s deathmatch spawns, up and down each, the walks that
left the ground with walkable floor inside 18 units below went from 101 to
53 of 600, and every remaining one is a ledge, a prop or a fall of more than
the snap's 9 units. VERIFIED: the same walks at 16 and 25 ms count the same
way, so the cmd rate is not the trigger. Those counts were taken with the
box sweep this section describes; the capsule ("The player is a capsule")
replaced it the same day and the seam rules above still apply to it. VERIFIED: `--probe-slope` against
the retail server on open ground reads `groundEntityNum` 1022 on all 1464
snapshots and no sight reversal at 8 ms and at 25 ms, and against a
staircase at 25 ms reads 27 airborne snapshots and 13 reversals, so retail
itself drops a walker pushing against steps.

### The player is a capsule

Where retail's origin sits on a grade, and why vcod's sat a unit higher.
Found by replaying retail's own cmd stream on vcod's mover from retail's
own state at every snapshot (`crates/server/tests/playerstate_slope_ab.rs`,
the rebased run): on the 3.3-degree street at mp_carentan (799, 1800-1900)
ours landed 1.08 units above retail on every snapshot, on the 4-degree grade
at (1190-1218, 2047-2073) 1.07, at the kerb at (1672, 928) retail stepped
down with its centre 15 units past the kerb's straight edge where ours kept
its corner on the kerb's diagonal edge for 7 more, and along the diagonal
wall brush at (-568, 2088)-(-444, 1964) retail slid with its centre 15.1 to
16.3 from the face where a box's corner would have been 5 units inside it.

VERIFIED: `ClientThink_real` (game.mp.i386.so) stores `trap_TraceCapsule`
into the pmove's three trace slots, `pm+0xe8`, `pm+0xec` and `pm+0xf0`
(relocations at 0x400b1, 0x400b8 and 0x400bf), and `trap_PointContents`
into `pm+0xf4` (0x400c6); `PM_StepSlideMove` calls `pm+0xe8` (0x3532f). So
every trace the mover makes is a capsule trace, against brushes and terrain
alike. VERIFIED: the tracemask it stores at `pm+0x34` (0x400a4) is
0x2810011 for `pm_type` 5 or less and 0x810011 above it (0x40098).
VERIFIED: `cod_lnxded` carries the string `^1Box collision on terrain
currently being faked with capsule collision` at 0x85320, between
`CM_GenerateTerrainCollide`'s error strings (0x85201-0x852c1) and
`CM_ChangeAreaPortalState`'s. INFERRED: the shape is Q3's
(`CM_TestBoundingBoxInCapsule`, `cm_trace.c`): radius the smaller of the
half width and half height, sphere centres `halfheight - radius` above and
below the box centre, every plane pushed out by the radius and tested
against the sphere nearest it. For the player box that is a radius of 15
with spheres 15 and 55 above the feet.

VERIFIED by capture, against that shape: a capsule of radius `r` rests on a
plane of normal `n` with its feet at `h + r (1 / n.z - 1)` above the point
height `h`, which is 0.045 on the 4-degree grade; retail's origins there
match vcod's *point* trace of the same mesh to within 0.03 (y 1899.57:
retail -35.76, point -35.759; y 1840.50: retail -38.44, point -38.46), and
a box rests at `h + r tan`, the 1.05 vcod carried. At the kerb the capsule's
footprint releases the straight edge with the centre 15 past it and is
16.3 from the diagonal edge there, so it drops where retail did (the
`run4` capture, `commandTime` 285441, `EV_STEP_VIEW` -7 at x 1656-1661);
a box's corner holds the diagonal until the centre is 21.2 past the straight
edge, x 1650, which is where vcod dropped.

vcod: `collision.rs` sweeps every brush as that capsule, `Capsule::of` on
the trace box, with its planes pushed out by the radius, Q3's sphere arm of
`CM_TraceThroughBrush`; the terrain, the patches and the static models each
take the shape retail gives them, below. A zero box is still a point, so
the bullet sweep is unchanged.

What the replay measured, rebased on retail's state at every snapshot
(`playerstate_slope_ab.rs`, the two committed captures, 1463 and 1469
snapshots). VERIFIED: with the box sweep, the 190 wish speed and the
unconditional down pass, the 8 ms capture read |dz| up to 8.0 and 1.08 on
every snapshot of the 3.3-degree street, dxy up to 5.9, with 9 ground
disagreements; with the wish speed fixed and the capsule on every prim as
a Q3 facet polyhedron, |dz| read p50 0, p95 0.006, p99 0.10, max 0.96 (the
foot of a kerb ramp at (1241, 2069)), dxy p50 0.02, p95 0.14, max 5.9,
and the 25 ms capture |dz| p95 0 with one 18.5 row, dxy p95 0.30, max 4.5,
69 ground disagreements. VERIFIED, after the three rules below went in:
the 8 ms capture reads |dz| p95 0.005, p99 0.026, max 0.129, dxy p95
0.130, p99 0.178, max 2.9, 2 rows past a unit; the 25 ms one |dz| 0 on
every row, dxy p95 0.146, p99 0.449, max 4.5, 12 rows past a unit; no
ground disagreement on either. Retail's own noise floor is the integer
truncation of the velocity it sends, under 0.05 per interval. The 4.5 row
is one 50 ms interval of motion at 90 units/s beside a diagonal post brush
at (-776, 2000), a cmd's worth, and is not chased.

### Static models are clipped as a segment

The 18.5 row of the 25 ms capture: at `commandTime` 1195225 retail stands
at (-750.3, 1778.1) against the `clip_nosight` brushes around a desk and a
chair (`plainchair` at (-730, 1757), `desk_rolltop` at (-750, 1737)),
where vcod's capsule stepped 18.6 up the chair's collision mesh.

VERIFIED: `cod_lnxded`'s `CM_LoadMap` parses every `misc_model` block of
the entity string itself (the `classname`/`misc_model`/`model`/`origin`/
`angles`/`modelscale_vec`/`modelscale` keys at 0x80cd1a7-0x80cd1e8, the
parser at 0x80515d4), registers each xmodel (0x8051420) and links the ones
whose descriptor has collision into a 2D tree (0x80594c4) keyed by the OR
of their collision surfaces' `contents` (the loader ORs each surface's
word, masked `& 0xdfff7ffb`, into the model at +0x48; 0x80c2b48 reads it).
VERIFIED: `SV_Trace` (0x80916f4) walks that tree only when its last
argument is set (0x805a58c from 0x80916f4, gated on `param_11`), and the
game syscall dispatcher (0x8088333) sets it for case 0x2b alone, the
`trap_LocationalTrace` wrapper (game.mp 0x73948); cases 0x22 `trap_Trace`
and 0x23 `trap_TraceCapsule` pass 0. So the player's moves, which
`ClientThink_real` runs through `trap_TraceCapsule`, never meet a static
model, and a prop is solid to a player only where a mapper wrapped it in
clip brushes, which is what the desk and chair have. VERIFIED: the static
walk takes the trace's `start` and `end` alone (0x805a58c copies the two
points and the mask, no bounds), transforms them into the model's frame
(0x8051984) and intersects the segment with each collision surface whose
`contents & mask` is set (0x80c203c): one-sided (`start` on or ahead of the
plane, `end` behind), fraction `(d_start - 0.125) / (d_start - d_end)`
(rodata 0x80db970), the crossing inside the two edge planes within -0.001
and 1.001 (0x80db974, 0x80db978), the hit carrying the surface's
`surf_flags` and `contents`. INFERRED: the fraction is not clamped, and
pmove reads a negative one as zero since it moves the origin only for a
positive one.

VERIFIED: `Bullet_Fire_Extended` (game.mp 0x78890) traces with mask
0x2802031 and `CanDamage` (0x5a098) with 0x2802091, both through
`trap_LocationalTrace`, so a bullet and a blast see the props; `G_RunMissile`'s move trace is the same
syscall (`cod11-combat.md` 12.1) with the missile's `clipmask`, 0x11 when
unset, and the retail capture rests a frag on a crate stack's own mesh
(`crates/server/tests/missile_ab.rs`). VERIFIED by census over the stock
maps: brush content bit 0x10 is glass (`glass@brokenwindow`,
`dam_window`: 0x2090, 0x8000010, 0x28000010), so a player and a bullet both
stop at a window pane and a `misc_model` lamp's glass surface (contents
0x10) stops a player's bullet, not a player.

vcod: `props::collision_tris` hands every collision surface with non-zero
`contents` to the world as `ModelTri`s; `CollisionWorld::shot_trace` and
`missile_trace` clip them by the bare segment with those epsilons and
masks, `box_trace` never does. The bounce parm a prop carries is now the
surface's own (21 on the crate stack), which `missile_ab` compares.

### Terrain is a swept sphere, a patch is a facet

The kerb-ramp foot at (1241, 2069) and a street row at (799, 2075): the
facet polyhedron lifted the capsule onto a flat's radius-wide slab a full
radius before the seam where retail's rose along the ramp; the pavement
kerb at (1059, 2445): retail stepped up the 8-unit kerb between x 1055.6
and 1059.1, 13 to 16 units before the edge at 1072, and stood at -23.875
with its centre 12.9 past the edge, where a swept sphere sags 7 units.

VERIFIED: lump 24 is two kinds of record (`CM_LoadMap`'s loop at
0x804b010): a byte 2 of 0 is a bezier patch (`u16 width`, `u16 height` at
+4/+6, `u32 first_vert` at +12 into lump 25) handed to
`CM_GeneratePatchCollide` (0x804dfb4), anything else a terrain partition
(`u16 vert_count`, `u16 index_count`, `u32 first_vert` at +8, `u32
first_index` at +12 into lump 26, indices relative to `first_vert`) handed
to `CM_GenerateTerrainCollide` (0x8051b30); either takes its contents from
its material. mp_carentan carries 568 terrain and 534 patch records, and
the render soups draw both. VERIFIED: the kerb wall at x 1072 is a 3x3
patch of `concrete@stalin1` (record 1048; 1007 and 993/994 are the walls at
the other two spots), and the pavement and the ramp are terrain (440, 452,
486, 511).

VERIFIED: the terrain clip (0x8052a58, reached from 0x80536c8 for a box or
a capsule, a box being "faked with capsule collision", the 0x80cd320
warning) sweeps one sphere per triangle: the lower one (start and end
shifted down by `halfheight - radius`) unless the partition's flag byte
says its faces all point down, then the upper. Against the face:
`d_end < radius + 0.125` (0x80cd308) and moving in; a start deeper than
that behind the face is solid only where the segment to the other sphere
crosses the triangle; else `f = (d_start - r - 0.125) / (d_start -
d_end)`, 0 when the start is already within, and the contact point's
barycentrics decide: inside, a face hit; outside, the vertices the point
is beyond are swept as spheres and the edges as cylinders (the edge frame
records at +0x40, the vertex records at +0x34, built by 0x8051b30, which
drops the edges shared by two triangles within its coplanarity tolerance
at 0x80cd2ec). Every fraction loses 1e-5 (0x80cd30c) and one at or under
that reads `startsolid` at 0. There is no bevel plane anywhere in it. The
point arm (0x8052894) is the static-model clip's: front face, 0.125 back
along the segment, barycentrics within -0.001 and 1.001 (0x80cd2f8-0x300).
INFERRED: the flag byte is 1 when the partition has a downward face and
no upward one (0x8051b30's tail: `bVar7 && !upward`), so a floor takes
the lower sphere and a ceiling the upper, which is the sphere nearest the
plane.

VERIFIED: a patch goes through `CM_TraceThroughPatchCollide` (0x804e944),
Q3's facet walk, and Q3's `CM_AddFacetBevels` (`cm_patch.c`) gives a
facet its surface plane, a perpendicular border per edge, the six axial
bevels and the edge-by-axis bevels, every one pushed out by the sphere's
radius in the trace. That is the box-like reach: a flat 3x3 kerb-wall
patch is one facet whose expanded top bevel holds the sphere at the full
kerb height 12.9 units past the edge, and its expanded side plane is what
the walking sphere hit at x 1057, where retail's step-up happened.

vcod: `CollisionWorld::build` takes the terrain triangles from lumps 24-26
(wound `cross(c - a, b - a)` out, Q3's `PlaneFromPoints`) and clips them
with the sphere sweep above (`clip_sphere_triangle`); the render soups of
model 0 stay facets under the polyhedron clip (`triangle_planes`), minus
the ones that draw terrain, matched as a soup whose centroid lies inside a
coplanar terrain triangle sharing one of its vertices (an edge match is
not enough: the render mesh triangulates the grid the other way, and a
flat patch abutting terrain shares an edge with it). A patch's bezier
tessellation is not built; its render soup stands in, which is exact for
the flat patches every kerb wall on carentan is. The `startsolid` a
terrain touch reads is kept, since retail's stance and prone checks read
the same flag off the same trace.

### A submodel's brushes are its entity's

The 5.9 dxy rows of the 8 ms capture, at (1758-1790, 2085-2118): retail
walks through the `clip_metal` brushes 4286-4294 of model 6, a
`script_brushmodel` with `script_gameobjectname bombzone` around the flak
gun, which vcod's world held as solid.

VERIFIED: `SP_script_brushmodel` (game.mp 0x60fb8) sets the brush model
and links the entity, and nothing else puts a submodel's brushes into the
clip; the world clip is model 0 (`CM_LoadMap` loads the submodels as
inline models the entity code links). VERIFIED: `dm.gsc:78` and `tdm.gsc`
hand `_gameobjects::main` an `allowed` list of their own name, `sd.gsc:123`
adds `bombzone` and `blocker`, and `_gameobjects.gsc` `delete()`s every
entity with a `script_gameobjectname` outside the list, so under `dm` both
of carentan's bombzone brush models (5 and 6) are gone before the first
client walks. The `clip_metal` box at (-196..-139, 2496..2606) the earlier
reading listed is model 5, the other bombzone. VERIFIED: the `clip_nosight`
world brush 4254 the same reading put retail inside is a sloped roof clip
whose bounding box holds the point and whose planes do not (side 9 puts
(-597, 1832, 144) 26 units outside); it was a bounding-box reading.

`delete()` is not the only script that takes a submodel out. VERIFIED:
`SP_script_brushmodel` is four calls and one store -- `trap_SetBrushModel`,
`InitScriptMover`, `self+0x118 = 1`, `trap_LinkEntity` -- with no other
instruction in the function, and `+0x118` is `r.contents`
(`docs/research/cod11-combat.md`, section 13). INFERRED, from that being
the whole body: the spawn function reads no `script_exploder` key and gives
an exploder brush model no state of its own, so every `script_brushmodel`
spawns solid and linked whatever keys it carries.

VERIFIED: `maps/mp/_load.gsc::main` is what takes an exploder's brushes
out. It walks every `script_brushmodel` and `script_model`, and for one
with `script_exploder` defined and `targetname` `exploder` or
`exploderchunk` it calls `hide()` and then, when the classname is not
`script_model`, `notsolid()`. VERIFIED from the entity lumps of every
`maps/MP/*.bsp` in the paks: four `script_brushmodel`s reach that arm --
mp_depot `*1`, mp_powcamp `*3` and `*9`, mp_rocket `*3` -- out of 38
`script_brushmodel`s in all.

vcod: `BrushPlanes` carries its model, `CollisionWorld::set_model_linked`
drops a model's brushes out of every trace, the `delete`, `solid` and
`notSolid` builtins call it for an entity whose `model` is a `*N`, and
`playerstate_slope_ab.rs` applies the `_gameobjects` rule itself since the
replay runs no script. mp_depot's `*1` is both an exploder and a
`bombzone`, so under every gametype but `sd` it is unlinked twice over: by
`_load.gsc`'s `notsolid()` and by `_gameobjects::main`'s `delete()`.

### The step event and the velocity scale

The tail of `PM_StepSlideMove`, 0x35659 to 0x35799. Both halves are in
`crates/common/src/pmove.rs` (`step_view`).

VERIFIED: 0x8F is `EV_STEP_VIEW`, index 143 of the event-name table in
`cgame_mp_x86.dll` (.data 0x30077040).

VERIFIED: the relocation at 0x35752 names the callee
`BG_AddPredictableEventToPlayerstate`, and the three pushes ahead of it
(0x3574c, 0x3574b, 0x3574a) are the event id 0x8F, the parm and `pm->ps`.

VERIFIED: retail puts the event on the wire on a plain walk. The hit capture
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-hit-shooter.txt`
carries `events=143` with parms 121, 132, 136, 137 and 143, which under the
+128 bias below are steps of -7, +4, +8, +9 and +15 units.

VERIFIED: the constants. The threshold at 0x35697 is the double 0.5 at
0x70f08; the rounding bias at 0x356af is the float 0.5 at 0x70ef8, taken with
the x87 rounding mode forced to truncate toward zero (`or ax,0xc00` @0x356c2);
the clamp at 0x35727-0x35738 is -16..24; the parm bias at 0x35742 is +128; the
scale factors at 0x35770 and 0x35776 are 0.8 (0x70f10) and 0.2 (0x70f14), and
the multiply at 0x3577c-0x35799 covers all three velocity components.

VERIFIED: the two reference heights are different slots. The event's step is
measured against `down_o`, the origin stored at 0x35116 right after
`PM_SlideMove` and before any step-up (loaded at 0x3568d); the velocity scale
is measured against `start_o`, the origin stored at 0x34ff4 on entry (loaded at
0x35761). Nothing writes either slot in between.

INFERRED, from the control flow of 0x35659-0x35799: the tail runs when
`ps->pm_type` is 5 or less (0x35660) and `PM_VerifyPronePosition` returned
non-zero (0x3567f), and raises nothing when the step is half a unit or less or
when the rounded step is 0 (0x356e9). The parm is that rounded step clamped and
biased. The scale, applied after the event, is
`0.2 + 0.8 * (1 - |origin[2] - start_o[2]| / stepSize)` with no clamp: a full
18-unit step costs four fifths of the frame's speed, a step of nothing costs
none.

INFERRED, from the control flow of 0x35116-0x35659: every arm past the entry
gate reaches the tail - the step-up, the down pass a grounded frame takes
without one, and a step-up the "did the flat slide get further" test reverted.
That is why the retail capture's parms run negative: the ground snap alone
raises the event.

INFERRED, from the control flow of 0x35057-0x35112: the entry gate lets an
airborne player through only on the jump-origin allowance. A blocked move whose
`fJumpOriginZ` is inside +/-0.001 rejoins the unblocked path at 0x350e5 and
takes the same `groundEntityNum` test, so with that field 0 an airborne frame
returns before the step-up. vcod does not model `fJumpOriginZ` and its gate
lets a blocked airborne move step, which is a divergence in the step; the
event and the scale are held to retail's reachable set by an explicit
on-ground test in `step_slide_move`, since announcing a step retail never
takes would put a view jolt on a client that retail's would not.

VERIFIED: `PM_VerifyPronePosition` (0x346e0) returns 1 when `pm_flags & 1` is
clear. INFERRED, from its control flow: with the bit set it runs the prone fit
check and, on a refusal, writes its two arguments back over `ps->origin` and
`ps->velocity` and returns 0, so a prone step that does not fit is undone and
neither half of the tail runs. vcod does not port that revert - it runs no
prone fit check inside the move.

Not ported, and not read past its shape: past 0x3579c a third block, gated on
a step of more than 3 units and on being on the ground, scales
`min(|step| / 2, 4)` by the 1.25 at 0x70f18.

**The consumer.** VERIFIED: `cgame_mp_x86.dll` handles 143 inside the event
switch of the function at 0x3001dc10, which reads and writes two floats at
.bss 0x302094dc and 0x302094e0 (Q3's `cg.stepChange` and `cg.stepTime`) and the
constants 24.0 (0x3006940c), -16.0 (0x30069788), 0.01 (0x300693f4) and 0.9
(0x300695ec). VERIFIED: the function at 0x30032860 reads the same pair and the
0.01 and subtracts from the view origin at 0x3020959c.

INFERRED, from the control flow of both: the handler carries any unfinished
smoothing forward as
`(100 - (cg.time - cg.stepTime)) * cg.stepChange * 0.01 * 0.9` while the
previous step is still inside its 100 ms window and 0 otherwise, adds
`parm - 128`, clamps the sum to -16..24 and stamps `cg.stepTime`; the function
at 0x30032860 then lowers the eye by
`(100 - (cg.time - cg.stepTime)) * cg.stepChange * 0.01` for those 100 ms.
That is Q3's `CG_StepOffset` with `STEP_TIME` 100, one event carrying the size
where Q3 has `EV_STEP_4..16`, and a 0.9 decay on the carry-forward Q3 does not
have. INFERRED: the arm is gated on the event's entity being the local client,
so the smoothing is the predicting client's own view and nothing else's.

## The wish speed

What a cmd asks the mover for, per frame. Found by replaying retail's own
cmd stream on vcod's mover (`crates/server/tests/playerstate_slope_ab.rs`):
the sight-held route walk of every `--save-slope` capture settles at 63 per
axis on a diagonal, 89.7 units/s, where vcod wished 190, and the ADS
capture's plain walk at 224 (`mp_carentan-dm-ads.txt`, `walk`: velocity
163, 154). Neither is `g_speed`.

VERIFIED: `PM_WalkMove` (0x2f258) calls the cmd scale at 0x2e690 with a
copy of the cmd (0x2f2bf), and `PM_AirMove` (0x2f03c) calls the one at
0x2e5bc (0x2f083). The two are different functions.

The walk scale, 0x2e690. VERIFIED, each a read of the instruction named:

- `forwardmove` (cmd+0x14) below zero is multiplied by `ps.backSpeedScale`
  (ps+0x360, 0x2e6b4) and `rightmove` (cmd+0x15) by `ps.strafeSpeedScale`
  (ps+0x35c, 0x2e6e8), both then `fabs`, and the larger is the `max`; a
  zero `max` returns 0 (0x2e712).
- `total` is `sqrt(forwardmove^2 + rightmove^2)` over the raw bytes
  (0x2e717-0x2e72d), and the scale is `ps.speed` (ps+0x44) times `max` over
  `127.0` (0x70884) times `total` (0x2e732-0x2e746), Q3's `PM_CmdScale`.
- `pm_flags` 0x80 set (the byte at ps+0xc read signed, 0x2e748) multiplies
  by `ps.walkSpeedScale` (ps+0x34c); clear multiplies by `ps.runSpeedScale`
  (ps+0x350) and then, when `ps.leanf` (ps+0x40) is non-zero, by
  `ps.leanSpeedScale` (ps+0x364) (0x2e75a-0x2e771).
- `pm_type` 2 multiplies by 3.0 (0x70888) and 3 by 6.0 (0x7088c), and both
  skip the rest (0x2e781-0x2e79c).
- The stance block (0x2e7a1-0x2e8d2) multiplies by `ps.proneSpeedScale`
  (ps+0x354) or `ps.crouchSpeedScale` (ps+0x358), and across an eye lerp
  between the prone and crouch heights (`ps.viewHeightLerpTarget` at ps+0xd8
  against ps+0x33c and ps+0x340, the fraction from 0x308cc) by the two
  blended by the fraction.
- `pm->waterlevel` (pm+0xd9, the byte fn 0x30778 writes 0 to 3 at
  0x30786-0x308b9) non-zero multiplies by `1 - waterlevel / 3.0 * 0.5`
  (0x70888, 0x70890; 0x2e8d7-0x2e8fd).
- `ps.weapon` (ps+0xb0) non-zero looks up `BG_GetInfoForWeapon` and, when
  the float at info+0x214 is above zero, multiplies by it (0x2e906-0x2e953).
  INFERRED: that field is the weapon file's `moveSpeedScale`, from the
  numbers alone: 190 * 1.18 (the carbine's value) is the 224 the ADS capture
  walks at, and 190 * 1.18 * 0.4 is the 89.7 the slope captures walk at.
- `wbuttons` bit 0x4 (cmd+0x5, 0x2e959) multiplies by 0.4 (0x70894). No
  measured key sets it (`docs/protocol-1.1.md`, "Usercmd input bits") and
  vcod does not port it.

The air scale, 0x2e5bc. VERIFIED: `max` is the largest of `|forwardmove|`,
`|rightmove|` and `|upmove|` (cmd+0x14..0x16, 0x2e5c7-0x2e601), `total` is
the root of the three squares (0x2e610-0x2e627), the scale is `ps.speed *
max / (127.0 * total)` (0x2e648-0x2e654, 0x70878), then the same
`pm_flags` 0x80 pick between `walkSpeedScale` and `runSpeedScale`
(0x2e656-0x2e664) and the same `pm_type` 2 and 3 factors, and nothing
else. INFERRED: a crouched or prone client's held `upmove` of -127 dilutes
its airborne wish the way a held jump does, since the byte enters `total`
unsigned.

The walk flag. VERIFIED: `PM_UpdatePlayerWalkingFlag` (0x33694) clears
`pm_flags` 0x80 first (0x3369d) and sets it (0x336dd) when `pm_type` is 5 or
less, `cmd.buttons` has 0x10 (pm+0x8), `pm_flags` bit 0x1 (prone) is clear,
`pm_flags` 0x20 (the ADS flag) is set and `weaponstate` (ps+0xb4) is none of
5 to 9. VERIFIED: in the `PM_NORMAL` arm of `PmoveSingle` it runs at
0x342d8, after `PM_UpdateAimDownSightFlag` (0x342d3) and before the ladder
probe and the move dispatch. INFERRED: 0x80 is therefore the ADS walk, the
sight held by a standing or crouched player who is not reloading, and
`walkSpeedScale` (0.4 on the wire) is what slows a sighted walk to 0.4 of
the run. Open: the ADS capture's snapshots carry the bit (`ads_in`,
`ads_walk` read `pm_flags` 0x400a0) while every `--save-slope` snapshot at
8, 16 and 25 ms reads 0x40020 with the sight held and the walk at 0.4, so
something between the move and the snapshot clears it in that run shape and
not the other; not chased, since a client recomputes the flag in its own
`PmoveSingle` and the speed says the server had it set.

VERIFIED: the wire playerstate carries the seven scales, and retail's spawn
sends `walkSpeedScale` 0.4, `runSpeedScale` 1.0, `proneSpeedScale` 0.15,
`crouchSpeedScale` 0.65, `strafeSpeedScale` 0.8, `backSpeedScale` 0.7 and
`leanSpeedScale` 0.4 (`crates/server/tests/fixtures/playerstate/
mp_carentan-dm.txt`; offsets 844 to 868 in the `playerStateFields` table at
cod_lnxded 0x80d229c). VERIFIED against the motion capture: `run_back` at
44 per axis and `strafe_right` at 50, 52 are 224 * 0.4 * 0.7 and 224 * 0.4
* 0.8 with `leanf` stuck at -1 from the earlier lean poses, and `crouch_run`
at 40, 42 is 224 * 0.4 * 0.65.

vcod: `pmove::wish` and `pmove::wish_air` port the two, with
`PlayerState::walking` as the flag and `WeaponDef::move_speed_scale` as the
weapon factor. Not ported: the crouch-to-prone blend across the eye lerp
and the `wbuttons` 0x4 factor.

## State reference (observed pm_flags bits, internal ps+0xC)

| bit | meaning | evidence |
|---|---|---|
| 0x01 | PRONE | tested everywhere stance matters (e.g. 0x3455A) |
| 0x02 | CROUCH | 0x34590 pattern |
| 0x04 | set by the ground-jump gate; read by viewheight-lerp timing | 0x31CD5, 0x345C9 |
| 0x10 | LADDER | set at 0x33937, cleared at 0x3377C/0x2ED7A |
| 0x80 | affects ladder-anim speed-scale choice | 0x323D7 |
| 0x20 | ADS held (blocks ground jump); set/cleared by PM_UpdateAimDownSightFlag | set @0x372A4/0x372B7, clear @0x3ABDC, tested at 0x31CCB |

(`PMF_PRONE`, `PMF_CROUCH`, `PMF_LADDER`, `PMF_SLIDING=0x100` agree with
CoDExtended's `shared.h`; bits 0x04/0x80/0x800/0x2000 are INFERRED from
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
5. SHIPPED - the ground snap of "Step-up and steep slopes": the down pass
   runs on every grounded frame and reaches `stepSize * 0.5` past the step it
   took. SHIPPED - `EV_STEP_VIEW` (143) with the rounded, clamped and biased
   step as its parm, and the post-step velocity scale, both under "The step
   event and the velocity scale". NOT ported from that tail: the
   `PM_VerifyPronePosition` revert that gates it (vcod runs no prone fit check
   inside the move) and the third block past 0x3579c. The snap's gate is
   retail's, the ground state taken before the move, with one exclusion: a
   waterjump in progress, whose launch is set inside the move and which the
   snap's clip would take away. The velocity re-test that used to stand in
   for that exclusion refused the snap to every walker rubbing a wall on a
   slope ("The ground snap"). The airborne arm's one exception
   (`pm_flags & 0x10` with `velocity[2] > 0`, 0x350F5-0x35112) is not taken
   either: vcod returns for every airborne player. That is a no-op today,
   since the snap is 0 on a ladder anyway, and it would only matter if the
   step-up half were ever wanted on a climb. The two collider artefacts that
   dropped a walker at terrain seams and beside kerbs, and their fixes, are
   under "What the collider does to a walker on a terrain seam".
6. Stands as the negative result: no mantle exists in retail 1.1. If
   ledge-climbing is wanted as a feature it would be a vcod extension with
   no retail counterpart - decide its constants, don't dig for them in the
   binary.
