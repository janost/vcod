# CoD 1.1 map cycle: intermission, `map_restart`, `map` and `map_rotate`

How a level ends and the next one starts: the two script builtins that ask for
it, `ExitLevel`, the engine's `SV_SpawnServer`, `SV_MapRestart_f` and
`SV_MapRotate_f`, the intermission arms in the game module, and what carries
across the boundary. It also records what is *not* in either binary: there are
no engine-side exit rules at all, so every "the round is over" decision in CoD
1.1 MP is script.

Evidence rules as everywhere in this directory, and this document carries no
document-level default because its provenance is not uniform. Two binaries are
mixed and each claim says which one it rests on:

- `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module, which
  carries a full dynamic symbol table. Addresses are module-relative (image
  base 0), the convention `cod11-gsc-object-model.md` and `cod11-combat.md`
  use. It is position-independent, so a plain `objdump -d` hides every call
  and cvar target; `python3 tools/re/annotate_func.py <elf> <symbol>` resolves
  them. `.rodata` runs at the same virtual address as its file offset in this
  module, `.data` runs `0x1000` above it.
- `cod_lnxded`, the 1.1d Linux dedicated server's **engine** executable. It is
  a non-PIC `EXEC` whose first LOAD segment sits at `0x08048000` with file
  offset 0, so a virtual address is the file offset plus `0x8048000` and
  `objdump -d` over an address range reads straight. It carries no symbol
  table, so **no engine name below is a symbol**. Three are established
  against these addresses elsewhere in the repo and are used unqualified:
  `SV_DropClient` (`0x8085cf4`), `SV_ClientEnterWorld` (`0x80877d8`) and
  `SV_ExecuteClientMessage` (`0x80872ec`), all three in
  `docs/protocol-1.1.md` and `tools/re/net-notes.md`. Every other engine name
  below is the Q3 or RTCW name for what the bytes at that address do, my
  label rather than a recovered one, whether or not the sentence using it
  repeats "my name for": `SV_SpawnServer`, `SV_MapRestart_f`,
  `SV_MapRotate_f`, `SV_Map_f`, `NextToken`, `SV_RestartGameProgs`,
  `SV_InitGameProgs`, `SV_ShutdownGameProgs`, `SV_SetConfigstring`,
  `SV_SendServerCommand`, `SV_SendClientGameState`, `SV_GentityNum`,
  `SV_CreateBaseline`, `SV_Frame`, `NET_OutOfBandPrint`, `CM_LoadMap`,
  `Hunk_Clear`, `COM_Parse`, `Cbuf_ExecuteText`, `Cmd_AddCommand`,
  `Cmd_Argv`, `Cvar_Get`, `Cvar_Set`, `Cvar_VariableString`,
  `Q_strncmp`,
  `Cvar_VariableValue`, `Cvar_InfoString`, `Com_Printf`, `Com_DPrintf`,
  `Com_Error`, `Q_strncpyz`, `VM_Call`, `VM_Free` and `va`.
  UNVERIFIED: every name in that list. What each section rests on is the
  address and what the instructions there do, both of which are cited. Any
  engine name below that is not in that list carries its own label where it
  appears, as `NET_Sleep` (3 step 4) and `FS_Restart` (3 step 9) do.

Seven passages below are a bulleted or numbered list introduced by a pair of
labelled sentences rather than by a label per item, the shape
`cod11-combat.md` uses: the list carries exactly two kinds of claim, the
operands and addresses on one side and the ordering and the branch conditions
on the other, and they need opposite labels. Such a pair covers the list items
that immediately follow it and nothing else; no prose paragraph in this
document takes its label from a sentence above it.

Claims that only restate an offset or a table another document already
established cite that document instead of repeating the derivation.
Playerstate field offsets are `docs/protocol-1.1.md`, "PlayerState delta";
`gclient_t` offsets are `cod11-hud-protocol.md` and `cod11-gsc-object-model.md`.

---

## 1. The two builtins: `map_restart` and `exitLevel`

VERIFIED: the `functions` builtin table (`0x7e508`, stride 12, reached from
`Scr_GetFunction` `0x5c15c`) holds `map_restart` at row 101 pointing at
`0x5f618` and `exitlevel` at row 102 pointing at `0x5f684`
(`tools/re/dump_builtins.py`). INFERRED, from the identifier case fold
`cod11-gsc-language.md` records: script may spell the second one `exitLevel()`
and reach the same row.

VERIFIED: the offsets, immediates, string addresses and call targets in the
list below, each read out of the instruction it sits in. INFERRED: the
ordering in it, and every "when" and "otherwise", which are branch conditions.

- A latch word at `level+0x29f0` is read first (`0x5f619`, `0x5f685`).
- When it is non-zero the builtin calls `Scr_Error` with `0x78204`
  (`"map_restart already called"`) if the latch reads 1, and with `0x7821f`
  (`"exitlevel already called"`) otherwise (`0x5f627..0x5f63f`,
  `0x5f693..0x5f6ab`).
- The latch then takes 1 for `map_restart` (`0x5f642`) and 2 for `exitlevel`
  (`0x5f6ae`).
- `level+0x1d50` takes 0 (`0x5f64c`, `0x5f6b8`), and then takes
  `Scr_GetInt(0)` when `Scr_GetNumParam()` is non-zero (`0x5f656..0x5f669`,
  `0x5f6c2..0x5f6d5`). Both builtins take the same optional argument.
- `map_restart` finishes with `trap_SendConsoleCommand(2, "map_restart\n")`,
  the string at `0x78238` (`0x5f671..0x5f67b`). `exitlevel` finishes by
  calling `ExitLevel` (`0x5f6dd`, relocation target `0x510b4`).

INFERRED, off the shared latch: the pair is one-shot per level, and the error
message names whichever of the two got there first rather than the one being
refused.

VERIFIED: `level+0x1d50` is written by `vmMain` case 15 (`0x50f08`) and
returned by case 16 (`0x50f10`); the jump table at `0x75728` puts those two
bodies at indices 15 and 16. VERIFIED: the only other reads of the word in the
module are the two in `G_ShutdownGame` at `0x4ff68` and `0x4ff79`.

VERIFIED: at `0x4ff68` a zero `level+0x1d50` makes `G_ShutdownGame` call
`trap_FreeClientScriptPers`, and at `0x4ff79` the same test supplies
`Scr_FreeGameVariable`'s only argument as `savePersist == 0`. INFERRED, off
those two branches: the word is a "keep the script-side persistence" flag, and
what it keeps is exactly two things, the per-client script `pers` object and
the `game` variable.

VERIFIED: `ClientConnect` (`0x4246c`) opens with
`bzero(&level.clients[n], 0x22c4)` at `0x424a7`, and both the map-change and
the restart client passes call `ClientConnect` again (3 step 22, 4 step 10).
INFERRED, from those two: nothing on the C side of a `gclient_t` survives
either boundary, so the script-side `pers` is the whole of what carries.

VERIFIED: `level` is zeroed for `0x2a14` bytes at the top of `G_InitGame`
(`0x4fc27`), and `0x29f0` is inside that range. INFERRED: the "already called"
latch therefore clears on every level and every restart, which is what makes
the guard per-level rather than per-process.

VERIFIED: `G_InitGame`'s fourth stack argument, `+0x14`, is the same
`savePersist` value, and the `test`/`jne` pair at `0x4fb87` targets `0x4fc1a`,
which is past both the `gameCvarTable` registration loop and the gametype
validity check. INFERRED, off that branch: a non-zero `savePersist` skips
both.

### 1.1 As implemented

`crates/server/src/game/builtins/cvar.rs` carries both builtins; the latch is
`level_latch` and the flag is `save_persist`, both on `crates/server/src/game/host.rs`'s
host struct, and the console line each queues lands in the host's `console`
vector that `crates/server/src/game/script.rs`'s `take_console` drains.
`Vm::take_game` and `Vm::install_game` (`crates/gsc/src/vm/mod.rs`) plus the
`pers_carry` on the host are the two halves of what `savePersist` preserves;
`crates/server/src/game/entity.rs`'s `spawn_client` takes the carried `pers`
back. vcod does not model `gameCvarTable`, so `G_InitGame`'s cvar-loop skip
has no counterpart.

---

## 2. `ExitLevel`

VERIFIED: `ExitLevel` is `0x510b4`, `0xac` bytes, and the module's relocation
table holds exactly one call to it, at `0x5f6de` inside the `exitlevel`
builtin. INFERRED, from that single caller: script is the only thing that ever
ends a level.

VERIFIED: the offsets, immediates, string addresses and call targets in the
list below. INFERRED: the ordering in it.

- `trap_SendConsoleCommand(2, "map_rotate\n")`, the string at `0x756b4`
  (`0x510bd..0x510c4`).
- `level+0x1fc` and `level+0x200` both take 0 (`0x510c9`, `0x510d3`); 6.3
  identifies the two as the axis and allies team scores.
- A first pass over `level.clients` (the pointer at `level+0`, stride
  `0x22c4`, `g_maxclients` at `level+0xc`) zeroes `client+0x20e0` for every
  client whose `client+0x20ec` reads 2 (`0x510dd..0x51118`). VERIFIED:
  `client+0x20e0` is the `score` script field and `client+0x20ec` is the
  connection state whose 2 is fully connected (`cod11-hud-protocol.md`,
  the scoreboard row table).
- A second pass over the same array writes 1 into `client+0x20ec` for every
  client that reads 2 (`0x5111a..0x5114d`). INFERRED: that demotes every
  connected client to the "connecting" state the scoreboard sends `-1` ping
  for, which is what stops the outgoing level counting them.
- `G_LogPrintf("ExitLevel: executed\n")`, the string at `0x756c0`
  (`0x5114f..0x51157`).

VERIFIED: `ExitLevel` writes nothing else: no intermission state on any
client, no camera choice, no write to `level+0x1d50` and no read of a timer.

### 2.1 As implemented

`crates/server/src/game/host.rs`'s `team_scores` and the score field on each
client are what the two passes clear; the queued `map_rotate` goes through the
same `console` vector as section 1's, and `crates/server/src/server.rs`'s
console drain in `tick` executes it.

---

## 3. `SV_SpawnServer`

My name for `cod_lnxded` `0x808a220`. VERIFIED: exactly two call sites,
`0x8083da4` inside `SV_Map_f` (5.1) and `0x8083ea4` inside `SV_MapRestart_f`'s
escalation arm (4.2).

VERIFIED: the offsets, immediates, string addresses and call targets named in
the numbered list below, each read out of the instruction it sits in.
INFERRED: the ordering of the list and every "when", "then" and "otherwise" in
it, all of which are branch conditions. Later tasks and sections cite these as
"doc 3, step N".

1. `Cvar_Get("cl_xmodelcheck", "0", 0x21)`'s `integer` goes to `0x80c2560`
   (`0x808a232..0x808a247`). Strings at `0x80d5462` and `0x80d5460`.
2. When `Cvar_VariableValue("sv_running")` is non-zero (`0x808a252`,
   `"sv_running"` at `0x80d542e`), `savePersist = VM_Call(gvm, 16)`
   (`0x808a274..0x808a281`), which is the word section 1 established. When it
   is zero, `savePersist` is set to 0 instead (`0x808a333`) and steps 3 and 4
   are skipped.
3. For every client whose state word reads greater than 2, that is `CS_PRIMED`
   or `CS_ACTIVE`, the server sends
   `NET_OutOfBandPrint(NS_SERVER, client->netchan.remoteAddress,
   va("loadingnewmap\n%s\n%s", <map>, sv_gametype->string))`
   (`0x808a2c0..0x808a304`). The format string is at `0x80d5471`, the client
   stride is `0x5a8fc`, the state word is at client offset 0 and the 20-byte
   `netadr_t` at client offset `0x528b8` is pushed by value.
4. A 250 ms sleep, `0x80c786c(0xfa)` at `0x808a324`. UNVERIFIED: that
   `0x80c786c` is Q3's `NET_Sleep`; what is verified is the constant and that
   nothing between it and step 3 touches the netchan.
5. `0x808147c()`, `0x8081494()`, then `SV_ShutdownGameProgs` (`0x80892f4`) at
   `0x808a35f`. VERIFIED: `SV_ShutdownGameProgs` calls `VM_Call(gvm, 1, 0)`,
   which is `G_ShutdownGame(restart = 0)`, then frees the VM and clears the
   `gvm` pointer at `0x80e30c4` (`0x80892fa..0x808933f`). INFERRED: a map
   change therefore unloads and reloads the game module, where a restart (4)
   keeps it.
6. The two banners `"------ Server Initialization ------\n"` (`0x80d54a0`) and
   `"Server: %s\n"` (`0x80d54c5`), then `Hunk_Clear` (`0x80682c8`) at
   `0x808a385`.
7. All 2048 configstring slots are freed (`0x808a390..0x808a3ae`), walking the
   pointer array at `0x8355678` with `i <= 0x7ff`. VERIFIED:
   `MAX_CONFIGSTRINGS` is 2048, corroborated by `SV_SetConfigstring`'s own
   bound check `index > 0x7ff` at `0x8089bfc`.
8. `memset(&sv, 0, 0x614ec)` at `0x808a3ba`, with `sv` at `0x8355260`. So
   `sv.state` is `0x8355260`, `sv.restarting` `0x8355264` and
   `sv.checksumFeed` `0x835526c`.
9. `sv.checksumFeed` takes `(rand() << 16) ^ rand() ^ 0x80c8520()` and is
   handed to `0x80621e4` (`0x808a3e1..0x808a413`). INFERRED, off the call
   order: that is Q3's `FS_Restart(sv.checksumFeed)`.
10. All 2048 configstring slots are pointed at the empty string `0x80d5243`
    (`0x808a422..0x808a440`).
11. `sv_running` is read again (`0x808a445`): zero takes `0x806dca0` and
    `0x8089df0`, non-zero takes `0x806e7e8` and, when `sv_maxclients->modified`
    is set, `0x8089f04` (`0x808a442..0x808a483`).
12. The snapshot entity and per-client snapshot rings are allocated from two
    sizes derived from `0x83b67b0` and `0x83b67b4` (`0x808a485..0x808a4c8`),
    and six counters at `0x83b67c8`, `0x83b67cc` and `0x83b67d8..0x83b67e4`
    are zeroed (`0x808a4aa..0x808a513`).
13. `svs.snapFlagServerBit ^= 4`, the byte at `0x83b67a8` (`0x808a516`).
    VERIFIED: `SV_MapRestart_f` toggles the same byte with the same mask
    (4 step 4). INFERRED: both a map change and a restart flip it.
14. `Cvar_Set("nextmap", "map_restart")` (`0x808a520`), names at `0x80d54dd`
    and `0x80d54d1`. See 7.3.
15. `0x808a12c(va("maps/mp/%s.bsp", <map>))` at `0x808a544`,
    `Cvar_Set("cl_paused", "0")` at `0x808a559`, then
    `CM_LoadMap(va("maps/mp/%s.bsp", <map>), 0, &checksum)` at `0x808a57f`
    and `Cvar_Set("mapname", <map>)` at `0x808a593`. The bsp path format is
    `0x80d54e5`; `"cl_paused"` `0x80d54f4`, `"mapname"` `0x80d54fe`.
16. The serverId, one byte at `0x80e30c0`, takes `+0x10` truncated to a byte
    (`movzbl`), and then a second `+0x10` when the high nibble came out zero
    (`0x808a598..0x808a5b7`). INFERRED, off that second add: the high nibble
    cycles 1 to 15 and never reads 0 after the first load.
    `Cvar_Set("sv_serverid", va("%i", id))` follows at `0x808a5d8`, names at
    `0x80d5506` and `0x80d53d0`.
17. `0x8355268` takes `0x833df1c` (`0x808a5e2`), `sv.state` takes 1
    (`0x808a5e7`), `Cvar_Set("sv_serverRestarting", "1")` (`0x808a601`), name
    at `0x80d5512`.
18. `0x80699d0(va("maps/mp/%s.bsp", <map>), 2)` at `0x808a620`, then
    `0x80bfea0(1)` at `0x808a62d`. UNVERIFIED: what either does.
    `0x80bfea0` is a two-instruction setter that writes its argument into
    `0x83179e0`.
19. `SV_InitGameProgs(savePersist)` at `0x808a63c`, my name for `0x8089124`.
    VERIFIED: it creates the VM and calls
    `VM_Call(gvm, 0, svs.time, <seed>, 0, savePersist)`, that is
    `G_InitGame(levelTime, seed, restart = 0, savePersist)`
    (`0x8089196..0x80891ab`). INFERRED, off that argument order against step
    2: the flag read from the *outgoing* level is what the *incoming* level's
    `G_InitGame` receives.
20. Three settle frames: `svs.time` (`0x83b67a4`) gains 100 and `0x808d3d4()`
    runs, three times (`0x808a644..0x808a65d`).
21. The baseline pass, Q3's `SV_CreateBaseline` inlined
    (`0x808a669..0x808a734`). It runs `i` from 1 to the entity count at
    `0x83b6684`, skips any entity whose `+0xf0` is zero, writes `i` into the
    entity's `s.number`, and copies 240 bytes of entity state plus eight more
    words from `+0xf4`, `+0xf8` and the six floats at `+0x11c..+0x130` into a
    `0x17c`-byte baseline record. INFERRED: `+0xf0` is `r.linked`, since
    nothing else in the record gates the copy, and entity 0 is never
    baselined because the loop starts at 1.
22. The client pass (`0x808a73a..0x808a7a3`): for every client whose state
    word reads greater than 1, `VM_Call(gvm, 2, i, client[+0x5a8f0])`, which
    is `ClientConnect`. A non-null return is a denial string and the client is
    dropped through `SV_DropClient` (`0x8085cf4`); otherwise the client's
    state word is set to 2, `CS_CONNECTED`.
23. The pure-server pak cvars: `sv_paks`, `sv_pakNames`, `sv_referencedPaks`
    and `sv_referencedPakNames` (`0x808a7a5..0x808a891`, names at `0x80d5526`,
    `0x80d556e`, `0x80d557a`, `0x80d558c`).
24. `SV_SetConfigstring(1, Cvar_InfoString(CVAR_SYSTEMINFO))` at `0x808a8c9`
    and `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))` at
    `0x808a8e1`, each clearing its own bit of the modified-flags word at
    `0x834a0e8` (`0x808a8b6`, `0x808a8e6`), then the `0x800`-flagged cvar
    range through `0x806fbec(0x8c, 0x40, 0x800)` at `0x808a8ff`; 3.2 says
    what that one writes.
25. `sv.state` takes 2 (`0x808a90b`), the heartbeat `0x8084bd0()` runs
    (`0x808a915`), `Cvar_Set("sv_serverRestarting", "0")` (`0x808a927`) and
    the closing banner `0x80d55c0` prints.

### 3.1 No gamestate is pushed

VERIFIED: the whole of `SV_SpawnServer`, `0x808a220` to `0x808a948`, contains
no call to `SV_SendClientGameState` (`0x8085eec`), and the module has exactly
three calls to that function, at `0x8087447`, `0x80876b1` and `0x8087a1e`, all
of them reached from `SV_ExecuteClientMessage` (`0x80872ec`) or the `donedl`
handler.

VERIFIED: `SV_SpawnServer` writes configstrings 1 and 0 at `0x808a8c9` and
`0x808a8e1`, its last write of `sv.state` ahead of both is the 1 of step 17,
and it holds no write of `sv.restarting` at all. VERIFIED:
`SV_SetConfigstring` (`0x8089bf0`) holds a `cmp sv.state, 2` at `0x8089c6c`
and a `cmp sv.restarting, 0` at `0x8089c75` between its store of the new
string and its per-client loop.

INFERRED, off those two compares: the loop runs only when `sv.state` reads 2
or `sv.restarting` is set, so neither of `SV_SpawnServer`'s two configstring
writes reaches a client. INFERRED, from that and the negative above: a map
change sends nothing at all on the reliable stream, what every client at
`CS_PRIMED` or above gets is the out-of-band `loadingnewmap` line of step 3,
and the gamestate arrives only once that client's next message reaches the new
server carrying the old serverId and `SV_ExecuteClientMessage` resends it
(4.4), which makes the map change pull-shaped.

### 3.2 The `0x800` cvar range is configstrings 140..203 and 204..267

VERIFIED: `0x806fbec`, the call of step 24, holds a walk of the cvar list
headed at `0x834a0e0` with its next pointer at `+0x24`, a
`test cvar->flags, 0x800` at `0x806fc05` and a call to
`0x808b148(<start>, <count>, cvar->name, cvar->string)` at `0x806fc16`.
VERIFIED: `0x808b148` holds a loop over configstrings `<start> + i` bounded by
`i < <count>`, a `cmp` of the slot's first byte against 0 at `0x808b172`, a
`SV_SetConfigstring(<start> + i, name)` at `0x808b17c`, a `strcasecmp` of the
slot against `name` at `0x808b18b`, a `Com_Error` with
`"SV_SetConfigValueForKey: overflow"` (`0x80d52e0`) at `0x808b1a6` and a
`SV_SetConfigstring(<start> + <count> + i, value)` at `0x808b1be`.

INFERRED, off those two compares and the error's address: a `0x800`-flagged
cvar takes the first slot that is empty or already carries its name, its name
goes in that slot and its value `<count>` slots higher, and a table with no
free slot errors.

VERIFIED: step 24 passes `<start>` `0x8c`, which is 140, and `<count>`
`0x40`, which is 64. INFERRED, from those two numbers against the ranges
`cod11-hud-protocol.md` and `cod11-server-handshake.md` record: this is the
writer for the 140..203 cvar names and 204..267 values that both documents
say they could not find in the game module. It is in the engine, not the game
module, which is why neither found it.

### 3.3 As implemented

`crates/server/src/server.rs`'s `spawn_server` carries this list;
`snap_flag_server_bit` is step 13, the `loadingnewmap` out-of-band is step 3,
and `crates/server/src/client.rs`'s `Client::reset_for_level` is what step 22
does to each surviving client. What keeps 3.1's negative is the caller set
rather than a gate inside one function: `send_configstring_update` sends
whatever it is handed, `broadcast_configstring_changes` is the per-frame half
and skips a client that has no gamestate to patch yet, and the spawn re-syncs
the copy that half diffs against after its settle frames, so the map path puts
no `d` on the wire at all. `crates/server/src/console.rs` owns the serverId
nibble arithmetic of step 16 so that 4 step 4 can share it.

Step 21 is the one vcod does not follow, and the divergence is in the code
rather than in the reading above. Retail's loop baselines every entity it
copies, so a map's items, movers and turrets reach a client's first frame as
a delta against their own baseline. `rebuild_baselines` writes only what
`--test-entities` puts on the wire and leaves script entities to go out
against a null baseline, which is what `Server::new` already does on the
first map; a map change keeps that rather than giving the second map a wire
shape the first never has. The cost is bytes on the frame each entity first
appears in, not correctness: a null baseline is a legal delta base, and every
later frame deltas against the previous one either way.

---

## 4. `SV_MapRestart_f`

My name for `cod_lnxded` `0x8083de4`. VERIFIED: registered as the console
command `map_restart` by `Cmd_AddCommand("map_restart", 0x8083de4)` at
`0x8084b0a`, the name string at `0x80d3fe7`. VERIFIED: it is also called
directly from `SV_Map_f` (`0x8083d96`) and from `SV_MapRotate_f`'s three
failure arms, which share one call at `0x808422d`.

VERIFIED: the offsets, immediates, string addresses and call targets in the
numbered list. INFERRED: the ordering and every branch condition in it.

1. When `0x8355268` already equals `0x833df1c` the function returns at once
   (`0x8083ded`). VERIFIED: the restarting path writes `0x833df1c` into
   `0x8355268` at `0x8083eef`, and `SV_SpawnServer` writes the same pair at
   `0x808a5e2`. INFERRED: this makes a second restart within one engine frame
   a no-op, and a restart immediately after a spawn a no-op too.
2. When `0x833efc0`'s `integer` is zero the function prints
   `"Server is not running.\n"` (`0x80d392e`) and returns
   (`0x8083dfe..0x8083e16`).
3. `savePersist = VM_Call(gvm, 16)` (`0x8083e20..0x8083e30`), the same getter
   section 1 established. When it is zero, two escalation checks run: a
   `g_gametype` cvar (pointer at `0x83b6778`) whose `latchedString` differs
   from its `string` prints `"g_gametype variable change -- restarting.\n"`
   (`0x80d3960`), and a `sv_maxclients` cvar (pointer at `0x8355250`) with
   `modified` set prints
   `"sv_maxclients variable change -- restarting.\n"` (`0x80d39a0`); either
   then does `Q_strncpyz(buf, Cvar_VariableString("mapname"), 0x40)` and calls
   `SV_SpawnServer(buf)` (`0x8083e7e..0x8083ea4`). When `savePersist` is
   non-zero both checks are jumped over (`0x8083e35`). INFERRED, off that
   jump: a script that asked to keep its persistence gets an in-place restart
   even across a gametype or maxclients change, where one that did not gets
   escalated to a full spawn.
4. `0x808b2c8()` zeroes six words at `0x83b67c8`, `0x83b67cc` and
   `0x83b67d8..0x83b67e4` (`0x8083eb0`), then `svs.snapFlagServerBit ^= 4`,
   the byte at `0x83b67a8` (`0x8083eba`).
5. The serverId, the same byte at `0x80e30c0`, takes
   `(id & 0xf0) + ((id + 1) & 0xf)` (`0x8083eb5..0x8083ece`), and
   `Cvar_Set("sv_serverid", va("%i", id))` follows (`0x8083ed3..0x8083eea`).
   INFERRED, against 3 step 16: a restart moves only the low nibble and a map
   change only the high one.
6. `sv.state` and `sv.restarting` both take 1 (`0x8083ef9`, `0x8083f03`), then
   `Cvar_Set("sv_serverRestarting", "1")` (`0x8083f0d..0x8083f1d`) and
   `0x80bfea0(1)` (`0x8083f27`).
7. `SV_RestartGameProgs(savePersist)`, my name for `0x8089350`, at
   `0x8083f33`.
8. Three settle frames, the same shape as 3 step 20: `svs.time += 100` then
   `0x808d3d4()`, three times (`0x8083f3b..0x8083f4d`).
9. For every client whose state word reads greater than 1,
   `SV_SendServerCommand(client, 1, "n")` (`0x8083f6e..0x8083f79`, the letter
   at `0x80d39f1`).
10. Then, for the same client, `VM_Call(gvm, 2, i, client[+0x5a8f0])`, that is
    `ClientConnect` (`0x8083f7e..0x8083f8f`). A non-null return drops the
    client through `SV_DropClient` and prints
    `"SV_MapRestart_f: dropped client %i - denied!\n"` (`0x80d3a00`).
11. Otherwise, only when the client's state word reads exactly 4
    (`CS_ACTIVE`), `SV_ClientEnterWorld(client, &client->lastUsercmd)`
    (`0x80877d8`) at `0x8083fc8`, the usercmd at client offset `0x10624`.
    INFERRED, off that compare: a `CS_PRIMED` client is not re-entered here
    and is promoted later by its own next message (4.4).
12. `sv.state` takes 2 and `sv.restarting` takes 0 (`0x8083fe1`, `0x8083feb`),
    then `Cvar_Set("sv_serverRestarting", "0")` (`0x8083ff5`).

### 4.1 `SV_RestartGameProgs`

My name for `0x8089350`. VERIFIED: it holds two `VM_Call`s,
`VM_Call(gvm, 1, 1)` at `0x808935d..0x8089367`, which is
`G_ShutdownGame(restart = 1)`, and
`VM_Call(gvm, 0, svs.time, <seed>, 1, savePersist)` at
`0x80893ad..0x80893c4`, which is
`G_InitGame(levelTime, seed, restart = 1, savePersist)`. INFERRED, off their
addresses: the shutdown runs first and the init second. VERIFIED: the function
holds no call to `VM_Free`, where `SV_ShutdownGameProgs` does (3 step 5).
VERIFIED: it also clears `client[+0x10a40]` for every client
(`0x80893d1..0x80893fa`).

VERIFIED: `G_InitGame`'s third argument, `+0x10`, is the `restart` flag, and
its only use in the function is to skip `ClearRegisteredItems`
(`0x4fe0c..0x4fe12`).

### 4.2 The escalation, and why `map <samemap>` is a restart

VERIFIED: `SV_Map_f` (`0x8083c68`, 5.3) holds a call to `SV_MapRestart_f` at
`0x8083d96`, one to `SV_SpawnServer` at `0x8083da4`, a
`cmp com_sv_running->integer, 0` at `0x8083d77` and a `strcasecmp` of the
requested name against the `mapname` cvar's string at `0x8083d8a`, and both
compares carry a jump to `0x8083da0`, the instruction ahead of the
`SV_SpawnServer` call. INFERRED, off those two compares and their jumps:
`map <currentmap>` on a running server is a restart and anything else is a
full spawn.

### 4.3 Where the new systeminfo comes from

VERIFIED: `SV_Frame`'s cvar flush at `0x808d090..0x808d0dd` holds a `test` of
bit 2 of the modified word at `0x834a0e8` at `0x808d090` over a
`SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))` at `0x808d0a9` and
an `and` clearing that bit at `0x808d0ae`, and a `test` of bit 3 at
`0x808d0b8` over a `SV_SetConfigstring(1, Cvar_InfoString(CVAR_SYSTEMINFO))`
at `0x808d0d1` and an `and` clearing it at `0x808d0d6`. INFERRED, off those
two tests: bit 2 pushes configstring 0 and bit 3 configstring 1, once per
frame in which the bit was set.

VERIFIED: `SV_SetConfigstring`'s broadcast gate is the compare pair of 3.1.
INFERRED, off step 5's `Cvar_Set` and that gate: the `d 1 ...` a client sees
after `n` is this flush carrying the bumped `sv_serverid`, which is the
mechanism behind the live measurement recorded in `docs/protocol-1.1.md`,
"map_restart and sv_serverid".

### 4.4 `SV_ExecuteClientMessage`'s two nibble branches

My name for `cod_lnxded` `0x80872ec`.

VERIFIED: it holds a `cmp` of the client's stored serverId at client offset
`0x5a8f8` against the `sv_serverid` byte at `0x80e30c0` at `0x808733f` whose
`je` targets `0x8087451`, and a `cmp client+0x10a64, 0` at `0x808734a` whose
`jne` targets the same `0x8087451`. VERIFIED: `client+0x10a64` is the
`downloadName` byte, the same offset `docs/protocol-1.1.md` uses for the
download state. INFERRED, off those two compares and their shared target: a
matching serverId takes the normal path, and so does a client with a
non-empty `downloadName` whatever its serverId.

VERIFIED: two `and`s masking both ids with `0xf0` sit at `0x8087357` and
`0x808735d`, and the `cmp` of the two results at `0x8087362` carries a `jne`
to `0x8087416`. VERIFIED: at `0x8087416` a `cmp` of the client's
`messageAcknowledge` (offset `0x10618`) against its `gamestateMessageNum`
(offset `0x1061c`) at `0x808741f` carries a `jle` to `0x808752c`, the
function's exit, and past that `cmp` sit a `Com_DPrintf` with
`"%s : dropped gamestate, resending\n"` (`0x80d4b00`) at `0x808743b` and a
call to `SV_SendClientGameState` (`0x8085eec`) at `0x8087447`. INFERRED, off
the first of those two compares: differing high nibbles are the map-change
case. INFERRED, off the second and its `jle`: the gamestate is resent only to
a client that has acknowledged a message past the one its last gamestate went
in, and every other client returns without reading an op.

VERIFIED: the fall-through from the `cmp` at `0x8087362` reaches a
`cmp client+0, 3` at `0x808736a` whose `jne` targets `0x808752c`. VERIFIED:
past that `cmp` sit `0x80bfea0(1)` at `0x8087378`, a `Com_DPrintf` with
`"Going from CS_PRIMED to CS_ACTIVE for %s\n"` (`0x80d46a0`) at `0x8087394`,
a store of 4 into `client+0` at `0x808739f`, a call to
`SV_GentityNum(clientNum)` at `0x80873b8` whose result goes into
`client+0x10a40` at `0x80873c2`, a store of -1 into `deltaMessage` (offset
`0x10b04`) at `0x80873c8`, a store of `svs.time` into offset `0x10b14` at
`0x80873dd` and a `VM_Call(gvm, 3, clientNum)` at `0x808740c`. VERIFIED: 3 is
`ClientBegin` in the `vmMain` jump table of section 1, which puts that case at
`0x50e70`. INFERRED, off the `cmp client+0, 3` and its `jne`: matching high
nibbles with a differing low nibble promote a `CS_PRIMED` client and drop the
message unread for every other state.

### 4.5 As implemented

`crates/server/src/server.rs`'s `map_restart` carries the numbered list,
sharing `crates/server/src/console.rs`'s nibble arithmetic with `spawn_server`
for step 5. `crates/server/src/game/script.rs`'s `load`/`load_from` take the
carried `game` and `pers` tables that 4.1's two `VM_Call`s stand for, and
`reconnect_client` is step 10. vcod has no VM to free, so 3 step 5's unload
against 4.1's reuse shows up only as whether those tables are carried; the
`restart` flag stays on `Server::load_scripts_with`, where what it decides is
that the animtree, the weapon files and the hit-location table are not
re-read, since a restart changes neither the paks nor the map.

Retail's restart has no baseline pass, and `map_restart` has none either.

`EF_TELEPORT_BIT` crosses `Client::reset_for_restart` the way every other
piece of playerstate does, which is to say it does not: step 11 rebuilds
`crates/server/src/spectate.rs`'s `ClientSim` through `enter_world`, and the
new one starts with the bit clear, so the incoming level's first spawn is the
flip that puts the client at 24. VERIFIED, from the retail round-restart
capture: the shooter reads `eFlags` 16 on the life before the restart and 24
on the one
after (`crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-shooter.txt`,
`!trace ms=697` and `ms=5765`), and the target the same. INFERRED, from that
pair against the same file's `ms=28607` and `ms=118532`, two consecutive
lives both at 24: what the restart moved is not an alternation the outgoing
life was carrying, since a plain respawn does not always change the word;
8.2 reads the rule off all three captures spawn for spawn. Leave the bit
pinned and a retail client interpolates a respawning player from its old
position to its new spawn.

Not modelled from the numbered list: step 6's `sv.state`, `sv.restarting`
and the `sv_serverRestarting` cvar, none of which vcod has a counterpart for
(what `sv.restarting` gates on retail, `SV_SetConfigstring`'s broadcast, vcod
does unconditionally); and step 10's drop of a client whose `ClientConnect`
returned a denial string, since `reconnect_client` has no denial to return.

Three divergences on the wire:

- The reliable stream carries `d 3`, `n` and `d 1`, in that order, and
  nothing else. VERIFIED, from the retail capture the order comes from
  (`crates/server/tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`, seq
  40 to 44): the burst opens with `d 13 70050` and a `d 12`, all five
  commands carry `ms=51016`, and `70050` is the number the `d 3` two lines
  later carries as its own `t` field
  (`d 3 n\ambient_mp_carentan\t\70050`). INFERRED, from that shared number
  and 3.1's compare pair: those are the *incoming* level's own writes,
  reaching the wire because `sv.restarting` is set and `SV_SetConfigstring`
  broadcasts every write while it is.
- vcod writes neither of those two off the level clock, which is why it has
  nothing to broadcast for them: `crates/server/src/game/builtins/env.rs`'s
  `ambient_play` pins configstring 3's `t` field to `0`, and configstring 13,
  `level.startTime`, is a static `"0"` in
  `crates/server/src/configstrings.rs`. Configstring 12 it does compute, and
  byte-identically (`set_cull_fog`, in `builtins/env.rs` beside
  `ambient_play`), but a restart
  rebroadcasts only 1 and 3, so a client that survives one keeps the old
  level's string in every other slot. Benign only because none of vcod's own
  values move across a restart; driving 3 and 13 off `level_time_ms` is what
  would make them move, and it moves the configstring gates with them.
- The cvar table is carried from the outgoing level rather than rebuilt from
  `default_mp.cfg`, on this path and on the map change alike, because
  retail's `Cvar_Set` writes a process-global table that neither boundary
  clears. A `+set` override is replayed on top either way, so it still
  outranks a script's `setCvar`.

---

## 5. `SV_MapRotate_f` and the rotation grammar

My name for `cod_lnxded` `0x80840c8`. VERIFIED: registered as the console
command `map_rotate` by `Cmd_AddCommand("map_rotate", 0x80840c8)` at
`0x8084b31`, the name string at `0x80d3ff3`.

VERIFIED: two cvars drive it, `sv_mapRotation` (pointer at `0x8355238`, name
`0x80d57b5`) and `sv_mapRotationCurrent` (pointer at `0x8355224`, name
`0x80d57c4`), both registered with an empty default at
`0x808ad5c..0x808ad82`.

### 5.1 `NextToken`

My name for `cod_lnxded` `0x8084014`. VERIFIED: the offsets, string addresses
and call targets below. INFERRED: the ordering and the branch conditions.

- It takes `sv_mapRotationCurrent`'s current string and runs `COM_Parse`
  (`0x8081d1c`) over it, which returns one token and advances the pointer
  (`0x808401c..0x808402e`).
- When the advanced pointer is null the function does
  `Cvar_Set("sv_mapRotationCurrent", "")` and returns null
  (`0x808403f..0x8084053`).
- Otherwise it copies the remainder onto the stack and does
  `Cvar_Set("sv_mapRotationCurrent", <remainder>)` before returning the token
  (`0x8084055..0x80840bb`).

INFERRED, off that write-back: the rotation is consumed destructively, one
token per call, the cvar is the whole of the cursor, and a server restarted
mid-rotation resumes where the cvar left it.

### 5.2 The loop

VERIFIED: the string addresses, call targets and immediates below. INFERRED:
the ordering and the branch conditions.

- Three banners print first: `"map_rotate...\n\n"` (`0x80d3a45`),
  `"\"sv_mapRotation\" is:\"%s\"\n\n"` (`0x80d3a55`) and
  `"\"sv_mapRotationCurrent\" is:\"%s\"\n\n"` (`0x80d3a80`)
  (`0x80840cf..0x8084106`).
- When `sv_mapRotationCurrent` is the empty string it is refilled from
  `sv_mapRotation` (`0x808410b..0x8084131`), and if the first `NextToken`
  still comes back null it is refilled and retried once more
  (`0x808413f..0x808415a`).
- A null token after that prints
  `"No map specified in sv_mapRotation - forcing map_restart.\n"`
  (`0x80d3ac0`) and calls `SV_MapRestart_f` (`0x808422d`).
- Token `"gametype"` (`0x80d3afb`, case-insensitive) reads one more token. A
  null one prints `"No gametype specified after 'gametype' keyword in
  sv_mapRotation - forcing map_restart.\n"` (`0x80d3b20`) and calls
  `SV_MapRestart_f`. Otherwise it prints `"Setting g_gametype: %s.\n"`
  (`0x80d3b79`), and when the server is running and the token differs from
  `g_gametype`'s current string case-insensitively it calls
  `VM_Call(gvm, 15, 0)` (`0x80841ae..0x80841e2`), which is section 1's
  `savePersist` setter writing 0. Then `Cvar_Set("g_gametype", <token>)`
  (`0x80d3b92`) and the loop continues.
- Token `"map"` (`0x80d3b9d`, case-insensitive) reads one more token. A null
  one prints `"No map specified after 'map' keyword in sv_mapRotation -
  forcing map_restart.\n"` (`0x80d3bc0`) and calls `SV_MapRestart_f`.
  Otherwise it prints `"Setting map: %s.\n"` (`0x80d3c0f`) and calls
  `Cbuf_ExecuteText(0, va("map %s\n", <token>))` (`0x8084242..0x8084256`),
  and returns.
- Anything else prints `"Unknown keyword '%s' in sv_mapRotation.\n"`
  (`0x80d3c40`), reads the next token and loops (`0x8084260..0x8084278`).

INFERRED, off the `VM_Call(gvm, 15, 0)` arm: a gametype change in the rotation
drops whatever persistence the outgoing level asked to keep, because the flag
`SV_SpawnServer` reads at 3 step 2 has been zeroed before the `map` line
queues.

### 5.3 `SV_Map_f`

My name for `cod_lnxded` `0x8083c68`. VERIFIED: it is registered twice, as
`map` (`0x80d3b9d`) at `0x8084b1f` and as `devmap` (`0x80d3919`) at
`0x8084b58`. VERIFIED: it holds a comparison of `Cmd_Argv(0)` against
`"devmap"` at `0x8083d13` whose result is kept in a register across the whole
function (`0x8083d1d..0x8083d20`), a `test` of that register at `0x8083dac`
with a `je` to `0x8083dc4`, and a `Cvar_Set("sv_cheats", "1")` (`0x80d3922`)
at `0x8083dbd`. INFERRED, off that test and its jump: `sv_cheats` is set only
when the command word was `devmap`.

VERIFIED: it holds a `Q_strncmp` of the map argument against `mp/`
(`0x80d38e3`) at `0x8083d2e` and against `mp\` (`0x80d38e7`) at `0x8083d45`,
and one `Q_strncpyz` at `0x8083d6a` reached from two argument setups, one
pushing the argument plus 3 with a bound of `0x3d` (`0x8083d51..0x8083d5a`)
and one pushing the argument itself with a bound of `0x40`
(`0x8083d60..0x8083d65`). INFERRED, off those two comparisons and the two
setups they select: either prefix is stripped and the bound shrinks with it. The compares picking
`SV_MapRestart_f` over `SV_SpawnServer` are 4.2's.

### 5.4 As implemented

`crates/server/src/console.rs` carries the `Command` parsing, 5.1's
destructive token consumer and 5.2's keyword loop;
`crates/server/src/server.rs`'s console drain in `tick` is what runs the
`map %s` line the loop queues. vcod does not model `devmap` or `sv_cheats`.

---

## 6. Intermission

### 6.1 What arms it

VERIFIED: `ClientEndFrame` (`0x40e98`) holds a `cmp client+0x20ec, 2` at
`0x40ebe` whose `jne` targets `0x414ed`, the function's exit, and a
`cmp client+0x20d0, 3` at `0x40ed1`. INFERRED, off those two compares and
their addresses: a client that is not fully connected reaches none of the
three arms, and for one that is, `sessionstate` 3 takes the intermission arm
(`cod11-gsc-object-model.md`, "What `ClientEndFrame` writes for a live
client's own view").

VERIFIED: `Scr_SetClientField` maps the four legal `sessionstate` strings onto
that word as `playing` 0, `dead` 1, `spectator` 2, `intermission` 3, comparing
against `scr_const+0xd0`, `+0xfa`, `+0x7c` and `+0xb6` and storing the four
literals at `0x41988`, `0x4199d`, `0x419b9` and `0x419ce`. VERIFIED: those
four `scr_const` slots are filled by `GScr_LoadConsts` from the strings at
`0x763b8`, `0x764cd`, `0x76164` and `0x762be`, and the error string at
`0x72dc0` reads
`"'%s' is an illegal sessionstate string. Must be playing, dead, spectator, or
intermission."`. VERIFIED: `0x419ce` is the module's only store of 3 into
`client+0x20d0`; the six other stores of that word write 0, 1 or 2. INFERRED,
from that: intermission is entered only by script assigning
`self.sessionstate = "intermission"`.

### 6.2 The two arms

`ClientEndFrame`'s intermission arm is `0x40ed6..0x40f1f`. VERIFIED: the
offsets and immediates in the list below, each a single store. INFERRED: the
order of the list.

- `ent.takedamage` (`+0x171`) 0 and `ent.r.contents` (`+0x118`) 0.
- `ent.r.svFlags` (`+0xf4`) `(svFlags & ~2) | 1`.
- `ps.pm_type` (`+0x4`, netfield offset 4) 5.
- `ps.viewmodelIndex` (`+0xbc`, netfield offset 188) 0.
- `ps.pm_flags` (`+0xc`) loses bit `0x40000`, written as
  `and byte [edx+0xe], 0xfb`. That is the "looking out of its own body" bit
  `cod11-gsc-object-model.md` records.
- `ps.eFlags` (`+0x80`, netfield offset 128) is masked with `0xfffbfbff`,
  which clears bits `0x400` and `0x40000`. UNVERIFIED: what either bit means.

The arm writes no health, and the client's own health is gone by the time it
is on the wire. VERIFIED: the dm map-change capture's `pm_type=5` traces read
`health` 0 and `eFlags` 24 where the same client read 100 and 16 on the frame
before (`crates/server/tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`).
VERIFIED: `ClientSpawn` (`0x4268c`), which `spawnIntermission`'s own
`self spawn(origin, angles)` reaches, bzeroes the whole `gclient_t` for
`0x22c4` bytes at `0x42804`, copying `client+0x20d0` out and back for `0x104`
bytes around it (`0x427ea`, `0x42824`), and stores no health at all.
INFERRED, from those two against the arm's store list: `ps.stats[0]` is
zeroed by that spawn and nothing copies `ent->health` back into it for a
client in intermission, which is where the 0 on the wire comes from.

VERIFIED: `ClientThink_real` (`0x3fee0`) holds a `cmp client+0x20d0, 3` at
`0x3ffca` whose `jne` targets `0x40010`, and the block at
`0x3ffcf..0x4000a` past it holds four stores and one jump: `client+0x21ec`
takes `client+0x21e8`, `client+0x21e8` takes the byte at `client+0x20f4`,
`client+0x21f8` takes `client+0x21f4`, `client+0x21f4` takes the byte at
`client+0x20f5`, and `0x4000a` jumps to `0x40653`, the function's exit.

INFERRED, off that compare and that jump: `sessionstate` 3 reaches only those
four stores, so no pmove runs, no events are generated and the usercmd's
movement axes and view angles are dropped; an intermission client cannot move,
and its view is whatever the last pre-intermission frame left.

### 6.3 The scoreboard drain, and which team score is which

VERIFIED: `G_RunFrame` carries an inlined drain at `0x50ae9..0x50b42` holding
a `cmp level+0x20c, 0` at `0x50ae9`, a client loop bounded by `level+0x1e0` at
`0x50af4`, a `cmp client+0x20ec, 2` at `0x50b0b`, a `cmp client+0x4, 5` at
`0x50b14`, a call to `DeathmatchScoreboardMessage(&g_entities[i])` at
`0x50b1e` and a store of 0 into `level+0x20c` at `0x50b3b`.

INFERRED, off those three compares and the store's address: the loop runs only
when `level+0x20c` is set, sends to each client that passes both compares, and
clears the flag afterwards. INFERRED, off the `client+0x4` compare against
6.2's store: `client+0x4` is `ps.pm_type`, the periodic scoreboard push is
exactly the set of clients in intermission, and the `pm_type` the intermission
arm writes is what selects them.

VERIFIED: `level+0x20c` is set to 1 by `CalculateRanks` at `0x50cf5` and by
the `setteamscore` builtin at `0x5ba7e`.

VERIFIED: the module's relocation table holds five `R_386_PC32` references to
`CalculateRanks` (`0x50c60`): at `0x418ef` and `0x41b07`, both inside the
range 6.1 reads `Scr_SetClientField`'s `sessionstate` stores out of, and at
`ClientConnect+0x20c` (`0x42678`), `ClientDisconnect+0x174` (`0x42c20`) and
`ClientBegin+0x21` (`0x42fa9`). INFERRED, from the first two sitting among
the client field setter's arms: a script write to a client field is what
dirties the flag during a live level, and the other three are the roster
changing.

VERIFIED: the `setteamscore` builtin (`0x5b9dc`, functions table row 88) reads
a const string through `Scr_GetConstString(0)`, holds a `cmp` of it against
`scr_const+0x4` at `0x5b9f2` and against `scr_const+0x8` at `0x5b9fb`, a
`Scr_Error` with `"Illegal team string '%s'. Must be allies, or axis."`
(`0x77fc0`) at `0x5ba22`, a `Scr_GetInt(1)` at `0x5ba2f`, a third `cmp`
against `scr_const+0x4` at `0x5ba37`, and two store pairs: `level+0x200` with
`trap_SetConfigstring(6, va("%i", n))` at `0x5ba40..0x5ba59`, and
`level+0x1fc` with `trap_SetConfigstring(5, ...)` at `0x5ba60..0x5ba79`. The
`"%i"` format is `0x77fac`. VERIFIED: `GScr_LoadConsts` fills `scr_const+0x4`
from `"allies"` (`0x75f41`) and `scr_const+0x8` from `"axis"` (`0x75f51`) at
`0x58598` and `0x585c8`.

INFERRED, off the third `cmp` and the two jumps out of it: the `allies` arm
takes the first store pair and the `axis` arm the second, and a string that is
neither takes the error.

INFERRED, from the two stores against the two consts: `level+0x1fc` is the
**axis** score and `level+0x200` the **allies** score, and configstring 5
carries axis where 6 carries allies.

VERIFIED: `getteamscore` (`0x5f18c`, row 87) reads the same two slots against
the same two consts (`0x5f1da..0x5f1f0`), and
`DeathmatchScoreboardMessage` (`0x459c0`) pushes `level+0x1fc` before
`level+0x200` into `"b %i %i %i%s"` (`0x737f0`) at `0x45b14..0x45b27`.
INFERRED, from that push order: the `b` command's token 2 is axis and token 3
is allies, which is what `cod11-hud-protocol.md` records off the scoreboard
writer alone; the two score builtins are the independent confirmation.

### 6.4 The three dead functions

VERIFIED: `IntermissionClientEndFrame` (`0x4166c`),
`ClientIntermissionThink` (`0x41628`) and
`SendScoreboardMessageToAllIntermissionClients` (`0x50bf0`) have no reference
of any kind in the module: no relocation targets them, no `call` reaches them
and their addresses appear as no immediate. VERIFIED:
`SendScoreboardMessageToAllIntermissionClients`'s body
(`0x50bf9..0x50c4b`) is the same drain `G_RunFrame` inlines at 6.3.

INFERRED, off those three being unreferenced: the Q3 intermission machinery
they came from was compiled but never wired up, and 6.2's two arms plus 6.3's
drain are the whole of intermission in the shipped module.

### 6.5 As implemented

`crates/server/src/spectate.rs` carries 6.2: `PmType::Intermission`,
the `pm_type` 5 and `eFlags` writes, and the "no pmove" arm.
`become_intermission` zeroes the sim's own health for the bzero above, and
`crates/server/src/server.rs`'s per-frame vitals mirror skips an
intermission sim so nothing writes it back.
`crates/server/src/server.rs`'s `scoreboard` carries 6.3's drain and takes its
two team scores from `crates/server/src/game/host.rs`'s `team_scores`, axis
first. `crates/server/src/game/builtins/score.rs` carries `getteamscore` and
`setteamscore` including their two configstring writes. vcod does not
reproduce 6.4's dead functions.

---

## 7. There are no engine-side exit rules

### 7.1 No symbols, no strings

VERIFIED: `game.mp.i386.so` exports no `CheckExitRules`, `BeginIntermission`,
`FindIntermissionPoint`, `MoveClientToIntermission` or `LogExit`; the only
symbols whose names contain `intermission` or `exit` are `ExitLevel`,
`ClientIntermissionThink`, `IntermissionClientEndFrame`,
`SendScoreboardMessageToAllIntermissionClients` and `g_intermissionDelay`, and
three of those five are unreferenced (6.4).

VERIFIED: neither binary contains the string `timelimit`, `scorelimit` or
`fraglimit` anywhere.

VERIFIED: `g_intermissionDelay` (`0x18f020`) has exactly one relocation
pointing at it, at `0x7ded0`, which is inside `gameCvarTable`
(`0x7de28`, `0x6a8` bytes). INFERRED: the cvar is registered and never read.

### 7.2 What that leaves

INFERRED, from 7.1 and from section 2's single caller: the engine never
decides that a level is over. Every stock gametype's own script counts its
score and its clock and calls `exitLevel()` or `map_restart()`, and the C side
does exactly what section 1 and 2 describe and nothing more. A vcod server
that implements sections 1 to 6 and runs the stock gametype scripts therefore
gets the exit rules for free, and a vcod server that hard-codes a time or
score limit in Rust is adding a rule retail does not have.

### 7.3 `nextmap` is registered, set and never read

VERIFIED: the string `"nextmap"` (`0x80d54dd`) has exactly two references in
`cod_lnxded`: `Cvar_Get("nextmap", "", 0x100)` at `0x808abf3`, and
`Cvar_Set("nextmap", "map_restart")` inside `SV_SpawnServer` at `0x808a520`
(3 step 14). INFERRED, from both being writes: nothing in the engine ever
reads the cvar, the Q3 convention of the gametype writing `nextmap` and the
engine executing it does not exist in CoD 1.1, and `map_rotate` (section 5) is
the whole of the rotation.

### 7.4 As implemented

Nothing in vcod implements exit rules, by design: the gametype scripts the
gsc VM already runs are what call `exitLevel()` and `map_restart()`, and
`crates/server/src/game/builtins/cvar.rs` is where those two land. vcod does
not model `nextmap` at all, since retail never reads it either.

---

## 8. What vcod's own server does, measured the same way

`--save-mapchange` and `--save-roundrestart`, the two probe modes that took
the retail captures, point at any server, so both were run against
`cargo run -p vcod-server`. Every claim in this section is
evidence about vcod, not about retail; the retail column is the three
committed fixtures under `crates/server/tests/fixtures/netchan/`. Neither run
is committed: both modes write into the retail fixtures' own names, so each
run's files were moved out of the tree afterwards and the directory checked
out again.

Two runs, 2026-09-06, both on `mp_carentan`:

- `dm`, 190 s, one probe. `--set g_gametype=dm --set scr_dm_timelimit=1 --set
  'sv_mapRotation=gametype dm map mp_carentan map mp_brecourt'`, against
  `--net-probe --probe-team allies --save-mapchange`. It spans two map ends:
  the first rotation token is the map already serving, the second is
  `mp_brecourt`.
- `sd`, 200 s and 160 s, two probes on opposite teams. `--set g_gametype=sd
  --set scr_sd_roundlength=1 --set scr_friendlyfire=1`, against
  `--probe-team axis --probe-target --save-roundrestart` and `--probe-team
  allies --save-roundrestart`. It spans three round restarts. Neither probe
  reached a team on this run, for the reason 8.3 records, so what it measured
  is the restart the round clock drives and not the one an elimination does.

### 8.1 What matches

VERIFIED, the `dm` run against `mp_carentan-dm-mapchange.txt`: two gamestates
and one out-of-band line, in retail's order and with retail's numbers.
`serverId` 16 on the first, 33 on the second; `mapname` `mp_carentan` then
`mp_brecourt`; 246 configstrings then 217, both counts equal to retail's. The
rotation's first token names the map already serving and took the restart
path, moving the id to 17 in the low nibble, and only the second token spawned
a server and moved it to 33 in the high nibble, which is the arithmetic 3 step
16 and 4 step 5 describe and what 4.2's reading of `map <samemap>` predicts.

VERIFIED, the same pair: the intermission burst is `u`, then
`v g_scriptMainMenu "main"`, then `v cg_objectiveText`, all three in one
millisecond, followed by the first `pm_type` 5 frame in the same millisecond.
That frame reads `pm_type=5 eFlags=24 health=0 weapon=0`, an empty `ammoclip`,
`eventSequence=0` with an empty ring, and `origin` 384.0,-624.0,184.0, which
is field for field what retail's first intermission frame carries, the origin
included. Both of the run's map ends held it for ten seconds and change,
`dm.gsc`'s own `wait 10` between `endMap` and `exitLevel`.

VERIFIED, the `dm` run: the restart's reliable burst is `d 3`, `n`, `d 1`, in
that order and all in one millisecond, and the `d 1` systeminfo carries
`sv_serverid\17`, retail's own value at that point. It is followed by the
reopened team menu (`v g_scriptMainMenu "team_americangerman"`,
`v scr_showweapontab "0"`, `t 0`, `v cg_objectiveText`), then one `pm_type` 4
frame at the intermission origin, then the weapon menu
(`v scr_showweapontab "1"`, `v g_scriptMainMenu "weapon_american"`, `t 1`) and
the first live frame. Retail's burst is the same commands in the same order
with the two extra `d` lines 8.2 records.

VERIFIED, both boundaries of the `dm` run: the first live frame after each
reads `pm_type=0 eFlags=16 health=100 weapon=12` and
`ammoclip=3:7,6:3,10:15`, which is retail's frame and retail's spawn loadout,
and the frame before it is a single `pm_type=4 eFlags=24 health=0` at the
level's own spectator point, `384.0,-624.0,184.0` on `mp_carentan` and
`-2779.0,1585.0,576.0` on `mp_brecourt`, both of them the origins retail's
capture reads at the same two moments.

VERIFIED, the `dm` run: one `loadingnewmap\nmp_brecourt\ndm` out-of-band line
and nothing else outside the reliable stream, and no gamestate on the wire
until after it. INFERRED, from that against `spawn_server`, which holds no
gamestate send: the second gamestate went out in answer to the probe's next
message rather than being pushed, so vcod's map change is pull-shaped the way
3.1 reads retail's.

VERIFIED, both runs: every `b` scoreboard line in the two capture halves that
send `score` is an answer to one, and the halves that send none carry none.
Nothing pushed a scoreboard at the intermission, which is the drain 6.3
reads.

VERIFIED, the `sd` run against `mp_carentan-sd-roundrestart-*.txt`: three
round restarts, `serverId` 16 to 19 in the low nibble with no gamestate at any
of them, each one's reliable burst `d 3`, `n`, `d 1` in one millisecond
followed by the client cvars the gametype re-sends, and each burst about five
seconds after the announcer command that ended the round. The gamestate
carries 273 configstrings, retail's count on the same map and gametype.

VERIFIED, the `sd` run: the reopened menu after a restart carries no `t`,
where the `dm` run's does. INFERRED, from the two against 1's `savePersist`
gate: `sd.gsc` passes 1 and its `pers["team"]` survives, so its
`Callback_PlayerConnect` skips the `openMenu` that a `dm` client, whose
`pers` was freed, takes.

### 8.2 What differs

The first is recorded in 4.5 already and is repeated here only as the
capture's confirmation. Each of the rest was a defect in vcod that the run
found; every bullet says what the capture read, what caused it and whether it
has since been fixed. Three are still open: the two the first paragraphs of
this section record and the `loadingnewmap` timing at the end.

VERIFIED, the `dm` run: the restart burst carries no `d 13` and no `d 12`
where retail's carries both, and its `d 3` reads `t\0` where retail's reads
the incoming level's start time. 4.5 records this as CS 3's `t` and CS 13's
start time not being driven off the level clock, and the capture is what it
was predicted from.

VERIFIED, the `dm` run: no `f` server command at all, against five in retail's
capture (`MPSCRIPT_CONNECTED` at each of the three level starts and
`MPSCRIPT_TIME_LIMIT_REACHED` at each of the two map ends). It was
`GameHost::builtin` routing `iprintln` to the same handler as `println`, so
the builtin only wrote a log line. Fixed: `builtins::io::iprint_line` sends
`f "<message>"` to every client, or to the receiver alone when the call has
one, which is the split between `Scr_MakeGameMessage`'s two callers at
`0x5cd28` and `0x45594`.

VERIFIED, the `dm` run: the intermission's `v cg_objectiveText` value is
`MPSCRIPT_WINS` and the localized separator, where retail's is that plus the
winner's name and colour code. It was `set_client_cvar` reading `args[0]` and
`args[1]` and no further, so the substitution arguments were dropped. Fixed:
`builtins::message::construct` packs them the way
`Scr_ConstructMessageString` (`game.mp.i386.so` `0x594d0`) does, a player
entity through the `%s^7` at `0x767d9`.

VERIFIED, the `dm` run: the life that begins after the restart carried
`eFlags` 24, the same word the intermission before it carried, where retail's
carries 16 against the intermission's 24. It was `ClientSim::respawn` flipping
the teleport bit only for a player spawn while the spectator and intermission
wire word was a pinned 24. VERIFIED, from the three committed captures read
spawn for spawn: retail's word alternates on *every* spawn, the connect's
`spawnSpectator` and the intermission camera included, which is why the `sd`
target's post-death spectator frame reads 16 where its first one read 24 and
why two of its consecutive lives both read 24
(`mp_carentan-sd-roundrestart-target.txt`, `!trace ms=335`, `ms=25436`,
`ms=28607` and `ms=118532`). INFERRED, from that rule against the `dm`
capture's restart, whose spectator frame repeats the intermission's 24
instead of flipping off it: a level boundary clears the bit with the rest of
the playerstate, so the incoming level's first spawn is what sets it. Fixed
to both: every mode's spawn consumes a flip, the wire word is the bit rather
than a constant, and a restart lets `enter_world` build a sim with the bit
clear instead of carrying the outgoing one.

VERIFIED, the `dm` run: the intermission frames read `viewangles` 0 on all
three axes where retail's read a yaw of 90, `mp_carentan`'s own
`mp_deathmatch_intermission` heading, and the two agree on the origin.
INFERRED, from `ClientSim::to_wire` leaving `viewangles` unwritten and
`spawn_delta_angles` putting the spawn yaw in `delta_angles` instead, which is
`docs/protocol-1.1.md`, "View angles": the heading is on the wire in
the other field and a client that adds `delta_angles` back arrives at the same
90. Whether a retail client's intermission camera actually faces the same way
is not settled by a headless capture and is on the hand-check list.

VERIFIED, the `dm` run: the `loadingnewmap` line and the gamestate reached the
probe in the same millisecond, where retail left 1493 ms between them.
INFERRED, from `spawn_server` queueing the line into the outbox that
`main.rs`'s loop flushes after the tick, with the map load inside that same
tick: the line leaves vcod after the load rather than before it, so a client
gets no advance warning and spends none of the load showing a loading screen.
Cosmetic on a map that loads in a second; open.

VERIFIED, the `sd` run: the announcer command `s 4` went out with no `d 528`
naming the alias before it, where retail sends the `d 528` and then the `s 4`.
VERIFIED, from a debug probe against the same server: configstring 528 was
empty in vcod's gamestate. It was `send_configstring_update` having callers on
the restart path only, so nothing a script allocated mid-level reached a
client that already had its gamestate. Fixed:
`Server::broadcast_configstring_changes` diffs the table against the copy the
clients last heard after every script frame and sends `d <index> <text>` for
each slot that moved, which is the broadcast 3.1's compare pair lets through
while `sv.state` reads 2. It runs ahead of the frame's own client commands, so
the `d` naming an alias precedes the `s` that plays it, and both level
boundaries re-sync that copy rather than replaying a whole table slot by
slot.

VERIFIED, from a debug probe against a server started with
`--set g_gametype=sd`: configstring 0 read `g_gametype\dm` while the `sd`
scripts were the ones running, which is why the `sd` run's fixtures came out
named for `dm`. It was `Server::new` stamping the table out of the config
before `main.rs` replays the `--set` list. Fixed: `Server::set_cvar` rewrites
slot 0 from the serverinfo string when it mirrors a cvar that string carries,
the way 4.3's cvar flush follows a serverinfo write with
`SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`.

### 8.3 What the runs could not reach

VERIFIED, the `sd` run: both probes answered the team menu and neither ever
left it, both reading `team` 3 and `pm_type` 4 for the whole run, where the
same probe against retail had its weapon menu 50 ms after its team menu and
spawned. INFERRED, from `sd.gsc`'s `Callback_PlayerConnect`, whose `openMenu`
comes before the `spawnSpectator` that calls `updateTeamStatus`, and from that
function opening with `wait 0`: the `t 0` was on the wire while the connect
thread was still suspended, and an answer arriving inside that frame notified
an event no thread was parked on and was lost. INFERRED, from 4.4's
`VM_Call(gvm, 3, clientNum)` against vcod's own ordering: retail raises
`ClientBegin` inside `SV_ExecuteClientMessage`, before `G_RunFrame` advances
`level.time`, so the callback's `wait 0` is due in that same frame's thread
pass; vcod dispatched the event at the top of `G_RunFrame`'s counterpart
instead, which put the `waittill` one frame late. Fixed:
`ScriptRuntime::run_frame` opens with a packet pass that dispatches the
netcode's events on the previous frame's clock and then steps whatever is
`Runnable`: the callbacks and their notifies, plus any waiter a later thread
woke at the end of the previous frame's thread pass. The callback is therefore
parked on `menuresponse` before the `t 0` it queued can be answered, and the
`sd` gates now answer the menu the instant it opens instead of a frame later.
INFERRED, off that pass stepping a state rather than a list of woken threads:
the second case resumes on the previous frame's clock and ahead of the entity
think pass. Neither capture separates it from a thread the same frame's
callbacks woke, so its cadence against retail is unmeasured.

The `sd` half of the run is therefore still unmeasured against retail: the
elimination-driven restart, the team scores, the win announcements and the
weapons a client keeps across a restart need a fresh capture now that the
menu is answerable.

The `dm` rotation was two maps and never wrapped, so the wrap back to the
first entry is measured by the gate and not by a capture.

### 8.4 The gates

This section is a measurement rather than a specification, so it has no "as
implemented" of its own. What holds the two shapes to retail on every test run
is `crates/server/tests/mapchange_ab.rs` and
`crates/server/tests/roundrestart_ab.rs`, which replay the committed captures;
the `savePersist` gate is the three `probe_persist_*` cases in
`crates/server/tests/semantics_ents.rs`, and the rotation grammar and the
serverId nibbles are unit tests in `crates/server/src/console.rs`.
