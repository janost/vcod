# CoD2 1.3 map cycle: map change, restarts, rotation, the gametypes' level ends

How a CoD2 1.3 server changes and restarts its level, and how the stock
gametypes end a round or a map, read off `cod2_lnxded` 1.3 (addresses are
its own) and checked against retail captures. CoD1's counterpart is
`cod11-map-cycle.md`; the wire side of a map change is
`cod2-protocol.md` 1.8 and 1.9.

## 1. The engine

### 1.1 `SV_SpawnServer` (0x8093520)

On a running server, for every client past `CS_PRIMED` (state above 2) it
sends the OOB `loadingnewmap\n<map>\n<g_gametype>` and then sleeps 250 ms
(`NET_Sleep(0xfa)`). VERIFIED (the format string, the `2 <` test, the
constant). It reads `G_GetSavePersist()` before `SV_ShutdownGameProgs` and
hands it to `SV_InitGameProgs`, so a level that called
`exitLevel(true)` or `map_restart(true)` carries `game[]` and `pers[]`
into the next. VERIFIED (the call order). It frees the configstrings,
draws a new checksum feed (`Sys_Milliseconds ^ rand() << 16 ^ rand()`),
loads the map, steps `sv_serverid` by 0x10 (and by 0x10 again when the
high nibble wraps to 0; the dvar starts at 0, so the first level is 16),
sets `nextmap` to `map_restart`, runs three 100 ms frames, builds the
baselines, and calls `ClientConnect` for every client past `CS_ZOMBIE`,
setting it to `CS_CONNECTED` (2). VERIFIED (the stores and calls). It
flips `DAT_08423088 ^= 4`, the snapshot flag bit the client reads as a
new server count. VERIFIED (the store).

### 1.2 `SV_MapRestart` (0x808be3e)

`map_restart` is `SV_MapRestart(0)` (0x808c0a8) and `fast_restart` is
`SV_MapRestart(1)` (0x808c0bc). VERIFIED (`Cmd_AddCommand` in
`SV_AddOperatorCommands`). The in-place arm runs when the argument is
set, `g_gametype` (after `SV_SetGametype`) equals the running level's and
`sv_maxclients` is unmodified; otherwise `G_SetSavePersist(0)` and
`SV_SpawnServer(mapname)`. VERIFIED (the three tests and the else arm).
The in-place arm, once per frame (`DAT_081a21b8 != DAT_08440208`):
`Dvar_ResetScriptInfo`, `SV_InitArchivedSnapshot`, the snapshot flag bit
flipped, `sv_serverid = ((id + 1) & 0xf) + (id & 0xf0)`, the game
restarted with the persist flag (0x809267e), three 100 ms frames, then
for every client past `CS_ZOMBIE`: `B` (persist 0) or `n` (persist 1) by
`SV_AddServerCommand`, `ClientConnect`, and for an active one (state 4)
`SV_ClientEnterWorld`. No `SV_FreeConfigstrings`, no new baselines.
VERIFIED (the stores and calls).

The script's `map_restart(persist)` (`GScr_MapRestart` 0x8116a3c) stores
the flag and runs `fast_restart` (cod2-protocol.md 1.9). VERIFIED.
`exitLevel` (`GScr_ExitLevel` 0x8116b6a) reaches `ExitLevel`
(0x81098b8), which queues `map_rotate`, zeroes both team scores and
every connected client's score, moves each connected client back to
connecting, and logs `ExitLevel: executed`. VERIFIED (the calls and
stores). vcod's builtin is 1.1's, the same but for the connecting
state.

### 1.3 `SV_MapRotate_f` (0x808c132) and `SV_Map_f` (0x808bd46)

`sv_mapRotationCurrent` is refilled from `sv_mapRotation` when empty, and
again when its first word reads nothing; each word comes off it through
`Com_Parse` (0x808c0d0), which writes the rest back. `gametype <g>` sets
`g_gametype` (and `G_SetSavePersist(0)` when it differs from the running
one) and reads on; `map <m>` runs `map <m>`; any other word prints
`Unknown keyword` and is skipped. No map, or a keyword with nothing after
it, ends in `fast_restart` ("forcing map_restart" in the message).
VERIFIED (the branches and the strings). Each ending prints its own line:
`No map specified in sv_mapRotation - forcing map_restart.` (0x81494e0),
`No gametype specified after 'gametype' keyword in sv_mapRotation -
forcing map_restart.` (0x8149540), `No map specified after 'map' keyword
in sv_mapRotation - forcing map_restart.` (0x81495c0); a skipped word
`Unknown keyword '%s' in sv_mapRotation.` (0x8149640). VERIFIED (the
strings in `.rodata`, each followed by its newline). vcod logs the three
through `engine::rotation::End::warning`.

The word reader (0x808c0d0) is `Com_Parse` (0x80b6db2), which runs
0x80b67f8 on session 0 (0x817e420) with `allowLineBreaks` 1. VERIFIED: that
session's flags in `.data` (0x817e820) read 1 at +0x405 (space-delimited)
and 0 at +0x406 (keep quotes) and +0x407. INFERRED, off 0x80b67f8's
branches on them: it is 1.1's reading (cod11-map-cycle.md 5.1) with one
difference, `\\` as well as `\"` unescaped inside quotes. Blanks
(bytes up to a space as a signed `char`, 0x80b64a0) and comments are
skipped, a quoted word ends past its closing quote, a bare word on the
blank that ended it, and the rest is written back from there, delimiter
and all. VERIFIED live (`unshare -rn`, `+set sv_mapRotation gametype dm
map mp_burgundy map mp_carentan +map_rotate +map_rotate`): the second
`map_rotate` printed `"sv_mapRotationCurrent" is:" map mp_carentan"`.

`map` and `devmap` lower the name, need `maps/mp/<name>.d3dbsp` to read
(else `Can't find map`), run `SV_SpawnServer`, then set `sv_cheats` to 1
for `devmap` and 0 for `map`. VERIFIED.

### 1.4 Stale server ids (`SV_ExecuteClientMessage` 0x809114e)

A message whose server id is the current one (or that comes during a
download) is read. One whose high nibble matches (sent before a
`fast_restart` reached the client) enters a `CS_PRIMED` client into the
world and is otherwise dropped. Any other id resends the gamestate when
the message acknowledges past the last gamestate's message. VERIFIED.

### 1.5 The intermission and the stock config

`MoveClientToIntermission` (0x80f6818), which `spawnIntermission` reaches,
sets `pm_type` 5 and writes configstring 5 (axis score) and 6 (allies
score). VERIFIED (the stores; cod2-protocol.md 6 names the two slots).

`default_mp.cfg` (in the iwds) sets the stock `scr_*` values, among them
`scr_dm_scorelimit 50` where `dm.gsc`'s own default is 100. VERIFIED (the
file; retail's dm sent `cg_objectiveText` with 50, section 2).

## 2. Retail captures

2026-10-22, retail `cod2_lnxded` 1.3 (`sv_pure 1`, `sv_maxclients 8`) on
mp_toujane inside `unshare -rn`, two `vcod --net-probe <addr> --game cod2
--probe-team allies|axis` clients with `RUST_LOG=vcod::play::cod2=debug`
(the client logs every server command, each gamestate and each
`loadingnewmap` with its local ms), rcon lines sent on a clock. VERIFIED
live, each line:

- dm, rcon `fast_restart`, `map_restart`, `map mp_carentan`, then
  `sv_mapRotation "gametype tdm map mp_toujane"` and `map_rotate`:
  `sv_serverid` read 16, 17, 33, 49, 65. Snapshot `serverTime` ran on
  through every change, 300 ms ahead of the wall clock after each (the
  three spawn frames). `fast_restart` sent, in one message, `d 13`
  (level start time), `d 3` (the ambient with that time), `d 12` (fog,
  unchanged but written again), `B`, `d 1` (the new `sv_serverid`), then
  the connect callbacks' commands; no gamestate. `map_restart`, `map` and
  `map_rotate` sent `loadingnewmap mp_toujane dm`, `loadingnewmap
  mp_carentan dm` and `loadingnewmap mp_toujane tdm`, each followed by a
  gamestate about 1.0-1.2 s later.
- tdm, `scr_tdm_timelimit 0.25` and `sv_mapRotation "gametype dm map
  mp_carentan"`: at the limit `MP_TIME_LIMIT_REACHED`, the draw
  announcer, `cg_objectiveText` `MP_THE_GAME_IS_A_TIE`, then `d 5 0` and
  `d 6 0` (the intermission); `ExitLevel: executed` and
  `loadingnewmap mp_carentan dm` 10.0 s later.
- sd: with one player a team, `MP_MATCHSTARTING`, 7.0 s later `H 0`,
  `G 0` (`resetScores`), 0.1 s later the restart: `d 13`, `d 3`, `d 12`,
  `H 0`, `G 0`, `n`, `d 1`.
- 2026-10-23, the same server on tdm mp_toujane with one `spectate
  --join` client, rcon `map mp_carentan`, `fast_restart`, `map_restart`:
  after each gamestate and after the `B`, the client's first commands
  were `t 4` (`serverinfo_tdm`) with `g_scriptMainMenu "ingame"` and
  the client a spectator (`pm_type` 4 in the snapshot after the `B`).
  VERIFIED. No team menu came until the client answered `mr <id> 4
  close`; then `t 1` (the `team_` menu) came, and after its
  `autoassign` `t 2` or `t 3` (the weapon menu). VERIFIED. This is
  `_menus.gsc`'s serverinfo `close` arm opening the team menu, and
  `Callback_PlayerConnect` opening the serverinfo menu for a client
  with no `pers["team"]`. VERIFIED, the stock iw_15 scripts. The `B`'s
  `d 1` came ahead of the `t 4`, logged at the same millisecond.
  VERIFIED.

## 3. vcod

`cod2::Cod2Server` (`crates/server/src/cod2/cycle.rs`) runs `map`,
`devmap`, `map_restart`, `fast_restart` and `map_rotate` from the console
and rcon, and the level's queued `fast_restart` and `map_rotate`; the
shared `map_restart` builtin queues `fast_restart` on CoD2. Section 1's
id steps, persist carry (`Cod2Level::take_carry`: `game[]` and `pers[]`
under `savePersist`, the compiled closures on a restart, the gametype's
compile on any load, as on CoD1: cod11-map-cycle.md 4.7), stale-id arms,
snapshot flag bit and the intermission's configstrings 5 and 6 are
implemented; the dvar table is one (a level's `setCvar` survives the
boundary), `default_mp.cfg` is exec'd at startup
(`Cod2Server::exec_default_cfg`), `--set g_gametype=` picks the load's
gametype, and `--gametype-script` overlays a gametype off disk as on CoD1.

The in-process gates (`crates/server/src/cod2/cycle_tests.rs`, `COD2_DIR`)
run two vcod clients through section 2's sequences and assert the ids,
the `loadingnewmap` lines, `B` before `d 1`, `n` for sd, both clients
playing after each change, the tdm time limit to the rotation's next map,
sd's bomb planted and defused, a ctf capture and an hq radio; two bots
play again after each of `map`, `map_restart`, `fast_restart`,
`map_rotate` and a passed `callvote map`, and through sd's `n`. The same
live recipe against vcod-server gave the same ids, the same 300 ms steps,
the tdm `loadingnewmap` 10.2 s after the intermission and the sd restart
7.0 s after `MP_MATCHSTARTING`.

What section 2's comparison found and how vcod now matches it:

- `SV_SetConfigstring` (0x8092780) sends a changed slot to every client
  past `CS_CONNECTED` at once, as `SV_GameSendServerCommand` sends a
  command, so the two interleave in write order. VERIFIED (the `2 <`
  test and the send inside the setter). vcod stamps each game
  configstring write and each queued command from one counter
  (`configstrings::next_write_seq`, `game::commands::ClientCommands`) and
  `Cod2Server::flush_game_commands` sends them in that order: the tdm
  intermission's `d 5 0` / `d 6 0` follow its `f` and `v` lines
  (`the_intermission_scores_follow_the_level_end_s_lines`).
- `SV_AddServerCommand` (0x8094750) takes a pending `d` of the same index
  out before it appends (`SV_FindPendingCommand` 0x8094530; 0x80944aa
  compares the first word of each). VERIFIED. `G_InitGame` writes the fog
  `""` (0x8109096 calls 0x80fe4bc with the empty string) and the map's
  `_fx` sets it back, so a restart sends one `d 12` with the unchanged
  value, where the second write stood. INFERRED from the two writes and
  the squash. vcod squashes `d` by index the same way.
- `setTeamScore` sends `H`/`G` through `SV_GameSendServerCommand(-1, ..)`
  (`GScr_SetTeamScore` 0x8115dda), the engine's clients, not the level's.
  VERIFIED. vcod queues it to `ALL_CLIENTS`, which the engine fans out, so
  the restart's `resetScores` reaches the clients the restarted level has
  not yet connected.
- Configstring 0 and 1 go out at the head of `SV_Frame` (0x80969b0, the
  `0x404` / `8` dvar-modified bits) before the frame's game runs, so a
  `fast_restart`'s `d 1` lands after its `B` and before the connect
  callbacks' commands. VERIFIED (the order in `SV_Frame`). vcod
  broadcasts them at the head of `engine::tick`.
  `a_restart_sends_retail_s_burst_in_write_order` gates dm's
  `d 13, d 3, d 12, B, d 1` and sd's `d 13, d 3, d 12, H, G, n, d 1`.
- Configstring 7 is written whole by 0x8106ec0, at `G_InitGame`'s tail
  past `level.initializing` and from `G_RunFrame` when `G_RegisterWeapon`
  (0x8107216) set its dirty word 0x864f97c; configstring 8 likewise
  (0x8106f7c, word 0x864f980). VERIFIED. vcod writes both from
  `builtins::cod2::write_weapon_configstrings` at the bootstrap's end and
  after each frame, so a restart that registers the same list sends no
  `d 7`.
- `SV_SpawnServer` writes the `loadingnewmap` lines to the socket and then
  `NET_Sleep(250)` (0x80d5cc4, a `select` on the socket that a packet ends
  early). VERIFIED. vcod-server sends them through `Cod2Server::set_net_now`
  and waits 250 ms, or until its reader thread reads a packet
  (`a_map_change_sends_loadingnewmap_then_sleeps`).

Not live-checked after these changes: the gates above replay section 2's
sequences in process only.
