# CoD4 1.7 demos: `record`, `stoprecord`, `demo`, `timedemo`

What the retail CoD4 1.7 client (`iw3mp.exe`) writes into a demo and how
it plays one back, read off its disassembly. CoD2's account is
`cod2-demos.md`; this doc lists what 1.7 keeps and what it changes.
Function names are INFERRED; the addresses and the facts are the 1.7
binary's. No retail-recorded 1.7 demo has been read and retail has not
played vcod's, so the byte layout below rests on the binary, gated
against a retail capture written in that layout (section 8).
vcod's code: `crates/cod4/src/net/demo.rs`
(the format, `Recording`, `DemoReader`, `gamestate_message`),
`vcod_cod4::net::conn::{Received::msg, Conn::no_delta}` (the bytes a
record holds, `demowaiting`'s move), and `crates/client/src/play/cod4.rs`
(`Cod4Session::{start_recording, stop_recording, play_demo}`,
`DemoPlay`, the demo clock).

`clc` sits at 0x8f4ce0 (VERIFIED, the `and eax,0x8f4ce0` select in
`CL_Disconnect` at 0x4696db). The offsets below are off it: `clientNum`
+4, `checksumFeed` +0x130, `reliableSequence` +0x134,
`serverMessageSequence` +0x2013c, `serverCommandSequence` +0x20140, the
last archive row written +0x4014c, `demoName` +0x40150 (64 bytes),
`demorecording` +0x40190, `demoplaying` +0x40194, `timedemo` +0x40198,
`demowaiting` +0x4019c, `firstDemoFrameSkipped` +0x401a0, the file
+0x401a4, the timedemo csv +0x401a8, its frames +0x401ac, start +0x401b0,
last frame +0x401b4 and base time +0x401b8 (VERIFIED, the absolute
addresses 0x914e1c..0x934e98 the functions below read and write).

## 1. Commands

- `CL_Init` registers `demo` and `timedemo`, both with `CL_PlayDemo_f`
  (0x469120), and gives both `Cmd_SetAutoComplete("demos", "dm_1")`
  (VERIFIED, the pushes of 0x469120 at 0x46f4e7 and 0x46f546 and the
  stores of 0x6d1b84 and 0x6d1b7c after them). So `timedemo` is a
  command, not a dvar; vcod drops CoD1's `timedemo` cvar on a CoD4
  install (`Shell::register_cod4_options`).
- `record [name]` (`CL_Record_f`, 0x468960): more than two words prints
  `record <demoname>`, recording prints `Already recording.`, and a
  `cls.state` (0xc5f900) other than 9 prints `You must be in level to
  record.` (VERIFIED, the compares at 0x46899b, 0x4689ce and 0x4689e4
  and the strings 0x6d02dc, 0x6d02f0, 0x6d0304). A name is copied into
  63 characters and formatted `demos/%s.dm_%d` with a literal 1; no name
  tries `demo%04i` from 0 to 9999 through `FS_FileExists` (0x55b190) and
  takes the first free (VERIFIED, 0x4689fa..0x468a86). It prints
  `recording to %s.` and opens the file with `FS_FOpenFileWrite`
  (0x55b4d0); a failed open prints `ERROR: couldn't open.` (VERIFIED,
  0x468a95..0x468abe). That the file lands under the game directory
  (`main/demos/`, or the mod's under `fs_game`) is INFERRED from the
  qpath; vcod writes `main/demos/` or the mod's
  (`App::cod4_demo_dirs`). `demo` and `timedemo` read the same list.
  Trap: the app picks the demo player by `App::game`, the install's game;
  the search path's game reads CoD2 for a CoD4 install's iwds, and
  picking by it ran CoD2's player and said `EXE_ERR_NOT_FOUND`.
- `stoprecord` (`CL_StopRecord_f`, 0x4688c0): `Not recording demo.`
  when not recording, else a byte 0 and two -1 longs, the close, and
  `Stopped demo.` (VERIFIED, 0x4688cc..0x468946).
- `demo <name>` / `timedemo <name>` (0x469120): not exactly one argument
  prints `%s <demoname>` with the command's name; a running server
  prints `listen server cannot play demo.`; then `CL_Disconnect(0)`
  (VERIFIED, 0x469133..0x469193). A name ending in `.dm_1` (case folded,
  `I_strnicmp` at 0x46920d) opens `demos/<name>`, any other
  `demos/<name>.dm_1`; a file that does not open is `Com_Error(1,
  "EXE_ERR_NOT_FOUND\x15<path>")` (VERIFIED, 0x469212..0x469275). So
  `timedemo 1` looks for `demos/1.dm_1`, on retail as on vcod. The error
  popup shows the text localized (`localized_code_post_gfx_mp`'s string
  with the path for its `&&1`: `demos/1.dm_1 not found.`); vcod localizes
  it as it does a server drop's reason (`play::cod4::localize_drop`).
  That `Com_Error` localizes its text for the popup is INFERRED.
- On an open file `CL_PlayDemo_f` keeps the name in `demoName`, closes the console, sets `cls.state` 5,
  `demoplaying`, the timedemo flag from `Cmd_Argv(0)` being `timedemo`,
  the last archive row to 0, and reads records while the state is 5 to
  7, then clears `firstDemoFrameSkipped` (VERIFIED, 0x46929c..0x469362).
- `CL_Disconnect` (0x4696b0) runs `stoprecord` in state 5 or above
  while recording, then closes the demo file and clears `demoplaying`
  and `demorecording` (VERIFIED, 0x4696e7..0x46974f).

## 2. The records

A file is records from its first byte to its last, with no header:
`CL_Record_f`'s first `FS_Write` (0x55c200) after the open is the
gamestate record's type byte at 0x468d69, and nothing between the open
at 0x468ab0 and there writes to the file (VERIFIED, the calls in that
range; the writes before 0x468d39 go to the message buffer).

Each record opens with a type byte (VERIFIED, `CL_ReadDemoMessage`
0x4690c0 reads one byte and switches on 0 and 1 at 0x4690f0). Any other
byte reads nothing more (INFERRED, the switch falls out to the return);
vcod's reader skips it.

Type 0, a server message (`CL_WriteDemoMessage`, 0x468850):

```
byte  0
long  serverMessageSequence   (clc +0x2013c)
long  length                  msg->cursize - headerBytes
byte  message[length]          from headerBytes
```

(VERIFIED, the four `FS_Write` calls.) `CL_PacketEvent` (0x46c4bb..)
saves `msg->readcount` past the netchan into `edi` before it reads the
acknowledge and decodes the rest, and passes that as `headerBytes`
(VERIFIED, the `mov edi,[esi+0x1c]` at 0x46c4e6 and the call at
0x46c57d), so the record is the plain 4-byte `reliableAcknowledge` and
the unscrambled, still compressed op block (VERIFIED, the decode at
0x46c52c runs from readcount past the acknowledge). It writes only when
the parse left the message legible, recording is on and `demowaiting`
is clear (VERIFIED, the compares at 0x46c53c, 0x46c565, 0x46c56e), and
first writes the archive rows added since the last (section 4). vcod:
`Received::msg`, `Recording::write`.

Type 1, one archive row (0x468750):

```
byte  1
long  index (0..255)
float origin[3]     row +0x04
float velocity[3]   row +0x10
long  row +0x20
long  row +0x1c
long  serverTime    row +0x00
float viewangles[3] row +0x24
```

(VERIFIED, the eight `FS_Write` calls and their offsets.) The reader
(0x468ea0) takes the same order into `cl.clientArchive[index]` and an
index above 255 prints `Demo file corrupt.` and ends the demo (VERIFIED,
the `cmp eax,0xff` at 0x468ec6 and the string 0x6d0398); it sets the
archive's write index to `index + 1` (VERIFIED, 0x468f5e).

The end is a type 0 record of sequence -1 and length -1 (VERIFIED,
`stoprecord`'s writes). `CL_ReadDemoMessage`'s type 0 arm (0x468f90)
reads into a 0x20000-byte buffer: a short sequence or length read, or a
-1 length, ends the demo through `CL_DemoCompleted`; a length past
0x20000 is `Com_Error(1, "CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN")`;
a short message read prints `Demo file truncated.` and ends it
(VERIFIED, 0x468fbb..0x46904a). A message whose acknowledge is more than
128 behind `reliableSequence` is skipped as live (VERIFIED, the `-0x80`
compare at 0x46907c).

## 3. The opening gamestate

`CL_Record_f` builds one message into a 0x20000-byte buffer (VERIFIED,
0x468ae8..0x468d39):

```
long  clc.reliableSequence (+0x134)
byte  svc_gamestate (1)
long  clc.serverCommandSequence (+0x20140)
byte  svc_configstring (2)
short count of slots with a non-zero offset
per slot i < 2442 with a non-zero offset (cl.gameState at 0xc628ec):
  bit 0, 12-bit i, the string (MSG_WriteBigString 0x505580)
per baseline (0xd28400, 1024 of 0xf4 bytes) whose first word is non-zero:
  byte svc_baseline (3), MSG_WriteDeltaEntity from a zeroed state,
  time -90000, force 1
byte  svc_EOF (7)
long  clc.clientNum (+4)
long  clc.checksumFeed (+0x130)
byte  svc_EOF (7)
```

(VERIFIED, the stores and calls in that range: the count loop over
0xc628ec..0xc64f14 at 0x468b90, the inline bit write of 0 and the
`MSG_WriteBits(i, 12)` at 0x468bf8..0x468c2c, the `push 0xfffea070`
before 0x50a280 at 0x468cce.) Every slot goes with an explicit index,
never the "next slot" bit a server's gamestate uses. `MSG_WriteBigString`
writes a string of 0x2000 bytes or more as the empty string, after a
warning (0x6e2a64) on channel 0x10, and any other with each 0x92 byte
turned into `'` (0x27) and its terminator, byte-aligned at `cursize`
(VERIFIED, 0x50559d..0x50562f); vcod's record does the same
(`demo::big_string`). `MSG_Init` (0x505130) starts `cursize` (+0x14)
and the bit count (+0x20) at 0 (VERIFIED). So the plain op block opens on the bytes 1,
`serverCommandSequence`, 2 and the count, and ends on 7, `clientNum`,
`checksumFeed`, 7, the bit stream between (VERIFIED, the stores at
0x468b2a..0x468bd0 and 0x468d03..0x468d32); vcod's record holds those
bytes there (`net_decode::a_demo_gamestate_parses_back_to_the_live_one`). The first long
stays plain and the rest is compressed (0x505370) behind it, written as
one type 0 record under `serverMessageSequence`; then all 256 archive
rows, and the last row written becomes the current index (VERIFIED,
0x468d3f..0x468ddc). It sets `demowaiting` (VERIFIED, the store of 1 at
0x468b09). That the baseline's first word is its entity number, so
entity 0's baseline is left out, is INFERRED from the netfield order
(`cod4-netfields.md` 1). That the -90000 is the delta's `time`, so every
time field of a baseline goes out whole, is INFERRED from
`MSG_WriteDeltaEntity`'s signature; not yet confirmed on 1.7.

Which slots hold a string: `CL_ParseGamestate` (0x473d00..0x473fe6)
copies every `constantConfigStrings` row (0x7209b0) below each received
slot into the table, skips the row of the slot received, and copies the
rows left after the last (VERIFIED, the loops at 0x473e55..0x473ece and
0x473f68..0x473fdd), so the client's table holds every default as a
string and the record writes them. vcod writes every slot the parsed
table carries (sent, `d`-updated or emptied) and every default left
(`demo::gamestate_message`). That an emptied slot keeps a non-zero
offset in 1.7's `CL_ConfigstringModified` is INFERRED; vcod writes it
empty, which reads the same either way.

On the committed `handshake-1.7.txt` gamestate vcod's record parses back
to the same 2442 slots, the same 62 baselines, `clientNum`,
`checksumFeed` and map centre, and its last op ends on its block's last
byte (VERIFIED, `net_decode::a_demo_gamestate_parses_back_to_the_live_one`).

## 4. Waiting for a full frame, and the archive

While `demowaiting` is set, `CL_WritePacket` writes the move as
`clc_moveNoDelta` (VERIFIED, the compare at 0x4640e1 picks the
`WriteBits(1, 3)` arm at 0x464113). It is cleared at 0x4737d5, inside
the snapshot parse, on the arm a delta byte of 0 takes: that byte reads
as `deltaNum` -1 (0x47377c), and a `deltaNum` of 0 or less clears it
and marks the snapshot valid with no base (VERIFIED, 0x473773..0x4737e5).
The parse runs before `CL_PacketEvent`'s write, so the full frame is
the first message written (VERIFIED, the call at 0x46c534 ahead of the
compare at 0x46c56e). vcod: `Conn::no_delta`, cleared on a snapshot
with delta 0 before the write.

A server command that arrives only in the messages read while
`demowaiting` is set never reaches the file: the gamestate record took
the table before it, the gated write drops its message (0x46c565) and the
acknowledge already sent stops the server resending it. A `d` in that
window leaves the demo's table short of the live one (INFERRED from the
gate and the gamestate record's `serverCommandSequence`; vcod's record
behaves so, as its live test showed with the two `*_1mc_boost` sound
slots stock `war` sets as the match starts).

`cl.clientArchive` is 256 rows of 0x30 bytes at `cl` +0x666cc with its
write index at +0x696cc (VERIFIED, 0x472c10). `CL_SetUserCmdOrigin`
(0x45ad30) stores the cgame's origin, velocity, view angles and two ints
with `cl.serverTime` (0xc628d0) (VERIFIED, the stores to
0xc84fa4..0xc84fd4); `CL_WritePacket` adds a row from them unless the
row before holds the same time (VERIFIED, the compare at 0x464261 and the
call at 0x46428c). The two ints are `bobCycle` (+0x1c) and
`movementDir` (+0x20) in `clientArchiveData_t`, INFERRED; not yet
confirmed on 1.7. vcod adds a row at each packet sent from the
predicted playerstate, or unpredicted the newest snapshot's with the
cmd's view (`Cod4Session::archive_row`).

## 5. Playback

`CL_SetCGameTime` (0x45c440), VERIFIED unless labelled:

- state 8 with a demo playing: the first call sets
  `firstDemoFrameSkipped` and returns; later calls read one record. With
  `cl.newSnapshots` set, `CL_FirstSnapshot` (0x45c250) runs unless the
  snapshot's `snapFlags & 2`: state 9, `serverTimeDelta` the snapshot's
  time less `cls.realtime`, `oldServerTime`, `serverTime` and the
  timedemo base time the snapshot's.
- state 9: with a demo playing and `cl_freezeDemo` set the time holds;
  else `serverTime = realtime + serverTimeDelta`, never below
  `oldServerTime` (0x45c52b..0x45c578).
- with `timedemo` (0x45c350): on its first frame the csv opens and the
  start time is taken; the frame count goes up and `serverTime` is
  `base + frames * 50`.
- then records while `serverTime >= cl.snap.serverTime`, stopping when
  the state leaves 9 (0x45c5dc..0x45c63c).

`CL_DemoCompleted` (0x468df0): with `timedemo`, when the ms since the
start are above 0, `%i frames, %3.1f seconds: %3.1f fps`; the csv
closes; `CL_Disconnect(0)`; `CL_NextDemo` (0x469380), which runs
`nextdemo` (VERIFIED, 0x468df3..0x468e90).

vcod: `Cod4Session::play_demo` reads to the gamestate, `Loader::start_demo`
loads its map behind the connect screen, and `DemoPlay::frame` runs the
arms above each step; the session walks the snapshots for their events
and HUD as a watched capture does, predicts nothing and sends nothing.
That the cgame predicts nothing in a demo is INFERRED from CoD2's
(`cod2-demos.md` 5). The recorder's eye is his playerstate lerped
between the two snapshots drawn, and his view angles, which his
snapshots leave out (`cod4-netfields.md` 7), come off the archive rows
lerped at the drawn time; that 1.7's cgame reads them so is INFERRED.

vcod reads every row of the file when the demo opens
(`DemoPlay::views`). A row is stamped with the recorder's drawn time,
which lags the newest snapshot, so it lands in the file a message or two
after the snapshot it belongs beside. Rows read only with their messages
leave nothing past the drawn time to lerp to: the view holds six frames
in seven at 144 fps and jumps once a snapshot. With every row read, a
recorder running back and forth and turning 30 degrees a second plays
back moving and turning every frame. VERIFIED live
(`cod4_live_recorded_run_plays_back_smoothly`).

Any key under 0x80 and Esc in a demo leave for the main menu, as CoD2's.
INFERRED (1.7's `CL_KeyEvent` not read).

## 6. Live run

`play::cod4::tests::cod4_live_recorded_demo_plays_back_what_the_session_saw`:
a spectator on vcod-server's mp_crash `war` beside four
shooting bots records 13 s, then plays the file in process: 76 KB, 260
messages (the gamestate, then 259 snapshots, the first a full frame), 0
parse errors; every snapshot equals the live session's for state,
entities and clients from that full frame on, the table at the end
equals the live one, and the 101 events walked past the first frame are
the live session's. Under `timedemo` the frame count is the snapshot
count within two. VERIFIED live.

## 7. Not done

- No retail client runs here, so `iw3mp.exe` has neither recorded a demo
  for vcod to read nor played one of vcod's. Section 8 gates both ways
  against the binary's layout; the run itself (a CD-keyless 1.7 client
  under Wine, as `cod2-demos.md` 6 ran for CoD2) is still to do.
- `cl_avidemo` and `timedemo`'s csv.
- 1.7's front end demo list.

## 8. Interchange with retail

The one `.dm_1` in the 1.7 install was recorded by vcod: its gamestate's
`sv_hostname` is `vcod`, and its records skip the message after the
gamestate, as vcod's `demowaiting` does (VERIFIED, `cargo run -p
vcod-cod4 --example demo_dump -- <file>`, which prints a file record by
record). So no retail-recorded demo exists to read, and the gate builds
one from a retail capture instead:
`play::cod4::tests::cod4_retail_framed_demo_plays_and_vcod_writes_it_alike`.

- Retail to vcod: `play-1.7-spec.txt` (a spectator on retail
  `cod4_lnxded`) walked message by message and written by a writer of its
  own off sections 2 to 4 (`RetailDemo`: the type bytes, sequences,
  lengths and row fields in `CL_WriteDemoMessage`'s, 0x468750's and
  `CL_StopRecord_f`'s order), each record the acknowledge and the
  unscrambled block retail's server sent. Its opening record is section
  3's message as vcod builds it, the one part the capture cannot supply.
  The record starts as the gamestate lands, because the capture's client
  never asked for the full frame a later start waits for; retail's
  `record` waits for the first snapshot. vcod's `demo` reads every record,
  parses every message, and holds the capture's snapshots state for state
  from the first, its closing table (the capture's `d` commands applied
  as a live client applies them) and the events walked past the first
  frame. VERIFIED (the gate).
- vcod to retail: vcod's `Recording`, fed the same messages, rows and
  `demowaiting`, writes the same file byte for byte, so its framing is
  section 2's throughout (VERIFIED, the gate). The opening record's plain
  bytes are section 3's (VERIFIED, `net_decode`). That `iw3mp.exe` plays
  vcod's files is INFERRED from the two together; no retail client ran.

## 9. The `RECORDING` line

`SCR_DrawDemoRecording` (0x474c80):

- It draws only while `demorecording` (+0x40190) is set (VERIFIED, the
  compare at 0x474c86). Its one caller is a case of the switch at
  0x474f36, which INFERRED is the screen's `cls.state` switch.
- The text is `RECORDING %s: %ik` (0x6d2b14) with `demoName` and a byte
  count over 1024, truncated toward zero (VERIFIED, 0x474ccd..0x474ce9).
  That the count is the file's position, read off the open file
  (0x474c93..0x474cc5), is INFERRED.
- The colour is row 7 of the table at 0x6bdf78, (1, 1, 1, 1) (VERIFIED,
  0x45c6a0 called with `'7'`, and the table's bytes).
- The font is the handle at 0xc5ae94, registered from
  `fonts/consoleFont` (0x6d0e5c) at 0x46cc1c (VERIFIED). Its scale is
  16.0000005 (the double at 0x70b970) over the font's +4 word, times the
  placement's grid scale, +0 across and +4 down (VERIFIED,
  0x474cfc..0x474d59). That +4 is the font's pixel height, so the line
  is 16 grid units tall, is INFERRED.
- The baseline sits at grid (5, 479): `x = 5 * scale + (+0x30)`, `y = 479
  * scale + (+0x34)` off the placement at 0xe343d8 (VERIFIED, 5.0 at
  0x70b730, 479.0 at 0x70b968). Those are `ScrPlace_ApplyX`'s `LEFT` and
  `ApplyY`'s `TOP` (`cod4-front-end.md` 10.1), and 0xe343d8 is laid out
  by `ScrPlace_SetupFloatViewport` over the whole window (VERIFIED, the
  call at 0x46cbaf). So at a wide window the line starts 5 grid units
  from the window's left edge, outside the centred 4:3 column. INFERRED
  (the two together).
- The draw call to 0x5f6b00 gets style 0, rotation 0 and a character
  limit of 0x7fffffff (VERIFIED, 0x474d04..0x474d8d). That 0x5f6b00 is
  `R_AddCmdDrawText` is INFERRED. It turns style 3 into flag 4, 6 into 0xc, 0x80
  into 1 and leaves every other style 0 (VERIFIED, 0x5f6bab..0x5f6bd8).
  That flag 0 draws no shadow, so the line has none, is INFERRED.

vcod: `Console::recording_line_cod4` on `ScreenPlacement::cod4`, one
scale for both axes (the grid's down one), the console's `consoleFont`,
no shadow. The look on screen is not yet checked by eye.
