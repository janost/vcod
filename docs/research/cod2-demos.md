# CoD2 demos: `record`, `stoprecord`, `demo`, `timedemo`

What the retail CoD2 1.3 client writes into a demo and how it plays one
back. Source: the Mac 1.3 client (`cod2mp_mac13_i386`, with symbols),
checked live against `CoD2MP_s.exe` 1.3 under Wine. CoD1's account is
`cod11-demos.md`; this doc lists what CoD2 keeps and changes. vcod:
`crates/cod2/src/net/demo.rs` (the format, `Recording`),
`vcod_cod2::net::Client::{start_recording, server_message, demo_message}`
(the write and the read), and `crates/client/src/play/cod2.rs`
(`Cod2Session::play_demo`, `start_recording`, the demo clock).

## 1. Commands and dvars

- `CL_Init` (Mac 0x14b032) adds `record` (`CL_Record_f`), `stoprecord`
  (`CL_StopRecord_f`), and both `demo` and `timedemo` with
  `CL_PlayDemo_f`, autocompleting from `demos` with extension `dm_1`
  (VERIFIED, the `Cmd_AddCommand` and `Cmd_SetAutoComplete` calls).
  `timedemo` is a command, not a dvar as on CoD1: `CL_PlayDemo_f` sets
  `clc` +0x4018c to whether `Cmd_Argv(0)` is `timedemo` (VERIFIED, the
  `I_stricmp` at 0x14c288). vcod drops its CoD1 `timedemo` cvar on a CoD2
  install (`Shell::use_cod2_cvars`) and `timedemo <name>` plays the demo
  timed.
- `cl_freezeDemo` (bool, 0, flags 0x1000), `cl_avidemo` (int),
  `cl_forceavidemo` and `nextdemo` (string, `""`, 0x1000) are registered
  there (VERIFIED, the `Dvar_Register*` calls). vcod reads all four
  (`cl_avidemo` in section 8).
- `record [name]` (Mac 0x149856): more than one argument prints `record
  <demoname>`; recording (`clc` +0x40184) prints `Already recording.`;
  `cls.state` other than 8 (`CA_ACTIVE` in CoD2's numbering) prints `You
  must be in a level to record.` (VERIFIED, the three compares and
  strings). A name is cut to 63 characters (`I_strncpyz(.., 0x40)`) and
  formatted `demos/%s.dm_%d` with a literal 1; no name tries
  `demo%04i` from 0 to 9999 with `FS_FileExists` and takes the first free
  (VERIFIED). It prints `recording to %s.`, or `ERROR: couldn't open.`
  when `FS_FOpenFileWrite` fails (VERIFIED).
- `stoprecord` (Mac 0x14724e): `Not recording a demo.` when not
  recording, else two -1 longs, the close and `Stopped demo.` (VERIFIED).
- `demo <name>` / `timedemo <name>` (Mac 0x14c288): exactly one argument
  (else `%s <demoname>` with the command's name), refused on a listen
  server (`listen server cannot play a demo.`), then `CL_Disconnect`. A
  name ending in `.dm_1` (case folded) opens `demos/<name>`, any other
  `demos/<name>.dm_1`; a file that does not open is
  `Com_Error(1, "EXE_ERR_NOT_FOUND\x15<path>")` (VERIFIED). It closes the
  console, sets `cls.state` 5, `clc.demoplaying` (+0x40188) and the
  timedemo flag, and reads messages while the state is 5 or 6 (VERIFIED,
  the loop), then clears `firstDemoFrameSkipped` (+0x40194) (VERIFIED).

## 2. The file

The extension number is 1 (VERIFIED, the `push 1` before `demos/%s.dm_%d`
in `CL_Record_f` and `.dm_%d` in `CL_PlayDemo_f`): a CoD2 `.dm_1` shares
its name with a CoD1 1.1 demo but not its ops, so vcod picks the reader by
the install's game, not the extension.

The records are CoD1's (VERIFIED, `CL_WriteDemoMessage` Mac 0x1497d4,
`CL_StopRecord_f`, `CL_ReadDemoMessage` 0x14c132):

```
long  serverMessageSequence   (clc +0x20138)
long  length
byte  message[length]          from msg.readcount: the ack, then the op block
...
long  -1
long  -1
```

`CL_PacketEvent` (Mac 0x14ced6) stores the packet's first long as
`serverMessageSequence`, reads the acknowledge, unscrambles the block
with `CL_Netchan_Decode` and parses, then, when the parse left the message
legible, recording is on and `demowaiting` (+0x40190) is clear, writes
`CL_WriteDemoMessage(msg, readcount)` with the `readcount` it saved
after the netchan, so the record is the plain 4-byte acknowledge and the
unscrambled, still compressed op block (VERIFIED). A message whose
acknowledge is more than 128 behind `reliableSequence` is not parsed live
or from a demo (VERIFIED, the `- 0x80` compare in both).

`CL_ReadDemoMessage` reads into a 0x20000-byte `LargeLocal`: a length
past it is `CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN` (fatal); a short
sequence read, a -1 length or a short message read end the demo through
`CL_DemoCompleted`, the last printing `Demo file was truncated.`
(VERIFIED). vcod: `vcod_cod2::net::demo::{MAX_MSGLEN, reader}`.

## 3. The opening gamestate

`CL_Record_f` writes into a 0x20000-byte buffer (VERIFIED):

```
long  clc.reliableSequence (+0x130)
byte  svc_gamestate (1)
long  clc.serverCommandSequence (+0x2013c)
per configstring i < 0x800 with a non-zero offset (cl +0x270c):
  byte svc_configstring (2), short i, bigstring
per baseline (cl +0xb3260, 0x400 of 0xf0 bytes) whose first word is non-zero:
  byte svc_baseline (3), MSG_WriteDeltaEntity from a zeroed state, forced
byte  svc_EOF (7)
long  clc.clientNum (+8)
long  clc.checksumFeed (+300)
byte  svc_EOF (7)
```

then keeps the first long plain, `MSG_WriteBitsCompress`es the rest
behind it, and writes one record of length `compressed + 4` under the
current `serverMessageSequence` (VERIFIED). It sets `demowaiting`
(VERIFIED, the store of 1 at +0x40190). The baseline test reads the
state's first word, its number, so entity 0's baseline is skipped
(INFERRED: `entityState_s` opens with `number`, as the delta writer's
first field in `cod2-protocol.md` 3.2).

vcod builds it with `write_gamestate` from the live configstring table
(`demo::gamestate_message`). The opening record of retail's own demo
re-encodes byte for byte from its parsed gamestate (VERIFIED,
`a_retail_demo_parses_every_record`), and on the committed
`mp_toujane-spectate.bin` session vcod's opening ops equal the server's
gamestate message past its leading server commands (VERIFIED,
`a_recorded_retail_session_plays_back_snapshot_for_snapshot`).

## 4. Waiting for a full frame

While `demowaiting` is set, `CL_WritePacket` (Mac 0x188154) writes
`clc_moveNoDelta` (op 1 in its 3 bits), as for `cl_nodelta` or no valid
snapshot (VERIFIED, the compare on +0x40190 before
`MSG_WriteBits(.., 0, 3)`), and `CL_PacketEvent` writes nothing.
`CL_ParseSnapshot` (Mac 0x1d5cb2) clears it on a frame whose delta byte
is 0 (VERIFIED, the store of 0 in the `deltaNum < 1` arm). The write
follows the parse, so the first message after the gamestate record is
that full frame's. vcod: `Recording::waiting`, read by
`Client::can_delta`.

## 5. Playback

`CL_SetCGameTime` (Mac 0x17ba76), VERIFIED unless labelled:

- state 7 (primed): with a demo playing, the first call sets
  `firstDemoFrameSkipped` (`clc` +0x40194) and returns; later calls read
  one message. When `cl.newSnapshots` (+0x2708) is set and the snapshot's
  `snapFlags & 2` is clear the state becomes 8, `serverTimeDelta` is the
  snapshot time minus `cls.realtime`, `oldServerTime` the snapshot time,
  and `clc.timeDemoBaseTime` (+0x401ac) the snapshot time; `activeAction`
  runs once. The call then carries on into the state 8 arm.
- state 8: unless the demo plays with `cl_freezeDemo` set,
  `serverTime = realtime + serverTimeDelta`, never below
  `oldServerTime`. No `cl_timeNudge` term (CoD1 has one).
  `CL_AdjustTimeDelta` (Mac 0x17b544) returns at once while a demo plays
  (its compare on +0x40188).
- with `timedemo`: while the csv handle (`clc` +0x4019c) is 0 it opens
  `demos/timedemo_%s_mode_%i.csv` through `FS_FOpenFileWrite`, with
  `cl.mapname` (cl +0x24710) cut past its last `/` or `\` and short of
  its last `.`, and `Dvar_GetInt("r_mode")`; `timeDemoStart` (+0x401a4)
  takes `Sys_Milliseconds` while it is 0, else with a handle the frame
  writes `%i,%i\n`, the frame count (+0x401a0) before this frame's and
  the ms since the last frame (+0x401a8, stamped every frame); the count
  goes up and `serverTime` is `base + frames * 50` (VERIFIED, the
  stores, the format strings and the call). `CL_InitCGame` (Mac
  0x17b412) formats `cl.mapname` as `maps/mp/%s.%s` from the serverinfo's
  `mapname` and `GetBspExtension` (VERIFIED), so the name is the map's.
  `Dvar_GetInt` (Mac 0x51dca) returns an int or enum dvar's integer, the
  enum's index, and `atoi` of any other's string (VERIFIED, the type
  test), so `r_mode` reads as its index in the mode list.
- then messages while `cl.snap.serverTime <= serverTime`, stopping when
  the state leaves 8 (a gamestate in the demo).

`CL_DemoCompleted` (Mac 0x14c096): with `timedemo`, when the ms since
`timeDemoStart` are above 0, `%i frames, %3.1f seconds: %3.1f fps`; the
csv closes and its handle clears; `CL_Disconnect`; `CL_NextDemo` (Mac
0x1472ea), which runs `nextdemo` once and clears it (VERIFIED).

vcod's csv: `Cod2Session::write_timedemo_csv` hands the session the
directory `record` writes to and `r_mode`'s index (`Shell::int`, vcod's
own `r_mode` -1 reading as -1); `TimedemoCsv` opens and writes as above
and closes as the demo ends (`timedemo_writes_a_row_a_frame_after_the_first`,
on the committed retail demo).

`CG_EntityEvent`'s `EV_STEP_VIEW` arm (Mac 0x1e4650) runs only while `cg`
+8, `CG_DrawActiveFrame`'s demo argument (Mac 0x1d4a50, stored from its
second parameter; `SCR_UpdateFrame` passes `clc.demoplaying != 0`), is
0, along with `cg_nopredict` and `cg_synchronousClients` (VERIFIED, the
tests and the store). A demo therefore draws no step smoothing. The
landing arm beside it has no such test, so a demo's eye dips on a
landing (INFERRED, the arm's only test is the client number).
`CG_CalcViewValues` (Mac 0x1d3b28) adds both, with the bob, on its first
person arm, demo or not (INFERRED, its branches; cod2-movement.md 18). vcod's demo eye is the snapshots'
lerped eye: no bob and no landing dip. Its played view has both and the
step (`Cod2Session::recoil_frame`, which takes no step in a demo).

`CL_SystemInfoChanged` (Mac 0x1d535a) reads `sv_serverid` and returns
while a demo plays: no cheat state, no `sv_iwds`, no `fs_game` (VERIFIED,
the compare on +0x40188 right after `sv_serverid`). vcod's demo keeps
the player's search path and fetches nothing.

`CL_KeyEvent` (Mac 0x1412e2): a key press under 0x80, or 200, with a demo
playing (or state 1 or 2) and no key catcher clears `nextdemo` and goes
to the Esc arm (VERIFIED); there, in state 8 with a demo playing, Esc
takes `UI_SetActiveMenu(1)`, the main menu, where a live game takes 2
(VERIFIED, the case 8 compare on +0x40188).

`SCR_UpdateFrame` (Mac 0x172d88) draws `RECORDING %s: %ik` in state 8
while recording, the name always, the `FS_FTell` size shifted down 10,
colour `'7'`, at (5, 479) through `CalcScreenPlacement`, font scale
1/3, only when `UI_IsFullscreen` returns 0 (VERIFIED). CoD2 has no
`cg_showdemoname`. vcod draws it so over a CoD2 session.

vcod's playback: `Cod2Session::play_demo` reads up to the gamestate, the
caller loads the map as a server's, and `Cod2Session::demo_frame` runs
the arms above; the frame draws the recorder from the snapshots with no
prediction and sends nothing. That the cgame skips prediction during a
demo is INFERRED from `CG_PredictPlayerState` (Mac 0x1ddafa) opening on
`cg` +8, the `CG_DrawActiveFrame` argument `SCR_UpdateFrame` passes as
`demoplaying != 0`.

## 6. Live runs

All in one `unshare -rn` namespace (loopback only):

- vcod recorded retail `cod2_lnxded` 1.3 (`mp_toujane`, dm, a moving
  second client): `vcod --net-probe 127.0.0.1:28970 --game cod2
  --probe-secs 20` with `VCOD_COD2_RECORD`, 31291 bytes. Played back
  headless (`--probe-demo`): 395 messages, 394 snapshots (393 deltas), 0
  parse errors, server time 17300..36950, 28 entities. VERIFIED live.
- Retail `CoD2MP_s.exe` 1.3 under Wine played that file as `timedemo
  vcod1`: its console printed `393 frames, 20.1 seconds: 19.5 fps` and
  the screenshots show mp_toujane through the recorder's spectator view.
  vcod's own `--probe-timedemo` counts the same 393 frames. VERIFIED live.
- Retail recorded a demo of its own: `set activeAction "record wine1"`
  then `devmap mp_toujane` (a CD-keyless prefix can host but cannot
  `connect`, `cod2-protocol.md` 16), killed after about a minute without
  `stoprecord`, so the file stops at its last 4096-byte flush (28672
  bytes, no trailer). Its first record is sequence 3, `reliableSequence`
  2. vcod plays it: 858 records, 857 snapshots (856 deltas), 0 parse
  errors, then `Demo file was truncated.` VERIFIED live; committed as
  `crates/cod2/tests/fixtures/demos/mp_toujane-retail.dm_1`.
- vcod recorded vcod-server (`--game cod2`, a joined `--probe-team
  allies` probe): 18 KB, 296 snapshots, 0 parse errors on playback.
  VERIFIED live.

## 7. Not done

- A demo's eye has no bob or landing dip (5).
- CoD2's stock `ui_mp/main.menu` has no demo list (its only demo item
  is `demo_only`, the retail demo build's notice), so the front end has
  none (VERIFIED, the stock menus in `iw_06.iwd`).
- A recorded demo started before the first snapshot: vcod refuses
  `record` until a snapshot is in, retail until `cls.state` 8.

## 8. Screenshots and `cl_avidemo`

Read out of `gfx_d3d_mp_x86_s.dll` (image base `0x10000000`) and
`CoD2MP_s.exe` 1.3, the names off the Mac build. CoD1's account is
`cod11-demos.md` 9; vcod's code is the same (`screenshot.rs`,
`renderer/capture.rs`, `frame_clock.rs`).

### 8.1 Commands and names

- `R_RegisterCommands` (`0x100052c0`) adds `screenshot` (`0x101924b8`,
  handler `0x100052a0`) and `screenshotJpeg` (`0x101924a8`, `0x100052b0`),
  both into `R_ScreenshotCommand` (`0x10026790`) with TGA or JPEG
  (VERIFIED). The exe's bindable list spells it `screenshotJPEG`
  (VERIFIED, `.data` `0x5dd184`, `0x5dd198`); commands match case
  folded.
- The first argument as on CoD1 (VERIFIED, case-sensitive compares):
  `levelshot` (`0x10196f68`) writes a 128 x 128 `levelshots/<map>.tga`
  (`0x10196f90`), `savegame` (`0x10196f5c`) a 512 x 512 `<arg2>.jpg`,
  `silent` (`0x10196f54`) silences, a lone other argument names
  `screenshots/<arg>.<ext>` (`0x10196f40`) with no existence check, and
  everything else numbers.
- Numbers: one static `lastNumber` (`0x109be634`, 0 at start) for both
  formats. Above 9999 it fails at once; else it formats
  `screenshots/shot%04i.%s` (`0x101970c4`, extensions `0x10196f74` and
  `0x10196f78`) and steps while `FS_FileExists` (exe `0x421ea0`,
  `fs_homepath/<fs_gamedir>/<name>`) finds the file. A free number of 9999
  or more prints `ScreenShot: Couldn't create a file` (`0x10196f1c`, silent
  or not), so `shot9998` is the last; else it takes the number and keeps
  the next (VERIFIED, `0x10026890`..`0x1002691f`).
- The file goes through `FS_WriteFile` (exe `0x423340`) under
  `fs_homepath/<fs_gamedir>/`, its result ignored; a failed open prints
  `Failed to open %s` (VERIFIED). `Wrote %s` (`0x10196f84`) follows
  unless `silent`, written or not (VERIFIED, `0x100269a3`).
- A developer F12 (key 178, keynames `0x5cddf0`) runs `screenshot` from
  the UI's key handler (exe `0x53ec91`, string `0x5c3010`) while
  `developer` is set (VERIFIED). vcod has no such key.

### 8.2 Format and frame

- TGA (`0x100265a0`): the 18-byte header is zero but byte 2 = 2, bytes
  12-15 the size, byte 16 = 24 and byte 17 = `0x20`, top-left origin;
  rows top-down, B, G, R (VERIFIED). CoD1's are bottom-up.
- JPEG: the DLL's own libjpeg, no D3DX; `R_SaveJpg` (`0x10014860`) at a
  fixed quality 90 (`push 0x5a`, `0x10026666`, `0x1002698e`), baseline,
  three components, rows top-down; the build reads byte 0 as blue
  (`rgb_ycc_convert` `0x10042c50`), so the BGR buffer keeps its colours
  (VERIFIED).
- The capture runs in the command (`R_GetFrontBufferData` `0x10026270`):
  an `A8R8G8B8` scratch surface, `GetFrontBufferData`, a read-only
  `LockRect`, bytes 0-2 a pixel (VERIFIED). Full screen it reads the
  window's size; windowed a monitor-sized surface, cropped to the
  client rectangle, so whatever covers the window comes along (VERIFIED,
  `MonitorFromWindow`, `GetMonitorInfoA`, `ClientToScreen`). INFERRED: the
  frame last presented. Its failures print `^1ERROR: cannot take
  screenshot: ...` (`0x10197084`, `0x10197040`, `0x10196fe8`,
  `0x10196fa4`) silent or not (VERIFIED).
- Gamma: `r_gamma` reaches the screen only through `SetGammaRamp`
  (`0x10030c8f`); nothing in the capture maps the pixels (VERIFIED).
  INFERRED (D3D9): the front buffer holds the bytes before the ramp, so a
  shot is darker than the screen at `r_gamma` above 1. vcod reads the
  gamma pass's scene before the ramp, the next frame it draws.
- There is no `cg_autoscreenshot` (VERIFIED, no such string in the exe or
  the DLL); nothing shoots at intermission or on a server command.

### 8.3 `cl_avidemo`

- `cl_avidemo` (`0x5a3220`) is an int, 0, 0 to `0x7fffffff`, flags
  0x1000; `cl_forceavidemo` (`0x5a3210`) a bool, 0, 0x1000 (VERIFIED).
- `CL_RunOncePerClientFrame(msec)` (exe `0x40f770`, called from
  `Com_Frame` at `0x435051`), when both are non-zero: in `CA_ACTIVE` (8,
  `0x609fe0`) or under `cl_forceavidemo` it runs `screenshot silent`
  (`0x5a3590`) at once through `Cmd_ExecuteString` (`0x4214c0`); then
  `msec = ftol(1000.0f / cl_avidemo * com_timescaleValue)` (`0x5c3ed8`,
  `0xc2610c`), 1 when 0; `cls.realtime` (`0x68a520`) moves by it
  (VERIFIED, `0x410b8c`..`0x410c00`). The step holds in any state; the
  shot waits for a game or a demo past its first snapshot.
- INFERRED (the Mac `Com_Frame`): `com_timescaleValue` is 1 unless
  `timescale` or `fixedtime` scales the frame, and the call runs before
  the frame draws, so each shot is the frame before. vcod reads
  `timescale`, steps its frame clock (`cls.realtime`; the demo clock and
  the session read it) and writes the frame it then draws.

