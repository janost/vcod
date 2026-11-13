# Retail server handshake and empty-map configstrings (CoD 1.1)

What the retail 1.1d Linux dedicated server sends before any player exists:
the three connectionless replies, the gamestate framing and the whole
configstring table for two stock maps. vcod-server copies its constants from
these captures.

Everything here is **VERIFIED live 2026-08-25** against `cod_lnxded` (1.1d)
with `game.mp.i386.so` (`gamedate: Nov 13 2003`) and the 1.5 install's assets,
unless a line says otherwise.

Contents: setup; connectionless replies; the gamestate message; configstrings
at gamestate; what a minimal server has to set; retail client check;
housekeeping (heartbeat, rcon, zombies, frame message order, drops,
reconnects, console commands, the stdin console, pings, rate, bans, shutdown, OOB `disconnect`,
challenge key, `fs_restrict`, private slots, ping limits, challenge table,
pacing, bare messages, fragmented gamestate, `snaps`, `g_password`); pak
checksums and pure servers; CD key authorization; raw captures.

## Setup

```
tools/run_server.sh mp_pavlov      # and again with mp_carentan
```

runs

```
cod_lnxded +set dedicated 1 +set fs_basepath <game install> \
    +set fs_homepath <homepath holding main/game.mp.i386.so> \
    +set net_port 28960 +set sv_maxclients 8 +map <map>
```

with `sv_hostname` `CoDHost` and `g_gametype` `dm` left at default. The
captures ran with `sv_pure 1`, the retail default, which overflowed
configstring 1 (the trap below). The script now sets `sv_pure 0`, so a rerun
does not reproduce the truncation; nothing else in the capture depends on
`sv_pure`.

Connectionless probe:

```python
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(2)
for q in [b"getinfo xyz", b"getstatus xyz", b"getchallenge"]:
    s.sendto(b"\xff\xff\xff\xff" + q, ("127.0.0.1", 28960))
    print(q, "->", s.recv(8192))
```

Gamestate:

```
RUST_LOG=debug cargo run -p vcod -- --net-probe 127.0.0.1:28960 --probe-secs 40
```

A lone spectator got no snapshots in this run, so the probe reached `Active`
and died on its 30 s snapshot-silence timeout, after the gamestate had
arrived.

## Connectionless replies

Each is `FF FF FF FF`, the command word, a newline, then the payload. Bytes
from `mp_pavlov`; only `mapname` differs on `mp_carentan`.

### `getinfo xyz` -> `infoResponse`

```
infoResponse\n\challenge\xyz\protocol\1\hostname\CoDHost\mapname\mp_pavlov\clients\0\sv_maxclients\8\gametype\dm\pure\1\sv_allowAnonymous\0\pswrd\0
```

- The server builds this string itself (`SV_Info`, `cod_lnxded` `0x808c1ac`),
  so the key order is fixed and is not the serverinfo order.
- `challenge` echoes the argument ("The challenge key" below covers what the
  line read does to it, and its absence without one).
- `protocol` is **1**. `gametype` is the short name. `pure` and `pswrd` are
  `0`/`1` flags. `clients` counts connected clients (`0` idle).
- `minPing`, `maxPing` and `game` did not appear. INFERRED from `SV_Info`
  (`0x808c1ac`), not tested: they are written only when
  `sv_minPing`/`sv_maxPing` are non-zero and `fs_game` is set.

### `getstatus xyz` -> `statusResponse`

```
statusResponse\n\g_gametype\dm\gamename\main\mapname\mp_pavlov\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\1\challenge\xyz\pswrd\0\n
```

The serverinfo string (configstring 0, byte-identical) plus
`\challenge\<arg>\pswrd\0` and a newline. One player line per connected client
follows; none here. Keys are in case-insensitive name order, as
`Cvar_InfoString` walks the cvar list: `sv_maxclients` sorts before
`sv_maxPing` and `sv_maxRate`.

### `getchallenge` -> `challengeResponse`

```
challengeResponse -1111288375
```

One signed integer, often negative. Q3's second field (the authorize flag) is
absent. Samples across the two runs: `-1111288375`, `-3172718`, `219667770`,
`-1225130010`.

### `connect` -> `connectResponse`

`connectResponse` with an empty payload on both maps (`oob [Connecting]
connectResponse ""` in the probe log).

## The gamestate message

`reliableAcknowledge` is `0` (plain LE long at bytes 0..4,
`docs/protocol-1.1.md`). The Huffman stream starts directly with
`svc_gamestate` (2) on both maps; no server command came before it or during
the run. `serverCommandSequence` is 0, `clientNum` 0 (first free slot).

The committed `crates/common/tests/fixtures/net/gamestate.bin`, from a
populated modded (CoDaM) server, opens with `svc_serverCommand 0 ""` before
`svc_gamestate`, so that empty command is not stock. `gamestate::parse`
accepts both. Whether it comes from the mod or a reliable command queued
before the gamestate is not established.

| map | non-empty configstrings | baselines |
|---|---|---|
| mp_pavlov | 207 | 43 |
| mp_carentan | 246 | 19 |

The populated `mp_carentan` capture also has 19 baselines: baselines are map
entities (script entities, doors, MG42s), not players.

## Configstrings at gamestate

Indices set on each empty map:

```
mp_pavlov   0 1 2 3 7 8 11 12 13 20 21 22 29 140-180 204-243 269-361 525 781 782
            1180-1187 1212 1213 1245 1246 1501-1505
mp_carentan 0 1 2 3 7 8 11 12 13 20 21 22 29 140-180 204-243 269-399 525-527 781
            1180-1187 1212 1213 1245 1246 1501-1505
```

The empty `mp_carentan` set equals the populated capture's, model and sound
ranges included, so nothing in the gamestate table depends on players. Unset
in both: 5 and 6 (team scores) and 15..18 (vote), which arrive later as `d
<index> <text>`. Unset in every capture: 14 (MOTD, `g_motd` empty), 44+
(locations), 108..139 (attachment tag names), 1100..1115 (shellshock).

### Configstring 0, serverinfo

`mp_pavlov`, 217 bytes:

```
\g_gametype\dm\gamename\main\mapname\mp_pavlov\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\1
```

`mp_carentan` differs only in `mapname` (219 bytes). The populated capture has
the same keys in the same order with `sv_maxclients\20`, `sv_pure\0`.

### Configstring 1, systeminfo

Identical on both maps, 1023 bytes:

```
\bg_fallDamageMaxHeight\480\bg_fallDamageMinHeight\256\g_synchronousClients\0\pmove_fixed\0\pmove_msec\8\sv_cheats\0\sv_pakNames\pak5 pak4 pak3 pak2 pak1 pak0 [...]\sv_paks\77111478 -1825805837 918160098 616334813 1265884747 1048127331 [...] \sv_pure\1\sv_referencedPakNames\main/pak5 main/pak4 main/pak3 main/pak2 main/pak1 main/pak0 [...]
```

Each `[...]` elides the 28 non-stock pak names (map downloads) and their
checksums, in the same order in each list.

**Trap.** The string stops inside `sv_referencedPakNames` at exactly 1023
characters with no `sv_serverid`, so the client reads serverId 0 all session.
`sv_pure 1` wrote the pak lists for all 34 pk3s in `main` into a
`MAX_INFO_STRING` (1024) buffer, and name order puts `sv_serverid` and
`timescale` last. The populated capture's 511-byte systeminfo has
`\sv_serverid\18\timescale\1` after `sv_referencedPaks`.

With `sv_pure 0` the three pak lists are omitted. INFERRED (the populated
capture, not re-captured: `\sv_pure\0`, `\sv_serverid\18\timescale\1`, no
`sv_pakNames` or `sv_paks`). A clean stock install would likely fit either
way.

`+set fs_game <dir>` adds `fs_game\<dir>` between `bg_fallDamageMinHeight` and
`g_synchronousClients`, and the mod's paks lead the lists (`<dir>/<pak>` in
`sv_referencedPakNames`). VERIFIED (live capture against retail 1.1d,
2026-10-11, `cod11-front-end.md` section 18).

A 1.1 systeminfo must stay under 1024 bytes with `sv_serverid` kept, because
`SV_ExecuteClientMessage` gates every client message on it.

Where the cut comes from, and why 1.5 and UO have none:

- 1.1's `SV_SpawnServer` builds the string with the big builder
  (`Cvar_InfoString_Big` 0x806fc84, an 8192-byte buffer at 0x812b620,
  `Info_SetValueForKey_Big` 0x80829e8 with a 0x401c-byte frame), then copies
  it with `Q_strncpyz(buf, info, 0x400)` (0x8083288, the `push 0x400` at
  0x808a897) before `SV_SetConfigstring(1, buf)` (0x808a8c9). That copy is the 1023.
  VERIFIED (disassembly).
- 1.5b copies with `0x2000` (`mov [esp+8],0x2000` at 0x80912c0, the copy
  0x8085e32, `SV_SetConfigstring(1, ...)` at 0x80912f2). VERIFIED
  (disassembly).
- UO's `SV_SpawnServer` (0x8091b72) copies with `0x2000`, and `SV_Frame`'s
  refresh on a systeminfo cvar change sets configstring 1 straight from
  `Cvar_InfoString_Big` (0x80748c0) with no copy. `Info_SetValueForKey_Big`
  (0x8087570) refuses a pair that would pass 8192. VERIFIED (decompile).
- A `sv_pure 1` UO server sends a 1318-byte systeminfo on mp_cassino and a
  1351-byte one on mp_j5outpost, every pak list whole and `sv_serverid` in
  place (`crates/server/tests/fixtures/configstrings/*-dm-uo-pure.txt`).
  VERIFIED.
- The gamestate carries a configstring as a big string, so a long one arrives
  whole. A later change goes through `SV_SetConfigstring`'s per-client half
  (1.1 0x8089bf0, UO 0x8090da8): under 1000 bytes it is `d <index> <text>`;
  from 1000 bytes it is cut into 999-byte pieces sent as `x <index> <piece>`,
  `y` for each middle piece, `z` for the last. This applies to a 1.1
  systeminfo of 1000..1023 bytes too. VERIFIED (decompile; the letters are the
  strings at 1.1 0x80d5244/6/8 and UO 0x80e54e4/6/8).
- The client joins the pieces in `CL_CGameServerCommand` (CoDMP.exe 0x4014e0,
  CoDUOMP.exe 0x401630). `x` starts a buffer as `d <index> "<piece>` (1.1) or
  `d <index> <piece>` (UO, after a three-token tokenize, so the piece is the
  rest of the line); `y` appends; `z` appends, closes the quote on 1.1 and
  runs the result as a `d`. The buffer is bounded at 8192 (`0x1fff` check).
  VERIFIED (decompile).

vcod-server bounds the systeminfo at 1023 on protocol 1 (trimming paks, never
`sv_serverid`) and at 8191 on 6 and 22, and sends every configstring of 1000
bytes or more as `x`/`y`/`z` (`net::configstring_commands`); the client joins
them. The UO gate `a_pure_server_s_systeminfo_matches_uo` matches both pure
captures byte for byte, `sv_serverid` included. It loads the recipe's first
map, then the captured one, since the captures were a second map load (32).

### Identical on both maps, index < 140

| cs | value |
|---|---|
| 2 | `cod` |
| 7 | `bar_mp bar_slow_mp bren_mp colt_mp enfield_mp fg42_mp fg42_semi_mp fraggrenade_mp kar98k_mp kar98k_sniper_mp luger_mp m1carbine_mp m1garand_mp mg42_bipod_duck_mp mg42_bipod_prone_mp mg42_bipod_stand_mp mk1britishfrag_mp mosin_nagant_mp mosin_nagant_sniper_mp mp40_mp mp44_mp mp44_semi_mp panzerfaust_mp ppsh_mp ppsh_semi_mp ptrs41_antitank_rifle_mp rgd-33russianfrag_mp springfield_mp sten_mp stielhandgranate_mp thompson_mp thompson_semi_mp` |
| 13 | `0` |
| 20 | `\winner\0` |
| 21 | `gfx/hud/hud@status_dead.tga` |
| 22 | `gfx/hud/hud@status_connecting.tga` |
| 29 | `gfx/hud/headicon@quickmessage` |

Configstring 7 is byte-identical to the populated capture's: the weapon list
belongs to the game module, not the map or round. 13 is `level.startTime`, `0`
just after the load.

### Map-dependent, index < 140

| cs | meaning | mp_pavlov | mp_carentan |
|---|---|---|---|
| 3 | ambient (worldspawn) | `n\ambient_mp_pavlov\t\0` | `n\ambient_mp_carentan\t\0` |
| 8 | registered-item bits | `1ce0cfb40000000001` | `7df31f0d1000000001` |
| 11 | `northyaw` | `90` | `0` |
| 12 | fog params | `0 6000 1 0.8 0.8 0.8 0` | `0 16500 1 0.7 0.85 1 0` |

All four come from worldspawn or script.

### 140..203 cvar names, 204..267 values

The client pairs them as `Cvar_Set(cs[140+i], cs[204+i])`
(`cod11-hud-protocol.md`); this capture confirms it live and pins the stock
values. The writer is in the engine, not the game module: `cod11-map-cycle.md`
3.2 reads it at `0x808b148` in `cod_lnxded`, walking the cvar list for the
`0x800` flag. Only 179/243 depends on the map. `scr_motd` (180) has an empty
value, which is also where the client's loop stops.

The table seeds `ENGINE_MIRRORED` in `crates/server/src/cvars.rs`: the 21
engine cvars the game module registers with the mirror flag. The mirror is one
name-sorted list of every flagged cvar, with no absolute slots. The 21 engine
names happen to sort ahead of the 20 `scr_*` the stock scripts register, so
the engine set fills 140..160 and the scripts' 161..180. The rest of the
module's cvar table is not mirrored and comes only from the module
(`cod11-gsc-object-model.md` section 18 has the dumper).

| cs name | name | cs value | mp_pavlov | mp_carentan |
|---|---|---|---|---|
| 140 | `bg_duck2prone_time` | 204 | `400` | `400` |
| 141 | `bg_foliagesnd_fastinterval` | 205 | `500` | `500` |
| 142 | `bg_foliagesnd_maxspeed` | 206 | `180` | `180` |
| 143 | `bg_foliagesnd_minspeed` | 207 | `40` | `40` |
| 144 | `bg_foliagesnd_resetinterval` | 208 | `500` | `500` |
| 145 | `bg_foliagesnd_slowinterval` | 209 | `1500` | `1500` |
| 146 | `bg_ladder_yawcap` | 210 | `100` | `100` |
| 147 | `bg_prone2duck_time` | 211 | `400` | `400` |
| 148 | `bg_prone_softyawedge` | 212 | `1` | `1` |
| 149 | `bg_prone_yawcap` | 213 | `85` | `85` |
| 150 | `bg_viewheight_crouched` | 214 | `40` | `40` |
| 151 | `bg_viewheight_prone` | 215 | `11` | `11` |
| 152 | `bg_viewheight_standing` | 216 | `60` | `60` |
| 153 | `g_ScoresBanner_Allies` | 217 | `gfx/hud/hud@mpflag_american.tga` | `gfx/hud/hud@mpflag_american.tga` |
| 154 | `g_ScoresBanner_Axis` | 218 | `gfx/hud/hud@mpflag_german.tga` | `gfx/hud/hud@mpflag_german.tga` |
| 155 | `g_ScoresBanner_None` | 219 | `gfx/hud/hud@mpflag_none.tga` | `gfx/hud/hud@mpflag_none.tga` |
| 156 | `g_ScoresBanner_Spectators` | 220 | `gfx/hud/hud@mpflag_spectator.tga` | `gfx/hud/hud@mpflag_spectator.tga` |
| 157 | `g_TeamColor_Allies` | 221 | `0.5 0.5 1` | `0.5 0.5 1` |
| 158 | `g_TeamColor_Axis` | 222 | `1 0.5 0.5` | `1 0.5 0.5` |
| 159 | `g_TeamName_Allies` | 223 | `GAME_ALLIES` | `GAME_ALLIES` |
| 160 | `g_TeamName_Axis` | 224 | `GAME_AXIS` | `GAME_AXIS` |
| 161 | `scr_allow_bar` | 225 | `1` | `1` |
| 162 | `scr_allow_bren` | 226 | `1` | `1` |
| 163 | `scr_allow_enfield` | 227 | `1` | `1` |
| 164 | `scr_allow_fg42` | 228 | `0` | `0` |
| 165 | `scr_allow_kar98k` | 229 | `1` | `1` |
| 166 | `scr_allow_kar98ksniper` | 230 | `1` | `1` |
| 167 | `scr_allow_m1carbine` | 231 | `1` | `1` |
| 168 | `scr_allow_m1garand` | 232 | `1` | `1` |
| 169 | `scr_allow_mp40` | 233 | `1` | `1` |
| 170 | `scr_allow_mp44` | 234 | `1` | `1` |
| 171 | `scr_allow_nagant` | 235 | `1` | `1` |
| 172 | `scr_allow_nagantsniper` | 236 | `1` | `1` |
| 173 | `scr_allow_panzerfaust` | 237 | `1` | `1` |
| 174 | `scr_allow_ppsh` | 238 | `1` | `1` |
| 175 | `scr_allow_springfield` | 239 | `1` | `1` |
| 176 | `scr_allow_sten` | 240 | `1` | `1` |
| 177 | `scr_allow_thompson` | 241 | `1` | `1` |
| 178 | `scr_allow_vote` | 242 | `1` | `1` |
| 179 | `scr_layoutimage` | 243 | `levelshots/layouts/hud@layout_mp_pavlov` | `levelshots/layouts/hud@layout_mp_carentan` |
| 180 | `scr_motd` | 244 | (unset) | (unset) |

### 269.. models (`CS_MODELS` = 268)

The snapshot's `modelindex` n refers to `cs[268 + n]`. Content and length
depend on the map.

| map | range | count | first two | last |
|---|---|---|---|---|
| mp_pavlov | 269..361 | 93 | `xmodel/crate_misc2a`, `xmodel/sandbags_curved_winter` | `xmodel/weapon_kar98scoped` |
| mp_carentan | 269..399 | 131 | `xmodel/static_vehicle_german_truck`, `xmodel/wehrmacht_soldier` | `xmodel/weapon_kar98scoped` |

Map props register first and weapon world models last, so one model has a
different index per map. An empty server registers no player body;
`playerbody_*` entries arrive when clients spawn.

### 524.. sound aliases (`CS_SOUNDS` = 524)

| map | indices | aliases |
|---|---|---|
| mp_pavlov | 525 | `world_hurt_me` |
| mp_carentan | 525, 526, 527 | `world_hurt_me`, `weap_mg42_loop`, `weap_mg42_cooldown` |

524 is unset on both, so the block is 1-based like the models. The populated
capture has 525..527 too. The client resolves almost every sound from the
alias csv; this block holds only the aliases scripts index by number.

### 780.. effects, menus, localized strings, materials

| cs | mp_pavlov | mp_carentan |
|---|---|---|
| 781 | `fx/impacts/newimps/minefield.efx` | `fx/explosions/explosion1_nolight.efx` |
| 782 | `fx/explosions/explosion1_nolight.efx` | (unset) |
| 1180 | `team_russiangerman` | `team_americangerman` |
| 1181 | `weapon_russian` | `weapon_american` |
| 1182 | `weapon_german` | `weapon_german` |
| 1183 | `viewmap` | `viewmap` |
| 1184 | `callvote` | `callvote` |
| 1185 | `quickcommands` | `quickcommands` |
| 1186 | `quickstatements` | `quickstatements` |
| 1187 | `quickresponses` | `quickresponses` |
| 1212 | `CGAME_USEMG42` | `CGAME_USEMG42` |
| 1213 | `CGAME_USEPTRS41` | `CGAME_USEPTRS41` |
| 1245 | `MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN` | `MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN` |
| 1246 | `MPSCRIPT_KILLCAM` | `MPSCRIPT_KILLCAM` |
| 1501 | `levelshots/layouts/hud@layout_mp_pavlov` | `levelshots/layouts/hud@layout_mp_carentan` |
| 1502 | `black` | `black` |
| 1503 | `hudScoreboard_mp` | `hudScoreboard_mp` |
| 1504 | `gfx/hud/hud@mpflag_none.tga` | `gfx/hud/hud@mpflag_none.tga` |
| 1505 | `gfx/hud/hud@mpflag_spectator.tga` | `gfx/hud/hud@mpflag_spectator.tga` |

- 780 is unset: effects are 1-based off 780, and the list is map script data.
- 1180/1181 are the team-selection menus, the only place the map's nationality
  shows; 1182..1187 are the rest of the precached menu block (`1180..1211`,
  hud-protocol doc).
- 1212/1213 sit just past it; 1245/1246 are in the localized-string block
  (`1244 + n`); 1501.. is the material block.
- All come from the game module except 1501.

## What a minimal server has to set

From the game module, byte-identical on both maps: 2 (`cod`), 7 (weapon list),
13, 20, 21, 22, 29, the 140/204 cvar pairs, 1180..1187, 1212, 1213, 1245,
1246, 1502..1505. Per map: 0, 1, 3, 8, 11, 12, 243, 269.., 525.., 781.., 1501.

## Retail client check

**PASS, VERIFIED live 2026-08-25**, against a release `vcod-server
mp_carentan` from before vcod-server sent snapshots. Retail 1.1 `CoDMP.exe`
got `connectResponse`; the server logged `connected`, `gamestate, 2641 bytes`,
the `cp` (ignored under `sv_pure 0`) and `sent its first move (serverTime 0)`
(expected before a first snapshot); the client loaded the map. No `usercmd not
parsed`, no `EXE_SERVER_IS_DIFFERENT_VER`. That proved the usercmd field
widths and order from `cod_lnxded 0x807b7f8` (compact and full branches) match
a 1.1 client; values went unchecked, as that server discarded them.

**Trap.** Every server message ends in `svc_EOF` (`SV_Netchan_Transmit`
appends it; retail's gamestate has it at decompressed byte 7205, `8`, then a
pad symbol). Without it the client fails after the map load with `ERROR:
CL_ParseServerMessage: Illegible server message 195` (the pad symbol read as
an op) and disconnects (server: `dropped: EXE_DISCONNECTED`). vcod's client
never reads past the gamestate body, so only retail's catches it.
`ServerNetchan::transmit` appends it; the byte-exact test pins it.

Two shells:

```
RUST_LOG=debug cargo run -p vcod-server -- mp_carentan
CoDMP.exe +set r_fullscreen 0 +connect 127.0.0.1:28960     # from the 1.1 install
```

Pass: the server logs `connected`, `gamestate, N bytes` and `sent its first
move`, with no `usercmd not parsed` at debug level; the client loads the map;
neither side prints `EXE_SERVER_IS_DIFFERENT_VER`.

| Symptom | Suspect |
|---|---|
| `usercmd not parsed` at debug level | the full-field usercmd branch, INFERRED from disassembly only; table in `tools/re/net-notes.md` |
| `Illegible server message N` right after the map loads | the missing `svc_EOF` terminator (`ServerNetchan::transmit`) |
| the client disconnects right after the gamestate with another error | the configstring table in `crates/server/src/configstrings.rs` |
| no `connectResponse` | `parse_connect` (`crates/common/src/net/connectionless.rs`) |

## Housekeeping: master heartbeat, rcon, zombie slots, private slots

Measured 2026-10-07 on `cod_lnxded` (1.1d) via `tools/run_server.sh` on a
spare port, a UDP listener as the master, `vcod --net-probe` as the client,
`rcon status` polled every 0.5-0.6 s for zombie timing. Each claim carries its
own label.

vcod's port: `crates/server/src/master.rs`, `crates/server/src/rcon.rs`, the
zombie half of `Server::drop_client`. vcod heartbeats `master.cod.pm`
(`sv_master1`'s default; its operator allowed it) on the protocol's port,
20510 or UO's 20610 (retail: always 20510), at most one packet per master host
a second.

### Master heartbeat (`SV_MasterHeartbeat`, 0x808ba0c)

- `sv_master1` defaults to `codmaster.activision.com`, `sv_master2..5` to
  empty. VERIFIED (strings at 0x8d6f0..0x8d735, `SV_Init`'s `Cvar_Get` calls).
- `dedicated` defaults to `"2"` (0x80cf769). VERIFIED.
- Heartbeats only at `dedicated 2` (`cmp [eax+0x20],2` at 0x808ba22). INFERRED
  (branch).
- Packet `\xff\xff\xff\xffheartbeat COD-1\n`. VERIFIED (capture; format
  `heartbeat %s\n` at 0x80d5978, `COD-1` at 0x80d5e69).
- Port always 20510: `sv_master1 127.0.0.1:29463` logged `resolved to
  127.0.0.1:20510` and the packet came on 20510. VERIFIED (capture).
- Cause: the swapped `strstr(":", name)` at 0x808baf0 (`push 0x501e` at
  0x808baff), Q3's bug. INFERRED.
- A name re-resolves only after its cvar changes; a failed one is cleared
  (`Cvar_Set(name, "")`) and never retried. INFERRED (0x808ba6f..0x808bad3).
- Interval 180 s (`add eax,0x2bf20` at 0x808ba3d), restarted by every send.
  VERIFIED (binary; capture: 179.998 s apart).
- Forced sends: `SV_Heartbeat_f` (0x8084bd0) sets the next time to -9999999,
  from the `heartbeat` command, `SV_SpawnServer` (0x808a915, every map load),
  `SV_DirectConnect` when the client is the first or fills the last slot
  (0x8085cd3), `SV_DropClient` when nobody is left at `CS_CONNECTED`+
  (0x8085edc). INFERRED (conditions).
- Captured heartbeats: server start, a probe's connect, its kick, `map
  mp_harbor`; none at `map_restart`. VERIFIED.
- `SV_MasterShutdown` (0x808d268) sends one `heartbeat flatline\n`. VERIFIED
  (capture, `rcon quit`).
- The flatline ignores the timer, still gated on `dedicated 2`. INFERRED.
- The live master answers within 0.2 s with `getchallenge <n>` and `getstatus
  <n>` from the address heartbeated, which a home NAT passes. VERIFIED
  2026-10-07 (bare socket; retail at `developer 1`).
- Retail answers that `getchallenge` by resolving
  `codauthorize.activision.com` and logging `sending getIpAuthorize for
  <master>:20510` (0x8084d90), no `challengeResponse`. VERIFIED (log).
- Public DNS resolves `codauthorize.activision.com` to the master's own
  address. VERIFIED (DNS).
- Neither server got listed: retail (`dedicated 2`, one heartbeat, quit after
  ~60 s) was absent from `getservers 1 full empty` at 15, 35 and 60 s among 48
  servers; vcod-server (heartbeat plus flatline) at 12 and 32 s. VERIFIED
  2026-10-07.
- Why (authorize round trip, address, other) is open. UNVERIFIED.
- vcod-server starts at `dedicated 1` on purpose: dev and agent runs stay
  unlisted unless `--set dedicated=2`.

### rcon (`SVC_RemoteCommand`, 0x808c404)

- Password cvar `rconPassword` (0x8d699, flags 0x100); no `rcon_password`.
  Names fold case (`+set rconpassword x` works). VERIFIED.
- The packet is tokenized whole: argv 1 the password, argv 2.. the command.
  INFERRED (`Cmd_Argv` calls).
- The line goes through the common query reader (`MSG_ReadStringLine`,
  `Cmd_TokenizeString`, "The challenge key"), so comments vanish before the
  rebuild. VERIFIED live on 1.1d, 1.5b, UO 1.51b (2026-10-16): `set vc_t
  "a//b"` reads back `"vc_t" is:"a^7"` (the rebuilt `set vc_t a//b ` is
  tokenized again); `set vc_t "a b//c"` reads `a b//c` (requoted; a quoted
  `//` is no comment); `set vc_t a/*/b` reads `a b`. Chat over rcon:
  `cod11-chat.md` 1.
- Rate limit: one request per 500 ms server-wide, any password or source. One
  within 499 ms of the last answered is dropped silently and does not restart
  the window. VERIFIED (capture: a second socket 5 ms later got nothing; a
  good password 450 ms after an answer got nothing; one 200 ms after a drop
  was answered).
- The check (`cmp eax,0x1f3` at 0x808c423, skipped while the last time is 0)
  precedes the password test. INFERRED.
- Its clock is the handling time: 0x808c410 calls 0x806d988 and compares with
  the last time at 0x80e30c8. VERIFIED (disassembly).
- 0x806d988 pumps `Com_GetRealEvent` to an empty event and returns its time
  (Q3's `Com_Milliseconds`). INFERRED (shape).
- With the 5 ms socket read ("Pings"): 505 ms apart answered 20/20, 490 ms
  0/10. VERIFIED (live, 1.1d, 2026-10-17).
- vcod-server rebuilds that moment (`handle_queued` in `main.rs`,
  `engine::Arrival::read`): the arrival, or the end of the previous packet's
  handling when later (an rcon `map_restart`); the tick is not counted, as
  retail's frame takes well under 1 ms. Debug build: same 20/20 and 0/10 (a
  tick-start read gave 10/20 at 505 ms). VERIFIED (live).
- Replies. VERIFIED (capture):

| case | reply |
|---|---|
| no `rconPassword` | `print\nNo rconpassword set on the server.\n` |
| wrong password | `print\nBad rconpassword.\n` |
| good | `print\n` plus the command's output; bare for an unknown or empty command and `say`; `map_restart` sends the restart's log once run |

- The command is rebuilt from argv 2 into a 1 KB buffer, each token quoted if
  empty or holding a byte <= space, each followed by a space (0x806dbd4); an
  overflow is not run. INFERRED.
- `Com_BeginRedirect` buffers 0x3ff0 bytes (0x808c526); a print that would
  pass 0x3fef flushes first (0x806b5db..0x806b5f5), so replies split at print
  boundaries. VERIFIED (capture: `fdir *.tga` came as 16350 + 14282 bytes
  after `print\n`).
- The rest goes at the redirect's end, even empty. INFERRED (fits the bare
  `print\n` replies).
- `Rcon from <addr>:\n<argv 2>` / `Bad rcon from ...` (0x80d5ae3, 0x80d5acd)
  print before the redirect, so not in the reply. INFERRED.
- `status` (`SV_Status_f` 0x80846b4): `map: <name>`, header, rule, a row per
  slot not `CS_FREE`, an empty line. Row as Q3: `%3i %5i `, `CNCT `/`ZMBI
  `/`%4i ` ping, name padded to 16, `%7i ` ms since last packet, address
  padded to 22, `%5i` qport, ` %5i` rate. Ports print as signed shorts
  (`127.0.0.1:-17446`). VERIFIED (capture).
- `NET_AdrToString` (0x8080ef4) prints `loopback` for type 2, `%i.%i.%i.%i:%i`
  for 4 (`NA_IP`), and the IPX format (0x80d3180) over ten zero bytes for any
  other, including `SV_AddTestClient`'s bot address (type 0, 0x80875cf). Its
  port is the `bot%d` counter in host order (0x80875e3), through `BigShort`
  and sign extension. VERIFIED (asm). No `bot` string in any of the three
  Linux servers. VERIFIED (strings).
- `probe_testclient` on 1.1d mp_pavlov (2026-10-18): `  0     0  999 bot1
  8700 00000000.000000000000:256    0  5000` (`bot2` `:512`, `bot3` `:768`):
  ping 999 (nothing acked), qport 0, rate 5000, so not LAN. VERIFIED
  (capture). vcod prints the same (`status_lists_test_clients_as_retail`) and
  gives its bots that form with slot + 1 as port; their ping stays 0.
- `clientkick <n>` (`SV_KickNum_f` 0x8084be4, usage `kicknum`): `Usage:
  kicknum <client number>`, `Bad slot number: %s`, `Bad client slot: %i`,
  `Client %i is not active` (0x8083b9c), else drops with `EXE_PLAYERKICKED`
  and stamps the last-packet time. INFERRED.
- Kicking slot 0 replied `print\n0:vcod EXE_PLAYERKICKED\n` (and `Going to
  CS_ZOMBIE for vcod` at `developer 1`). VERIFIED.

### Zombie slots (`SV_DropClient` 0x8085cf4, `SV_CheckTimeouts` 0x808cbc0)

The drop:

- Sets `CS_ZOMBIE` (1) at once, before the game's disconnect; returns early on
  a zombie. INFERRED (0x8085d00..0x8085d29).
- Unless the reason is `EXE_DISCONNECTED`, every client above `CS_CONNECTED`
  but the zombie gets `e "\x15<name>^7 \x14<reason>"` (0x80d45b7; `> 2` test
  in 0x808b900); then `<slot>:<name> <reason>` prints and the zombie gets `w
  "<reason>"`. INFERRED.
- A timeout drop frees the slot at once (`*piVar3 = 0` in 0x808cbc0), so no
  `w`. INFERRED.
- A zombie is not free for a new connect; one from the same address and qport
  (or port) reconnects into it. INFERRED (Q3's `SV_DirectConnect` shape; not
  measured).

What it is sent:

- A message per snapshot interval: unacked server commands, then a snapshot.
  `SV_SendClientSnapshot` (0x808f844) calls `SV_BuildClientSnapshot`
  (0x808f130) for it, which zeroes the ring slot's entity and client counts
  and skips the playerstate copy and entity walk (`state != 1`).
  `SV_WriteSnapshotToClient` (0x808dee0) gives no delta base (only `CS_ACTIVE`
  gets one). So: the playerstate of 32 messages earlier, from null, no
  entities, no clients. INFERRED (decompile).
- 2026-10-12, a probe decoding past its `disconnect` (namespace, `rate 1000`,
  `snaps 30`): each message `w "EXE_DISCONNECTED"`, then `deltaNum` 0,
  `snapFlags` 5, a full spectator playerstate (808 bits), entity terminator,
  no client: 131 bytes decoded, 136 on the wire. Disconnecting 25 messages in
  gave a null playerstate (16 bits, 36 bytes): that ring slot was never
  written. VERIFIED (capture).
- Its packets still pass the netchan and move its acks, but are not executed
  and do not move its last-packet time (`cmp [piVar2],1` before the store at
  0x808ca44). INFERRED.
- Paced by `SV_SendMessageToClient` like any client; not `CS_ACTIVE`, so >= 1
  s apart off the LAN (`0x808f806..0x808f824`). VERIFIED (code). The "Message
  pacing" capture (2026-10-11): 136-byte messages 997 ms apart. VERIFIED
  (capture).
- vcod: `zombie_message` writes the ring slot the same way;
  `Server::send_zombie` paces through `Client::pace`. Same captures: two
  messages 1000 ms apart, same layout, 136 bytes. VERIFIED (capture,
  2026-10-12, sizes only).
- Trap: retail's zombie `velocity[2]` reads -0.0 after the stop (from
  `PM_SlideMove`'s crease and the axial brush normals, kept by an idle
  `PM_FlyMove`; protocol doc, "Spectator"). A +0.0 cost 4 bytes against the
  capture.

When it goes:

- `sv_zombietime` defaults to 2 (0x80d56cf). VERIFIED.
- `SV_CheckTimeouts` (0x808cbc0) runs after the clock advance and game frame,
  right before `SV_SendClientMessages` (0x809045c), and frees when
  `lastPacketTime < svs.time - sv_zombietime * 1000`. VERIFIED (code).
- The last-packet time is the last frame's `svs.time`, so the slot goes on the
  frame 2050 ms past the drop's packet, before that frame's message: two
  zombie messages off the LAN, 1000 ms apart. VERIFIED (capture,
  2026-10-11/12).
- After `clientkick` (which stamps the time) the row read `ZMBI` 500, 1050,
  1550 ms at 0.51, 1.03, 1.53 s, gone at 2.04 s; after a client's own
  disconnect, last `ZMBI` 1550 ms, free 510 ms later. VERIFIED (capture).
- vcod: `check_timeouts` frees off `Client::last_packet_ms` on the advanced
  clock, before any message.

A silent client:

- Same pass, same clock (0x808cbc0). VERIFIED (code): a `lastPacketTime` ahead
  of `svs.time` is pulled back; above `CS_ZOMBIE` with a time under `svs.time -
  sv_timeout * 1000`, `timeoutCount` (`cl+0x10b1c`) increments and past 5
  the client drops with `EXE_TIMEDOUT` and is freed; otherwise the count
  zeroes. A slot with `cl+0x5a8f4` set (the bot connect path at 0x80876a0
  stores 1) is skipped. `sv_timeout` is read `->integer` each frame, default
  240 (0x80d56c0).
- So it drops on the sixth stale frame, 250 ms past the timeout at 20 Hz.
  INFERRED.
- 2026-10-12, loopback `mp_carentan`, `+set sv_timeout N`, `--probe-edge
  silence`: fly 5 s, send a last packet right after a snapshot (stamped with
  its time T), then silence while decoding. Last snapshot `serverTime`: T +
  1250 at N=1 (3 runs), T + 2250 at 2 (3 runs), T + 3250 at 3. Nothing after
  (no `w`, no OOB). VERIFIED (capture).
- Reading: T + N*1000 is not stale (`<`), +50 is the first stale frame, +300
  the sixth, which drops before its message. A check ahead of the clock
  advance would read a frame later; a first-stale-frame drop 250 ms earlier.
  INFERRED (totals; clock order and compare not separable).
- vcod: the same pass in `check_timeouts`, before the snapshots. Against
  vcod-server: T + 1250 (4 runs), T + 2250 (2 of 3; the third, T + 2300,
  landed a frame late on a loaded host). Pinned by
  `a_silent_client_is_dropped_on_its_sixth_stale_frame` and
  `a_silent_client_s_last_snapshot_is_250_ms_past_its_timeout`
  (`crates/server/tests/connect.rs`). Trap: a wall-clock check ahead of the
  clock advance drops on the first stale frame.

A zombie never active:

- `--probe-edge zombie` (2026-10-12): `disconnect` in the first message after
  the gamestate, no usercmd, so `CS_ZOMBIE` without `CS_ACTIVE`. Retail sent
  40 messages, one a frame from the next frame, the first with `w
  "EXE_DISCONNECTED"`, each a snapshot with `snapFlags` 6, no delta, null
  playerstate (`commandTime` 0), no entities; the last 1950 ms after the
  first, so the slot freed on the frame 2050 ms past the drop's, before its
  message (`Going from CS_ZOMBIE to CS_FREE`). VERIFIED (capture).
- `--probe-edge zombie-active` (5 s in the world, then `disconnect`): the same
  40 with `snapFlags` 4 and the playerstate of 32 messages earlier. VERIFIED
  (capture).
- vcod-server: the same 40, flags 6 and 4.
  `a_never_active_zombie_is_sent_not_active_snapshots_for_two_seconds` pins
  the first.

### The order of a frame's messages (`SV_SendClientMessages`)

All three servers send a frame's messages in one slot-order pass, whatever
each slot's state; zombies and unentered clients get no pass of their own.

| | 1.1d | 1.5b | UO 1.51b |
|---|---|---|---|
| `SV_CheckTimeouts` | 0x808cbc0 | 0x8093eac | 0x8094e0d |
| `SV_SendClientMessages` | 0x809045c | 0x8098a34 | 0x8099a2c |
| `SV_SendClientSnapshot` | 0x808f844 | 0x8097c2f | 0x8098c23 |
| download blocks | 0x8086290 | 0x808b8a9 | 0x808c559 |

- `SV_Frame` ends `SV_CheckTimeouts`, `SV_SendClientMessages`, the snapshot
  archive (1.1d 0x808fb84), `SV_MasterHeartbeat("COD-1")`. VERIFIED (call
  sequences, three decompiles).
- `SV_CheckTimeouts` frees stale zombies (`Going from CS_ZOMBIE to CS_FREE`)
  before any message. VERIFIED (the string in each).
- `SV_SendClientMessages` walks slots 0..`sv_maxclients - 1` once; a slot with
  state != 0 whose `nextSnapshotTime` has come gets its next netchan fragment
  if any, else `SV_SendClientSnapshot`. VERIFIED (those are the loop's only
  tests).
- `SV_SendClientSnapshot` serves states 1-4: snapshot and server commands only
  for 4 (`CS_ACTIVE`) or 1 (`CS_ZOMBIE`), download blocks for any state but 1,
  then `svc_EOF`. So a `CS_CONNECTED`/`CS_PRIMED` slot's 9-byte bare message
  goes on its own turn. VERIFIED (`== 4 || == 1`, `!= 1` in all three).
- 2026-10-24, loopback `mp_carentan` under `unshare -rnm`, an `AF_PACKET`
  socket on `lo` logging sends in order. Slot 0 joined through a relay passing
  only OOB packets, so it stayed `CS_CONNECTED` (`status`: `CNCT`); slot 1
  stayed in the world. Slot 0's 9-byte message went before slot 1's snapshot
  in every frame holding both: 1.1d 299/299, 1.5b 300/300, UO 299/299.
  Exceptions were only slot 1's connect frames, carrying the handler's own
  pre-frame sends. VERIFIED (capture).
- A zombie at slot 0 (`--probe-edge zombie-active` first, a plain probe
  second, the first leaving after 5 s): its 124-136 byte message preceded slot
  1's snapshot every frame for two seconds, on all three. VERIFIED (capture).
- vcod-server runs one pass too: `Server::message_due` takes every socketed
  slot, zombies included; `send_client_snapshot` sends by state;
  `check_timeouts` frees stale zombies first. On 1.1: slot 0's 9-byte message
  first in 300/300 frames (the one burst out of order held an rcon reply), the
  zombie's 124-125 bytes first in all 40 frames. VERIFIED (capture,
  2026-10-24). `a_frames_messages_go_out_in_slot_order` pins both. Trap:
  separate in-world, unentered and zombie passes sent slot 1's snapshot first
  in 294/300 frames.

### When the game hears a drop (`SV_DropClient`, `ClientDisconnect`)

- 1.1d's `SV_DropClient` (0x8085cf4): `CS_ZOMBIE`, clear the challenge, the
  `e` notice, print `<slot>:<name> <reason>`, the game's `ClientDisconnect`
  (`VM_Call` 5, guarded by `sv.state == SS_GAME`, returns to 0x8085e56), then
  the zombie's `w`. INFERRED (control flow).
- 1.5b (0x808ac11) and UO (0x808ba15) call the game first: `SV_FreeClient`
  (1.5b 0x8089daf, UO 0x808abb3; `VM_Call` 7, then `SV_SetUserinfo(slot, "")`)
  before `Going to CS_ZOMBIE`, the state write and the notice. INFERRED
  (control flow).
- `ClientDisconnect` (1.1d 0x42aac, 1.5b 0x4cf5f, UO 0x55433), in order:
  `Scr_Notify(player, "menuresponse", "-1", "disconnect")` (1.1d strings
  0x73113, 0x73108; `Cmd_MenuResponse_f`'s `scr_const` slot); the follower
  pass (1.1d `Cmd_FollowCycle_f(ent, 1)`, `StopFollowing` if that fails; 1.5b
  and UO `StopFollowing` alone); the first complaint window naming the leaver
  and its `m -2`; UO's `VEH_UnlinkPlayer`; `HudElem_ClientDisconnect`; the
  callback; `G_FreeEntity`; `CalculateRanks` (1.1d 0x42c1f). INFERRED (control
  flow).
- 2026-10-18, `client-probes/probe_dropclient` on all three (`+set sv_timeout
  3`), a `--probe-edge watch` spectator beside `silence` and `zombie-active`
  clients; alike unless noted. VERIFIED (capture):
  - The timeout drop's callback reads `getTime()` of the frame just run, and
    its `iPrintLn` rides that frame's message with the notice: `e
    "...EXE_TIMEDOUT"` then `f "PROBE_GONE 1"` on 1.1d, `f` then `e` on 1.5b
    and UO.
  - A `disconnect` between frames runs the callback on the last frame's
    `getTime()`; its line rides the next message.
  - The callback's `iPrintLn` reaches neither the zombie (its message holds
    only `w`) nor, on 1.5b and UO, the client still in the game's slot.
  - The `menuresponse` waiter on the player's own thread never runs. A level
    thread waiting on the player wakes with `[-1] [disconnect]` on the next
    frame's `getTime()`, after the callback; a `wait 0.05` the callback starts
    runs a frame later.
- vcod: `Server::drop_client` runs `ScriptRuntime::client_disconnect` inside
  the drop (after the notice on 1.1, before on 1.5/UO) with the slot already
  out of `clients`, sending its commands and configstrings at once; waiters
  run in the next frame's first pass (`Vm::notify`); the disconnect sets
  `ranks_dirty`. Same lines and layout as retail. Pinned by
  `a_timed_out_clients_callback_lines_go_out_with_its_drop_notice`,
  `a_drop_notifies_menuresponse_for_the_next_frame`,
  `a_dropped_client_stops_its_followers_on_1_5`. Trap: queueing the disconnect
  for the next packet pass sends a timeout's line a message after the notice
  and never notifies `menuresponse`.
- A command the callback sends the leaver: no engine tests the slot's state on
  the way in. 1.1d's `SV_GameSendServerCommand` (0x80894bc) hands any slot to
  `SV_SendServerCommand`; `SV_AddServerCommand` (0x808b680) only squashes. A
  slot not `CS_ACTIVE` (or 32 past its ack) drops its unsent type-0 commands
  and takes no new one; type 1 still goes. The zombie's `w` (0x8085e74) goes
  with the state already `CS_ZOMBIE`, squashing every unsent print. INFERRED
  (control flow).
- 2026-10-18, `client-probes/probe_reconnect`, callback sending `self
  iPrintLn` and `self setClientCvar("probe_self", n)`, a `zombie-active`
  client: the zombie's message holds `v probe_self "2"` then `w
  "EXE_DISCONNECTED"`, no print, on all three. VERIFIED (capture).
- vcod: `Server::client_disconnect` returns the leaver's commands;
  `drop_client` queues them on the zombie and squashes the ignorable ones
  (`Client::squash_ignorable`) before the `w`. Same two commands.
  `a_leavers_zombie_keeps_the_callbacks_reliable_commands_only` pins it.

### A reconnect into a held slot (`SV_DirectConnect`)

A connect from a held slot's address and qport (or port), past
`sv_reconnectlimit`, prints `%s:reconnect` and takes the slot.

- 1.1d (0x80859b1..0x80859cd): above `CS_ZOMBIE` it frees only the script id
  (0x808d34c, as `SV_DropClient` ends) and goes to `ClientConnect` (`VM_Call`
  2, 0x8085baa); no `ClientDisconnect`. `ClientConnect` (0x4246c) zeroes the
  `gclient_t` and entity and runs `G_InitGentity` on the held player in place.
  INFERRED (control flow).
- 1.5b (0x808a6e3) and UO (0x808b4e7) call `SV_FreeClient` there, so
  `ClientDisconnect` runs first. INFERRED (control flow).
- 2026-10-18, `client-probes/probe_reconnect`, a `--probe-edge reconnect`
  client reconnecting from its own socket and qport 5 s in. VERIFIED
  (capture):
  - 1.1d runs no disconnect callback. The first connect's player thread keeps
    running and reads the second connect's fields; the second callback finds
    the first's script field defined and `pers` empty. Both threads die at the
    final drop.
  - 1.5b and UO run the disconnect then the connect callback on one
    `getTime()`; the second finds neither field nor `pers`.
- vcod: the reconnect arm runs `Server::client_disconnect` inside the connect
  on 1.5/UO; on 1.1 none, and `ObjectTable::spawn_client` keeps the held
  player's script object (threads, script fields) with fresh engine fields and
  `pers`. `a_reconnect_into_a_held_slot_follows_the_protocol` pins it; same
  lines as retail.
- On 1.1 the area tree entry stays: the link is the engine's svEntity
  (cod11-combat.md 14.7) and `SV_UnlinkEntity` (0x8091f94) is not called.
  Walks read the zeroed `r.contents` and box and skip it; the next link
  (spawn's unlink, or a spectator's contents-0 unlink) removes it
  (`AreaTree::zero_entity`, `a_reconnect_on_1_1_leaves_the_area_entry`).
  INFERRED.
- vcod's own guard: a connect with another challenge into a slot heard from in
  the last 3 s is refused; retail takes it at once.

### When the client's `begin` is heard (`ClientBegin`)

- 1.1d's `ClientBegin` (0x42f88) marks the client connected, runs
  `CalculateRanks`, notifies `begin`. INFERRED (control flow).
- `probe_dropclient` and `probe_reconnect` on all three (2026-10-18): the line
  after `waittill("begin")` reads the client's first snapshot's `serverTime`,
  a frame after the entering packet; the waiter runs in the next frame's
  thread pass like other engine notifies. VERIFIED (capture).
- vcod: `ClientEvent::Begin` notifies via `Vm::notify` and sets `ranks_dirty`
  (as `ClientConnect`'s closing `CalculateRanks` at 0x42677 does), printing
  the same `serverTime`. A packet-pass waiter reads 50 ms early.
  `the_begin_waiter_runs_on_the_next_frames_clock` pins it.

### Console commands over rcon

Measured 2026-10-08 the same way (retail on a spare port, `rconPassword pw`,
one `vcod --net-probe` where a client is needed). Reply strings VERIFIED
(capture) unless marked.

`SV_AddOperatorCommands` (0x8084a3c) registers `heartbeat`, `kick`, `banUser`,
`banClient`, `clientkick`, `status`, `serverinfo`, `systeminfo`, `dumpuser`,
`map_restart`, `map`, `map_rotate`, `gameCompleteStatus`, `devmap`,
`killserver`, `scriptUsage`, `stringUsage`, and `say` only when `dedicated` is
non-zero. VERIFIED (string table at its calls). vcod ports all of them.

`kick <name>` (`SV_Kick_f` 0x8084288):

- Exactly one argument, else `Usage: kick <player name>\nkick all = kick
  everyone\n` (0x80d3c80).
- `SV_GetPlayerByName` (0x8083aa0) takes the first slot not `CS_FREE` whose
  name matches case-insensitively, raw or after `Q_CleanStr`; a miss prints
  `Player %s is not on the server\n` (0x80d3880). INFERRED (control flow).
- The miss line and a case-insensitive hit (`kick VCOD` dropped `vcod`:
  `0:vcod EXE_PLAYERKICKED\n`). VERIFIED.
- The lookup precedes the `all` test, so `kick all` prints the miss line.
  VERIFIED (empty server).
- `all` then drops every used slot but a loopback one with `EXE_PLAYERKICKED`,
  stamping each last-packet time. INFERRED.
- A loopback client kicked by name broadcasts `e "EXE_CANNOTKICKHOSTPLAYER"`
  (0x80d3cc9) instead. INFERRED; a dedicated server has none.

Info commands:

- `dumpuser <name>` (`SV_DumpUser_f` 0x8084cc0): `Usage: info <userid>\n`
  (0x80d3f5a) unless one argument; same lookup; `userinfo\n--------\n` and
  `Info_Print` of the userinfo, which ends with the `ip` `SV_DirectConnect`
  (0x8085498) appends (key 0x80d43da, port as a signed short: `ip
  127.0.0.1:-9130`). VERIFIED.
- `Info_Print` (0x806bbd4): a line per pair, key left-justified in 20 columns,
  a longer key running into its value (`bg_fallDamageMaxHeight480`). VERIFIED.
- `serverinfo` / `systeminfo` (0x8084c68 / 0x8084c94): `Server info
  settings:\n` (0x80d3f2c) / `System info settings:\n`, then `Info_Print` of
  the string. Retail's serverinfo keys are the 14 of vcod's configstring 0,
  same order; a `set sv_hostname` shows at once. VERIFIED. vcod's systeminfo
  carries the pak lists ("Pak checksums and pure servers").
- `say <text>` (`SV_ConSay_f` 0x8084974): nothing without an argument; else
  `"console: "` (0x80d3f1a) plus the joined arguments to every client past
  `CS_CONNECTED` as `h "\x15%s"` (0x80d3f24, type 0); bare `print\n` reply.
  VERIFIED (the probe read `h "\x15console: hello there"`).

Cvars:

- `set` / `seta`: `usage: set <variable> <value>\n` (0x80cfdc0; `seta` names
  itself) under two arguments; else argv 2.. joined by single spaces (`set foo
  a b c` reads `a b c`), empty reply. VERIFIED.
- `Cvar_Command`: a first word naming a cvar prints `"%s" is:"%s^7"
  default:"%s^7"\n` (0x80cfd40) in the registration spelling (`foo2` answered
  `"FOO2"`), plus `latched: "%s"\n` (0x80cfd5f) while one waits; a second word
  sets it. The default is the creation value: `set foo a b c`, `foo baz` reads
  `default:"a b c^7"`; `scr_dm_timelimit`, created by `default_mp.cfg`'s
  `set`, reads `default:"30^7"` after a `set` to 5. A non-cvar is an unknown
  command, silent. VERIFIED.
- `g_gametype` latches: `set g_gametype tdm` replies `g_gametype will be
  changed upon restarting.\n` (0x80cfc40), the query reads `"g_gametype"
  is:"dm^7" default:"dm^7"\nlatched: "tdm"\n`, and `serverinfo` keeps `dm`
  until the restart. `cvarlist` flags `g_useGear` `L` too. VERIFIED.
- The latch (on `g_gametype`, `sv_maxclients`, `g_useGear`): a value other
  than the live and waiting ones prints that line and waits; the waiting value
  again is silent; the live value is silent and clears the wait. VERIFIED.
- vcod latches `g_gametype` and `sv_maxclients` (the two `SV_MapRestart_f`
  escalates on, map-cycle doc 4) in its own fields and every other `L` cvar in
  the cvar table, applied at a level load.
- `Cvar_Set2`'s other refusals, name as typed. VERIFIED (rcon). vcod applies
  all four from the table's flags.

| flag | reply | examples |
|---|---|---|
| `R` | `%s is read only.\n` (0x80cfbf8) | `set sv_cheats 1`, `set version x` |
| `I` | `%s is write protected.\n` (0x80cfc0a) | `fs_game`, `net_qport` |
| `C` at `sv_cheats 0` | `%s is cheat protected.\n` (0x80cfc22) | `set timescale 2`, `timescale 2`; silent at `sv_cheats 1` |

`cvarlist [filter]` (`Cvar_List_f` 0x806f530):

- A line per matching cvar: seven flag columns, then ` %s "%s"\n`. Columns in
  order 0x4 `S`, 0x2 `U`, 0x40 `R`, 0x10 `I`, 0x1 `A`, 0x20 `L`, 0x200 `C`, a
  space when clear. INFERRED (branch order). The letters and columns VERIFIED
  (`S    L  g_gametype`, `    AL  g_useGear`, `     LC fs_ignoreLozalized`).
- Ends `\n%i total cvars\n%i cvar indexes\n`, both counting every cvar (210
  and 210, idle `dm`). VERIFIED.
- Case-insensitive sort, letters folded to lower: `scr_hq_scorelimit` <
  `scr_hqt_scorelimit`, `g_ScoresBanner_Allies` < `g_scriptMainMenu`.
  VERIFIED.
- The filter is `Com_Filter` (0x8067774, `casesensitive` 0), Q3's except that
  a `*` run ends only at the next `*` or `?` (a `[` inside is literal).
  Otherwise Q3's: the filter's end matches the rest of the name, `?` one
  character, a `*` run at its first occurrence (0x8067660, no backtracking),
  `[...]` a set with `a-z` ranges and `]]` a literal `]`, `[[` drops the first
  `[`. INFERRED (0x8067774's control flow).
- On 1.1d, each VERIFIED: `fs_` listed all nine `fs_*`; `G_SPEED`, `g_spe`,
  `g_spe?`, `g_speed??`, `[[g]_speed`, `g_s[pq-r]`, `G_[A-z]PEED` `g_speed`
  alone; `sv_*rate` `sv_maxRate`; `*ps` `g_banIPs` among others; `g_[s]` the
  eight `g_s*`; `*_[s]`, `*[ab]`, `g_s[]]`, `g_speed[]`, `g_speed[a]` nothing.
- A `?` past the name's end makes retail read past the terminator (`g_speed??`
  matched); vcod matches nothing there (`cvars::com_filter`).

The registry:

- `tools/capture_cvars.py` lists a retail server's cvars with flags and
  printed defaults; it wrote `crates/server/src/cvars/registry.rs`, vcod's
  seed. VERIFIED.
- Besides `default_mp.cfg`'s and the scripts' cvars, defaults differing from
  running values: `dedicated 2`, `net_port 28960`, `sv_maxclients 20`,
  `sv_pure 1`, `sv_hostname CoDHost`, `mapname nomap`. VERIFIED.
- A `+set` before registration keeps its value, the registration still sets
  the default (`is:"8" default:"20"`). Registering a `set`-created cvar takes
  over its default (`scr_allow_fg42` `is:"0" default:"1"` after
  `default_mp.cfg`'s `set` and `makeCvarServerInfo(..., "1")`). VERIFIED.
- That homepath mounted 1.5's localized paks, whose `default_mp.cfg` also sets
  `scr_killcam`, `scr_freelook`, `scr_teambalance`, `scr_spectateenemy` and
  four `scr_hq*` limits: 8 more than vcod on a 1.1 install. VERIFIED.

Map commands:

- `map` / `devmap` (`SV_Map_f` 0x8083c68, map-cycle doc 5.3): a missing map
  prints `Can't find map %s\n` (0x80d3903) with the path tried:
  `maps/mp/%s.bsp`, `maps/%s.bsp` for an `mp/` argument, `maps/mp/.bsp` for
  none. VERIFIED.
- `sv_cheats` is set after the load (0x8083dac..0x8083dd1): `"1"` for
  `devmap`, `"0"` (0x80d392c) for `map`. VERIFIED (probe: after `devmap
  mp/mp_carentan` then `map mp_harbor` both gamestates carried `sv_cheats\1`,
  and after the second a configstring 1 update followed while the cvar queried
  0).
- `devmap` on the running map took the restart path, printed `==== RestartGame
  ====`, then a configstring 1 update. VERIFIED.
- The restart banner: `map_restart`, `map_restart 1`, `map_restart 5`,
  `map`/`devmap <running map>` each answered one 165-byte packet, client or
  not: `print\n==== RestartGame ====\n------- Game Initialization
  -------\ngamename: main\ngamedate: Nov 13 2003\n0 teams with 0
  entities\n-----------------------------------\n`. VERIFIED (2026-10-09,
  mp_carentan dm).
- Its parts: `G_ShutdownGame`'s banner (0x4feaa, string 0x75665); three
  `G_InitGame` lines (0x4fb2e, 0x4fb40, 0x4fb55; `main` 0x75430, `Nov 13 2003`
  0x7541b compiled in); `G_FindTeams`' team line (0x4faff, format 0x75480);
  `G_InitGame`'s closing dashes (0x4fe40, string 0x75640) after
  `G_SpawnEntitiesFromString`, before `Scr_InitSystem` and the gametype load.
  VERIFIED.
- `fast_restart` answered an empty `print\n`. VERIFIED.
- vcod prints the block as one string before the script load
  (`restart_game_banner` in `server.rs`, with 1.5's and UO's name and date),
  team count 0 (no stock MP map has a `team` key). Retail's reply to a fresh
  `map` load is the engine's server and filesystem init around the same block;
  vcod prints nothing.
- `quit` gets no reply: `Com_Quit_f` exits inside the redirect. VERIFIED (no
  packet).

The next three: 2026-10-16, retail 1.1d started with `+set dedicated 2 +set
sv_master1 127.0.0.1` before `+map mp_carentan` (heartbeat stays local), one
probe in the game, a socket on 127.0.0.1:20510 as the master.

`gameCompleteStatus [challenge]` (`SV_GameCompleteStatus_f` 0x8084d54):

- Thunks to `SV_MasterGameCompleteStatus` (0x808bbc4). VERIFIED.
- No-op unless `dedicated` is 2 (`+0x20 == 2` at 0x808bbd9). INFERRED.
- Per non-empty `sv_master1..5`: resolve only if the cvar's `modified` flag is
  set, clear it, print `Resolving %s` / `%s resolved to %i.%i.%i.%i:%i` /
  `Couldn't resolve address: %s` as the heartbeat does, then `Sending
  gameCompleteStatus to %s\n` (0x80d59a0) and send. VERIFIED (strings,
  replies).
- Its address table is 0x8160b20, the heartbeat's 0x8160aa0 (0x808ba0c's
  `local_c`), both clearing the same flag. VERIFIED (export).
- So whichever runs first after a `sv_masterN` change resolves it and the
  other sends to a zeroed address. VERIFIED (capture: with `sv_master1`
  resolved by the load's heartbeat, `gameCompleteStatus` replied
  `print\nSending gameCompleteStatus to 127.0.0.1\n` and nothing reached
  20510; after `set sv_master2 127.0.0.1`, `gameCompleteStatus abc` replied
  `Sending gameCompleteStatus to 127.0.0.1\nResolving 127.0.0.1\n127.0.0.1
  resolved to 127.0.0.1:20510\nSending gameCompleteStatus to 127.0.0.1\n` and
  one packet came; a forced `heartbeat` then sent one, not two).
- At `dedicated 1` the reply is a bare `print\n`. INFERRED (the gate). vcod
  matches.
- The packet (`SVC_GameCompleteStatus` 0x808bfa8): `Cvar_InfoString(4)`,
  `Info_SetValueForKey(.., "challenge", Cmd_Argv(1))` (key 0x80d59cb), the
  `demo` keyword under `fs_restrict`, then `%i %i "%s"\n` (score, ping, name)
  per client at `CS_CONNECTED`+, as `gameCompleteStatus\n%s\n%s`. VERIFIED
  (strings, calls).
- Captured:
  `\xff\xff\xff\xffgameCompleteStatus\n\g_gametype\dm\gamename\main\mapname\mp_carentan\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\0\challenge\abc\n0
  0 "vcod"\n`. No argument, no `challenge` key (empty values are dropped); no
  `pswrd`. VERIFIED.
- vcod adds the `demo` keyword as for `statusResponse` ("`fs_restrict`").

`scriptUsage` (0x8084d64 -> 0x809d2d0, ported in shape):

- Walks every thread object (type 10 under flag 0x60), collects call stacks,
  sorts and groups equal stacks; per group `count: %d, var usage: %d, endon
  usage: %d\n`, the innermost position, `called from:\n` and each caller's.
  Then `ent type '%s'... count: %d, var usage: %d\n` per object class with
  any, between `********************************` rules. A position is `@
  %d\n` (bytecode offset) without loaded sources, `(file '%s'%s, line %d)`
  with them (0x809b9b0, 0x809b89c). VERIFIED (strings, calls). What each count
  totals: INFERRED.
- One client: `count: 1, var usage: 7, endon usage: 2\n@ 1480\ncalled from:\n@
  7903\ncount: 1, var usage: 2, endon usage: 0\n@ 6334\ncount: 1, var usage:
  5, endon usage: 0\n@ 7442\n`, then `ent type 'entity'... count: 211, var
  usage: 431`, `ent type 'hudelem'... count: 1, var usage: 1`. VERIFIED.
- vcod's counts are its VM's (`game::usage`): var usage the defined locals
  across frames, endon usage the endons held, positions always `(file '...',
  line N)`; an entity's var usage its script struct's fields, without the map
  keys retail keeps as variables (210 entities, 4 vars, mp_carentan, no
  client).
- Order: a thread's record is its code positions innermost first (current,
  then callers from the stack's type-6 entries); `qsort` with 0x80a2218
  compares them pointer by pointer, a shorter record first on a shared prefix;
  a group is a run of equal records. INFERRED (0x809d2d0, 0x80a2218).
- Pointer order is the code buffer's, filled by `GScr_LoadScripts` (game
  0x693c8) through `GScr_LoadGameTypeScript` (0x69158: gametype, then
  `_callbacksetup`), then `Scr_LoadScript("maps/mp/<map>")`. INFERRED.
- At `developer 1` and `developer_script 1` a position prints as `(file
  'maps\mp\gametypes\tdm.gsc', line 992)\n`, the source line verbatim (`\r\n`
  kept) and spaces ending in `*` under the column. VERIFIED.
- mp_brecourt: each gametype listed its groups in line order, then
  `_minefields`' (`count: 32`, line 19) though `_minefields` sorts first by
  name: `tdm` 992, 1184; `sd` 1246, 1291, 1691, 1822; `re` 1379, 1424, 1824,
  1905, 1988; `bel` 1076, 1265. VERIFIED.
- vcod sorts by the file's install order (`Vm::thread_usage`; the gametype's
  closure, then the level's, each file before those it calls), then line,
  matching every capture (`script_usage_orders_files_as_retail`). Where
  install or line order departs from retail's code order the groups differ. No
  source line or caret.

`stringUsage` (0x8084d74 -> 0x809a780, ported in shape):

- The script memory tree is a buddy allocator over 65535 eight-byte buckets
  (`MT_GetSize` 0x809ae08 rounds up to a power of two of buckets, max 2^16).
  Prints `%d subtree has %d * %d = %d free buckets\n` for orders 0-16, two
  rules, `total memory alloc buckets: %d (%d instances)\n`, `total memory free
  buckets: %d\n` (65535 minus allocated). VERIFIED (strings, loop).
- Captured: `total memory alloc buckets: 4580 (1158 instances)`, `total memory
  free buckets: 60955`. VERIFIED.
- vcod has no tree: instances are the VM interner's strings (never freed),
  each four header bytes plus text and NUL rounded as `MT_GetSize` does; free
  buckets list one block per set bit. vcod's own model; 4171 buckets, 1010
  strings on mp_carentan.
- The four-byte header. INFERRED (Q3's `RefString`).

### The stdin console (`Sys_ConsoleInput`, 0x80c5898)

Read off `cod_lnxded` 1.1d with `objdump`, 2026-11-12. CoD2 1.3's and CoD4
1.7's are in `cod2-protocol.md` 17.8 and `cod4-handshake.md` 14.4.

- `Sys_ConsoleInputInit` (0x80c576c) ignores `SIGTTIN` and `SIGTTOU`
  (`signal(0x15, 1)`, `signal(0x16, 1)` at 0x80c578d, 0x80c5799). VERIFIED.
- It registers `ttycon` with default `"1"` and flags 0 (`Cvar_Get` at
  0x80c57b0; the strings at 0x80dbbb7 and 0x80dbbb5). VERIFIED.
- Its raw mode clears `ICANON` and `ECHO` in `c_lflag` and `INPCK` and
  `ISTRIP` in `c_iflag`, and sets `VMIN` 1 and `VTIME` 0 (0x80c5839..
  0x80c5851). VERIFIED.
- That mode, with `Started tty console (use +set ttycon 0 to disable)`
  printed first, is taken when `ttycon` is non-zero and `isatty(0)` returns
  1 (0x80c57df). Otherwise `stdin is not a tty, tty console mode failed` is
  printed and `ttycon` set to `"0"` (0x80c5865, 0x80c587c). INFERRED.
- The tty mode is Q3 1.32's line editor, below. No `]` prompt is printed.
  INFERRED (no write of one on the branch).
- The line path's `select` on fd 0 has a zero timeout (0x80c5f79). Its
  `read` takes up to 256 bytes into 0x8318240 (0x80c5fa8), and the last byte
  read is overwritten with a NUL (0x80c5fc9). VERIFIED.
- That path runs without the tty mode, while the cvar at 0x833dee0
  (`dedicated`, as in Q3's `Sys_ConsoleInput`) is non-zero and while
  `stdin_active` (0x80e6460, 1 in the file) is set. The cut byte is the
  newline. INFERRED.
- A read of 0 bytes, EOF, clears `stdin_active` (0x80c5fb1). Stdin is not
  read again and the server keeps running. Under `nohup` or a service
  manager stdin is `/dev/null`, so the first poll ends the reading.
  INFERRED.
- `Sys_GetEvent` (0x80c61e4) calls `Sys_ConsoleInput` at 0x80c6225. Case 5
  of `Com_EventLoop`'s jump table (0x80cf5c0), `SE_CONSOLE`, calls
  `Cbuf_AddText` with the text and with `"\n"` (0x806c0e7, 0x806c103).
  VERIFIED.
- So the line runs at the next `Cbuf_Execute`, through the dispatch an
  rcon line takes, with the console's privilege. INFERRED.
- Nothing echoes the line. `Sys_Print` passes the text to `fputs` on
  `stderr` (0x80c6507, `stderr` at 0x80f8488), with no colour stripping in
  between. INFERRED.

#### The tty line editor

Read off `cod_lnxded` 1.1d with `objdump`, 2026-11-13. 1.5b and UO 1.51b
carry the same editor, split into helpers: 1.5b's `Sys_ConsoleInput` is
0x80d36ca, `tty_Hide` 0x80d30d9, `tty_Show` 0x80d3121, `Hist_Add`
0x80d31b5 (its cap compare at 0x80d3271); UO's `Sys_ConsoleInput` is
0x80c8a5e and `Hist_Add` 0x80c8549 (cap at 0x80c8605). VERIFIED. 1.5b
reads `tty_erase` before the copy as 1.1d does (0x80d365a). INFERRED
(instruction order).

- Stdin is made non-blocking (`fcntl(0, F_SETFL, flags | O_NONBLOCK)`,
  0x80c69b4). VERIFIED. The editor reads one byte a call (0x80c58db).
  VERIFIED.
- `tty_erase` (0x80e6470) and `tty_eof` (0x80e6474) are read out of the
  zeroed local struct at 0x80c5818 and 0x80c5821, before the `rep movsl`
  at 0x80c5837 copies the `tcgetattr` result into it. So `tty_erase` is 0.
  INFERRED (instruction order).
- A byte equal to `tty_erase`, 0x7f or 8 (0x80c58f3, 0x80c5901, 0x80c5906)
  erases one character, writing `\b`, ` `, `\b` to fd 1, and nothing on an
  empty line. VERIFIED. So NUL, DEL and BS all erase. INFERRED.
- A byte that is not NUL and compares at most 0x1f as a signed char
  (`cmp $0x1f,%dl; jg` at 0x80c596a) is a control byte. VERIFIED. Every
  byte past 127 is one too. INFERRED.
- Enter (`\n`, 0x80c5973): the 32 history slots of 0x11c bytes at
  0x83184a0 shift down one, the line is copied into slot 0, the count
  (0x80e647c) climbs while it is at most 0x1f (0x80c59e7) and
  `hist_current` (0x80e6478) goes to -1 (0x80c59f2). VERIFIED. Every line
  goes in, an empty one included, with no duplicate check. INFERRED. The
  line is copied out (0x80c5a09), the field cleared (0x80c5a1b) and a bare
  `\n` written to fd 1 (0x80c5a2f). VERIFIED.
- Tab (0x80c5a40): `tty_Hide`, `Field_CompleteCommand` (0x806d43c, called
  at 0x80c5ad9), a leading `\` cut off the result (0x80c5b16), the cursor
  set to its length, `tty_Show`. VERIFIED.
- Any other control byte reads one more (0x80c5b9c). If it is `[` or `O`
  (0x80c5bb3, 0x80c5bb7) a third is read (0x80c5bc7): `A` is up, `B`
  down, `C` and `D` do nothing (0x80c5be8, 0x80c5bde, 0x80c5bf1,
  0x80c5bf9). VERIFIED. Anything else, a read that finds no byte
  included, prints `droping ISCTL sequence: %d, tty_erase: %d` through the
  developer print (0x80c5ebc) and reads stdin until it is empty
  (0x80c5ed8). VERIFIED. So a byte past 127 (UTF-8) is dropped with the
  one after it. INFERRED.
- Up and down end with the same drain. Up shows the next older slot only
  while one is left (`hist_current + 1 < count`, 0x80c5c0d). Down steps
  `hist_current` back while it is at least 0 and shows that slot, or an
  empty line at -1 (0x80c5d67). VERIFIED. So down on a line never taken
  from the history clears it. INFERRED.
- Any other byte goes into the buffer at the cursor (0x80c5ef5) and is
  echoed (0x80c5f09). No compare against the 256-byte buffer's size is on
  that path. INFERRED.
- `tty_Hide` writes the backspace triple once per character while the hide
  count (0x80e646c) is 0, then counts up. `tty_Show` counts down and at 0
  rewrites the line. `Sys_Print` (0x80c6478) wraps its `fputs` to stderr
  (0x80c6507) in the two (0x80c64be, 0x80c653e), so every print takes the
  typed line down and puts it back. VERIFIED.
- `Field_CompleteCommand` tokenizes the line (`Cmd_TokenizeString`
  0x805b398) and takes `argv[0]` past one leading `\` or `/`. Empty, it
  does nothing. VERIFIED. `FindMatches` (0x806d22c) runs over
  `Cmd_CommandCompletion` (0x805b21c), then `Cvar_CommandCompletion`
  (0x806f94c): a case-blind prefix compare, the first match copied, each
  later one cutting the copy where the two differ. VERIFIED.
- That cut runs over the later name's length only, so a later match that
  is a prefix of the copy leaves it whole. INFERRED. Both lists are walked
  newest registration first. INFERRED (Q3's `cmd_functions` and
  `cvar_vars`).
- One match: the line becomes `\%s` (0x80cfa68) of it, plus ` `
  (0x80cfa64) when the line was one token, or else `ConcatRemaining`
  (0x806d2f8): the old line past the word, found with `strstr`, or failing
  that each later argument after a space, quoted with `"` (0x80cfa66) when
  it holds one. VERIFIED.
- Several: the line becomes `\%s` of the shared part plus
  `ConcatRemaining`'s, `]%s\n` (0x80cfa6c) prints it, and `PrintMatches`
  (0x806e3ec) prints `    %s\n` for each name starting with the shared
  part, over the same two lists. VERIFIED.
- Exit: `Sys_Quit` (0x80c6ad8) and `Sys_Error` (0x80c54e0) print
  `Shutdown tty console` (0x80c6b24) and put back the saved settings with
  `tcsetattr(0, TCSADRAIN)` (0x80c6b35). VERIFIED.

vcod (`crates/server/src/stdin_console.rs`): a thread reads stdin a line at
a time and the frame loop pushes each non-blank line into the console queue
(`EngineCore::console`) ahead of `drain_console`. So a line runs on the
server thread like an rcon or Ctrl-C `quit`, in all three games. vcod ignores
`SIGTTIN` and `SIGTTOU` as retail does. EOF ends the thread. A read error
(`EIO` for a background process on its terminal) is retried once a second.

Under CoD1, on a terminal, with `ttycon` not set to 0 (`--set ttycon=0`),
`crates/server/src/tty_console.rs` runs the editor above instead: the same
raw mode, keys, history, flushes, completion, prints and exit restore. Its
thread blocks on `read` rather than polling, and a tab asks the frame loop
for the names, which it answers in `StdinConsole::queue`. CoD2 and CoD4
keep the line reader, as their `ttycon` is forced off (cod2-protocol.md
17.8, cod4-handshake.md 14.4). vcod differences:

- The names are vcod's: the commands its console runs, in
  `console::COMMANDS` order (and `COMMANDS_15` on 1.5 and UO), then the
  cvars sorted case-folded. The shared part is the true common prefix, so
  retail's uncut-prefix case above does not happen.
- The line stops at 255 characters.
- Output is the log's: `log::info!` lines on stderr with env_logger's
  prefix, each taken around the typed line as `Sys_Print` does. The
  completion lines print bare, as retail's.
- Retail's `stdin is not a tty` line is not printed.
- The terminal comes back from an `atexit` hook and the panic hook, so
  `quit`, Ctrl-C, `SIGTERM` and a crash all restore it.
- `--no-console` leaves stdin unread. Retail has no switch for that:
  `ttycon 0` only turns the line editor off.

Gates: `stdin_console::tests` (a pipe's line arrives, its EOF stops only
the reading, a line reaches CoD2's and CoD4's servers),
`server::tests::a_stdin_line_runs_on_the_console` (CoD1),
`tty_console::tests` (keys to line, history, flushes and completion) and
`server::tests::a_tab_gets_the_commands_and_cvars`.

### Pings (`SV_CalcPings`, 0x808cab8)

- `SV_SendMessageToClient` (0x808f680) stamps `frames[outgoingSequence & 31]`
  with `messageSent = svs.time`, `messageAcked = -1` for every message,
  gamestate (`SV_SendClientGameState` 0x8085eec) included. `SV_UserMove`
  (0x8086fa4), once a move message decodes, writes `svs.time` into
  `frames[messageAcknowledge & 31].messageAcked`, overwriting an earlier ack.
  INFERRED.
- Once a frame, before the clock advances (ahead of `SV_Frame`'s `svs.time +=
  frameMsec` loop): 999 unless `CS_ACTIVE` with a game entity; else the mean
  of `messageAcked - messageSent` over slots with `messageAcked > 0`, integer
  division, capped at 999 (999 with none), copied to `ps->ping` (`+0x20c8`).
  No bot branch. INFERRED.
- Both stamps step 50 ms and packets are read between frames, so an ack before
  the next frame counts 0, a frame later 50. VERIFIED (capture: a loopback
  probe's `status` ping read 0, 1 or 3 over 15 polls, one or two 50 ms samples
  in 32).
- `status` prints `%4i`; `getstatus` player lines are `%i %i "%s"\n`.
  VERIFIED.
- `status` reads `cl->ping`; `getstatus` (0x808bd58) writes score and
  `cl->ping` per slot past `CS_ZOMBIE`; the scoreboard shows `ps.ping` capped
  at 999, -1 while connecting (hud protocol doc 3). INFERRED.

The read schedule:

- The dedicated loop (`main` 0x80c6870, loop 0x80c69c0) is `usleep(5000)` then
  `Com_Frame` (0x806ce88), forever. VERIFIED (`push $0x1388`, `call usleep`,
  `call 0x806ce88`, `jmp 0x80c69c0`).
- `Com_Frame` runs `Com_EventLoop` (0x806bed4), handling every queued event
  (packets through `SV_PacketEvent`) until at least 1 ms passed, then
  `SV_Frame` (0x808cdf8) with the elapsed msec; `com_maxfps` counts only at
  `dedicated 0`. VERIFIED (decompile, 2026-10-19).
- Detail, VERIFIED (same): `Com_EventLoop` loops to a type-0 event, the pushed
  ring first, then `Sys_GetEvent` (0x806bdb0; its `0x80c61e4` queues the
  console line, then each `Sys_GetPacket` datagram, 0x80c71f0, `recvfrom` at
  0x80c723f, as type 6). Type 6 goes to `SV_PacketEvent` (0x808c870, called at
  0x806c1b8) while `sv_running`. It then drains the loopback queues
  (`0x8080fc0`, the server's at 0x806bfec) and returns the type-0 time.
  `Com_Frame` loops `do ... while (msec < min)` at 0x806cfe0..0x806d001 (`min`
  1, or `1000 / com_maxfps` when positive at `dedicated 0`), then
  `Cbuf_Execute` (0x805aa7c), then `SV_Frame` at 0x806d0e4.
- `SV_Frame` adds the msec to `sv.timeResidual` and runs a game frame once it
  reaches `1000 / sv_fps`, snapshots at the end. INFERRED.
- So retail reads the socket every 5 ms and stamps a move with the frame time
  current at arrival; only a packet landing during a frame's run gets that
  frame. `NET_Sleep` (0x80c786c, `select`) is called only from
  `SV_SpawnServer`'s map-change wait, so no frame wakes on a packet. INFERRED.
- The overwrite matters because the last move acking frame N is sent just
  before the client reads N + 1; stamped with N + 1's time it reads 50.
  Retail's window for that is its frame run. INFERRED.
- vcod: `Client::stamp_sent` / `stamp_acked` / `calc_ping`,
  `Server::calc_pings`. A reader thread stamps arrivals, the tick handles them
  in order at its start, and `Server::ack_stamp` stamps an ack with the newest
  frame whose messages had gone out at arrival (`Server::frame_sent`),
  treating retail's frame as instant. A packet held past
  `MAX_PACKETS_PER_FRAME` from an earlier tick takes the last frame's time.
  vcod bots read 0 (Q3's rule; retail has no bots).
- Trap: stamping with the tick's time widens the window to the whole tick. A
  loopback `--net-probe` (16 ms cmds) then read 21-50 on the debug build (25
  ms mean tick, loaded host), samples alternating 0 and 50. VERIFIED (capture,
  2026-10-08).
- 2026-10-08, loopback mp_harbor, one probe, 20 polls 1 s apart: retail
  (29561) `1 0 0 1 1 1 3 0 0 0 0 0 0 1 1 0 0 0 0 0`; vcod debug (29562) `3 0 1
  1 0 1 0 1 0 0 1 0 1 0 0 3 0 0 0 0`, tick 11-19 ms mean. VERIFIED (capture).

### A quiet client: `ClientEndFrame`'s dead 0x1000

Q3's `EF_CONNECTION` rule survives in all three game modules but never reaches
the wire: the entity's `eFlags` are overwritten from the playerstate right
after.

| module | stamp | `ClientEndFrame` | bit set | copies (`eFlags` source) |
|---|---|---|---|---|
| 1.1d game.mp.i386.so | `gclient+0x21e4` | 0x40e98 | 0x1000, 0x4112d..0x41153 | 0x41170..0x41191 (`ps+0x80`) |
| 1.5b game.mp.i386.so | `gclient+0x21ec` | 0x3ab2a | 0x1000, compare 0x3adfd, `or` 0x3ae0d | 0x3ae71, 0x3ae8d (`ps+0x80`) |
| UO 1.51b game.mp.uo.i386.so | `gclient+0x4628` | 0x42f59 | 0x800, compare 0x432ff, `or` 0x4330f | 0x43373, 0x4338f (`ps+0x84`) |

- 1.1d: `ClientThink` stores `level.time` (`level+0x1e8`) into the stamp after
  `trap_GetUsercmd` (0x41592). `ClientEndFrame` sets `ent+0x9` bit 0x10
  (`s.eFlags` 0x1000) when `level.time - stamp > 1000`, else clears it. Next
  on either branch, `BG_PlayerStateToEntityStateExtrapolate` (with
  `g_smoothClients`) or `BG_PlayerStateToEntityState` stores `ps.eFlags` over
  `s.eFlags` whole before setting its own bits. Nothing else in the module
  touches the bit. VERIFIED.
- 1.5b (with `G_SetClientSound` between) and UO: the same at the table's
  addresses, each copy storing the whole word. VERIFIED.
- 2026-10-20, mp_carentan, `client-probes/probe_passthru` with `probe_teleport
  1`: an axis probe SIGSTOPped 5 s while an allied one recorded. 1.1d:
  `pos.trTime` held 26466 from 26700 to 31500 (97 snapshots), `eFlags` 0x18
  throughout. UO: `pos.trTime` held 30300 for 97 snapshots, `eFlags` 0x18 raw,
  never 0x800. VERIFIED (capture).
- So the cgame's disconnected icon (gametypes doc 7.3, "The second sprite")
  never draws for a lagging player on a stock server. INFERRED. vcod-server
  leaves the bit off (`ClientSim::entity_eflags`).

### Rate (`SV_UserinfoChanged`, 0x8086ab4)

- `cl->rate` (what `status` prints) is 99999 when `Sys_IsLANAddress`
  (0x80c72f8) holds and `dedicated` is not 2; else the userinfo `rate` through
  `strtol`, clamped to 1000..90000, 5000 when empty or missing. INFERRED
  (control flow).
- The read is `__strtol_internal(s, 0, 10, 0)` at `0x8086b93` (`snaps`' at
  `0x8086d0a`). VERIFIED.
- Past `i32` glibc's `strtol` saturates before the clamp, as
  `vcod_common::text::atoi` (used by both vcod servers' `engine::link`) does.
  INFERRED.
- `rate 25000` from loopback read 99999 at `dedicated 1`, 25000 at `dedicated
  2`. VERIFIED (capture).

`Sys_IsLANAddress` (cod_lnxded `0x80c72f8`, CoDMP.exe `0x464be0`) is Q3
1.32's. VERIFIED (decompiles), check by check:

- Types: lnxded passes loopback (2) and IPX (5), CoDMP.exe also 0 (bot); any
  type but IP (4) then fails.
- CoDMP.exe alone passes exactly `127.0.0.1`.
- Class A (`ip[0] & 0x80 == 0`): first octet equals a local address's. Class B
  (`& 0xc0 == 0x80`): first two octets, or both in 172.16/12. Class C: first
  three, or both in 192.168/16.
- No RFC 1918 shortcut: 10/8 is plain class A; 172.16/12 and 192.168/16 need a
  local address in the block.

The local table is `NET_GetLocalAddress`: `gethostname`, `gethostbyname`, up
to 16 IPv4 addresses, printed `Hostname: %s`, `Alias: %s`, `IP: %i.%i.%i.%i`
(CoDMP.exe `0x465360`, table `0x8e3c78`, count `0x8e3cb8`; lnxded `0x80c7438`,
table `0x831a880`, count `0x831a864`). VERIFIED.

- lnxded keeps unix_net.c's off-by-one: the count starts at 1, slot 0 stays
  0.0.0.0, so 15 real addresses. VERIFIED.
- No `SIOCGIFCONF`; the only ioctl is `FIONBIO`. VERIFIED.
- A host whose name resolves only to 127.0.1.1 sees no LAN peer as LAN.
  INFERRED.

Callers. VERIFIED:

| binary | caller | effect for a LAN address |
|---|---|---|
| lnxded | `SV_GetChallenge` `0x8084d90` | no authorize request under `net_lanauthorize 0` |
| lnxded | `SV_DirectConnect` `0x8085498` | `sv_minPing`/`sv_maxPing` skipped |
| lnxded | `SV_UserinfoChanged` `0x8086ab4` | the rate above |
| lnxded | `SV_SendClientMessage` `0x808f680` | no `SV_RateMsec` throttling |
| CoDMP.exe | `CL_ReadyToSendPacket` `0x40b940` | docs/protocol-1.1.md, "The client's send rate" |
| CoDMP.exe | `CL_CheckForResend` `0x4103d0` | no key authorize under `net_lanauthorize 0` |
| CoDMP.exe | `0x452da0`, `0x453390`, `0x454d70`, `0x45dae0` | the listen server's copies of the four above |

No browser path calls it; the Local source is a broadcast `getinfo xxx`
(`CL_LocalServers_f` `0x413710`). VERIFIED.

vcod (`vcod_common::net::lan`) runs the same match on both sides against the
host's interface addresses (`getifaddrs` on Unix, loopback included) instead
of the hostname, so a 127.0.1.1 host still sees its LAN; on Windows it
resolves `COMPUTERNAME` as retail does.

### Bans (`SV_BanUser_f` 0x8084394, `SV_BanNum_f` 0x8084524)

- 1.1 has no ban file (no `ban.txt` string). Both commands find the client as
  `kick`/`clientkick` do, send OOB `banUser %i.%i.%i.%i` (0x80d3d7f) with its
  IPv4 address to `codauthorize.activision.com:20500`, and print `%s was
  banned from coming back\n` (0x80d3da0). No drop. A loopback client gets `e
  "EXE_CANNOTKICKHOSTPLAYER"`. INFERRED (0x8084394's control flow).
- Capture (`banUser vcod`, `banClient 0`, a UDP listener at the authorize
  name's address). VERIFIED:
  - usage `Usage: banUser <player name>\n` (0x80d3ce6), `Usage: banClient
    <client number>\n` (0x80d3dc0); misses `Player nobody is not on the
    server`, `Client 3 is not active`, `Bad slot number: x`;
  - the first ban: `Resolving codauthorize.activision.com\n`,
    `codauthorize.activision.com resolved to <ip>:20500\n`, `vcod was banned
    from coming back\n`; later ones the last line only;
  - the listener read `\xff\xff\xff\xffbanUser 127.0.0.1` from the server's
    port each time; the probe stayed connected.
- The ban bites only through the authorize server's answer to a later
  `getIpAuthorize`, sent by `SV_GetChallenge` for an off-LAN client.
  `SV_AuthorizeIpPacket` (0x808514c) relays `deny` as
  `error\nEXE_ERR_CDKEY_IN_USE` (empty reason or `INVALID_CDKEY`), `needcdkey`
  (`CLIENT_UNKNOWN_TO_AUTH`, `BAD_CDKEY`), else `error\nEXE_ERR_BAD_CDKEY`
  (`BANNED_CDKEY` included). INFERRED (0x8085339..0x80853d9). The reason for a
  banned address cannot be measured; the server no longer answers.
- vcod: no authorize detour; its list is 1.5's `ban.txt` (`engine::bans`,
  `--ban-file` to keep it), a line per client of its guid stand-in (off-LAN
  IPv4 as a big-endian `i32`, LAN 0) and cleaned name. A banned guid's
  `getchallenge` gets `error\nEXE_ERR_BAD_CDKEY`; a LAN client has no guid and
  is never refused, as on retail. No `Resolving` lines.

### 1.5 temporary bans (`tempBanUser` 0x8087f2a, `tempBanClient` 0x8088087)

cod_lnxded 1.5b, from the asm.

- Registered beside `banClient` (names 0x80dddae, 0x80dddba; handlers stored
  at 0x80887ed, 0x8088801). VERIFIED.
- Each calls `kick`'s and `clientkick`'s helper (0x8087d89 by name, 0x8087e9d
  by slot) with a 64-byte name buffer. Its tail (0x8087ceb): `e
  "EXE_CANNOTKICKHOSTPLAYER"` (0x80dda69) for loopback; else copy and clean
  the name (`cl+0x10a44`, 0x80860de), keep `cl+0x5a980`, drop with
  `EXE_PLAYERKICKED` (0x80dda86), return the kept value. VERIFIED.
- `cl+0x5a980` is the guid: the connect copies the challenge's `+0x2c`
  (0x808a1d0) there (0x808a91a), which the authorize `accept` sets with `atoi`
  (0x8089696, `accept` at 0x80de100). VERIFIED.
- Guid 0 stops after the kick (`je` at 0x8087f4a). VERIFIED. A LAN client is
  never asked about, so its guid is 0 (`getGuid` 0 in the 1.5b log). INFERRED.
- Otherwise it prints `%s (guid %i) was kicked for cheating\n` (0x80ddb00) and
  calls 0x80890f5, writing guid and the scaled game clock (0x83ccd84,
  `cod15-protocol-diff.md` 12) into a 16-entry, 8-byte table at 0x83e1e0c.
  VERIFIED. The entry is the first with guid 0, else the lowest time
  (0x8089096's loop). INFERRED.
- The check (0x8088f75, called at 0x808979c on the `accept` path) passes guid
  0 and refuses an entry whose `svs.time - time` is not above
  `sv_kickBanTime->value * 1000.0` (cvar 0x836b7c8, `"sv_kickBanTime"` default
  `"300"` at 0x80dfb1f, 1000.0 at 0x80ddfe4). VERIFIED (operands). The `ja` at
  0x8088fda gives "not above". INFERRED.
- Refusal: prints `rejected connection from temporarily banned GUID %i\n`
  (0x80de1a0), sends `error\n\x15You are temporarily banned from this server`
  (0x80de1e0). VERIFIED (strings).
- The cvar is read at the check, so a change rescales existing bans; nothing
  empties an entry. INFERRED.
- vcod: an off-LAN address stands in for the guid (`engine::bans::guid_of`); a
  LAN client is kicked, not banned; the refusal is retail's string at
  `getchallenge` (`Server::temp_ban_slot`, `engine::bans::TempBans`, shared
  with CoD2, whose table is the same). The prints carry the stand-in; script
  `getGuid` stays 0. The table runs on the game clock as retail's (the
  unscaled clock would age a ban through a match timeout).

### 1.5 permanent bans (`banUser` 0x8087f73, `banClient` 0x8087fc7, `unbanUser` 0x808801b)

cod_lnxded 1.5b, from the asm. UO matches it instruction for instruction.
VERIFIED (`coduo_lnxded` 1.51b, jumps relative, absolute addresses masked):

| piece | 1.5b | UO 1.51b |
|---|---|---|
| `banUser`, `banClient`, `unbanUser` (70 instructions) | 0x8087f73, 0x8087fc7, 0x808801b | 0x8088d74, 0x8088dc8, 0x8088e1c |
| file check, ban, unban (275 instructions) | 0x8088ff8..0x80893d0 | 0x8089dfc, 0x8089f25, 0x808a021 |
| guid | `cl+0x5a980` | `cl+0xab080` |
| address type | `cl+0x52948` | `cl+0x9b048` |
| file check called from `accept` | 0x80896bc | 0x808a4c0 |

UO's strings: `.rodata` 0x80e3f28 (`ban.txt`), 0x80e3f66 and 0x80e3f84 (unban
replies), 0x80e4060 (refusal print). So the 1.5b measurements below hold on
UO. INFERRED (the match).

- No authorize detour: no `banUser <ip>` send, no `was banned from coming
  back` string. VERIFIED.
- `banUser`/`banClient` print `Server is not running.\n` (0x80dd74f) without a
  level and usage unless two arguments, look up as 1.1 (0x8087300 by name,
  0x8087417 by slot), then call 0x8089121. VERIFIED.
- 0x8089121: loopback gets `e "EXE_CANNOTKICKHOSTPLAYER"` (0x80ddff0). Guid 0
  returns at once: a LAN client is neither banned nor kicked, silently.
  VERIFIED (asm; rcon on 1.5b against a LAN probe: both commands answered
  `print\n` alone, the probe stayed, no `ban.txt`).
- A guid already listed returns too. Else: open `ban.txt` (0x80ddfe8) for
  append (0x8065e76, mode 2), return on failure, write `%i %s\r\n` (0x80de00d)
  with guid and `Q_CleanStr`'d name, close, drop with `EXE_PLAYERKICKED`
  (0x80de015), store 0x83ccd88 in `cl+0x10c1c`. VERIFIED. That store restarts
  the packet clock (Q3's `SV_KickNum_f`). INFERRED.
- File check (0x8088ff8): guid 0 passes; else read `ban.txt` via the search
  path (0x8062fbd), each line's first token (0x8085323) an `atoi` guid, rest
  skipped (0x80854d2). Runs at `accept` before the temporary check (0x80896bc,
  then 0x808979c). A hit prints `rejected connection from permanently banned
  GUID %i\n` (0x80de120), sends `error\n\x15You are permanently banned from
  this server` (0x80de160), clears the challenge. VERIFIED.
- `unbanUser` (0x808921d): two arguments, else `Usage: unban <client name>\n`
  (0x80ddb82); silent without `ban.txt`. It cleans the argument; per line
  skips the guid token and following blanks and matches when the rest equals
  the name, ignoring case, up to `\r` or `\n`; matches are cut. The buffer is
  always written back, then `unbanned %i user(s) named %s\n` (0x80de026) or
  `no banned user has name %s\n` (0x80de044) with the cleaned name. INFERRED
  (control flow).
- rcon on 1.5b. VERIFIED: `unbanUser ^1foo` on `12345 Foo\r\n67890 Bar\r\n`
  gave `unbanned 1 user(s) named foo`, leaving `67890 Bar\r\n`; `1 Dup\n2
  dup\r\n3 Other\n4 Baz` lost both dups (`unbanned 2`) and `baz` on an
  unterminated last line was not found; `5   Spaced Name\r\n` went for
  `"spaced name"`; no file gave an empty `print\n`.
- vcod: 1.5/UO `banUser`/`banClient` use an off-LAN address as the guid
  (big-endian `i32`) and leave LAN clients alone, as retail does guid 0. The
  file is retail's format and handling (`engine::bans::BanFile`, also CoD2's
  `ban.txt`, `cod2-protocol.md` 17.7, at `--ban-file`), checked at
  `getchallenge` before the temporary bans with retail's print and refusal;
  `unbanUser` gives retail's three replies (empty for no file). A ban appended
  to an unterminated last line runs into it, as on retail. Old vcod `<ip>
  <name>` lines are rewritten as guid lines on open.

### Shutdown (`SV_Shutdown` 0x808ad8c)

- `SV_FinalMessage` runs twice over every slot past `CS_ZOMBIE`: for a
  non-loopback client `e "%s"` with the reason (type 0, so an inactive client
  drops it) and a bare `w` (0x80d57e1, type 1), then `nextSnapshotTime = -1`
  and `SV_SendClientSnapshot`. `quit` (0x806d910) passes `EXE_SERVERQUIT`,
  `killserver` `EXE_SERVERKILLED`. INFERRED.
- `rcon quit`: the probe read `e "EXE_SERVERQUIT"`, `w`, and dropped, reading
  nothing more, so the second pass is unseen. VERIFIED (capture).
- `killserver` (0x8084d3c) calls `0x806dc68`: `SV_Shutdown` between the hunk
  calls, then return; no exit. `SV_Frame` does the same on `sv_killserver` and
  resets it. With `sv_running` (0x833efc0) 0 the event loop skips
  `SV_PacketEvent` (0x808c870, returning to 0x806c1bd). INFERRED.
- `rcon killserver`: no reply; the probe read `e "EXE_SERVERKILLED"` and
  dropped; then rcon `status`, rcon `map mp_harbor` and `getinfo` went
  unanswered with the process up. Only the stdin console could load a map.
  VERIFIED (capture).
- The stdin read's tests are `dedicated` and `stdin_active` (above), which
  `SV_Shutdown` leaves alone, so stdin keeps being read. INFERRED.
- vcod: the same. Checked 2026-11-13 on a pty and on a pipe: after
  `killserver` on stdin `getinfo` went unanswered, and a stdin `map
  mp_harbor` (CoD4: `map mp_crash`) brought the server back.

### Out-of-band `disconnect`

- `SV_ConnectionlessPacket` (0x808c63c) matches `"disconnect"` at 0x808c827
  and does nothing; any other unknown command prints `bad connectionless
  packet from %s:\n%s\n`. VERIFIED (decompile). Clients leave through the
  netchan `disconnect`. vcod ignores it too.
- The server never sends one; the client's rule matters. `CL_DisconnectPacket`
  (CoDMP.exe 0x410620) acts only if the state at 0x155f2c0 is non-zero (0
  disconnected; 3 set at `connectResponse`, 0x410c7b), the packet is from the
  server's address (`NET_CompareAdr` 0x449230 against 0x15ef028), and
  `cls.realtime` (0x155f3e0) minus `clc.lastPacketTime` (0x15ce868) is at
  least 3000 (`cmp eax,0xbb8; jl` at 0x410663); then `Com_Error(1,
  "EXE_SERVER_DISCONNECTED")`. INFERRED.
- `clc.lastPacketTime` is written at `connectResponse` (0x410c85), every
  sequenced packet from the server's address at state 3+ before
  `Netchan_Process` (0x4110b1, so a rejected packet still counts), and demo
  playback (0x40eac4); never by a connectionless packet. INFERRED.
- So a forged `disconnect` cannot cut a fed link, and one arriving while
  challenging reads a stale or zero stamp and drops at once.
- vcod's client (`NetClient::handle_oob`, `crates/common/src/net/mod.rs`,
  `DISCONNECT_GUARD`) has the same guard and stamps at the same two live
  points. Never run against CoDMP.exe: nothing sends this packet.

### The challenge key (`SV_ConnectionlessPacket` 0x808c63c, `Info_SetValueForKey` 0x80827d4)

2026-10-16, stock 1.1d, 1.5b and UO 1.51b on loopback (`tools/run_server.sh`,
no client), one `getinfo` or `getstatus` per packet. All three answered every
case alike. VERIFIED (captures).

- A bare `getinfo`/`getstatus` (or `getinfo ""`) has no `challenge` key;
  `infoResponse` opens `\protocol\...`. VERIFIED.
- `challenge` is `Cmd_Argv(1)`. `MSG_ReadStringLine` stops at NUL or newline
  and turns `%` and bytes above 127 into `.` (`a%b` -> `a.b`, UTF-8 `é` ->
  `..`, `0xff` -> `.`, `0x7f` kept). VERIFIED.
- `Cmd_TokenizeString` (0x805b398) splits on any byte <= space (`0x01`, `\r`,
  tab end a token); `"` opens a token to the next `"` or line end (`"a b" c`
  -> `a b`, `"ab` -> `ab`, `a"b` -> `a`); `//` ends the line, `/* */` is
  skipped (`a//b`, `a/*x*/b` -> `a`; `/*x*/b` -> `b`). VERIFIED.
- `Info_SetValueForKey` (0x80827d4) drops `\`, `;` and `"` from the value
  (`a\b`, `a;b` -> `ab`), moves the key to the end, adds nothing for an empty
  value; any of those three in the key is a `Com_Error`. VERIFIED (decompile).
  No cap of its own: 200, 600 and 750-byte challenges came back whole.
  VERIFIED.
- `Cvar_InfoString` (0x806fc30; `getstatus`' serverinfo and configstring 0)
  puts each cvar through 0x80827d4, `Cvar_InfoString_Big` (0x806fc84,
  systeminfo) through 0x80829e8, which strips the same into an 8 KB string.
  VERIFIED (decompile).
- So a cvar keeps `;` and `\` and only the info string loses them. VERIFIED
  live: `sv_hostname a;b\c` came back `\sv_hostname\abc` in 1.1d's `getstatus`
  and `\hostname\abc` in 1.5b's and UO's `getinfo`; `sv_hostname "x y;z"` as
  `\hostname\x yz` in 1.1d's `getinfo`. vcod's `Info::set` strips the same.
- A `"` reaches a cvar only from a script; no rcon or `+set` line carries one.
  VERIFIED live 2026-10-20, all three, `client-probes/probe_svinfoquote` on
  mp_carentan, one client, getinfo/getstatus from a socket
  (`crates/server/tests/fixtures/handshake/svinfo-quote.txt`):
  `setCvar("sv_hostname", "q\"h;s\\b e")` reads back whole while configstring
  0, `getinfo` and `getstatus` carry `qhsb e`; `"lead` gives a `d 0` with
  `lead`; a lone `"` drops the key from configstring 0 and both replies;
  `tail"` restores it as `tail`. vcod matches (`connect.rs`,
  `a_quote_in_a_serverinfo_value_is_stripped_like_retail`).
- The limit is 1024 bytes (`> 0x400` at 0x80827d4's end). 1.1d's mp_carentan
  `infoResponse` holds 123 bytes beside `\challenge\`: an 890-byte challenge
  (exactly 1024) was answered, 891 crashed the process (exit 139), 900 printed
  `ERROR: Info string length exceeded` and dropped the level, leaving the
  process idle with no map. A query can take a 1.1d server down. VERIFIED.
- 1.5b and UO print `Info string length exceeded` with the key, skip the pair
  and go on: a 1000-byte `getinfo` came back `\challenge\<1000>\protocol\6`
  alone; a 1000-byte `getstatus` as the serverinfo without `challenge`,
  `\pswrd\0\mod\1` still at the end. VERIFIED.
- vcod (`oob_argv` and `Server::info_pairs` in `crates/server/src/server.rs`,
  `Info::set_value_for_key` in `crates/common/src/net/connectionless.rs`)
  reads, tokenizes and strips the same, skips an overflowing pair on 1.5/UO,
  and on 1.1 sends no reply and keeps its level. A loopback A/B of these cases
  matched retail byte for byte on all three protocols, except 1.5b's archived
  `g_timeoutsAllowed 2` from its homepath config.

### `fs_restrict`

Q3's demo restriction, in all three servers. Addresses are cod_lnxded 1.1d
unless named.

- `Cvar_Get("fs_restrict", "", CVAR_INIT)` at 0x8061d68: only the command line
  sets it; a later `set` is write protected. VERIFIED (strings, calls).
- `FS_SetRestrictions` (0x80713e8; 1.5b ~0x8075af8, UO 0x807625a): if
  non-zero, set `1`, print `Running in restricted demo mode.`, and on 1.1/1.5
  restart the filesystem on `demomain` (0x80d03c4; 1.5b 0x80da244); UO has no
  `demomain` and keeps its path. Every pak must checksum 0xb1f595f5 (1.1 `cmp`
  at 0x8071461; 1.5b and UO compare checksum xor 0x2261994 with 0xb3d38c61),
  else `Corrupted pak0.pk3: %u` is fatal. VERIFIED (strings, calls).
- On the stock 1.1 install `+set fs_restrict 1` stopped with `Sys_Error:
  Couldn't load default_mp.cfg` (no `demomain/`); with the stock paks copied
  there under other names, `Sys_Error: Corrupted pak0.pk3: 1252304247` (pak6's
  checksum). Only the CoD demo's data passes; no full install runs restricted.
  VERIFIED.
- Restricted (or with server paks set), `FS_FOpenFileRead` serves only `.cfg`,
  `.menu`, `.svg`, `.game`, `.dm_1`, `.dat` from a directory, not a pak
  (pointer 0x8339c9c read at 0x805d6df, suffixes 0x80cd875..0x80cd931).
  VERIFIED (strings, calls).
- `SVC_Status` and `SVC_GameCompleteStatus` (0x808bda3, 0x808bff3; 1.5b
  0x80924c5, UO 0x809336d) test `Cvar_VariableValue("fs_restrict")` after the
  `challenge` key and set `sv_keywords` to `demo <sv_keywords>` (`demo %s` at
  0x80d59ed) via `Info_SetValueForKey`, moving it to the end before `pswrd`.
  `SVC_Info` has no such test. VERIFIED (calls). Unseen on the wire: no
  restricted server starts here.
- `SV_AuthorizeIpPacket` (0x808514c) answers an authorize `demo` verdict with
  `challengeResponse` when restricted, `error\nEXE_ERR_NOT_A_DEMO_SERVER`
  otherwise. INFERRED (decompile).
- vcod-server (`main.rs`): a non-zero `--set fs_restrict` searches `demomain`
  for `main` (UO's `uo` stays) and stops with `Corrupted pak0.pk3: %u` on the
  first non-demo pak, or `Couldn't load default_mp.cfg` without one (the stock
  1.1 install printed the same `1252304247`). The status builders add `demo`.
  No authorize, so no `ipAuthorize` branch; the directory filter is not
  ported, as no restricted vcod-server gets past its paks.

### Private slots (`SV_DirectConnect` 0x8085498)

2026-10-09, `cod_lnxded` (1.1d), `sv_maxclients 8`, `+set sv_privateClients 2
+set sv_privatePassword pp`, a throwaway `NetClient` example joining clients
with chosen `password` values in turn, `rcon status` for the slots.

- `sv_privateClients` default `"0"`, flags 4 (`CVAR_SERVERINFO`);
  `sv_privatePassword` default `""`, flags 0x100 (`Cvar_Get` 0x806ea34, names
  pushed at 0x808a9cf and 0x808ab7d in `SV_Init`). VERIFIED.
- Both are live: an rcon change governed the next connect and `getinfo`.
  VERIFIED (capture).
- The reconnect search runs first, unchanged: an address and qport (or port)
  holding a slot, live or zombie, takes it back whatever the password.
  VERIFIED (capture: private slot 0 reconnecting passwordless from its qport
  got slot 0).
- A new client: `Info_ValueForKey(userinfo, "password")` (key 0x80d44e3)
  `strcmp`ed with `sv_privatePassword` (0x8085a26); equal searches free slots
  from 0, else from `sv_privateClients->integer` (0x8085a36). Free means state
  0, so a zombie is not. None free sends `error\nEXE_SERVERISFULL` (0x80d44ec)
  and prints `Rejected a connection.` (0x80d4503) at developer level.
  INFERRED.
- No Q3 local-address fallback for bots. VERIFIED (capture: with both private
  slots free, six passwordless clients took 2..7 and a seventh got
  `EXE_SERVERISFULL`; then `pp` clients took 0 and 1 and a third got
  `EXE_SERVERISFULL`; `password wrong` searches like none).
- An empty `sv_privatePassword` equals a missing `password`, so that client
  searches from 0: after `sv_privatePassword ""` a passwordless client got
  slot 1 (0 held), `password wrong` slot 2. Private slots bite only with a
  non-empty password. VERIFIED (capture).
- `g_password` reads the same key, so a private client on a passworded server
  needs both equal. INFERRED.
- `SVC_Info` (0x808c1ac) counts `clients` over slots from `sv_privateClients`
  up with state > 1 and sends `sv_maxclients - sv_privateClients`, unclamped;
  `getstatus` lists every player and carries `sv_privateClients`. VERIFIED
  (capture: `clients 6 sv_maxclients 6` with six public and two private in;
  `sv_privateClients 12` read `sv_maxclients -4`).
- `SV_AddTestClient` searches from 0 (0x8087548), ignoring private slots.
  INFERRED. vcod's bots too.
- vcod: `svc_direct_connect` keeps its reconnect and zombie arms and searches
  from `Server::private_clients` unless `password` matches; `svc_info` and the
  serverinfo follow the cvars live. Same slots, refusals and `getinfo` as
  retail.

### The serverinfo string follows every write

- `SV_Frame` tests `cvar_modifiedFlags & CVAR_SERVERINFO` once a frame and
  rewrites configstring 0 with `Cvar_InfoString(CVAR_SERVERINFO)` (map-cycle
  doc 4.3). The flag, not the writer, decides, so a script `setCvar` of
  `sv_privateClients` or `sv_hostname` reaches configstring 0 and `getstatus`
  in the same frame as a console `set`. INFERRED (Q3 lineage, the flag).
- The console half (an rcon change in the next `getinfo`, "Private slots").
  VERIFIED (capture).
- `Cvar_InfoString` skips empty flag-4 cvars, so `sv_keywords` and
  `sv_mapname` (empty on stock) never show. VERIFIED (the `S` rows of
  `crates/server/src/cvars/registry.rs` are the capture's keys plus those
  two).
- `SVC_Info` reads the live `sv_hostname->string`. INFERRED (Q3 lineage).
- vcod: `Server::serverinfo` builds the string from the level's cvar table
  (every `S` cvar with a value, case-folded order, `sv_pure` from the pak
  lists); `Server::flush_serverinfo` compares it with configstring 0 once a
  tick after the script; `getinfo` reads the live `sv_hostname`.

### Ping limits (`SV_DirectConnect` 0x8085498)

- The challenge record (0x2c bytes at `0x83b67f8`): address +0, challenge
  +0x14, `time` +0x18, `pingTime` +0x1c, `firstTime` +0x20, measured ping
  +0x24, `connected` +0x28. VERIFIED (writes at `0x8084e65`..`0x8084e9c`).
- `pingTime` is stamped when `challengeResponse` goes: at once for a LAN
  client under `net_lanauthorize 0` (`0x8084edc`), else on the authorize
  answer (`0x8085208`). VERIFIED (code).
- At the connect, challenge matched: a stored ping of 0 becomes `svs.time -
  pingTime` (`0x8085795`), `Client %i connecting with %i challenge ping`
  (`0x80d43e0`) prints, the challenge is marked connected. VERIFIED (code).
- Off the LAN only (`Sys_IsLANAddress` at `0x80857ee`), as floats (`->value`);
  the keys name what the server accepts. VERIFIED (strings `0x80d440d`,
  `0x80d4466`, `0x80d4440`, `0x80d44a0`; compares at `0x808581b`,
  `0x8085880`):

| test | client gets | console |
|---|---|---|
| `sv_minPing` set, ping below | `error\nEXE_ERR_HIGH_PING_ONLY` | `Client %i rejected on a too low ping` |
| `sv_maxPing` set, ping above | `error\nEXE_ERR_LOW_PING_ONLY` | `Client %i rejected on a too high ping: %i` |

- Taking a slot clears the stored ping (`0x8085c16`), so a later connect on
  that challenge measures again. VERIFIED (code).
- `SVC_Info` adds `minPing`, `maxPing` (`%i` of `->integer`, each only when
  non-zero) after `pure`, then `game` (`fs_game`, when set),
  `sv_allowAnonymous`, `pswrd` (key pushes at `0x808c318`, `0x808c344`).
  VERIFIED (code).
- vcod: `svc_direct_connect` keeps the first connect's ping on the challenge
  and runs both tests off the LAN; `svc_info` writes both keys, and `game`
  under `--set fs_game` (matched against retail, `cod11-front-end.md` 18,
  "vcod"). Without the authorize detour, vcod's ping is the
  `challengeResponse`-to-`connect` round trip.

### The challenge table (1.1d `SV_GetChallenge` 0x8084d90, `SV_DirectConnect` 0x8085498, `SV_DropClient` 0x8085cf4)

Read 2026-10-23 off the decompiles and 1.1d's disassembly, with 1.5b, UO 1.51b
and CoD2 1.3. All keep Q3's table but for the record size.

| | 1.1d | 1.5b | UO 1.51b | CoD2 1.3 |
|---|---|---|---|---|
| table, record | 0x83b67f8, 0x2c | 0x83ccde0, 0x54 | 0x84f7060, 0x54 | 0x84230dc, 0x74 |
| `SV_GetChallenge` | 0x8084d90 | 0x80889d0 | 0x80897d4 | 0x808d18e |
| `SV_DirectConnect` | 0x8085498 | 0x8089e7e | 0x808ac82 | 0x808e2aa |
| `SV_DropClient` | 0x8085cf4 | 0x808ac11 | 0x808ba15 | 0x808f02e |
| address compare | 0x8080dec | 0x808400f | 0x8084af7 | 0x806c54c |
| slot times | `svs.time` 0x83b67a4 | 0x83ccd88 | 0x84f7008 | `svs.time` 0x8423084 |
| challenge's xor | 0x83b67a4 | 0x83ccd84 | 0x84f7004 | 0x8423084 |

VERIFIED (each address in its decompile: the 0x400 bound, the stride, the
writes).

- 1024 slots in each (bound 0x400). VERIFIED.
- No slot expires: 1.1d names the base only in `SV_GetChallenge`, the
  authorize reply (0x808514c), `SV_DirectConnect` and `SV_DropClient`, and
  only the authorize wait reads an age (`svs.time - firstTime < 5001`: ask
  again or answer). VERIFIED (every 1.1d reference to 0x83b67f8).
- The other three have the same four users. INFERRED (only the three functions
  above read).
- `getchallenge`: reuses the first slot with `connected` 0 and the same
  address, tracking on the way the lowest `time` (+0x18), first of a tie,
  connected included. No match overwrites that one: address; challenge `rand()
  << 16 ^ rand() ^` the xor clock; `time` and `firstTime` (+0x20) the slot
  clock; ping (+0x24) and `connected` (+0x28) 0. A re-asked slot keeps its
  `time`, so the oldest written goes, not the longest unasked. VERIFIED
  (0x8084d90's loop and writes; same writes at 0x80889d0, 0x80897d4,
  0x808d18e).
- The answer-at-once path (`net_lanauthorize` 0 and LAN, or the authorize
  server timed out) stamps `pingTime` (+0x1c) on the slot clock and sends
  `challengeResponse`. VERIFIED.
- All three places compare with `NET_CompareAdr`: type, then for `NA_IP` four
  bytes and port (1.1d 0x8080dec, port last; 1.5b, UO, CoD2 via
  `NET_CompareAdrSigned`, port first). `NET_CompareBaseAdr` (1.1d 0x8080e60,
  string `NET_CompareBaseAdr: bad address type`) is not used. VERIFIED (1.1d
  `call 0x8080dec` at 0x8085702; the others by name or string).
- `connect`: for a non-loopback address (`NET_IsLocalAddress`, 1.5b
  0x808403c), the first slot, connected or not, matching address and
  challenge; none gives `error\nEXE_BAD_CHALLENGE`. A stored ping of 0 becomes
  `slot clock - pingTime`; `connected` is set. Taking a slot clears the ping
  (1.1d 0x8085c16; 1.5b, UO, CoD2 at their `connectResponse`). VERIFIED.
- `SV_DropClient`: the first slot with the client's address gets `connected`
  0, and the loop stops. VERIFIED. Skipped when `cl+0x10a40` (1.1d, 1.5b; CoD2
  `cl+0x20c44`) is set, which vcod reads as the bot flag. INFERRED.
- 1.5b and UO time slots on the unscaled clock (0x83ccd88, the pacing's,
  `cod15-protocol-diff.md` 12) and xor with the scaled one (0x83ccd84).
  VERIFIED (operands).
- vcod: `engine::challenge::Challenges` serves both servers, timed on
  `svs_time_ms()` (CoD1) and `svs.time` (CoD2); the draw is `(rand() << 16) ^
  rand() ^ sv_time_ms` (the game clock, 1.5's 0x83ccd84), CoD2's on
  `svs.time`. Address and port must match, so a connect from another port is
  refused, as on retail.

### Message pacing (`SV_SendClientMessages` 0x809045c)

- Each frame a due client (`nextSnapshotTime` reached) gets an unsent fragment
  or a new message from `SV_SendClientSnapshot` (0x808f844). VERIFIED (code).
- That writes server commands and snapshot only for an active client or
  zombie, `SV_WriteDownloadToClient` for any but a zombie, then `svc_EOF`. A
  downloading (inactive) client's messages hold the reliable ack and its
  blocks only. VERIFIED (code; state tests at `0x808f855`, `0x808f8a1`,
  `0x808f92a`).
- `SV_SendMessageToClient` (0x808f680) then sets `nextSnapshotTime`: `svs.time -
  1` (next frame) for loopback or LAN; else `svs.time + max(snapshotMsec,
  (min(bytes, 1500) + 48) * 1000 / rate)`, `rate` capped by a non-zero
  `sv_maxRate` (floored at 1000), and at least 1 s for a client neither active
  nor downloading. VERIFIED (code).
- Later fragments go one per turn, each setting `svs.time + (min(bytes left, 1500) + 48) *
  1000 / rate`, no LAN shortcut (`0x80904d9`..`0x8090546`).
  VERIFIED (code).
- A loopback download from 1.1d: three blocks a message (LAN rate 99999 over
  50 ms), a message every 200-300 ms, one fragment a frame. VERIFIED (capture,
  2026-10-09, loaded host).
- Off the LAN, 2026-10-11: 1.1d on `mp_carentan` dm in one namespace
  (203.0.113.1), a spectating probe in another (198.51.100.2) behind a logging
  relay over a veth pair, `--probe-rate`/`--probe-snaps` setting the userinfo.
  VERIFIED (capture):

| `rate` | `snaps` | snapshots |
|---|---|---|
| 5000 | 20 | 19-21 bytes, 50 ms apart (the `snaps` floor; the rate's 14 ms is under it) |
| 25000 | 10 | 100 ms apart (the `snaps` floor) |
| 1000 | 30 | 20 bytes, 100 ms apart (the rate's 68 ms rounded to a frame); the first (304 bytes) followed 350 ms later, its own `(304 + 48) * 1000 / 1000` |

- First snapshot after the gamestate (2026-10-12, spectator, `mp_carentan`
  dm): 293-304 bytes on retail. Decoded (290 bytes): five server commands, the
  playerstate from null (720 bits), twelve entities at 1-2 bytes against their
  baselines, the two turrets at 8-9 (barrels moved: `angles2[0]` -20 to -63
  and -72), one client. VERIFIED (capture).
- vcod baselines what it sends after the settle frames (map-cycle doc 3.3);
  its first snapshot decodes to retail's 2448 bits (293-294 bytes). Trap:
  without baselines every entity goes against null at 12-34 bytes, 566 in all.
- vcod: `Client::pace`, `Client::first_packet` and `Client::next_fragment`
  apply these rules to every message: frame-loop snapshots and downloads, and
  the gamestate.

### Clients not in the world: the bare message and the 1 s keepalive

- `SV_SendClientMessages` (0x809045c) walks every slot not `CS_FREE` (`cmpl
  $0x0,(%ebx)` at `0x80904a0`), so `CS_CONNECTED` and `CS_PRIMED` clients get
  a message each turn. VERIFIED (code).
- For them `SV_SendClientSnapshot` (0x808f844) writes no commands or snapshot
  (tests at `0x808f855`, `0x808f8a1` pass 4 and 1 only), then the download,
  then `svc_EOF`. With no download: 4-byte sequence, 4-byte reliable ack, one
  compressed `svc_EOF` byte, 9 bytes. VERIFIED (code).
- `SV_SendMessageToClient` (0x808f680) floors a client neither `CS_ACTIVE` nor
  downloading (byte `+0x10a64`) at `svs.time + 1000` (`0x808f806..0x808f824`);
  a LAN client takes `svs.time - 1` first, so a message a frame. VERIFIED
  (asm).
- Every switch to `CS_ACTIVE` sets `nextSnapshotTime = svs.time`:
  `SV_ClientEnterWorld` (0x80877d8, store `0x8087838`), the entering cmd in
  `SV_UserMove` (0x8086fa4, its `CS_PRIMED` branch),
  `SV_ExecuteClientMessage`'s restart path (0x80872ec), the bot add
  (0x808753c); `SV_DirectConnect` (0x8085498) at connect too. So the first
  snapshot goes next frame, not after the keepalive. VERIFIED (code).
- 2026-10-10, 1.1d `mp_carentan`, a relay dropping the probe's sequenced
  packets for 3-3.5 s after `connectResponse` and again after the first
  gamestate fragment (so `CS_CONNECTED`, then `CS_PRIMED`). VERIFIED
  (capture):
  - Loopback: 9 bytes every 46-51 ms in both states, 63 before the gamestate.
  - Off the LAN (198.51.100.2 to 203.0.113.1 over a veth pair): 9 bytes every
    995-1003 ms while connected; second gamestate fragment 976 ms after the
    first, the rest 98-102 ms apart (rate); a 9-byte message 51 ms after the
    last, then every 1000 ms while primed; the first snapshot 15 ms after the
    entering cmd.
- vcod: `Server::send_unentered` sends a connected or primed client its
  fragment, blocks or bare message on its turn, paced by `Client::pace`;
  `enter_world` makes it due at once. Both captures matched: a 9-byte message
  a frame on loopback; off the LAN 1000 ms apart in both states, the second
  fragment 926 ms after the first, the first snapshot a tick after the
  entering cmd. (vcod answers at the next tick start, retail between frames;
  both add 1000 to the last frame's time.) VERIFIED (capture, same relay).
- 2026-10-18, `--probe-edge watch` joining 1.1d on loopback: gamestate is
  message 1, first snapshot message 2, as retail handles the connect and the
  first client message on arrival between frames and the client answers each
  before the next frame. vcod-server's loop handles an inter-tick packet at
  once and sends its answer: also 1 and 2. VERIFIED (capture). Trap: reading
  only at tick start and sending at tick end makes them 2 and 3.
- A packet found queued at tick start (it arrived while the last tick overran)
  is answered there, before the tick, not with its output. Retail's
  `Com_EventLoop` runs queued handlers before `SV_Frame` ("Pings", the read
  schedule), and `NET_SendPacket` (0x8080d28) writes the socket inside the
  handler: loopback to the loopback ring, everything but the bot and bad types
  to `Sys_SendPacket` (0x80c7914, `sendto` at 0x80c79ba), which the OOB
  senders 0x8080920 and 0x8080a8c call likewise. VERIFIED (decompile).

### The gamestate goes out fragmented (`SV_SendClientGameState` 0x8085eec)

- First it flushes every fragment left of an earlier message
  (`Netchan_TransmitNextFragment` 0x808dcf8, called at `0x8085f34`, looped
  until `unsentFragments` clears), all in that call. VERIFIED (code).
- It builds into a 16384-byte buffer (`MSG_Init(..., 0x4000)` at `0x8085fa7`);
  `MSG_WriteByte` (0x807f090) and `MSG_WriteData` (0x807eef0) set the overflow
  flag and drop writes when full. VERIFIED (code). Nothing reads the flag
  before sending. VERIFIED (asm, `0x8085eec..0x8086168`). So a gamestate past
  16384 plain bytes arrives cut, with no `svc_EOF`. INFERRED.
- It sends via `SV_SendMessageToClient` (0x808f680), so pacing applies; the
  client is `CS_PRIMED`, not downloading, so off the LAN the second fragment
  waits at least 1 s. VERIFIED (code).
- `Netchan_Transmit` (0x8080320) errors past 16384 bytes (`Netchan_Transmit:
  length = %i`, `0x80d3000`); under 1300 (`cmp 0x514`) it sends one packet,
  else keeps the message as fragments and sends the first. VERIFIED (code).
- Off the LAN, the 2026-10-11 capture (six fragments, 5 x 1308 + 328 on the
  wire). VERIFIED (capture):

| `rate` | second fragment | the rest |
|---|---|---|
| 25000 | 975 ms | 98-104 ms apart (62 ms rounded to a frame) |
| 5000 | 958 ms | 346-353 ms apart (310 ms rounded up) |
| 1000 | 1507 ms | 1548-1552 ms apart, the 328-byte last included |

- So a fragment's pace comes off the bytes left before it (`min(left, 1500) +
  48`, as `0x80904d9`..`0x8090546` reads), and the first's off the whole
  message (1548 ms, past the 1 s floor). The first snapshot came 52 ms after
  the last fragment at every rate (the entering cmd makes the client due).
- vcod-server, same rates: the same gaps within 3 ms. With baselines
  (2026-10-12) its gamestate is 5 x 1308 + 250 against retail's 5 x 1308 +
  652: the retail homepath's extra pak lists in configstring 1 (326 bytes) and
  seven baselines vcod then lacked (77). VERIFIED (capture). vcod writes those
  baselines (map-cycle doc 3.3).
- 2026-10-09, a loopback probe on 1.1d `mp_carentan`: six fragments under one
  sequence (5 x 1300 + 645), the first right after the `connect` reply, the
  rest 41, 56, 47, 48, 51 ms apart, one a frame; before them one 9-byte
  `CS_CONNECTED` message. VERIFIED (capture, logging relay).
- vcod: `Server::send_gamestate` flushes old fragments, sends the first and
  paces the client; the frame loop sends the rest one a turn. The same probe
  got the 6718-byte gamestate in five fragments, one a frame. VERIFIED
  (capture). Past 16384 plain bytes (32768 on UO, whose buffer is double,
  `coduo-diff.md` 7) vcod warns and sends it whole.

### `snaps` (`SV_UserinfoChanged` 0x8086ab4)

- `snapshotMsec` (client field `0x528a8`) is 50 for an empty or missing
  `snaps`, else `1000 / snaps` with `snaps` `strtol`ed and clamped to 1..30:
  `30` gives 33, `0` or junk 1000, past 30 33. VERIFIED (asm
  `0x8086cad..0x8086d44`: `cmp $0x1e` at `0x8086d20`, 50 at `0x8086d44`).
- Readers: `SV_SendMessageToClient` floors an off-LAN interval with it
  (`0x808f7d8`); `SV_WriteDownloadToClient` sends `rate * snapshotMsec / 1000
  / 2048 + 1` blocks a message (`imul` at `0x80865d9`). LAN intervals ignore
  it. VERIFIED (code).
- 2026-10-09, a loopback probe downloading `bonneville.pk3` from 1.1d (rate
  99999): 11394-11496-byte messages at `snaps 10` (five blocks), 4541-4572 at
  `snaps 30` (two). vcod-server sent the same sizes byte for byte. VERIFIED
  (capture, relay, a scratch probe build taking `snaps` from the environment).
- vcod: `Client::snapshot_msec`, used by the pace and the block count.

### `g_password` (`ClientConnect`, game.mp.i386.so 0x4246c)

2026-10-09, `cod_lnxded` (1.1d), `+set g_password secret`, a throwaway
connectionless client (getinfo, getstatus, getchallenge, a Huffman `connect`
with chosen userinfo) and `vcod --net-probe` for the level-boundary drops.

- The check is the game module's. `ClientConnect` skips it when the userinfo
  `ip` (key 0x730a4) is `localhost` (0x730a7); else it reads `password`
  (0x730cf) and denies when `g_password` is non-empty, not `none` (0x730d8,
  `Q_stricmp`), and `strcmp`-different. The denial frees the entity and
  returns `GAME_INVALIDPASSWORD` (0x730dd). No bot or first-time parameter (Q3
  has both). INFERRED (0x425da..0x42666, calls resolved with
  `tools/re/annotate_func.py`).
- `SV_DirectConnect` writes `ip localhost` only when `NET_IsLocalAddress`
  (0x8080ed8: type 0 or 2) holds, else the address (0x80858d3, 0x808576c). So
  test clients (`SV_AddTestClient`, zeroed address at 0x80875cf) are exempt
  and UDP from 127.0.0.1 is not. INFERRED.
- A passwordless connect from 127.0.0.1, and one sending its own
  `\ip\localhost`, were both refused. VERIFIED (capture).
- A refusal sends OOB `error\nGAME_INVALIDPASSWORD` (`error\n%s` at 0x80d42c5,
  sent at 0x8085bd2), frees the slot, prints `Game rejected a connection: %s.`
  at developer level; the game's verdict follows the engine's checks
  (challenge, ping, `sv_privatePassword`, `EXE_SERVERISFULL`). INFERRED
  (0x8085baa..0x8085bf2).
- No `password`, `wrong` and `Secret` each got exactly
  `error\nGAME_INVALIDPASSWORD`; `secret` got `connectResponse`. VERIFIED
  (capture).
- Registered with flags 0 (`dump_cvars.py`), so live: an rcon change showed in
  the next `getinfo` and governed the next connect, no map load. VERIFIED
  (capture).
- `getinfo`/`getstatus` send `pswrd 1` when
  `Cvar_VariableString("g_password")` is non-empty (0x808c3ae, 0x808bf23):
  `secret` and `none` read 1, `""` 0. So `none` shows as passworded in the
  browser yet admits passwordless connects. VERIFIED (capture).
- `SV_MapRestart_f` (0x8083f8f) and `SV_SpawnServer` (0x808a765) rerun
  `ClientConnect` for every client; a denial goes through `SV_DropClient` with
  the same string. VERIFIED (capture: a probe joined under `g_password ""`,
  rcon set `secret`, and both `map_restart` and `map mp_harbor` dropped it
  with `w "GAME_INVALIDPASSWORD"`; the restart also printed `SV_MapRestart_f:
  dropped client 3 - denied!` per client).
- vcod (`Server::password_denied`, `crates/server/src/server.rs`) reads
  `g_password` from the level's cvar table and tests in `svc_direct_connect`
  after the full-server check and at both level boundaries before the script's
  reconnect; its bots are exempt like retail's test clients.

## Pak checksums and pure servers

2026-10-09, `cod_lnxded` (1.1d) on a spare port, `fs_basepath` a stock 1.1
install (`pak0`-`pak6`, `bonneville`, two localized paks), a scratch homepath,
`vcod --net-probe` as client. Code facts are from the decompiles; each claim
has its own label. vcod's port: `crates/common/src/pak_checksum.rs`,
`Pk3Fs::paks` and `search_paks` (`crates/common/src/pk3.rs`), `PakLists`
(`crates/server/src/configstrings.rs`), `PureCheck`
(`crates/server/src/server.rs`).

### The checksum

- `FS_LoadZipFile` (CoDMP.exe `0x42ac20`, from `FS_AddGameDirectory`
  `0x42bd30`) keeps, in central-directory order, the CRC-32 of every entry
  with non-zero uncompressed size (`unz_file_info+0x1c`); no name filter.
  VERIFIED.
- `pack+0x304` = `Com_BlockChecksum(crcs)` (`0x4445b0`), `pack+0x308` =
  `Com_BlockChecksumKey(crcs, fs_checksumFeed)` (`0x444640`, the 4 key bytes
  ahead of the CRCs). Both MD4 (`67452301`.. init, `0x5a827999`, `0x6ed9eba1`
  round constants) folded `d0 ^ d1 ^ d2 ^ d3`, printed `%i`; lnxded has the
  same MD4. VERIFIED.
- It reproduces retail: 1.1's `pak1`-`pak5` give `1265884747 616334813
  918160098 -1825805837 77111478`, `pak6` `1252304247`, as both captures.
  VERIFIED.
- 1.1's `pak0` reads `-363972028`, 1.5's `1048127331`: the files differ (md5,
  size). VERIFIED.
- `pak6.pk3` holds only `ui_mp_x86.dll`, `cgame_mp_x86.dll`,
  `game_mp_x86.dll`. VERIFIED.

### The four cvars (`SV_SpawnServer`, map-cycle doc section 3 step 23)

| cvar | built by | entries | with `sv_pure 0` |
|---|---|---|---|
| `sv_paks` | `FS_LoadedPakChecksums` `0x8071600` | non-localized paks, `%i ` each | `""` |
| `sv_pakNames` | `FS_LoadedPakNames` `0x8071580` | same paks, bare name, space-joined | `""` |
| `sv_referencedPaks` | `FS_ReferencedPakChecksums` `0x80717a4` | every pak, `%i ` each | same |
| `sv_referencedPakNames` | `FS_ReferencedPakNames` `0x80716cc` | every pak, `<game>/<name>` | same |

- The table. VERIFIED (code, the capture below).
- An empty value drops its key from the systeminfo. VERIFIED (the `sv_pure 0`
  capture has no `sv_paks`).
- On 1.1 "referenced" is every pak: Q3's `pack->referenced ||
  Q_stricmpn(gamename, BASEGAME)` reads the address of CoD's four-byte
  `referenced[]` (`cmp $0xfffffcf0` at `0x80717c7`), never null. VERIFIED
  (code).
- That it is a bug. INFERRED. Effect: an unused map download still lands in
  every client's download list.

1.5b and UO fixed it; their referenced pair is a real subset:

- 1.5b (`FS_ReferencedPakChecksums` 0x8075cf2, names 0x8075d9e): `referenced`
  (`pack+0x310`) set, or a directory failing `Q_stricmpn(dir, "main", 4)`.
  VERIFIED (disassembly).
- UO (0x807647e, names 0x8076564): set, or failing both `Q_stricmpn(dir,
  "main", 4)` and `Q_stricmpn(dir, fs_basegame, strlen(fs_basegame))`,
  `fs_basegame` default `uo` (0x80dd585). So stock `uo/`/`main/` paks list
  only when read from; mod directories always. VERIFIED (decompile).
- `FS_FOpenFileRead` (1.1 0x805cc18, 1.5b 0x8061c40, UO 0x8061a74) sets the
  byte on a pak read unless the name ends `.shader`, `.txt`, `.cfg`,
  `.config`, `.bot`, `.arena`, `.menu` or contains `levelshots`; UO also
  exempts `soundaliases`, 1.5b not (its string 0x80d8c37 is read only at
  0x806ded5 and 0x806f697). An existence check sets nothing. VERIFIED
  (decompile, disassembly).
- `FS_ClearPakReferences(0)` (UO 0x8065a4d) clears them at the top of
  `SV_SpawnServer` and in `FS_Restart` (0x8065bbb); step 23 reads them after
  the game's init and three settle frames; `map_restart` never rebuilds the
  cvars. VERIFIED (decompile).
- At `fs_debug 1` (`FS_FOpenFileRead: <file> (found in '<pak>')`), UO on
  mp_cassino read from exactly `pakuo07 pakuo04 pakuo03 pakuo01 pakuo00 pak9
  pak8 pak5 pak4 pak0`, the capture's list; 1.5b on mp_carentan from `pak9
  pak8 pak5 pak4 pak1 pak0` (`pak1` only for `soundaliases/*.csv`, `pak9` for
  `iw_sound2.csv` and more). VERIFIED.

vcod:

- `Pk3Fs` keeps the byte per pak (`flags_reference`, 1.1's exempt list);
  vcod-server clears it before reading the BSP and builds the pair after the
  script load (`Server::level_paks`).
- Since vcod reads some files later or never, on 1.5 and UO it makes the
  flagging reads at that point: each indexed model's descriptor and LOD 0
  `xmodelparts` (retail's `G_ModelIndex` precache); 1.5's every
  `soundaliases/*.csv`; UO's map sound load spec; each `#using_animtree` tree
  on both.
- The UO gates (mp_cassino dm, ctf, dom, hq; mp_foy bas; mp_j5outpost) and the
  1.5 gate (mp_carentan) match retail's pair exactly;
  `a_pak_holding_the_load_spec_or_an_animtree_is_referenced` covers a pak
  holding either file. VERIFIED (gates).

Post-clear reads, retail UO's `fs_debug 1` against vcod's `read <file> from
<pak>` trace (2026-10-17, mp_cassino dm, mp_foy bas):

- vcod reads every weapon file, weapon world and pickup model and vehicle
  model retail reads. Retail's flagged reads vcod lacked:
  `soundloadspecs/mp/<map>.csv` and `animtrees/flak88.atr` (plus `xanim/void`
  from `pak0`, flagged anyway). `configure_mp.csv` precedes the clear and
  flags nothing. VERIFIED (live).
- UO's spawn calls `0x806e35a(va("maps/mp/%s.bsp"), 2)` after `sv_serverid`:
  with 1 or 2 it opens `soundloadspecs/mp/<map>.csv` (else
  `soundloadspecs/mp/default.csv`, `0x806e15e`) and reads the `soundaliases/`
  files listed; other values list `soundaliases/*.csv` as 1.5. VERIFIED
  (decompile, coduo_lnxded). No `soundloadspecs` string in 1.5b or 1.1d.
  VERIFIED (strings).
- The animtree reads follow the compile: `Can't find animtrees/turret88.atr`,
  `animtrees/flak88.atr`, `animtrees/multiplayer.atr`, after
  `mp/playeranim.script` on mp_foy. VERIFIED (log). Each is a loaded script's
  `#using_animtree` (`_flak_gmi.gsc`'s `flak88`). INFERRED.
- All three servers hold `animtrees/%s.atr` (file offsets 1.1d 0x8e904, 1.5b
  0x98e04, UO 0x9ed44). VERIFIED (strings). vcod reads the tree on 1.5 too;
  that 1.5b does at the same point. INFERRED.
- A scratch UO install with `uo/zzz_spec.pk3` (only
  `soundloadspecs/mp/mp_foy.csv`) and `uo/zzz_tree.pk3` (only
  `animtrees/flak88.atr`), mp_foy bas: retail lists `uo/zzz_tree uo/zzz_spec
  uo/pakuo07 uo/pakuo04 uo/pakuo03 uo/pakuo01 uo/pakuo00 main/pak9 main/pak8
  main/pak5 main/pak4 main/pak0`, vcod-server (via `--net-probe`) the same.
  VERIFIED (live).

Defaults and order:

- `sv_pure` defaults to `1` (`0x80d542c`); on with no pak loaded it prints
  `WARNING: sv_pure set but no PK3 files loaded` (`0x80d5540`). VERIFIED.
- Order is the search path's, highest first: later game directory first, each
  directory's paks in reverse sorted order, all `localized_*` paks after the
  rest. `FS_AddGameDirectory` sorts `mp_` as `zz` and `localized_` as a space.
  VERIFIED (both captures: the homepath's `pak5..pak0`, the basepath's `pak6
  pak5..pak0 bonneville`, then each one's localized paks).
- Capture (`sv_pure 1` before the map loads, stock 1.1 basepath), configstring
  1's pak part. VERIFIED:

```
\sv_pakNames\pak6 pak5 pak4 pak3 pak2 pak1 pak0 bonneville\sv_paks\1252304247 77111478 -1825805837 918160098 616334813 1265884747 -363972028 2000562497 \sv_pure\1\sv_referencedPakNames\main/pak6 main/pak5 main/pak4 main/pak3 main/pak2 main/pak1 main/pak0 main/bonneville main/localized_english_pak1 main/localized_english_pak0\sv_referencedPaks\1252304247 77111478 -1825805837 918160098 616334813 1265884747 -363972028 2000562497 -961133319 -1187400494 \sv_serverid\16\timescale\1
```

- Trap: `+set sv_pure 1` after `+map` (where `tools/run_server.sh`'s extra
  arguments go) shows `sv_pure 1` with no `sv_paks`, the lists built under the
  load's 0, yet still drops clients without `cp`, as the drop reads the live
  cvar. VERIFIED.

### The client side

- `CL_SystemInfoChanged` (`0x415eb0`) forces every systeminfo key into a
  client cvar, flags ignored, unless a demo plays or a local server runs.
  VERIFIED (cod11-front-end.md 16, "The systeminfo sets client cvars", with
  1.5 and UO addresses).
- It passes `sv_paks`/`sv_pakNames` to `FS_PureServerSetLoadedPaks`
  (`0x43c290`) and the referenced pair to `FS_ServerSetReferencedPaks`
  (`0x43c530`), each erroring `pak sum/name mismatch` on unequal counts; a
  non-empty list prints `Connected to a pure server.` VERIFIED.
- `FS_PakIsPure` (`0x428ca0`) passes a pak if the list is empty or holds its
  checksum (names ignored); it gates `FS_FOpenFileRead`, so other paks are
  invisible on a pure server. VERIFIED. Localized paks are never in `sv_paks`;
  that they stay readable is INFERRED (the client would lose its text).
- `CL_SendPureChecksums` (`0x40fac0`) sends a reliable `cp` (built as `Va `
  plus 13 and 15 on its first two bytes) after `CL_InitCGame` in
  `CL_DownloadsComplete` (`0x40ffb0`) and after `vid_restart`, pure or not; no
  serverId, unlike Q3 1.32. VERIFIED.
- Body (`FS_ReferencedPakPureChecksums` `0x43c050`, `"%i "` each): the keyed
  checksum of the first pak flagged for `cgame_mp_x86.dll` (`pack+0x312`),
  then for `ui_mp_x86.dll` (`+0x311`), `@`, each non-localized pak with the
  general flag (`+0x310`), then `checksumFeed ^ (each general pak) ^ count`.
  VERIFIED.
- `FS_FOpenFileRead` sets the general flag on any read except `.shader`,
  `.txt`, `.cfg`, `.config`, `.bot`, `.arena`, `.menu` and `levelshots` paths
  (`0x429e3e`..); the three DLL flags match obfuscated names. VERIFIED
  (strings). They decode to `qagame_mp_x86.dll`, `cgame_mp_x86.dll`,
  `ui_mp_x86.dll`. INFERRED.
- The flags clear at `FS_Restart` and `vid_restart` (`0x42d170`). VERIFIED.
- `FS_ComparePaks` (`0x43b830`) skips stock paks (`FS_iwPak` `0x43b6b0`:
  `pak0`-`8`, `mp_pak`, `sp_pak`, `mp_bin`, `localized_*_pak`) and calls a
  referenced pak missing if no loaded pak has its checksum. A same-named file
  on disk makes the download land as `<name>.%08x.pk3` (lowercase hex) and
  adds ` (local file exists with wrong checksum)` to `Need paks:`. VERIFIED.

### The server's check

- `cp` is `SV_VerifyPaks_f` (`0x808674c`) in the ucmd table at `0x80e2f5c`
  with `userinfo`, `disconnect`, `vdr`, `download`, `nextdl`, `stopdl`,
  `donedl`, `retransdl`. VERIFIED.
- `vdr` (`0x8087b14`) resets `pureAuthentic` (`cl+0x528ac`) to 0; nothing else
  does after connect, so a map change keeps the verdict until the next `cp`.
  VERIFIED.
- In order: the server's paks hold both client DLLs (`FS_FileIsInPAK`); over
  five tokens; tokens 1 and 2 equal the cgame and ui pure checksums and do not
  start `@`; token 3 is `@`; no duplicate general token; each in
  `FS_LoadedPakPureChecksums` (`0x8071664`, non-localized); `count ^
  checksumFeed ^ (each)` equals the last. Pass sets `pureAuthentic` 1, else 2.
  VERIFIED.
- `SV_ExecuteClientMessage` (`0x80872ec`) drops `pureAuthentic` 2 under
  `sv_pure` with `EXE_UNPURECLIENTDETECTED` after the message's commands;
  `SV_UserMove` (`0x8086fa4`) drops one still 0 with
  `EXE_CANNOTVALIDATEPURECLIENT` after the entering cmd. VERIFIED (code).
- Live: no `cp` was dropped with
  `EXE_SERVERDISCONNECTREASON\x14EXE_CANNOTVALIDATEPURECLIENT` right after the
  gamestate; `cp <pak6> <pak6> @ <three paks> <key>` stayed Active. VERIFIED.
- So a pure server whose paks lack the DLLs (1.5's server directory has no
  `pak6`) drops everyone. INFERRED.

### Serving a download

- ucmds: `download` (`0x8087a64`: closes any transfer, keeps up to 64 bytes of
  name), `nextdl` (`0x8086168`), `stopdl` (`0x8087960`), `donedl`
  (`0x80879fc`: `SV_SendClientGameState`), `retransdl` (`0x8087a2c`).
  VERIFIED.
- `SV_WriteDownloadToClient` (`0x8086290`) opens the file on the next message.
  A refusal is `svc_download`, block 0, size -1, the string with the name
  after `\x15`. VERIFIED (strings `0x80d4838`, `0x80d48c2`, `0x80d48a0`,
  `0x80d4914`):

| case | string |
|---|---|
| stock pak (`FS_idPak(name, "main")`) | `EXE_CANTAUTODLGAMEPAK` |
| `sv_allowDownload 0` | `EXE_AUTODL_SERVERDISABLED` (`_PURE` with `sv_pure` on) |
| unopenable or empty | `EXE_AUTODL_FILENOTONSERVER` |

- The stock test never fires: `FS_idPak` (`0x80709c0`, at `0x80862e8`) gets
  the name with `.pk3` and compares with `%s/pak%d`. INFERRED (code). 1.5b
  streamed a requested `main/pakb.pk3` (cod15-protocol-diff.md 10). VERIFIED
  (live). vcod serves listed stock paks too.
- 0x800 bytes a block into an 8-slot window, an empty block after EOF; each
  block writes `block`, the size (block 0), length and bytes. Per message: the
  rate over one snapshot interval in 2048-byte units plus one (`sv_maxRate`
  caps the rate). A fully sent window waits; 1000 ms without an ack restarts
  from the first unacked block. VERIFIED (code).
- So 1.1d's blocks are 2048 bytes; docs/protocol-1.1.md's 8192- and 11800-byte
  blocks came off public servers. INFERRED.
- `nextdl <n>` with the due block advances, or completes on the empty block
  (`clientDownload: %d : file "%s" completed`); another `n` drops with `broken
  download`. `retransdl <n>` with the due block rewinds the cursor. VERIFIED
  (code).
- `FS_SV_FOpenFileRead` (`0x806ffb8`) opens the name as sent under
  `fs_homepath`, `fs_basepath`, `fs_cdpath` (`FS_BuildOSPath` `0x8062d70`)
  with `fopen`; no extension, pak list or `..` check. VERIFIED (code).
- A loopback client on 1.1d got all 581180 bytes of `main/game.mp.i386.so`,
  and again as `main/../main/game.mp.i386.so` (285 blocks each, `begining`
  logged both times). Any client can read any file the process can,
  `server.cfg` and its `rconPassword` included. VERIFIED (capture,
  2026-10-09).
- Blocks ride the per-client message ("Message pacing"); an inactive client's
  carry only the download, at its `nextSnapshotTime`.
- `CL_ReadyToSendPacket` (CoDMP.exe `0x40b940`) paces the acks: none within 50
  ms of the last while `downloadTempName` is set, else one a second until
  primed. VERIFIED (code).

### vcod

- The client sends `cp` after each map load (the probe after each gamestate,
  touching the map's pak first), reopens its search path under a pure server's
  `sv_paks`, and downloads by checksum with retail's `name.%08x.pk3` rule
  (`net::download::candidates_for_map`).
- The server publishes all four cvars from its mounted paks and verifies `cp`
  alike. Its `sv_pure 1` systeminfo on the 1.1 install matched the capture
  above byte for byte. VERIFIED.
- `sv_pure` defaults to 1, retail's default, and a set holds across levels
  (VERIFIED, `sv_pure_defaults_to_retails_and_a_set_outlives_the_level`). Until
  2026-11 every level load forced 0, which also undid a `--set sv_pure=1`
  at the first map change. No commit or doc gave a reason, and the client
  already sent `cp` on every load. `Server::new`, the tests' constructor,
  runs 0: `tools/run_server.sh` runs retail at 0 for the captures the
  gates replay, and the tests' clients send no `cp`.
- A 1.1 systeminfo past `MAX_INFO_STRING` loses paks off the list ends, the
  referenced pair first, never `sv_serverid`; 1.5 and UO keep
  `BIG_INFO_STRING` ("Configstring 1, systeminfo").
- Downloads (`crates/server/src/download.rs`) are limited on purpose to
  mounted paks listed in `sv_referencedPakNames`: those are all a retail
  client asks for (`FS_ComparePaks` lists from them), and retail's open path
  serves any file. Blocks go in the per-client message at `nextSnapshotTime`,
  alone before the world and after the snapshot once in it, with no server
  commands until active; a fragmented message goes a fragment a turn.
- The client sends a packet every 50 ms during a download (`NetClient`); the
  probe sends no usercmd until downloads end and the next gamestate is in, so
  it never enters mid-download.
- `--net-probe --probe-download <dir>` runs it headless. Against this
  section's retail 1.1d with `zzz_zfunmod` and `n_degaulle` in its homepath
  and a `zzz_zfunmod.pk3` copy of `pak6` in the scratch dir, it fetched
  `main/zzz_zfunmod.pk3` as `zzz_zfunmod.1b7cedba.pk3` and
  `main/n_degaulle.pk3` under its name; checksums (`461172154`, `-479819976`)
  matched `sv_referencedPaks`, and `donedl` brought the gamestate back.
  VERIFIED.
- Against vcod-server it fetched the same two with matching checksums,
  `n_degaulle` as `n_degaulle.e3668738.pk3` past a same-named file. VERIFIED.

## CD key authorization (the clients)

Outside the document-level default: every line here is from the Windows
clients (1.1 and 1.5 `CoDMP.exe`, `CoDUOMP.exe` 1.51, `CoD2MP_s.exe` 1.3,
`iw3mp.exe` 1.7) and carries its own label. The server side (`getIpAuthorize`,
verdicts) is in "Bans" and cod2-protocol.md 1.3.

| | 1.1 | 1.5 | UO 1.51 | CoD2 1.3 | CoD4 1.7 |
|---|---|---|---|---|---|
| `CL_RequestAuthorization` | 0x40f260 | 0x40fe60 | 0x410bb0 | 0x40d130 | 0x469b20 |
| authorize host | `codauthorize.activision.com` | same | `coduoauthorize.activision.com` | `cod2master.activision.com` | `cod4master.activision.com` |
| port (`BigShort` imm.) | 20500 (0x40f2fb) | 20500 (0x40fefb) | 20600 (0x410c41) | 20700 (0x40d1b0) | 20800 (0x469baa) |
| packet | `getKeyAuthorize %i %s` | same | same | `getKeyAuthorize %i %s PB %s` | same as CoD2 |
| key read from | file `main/codkey` | registry | registry | registry | registry |
| `getchallenge` | bare | bare | bare | `getchallenge 0 "%s"` | same as CoD2 |

VERIFIED (the format, host and port immediates at those addresses;
`getchallenge` strings at 1.1 0x567094, 1.5 0x5a66f8, UO 0x594300, CoD2
0x5a38a4, CoD4 0x6d09c4).

The packet:

- `%i` is `cl_anonymous` (registered at 1.1 0x56753c, CoD2 0x5a3c20, CoD4
  0x6d05d0). VERIFIED.
- `%s` is the key: `cl_cdkey`'s first 32 characters minus all but `0-9a-zA-Z`
  (compare chain at 1.1 0x40f3c0, CoD2 0x40d260). VERIFIED. `cl_cdkey` holds
  16 characters and a NUL, so 16 go. INFERRED (1.5 stores the NUL at 0x5bb8c4,
  UO at 0x5c51dc).
- Under `fs_restrict` the word `demo` goes instead (1.1 0x40f379). VERIFIED.
- CoD2's and CoD4's `PB %s` is `CL_BuildMd5StrFromCDKey` (CoD2 0x40cfc0, CoD4
  0x4699a0), also carried by `getchallenge 0 "%s"` (calls at CoD2 0x40e3d6,
  CoD4 0x46b29c). VERIFIED.
- Its steps: the same filter, a white-space trim (CoD2 0x40cf40), a case fold,
  MD5 from state words `0x6f1cd602 0x226c74be 0xb31c088d 0x555a9639` (CoD2
  0x40d053, CoD4 0x469a37) instead of RFC 1321's, bit count 0, over `strlen`,
  the 16 bytes printed `%02x` (0x5a3cc8). VERIFIED. Their order. INFERRED
  (call order).
- CoD2 folds to upper (`_strupr` 0x58ce80: `a`..`z` less 0x20), CoD4 to lower
  (0x688e3b: `A`..`Z` plus 0x20). VERIFIED.
- Update and final are plain MD5. INFERRED (the transform at CoD2 0x4bc3b0
  holds 0xd76aa478; the final's 0x38/0x3f padding test at 0x4bce22).

The check and the send:

- `CL_CDKeyValidate` (1.1 0x414260, 1.5 0x415ab0, UO 0x416f40, CoD2 0x4b3b20,
  CoD4 0x4712e0) runs first: a CRC-16 over the 16 characters, each XORed in
  and shifted right 8 times, `0x14002` XORed before the shift on an odd value
  (reflected 0xa001, initial 0), printed `%04x`, compared caselessly with the
  4-character checksum. VERIFIED (`xor eax,0x14002` and the `cmp ecx,0x10`
  loop in all five).
- A mismatch ends with `Com_Error(1, "EXE_ERR_INVALID_CD_KEY")` (1.1
  0x40f286). INFERRED (branch).
- A valid `main/codkey` passes. VERIFIED (checked without printing the key).
- `CL_CheckForResend`'s challenging arm calls it before every `getchallenge`,
  unless `net_lanauthorize` is 0 and `Sys_IsLANAddress` calls the server local
  (1.1 0x4105b5..0x4105df, 1.5 0x4112b3, UO 0x4120da, CoD2 0x40e316, CoD4
  0x46b1d7 off the dvar at 0x14e88b0). INFERRED (branch order).
- One datagram, no wait: `getchallenge` follows in the same call (1.1
  0x4105e4). INFERRED.
- The name resolves once and is cached (1.1 0x40f2a8 tests the cached port).
  INFERRED.
- CoD2 re-sends every 300000 ms while connected (0x40d310: state 8, `realtime -
  authRequestTime > 0x493e0`). INFERRED.
- A server's `needcdkey` (`CL_ConnectionlessPacket`, 1.1 0x410f00, CoD2
  0x40f047) sets the connect line to `EXE_AWAITINGCDKEYAUTH` and resends the
  key; challenge resends continue. INFERRED.
- `keyAuthorize` from the authorize server is matched and dropped (CoD2 Mac
  `CL_ConnectionlessPacket`). INFERRED.
- Relayed refusals (`error\nEXE_ERR_CDKEY_IN_USE`, `error\nEXE_ERR_BAD_CDKEY`,
  "Bans") arrive as any `error`.

Where the key lives:

- 1.1: `codkey` in `fs_basepath`'s `main/` (`Com_ReadCDKey` 0x436930, called
  with `main` at 0x42d0f3): 16 key bytes, then 4 checksum. VERIFIED (the two
  `FS_Read` lengths, a real key file). The tail is `Com_WriteCDKey`'s
  (0x436bc0) comment lines, written after upper-casing both parts (`toupper`
  via the ctype `_LOWER` mask, 0x525f07). VERIFIED.
- `Com_AppendCDKey` (0x436a40, called with `fs_game` at 0x42d122) appends a
  mod directory's `codkey` alike. VERIFIED.
- The rest read a 21-byte registry string, key then checksum, and no file.
  VERIFIED:

| client | key | value | addresses |
|---|---|---|---|
| 1.5 | `HKLM\SOFTWARE\Activision\Call of Duty` | `codkey` | 0x438ee0, 0x5a1f58 |
| UO | `...\Call of Duty United Offensive` | `key` | 0x43b070, 0x598704, 0x598700 |
| CoD2 | `...\Call of Duty 2` | `codkey` | 0x433237, 0x5a7f60 |
| CoD4 | `...\Call of Duty 4` | `codkey` | 0x4fde77, 0x6e13ac |

- `exe.str` lines: 1.1 `AWAITINGCDKEYAUTH` "Awaiting CD key authorization",
  `ERR_INVALID_CD_KEY` "CD Key not valid. Please enter CD Key in Multiplayer
  Options/Enter CD-Key menu ...", `ERR_CDKEY_IN_USE` "CD Key in use. Please
  try reconnecting later.", `ERR_BAD_CDKEY` "Bad CD Key."; CoD2 says "Key
  code" for "CD key". VERIFIED (`localized_english_pak1.pk3`,
  `localized_english_iw09.iwd`). CoD4's `localized_code_post_gfx_mp.ff` has
  the same four names. VERIFIED.

vcod (`vcod_common::net::cdkey`) sends the same packet to the same host on its
own socket and thread, for an off-LAN server, before each `getchallenge`.
`needcdkey` without a key ends the connect with `EXE_ERR_INVALID_CD_KEY`
(retail always has a key); with one it resends, and a timeout reads
`EXE_AWAITINGCDKEYAUTH`. The key is `VCOD_CDKEY`, else (1.1 and 1.5) the
install's `main/codkey`; no registry, no `Com_AppendCDKey` mod key, no CoD2
5-minute re-send.

Live, 2026-10-24, CoD2 1.3. VERIFIED:

- Two public off-LAN servers answered vcod's keyless bare `getchallenge` with
  `\xff\xff\xff\xffneedcdkey` about 200 ms later; vcod ended the first connect
  with `EXE_ERR_INVALID_CD_KEY`.
- Retail `cod2_lnxded` on mp_toujane in `unshare -rnm` (server 203.0.113.1,
  off-LAN client 198.51.100.2, `/etc/hosts` naming every authorize host
  198.51.100.9, where a stand-in logged each datagram's command and length and
  answered `getIpAuthorize` with `accept`). With the dummy key
  `AAAA-AAAA-AAAA-AAAA` the off-LAN join sent one 74-byte `getKeyAuthorize 0
  <16> PB <32 hex>` to port 20700, and the server's `getIpAuthorize` grew from
  48 to 84 bytes with the `PB` string `getchallenge` carries. Keyless, no
  `getKeyAuthorize` went. From loopback, key or not, nothing reached the
  stand-in or the server's authorize. All four joins reached snapshots and
  were logged.

## Raw captures

In the repo:
`crates/server/tests/fixtures/configstrings/mp_{pavlov,carentan}-dm.txt`, the
whole non-empty table off a retail `dm` server, one `<index> <value>` a line;
`crates/server/tests/configstrings_ab.rs` diffs ours against them slot by slot
on both maps. Retake with `tools/run_server.sh <map>` and `cargo run -p vcod
-- --net-probe 127.0.0.1:28960 --save-configstrings`. The fixture header
records the settings slots depend on (`g_gametype dm`, `sv_maxclients 8`,
`sv_pure 0`, stock `scr_*` defaults).

**Configstrings 0 and 1 are provenance, not measurement.** 0 carries the
capture's hostname and `sv_maxclients`, 1 its pak lists, which follow
`sv_pure` and the homepath's pk3s. Both are in `configstrings_ab.rs`'s
`STRUCTURAL_SKIP`; `crates/server/tests/connect.rs` covers what the server
writes there. A re-capture changing them is no regression.

Not in the repo (regenerate as above), per map: the three connectionless
replies, the full `RUST_LOG=debug` probe log, the `cs[i] = ...` lines, and the
raw gamestate from `--save-snapshots`, which leaves the committed fixture
alone.
