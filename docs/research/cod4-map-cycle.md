# Call of Duty 4 1.7: the map cycle, votes and the menu answers

How a 1.7 server ends a level and starts the next one (`map`, `devmap`,
`map_restart`, `fast_restart`, `map_rotate`), how a client follows it, the
engine vote (`callvote`, `vote`, `CheckVote`), and one rule of the menu
answers. vcod-server's CoD4 side is `crates/server/src/cod4/cycle.rs`,
the vote `crates/server/src/game/vote.rs` under `VoteRules::COD4`; the
client's half is `vcod_cod4::net::conn` and `play::cod4`.

## Sources

- `cod4_lnxded`, md5 `bf8448740b3f4cb74c2254f01af14cdd` (the binary of
  `cod4-handshake.md`), read in `objdump -d`. Addresses are virtual (file
  offset plus 0x8048000). Names are the Q3 lineage's. A claim labelled
  INFERRED that names no 1.7 address is not yet confirmed on 1.7.
- Two live runs on loopback mp_crash `war` inside `unshare -rnm`
  (`cod4-handshake.md` 7) with `cod4_probe` joined: `fast_restart` then
  `map mp_backlot` over rcon (the cycle run), and the menu answers of
  section 5 (`play-1.7-menus-together.txt`).

## 1. The spawn: `map`, `devmap`, `map_rotate`'s map

- `SV_SpawnServer` is 0x81748da. It sends `loadingnewmap\n%s\n%s` (the
  map, `g_gametype`; 0x82319ed, xref 0x81751f0) to every client past
  `CS_CONNECTED` through `NET_OutOfBandPrint` (0x813d066), sleeps
  `NET_Sleep(250)` (0x817525a) whether or not a client was told, loads,
  and sends `requeststats\n` (0x82310a8, xref 0x8174ee7) to every client
  still connected. VERIFIED (the strings and xrefs).
- The order around them (clients put back to `CS_CONNECTED` through
  `ClientConnect`, `statPacketsReceived` zeroed, `sv_serverid`'s high
  nibble stepped) is INFERRED (`SV_SpawnServer`).
- Live, `map mp_backlot` under a playing probe: `loadingnewmap\nmp_backlot
  \nwar` at 31065 ms, then `requeststats` nine times at 31448 ms (one a
  client message while the stats are missing), the seven stats packets
  answered `statResponse` down to 0, then a new gamestate. VERIFIED (the
  cycle run).
- vcod-server writes the lines to the socket and waits the 250 ms before
  the load, as its CoD2 server does (`Cod4Server::set_net_now`); a test's
  server without a socket leaves them in its outbox. VERIFIED
  (`cod4_a_map_change_sends_loadingnewmap_then_sleeps`).
- `SV_Map_f` lowers the name, refuses one without a zone with `Can't find
  map "%s".` (0x8230410), spawns, then sets `sv_cheats` to 1 for `devmap`
  and 0 for `map`. The string is VERIFIED; the rest INFERRED.
- `savePersist` carries `game[]` and every `pers[]` into the spawned
  level, as on CoD1 (`cod11-map-cycle.md` 1). INFERRED
  (`G_GetSavePersist` in `SV_SpawnServer`).
- vcod-server walks the five common zones once and only the map's zone
  on a spawn, and resumes the last gametype's script compile when its
  files read the same (`Carry::gametype`). vcod's choice, not retail's.

## 2. The restart: `fast_restart`, `map_restart`

`SV_MapRestart` is 0x816c306 (`fast_restart` passes 1, `map_restart` 0).
Its arms:

| Case | What it does | Label |
|---|---|---|
| argument 0, or `g_gametype` differs from the running one (compare at 0x816c3cf) | spawns the map instead (section 1) | VERIFIED |
| `sv_maxclients` modified | spawns the map; vcod's fixed slot count never meets it | INFERRED |
| level already started or restarted this frame (`com_frameTime` against `sv.start_frameTime`, 0x816c3dc) | nothing | VERIFIED |
| otherwise | OOB `fastrestart` (0x822ffec) to every client past `CS_ZOMBIE`; flips the snapshot's server-count bit (`xorl $4`, 0x816c45f); steps `sv_serverid`'s low nibble, keeps the high one (0x816c466); restarts the game; runs three frames 100 ms apart; per client past `CS_ZOMBIE` queues `n` (`savePersist`) or `B` (`0x6e` / `0x42`, 0x816c540), runs `ClientConnect`, and for one in the world (state 4) `SV_ClientEnterWorld` | VERIFIED |

- Live, rcon `fast_restart` under a playing probe: `fastrestart`, then
  `B` and `d 1 <systeminfo>` with the new id. VERIFIED (the cycle run).
- A message whose `serverId` shares the high nibble with the running id
  is from before a `fast_restart`: nothing in it is read, and a loading
  client enters the world. One from another level runs its opening
  client commands as from the old server, then gets the gamestate again
  once it acknowledges past the last one. INFERRED
  (`SV_ExecuteClientMessage`).
- vcod reads only the opening commands of such a message: its usercmds
  are keyed to the last level's checksum feed and decode as garbage
  (reading them dropped the client with `EXE_LOSTRELIABLECOMMANDS`).
- `SV_RestartGameProgs` keeps the configstring table; the new level's
  writes go out as `d`. INFERRED. vcod hands the old table to the new
  script host, whose indexers find the same names in the same slots.

## 3. `map_rotate`

- `SV_MapRotate_f` is 0x816dae2. It prints `map_rotate...`, then
  `"sv_mapRotation" is:"%s"` and `"sv_mapRotationCurrent" is:"%s"`;
  refills `sv_mapRotationCurrent` from `sv_mapRotation` when it is empty
  and again when its first word reads none; sets `g_gametype` on each
  `gametype <g>` (`Setting g_gametype: %s.`, dropping `savePersist` when
  it changes); skips any other word with `Unknown keyword '%s' in
  sv_mapRotation.`; and on `map <m>` prints `Setting map: %s.` and runs
  `map <m>`. A walk that ends without a map prints one of its three
  `... - forcing map_restart.` lines and calls `SV_MapRestart(1)`
  (0x816c306), a `fast_restart`. VERIFIED (the code and every string).
  The walk and the strings are CoD2's (`cod2-map-cycle.md` 1.3); vcod
  runs `crate::engine::rotation`'s.
- The stock script's intermission ends in `exitLevel(false)`
  (`_globallogic.gsc` `endGame`, 30 s past the intermission's start at
  `scr_intermission_time` 30), which queues `map_rotate`. INFERRED (the
  script).
- On ours a one-minute `war` match ends, rotates to the next map (or,
  with a rotation naming no map, restarts in place), and both players
  hurt each other again there. VERIFIED
  (`cod4_damage_survives_the_time_limit` and `..._restarted`).
- `default_mp.cfg` (in `localized_english_iw00.iwd`) execs
  `server_map.cfg` (in `iw_00.iwd`), which sets `sv_mapRotation` to 15
  stock maps (`map mp_backlot map mp_bloc ...`, no `gametype`), so a
  server with no rotation of its own rotates through them under its
  gametype. VERIFIED (the iwds). vcod-server runs the same config.

## 4. The votes

`Cmd_CallVote_f` (0x80adbf2), `Cmd_Vote_f` (0x80ada2a) and `CheckVote`
(0x80b423a) are CoD2's (`cod2-protocol.md` 3.4.2): `g_oldVoting` (the
dvar at 0x84bc36c, its name VERIFIED in the binary) picks the engine
vote or the script's notifies (0x80bf8b0, 0x80bf87e), as on CoD2. The
engine vote differs from CoD2's in three ways (code and strings at
0x8219321..0x8219b7b):

- A call needs two clients connected (`level.numConnectedClients < 2`
  answers `GAME_VOTINGNOTENOUGHPLAYERS`), tested right after
  `g_allowVote` under either `g_oldVoting`. Then, under the engine vote,
  a running vote, `voteCount > 2` (`GAME_MAXVOTESCALLED`) and a
  spectator caller are refused, in that order. VERIFIED. That nothing
  writes the count is INFERRED.
- The `map` and `typemap` kinds test the map only when the dvar at
  0x88a53f0 is 0. VERIFIED. That dvar is `useFastFile` (the fast file
  test), on in every 1.7 build, so any map name passes the call and a bad
  one fails at the `map` command. INFERRED.
- Configstring 13 is `%i %i`, the vote's end in `level.time` and
  `sv_serverId`; 14 the display string, 15 and 16 the counts. VERIFIED.

The verdict at the end is CoD2's: `yes > no + ceil((voters - yes - no) *
g_voteAbstainWeight)` (the `frndint` under rounding control 10b at
0x80b42a8, then a truncating `fistp`). While it runs, a majority of
`voters / 2 + 1` passes and a `no` count past `voters - majority` fails
it. VERIFIED.

The kinds and their commands (`map_restart` runs `fast_restart`,
`map_rotate`, `map %s`, `g_gametype %s; map_restart`, the three `typemap`
forms, `clientkick "%d"` and `tempBanClient "%d"`), the display strings
(`GAME_VOTE_MAPRESTART`, `GAME_VOTE_NEXTMAP`, `GAME_VOTE_MAP\x15%s`,
`GAME_VOTE_GAMETYPE\x14%s`, `GAME_VOTE_KICK\x15(%i)%s`) and the usage
line are CoD2's. VERIFIED (the strings). The gametypes are
`maps/mp/gametypes/_gametypes.txt`'s, each named by the first token of
its `<name>.txt`. INFERRED (`Scr_ParseGameTypeList_FastFile`).
`ClientCommand` takes only `say`, `say_team` and `score` while the
client's `pm_type` is `PM_INTERMISSION`. INFERRED.

`tempBanClient` and `tempBanUser` drop with `EXE_PLAYERKICKED`, print
`%s (guid "%s") was kicked for cheating` and hold the GUID out for
`sv_kickBanTime` seconds (300) with `You are temporarily banned from this
server`. The strings are VERIFIED. Where 1.7 tests the ban is not read;
vcod tests it at the connect (`Cod4Game::temp_bans`). INFERRED
(`SV_BanGuidBriefly`, `SV_IsTempBannedGuid`).

On ours a `map_restart` vote that the second client's yes passes sends
`GAME_VOTEPASSED` and restarts 3 s on, and a `map` vote spawns the map
with both back in. VERIFIED (`cod4::vote_tests`).

## 5. A team and a class answer in one packet

The team menu's answer and the class menu's in one client message lose
the class: the player stays on the team as a spectator and never spawns.
Seen live on a first join (`mr 16 8 axis` and `mr 16 15 assault_mp,0` in
one packet), back from the spectators (`allies` and `specops_mp,0`) and
on a team swap (`axis` and `sniper_mp,0`). VERIFIED (the menus run;
`play-1.7-menus-together.txt` holds the second and third).

The team answer's notify moves `_menus.gsc`'s `onMenuResponse` thread
into the time array for the next thread pass, so the class answer, read
in the same message, finds no thread waiting on `menuresponse`. INFERRED
(`VM_Notify`; threads run only from `G_RunFrame`). Ours does the same
(`cod4_a_class_answered_with_the_team_is_lost`). A client clicking the
two menus sends them apart.

## 6. The client

- `CL_ConnectionlessPacket` answers `loadingnewmap` by closing the menus
  and re-sending its stats, `requeststats` by starting `CA_SENDINGSTATS`
  unless already sending, and `fastrestart` by marking the server as
  restarting. INFERRED.
- vcod's connection keeps `loadingnewmap`'s map until the gamestate,
  sends the seven stats packets once a second at most on `requeststats`
  and the missing ones on a non-zero `statResponse`, answers every new
  gamestate with its own `cp`, and applies each `d <index> <string>`
  newer than the table to the gamestate (so a `fast_restart`'s `d 1`
  moves the next message's `serverId`). On retail the probe took the new
  gamestate after `map mp_backlot`. VERIFIED (the cycle run).
- A gamestate naming another map makes vcod load it as the start-up does
  (`cod4_fly::load` over the gamestate's models and weapons) and start
  the level's prediction, event rings and menus over; one for the same
  map keeps the world. VERIFIED on ours
  (`cod4_live_map_change_names_the_new_map_and_spawns_there`). How the
  drawn map looks across the change is pending a hand check.
