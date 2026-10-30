# CoD 1.5 movement against 1.1

What patch 1.5 changed in client-predicted movement (`bg_pmove` and the
weapon machine), measured on the 1.5b server and read out of its game
module. `cod15-protocol-diff.md` covers the wire. In short, 1.5:

- adds a jump penalty;
- drops the prone dive;
- steps the eye one stance at a time, the body's stance following the eye;
- halves a standing lean;
- changes a few step, ladder and weapon details.

Pmove's tunables did not move.

Contents: sources, captures, 1 function map, 2 jump penalty, 3 stance and
eye, 4 lean, view and prone body, 5 ground, step, ladder, landing (with
`ps.speed`, the fly move, `ps.gravity`), 6 weapons and view, 7 stored
floats, 8 the Windows clients, unchanged, what vcod does.

## Sources and conventions

Binaries as in `cod15-protocol-diff.md`: 1.1d and 1.5b `game.mp.i386.so`,
1.1 and 1.5 `cgame_mp_x86.dll`. Addresses are game module `nm` addresses
unless a cgame VA (base 0x30000000) is named; Ghidra's import of either
module sits 0x10000 higher. Functions were paired through `PmoveSingle`'s
call order (1.1 0x33dfc, 1.5 0x27eb7), with `tools/re/annotate_func.py`
resolving calls and constants, and checked against the captures below.

1.5's module is an unoptimised build, one frame per function, so 1.1's
inlined helpers are separate functions there. A different shape is not a
different behaviour. Every claim carries its own label: VERIFIED is read
out of a binary or a capture, INFERRED is read off control flow.

## The captures

`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-moves-1.1.txt`
and `...-moves-1.5.txt`: `moves_script` (`crates/client/src/probe.rs`,
`--probe-moves`) on the street placement of `client-probes/probe_prone`,
run against `tools/run_probe.sh` on each server, cmds every 8 ms, every
snapshot's decoded playerstate logged. They show, all VERIFIED:

- 1.5 sets `pm_flags` 0x2000 at a jump's takeoff and keeps it about 1.8 s
  past the landing (`pm_time` 1765 on the first snapshot after the
  landing frame).
- A jump from rest peaks 39.3 to 40.8 up on both (125 fps cmds). Pressed
  about 900 ms after the previous takeoff, 1.5's peaks 16.6 to 18.1 up
  (four times, standing and running); 1.1's 40.0 to 41.1.
- `fJumpPeak` on 1.5 is the takeoff height and stays after the landing; on
  1.1 it is the takeoff plus 39 and reads 0 on the ground.
- A prone press while running forward or backward lifts 1.1's body
  (`pm_flags` 0x4, vz 200), not 1.5's.
- From standing, 1.5's `viewHeightTarget` steps 60, 40, 11, `eFlags`
  reading crouch until the second step (about 200 ms in); 1.1's goes
  straight to 11 with `eFlags` prone at once.
- Prone entry on 1.5 sets 0x2000 with `pm_time` 1800 (less the cmd).

`...-turns-1.1.txt` and `...-turns-1.5.txt`: `turns_script`
(`--probe-turns`) from the same placement facing world yaw 90: a crawl
back until the legs near the kerb, quarter turns each way, a crawl with
the view swinging, a turn past the yaw cap, then a run, lean runs, a lean
strafe, a sighted walk and a leaning crouch walk. VERIFIED: 1.5 reads
`pm_flags` 0x8000 on 29 snapshots of the quarter turns, where its
`delta_angles[1]` moves with no cap involved (the view pull, section 4).

`crates/server/tests/moves_ab.rs` replays all four through the client's
predictor, each under its own rules, and the 1.5 captures under 1.1's to
show the gate tells them apart.

## 1. Function map

| Function | 1.1 | 1.5 |
|---|---|---|
| `PmoveSingle` | 0x33dfc | 0x27eb7 |
| ground trace | 0x30474 | 0x21ccd |
| `PM_SetWaterLevel` | 0x30778 | 0x2219f |
| `PM_CrashLand` | 0x2fd68 | 0x2115e |
| `PM_CheckDuck` | 0x316f4 | 0x232c4 |
| `PM_ViewHeightAdjust` | 0x309d8 | 0x226a6 |
| `PM_GetViewHeightLerpTime` | 0x345b8 | 0x223a6 |
| `PM_GetEffectiveStance` | 0x34554 | 0x1e9a5 |
| `PM_DropTimers` | 0x32a44 | 0x25aef |
| `PM_CheckLadderMove` | 0x336e8 | 0x27330 |
| `PM_LadderMove` | 0x33944 | 0x27846 |
| `PM_WalkMove` | 0x2f258 | 0x2003d |
| `PM_AirMove` | 0x2f03c | 0x1fe8d |
| `PM_CheckJump` | 0x2eb98 | 0x1f985 (takeoff 0x1f882) |
| `PM_Friction` | 0x2e460 | 0x1eab3 |
| `PM_CmdScale`, walk / air and ladder | 0x2e690 / 0x2e5bc | 0x1f08b / 0x1eec9 |
| `PM_SlideMove` / `PM_StepSlideMove` | 0x347c0 / 0x34fbc | 0x28d7d / 0x29942 |
| `PM_Footsteps` | 0x322c8 | 0x24ae8 |
| `PM_UpdateViewAngles` / `PM_UpdateLean` | 0x32d7c / 0x32ac8 | 0x26118 / 0x25c2e |
| `BG_CheckProneValid` | 0x2d428 | 0x1cf58 |

Symbols and call sites VERIFIED. The pairing is INFERRED from call order.

## 2. The jump penalty

1.5's 0x2000 rides `pm_time`, the timer of 1.1's knockback (0x100) and
damage knockback (0x200). In 1.1 only `PM_CrashLand` sets the bit
(0x2ffab-0x2ffe0, `fJumpOriginZ` non-zero, `pm_time` 200), after the
ground trace has zeroed `fJumpOriginZ` (0x305c8), so it never fires
(INFERRED). 1.5 removed that block (VERIFIED by absence).

- Takeoff (0x1f882), called by `PM_CheckJump` with 39.0 (immediate at
  0x1fa4e): `v2 = (float)(gravity * (h + h))`; with 0x2000 set and
  `pm_time <= 1800` (cmp 0x708 at 0x1f906), `v2 /= curve(pm_time)`
  (0x1f917); `vz = sqrt(v2)` (0x1f92c); `pm_flags |= 0x2008` (0x1f94b);
  `pm_time = 0` (0x1f95d); `fJumpOriginZ = origin.z` (0x1f97b), the
  takeoff, not 1.1's takeoff plus 39. VERIFIED.
- Curve (0x1ea52 for the jump, 0x1e9f1 for friction, one formula with its
  own rodata each): `t * 1.5 * 0.00058823527 + 1.0` up to t = 1699 (cmp
  0x6a3), 2.5 above (rodata 0x7566c..0x75678, copies at
  0x7567c..0x75688). 1 at 0, 2.5 from 1700 on. VERIFIED.
- Landing, at the top of `PM_WalkMove` before `PM_CheckJump`, with 0x2000
  set. The constants VERIFIED, the order INFERRED:

  | Condition | Effect |
  |---|---|
  | `pm_time > 1800` (a landing stun) | clear 0x2000 and `fJumpOriginZ`, velocity x 0.65 (0x20076, 0x2009a, rodata 0x756e8) |
  | `pm_time == 0` (first walking frame after a jump), `origin.z < fJumpOriginZ + 18` (0x756ec, 0x20169) | `pm_time` 1800, velocity x 0.65 |
  | `pm_time == 0`, otherwise | `pm_time` 1200, velocity x 0.5 (0x201e2, 0x756f0) |

  All three velocity components scale.
- `PM_Friction`'s ground control: 0x100 still multiplies by 0.3;
  otherwise 0x2000 with `pm_time <= 1800` multiplies by the curve
  (0x1ec14..0x1ec30). VERIFIED.
- `PM_DropTimers` is 1.1's: it clears 0x100, 0x200 and 0x2000 when the
  timer runs out (mask 0xffffdcff at 0x25b3e) and leaves `fJumpOriginZ`.
  VERIFIED.
- Also cleared with `fJumpOriginZ` by the ground trace's steep branch
  (0x220f5, 0x22107), a failed `PM_CorrectAllSolid` (0x219ec), and
  `PM_StepSlideMove` on a ladder or for an airborne body whose penalty
  timer runs (0x2997f-0x299b2, 0x299dd). The stores VERIFIED, the
  conditions INFERRED.
- Prone entry arms it: `PM_CheckDuck` sets 0x2000 and `pm_time` 1800 when
  the eye target steps to the prone height (0x24037, 0x24049). VERIFIED.

At gravity 800 (INFERRED from the formula, matched by the capture): an
unpenalised jump takes off at 249.8 and rises 39; at `pm_time` 1800 at
158.0 and 15.6; at 1200 at 174.1 and 18.9. Friction after a landing
starts at 2.5 times 1.1's and falls back to 1.1's over 1.7 s.

The penalty never reaches the accel, the wish speed or the cmd scale (no
read in 0x1ed62, 0x1f08b, 0x1eec9; VERIFIED by absence). Because it runs
on `pm_time`, `PM_SlideMove`'s tail (a running timer hands the primal
velocity back) now applies for 1.2-1.8 s after every landing (INFERRED).
The airborne clear takes 0x2000 off and leaves `pm_time` running, so a
body that walks off a ledge inside the penalty falls with the timer on
and no flag, and its velocity into a wall survives the slide. VERIFIED,
retail UO 1.51b, `mp_streets-dm-slope-8ms-crouchjump-streets.txt`: off
the sill at ct 21548 `pm_flags` lose 0x2000, `pm_time` 792 counts on, and
the body falls down the wall at x 233.13 keeping `velocity[0]` -146. So
the predictor's `from_wire` keeps a 1.5 or UO `pm_time` with no flag;
1.1's still means nothing without 0x100 or 0x200.

## 3. Stance and the eye

1.5's body stance is the eye's target, `viewHeightTarget` (ps+0xcc), not
the flags.

- `PM_GetEffectiveStance` (0x1e9a5): crouch when the target is the crouch
  view height, prone when it is the prone one, else standing. 1.1's mixed
  the flags with the lerp's target, time and direction. VERIFIED. The
  jump, the walk accel and scale, the ladder check, the footsteps, the
  lean and `BG_PlayerStateToEntityState` read it (INFERRED from the
  callers).
- `PM_CheckDuck` still sets `pm_flags` 0x1 and 0x2 from the keys with
  1.1's traces and events. Changes (VERIFIED unless marked):
  - every stance trace masks bodies out (`tracemask & ~0x2000000`, 0x23a18
    and six more), so another player no longer blocks a rise;
  - an airborne press on a body not already prone is dropped with no
    flag, 0x8000 or event (0x237d6, 0x23921);
  - a prone fit clears 0x2 (0x2390c), as does every cmd that holds prone,
    so 0x1 and 0x2 never stand together;
  - no dive: no write of 0x4, no `sqrt`, no vz (by absence);
  - `BG_CheckProne` takes a body length, 60 here (0x237e3): 1.1's fixed
    54 plus the box's 6.
- The target steps only while no leg of the eye runs
  (`viewHeightLerpTime == 0`, 0x23e3c). Prone asked: a standing target
  goes to crouch, any other non-prone target to prone (arming the
  penalty). Prone not asked: a prone target goes to crouch, otherwise
  crouch or stand by 0x2. VERIFIED.
- The box and `eFlags` 0x20/0x40 follow the effective stance after the
  step (0x24155..0x241ce): 30 prone, 50 crouched, 70 standing. VERIFIED.
- `PM_ViewHeightAdjust`'s curves are 1.1's byte for byte (1.5 .data
  0x814a0, 0x81520, 0x815a0, 0x81680; VERIFIED). The leg lengths come
  from `PM_GetViewHeightLerpTime` (0x223a6), the cvars gone (VERIFIED
  immediates):

  | Leg | 1.5 ms | 1.1 ms |
  |---|---|---|
  | into prone | 400 | |
  | down to crouch | 200 | 150 |
  | up to crouch | 400 | |
  | to standing | 200 | |

  Since the target moves only between legs, 1.1's mid-leg turn back is
  dead code for a live player, and stand to prone is two legs, the second
  starting on the cmd after the first ends (INFERRED). The capture's
  `viewHeightLerpTime` agrees (VERIFIED).

So a crouch tap runs the full 200 ms down and 400 up; from standing to
prone the box is 50 and `eFlags` read crouch for the first leg, and a
rise from prone keeps the 50 box for the 400 ms leg (INFERRED).

A mounted player runs the same `PM_CheckDuck` (`PmoveSingle`'s mounted
arm calls 0x332c4; UO's calls 0x37e16) with its flags set to the gun's
stance, so the target and `eFlags` step to the gun's stance on the mount
(VERIFIED decompile and live, `cod11-turrets.md` 14.8).

## 4. Lean, view and prone body

- `PM_UpdateLean`: scale 0.25 with a prone effective stance, 0.5
  otherwise, standing included (0x25ce0..0x25cf5, immediates 0x3e800000
  and 0x3f000000; VERIFIED), so a standing lean tops out at half 1.1's.
  Ramp times 350/280 are 1.1's (0x75a60/0x75a64). The wall trace now runs
  for a linked player too (VERIFIED by absence of 1.1's 0x32c63 skip).
- `PM_CmdScale`: a lean counts as the walk, `walkSpeedScale` in place of
  `runSpeedScale * leanSpeedScale` on the ground (0.4 either way with
  stock values) and newly in the air and on a ladder (0x1eec9). VERIFIED.
- `PM_UpdateViewAngles` (0x26118), prone branch. The soft edge is always
  on (1.1's `bg_prone_softyawedge` default). VERIFIED. The order is
  INFERRED from control flow, with `d = AngleDelta(proneDirection, view)`
  folded through the shorts (`AngleNormalize360` 0x36a3c multiplies the
  float by the float 182.04445 on the x87 stack and truncates):
  - swing: past the soft edge, or moving with `d` non-zero, the candidate
    is 1.1's (55 a second, rodata 0x75a8c, or the view when one frame
    covers it). While the candidate does not fit, it walks a degree at a
    time back toward the old heading (`|delta| > 1`, double 1.0 at
    0x75a90). Within a degree it jumps to the old heading, marks a
    refusal and tries once more; a failure there skips to the cap. A
    fitting candidate is taken only when a 45-unit body (immediate
    0x42340000) fits along both the view and the candidate, else a
    refusal;
  - fit (0x25ff5): `f = |AngleDelta(yaw, view)| / 240` (rodata 0x75a68),
    the body tried along `AngleNormalize360Accurate(yaw - (1 - f) *
    AngleDelta(yaw, view))` with length `60 (1 - f) + 45 f` (0x75a74,
    0x75a70), writing the prone body on a fit;
  - pull: with the view still off the heading, while the 45-unit body
    along the heading or the fit there fails, the view turns toward the
    body a degree at a time (the full remainder within a degree), each
    turn pushed into `delta_angles[1]` and marked a refusal; the heading
    moves by the new `d` whenever the 45-unit test failed;
  - cap: 1.1's 85, measured on the last `d`, placing the view off the new
    heading;
  - a refusal sets 0x8000. When the final view is within a degree of the
    view before the cmd and on the same side as the cmd's own view, 0.98
    of that degree is added back (rodata 0x75a98).

  The constants and calls VERIFIED. VERIFIED, vcod measurement
  (`moves_ab.rs`): the turns capture's `proneDirection`, view and
  `delta_angles[1]` match on all 1100 snapshots, the 29 with 0x8000
  included; under 1.1's rules 54 `proneDirection` and 140
  `delta_angles[1]` rows miss.
- `BG_CheckProneValid` (0x1cf58): the body length is an argument (60 from
  every caller but the swing; the sweep is the length less 6, the bent
  reach the length less 30). A sample landing on a plane with normal.z <
  0.7 refuses outright, ground or not (0x1d842, 0x1dd86, 0x1def4, rodata
  0x74f88). A blocked bent-leg sweep from the hips tries once more from
  the hit 18 units up (rodata 0x74fb0). An off-ground caller with a clear
  sweep still runs the ground samples and writes the body they give; a
  sample that misses lays it flat and accepts, as 1.1's early return did.
  The constants and trace counts VERIFIED, the order INFERRED.
- `PM_UpdatePronePitch` is 1.1's in effect (VERIFIED constants).

### The airborne prone body

VERIFIED, `game.mp.i386.so` 1.5b: `PM_UpdatePronePitch` (0x26d80) calls
`BG_CheckProne` (0x26ed6) only when `groundEntityNum` (ps `+0x54`) is
0x3ff, handing it `fTorsoHeight`, `fTorsoPitch` and `fWaistPitch` (ps
`+0x3c4`, `+0x3c8`, `+0x3cc`) to write. INFERRED from the order: the call
runs at the cmd's start, so it samples the ground under the origin the
previous cmd left. With the off-ground ground samples above, a prone body
that drops writes the chest height and pitches it reads mid-air, and
nothing rewrites them once it lands and lies still.

VERIFIED, live (retail 1.5b and UO 1.51b, `client-probes/probe_pronebones`,
fixtures in `crates/server/tests/fixtures/pronebones/`): a prone body
dropped from `setorigin`'s raised unit onto mp_carentan's flat spot
(1435.88, 2675.88) lands at z -23.875 with `fTorsoHeight` -0.1341343 on
both servers, where the ground gives about 0. Its last airborne cmd
started 50 ms into the fall at -23.740 (the snapshot before the
landing), and -17.8745 (the chest sample's hit) - (-23.740) - 6 is
-0.1345. The same drop timed 16, 16, 18 ms from a cmd on the placement's
frame (the first placement after a spawn) leaves -0.3521347: the cmd
that starts at -23.5225. On mp_carentan's mound the drop is longer and
the body slides on landing, so the slide's own checks rewrite the
pitches (-0.536, -13.760, -12.524 on both). On 1.1 an off-ground caller
gets the flat body, so 1.1's lies flat after any drop (cod11-mantle.md,
"The fit check").

VERIFIED, same runs: the net probe's cmds run 16, 16 and 18 ms apart,
the last on each server frame's time, and that one arrives after the
frame has run (the snapshot's `commandTime` on the placement's frame is
the frame's time less 18). vcod reproduces each wire body above to the
float when its gate's cmds follow that cadence (`ProbeHold::framed`,
`pronepose_ab`), and misses by up to 0.3 units on an even 16 ms one. A
retail run with a 2 ms cmd inside the drop lands with another body, and
so does one whose first lie-down took a heading off the view
(`proneDirection` -45 where the client held 270); the fixtures exclude
both. That heading is the spawn point's yaw (below).

VERIFIED, `fTorsoHeight`'s arithmetic (`BG_CheckProneValid`'s last step,
1.1d `game.mp.i386.so` 0x2e14c, 1.5b 0x1e589, UO game.mp.uo 0x22ecf): the
chest sample's z less the origin's z less 6 is one x87 expression stored
once, the difference never rounded on its own. VERIFIED, live (gdb on
1.5b and UO, the `fstps` at that store): the chest sample on
mp_carentan's flat spot is -17.874535 (0xc18eff0c) on both servers, and
the stored body off an origin at -22.885998 is -0.98853683, where
rounding the difference first gives -0.9885349; the flat lie-down at
spawn on mp_carentan reads 0.0004427433 on the wire, not 0.00044250488.
vcod: `pmove::check_prone` takes the difference in `f64`.

### The spawn's lie-down

VERIFIED, `game.mp.i386.so` 1.5b `ClientSpawn` (0x3c9a8): after the
`gclient_t` memset it reads the client's last usercmd into `pers.cmd`
(`trap_GetUsercmd`, 0x3cce6), sets the view with `SetClientViewAngle`
(0x3ceac), stamps `pers.cmd.serverTime` with `level.time` (0x3cefe), sets
`commandTime` to `level.time - 100` and runs `ClientThink_real` on
`pers.cmd` (0x3cf35). UO 1.51b's (0x44df4) is the same:
`trap_GetUsercmd` at 0x45162, `ClientThink_real` at 0x45409. 1.1d's runs
a zeroed cmd (`memset` of 0x18 bytes at 0x42a2f) whose angles are the
negated `delta_angles` (cod11-combat.md, 9.2). INFERRED: on 1.5 and UO
the spawn's 100 ms think runs the client's held keys and stance at the
spawn's view.

VERIFIED, 1.5b: `PmoveSingle` calls `PM_CheckDuck` (0x28814) before the
ground trace (0x28819), and `PM_CheckDuck` drops a prone press while
`groundEntityNum` (ps `+0x54`) is 0x3ff (0x237d6). The memset leaves it
0. INFERRED: the think's first step takes the press even for a spawn
point above the ground.

VERIFIED, live (retail 1.1d, 1.5b, UO 1.51b, `client-probes/probe_pronebones`
with `probe_spawn_yaw 315`, two runs each, fixtures
`crates/server/tests/fixtures/pronebones/mp_brecourt-spawn315*.txt`): a
client holding prone at world yaw 270 is `spawn`ed on mp_brecourt's spot
at yaw 315 and turned to 270 on the same frame.

- 1.5 and UO: the spawn's snapshot is already prone (`pm_flags` 0x40001)
  with `proneDirection` -45, and the heading stays -45 for the rest of
  the run with the view 45 off it. The eye's prone leg steps 166 ms after
  the spawn (`pm_time` 1784 on the snapshot 200 ms in). INFERRED: with
  the view inside the soft edge and no move, nothing swings the heading;
  the crouch leg started at the end of the think's first 66 ms step.
- 1.1: the spawn's snapshot stands (`pm_flags` 0x40000) and the next cmd
  lies down along 270 (-90).

A dm spawn point's yaw is what those brecourt runs lay down along.

VERIFIED, same runs: through the crouch leg the legs play the crouch idle
(`pb_crouch_alert_unarmed`), then the prone one. 1.5's `PM_Footsteps`
calls `PM_GetEffectiveStance` (0x24c46), so the anim's stance is the
stepped eye target (section 3). 1.1's run reads `pb_crouch_alert_unarmed`
for one snapshot too: 1.1's `PM_Footsteps` picks its idles off
`viewHeightLerpTarget` alone (player-model-anim-system.md, "How retail
picks the movetype", "The stance").

VERIFIED, 1.5b `ClientSpawn`: every conditional jump in it lands at or
before 0x3cc41, none between there and the `ClientThink_real` call
(0x3cf35); UO's last lands at 0x450bd, before its call at 0x45409.
INFERRED: a spectator's spawn thinks on the last cmd as a player's does.
VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, `client-probes/probe_specspawn`
beside `--probe-edge spec-fly`, fixtures
`crates/server/tests/fixtures/specspawn/mp_carentan*.txt`): a spectator
holding forward, `spawn`ed again as a spectator at (-224, 60, 200) facing
0, reads (-200.402, 60, 200) in the spawn frame's snapshot on 1.5 and UO
(100 ms of flight on his last cmd, five spawns each) and (-224, 60, 200)
on 1.1. The script's own read of `self.origin` straight after the `spawn`
gives the same. INFERRED from the call order: `ClientThink_real` returns
inside `ClientSpawn`, which the builtin (1.1d 0x455cc) calls before it
returns to the script.

vcod:

- The sim moves in the frame's spawn pass, after the spawning thread, so
  the `spawn` builtin runs the same think on a fresh sim (the client's
  last cmd mirrored into `GameHost::client_last_cmd`) and writes and
  links that origin.
- `ClientSim::spawn_cmd` runs the client's last cmd under 1.5's and UO's
  rules, a 1.5 spawn starts with the body on the ground, and
  `update_anims` takes 1.5's stance off the eye target.
- Gates: `pronepose_ab`'s three `a_spawn_lies_down_on_its_own_yaw_*`
  hold the heading, the prone body and every bone to retail's;
  `specspawn_ab` holds the spectator's spawn frame and the `PROBE spec`
  lines.

**The think on a slope, standing.** VERIFIED, live (retail UO 1.51b,
`client-probes/probe_leanhit`, fixture
`crates/server/tests/fixtures/leanhit/mp_carentan-leanhit-cmds-uo.txt`
and the target probe's raw log): the shooter spawned standing on the tdm
point (976, -792, -2.04889) facing 180, his last cmd a spectator's with
no keys or buttons, and the spawn frame's snapshot carried his `trDelta`
as (6, 3, -1).

VERIFIED, `client-probes/probe_placebits` on all three Linux servers
(`crates/server/tests/fixtures/spawnpoints/placebits-retail.txt`):
`placeSpawnpoint` leaves that point (entity 92) at -(2 + 820224 / 2^24)
on 1.5b and UO and -(2 + 819968 / 2^24) on 1.1d; every other mp_carentan
tdm point is the same float on all three. The drop ends on a terrain edge
(triangle 1526's first edge, normal about (0.207, 0.186, 0.961)), and
1.5b and UO sweep it on their own build's edge frame. VERIFIED,
`coduo_lnxded` 0x805403e's edge loop normalizes the edge with
`VectorNormalize` (0x8066755) and takes its `u` from
`PerpendicularVector` (0x80676b4), whose `ProjectPointOnPlane`
(0x806aa4e) stores the squared length (0x806aa82), its reciprocal
(0x806aa8a), the scaled dot (0x806aabe) and each scaled normal component
(0x806aac9..0x806aae5); `v` is `CrossProduct` (0x80666b1, one store per
component). 1.1d's keeps those on the stack (section 7).

The spawn think's first 66 ms step falls from rest, and a fall from rest
moves exactly half its end velocity, so the tail's clamp (section 7) is a
tie the start height's last bits settle. VERIFIED, vcod measurement: from
the placed float the clamp takes the move and leaves -26, the second step
lands on triangle 1526's neighbour (normal about (0.120, 0.063, 0.991))
at -53.2 and the clip leaves (6, 3, -1), as retail. From `f32(-2.04889)`,
the fixture's `%g` print, the clamp does not take it, the landing is at
-80.2 and the clip leaves (10, 5, -1). VERIFIED, vcod measurement (the
tail's stores emulated over the 42 floats that print as -2.04889): 20
take the clamp.

vcod: `Terrain` keeps a second set of edge records built 1.5b's and UO's
way (`edges_v22`), which the stored edge sweep reads, and a test's spawn
pin takes the origin of the one entity its `%g` print rounds from
(`client_spawn`'s `pinned_spot`). `placespawn_ab`'s three
`placespawnpoint_*_to_the_bit` gates hold every stock map's spawnpoints
to retail's float except the rows cod11-gsc-object-model.md 16.2 lists,
and `leanhit_ab`'s UO `-cmds` replay holds the shooter's eye with no
offset. The 1.5b `-cmds` run's target spawned on the same point but lay
down in the think, so no capture shows a standing 1.5b think on a slope.

**The framed cadence elsewhere.** VERIFIED, vcod measurement
(`ProbeHold::framed` forced on for every `ProbeHold` user):
`scriptmodel_ab`, `vehicle_ram_ab` and `vehicle_shell_ab` pass either way
at both installs; at the UO install `vehicle_hits_ab` loses one test and
`vehicle_armor_ab` two (a pistol round that hits a seat on retail misses
on ours); at 1.1 `scriptmodel_ab`'s prone rest on a grade fails.
INFERRED: a gate's cadence has to be the one its capture's `CMDS` lines
show, so `framed` stays a per-gate choice: on for a capture taken with
the fall probe's 16/16/18 cmds whose result rides an airborne cmd's
start, off for the vehicle probes' 16 ms holds. VERIFIED, live (1.5b and
UO 1.51b, the `--probe-uo-weapon throw` thrower's cmd times logged): that
mode's cmds also run 16 or 17 ms apart from each snapshot's time (12000,
12016, 12032 before the 12050 frame), so `scriptmodel_ab`'s 1.5 and UO
thrower takes `framed`, which lands him on retail's float after the
script's `setOrigin` (cod11-combat.md 2.1, "The thrower's slide").

## 5. Ground, step, ladder, landing

- Ground trace: 1.1 zeroes `fJumpOriginZ` on every hit (0x305c8), 1.5
  only in the steep branch. VERIFIED.
- The sight off the ground. 1.5 adds a `pml` int at +0x34
  (`almostGroundPlane`; `groundTrace` moves to +0x38). VERIFIED, the
  stores: the ground trace (0x21ccd) sets it with `groundPlane` on a hit,
  steep or walkable, and clears it on a kick-off and on an all-solid it
  cannot correct. `PM_GroundTraceMissed` (0x21a17) traces the box from
  the origin a unit down (`.rodata` 0x7597c, 1.0) when `groundEntityNum`
  is already 1023 and sets it when that meets anything; else it traces 64
  down (0x75968) and sets it when the fraction is under 1/64 (0x75978), a
  floor within that unit. `PmoveSingle`'s linked arm (0x27eb7) clears it.
  UO's are the same (0x2656d, `.rodata` 0x9b81c, 0x9b808, 0x9b818).
  VERIFIED, the reader: `PM_UpdateAimDownSightFlag` (1.5 0x2cf6c, UO
  0x32c0b) tests +0x34 where 1.1's (0x37230) tests `pml.groundPlane` at
  +0x30; nothing else reads it. 1.1's `PM_GroundTraceMissed` (0x3037c)
  runs no drop trace. VERIFIED, `leanhit-press-1.5`: the shooter, re-pinned
  a unit and a bit over the floor at 12500, falls 0.63 through the 12500
  cmd and sights on the 12516 cmd, a cmd before his landing: retail's
  first-round scope lag of 4 65536ths (`cod11-combat.md` 15.1). vcod:
  `PlayerState::sight_ground`, the near flag set in `ground_trace`; 1.1
  reads `ground_plane`, so a body on a steep face keeps the sight there.
- `PM_Friction`: under one unit of speed 1.5 zeroes x, y and z (0x1eb4f),
  1.1 only x and y. Air moves call it, so a vertical jump's apex can snap
  vz to 0. VERIFIED.
- `PM_StepSlideMove` (VERIFIED unless marked):
  - the jump step tests 0x2000 and climbs to `fJumpOriginZ + 39`
    (0x75ca8);
  - a rising climber steps whether blocked or not;
  - the ground snap reaches 9 (0x75cb4), where 1.1 reaches half the step
    size (5 prone);
  - the ground-plane flag is `pml.groundPlane`, a steep plane's included
    (INFERRED);
  - the step event, its velocity scale, its footstep and the prone check
    run only for a move that started on a ground plane off a ladder
    (0x2a46f), where 1.1 ran them for every move past the entry gate.
- `PM_CheckLadderMove`: skipped only while `pm_time` runs with 0x100 or
  0x200 set (0x2734f-0x27369), not under the penalty; a new grab needs a
  second hit along the face's normal (0x27652, 0x277a9); leaving the rungs
  airborne plays the jump anim (0x27838). VERIFIED calls. The climb is
  1.1's: `mp_ship-dm-ladder-1.5.txt` and `-uo.txt` settle at 1.1's rates
  and `ladder_ab.rs` matches both under their rules (`cod11-mantle.md`,
  "Ladders"). VERIFIED. The climb's accelerate is a call (0x27a7e to
  0x1ed62) where 1.1 inlines it; its `ps.friction` divide (0x1ee4f)
  divides by the 1.0 `ClientSpawn` writes (0x3ce4b), the field's only
  store. VERIFIED.
- `PM_CrashLand`: the land anim plays from the damage branch only
  (0x2144f), where 1.1 played it for any landing under -220. VERIFIED.
- `PM_trace` (0x1e738), new: a trace starting inside a body adds it to the
  touch list, drops bodies from the mask for the rest of the move, and
  retraces. Every mover trace but `PM_GroundTraceMissed` goes through it.
  VERIFIED. Modelled, and measured on the spawn's think
  (cod11-player-clip.md 13).
- `PmoveSingle`: `PM_Footsteps` runs before `PM_Weapon` (0x28889, 0x2888e;
  1.1 after, 0x34331/0x34336); the talk button keeps the sight bit
  (`buttons &= 0x12`, 0x27f5e; below); the ADS-prone move zeroing tests
  the effective stance; every non-normal arm clears the ladder flag.
  VERIFIED. UO keeps 1.5's order (0x2e04e, 0x2e053). VERIFIED.
  `pmove::pmove` runs each version's order, so a step that fires and
  steps rings the shot first on 1.1 only
  (`the_shot_and_the_footstep_ring_in_each_versions_order`).
- The talk button. A cmd with `buttons` 0x2 (console, menu or chat line
  up) is masked in `pm->cmd` before any `pm_type` arm: 1.1 writes
  `buttons = 2` (game.mp.i386.so 0x3400a), 1.5 `buttons &= 0x12`
  (0x27f5e) and UO the same (0x2d5d7). VERIFIED. So 1.5 and UO keep the
  sight and drop attack, melee, use and UO's sprint. All three keep `wbuttons & 0xc2` (crouch, prone, the held-stance
  bit; 1.1 0x3400e, 1.5 0x27f6d) and zero forward, right and up (cmd
  +0x18..+0x1a). VERIFIED. 1.1 masks after its `eFlags` 0x400 test, which
  checks the talk bit itself (0x33fd8); 1.5 masks first and its 0x400 test
  (0x282c5) reads the masked attack. INFERRED. Both set `eFlags` 0x40000
  on a talking cmd and clear it on any other (1.1 0x33f72/0x33f82, 1.5
  0x281f8/0x2821f); UO's bit is 0x20000 (0x2d8cf), 0x40000 in 1.1's
  layout. VERIFIED. Between steps `Pmove` (1.1 0x3447c) writes only
  `cmd.serverTime` and `cmd.upmove` (0x344e4, 0x344f8). VERIFIED. The next
  cmd's prone crawl test (`oldcmd.buttons & 0x10`) reads the raw sight a
  talking cmd carried. VERIFIED, `talk-1.1` at ct 34741: retail keeps the
  sight down on the first crawl cmd after a sighted prone talk, where a
  masked `oldcmd` would raise it. VERIFIED, live (1.1d, 1.5b and UO
  1.51b, `--probe-talk`, fixtures
  `mp_carentan-dm-slope-8ms-talk-{1.1,1.5,uo}.txt`): 1.1's sight drops
  over 0.75 s under the bit and comes back up after, 1.5's and UO's hold
  at 1; a talking run stops under friction; a lean, a shot and a jump do
  nothing; a crouch holds; `eFlags` reads 262168 (0x40018) standing.
  vcod: `cmd::talk_masked` in `player_step`, `spectator_step` and
  `dead_step`; `ps.last_cmd_ads` takes the raw cmd; the server's
  `ClientSim::talking` sets `flags::EF_TALK`. `moves_ab` replays all
  three exactly. When the client sets the bit: `cod11-console.md` 6.
- `PM_Footsteps` (0x24ae8): the rate formula is 1.1's
  (`cod11-sound-system.md` 9a), every intermediate stored as a float, K at
  rodata 0x75a2c-0x75a48, the stance the effective one. VERIFIED. A lean
  counts as the walk in the rate (`pm_flags` 0x80 or `leanf` non-zero,
  0x24bf7) and in the step ids (0x3105c), but not in
  `PM_ShouldMakeFootsteps` (0x24a5a), so a lean run is audible with walk
  ids. VERIFIED. UO drops the lean from the ground cadence only
  (`coduo-movement.md` 16). The ladder branch also skips `pm_type` 1 and
  takes its quiet window from `pm_ladderJumpTime` (300, `.rodata`
  0x75630). VERIFIED. An airborne body, ladder or not, whose effective
  stance differs from its flags' (`pm_flags & 3` against 1 prone, 2
  crouch, asm 0x24e51) falls through to the ground code. VERIFIED.
- `PM_Weapon` 0x30643: 1.1's 255 store for a prone body with move keys or
  a swing (`cod11-combat.md` 2.1). VERIFIED.

### ps.speed

- Every mover reads `ps.speed` (int, `ps+0x44`, UO `ps+0x48`): 1.1d's
  `PM_CmdScale` (0x2e5bc), the walk's scale (0x2e690) and `PM_Footsteps`'
  full speed (0x322c8) load it through `pm->ps`; 1.5b's walk scale (Ghidra
  0x12873) and UO's air, walk and footstep scales (Ghidra 0x14013,
  0x14073, 0x16466) read the same field. VERIFIED, decompile. The fly move
  reads it too, so a spectator's 400 and a playing body's 190 are one
  field.
- 1.1d's `ClientEndFrame` stores `(int)g_speed.value` there on its playing
  and dead arm (`fistp` under an `| 0xc00` control word, 0x410cf-0x41100):
  truncated, read again every frame. VERIFIED, disassembly.
- 1.5b's and UO's store the client's `maxspeed` instead (1.5b
  0x3ada2-0x3ada8 from `gclient_t` 0x2150, UO 0x43299-0x4329f from
  0x4588). VERIFIED, disassembly. `ClientConnect` seeds it with
  `g_speed`'s integer (1.5b 0x3c852, UO 0x44c86), so a `g_speed` change
  reaches a client at its next connect; `ClientSpawn`'s save of
  0x20cc..0x21dc keeps it across spawns (INFERRED, decompile). `maxspeed`
  is a 1.5 and UO client field (0xc005, 1.5b `GScr_AddFieldsForClient`);
  its setter (1.5b 0x3b88e, UO 0x43b82) takes `Scr_GetInt`, floors it at
  0 and copies it as a float into the entity's `speed` (`ent+0x1e8`).
  VERIFIED, decompile and field dump.
- `ClientSpawn` runs `ClientEndFrame` ahead of its think (1.1d 0x42a75,
  1.5b decompile `ClientEndFrame` then `ClientThink_real`), so the spawn's
  own think moves at that speed. `SpectatorThink` writes 400 (0 under
  1.5's refused free flight) ahead of the spectator's pmove. VERIFIED.
- vcod: `PlayerState::speed`, mirrored onto `ClientSim::max_speed` by
  `GameHost::client_speed` each frame and stored at the end frame and the
  spawn; the predictor reads it off the wire (`predict::from_wire`), a
  spectator's 400 included (`predict::predictable` takes `pm_type` 4).

### The fly move

- 1.5b's and UO's `PM_FlyMove` (1.5 0x1fcf9, UO 0x24838) runs
  `PM_Friction` (1.5 0x1eab3, UO 0x234ef), whose `pm_type` 4 term adds
  `speed * pm_spectatorfriction * frametime` to the stored drop after the
  ground and water terms; under one unit of speed all three components
  stop. The speed sums `(vz^2 + vy^2) + vx^2`. VERIFIED, decompile.
- Then `PM_CmdScale` (UO 0x23905): `max * ps.speed / (127 * sqrt(u^2 +
  f^2 + r^2))`, stored, times the run scale (1 here). The wish is `r *
  scale * pml.right + f * scale * pml.forward`, `u * scale` added to z,
  each component stored. The lean's 512 down (`wbuttons` 0x20) and 256 up
  (0x10) apply only while `ps.speed` is not 0 (UO `cmpl $0x0,0x48(%eax)`
  at 0x2492e, 1.5 `ps+0x44`); 1.1d adds them whatever the speed
  (0x2ef19-0x2ef4a). Then the stored `VectorNormalize` and
  `PM_Accelerate` (UO 0x2379e) at `pm_flyaccelerate` 8 with the floor of
  100, and `PM_StepSlideMove(0)`. VERIFIED, decompile and disassembly.
- VERIFIED (measured): with these in (`pmove::spectator_move`'s `v15`
  arm), the predictor replays the 79 spectating snapshots of
  `mp_carentan-dm-slope-8ms-uostates-uo.txt` on retail's origin and
  velocity bit for bit (`coduo-movement.md`, the uostates capture).
- VERIFIED (measured, `predict_ab`'s
  `predictor_flies_retail_spectator_1_5`): on 1.5b's own server,
  `client-probes/probe_states` (`mp_carentan-dm-slope-8ms-states-1.5.txt`),
  the predictor replays all 439 spectating snapshot pairs bit for bit on
  origin, velocity and eye, 311 of them moving. Under 1.1's unstored
  arithmetic 155 of the 439 differ.

### ps.gravity

- Every mover reads the int `ps.gravity` (`ps+0x3c`, UO `ps+0x40`), never
  a constant. 1.1d's eight loads, each `fildl 0x3c(...)` off `pm->ps`:
  `PM_CrashLand` (0x2fdb4), the jump (0x2ec45), the slick walk (0x2f5a9),
  the dive (0x31cf9), the ladder's decay (0x33bb2, 0x33bd2),
  `PM_SlideMove`'s fall (0x34813) and the jump step's cap (0x355f2).
  1.5b's and UO's builds have the same eight (1.5b 0x1f8de..0x2a3e8, UO
  0x243dc..0x2fcdc, each the same function in order). VERIFIED,
  disassembly.
- 1.1's cgame (`cgame_mp_x86.dll`, no symbols) has a `fild` of `ps+0x3c`
  at seven sites, each feeding the expression the `.so` builds at one of
  those loads. VERIFIED, disassembly and the Ghidra export:

  | Site (function) | Expression |
  |---|---|
  | 0x30007bd1 (0x30007b60) | `velocity[2] = sqrt(gravity * k)` |
  | 0x3000832b (0x30008070) | `velocity[2] - gravity * frametime` |
  | 0x30008b54 (0x30008ac0) | the int into a local, as the crash land takes it |
  | 0x3000a4d5 (0x30009de0) | `velocity[2] = sqrt(2 * h * gravity)`, `h` 34.0 or 24.0 (0x3000a4c3..0x3000a4cd), the pair the `.so` reads off `.rodata` 0x70be8 and 0x70bec for its store at 0x31cf9 |
  | 0x3000c1e7 (0x3000c060) | `velocity[2] +/- gravity * frametime`, which the `.so`'s ladder loads twice (0x33bb2, 0x33bd2) |
  | 0x3000cbf2 (0x3000cbc0) | `velocity[2] - gravity * frametime` into the slide's end velocity |
  | 0x3000da2a (0x3000d460) | `sqrt(2 * h * gravity)` |

  INFERRED, off those expressions and the shared order: the seven are the
  jump, the slick walk, the crash land, the dive, the ladder,
  `PM_SlideMove` and the step cap, so the predictor reads `ps.gravity`
  wherever the server does.
- `ClientEndFrame` stores `(int)g_gravity.value` there, truncated (`fistp`
  under `| 0xc00`), every frame, on all three: 1.1d 0x410aa-0x410c3 (then
  copied to `ps+0x3c`), 1.5b 0x3ad81-0x3ad96, UO 0x43278-0x4328d.
  VERIFIED, disassembly. The `g_speed` store follows it. The wire's
  16-bit `gravity` field carries it.
- `SP_worldspawn` sets `g_gravity` to the map's `gravity` key, default
  `"800"` (1.1d 0x61df6-0x61e0a, rodata 0x7962c `800`, 0x79630 `gravity`,
  0x79638 `g_gravity`; 1.5b and UO decompile, same call). VERIFIED. So
  the map load overwrites a `+set g_gravity 400` on the command line;
  only a later `set` or a script `setCvar` sticks, until the next load.
  No BSP in the stock 1.1, 1.5 or UO paks holds a `"gravity"` key
  (VERIFIED, a byte search of all 131).
- vcod: `PlayerState::gravity`, mirrored onto `ClientSim::gravity` by
  `GameHost::client_gravity` each frame and stored at the end frame and
  the spawn; the predictor reads it off the wire (`predict::from_wire`).
  `spawn::spawn_entities_from_string` writes the worldspawn key.

#### The speedgrav captures

`mp_carentan-dm-slope-8ms-speedgrav-{1.1,1.5,uo}.txt`: the moves script
from probe_prone's street placement under `client-probes/probe_speedgrav`,
`ps.speed` 250 (`+set g_speed 250` on 1.1d, the script's `maxspeed` 250
on 1.5b and UO with `g_speed` left at 190) and `ps.gravity` 400
(`setCvar` after the load) on every snapshot. VERIFIED: the 1.5 and UO
servers send the `maxspeed`, not `g_speed`, and a jump's peak stays 39 to
41 while its flight lasts about `sqrt 2` times the stock one.

`moves_ab.rs` replays all three through the predictor, and again with the
wire's speed and gravity put back to 190 and 800, which must fail;
`predict_ab.rs` runs the 1.5 and UO ones through the server sim side by
side. All three replays match on every gated field. The faster 1.1 run
reaches the kerb wall at x 1656, where the prone press leaves the body
airborne on the wall's near-vertical face for a second. Two traps there,
both closed: the torso pitch and the pitch cap's `delta_angles[0]`
pushes need 1.1's prone pitch ease to take the short truncation
(cod11-mantle.md, "The prone pitches"), and one diagonal step onto the
kerb (ct 62946) rides the capsule tie fixes.

## 6. Weapons and view

- `PM_UpdateAimDownSightLerp` (0x2d1a9): `rechamberWhileAds 0` with
  `weaponstate` 4 lowers the sight (cmp at 0x2d2f2). Stock 1.5 sets 0 on
  the springfield, kar98k_sniper and mosin_nagant_sniper and 1 on every
  other gun; a missing key parses as 0. VERIFIED. vcod reads a missing key
  as 0 too (`WeaponDef::rechamber_drops_ads`). Every stock 1.5 and UO
  file that omits it (the grenades, binoculars, satchel, flamethrower,
  PTRS41, bipod, tank and turret guns) has no `boltAction 1`, and only a
  bolt action rechambers, so the reading moves only a third-party file.
  VERIFIED, the `weapons/mp/` files of the stock `pak*.pk3` and
  `pakuo*.pk3`.
- Reload start with a spent case (0x2dd15, asm 0x2deee): the delay is the
  bolt time when under the reload's own delay, else the reload's own
  (1.1: else 1), and never 0. VERIFIED.
- `disableWeapon` sets `pm_flags` 0x100000, which the weapon change treats
  like the ladder bit: the change check puts a held weapon away to 0
  (0x2f937) and the pickup raises 0 (0x2e4bc). VERIFIED. vcod's pmove
  holsters on `PlayerState::weapon_disabled` as on the ladder, and the
  wire rewrite carries the bit as `flags::PMF_WEAPON_DISABLED`
  (cod15-protocol-diff.md 13).
- The weapon file table grew five fields (`rechamberWhileAds` 0x2d0,
  `adsViewErrorMin`/`Max` 0x2d4/0x2d8, `horizViewJitter`/`vertViewJitter`
  0x41c/0x420); every field from `cookOffHold` on moved 0xc. VERIFIED
  (table at 0x817a0, 253 entries). vcod parses by key.
- `BG_WeaponFireRecoil` (0x3246c) kicks the server's gun spring off
  `G_PlayerEvent` (0x3b2e4) on fire events, into `gclient` past the
  playerstate: a server-side aim offset off the server's RNG, nothing on
  the wire, nothing to predict. The cgame's own view kick is 1.1's.
  VERIFIED, by a capture; the reads are in the next section.
- Bob. VERIFIED unless marked:
  - the cvars are `cg_bobAmplitudeStanding` 0.007,
    `cg_bobAmplitudeDucked` 0.0075, `cg_bobAmplitudeProne` 0.03 and
    `cg_bobMax` 8; `bg_bob*` are the C variables behind them;
  - 1.1's cgame already registers the four (table
    0x30074d70..0x30074da0, flag 0x200); 1.5's game module adds them
    (`.data` 0x83990..0x839d8, flag 0x800) and 1.5's cgame registers them
    with 0xa00 (0x30077220..0x30077250), so a 1.5 server's values reach
    its clients through the 140/204 mirror;
  - they scale the gun bob (cgame 1.5 0x30013520, capped at 10) and the
    eye's walk bob (`CG_OffsetFirstPersonView` 1.1 0x30032ae0, 1.5
    0x30034180, through `BG_GetVerticalBobFactor` and
    `BG_GetHorizontalBobFactor`, 1.5 game 0x30a54 and 0x30b37, capped at
    `cg_bobMax`), each amplitude the speed times the stance's cvar;
  - the sight's view bob (capped at 45) reads the same three, as it
    shares the gun bob's stance multiplier in 1.1's game module
    (INFERRED);
  - 1.5 picks the stance by `viewHeightTarget`, the stepped eye target;
  - 1.5's `CalcMuzzlePoint` adds the eye bob to the server's shot origin;
  - the eye is never set below the origin plus 8 (cgame 1.1 0x30032de5).
- `PM_Weapon`'s other states, the ADS flag, the walking flag and the
  spread are 1.1's (side-by-side decompile, INFERRED).

### The server's gun kick

1.1d's game module never kicks the gun-kick spring of `cod11-combat.md`
15.2, so a 1.1 server's aim block adds nothing for it. 1.5b's does, and
so does United Offensive's (both VERIFIED by the captures below).

- `BG_PlayerStateToEntityState` (1.5b 0x1c449, called by
  `ClientThink_real` (0x39116) after `Pmove` and again by
  `ClientEndFrame` (0x3ab2a)) hands every event from
  `entityEventSequence` to `eventSequence` to `G_PlayerEvent(entnum,
  event)` before it copies it to the entity. VERIFIED. `ClientThink_real`
  runs `ClientEvents`, which fires the shot, after that copy. VERIFIED,
  the call order. So a shot's kick reaches the aim of the next cmd, never
  its own. INFERRED.
- `G_PlayerEvent` (1.5b 0x3b2e4) calls `BG_WeaponFireRecoil(ps,
  &client+0x22c0, local)` for events 159, 160, 161 and 168
  (`EV_FIRE_WEAPON`, `B`, `LASTSHOT`, `MG42`; 1.5's ids are 1.1's) and
  nothing else. UO's (0x435cc) does the same for its 163..166 and 173,
  adding `EV_FIRE_WEAPONC`, with the spring at `client+0x4714`. VERIFIED.
- `BG_WeaponFireRecoil` (1.5b 0x3246c, UO 0x396bc) draws four `rand()`s,
  each scaled by 1/2^31 (`.rodata` 0x77758 is 2147483648.0, UO 0x9d990):
  first the view kick's pitch and yaw, off the `ads` pair when
  `fWeaponPosFrac` is exactly 1.0 (0x77754) and the `hip` pair otherwise,
  written into the caller's local and dropped; then the gun kick's pitch
  and yaw, off the `ads` pair when the fraction is above 0 (0x77760),
  added to the spring's two speeds. Each is `min + (max - min) * r`. The
  1.5 file offsets are 1.1's plus 0xc (gun kick 0x35c..0x368 and
  0x39c..0x3a8, view kick 0x37c..0x388 and 0x3bc..0x3c8); UO's sit at
  0x3d4..0x448. VERIFIED.
- The spring (1.5b 0x31940 stepping 0x3171a per axis) is 1.1's: 5 ms
  substeps (0x77700) of `msec * 0.001`, the four keys blended hip to
  sight by the fraction, settled under 0.25 degrees (0x776e8) and 1 a
  second (0x776f0), run only for an `aimDownSight` weapon. Its angles
  reach the shot only down a sight with the fraction non-zero, through the
  aim block's composition. VERIFIED, the reads and constants; INFERRED,
  the order.

Measured, VERIFIED: `gunkick_ab.rs`'s three captures
(`mp_carentan-dm-slope-8ms-gunkick-{1.1,1.5,uo}.txt`) hold a thompson
whose file has no spread and its gun-kick ranges pinned (pitch -60, yaw
40), fired from the hip, then down the sight, at a wall 1091 units out.
From the hip every round of all three strikes one spot. Down the sight
1.1d's thirteen rounds stay within two units of it; 1.5b's climb from z
18 to 81 and UO's from 15 to 81, each a few units across, the spring at
its 4-degree `gunMaxPitch`. Ours replays the cmds on retail's level times
and lands every round within a unit of retail's on all three.

Measured, VERIFIED (live, retail UO 1.51b): the `vehicle_armor_ab` sweep
recipe on mp_cassino with the shooter's `--save-slope` on, its thompson
file's spread zeroed and the kick left stock (`adsGunKickPitch`
-55..-50, yaw -60..90). The net probe sends its cmds on a 60 Hz grid, 0,
16 and 32 or 33 ms past each frame's time, so the 85 ms `fireTime` fires
on every sixth cmd, 100 ms apart, and every round left within 0.3
degrees of the view the gsc set. With the kick pinned as above in the
same recipe the rounds sat 3.0 to 3.35 degrees above the view. The
spring's balance turns on the round spacing: at 100 ms a stock round's
kick settles before the next; at 96 ms (even 16 ms cmds, the sixth at 96)
the remainder adds up to `gunMaxPitch` over about 12 s (INFERRED, the
spring stepped by hand both ways; vcod's spring lands the same at each
spacing). A gate whose held trigger fires under the kick sends the
probe's 60 Hz cmds (`ProbeHold::sixty`).

**As implemented.** `ClientSim::fire_recoil` (`crates/server/src/spectate.rs`)
draws the four off the server's RNG for each fire event a cmd's pmove
raised, under `MoveRules::v15()` only, and `ClientSim::update_aim` steps
the spring (`pmove::aim::GunKick`) before each cmd's aim block and hands
it in. A spawn clears it with the rest of the aim state.

## 7. Stored floats

1.5b's and UO's modules store every float variable where 1.1d's keep the
x87 stack, so a fall's last bits differ between the builds. Addresses:
1.5b `game.mp.i386.so` and `cod_lnxded`, UO `game.mp.uo.i386.so` and
`coduo_lnxded`. Four stores reach a player's origin and velocity, all
VERIFIED:

| Store | 1.5b | UO | 1.1d |
|---|---|---|---|
| `PM_SlideMove`'s gravity: `velocity[2] - gravity * frametime` stored before `(velocity[2] + end) * 0.5` reads it back | 0x28e20-0x28e5f, store 0x28e34 | 0x2e714-0x2e753, store 0x2e728 | averages the unrounded end (`cod11-mantle.md`, "Frame flow") |
| default arm's tail: the displacement's three components (0x288b3, 0x288cc, 0x288e5), the rate `\|d\|^2 / frametime^2` (0x28912) and the squared speed (0x28967) stored; the speed times 0.25 (rodata 0x75ae0) compared with the stored rate; each new component `(1.0 / frametime) * d` on the stack (0x28993-0x289e3) | 0x288ad-0x289e3 | 0x2e072 on | keeps x on the stack, never stores the rate or speed (`cod11-mantle.md`, "The tail of the default arm") |
| terrain edge frame: the edge normalized by the stored `VectorNormalize`, its `u` from a `PerpendicularVector` whose projection stores each step | | 0x805403e's edge loop | keeps them on the stack |
| `CM_BoxTrace`'s end point: the shifted end stored (0x805acdd, UO 0x805ab11) before the delta is taken from it; the end point still `start + fraction * delta` rounded once (0x805b2xx) | 0x805aba1 | 0x805a9d5 | keeps it on the stack (0x8056310) |

A capsule swept onto a terrain edge reads the frame, so a long drop onto
one ends a few fraction units off 1.1d's (section 4, "The think on a
slope, standing"). On a clear fall the trace's end point is not 1.1d's in
the last bit.

VERIFIED, `spawnview_ab` against the 1.5b and UO logs
(`cod11-gametypes-re-bel.md` 8.6): with these in, a spawn think from 2
under the spot takes the move (3.0888) and one from 4 over it ends at
-0.00680161, both as retail, and the other heights stay right. A tail in
`f32` throughout with 1.1d's trace end misses both. VERIFIED, `moves_ab`:
the 1.5 moves capture's one `velocity` miss, a frame past a jump's apex,
goes once its replay world runs `Arith::Stored`.

vcod: `pmove::clamp_velocity_to_move` and `slide_move`'s gravity take the
stores under `MoveRules::v15()`; `collision::cm_endpos` takes the stored
end under `Arith::Stored`, which the server sets on protocols 6 and 22
(`Arith::for_protocol`). The client's world, `play::replay` and
`play::loopback` run `Arith::for_client()` instead, the Windows client's
build (section 8); `play::replay` runs the Linux build only to compare
against it.

- The locational bone trace (1.5b `cod_lnxded` 0x80d2000, UO
  `coduo_lnxded` 0x80c7394) stores every transformed point, slab distance
  and entry fraction, where 1.1d's 0x80c4564 keeps them. VERIFIED. The
  stores move a round's entry by about 3e-5 units. vcod ports them:
  `bonetrace::bone_trace_in` runs `bone_trace_stored` under
  `Arith::Stored` (`cod11-combat.md` 3.1, "The arithmetic on 1.5 and UO";
  `cod11-turrets.md` 13.5).

## 8. The Windows clients

The retail clients are one MSVC build of the collision and pmove code on
every patch. It stores some of the floats section 7's gcc build stores
and keeps the rest, at 53-bit precision. vcod predicts with it
(`Arith::Msvc`); the server keeps the Linux rules. Modules: 1.1
`CoDMP.exe` and `cgame_mp_x86.dll`, 1.5 `CoDMP.exe` (Ghidra's
`CoDMP15.exe`) and `cgame_mp_x86.dll`, UO 1.51 `CoDUOMP.exe` and
`uo_cgame_mp_x86.dll`. Addresses are VAs (exe base 0x400000, cgame
0x30000000).

- Precision: 1.5's exe calls `_controlfp(0x10000, 0x30000)` (`_PC_53`) at
  0x569040, UO's at 0x5783fc; no game code loads a control word of its
  own, and the CRT's trig wrappers test for 0x27f (1.5 0x5728a4).
  VERIFIED. INFERRED: the call is the CRT's startup default, so a value
  kept on the stack is an `f64`.
- One build: 1.1's and 1.5's patch code (1.1 0x41a2a0-0x41f780, 1.5
  0x41c1a0-0x421680), brush clip (1.1 0x4230c0, 1.5 0x424fc0) and
  `CM_BoxTrace` (1.1 0x424530, 1.5 0x426430) match instruction for
  instruction with addresses masked; UO's (brush clip 0x426a80) is another
  MSVC's register allocation with the same stores. VERIFIED.
- `PM_SlideMove`'s gravity: stored before the average, as 1.5b's (1.1
  cgame 0x3000cc1c, 1.5 0x3000d9dd, UO 0x3000e9b1). 1.1d keeps it.
  VERIFIED.
- `PM_Friction` (1.1 cgame 0x30007430, 1.5 0x30007b40, UO 0x30008470).
  VERIFIED, disassembly:
  - the speed sums `(vz^2 + vy^2) + vx^2`, kept on 1.1's stack, stored on
    1.5's and UO's (`fst [esp+8]`, 1.5 0x30007b82, UO 0x300084b2);
  - 1.5's and UO's also store the control `max(speed, 100)` once
    (0x30007be9, 0x30008519), which rounds nothing;
  - on the stack: the knockback's 0.3, the jump penalty's scale (1.5
    0x30007ae0, UO 0x30008410: 2.5 from `pm_time` 1700, else `pm_time *
    0.00088235294 + 1` with `1.5 / 1700` one float, 0x3a674db4, returned
    on the stack), the drop, the new speed and its ratio; each component
    rounded once;
  - ground term `(frametime * 5.5) * control` on 1.1 and 1.5,
    `(control * frametime) * 5.5` on UO; water term `((waterlevel * 1.0)
    * frametime) * speed` (UO without the 1.0);
  - 1.1 doubles the control under `pm_flags` 0x2000.

  So 1.5's and UO's clients keep what 1.5b's `PM_Friction` stores
  (section 7), and 1.1's client keeps the speed 1.1d stores. vcod:
  `pmove::friction_msvc` under `Arith::Msvc`. VERIFIED (measured,
  `play::replay::the_windows_build_draws_no_new_correction`): every
  own-body capture's corrections read the same with it and with the Linux
  builds' friction (1.5 moves 44, turns 14, UO sprint 125, 1.1 moves 7,
  springfield 2), so no capture tells the two apart.
- The default arm's tail (1.1 cgame 0x3000c90d, 1.5 0x3000d6c4, UO
  0x3000e64f): x of the displacement stays on the stack, y and z are
  stored, and the rate `((dz^2 + dy^2) + dx^2) / (ft * ft)` and the
  squared speed stay on the stack; 1.1 and 1.5 sum the speed `(vz^2 +
  vy^2) + vx^2`, UO `(vx^2 + vy^2) + vz^2`. The compare and `(1.0 / ft) *
  d` are 1.1d's. VERIFIED.
- `CM_BoxTrace`'s end point: the shifted end is kept (1.5 0x4264d6 `fsts`
  with the delta taken off the stack at 0x4264da, UO 0x427f76), as 1.1d's;
  the delta and the end point round as both Linux builds. VERIFIED.
- The brush clip (1.5 0x424fc0, UO 0x426a80). VERIFIED:
  - stored: `d1` (0x42503f), the enter path's `d1 - d2` (0x42506d), the
    enter fraction (0x4250d1), the leave fraction (0x425169);
  - kept: `d2` (0x425043), the leave path's `d1 - d2` (0x425146), `d1 -
    0.125` (0x4250b0), the enter fraction in its compare with the leave
    one (0x4250d5);
  - on a slanted side `dist + radius` (0x4251c7) and the shifted start
    (0x4251fb) are kept and only the shifted end is stored (0x42521e),
    where both Linux builds store all three.
- The patch build, against bsp-ibsp59-format.md's table. VERIFIED:
  - stored as 1.5b's: `VectorNormalize` (0x42fc00), `CM_AddFacetBevels`'s
    two distances (0x41dcb8, 0x41dcdd), `BaseWindingForPlane`'s (0x42027b,
    the corners' first steps 0x4203e3 and 0x420428), and
    `ChopWindingInPlace`'s distance and split fraction (0x420a52,
    0x420c2c), whose dot product it keeps;
  - kept: `CM_NeedsSubdivision` (0x41c500, the deviation and its length
    too, which both gccs round), `CM_Subdivide` (0x41c560), `CM_FindPlane`
    (0x41cf00), `CM_PointOnPlaneSide` (0x41d0c0);
  - UO's (0x41db40 on) store alike.
- The patch traces: `CM_CheckFacetPlane` (1.5 0x41efe0, UO 0x420a70)
  stores `d1` and `d2` (0x41f002, 0x41f01d) and keeps both fractions
  through the clamp and the compare; the point trace (0x41ead0) stores
  each plane's `d1` (0x41eb4d) and keeps its `d2` and the struck plane's
  distances. VERIFIED.

INFERRED, off those: a Windows client predicts a fall from rest as 1.1d
falls on every patch (`a_fall_from_rest_ties_the_tail_by_the_start_height`),
so on a 1.5 or UO server it misses the tail's tie wherever 1.5b's stores
take the other side. VERIFIED,
`play::replay::the_windows_build_draws_no_new_correction`: over the 1.1,
1.5 and UO own-body captures, all taken against Linux servers, the
Windows build adds last-bit corrections (1.5 moves 13 to 46, UO sprint
114 to 125, 1.1 moves 5 to 8) and none past the predictor's 0.1 miss
epsilon.

Not read on Windows: the terrain point clip (`Terrain::point_clip_stored`
is the Linux builds'; the client takes the kept one), the patch position
test, and the facet walk that answers only whether a facet stops a trace
(1.5 0x41ee90, stores `d1`). Not modelled: the Windows dot products'
order, `(z n2 + y n1) + x n0` in `CM_FindPlane` (0x41cf00), where vcod
sums x first.

The rest of the walk, in 1.5b's `game.mp.i386.so` unless named:

- `PM_Accelerate` is one function (0x1ed62), called by `PM_WalkMove`
  (0x204c9), `PM_AirMove` (0x1ffd7), `PM_LadderMove` (0x27a7e) and three
  more sites (0x1fe76, 0x20b0c, 0x20fb7). Stored: the speed along the
  wish (`[ebp-0x1c]`), the lack, and the rate `accel * frametime *
  max(wishspeed, pm_stopspeed)` (`pm_stopspeed` through the GOT slot at
  ebx+0xbbc, 100) once; the rate is capped at the lack, times `1 /
  ps.friction` with a ground entity, capped again, and each component
  `speed * wishdir + velocity` rounded once. VERIFIED. The walk's wish
  speed is `VectorNormalize`'s length times the scale, stored
  (0x203ec-0x203f5), and the accel the stance picks, times the
  knockback's 0.25, stored (0x204ac) before the call; the air's wish
  speed likewise (0x1ffb2-0x1ffbb) with `pm_airaccelerate`. VERIFIED. So
  1.5's air accelerate takes the walk's floor of 100, as 1.1's own inline
  one does (`cod11-mantle.md`, "The walk's accel floor"), and a jump while
  leaning wishes the walk's 76 until the lean has eased back (the air cmd
  scale takes a lean as the walk, 0x1eec9).
- `PM_Friction` (0x1eab3) stores the speed (`sqrt` of the dot, as 1.1),
  the control, its knockback or jump penalty scale, the drop after each
  term, the new speed and its ratio to the old (0x1ebde, 0x1ec02,
  0x1ec3a, 0x1ec88, 0x1ecce, 0x1ecfa, 0x1ed03, 0x1ed25); each component is
  the float times that ratio, rounded once (0x1ed2e-0x1ed59). VERIFIED.
  The water term multiplies `((speed * pm_waterfriction) * waterlevel) *
  frametime`: the stored speed times the cvar (0x1eca2-0x1eca5, GOT slot
  0xcfc, `R_386_GLOB_DAT` `pm_waterfriction` at 0x89a94), the `fild` of
  the byte at `ps+0xd9` times that (0x1ecb7-0x1ecbe), then
  `pml.frametime` (0x1ecc6). UO's is the same sequence at
  0x236d8-0x2370a (GOT slot 0xeb4, `pm_waterfriction` at 0xb46cc, the
  byte at `ps+0xf1`). VERIFIED, disassembly. Ghidra prints both as `level
  * speed * W * dt`, which is not the instruction order. 1.1d keeps the
  speed on the stack and folds `pm_waterfriction` out: `(speed *
  waterlevel) * frametime` (0x2e554-0x2e565). VERIFIED. vcod:
  `pmove::friction_stored` under `MoveRules::v15()`, whose water term
  runs in that order; no capture wades, so none measures it.
- The walk's cmd scale (0x1f08b, called from `PM_WalkMove` at 0x202ec)
  stores every step: the back and strafe products after `fabs` (0x1f0c9
  or 0x1f0e1, 0x1f107), the max (0x1f12c), the length (`sqrt` of the int
  sum, 0x1f18a), the base `(speed * max) / (length * 127)` (0x1f1aa), and
  the product after the walk or run scale (0x1f1f3, 0x1f20b), the stance
  scale (0x1f386, 0x1f3a4), the wade factor `1 - (1 - pm_waterSwimScale)
  * (waterlevel / 3)` and its product (0x1f3d7, 0x1f3f3, 0x1f3fc), the
  weapon's `moveSpeedScale` (0x1f462) and the shellshock's 0.4 (0x1f47d).
  A stance leg's blend `to * f + (1 - f) * from` stays on the stack and
  is multiplied in with one store (0x1f2e8, 0x1f368); `f` is the float
  `PM_GetViewHeightLerpTime` returns. VERIFIED, disassembly. 1.1d keeps
  the whole product on the stack (`cod11-mantle.md`, "The wish speed").
  VERIFIED, gdb on 1.5b (a `--probe-patchwalk 180` kerb run, dprintfs on
  `Pmove` and on `PM_StepSlideMove` past its prologue, 0x29957): of 2097
  cmds the velocity entering `PM_StepSlideMove` differed from ours in
  223, by one to three ulps; with these stores ported it differs in none
  but a teleport's cmds and one cmd retail ran as 50 ms where the capture
  logs six. VERIFIED, same run: the cmd that put ours a float step off
  retail's kerb (x 1007.12506 against 1007.125) had the same origin and
  velocity going in. INFERRED, from 0x295aa-0x2961e: the slide there
  meets the `x = 992` wall and the slope of brush 2281 in one bump, and
  `DotProduct(clipVelocity, planes[i])` after the two clips is 9.7e-7
  over 0 with the unstored wish speed and under 0 with one a float step
  larger, so retail slides along the crease (`CrossProduct`, 0x29667) and
  holds x. vcod: `pmove::wish`'s `WishArith::Stored` arm under
  `MoveRules::v15()` off `Arith::Msvc`; `predict_ab`'s 1.5b kerb (ct
  29991) and pole (ct 30492) rows match. VERIFIED, `predict_ab`.
- The Windows cgames' walk scale (1.1 `cgame_mp_x86.dll` 0x300076e0, 1.5
  0x30007e40, UO `uo_cgame_mp_x86.dll` 0x30008770, each called once from
  its `PM_WalkMove`: 1.1 0x300080d3, 1.5 0x3000891e) stores two steps and
  keeps the rest at 53 bits. VERIFIED, disassembly:
  - stored: the strafe product after `fabs` (1.1 0x30007728, 1.5
    0x30007e88, UO 0x300087c4) and the product after the walk, run or
    (UO) sprint scale (1.5 0x30007f04, UO 0x30008886); 1.1 stores after
    the walk or run scale (0x300077af, 0x30007791) and again after
    `leanSpeedScale` (0x300077af);
  - kept: the back product, the max when the back product wins (`fcom`
    against the stored strafe), `sqrt` of the int sum, the base `(speed *
    max) / (length * 127)`, the stance blend `(1 - f) * from + f * to` and
    its product, the wade factor `1 - (waterlevel * 0.33333334) * (1 -
    0.5)` (1.5 0x30008004-0x30008022; the third a float, rodata
    0x3006b7b4 = 0x3eaaaaab, where 1.1d and 1.5b divide by 3.0) and its
    product, the weapon's `moveSpeedScale`, the shellshock's 0.4;
  - the scale comes back on the stack and `PM_WalkMove` stores it (1.5
    0x30008926, 1.1 0x300080db), then stores the length times it (1.5
    0x30008aa4);
  - 1.5's and UO's take a lean as the walk (1.5 `ps+0x34c`, UO
    `ps+0x584`), as 1.5b does (0x1f1e0); 1.1's picks the run scale, then
    the lean's (`ps+0x364`).

  vcod: `pmove::wish`'s `WishArith::Msvc` arm under `Arith::Msvc`, on
  every protocol's client
  (`pmove::tests::the_windows_walk_scale_stores_the_run_product`: a
  crouched diagonal run wishes a float step under 1.1d's). VERIFIED
  (measured, `play::replay::the_windows_build_draws_no_new_correction`):
  the springfield 1.1 capture goes from 0 to 2 last-bit corrections,
  every other own-body capture reads as before (1.5 moves 44, turns 14,
  UO sprint 126, 1.1 moves 3), none past 0.1.
- The Windows cgames' `PM_Accelerate` (1.1 `cgame_mp_x86.dll` 0x30007560,
  1.5 0x30007cb0, UO `uo_cgame_mp_x86.dll` 0x300085d0) is one instruction
  sequence in all three, UO's with its own playerstate offsets: the wish
  direction in `edx`, the wish speed and the accel as float arguments.
  The dot `(vz * dz + vy * dy) + dx * vx` stays on the stack; the lack
  `wishspeed - dot` is stored (`fst [esp+4]`, 1.1 0x30007583) and tested
  `<= 0` on the stack; the control `max(wishspeed, 100)` (rodata
  0x300610a4, UO 0x300715ec) times `pml.frametime` times the accel stays
  on the stack (0x300075b5-0x300075bb), is capped at the stored lack,
  then with a ground entity multiplied by `1.0 / ps.friction`
  (0x300075d9, `ps+0x37c`, UO `ps+0x5c0`) and capped again; each
  component `rate * dir + velocity` is rounded once
  (0x300075f8-0x30007617). No `fldcw`. VERIFIED, disassembly. 1.1d's
  inline accelerate keeps the wish speed and the lack, 1.5b's stores the
  dot, the lack and the rate. The floor and the accels (9, 12, 19, 1 at
  1.1 rodata 0x300610bc-0x300610c8, the knockback's 0.25 at 0x3006950c,
  its product stored at 0x300082fd) are the Linux builds'. VERIFIED.
  Callers: 1.1 `PM_WalkMove` (0x3000830f), `PM_AirMove` (0x30007fd7,
  accel 1.0), `PM_LadderMove` (0x3000c1cc, the decompiled
  `FUN_3000c060`) and three moves vcod has no use for (0x30007ee0,
  0x30008693, 0x300089c4); 1.5 `PM_WalkMove` 0x30008b2b and `PM_AirMove`
  0x30008777, UO 0x30009510 and 0x30009150, six callers each. VERIFIED.
  INFERRED, off the order: 1.5's 0x3000cf3c and UO's 0x3000dddd are their
  ladder moves' calls.
- The Windows `PM_WalkMove` around the call (1.1 0x300080df-0x3000841e,
  1.5 0x3000892a-0x30008be3, UO 0x30009313-0x300095c8). VERIFIED,
  disassembly:
  - the flattened forward and right go through the cgame's
    `VectorNormalize` (1.1 0x30039cc0, 1.5 0x3003b3d0, UO 0x30049700),
    which stores the squared length, the length and its reciprocal as
    1.5b's does;
  - the wish vector's components are stored and normalized the same way,
    and the returned length times the stored scale is stored as the wish
    speed (1.1 0x30008253, 1.5 0x30008aa4, UO 0x3000948d);
  - the accel the stance picks, times 0.25 under a knockback, is stored
    (1.1 0x300082fd);
  - after the call: the speed `sqrt` kept and stored (1.1 0x3000835e),
    summed `(vx^2 + vy^2) + vz^2` on 1.1 and UO and `(vz^2 + vy^2) +
    vx^2` on 1.5 (0x30008b5c-0x30008b70); the clip onto the ground; a
    velocity that still points the same way normalized through the stored
    `VectorNormalize` (1.1 0x300083f0) and scaled by the stored speed.
- The walk's three clips and the air move's ground-plane clip are inline
  `PM_ClipVelocity`s that sum the dot `(z + y) + x` and multiply a dot
  `>= 0` by the float `1 / 1.001` (1.1 rodata 0x30069750, 1.5 0x3006b9b8,
  UO 0x3007c248, all 0x3f7fbe87, `(float)(1 / 1.001f)`) where the Linux
  builds divide by 1.001; a negative one takes 1.001 (1.1 0x30069540).
  Sites: 1.1 0x30008114, 0x30008185, 0x3000839f, 0x3000801e; 1.5
  0x30008967, 0x300089d8, 0x30008bbb, 0x300087be; UO 0x30009350,
  0x300093c1, 0x3000959d, 0x30009197. VERIFIED. The slide move's clips
  divide (1.1 0x3000cf90, 1.5 0x3000dd6e, UO 0x3000ed44). VERIFIED.

  vcod, under `Arith::Msvc` only: `pmove::accelerate_msvc` in the walk,
  air and ladder moves, `pmove::clip_velocity_msvc` in the walk and the
  air, and the stored `VectorNormalize` and length in `pmove::wish` and
  after the walk's clip on every protocol
  (`pmove::tests::the_windows_accelerate_stores_the_lack`,
  `the_windows_walk_clip_takes_the_reciprocal_overclip`). The 1.1 ladder
  keeps 1.1d's wish ahead of the call; its `VectorNormalize2`
  (0x30039ef0) is not read. VERIFIED (measured,
  `play::replay::the_windows_build_draws_no_new_correction`): 1.5 moves
  44, turns 14 and UO sprint 126 read as before; 1.1 moves goes from 3 to
  4, turns from 0 to 3, springfield from 2 to 0, none past 0.1. Taken out
  one at a time, the accelerate and the clip constant move no capture;
  the wish's stored normalize takes springfield's 2 away and the stored
  normalize after the walk's clip adds moves' one and turns' three. The
  captures are Linux servers', so the count measures the Windows build's
  distance from them, not its fidelity.
- `pm_type` 2 and 3 in the walk scale: 1.5b multiplies the walk or run
  product by 3.0 (rodata 0x756b8, 0x1f221-0x1f229) or 6.0 (0x756bc,
  0x1f244-0x1f24c), stored, and skips the stance and wade factors; the
  Windows cgames do the same on the stack (1.5 0x30007f08-0x30007f2b,
  rodata 0x3006b6b0 3.0 and 0x3006b66c 6.0), and 1.1d too
  (0x2e781-0x2e79c). VERIFIED. The walk scale's only caller is
  `PM_WalkMove` (0x202ec), which `PmoveSingle` calls only from its
  default arm (0x28873). Its jump table (GOT - 0x132b0, `pm_type` 0..7)
  sends 0 and 6 there, 1 and 7 to the linked arm (0x286f1), 2 to 0x28663
  (noclip, whose move 0x2084c reads the air scale 0x1eec9 at 0x20a34), 3
  to 0x28699 (UFO, 0x20bae), 4 to 0x28628 and 5 to 0x286cf. VERIFIED,
  disassembly. INFERRED, off that table: no move reaches the walk scale's
  3.0 and 6.0, so vcod, which has no noclip or UFO, ports neither. The
  air scale carries the same two factors (0x1f050-0x1f079).
- `PM_Accelerate`'s `1 / ps.friction` (0x1ee49-0x1ee5a, the 1.0 at rodata
  0x7569c over `ps+0x37c`, behind `groundEntityNum != 1023` at 0x1ee36):
  the module's only store to `ps+0x37c` is `ClientSpawn`'s 1.0 (0x3ce4b,
  `movl $0x3f800000`); the field is the wire's `friction` (offset 892).
  VERIFIED, disassembly. INFERRED: the factor is exactly 1 and the rate
  it scales is already a stored float, so the multiply and its store
  change no bit; vcod leaves it out (`pmove::accelerate_stored`). vcod's
  server sends `friction` 1.0 as well.
- `PM_StepSlideMove` (0x29942) forms the step-up's end on the stack
  (`stepSize + 1` plus the start's z, 0x29ccf-0x29ce2) and stores what
  the up trace left, `(stepSize + 1) * fraction - 1` (0x29d46-0x29d5f);
  the down pass takes the step off the end and stores it, then the snap
  of 9 when on a ground plane, stored again (0x29e83-0x29eac).
  `PM_ClipVelocity` (0x1e8e5) stores the dot, the overclipped backoff and
  each component's change; `VectorNormalize` (0x32f73) stores the squared
  length, the length and its reciprocal. VERIFIED.

**Which stores move a snapshot.** `moves_ab`'s `MOVES_EXACT=1` counts how
many snapshots' predicted origins are retail's to the bit, past the
gate's 0.05 tolerance. Before the slope work below: the 1.5 moves capture
1152 of 1222, its turns 1033 of 1100, UO's sprint 876 of 900, both seat
captures and the three uofoot ones the same, with and without each of
these stores ported (each tried alone: the accelerate, the friction, the
walk's stored wish speed, the clip, the down pass's two subtractions).
VERIFIED (measured). INFERRED: the per-frame velocity snap swallows a
velocity's last bit, so a store that only rounds it differently changes
an origin once in thousands of frames. The one store that changes what a
body does is the air accelerate's floor: `pmove::accelerate_stored` for
1.5's and UO's walk and air, `pmove::air_accelerate` for 1.1's air
(`pmove::tests::a_wish_under_100_in_the_air_accelerates_at_100s_rate`).

The slope misses that remained were the ground's z, one or two ulps
under retail's on the slope; `Arith::Kept` collision drops the moves
capture to 1118. Closed by the steps below, each count against the line
before:

- The terrain capsule clip. VERIFIED, 1.5b `cod_lnxded` 0x8055aab
  (reached from 0x8056463 and 0x805648b), disassembly: the same steps as
  1.1d's 0x8052a58 with every variable stored: `d_e` and `d_s` (`fstps
  -0x20`, `-0x1c`), their difference (`-0x88`), the near-pad fraction
  (`-0x24`), both barycentric distances (`-0x3c`, `-0x40`). The corner
  sweep (0x8056695-0x8056920) stores the offset, the separation, the dot
  and the discriminant. The edge sweep (0x80562c5-0x8056690) stores the
  offset, its three frame dots, the separation, the delta's two frame
  dots, `bq`, `aa`, the discriminant, the run along the edge and both
  normal factors, and stores the normal after its `u` term and again
  after its `v` term. It reads the triangle records 0x8054176 builds and
  the planes the UO build's 0x805403e makes (`Terrain::point_clip_stored`
  already read those). Ported as `Terrain::clip_capsule`'s `stored` arm,
  which `CollisionWorld` takes under `Arith::Stored`: 1.5 moves 1152 to
  1218 of 1222, turns 1033 to 1098 of 1100, UO sprint 876 to 896 of 900.
  VERIFIED (measured).
- `PM_ClipVelocity` (0x1e8e5) stores the dot, the backoff and each
  component's change, and `VectorNormalize` (0x32f73, the one
  `PM_SlideMove`'s and `PM_WalkMove`'s calls bind to through the PLT; the
  copy at 0x42f73 in the export is the same code) stores the squared
  length, the length and its reciprocal. VERIFIED, disassembly. With the
  slide's crease (`CrossProduct`, the stored normalize, and the stored dot
  at 0x296ad and 0x296f3), the stored plane test (0x293b5) and the walk's
  flattened `pml.forward` and `pml.right` clipped and normalized the
  stored way (0x2008c-0x203f5): 1.5 moves 1222 of 1222, turns 1100 of
  1100, UO's pole run 361 to 370 of 372 and the pillar run 372 of 372.
  VERIFIED (measured). vcod: `pmove::clip_velocity`'s `stored` arm,
  `x87::normalize_stored`.
- `PM_Friction`'s stores (the control, the drop, the new speed and its
  ratio, above): with `pmove::friction_stored` in, the one snapshot left,
  UO's 8 ms low-window jump (`mp_streets-dm-slope-8ms-crouchjump-low.txt`,
  ct 22491), matches, and so do one more of UO's pole run (371 of 372) and
  the 50 ms cassino crouch jump (136 of 136); no count drops at the 1.1 or
  the UO install. VERIFIED (measured). From ct 22466 the body slides along
  patch 462 over terrain 1536, and the second bump's crease test
  (`dot(clipped, plane_i) >= 0`, kept on the stack, 0x295c8-0x29623)
  reads a dot of a few 1e-8 whose sign any one-ulp change in the walk's x
  or y velocity flips (a gcc -O0 x87 build of Q3's slide on vcod's inputs
  gives vcod's +3.5e-8, so the slide itself was not the source).
  VERIFIED, `client-probes/probe_slidenormals` on UO 1.51b: `bulletTrace`'s
  normals on terrain 1536 and patch 462 are vcod's floats to the bit,
  residual 0 in every component. Trap: 1.1d's unrounded friction on a 1.5
  or UO build puts the walk's velocity into the slide an ulp off
  retail's.
- The air wish direction. VERIFIED, 1.5b `PM_AirMove` (0x1fe8d): it
  zeroes `pml.forward[2]` and `pml.right[2]`, runs both through
  `VectorNormalize`, sums `right * rightmove + forward * forwardmove` in x
  and y with z 0, and normalizes that. vcod's air wish takes x and y
  straight off the cosine and sine of the yaw. Ported the 1.5b way it
  moves no snapshot of any 1.5 or UO capture (every count above the
  same, measured), so vcod keeps the plain form. The walk's `pml.forward`
  already comes from `AngleVectors` as 1.5b computes it
  (`pmove::view_vectors`: the angles to radians as floats, `fsincos`
  stored, the products rounded once, 0x43e18).

## Unchanged

Every `pm_*` tunable (1.5 rodata 0x75624-0x75664 against 1.1's
0x70824-0x70864), jump height 39, the 500 ms jump gate, the 300 ms ladder
re-grab, the spread kick of 64 capped at 255, `PM_SlideMove`,
`PM_ClipVelocity`, the accelerate's floor and caps, `PM_LadderMove`,
`PM_AirMove`, the dead friction, `PM_SetWaterLevel`, the stun and fall
damage. VERIFIED.

## What vcod does with this

`pmove::MoveRules` (`V1_1`, `V1_5`, `Uo`) picks the patch.
`PlayerState::rules` carries it, set by `predict::from_wire` off the
connection's protocol, so the client predicts 1.5 on a 1.5 server; a
protocol 6 `vcod-server` runs it for every client, bots included
(cod15-protocol-diff.md 13). Under `V1_5` (and `Uo`, through
`MoveRules::v15()`) pmove runs:

- sections 2 and 3 and all of 4;
- from section 5: the friction, ground trace and near-ground sight flag,
  step, ladder, land anim, footstep and talk-button changes, `PM_trace`
  (`MoveWorld::pm_trace`, cod11-player-clip.md 13), `ps.speed`,
  `ps.gravity` and the fly move;
- from section 6: `rechamberWhileAds`, the bolt delay, `disableWeapon`
  and the server's gun kick;
- section 7's stores on the server (`Arith::Stored`), section 8's on the
  client (`Arith::Msvc`).

`pm_flags` 0x2000 keeps its place through the wire rewrite and 0x100000
lands on `flags::PMF_WEAPON_DISABLED`. The client reads the bob cvars out
of the mirror on a 1.5 server (`pmove::BobCvars`, the defaults elsewhere)
and draws the eye bob on both. The ADS-prone zeroing runs on every patch
(`coduo-movement.md` 13).
