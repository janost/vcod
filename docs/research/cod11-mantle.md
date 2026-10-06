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
  inspected. They implement: `PM_CorrectAllSolid` (fn at 0x30214, see "A start
  inside a solid"), ground trace +
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
| | fn 0x316F4 | spectator bbox setup, stance transitions, viewheight lerp, the prone dive (read as the ground jump until 2026-09-27, "Jumps"), ground snap |
| | fn 0x30474 | ground trace, walking/steep-slope categorisation, landing events |
| | `PM_UpdateAimDownSightFlag` etc. | weapon/stance updates, then `PM_UpdatePronePitch` (0x342dd) |
| | fn 0x336E8 | **ladder probe**, sets/clears `PMF_LADDER` + `vLadderVec` |
| move | dispatch at 0x34305 | `pm_flags & PMF_LADDER` -> `PM_LadderMove` (0x33944); `pml.walking` (pml+0x2c) set -> `PM_WalkMove` (0x2F258); otherwise `PM_AirMove` (0x2F03C) |
| post | fns 0x30474, 0x30778 again | re-categorise |
| tail | 0x34398-0x3443d, then `trap_SnapVector` at 0x34451 | velocity clamp and snap, below |

Both movers are Q3-shaped: wishdir from cmd and yaw, wishspeed from the cmd
scale ("The wish speed" below), accelerate (`pm_accelerate` = 9.0 ground /
1.0 air), then `PM_StepSlideMove(gravity)`. An earlier read of this table
had the two movers swapped and 0x2F258 down as a steep-slope mover:
VERIFIED, 0x2F258 opens with a call to 0x2eb98 whose non-zero return goes to
0x2F03C (0x2f26a), Q3's `PM_CheckJump` then `PM_AirMove` at the top of
`PM_WalkMove` ("Jumps"), and the dispatch tests `pml+0x2c` (0x34312), the `walking`
slot of the `pml_t` layout the rest of this module matches.

### The tail of the default arm

VERIFIED, in `game.mp.i386.so`: 0x34398-0x343e8 take `ps.origin` less
`pml.previous_origin` (pml+0x68..0x70, stored from `ps.origin` at
0x34076-0x34091, ahead of the `pm_type` dispatch) and divide its squared
length by the square of `pml.frametime` (pml+0x24, `msec * 0.001` at
0x340b8); 0x343ea-0x343ff multiply the squared `ps.velocity` by the float
0.25 at rodata 0x70ce4; 0x34410-0x3443a store the displacement over the
frame time into all three velocity components. INFERRED, off the `jne` at
0x3440e: the store runs when the scaled squared velocity is the larger, so a
frame that moved less than half what its velocity says leaves with the
displacement's own rate as its velocity.

VERIFIED: 0x34443-0x34451 push `ps+0x20`, `ps.velocity`, and call
`trap_SnapVector` (0x63c04, relocation at 0x34452), which passes syscall 0x3a
through the engine's pointer (0x63c11). VERIFIED, in `cod_lnxded`: the game
module's dispatcher is 0x8087dcc, the function `VM_Create("game", ...)`
registers (0x8089124, name at rodata 0x80d51fc); its case 0x3a calls
0x80c8810, which saves the control word, loads the one at .data 0x80e68b4
(0x037f), and runs `fld`/`fistp`/`fild`/`fstp` over the three floats before
restoring it. INFERRED, off that control word's rounding field (bits 10-11
clear): each component is rounded to the nearest integer, ties to even, and
a component that rounds to zero is stored as +0.0, since it passes through
an integer.

INFERRED, off the dispatch at 0x341c1 and the table at rodata 0x70ce8: only
`pm_type` 0 and 6 reach the tail; the linked arm (1 and 7), the spectator
arm and the others `jmp` to 0x34456, past it.

VERIFIED, off the committed captures: every `velocity` in
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms.txt` and
`-sd-plant-attacker.txt` is a whole number. vcod: `pmove::pmove` and
`pmove::dead_move` end in `clamp_velocity_to_move` and `snap_velocity`.
What the two changed in the slope replay is under "The walk's accel floor".
A consequence Q3 players know: at 8 ms a frame's gravity is 6.4 and the snap
keeps about 6 of it, so a 125 fps jump rises about 36 units where a 20 ms
one rises 34 (`a_125_fps_jump_goes_higher` in `pmove.rs`).

## Jumps

Read out of `game.mp.i386.so` on 2026-09-27 with `tools/re/annotate_func.py`.
Every jump a player takes, off the ground or off a ladder, is one function,
`PM_CheckJump` at 0x2eb98. Two earlier reads of this section were wrong
about it: the block at 0x31cc0 that it called the ground jump is the prone
dive ("Going prone", under "Prone"), and 0x2eb98 was down as a push-off
reached from a ladder and a steep-slope mover only, where the "steep-slope
mover" 0x2f258 is `PM_WalkMove` ("Frame flow"). The dive's heights 34 and 24
were what vcod jumped at until the 2026-09-27 port.

### Who calls it

VERIFIED: the full disassembly has two calls to 0x2eb98, at 0x2f261 (the
first instruction of `PM_WalkMove`) and at 0x3394c (inside
`PM_LadderMove`). VERIFIED: on a non-zero return `PM_WalkMove` calls
`PM_AirMove` (0x2f26a), stores `cmd.serverTime` (pm+4) into `ps.jumpTime`
(ps+0x64, 0x2f279) and returns (0x2f27c). INFERRED: a player on walkable
ground jumps at the top of the walk move, the jump frame flies rather than
walks, and `jumpTime` is stamped after that flight, on every jump.

### The gates

VERIFIED, the reads at the top of 0x2eb98, in order: `cmd.serverTime -
ps.jumpTime` against 0x1f3 (0x2ebb3); `pm_flags` 0x800 (0x2ebbe), 0x2000
(0x2ebc3) and 0x1 (0x2ebc8); `viewHeightLerpTarget` (ps+0xd8) against
`proneViewHeight` (ps+0x33c, 0x2ebd2); `viewHeightLerpTime` (ps+0xd4),
`crouchViewHeight` (ps+0x340) and `viewHeightLerpDown` (ps+0xdc) at
0x2ebda-0x2ebf3; `pm_flags` 0x2 (0x2ebf5); the lerp time and the crouch
height again (0x2ebf9-0x2ec03); `cmd.upmove` (pm+0x1a) against 9
(0x2ec05); `pm_flags` 0x8 (0x2ec0d), and on it a store of 0 into
`cmd.upmove` (0x2ec13). Field offsets are CoDExtended's `shared.h`.

INFERRED, from the branches, each of which returns 0: a jump is refused
inside 500 ms of the last one (the delta must exceed 499); on `pm_flags`
0x800 or 0x2000; prone (0x1) or crouched (0x2); while the eye's target is
the prone height; while a lerp toward the crouch height runs; unless
`upmove` is 10 or more; and while 0x8, the held-jump latch, is set, in which
case `upmove` is also zeroed for the rest of the frame. So only a standing
player jumps, and a held key jumps once. The four stance and lerp gates are
the same test the walk's scale and accel make ("The eye through a stance
change"), and vcod's `move_stance` is that test.

VERIFIED: the latch is set by the jump (0x2ec36) and cleared in
`PmoveSingle` when `cmd.upmove` is 9 or less (0x3412d-0x34135), and `Pmove`
writes 20 into `cmd.upmove` after each `PmoveSingle` while 0x8 is set
(0x344f2-0x344f8). INFERRED: a cmd chopped into several steps keeps the
latch through all of them, and its later steps' air wish reads `upmove` 20
rather than 127. vcod ports the latch and not the 20.

VERIFIED: 0x800 is set in `ClientSpawn` (0x429d6) and cleared in
`PmoveSingle` (0x34000) behind tests of `pm_type` and the attack bit
(0x33f9a-0x33ffc). INFERRED: it is Q3's `PMF_RESPAWNED`, and refuses a jump
only on a spawn's first cmds until attack is released. INFERRED, from
`ClientSpawn`'s closing `ClientThink_real` on a cmd with no buttons
(`cod11-spectator-follow.md` 13): a live player never carries it past its
spawn, so the jump gate is unreachable; the flag itself is modelled there.

VERIFIED: 0x2000 is set, with `pm_time` 200, in `PM_CrashLand`
(0x2ffd7-0x2ffe0) behind a test of `fJumpOriginZ` against +/-0.001
(0x2ffb3-0x2ffd1), and cleared by `PM_DropTimers`. VERIFIED: the ground
trace (fn 0x30474) zeroes `fJumpOriginZ` on every hit at 0x305c8, ahead of
its only call to `PM_CrashLand` at 0x30721. INFERRED: that landing lockout
never arms. VERIFIED on the wire: the bump walker lands at stand/jump
`commandTime` 39682 with `pm_flags` 0x40008 and `pm_time` 0. Not modelled.

### The takeoff

VERIFIED, the stores once past the gates: `pml.groundPlane` (pml+0x30) and
`pml.walking` (pml+0x2c) zeroed (0x2ec20, 0x2ec2a); `pm_flags |= 0x8`
(0x2ec36); `groundEntityNum` 0x3ff (0x2ec3c); `velocity[2] =
sqrt(ps.gravity * 78.0)` (0x2ec45-0x2ec52, 78.0 at rodata 0x708c8);
`fJumpOriginZ` (ps+0x68) `= origin[2] + 39.0` (0x2ec57-0x2ec60, 39.0 at
0x708cc). At gravity 800 the takeoff is 249.8, and `39 = 249.8^2 / (2 * 800)`
is the jump's apex, so `fJumpOriginZ` is the height the jump peaks at.
INFERRED: the horizontal velocity is left alone.

VERIFIED, on the wire: both motion captures sample `jump_takeoff` at
`commandTime` 59082 (carentan) and 58082 (pavlov) with `jumpTime` 16 ms
earlier and `velocity[2]` 224, which is 249.8 less two 16 ms frames of
gravity through the snap (237, 224). `fJumpPeak`, the wire name of
`fJumpOriginZ` (netfield offset 104), reads -0.875 over an origin at
-39.875 and 272.343 over 233.343, 39 up in both, and 0 at `land`. The bump
walker's first airborne row (stand/jump 39083) reads `velocity[2]` 223.

### Off a ladder

VERIFIED: 0x2ec63-0x2ec69 test `pm_flags` 0x10 and jump past the whole
push-off block (0x2ec6f-0x2ed7a) when it is clear. Inside it: `velocity[2]`
is scaled by 0.75 (0x2ec72, rodata 0x708d0); a flattened copy of
`pml.forward` is normalized (0x2ec7e-0x2eca0); its reflection off
`vLadderVec` (ps+0x58) with the -2.0 at 0x708d4 is taken when the dot of
`vLadderVec` with the full pitched `pml.forward` is below zero
(0x2ecaa-0x2ecd3, reflection 0x2ece3-0x2ed36), normalized in 3D (0x2ed36),
and x and y alone scale by `pm_ladderPushOff` 128.0 (0x2ed5a, 0x7082c);
`pm_flags` 0x10 is cleared (0x2ed7a). INFERRED: only a ladder push-off
resets the horizontal velocity, to exactly 128 along the forward reflected
off a wall the player faces, or along the plain forward otherwise. An
earlier read had "off a ladder, along the horizontal forward" as the ground
case; it is the not-facing arm of the ladder block.

VERIFIED: 0x2ed80 tests 0x10 again right after that clear and picks event
0x53 on it (0x2ed86). INFERRED: unreachable.

### The event, the spread and the anim

VERIFIED: the event is 0 when the last ground trace's surface flags
(pml+0x50) carry 0x2000, otherwise `(flags & 0x1f00000) >> 20`, 0 when that
is 0 and `+ 0x46` otherwise (0x2ed90-0x2eda8), handed to
`BG_AddPredictableEventToPlayerstate` (0x2edb9), which returns without
writing on event 0 (0x2e31e). VERIFIED, on the wire: the carentan capture's
takeoff adds event 75 and pavlov's 74, `EV_JUMP_*` on materials 5 and 4.
An earlier read in `cod11-sound-system.md` said the ground jump emits no
event.

VERIFIED: `aimSpreadScale` (ps+0x3d8) takes 64.0 (0x708dc), capped at 255.0
(0x708e0), at 0x2edc6-0x2edf3; `BG_AnimScriptEvent(ps, 3, 0, 1)` or `(ps, 4,
0, 1)` on the sign of `cmd.forwardmove` (pm+0x18) at 0x2ee02-0x2ee19
(`JUMP`, `JUMPBK`); the function returns 1 (0x2ee5e). VERIFIED, on the wire:
the takeoff samples read `aimSpreadScale` 66.285 and 67.815 from 0 before.

### The jump's step

`PM_StepSlideMove`'s entry gate, 0x35057-0x35112. VERIFIED, the reads: the
slide's blocked result (0x35057), `fJumpOriginZ` against -0.001 and 0.001
(0x3505f-0x3507d, rodata 0x70ef0/0x70ef4), `groundEntityNum` against 0x3ff
(0x3507f), the entry height against `fJumpOriginZ` (0x3508c), and the step
size set to 18 (0x3509b) and compared with the room left under the origin
(0x350a7-0x350ce); on the unblocked side `groundEntityNum` again (0x350ec),
`pm_flags` 0x10 (0x350f5) and `velocity[2]` against 0 (0x350ff).

INFERRED, from those branches: a grounded move always goes on to the body.
An airborne one goes on only when it was blocked, `fJumpOriginZ` is set and
the move started below it, with the step size cut to `fJumpOriginZ - z`
where that is under 18 and the move returning unstepped where that is under
1 (0x350ce); or when it is on a ladder moving up. Every other airborne move
returns before the step-up and before the event tail.

VERIFIED: the flag that path sets (`edi`, 0x350da) is read at 0x35450,
where a step the revert test kept is tested against `fJumpOriginZ`
(0x35458-0x35468), and at 0x355a4, where a step that rose past the plain
slide's height (0x355b4-0x355c7) computes `fJumpOriginZ - z` (0x355cd),
zeroes `velocity[2]` under 0.1 (0x355d2-0x355e4, rodata 0x70f00) and
otherwise stores `sqrt(2 * that * ps.gravity)` when it is the smaller
(0x355f0-0x35652). INFERRED: a jump's step that ends at or above the jump's
origin is reverted like any other that got nowhere, and one that rose may
climb no faster than what just reaches the origin. So a jump plus a step
never rises past the jump's own apex. The tail at 0x35659 is past all of it,
so a jump's step announces itself and pays the velocity scale like a
grounded one ("The step event and the velocity scale").

VERIFIED, on the wire: at stand/jump 39483 and 39533 the retail walker,
falling past the standing target's shoulder 3 to 7 units under its jump's
origin, holds the target's side at 30.1248; a mover that stepped every
blocked airborne move went over the shoulder and came down 30.01 from it
(`cod11-player-clip.md`, section 12).

### vcod

`pmove::check_jump` is `PM_CheckJump`, called from the walk and the ladder
move; `PlayerState::jump_latched` is 0x8, `since_jump_ms` the delta to
`jumpTime` and `jump_origin_z` `fJumpOriginZ`, which the ground trace
zeroes on every hit. `step_slide_move` takes the entry gate, the revert and
the speed cap above. The server sends `jumpTime` and `fJumpPeak`, and the
predictor reads both back. Not modelled: the 0x800 and 0x2000 gates (both
unreachable; 0x2000's other reader, `PM_Friction`'s doubled control, is in
`cod11-player-clip.md` 8.7) and the chop's `upmove` 20. `crates/server/tests/bump_ab.rs` holds every row of
the walker's three jumps to 0.000, and `playerstate_motion_ab.rs` the
takeoff's fields and the `land` anim, which needs the probe's 16 ms cmds:
at one cmd a 50 ms frame the landing frame starts at -210, short of the
-220 `PM_CrashLand` wants.

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

What the section above leaves out, read on 2026-09-27 with
`tools/re/annotate_func.py` and measured with the prone crawl capture below:

- **The body also swings while the player moves.** INFERRED, from the branch
  at 0x330c0: inside the soft edge the swing still runs when the usercmd's
  `forwardmove`/`rightmove` word (`cmd+0x14`, compared as one 16-bit value)
  is non-zero and the delta is not zero, so a crawling player's body follows
  the view from the first degree. VERIFIED live: in both prone captures
  `proneDirection` moved on 172 snapshots, and a mover that swung only past
  the soft edge held it still on 99 of them.
- **The yaw cap measures before the swing and places after it.** INFERRED,
  from 0x3306d-0x3307b and 0x331c8-0x33265: the excess pushed into
  `delta_angles[1]` is computed from the delta taken before the swing, and the
  view is then set to `AngleNormalize360(proneDirection -/+ bg_prone_yawcap)`
  off the direction the swing just stored at 0x33193. A server that measured
  the excess after the swing pushed `delta_angles` short by one frame's swing
  and read 0.88 degrees off retail's on the capture's cap.
- **The pitch cap.** VERIFIED: the constants 45.0 and -45.0 at rodata
  0x70c94 and 0x70c98, loaded at 0x332a7 and 0x332be; the push lands in
  `delta_angles[0]` at 0x33311 and the view is rebuilt off `ps+0x370` at
  0x33320-0x33345. INFERRED, from that sequence: with
  `d = AngleNormalize180(AngleDelta(proneTorsoPitch, viewangles[0]))`, a `d`
  past 45 either way adds `ANGLE2SHORT(d -/+ 45)` (truncated) to
  `delta_angles[0]` and sets the view pitch to
  `AngleNormalize180(proneTorsoPitch -/+ 45)`. So a prone view pitches at most
  45 degrees off the ground's pitch along the view, not off level. VERIFIED
  live: at street `commandTime` 49392 the view held at 40.91 with
  `proneTorsoPitch` -4.09 while the cmd kept asking for more and
  `delta_angles[0]` walked from 65411 down to 59339.
- **The fit trace starts at 24.** VERIFIED: every caller passes 30.0 as the
  height (0x70bdc, 0x70c8c, 0x70c9c, 0x70d20) and the function subtracts the
  6.0 half box (0x70178) from it before adding the origin's z. INFERRED, from
  the arithmetic between 0x2d57a and the trace: the backwards sweep runs
  24 units above the feet. VERIFIED live: with the sweep at 11 the mover
  refused swings on the mound that retail took, 3.7 degrees of
  `proneDirection` apart; at 24 the gap is 0.5.

### The prone pitches, `PM_UpdatePronePitch` (0x3338c)

VERIFIED: `PmoveSingle`'s normal arm calls it at 0x342dd, after the ground
trace, the ADS flag and the walking flag and before the move dispatch;
`PM_UpdateViewAngles` runs at 0x340fc, before the stance. The rate 70.0 at
rodata 0x70ca4 multiplies `pml.frametime` at 0x33504 and 0x33601.

INFERRED, from 0x3338c-0x3368d: on a prone player, each of
`proneDirectionPitch` (ps+0x36c) and `proneTorsoPitch` (ps+0x370) moves
toward a target by at most `70 * frametime` degrees a frame and is folded by
`AngleNormalize180`. The targets are `PitchForYawOnNormal(proneDirection,
n)` and `PitchForYawOnNormal(viewangles[1], n)` with `n` the ground trace's
plane normal while `pml.groundPlane` is set, and 0 otherwise. The airborne
branch also runs `BG_CheckProne` and raises event 141 with `pm_flags` 0x8000
on a refusal; no capture has shown either.

`PitchForYawOnNormal` (0x3d274): VERIFIED, the constants: pi/180 (double at
0x72a10), -180.0 and pi (doubles at 0x72a20, 0x72a28), 360.0 (0x72a30), and
the degenerate answers 270.0 and 90.0 (0x72a18, 0x72a1c). INFERRED, from the
x87 sequence: the yaw's horizontal unit vector is projected onto the plane,
and the result is `-atan2(z, |xy|)` in degrees, wrapped into 0..360; a
projection with no horizontal part answers 270 when it points up, else 90.
VERIFIED live: the street's first prone snapshot reads both pitches 355.91,
which is -4.09 facing up a street whose normal leans 0.07 toward -y.

### Going prone, the tail of `PM_CheckDuck` (0x316f4)

VERIFIED, the constants: 34.0 and 24.0 (0x70be8, 0x70bec), 255.0 (0x70bf0),
0.25 (0x70bf8), -45.0 and 45.0 (0x70bfc, 0x70c00). INFERRED, from
0x31ca1-0x31f50, on the frame a prone press is taken:

- a non-zero `forwardmove`/`rightmove` word (0x31cae) clears `pm_flags`
  0x400 and the ADS flag; then, with the ADS flag clear and a non-zero
  `forwardmove` byte (0x31ccb-0x31cd3), `pm_flags` 0x4 is set and a player on
  the ground is thrown upward at `sqrt(2 * height * gravity)`, height 34 when
  `pm_flags & 3` was clear before the frame (0x317e8) and 24 otherwise, with
  `groundEntityNum` 1023 and `aimSpreadScale` 255. Since the move clears the
  ADS flag first, any forward or back cmd dives;
- `proneDirection` takes `viewangles[1]` (0x31dfc), `proneDirectionPitch`
  takes `PitchForYawOnNormal` off a 0.25-unit trace down, or 0 when it misses
  or starts solid (0x31e57-0x31ea7), and `proneTorsoPitch` takes that pitch
  clamped to within 45 of the view's (0x31ed6-0x31f4a).

INFERRED, from the three stores that clear `pm_flags` 0x4 (0x31971, 0x319a6,
0x31a13), on the arms where the prone key is up or the fit refuses the press:
the flag lives as long as the prone key is held. VERIFIED:
`PM_GetViewHeightLerpTime` (0x345b8) returns 200 for the prone target when
`pm_flags & 4` is set and `bg_duck2prone_time` otherwise.

VERIFIED live, the street capture's forward press at `commandTime` 69991:
`groundEntityNum` 1023, `velocity[2]` 195 one snapshot after a run at 224,
`pm_flags` 0x40005, and the eye from 60 to 11 in about 300 ms against the
still press's 550. The backward press dives the same way; the sideways press
does not.

### The landing damp

`PM_CrashLand` (0x2fd68) multiplies the velocity by 0.67 (0x70a38, at
0x300dd) on a damage-free landing of 12 units or more
(`cod11-sound-system.md`, "Landing"). VERIFIED live: every dive lands with
it, the street's forward dive going from 224 to 125 within the landing
snapshot, and a mover without it read 2.2 to 2.7 units ahead of retail on
the snapshot after each dive landed. A damaging landing takes the stun's
multiplier instead, and a slick or fatal one the same 0.67
(`cod11-player-clip.md` 8.4).

### The eye through a stance change

Read out of `game.mp.i386.so` on 2026-09-27 with
`tools/re/annotate_func.py`. The function at 0x309d8 has no symbol; this
doc calls it `PM_ViewHeightAdjust`. Field offsets are CoDExtended's
`shared.h`: `viewHeightTarget` ps+0xcc (int), `viewHeightCurrent` ps+0xd0
(float), `viewHeightLerpTime` ps+0xd4, `viewHeightLerpTarget` ps+0xd8,
`viewHeightLerpDown` ps+0xdc, and the prone, crouch and standing heights at
ps+0x33c, ps+0x340 and ps+0x344.

VERIFIED: `PM_CheckDuck` (0x316f4) calls it at 0x31833, beside a store of
`deadViewHeight` (ps+0x348) into ps+0xcc, and at 0x31fd3; `PmoveSingle`
calls `PM_CheckDuck` at 0x342c9 and `PM_WalkMove` at 0x3431b. INFERRED,
from that order: the walk reads the eye and the leg this frame left.

The legs' lengths. VERIFIED, 0x30b46-0x30b98: the immediates 200 (0xc8),
150 (0x96) and 100 (0x64) and the cvars `bg_duck2prone_time` and
`bg_prone2duck_time` (400 each), picked off `viewHeightLerpTarget`,
`viewHeightLerpDown` and `pm_flags` 0x4. INFERRED, from the branches: a leg
to the prone height takes 200 ms on 0x4 (the dive) and `bg_duck2prone_time`
otherwise; a leg down to the crouch height 100 ms on 0x4 and 150 otherwise;
a leg up to the crouch height `bg_prone2duck_time`; any other leg 200.
`PM_GetViewHeightLerpTime` (0x345b8) makes the same pick.

The curves. VERIFIED, five tables of 12-byte records `{int percent, float
height, int offset}` ending at percent -1, in `.data` (raw values, no
`.rel.data` entry in any of them):

| table | percent: height |
|---|---|
| 0x7c730, down to crouch | 0:60, 1:59.5, 4:58.5, 30:56, 80:44, 90:41.5, 95:40.5, 100:40 |
| 0x7c79c, up to standing | 0:40, 5:40.5, 10:41.5, 20:44, 70:56, 96:58.5, 99:59.5, 100:60 |
| 0x7c808, down to prone | 0:40, 11:38, 22:33, 34:25, 45:16, 50:15, 55:16, 70:18, 90:17, 100:11 |
| 0x7c88c, the dive | 0:40, 100:11 |
| 0x7c8b0, up to crouch | 0:11, 5:10, 30:21, 50:25, 67:31, 83:34, 100:40 |

The offset word is 0 in every record. INFERRED, from 0x30b9d-0x31059: each
frame of a running leg takes `percent = (cmd.serverTime -
viewHeightLerpTime) * 100 / length` in integers, clamped to 0..100, and sets
`viewHeightCurrent` linearly between the two records either side of it; at
100 the eye takes `viewHeightLerpTarget` and `viewHeightLerpTime` and ps+0xe0
are zeroed (0x30bcf-0x30bfe). INFERRED, from 0x3106e-0x3113f: a change of
more than 0.05 (double at 0x70bc0) in the interpolated offset pushes the
origin along the view's horizontal forward through `PM_StepSlideMove`; with
every offset 0 that never runs.

Starting a leg. INFERRED, from 0x315f2-0x316df: with no leg running and the
eye off `viewHeightTarget`, `viewHeightLerpTime` takes `cmd.serverTime` and
the leg aims at the next stance height on the way: toward prone,
`viewHeightLerpDown` 1 and the crouch height while the eye is above it,
else prone; toward crouch, down while the eye is above it; toward standing,
up, and the crouch height while the eye is below it. So standing to prone
and back are two legs each. INFERRED, from the jump at 0x30c01 to 0x31149:
the frame a leg ends falls through to this start, so the second leg is
stamped on the same frame. INFERRED, from 0x31149-0x312d4: a target on the
other side of a running leg's direction turns it back, `percent` becoming
`100 - percent`, `viewHeightLerpDown` flipping, `viewHeightLerpTarget`
moving to the leg's other end and `viewHeightLerpTime` set to
`cmd.serverTime - (int)(percent * 0.01 * length)` (0.01 float at 0x70bcc,
the truncating store at 0x312c6); a target further along the same way waits
for the leg to end. INFERRED, from 0x30a58-0x30b27: a target that is no
stance height, the dead one, zeroes `viewHeightLerpTime` and moves the eye at
180 (0x70bb8) times the frame time, the 180 per second of
`cod11-combat.md` 8.1.

VERIFIED live, the street capture's sideways prone press, first cmd at
80958: `vh` reads 56.673077 at `commandTime` 80993 (percent 23 of the 150 ms
leg down to crouch, 58.5 - 19/26 * 2.5), 41.5 at 81094 (percent 90), then
38.909092 at 81135, 27 ms into a 400 ms leg to prone stamped at 81108
(percent 6, 40 - 6/11 * 2), 15.8 at 81293 (percent 46) and 17.95 at 81394
(percent 71). That is the dip the section below used to list as not
modelled. The dive's 300 ms is its 100 and 200 ms legs.

The walk scale. VERIFIED, 0x2e7a1-0x2e7f0: reads of `pm_flags` 0x1,
`viewHeightLerpTarget` against the prone height, `viewHeightLerpTime`,
`viewHeightLerpTarget` against the crouch height, `viewHeightLerpDown` and
`pm_flags` 0x2; then two calls of 0x308cc, with (crouch, prone) at 0x2e810
and (prone, crouch) at 0x2e868, and the loads of `proneSpeedScale`
(ps+0x354) and `crouchSpeedScale` (ps+0x358) beside each. INFERRED, from
the branches:

- the stance the rest of the block falls back to is prone on `pm_flags`
  0x1, on a `viewHeightLerpTarget` at the prone height, or on a running leg
  up to the crouch height; else crouch on `pm_flags` 0x2 or on any running
  leg to the crouch height; else standing. The same test, inlined, picks the
  walk's accel at 0x2f436-0x2f490 and refuses a jump at 0x2ebc8-0x2ec03;
- 0x308cc answers 0 with no leg running, when its second argument is not
  `viewHeightLerpTarget`, or when that is the crouch height and the leg did
  not start at its first argument (prone going up, standing going down);
  otherwise `(cmd.serverTime - viewHeightLerpTime) / length` as a float,
  clamped to 0..1 (0x3099f-0x309c9);
- a non-zero fraction `f` from crouch to prone scales the wish by
  `f * proneSpeedScale + (1 - f) * crouchSpeedScale` (0x2e82d-0x2e8a0), one
  from prone to crouch by `f * crouchSpeedScale + (1 - f) * proneSpeedScale`,
  and only when both are 0 does the fallback stance pick one scale.

So a prone press from standing crawls through the leg down to crouch, since
`pm_flags` 0x1 is already set, and then runs from crouch speed down to
prone speed across the leg to prone; standing up does the mirror, then runs
at full speed through the leg up to standing; the standing-crouch legs
never blend. VERIFIED live, the same press: the strafe's 224 * 0.8 = 179
decays to `vel` 141 and 112 at 80993 and 81035 under the prone wish and
reads 27 at 81094 after a step, then climbs to 70 and 98 at 81135 and 81193
and falls through 86, 75, 66, 52 and 41, where the blended wish slides from
116 at the leg's start toward 27.

vcod: `pmove::view_height_adjust` ports the function without the offset
term, `move_stance` the stance test, `view_lerp_frac` 0x308cc and
`stance_speed_scale` the blend; `pmove::weapon::hip_spread_min` reads the
same legs for the hip cone (`cod11-combat.md` 2.1). `PlayerState::view_lerp_ms` is `cmd.serverTime
- viewHeightLerpTime`; the server and the predictor turn it into the wire
stamp. The prone captures do not record the three lerp fields, so the gate
carries our own leg across each rebase and holds the eye to retail's `vh`
instead. VERIFIED, vcod measurement (2026-09-27, `playerstate_slope_ab.rs`
with `SLOPE_REPORT=1`, rebased): the street's max dxy went from 2.736 to
0.429, its p99 from 0.233 to 0.048 and its rows past a unit from 1 to 0;
the mound (0.522, none past a unit) and the two route captures did not
move; the eye matches retail's printed `vh` on all 2851 prone rows.

### What the prone crawl capture measured

`--save-slope --probe-prone <uphill yaw>` against
`tools/run_probe.sh client-probes/probe_prone mp_carentan +set probe_teleport 1
+set probe_spot street|mound` (fixtures
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-prone-*.txt`):
a still prone press, crawls up, down and across the grade, a 150-degree turn
past the cap, a crawl with the view swinging, a pitch sweep to +/-80, a crawl
looking down, a turn and crawl downhill, and prone presses while running
forward, back and sideways, with the sight held and from a crouch. The
street is the 4-degree street at (900 1930), the mound the 19-degree terrain
at (-224 60). `playerstate_slope_ab.rs` replays the cmds on our mover,
rebased on retail's state at every snapshot.

VERIFIED, against the mover before this section's fixes: each dive read up
to 10.7 units off retail, `proneDirection` stood still through 99 of
retail's 172 swinging snapshots, the view pitch read 39.1 degrees off on the
street and 48.7 on the mound where retail's cap held it, and the yaw cap's
`delta_angles[1]` 0.88 off. Each of those is a correction a retail client
predicting its own prone movement takes from the server, which is the
"twitches sometimes when proning" of the hand check. After them: origin
|dz| at most 0.17 on the mound up to `commandTime` 84000, no row past a
unit on either spot (the last, 2.7 units on a sideways prone press, went
with the walk scale's blend, "The eye through a stance change"),
`proneDirection` within 0.5 degrees, both pitches within 1.42, the view and
`delta_angles` within one 16-bit step and the eye on retail's.

Still apart, and not modelled:

- which of two terrain facets the ground trace names under a crawl, 1.4
  degrees of prone pitch on the mound;
- a prone player wedged airborne against the `clip_nosight` brush behind
  the mound's crest, where the capture's last three presses ended; the gate
  stops at 84000 for that reason;
- the ground samples of `BG_CheckProneValid` past its first trace, the
  prone-position revert after a step (`PM_VerifyPronePosition`, port note 5),
  event 141 and `pm_flags` 0x8000 and 0x400, and `fTorsoHeight`,
  `fTorsoPitch` and `fWaistPitch`.

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

- First calls `PM_CheckJump` ("Jumps"); if it jumped (push-off), runs the
  normal mover and stores `jumpTime`, done.
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
  part of the step decision ("The jump's step", under "Jumps").
- The ground-trace function (0x30474) classifies the ground contact: impact
  velocity along the normal > 10 leaves the ground and fires the JUMP/JUMPBK
  anim events; normal.z >= 0.7 marks walking; anything flatter keeps
  `groundPlane` and clears `walking` ("A steep plane still steers the
  fall", below). An earlier read of this file called pml+0x2C a steep-slope
  flag and 0x2F258 a steep-slope mover; pml+0x2C is `walking` and 0x2F258 is
  `PM_WalkMove` ("The walk's accel floor"). VERIFIED: the ladder probe's
  30/8 choice (0x33700-0x33711) tests pml+0x2C at 0x33706. INFERRED: the
  "steep-slope flag" under "Ladders" is `walking`.
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
where `stepUp` is the up-trace's fraction times `stepSize + 1`, less one
("The step-up stops a unit short", below), and at 0x352E8
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
pass, and so is the waterjump exclusion that outlived it: 1.1 MP has no
water jump ("Water").

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

### The step-up stops a unit short

The turret capture's strafe (`crates/server/tests/fixtures/turret/mp_carentan-dm-turret.txt`,
`[phase strafe]`) slides along the `clip_nosight_dirt` brush beside the
mp_carentan MG nest and steps onto its sloped top, whose normal reads
(0, 0.789, 0.614) in vcod's clip, 52 degrees.

VERIFIED, `PM_StepSlideMove`: the up trace runs from the start to `stepSize
+ 1` above it (0x35173-0x3518c); at 0x351ca-0x351cf the fraction it returns
is multiplied by `stepSize + 1` and 1.0 subtracted, and the result is stored
at 0x351d8 into the slot 0x352d8 reads as `stepUp` and compared with 1.0 at
0x351de; the string at 0x70d88, `%i:not enough step room`, is pushed at
0x351fe, and 0x3520b stores 0 into that slot. The other arm writes the
origin as the start's x and y and the start's z plus the slot
(0x35226-0x35241). INFERRED: the step lifts the player a unit less than the
room the up trace found, and not at all when that is under a unit; with
nothing overhead an 18-unit step lifts 18, not 19. No allsolid test is read
in between; a trace that starts in solid returns fraction 0, which reads as
-1 and so as no room.

VERIFIED by capture, fixture lines 1512-1513: retail raises `EV_STEP_VIEW`
parm 139 (11 units) on the step at 39800 and reads origin (1704.5, 1917.9,
-14.1). VERIFIED, vcod measurement (`turret_ab.rs` with `TURRET_REPORT=1`):
lifting the full `fraction * (stepSize + 1)` raised parm 142 and put ours
at z -10.8 there; with the unit taken off, ours raises 139 and its origin
is within the gate's 0.25 of retail's. The extra unit mattered because the
second slide ran up a face too steep to stand on, where every unit of
height is more ground covered; on a step with a flat top the down pass
lands both heights on the same floor, which is why no slope capture caught
it.

### A start inside a solid

VERIFIED, `client-probes/probe_blastloop` on retail, 2026-10-05: a player
the `setOrigin` method set down at (-246.8, 2473.1, -32) read
(-246.80, 2473.10, -31.00) a second later, its client sending cmds with no
movement, where the others set down at z -32 nearby settled at -31.87. Its
box starts nine units inside a `clip_metal` brush of mp_carentan's
bombzone_A `script_brushmodel` (`*5`, top at -21.875).

VERIFIED, `game.mp.i386.so`, the offsets, immediates and call targets in
this list. INFERRED: the ordering and every condition in it.

- The ground trace (0x30474) runs from the origin plus 0.25 to the origin
  less 0.25 (`.rodata 0x70ba8`, 0x304c0-0x304d3), and with the trace's
  allsolid byte (`+0x2e`) set it calls 0x30214 (0x30526-0x3053a), a zero
  return skipping the rest of the categorisation.
- 0x30214 is `PM_CorrectAllSolid`. It loops 26 times (0x30336-0x3033d) over
  the unit offsets at `.rodata 0x70a40`, every (i, j, k) in {-1, 0, 1}³ but
  the origin, in the order (0,0,1), (-1,0,1), (0,-1,1), (1,0,1), (0,1,1),
  (-1,0,0), (0,-1,0), (1,0,0), (0,1,0), (0,0,-1), then the four `k = -1`
  edges and the eight corners. Each is a box trace with start and end equal
  (0x30230-0x3028a), passed over while its startsolid byte (`+0x2f`) is set
  (0x3028f). The first clear one becomes the origin, a one-unit trace down
  from it the ground trace, and that trace's end the origin again, and it
  returns 1 (0x30299-0x30334). With none clear it writes
  `groundEntityNum = 0x3ff` (`ps+0x54`), clears `pml.groundPlane` and
  `pml.walking` (`pml+0x30`, `+0x2c`) and `ps+0x68`, the jump's origin z,
  and returns 0 (0x30343-0x3036e).
- `PM_SlideMove` (0x347c0) tests its first trace's allsolid byte
  (0x34993-0x3499a) and on that arm writes 0 to `velocity.z` and returns 1
  (0x348a5-0x348b8); the origin's one store (0x349b2) is past the test.
- `PM_StepSlideMove`'s gate (0x35057-0x3510c) returns for a blocked move
  with the jump's origin z inside +-0.001 and no ground entity unless
  `pm_flags & 0x10` is set with `velocity.z` above 0, ahead of the up trace
  (0x35173).

INFERRED, from the above: a box one unit from free is nudged out and set
down; one deeper is in the air with no jump origin, its slide returns
unmoved with `velocity.z` zeroed, and the step's gate returns, so it holds
its origin frame after frame, which is the capture. The up trace that
would have found room above the brush never runs.

vcod: `correct_all_solid` in `crates/common/src/pmove.rs` is
`PM_CorrectAllSolid`, called from `ground_trace` on an allsolid trace, and
`step_slide_move` keeps retail's gate and revert for a start in solid,
where it used to step out of one. `a_player_set_down_inside_a_brush_stays_there`
pins both arms, and `probe_blastloop` against `vcod-server` reads -31.00 for
that player. vcod's ground trace still starts at the origin rather than
0.25 above it, so a box resting exactly on a face reads allsolid and takes
the (0, 0, 1) nudge and the drop back: not measured to differ.

### A steep plane still steers the fall

After that step the strafe falls back down the 52-degree face to the
floor.

VERIFIED: the ground trace (0x30474) loads 0.7 from 0x70bb4 at 0x30687 and
has two stores past it: `pml+0x30` 1 and `pml+0x2c` 0 at 0x306c3-0x306cd,
both 1 at 0x306e6-0x306f0. INFERRED: those are Q3's `groundPlane` and
`walking`, and a plane too steep to walk on keeps `groundPlane` with its
normal in `groundTrace` (the normal at pml+0x44) while clearing
`walking`, as Q3's `PM_GroundTrace` does. VERIFIED: the move dispatch tests
`pml+0x2c` (0x34312) to pick `PM_WalkMove`, so a player on a steep plane
takes `PM_AirMove` (0x2f03c). VERIFIED: `PM_AirMove` tests `pml+0x30` at
0x2f1d3, and the loop at 0x2f225-0x2f238 subtracts the plane's normal (from
pml+0x44) times the velocity's dot with it, scaled by the 1.001 at 0x708f0,
from the velocity, just before calling `PM_StepSlideMove` (0x2f241).
VERIFIED: `PM_SlideMove` (0x347c0) tests `pml+0x30` at 0x3483b before
calling `PM_ClipVelocity` with `pml+0x44` and the 1.001 at 0x70d78, and
again at 0x34875 before loading `pml+0x44`..`pml+0x4c` into the first slot
of its plane list. INFERRED: on a steep plane the fall is clipped onto the
plane before the move and the plane is one of the slide's planes from the
start, Q3's "slide along the steep plane"; neither depends on `walking`.

vcod read `walking` (`on_ground`) for both and clipped nothing in the air.
VERIFIED, vcod measurement (`turret_ab.rs` with `TURRET_REPORT=1`, the step
above already ported): the slide down the face and the snapshots up to the
landing at 39950 matched to print precision either way, and from the
landing on ours ran ahead of retail along the face's own direction, y, by
0.29 at 39950 growing to 1.1 where the refused phase came to rest
(retail (1678.6, 1952.4), ours (1678.61, 1953.50)), with x within 0.04
throughout; and ours' landing raised event 29 where retail's raised 6
(fixture line 1526). With `PM_AirMove`'s clip and `PM_SlideMove` reading
`groundPlane`, every strafe and refused row of the gate matches, the
landing event included. VERIFIED, vcod measurement (the gates' printed
summaries, both changes against neither): the `bump_ab.rs` phases read the
same; of
the four `playerstate_slope_ab.rs` captures the 25 ms route's rebased p95
dxy went from 0.004 to 0.001 with the step, and the prone mound's free run
from median dxy 0.176, max 19.79 to 0.163, 19.49; every other statistic
of the four is unchanged.

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
ground disagreement on either. What the velocity snap and the walk's accel
floor did to these numbers is under "The walk's accel floor". The 4.5 row
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
masks, `box_trace` never does. Glass brushes, whose words carry no SOLID
bit, enter the clip beside the SOLID and PLAYERCLIP ones, and a world-model
render soup lying on a pane's faces does not: mp_depot draws a pane's outer
face with a lump-0 entry whose contents are 0x1, where the brush's own word
is 0x8000010, and retail's round goes on through it
(`cod11-combat.md` 2.4). The bounce parm a prop carries is now the
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
`SP_script_brushmodel` is three calls and one store -- `trap_SetBrushModel`,
`InitScriptMover`, `self+0x118 = 1`, `trap_LinkEntity` -- with no other
instruction in the function, and `+0x118` is `r.contents`
(`docs/research/cod11-combat.md`, section 13). INFERRED, from that being
the whole body: the spawn function reads no `script_exploder` key and gives
an exploder brush model no state of its own, so every `script_brushmodel`
spawns solid and linked whatever keys it carries.

VERIFIED, from the asset text: `maps/mp/_load.gsc::main` loops over the
`script_brushmodel` and `script_model` arrays, and its body holds an arm
whose tests are `isdefined(.script_exploder)` and `.targetname` against the
literals `exploder` and `exploderchunk`, and whose statements are `hide()`
and a `notsolid()` under a further test of `.classname != "script_model"`.
INFERRED, from those tests and statements: that arm is what takes an
exploder brush model's brushes out of the clip at map load, and the
classname test is why a `script_model` keeps its solidity.

VERIFIED, from the entity lump of every `maps/MP/*.bsp` in the mounted paks
(16 BSPs, a superset of the 13 shipped with 1.1): four `script_brushmodel`s
carry both a `script_exploder` key and a `targetname` of `exploder` --
mp_depot `*1`, mp_powcamp `*3` and `*9`, mp_rocket `*3` -- and none carries
`exploderchunk`. INFERRED: those four are the entities that arm unlinks,
and every other `script_brushmodel` keeps its brushes.

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
airborne player through only on the jump-origin allowance or up a ladder,
and everything it lets through reaches the tail ("Jumps", "The jump's
step"). vcod ports the gate, so the event and the scale run for exactly
those moves.

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

## The walk's accel floor

VERIFIED, in `PM_WalkMove` (0x2f258): the accelerate is inline, not a call.
0x2f4b0-0x2f4ca pick 19.0 (0x708f8), 9.0 (0x70900) or 12.0 (0x708fc);
0x2f492-0x2f4a8 take 1.0 in their place on bit 0x2 of pml+0x50 or `pm_flags`
0x200; 0x2f4d8-0x2f4de multiply by 0.25 (0x70904) on `pm_flags` 0x100.
0x2f4e4-0x2f502 form the wish speed less the velocity along the wish
direction; 0x2f50f-0x2f52c load 100.0 (rodata 0x70908) and multiply the
accel by the frame time and by the wish speed or that 100; 0x2f53e-0x2f54f
divide by `ps.friction` (ps+0x37c, the netfield `friction`, 1.0 on every
capture); 0x2f563-0x2f582 add the result along the wish direction to the
velocity. INFERRED, off the compares at 0x2f506, 0x2f515, 0x2f52e, 0x2f545
and 0x2f551: nothing is added when the wish is already met, the rate is
`accel * frametime * max(wishspeed, 100)`, the division applies only with a
ground entity, and the result is capped at what the wish still lacks, both
before and after the division. Q3's `PM_Accelerate` has no floor: a wish
under 100, the 89.7 of a sighted walk or the 28.5 of a crawl, gains at 100's
rate and stops at its own speed. 0x70908 has no other reference in the pmove
range 0x2e400-0x35e00 (`objdump` byte scan), so the air, water and ladder
accelerations do not take it.

VERIFIED, off `mp_carentan-dm-slope-8ms.txt`: a sighted diagonal walk from
rest reads `vel` -8, -18, -32, -42, -56 and -63 per axis on consecutive
snapshots, 2 per axis per 8 ms cmd, where the unfloored rate less the
100-floored friction's 4.4 leaves 1.46 per axis and the floored one 1.98.

VERIFIED, vcod measurement (2026-09-23, `playerstate_slope_ab.rs` with
`SLOPE_REPORT=1`, rebased on retail's state at every snapshot):

| | 8 ms dxy p95 / p99 | 25 ms dxy p95 / p99 | 8 ms free-run dxy median |
|---|---|---|---|
| neither | 0.130 / 0.178 | 0.146 / 0.449 | 63.1 |
| snap alone | 0.152 / 0.216 | 0.159 / 0.445 | 141.3 |
| floor alone | 0.047 / 0.059 | 0.018 / 0.430 | 17.7 |
| both | 0.000 / 0.001 | 0.004 / 0.430 | 0.024 |

The max rows (2.9 and 4.5 units) and the counts past a unit (2 and 12) do
not move; the velocity clamp of "The tail of the default arm" changes no
row of either capture. The snap alone made the tail worse because it turned
the missing floor's 1.46 per cmd into a whole 1 where retail rounds 1.98 to
2. With both, the free run, which never rebases, stays within 0.024 of
retail for half the 1463 snapshots of the 8 ms route.

The friction's own floor stays the flat 100 of 0x2e500 (`PM_STOPSPEED`).
vcod scaled it by stance until this floor was found, because without it a
crawl's 4.33 gain per frame lost to that friction's 4.40.

### The ground clip keeps the speed

VERIFIED, in `PM_WalkMove` past the accelerate: 0x2f5b8-0x2f5d9 store the
velocity's 3D length; 0x2f5dc-0x2f5f1 keep a copy of the velocity;
0x2f5f4-0x2f64b clip it onto the ground plane (pml+0x44) with the 1.001
overclip at rodata 0x708f4; 0x2f654-0x2f672 dot the clipped velocity with
the copy and compare it with 0; 0x2f685 calls `VectorNormalize` on it and
0x2f694-0x2f6b3 scale all three components by the stored length. INFERRED,
off the `jne` at 0x2f67c: whenever the clip leaves the velocity pointing
the same way, it keeps its speed. Q3 has the same rescale with no test. A
grounded frame whose velocity still carries a fall turns the fall into
ground speed.

VERIFIED, 0x2f58c-0x2f5b5: gravity is applied to `velocity[2]` ahead of all
this when the ground's surface flags (pml+0x50) carry 0x2 or `pm_flags`
0x200 is set. INFERRED: Q3's slick-or-knockback gravity with CoD's own
bits. 0x200 is the timer a hit starts (`cod11-combat.md` 16.1), which vcod
carries beside the player-clip push's 0x100; 0x2 is `surfaceparm slick`
("Slick ground" below).

VERIFIED, off the bump walker (`mp_carentan-dm-bump-walker.txt`): after
the push at crouch/jump 70633 the walker lands under the knockback timer at
70832 on ground 1022 with `vel=-111,4,-175`, and the next snapshot, 70882,
reads `vel=-186,8,0` with up and forward released. A mover that clipped
without the rescale read `-93,4,0` there and 4.75 units short; with it the
row, and prone/land's likewise, match to 0.000. VERIFIED, vcod measurement
(2026-09-27, `playerstate_slope_ab.rs` with `SLOPE_REPORT=1`): the rescale
takes the 8 ms route's free-run median dxy from 0.024 to 0.021 and its rows
past a unit from 2 to 1, the prone street's free-run median from 0.021 to
0.010, and moves nothing on the 25 ms route.

### Slick ground

VERIFIED: CoDMP.exe's surfaceparm table holds `{"slick", 0, 0x2, 0}` at
0x571a40 (name string 0x558ba0), in the record shape of Q3's `infoParms`
(name, clearSolid, surface flags, contents); the same table reads `ladder`
0x8 and `nosteps` 0x2000, the two surface bits vcod already reads.

VERIFIED: 0x30302, 0x30514 and 0x30596 each start a 12-dword `rep movs`
of a trace into pml+0x34, the first two right after a call through
`pm->trace` (pm+0xe8, 0x30300 and 0x30512). With the trace layout of
`cod11-combat.md` (surface flags at +28), pml+0x50 is the ground trace's
surface flags.

VERIFIED, by a scan of `.rel.text` for `pml` relocations whose addend is
0x50: the module reads pml+0x50 at eleven places. Four test 0x2: 0x2e4ed
(`PM_Friction`), 0x2f492 and 0x2f58c (`PM_WalkMove`) and 0x30013
(`PM_CrashLand`, 0x2fd68). 0x2feaa tests 0x1 (`nodamage`). The other six,
0x2ed90, 0x30109, 0x30153, 0x30180, 0x301df and 0x32161
(`PM_FootstepEvent`), read 0x2000 and the material field 0x1f00000.
INFERRED: no footstep, jump or ground-trace path reads the slick bit.

What each of the four does:

- `PM_Friction`. VERIFIED: 0x2e4ed tests the bit with a `jne` to 0x2e54b;
  0x2e500-0x2e549 are the ground term (the 100-floored control times 5.5
  times the frame time) and 0x2e54b starts the water term. INFERRED, off
  that `jne`: slick ground takes no ground friction; wading friction still
  applies.
- `PM_WalkMove` accel. VERIFIED: 0x2f492 tests the bit with a `jne` to the
  `fld1` at 0x2f4a8; 0x2f4b0-0x2f4ca load the stance's 19, 9 or 12. The 1.0
  is an `fld1`, equal to `pm_airaccelerate` (0x70848, 1.0) but not a load of
  it. INFERRED, off that `jne`: the walk on slick ground accelerates at 1.0,
  still multiplied by the 0.25 of `pm_flags` 0x100 (0x2f4d8) and by
  `max(wishspeed, 100)` (0x2f50f).
- `PM_WalkMove` gravity. VERIFIED: 0x2f58c tests the bit with a `jne` to
  0x2f5a2; 0x2f5a9-0x2f5b5 subtract `ps.gravity` (ps+0x3c, `fild`) times
  the frame time (pml+0x24) from `velocity[2]`. INFERRED, off that `jne`
  and the clip of "The ground clip keeps the speed" after it: on flat slick
  ground the rescale turns that gravity into ground speed,
  `sqrt(v^2 + (g dt)^2) - v` a frame, so from rest a 16 ms frame gains 13.2
  where the accel alone gives 3.0, and at 80 units/s an 8 ms frame gains
  0.26.
- `PM_CrashLand`. VERIFIED: 0x30013 tests the bit with a `jne` to 0x300d5,
  and 0x3000a holds a `cmp esi, 0x63`; 0x30020-0x300d3 store
  `35 * esi + 500`, capped at 2000, into `pm_time` (ps+0x10, 0x300af), OR
  0x100 into `pm_flags` (0x300b4) and scale the velocity (0x300ba-0x300d1).
  INFERRED, off that `jne`: slick ground takes no landing stun.

vcod models the friction, accel and gravity arms in `pmove.rs` (`on_slick`,
reading `SURF_SLICK` off the ground trace's `surface_flags`). The landing
arm belongs to the stun, which this change does not port.

VERIFIED, read out of lump 0 of each map: no material of the fourteen
`maps/mp` maps in 1.5's `pak[0-9].pk3` (the twelve of 1.1's plus mp_bocage
and mp_neuville), of mp_stalingrad or mp_tigertown, or of any single-player
map in 1.1's `pak[0-9].pk3` carries 0x2, and no `.shader` file in any pak
declares `surfaceparm slick`. INFERRED: the arm never fires on stock
content, so it was ported from the binary alone and is unmeasured against
retail. Open: a brush hit in vcod takes the brush's lump-4 material flags,
where the impact evidence in `cod11-combat.md` points at the hit side's
(INFERRED there); a custom map slicking one face of a brush would differ.

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
  Which scale, and when the blend applies, is in "The eye through a stance
  change".
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
weapon factor, and `stance_speed_scale` as the stance block. Not ported:
the `wbuttons` 0x4 factor.

## Water

VERIFIED: `PmoveSingle`'s default arm dispatches the move to three
functions only (0x342fe-0x34322): `PM_LadderMove` (0x33944) on
`PMF_LADDER`, `PM_WalkMove` (0x2f258) on `pml.walking`, `PM_AirMove`
(0x2f03c) otherwise. Neither mover calls anything water-shaped:
`PM_WalkMove` calls `PM_CheckJump` (0x2eb98), `PM_AirMove`,
`PM_Friction` (0x2e460), `PM_CmdScale` (0x2e690), `VectorNormalize`,
`PM_StepSlideMove` and the movement-dir update (0x2e970); `PM_AirMove`
calls `PM_Friction` at 0x2f045, the air cmd scale (0x2e5bc),
`VectorNormalize` and `PM_StepSlideMove`. VERIFIED: the only `pm_time`
stores in the pmove range are the landing's and `PM_DropTimers`'
(`cod11-player-clip.md` 8.5), and the rodata float 350 (0x70c60) is read
only by the lean (0x32bf8, 0x32c26). INFERRED: 1.1 MP has neither Q3's
`PM_WaterMove` nor its water jump (`PM_CheckWaterJump`,
`PM_WaterJumpMove`, `PMF_TIME_WATERJUMP`); a player in deep water walks
on the bottom and falls through it at full gravity.

VERIFIED, where the water level (pm+0xd9, written by 0x30778) reaches the
move: the wade scale in the cmd scale ("The wish speed"); `PM_Friction`,
which skips the ground term above level 1 (0x2e4db) and adds `speed *
waterlevel * frametime` (0x2e54b-0x2e56b); the fall damage, halved at
level 2 (`cod11-player-clip.md` 8.3); and `PM_CrashLand`, which returns at
level 3.

vcod: `pmove.rs` dropped its RTCW port of the swim and the water jump on
2026-10-06, and `air_move` runs `friction` first, as `PM_AirMove` does.

## State reference (observed pm_flags bits, internal ps+0xC)

| bit | meaning | evidence |
|---|---|---|
| 0x01 | PRONE | tested everywhere stance matters (e.g. 0x3455A) |
| 0x02 | CROUCH | 0x34590 pattern |
| 0x04 | the prone dive, set by a prone press on a moving player; read by viewheight-lerp timing | 0x31CD5, 0x345C9 ("Going prone") |
| 0x10 | LADDER | set at 0x33937, cleared at 0x3377C/0x2ED7A |
| 0x80 | affects ladder-anim speed-scale choice | 0x323D7 |
| 0x08 | held-jump latch: set by a jump, cleared when `upmove` <= 9 | set @0x2EC36, clear @0x34135, tested at 0x2EC0D ("Jumps") |
| 0x20 | ADS held (blocks the prone dive); set/cleared by PM_UpdateAimDownSightFlag | set @0x372A4/0x372B7, clear @0x3ABDC, tested at 0x31CCB |
| 0x800 | Q3's `PMF_RESPAWNED`: set at spawn, cleared once attack is up; refuses a jump | set @0x429D6, clear @0x34000, tested at 0x2EBBE |
| 0x2000 | 200 ms landing lockout after a jump; refuses a jump, never arms ("Jumps") | set @0x2FFE0, tested at 0x2EBC3 |

(`PMF_PRONE`, `PMF_CROUCH`, `PMF_LADDER`, `PMF_SLIDING=0x100` agree with
CoDExtended's `shared.h`; bits 0x04/0x08/0x80/0x800/0x2000 are INFERRED from
usage sites only.)

## Tunables (all read directly from rodata, VERIFIED values)

| symbol | address | value |
|---|---|---|
| pm_stopspeed | 0x70824 | 100.0 |
| walk accel floor | 0x70908 | 100.0 |
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

1. SHIPPED - `PM_CheckJump` ("Jumps"): standing only, 500 ms cooldown,
   held-key latch, `vz = sqrt(78 * gravity)` with the horizontal kept,
   `fJumpOriginZ` and the jump's step, `EV_JUMP_*` and the spread kick. The
   stance-dependent 34/24 jump shipped before it was the prone dive's.
2. SHIPPED - friction 5.5, accelerate 9, stopspeed 100, stance accelerates
   12 ducked / 19 prone, and the walk's accel floor of 100 ("The walk's
   accel floor"). The stance-scaled stopspeed vcod carried until 2026-09-23
   stood in for that floor and is gone.
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
   retail's, the ground state taken before the move. A velocity re-test
   that once stood in for a waterjump exclusion refused the snap to every
   walker rubbing a wall on a slope ("The ground snap"); 1.1 MP has no
   water jump ("Water"). The airborne arm's one exception
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
