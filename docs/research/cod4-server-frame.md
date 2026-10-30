# Call of Duty 4 1.7: the server frame (`SV_Frame`)

The order `cod4_lnxded` 1.7 runs a server frame in: what runs ahead of the
game frames, what follows each one, when a client's cmds are thought, where
zombies are freed and sent, where the snapshot archive runs, and the order of
a frame's messages. It answers M1 and M2 of the CoD4 server design. The
engine follows it through `GameModule::HEAD` and `GameModule::POST_EACH_FRAME`
(`crates/server/src/engine/game.rs`).

## Sources

- `cod4_lnxded`, md5 `bf8448740b3f4cb74c2254f01af14cdd`, the binary of
  `cod4-handshake.md`. Stripped: every address below was found from a string
  xref or a call site, then read in `objdump -d` and Ghidra (`DecompAt.java`
  on a `-noanalysis` import). Function names are the Q3 lineage's. A claim
  labelled INFERRED that names no 1.7 address is not yet confirmed on 1.7.
- Three live runs, 2026-10-24, on loopback `mp_crash` `war` inside
  `unshare -rnm` (`cod4-handshake.md` 7), `sv_maxclients 8`, `developer 0`.
  `cod4_probe` joined as a spectator in the team menu and logged every
  datagram. An `AF_PACKET` socket on `lo` logged every datagram the server
  sent, in send order, as the sendorder runs did
  (`cod11-server-handshake.md`, "The order of a frame's messages").

Order and branch facts read off the code are INFERRED; the live runs check
the ones that show on the wire.

## 1. The frame

`SV_FrameInternal` (0x8177010) takes the milliseconds since the last call.
Its steps, in order:

1. A queued error, or `sv_running` 0, returns at once. INFERRED (the tests
   at 0x817701c and 0x817705e).
2. With `cl_paused` (dvar pointer 0x88a5440) set and at most one client at
   `CS_CONNECTED` or past, it sets `sv_paused` (0x88a5454) and returns;
   otherwise it clears `sv_paused`. INFERRED (0x8177069..0x81770bf). The two
   dvar names are VERIFIED (their registrations).
3. `frameMsec = 1000 / sv_fps` (dvar 0x13ed7bd0), and `timeResidual`
   (0x13e77f84) grows by the call's milliseconds. While it is under
   `frameMsec` the function returns before pings, bots, timeouts, messages
   or the heartbeat, so `MIN_FRAMES` is 0. INFERRED (0x81770c4..0x81770e6).
4. The time-wrap checks: `svs.time` (0x90b4204) past 0x70000000, or one of
   six counters near `INT_MAX`, restarts the map with
   `EXE_SERVERRESTARTTIMEWRAP` or its kin (strings 0x823230c..0x82325c4).
   INFERRED (0x81770ec..0x817716b).
5. `SV_CalcPings` (0x81756e0), then `SV_PreFrame` (0x8176ede). INFERRED (the
   calls at 0x8177318 and 0x817731d). 0x81756e0 walks the slots and stores
   999 as a client's ping. VERIFIED (the `0x3e7` stores at 0x817570a and
   0x81757da).
6. Per owed frame: `timeResidual -= frameMsec`, `svs.time += frameMsec`,
   0x817bed2, 0x80b61fc(`svs.time`), 0x815cf9a(0). INFERRED (0x8177341..
   0x8177366). 0x817bed2 bumps a counter and resets an aligned buffer
   pointer, `SV_ResetSkeletonCache`; 0x80b61fc is `G_RunFrame`.
7. After every frame, the last one included: `SV_CheckTimeouts`
   (0x8175830), `SV_SendClientMessages` (0x817afee) and
   `SV_MasterHeartbeat("COD-4")` (0x81780e4). INFERRED (the loop at
   0x8177324..0x8177371 runs the three between frames; 0x8177373..0x8177384
   after the last). The `COD-4` argument is VERIFIED (0x8232326 at both
   heartbeat calls). Section 6 shows it on the wire. This is `SV_PostFrame`,
   unlike CoD1 and CoD2, which run one round
   of timeouts and messages per `SV_Frame`.

### `SV_PreFrame`

`SV_PreFrame` (0x8176ede) opens a profile scope named `SERVER: bots`
(0x82322ff). VERIFIED. It then calls 0x817bed2 and runs, in this order
(INFERRED, the body read top to bottom):

- The bots: each slot whose state is not 0 and whose netchan address type
  (slot offset 0x20) is 0 goes through 0x81753f6, which builds a cmd and
  hands it to `SV_ClientThink` (0x816deca) itself. INFERRED (0x8176f2b..
  0x8176f4a and 0x81753f6's tail call). Address type 0 is `NA_BOT`.
- Configstring 0: when `dvar_modifiedFlags` (0x1402b260) holds 0x404,
  `SV_SetConfigstring(0, Dvar_InfoString(0, 0x404))` (0x8173e54, 0x81264d4,
  names), and the bits are cleared. INFERRED (0x8176fb0..
  0x8176fd4).
- Configstring 1: when it holds 8, 0x817411c, the systeminfo writer. INFERRED (0x8176f69).
- The mirror: when it holds 0x100, 0x81264fc(0x14, 0x80, 0x100), then the
  bit is cleared. INFERRED (0x8176f72..0x8176f97). The block is
  configstrings 20 on, 128 of them, names from 20 and values from 148
  (`cod4-handshake.md` 5).

So the head is pings, bots, configstrings 0 and 1, the mirror: each once per
`SV_Frame`, and only when a frame is owed. INFERRED (steps 3 and 5). CoD2's
head is configstrings, bots, pings (0x80969b0), CoD1's bots, pings.

## 2. The cmds: thought on arrival

`SV_UserMove` (0x8172560) reads the message's cmds into a stack array. For a
client at `CS_ACTIVE` (4) it then calls `SV_ClientThink` (0x816deca) on each
cmd newer than `lastUsercmd` (slot offset 0x20e68) and no newer than the
message's last, in order. INFERRED (control flow). A client at
`CS_PRIMED` (3) goes `CS_ACTIVE` there first and enters the world through
0x80a7700, `SV_ClientEnterWorld`. INFERRED (the `state == 3` arm).
That arm logs `Going from CS_CLIENTLOADING to CS_ACTIVE for %s` (0x8230600).
VERIFIED. On a pure server a client with no `cp` is dropped in the same
function instead (`cod4-handshake.md` 8).

`SV_ClientThink` (0x816deca) refuses a cmd 20000 ms or more past `svs.time`
with a console line, copies it to `lastUsercmd`, and calls into the game
only for a client at `CS_ACTIVE`. INFERRED (the `< 0x4e21` test and the
`state != 4` return). So `THINK` is `OnArrival`, as on CoD2.

## 3. Timeouts and zombies

`SV_CheckTimeouts` (0x8175830) walks every slot and clamps a
`lastPacketTime` that lies in the future to `svs.time`. A zombie (state 1)
whose last packet is older than `sv_zombietime` is freed: state and
`lastPacketTime` zeroed. A client past its limit (`sv_timeout` at
`CS_ACTIVE`, `sv_connectTimeout` while loading) counts a stale call, and the
sixth drops it with `EXE_TIMEDOUT`; a fresh packet resets the count.
INFERRED (control flow). The strings are VERIFIED: `Going from
CS_ZOMBIE to CS_FREE for client #%i` (0x82323f4, referenced at 0x81759b1),
`EXE_TIMEDOUT` (0x82321f6), and the three dvars' registrations (pointers
0x13ed7bd4, 0x13ed7bd8, 0x13ed7bdc). It runs after every frame, so the six
are frames, not `SV_Frame`s. INFERRED (section 1, step 7).

A zombie has no pass of its own: `SV_SendClientMessages` sends it on its
slot's turn with every other slot (section 4), as CoD2 does. INFERRED
(sections 3 and 4).

Live: a probe that went silent under `sv_timeout 4` was dropped (`0:z
EXE_TIMEDOUT` on the console), and its zombie was then sent seven 207-byte
messages over 2.25 s, five a frame apart and two a second apart. VERIFIED
live. Why the pacing changes after five is open.

## 4. `SV_SendClientMessages` and the archive

`SV_SendClientMessages` (0x817afee) makes two passes over the slots, both
in slot order. INFERRED (control flow):

1. Each slot whose state is not 0 and whose `nextSnapshotTime` (slot offset
   0x21464) has come: with netchan fragments left (offset 0x48 not 0) it
   sends the next one at once (0x8178672) and sets the next time from the
   rate, `(min(len, 1500) * 1000 + 48000) / rate`. Without fragments it
   marks the slot, and for a client at `CS_ACTIVE` or a zombie builds its
   snapshot (0x817a258).
2. Each marked slot: a fresh message, then for `CS_ACTIVE` or a zombie the
   server commands and the snapshot (0x8179b0e, 0x8179c62), then 0x817ad58,
   which writes the download block for any state but the zombie's, ends and
   sends the message; then 0x817bab2.

So a frame's fragments (of a gamestate or a large snapshot) all go out
before any slot's new message, and inside each pass slot 0 comes first
whatever its state. INFERRED (the two passes). Live:

- A probe held at `CS_CONNECTED` at slot 0 (a relay passed only its
  out-of-band packets; `status` read `CNCT`) and a probe at slot 1. In 299
  of the 310 frames that held a message for both, slot 0's 9-byte message
  went out first. The other 11 were slot 1's connect (the packet handler's
  own send) and the ten frames that carried a fragment of slot 1's
  gamestate (1310 bytes each, then 1250 and 764): the fragment went first.
  VERIFIED live.
- A zombie at slot 0 and a probe at slot 1: the zombie's message went first
  in all 7 of its frames. VERIFIED live.
- The gamestate's fragments went out 347 to 353 ms apart on loopback.
  VERIFIED live (M7's question).

The archive is the tail of `SV_SendClientMessages`: when `sv.state`
(0x13e77f80) is 2 it copies the frame's snapshot into the archive ring.
INFERRED (control flow). Its messages are VERIFIED there:
`SV_ArchiveSnapshot: ignoring snapshot because it overflowed.` (0x8232d00,
referenced at 0x817b6c4) and `svs.nextArchivedSnapshotFrames wrapped`
(0x8232cd8, at 0x817b5d7). So each frame archives after its own messages:
`ARCHIVE_AFTER_MESSAGES` holds, and with section 1 step 7 there is no
archive between frames. INFERRED.

## 5. The heartbeat

`SV_MasterHeartbeat` (0x81780e4) sends only when the dvar at 0x13fa5c80
reads 2 and `svs.time` has reached the next beat, which it then sets 180000
ms on. INFERRED (control flow). That dvar is `dedicated`, an enum of
`listen server`, `dedicated LAN server`, `dedicated internet server`
(registered at 0x8121cc6), and the strings are `Sending heartbeat to %s`
(0x823279f) and `heartbeat %s` (0x82327b8). VERIFIED. Yet each `+set
dedicated 1` run here sent one datagram to port 20810, as `cod4-handshake.md`
1 saw. VERIFIED live. Why is open.

## 6. A hitch: one message per owed frame

The server was stopped with `SIGSTOP` for 150 ms and resumed, ten times,
1.5 s apart, with one spectator probe in the world (`sv_fps` 20, loopback):

- Each resume was followed by three messages (four after the two 202 ms
  stops) sent within 0.05 ms of each other (the `lo` log: 8145.52, 8145.56,
  8145.57 ms), their snapshots' `serverTime`s 50 ms apart. VERIFIED live.
- Over the whole run every snapshot's `serverTime` stepped 50 past the last
  (597 of 598; the other, 250, is the first snapshot after the gamestate).
  VERIFIED live.

A server that sent once per `SV_Frame` would have sent one message per
resume, its `serverTime` 150 or 200 past the last. This is section 1 step 7
seen from the wire.

## 7. What vcod's engine does with it

`Cod4Game` sets (S-C4-1 adds the module):

| Const | Value | Rests on |
|---|---|---|
| `HEAD` | `HeadOrder::PingsBotsCs` | section 1 |
| `POST_EACH_FRAME` | true | section 1 step 7, section 6 (live) |
| `THINK` | `ThinkTiming::OnArrival` | section 2 |
| `MIN_FRAMES` | 0 | section 1 step 3 |
| `ARCHIVE_AFTER_MESSAGES` | true | section 4 |

Zombies are freed in `SV_CheckTimeouts` and sent on their slot's turn in the
one `SV_SendClientMessages` pump (section 3), as on CoD2.

`engine::send_messages` follows section 4's two passes under
`GameModule::FRAGMENTS_FIRST`, which only `Cod4Game` sets: every due slot's
fragment in slot order, then the marked slots' new messages. CoD1 and CoD2
keep one pass.

## 8. `sv_fps`

### The dvar

`SV_Init` registers `sv_fps` (name 0x82317c1, description `Server frames
per second` 0x82317a8) through the int registrar 0x81a2596 with default
20, minimum 10, maximum 1000 and flags 0, and keeps its pointer at
0x13ed7bd0. VERIFIED (the immediates at 0x81735a0..0x81735d4). Flags 0:
no latch. A query prints `"sv_fps" is: "30^7" default: "20^7"` and
`Domain is any integer from 10 to 1000`; `sv_fps 5` and `sv_fps 2000`
print `'5' is not a valid value for dvar 'sv_fps'` with the domain and
leave the value as it was. VERIFIED live (rcon, 2026-10-27).

`SV_FrameInternal` reads it on every call (0x81770c4, section 1 step 3).
INFERRED. A change takes the next frame: an rcon `sv_fps 40` on a server
started at 30 stepped the next snapshot 25 past the last, and `sv_fps 20`
after it 50. VERIFIED live.

### What it scales

The frame is `1000 / sv_fps` in integer milliseconds (0x81770c4's
`idiv`). INFERRED. Live, loopback,
`mp_crash` `war`, one `cod4_probe` on axis with `snaps 20`
(`play-1.7-svfps40.txt` is the 40 run's first 8.5 s):

- At 40 every snapshot's `serverTime` stepped 25 past the last (995 of
  996; the other, 175, is the first after the gamestate), at 30 33 (the
  first 198). VERIFIED live. Section 6 has 20's 50 and 250.
- A loopback client is a LAN one, paced by nothing (section 4), so it
  got a message every frame whatever its `snaps`: 40 a second at 40.
  VERIFIED live. Off the LAN `snapshotMsec` is `1000 / snaps` with `snaps`
  clamped to 30 at most, 1000 for `snaps` 0 or less, and no `sv_fps` term
  (`SV_UserinfoChanged`, 0x81704b5..0x81704d4). VERIFIED. That a `snaps
  20` client off the LAN then gets every other frame at 40 is INFERRED;
  no off-LAN run was made.
- `G_RunFrame` (0x80b61fc) bumps `level.framenum` (0x836f8a8), moves
  `level.time` (0x836f8ac) to `level.previousTime` (0x836f8b0), takes the
  frame's `svs.time` as `level.time` and stores the difference as
  `level.frametime` (0x836f8b4). INFERRED (the stores at 0x80b620b..
  0x80b622b).
- A script `wait` counts frames: `showClientScoreBar(5.0)`
  (`_hud.gsc`), which sets `ui_score_bar` 1 at the spawn and 0 after its
  `wait`, sent the 0 2500 ms of level time after the 1 at 40 (35975 to
  38475) and 3300 ms at 30 (35907 to 39207): 100 frames each. VERIFIED
  live. `cod4-gsc.md` 4 has the rounding to frames.

The other readers of the dvar, each read off its arithmetic:

- 0x80a2a54, the antilag: a shot is rewound only when it is older than
  `1000 / sv_fps` and no older than 400 ms. INFERRED.
- 0x81798ca, the walk for a client's position at a time: `1000 / sv_fps`
  and `400` over it. INFERRED.
- 0x81795fa and 0x817a30f turn an age in ms into archived frames,
  `ms * sv_fps / 1000` (the 0x10624dd3 multiply), against a ring of 1200
  frames (0x4b0). INFERRED.
- 0x817968c turns frames back into ms, `frames * 1000 / sv_fps`.
  INFERRED.
- 0x8178993 and 0x817b2b7 copy the value into a block at 0x13f181dc beside
  `svs.time`; what reads it is open. INFERRED.

`level.frametime` has six readers besides its store. VERIFIED (the loads
at 0x80a4838, 0x80b62c6, 0x80caa15, 0x80d113f, 0x80d14bf, 0x80d1a97).
0x80a4838 hands it to the end frame's controller setup, and 0x80b62c6
hands it, as seconds (`* 0.001f`, 0x8209640), to a per-entity call in
`G_RunFrame` (0x817c360). INFERRED. 0x80d113f, 0x80d14bf and 0x80d1a97
are vehicle code: its step in seconds and a vehicle gun's `fireTime -=
frametime`. INFERRED (`G_VehEntHandler_Think`).

What stays 50 whatever the dvar:

- `turret_track`'s fire timers: `fireTime -= 50` at 0x80b7bec, 0x80b7c22
  and 0x80b8f37. VERIFIED (the `sub $0x32`s).
- `turret_think_client` runs from each gunner's `ClientEndFrame`, every
  frame, so a mounted gun fires faster in level time above `sv_fps` 20.
  INFERRED.
- `nextthink = level.time + 50` at 0x809c6ed, 0x80a03c0, 0x80b71ad,
  0x80b753f, 0x80b8ebb, 0x80ba20f, 0x80ba9d8, 0x80bab06, 0x80cdb76,
  0x80d12ff and 0x80d1f00 (each `add $0x32` stored at entity offset
  0x19c). VERIFIED. Those thinks run on the first frame 50 ms on.
  INFERRED.
- `G_ExplodeMissile`'s smoke grenade still in the air: `nextthink = 50`,
  not `level.time + 50` (0x80940f1, after the 60000 ms age test at
  0x80940ea). VERIFIED. It is due again on the next frame. INFERRED.
- A missile's bounce reads `level.previousTime` (0x80930e3, 0x8094702,
  0x80949fe, 0x8094c8c, 0x8094d7a), not `level.time - 50`. VERIFIED (the
  loads).
- The rocket's `0.05` steps (the missile trajectory step 0x8095928 and its
  steering) read neither clock. INFERRED (no `level.frametime` read in
  0x8095928..0x8098832).

### What vcod does with it

`Cod4Server::frame_ms` reads the dvar each frame (out of the domain it
reads 20, and the console refuses such a value with retail's text), and
`GameModule::frame_step_ms` and `wall_frame_ms` step `svs.time` and the
frame loop's slot by it. The level keeps `sv_fps`, `level.previousTime`
and `level.frametime` (`Cod4Level::set_sv_fps`, `previous_time`,
`frame_time`), and from them:

| What | Reads |
|---|---|
| The VM's `wait` | frames of `1000 / sv_fps` (`Vm::set_frame_ms`), set at the load from the dvar |
| The antilag's age floor, its walk, its archive | `1000 / sv_fps`, frames back by `age * sv_fps / 1000`, one second of frames |
| The killcam's look back | ten seconds of frames |
| The bodies' anim swing and controllers | `level.frametime` |
| A missile's bounce and land time | `level.previousTime` |
| A smoke grenade's retry | `nextthink` 50 |
| A mounted gun's fire timer | 50 a frame |
| A helicopter's think | every 50 ms of level time, 0.05 s a think |

The helicopter's think (0x809fbd6) sets `nextthink = level.time + 50`
(0x80a03bb) and reads no `level.frametime` (none of the seven loads lies
in it or the missile and vehicle-AI code before 0x80a4838), so its move
is a fixed step a think. INFERRED. That `Helicopter_Think` and
`VEH_UpdateMoveToGoal` step 0.05 under that think is INFERRED too, not
yet confirmed on 1.7. vcod's
helicopter thinks on the first frame 50 ms past its last
(`Heli::next_think`), 0.05 s a think: every frame at 20, every other one
at 40. The `level.frametime` readers at 0x80d113f..0x80d1a97 belong to
the ground vehicles (`G_VehEntHandler_Think`), which no stock
MP map spawns. INFERRED.

The bots' brain, its timers and the nav graph's walks were built on a 50
ms tick, which retail has no counterpart of. An `SV_Frame` runs a think
for each 50 ms of level time its frame covers, each on its own clock
(`Cod4Bots::think_times`): one a frame at 20, one every other frame at
40, two a frame at 10, so a bot's cmds stay the 50 ms its walks were
proved with. `bot_match GAME=cod4` (mp_crash `war`, 6 bots, seeds 1 and
2, 300 s of level time) printed the same at 20 before and after; at 10,
where before each frame gave one 100 ms cmd:

| sv_fps 10 | kills | hits | rounds | throws | mantles | walked u/s |
|---|---|---|---|---|---|---|
| one think a frame | 33, 35 | 129, 133 | 490, 408 | 13, 15 | 27, 39 | 82, 83 |
| a think each 50 ms | 43, 34 | 154, 127 | 645, 488 | 19, 21 | 25, 55 | 87, 97 |

At 20 the same seeds give 39, 56 kills and walk 73, 101 u/s.

The first snapshot after the gamestate goes out in two fragments, and
the next message waits out the rate on the last one, `rate_msec(len,
sv_maxRate 5000)` (section 4): retail's last fragment was 700 bytes,
149 ms, so the next snapshot came 175 after the first at 40 (fragments
at 33400 and 33425, then 33575) and 198 at 30. VERIFIED live (the
fragment sizes and times). vcod's was 755 bytes, 158 ms, so 200 at 40.
Field by field, against the capture's first snapshot (VERIFIED live):

- The team menu's spectator reads `moveSpeedScaleMultiplier` 1;
  `ClientEndFrame` copies the session's into the playerstate for every
  session, and vcod wrote it only for a
  player. vcod now writes it at the end frame's head.
- The connect's `v ui_3dwaypointtext "1"` and `v ui_deathicontext "1"`
  (`_menus.gsc`'s `onPlayerConnect`, on `connecting` after the callback's
  `waittillframeend`) went out ahead of the gamestate, whose command
  sequence reads 2: the probe's seven stats packets were answered 4 to 9
  ms apart, so two frames (27 and 48 ms, each a 9-byte message) ran
  between the connect and the gamestate. vcod answered all seven within
  2 ms, the gamestate went before any frame, and the two commands rode
  the first snapshot instead: about 60 bytes, the rest of the 55. Where a
  frame runs during the stats, as in `play::cod4`'s
  `cod4_live_first_snapshot_as_retail`, vcod's gamestate reads 2 and its
  first snapshot is retail's word for word but `commandTime`. INFERRED
  (the order is the packet timing, not a rule).

Open: the off-LAN snapshot rate above 20.

`play::cod4`'s `cod4_live_sv_fps_40_frames_and_waits_as_retail` replays
`play-1.7-svfps40.txt` through the client and joins vcod-server at
`sv_fps` 40 the same way: both read steps of 25 and the 2500 ms wait.

## 9. Slow ticks

The C4-104 soak (`cod4-map-sweep.md`, "The soak") saw three vcod-server
ticks of 101 to 125 ms over 105 map and gametype pairs. This section is
vcod's own tick, measured; retail has no counterpart in it. Every number
below is VERIFIED (the logs of the runs named), on a many-core machine shared
with other jobs, each run pinned to cores 0-7 (`taskset`) under a
memory cap.

### The instruments

- `vcod_server::tick_profile`: a `phase` guard times a part of the tick
  per thread, nested parts named `outer/inner`: `console`, `packets`,
  `head` (`head/pings`, `head/bots` with `bots/manage`, `bots/nav tick`,
  `bots/commands and inputs`, `bots/sense`, `bots/deal`, `bots/bodies and
  paths`, `bots/cover and vantage`, `bots/goal`, `bots/route` (the plan
  budget), `bots/probes`, `bots/think`, `bots/land`, `bots/moves`,
  `head/configstrings`), on CoD1 `console/map_restart` (`/load`,
  `/settle`, `/reenter`; the level load's `load/compile`, `load/spawn`
  with its `spawn/*` parts, `load/bootstrap`, cod11-map-cycle.md 4.9),
  `frame` (`G_RunFrame`:
  `frame/touch triggers`, `frame/scripts`, `frame/script ops`,
  `frame/entities`, `frame/trace bodies`, `frame/mantle and hud`,
  `frame/end frames`, `frame/vote and archive`, `frame/flush` with the
  `game log` write), `frame out`, `timeouts`, `messages` (`messages/snapshot`,
  `messages/snapshot world`), `archive`, `heartbeat`. Off it costs a
  relaxed load a phase.
- `tick_profile::Split` splits a tick's wall time off
  `/proc/thread-self` into run (on a CPU), queued (runnable, no CPU),
  asleep (the rest), minor and major faults and voluntary switches.
- `tick_profile::Watch` samples, every millisecond of a tick, the ticking
  thread's kernel wait channel and the syscall every other thread of the
  process is in.
- `vcod-server` with `VCOD_TICK_PROFILE_MS=<ms>` logs each tick past it;
  `bot_match GAME=cod4 SPIKE_MS=<ms>` prints them; the soak logs every
  tick past `VCOD_SOAK_TICK_LOG_MS` (100), with `VCOD_SOAK_TICK_WATCH=1`
  the waits, and `VCOD_SOAK_CLIENT_LOG_MS` the client's slow frames
  beside them, all stamped with the seconds since the process started.

### The server alone

| run | ticks | past 20 ms | worst outside a load |
|---|---|---|---|
| `bot_match` mp_citystreets `war`, 8 bots, `SPIKE_MS=0` | 1200 | 0 | 11.0 ms (p50 2.5, p99 7.3; 1.5 minor faults a tick) |
| `bot_match` mp_citystreets, mp_shipment `war`, 12 bots, `JOBS=2` | 12000 | (none past 40) | under 40 |
| `bot_match` mp_crash, mp_strike, mp_vacant, mp_overgrown, mp_crossfire, mp_pipeline `koth`, 12 bots, `JOBS=2` | 18000 | 23 | 26.9 ms |
| `vcod-server --game cod4 --bots 8 --bots-shoot`, mp_citystreets then mp_strike `war` (2 min limit), `VCOD_TICK_PROFILE_MS=20`, 9 min | about 10000 | 2 | none |

- The 23 ticks of 20 to 27 ms were all on a CPU (asleep under 1 ms, no
  fault), in the bots' pass: `bots/probes` (the drop and lob probes and
  the game's pre-think) up to 22.2 ms over 12 bots, `bots/route` up to
  16.1 ms, `bots/moves` up to 12.4 ms. Under the 50 ms frame.
- The two the live server logged were its map loads (the `console`
  phase): 634 ms and 789 ms, the second with 24944 major faults reading
  the next map's zones. The soak and `bot_match` leave a load out, as a
  client sits behind its loading screen through retail's.
- The live server built each map's nav graph in the background while its
  bots played: no tick of either build passed 20 ms. `bot_match` with two
  jobs ran one match's ticks beside the other's graph build in one
  process: nothing past 27 ms.
- No logged tick named the `game log`, `timeouts` or `heartbeat` phase
  (a phase under 1 ms is left off the line), and none grew with the
  level's age.

### The soak's spikes

The soak's server shares its process, and eight cores, with a client and
lavapipe. Four soak runs (`tools/cod4_soak.sh`):

| run | pairs | ticks past 25-40 ms | worst | where |
|---|---|---|---|---|
| one map a process, mp_citystreets, mp_shipment, `war` `sd`, log past 40 | 4 | 0 | - | - |
| two maps a process (`PAIR=2`), six maps, `war` `sd` `koth`, log past 15 | 18 | 12 past 30 | 84.7 ms | 5 s or 35 s into a level |
| `PAIR=2`, four maps, `war` `sd`, run stamps | 8 | 9 past 25 | 76.6 ms | all at 36.5 to 36.6 s into the process |
| `PAIR=2`, mp_bog, mp_strike, 1 min levels, the watch on | 4 | 7 past 25 | 71.4 ms | all at 6.7 to 8.2 s into the process |

- Each one came within two seconds of a client joining a level, on both
  servers of the process at once: the second job's client joining at 36.6
  s stalled the first job's server (35.7 s into its level) and its own
  (5.6 s into its). Past the joins nothing passed 25 ms.
- Their time was asleep, not work: 18 to 66 ms asleep with 100 to 200
  minor faults, against 1.5 faults a tick alone; the phase it landed in
  varied (`frame/scripts`, `frame/end frames`, `frame/vote and archive`,
  `head/configstrings`, `bots/cover and vantage`).
- The watch: the server thread waited in `lock_mm_and_find_vma` (a page
  fault waiting for the address space's lock), `do_mprotect_pkey` (its
  own allocator growing its heap) and `futex_wait`, while the client's
  threads (lavapipe's inherit the client thread's name) sat in `mprotect`
  (syscall 10) for most samples, and in `brk` (12). A join builds the
  level's pipelines and prewarms its models and effects
  (`Cod4Client::prewarm`). That lavapipe's shader compiles and the
  client's allocator are the `mprotect` callers, and that each holds the
  process's `mmap_lock` for writing while every other thread's page
  fault and heap growth waits on it, is INFERRED: the watch sees the
  syscall, not its caller.
- The scope saw no `memory.high` event and no CPU throttling
  (`memory.events`, `cpu.stat`) in any of them.

So the soak's stalls are the harness: a dedicated vcod-server shares its
address space with no client and no software renderer. The three
C4-104 counted (mp_citystreets `war` 73 s and `sd` 6 s, mp_shipment `war`
10 s) were not reproduced; two came 6 and 10 s into a level, where its
client joins, and fit this. The one 73 s in is not explained (INFERRED
that it is the same: the client's first draw of something lavapipe had
not compiled).

The soak's table now counts the stalls past 100 ms on a CPU apart
(`server stalls, on CPU`): a stall there is the server's own work.

### Long runs, past 40 ms (2026-10-28)

Server alone, the same core pinning, every tick past 40 ms logged
(`SPIKE_MS=40`, `VCOD_TICK_PROFILE_MS=40`); `bot_match`'s first tick
of a level, which waits on the graph it asks for, is left out. VERIFIED
(the runs' logs).

| run | level time | ticks past 40 ms | what they were |
|---|---|---|---|
| `bot_match GAME=cod4` `war`, 12 bots, mp_citystreets, mp_crash, mp_strike, mp_backlot, mp_bog, mp_shipment, 12000 ticks each | 60 min | 6 | the rotation's map load (`console` 349 to 374 ms), one a match |
| the same six maps, `sd` | 60 min | 44 | 5 map loads; 39 round restarts, 83 to 101 ms on a CPU |
| `vcod-server --game cod4 --bots 8 --bots-shoot`, `war` 3 min limit, 11 min each on mp_citystreets, mp_crash, mp_strike | 33 min | 6 | the rotation's map load (`console` 599 to 869 ms), two a map |

- Nothing else passed 40 ms: no tick of a level in play, none growing
  with the level's age, no `bots/*` part past 10 ms.
- The round restart was `fast_restart`'s `console` (67 to 83 ms) plus the
  frames it owed. Its phases put 57 to 65 ms in reopening `main/`'s iwds
  for the scripts' file reads (24243 files indexed), 6 to 10 ms in the
  scripts' `G_InitGame`. The level's zones now keep the opened set
  (`Cod4Zones::iwds`), and the same 4000 ticks of mp_shipment and
  mp_crash `sd` restart in 32 to 39 ms, with the same kills, hits and
  rounds.
- So 73 s into a `war` level the server alone does nothing past 40 ms:
  the C4-104 soak's 73 s tick stays unreproduced, and this section's
  harness reading (the client's draws in the same process) is the one
  left standing. INFERRED.

## 10. Full servers, every game (2026-10-29)

Section 9's runs had 8 to 12 bots. This one fills the server: 24 bots on
each game's stock map, and 24 and 32 on two CoD4 maps, `war`. vcod's own
tick, measured; retail has no counterpart in it. The runs: the shared
many-core machine under a memory cap, one match a process
(`JOBS=1`), seed 1, 2400 ticks (2 minutes), tick 0 (the nav graph's
wait) left out.

### The instrument

`bot_match TICKSTATS=1` (any game) prints each match's tick time (p50,
p90, p99, max and the slowest tick's phases) and every
`tick_profile` phase's mean and p99 a tick. `OBSERVERS=N` (CoD1 and
CoD2) joins N spectator clients at their own non-LAN addresses, for a
server with humans to build snapshots for. The probe pass splits into
`probes/pre-think`, `probes/drops and walls` and `probes/lob`.

Live, the same phases on a running server:

    VCOD_TICK_PROFILE_MS=15 cargo run --release -p vcod-server -- mp_crossfire \
      --game cod4 --game-dir <CoD4 install> --max-clients 32 --bots 24 --bots-shoot

logs every tick past 15 ms with its phases while a client plays on it.

### Before and after

ms a tick, p50 / p99 / max. Before is the base, after the
change; each pair's `DIGEST=1` line is the same, so
every bot did the same thing on every tick. VERIFIED (the runs' logs).

| game, map, `GAMETYPE` | bots | before | after |
|---|---|---|---|
| CoD1 1.1 mp_carentan `tdm` | 24 | 5.1 / 15.4 / 29.2 | 3.9 / 7.0 / 9.0 |
| UO 1.51 mp_cassino `tdm` | 24 | 8.3 / 16.8 / 29.6 | 6.1 / 11.8 / 20.8 |
| CoD2 1.3 mp_toujane `tdm` | 24 | 10.4 / 16.9 / 30.2 | 3.4 / 5.6 / 11.1 |
| CoD4 1.7 mp_crossfire `war` | 24 | 17.8 / 32.0 / 50.8 | 11.9 / 19.5 / 28.5 |
| CoD4 1.7 mp_crossfire `war` | 32 | 28.6 / 49.1 / 111.3 | 18.0 / 30.9 / 60.4 |
| CoD4 1.7 mp_crash `war` | 24 | 8.8 / 22.1 / 35.2 | 4.6 / 9.0 / 20.0 |
| CoD4 1.7 mp_crash `war` | 32 | 12.5 / 20.7 / 27.3 | 5.5 / 10.2 / 27.1 |

`sv_maxclients` takes 32 on vcod-server (`--max-clients`); the runs'
`max_clients` is the bots plus two. The 20 to 27 ms maxima left on
mp_crash are a level's first scripted frames (`frame/scripts`, tick 60).

### What cost what

- CoD4: `touch_items` (`G_TouchItems` per item and player, each frame)
  rebuilt every playing client's pmove state off the wire playerstate,
  and `PlayerState::from_wire` found each of its 120-odd fields by name in
  the 141-entry `PLAYER_STATE` table, formatting `origin[0]` and its kin
  as strings: about 5 ms of `frame` at 24 to 32 bots. The field words are
  now constants looked up at compile time (`crates/cod4/src/pmove/wire.rs`,
  `ix!`); `frame` fell from 5 to 8 ms a tick to about 3. VERIFIED.
- CoD2: `Player_UpdateLookAtEntity` (Mac 0x1cd1d4, cod2-combat.md 9.2)
  traces 15000 units along every playing client's view each end frame.
  The common collision world's BVH walk cut nodes and prims by the
  sweep's axis-aligned box, which for a long diagonal ray takes in most
  of the map: 70% of CoD2's tick, most of it in static model triangles.
  A sweep longer than 256 units now also drops a model triangle, a
  terrain partition, or a node holding only those, that its segment
  passes clear of (a slab test on the box grown by the swept shape, a
  unit, a hundredth of the box for a model triangle's barycentric slack
  and the capsule's sphere offset; `TraceInput::misses` and
  `TraceInput::tight` in `crates/common/src/collision.rs`); the fraction
  cut stays the box test's. Brushes and patches are never culled: a
  model-0 patch's leaf-walk clip met mp_carentan's patch 9 56 units off
  its box, and a brush notes a near tie (`note_contact`) for a vertical
  point trace 160 units off its box (UO mp_cassino), which sends the
  trace through retail's leaf order and can name the other of two
  triangles a hit ties on. With them culled, `bm11_tdm` parted from the
  base at tick 2300 of mp_carentan seed 1. A run that traced every
  sweep both ways on 1.1, UO and CoD2 maps found no other difference.
  CoD2's `frame` fell from 7.6 to 0.9 ms a tick, and CoD1's bot sight
  lines (`bots/sense`, p99 10.5 to 3.4 ms) and UO's `frame` (3.1 to 1.6)
  with it. VERIFIED.
- The bots' wall feelers (`nav::wall_ahead`, eight box traces a bot) ran
  on every tick, but only a wander heading reads them. They now run on
  the ticks `Bot::reads_walls` names (those `Bot::reads_drops` names, a
  bipod down, a body at its cover spot), as the drop probe already did:
  CoD4's `bots/sense` at 32 bots fell from 4.2 to 1.2 ms. An instrumented
  run (CoD1, UO, CoD2 and CoD4 matches, 3000 ticks each) saw no heading
  picked on a tick the gate left out. VERIFIED.
- Every xanim sample (`skeleton::sample_records`, the bodies posed for a
  shot or a hit trace) copied its track's whole key list to read two
  keys. VERIFIED (the dev build's samples).
- Snapshots are not the cost: 8 spectators beside 24 bots added 0.55 ms
  (CoD1 mp_carentan) and 0.45 ms (CoD2 mp_toujane) to `messages`, about
  0.07 ms a client. VERIFIED.

### What is left

CoD4 at 32 bots on mp_crossfire spends its tick in the bots' own work:
the drop probe's box traces (`probes/drops and walls`, 4.6 ms mean, 45 ms
on the tick most bots wander at once), pmove (`bots/moves`, 4 ms), the
plan budget and follower (`bots/route`, 3.7 ms). Spreading the probes
over ticks or sharing them between bots changes what the bots do, which
the digests forbid; the follower and the probe's shape belong to the CoD4
bot movement work. The tick stays under the 50 ms frame at p99. VERIFIED.

## 11. 32 bots, the same decisions (2026-10-29)

Section 10 left CoD4's 32-bot tick in the bots' drop probes, pmove and
route planning. This pass cuts what those cost without changing a bot's
cmd: every change below is a pure-cost path. vcod's own tick; retail has
no counterpart. Runs as section 10's (seed 1, 2400 ticks, `JOBS=1`, the
shared machine under a memory cap).

### Before and after

ms a tick, p50 / p99 / max, `war`. Before is the base,
after the change. Each pair's `DIGEST=1` line is the
same, and so are the `DIGEST=500` lines of 50 CoD4 matches (12 bots,
3000 ticks, seeds 1 and 2 on mp_crash, mp_crossfire, mp_backlot, mp_bog
and mp_showdown under `war`, `sd`, `koth`, `sab` and `dom`) and of 1.1
(mp_carentan, mp_harbor), UO (mp_cassino) and CoD2 (mp_toujane,
mp_carentan) `tdm` matches. VERIFIED (the runs' logs).

| map | bots | before | after |
|---|---|---|---|
| mp_crossfire | 24 | 12.5 / 21.1 / 32.4 | 8.0 / 16.8 / 25.1 |
| mp_crossfire | 32 | 18.6 / 31.8 / 63.2 | 11.1 / 20.8 / 29.5 |
| mp_crash | 24 | 4.2 / 7.4 / 20.2 | 3.4 / 7.8 / 17.2 |
| mp_crash | 32 | 5.6 / 10.1 / 26.3 | 4.7 / 7.9 / 22.1 |

The before column is lower than section 10's after: other branches
landed between the two.

### What cost what

A sampling profile (gdb's backtraces on a signal, 300 to 400 samples of
a 32-bot mp_crossfire match) put two thirds of the tick in
`clipmap::Cx::trace_tree` and the brush tests under it, called by the
drop probe, the follower's walks (`Follower::pull`, `edge_holds`) and
both pmoves.

- The BSP walk (`CM_TraceThroughTree`, `crates/common/src/clipmap/trace.rs`)
  split a sweep down to every leaf its box crosses, and most leaves hold
  nothing that box meets. Each node now carries the box round its
  leaves' brush boxes and AABB tree roots (`ClipMap::node_reach`, built on
  the first sweep); a node the sweep's box (grown by the trace's box and a
  unit) misses is passed over. Every test under such a node is a
  brush-box cull, a brush or a `CM_CullBox` that fails on that axis, so
  the result is the same. The brush test (`CM_TraceThroughBrush`) now
  returns first where both ends of the sweep stand more than a unit
  outside one axial side, which its axial loop does too. Both are in the
  shared kernel: CoD2's and CoD4's traces.
- The drop and wall probes (`bots::driver::drop_probe`, `wall_probe`)
  read only the world and the feet. CoD4's bot world holds still for the
  level (the clip map and the brush entities standing on its first
  frame), so `BotWorld::STILL_WORLD` keeps each verdict by the feet's bit
  pattern (`Probes`, 16384 spots, cleared when full and on a new map).
  `probes/drops and walls` at 32 bots fell from 4.9 to 2.7 ms mean, and
  to 1.6 with the kernel cuts above. CoD1
  and CoD2 move brushes under their bots, and keep probing.
- `hud_elems` (`crates/server/src/game/cod4/wire.rs`), every client's hud
  every frame, found each of 40 `hudelem_s` words and 17 script fields by
  name; the words are compile-time constants (`hw!`) and the fields' engine
  cells resolved once (`hf!`). `frame/mantle and hud` fell from 0.43 to
  0.13 ms.
- `hunt::vantage` gathered its rings in a `HashSet` and then sorted them;
  a flag per node and the rings in the order reached do the same work.
  The sort is stable, so a tie in distance now goes to the node reached
  first where the set's random order picked one before. INFERRED (no
  digest moved).
- The gsc scheduler's `notify` sorted every thread before it looked for
  the waiters; it picks the waiters first, then sorts them, which keeps
  their order (a stable sort of a subsequence).

### What is left

At 32 bots on mp_crossfire the tick's mean is the follower's walks
(`bots/route`, 2.4 ms), CoD4's pmove for the bots' cmds (`bots/moves`,
2.8 ms) and the frame (2.4 ms). Sharing a walk between bots or a probe
between nearby feet changes what they read; threading the per-bot pass
needs the follower and brain off `&mut` world state first. Not done: a
brush met twice in one sweep (from two leaves) is tested twice, but the
second test can write `contents` over a brush between, so skipping it
is not safe; of the brush tests past the new axial check, about half
repeat one the same sweep already made. VERIFIED (counters in a
32-bot mp_crossfire run).
