# Demos: `record`, `stoprecord`, `demo`

What the retail client writes into a demo and how it plays one back, read
out of `CoDMP.exe` 1.1 (image base `0x00400000`, md5 in
`cod11-events-and-fx.md`), `CoDMP15.exe` 1.5 and `CoDUOMP.exe` 1.51.
Function names follow the Quake III functions of the same shape. vcod's implementation is `crates/common/src/net/demo.rs` (the
format), `NetClient::start_recording` / `play_demo` / `pump_demo` in
`crates/common/src/net/mod.rs` (the record and the read), and
`crates/client/src/play/demos.rs` (file names, the automatic demo).

## 1. Commands and cvars

`CL_Init` (`0x411e60`) registers `record` with `0x40e540`, `stoprecord`
with `0x40e430` and `demo` (the string at `0x56754c`) with `0x40eb40`
(VERIFIED, the registrar `0x428840` calls). It registers the cvars
`timedemo`, `cl_avidemo`, `cl_forceavidemo` and `cl_freezeDemo` beside
them (VERIFIED, the strings). `timedemo` is `"0"` with no flags (VERIFIED,
the `Cvar_Get` at `0x411fd8`, handle `0x15f706c`) and `cl_freezeDemo`
`"0"` with 0x100, `CVAR_TEMP` (VERIFIED, `0x411f8e`, handle `0x15f7074`).
`nextdemo` is never registered: `CL_NextDemo` finds it by name
(`0x439280`) and reads `""` when it is missing (VERIFIED, `0x40ed80`).
1.5 and UO register the two the same (VERIFIED, `CoDMP15.exe` and
`CoDUOMP.exe` `CL_Init`, the `"cl_freezeDemo"` and `"timedemo"` calls).
vcod has the three commands, `timedemo`, `cl_freezeDemo` and `nextdemo`,
and `cl_avidemo` and `cl_forceavidemo` (section 9).

- `record [name]` prints `record <demoname>` with more than one argument,
  `Already recording.` while recording, and `You must be in a level to
  record.` unless `cls.state` (`0x155f2c0`) is 6, `CA_ACTIVE` (VERIFIED,
  the three compares and strings in `0x40e540`). A name writes
  `demos/<name>.dm_1`; none tries `demos/demo0000.dm_1` up to
  `demo9999` (`0x40e4b0` formats `demo%i%i%i%i`) and takes the first that
  does not open (VERIFIED). It prints `recording to %s.`.
- `stoprecord` prints `Not recording a demo.` when not recording, else
  writes the trailer, closes the file and prints `Stopped demo.`
  (VERIFIED, `0x40e430`). `CL_Disconnect` (`0x40ef90`) calls it while
  recording (VERIFIED).
- `demo <name>` takes exactly one argument (`playdemo <demoname>`
  otherwise) and refuses on a listen server (VERIFIED, `0x40eb40`). A name
  that already ends in `.dm_1` opens `demos/<name>`, any other
  `demos/<name>.dm_1` (VERIFIED, the `.dm_%d` compare). It disconnects,
  sets `clc.demoplaying` (`0x15ef004`), sets `cls.state` 3 and reads
  messages until the state reaches 5, the gamestate parsed (VERIFIED, the
  loop on `0x155f2c0`).

The number after `.dm_` is a literal per binary: 1 in 1.1 (`0x40e540`), 2
in 1.5 (`CoDMP15.exe` `0x40f210`), 3 in UO (`CoDUOMP.exe` `0x40fb80`)
(VERIFIED, the `push` before each `demos/%s.dm_%d` format). Each retail
client reads only its own.

vcod writes `demos/` under the server's `fs_game` directory, else under
the player's mod directory, else under the base directory its config sits
in, and reads from the same list. Its `demo` plays all three: a bare name
takes the first of `.dm_1`, `.dm_2`, `.dm_3` that exists, and the number
picks the protocol the file is parsed under.

## 2. The file

A demo is a run of records closed by a trailer (VERIFIED, `0x40e3d0`
`CL_WriteDemoMessage`, `0x40e430`, `0x40e9c0` `CL_ReadDemoMessage`):

```
long  serverMessageSequence   (clc.serverMessageSequence, 0x15de99c)
long  length
byte  message[length]
...
long  -1
long  -1
```

The message is what `CL_PacketEvent` (`0x410ff0`) holds after the netchan:
the bytes from `msg.readcount` on, where `readcount` sits past the
sequence the netchan read (VERIFIED, `edi` loaded from `msg+0x10` at
`0x4110c8` and passed to `0x40e3d0`). So a record is the plain 4-byte
`reliableAcknowledge` and the op block, Huffman-compressed as it came,
the XOR scramble already undone by `0x414780`, which runs on the bytes
after the acknowledge before the parse (VERIFIED, the call at
`0x411103`). A fragmented message is written once, whole. A record
carries no time; playback paces off the snapshots inside.

`CL_ReadDemoMessage` ends the demo (`CL_DemoCompleted`, `0x40e920`) on a
short sequence read, a length of -1, or a short message read, the last
printing `Demo file was truncated.` (VERIFIED). A length past its buffer
is a fatal error (`CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN`): 16384
bytes in 1.1 and 1.5, 32768 in UO (VERIFIED, the buffer sizes in
`0x40e9c0` and `CoDUOMP.exe` `0x410060`). The read stamps
`clc.lastPacketTime`, reads the acknowledge as `MSG_ReadLong` and hands
the message to `CL_ParseServerMessage` (`0x4165f0`) (VERIFIED).

## 3. The opening gamestate

`record` writes a gamestate of its own before any server message
(VERIFIED, `0x40e540`): into a 16384-byte buffer, plain,

```
long  clc.reliableSequence (0x15ce994)     where a server message has its ack
byte  svc_gamestate (2)
long  clc.serverCommandSequence (0x15de9a0)
per configstring i < 2048 with a non-zero string offset:
  byte svc_configstring (3), short i, bigstring
per entity baseline whose number is non-zero:
  byte svc_baseline (4), the delta from a null state, forced (0x445e40)
byte  svc_EOF (8)
long  clc.clientNum (0x15ce860)
long  clc.checksumFeed (0x15ce98c)
byte  svc_EOF (8)
```

then copies the first long and Huffman-compresses the rest behind it
(`0x4448b0`) and writes it as one record under the current
`serverMessageSequence` (VERIFIED). The configstrings are the client's
table as it stands, every `d` already applied. UO sizes the buffer 32768
and writes a vehicle (`eType` 12) baseline through its vehicle field
table (VERIFIED, `CoDUOMP.exe` `0x40fb80`); 1.5's is 1.1's (VERIFIED,
`CoDMP15.exe` `0x40f210`).

vcod builds it with `gamestate::write` and the trailing `svc_EOF`. Its ops
are byte for byte the retail server's gamestate past that message's
leading empty `svc_serverCommand` (VERIFIED, the committed
`gamestate.bin` in `demo.rs`'s test).

## 4. Waiting for a full frame

`record` sets `clc.demowaiting` (`0x15ef008`) (VERIFIED, `0x40e6ea`).
While it is set, `CL_WritePacket` (`0x40ba50`) sends `clc_moveNoDelta`,
the same arm as `cl_nodelta`, no valid snapshot, or a last message that
was not the snapshot's (VERIFIED, the compares at `0x40bc44`..`0x40bc6f`),
and `CL_PacketEvent` writes nothing (VERIFIED, `0x41111a`).
`CL_ParseSnapshot` (`0x415bb0`) clears it on a frame whose delta byte is
0, before it checks the frame (VERIFIED, `0x415c5f`). The write follows
the parse, so the first message written is that full frame's.

## 5. Playback

`CL_SetCGameTime` (`0x404d60`) reads the demo (VERIFIED):

- in `CA_PRIMED` (5), the map loaded and no snapshot yet: skip the first
  frame (`clc.firstDemoFrameSkipped`, `0x15ef00c`, set at `0x404d85`),
  then one message a frame (`0x40e9c0` at `0x404d90`), and when that
  message brought a snapshot `CL_FirstSnapshot` (`0x404d00`, called at
  `0x404da8`) makes the state 6 (VERIFIED, the calls and the store).
  The same call then goes on into the `CA_ACTIVE` arm below (INFERRED:
  the state compare after `CL_FirstSnapshot` falls through to it), so
  that frame already counts as `timedemo`'s first and reads past the
  first snapshot. Only `CL_Disconnect`'s clear of `clc` (`0x15ce860`,
  0xa201 dwords, the `rep stos` at `0x40f0ac`) resets the skip
  (VERIFIED, no other store to `0x15ef00c`), and `demo` disconnects
  first, so each playback skips one frame and a gamestate inside the
  demo skips none;
- in `CA_ACTIVE` (6), unless the demo plays with `cl_freezeDemo` set, the
  clock: `realtime + serverTimeDelta - cl_timeNudge` (the nudge kept to
  +-30), never below `cl.oldServerTime`. Frozen, `cl.serverTime`
  (`0x1434a64`) keeps last frame's value;
- with `timedemo` set, `clc.timeDemoFrames` (`0x15ef014`) counts up and
  the time becomes `clc.timeDemoBaseTime` (`0x15ef01c`) plus 50 ms a
  frame, frozen or not (VERIFIED, `* 0x32` at `0x404d60`).
  `clc.timeDemoStart` (`0x15ef018`) takes `Sys_Milliseconds` on the
  first such frame. `CL_FirstSnapshot` sets the base to the first
  snapshot's time (VERIFIED, `0x404d00`), so after a gamestate inside a
  demo the count carries on from the new base;
- then messages while `cl.snap.serverTime <= cl.serverTime`, stopping if
  the state leaves 6, which a gamestate in the demo does.

Nothing lifts the frame cap under `timedemo`. `Com_Frame` (`0x437f40`)
waits until `Sys_Milliseconds` has moved `minMsec` past the last frame,
`minMsec` being 1, or `1000 / com_maxfps` (integer) when `com_maxfps`
(handle `0x163b3ec`) is above 0 and `dedicated` (`0x163a200`) is 0
(VERIFIED, `0x438040`..`0x438062`). `com_maxfps` is `"85"`, flag 1,
`CVAR_ARCHIVE` (VERIFIED, the `Cvar_Get` at `0x437807`). `timedemo`'s
handle `0x15f706c` is read only by `CL_SetCGameTime` and
`CL_DemoCompleted` (VERIFIED, its two loads in the decompile). 1.5's
`Com_Frame` (`CoDMP15.exe` `0x43a3d0`) and UO's (`CoDUOMP.exe`
`0x43c580`) make the same test and register `com_maxfps` `"85"`, flag 1
(VERIFIED, the default strings at `0x5a1a6c` and `0x59821c`). So a
retail `timedemo` runs at most about 90 frames a second by default, and
a `com_maxfps` over 1000 waits not at all. vcod registers it `"0"`
instead of `"85"` so a high-refresh screen under vsync is not held near
90 fps; `com_maxfps 85` restores retail's pacing.

`CL_AdjustTimeDelta` (`0x404bf0`) returns at once while a demo plays
(VERIFIED, its first compare on `0x15ef004`), so the delta set by the
first snapshot never slews; thawing a frozen demo jumps to the wall
clock's time. `CL_WritePacket` sends nothing while one plays (INFERRED:
`0x40b940` and `0x40ba6d` read `0x15ef004`).

A demo's messages carry the recorder's `reliableAcknowledge`, past the
playback's own reliable sequence of 0: the first message after the
gamestate in `crates/common/tests/fixtures/net/snapshots.bin` acks 1
(VERIFIED, its first record). Q3's `CL_AddReliableCommand` (`code/client/cl_main.c`) takes
`clc.reliableSequence - clc.reliableAcknowledge` on `int`s, so that
negative gap is no overflow (VERIFIED, the GPL source). vcod's
`NetClient::send_reliable` compares the gap signed for that reason: an
unsigned compare wraps and drops the playback with `reliable command
overflow` on the `cp` after the load, before a frame draws
(`a_demo_ack_past_the_sequence_is_no_overflow`). CoDMP.exe's own compare
has not been located (its overflow error string is not in the image).

`CL_DemoCompleted` (`0x40e920`), on the trailer or a short read: with
`timedemo` set it prints `%i frames, %3.1f seconds: %3.1f fps` (the
frames, the ms since `timeDemoStart` times 0.001, and frames times 1000
over those ms) when the ms are above 0, then `CL_Disconnect` and
`CL_NextDemo` (`0x40ed80`) (VERIFIED). `CL_NextDemo` prints `CL_NextDemo:
%s` through the developer print and, when `nextdemo` is not empty, sets
it to `""` and runs it as a command line (VERIFIED). 1.5 and UO keep the
same strings (VERIFIED, `CoDMP15.exe` and `CoDUOMP.exe`).

`CL_KeyEvent` (`0x40dc30`) turns a key press into Esc while a demo plays
(or a cinematic, state 7 or 8) and no key catcher is up, when the key
number is under 0x80 or is 0xc8 (`MOUSE1`), clearing `nextdemo` first
(VERIFIED). The console keys 0x60 and 0x7e are taken before it
(VERIFIED). Esc then, in state 6 with a demo playing, opens the main menu
(`UI_SetActiveMenu` 1) where a live game opens the script main menu
(VERIFIED, the `0x15ef004` compare before `VM_Call(uivm, 7, 1)`;
`cod11-front-end.md` 13). The demo keeps playing under it (INFERRED:
nothing on that path disconnects).

### The cgame under `cg.demoPlayback`

`CL_CGameRendering` (`0x404bc0`) passes `clc.demoplaying` as
`CG_DRAW_ACTIVE_FRAME`'s third argument (VERIFIED, `VM_Call(cgvm, 3,
cl.serverTime, stereo, demoplaying, 0, 0)`). The cgame's `vmMain`
(`cgame_mp_x86.dll` `0x300203b0`) hands it in `edx` to
`CG_DrawActiveFrame` (`0x30033980`), which stores it in `cg.demoPlayback`
(`0x301e2148`) (VERIFIED). It is read in three places, each beside
`pm_flags` 0x10000 (`PMF_FOLLOW`), `0x301e172c` and `0x301e1bac`
(VERIFIED):

- `CG_PredictPlayerState` (`0x300294f0`): any of them takes
  `CG_InterpolatePlayerState` (`0x300291d0`) and returns, so nothing is
  predicted;
- `CG_TransitionSnapshot` (`0x3002faa0`): any of them runs the
  playerstate transition off the old and new snapshots: `0x300287f0`
  when the playerstate's `+0xf0` changed and `+0xfc` is non-zero, then
  `0x30028740` and `0x30028be0`, as for a followed player;
- `CG_EntityEvent` (`0x3001dc10`) event 0x8f, `EV_STEP_VIEW`: none of
  them may be set for the step smoothing to run.

So a demo draws as a follow does. vcod draws the recorder as a followed
player off the snapshots: no prediction, the playerstate's events and
damage feedback off the snapshot. vcod has no `EV_STEP_VIEW` smoothing
online either.

vcod reads through `play::demos::read_frame`: `NetClient::pump_demo` up
to the first gamestate, nothing while the map loads, then
`NetClient::pump_demo_primed` (the playback's one skipped frame, then a
message a frame) until a snapshot seeds the clock, and in that frame and
every later one while the newest snapshot is no later than the frame's
time (`ServerClock::time_at`, one snapshot past the drawn time),
breaking at a gamestate. Its frames are paced by `com_maxfps` (archived;
"0" in vcod, "85" on retail) as `Com_Frame`'s are. The clock runs `ServerClock::update_demo`,
which never adjusts, holds under `cl_freezeDemo` and steps 50 ms a frame
under `timedemo` (`play::demos::TimeDemo` keeps the count for the whole
demo). It answers no script menu, sends nothing, and on the demo's end
prints the `timedemo` line, returns to the main menu and runs `nextdemo`.
A key under 0x80 or `MOUSE1` with no menu, console or chat field up opens
the main menu and clears `nextdemo`.

### The systeminfo and a missing map

`CL_SystemInfoChanged` (`0x415eb0`) reads `sv_serverid` and returns while
a demo plays: no cheat state, no `FS_PureServerSetLoadedPaks`, no
`FS_PureServerSetReferencedPaks`, no systeminfo cvars, so no `fs_game`
and no download list (VERIFIED, the `0x15ef004` compare ahead of those
calls). The map then loads off the player's own search path. The cgame's
`CG_Init` loads the collision map first (trap 0x1f after `LOADING...
collision map`, `0x30022f90`) (VERIFIED), and `CM_LoadMap` (`0x419c90`)
drops with `EXE_ERR_COULDNT_LOAD` and the `maps/mp/<map>.bsp` path when
the file does not open (VERIFIED, `Com_Error(1, ...)` with the string at
`0x5650bc`); `exe.str` reads `Couldn't load %s` (VERIFIED,
`localized_english_pak1.pk3`). That trap 0x1f is `CM_LoadMap` is
INFERRED from Q3's `CG_CM_LOADMAP` order.

vcod's demo keeps the player's `fs_game` and pure list, fetches nothing,
and a map no pak holds fails the load with that text in the error popup
(`MapLoader::without_downloads`).

## 6. Automatic demos

1.1's cgame has no automatic demo. 1.5's `CG_ServerCommand`
(`cgame_mp15.dll` `0x3002f660`) handles `C` and `D` (VERIFIED, cases 0x43
and 0x44): with `cg_autodemo` 1 (`atol`), `C` runs `record %s-%s` through
trap 0xf5 and `D` runs `stoprecord`. The first `%s` is the `name` cvar
kept to `a-z`, `A-Z` and `0-9` (`0x3002f470`), the second trap 0xf4,
which `CoDMP15.exe` answers at `0x46af40` with the local date and time
formatted `yyyyMMdd-` and `HHmmss` (VERIFIED, the `GetDateFormatA` and
`GetTimeFormatA` pictures). UO's cgame keeps both, and its killcam's
`C <ms>` reads as a start (`coduo-diff.md`, "Server and client
commands"). The game module sends them from `autoDemoStart` and
`autoDemoStop` under `g_autodemo` (`cod15-protocol-diff.md` 8).

Both cgames carry `cg_autodemo` in their cvar table as `"0"`, flags 1,
`CVAR_ARCHIVE` (VERIFIED, the rows at `cgame_mp_x86.dll` 1.5 `0x30077030`
and `uo_cgame_mp_x86.dll` `0x300858d0`: handle, name, default, flags). 1.1's
cgame has no such string (VERIFIED). vcod registers it the same and runs
the two commands on a 1.5 or UO server.

## 7. Not done

- No retail client has played a vcod demo: no retail client runs here.
  Section 9 gates the other direction and vcod's writer against retail's
  layout.

## 8. The `RECORDING` line

`SCR_DrawScreenField` (`0x416dd0`) calls `SCR_DrawDemoRecording`
(`0x416b10`, at `0x416f19`) in `CA_ACTIVE` only, right after
`CL_CGameRendering`, so it draws over the game and under the menus and
console (VERIFIED, case 6 of the state switch, jump table `0x416fb0`).
The whole switch runs only when the UI's `UI_IS_FULLSCREEN` (`VM_Call`
6, the call at `0x416e60`) returns 0 (VERIFIED), which
`ui_mp_x86.dll` `0x40016ab0` answers 1 for any menu on the open stack
with window flag 4 (visible) and its fullscreen field (`+0xbc`) set
(VERIFIED): under a fullscreen menu there is no line. While `clc.demorecording` (`0x15ef000`) is set it formats
`RECORDING %s: %ik` with `clc.demoName` (`0x15eefc0`, the name `record`
was given or `demoNNNN`, 63 characters at most) and the file's size from
`ftell` (or the zip entry's offset for a file in a pk3) shifted down 10
(VERIFIED). It draws through `0x416a30`, the console's text helper, at x
5, y 470 with the colour at `0x5419c0`, (1, 1, 1, 1) (VERIFIED). That
helper takes window pixels and hands the renderer `x * 640 / vidWidth`,
`(y + 16) * 480 / vidHeight`, font 5 at scale `480 / vidHeight / 3` and
an 8 px cell (VERIFIED, `0x416a30`): the console's own text, its baseline
486 window px down whatever the resolution, so below a 480-line window's
edge.

1.5 (`CoDMP15.exe` `0x4185a0`) and UO (`CoDUOMP.exe` `0x419a60`) print
`RECORDING %s: %ik` when `cg_showdemoname` is 1 and `RECORDING: %ik`
otherwise, the size pushed for both (VERIFIED, the `push` ahead of the
branch at `0x41862c`), and draw it white (colour code `'7'`) straight at
(5, 479) on the 640x480 grid, font 5, scale 1/3, an 8 unit cell
(VERIFIED, the renderer call at `0x418689`): it scales with the window.
`cg_showdemoname` is `"1"`, archived, in both cgames' cvar tables
(VERIFIED, the rows beside `cg_autodemo`). 1.5 and UO call it in `CA_ACTIVE` only, right after `CL_CGameRendering`,
and only when `UI_IS_FULLSCREEN` returns 0 (VERIFIED: `CoDMP15.exe`
`0x4188d0`, the UI call at `0x418970`, table `0x418af4`, case 6 at
`0x418a2b` calling `0x4185a0` at `0x418a37`; `CoDUOMP.exe` `0x419d80`,
`0x419e1d`, table `0x419fa0`, case 6 at `0x419ed8` calling `0x419a60` at
`0x419ee4`). Under a fullscreen menu their switch still runs
`CL_CGameRendering` for states 4 to 6, but not the line (VERIFIED, the
second switch).

vcod draws it in the online frame's live phase over the HUD and under
the menus, none of them fullscreen (`Console::recording_line`,
`Ui::fullscreen`), its size the bytes the
`DemoWriter` has written.

## 9. Interchange with retail

`cargo run -p vcod-common --example demo_dump -- <file.dm_N>` prints a
file record by record: sequence, length, acknowledge, the leading server
commands, the gamestate's counts and serverinfo keys, a snapshot's time
and delta byte. The facts below rest on its output and on two gates in
`crates/common/src/net/mod.rs`.

### 9.1 A retail-recorded demo

The 1.5 install holds one demo, `main/demos/Timedemo1.dm_2` (333920
bytes). Its first record's serverinfo reads `protocol 2`, `shortversion
1.2`, `sv_hostname Ned's World`, `mapname mp_dawnville`, `g_gametype tdm`
(VERIFIED, `demo_dump`). vcod speaks protocols 1, 6 and 22 only
(`demo::protocol_of_extension`; VERIFIED), so vcod did not write it. A
retail 1.2 client recorded it on a 1.2 server (INFERRED: a client joins
only a server of its own protocol). No 1.2 binary is here, so that 1.2's
`record` also formats a literal 2 is INFERRED from the file's name.

Its records, VERIFIED (`demo_dump`):

- 1534 records, then the two -1 longs.
- Record 1, sequence 26080, acknowledge 37: `svc_gamestate` first, with no
  server command ahead of it. Then `serverCommandSequence` 520, 259
  configstrings, 19 baselines, `clientNum` 14 and `svc_EOF`. That is
  section 3's message.
- Record 2, sequence 26081: a snapshot with delta byte 0. Every later
  record holds one snapshot, at consecutive sequences, with delta byte 1
  (2 once, record 658). The full frame section 4 waits for came in the
  next message.

`a_retail_recorded_demo_plays_and_vcod_writes_its_records_alike` reads
the file from `COD_DIR` and passes vacuously without it. VERIFIED (the
gate, at the 1.5 install):

- `DemoReader` reads every record and stops on the trailer, and
  `DemoWriter` writes the records back byte for byte.
- `demo::gamestate_message`, built from record 1's parsed gamestate and its
  acknowledge, comes out identical to record 1, compressed bytes included.
- `demo` under protocol 6 turns each of the 1533 later records into a new
  frame with a later `serverTime`, then ends on the trailer.

That 1.2's netfield tables match 1.5's for every field those frames use
is INFERRED (no frame lost its delta base).

### 9.2 A retail capture framed as retail frames it

Each retail server was captured live with `vcod --net-probe --save-demo
--capture-tag <tag>` (`docs/dev/retail.md`): 1.1d and 1.5b on mp_carentan
`dm`, UO 1.51b on mp_cassino `dm`, whose two tanks give the gamestate two
`eType` 12 baselines. The recorder joined allies and walked the
`--probe-slope` route beside a second probe on axis. The probe:

- writes every server message past the gamestate as
  `crates/common/tests/fixtures/net/snapshots-<tag>.bin`, beside
  `gamestate-<tag>.bin`;
- runs vcod's `record` from the first snapshot, `record`'s earliest point,
  into `<tag>.dm_<n>`.

In all three captures the record started on the first snapshot's message
(2 on 1.1, 3 on 1.5 and UO). The next message was a full frame, answering
the recorder's `clc_moveNoDelta` (VERIFIED, `demo_dump`).

`a_retail_framed_demo_plays_and_vcod_writes_it_alike` writes each capture
again with `RetailDemo`. That writer is separate from vcod's and follows
sections 2 to 4:

- the opening gamestate at the record point;
- nothing until a message whose snapshot has delta byte 0, read off the
  message's plain head;
- then each message under its own sequence, and the trailer.

The record point and the recorder's `reliableSequence` come off the live
file's first record, because no server message carries them. VERIFIED (the
gate) for each patch:

- The opening's plain ops match the server's gamestate past its leading
  commands, with `serverCommandSequence` the client's. UO's included,
  whose tank baselines go through the vehicle field table.
- vcod's recorder, replayed on the same messages, writes `RetailDemo`'s
  file byte for byte, and so did the live probe.
- vcod's `demo` plays the file frame for frame as the capture parsed. Its
  closing configstring table matches, it ends on the trailer and it sends
  nothing.

| Patch | File | Messages captured | Frames played |
|---|---|---|---|
| 1.1 | 33285 bytes | 591 | 590 |
| 1.5 | 33927 bytes | 592 | 591 |
| UO | 41411 bytes | 584 | 583 |

### 9.3 1.5 and UO write as 1.1 does

VERIFIED (`objdump` of `CoDMP15.exe` and `CoDUOMP.exe`):

- `CL_WriteDemoMessage` (1.5 `0x40f0a0`, UO `0x40fa10`) writes
  `clc.serverMessageSequence` (`0x18a9a5c`, `0x4dd899c`), then
  `cursize - headerBytes`, then the bytes from `headerBytes`, through the
  same `FS_Write` (`0x42c6e0`, `0x42e290`) as 1.1.
- `stoprecord` (`0x40f100`, `0x40fa70`) writes the two -1 longs.
- `CL_PacketEvent` keeps the `readcount` past the netchan in `edi`
  (`0x4121e5`, `0x413323`). It reads the acknowledge, decodes and parses,
  then calls the write only while `clc.demorecording` (`0x18ba6c8`,
  `0x4de9608`) is set and `clc.demowaiting` (`0x18ba6d0`, `0x4de9610`)
  is clear (`0x412231`..`0x412246`, `0x41336d`..`0x413382`).
- `record` sets `demowaiting` (`0x40f3b4`, `0x40fd18`), the snapshot parse
  clears it on a delta of 0 or less (`0x41748f`, `0x41891c`), and
  `CL_WritePacket` reads it on the no-delta test (`0x40c78d`, `0x40ce82`).

The differences stay those of sections 1 and 3: the `.dm_<n>` number, UO's
32768-byte buffer and UO's vehicle baselines.

### 9.4 vcod to retail

No retail client runs here: the 1.1, 1.5 and UO clients are Windows
binaries, and none has been run under Wine. So a retail client has not
played a vcod demo. That one would is INFERRED from 9.1 and 9.2: vcod
writes a retail 1.2 recording's opening byte for byte and frames retail's
messages exactly as retail's writes do.

## 10. Screenshots and `cl_avidemo`

The renderer's `screenshot` and `screenshotJPEG`, the 1.5 and UO cgame's
`cg_autoscreenshot`, and `cl_avidemo`'s shot a frame. 1.1 addresses
first, 1.5 (`CoDMP15.exe`) and UO (`CoDUOMP.exe`) after them; the code is
the same in all three unless a bullet says otherwise. vcod:
`crates/client/src/screenshot.rs`, the read-back in
`crates/client/src/renderer/capture.rs`, the clock in
`crates/client/src/frame_clock.rs`.

### 10.1 Commands

- The renderer registers `screenshot` (`0x4b25a0`; 1.5 `0x4bd480`, UO
  `0x4c2d60`) and `screenshotJPEG` (`0x4b2770`; 1.5 `0x4bd650`, UO
  `0x4c2f60`) through `ri.Cmd_AddCommand` (pushes at `0x4b44d1` and
  `0x4b44e1`; 1.5 `0x4bf3c0`, `0x4bf3d0`; UO `0x4c4d22`, `0x4c4d32`).
  VERIFIED.
- Both read their first argument the same way, exact compares (VERIFIED):
  `levelshot` (`0x558870`) writes `levelshots/<map>.tga` (`0x558890`), a
  128 x 128 box-filtered TGA, and always prints `Wrote`; `savegame`
  (`0x558864`) with three arguments and a non-empty second writes
  `<arg2>.jpg` (`0x55887c`) at 512 x 512, quality 90, with no line;
  `silent` (`0x55885c`) only silences; any other lone argument names the
  file; everything else numbers it.
- vcod reads the same arguments and writes neither `levelshot` nor
  `savegame`.

### 10.2 Names

- A named shot is `screenshots/%s.tga` (`0x558848`) or
  `screenshots/%s.jpg` (`0x558810`), the argument as typed, with no
  existence check, so it overwrites (VERIFIED).
- A numbered one is `screenshots/shot%04i.tga` (`0x5588dc`) or `.jpg`
  (`0x5588a4`). Each format keeps its own static counter (`0x571794` TGA,
  `0x571798` JPEG; 1.5 `0x5b2294`, UO `0x5ce90c`), -1 in `.data` and so 0
  on the first shot. The counter steps past every name `FS_ReadFile`
  finds and stores the found number plus one, so a later shot never looks
  below it (VERIFIED). INFERRED: the read searches the whole path, so a
  copy in a pak counts as taken; vcod looks in the folder it writes to.
- The TGA gives up at 9999 and never writes `shot9999.tga`; the JPEG
  gives up at 10000. Either prints `ScreenShot: Couldn't create a file`
  (`0x558824`) whatever `silent` says (VERIFIED).
- INFERRED (Q3's `FS_WriteFile`): the path is under `fs_homepath` and the
  game folder, the mod's under `fs_game`. vcod writes
  `<install>/<fs_game or main or uo>/screenshots/`.

### 10.3 Format

- TGA (`0x4b0f90`; 1.5 `0x4bbe30`, UO `0x4c1800`): an 18-byte header, zero
  but byte 2 = 2 (uncompressed truecolour), bytes 12-15 the width and
  height little-endian, byte 16 = 24 and byte 17 = 0, so the rows run
  bottom-up; then `glReadPixels(0, 0, vidWidth, vidHeight, GL_RGB)` with R
  and B swapped, the whole window (VERIFIED).
- JPEG (`0x4b10a0`; 1.5 `0x4bbf50`, UO `0x4c1920`): read as `GL_RGBA`,
  then `SaveJPG(name, 95, w, h, 1, buf)` (`0x4ed610`; quality pushed at
  `0x4b1126`, 1.5 `0x4bbfec`, UO `0x4c19bc`). The 1 feeds the rows from
  the last, so the file is top-down (VERIFIED, `0x4ed820`). No cvar sets
  the quality. INFERRED: libjpeg's default sampling.
- vcod writes the same TGA header and rows and a baseline JPEG at 95 off
  the `image` crate's encoder, not byte for byte libjpeg's.

### 10.4 Gamma and the frame

- The read-back pixels go through the gamma table (`0x11de160`, TGA at
  `0x4b1040`, JPEG through `0x4ea4b0`) only when `overbrightBits`
  (`0x16c55b8`) is above 0 and the device takes gamma (`0x16c3a8c`)
  (VERIFIED; 1.5 `0x199d798` / `0x199bc6c`, UO `0x4885898` /
  `0x489a04c`). The table carries the overbright shift (section 2 of
  `cod11-gamma.md`). INFERRED: a full screen shot shows what the screen
  did, and a windowed one, with no overbright bits, the framebuffer before
  `r_gamma`. vcod reads the swapchain in the first case and the gamma
  pass's scene before the ramp in the second (`screenshot::pre_gamma`).
- The command reads the framebuffer in its own handler, no render command
  (VERIFIED). 1.1 never calls `glReadBuffer` (VERIFIED, no call through
  `0x16c438c`), so it reads `GL_BACK`; 1.5 and UO switch to `GL_FRONT`
  around the read and back (VERIFIED, 1.5's qgl pointer at `0x199c56c`,
  UO's `0x4899504`), the frame last shown. INFERRED: a console
  `screenshot` runs from `Cbuf_Execute` (`0x428300` at `0x438212`) before
  that frame draws. vcod reads the next frame it draws, console and menus
  included.
- The line is `Wrote %s` (`0x558884`) with the path from `screenshots/`,
  unless `silent` (VERIFIED; 1.5 `0x596e7c`, UO `0x5a4704`). INFERRED (Q3):
  it prints whether the write took or not; vcod prints `Failed to open`
  before it when the write fails.

### 10.5 `cg_autoscreenshot`

- 1.1's cgame has no such string (VERIFIED). 1.5's and UO's carry it in
  their cvar tables, `"0"`, flags 1 (`CVAR_ARCHIVE`) (VERIFIED, the rows
  at `0x30077024` and UO `0x300858c4`).
- `CG_ServerCommand`'s `E` (1.5 `0x3002fa61`, UO `0x3003b221`) reads it
  with `trap_Cvar_VariableStringBuffer` into two bytes and goes on only
  when `atoi` is 1 (VERIFIED). It reads `name`, keeps `a-z`, `A-Z` and
  `0-9` (`0x3002f470`, the `^` of a colour code dropped and its digit
  kept), and runs `va("screenshotJPEG %s-%s", stamp, name)` (`0x30064988`)
  through trap 0xf5 (VERIFIED, the pushes: the stamp from trap 0xf4 at
  `0x30031c40`, then the name). The stamp is `yyyyMMdd-HHmmss` (section
  6). So the file is `screenshots/<yyyyMMdd-HHmmss>-<name>.jpg`. UO's
  traps are 0xf9 and 0xfa (VERIFIED).
- Nothing in the cgame shoots at intermission (VERIFIED, the only
  reader is `E`). The game module sends `E` from `autoScreenshot` under
  `g_autoscreenshot` (`cod15-protocol-diff.md` 8). vcod runs it the same
  (`screenshot::auto_command`).

### 10.6 `cl_avidemo`

- `CL_Init` registers `cl_avidemo` and `cl_forceavidemo` `"0"` with no
  flags (VERIFIED, `0x411fe3`, `0x411ff8`; 1.5 `0x4134f9`, UO
  `0x41468a`).
- `CL_Frame(msec)` (`0x4112f1`; 1.5 `0x4127c2`) when `cl_avidemo` and
  `msec` are both non-zero: in `CA_ACTIVE` (6) or under
  `cl_forceavidemo` it runs `screenshot silent` (`0x566d40`) at once
  (`Cmd_ExecuteString` `0x428a80`), then sets `msec =
  ftol(1000.0f / cl_avidemo * timescale)` (`0x569090`, `timescale` at
  `0x163a234`, the truncating `0x538be0`), 1 when that is 0 (VERIFIED).
  Float math, not Q3's integer `1000 / n`. No demo check: a demo plays in
  state 6 (INFERRED).
- UO (`0x41391f`) takes `fabs(cl_avidemo)` in the step and runs
  `screenshot silent` for a positive count and `screenshotjpeg silent`
  (`0x593ddc`) for a negative one (VERIFIED).
- vcod: the frame clock moves the step a frame, the demo and the
  session reading it, and the next frame is written. A negative count on
  1.1 and 1.5 steps the clock back on retail; vcod holds it at 1 ms.
- INFERRED (Q3's `ui_shared`): the 1.1 cgame's menu key handler queues
  `screenshot` (`0x300613c0`) on F12 while `developer` is set
  (`0x300451a8`). vcod has no such key.

