# Shellshock in CoD 1.1 MP

What a retail 1.1 client does with `self shellShock(name, duration)` once
the three playerstate fields arrive. The server half is in
`cod11-gsc-language.md` section 9, "Indexing past the first wait".

Modules: `cgame_mp_x86.dll` 1.1 (base `0x30000000`), `CoDMP.exe` 1.1
(`0x00400000`), `game.mp.i386.so` 1.1d (file addresses). Where a register
argument is cited, it was read off the disassembly at the instruction
given. Syscall numbers are cases of `CL_CgameSystemCalls` (`CoDMP.exe
0x401df0`).

## 1. Stock use

- VERIFIED, `pak4.pk3`: six files, `scripts/death.shock`, `default.shock`,
  `default_nomusic.shock`, `groggy.shock`, `pain.shock`, `stop.shock`.
- VERIFIED, grep of every `maps/MP/gametypes/*.gsc` in the 1.1 paks: no
  stock MP script calls `shellShock` or `precacheShellShock`; `dm.gsc` has
  `stopShellshock()` three times, all commented out (lines 580, 625, 662).
  The callers are single-player (`maps/_shellshock.gsc`, `_mortar.gsc`,
  `_utility.gsc`, `stalingrad.gsc`, `truckride.gsc` and others). Nothing in
  1.1 MP shocks a player on its own, a grenade included.
- VERIFIED, `soundaliases/*.csv` in the paks: `shellshock_loop`
  (`misc/shellshock_loop.wav`, volume 0.6, channel `shellshock`, streamed,
  looping), `shellshock_loop_silent` (the same file at volume 0),
  `shellshock_end` (`misc/shellshock_exit.wav`, 1.25, `announcer`,
  streamed) and `shellshock_end_abort` (`null.wav`, 0, `announcer`), none
  with a loadspec, so every map has them.

## 2. Registration

- VERIFIED, the gamestate's media loop `0x30020da0`: for configstrings
  1101..1115 (`0x44d` up, 15 slots, stopping at the first empty one) it
  loads `scripts/<name>.shock` (`0x3002ea70`) and builds that slot's record
  (`0x3002eb30`); a load that fails is `CG_Error("couldn't register shell
  shock '%s' -- see console\n")` (`0x30020750`), which drops the client.
- VERIFIED, the configstring-change handler `0x3002c6b0`: an index in
  1100..1115 with a non-empty name loads and builds the same way, and a
  failure there returns quietly. The server's indexer starts at 1101, so
  1100 never carries a name.
- VERIFIED, `0x30033cd3`-`0x30033d1b`: the records sit at `0x301d29fc +
  i * 0x7c`, `i` the configstring's slot (1..15). Record 0 belongs to the
  console command (section 7).

## 3. The file

- VERIFIED, `0x3002ea70`: `FS_ReadFile("scripts/%s.shock")` (syscalls
  0xf, 0x10, 0x12), then syscall 0x15 with the name table at `0x300748a0`
  (27 entries), then syscall 8 (`Cvar_Update`) over the 27 `vmCvar_t`s at
  `0x30074910`.
- VERIFIED, syscall 0x15 is `CoDMP.exe 0x438eb0`: `COM_Parse` (`0x449c10`)
  takes a token, `Q_stricmp` matches it against the table, a match takes
  the next token on the same line (`allowLineBreaks` 0) and `Cvar_Set`s it
  (`0x439650`), a miss prints `^3WARNING: unknown cvar '%s' in file '%s'`;
  either way the rest of the line is skipped. At the end, any of the 27 not
  set prints `^1ERROR: the following cvars were not specified in file
  '%s'` and each name, and the file fails.
- VERIFIED, `COM_Parse`: `//` and `/* */` comments are skipped, so
  `groggy.shock`'s commented second block is inert.
- VERIFIED, the six stock files: every one carries
  `cg_shock_viewKickFadeTime`, which is not in the table, so each load
  prints the unknown-cvar warning for it and the value goes nowhere.
- VERIFIED, `CG_RegisterCvars` table at `0x30075260` (16-byte rows: vmCvar,
  name, default, flags): the 27 defaults are `screenBlendTime 0.4`,
  `screenBlendFadeTime 1`, `viewKickPeriod .75`, `viewKickRadius .05`,
  `sound 1`, `soundFadeInTime 0.25`, `soundFadeOutTime 2.5`,
  `soundLoopFadeTime 1.5`, `soundLoopEndDelay -3.0`, `soundRoomType
  underwater`, `soundWetLevel 0.5`, `soundModEndDelay 2`, `volume_auto
  0.1`, `volume_menu 1.0`, `volume_weapon 0.5`, `volume_voice 0.2`,
  `volume_item 0.1`, `volume_body 0.1`, `volume_local 1.0`, `volume_music
  1.0`, `volume_announcer 1.0`, `volume_shellshock 1.0`, `mouse 1`,
  `mouse_maxpitchspeed 90`, `mouse_maxyawspeed 90`,
  `mouse_sensitivityscale 0.5`, `mouse_fadeTime 2` (all `cg_shock_`
  prefixed), every one flagged `0x200`.
- VERIFIED, `0x3002e990` (syscall 0x14, `CoDMP.exe 0x438e30`): the reverse,
  the 27 cvars written as `name "value"` lines to `scripts/<name>.shock`.
  INFERRED: a developer's save; nothing in the MP cgame calls it.

## 4. The record

VERIFIED, `0x3002eb30` (stores read off the disassembly, `0x3002eb33` to
`0x3002f180`). "ms" is `(int)(value * 1000 + 2^-30)` under the x87's
round-to-nearest (`0x30069478`, `9.313226e-10`), "ms>=1" that with a floor
of 1, "0..1" a clamp where a NaN lands on 0.

| Offset | From | Transform |
|---|---|---|
| `+0x00` | constant | 3000, the kick's fade (`0x3002ec13`) |
| `+0x04` | `viewKickPeriod` | `0.001 / max(period, 0.001)`, kicks per ms |
| `+0x08` | `viewKickRadius` | as is |
| `+0x0c` | `screenBlendFadeTime` | ms>=1 |
| `+0x10` | `screenBlendTime` | ms>=1 |
| `+0x14` | `sound` | `integer != 0` |
| `+0x18` | `soundFadeInTime` | ms>=1 |
| `+0x1c` | `soundFadeOutTime` | ms>=1 |
| `+0x20` | `soundWetLevel` | 0..1 |
| `+0x24` | `soundRoomType` | `strncpy` 15, `+0x33` = 0 |
| `+0x34`..`+0x58` | ten `volume_*` | 0..1 each, by channel (below) |
| `+0x5c` | `soundModEndDelay` | ms, signed |
| `+0x60` | `soundLoopFadeTime` | ms>=1 |
| `+0x64` | `soundLoopEndDelay` | ms, signed |
| `+0x68` | `mouse` | `integer != 0` |
| `+0x6c` | `mouse_fadeTime` | ms>=1 |
| `+0x70` | `mouse_sensitivityscale` | as is |
| `+0x74` | `mouse_maxpitchspeed` | as is |
| `+0x78` | `mouse_maxyawspeed` | as is |

The volumes. VERIFIED, the stores: `auto` to `+0x34`, `menu` `+0x38`,
`body` `+0x3c`, `item` `+0x40`, `weapon` `+0x44`, `voice` `+0x48`, `local`
`+0x4c`, `music` `+0x50`, `announcer` `+0x54`, `shellshock` `+0x58`.
VERIFIED, `0x3002f2d0` hands `+0x34`..`+0x58` to syscall 0xd8 as a
ten-float array in that order, and `CoDMP.exe 0x44e140` stores element `i`
as channel `i`'s multiplier (`0x8e079c + i * 12`, the per-channel factor
`cod11-sound-system.md` section 2 left unnamed), channel order `auto, menu,
weapon, voice, item, body, local, music, announcer, shellshock`. So the
four 3D channels after `menu` are crossed: `cg_shock_volume_body` scales
the weapon channel, `item` voices, `weapon` items and `voice` bodies.
`default.shock` takes weapons to 0.1 and items to 0.5.

## 5. The frame

VERIFIED, the frame function `0x30033980`: after the snapshot processing
it picks the record (`0x30033cd3`): the playerstate behind the pointer
at `0x301e2160` (`ps` at `+0xc`; INFERRED: `cg.snap`) with
`shellshockIndex` (`+0x3e8`) of 0 takes
record 0 with the console's start and duration (`0x3020d5d0`, `0x3020d5d4`),
anything else record `i` with `shellshockTime` (`+0x3ec`) and
`shellshockDuration` (`+0x3f0`), all three into `0x3020d5b0`..`b8`. Then
`0x3002f7d0` (`0x30033d21`).

VERIFIED, `0x3002f7d0`: with `elapsed = cg.time - start` (`cg.time`
`0x30207148`), `start == 0` or `elapsed < 0` is the reset `0x3002f270`
(below); otherwise the sound half, the mouse half and the kick half run
and syscall 0x57 gets `elapsed < duration`.

VERIFIED, `0x3002f270`: the sound reset `0x3002f190` (volumes all 1 from
`0x30060d10`, syscall 0xd9 `"generic"` at 0, and with the end latch
`0x3020d5bc` set it clears it and plays `shellshock_end_abort`), the mouse
scale `0x3020d5c0` back to 1, syscall 0xf0 (0, 0), the kick
(`0x3020d5c4`, `c8`) to 0, syscall 0x57 0.

### Sound (`0x3002f2d0`)

VERIFIED, with `+0x14` clear the reset `0x3002f190` and nothing else.
Otherwise, all in ms:

- `until_off = modEndDelay - elapsed + fadeOut + duration`. With
  `until_off < fadeOut`, `f = until_off / fadeOut`; otherwise with
  `elapsed < fadeIn`, `f = elapsed / fadeIn`; otherwise 1. A negative `f`
  is 0.
- Each channel's volume is `(record - 1) * f + 1` (syscall 0xd8, fade 0).
- Syscall 0xd9 gets `"generic"` and 0 when `f == 0`, else the record's
  room and `f * wet`.
- `until_loop = loopEndDelay - elapsed + loopFade + duration`; while it is
  above 0, syscall 0xc3 starts or refreshes the blend of `shellshock_loop`
  and `shellshock_loop_silent` at `1 - until_loop / loopFade` (0 at
  least), on entity `0x3ff` at the origin `0x300608e8`.
- `end = start + duration + loopEndDelay`. Before `end`, a set end latch is
  cleared and `shellshock_end_abort` plays. At or after it, when the latch
  is not `end` already, the latch takes `end` and `shellshock_end` plays
  with `cg.time - end` as syscall 0xc2's last argument.

INFERRED: the 0xc3 blend weights the second alias by the factor
(`CoDMP.exe 0x44d670` blends a record field as `t * second + (1 - t) *
first`), so the loop plays at 0.6 until `end` and fades to the silent
twin's 0 over `loopFade`. INFERRED: 0xc2's last argument is how far into the
sound to start. INFERRED: `null.wav` on the announcer channel and entity
`0x3ff` ends a playing `shellshock_end` through the replacement rule
(`cod11-sound-system.md` section 2).

INFERRED, the formulas on `default.shock`'s numbers (5 s shock): every
channel ducks over 0.25 s, holds until 0.75 s before the end, then recovers
over 2.5 s. The loop plays from the start and fades over 2 s from 1.5 s
before the end, when the exit sound plays.

VERIFIED, `CoDMP.exe 0x44f3c0` (syscall 0xd9): the room name goes through
`0x44f340` (an index 0..25 or one of `generic, paddedcell, room, bathroom,
livingroom, stoneroom, auditorium, concerthall, cave, arena, hangar,
carpetedhallway, hallway, stonecorridor, alley, forest, city, mountains,
quarry, plain, parkinglot, sewerpipe, underwater, drugged, dizzy,
psychotic`, table `0x575b48`) into `AIL_set_digital_master_room_type` and
`AIL_set_3D_room_type`; the wet level becomes a target the sound frame
ramps to (`0x8e0844`..`0x8e084c`) and every sample's reverb levels are set
to dry 1, wet that level (`AIL_set_sample_reverb_levels`,
`AIL_set_stream_reverb_levels`, `AIL_set_3D_sample_effects_level`).

### Mouse (`0x3002f5b0`)

VERIFIED: with `+0x68` clear, scale 1 and syscall 0xf0 (0, 0). Otherwise
`remaining = duration - elapsed`; under `mouse_fadeTime` (and at least 1)
`f = remaining / fade` and the scale is `(sensitivity - 1) * f + 1` with
caps `maxpitch / f`, `maxyaw / f`; at or past the fade the record's own
values; under 1 the reset.

VERIFIED, the scale (`0x3020d5c0`) multiplies the zoom's sensitivity
factor (`0x3020b5f4`) before syscall 0x55 (`0x30033e97`), as
`cod11-console.md` section 5 has it.

VERIFIED, syscall 0xf0 stores pitch and yaw caps at `CoDMP.exe
0x143a980`, `0x143a984`, and `CL_MouseMove` (`0x40b240`) clamps the frame's
yaw turn (`0x40b43e`) and pitch turn (`0x40b51e`) to `±frame_msec * cap *
0.001` when the cap is not 0: degrees a second.

### View kick (`0x3002f6b0`, `0x3002e7a0`)

VERIFIED, `0x3002f6b0` (`eax` the duration, `edx` the record, the stack
argument `elapsed`): `remaining = duration - elapsed`; under 1 the kick is
0. `f = remaining / 3000` under 3000, else 1; `amp = (3 - 2f) f^2 *
radius`. `t = elapsed * rate`, `i = floor(t)` (the `fistp` of `t -
0.49999999`), `frac = t - i`; `base = (duration * 61 + i) & 0x7f` indexes
128 points at `0x300608f8` (with the first three repeated after them, 131
in all), and each axis is the cubic `((p2 - p0 + (a frac + p0 - p1 - a))
frac) frac + p1` with `a = p3 - p2 + p1 - p0` over points `base..base+3`,
times `amp`. VERIFIED, the dev function `0x3002e720` prints such a table
(`"\t{%f, %f},\n"`) of random points in the unit disc, which is where the
six-decimal values came from.

VERIFIED, `0x3002e7a0`, called at `0x30033dbd` after the quake's
`0x30017f00` (`0x30033da9`): with either kick component non-zero, `v0 =
normalize(1, kx, ky)`, `v1 = normalize((0,0,1) x v0)`, `v2 = v0 x v1`, and
`MatrixMultiply` (`0x3003ab40`) of those rows onto the refdef's axis
(`0x302095a0`). So the forward leans by `kx` toward the view's left and
`ky` toward its up. INFERRED: the viewmodel, posed later off the refdef
(`0x300371f0` at `0x30033e04`), moves with it.

### Weapon sway

VERIFIED, `0x300361f0` (`cod11-combat.md` 15.3): the turn sway's scale is
`1 + (swayShellShockScale - 1) * (3 - 2f) f^2`, `f` the kick's fade off the
same record and times, 1 with under 1 ms left. VERIFIED, the 1.1 MP weapon
files: every one has `swayShellShockScale 5` but the three `mg42_bipod_*`
and `PTRS41_Antitank_Rifle_mp`, which have none (0, so the sway stops at a
shock's height).

### Movement

- VERIFIED, `CoDMP.exe` syscall 0x57 stores `0x143a978`, and the cmd's
  button fill (`0x40b600`, the test at `0x40b675`) sets `wbuttons` 0x4
  while it is non-zero. So a shocked client holds the bit for as long as
  `elapsed < duration`.
- VERIFIED, `game.mp.i386.so` `PM_CmdScale` (`0x2e959`): with `cmd+5 &
  4`, the wish scale takes `* 0.4` (rodata `0x70894`), last, after the
  weapon's `moveSpeedScale`.
- VERIFIED, capture (section 8): the shocked carbine runs at 89.6 and backs
  at 62.72 (the wire's snapped 90 and -63), 0.4 of the 224 and 156.8 the
  same walk reads unshocked.

### Screen blend (`0x3002e900`)

VERIFIED, called from `0x30018940` at `0x30018932` and
`0x30018c95`, after syscall 0x45 (`RenderScene`) and before the 2D
(`0x30019990`): with `start != 0`, `duration > 0` and `remaining = start +
duration - cg.time > 0`, the decay is `screenBlendTime`, or under
`screenBlendFadeTime` left `round(remaining / fade * blendTime)`; with the
capture flag (`0x3020d5cc`) set syscall 0x47 (decay), then syscall 0x46,
and the flag is set. Otherwise the flag is cleared.

VERIFIED, `CoDMP.exe`: 0x46 and 0x47 call refexport slots `+0x90` and
`+0x94` (`GetRefAPI` `0x4b4a30`), `0x4de590` and `0x4de5c0`, which queue
render commands 9 and 10 (10 only with a positive decay). The backend
(`0x4d8ab0`) runs 9 as `0x4d8620`: a call through `0x16c4af8` with
`0xde1` (`GL_TEXTURE_2D`), zeros and the screen's size (INFERRED:
`glCopyTexSubImage2D` of the whole framebuffer into the `<screen>` image),
and the capture time
(`0x16c4dfc`) from the backend's `Sys_Milliseconds` (`0x16d8a08`, set at
`0x4d6d98`). It runs 10 as `0x4d86b0`: with `since = now - capture <
decay`, alpha is `min(0.01 ^ (since / decay), 0.99) * 255` rounded
(`0x5690a8`, `0x569020`, `0x568ec0`), and the capture is drawn over the
whole screen at that alpha with the `<screen>` shader built at `0x4fda00`,
state `0x65`. INFERRED, Q3's bits: `GLS_SRCBLEND_SRC_ALPHA |
GLS_DSTBLEND_ONE_MINUS_SRC_ALPHA`.

INFERRED, from the order of the two: each shocked frame draws the last
frame's finished 3D view over this one and keeps the result. The trail
keeps 1% of a frame after the decay (400 ms in `default.shock`), 83% from
one 60 Hz frame to the next. The first frame only captures. The HUD is not
in it.

## 6. The stock files

VERIFIED, `pak4.pk3` (CRLF text, values quoted):

| File | blend / fade | kick period / radius | sound | in / out / loop fade | loop end / mod end | room, wet | mouse: sens, caps, fade |
|---|---|---|---|---|---|---|---|
| `default` | 0.4 / 1 | 0.75 / 0.05 | 1 | 0.25 / 2.5 / 2 | -1.5 / -0.75 | underwater 0.5 | 0.5, 90/90, 2 |
| `default_nomusic` | as `default`, `volume_music` 1 | | | | | | |
| `death` | 0.7 / 1 | 0 / 0 | 1 | 2 / 2.5 / 1.5 | -9999 / -0.75 | underwater 0.5 | off (1, 900000/90000, 0) |
| `groggy` | 0.3 / 1 | 0.75 / 0.05 | 0 | 20 / 2.5 / 2 | -1.5 / -0.75 | underwater 0.1 | 0.5, 90/90, 2 |
| `pain` | 0.1 / 0.15 | 0.15 / 0.005 | 0 | | | | off |
| `stop` | 0 / 0 | 0 / 0 | 1 | 0 / 0 / 0 | 0 / 0 | generic 0 | off |

`default`'s volumes: `auto` 0.1, `menu` 1, `weapon` 0.5, `voice` 0.2,
`item` 0.1, `body` 0.1, `local` 0.1, `music` 0.1, `announcer` 1,
`shellshock` 1 (before the crossing of section 4).

## 7. `cg_shellshock`

VERIFIED, `0x30013e70`: `cg_shellshock <duration> [file]`; with a file it
loads it first (and returns on a failure), then builds record 0 from the
cvars as they stand (the last file read, or the defaults) and starts it at
`cg.time` for `duration * 1000` ms (`0x3020d5d0`, `d4`). It plays only while
the playerstate's index is 0. No cheat check.

## 8. Measured

Retail 1.1d on mp_carentan, `client-probes/probe_shock.gsc` with a
`--save-shock` client (`crates/server/tests/fixtures/shellshock/`).
VERIFIED (capture):

- `shellShock("default", 2.5)` at level time 16000: the next snapshot reads
  index 1, time 16000, duration 2500. `shellShock("pain", 1.25)` over it
  at 19000: index 2 (the second precached name), 19000, 1250.
  `stopShellShock()` at 19500: 0, 0, 0. `shellShock("default", 0)` at
  20500: 1, 20500, 0. `shellShock("default", 3)` at 21500: 1, 21500, 3000.
- With the client holding `wbuttons` 0x4 through each shock, the forward
  run reads 90 and the back -63 on the wire; with it clear, 224 and -157.

`crates/server/tests/shellshock_ab.rs` replays the client's cmds on
vcod-server under the same gsc. The three fields, the origin and the
velocity match exactly on every retail snapshot, the placement's included.

VERIFIED (capture, replay): the placement snapshot (t 15000) acknowledged
commandTime 14968, and the client's first logged `!cmd` is 15018. Retail
also ran an unlogged holding cmd at 15000, sent before the probe saw the
placement (cod11-movers.md 15, "The cmd at the placement"). The gate runs
the placement frame to 14968 and that cmd at 15000 before the log's first.
Without it the first forward cmd runs from 14950 and leaves a constant 1.66
units on the origin.

## vcod

- The server writes the fields (`builtins/client.rs`); `pmove::wish` takes
  `SCALE_SHELLSHOCK` on `PmInput::shellshock` (`wbuttons` 0x4), on the
  server and in the client's prediction.
- `crates/client/src/play/shock.rs` is the cgame half: the parser and the
  record, the per-frame sound, mouse, kick and blend state, `kicked_angles`
  and `sway_scale`. `main.rs` reads the fields off the interpolation's
  older snapshot (`cg.snap`), turns the view after the quake, sets
  `wbuttons` 0x4 and the mouse caps (`play::input::PlayInput`), the
  sensitivity factor, the audio and the screen blend.
- `crate::screen_blend` runs render commands 10 and 9 on the gamma scene
  after the world's post effects (`vc_hdr`, `vc_grade`, bloom) and before
  the HUD.
- `AudioSystem::set_shellshock`: per-channel volumes in `VoiceTable`, the
  loop and the end cue on entity 1023.

Divergences:

- The room is one reverb on the main track (`audio/mod.rs`,
  `ROOM_FEEDBACK`), not Miles' 26 room types: any room but `generic` gets
  it at the wet level, the dry path kept at 1.
- `shellshock_end_abort` stops the end cue directly: vcod plays nothing for
  a `null.wav` row, so its replacement would not happen.
- A `.shock` file that fails to load is a warning and no effect, not
  `CG_Error`.
- `CL_MouseMove` clamps a frame's summed motion; vcod spends the motion
  once a frame too (`App::mouse_frame`), under the cap the frame before
  set.
