# CoD 1.1 map cycle: intermission, `map_restart`, `map` and `map_rotate`

How a level ends and the next one starts: the two script builtins that ask
for it, `ExitLevel`, the engine's `SV_SpawnServer`, `SV_MapRestart_f` and
`SV_MapRotate_f`, the intermission arms in the game module, and what carries
across the boundary. Neither binary has engine-side exit rules: every "the
round is over" decision in CoD 1.1 MP is script.

No document-level default; the provenance is mixed and each claim names its
binary:

- `game.mp.i386.so`, the 1.1d Linux dedicated server's MP game module, with
  a full dynamic symbol table. Addresses are module-relative (image base 0),
  as in `cod11-gsc-object-model.md` and `cod11-combat.md`. It is
  position-independent, so plain `objdump -d` hides every call and cvar
  target; `python3 tools/re/annotate_func.py <elf> <symbol>` resolves them.
  `.rodata` runs at the same virtual address as its file offset, `.data`
  `0x1000` above it.
- `cod_lnxded`, the 1.1d Linux dedicated server's **engine**. A non-PIC
  `EXEC` whose first LOAD segment sits at `0x08048000` with file offset 0, so
  a virtual address is the file offset plus `0x8048000` and `objdump -d`
  reads straight. It has no symbol table, so **no engine name below is a
  symbol**. Three are established elsewhere and used unqualified:
  `SV_DropClient` (`0x8085cf4`), `SV_ClientEnterWorld` (`0x80877d8`) and
  `SV_ExecuteClientMessage` (`0x80872ec`), all in `docs/protocol-1.1.md`
  and `tools/re/net-notes.md`. Every other engine name is the Q3 or RTCW
  name for what the bytes do, a label rather than a recovered one:
  `SV_SpawnServer`, `SV_MapRestart_f`, `SV_MapRotate_f`, `SV_Map_f`,
  `NextToken`, `SV_RestartGameProgs`, `SV_InitGameProgs`,
  `SV_ShutdownGameProgs`, `SV_SetConfigstring`, `SV_SendServerCommand`,
  `SV_SendClientGameState`, `SV_GentityNum`, `SV_CreateBaseline`,
  `SV_Frame`, `NET_OutOfBandPrint`, `CM_LoadMap`, `Hunk_Clear`, `COM_Parse`,
  `Cbuf_ExecuteText`, `Cmd_AddCommand`, `Cmd_Argv`, `Cvar_Get`, `Cvar_Set`,
  `Cvar_VariableString`, `Q_strncmp`, `Cvar_VariableValue`,
  `Cvar_InfoString`, `Com_Printf`, `Com_DPrintf`, `Com_Error`,
  `Q_strncpyz`, `VM_Call`, `VM_Free` and `va`. UNVERIFIED: every name in that
  list. Each section rests on the cited address and what its instructions
  do. An engine name not in the list carries its own label where it
  appears, as `NET_Sleep` (3 step 4) and `FS_Restart` (3 step 9) do.

Seven lists below open with a pair of labelled sentences instead of a label
per item, the shape `cod11-combat.md` uses: the operands and addresses take
one label, the ordering and branch conditions the other. Such a pair covers
only the list items right after it; no prose paragraph takes its label from
a sentence above it.

Claims that restate an offset or table another document established cite
that document. Playerstate field offsets: `docs/protocol-1.1.md`,
"PlayerState delta". `gclient_t` offsets: `cod11-hud-protocol.md` and
`cod11-gsc-object-model.md`.

---

## 1. The two builtins: `map_restart` and `exitLevel`

VERIFIED: the `functions` builtin table (`0x7e508`, stride 12, reached from
`Scr_GetFunction` `0x5c15c`) holds `map_restart` at row 101 pointing at
`0x5f618` and `exitlevel` at row 102 pointing at `0x5f684`
(`tools/re/dump_builtins.py`). INFERRED, from the identifier case fold
`cod11-gsc-language.md` records: `exitLevel()` reaches the same row.

VERIFIED: the offsets, immediates, string addresses and call targets in the
list below, each read out of its instruction. INFERRED: the ordering, and
every "when" and "otherwise" (branch conditions).

- A latch word at `level+0x29f0` is read first (`0x5f619`, `0x5f685`).
- When it is non-zero the builtin calls `Scr_Error` with `0x78204`
  (`"map_restart already called"`) if the latch reads 1, and with `0x7821f`
  (`"exitlevel already called"`) otherwise (`0x5f627..0x5f63f`,
  `0x5f693..0x5f6ab`).
- The latch takes 1 for `map_restart` (`0x5f642`) and 2 for `exitlevel`
  (`0x5f6ae`).
- `level+0x1d50` takes 0 (`0x5f64c`, `0x5f6b8`), then `Scr_GetInt(0)` when
  `Scr_GetNumParam()` is non-zero (`0x5f656..0x5f669`,
  `0x5f6c2..0x5f6d5`). Both builtins take the same optional argument.
- `map_restart` ends with `trap_SendConsoleCommand(2, "map_restart\n")`,
  the string at `0x78238` (`0x5f671..0x5f67b`). `exitlevel` ends by calling
  `ExitLevel` (`0x5f6dd`, relocation target `0x510b4`).

INFERRED, off the shared latch: the pair is one-shot per level, and the error
names whichever of the two got there first, not the one refused.

VERIFIED: `level+0x1d50` is written by `vmMain` case 15 (`0x50f08`) and
returned by case 16 (`0x50f10`); the jump table at `0x75728` puts those two
bodies at indices 15 and 16. VERIFIED: the only other reads of the word in
the module are the two in `G_ShutdownGame` at `0x4ff68` and `0x4ff79`.

VERIFIED: at `0x4ff68` a zero `level+0x1d50` makes `G_ShutdownGame` call
`trap_FreeClientScriptPers`, and at `0x4ff79` the same test supplies
`Scr_FreeGameVariable`'s only argument as `savePersist == 0`. INFERRED, off
those two branches: the word is a "keep script-side persistence" flag, and
it keeps exactly the per-client script `pers` object and the `game`
variable.

VERIFIED: `ClientConnect` (`0x4246c`) opens with
`bzero(&level.clients[n], 0x22c4)` at `0x424a7`, and both the map-change and
the restart client passes call `ClientConnect` again (3 step 22, 4 step 10).
INFERRED, from those two: nothing on the C side of a `gclient_t` survives
either boundary, so the script-side `pers` is all that carries.

VERIFIED: `level` is zeroed for `0x2a14` bytes at the top of `G_InitGame`
(`0x4fc27`), and `0x29f0` is inside that range. INFERRED: the latch clears on
every level and every restart, so the guard is per level, not per process.

VERIFIED: `G_InitGame`'s fourth stack argument, `+0x14`, is the same
`savePersist` value, and the `test`/`jne` pair at `0x4fb87` targets
`0x4fc1a`, past both the `gameCvarTable` registration loop and the gametype
validity check. INFERRED, off that branch: a non-zero `savePersist` skips
both.

### 1.1 As implemented

- `crates/server/src/game/builtins/cvar.rs` carries both builtins. The latch
  is `level_latch` and the flag `save_persist`, both on the host struct
  (`crates/server/src/game/host.rs`). The console line each queues lands in
  the host's `console` vector, which `ScriptRuntime::take_console`
  (`crates/server/src/game/script.rs`) drains.
- `Vm::take_game` and `Vm::install_game` (`crates/gsc/src/vm/mod.rs`) and
  the host's `pers_carry` are the two halves of what `savePersist` keeps;
  `spawn_client` (`crates/server/src/game/entity.rs`) takes the carried
  `pers` back.
- vcod does not model `gameCvarTable`, so `G_InitGame`'s cvar-loop skip has
  no counterpart.

---

## 2. `ExitLevel`

VERIFIED: `ExitLevel` is `0x510b4`, `0xac` bytes, and the module's
relocation table holds exactly one call to it, at `0x5f6de` inside the
`exitlevel` builtin. INFERRED, from that single caller: only script ends a
level.

VERIFIED: the offsets, immediates, string addresses and call targets in the
list below. INFERRED: the ordering.

- `trap_SendConsoleCommand(2, "map_rotate\n")`, the string at `0x756b4`
  (`0x510bd..0x510c4`).
- `level+0x1fc` and `level+0x200` both take 0 (`0x510c9`, `0x510d3`); 6.3
  identifies them as the axis and allies team scores.
- A first pass over `level.clients` (pointer at `level+0`, stride `0x22c4`,
  `g_maxclients` at `level+0xc`) zeroes `client+0x20e0` for every client
  whose `client+0x20ec` reads 2 (`0x510dd..0x51118`). VERIFIED:
  `client+0x20e0` is the `score` script field and `client+0x20ec` the
  connection state whose 2 is fully connected (`cod11-hud-protocol.md`, the
  scoreboard row table).
- A second pass writes 1 into `client+0x20ec` for every client that reads 2
  (`0x5111a..0x5114d`). INFERRED: every connected client drops to the
  "connecting" state the scoreboard sends `-1` ping for, so the outgoing
  level stops counting them.
- `G_LogPrintf("ExitLevel: executed\n")`, the string at `0x756c0`
  (`0x5114f..0x51157`).

VERIFIED: `ExitLevel` writes nothing else: no intermission state on any
client, no camera choice, no write to `level+0x1d50` and no read of a timer.

### 2.1 As implemented

The host's `team_scores` (`crates/server/src/game/host.rs`) and each
client's score field are what the two passes clear. The queued `map_rotate`
goes through section 1's `console` vector, and `Server::drain_console`
(`crates/server/src/server.rs`) runs it.

---

## 3. `SV_SpawnServer`

Label for `cod_lnxded` `0x808a220`. VERIFIED: exactly two call sites,
`0x8083da4` inside `SV_Map_f` (5.1) and `0x8083ea4` inside
`SV_MapRestart_f`'s escalation arm (4.2).

VERIFIED: the offsets, immediates, string addresses and call targets in the
numbered list below, each read out of its instruction. INFERRED: the
ordering and every "when", "then" and "otherwise" (branch conditions).
Other sections cite these as "doc 3, step N".

1. `Cvar_Get("cl_xmodelcheck", "0", 0x21)`'s `integer` goes to `0x80c2560`
   (`0x808a232..0x808a247`). Strings at `0x80d5462` and `0x80d5460`.
2. When `Cvar_VariableValue("sv_running")` is non-zero (`0x808a252`,
   `"sv_running"` at `0x80d542e`), `savePersist = VM_Call(gvm, 16)`
   (`0x808a274..0x808a281`), section 1's word. When it is zero,
   `savePersist` is set to 0 (`0x808a333`) and steps 3 and 4 are skipped.
3. For every client whose state word reads above 2 (`CS_PRIMED` or
   `CS_ACTIVE`), `NET_OutOfBandPrint(NS_SERVER,
   client->netchan.remoteAddress, va("loadingnewmap\n%s\n%s", <map>,
   sv_gametype->string))` (`0x808a2c0..0x808a304`). Format string at
   `0x80d5471`, client stride `0x5a8fc`, state word at client offset 0, the
   20-byte `netadr_t` at client offset `0x528b8` pushed by value.
4. A 250 ms sleep, `0x80c786c(0xfa)` at `0x808a324`. UNVERIFIED: that
   `0x80c786c` is Q3's `NET_Sleep`. Verified are the constant and that
   nothing between it and step 3 touches the netchan.
5. `0x808147c()`, `0x8081494()`, then `SV_ShutdownGameProgs` (`0x80892f4`)
   at `0x808a35f`. VERIFIED: `SV_ShutdownGameProgs` calls
   `VM_Call(gvm, 1, 0)` (`G_ShutdownGame(restart = 0)`), frees the VM and
   clears the `gvm` pointer at `0x80e30c4` (`0x80892fa..0x808933f`).
   INFERRED: a map change unloads and reloads the game module; a restart (4)
   keeps it.
6. The banners `"------ Server Initialization ------\n"` (`0x80d54a0`) and
   `"Server: %s\n"` (`0x80d54c5`), then `Hunk_Clear` (`0x80682c8`) at
   `0x808a385`.
7. All 2048 configstring slots are freed (`0x808a390..0x808a3ae`), walking
   the pointer array at `0x8355678` with `i <= 0x7ff`. VERIFIED:
   `MAX_CONFIGSTRINGS` is 2048, matching `SV_SetConfigstring`'s bound check
   `index > 0x7ff` at `0x8089bfc`.
8. `memset(&sv, 0, 0x614ec)` at `0x808a3ba`, `sv` at `0x8355260`: `sv.state`
   is `0x8355260`, `sv.restarting` `0x8355264`, `sv.checksumFeed`
   `0x835526c`.
9. `sv.checksumFeed` takes `(rand() << 16) ^ rand() ^ 0x80c8520()` and goes
   to `0x80621e4` (`0x808a3e1..0x808a413`). INFERRED, off the call order:
   that is Q3's `FS_Restart(sv.checksumFeed)`.
10. All 2048 slots are pointed at the empty string `0x80d5243`
    (`0x808a422..0x808a440`).
11. `sv_running` is read again (`0x808a445`): zero takes `0x806dca0` and
    `0x8089df0`, non-zero takes `0x806e7e8` and, when
    `sv_maxclients->modified` is set, `0x8089f04` (`0x808a442..0x808a483`).
12. The snapshot entity and per-client snapshot rings are allocated from two
    sizes derived from `0x83b67b0` and `0x83b67b4` (`0x808a485..0x808a4c8`),
    and six counters at `0x83b67c8`, `0x83b67cc` and `0x83b67d8..0x83b67e4`
    are zeroed (`0x808a4aa..0x808a513`).
13. `svs.snapFlagServerBit ^= 4`, the byte at `0x83b67a8` (`0x808a516`).
    VERIFIED: `SV_MapRestart_f` toggles the same byte with the same mask
    (4 step 4). INFERRED: a map change and a restart both flip it.
14. `Cvar_Set("nextmap", "map_restart")` (`0x808a520`), names at
    `0x80d54dd` and `0x80d54d1`. See 7.3.
15. `0x808a12c(va("maps/mp/%s.bsp", <map>))` at `0x808a544`,
    `Cvar_Set("cl_paused", "0")` at `0x808a559`, then
    `CM_LoadMap(va("maps/mp/%s.bsp", <map>), 0, &checksum)` at `0x808a57f`
    and `Cvar_Set("mapname", <map>)` at `0x808a593`. Bsp path format
    `0x80d54e5`, `"cl_paused"` `0x80d54f4`, `"mapname"` `0x80d54fe`.
16. The serverId, one byte at `0x80e30c0`, takes `+0x10` truncated to a byte
    (`movzbl`), then a second `+0x10` when the high nibble came out zero
    (`0x808a598..0x808a5b7`). INFERRED, off that second add: the high nibble
    cycles 1 to 15 and never reads 0 after the first load.
    `Cvar_Set("sv_serverid", va("%i", id))` follows at `0x808a5d8`, names at
    `0x80d5506` and `0x80d53d0`.
17. `0x8355268` takes `0x833df1c` (`0x808a5e2`), `sv.state` takes 1
    (`0x808a5e7`), `Cvar_Set("sv_serverRestarting", "1")` (`0x808a601`),
    name at `0x80d5512`.
18. `0x80699d0(va("maps/mp/%s.bsp", <map>), 2)` at `0x808a620`, then
    `0x80bfea0(1)` at `0x808a62d`. UNVERIFIED: what either does.
    `0x80bfea0` is a two-instruction setter that writes its argument into
    `0x83179e0`.
19. `SV_InitGameProgs(savePersist)` at `0x808a63c`, label for `0x8089124`.
    VERIFIED: it creates the VM and calls
    `VM_Call(gvm, 0, svs.time, <seed>, 0, savePersist)`, i.e.
    `G_InitGame(levelTime, seed, restart = 0, savePersist)`
    (`0x8089196..0x80891ab`). INFERRED, off that argument order against step
    2: the flag read from the *outgoing* level is what the *incoming* level's
    `G_InitGame` receives.
20. Three settle frames: `svs.time` (`0x83b67a4`) gains 100 and
    `0x808d3d4()` runs, three times (`0x808a644..0x808a65d`).
21. The baseline pass, Q3's `SV_CreateBaseline` inlined
    (`0x808a669..0x808a734`). It runs `i` from 1 to the entity count at
    `0x83b6684`, skips any entity whose `+0xf0` is zero, writes `i` into the
    entity's `s.number`, and copies 240 bytes of entity state plus eight
    words from `+0xf4`, `+0xf8` and the six floats at `+0x11c..+0x130` into
    a `0x17c`-byte baseline record. INFERRED: `+0xf0` is `r.linked`, since
    nothing else in the record gates the copy, and entity 0 is never
    baselined because the loop starts at 1.
22. The client pass (`0x808a73a..0x808a7a3`): for every client whose state
    word reads above 1, `VM_Call(gvm, 2, i, client[+0x5a8f0])`, which is
    `ClientConnect`. A non-null return is a denial string and the client is
    dropped through `SV_DropClient` (`0x8085cf4`); otherwise its state word
    becomes 2, `CS_CONNECTED`.
23. The pure-server pak cvars `sv_paks`, `sv_pakNames`, `sv_referencedPaks`
    and `sv_referencedPakNames` (`0x808a7a5..0x808a891`, names at
    `0x80d5526`, `0x80d556e`, `0x80d557a`, `0x80d558c`). What each holds:
    `cod11-server-handshake.md`, "Pak checksums and pure servers".
24. `SV_SetConfigstring(1, Cvar_InfoString(CVAR_SYSTEMINFO))` at `0x808a8c9`
    and `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))` at
    `0x808a8e1`, each clearing its own bit of the modified-flags word at
    `0x834a0e8` (`0x808a8b6`, `0x808a8e6`), then the `0x800`-flagged cvar
    range through `0x806fbec(0x8c, 0x40, 0x800)` at `0x808a8ff` (3.2).
25. `sv.state` takes 2 (`0x808a90b`), the heartbeat `0x8084bd0()` runs
    (`0x808a915`), `Cvar_Set("sv_serverRestarting", "0")` (`0x808a927`) and
    the closing banner `0x80d55c0` prints.

### 3.1 No gamestate is pushed

VERIFIED: `SV_SpawnServer` (`0x808a220` to `0x808a948`) contains no call to
`SV_SendClientGameState` (`0x8085eec`). The module has exactly three calls
to it, at `0x8087447`, `0x80876b1` and `0x8087a1e`, all reached from
`SV_ExecuteClientMessage` (`0x80872ec`) or the `donedl` handler.

VERIFIED: `SV_SpawnServer` writes configstrings 1 and 0 at `0x808a8c9` and
`0x808a8e1`, its last write of `sv.state` ahead of both is step 17's 1, and
it never writes `sv.restarting`. VERIFIED: `SV_SetConfigstring`
(`0x8089bf0`) holds a `cmp sv.state, 2` at `0x8089c6c` and a
`cmp sv.restarting, 0` at `0x8089c75` between its store of the new string
and its per-client loop.

INFERRED, off those two compares: the loop runs only when `sv.state` reads 2
or `sv.restarting` is set, so neither of `SV_SpawnServer`'s configstring
writes reaches a client. INFERRED, from that and the negative above: a map
change sends nothing on the reliable stream. Every client at `CS_PRIMED` or
above gets step 3's out-of-band `loadingnewmap`, and the gamestate arrives
only once that client's next message reaches the new server with the old
serverId and `SV_ExecuteClientMessage` resends it (4.4). The map change is
pull-shaped.

On the client that gamestate frees the cgame and loads a fresh one once the
downloads are done (`CL_DownloadsComplete`, CoDMP.exe `0x40ffb0`), chat ring
and all: `cod11-front-end.md` section 17, "A map change reloads the cgame".

### 3.2 The `0x800` cvar range is configstrings 140..203 and 204..267

VERIFIED: `0x806fbec`, step 24's call, walks the cvar list headed at
`0x834a0e0` (next pointer at `+0x24`), with a `test cvar->flags, 0x800` at
`0x806fc05` and a call to
`0x808b148(<start>, <count>, cvar->name, cvar->string)` at `0x806fc16`.
VERIFIED: `0x808b148` loops over configstrings `<start> + i` bounded by
`i < <count>`, with a `cmp` of the slot's first byte against 0 at
`0x808b172`, a `SV_SetConfigstring(<start> + i, name)` at `0x808b17c`, a
`strcasecmp` of the slot against `name` at `0x808b18b`, a `Com_Error` with
`"SV_SetConfigValueForKey: overflow"` (`0x80d52e0`) at `0x808b1a6` and a
`SV_SetConfigstring(<start> + <count> + i, value)` at `0x808b1be`.

INFERRED, off those two compares and the error's address: a `0x800`-flagged
cvar takes the first slot that is empty or already carries its name, its
name goes in that slot and its value `<count>` slots higher, and a full
table errors.

VERIFIED: step 24 passes `<start>` `0x8c` (140) and `<count>` `0x40` (64).
INFERRED, against the ranges `cod11-hud-protocol.md` and
`cod11-server-handshake.md` record: this is the writer for the 140..203 cvar
names and 204..267 values that neither document found in the game module.
It is in the engine.

### 3.3 As implemented

- `Server::spawn_server` (`crates/server/src/server.rs`) carries the list:
  `snap_flag_server_bit` is step 13, the `loadingnewmap` out-of-band step 3,
  and `Client::reset_for_level` (`crates/server/src/client.rs`) is step 22's
  effect on each surviving client.
- 3.1's negative is kept by the caller set, not a gate inside one function:
  `send_configstring_update` sends whatever it is handed,
  `broadcast_configstring_changes` is the per-frame half and skips a client
  with no gamestate to patch yet, and the spawn re-syncs the copy that half
  diffs against after its settle frames. The map path puts no `d` on the
  wire.
- `crates/server/src/console.rs` owns step 16's serverId nibble arithmetic
  (`next_map_id`), shared with 4 step 5 (`next_restart_id`).
- Step 21 runs on every load, the first map's included
  (`Server::load_scripts` runs the three settle frames and the baseline
  pass as `spawn_server` does). `rebuild_baselines` writes what
  `crate::game::wire::baseline_entities` builds after the settle frames,
  plus `--test-entities`: the snapshot set plus the linked entities no
  snapshot carries:
  - Every trigger. Its spawn sets `SVF_NOCLIENT` (`r.svFlags` 1 at `+0xf4`)
    and the brush model's `s.index`, and `SV_LinkEntity` packs `solid`
    0xffffff for a brush model. VERIFIED: `InitTrigger` (0x65784),
    `SP_trigger_multiple` (0x64c50), `SP_trigger_hurt` (0x64ef8) and
    `SP_trigger_lookat` (0x65df0) store 1 at `+0xf4` and OR 2 into
    `s.eFlags` (`+0x8`). `SP_trigger_use` (0x5742c) stores 1 at `+0xf4` and
    writes no `eFlags`. Its cursor hint (2 at `+0xdc`) and hint string (0xff
    at `+0xd8`) are inside the entity state, as `dmgFlags` and
    `_union.scale`.
  - Every `script_origin`. VERIFIED: `InitScriptMover` (0x60214) sets
    `s.eType` 8 and `r.svFlags` 0x80, and `SP_script_origin` (0x61034)
    links it with contents 0. INFERRED: with contents 0 it is in no
    area-tree node and has no clusters, so no snapshot carries it.
- `pos.trBase` is the entity's origin and `apos.trBase` its angles; a
  trigger with no `origin` key reads zero.

VERIFIED, against three retail gamestates (`tools/run_server.sh
mp_carentan`, then with `+set g_gametype sd` and `re`): vcod's baselines
match field for field: 19 on dm, 26 on sd (the bomb triggers, the
`trigger_lookat` and both `script_brushmodel`s) and 22 on re (two
`trigger_use` with hint strings 2 and 3, read back by `setHintString`). The
configstrings match too, except 0 and 1, which differ by `sv_hostname` and
the pak lists (from the paks the server's homepath holds). With those two
copied across, the dm gamestate is the same plain message, 7116 bytes.
Without the unsent baselines vcod's dm gamestate was about 400 bytes
smaller: 326 of them the pak lists (environment), 77 the seven missing
baselines. Test: `the_first_maps_baselines_match_retail`, against the
committed fixture, every key and field.

---

## 4. `SV_MapRestart_f`

Label for `cod_lnxded` `0x8083de4`. VERIFIED: registered as the console
command `map_restart` by `Cmd_AddCommand("map_restart", 0x8083de4)` at
`0x8084b0a`, name string at `0x80d3fe7`. VERIFIED: also called directly from
`SV_Map_f` (`0x8083d96`) and from `SV_MapRotate_f`'s three failure arms,
which share one call at `0x808422d`.

VERIFIED: the offsets, immediates, string addresses and call targets in the
numbered list. INFERRED: the ordering and every branch condition.

1. When `0x8355268` already equals `0x833df1c` the function returns
   (`0x8083ded`). VERIFIED: the restarting path writes `0x833df1c` into
   `0x8355268` at `0x8083eef`, and `SV_SpawnServer` writes the same pair at
   `0x808a5e2`. INFERRED: a second restart within one engine frame is a
   no-op, and so is a restart right after a spawn.
2. When `0x833efc0`'s `integer` is zero it prints
   `"Server is not running.\n"` (`0x80d392e`) and returns
   (`0x8083dfe..0x8083e16`).
3. `savePersist = VM_Call(gvm, 16)` (`0x8083e20..0x8083e30`), section 1's
   getter. When it is zero, two escalation checks run: a `g_gametype` cvar
   (pointer at `0x83b6778`) whose `latchedString` differs from its `string`
   prints `"g_gametype variable change -- restarting.\n"` (`0x80d3960`), and
   a `sv_maxclients` cvar (pointer at `0x8355250`) with `modified` set
   prints `"sv_maxclients variable change -- restarting.\n"` (`0x80d39a0`).
   Either then does `Q_strncpyz(buf, Cvar_VariableString("mapname"), 0x40)`
   and calls `SV_SpawnServer(buf)` (`0x8083e7e..0x8083ea4`). When
   `savePersist` is non-zero both checks are jumped over (`0x8083e35`).
   INFERRED, off that jump: a script that keeps its persistence gets an
   in-place restart even across a gametype or maxclients change; one that
   does not is escalated to a full spawn.
4. `0x808b2c8()` zeroes six words at `0x83b67c8`, `0x83b67cc` and
   `0x83b67d8..0x83b67e4` (`0x8083eb0`), then `svs.snapFlagServerBit ^= 4`,
   the byte at `0x83b67a8` (`0x8083eba`).
5. The serverId byte at `0x80e30c0` takes
   `(id & 0xf0) + ((id + 1) & 0xf)` (`0x8083eb5..0x8083ece`), and
   `Cvar_Set("sv_serverid", va("%i", id))` follows
   (`0x8083ed3..0x8083eea`). INFERRED, against 3 step 16: a restart moves
   only the low nibble and a map change only the high one.
6. `sv.state` and `sv.restarting` both take 1 (`0x8083ef9`, `0x8083f03`),
   then `Cvar_Set("sv_serverRestarting", "1")` (`0x8083f0d..0x8083f1d`) and
   `0x80bfea0(1)` (`0x8083f27`).
7. `SV_RestartGameProgs(savePersist)`, label for `0x8089350`, at
   `0x8083f33`.
8. Three settle frames, as 3 step 20: `svs.time += 100` then `0x808d3d4()`,
   three times (`0x8083f3b..0x8083f4d`).
9. For every client whose state word reads above 1,
   `SV_SendServerCommand(client, 1, "n")` (`0x8083f6e..0x8083f79`, the
   letter at `0x80d39f1`).
10. Then, for the same client, `VM_Call(gvm, 2, i, client[+0x5a8f0])`, i.e.
    `ClientConnect` (`0x8083f7e..0x8083f8f`). A non-null return drops the
    client through `SV_DropClient` and prints
    `"SV_MapRestart_f: dropped client %i - denied!\n"` (`0x80d3a00`).
11. Otherwise, only when the client's state word reads exactly 4
    (`CS_ACTIVE`), `SV_ClientEnterWorld(client, &client->lastUsercmd)`
    (`0x80877d8`) at `0x8083fc8`, the usercmd at client offset `0x10624`.
    INFERRED, off that compare: a `CS_PRIMED` client is not re-entered here;
    its own next message promotes it (4.4).
12. `sv.state` takes 2 and `sv.restarting` 0 (`0x8083fe1`, `0x8083feb`),
    then `Cvar_Set("sv_serverRestarting", "0")` (`0x8083ff5`).

### 4.1 `SV_RestartGameProgs`

Label for `0x8089350`. VERIFIED: it holds two `VM_Call`s,
`VM_Call(gvm, 1, 1)` at `0x808935d..0x8089367` (`G_ShutdownGame(restart =
1)`) and `VM_Call(gvm, 0, svs.time, <seed>, 1, savePersist)` at
`0x80893ad..0x80893c4` (`G_InitGame(levelTime, seed, restart = 1,
savePersist)`). INFERRED, off their addresses: the shutdown runs first.
VERIFIED: it holds no call to `VM_Free`, where `SV_ShutdownGameProgs` does
(3 step 5). VERIFIED: it also clears `client[+0x10a40]` for every client
(`0x80893d1..0x80893fa`).

VERIFIED: `G_InitGame`'s third argument, `+0x10`, is the `restart` flag, and
its only use in the function is to skip `ClearRegisteredItems`
(`0x4fe0c..0x4fe12`).

### 4.2 The escalation, and why `map <samemap>` is a restart

VERIFIED: `SV_Map_f` (`0x8083c68`, 5.3) holds a call to `SV_MapRestart_f` at
`0x8083d96`, one to `SV_SpawnServer` at `0x8083da4`, a
`cmp com_sv_running->integer, 0` at `0x8083d77` and a `strcasecmp` of the
requested name against the `mapname` cvar's string at `0x8083d8a`; both
compares carry a jump to `0x8083da0`, the instruction ahead of the
`SV_SpawnServer` call. INFERRED, off those compares and jumps:
`map <currentmap>` on a running server is a restart and anything else a full
spawn.

### 4.3 Where the new systeminfo comes from

VERIFIED: `SV_Frame`'s cvar flush at `0x808d090..0x808d0dd` holds a `test`
of bit 2 of the modified word at `0x834a0e8` at `0x808d090` over a
`SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))` at `0x808d0a9` and
an `and` clearing that bit at `0x808d0ae`, and a `test` of bit 3 at
`0x808d0b8` over a `SV_SetConfigstring(1, Cvar_InfoString(CVAR_SYSTEMINFO))`
at `0x808d0d1` and an `and` clearing it at `0x808d0d6`. INFERRED, off those
two tests: bit 2 pushes configstring 0 and bit 3 configstring 1, once per
frame in which the bit was set.

VERIFIED: `SV_SetConfigstring`'s broadcast gate is 3.1's compare pair.
INFERRED, off step 5's `Cvar_Set` and that gate: the `d 1 ...` a client sees
after `n` is this flush carrying the bumped `sv_serverid`, the mechanism
behind `docs/protocol-1.1.md`, "map_restart and sv_serverid".

### 4.4 `SV_ExecuteClientMessage`'s two nibble branches

Label for `cod_lnxded` `0x80872ec`.

VERIFIED: it holds a `cmp` of the client's stored serverId (client offset
`0x5a8f8`) against the `sv_serverid` byte at `0x80e30c0` at `0x808733f`
whose `je` targets `0x8087451`, and a `cmp client+0x10a64, 0` at
`0x808734a` whose `jne` targets the same `0x8087451`. VERIFIED:
`client+0x10a64` is the `downloadName` byte, the offset
`docs/protocol-1.1.md` uses for the download state. INFERRED, off the two
compares and their shared target: a matching serverId takes the normal path,
and so does a client with a non-empty `downloadName` whatever its serverId.

VERIFIED: two `and`s masking both ids with `0xf0` sit at `0x8087357` and
`0x808735d`, and the `cmp` of the results at `0x8087362` carries a `jne` to
`0x8087416`. VERIFIED: at `0x8087416` a `cmp` of the client's
`messageAcknowledge` (offset `0x10618`) against its `gamestateMessageNum`
(offset `0x1061c`) at `0x808741f` carries a `jle` to `0x808752c`, the
function's exit; past it sit a `Com_DPrintf` with
`"%s : dropped gamestate, resending\n"` (`0x80d4b00`) at `0x808743b` and a
call to `SV_SendClientGameState` (`0x8085eec`) at `0x8087447`. INFERRED, off
the first compare: differing high nibbles are the map-change case. INFERRED,
off the second and its `jle`: the gamestate is resent only to a client that
has acknowledged a message past the one its last gamestate went in; every
other client returns without reading an op.

VERIFIED: the fall-through from the `cmp` at `0x8087362` reaches a
`cmp client+0, 3` at `0x808736a` whose `jne` targets `0x808752c`. VERIFIED:
past it sit `0x80bfea0(1)` at `0x8087378`, a `Com_DPrintf` with
`"Going from CS_PRIMED to CS_ACTIVE for %s\n"` (`0x80d46a0`) at
`0x8087394`, a store of 4 into `client+0` at `0x808739f`, a call to
`SV_GentityNum(clientNum)` at `0x80873b8` whose result goes into
`client+0x10a40` at `0x80873c2`, a store of -1 into `deltaMessage` (offset
`0x10b04`) at `0x80873c8`, a store of `svs.time` into offset `0x10b14` at
`0x80873dd` and a `VM_Call(gvm, 3, clientNum)` at `0x808740c`. VERIFIED: 3 is
`ClientBegin` in section 1's `vmMain` jump table, which puts that case at
`0x50e70`. INFERRED, off the `cmp client+0, 3` and its `jne`: matching high
nibbles with a differing low nibble promote a `CS_PRIMED` client and drop the
message unread for every other state.

### 4.5 As implemented

- `Server::map_restart` (`crates/server/src/server.rs`) carries the numbered
  list, sharing `console.rs`'s nibble arithmetic with `spawn_server` for
  step 5. `ScriptRuntime::load`/`load_from` take the carried `game` and
  `pers` tables that 4.1's two `VM_Call`s stand for, and
  `ScriptRuntime::reconnect_client` is step 10.
- vcod has no VM to free, so 3 step 5's unload against 4.1's reuse shows
  only as whether those tables are carried. The `restart` flag on
  `Server::load_scripts_with` decides that the animtree, the weapon files
  and the hit-location table are not re-read, since a restart changes
  neither the paks nor the map.
- Retail's restart has no baseline pass, and vcod's has none either.
- `EF_TELEPORT_BIT` does not cross `Client::reset_for_restart`: step 11
  rebuilds `ClientSim` (`crates/server/src/spectate.rs`) through
  `enter_world`, the new one starts with the bit clear, and the incoming
  level's first spawn flips it to 24. VERIFIED, the retail round-restart
  capture: the shooter reads `eFlags` 16 on the life before the restart and
  24 on the one after
  (`crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-shooter.txt`,
  `!trace ms=697` and `ms=5765`), and the target the same. INFERRED, from
  that pair against the same file's `ms=28607` and `ms=118532` (two
  consecutive lives both at 24): the restart did not continue an alternation
  the outgoing life carried, since a plain respawn does not always change
  the word; 8.2 reads the rule off all three captures. Leave the bit pinned
  and a retail client interpolates a respawning player from its old spot to
  its new spawn.
- Not modelled: step 6's `sv.state`, `sv.restarting` and the
  `sv_serverRestarting` cvar (what `sv.restarting` gates on retail,
  `SV_SetConfigstring`'s broadcast, vcod does unconditionally), and step
  10's drop on a `ClientConnect` denial string, since `reconnect_client`
  has no denial to return.

The reliable stream carries the incoming level's configstring writes ahead
of `n` (1.5's and UO's `B`) and `d 1`. VERIFIED, the retail capture
(`crates/server/tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`, seq 40
to 44): the burst is `d 13 70050`, `d 12 0 16500 1 0.7 0.85 1 0`,
`d 3 n\ambient_mp_carentan\t\70050`, `n`, `d 1`, all at `ms=51016`. A team
score is another slot that moves: `ExitLevel` zeroes the score in the game
but not configstring 5 or 6, and the next level's `setTeamScore(..., 0)`
changes it, which retail UO sends as `d 6 0` ahead of its `B`
(`coduo-diff.md` 22, "Round and map ends"; VERIFIED, the captures).

VERIFIED, `game.mp.i386.so`, where those three come from: `SP_worldspawn`
(0x61cec) writes configstring 2 (`"cod"`, 0x61d3e), 13 as
`va("%i", level.startTime)` (`level+0x1f4`, 0x61d43..0x61d5c), 3 as
`va("n\\%s", ambienttrack)` or `""` when the worldspawn has no
`ambienttrack` key (0x61d72..0x61daa), 4, 14 (`g_motd`) and 11 (`northyaw`),
in that order. `G_InitGame` stores `levelTime` in both `level.time` and
`level.startTime` (0x4fc6d, 0x4fc72) and, after the spawn and
`SaveRegisteredItems`, calls `G_setfog("0")` (0x4fe2e..0x4fe33), which is
`trap_SetConfigstring(12, "0")` (`G_setfog` 0x48fa4). The map's own
`setCullFog` and `ambientPlay` (`mp_carentan.gsc` calls them in that order)
write 12 and 3 again, `ambientPlay`'s `t` being `level.time` plus the fade
(`GScr_AmbientPlay` 0x5adc4, `n\\%s\\t\\%i` at 0x77865). VERIFIED, the 1.5b
and UO 1.51b `SP_worldspawn`s (0x66960..0x66aca, 0x72b5b..0x72cc5) write the
same order with UO's start time at 14 and its motd at 15, and 1.5b's
`G_InitGame` calls `G_setfog` at 0x4f4fd. INFERRED, from the capture against
those writes and the queue's squash (`docs/protocol-1.1.md`, "The server
command queue"): `sv.restarting` lets every write that changes its slot
through; the fog cleared to `"0"` and set back to the same text still
queues a `d 12`, and the squash keeps one `d` per slot at its last write,
which orders the burst 13, 12, 3.

As implemented: the game's table is a `ConfigStrings`
(`crates/server/src/configstrings.rs`) whose only write is `set`, shaped as
`SV_SetConfigstring`: a write that leaves its slot unchanged is dropped, one
that changes it is logged in order. `SP_worldspawn`'s writes (the start
time, 3, 4 from the worldspawn's `message` key, `g_motd` at the start time's
next slot, 11) and `G_InitGame`'s fog go through it like every builtin's.
`map_restart` replays that log, from the load through the settle frames,
against what the clients hold, then any slot the engine itself left
different (the cvar mirror) in slot order, through `send_server_command`,
whose squash is retail's. The frame's diff orders its slots by their last
write in the log too, and drains it. `tests/mapchange_ab.rs` compares the
burst slot for slot (vcod sends `d 13 70050` and an ambient `t` of 70050);
`a_restart_s_burst_keeps_the_write_order` pins a level that writes 6 ahead
of 5.

VERIFIED, `game.mp.i386.so`: 4 is `G_SpawnString("message", "")` (0x61dc0,
the key at 0x79147) written as is (0x61dcc), and the motd is `g_motd`'s
string (`vmCvar_t` + 0x10, 0x61dd9) into 14 (0x61dde). 1.5b writes the same
two at 0x66a24 and 0x66a3d (14), its key `message` at 0x7e60b; UO at
0x72c1f and 0x72c38 (15, past its start time at 14), the key at 0xa51eb. No
stock worldspawn has a `message` key (1.1, 1.5 or UO paks) and `g_motd`
defaults to `""`, so both slots read empty on retail and vcod alike.

VERIFIED live on 1.1d, 1.5b and UO 1.51b
(`crates/server/tests/fixtures/configstrings/*-motd*.txt`, recipe in their
headers): a homepath `zzz_` pak holding the stock map with
`"message" "vcod worldspawn message"` added to its worldspawn
(`tools/bsp_worldspawn_key.py`), and `+set g_motd "Welcome to vcod"`, read
back 4 as the key's value and 14 (UO 15) as the motd at the gamestate. The
same runs set `sv_hostname` and `sv_privateClients` (both in 0) and
`scr_motd` (1.1's mirror value at 244, 1.5's and UO's `ui_motd` at 250 and
353). Each `configstrings_*_ab` gate has a row for its version.

VERIFIED live on 1.1d, same run (`tools/run_server.sh` puts every `+set`
after `+map`): 14 held the motd, and `scr_allow_vote 0` read back 0 over
rcon and in the mirror (242) with `g_allowVote` at 1, where `dm.gsc`'s
load-time `setCvar("scr_allow_vote", level.allowvote)` writes 1. VERIFIED,
`cod_lnxded` 1.1d `Com_Init` (0x806c654): the command line is split on `+`
into console lines (0x806c6e3..0x806c71d), then `Com_StartupVariable(NULL)`
(0x806bb0c; every line whose first token is `set` is `Cvar_Set` and flagged
`0x80`) runs ahead of the four `exec`s (0x806c738) and again after
`Cbuf_Execute` has run them (0x806c8da), so a `+set` outranks
`default_mp.cfg` and the configs. At the end every non-empty line, `set`
lines included, is `Cbuf_AddText` with a `"\n"` in command-line order
(0x806cd80..0x806cdcb; the `Q_stricmpn(line, "set", 3)` at 0x806cd9a feeds
only the "added a command" flag), so a `+set` after `+map` runs again once
`map` has loaded the level. VERIFIED: `Com_StartupVariable`'s other callers
are `FS_InitFilesystem` (0x80624be..0x8062522, the `fs_*` names) and the
journal (0x806bcbe); nothing under `SV_SpawnServer` or `SV_MapRestart_f`
calls it, so a command-line set applies at startup only and a later load
leaves the table as the outgoing level left it. VERIFIED, the same shape on
1.5b (`Com_StartupVariable` 0x8070184, called from `Com_Init` at 0x8070f86,
0x8070f92 and 0x8071063 besides the filesystem and journal; the
startup-command loop's `"set"` compare at 0x807029d) and UO 1.51b
(0x80707bc; 0x80715dc, 0x80715e8, 0x80716b9; the loop at 0x80708d5).
INFERRED, from the startup order and the live run: `SP_worldspawn` saw
`g_motd` through the first pass, and the replay after `+map` overwrote
`dm.gsc`'s 1.

As implemented:

- `Server::set_cvar` before the first load records a `+set` that the first
  level's table takes over `default_mp.cfg` (`Server::cvars`); after it,
  it is a plain `Cvar_Set` on the running (or carried) table. Later loads
  never replay it, so a script's `setCvar` of the same cvar stands.
  `g_gametype` and `sv_maxclients` are latched and wait for the next load.
- A `vcod-server --set` after the map argument is also queued as a console
  `set` behind the load (`Server::push_set`), retail's `+map x +set ...`;
  one before it is the `+set ... +map x` case. The configstring gates' motd
  rows push their sets after the load, as `run_server.sh` placed them.
  `server::tests::a_set_applies_once_and_a_scripts_write_outlives_later_loads`
  and `a_set_after_the_map_outranks_the_scripts_load_time_write` pin both.
- `sv_hostname` follows the same rule: the config's (`--hostname`) goes
  into the first level's table only, so a script's or an rcon `set` of it
  survives later loads (`a_scripts_hostname_outlives_later_loads`).

VERIFIED live on 1.1d (`probe_hostname`, its section in
`retail-captures.txt`, recipe in the probe's header; `semantics_ents.rs`
`probe_hostname_matches_retail` replays it on vcod): each load's `main()`
appends an `x` to `sv_hostname` and reads every earlier one back, across
`map_restart(false)` and an `exitLevel(false)` rotation from mp_pavlov to
mp_brecourt, with the hostname from the homepath's config and with `+set
sv_hostname probehost` ahead of `+map`. A `getstatus` after the map change
read `sv_hostname` `probehostxxx`, all three writes. With the `+set` after
`+map`, as `run_probe.sh` passes it, the second pass read none: the `set`
ran once more after the first load and overwrote the first pass's write
(the `Com_AddStartupCommands` rule above, not a load re-stamping it). A map
change writes configstring 0 again after the scripts and the settle frames
(step 24), so a serverinfo cvar the level's init set is in the gamestate.

The cvar table is carried from the outgoing level, not rebuilt from
`default_mp.cfg`, on this path and on the map change alike, because
retail's `Cvar_Set` writes a process-global table that neither boundary
clears. A `+set` goes in at the first load only, so a script's `setCvar` of
it outlives both boundaries, as on retail.

### 4.6 What the engine keeps across a restart

VERIFIED, the retail round-restart captures
(`crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-*.txt`
and a second pair): the configstring table survives a
`map_restart`. The move-in aliases `german_move_in` and `american_move_in`
are allocated and broadcast once, as `d 528` and `d 529` 1.7 s after the
first restart, and never again across the three restarts that follow. The
kill after a restart drops a `kar98k_mp` and the wire carries no `d 8`, no
new model slot and no new localized-string slot for it, where
`crates/server/tests/fixtures/configstrings/mp_carentan-sd.txt` places that
weapon's world model at slot 340 and `SD_AXISHAVEBEENELIMINATED` at 1256 at
load. VERIFIED, `maps/mp/gametypes/sd.gsc` (pak5): `_teams::precache` and
the `precacheString` list run only while `game["gamestarted"]` is unset, and
`endRound` restarts with `map_restart(true)`, which keeps `game[]` (section
1). INFERRED, from the two together: the incoming level registers none of
those, so the slots it finds on the first kill are the outgoing level's,
which needs both the table and the item registry to outlive the restart.
That is the Q3 lineage's shape too: `SV_SpawnServer` clears
`sv.configstrings` and `SV_MapRestart_f` does not.

As implemented: `map_restart` overlays the static slots (0, 1 and the
constants) onto the outgoing table instead of replacing it, the host's range
allocators are seeded from the table it is handed (`Allocators::seeded`),
and `Carry` carries the `Items` registry across a restart whether or not
the level asked to persist (engine state, not script state).
`map_restart(false)` frees `game[]` and the script precaches again, into the
same slots. `tests/roundrestart_ab.rs`'s slot-set comparison holds it. Trap:
without the overlay vcod re-sent the move-in aliases after every restart and
wrote `d 8`, `d 384` and `d 1245` on the first kill after one.

### 4.7 The first snapshot after a restart

The frame loop sets it, not `SV_MapRestart_f`.

VERIFIED: `SV_MapRestart_f`'s calls (`0x8083de4..0x8084014`) include no call
to `SV_SendClientMessages` (`0x809045c`), so step 8's three settle frames
move `svs.time` 300 ms and send nothing. VERIFIED: step 11's
`SV_ClientEnterWorld` stores `svs.time` into `nextSnapshotTime` (client
offset `0x10b14`, store at `0x8087838`; `cod11-server-handshake.md`,
"Clients not in the world"). INFERRED, from those two: a client that was in
the world is due at the next `SV_Frame`, which sends it the restart's first
snapshot.

Labels for `cod_lnxded`: `Com_Frame` `0x806ce88`, `Cbuf_Execute`
`0x805aa7c`, `Com_EventLoop` `0x806bed4`. UNVERIFIED: all three names.
VERIFIED: `0x805aa7c` splits its buffer on `;` and newlines outside quotes
and runs each piece.

VERIFIED: the offsets, immediates and call targets in the list below.
INFERRED: the ordering and every "when" and "while".

- `Com_Frame` stores `Com_EventLoop`'s return in `com_frameTime`
  (`0x833df1c`) at `0x806cfe5` and subtracts the last frame's stamp
  (`0x8111f88`) at `0x806cff9`, giving `msec`.
- `Cbuf_Execute` runs at `0x806d003`. The new stamp goes into `0x8111f88`
  at `0x806d00d`, after it.
- On a dedicated server (`0x833dee0`'s integer non-zero, `0x806d088`),
  `msec` above 500 prints `"Hitch warning: %i msec frame time\n"`
  (`0x80cf9e0`, at `0x806d0a0`), and `msec` is capped at 5000 (`0x1388`,
  `0x806d0c0`).
- `SV_Frame(msec)` is called at `0x806d0e4`.
- `SV_Frame` (`0x808cdf8`) adds `msec` to a residual at `0x8355270`
  (`0x808cec1`) and runs a frame only when the residual holds
  `1000 / sv_fps`.
- Once per call: `0x808cccc` for each slot whose address type (client
  offset `0x528b8`) reads 0, the call at `0x808d152`, then `0x808cab8` at
  `0x808d192`. INFERRED, from what each writes: the bot think (a synthesized
  usercmd into `0x808789c`) and the ping average (`999` for a slot not
  active).
- The game frame loop, `0x808d1a5..0x808d224`: the residual loses a frame
  (`0x808d1a5`), `svs.time` gains one (`0x808d1ab`) and
  `VM_Call(gvm, 10, svs.time)`, `G_RunFrame`, runs (`0x808d1fa`). While the
  residual still holds a frame (`cmp` at `0x808d21e`, `jge` to
  `0x808d1a0`), `SV_ArchiveSnapshot` (`0x808fb84`) runs at `0x808d1a0` and
  the loop goes round again.
- Once after the loop: `SV_CheckTimeouts` (`0x808cbc0`, at `0x808d241`),
  `SV_SendClientMessages` (`0x809045c`, at `0x808d246`), the archive again
  (`0x808d24b`) and the heartbeat (`0x808ba0c`, at `0x808d258`).

INFERRED, from that order: the wall time a console command takes inside
`Cbuf_Execute` lands in the next `Com_Frame`'s `msec`. A script-queued
`map_restart` runs in the `Cbuf_Execute` after the frame that queued it, and
the frames its own run time owes come due together: they run back to back
in one `SV_Frame` and only the last is sent. INFERRED: an rcon `map_restart`
runs inside `Com_EventLoop`, ahead of the stamp, so the same `Com_Frame`'s
`SV_Frame` owes its time. Either way the first snapshot reads the last one
before, plus 300, plus 50 per frame owed. How many frames are owed depends
on how long the host takes to run the restart.

VERIFIED, live: retail 1.1d on `mp_carentan` with
`client-probes/probe_restart` (dm, plus a thread that calls
`map_restart(probe_persist)` 3 s after a player joins, repeated) or plain dm
and an rcon `map_restart`, one `vcod --net-probe --probe-edge watch` client
on loopback, the host under moderate to heavy build load. T is the last
snapshot before the restart; the gap is the wall time between it and the
next snapshot on the probe.

| Trigger | First snapshot | Wall gap (ms) | `commandTime` |
|---|---|---|---|
| `map_restart(0)`, spectator | T+500, T+550, T+450 | 228, 263, 193 | T+350 |
| `map_restart(1)`, spectator | T+850, T+550, T+900, T+1100, T+950, T+1000 | 582, 272, 614, 840, 688, 743 | T+350 |
| `map_restart(0)`, in play | T+1100, T+1250, T+1000 | 848, 994, 752 | T+350 |
| rcon `map_restart`, spectator | T+500, T+500, T+450, T+500 | 234, 223, 153, 206 | T+350 |

The first snapshot always had `deltaNum` -1 and the toggled
`SNAPFLAG_SERVERCOUNT`. The first snapshot minus 300 tracks the wall gap to
within a frame under every trigger, `savePersist` or not. A fixed "500 ms
after the restart frame" (one `probe_playersound` run) is one sample of
this, not a rule.

As implemented:

- `crates/server/src/main.rs` runs `Server::drain_console` before it counts
  the frames it owes, so a `map_restart`'s run time is counted, and
  `engine::tick` runs `SV_Frame`'s shape: bots and pings once, each owed
  frame through `game_frame` with an archive between, then timeouts,
  snapshots (`engine::send_messages`), the unentered and zombie messages
  and the heartbeat once. The catch-up cap is the dedicated `msec` cap of
  5 s. Trap: a tick that runs one frame and sends one snapshot sends the
  restart's first snapshot at T+350 and bursts a snapshot per owed frame
  after it.
- A restart keeps the level's parse of the BSP (`Carry::map`: the entity
  string and the two brush-model tables) beside the area tree, since
  `SV_MapRestart_f` keeps the map. INFERRED, from the call list above (no
  `CM_LoadMap`, no `0x808a12c`): retail's restart re-reads no map either.
  On the loaded host the read and parse were 180-240 ms of a 300-500 ms
  restart. With both changes, on the same host and load, vcod read T+450 to
  T+500 in five `map_restart(1)` runs and T+500 to T+1000 in five
  `map_restart(0)` runs, each with `commandTime` T+350. VERIFIED (capture,
  same probe). A restart's run time is the host's, so no test pins a wall
  figure. `tests/restart_first_snapshot.rs` pins the frame-count rule:
  owing one frame or five, exactly one snapshot arrives, at T+300 plus 50 a
  frame, with `commandTime` T+350.
- A restart keeps the compiled scripts (`Carry::scripts`: the installed
  functions and the interner, taken before anything ran). The new level
  starts a fresh `Vm` on them, so heap, threads and clock reset; `game[]` is
  planted after, on both paths, so a cached and a fresh compile intern
  alike. A new `--gametype-script` overlay compiles again.
- A map load compiles the map's closure again but resumes the gametype's,
  which is compiled first on every load and is the same whatever the map.
  `Carry::gametype` holds the `Program` and the `vcod_gsc::LoaderState` as
  they stood after it, used under the same gametype entry and builtin set
  once every file that loader read still reads the same text
  (`LoaderState::current`, which re-reads them all, so a compile's pak
  references are flagged again). The resumed compile equals the fresh one:
  same atoms in the same order, same functions, same animtrees
  (`a_cod1_load_resumes_...`, CoD2's `a_map_change_resumes_...`).
- Measured, release: on CoD2 the script stage of a map change went from
  12-16 ms to 7-10 ms (three stock maps through rcon `map`, min of five).
  mp_harbor dm, rcon `map_restart`, host loaded by other builds: the compile
  was 87-139 ms of a 149-229 ms restart tick before, 0.2-1.8 ms after, with
  the tick at 46-52 ms. Unloaded, the tick read 13-20 ms before and 10-12 ms
  after. This is vcod's own cost and says nothing about whether retail
  recompiles.

---

### 4.8 The script's random draws start over at every load

VERIFIED, `game.mp.i386.so` 1.1d: `randomInt` (builtin 0x5e470) is
`irand(0, n)` (0x3d47c) and `randomFloat` (0x5e4bc) `flrand(0, f)`
(0x3d43c). Both step one 32-bit state at 0x7dcd8 as
`s = s * 0x343fd + 0x269ec3` and draw its bits 17..31, `irand` as
`min + (r * (max - min) >> 15)`. VERIFIED: `G_InitGame` (0x4fb14) calls
`srand(randomSeed)` at 0x4fb63 and then `Rand_Init(trap_Milliseconds())` at
0x4fb6e..0x4fb74; `Rand_Init` (0x3d42c) only stores its argument into
0x7dcd8. VERIFIED, the same three calls in 1.5b (`G_InitGame` 0x4f10a:
`srand` 0x4f168, `trap_Milliseconds` 0x4f16d, `Rand_Init` 0x4f175;
`randomInt` 0x5f6a8 calls `irand` 0x38153 at 0x5f702) and UO 1.51b
(`G_InitGame` 0x586cf: 0x5872d, 0x58732, 0x5873a; `randomInt` 0x6b397 calls
`irand` 0x3f3a3 at 0x6b3f1), with the same constants in `irand`. VERIFIED,
`cod_lnxded` 1.1d: both `G_InitGame` calls (3 step 19 at 0x808919f, 4.1 at
0x80893b0) pass the return of 0x806d988 as `randomSeed`, a function that
drains the event queue at 0x8112040 and returns the last event's time.
INFERRED, off that shape against Q3's: it is `Com_Milliseconds`, and the
`srand` seed feeds only libc `rand()`, which `randomInt` does not read.

So a script's draws are neither carried across a level boundary nor
repeated: every `G_InitGame`, a `map_restart` included, seeds them from the
wall clock. VERIFIED,
`crates/server/tests/fixtures/gametypes/mp_cassino-hqtime-*-uo.txt`: the
level after the time limit's restart shows a different hq radio from the
first (`coduo-diff.md` 22).

As implemented:

- `Server::with_seed`, which the binary calls with the wall clock, keeps a
  level seed that seeds `GameHost::rng` at each load through `Carry::seed`
  and steps after it, so every level, restarts included, draws anew.
  `Server::new` is `with_seed` from the constant `RNG_SEED`: a test's first
  level draws the same every run and every later level draws anew, as on
  retail. `server.rs`'s `a_restart_reseeds_the_script_draws`, on
  `Server::new`, fails without the step.
- No seed reproduces a wall-clock draw, so the UO `gametypes_ab` probes that
  restart (`tdmend`, `ctftime`, `bas`, `bastime`, `basend`) run `with_seed`
  and replay each probe's retail spawn picks (`Server::pin_spawns`, read off
  the captures' first playing snapshot of each life). Trap: a `new` that
  repeats `RNG_SEED`'s draws on every level lets those gates match retail's
  second level by accident.
- Every `vcod-server` test binary that restarts or changes map (killcam,
  killcam_ab, mapchange_ab, restart_first_snapshot, roundrestart_ab,
  votes_ab, demos, gametypes_ab, configstrings_ab, configstrings_uo_ab,
  movers_ab, playsound_broadcast, push_ab, ride_ab, semantics_ents,
  turret_ab, two_clients, bots) passes at the 1.1 and UO installs, and again
  under a second seed stream (each step xored with a constant), and the 1.5
  restart gates at a 1.5 install. INFERRED, off those runs: none rides a
  repeated draw on a later level, so none needs pins.
- The script's four draws, the grenade's tumble and UO's barrage step
  `GameHost::holdrand` (`vcod_common::rng::HoldRand`, the LCG CoD2 uses
  too), seeded `seed as u32` from the level seed at each load.
  `Server::pin_script_rand` sets it for the running level, for a gate that
  replays a capture's draws. glibc's `rand()` stream (below) stays on
  vcod's xorshift (`GameHost::rng`, `Missiles`' bounce draws).
- Moving the draws to `holdrand` moved every test's first-level spawn
  picks and the probe target's head and gear: 31 `vcod-server` tests at
  1.1 and 7 at UO had stood on the xorshift's picks. The tests that need
  a spot pin it (`common::pin_carentan_pair`, `pin_spawns_at`; turret
  captures that predate `PROBE spawnat` pin the point whose spawn drift
  matches retail's). `leanhit_ab` and `scriptmodel_ab` seed `holdrand`
  with the smallest seed that draws the capture's logged `PROBE models`
  set (variant `randomInt(3)`, then head `randomInt(20)`). The UO
  artillery gate pins retail's shell points (`Server::pin_barrage_points`):
  an incoming outside the client's view is not sent, so the count seen
  rides the yaw and spread draws.

**Who draws `holdrand`.** VERIFIED, every call site of `irand`, `flrand`
and `Rand_Init` in each module (`R_386_PC32` relocations on 1.1d, calls
to the PLT stubs on 1.5b and UO):

| Build | `randomInt` | `randomFloat` | `randomIntRange` | `randomFloatRange` | `fire_grenade` | other |
|---|---|---|---|---|---|---|
| 1.1d | 0x5e470, `irand` at 0x5e4ad | 0x5e4bc, `flrand` at 0x5e4da | 0x5e4f0, `irand` at 0x5e53e | 0x5e554, `flrand` at 0x5e5c5 | `flrand` at 0x5459e, 0x545c2 | none |
| 1.5b | 0x5f6a8, 0x5f702 | 0x5f715, 0x5f744 | 0x5f757, 0x5f7c6 | 0x5f7d9, 0x5f84f | 0x554df, 0x5550b | none |
| UO 1.51b | 0x6b397, 0x6b3f1 | 0x6b404, 0x6b433 | 0x6b446, 0x6b4b5 | 0x6b4c8, 0x6b53e | 0x5fa95, 0x5fac1 | `G_BarrageThink` `irand` at 0x5e86b, 0x5e926, 0x5e946 |

VERIFIED: `Rand_Init`'s one caller in each module is `G_InitGame`, so
nothing else reseeds the stream inside a level. VERIFIED,
`tools/re/dump_builtins.py`: the four are rows 34-37 of 1.1d's
`functions` table, 36-39 of 1.5b's and 39-42 of UO's.

**The builtins**, read in 1.1d; 1.5b and UO take the same branches
(VERIFIED, their disassembly at the addresses above):

- VERIFIED: `randomInt(n)` takes `Scr_GetInt(0)`; `n` not above 0 prints `"RandomInt
  parm: %d  "` (.rodata 0x773c2) and raises `"RandomInt parm must be
  positive integer.\n"` (0x773e0) without drawing; else `irand(0, n)`.
- VERIFIED: `randomFloat(f)` is `flrand(0, Scr_GetFloat(0))`, no check.
- VERIFIED: in `randomIntRange(lo, hi)`, `hi` not above `lo` raises `"RandomIntRange
  range must be positive integer.\n"` (0x77440); else `irand(lo, hi)`.
- VERIFIED: in `randomFloatRange(lo, hi)`, the `fcom` at 0x5e577 (1.5b `fucompp`
  0x5f80f) raises `"Scr_RandomFloatRange range must be positive
  float.\n"` (0x774c0) when `hi` is not above `lo`; an unordered
  compare (a NaN bound) takes the draw.

**The arithmetic.** VERIFIED: `irand` (1.1d 0x3d47c) is
`min + ((max - min) * (s >> 17) >> 15)` in 32-bit ints. `flrand` (1.1d
0x3d43c) is `(max - min) * (s >> 17) * 2^-15 + min` on the x87 stack
(the factor a float at .rodata 0x72a40, 3.0517578e-05), returned
unrounded; 1.5b's (0x380ea) and UO's (0x3f33a) store `s >> 17` through
a float, divide by 32768 and round the result to a float before
returning. INFERRED: for float bounds the two give the same float, since
the 15-bit draw and the power-of-two scale add no rounding a float
result keeps. VERIFIED, `fire_grenade` 1.1d 0x545aa, 0x545c7: the tumble
adds 720 or 360 to the returned value on the x87 stack; `flrand(-45, 45)`
is a multiple of 2^-14 under 45, so the sum is exact in a float either
way (INFERRED).

**The seed.** VERIFIED: the seed is `trap_Milliseconds()`'s return.
INFERRED, off Q3's `trap_Milliseconds` being `Sys_Milliseconds`: it is
the server process's wall clock in milliseconds, so no two runs draw
alike and a capture's draws replay only from a seed recovered off them.

### 4.9 What a restart costs

VERIFIED, live: the round trip of an rcon `map_restart` to retail's `print`
reply, which `SV_MapRestart_f` sends only once the level is re-inited; five
restarts each, no client in, loopback, in a sealed namespace
(`unshare -rnm`):

| Server | Map, gametype | Round trip |
|---|---|---|
| 1.1d `cod_lnxded` | mp_carentan sd | 26 to 35 ms |
| UO 1.51b `coduo_lnxded` | mp_foy sd | 109 to 116 ms |
| UO 1.51b | mp_foy bas | 110 to 124 ms |
| UO 1.51b | mp_cassino sd | 96 to 101 ms |

Retail UO's restart holds its frame loop about 100 ms, four times 1.1's.
Where UO spends it is not read.

vcod-server, the same restarts with six shooting bots
(`VCOD_TICK_PROFILE_MS=20`; the `console/map_restart` and `load/*` phases,
`spawn/*` inside `load/spawn`), mp_foy sd:

- The restart tick is 22 to 25 ms (`load/spawn` 15, 6 of it the 36
  vehicles' spawns; `load/bootstrap` 3; the three settle frames 1.3).
  mp_cassino sd's and mp_foy bas's restart ticks stay under 20. The scripts
  are not recompiled (`load/compile` under 0.1 ms off `Carry::scripts`).
- Trap: UO's `G_ParseScrVehicleInfo` (`vehicle::load_level`,
  `coduo-vehicles.md` 10.1) lists files through
  `Pk3Fs::list_dir_in_search_order`. Reopening every pak and parsing its
  central directory each time cost 100 to 126 ms of a 125 to 131 ms tick; it
  reads the paks off the `Pk3Fs`'s open cache, in the same order (each pak's
  central-directory order, newest pak first).

The bot pass after a `bas` restart is a separate cost. Measured on mp_foy
bas, six shooting bots, runs of five restarts:

- `Server::breach_stands`, asked of each base by a rocket carrier after
  every graph rebuild (a restart relinks the walls, so the graph rebuilds),
  traced `breach_aim` from every graph node 0 to `BREACH_STAND_FAR` off the
  base: `head/bots` ticks of 66 to 292 ms within two seconds of a restart,
  the longest in `bots/deal`. It ranks those nodes by sector and score first
  and traces only until each sector has its stand, the same stands
  (`bmuo_bas` and `dguo_bas` match the base recording): `bots/deal` 43 to
  55 ms on those ticks.
- `bots/goal` read 50 to 96 ms on a few ticks, split into `goal/give way`,
  `goal/pre route`, `goal/brain` and `goal/drive`; all of it was
  `goal/drive`, one driver's `drive_waypoint` plan running 8k to 21k
  uncached hull fit tests (about 3.5 us each). Restarts did not empty the
  cache; each spike was a plan into nodes no plan had reached. The fits are
  tested as A* pops a node, kept across graph rebuilds (dropped only where a
  toggled submodel's bounds reach) and warmed at 600 tests a tick
  (`bot-objectives.md` 10.3): over two runs of five restarts `bots/goal`
  peaks at 17 ms and the warm pass (`bots/nav tick`) at 16.
- `bots/deal` then read 38 to 69 ms on a few ticks, split into
  `deal/vehicles`, `deal/breach models` (with `models/script` and
  `models/vehicles` inside), `deal/uo` and `uo/breach stands`. Three costs:
  - `breach_stands` walked the graph's strongly connected components again
    for each base, 9 to 26 ms each (43 on a loaded machine), six bases on
    one or two ticks. The finished graph indexes them and nothing changes
    it after; `NavGraph::component_ids` reads those in 0.0 ms, and
    `NavGraph::zone_stand`'s pick does the same.
  - Its traces, 24 to 94 candidates a base, ran 2.4 to 13 ms a base, one
    sector after another. The 16 sectors search on up to 8 threads
    (`first_passes`): items go out by their place in their sector, one past
    a stand its sector already found is skipped, so each sector keeps the
    first candidate in order with a shot, the stand the serial search took.
    0.9 to 2.4 ms a base. `PlacedModel`'s triangles are `Arc` for that.
  - The first rocket carrier's model gather after a restart read 20 to 27
    ms: `models/vehicles` 16 to 23 (each vehicle's rig, xmodel and
    skeleton) and `models/script` 4 to 6 (script models' collision), loaded
    again because each level's `GameHost` starts empty. A restart reads the
    same files off the same paks and keeps the referenced-pak lists, so
    `Carry::models` hands the new level the old one's `xmodel_collision`,
    `xmodel_bounds` and vehicle rigs; a map change does not.
- With all three, two runs of five restarts held no tick over the 20 ms
  report threshold; the six bases' stands together cost 9 ms on the tick
  both teams first carried a rocket. `bmuo_*`, `dguo*` and
  `it_bot_vehicles_uo` match the base recording.

A full server's tick in play (24 bots on 1.1's mp_carentan and UO's
mp_cassino `tdm`, beside CoD2's and CoD4's) and where it goes:
`cod4-server-frame.md` 10.

## 5. `SV_MapRotate_f` and the rotation grammar

Label for `cod_lnxded` `0x80840c8`. VERIFIED: registered as the console
command `map_rotate` by `Cmd_AddCommand("map_rotate", 0x80840c8)` at
`0x8084b31`, name string at `0x80d3ff3`.

VERIFIED: two cvars drive it, `sv_mapRotation` (pointer at `0x8355238`, name
`0x80d57b5`) and `sv_mapRotationCurrent` (pointer at `0x8355224`, name
`0x80d57c4`), both registered with an empty default at
`0x808ad5c..0x808ad82`.

### 5.1 `NextToken`

Label for `cod_lnxded` `0x8084014`. VERIFIED: the offsets, string addresses
and call targets below. INFERRED: the ordering and the branch conditions.

- It runs `COM_Parse` (`0x8081d1c`) over `sv_mapRotationCurrent`'s string,
  which returns one token and advances the pointer
  (`0x808401c..0x808402e`).
- When the advanced pointer is null it does
  `Cvar_Set("sv_mapRotationCurrent", "")` and returns null
  (`0x808403f..0x8084053`).
- Otherwise it copies the remainder onto the stack and does
  `Cvar_Set("sv_mapRotationCurrent", <remainder>)` before returning the
  token (`0x8084055..0x80840bb`).

INFERRED, off that write-back: the rotation is consumed one token per call,
the cvar is the whole cursor, and a server restarted mid-rotation resumes
where the cvar left it.

`COM_Parse` (`0x8081d1c`) is `Com_ParseExt(&p, 1)` (`0x808170c`) on the
parse session `PTR_DAT_080e2f40` points at. VERIFIED: the console's session,
at `0x80de980`, holds 1 at `+0x408` and 0 at `+0x40c` in `.data`
(`0x80ded80`), and the 1.5b (`0x80ea0a0`) and UO 1.51b (`0x80f09a0`)
engines' sessions hold the same. INFERRED, off `Com_ParseExt`'s branches on
those two words (`+0x40c` 0 keeps the inline parser, `+0x408` non-zero takes
the space-delimited arm over the C-token one):

- Blanks are skipped first: any byte below `!` as a signed `char`, so bytes
  from 0x80 up are blanks too. `//` runs to the newline, `/* */` to the
  first `*/` after the opening `*` (`/*/` is a whole comment). Nothing left
  sets the pointer null, which `NextToken` turns into an empty cvar.
- A `"` opens a quoted word, read to the next `"` with `\"` as a quote; the
  pointer is left past the closing quote. An unterminated one leaves it past
  the string's terminator, reading whatever memory follows.
- Anything else is a word of bytes above the space; the pointer is left on
  the blank that ended it.
- A word keeps 1023 bytes; more are dropped.

So the rest written back keeps its delimiter. VERIFIED live (1.1d, inside
`unshare -rn`, `+set sv_mapRotation gametype dm map mp_harbor map
mp_brecourt +map_rotate +map_rotate`): the second `map_rotate` printed
`"sv_mapRotationCurrent" is:" map mp_brecourt"`, its leading blank kept.
INFERRED, off the non-null pointer the quoted arm leaves: a quoted `""` is a
word, not the end.

### 5.2 The loop

VERIFIED: the string addresses, call targets and immediates below.
INFERRED: the ordering and the branch conditions.

- Three banners print first: `"map_rotate...\n\n"` (`0x80d3a45`),
  `"\"sv_mapRotation\" is:\"%s\"\n\n"` (`0x80d3a55`) and
  `"\"sv_mapRotationCurrent\" is:\"%s\"\n\n"` (`0x80d3a80`)
  (`0x80840cf..0x8084106`).
- An empty `sv_mapRotationCurrent` is refilled from `sv_mapRotation`
  (`0x808410b..0x8084131`); if the first `NextToken` still returns null it
  is refilled and retried once more (`0x808413f..0x808415a`).
- A null token after that prints
  `"No map specified in sv_mapRotation - forcing map_restart.\n"`
  (`0x80d3ac0`) and calls `SV_MapRestart_f` (`0x808422d`).
- Token `"gametype"` (`0x80d3afb`, case-insensitive) reads one more token.
  A null one prints `"No gametype specified after 'gametype' keyword in
  sv_mapRotation - forcing map_restart.\n"` (`0x80d3b20`) and calls
  `SV_MapRestart_f`. Otherwise it prints `"Setting g_gametype: %s.\n"`
  (`0x80d3b79`), and when the server is running and the token differs from
  `g_gametype`'s string case-insensitively it calls `VM_Call(gvm, 15, 0)`
  (`0x80841ae..0x80841e2`), section 1's `savePersist` setter writing 0.
  Then `Cvar_Set("g_gametype", <token>)` (`0x80d3b92`) and the loop
  continues.
- Token `"map"` (`0x80d3b9d`, case-insensitive) reads one more token. A
  null one prints `"No map specified after 'map' keyword in sv_mapRotation
  - forcing map_restart.\n"` (`0x80d3bc0`) and calls `SV_MapRestart_f`.
  Otherwise it prints `"Setting map: %s.\n"` (`0x80d3c0f`), calls
  `Cbuf_ExecuteText(0, va("map %s\n", <token>))` (`0x8084242..0x8084256`)
  and returns.
- Anything else prints `"Unknown keyword '%s' in sv_mapRotation.\n"`
  (`0x80d3c40`), reads the next token and loops (`0x8084260..0x8084278`).

VERIFIED (UO 1.51b, `probe_ctftime`,
`tests/fixtures/gametypes/mp_cassino-ctftime-*-uo.txt`): a gametype script's
`setCvar("sv_mapRotation", ...)` drives the next `map_rotate`: the probe
writes `gametype probe_ctftime map mp_foy` at load, and the
`exitLevel(false)` at its map end loads mp_foy. INFERRED, off section 5's
`Cvar_Set` calls and the game module's `setCvar` being a `trap_Cvar_Set`:
both rotation cvars live in the one engine table, so a script can also read
or rewrite `sv_mapRotationCurrent`. vcod's `map_rotate` reads both from the
script's cvar table when a level is loaded and writes the queue back there
(`Server::exec_console_line`, `console::Command::MapRotate`). Trap: reading
only the server's own copy (filled by `--set` and rcon) misses the
script's write.

INFERRED, off the `VM_Call(gvm, 15, 0)` arm: a gametype change in the
rotation drops whatever persistence the outgoing level asked to keep,
because the flag `SV_SpawnServer` reads at 3 step 2 is zeroed before the
`map` line queues.

### 5.3 `SV_Map_f`

Label for `cod_lnxded` `0x8083c68`. VERIFIED: registered twice, as `map`
(`0x80d3b9d`) at `0x8084b1f` and as `devmap` (`0x80d3919`) at `0x8084b58`.
VERIFIED: it compares `Cmd_Argv(0)` against `"devmap"` at `0x8083d13`, keeps
the result in a register across the function (`0x8083d1d..0x8083d20`),
`test`s that register at `0x8083dac` with a `je` to `0x8083dc4`, and has a
`Cvar_Set("sv_cheats", "1")` (`0x80d3922`) at `0x8083dbd`. INFERRED, off that
test and its jump: `sv_cheats` is set only when the command word was
`devmap`.

VERIFIED: it holds a `Q_strncmp` of the map argument against `mp/`
(`0x80d38e3`) at `0x8083d2e` and against `mp\` (`0x80d38e7`) at `0x8083d45`,
and one `Q_strncpyz` at `0x8083d6a` reached from two argument setups, one
pushing the argument plus 3 with a bound of `0x3d` (`0x8083d51..0x8083d5a`)
and one pushing the argument itself with a bound of `0x40`
(`0x8083d60..0x8083d65`). INFERRED, off those comparisons and the setups they
select: either prefix is stripped and the bound shrinks with it. The
compares picking `SV_MapRestart_f` over `SV_SpawnServer` are 4.2's.

### 5.4 As implemented

`crates/server/src/console.rs` parses the `Command`s.
`crates/server/src/engine/rotation.rs` holds 5.1's token consumer (`Words`,
`COM_Parse`'s space-delimited arm with the rest kept from the delimiter on)
and 5.2's keyword loop (`rotate`), which CoD2's and CoD4's servers run too.
`Server::drain_console` runs the `map %s` line the loop queues. `devmap`
sets `sv_cheats` after the load (handshake doc, "Console commands over
rcon").

---

## 6. Intermission

### 6.1 What arms it

VERIFIED: `ClientEndFrame` (`0x40e98`) holds a `cmp client+0x20ec, 2` at
`0x40ebe` whose `jne` targets `0x414ed`, the function's exit, and a
`cmp client+0x20d0, 3` at `0x40ed1`. INFERRED, off those compares: a client
not fully connected reaches none of the three arms, and for one that is,
`sessionstate` 3 takes the intermission arm (`cod11-gsc-object-model.md`,
"What `ClientEndFrame` writes for a live client's own view").

VERIFIED: `Scr_SetClientField` maps the four legal `sessionstate` strings
onto that word as `playing` 0, `dead` 1, `spectator` 2, `intermission` 3,
comparing against `scr_const+0xd0`, `+0xfa`, `+0x7c` and `+0xb6` and storing
the four literals at `0x41988`, `0x4199d`, `0x419b9` and `0x419ce`.
VERIFIED: `GScr_LoadConsts` fills those four `scr_const` slots from the
strings at `0x763b8`, `0x764cd`, `0x76164` and `0x762be`, and the error
string at `0x72dc0` reads `"'%s' is an illegal sessionstate string. Must be
playing, dead, spectator, or intermission."`. VERIFIED: `0x419ce` is the
module's only store of 3 into `client+0x20d0`; the six other stores write 0,
1 or 2. INFERRED: only a script's `self.sessionstate = "intermission"`
enters intermission.

### 6.2 The two arms

`ClientEndFrame`'s intermission arm is `0x40ed6..0x40f1f`. VERIFIED: the
offsets and immediates below, each a single store. INFERRED: the order of
the list.

- `ent.takedamage` (`+0x171`) 0 and `ent.r.contents` (`+0x118`) 0.
- `ent.r.svFlags` (`+0xf4`) `(svFlags & ~2) | 1`.
- `ps.pm_type` (`+0x4`, netfield offset 4) 5.
- `ps.viewmodelIndex` (`+0xbc`, netfield offset 188) 0.
- `ps.pm_flags` (`+0xc`) loses bit `0x40000` (`and byte [edx+0xe], 0xfb`),
  the "own view" bit `cod11-gsc-object-model.md` records.
- `ps.eFlags` (`+0x80`, netfield offset 128) is masked with `0xfffbfbff`,
  clearing `0x400` and `0x40000`: the held trigger and the talk bit
  `PmoveSingle` sets (`cod15-movement.md` 5, "The talk button"). VERIFIED.

The arm writes no health, and the client's own health is gone by the time it
is on the wire. VERIFIED: the dm map-change capture's `pm_type=5` traces read
`health` 0 and `eFlags` 24 where the same client read 100 and 16 on the
frame before (`crates/server/tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`).
VERIFIED: `ClientSpawn` (`0x4268c`), which `spawnIntermission`'s
`self spawn(origin, angles)` reaches, bzeroes the whole `gclient_t` for
`0x22c4` bytes at `0x42804`, copying `client+0x20d0` out and back for
`0x104` bytes around it (`0x427ea`, `0x42824`), and stores no health.
INFERRED, from those two against the arm's stores: that spawn zeroes
`ps.stats[0]` and nothing copies `ent->health` back for a client in
intermission, hence the 0 on the wire.

VERIFIED: `ClientThink_real` (`0x3fee0`) holds a `cmp client+0x20d0, 3` at
`0x3ffca` whose `jne` targets `0x40010`; the block at `0x3ffcf..0x4000a`
past it holds four stores and one jump: `client+0x21ec` takes
`client+0x21e8`, `client+0x21e8` takes the byte at `client+0x20f4`,
`client+0x21f8` takes `client+0x21f4`, `client+0x21f4` takes the byte at
`client+0x20f5`, and `0x4000a` jumps to `0x40653`, the function's exit.

INFERRED, off that compare and jump: `sessionstate` 3 reaches only those
four stores, so no pmove runs, no events are generated and the usercmd's
movement and view angles are dropped. An intermission client cannot move,
and its view is whatever the last pre-intermission frame left.

VERIFIED: the call to `G_TouchTriggers` is at `0x405b3`, past the
`0x4000a` jump, and the `sessionstate` 2 arm at `0x40010` tail calls
`SpectatorThink` (`0x3fab8`) and jumps to the same `0x40653` exit at
`0x40022`. INFERRED, off those two jumps: neither an intermission client nor
a spectator reaches the touch pass. vcod's `ScriptRuntime::touch_triggers`
and `touch_triggers_at` touch only at `ps.pm_type <= 1`
(`TOUCH_MAX_PM_TYPE`), which also takes the dead out
(`cod11-gsc-object-model.md` 8.2).

VERIFIED: the call at `0x405b3` sits behind a `cmp client+0x21d8, 0` at
`0x405a6` whose `jne` skips it, and `client+0x21d8` is written only in
`ClientCommand` (`0x48b81`) and `Cmd_Noclip_f` (`0x494bd`). INFERRED, off
the writers: that word is the noclip flag and the compare is a noclip
exemption, not a state gate. vcod has no noclip and models none of it.

The spectator arm copies no health either. VERIFIED, the round-restart
target capture: a player killed by its own `kill` reads `pm_type=6
health=0`, then `pm_type=4 eFlags=16 health=0` on every frame of the
spectator spawn `Callback_PlayerKilled` parks it in (`ms=25436` to
`28607`), and 100 again only on the restart's own spawn. vcod's
`become_spectator` zeroes the sim's health, as `become_intermission` does,
and the per-frame vitals mirror (`mirror_vitals`) skips both. Trap: copying
the entity's 100 back in puts health on those frames.

### 6.3 The scoreboard drain, and which team score is which

VERIFIED: `G_RunFrame` carries an inlined drain at `0x50ae9..0x50b42`
holding a `cmp level+0x20c, 0` at `0x50ae9`, a client loop bounded by
`level+0x1e0` at `0x50af4`, a `cmp client+0x20ec, 2` at `0x50b0b`, a
`cmp client+0x4, 5` at `0x50b14`, a call to
`DeathmatchScoreboardMessage(&g_entities[i])` at `0x50b1e` and a store of 0
into `level+0x20c` at `0x50b3b`.

INFERRED, off those three compares and the store's address: the loop runs
only when `level+0x20c` is set, sends to each client that passes both
compares, and clears the flag afterwards. INFERRED, off the `client+0x4`
compare against 6.2's store: `client+0x4` is `ps.pm_type`, so the periodic
scoreboard push goes exactly to the clients in intermission, selected by the
`pm_type` the intermission arm writes.

VERIFIED: `level+0x20c` is set to 1 by `CalculateRanks` at `0x50cf5` and by
the `setteamscore` builtin at `0x5ba7e`.

VERIFIED: the module's relocation table holds five `R_386_PC32` references
to `CalculateRanks` (`0x50c60`): at `0x418ef` and `0x41b07`, both inside the
range 6.1 reads `Scr_SetClientField`'s `sessionstate` stores from, and at
`ClientConnect+0x20c` (`0x42678`), `ClientDisconnect+0x174` (`0x42c20`) and
`ClientBegin+0x21` (`0x42fa9`). INFERRED, from the first two sitting among
the client field setter's arms: a script write to a client field dirties the
flag during a live level; the other three are the roster changing.

VERIFIED: the `setteamscore` builtin (`0x5b9dc`, functions table row 88)
reads a const string through `Scr_GetConstString(0)`, `cmp`s it against
`scr_const+0x4` at `0x5b9f2` and against `scr_const+0x8` at `0x5b9fb`, has a
`Scr_Error` with `"Illegal team string '%s'. Must be allies, or axis."`
(`0x77fc0`) at `0x5ba22`, a `Scr_GetInt(1)` at `0x5ba2f`, a third `cmp`
against `scr_const+0x4` at `0x5ba37`, and two store pairs: `level+0x200`
with `trap_SetConfigstring(6, va("%i", n))` at `0x5ba40..0x5ba59`, and
`level+0x1fc` with `trap_SetConfigstring(5, ...)` at `0x5ba60..0x5ba79`. The
`"%i"` format is `0x77fac`. VERIFIED: `GScr_LoadConsts` fills
`scr_const+0x4` from `"allies"` (`0x75f41`) and `scr_const+0x8` from
`"axis"` (`0x75f51`) at `0x58598` and `0x585c8`.

INFERRED, off the third `cmp` and its two jumps: `allies` takes the first
store pair, `axis` the second, and any other string the error.

INFERRED, from the two stores against the two consts: `level+0x1fc` is the
**axis** score and `level+0x200` the **allies** score; configstring 5
carries axis and 6 allies.

VERIFIED: `getteamscore` (`0x5f18c`, row 87) reads the same two slots against
the same two consts (`0x5f1da..0x5f1f0`), and `DeathmatchScoreboardMessage`
(`0x459c0`) pushes `level+0x1fc` before `level+0x200` into `"b %i %i %i%s"`
(`0x737f0`) at `0x45b14..0x45b27`. INFERRED, from that push order: the `b`
command's token 2 is axis and token 3 allies, as `cod11-hud-protocol.md`
records off the scoreboard writer alone; the two score builtins confirm it
independently.

### 6.4 The three dead functions

VERIFIED: `IntermissionClientEndFrame` (`0x4166c`),
`ClientIntermissionThink` (`0x41628`) and
`SendScoreboardMessageToAllIntermissionClients` (`0x50bf0`) have no
reference in the module: no relocation targets them, no `call` reaches them
and no immediate holds their addresses. VERIFIED:
`SendScoreboardMessageToAllIntermissionClients`'s body (`0x50bf9..0x50c4b`)
is the drain `G_RunFrame` inlines at 6.3.

INFERRED, off those three being unreferenced: the Q3 intermission machinery
was compiled but never wired up; 6.2's two arms and 6.3's drain are the
whole of intermission in the shipped module.

### 6.5 As implemented

- `crates/server/src/spectate.rs` carries 6.2: `PmType::Intermission`, the
  `pm_type` 5 and `eFlags` writes, and the no-pmove arm.
  `ClientSim::become_intermission` zeroes the sim's own health for the bzero
  above, and `mirror_vitals` (`crates/server/src/server.rs`) skips an
  intermission sim so nothing writes it back.
- 6.3's drain runs off `ScriptRuntime::take_ranks_dirty` (the host's
  `ranks_dirty`, `level+0x20c`) in `crates/server/src/server/cod1_frame.rs`;
  the rows come from `GameHost::scoreboard`
  (`crates/server/src/game/scoreboard.rs`), the team scores from the host's
  `team_scores`, axis first.
- `crates/server/src/game/builtins/score.rs` carries `getteamscore` and
  `setteamscore`, their two configstring writes included.
- vcod does not reproduce 6.4's dead functions.

---

## 7. There are no engine-side exit rules

### 7.1 No symbols, no strings

VERIFIED: `game.mp.i386.so` exports no `CheckExitRules`,
`BeginIntermission`, `FindIntermissionPoint`, `MoveClientToIntermission` or
`LogExit`. The only symbols whose names contain `intermission` or `exit` are
`ExitLevel`, `ClientIntermissionThink`, `IntermissionClientEndFrame`,
`SendScoreboardMessageToAllIntermissionClients` and `g_intermissionDelay`;
three of the five are unreferenced (6.4).

VERIFIED: neither binary contains the string `timelimit`, `scorelimit` or
`fraglimit`.

VERIFIED: `g_intermissionDelay` (`0x18f020`) has exactly one relocation
pointing at it, at `0x7ded0`, inside `gameCvarTable` (`0x7de28`, `0x6a8`
bytes). INFERRED: the cvar is registered and never read.

### 7.2 What that leaves

INFERRED, from 7.1 and section 2's single caller: the engine never decides
that a level is over. Every stock gametype's script counts its own score and
clock and calls `exitLevel()` or `map_restart()`; the C side does what
sections 1 and 2 describe and nothing more. A vcod server that implements
sections 1 to 6 and runs the stock gametype scripts gets the exit rules for
free. A time or score limit hard-coded in Rust is a rule retail does not
have.

### 7.3 `nextmap` is registered, set and never read

VERIFIED: the string `"nextmap"` (`0x80d54dd`) has exactly two references in
`cod_lnxded`: `Cvar_Get("nextmap", "", 0x100)` at `0x808abf3`, and
`Cvar_Set("nextmap", "map_restart")` inside `SV_SpawnServer` at `0x808a520`
(3 step 14). INFERRED, from both being writes: the engine never reads the
cvar. The Q3 convention of the gametype writing `nextmap` and the engine
running it does not exist in CoD 1.1; `map_rotate` (section 5) is the whole
of the rotation.

### 7.4 As implemented

vcod implements no exit rules, by design: the stock gametype scripts call
`exitLevel()` and `map_restart()`, which land in
`crates/server/src/game/builtins/cvar.rs`. vcod does not model `nextmap`,
since retail never reads it.

---

## 8. What vcod's own server does, measured the same way

`--save-mapchange` and `--save-roundrestart`, the probe modes that took the
retail captures, point at any server, so both ran against
`cargo run -p vcod-server`. Every claim here is evidence about vcod; the
retail column is the three committed fixtures under
`crates/server/tests/fixtures/netchan/`. Neither run is committed: both modes
write into the retail fixtures' own names, so each run's files were moved
out of the tree and the directory checked out again.

Two runs, both on `mp_carentan`:

- `dm`, 190 s, one probe. `--set g_gametype=dm --set scr_dm_timelimit=1
  --set 'sv_mapRotation=gametype dm map mp_carentan map mp_brecourt'`,
  against `--net-probe --probe-team allies --save-mapchange`. It spans two
  map ends: the first rotation token is the map already serving, the second
  `mp_brecourt`.
- `sd`, 200 s and 160 s, two probes on opposite teams. `--set
  g_gametype=sd --set scr_sd_roundlength=1 --set scr_friendlyfire=1`,
  against `--probe-team axis --probe-target --save-roundrestart` and
  `--probe-team allies --save-roundrestart`. It spans three round restarts.
  Neither probe reached a team (8.3), so it measured the restart the round
  clock drives, not the one an elimination does.

### 8.1 What matches

VERIFIED, the `dm` run against `mp_carentan-dm-mapchange.txt`: two
gamestates and one out-of-band line, in retail's order and with retail's
numbers. `serverId` 16 on the first, 33 on the second; `mapname`
`mp_carentan` then `mp_brecourt`; 246 configstrings then 217, both equal to
retail's. The rotation's first token named the map already serving and took
the restart path (id to 17, low nibble); only the second token spawned a
server (id to 33, high nibble). That is 3 step 16's and 4 step 5's
arithmetic and 4.2's reading of `map <samemap>`.

VERIFIED, the same pair: the intermission burst is `u`, then
`v g_scriptMainMenu "main"`, then `v cg_objectiveText`, all three in one
millisecond, followed by the first `pm_type` 5 frame in the same
millisecond. That frame reads `pm_type=5 eFlags=24 health=0 weapon=0`, an
empty `ammoclip`, `eventSequence=0` with an empty ring, and `origin`
384.0,-624.0,184.0: field for field retail's first intermission frame, the
origin included. Both map ends held it ten seconds and change, `dm.gsc`'s
`wait 10` between `endMap` and `exitLevel`.

VERIFIED, the `dm` run: the restart's reliable burst is `d 3`, `n`, `d 1`,
in that order, all in one millisecond, and the `d 1` systeminfo carries
`sv_serverid\17`, retail's value at that point. Then the reopened team menu
(`v g_scriptMainMenu "team_americangerman"`, `v scr_showweapontab "0"`,
`t 0`, `v cg_objectiveText`), one `pm_type` 4 frame at the intermission
origin, the weapon menu (`v scr_showweapontab "1"`,
`v g_scriptMainMenu "weapon_american"`, `t 1`) and the first live frame.
Retail's burst is the same commands in the same order plus the `d 13` and
`d 12` 8.2 records.

VERIFIED, both boundaries of the `dm` run: the first live frame after each
reads `pm_type=0 eFlags=16 health=100 weapon=12` and
`ammoclip=3:7,6:3,10:15`, retail's frame and spawn loadout. The frame before
it is a single `pm_type=4 eFlags=24 health=0` at the level's spectator
point, `384.0,-624.0,184.0` on `mp_carentan` and `-2779.0,1585.0,576.0` on
`mp_brecourt`, the origins retail's capture reads at the same moments.

VERIFIED, the `dm` run: one `loadingnewmap\nmp_brecourt\ndm` out-of-band
line and nothing else outside the reliable stream, and no gamestate on the
wire until after it. INFERRED, from that against `spawn_server`, which holds
no gamestate send: the second gamestate answered the probe's next message,
so vcod's map change is pull-shaped as 3.1 reads retail's.

VERIFIED, both runs: every `b` scoreboard line in the capture halves that
send `score` answers one, and the halves that send none carry none. Nothing
pushed a scoreboard at the intermission, which is 6.3's drain.

VERIFIED, the `sd` run against `mp_carentan-sd-roundrestart-*.txt`: three
round restarts, `serverId` 16 to 19 in the low nibble with no gamestate at
any of them, each one's reliable burst `d 3`, `n`, `d 1` in one millisecond
followed by the client cvars the gametype re-sends, each burst about five
seconds after the announcer command that ended the round. The gamestate
carries 273 configstrings, retail's count on the same map and gametype.

VERIFIED, the `sd` run: the reopened menu after a restart carries no `t`,
where the `dm` run's does. INFERRED, from the two against section 1's
`savePersist` gate: `sd.gsc` passes 1 and its `pers["team"]` survives, so
its `Callback_PlayerConnect` skips the `openMenu` that a `dm` client, whose
`pers` was freed, takes.

### 8.2 What differs

Each bullet was a defect in vcod the runs found. One is still open, the
`loadingnewmap` timing (the sixth bullet).

- Closed. VERIFIED, the `dm` run: the restart burst carried no `d 13` and no
  `d 12` where retail's carries both, and its `d 3` read `t\0` where retail's
  reads the incoming level's start time. 4.5's write log and squash now
  drive both off the level clock, and `tests/mapchange_ab.rs` compares the
  burst slot for slot.
- Closed. VERIFIED, the `dm` run: no `f` server command at all, against five
  in retail's capture (`MPSCRIPT_CONNECTED` at each of the three level
  starts, `MPSCRIPT_TIME_LIMIT_REACHED` at each of the two map ends). Cause:
  `iprintln` routed to `println`'s handler. `builtins::io::iprint_line`
  sends `f "<message>"` to every client, or to the receiver alone when the
  call has one: the split between `Scr_MakeGameMessage`'s two callers at
  `0x5cd28` and `0x45594`.
- Closed. VERIFIED, the `dm` run: the intermission's `v cg_objectiveText`
  value was `MPSCRIPT_WINS` and the localized separator, where retail's
  adds the winner's name and colour code. Cause: `set_client_cvar` read only
  `args[0]` and `args[1]`. `builtins::message::construct` packs the
  substitution arguments as `Scr_ConstructMessageString`
  (`game.mp.i386.so` `0x594d0`) does, a player entity through the `%s^7` at
  `0x767d9`.
- Closed. VERIFIED, the `dm` run: the life after the restart carried
  `eFlags` 24, the intermission's word, where retail's carries 16 against
  the intermission's 24. Cause: `ClientSim::respawn` flipped the teleport
  bit only for a player spawn and pinned the spectator and intermission word
  at 24. VERIFIED, from the three committed captures read spawn for spawn:
  retail's word alternates on *every* spawn, the connect's `spawnSpectator`
  and the intermission camera included, which is why the `sd` target's
  post-death spectator frame reads 16 where its first one read 24 and why
  two of its consecutive lives both read 24
  (`mp_carentan-sd-roundrestart-target.txt`, `!trace ms=335`, `ms=25436`,
  `ms=28607` and `ms=118532`). INFERRED, from that rule against the `dm`
  capture's restart, whose spectator frame repeats the intermission's 24
  instead of flipping: a level boundary clears the bit with the rest of the
  playerstate, so the incoming level's first spawn sets it. vcod: every
  mode's spawn consumes a flip, the wire word is the bit, and a restart lets
  `enter_world` build a sim with the bit clear.
- Closed. VERIFIED, the `dm` run: the intermission frames read `viewangles`
  0 on all three axes where retail's read a yaw of 90 (`mp_carentan`'s
  `mp_deathmatch_intermission` heading); the origins agree. `PM_UpdateViewAngles`
  returns at `pm_type` 5 and a spawn's `SetClientViewAngle` writes the field
  (`docs/protocol-1.1.md`, "View angles"); `to_wire` writes the sim's view
  in every mode, so the camera reads the spawn's 0, 90. Whether a retail
  client's intermission camera faces the same way is a pending hand check.
- Open. VERIFIED, the `dm` run: the `loadingnewmap` line and the gamestate
  reached the probe in the same millisecond, where retail left 1493 ms
  between them. INFERRED, from `Server::spawn_server`: it queues the line
  into the outbox that `main.rs`'s loop flushes after the tick, with the map
  read and load inside that tick and no 3 step 4 sleep, so the line leaves
  after the load and a client spends none of it on a loading screen.
  Cosmetic on a map that loads in a second. CoD2's and CoD4's
  `spawn_server` (`cod2/cycle.rs`, `cod4/cycle.rs`) already flush the line
  and sleep `SPAWN_SLEEP_MS` before the load.
- Closed. VERIFIED, the `sd` run: the announcer command `s 4` went out with
  no `d 528` naming the alias before it, where retail sends `d 528` then
  `s 4`. VERIFIED, a debug probe against the same server: configstring 528
  was empty in vcod's gamestate. Cause: `send_configstring_update` had
  callers on the restart path only. `Server::broadcast_configstring_changes`
  diffs the table against the copy the clients last heard after every script
  frame and sends `d <index> <text>` for each slot that moved (the broadcast
  3.1's compare pair lets through while `sv.state` reads 2). It runs ahead
  of the frame's client commands, so the `d` naming an alias precedes the
  `s` that plays it, and both level boundaries re-sync the copy instead of
  replaying a whole table slot by slot.
- Closed. VERIFIED, a debug probe against a server started with
  `--set g_gametype=sd`: configstring 0 read `g_gametype\dm` while the `sd`
  scripts ran, which is why the `sd` run's fixtures came out named for `dm`.
  Cause: `Server::new` stamped the table from the config before `main.rs`
  replayed the `--set` list. `Server::set_cvar` rewrites slot 0 from the
  serverinfo string when it mirrors a cvar that string carries, as 4.3's
  flush follows a serverinfo write with
  `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`.

### 8.3 What the runs could not reach

VERIFIED, the `sd` run: both probes answered the team menu and never left
it, reading `team` 3 and `pm_type` 4 for the whole run, where the same probe
against retail had its weapon menu 50 ms after its team menu and spawned.
INFERRED, from `sd.gsc`'s `Callback_PlayerConnect`, whose `openMenu` comes
before the `spawnSpectator` that calls `updateTeamStatus`, and from that
function opening with `wait 0`: the `t 0` was on the wire while the connect
thread was suspended, and an answer arriving inside that frame notified an
event no thread was parked on and was lost. INFERRED, from 4.4's
`VM_Call(gvm, 3, clientNum)` against vcod's ordering: retail raises
`ClientBegin` inside `SV_ExecuteClientMessage`, before `G_RunFrame` advances
`level.time`, so the callback's `wait 0` is due in that frame's thread pass;
vcod dispatched the event at the top of `G_RunFrame`'s counterpart, a frame
late. Closed: `ScriptRuntime::run_frame` opens with a packet pass that
dispatches the netcode's events on the previous frame's clock, each callback
running to its first suspend; what they woke runs in the frame's first pass
(`cod11-gsc-language.md`, "The packet pass"). The callback is parked on
`menuresponse` before its `t 0` can be answered, and the `sd` gates answer
the menu the instant it opens. A waiter a later-started thread woke is taken
by the frame's pass, not resumed on the previous frame's clock, which
`probe_notify_frame` measures (`cod11-gsc-language.md`, the thread pick
paragraph).

So the `sd` half is still unmeasured against retail: the elimination-driven
restart, the team scores, the win announcements and the weapons a client
keeps across a restart need a fresh capture now that the menu is
answerable.

The `dm` rotation was two maps and never wrapped, so the wrap back to the
first entry is measured by the gate, not by a capture.

### 8.4 The gates

This section is a measurement, so it has no "as implemented". What holds
the two shapes to retail on every test run is
`crates/server/tests/mapchange_ab.rs` and
`crates/server/tests/roundrestart_ab.rs`, which replay the committed
captures. The `savePersist` gate is the three `probe_persist_*` cases in
`crates/server/tests/semantics_ents.rs`. The rotation grammar and the
serverId nibbles are unit tests in `crates/server/src/console.rs` and
`crates/server/src/engine/rotation.rs`.

## 9. `games_mp.log`

The same file on 1.1d, 1.5b, United Offensive 1.51b and CoD2 1.3, and on
CoD4 1.7 (`cod4-server-gaps.md`, "games_mp.log").

- `G_LogPrintf` (1.1d `game.mp.i386.so` 0x6010c) writes `%3i:%i%i ` (file
  0x756d5) with the minutes and the two digits of the seconds of
  `level.time`, then the text, to the open file and nowhere else. VERIFIED.
  1.5b's, UO's and CoD2's format is `%3i:%i%i %s` (cod2_lnxded file
  0x112aed). VERIFIED (the strings).
- `G_InitGame` opens it and writes the 60-dash separator and `InitGame: %s`
  (1.1d file 0x755fe) with the serverinfo cvars; `G_ShutdownGame` writes
  `ShutdownGame:` (0x756a2) and the separator, or on a `map_restart`
  (`G_ShutdownGame(1)`) `RestartGame:` (0x75694), then closes it. VERIFIED
  (the strings). 1.5b's and UO's modules carry `RestartGame:` too (1.5b file
  0x7ad14, UO 0xa12f4); CoD2 1.3 and CoD4 1.7 have only `ShutdownGame:`
  (cod2_lnxded file 0x112abd). VERIFIED (the strings).
- The retail 1.1d run (tdm on mp_carentan, `g_log games_mp.log`,
  `g_logSync 1`; one probe joined axis, said `hello log`, team-said `team
  log`, told slot 0 `private` and ran `kill`; rcon `map_restart`, `map
  mp_harbor`, `quit`) wrote: separator, `InitGame:
  \g_gametype\tdm\gamename\main\mapname\mp_carentan\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\0`,
  `J;0;alpha`, `say: alpha: hello log`, `sayteam: alpha: team log`,
  `tell: alpha to alpha: private`, the script's `K;` line, `RestartGame:`
  and the separator at the restart's clock, then separator, `InitGame:` and
  `J;` again, `ShutdownGame:`, separator, separator, mp_harbor's
  `InitGame:`, `J;`, and at the `quit` `ShutdownGame:` and the separator.
  VERIFIED live. The stamps ran on across the three levels (`0:28` at the
  restart, `0:37` at the map, `0:50` at the quit), so `level.time` starts at
  the server's clock there, as on CoD4. VERIFIED live.
- 1.1's chat lines: `G_Say` (0x46c14) logs `say: %s: %s` (file 0x739ae) or
  `sayteam: %s: %s` (0x739dd) with the cleaned name, a tell nothing;
  `Cmd_Tell_f` (0x47210) logs `tell: %s to %s: %s` (0x73a34) ahead of its
  two `G_Say`s. VERIFIED (the strings; the run's lines). 1.5b's are
  `say;%d;%d;%s;%s` (file 0x786ed), `sayteam;`, `saysquad;` (0x7879f) and
  `tell;%d;%d;%s;%d;%d;%s;%s` (0x787ea), UO's the same (0x9ea72, 0x9eb6f),
  CoD2's `say;` and `sayteam;` (cod2_lnxded file 0x110f1e, 0x110f51) and no
  `tell;`. VERIFIED (the strings). The first `%d` is the GUID, 0 on a LAN
  server with no authorize, the second the slot: CoD2's run logged
  `say;0;0;alpha;one`. VERIFIED live. Which name a 1.5 `tell;` line carries
  is INFERRED (1.1's raw `netname`).
- `Touch_Item` logs 1.1's `Weapon: %i %s` (file 0x74d06) or `Item: %i %s`
  (0x74d15) (`cod11-items.md`), 1.5b's `Weapon;%d;%d;%s;%s` (0x79f86) and
  `Item;` (0x79f9a), UO's (0xa0406) and CoD2's (cod2_lnxded file 0x1123a6)
  the same. VERIFIED (the strings; 1.5b's logs read
  `Weapon;0;0;vcod;fraggrenade_mp`).
- The CoD2 1.3 run (tdm on mp_toujane, the same sets; one player joined,
  said three lines 100 ms apart and ran `kill`; rcon `map_restart`, `map
  mp_carentan`, `quit`) wrote one `say;` line (the flood window took the
  other two), `ShutdownGame:` at the restart as at the map change, and
  `InitGame:` without `scr_friendlyfire` and `scr_killcam` on the first
  level and with them on the next two. VERIFIED live.

vcod: the game queues each line with `level.time` (`GameHost::log_printf`)
and each engine writes them after each frame through
`engine::gamelog::LevelLog`, opened at its level load under
`<fs_homepath>/<fs_game or the base game>`. On the runs above vcod-server
wrote the same lines in the same order (1.1: `sv_hostname` aside; the
probe's own lines a second earlier, its join being faster). Its CoD2 first
level's `InitGame:` lists `scr_friendlyfire` and `scr_killcam`, which
retail's gametype adds to the serverinfo only after the line.
