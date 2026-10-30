# Retail server handshake and empty-map configstrings (CoD 1.1)

What the retail 1.1d Linux dedicated server puts on the wire before any player
exists: the three connectionless replies, the gamestate framing, and the whole
configstring table for two stock maps. This is the reference the vcod server
crate copies its constants from, so the values below are verbatim captures, not
transcriptions.

Everything here is **VERIFIED live 2026-08-25** against `cod_lnxded` (the
1.1d Linux dedicated server) with the MP game module `game.mp.i386.so`
(`gamedate: Nov 13 2003`) and the 1.5 install's assets, unless a line says
otherwise.

## Setup

```
tools/run_server.sh mp_pavlov      # and again with mp_carentan
```

which runs

```
cod_lnxded +set dedicated 1 +set fs_basepath <game install> \
    +set fs_homepath <homepath holding main/game.mp.i386.so> \
    +set net_port 28960 +set sv_maxclients 8 +map <map>
```

Defaults left alone: `sv_hostname` `CoDHost`, `g_gametype` `dm`.

These captures ran with `sv_pure 1`, the retail default; at capture time the
script did not set it. That is what overflowed configstring 1 past
`MAX_INFO_STRING` (the trap below). The script now sets `+set sv_pure 0`, so a
rerun will not reproduce the truncation. Nothing else in this capture depends
on `sv_pure`, so I did not re-run it.

The connectionless probe was

```python
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(2)
for q in [b"getinfo xyz", b"getstatus xyz", b"getchallenge"]:
    s.sendto(b"\xff\xff\xff\xff" + q, ("127.0.0.1", 28960))
    print(q, "->", s.recv(8192))
```

and the gamestate came from

```
RUST_LOG=debug cargo run -p vcod -- --net-probe 127.0.0.1:28960 --probe-secs 40
```

A lone spectator gets no snapshots, so the probe reaches `Active` and then dies
on its 30 s snapshot-silence timeout. That is the expected end of the run; the
gamestate has already arrived by then.

## Connectionless replies

All three are `FF FF FF FF` followed by the command word, a newline, then the
payload. Byte-for-byte, `mp_pavlov`:

### `getinfo xyz` -> `infoResponse`

```
infoResponse\n\challenge\xyz\protocol\1\hostname\CoDHost\mapname\mp_pavlov\clients\0\sv_maxclients\8\gametype\dm\pure\1\sv_allowAnonymous\0\pswrd\0
```

Key order is fixed and is *not* the serverinfo cvar order: the server builds
this string itself (`SV_Info`, `cod_lnxded` `0x808c1ac`). `challenge` echoes my
argument ("The challenge key" below has what the line read and the info
string do to it, and its absence without one). `protocol` is **1**. `gametype` is the short name, not
`g_gametype`'s key. `pure` and `pswrd` are `0`/`1` flags, not cvar names.

`minPing`, `maxPing` and `game` did not appear. Inferred from a read of
`SV_Info` (`0x808c1ac`), not tested here: those three are conditional on
`sv_minPing`/`sv_maxPing` being non-zero and `fs_game` being set, all of which
are default in this run. `clients` counts connected clients and was `0` on an
idle server.

Only `mapname` differs on `mp_carentan`.

### `getstatus xyz` -> `statusResponse`

```
statusResponse\n\g_gametype\dm\gamename\main\mapname\mp_pavlov\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\1\challenge\xyz\pswrd\0\n
```

This is the serverinfo cvar string (configstring 0, below, identical byte for
byte) with `\challenge\<arg>\pswrd\0` appended, then a trailing newline. Player
lines would follow that newline, one per connected client; with nobody on the
server there are none. Only `mapname` differs on `mp_carentan`.

The serverinfo keys are in ascending case-insensitive name order, which is how
`Cvar_InfoString` walks the cvar list: `sv_maxclients` before `sv_maxPing` and
`sv_maxRate`, which case-sensitive ASCII order would reverse.

### `getchallenge` -> `challengeResponse`

```
challengeResponse -1111288375
```

**One** integer, space-separated, printed signed and freely negative. Q3's
second field (the authorize-server flag) is absent. Four samples across the two
map runs: `-1111288375`, `-3172718`, `219667770`, `-1225130010`.

### `connect` -> `connectResponse`

`connectResponse` with an empty payload, from both map runs
(`oob [Connecting] connectResponse ""` in the probe log).

## The gamestate message

`reliableAcknowledge` is `0` (plain LE long at bytes 0..4, as documented in
`docs/protocol-1.1.md`), and the Huffman stream then starts **directly with
`svc_gamestate` (2)** on both maps. No `svc_serverCommand` precedes it, and the
probe logged no server command at all during the run.

That is a divergence from the committed capture in
`crates/common/tests/fixtures/net/gamestate.bin`, which was taken from a
populated public server and does begin with `svc_serverCommand 0 ""` before
`svc_gamestate`. So the leading empty command is not part of the handshake the
stock server emits; the parser has to accept both, which
`gamestate::parse` already does. Whether the public server's leading command
comes from its mod (CoDaM) or from a reliable command queued before the
gamestate is not established here.

`serverCommandSequence` is 0. `clientNum` is 0 (first free slot).

Baselines, empty map:

| map | non-empty configstrings | baselines |
|---|---|---|
| mp_pavlov | 207 | 43 |
| mp_carentan | 246 | 19 |

The committed populated `mp_carentan` capture also has 19 baselines, so
baselines are map geometry (script entities, doors, MG42s), not players.

## Configstrings at gamestate

Indices set on each empty map:

```
mp_pavlov   0 1 2 3 7 8 11 12 13 20 21 22 29 140-180 204-243 269-361 525 781 782
            1180-1187 1212 1213 1245 1246 1501-1505
mp_carentan 0 1 2 3 7 8 11 12 13 20 21 22 29 140-180 204-243 269-399 525-527 781
            1180-1187 1212 1213 1245 1246 1501-1505
```

The empty `mp_carentan` index set is **identical** to the committed populated
`mp_carentan` capture's, down to the model and sound ranges. Nothing in the
gamestate table depends on players being present: 5 and 6 (team scores) and
15..18 (vote) are unset in both, and arrive later as `d <index> <text>`
configstring updates. 14 (MOTD, `g_motd` empty), 44+ (locations), 108..139
(attachment tag names) and 1100..1115 (shellshock) are unset in every capture I
have.

### Configstring 0, serverinfo

`mp_pavlov`, 217 bytes:

```
\g_gametype\dm\gamename\main\mapname\mp_pavlov\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\1
```

`mp_carentan` is the same with `mapname\mp_carentan` (219 bytes). Same key set
and order as the populated capture, which differs only in the values
(`sv_maxclients\20`, `sv_pure\0`).

### Configstring 1, systeminfo

Identical on both maps, 1023 bytes:

```
\bg_fallDamageMaxHeight\480\bg_fallDamageMinHeight\256\g_synchronousClients\0\pmove_fixed\0\pmove_msec\8\sv_cheats\0\sv_pakNames\pak5 pak4 pak3 pak2 pak1 pak0 [...]\sv_paks\77111478 -1825805837 918160098 616334813 1265884747 1048127331 [...] \sv_pure\1\sv_referencedPakNames\main/pak5 main/pak4 main/pak3 main/pak2 main/pak1 main/pak0 [...]
```

Elided at each `[...]`: the 28 non-stock pak names (map downloads) and their
checksums, in the same order in each list.

**This one is a trap, and the capture is why I know.** It stops mid-list
inside `sv_referencedPakNames` at exactly 1023 characters, and it carries **no
`sv_serverid`**, so the client reads a serverId of 0 for the whole session. The
install I captured against holds 34 pk3s in `main` (the stock paks plus map
downloads from public servers), and with `sv_pure 1` the server writes
`sv_pakNames`, `sv_paks` and `sv_referencedPakNames` for all of them into a
`MAX_INFO_STRING` (1024) buffer. Keys are written in name order, so
`sv_serverid` and `timescale` are the two that fall off the end. The populated capture's systeminfo is 511 bytes and does
carry `\sv_serverid\18\timescale\1` after `sv_referencedPaks`, which is the
cross-check.

The trigger is `sv_pure 1`. With `sv_pure 0` the server omits the three pak
lists entirely (inferred from the populated capture, not re-captured). That
capture's cs 1 is 511 bytes, carries `\sv_pure\0` and
`\sv_serverid\18\timescale\1`, and has no `sv_pakNames` or `sv_paks` at all,
so a `sv_pure 0` rerun fits inside 1024 and keeps `sv_serverid`. A clean stock
install would likely fit either way.

A server started with `+set fs_game <dir>` adds `fs_game\<dir>` between
`bg_fallDamageMinHeight` and `g_synchronousClients`, and its mod's paks
lead the lists (`<dir>/<pak>` in `sv_referencedPakNames`). VERIFIED (a live capture against retail
1.1d, 2026-10-11, `cod11-front-end.md` section 18).

Either way the vcod server must keep a 1.1 systeminfo under 1024 bytes and
`sv_serverid` has to survive the trim, because `SV_ExecuteClientMessage` gates
every client message on it.

Where the cut comes from, and why 1.5 and UO have none:

- 1.1's `SV_SpawnServer` builds the string with the big builder
  (`Cvar_InfoString_Big` 0x806fc84, an 8192-byte buffer at 0x812b620 and an
  `Info_SetValueForKey_Big` 0x80829e8 with a 0x401c-byte frame), then copies
  it with `Q_strncpyz(buf, info, 0x400)` (0x8083288, the `push 0x400` at
  0x808a897) before `SV_SetConfigstring(1, buf)` (0x808a8c9). The 1023 is that
  copy. VERIFIED (disassembly).
- 1.5b copies with `0x2000` (`mov [esp+8],0x2000` at 0x80912c0, the copy
  0x8085e32, then `SV_SetConfigstring(1, ...)` at 0x80912f2). VERIFIED
  (disassembly).
- UO's `SV_SpawnServer` (0x8091b72) does the same with `0x2000`, and
  `SV_Frame`'s refresh on a systeminfo cvar change sets configstring 1
  straight from `Cvar_InfoString_Big` (0x80748c0) with no copy at all.
  `Info_SetValueForKey_Big` (0x8087570) refuses a pair that would take the
  string past 8192. VERIFIED (decompile). Live: a `sv_pure 1` UO server sends
  a 1318-byte systeminfo on mp_cassino and a 1351-byte one on mp_j5outpost,
  every pak list whole and `sv_serverid` in place
  (`crates/server/tests/fixtures/configstrings/*-dm-uo-pure.txt`, VERIFIED).
- The gamestate carries a configstring as a big string, so the long one
  arrives whole. A later change goes out through `SV_SetConfigstring`'s
  per-client half (1.1 0x8089bf0, UO 0x8090da8): under 1000 bytes it is
  `d <index> <text>`; from 1000 bytes it is cut into 999-byte pieces sent as
  `x <index> <piece>`, then `y` for each middle piece and `z` for the last.
  VERIFIED (decompile; the letters are the strings at 1.1 0x80d5244/6/8 and
  UO 0x80e54e4/6/8). This applies to a 1.1 systeminfo between 1000 and 1023
  bytes too.
- The client puts the pieces back together in `CL_CGameServerCommand`
  (CoDMP.exe 0x4014e0, CoDUOMP.exe 0x401630): `x` starts a buffer as
  `d <index> "<piece>` (1.1) or `d <index> <piece>` (UO, after a
  three-token tokenize, so the piece is the rest of the line), `y` appends,
  `z` appends, closes the quote on 1.1, and runs the result as a `d`. The
  buffer is bounded at 8192 (`0x1fff` check). VERIFIED (decompile).

vcod's server bounds the systeminfo at 1023 on protocol 1 (trimming paks,
not `sv_serverid`) and at 8191 on 6 and 22, and sends every configstring of
1000 bytes or more as `x`/`y`/`z` (`net::configstring_commands`); the client
puts them back together. The UO gate `a_pure_server_s_systeminfo_matches_uo`
matches both pure captures byte for byte, `sv_serverid` included: it
loads the recipe's first map, then the captured one, as the captures were
a second map load (32).

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

Configstring 7 is byte-identical to the populated capture's, so the weapon list
is a property of the game module, not the map or the round. 13 is
`level.startTime` and reads `0` because the map had just loaded.

### Map-dependent, index < 140

| cs | mp_pavlov | mp_carentan |
|---|---|---|
| 3 | `n\ambient_mp_pavlov\t\0` | `n\ambient_mp_carentan\t\0` |
| 8 | `1ce0cfb40000000001` | `7df31f0d1000000001` |
| 11 | `90` | `0` |
| 12 | `0 6000 1 0.8 0.8 0.8 0` | `0 16500 1 0.7 0.85 1 0` |

3 is the ambient (worldspawn), 8 the registered-item bits, 11 `northyaw`, 12 the
fog params. All are worldspawn or script, so all map data.

### 140..203 cvar names, 204..267 values

The pairing (`Cvar_Set(cs[140+i], cs[204+i])`, offset 64) is already recorded in
`docs/research/cod11-hud-protocol.md`; this capture confirms it live
and pins the stock values. The server side of it is in the engine rather than
the game module, which is why neither document found it there:
`docs/research/cod11-map-cycle.md` 3.2 reads the writer at `0x808b148` in
`cod_lnxded`, walking the cvar list for the `0x800` flag. Only 179/243 is
map-dependent. `scr_motd` (180) has an empty value, which is also the loop's
stop condition on the client.

The table below is the seed for `ENGINE_MIRRORED` in
`crates/server/src/cvars.rs`, the 21 engine cvars the game module registers
with the mirror flag, not a description of a constant in `configstrings.rs`.
The mirror is one name-sorted list of every flagged cvar and nothing writes
an absolute slot; these 21 engine names all sort ahead of the 20 `scr_*` the
stock scripts register, which is the only reason the engine set fills
140..160 and the script set 161..180. The rest of the game module's cvar
table is not mirrored and cannot be recovered from a capture at all, only
from the module: section 18 of `docs/research/cod11-gsc-object-model.md`
has the dumper.

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

Map-dependent in both content and length; the index is the model index the
snapshot's `modelindex` refers to, `cs[268 + n]`.

| map | range | count | first two | last |
|---|---|---|---|---|
| mp_pavlov | 269..361 | 93 | `xmodel/crate_misc2a`, `xmodel/sandbags_curved_winter` | `xmodel/weapon_kar98scoped` |
| mp_carentan | 269..399 | 131 | `xmodel/static_vehicle_german_truck`, `xmodel/wehrmacht_soldier` | `xmodel/weapon_kar98scoped` |

Map props are registered first and the weapon world models last, so the same
model has a different index on each map. No player-body model is registered on
an empty server: `playerbody_*` entries arrive later, when clients spawn.

### 524.. sound aliases (`CS_SOUNDS` = 524)

| map | indices | aliases |
|---|---|---|
| mp_pavlov | 525 | `world_hurt_me` |
| mp_carentan | 525, 526, 527 | `world_hurt_me`, `weap_mg42_loop`, `weap_mg42_cooldown` |

524 itself is unset on both, so this block is 1-based like the models. The
populated capture has 525..527 as well. Almost every sound the client plays is
resolved from the alias csv on the client side; this block is only for the
aliases scripts index by number.

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

780 is unset, so effects are 1-based off 780 too, and the effect list is map
script data. 1180/1181 are the team-selection menus, the only place the map's
nationality shows up in the table; 1182..1187 are the rest of the precached menu
block (`1180..1211` per the hud-protocol doc). 1212/1213 sit just past it and
1245/1246 in the localized-string block (`1244 + n`); 1501.. is the material
block. All of those come from the game module, not the map, except 1501.

## What a minimal server has to set

Everything below index 140 that is not worldspawn data: 2 (`cod`), 7 (weapon
list), 13, 20, 21, 22, 29, plus the 140/204 cvar pairs, plus 1180..1187,
1212, 1213, 1245, 1246, 1501..1505. Those are byte-identical on both maps and
come from the game module. 0, 1, 3, 8, 11, 12, 243, 269.., 525.., 781.., 1501
are per-map.

## Retail client check

**Status: PASS, VERIFIED live 2026-08-25.** Against a release
`vcod-server mp_carentan` with the svc_EOF fix, the retail 1.1 client
(`CoDMP.exe`) got `connectResponse`; the server logged `connected`,
`gamestate, 2641 bytes`, the ignored `cp` (the client's pak checksums, which
the server ignores under `sv_pure 0`) and `sent its first move (serverTime 0)`;
the client's loading bar filled and it stayed on the map waiting for the first
snapshot, which is where a Q3-family client enters the world and which this
server does not send yet. No `usercmd not parsed`, no
`EXE_SERVER_IS_DIFFERENT_VER`. `serverTime 0` is expected, since the client
has no server clock before its first snapshot. This proves that the retail client's
moves, compact and full-field branches included, parsed for the whole session
without a truncation error, so the field *widths and order* transcribed from
`cod_lnxded 0x807b7f8` are consistent with what a 1.1 client sends. The field
*values* are still unchecked, because the server parses and discards them; a
layout that is wrong but the same length would only show once moves drive a
player. A first run had failed right after the map loaded with
`ERROR: CL_ParseServerMessage: Illegible server message 195` and a client
`disconnect` (server: `dropped: EXE_DISCONNECTED`), because
`ServerNetchan::transmit` did not append the `svc_EOF` that `SV_Netchan_Transmit` puts after the last op
of every message. The retail capture has it (byte 7205 of the decompressed
gamestate is `8`, then one pad symbol); `195` was the decompressor's pad symbol
read as an op. vcod's own client stops after the gamestate body without
looking, so only the retail client could catch it. `transmit` now appends the
terminator and the byte-exact test pins that byte. Separately, vcod's own
client (`vcod --net-probe 127.0.0.1:28960`) against `vcod-server mp_carentan`
on the same day reached `connected`, `gamestate, 2641 bytes` and
`sent its first move`, then sat at `Active, no snapshot yet` until its own 30 s
timeout, because snapshots are not implemented.

To run the retail check, two shells:

```
RUST_LOG=debug cargo run -p vcod-server -- mp_carentan
CoDMP.exe +set r_fullscreen 0 +connect 127.0.0.1:28960     # from the 1.1 install
```

Pass: the server log shows `connected`, `gamestate, N bytes` and
`sent its first move`, with no `usercmd not parsed` at debug level; the client
loads the map and then waits for snapshots until its own timeout; neither side
prints `EXE_SERVER_IS_DIFFERENT_VER`.

Where each failure points:

| Symptom | Suspect |
|---|---|
| `usercmd not parsed` at debug level | the full-field usercmd branch, inferred from disassembly only; table in `tools/re/net-notes.md` |
| `Illegible server message N` right after the map loads | the message terminator: every server message must end in `svc_EOF` (`ServerNetchan::transmit`); this is what the first run hit |
| the client disconnects right after the gamestate with another error | the configstring table in `crates/server/src/configstrings.rs` |
| no `connectResponse` | `parse_connect` (`crates/common/src/net/connectionless.rs`) |

## Housekeeping: master heartbeat, rcon, zombie slots, private slots

Measured 2026-10-07 against `cod_lnxded` (1.1d) with `tools/run_server.sh`
on a spare port, a UDP listener standing in for the master, `vcod
--net-probe` as the client, and `rcon status` polled 0.5-0.6 s apart for the
zombie timing. Each claim carries its own label; the scripts were throwaway.
vcod's port is `crates/server/src/master.rs`, `crates/server/src/rcon.rs`
and the zombie half of `Server::drop_client`. vcod's heartbeat goes to
`master.cod.pm` (`sv_master1`'s default; its operator allowed it) on the
protocol's port of it, 20510 or UO's 20610, in place of retail's fixed
20510, and drops a packet to a master host within a second of the last.

### Master heartbeat (`SV_MasterHeartbeat`, 0x808ba0c)

- Cvars, from `SV_Init`'s registrations: `sv_master1` defaults to
  `codmaster.activision.com`, `sv_master2..5` to empty. VERIFIED (strings at
  0x8d6f0..0x8d735, the `Cvar_Get` calls in the export).
- `dedicated` defaults to `"2"` (the `Cvar_Get` default at 0x80cf769).
  VERIFIED. The heartbeat runs only when `dedicated` is 2 (`cmp [eax+0x20],2`
  at 0x808ba22); `dedicated 1` never heartbeats. INFERRED from the branch.
- The packet is `\xff\xff\xff\xffheartbeat COD-1\n`. VERIFIED by capture;
  the format `heartbeat %s\n` is at 0x80d5978 and `COD-1` at 0x80d5e69.
- The port is always 20510. `sv_master1 127.0.0.1:29463` logged
  `127.0.0.1:29463 resolved to 127.0.0.1:20510` and the packet arrived on
  20510. VERIFIED by capture. The cause is the swapped `strstr(":", name)`
  at 0x808baf0 (the `push 0x501e` follows at 0x808baff), the same bug Q3's
  source has. INFERRED.
- A name is resolved again only after its cvar changes; one that does not
  resolve is cleared with `Cvar_Set(name, "")` and so never retried. INFERRED
  from 0x808ba6f..0x808bad3.
- Interval: 180 s (`add eax,0x2bf20` at 0x808ba3d). VERIFIED in the binary
  and by capture: two periodic heartbeats 179.998 s apart. Every send,
  forced or not, restarts the 180 s.
- Forced sends (`SV_Heartbeat_f` 0x8084bd0 sets the next time to
  -9999999): the `heartbeat` console command; `SV_SpawnServer` (call at
  0x808a915), so the first frame of every map load; `SV_DirectConnect` when
  the connecting client is the first or fills the last slot (0x8085cd3);
  `SV_DropClient` when no client is left at `CS_CONNECTED` or above
  (0x8085edc). INFERRED for the conditions. VERIFIED by capture: a heartbeat
  at server start, at a probe's connect, at its kick, at `map mp_harbor`, and
  none at `map_restart`.
- Shutdown: `SV_MasterShutdown` (0x808d268) sends one `heartbeat
  flatline\n`. VERIFIED by capture after `rcon quit`. It ignores the timer
  but is still gated on `dedicated 2`. INFERRED.
- The live master answers a heartbeat within 0.2 s with `getchallenge <n>`
  and `getstatus <n>` from the address it was sent to, which a home NAT lets
  through. VERIFIED 2026-10-07 with one heartbeat from a bare socket, and
  again on retail with `developer 1` (`SV packet <master>:20510 :
  getchallenge`, then `: getstatus`).
- Retail answers that `getchallenge` by resolving
  `codauthorize.activision.com` and logging `sending getIpAuthorize for
  <master>:20510` (0x8084d90), not with a `challengeResponse`.
  VERIFIED by log. Public DNS resolves `codauthorize.activision.com` to the
  master's own address. VERIFIED (DNS lookup).
- Neither server got listed. Retail (`dedicated 2`, one heartbeat, stopped
  with `rcon quit` after about 60 s) was missing from `getservers 1 full
  empty` at 15, 35 and 60 s, among 48 listed servers. vcod's server (one
  heartbeat and its flatline) was missing at 12 and 32 s. VERIFIED
  2026-10-07. With both unlisted there was no behaviour to copy. Whether the
  authorize round trip, the address or something else keeps a server off
  the list is still open. UNVERIFIED.
- vcod's binary starts at `dedicated 1`, not retail's 2. That is deliberate:
  dev and agent runs stay off the public list unless they opt in with
  `--set dedicated=2`.

### rcon (`SVC_RemoteCommand`, 0x808c404)

- The password cvar is `rconPassword` (registered at 0x8d699, flags 0x100);
  Q3's `rcon_password` does not exist. Cvar names fold case, so
  `+set rconpassword x` works. VERIFIED.
- The whole packet is tokenized; argv 1 is the password, argv 2 on the
  command. INFERRED from the `Cmd_Argv` calls. The line is the one every
  query reads (`MSG_ReadStringLine`, `Cmd_TokenizeString`, "The challenge
  key"), so a comment is gone before the rebuild below. VERIFIED live on
  1.1d, 1.5b and UO 1.51b (2026-10-16): `set vc_t "a//b"` read back as
  `"vc_t" is:"a^7"`, since the rebuilt `set vc_t a//b ` is tokenized again;
  `set vc_t "a b//c"` as `a b//c` (requoted, and a quoted `//` is no
  comment); `set vc_t a/*/b` as `a b`. Chat over rcon in
  `docs/research/cod11-chat.md` section 1.
- Rate limit: one request per 500 ms server-wide, whatever the password or
  source. A request arriving within 499 ms of the last answered one is
  dropped with no reply, and a dropped request does not restart the window.
  VERIFIED by capture: a second socket 5 ms after the first got nothing, a
  good password 450 ms after an answered one got nothing, and one 200 ms
  after a dropped one was answered. The check (`cmp eax,0x1f3` at
  0x808c423, skipped while the last time is still 0) runs before the
  password test. INFERRED.
- The window's clock is the time at the handling: `SVC_RemoteCommand`
  (1.1d 0x808c404) calls 0x806d988 first (0x808c410) and compares its
  result against the last time at 0x80e30c8. VERIFIED (disassembly).
  0x806d988 pumps `Com_GetRealEvent` until an empty event and returns its
  time, Q3's `Com_Milliseconds`. INFERRED (its shape against Q3). With
  the 5 ms socket read ("Pings"), two requests 505 ms apart
  were both answered on 20 of 20 tries and 490 ms apart on 0 of 10.
  VERIFIED (live, 1.1d, 2026-10-17). vcod-server rebuilds that moment
  (`handle_queued` in `main.rs`, `engine::Arrival::read`): a packet's arrival, or the end of the
  previous packet's handling when that ran later (an rcon `map_restart`);
  its tick is not counted, since retail's frame runs in well under a
  millisecond. Same 20 of 20 and 0 of 10 on vcod's debug build, where
  the tick-start read had answered 10 of 20 at 505 ms. VERIFIED (live).
- Replies, VERIFIED by capture:
  - no `rconPassword` set: `print\nNo rconpassword set on the server.\n`;
  - wrong password: `print\nBad rconpassword.\n`;
  - a good one: `print\n` followed by everything the command printed. An
    unknown command, an empty command and `say` print nothing, so the
    reply is a bare `print\n` (more in "Console commands over rcon").
    `map_restart` replies with the restart's log
    once it has run.
- The command line is rebuilt from argv 2 on, each token quoted when it is
  empty or holds a byte at or below a space, each followed by one space
  (0x806dbd4), into a 1 KB buffer; one that overflows is not run. INFERRED.
- Output is collected by `Com_BeginRedirect` into a 0x3ff0-byte buffer
  (0x808c526). A print that would take it past 0x3fef bytes sends what is
  there first (0x806b5db..0x806b5f5), so the reply splits at a print
  boundary. VERIFIED by capture: `fdir *.tga` came back as two packets,
  16350 and 14282 bytes of text after `print\n`. The rest goes out when the
  redirect ends, even when empty. INFERRED, and consistent with the bare
  `print\n` replies.
- The log line is `Rcon from <addr>:\n<argv 2>` or `Bad rcon from
  <addr>:\n<argv 2>` (0x80d5ae3, 0x80d5acd), printed before the redirect
  starts, so it is not in the reply. INFERRED.
- `status` (`SV_Status_f` 0x80846b4) prints `map: <name>`, a header, a rule,
  one row per slot not `CS_FREE`, then an empty line. Row format Q3's:
  `%3i %5i `, then `CNCT `, `ZMBI ` or `%4i ` ping, the name padded to 16,
  `%7i ` milliseconds since the last packet, the address padded to 22,
  `%5i` qport, ` %5i` rate. The port prints as a signed short
  (`127.0.0.1:-17446`). VERIFIED by capture.
- A test client's row: `NET_AdrToString` (0x8080ef4) prints `loopback`
  for type 2 and `%i.%i.%i.%i:%i` for type 4 (`NA_IP`); every other type,
  the bot address `SV_AddTestClient` builds (type 0, 0x80875cf) among them,
  takes the IPX format at 0x80d3180 over ten zero bytes. The port is the
  `bot%d` counter stored in host order (0x80875e3) and goes through
  `BigShort` and a sign extension like an IP's. VERIFIED (asm). None of the
  three Linux servers holds a `bot` string. VERIFIED (strings). Live,
  2026-10-18, `probe_testclient` on 1.1d mp_pavlov with `rcon status`:
  `  0     0  999 bot1               8700 00000000.000000000000:256    0  5000`
  (`bot2` `:512`, `bot3` `:768`): ping 999 (nothing acked), qport 0, and
  the userinfo's rate 5000, so the bot address is not LAN. VERIFIED
  (capture). vcod prints the same for a test client
  (`status_lists_test_clients_as_retail`) and gives its own bots the same
  address form with the slot plus one as the port; their ping stays 0.
- `clientkick <n>` (`SV_KickNum_f` 0x8084be4, usage `kicknum`) prints
  `Usage: kicknum <client number>`, `Bad slot number: %s`, `Bad client slot:
  %i` or `Client %i is not active` (0x8083b9c) and otherwise drops with
  `EXE_PLAYERKICKED` and sets the slot's last-packet time to now. INFERRED.
  The reply to a kick of slot 0 was `print\n0:vcod EXE_PLAYERKICKED\n`
  (plus `Going to CS_ZOMBIE for vcod` under `developer 1`). VERIFIED.

### Zombie slots (`SV_DropClient` 0x8085cf4, `SV_CheckTimeouts` 0x808cbc0)

- A drop sets the slot to `CS_ZOMBIE` (1) at once, before the game's
  disconnect runs, and returns early on a slot already a zombie. INFERRED
  from 0x8085d00..0x8085d29.
- Unless the reason is `EXE_DISCONNECTED`, every client above `CS_CONNECTED`
  is sent `e "\x15<name>^7 \x14<reason>"` (0x80d45b7, the `> 2` state test
  in 0x808b900); the zombie is not among them. Then `<slot>:<name> <reason>`
  is printed and the zombie gets `w "<reason>"`. INFERRED.
- A zombie is still sent a message each snapshot interval: its unacked
  server commands, then a snapshot. `SV_SendClientSnapshot` (0x808f844)
  calls `SV_BuildClientSnapshot` (0x808f130) for a zombie as for an active
  client, and that zeroes the ring slot's entity and client counts, then
  skips the copy of the playerstate and the entity walk for a zombie (the
  `state != 1` test before them). `SV_WriteSnapshotToClient` (0x808dee0)
  writes it with no delta base, since only a `CS_ACTIVE` client gets one.
  So the snapshot is the playerstate the slot held 32 messages earlier,
  written from null, no entities and no clients. INFERRED from the
  decompile of the three functions.
- Live, 2026-10-12, a probe that kept decoding after its `disconnect`
  (namespace capture, `rate 1000`, `snaps 30`): each zombie message held
  `w "EXE_DISCONNECTED"`, then a snapshot with `deltaNum` 0, `snapFlags` 5,
  a full spectator playerstate (808 bits), the entity terminator and no
  client: 131 bytes decoded, 136 on the wire. A probe that disconnected 25
  messages in got a null playerstate (16 bits), 36 bytes: its slot had never
  been written. VERIFIED (capture).
- A zombie's packets still go through the netchan and update its message
  and reliable acks; they are not executed and do not move its last-packet
  time (`cmp [piVar2],1` before the store at 0x808ca44). INFERRED.
- A zombie's messages are paced by `SV_SendMessageToClient` like any
  other, and a zombie is not `CS_ACTIVE`, so off the LAN they come at
  least a second apart (`0x808f806..0x808f824`). VERIFIED (code). Live,
  2026-10-11, the namespace capture of "Message pacing": after the probe's
  `disconnect` retail sent 136-byte messages 997 ms apart. VERIFIED
  (capture). vcod sent its zombie a message every frame, growing to 436
  bytes once the deltas ran out; its turn (`Server::send_zombie`) now
  paces it through `Client::pace`, and the same capture reads 1000 ms apart.
- `sv_zombietime` defaults to 2 (0x80d56cf). VERIFIED. The slot is freed
  once its last-packet time is more than `sv_zombietime` seconds old.
  `SV_CheckTimeouts` (0x808cbc0) runs after the frame's clock advance and
  game frame, right before `SV_SendClientMessages` (0x809045c), and
  compares `lastPacketTime < svs.time - sv_zombietime * 1000`. VERIFIED
  (code). The last-packet time is `svs.time` between frames, the last
  frame's, so the slot goes on the frame 2050 ms past the drop's packet,
  before that frame's message: two zombie messages off the LAN, 1000 ms
  apart. VERIFIED (capture, 2026-10-11 and 2026-10-12).
  VERIFIED by capture: after `clientkick` (which stamps that time) the row
  read `ZMBI` with 500, 1050 and 1550 ms at 0.51, 1.03 and 1.53 s, and was
  gone at 2.04 s. After a client's own disconnect the last `ZMBI` row read
  1550 ms and the next poll, 510 ms later, found the slot free.
- A timeout drop frees the slot at once (`*piVar3 = 0` after the drop in
  0x808cbc0), so the timed-out client is never sent its `w`. INFERRED.
- A zombie is not free for a new connect, but a connect from the same
  address and qport (or port) reconnects into it. INFERRED from Q3's
  `SV_DirectConnect`, which this one shares the shape of; not measured.
- vcod: `zombie_message` writes the ring slot the same way, and
  `check_timeouts` frees a slot on the advanced clock before any message
  goes out, off `Client::last_packet_ms`. The same capture against vcod-server: the same
  two messages 1000 ms apart, the same decoded layout. Where retail's was
  136 bytes, vcod's was 128-129. The difference was the playerstate:
  retail's spectator had stepped up 8 units with an `EV_STEP_VIEW` (143)
  and moved `bobCycle`, and vcod's had flown through the wall (protocol
  doc, "Spectator"). Before this, vcod repeated the last frame it sent as a
  delta. With the spectator's flight fixed (2026-10-12), the same capture
  reads 132 bytes against 136, and the two playerstates differ only in
  `velocity[2]`: retail's reads -0.0 after the stop, vcod's +0.0. VERIFIED
  (capture, decoded). The -0 comes from `PM_SlideMove`'s crease and the
  brush clip's axial normals, and an idle `PM_FlyMove` keeps it (protocol
  doc, "Spectator"). With both fixed, the same capture against vcod-server
  read 136 bytes, two messages 1000 ms apart, as retail's. VERIFIED
  (capture, 2026-10-12, sizes only).
- `SV_CheckTimeouts` treats a client in a slot the same way, on the same
  clock (0x808cbc0). VERIFIED (code): a `lastPacketTime` ahead of
  `svs.time` is pulled back to it; a slot above `CS_ZOMBIE` whose time is
  under `svs.time - sv_timeout * 1000` increments `timeoutCount`
  (`cl+0x10b1c`) and is dropped with `EXE_TIMEDOUT` and freed once the
  count passes 5; any other slot has the count zeroed; a slot with the
  word at `cl+0x5a8f4` set (the one the bot connect path at 0x80876a0
  stores 1 in) is skipped. `sv_timeout` is read as `->integer` each frame,
  default 240 (0x80d56c0). INFERRED: a silent client goes on the sixth
  frame past the timeout, 250 ms after it on a 20 Hz server. vcod: the
  same pass in `check_timeouts`, run before the snapshots
  (`a_silent_client_is_dropped_on_its_sixth_stale_frame`). Before
  2026-10-12 it ran on wall-clock time ahead of the clock advance and
  dropped on the first stale frame.
- Live, 2026-10-12, loopback `mp_carentan` with `+set sv_timeout N`, the
  probe's `--probe-edge silence`: it flew 5 s, sent one last packet
  straight after a snapshot (so the server stamped it with that
  snapshot's frame time T) and then sent nothing while it kept decoding.
  The last snapshot it got read `serverTime` T + 1250 at `sv_timeout` 1
  (3 runs), T + 2250 at 2 (3 runs) and T + 3250 at 3. No `w`, no OOB
  packet and nothing else followed. VERIFIED (capture). The frame at
  T + N*1000 is not stale (`<`, not `<=`), T + N*1000 + 50 is the first
  stale one, and T + N*1000 + 300 is the sixth, which drops before its
  own message. A check ahead of the clock advance would read one frame
  later, and a drop on the first stale frame 250 ms earlier. Both clocks
  and the count fit only as above. INFERRED from the totals; the run
  cannot split the clock order from the compare. Against vcod-server the
  same probe read T + 1250 (4 runs) and T + 2250 (2 runs of 3; the third,
  T + 2300, came on a loaded machine where the packet landed a frame
  late). `a_silent_client_s_last_snapshot_is_250_ms_past_its_timeout` in
  `crates/server/tests/connect.rs` pins it.
- Live, the same day, `--probe-edge zombie`: the probe sent `disconnect`
  in the first message after its gamestate, with no usercmd, so the slot
  went `CS_ZOMBIE` without ever being `CS_ACTIVE`. Retail sent 40
  messages, one a frame from the next frame on, the first carrying
  `w "EXE_DISCONNECTED"`. Each held a snapshot with `snapFlags` 6, no
  delta, a null playerstate (`commandTime` 0) and no entities, and the
  last went 1950 ms past the first, so the slot went free on the frame
  2050 ms past the drop's, before its message (`Going from CS_ZOMBIE to
  CS_FREE`). `--probe-edge zombie-active` (5 s in the world, then
  `disconnect`) got the same 40 messages with `snapFlags` 4 and the
  playerstate of 32 messages earlier. VERIFIED (capture). vcod-server
  sent the same on both runs: 40 messages, 6 and 4.
  `a_never_active_zombie_is_sent_not_active_snapshots_for_two_seconds`
  pins the first.

### The order of a frame's messages (`SV_SendClientMessages`)

All three servers send a frame's messages in one pass over the slots, in
slot order, whatever each slot's state. There is no separate pass for
zombies or for clients not yet in the world.

- `SV_Frame` ends `SV_CheckTimeouts`, `SV_SendClientMessages`, the
  snapshot archive, `SV_MasterHeartbeat("COD-1")`: 1.1d 0x808cbc0,
  0x809045c, 0x808fb84; 1.5b 0x8093eac, 0x8098a34; UO 1.51b 0x8094e0d,
  0x8099a2c. VERIFIED (the call sequences in the three decompiles).
- `SV_CheckTimeouts` frees the stale zombies (`Going from CS_ZOMBIE to
  CS_FREE`) on all three, before any message goes out. VERIFIED (the
  string inside each of the three functions).
- `SV_SendClientMessages` walks slot 0 to `sv_maxclients - 1` once. A slot
  whose state is not 0 and whose `nextSnapshotTime` has come gets its next
  netchan fragment if one is left, else `SV_SendClientSnapshot`: 1.1d
  0x808f844, 1.5b 0x8097c2f, UO 0x8098c23. VERIFIED (the loop: `state !=
  0` and the time test are its only conditions).
- `SV_SendClientSnapshot` serves states 1 to 4. It builds the snapshot
  and writes the server commands and the snapshot only for state 4
  (`CS_ACTIVE`) or 1 (`CS_ZOMBIE`). It writes the download blocks for any
  state but 1 (1.1d 0x8086290, 1.5b 0x808b8a9, UO 0x808c559), then
  `svc_EOF`. So a `CS_CONNECTED` or `CS_PRIMED` slot's bare 9-byte message
  goes out on that slot's own turn. VERIFIED (the `== 4 || == 1` and `!= 1`
  tests in all three).
- Live, 2026-10-24, loopback `mp_carentan` in a `unshare -rnm` namespace.
  An `AF_PACKET` socket on `lo` logged every datagram the server sent, in
  send order. A probe joined through a relay that passed only out-of-band
  packets, so its slot stayed `CS_CONNECTED` (`status`: slot 0 `CNCT`).
  A second probe joined at slot 1 and stayed in the world. On 1.1d, in
  every frame that held both (299 of 299), slot 0's 9-byte message went
  out before slot 1's snapshot. 1.5b gave 300 of 300 and UO 1.51b 299 of
  299. VERIFIED (capture). The only frames out of that order were the
  ones where slot 1 connected: they carry the packet handler's own sends,
  which happen before the frame.
- The same with a zombie at slot 0: `--probe-edge zombie-active` joined
  first, a plain probe second, and the first disconnected after 5 s. For
  the two seconds the zombie lasted, its 124-136 byte message went out
  before slot 1's snapshot in every frame, on 1.1d, 1.5b and UO 1.51b.
  VERIFIED (capture).
- vcod-server sends the snapshots of the
  clients in the world in slot order (`engine::send_messages`), then
  `send_unentered`, then `send_zombies`. The same two captures against it
  on 1.1: slot 1's snapshot went out before slot 0's 9-byte message in
  294 of 300 frames, and before slot 0's zombie message in all 40 zombie
  frames. VERIFIED (capture). Only the order inside a frame differs; the
  bytes, the pacing and the frame each message goes out on match.
- vcod-server now runs retail's one pass: `Server::message_due` takes
  every socketed slot, zombies included, `send_client_snapshot` sends a
  zombie's, an unentered client's or an active client's message by
  state, and `check_timeouts` frees the stale zombies first. The same two
  captures on 1.1: slot 0's 9-byte message first in 300 of 300 frames
  that held both (the one burst out of order held an rcon reply), and
  the zombie's 124-125 byte message first in all 40 zombie frames.
  VERIFIED (capture, 2026-10-24). `a_frames_messages_go_out_in_slot_order`
  pins both.

### When the game hears a drop (`SV_DropClient`, `ClientDisconnect`)

- 1.1d's `SV_DropClient` (0x8085cf4) sets the slot `CS_ZOMBIE`, clears
  its challenge, sends the `e` notice and prints `<slot>:<name> <reason>`,
  then calls the game's `ClientDisconnect` (`VM_Call` 5, guarded by
  `sv.state == SS_GAME`, returning to 0x8085e56), then queues the
  zombie's `w`. INFERRED (control flow).
- 1.5b's (0x808ac11) and UO's (0x808ba15) call the game first: their
  `SV_FreeClient` (1.5b 0x8089daf, UO 0x808abb3; `VM_Call` 7, then
  `SV_SetUserinfo(slot, "")`) runs before `Going to CS_ZOMBIE`, the state
  write and the `e` notice. INFERRED (control flow).
- `ClientDisconnect` (1.1d 0x42aac, 1.5b 0x4cf5f, UO 0x55433) opens with
  `Scr_Notify(player, "menuresponse", "-1", "disconnect")` (1.1d strings
  0x73113 and 0x73108, the `scr_const` slot `Cmd_MenuResponse_f` uses),
  then the follower pass (1.1d `Cmd_FollowCycle_f(ent, 1)` and
  `StopFollowing` when that fails; 1.5b and UO `StopFollowing` alone),
  the first complaint window naming the leaver and its `m -2`, UO's
  `VEH_UnlinkPlayer`, `HudElem_ClientDisconnect`, the callback,
  `G_FreeEntity`, and `CalculateRanks` last (1.1d 0x42c1f). INFERRED
  (control flow).
- Live, 2026-10-18, `client-probes/probe_dropclient` on all three retail
  servers (`+set sv_timeout 3`), a `--probe-edge watch` spectator beside a
  `--probe-edge silence` client and a `--probe-edge zombie-active` one,
  the same on 1.1d, 1.5b and UO 1.51b unless a bullet says otherwise:
  - The timeout drop's callback reads `getTime()` of the frame just run,
    and its `iPrintLn` rides that frame's message beside the drop notice:
    `e "...EXE_TIMEDOUT"` then `f "PROBE_GONE 1"` on 1.1d, `f` then `e`
    on 1.5b and UO. VERIFIED (capture).
  - A `disconnect` between frames runs the callback on the last frame's
    `getTime()` and its line rides the next message. VERIFIED (capture).
  - Neither the zombie (its message holds only `w`) nor, on 1.5b and UO,
    the client the game still sees in its slot hears the callback's
    `iPrintLn`. VERIFIED (capture).
  - The `menuresponse` waiter on the player's own thread never runs. A
    level thread waiting on the player wakes with `[-1] [disconnect]` and
    runs on the next frame's `getTime()`, after the callback; a `wait
    0.05` the callback started runs a frame after that. VERIFIED
    (capture).
- vcod, before 2026-10-18: the drop queued the disconnect for the next
  frame's packet pass, so a timeout's line went out a message after the
  notice, and nothing notified `menuresponse`. Now `Server::drop_client`
  runs `ScriptRuntime::client_disconnect` inside the drop, after the
  notice on 1.1 and before it on 1.5 and UO, with the slot already out
  of `clients`, and sends its commands and configstrings at once; the
  notify's waiters run in the next frame's first pass (`Vm::notify`), and
  the disconnect sets `ranks_dirty`. The same three runs against
  vcod-server print the same lines and the same message layout.
  `a_timed_out_clients_callback_lines_go_out_with_its_drop_notice`,
  `a_drop_notifies_menuresponse_for_the_next_frame` and
  `a_dropped_client_stops_its_followers_on_1_5` pin it.
- A command the callback sends the leaver alone. Neither engine tests
  the slot's state on the way in: 1.1d's `SV_GameSendServerCommand`
  (0x80894bc) hands any slot to `SV_SendServerCommand`, and
  `SV_AddServerCommand` (0x808b680) only squashes. A slot not `CS_ACTIVE`
  (or 32 commands past its ack) loses its unsent type-0 commands and
  takes no new one; a type-1 command still goes on. The zombie's `w`
  goes through the same call (0x8085e74) with the state already
  `CS_ZOMBIE`, so it squashes every print still unsent. INFERRED
  (control flow).
- Live, 2026-10-18, `client-probes/probe_reconnect`, its callback sending
  the leaver `self iPrintLn` and `self setClientCvar("probe_self", n)`,
  with a `--probe-edge zombie-active` client: the zombie's message holds
  `v probe_self "2"` then `w "EXE_DISCONNECTED"` on 1.1d, 1.5b and UO
  1.51b, and no print. VERIFIED (capture).
- vcod: `Server::client_disconnect` hands back the commands addressed to
  the leaver's slot, and `drop_client` queues them on the zombie, then
  squashes its ignorable commands (`Client::squash_ignorable`) ahead of
  the `w`. The same run against vcod-server prints the same two
  commands. `a_leavers_zombie_keeps_the_callbacks_reliable_commands_only`
  pins it.

### A reconnect into a held slot (`SV_DirectConnect`)

A connect from the address and qport (or port) of a slot that is not
free, past `sv_reconnectlimit`, prints `%s:reconnect` and takes that slot.

- 1.1d (0x80859b1..0x80859cd): when the slot's state is above
  `CS_ZOMBIE`, it frees only the client's script id (0x808d34c, the same
  call `SV_DropClient` ends with) and goes on to the game's
  `ClientConnect` (`VM_Call` 2, 0x8085baa). Nothing calls
  `ClientDisconnect`. The game's `ClientConnect` (0x4246c) zeroes the
  `gclient_t` and the entity and runs `G_InitGentity` over the held
  player in place. INFERRED (control flow).
- 1.5b (0x808a6e3) and UO 1.51b (0x808b4e7) call `SV_FreeClient` there
  instead, which runs the game's `ClientDisconnect` before the connect.
  INFERRED (control flow).
- Live, 2026-10-18, `client-probes/probe_reconnect` on all three, a
  `--probe-edge reconnect` client connecting again from its own socket
  and qport 5 s after it entered:
  - 1.1d runs no disconnect callback. The thread the first connect
    started on the player keeps running and reads the second connect's
    fields; the second connect's callback finds the first one's script
    field defined and `pers` empty. Both threads die at the client's
    final drop. VERIFIED (capture).
  - 1.5b and UO run the disconnect callback, then the connect callback,
    on the same `getTime()`. The second connect finds neither the field
    nor `pers`. VERIFIED (capture).
- vcod: the reconnect arm runs `Server::client_disconnect` inside the
  connect on 1.5 and UO. On 1.1 it runs none, and `ObjectTable::
  spawn_client` keeps the held player's script object (its threads and
  script fields), with the engine fields and `pers` fresh. The area tree's
  entry stays where it was filed, since the link lives in the engine's
  svEntity (cod11-combat.md 14.7) and `SV_UnlinkEntity` (0x8091f94) is not
  called; a walk reads the zeroed `r.contents` and box, so it never takes
  the entity, and the next link (the spawn's unlink, or the contents-0
  unlink of a spectator) takes it out (`AreaTree::zero_entity`,
  `a_reconnect_on_1_1_leaves_the_area_entry`). INFERRED.
  `a_reconnect_into_a_held_slot_follows_the_protocol` pins it. The same
  runs against vcod-server print the same lines. vcod's own guard stays:
  a connect with another challenge into a slot still heard from in the
  last 3 s is refused, where retail takes it at once.

### When the client's `begin` is heard (`ClientBegin`)

- 1.1d's `ClientBegin` (0x42f88) sets the client connected, runs
  `CalculateRanks` and notifies `begin` on the player. INFERRED (control
  flow).
- Live, 2026-10-18, `client-probes/probe_dropclient` and
  `probe_reconnect` on all three: the connect callback's line after
  `waittill("begin")` reads the `serverTime` of the client's first
  snapshot, a frame after the packet that entered the client. The waiter
  runs in the next frame's thread pass, as the other engine notifies do.
  VERIFIED (capture).
- vcod, before 2026-10-18: the waiter ran in the packet pass, a frame
  (50 ms) earlier. Now `ClientEvent::Begin` notifies through
  `Vm::notify` and sets `ranks_dirty`, as `ClientConnect`'s
  closing `CalculateRanks` (0x42677) does. Ours prints the first
  snapshot's `serverTime` too. `the_begin_waiter_runs_on_the_next_frames_clock`
  pins it.

### Console commands over rcon

Measured 2026-10-08 the same way (retail on a spare port, `rconPassword pw`,
one `vcod --net-probe` for the commands that need a client). Reply strings
VERIFIED by capture unless marked. `SV_AddOperatorCommands` (0x8084a3c)
registers `heartbeat`, `kick`, `banUser`, `banClient`, `clientkick`,
`status`, `serverinfo`, `systeminfo`, `dumpuser`, `map_restart`, `map`,
`map_rotate`, `gameCompleteStatus`, `devmap`, `killserver`, `scriptUsage`,
`stringUsage`, and `say` only when `dedicated` is non-zero. VERIFIED (string
table at 0x8084a3c's calls). vcod ports all of them; the last three are
below.

- `kick <name>` (`SV_Kick_f` 0x8084288, ported): exactly one argument, else
  `Usage: kick <player name>\nkick all = kick everyone\n` (0x80d3c80).
  `SV_GetPlayerByName` (0x8083aa0) walks every slot not `CS_FREE` and takes
  the first whose name matches the argument case-insensitively, as is or
  after `Q_CleanStr`; a miss prints `Player %s is not on the server\n`
  (0x80d3880). INFERRED from control flow; the miss line and the
  case-insensitive hit (`kick VCOD` dropped `vcod`, reply
  `0:vcod EXE_PLAYERKICKED\n`) VERIFIED. The lookup runs before the `all`
  test, so `kick all` prints the miss line. VERIFIED on an empty server.
  It then drops every slot in use except a loopback one with
  `EXE_PLAYERKICKED`, stamping each slot's last-packet time. INFERRED. Kicking a loopback client by name broadcasts
  `e "EXE_CANNOTKICKHOSTPLAYER"` instead (0x80d3cc9). INFERRED; a dedicated
  server has none.
- `dumpuser <name>` (`SV_DumpUser_f` 0x8084cc0, ported): usage
  `Usage: info <userid>\n` (0x80d3f5a) unless exactly one argument, the
  same name lookup, then `userinfo\n--------\n` and `Info_Print` of the
  client's userinfo. The userinfo ends with `ip`, which `SV_DirectConnect`
  (0x8085498) appends with the key at 0x80d43da and the port as a signed
  short (`ip                  127.0.0.1:-9130`). VERIFIED.
- `Info_Print` (0x806bbd4): one line per pair, the key left-justified in 20
  columns, a longer key running straight into its value
  (`bg_fallDamageMaxHeight480`). VERIFIED.
- `serverinfo` / `systeminfo` (0x8084c68 / 0x8084c94, ported):
  `Server info settings:\n` (0x80d3f2c) or `System info settings:\n`, then
  `Info_Print` of the serverinfo or systeminfo cvar string. Retail's
  serverinfo keys are the 14 vcod's configstring 0 already carries, in the
  same order; a `set sv_hostname` shows up in it at once. VERIFIED. vcod's
  systeminfo carries the pak lists since 2026-10-09 (see "Pak checksums and
  pure servers").
- `say <text>` (`SV_ConSay_f` 0x8084974, ported): nothing without an
  argument; otherwise `"console: "` (0x80d3f1a) plus the joined arguments
  goes to every client past `CS_CONNECTED` as `h "\x15%s"` (0x80d3f24), a
  type-0 command. The rcon reply is a bare `print\n`. VERIFIED: the probe
  read `h "\x15console: hello there"`.
- `set` / `seta` (ported): `usage: set <variable> <value>\n` (0x80cfdc0;
  `seta` names itself) with fewer than two arguments; otherwise the value
  is argv 2 onward joined by single spaces (`set foo a b c` reads back
  `a b c`) and the reply is empty. VERIFIED.
- `Cvar_Command` (ported): a line whose first word names a cvar prints
  `"%s" is:"%s^7" default:"%s^7"\n` (0x80cfd40) with the registration
  spelling (`foo2` answered `"FOO2"`), plus `latched: "%s"\n` (0x80cfd5f)
  when a latched value waits; with a second word it sets the cvar
  (`foo baz`). The default is the value the cvar was created with: `set foo
  a b c` then `foo baz` reads `default:"a b c^7"`, and `scr_dm_timelimit`,
  which only `default_mp.cfg`'s `set` creates, reads `default:"30^7"` after
  a `set` to 5. VERIFIED. A name that is no cvar is an unknown command and
  prints nothing.
- `g_gametype` is latched: `set g_gametype tdm` replies `g_gametype will be
  changed upon restarting.\n` (0x80cfc40), and the query then reads
  `"g_gametype" is:"dm^7" default:"dm^7"\nlatched: "tdm"\n`. VERIFIED.
  `cvarlist` flags `g_useGear` `L` too. VERIFIED. The serverinfo
  configstring keeps the old value until the restart: `serverinfo` read
  `g_gametype dm` with `tdm` latched. VERIFIED. vcod keeps `g_gametype`'s
  and `sv_maxclients`' latch in its own fields (the two `SV_MapRestart_f`
  escalates on, map-cycle doc section 4) and every other `L` cvar's in the
  cvar table, which a level load applies.
- The latch, measured over rcon on `g_gametype`, `sv_maxclients` and
  `g_useGear`: a value other than the live one and the waiting one prints
  `%s will be changed upon restarting.\n` (0x80cfc40) and waits; the
  waiting value again prints nothing; the live value prints nothing and
  clears the wait (the next query has no `latched:` line). VERIFIED.
- The other refusals of `Cvar_Set2`, each VERIFIED by rcon: an `R` cvar
  prints `%s is read only.\n` (0x80cfbf8; `set sv_cheats 1`, `set version
  x`), an `I` cvar `%s is write protected.\n` (0x80cfc0a; `fs_game`,
  `net_qport`), and a `C` cvar while `sv_cheats` is 0 `%s is cheat
  protected.\n` (0x80cfc22; `set timescale 2`, and `timescale 2` through
  `Cvar_Command`). The name is the one typed. Under `sv_cheats 1` the same
  `set timescale` answers nothing. vcod applies all four from the cvar
  table's flags.
- `cvarlist [filter]` (`Cvar_List_f` 0x806f530): one line per cvar whose
  name matches the filter, seven flag columns then ` %s "%s"\n`; the
  columns test, in order, 0x4 `S`, 0x2 `U`, 0x40 `R`, 0x10 `I`, 0x1 `A`,
  0x20 `L`, 0x200 `C`, each a space when clear. INFERRED from the branch
  order; the letters and their columns VERIFIED (`S    L  g_gametype`,
  `    AL  g_useGear`, `     LC fs_ignoreLozalized`). It ends with
  `\n%i total cvars\n%i cvar indexes\n`, both counting every cvar, matched
  or not (210 and 210 on an idle `dm` server). VERIFIED. The list is sorted
  case-insensitively with letters folded to lower case: `scr_hq_scorelimit`
  sorts before `scr_hqt_scorelimit` and `g_ScoresBanner_Allies` before
  `g_scriptMainMenu`. VERIFIED. The filter is `Com_Filter` (0x8067774,
  called with `casesensitive` 0), Q3's with one change: a `*` run ends at
  the next `*` or `?` only, so a `[` inside one is literal. INFERRED from
  0x8067774's control flow. The rest follows Q3's: the filter's end
  matches whatever is left of the name, `?` steps one character, a `*`
  run is found at its first occurrence (0x8067660, no backtracking),
  `[...]` is a set with `a-z` ranges and `]]` for a literal `]`, and `[[`
  drops the first `[`. INFERRED from the same function. Measured over
  rcon on 1.1d, each VERIFIED: `cvarlist fs_` listed all nine `fs_*`;
  `G_SPEED`, `g_spe`, `g_spe?`, `g_speed??`, `[[g]_speed`, `g_s[pq-r]`
  and `G_[A-z]PEED` listed `g_speed` alone; `sv_*rate` `sv_maxRate`;
  `*ps` `g_banIPs` among others; `g_[s]` the eight `g_s*`; `*_[s]`,
  `*[ab]`, `g_s[]]`, `g_speed[]` and `g_speed[a]` nothing. A `?` past the
  name's end leaves retail reading past its terminator (`g_speed??`
  still matched, the filter ending there); vcod lets nothing more match
  at that position (`cvars::com_filter`).
- The registry: `tools/capture_cvars.py` lists every cvar a retail server
  holds with its flags and the default its query prints, and wrote
  `crates/server/src/cvars/registry.rs`, which seeds vcod's table. VERIFIED.
  Leaving out the cvars `default_mp.cfg` creates and the ones the scripts
  register, the defaults that differ from the running values were
  `dedicated 2`, `net_port 28960`, `sv_maxclients 20`, `sv_pure 1`,
  `sv_hostname CoDHost`, `mapname nomap`. VERIFIED. A `+set` before the
  registration keeps its value and the registration still sets the
  default (`sv_maxclients` read `is:"8" default:"20"`). A registration of a
  cvar a `set` created takes over its default: `scr_allow_fg42` read
  `is:"0" default:"1"` after `default_mp.cfg`'s `set` and the script's
  `makeCvarServerInfo(..., "1")`. VERIFIED. That retail homepath mounted
  1.5's localized paks, whose `default_mp.cfg` also sets `scr_killcam`,
  `scr_freelook`, `scr_teambalance`, `scr_spectateenemy` and four `scr_hq*`
  limits, so its count ran 8 above vcod's on a 1.1 install. VERIFIED.
- `map` / `devmap` (`SV_Map_f` 0x8083c68, map-cycle doc 5.3): the existence
  check prints `Can't find map %s\n` (0x80d3903) with the path it tried:
  `maps/mp/%s.bsp`, or `maps/%s.bsp` for an argument starting `mp/`, and
  `maps/mp/.bsp` with no argument at all. VERIFIED (`devmap`, `devmap
  nosuchmap`). The `Cvar_Set("sv_cheats", ...)` comes after the load
  (0x8083dac..0x8083dd1), `"1"` for `devmap` and `"0"` (0x80d392c) for
  `map`. VERIFIED by probe: after `devmap mp/mp_carentan` and then `map
  mp_harbor`, both gamestates carried systeminfo `sv_cheats\1`, and after
  the second one a configstring 1 update followed while `sv_cheats` queried
  0. `devmap` on the running map took the restart path, printed the game
  module's `==== RestartGame ====` banner block, and was followed by a
  configstring 1 update. VERIFIED.
- The restart banner. `map_restart`, `map_restart 1`, `map_restart 5`,
  `map <the running map>` and `devmap <the running map>` over rcon each
  answered with one packet, the same 165 bytes with and without a client
  connected: `print\n==== RestartGame ====\n------- Game Initialization
  -------\ngamename: main\ngamedate: Nov 13 2003\n0 teams with 0
  entities\n-----------------------------------\n`. VERIFIED (2026-10-09,
  mp_carentan dm). `==== RestartGame ====` is `G_ShutdownGame`'s (0x4feaa,
  string 0x75665); the next three lines are `G_InitGame`'s (0x4fb2e,
  0x4fb40, 0x4fb55), with `main` (0x75430) and `Nov 13 2003` (0x7541b)
  compiled in; the team line is `G_FindTeams`' (0x4faff, format 0x75480),
  and the closing dashes are `G_InitGame`'s (0x4fe40, string 0x75640),
  printed after `G_SpawnEntitiesFromString` and ahead of `Scr_InitSystem`
  and the gametype load. VERIFIED. `fast_restart` answered an empty
  `print\n`. VERIFIED. vcod prints the block as one string ahead of the
  script load (`RESTART_GAME_BANNER`), with the team count fixed at 0: no
  stock MP map has a `team` key. A fresh `map` load's reply on retail is the
  engine's server and filesystem init around the same `G_InitGame` block;
  vcod prints nothing for it.
- `quit` gets no rcon reply: `Com_Quit_f` exits inside the redirect, before
  it is flushed. VERIFIED (no packet came back).

The last three were measured 2026-10-16 on retail 1.1d started by hand with
`+set dedicated 2 +set sv_master1 127.0.0.1` ahead of `+map mp_carentan`
(so the heartbeat never left the machine), one `vcod --net-probe` client in
the game and a socket on 127.0.0.1:20510 standing in for the master.

- `gameCompleteStatus [challenge]` (`SV_GameCompleteStatus_f` 0x8084d54,
  ported) calls `SV_MasterGameCompleteStatus` (0x808bbc4). VERIFIED (the
  thunk). It does nothing unless `dedicated` is 2 (`+0x20 == 2` at
  0x808bbd9), the heartbeat's test. INFERRED. Per non-empty `sv_master1..5`
  it resolves the name only when the cvar's `modified` flag is set, clears
  the flag, prints `Resolving %s`, `%s resolved to %i.%i.%i.%i:%i` or
  `Couldn't resolve address: %s` as the heartbeat does, then prints
  `Sending gameCompleteStatus to %s\n` (0x80d59a0) and sends. VERIFIED
  (strings, and the replies below). The resolved addresses live in its own
  table at 0x8160b20; the heartbeat's is 0x8160aa0 (0x808ba0c's
  `local_c`), and both clear the same flag. VERIFIED (the two tables in the
  export). So whichever of the two runs first after a change of
  `sv_masterN` resolves it, and the other sends to a zeroed address, which
  goes nowhere. VERIFIED by capture: with `sv_master1` resolved by the
  heartbeat at load, `gameCompleteStatus` replied `print\nSending
  gameCompleteStatus to 127.0.0.1\n` and no packet reached 20510; after
  `set sv_master2 127.0.0.1`, `gameCompleteStatus abc` replied
  `Sending gameCompleteStatus to 127.0.0.1\nResolving 127.0.0.1\n127.0.0.1
  resolved to 127.0.0.1:20510\nSending gameCompleteStatus to 127.0.0.1\n`
  and one packet arrived; a forced `heartbeat` then sent one heartbeat, not
  two. With `dedicated 1` the reply is a bare `print\n`. INFERRED from the
  gate; vcod's does the same.
- The packet (`SVC_GameCompleteStatus` 0x808bfa8): `Cvar_InfoString(4)`,
  `Info_SetValueForKey(.., "challenge", Cmd_Argv(1))` (key 0x80d59cb), the
  `demo` keyword when `fs_restrict` is set, then `%i %i "%s"\n` (score,
  ping, name) per client at `CS_CONNECTED` or above, sent as
  `gameCompleteStatus\n%s\n%s`. VERIFIED (strings and calls). Captured:
  `\xff\xff\xff\xffgameCompleteStatus\n\g_gametype\dm\gamename\main\mapname\mp_carentan\protocol\1\shortversion\1.1\sv_allowAnonymous\0\sv_floodProtect\1\sv_hostname\CoDHost\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0\sv_minPing\0\sv_privateClients\0\sv_pure\0\challenge\abc\n0
  0 "vcod"\n`. Without an argument the `challenge` key is absent:
  `Info_SetValueForKey` drops an empty value. VERIFIED. Unlike
  `statusResponse` there is no `pswrd`. VERIFIED. vcod adds the `demo`
  keyword as `statusResponse` does ("`fs_restrict`" below).
- `scriptUsage` (0x8084d64 -> 0x809d2d0, ported in shape): walks every
  thread object in the variable table (type 10 under flag 0x60), collects
  each one's call stack, sorts and groups threads whose stacks match, and
  per group prints `count: %d, var usage: %d, endon usage: %d\n`, the
  innermost position and then `called from:\n` and each caller's; then one
  `ent type '%s'... count: %d, var usage: %d\n` per object class holding
  any, between `********************************` rules. A position prints
  `@ %d\n` (bytecode offset) without script sources loaded and
  `(file '%s'%s, line %d)` with them (0x809b9b0, 0x809b89c). VERIFIED
  (strings, calls); INFERRED for what each count totals. Captured with one
  client: `count: 1, var usage: 7, endon usage: 2\n@ 1480\ncalled
  from:\n@ 7903\ncount: 1, var usage: 2, endon usage: 0\n@
  6334\ncount: 1, var usage: 5, endon usage: 0\n@ 7442\n`, then `ent type
  'entity'... count: 211, var usage: 431` and `ent type 'hudelem'...
  count: 1, var usage: 1`. VERIFIED. vcod's numbers are its own VM's
  (`game::usage`): a thread's var usage is its defined locals across
  frames, its endon usage the endons it holds, a position is always the
  `(file '...', line N)` form; an entity's
  var usage is the fields its script struct holds, which leaves out the
  map keys retail keeps as variables (ours read 210 entities, 4 vars on
  mp_carentan with no client).
- `scriptUsage`'s order: each thread's record is its code positions,
  innermost first (the current position, then each caller's from the
  stack's type-6 entries), and `qsort` with 0x80a2218 orders records by
  those pointers element by element, a shorter record first on a shared
  prefix; a group is a run of equal records. INFERRED from 0x809d2d0 and
  0x80a2218. A pointer's order is the compiled code buffer's, which
  `GScr_LoadScripts` (game module 0x693c8) fills through
  `GScr_LoadGameTypeScript` (0x69158: the gametype, then
  `_callbacksetup`) and then `Scr_LoadScript("maps/mp/<map>")`. INFERRED. With `developer 1` and `developer_script 1` retail
  prints positions as `(file 'maps\mp\gametypes\tdm.gsc', line 992)\n`
  followed by the source line verbatim (its `\r\n` kept) and a line of
  spaces ending in `*` under the column. VERIFIED. On mp_brecourt every
  gametype listed its own groups in line order and then `_minefields`'
  one (`count: 32` at line 19), though `_minefields` sorts first by name;
  `tdm` gave lines 992 and 1184, `sd` 1246, 1291, 1691 and 1822, `re`
  1379, 1424, 1824, 1905 and 1988, `bel` 1076 and 1265. VERIFIED. vcod
  sorts a position by its file's place in install order
  (`Vm::thread_usage`; the loader installs the gametype's closure, then
  the level's, each file before the files it calls), then its line; that
  matches every capture above (`script_usage_orders_files_as_retail`).
  Where vcod's install order or a file's line order departs from
  retail's code order the groups still differ; vcod prints neither the
  source line nor the caret.
- `stringUsage` (0x8084d74 -> 0x809a780, ported in shape): the script
  memory tree, a buddy allocator over 65535 eight-byte buckets
  (`MT_GetSize` 0x809ae08 rounds a request up to a power of two of
  buckets, at most 2^16). It prints `%d subtree has %d * %d = %d free
  buckets\n` for orders 0 to 16 (free blocks of each order), two rules,
  `total memory alloc buckets: %d (%d instances)\n` and `total memory free
  buckets: %d\n` (65535 minus the allocated). VERIFIED (strings, the
  loop). Captured: `total memory alloc buckets: 4580 (1158 instances)`,
  `total memory free buckets: 60955`. VERIFIED. vcod has no such tree:
  its instances are the strings the VM's interner holds (never freed),
  each sized as four header bytes, the text and its NUL rounded up as
  `MT_GetSize` does, and the free buckets are listed one block per set bit,
  a compacted tree. That model is vcod's, not retail's; the header size is
  INFERRED from Q3's `RefString`. Ours read 4171 buckets for 1010 strings
  on mp_carentan.

### Pings (`SV_CalcPings`, 0x808cab8)

- `SV_SendMessageToClient` (0x808f680) stamps
  `frames[outgoingSequence & 31]` with `messageSent = svs.time` and
  `messageAcked = -1` for every message, the gamestate's
  (`SV_SendClientGameState` 0x8085eec) included. `SV_UserMove` (0x8086fa4),
  once a move message's cmds decode, writes `svs.time` into
  `frames[messageAcknowledge & 31].messageAcked`, overwriting an earlier
  ack of the same message. INFERRED.
- Once a frame, before the clock advances (`SV_Frame`, the call ahead of
  the `svs.time += frameMsec` loop), each client's ping is 999 unless it is
  `CS_ACTIVE` with a game entity; otherwise the mean of `messageAcked -
  messageSent` over the 32 slots with `messageAcked > 0`, integer division,
  capped at 999, and 999 with none. The result is copied to `ps->ping`
  (`+0x20c8`). There is no bot branch. INFERRED.
- Both stamps are `svs.time`, which moves in 50 ms steps, and packets are
  read between frames, so a message acked before the next frame counts 0
  and one acked a frame later 50. VERIFIED by capture: a loopback probe's
  `status` ping read 0, 1 or 3 over 15 polls (one or two 50 ms samples in
  32).
- The format strings: `status` prints the ping as `%4i`, and `getstatus`'
  player lines are `%i %i "%s"\n`. VERIFIED. The fields: `status` reads
  `cl->ping`, `getstatus` (0x808bd58) writes score then `cl->ping` for
  every slot past `CS_ZOMBIE`, and the scoreboard's ping is `ps.ping`
  capped at 999, or -1 for a client still connecting (hud protocol doc,
  section 3). INFERRED.
- The read schedule. The dedicated main loop (`main`, cod_lnxded
  0x80c6870, the loop at 0x80c69c0) is `usleep(5000)` then `Com_Frame`
  (0x806ce88), forever. VERIFIED (`push $0x1388`, `call usleep`, `call
  0x806ce88`, `jmp 0x80c69c0`).
- `Com_Frame` runs `Com_EventLoop` (0x806bed4), which drains every queued
  event, packets included, and handles each at once through
  `SV_PacketEvent`, until at least 1 ms has passed (`com_maxfps` is read
  only when `dedicated` is 0), then calls `SV_Frame` (0x808cdf8) with the
  elapsed msec. VERIFIED, `cod_lnxded` decompile, 2026-10-19:
  `Com_EventLoop` loops until an event of type 0, taking the pushed ring
  first and then `Sys_GetEvent` (0x806bdb0, whose `0x80c61e4` queues the
  console line and then each `Sys_GetPacket` (0x80c71f0, `recvfrom` at
  0x80c723f) datagram as type 6); a type 6 event goes to `SV_PacketEvent`
  (0x808c870, the call at 0x806c1b8) while `sv_running` is set; then it
  drains the loopback queues (`0x8080fc0`, the server's at 0x806bfec) and
  returns the type-0 event's time. `Com_Frame` calls it in the
  `do ... while (msec < min)` loop at 0x806cfe0..0x806d001 (`min` 1, or
  `1000 / com_maxfps` when that is positive and `dedicated` 0), then
  `Cbuf_Execute` (0x805aa7c), then `SV_Frame` at 0x806d0e4.
- `SV_Frame` adds the msec to `sv.timeResidual` and runs a game frame only
  once the residual reaches `1000 / sv_fps`, sending the snapshots at the
  end. INFERRED.
- So retail reads the socket every 5 ms and stamps a move message with the
  frame time current when it arrived; only a packet that lands during a
  frame's own run is stamped with that frame. `NET_Sleep` (0x80c786c,
  `select` on the socket) is called only from `SV_SpawnServer`'s map-change
  wait, so the frame does not wake on a packet. INFERRED.
- Why the overwrite matters: the last move message acking frame N is the one
  a client sends just before it reads N + 1. If that one is stamped with
  frame N + 1's time, slot N reads 50. Retail's window for that is its frame
  run; vcod's was the whole tick, since the tick read the socket only at its
  start. INFERRED.
- Before the change, a loopback `--net-probe` (16 ms cmd interval) read
  21-50 in `status` on vcod's debug build, ticking at a 25 ms mean on a
  loaded host, and the ring alternated 0 and 50 ms samples. VERIFIED by
  capture (2026-10-08).
- vcod: `Client::stamp_sent` / `stamp_acked` / `calc_ping`,
  `Server::calc_pings`. A reader thread stamps each packet's arrival, the
  tick still handles them in arrival order at its start, and
  `Server::ack_stamp` stamps a move message's ack with the frame time
  of the newest frame whose messages had gone out at arrival
  (`Server::frame_sent`), modelling retail's frame as instantaneous. A
  packet left over past `MAX_PACKETS_PER_FRAME` from an earlier tick takes
  the last frame's time. A vcod bot reads 0, Q3's rule; retail has no bots.
- Measured 2026-10-08 on loopback, mp_harbor, one `--net-probe`, 20
  `status` polls 1 s apart: retail (port 29561) read 0-3 (`1 0 0 1 1 1 3 0 0
  0 0 0 0 1 1 0 0 0 0 0`); vcod's debug build (29562) read 21-50 before the
  change and 0-3 after (`3 0 1 1 0 1 0 1 0 0 1 0 1 0 0 3 0 0 0 0`) with the
  tick still at 11-19 ms mean. VERIFIED by capture.

### A quiet client: `ClientEndFrame`'s dead 0x1000

Q3's `EF_CONNECTION` rule survives in all three game modules and never
reaches the wire, because the entity's `eFlags` are overwritten from the
playerstate right after it.

- VERIFIED, game.mp.i386.so 1.1d: `ClientThink` stores `level.time`
  (`level+0x1e8`) into `gclient+0x21e4` after `trap_GetUsercmd` (0x41592).
  `ClientEndFrame` (0x40e98) sets byte `ent+0x9` bit 0x10 (`s.eFlags`
  0x1000) when `level.time - gclient+0x21e4 > 1000` and clears it
  otherwise (0x4112d..0x41153). The next call on either branch is
  `BG_PlayerStateToEntityStateExtrapolate` (with `g_smoothClients`) or
  `BG_PlayerStateToEntityState` (0x41170..0x41191), and both store
  `ps.eFlags` (`ps+0x80`) over `s.eFlags` whole before they set their own
  bits. No other instruction in the module sets or clears that bit on a
  playerstate or entity.
- VERIFIED, 1.5b game.mp.i386.so: the same rule on `gclient+0x21ec`
  (`ClientEndFrame` 0x3ab2a, compare at 0x3adfd, `or edx,0x1000` at
  0x3ae0d), followed by `G_SetClientSound` and the same two copies
  (0x3ae71, 0x3ae8d); `BG_PlayerStateToEntityState` stores `ps+0x80` into
  `s.eFlags` whole.
- VERIFIED, UO 1.51b game.mp.uo.i386.so: the same on `gclient+0x4628`
  with UO's bit 0x800 (`ClientEndFrame` 0x42f59, compare 0x432ff, `or
  edx,0x800` 0x4330f), then the copies at 0x43373 / 0x4338f, which store
  `ps+0x84` into `s.eFlags` whole.
- VERIFIED by capture, 2026-10-20, mp_carentan under
  `client-probes/probe_passthru` with `probe_teleport 1`: an axis
  `--net-probe` stopped with SIGSTOP for 5 s while an allied probe
  recorded snapshots. On 1.1d its entity's `pos.trTime` held at 26466
  from 26700 to 31500 (97 snapshots, about 5 s without a cmd) and its
  `eFlags` read 0x18 throughout; on UO 1.51b `pos.trTime` held at 30300
  for 97 snapshots and `eFlags` read 0x18 raw throughout, never 0x800.
- INFERRED: the cgame's disconnected icon (gametypes doc 7.3, "The
  second sprite") therefore never draws for a lagging player on a stock
  server. vcod-server leaves the bit off as retail does
  (`ClientSim::entity_eflags`).

### Rate (`SV_UserinfoChanged`, 0x8086ab4)

- `cl->rate`, which `status` prints, is 99999 when `Sys_IsLANAddress`
  (0x80c72f8) holds and `dedicated` is not 2. Otherwise it is the
  userinfo's `rate` through `strtol`, clamped to 1000..90000, or 5000 when
  the key is empty or missing. INFERRED from control flow. VERIFIED: the
  read is `__strtol_internal(s, 0, 10, 0)` at `0x8086b93` (`snaps`' at
  `0x8086d0a`). INFERRED, off glibc's `strtol`: a value past `i32`
  saturates before the clamp, as `vcod_common::text::atoi` does, which
  both vcod servers' `engine::link` reads take. VERIFIED by capture: a loopback probe sending `rate 25000` read 99999 under
  `dedicated 1`; under `dedicated 2` (the 2026-10-07 captures above) it read
  25000.
- `Sys_IsLANAddress` (cod_lnxded `0x80c72f8`, CoDMP.exe `0x464be0`) is Q3
  1.32's. VERIFIED from the decompiles, check by check:
  - Address types: lnxded passes loopback (2) and IPX (5); CoDMP.exe also
    passes type 0 (bot). Any type other than IP (4) then fails.
  - CoDMP.exe alone passes `127.0.0.1` exactly; lnxded has no such line.
  - Class A (`ip[0] & 0x80 == 0`): the first octet equals a local address's.
  - Class B (`ip[0] & 0xc0 == 0x80`): the first two octets match, or both
    addresses sit in 172.16/12.
  - Class C (the rest): the first three octets match, or both sit in
    192.168/16.
  - There is no RFC 1918 shortcut: 10/8 is plain class A, and 172.16/12 or
    192.168/16 count only with a local address in the same block.
- The local table is `NET_GetLocalAddress`: `gethostname` then
  `gethostbyname`, at most 16 IPv4 addresses, printed as `Hostname: %s`,
  `Alias: %s`, `IP: %i.%i.%i.%i` (CoDMP.exe `0x465360`, table `0x8e3c78`,
  count `0x8e3cb8`; lnxded `0x80c7438`, table `0x831a880`, count
  `0x831a864`). VERIFIED. lnxded keeps Q3 unix_net.c's off-by-one: the
  count starts at 1 and slot 0 stays 0.0.0.0, so it holds 15 real
  addresses. VERIFIED. No `SIOCGIFCONF`: the binary's only ioctl is
  `FIONBIO`. VERIFIED. On a host whose hostname resolves to 127.0.1.1
  alone, every real LAN peer fails the check. INFERRED.
- Callers. lnxded, four (VERIFIED): `SV_GetChallenge` `0x8084d90` skips the
  authorize server for a LAN client under `net_lanauthorize 0`;
  `SV_DirectConnect` `0x8085498` applies `sv_minPing`/`sv_maxPing` only off
  the LAN; `SV_UserinfoChanged` `0x8086ab4` (the rate above);
  `SV_SendClientMessage` `0x808f680` skips `SV_RateMsec` throttling for a
  LAN client. CoDMP.exe, six (VERIFIED): `CL_ReadyToSendPacket` `0x40b940`
  (see docs/protocol-1.1.md, "The client's send rate"), `CL_CheckForResend`
  `0x4103d0` (the key authorize skip under `net_lanauthorize 0`), and the
  listen server's copies of the four above (`0x452da0`, `0x453390`,
  `0x454d70`, `0x45dae0`). No browser path calls it: the Local source is
  broadcast `getinfo xxx` alone (`CL_LocalServers_f` `0x413710`). VERIFIED.
- vcod (`vcod_common::net::lan`) runs the same class match on client and
  server against the host's interface addresses (`getifaddrs` on Unix,
  loopback always included) instead of the hostname lookup, so a Linux host
  whose name resolves to 127.0.1.1 still sees its LAN. On Windows it resolves
  `COMPUTERNAME`, as retail does.

### Bans (`SV_BanUser_f` 0x8084394, `SV_BanNum_f` 0x8084524)

- 1.1 keeps no ban file: the binary has no `ban.txt` string. Both commands
  find the client the way `kick` and `clientkick` do, then send the
  out-of-band `banUser %i.%i.%i.%i` (0x80d3d7f) with the client's IPv4
  address to `codauthorize.activision.com` port 20500 and print `%s was
  banned from coming back\n` (0x80d3da0) with the client's name. The client
  is not dropped. A loopback client gets `e "EXE_CANNOTKICKHOSTPLAYER"`
  instead. INFERRED from 0x8084394's control flow.
- VERIFIED by capture (`banUser vcod`, `banClient 0` against a probe, with
  a UDP listener on the address the host resolves the authorize name to):
  usage lines `Usage: banUser <player name>\n` (0x80d3ce6) and `Usage:
  banClient <client number>\n` (0x80d3dc0); the misses `Player nobody is
  not on the server`, `Client 3 is not active`, `Bad slot number: x`; the
  first ban's reply `Resolving codauthorize.activision.com\n`
  `codauthorize.activision.com resolved to <ip>:20500\n` then `vcod was
  banned from coming back\n`, later ones only the last line; the listener
  read `\xff\xff\xff\xffbanUser 127.0.0.1` from the server's own port each
  time; the probe stayed connected and `status` still listed it.
- The ban takes effect only through the authorize server's answer to a
  later `getIpAuthorize`, which `SV_GetChallenge` sends for a client that
  is not on the LAN (`net_lanauthorize 0`). `SV_AuthorizeIpPacket`
  (0x808514c) relays a `deny` to the client as `error\nEXE_ERR_CDKEY_IN_USE`
  with an empty reason or `INVALID_CDKEY`, as `needcdkey` for
  `CLIENT_UNKNOWN_TO_AUTH` or `BAD_CDKEY`, and as
  `error\nEXE_ERR_BAD_CDKEY` for anything else, `BANNED_CDKEY` included.
  INFERRED (0x8085339..0x80853d9). What reason the authorize server gives
  for a banned address is not measurable; it no longer answers.
- vcod: no authorize detour, so the list is vcod's own: 1.5's `ban.txt`
  (`engine::bans`, `--ban-file` to keep it), the client's guid stand-in
  (an off-LAN IPv4 address as a big-endian `i32`, a LAN client's 0) and
  its cleaned name a line. `getchallenge` from a banned guid gets
  `error\nEXE_ERR_BAD_CDKEY`; a LAN client has no guid and is never
  refused, as on retail. The reply skips the two `Resolving` lines and
  nothing goes to the authorize server.

### 1.5 temporary bans (`tempBanUser` 0x8087f2a, `tempBanClient` 0x8088087)

cod_lnxded 1.5b, read from the asm.

- Registered beside `banClient` (names at 0x80dddae, 0x80dddba, handlers
  stored at 0x80887ed and 0x8088801). VERIFIED.
- Each calls the helper `kick` and `clientkick` use (0x8087d89 by name,
  0x8087e9d by slot) with a 64-byte name buffer. Its tail (0x8087ceb)
  holds `e "EXE_CANNOTKICKHOSTPLAYER"` (0x80dda69) for a loopback client,
  copies the client's name (`cl+0x10a44`) into the buffer and cleans it
  (0x80860de), keeps `cl+0x5a980`, drops the client with
  `EXE_PLAYERKICKED` (0x80dda86) and returns what it kept. VERIFIED.
- `cl+0x5a980` is the guid: the connect path reads the challenge's
  `+0x2c` (0x808a1d0) and stores it there (0x808a91a); that field is what
  the authorize server's `accept` reply sets with `atoi` (0x8089696,
  string `accept` 0x80de100). VERIFIED.
- With a guid of 0 the handler stops after the kick (`je` at 0x8087f4a).
  VERIFIED. A LAN client is never asked about, so its guid is 0 (`getGuid`'s
  0 in the 1.5b log). INFERRED.
- With any other guid the handler prints `%s (guid %i) was kicked for
  cheating\n` (0x80ddb00) with the cleaned name and calls 0x80890f5, which
  writes the guid and the game clock (0x83ccd84, the scaled one,
  `cod15-protocol-diff.md` 12) into a 16-entry table at
  0x83e1e0c, 8 bytes an entry. VERIFIED. The entry is the first whose guid
  is 0, else the one with the lowest time (0x8089096's loop). INFERRED.
- The check (0x8088f75) is called at 0x808979c, on the path that handles
  the authorize `accept`. VERIFIED. It passes a guid of 0 and refuses one
  with an entry whose `svs.time - time` is not above `sv_kickBanTime->value
  * 1000.0` (cvar pointer 0x836b7c8, `"sv_kickBanTime"` default `"300"` at
  0x80dfb1f, the float 1000.0 at 0x80ddfe4). VERIFIED operands; the `ja` at
  0x8088fda makes the comparison "not above". INFERRED. The refusal prints
  `rejected connection from temporarily banned GUID %i\n` (0x80de1a0) and
  sends `error\n\x15You are temporarily banned from this server`
  (0x80de1e0). VERIFIED strings. The cvar is read at the check, so changing
  it lengthens or shortens bans already made, and no code empties an
  entry. INFERRED.
- vcod has no authorize server: an off-LAN client's address stands in for
  its guid (`engine::bans::guid_of`), a LAN client is kicked and not
  banned, and the refusal is retail's error string, sent at
  `getchallenge` (`Server::temp_ban_slot`, `engine::bans::TempBans`,
  shared with CoD2, whose table is the same). The print and the
  `rejected connection` line carry that stand-in; the script's `getGuid`
  stays 0. The table is stamped on the game clock, as retail's is; before
  2026-10-23 vcod stamped the unscaled one, so a ban aged through a match
  timeout.

### 1.5 permanent bans (`banUser` 0x8087f73, `banClient` 0x8087fc7, `unbanUser` 0x808801b)

cod_lnxded 1.5b, read from the asm. VERIFIED, `coduo_lnxded` 1.51b: UO's
code is 1.5b's instruction for instruction. Its `banUser` 0x8088d74,
`banClient` 0x8088dc8 and `unbanUser` 0x8088e1c match 1.5b's three
(70 instructions), and the file check 0x8089dfc, the ban 0x8089f25 and the
unban 0x808a021 match 0x8088ff8..0x80893d0 (275 instructions), with jump
targets taken relative and absolute addresses masked. The only operands
that differ are the `client_t` offsets: the guid at `cl+0xab080` (1.5b
`cl+0x5a980`) and the address type at `cl+0x9b048` (1.5b `cl+0x52948`).
The authorize `accept` calls the file check at 0x808a4c0 (1.5b
0x80896bc), and the strings sit at `.rodata` 0x80e3f28 (`ban.txt`),
0x80e3f66 and 0x80e3f84 (the two unban replies) and 0x80e4060 (the
refusal print). INFERRED from the match: what is measured on 1.5b below
holds on UO.

- 1.5 drops 1.1's authorize detour: neither command sends `banUser <ip>`
  and the binary has no `was banned from coming back` string. VERIFIED.
- `banUser` and `banClient` print `Server is not running.\n` (0x80dd74f)
  without a level, their usage lines with an argument count other than 2,
  look the client up as 1.1 does (0x8087300 by name, 0x8087417 by slot)
  and hand it to 0x8089121. VERIFIED.
- 0x8089121: a loopback client gets `e "EXE_CANNOTKICKHOSTPLAYER"`
  (0x80ddff0). A guid (`cl+0x5a980`) of 0 returns at once, so a LAN client
  is neither banned nor kicked and nothing prints. VERIFIED (asm), and by
  rcon against a LAN probe on the 1.5b server: `banUser vcod` and
  `banClient 0` each answered `print\n` alone, `status` still listed the
  probe, and no `ban.txt` appeared. A guid already in `ban.txt` returns
  too. VERIFIED. Otherwise the server opens `ban.txt` (0x80ddfe8) for
  append (0x8065e76, mode 2), returns if that fails, writes `%i %s\r\n`
  (0x80de00d) with the guid and the `Q_CleanStr`'d name, closes it, drops
  the client with `EXE_PLAYERKICKED` (0x80de015) and stores 0x83ccd88 in
  `cl+0x10c1c`. VERIFIED. That store is the kick's restart of the packet
  clock (Q3's `SV_KickNum_f`). INFERRED.
- The file check (0x8088ff8): a guid of 0 passes; otherwise it reads
  `ban.txt` through the search path (0x8062fbd), takes each line's first
  token (0x8085323) as an `atoi` guid and skips the rest of the line
  (0x80854d2). VERIFIED. It runs at the authorize `accept` before the
  temporary check (0x80896bc, then 0x808979c): a hit prints `rejected
  connection from permanently banned GUID %i\n` (0x80de120), sends
  `error\n\x15You are permanently banned from this server` (0x80de160)
  and clears the challenge. VERIFIED.
- `unbanUser` (0x808921d) needs an argument count of 2, else `Usage: unban
  <client name>\n` (0x80ddb82). It reads `ban.txt` and prints nothing
  when there is none. It cleans the argument, and for each line skips the
  guid token and the blanks after it and matches when the rest of the line
  equals the name ignoring case up to a `\r` or `\n`; a match is cut out
  of the buffer. The buffer is written back whether or not anything
  matched, then `unbanned %i user(s) named %s\n` (0x80de026) or `no banned
  user has name %s\n` (0x80de044) prints with the cleaned name. INFERRED
  from 0x808921d's control flow. VERIFIED by rcon on the 1.5b server:
  `unbanUser ^1foo` against `12345 Foo\r\n67890 Bar\r\n` answered
  `unbanned 1 user(s) named foo` and left `67890 Bar\r\n`; `1 Dup\n2
  dup\r\n3 Other\n4 Baz` lost both dups (`unbanned 2`), and `baz`, on a
  last line with no line end, was not found; `5   Spaced Name\r\n` went
  for `"spaced name"`; with no file the reply was an empty `print\n`.
- vcod: no guid, so a 1.5 or UO `banUser`/`banClient` treats an off-LAN
  address as the guid (a big-endian `i32`) and does nothing to a LAN
  client, as retail does to a guid of 0. The file is retail's, read and
  written as retail does: `engine::bans::BanFile`, the code CoD2's
  `ban.txt` runs on (`cod2-protocol.md` 17.7), at `--ban-file`. It is
  checked at `getchallenge` before the temporary bans with retail's
  print and refusal, and `unbanUser` gives retail's three replies, an
  empty one standing for no file. A ban appended after a last line with
  no line end runs into it, as on retail. Until 2026-10-23 vcod kept its
  own `<ip> <name>` lines and ended such a line before appending; a file
  in that shape is rewritten into guid lines when the server opens it.

### Shutdown (`SV_Shutdown` 0x808ad8c)

- `SV_FinalMessage` runs twice over every slot past `CS_ZOMBIE`: for a
  client that is not loopback, `e "%s"` with the reason (type 0, so a
  client not yet active drops it) and a bare `w` (0x80d57e1, type 1), then
  `nextSnapshotTime = -1` and `SV_SendClientSnapshot`. `quit`
  (0x806d910) passes `EXE_SERVERQUIT`, `killserver` `EXE_SERVERKILLED`.
  INFERRED. VERIFIED by capture: on `rcon quit` the probe read
  `e "EXE_SERVERQUIT"` then `w` and dropped; it read nothing after the
  first packet, so the second pass is not seen.
- `killserver` (0x8084d3c) calls `0x806dc68`, which runs `SV_Shutdown`
  between the hunk calls and returns; the process does not exit. INFERRED.
  `SV_Frame` does the same when `sv_killserver` is set, and resets it.
  INFERRED. With `sv_running` (0x833efc0) at 0 the event loop skips the
  call to `SV_PacketEvent` (0x808c870, returning to 0x806c1bd). INFERRED.
  VERIFIED by capture: `rcon killserver` got no reply, the probe read `e
  "EXE_SERVERKILLED"` and dropped, and afterwards rcon `status`, rcon `map
  mp_harbor` and `getinfo` all went unanswered while the process stayed
  up. Only its stdin console could load a map again. vcod: the same, and a
  console `map` (which only a test can push; vcod-server has no stdin
  console) brings it back.

### Out-of-band `disconnect`

- `SV_ConnectionlessPacket` (0x808c63c) compares the command with
  `"disconnect"` at 0x808c827 and, on a match, does nothing; any other
  unmatched command prints `bad connectionless packet from %s:\n%s\n`.
  VERIFIED (read off the decompile of 0x808c63c). A client leaves through the
  netchan `disconnect` command. vcod ignores it the same way.
- The server never sends one either; the rule that matters is the
  client's. `CL_DisconnectPacket` (CoDMP.exe 0x410620) returns unless the
  client state at 0x155f2c0 is non-zero (0 is disconnected; 3 is set at
  `connectResponse`, 0x410c7b), unless the packet came from the server's
  address (`NET_CompareAdr` 0x449230 against 0x15ef028), and unless
  `cls.realtime` (0x155f3e0) minus `clc.lastPacketTime` (0x15ce868) is at
  least 3000 (`cmp eax,0xbb8; jl` at 0x410663). Only then does it call
  `Com_Error(1, "EXE_SERVER_DISCONNECTED")`. INFERRED.
- `clc.lastPacketTime` is written at three places: `connectResponse`
  (0x410c85), every sequenced packet from the server's address while the
  state is at least 3, ahead of `Netchan_Process` (0x4110b1, so a packet
  the netchan then rejects still counts), and demo playback (0x40eac4).
  Connectionless packets never write it. INFERRED. So a forged
  `disconnect` cannot cut a link the server is still feeding, and one
  arriving while challenging reads a stale or zero stamp and drops at once.
- vcod's client (`NetClient::handle_oob`, `crates/common/src/net/mod.rs`)
  applies the same guard and stamps at the same two live points. A real
  CoDMP.exe was not run against it: nothing on the wire sends this packet.

### The challenge key (`SV_ConnectionlessPacket` 0x808c63c, `Info_SetValueForKey` 0x80827d4)

Measured on 2026-10-16 against the stock 1.1d, 1.5b and UO 1.51b Linux
servers on loopback (`tools/run_server.sh`, no client on), one `getinfo` or
`getstatus` per packet. All three answered every case below the same way.
VERIFIED (captures).

- A bare `getinfo` / `getstatus` (or `getinfo ""`) sends no `challenge`
  key at all; `infoResponse` then opens `\protocol\...`. VERIFIED.
- `challenge` is `Cmd_Argv(1)` of the packet's line. The line is read by
  `MSG_ReadStringLine`, which stops at a NUL or a newline and turns `%`
  and every byte above 127 into `.` (`a%b` -> `a.b`, UTF-8 `é` -> `..`,
  `0xff` -> `.`, `0x7f` kept). VERIFIED. `Cmd_TokenizeString` (0x805b398)
  then splits on any byte up to a space (`0x01`, `\r`, tab end the token),
  a `"` opens a token that runs to the next `"` or the line's end (`"a b"
  c` -> `a b`, `"ab` -> `ab`, `a"b` -> `a`), `//` ends the line and `/* */`
  is skipped (`a//b`, `a/*x*/b` -> `a`; `/*x*/b` -> `b`). VERIFIED.
- `Info_SetValueForKey` (0x80827d4) drops every `\`, `;` and `"` from the
  value (`a\b`, `a;b` -> `ab`), removes the key and appends it at the
  end, and adds nothing for an empty value. A `\`, `;` or `"` in the key
  is a `Com_Error`. VERIFIED (decompile of 0x80827d4). There is no length
  cap of its own short of the string's: a 200, 600 and 750 byte challenge
  came back whole. VERIFIED.
- `Cvar_InfoString` (0x806fc30), the serverinfo `getstatus` and
  configstring 0 carry, puts each cvar through the same 0x80827d4, and
  `Cvar_InfoString_Big` (0x806fc84, the systeminfo) through 0x80829e8,
  which strips the same three bytes into an 8 KB string. VERIFIED
  (decompile). So a cvar value keeps `;` and `\` and only the info string
  loses them: VERIFIED live, rcon `sv_hostname a;b\c` came back
  `\sv_hostname\abc` in 1.1d's `getstatus` and `\hostname\abc` in
  1.5b's and UO 1.51b's `getinfo`, and `sv_hostname "x y;z"` as
  `\hostname\x yz` in 1.1d's `getinfo`. vcod's `Info::set`
  strips the same way (it used to refuse a value with a `\`).
- A `"` reaches a cvar only from a script; no rcon or `+set` line carries
  one. VERIFIED live 2026-10-20 on 1.1d, 1.5b and UO 1.51b,
  `client-probes/probe_svinfoquote` on mp_carentan with one client and
  getinfo/getstatus from a socket
  (`crates/server/tests/fixtures/handshake/svinfo-quote.txt`): after
  `setCvar("sv_hostname", "q\"h;s\\b e")` the cvar reads back whole and
  configstring 0, `getinfo` and `getstatus` all carry `qhsb e`; set to
  `"lead` the `d 0` update carries `lead`; set to a lone `"` the key is gone
  from configstring 0 and both replies; `tail"` brings it back as `tail`.
  All three servers sent the same. vcod's does too (`connect.rs`,
  `a_quote_in_a_serverinfo_value_is_stripped_like_retail`).
- The string's limit is 1024 bytes (`> 0x400` at the end of 0x80827d4).
  1.1d's `infoResponse` on mp_carentan holds 123 bytes beside
  `\challenge\`, so an 890-byte challenge (exactly 1024) was answered and
  891 was not: that one crashed the process (exit 139); a 900-byte one
  printed `ERROR: Info string length exceeded` and dropped the level, the
  process left idle with no map. VERIFIED. A query can take a 1.1d server
  down. 1.5b and UO instead print `Info string length exceeded` with the
  key, skip that pair and go on: a 1000-byte `getinfo` came back as
  `\challenge\<1000>\protocol\6` and nothing else, and a 1000-byte
  `getstatus` as the serverinfo with no `challenge` and `\pswrd\0\mod\1`
  still at the end. VERIFIED.
- vcod (`oob_argv` and `Server::info_pairs` in
  `crates/server/src/server.rs`, `Info::set_value_for_key` in
  `crates/common/src/net/connectionless.rs`) reads the line, tokenizes and
  strips the same way, and skips an overflowing pair on 1.5 and UO. On
  1.1 it sends no reply and keeps its level. A loopback A/B of the cases
  above came back byte for byte retail's on all three protocols, except
  1.5b's archived `g_timeoutsAllowed 2` from its homepath config.

### `fs_restrict`

Present in all three servers; the code is Q3's demo restriction.
VERIFIED (strings and calls, cod_lnxded 1.1d unless named).

- `fs_restrict` is `Cvar_Get("fs_restrict", "", CVAR_INIT)` at 0x8061d68,
  so only the command line sets it; `set fs_restrict` afterwards is
  refused as write protected.
- `FS_SetRestrictions` (0x80713e8; 1.5b around 0x8075af8, UO 0x807625a):
  with it non-zero it sets it to `1`, prints `Running in restricted demo
  mode.`, and on 1.1 and 1.5 restarts the filesystem on `demomain`
  (0x80d03c4; 1.5b 0x80da244). UO has no `demomain` string and keeps its
  search path. Every pak on the path must then have checksum 0xb1f595f5
  (1.1 `cmp` at 0x8071461; 1.5b and UO compare the checksum xor 0x2261994
  against 0xb3d38c61), else `Corrupted pak0.pk3: %u` is fatal. VERIFIED.
  `+set fs_restrict 1` on the stock 1.1 install stopped with
  `Sys_Error: Couldn't load default_mp.cfg` (no `demomain/`); with the
  stock paks copied into `demomain/` under other names it stopped with
  `Sys_Error: Corrupted pak0.pk3: 1252304247`, pak6's checksum.
  VERIFIED. Only the CoD demo's own data passes, so no full install runs
  restricted.
- While restricted (or with server paks set) `FS_FOpenFileRead` serves
  only `.cfg`, `.menu`, `.svg`, `.game`, `.dm_1` and `.dat` from a
  directory, not from a pak (fs_restrict pointer 0x8339c9c read at
  0x805d6df, the suffixes at 0x80cd875..0x80cd931). VERIFIED (strings and
  calls).
- `SVC_Status` and `SVC_GameCompleteStatus` (0x808bda3, 0x808bff3; 1.5b
  0x80924c5, UO 0x809336d) test `Cvar_VariableValue("fs_restrict")` right
  after the `challenge` key and set `sv_keywords` to `demo
  <sv_keywords>` (`demo %s` at 0x80d59ed) through `Info_SetValueForKey`,
  so the key moves to the end, before `pswrd`. `SVC_Info` has no such
  test. VERIFIED (calls). Not seen on the wire: no restricted server
  starts here.
- `SV_AuthorizeIpPacket` (0x808514c) answers an authorize server's `demo`
  verdict with `challengeResponse` on a restricted server and
  `error\nEXE_ERR_NOT_A_DEMO_SERVER` otherwise. INFERRED (decompile).
- vcod-server follows the startup rule (`main.rs`): a non-zero
  `--set fs_restrict` searches `demomain` for `main` (UO's `uo` stays),
  and stops with retail's `Corrupted pak0.pk3: %u` on the first pak that is
  not the demo's or `Couldn't load default_mp.cfg` without one; on the
  stock 1.1 install it printed the same `1252304247`. The status builders
  add the `demo` keyword. vcod authorizes nobody, so the `ipAuthorize`
  branch has no counterpart, and the directory file filter is not ported:
  no restricted vcod-server gets past its paks.

### Private slots (`SV_DirectConnect` 0x8085498)

Measured 2026-10-09 against `cod_lnxded` (1.1d) on a spare port with
`sv_maxclients 8`, `+set sv_privateClients 2 +set sv_privatePassword pp`,
a throwaway example on `NetClient` joining clients with chosen `password`
userinfo values one after another, and `rcon status` for the slots.

- `sv_privateClients` is registered default `"0"` with flags 4
  (`CVAR_SERVERINFO`), `sv_privatePassword` default `""` with flags 0x100
  (`Cvar_Get` 0x806ea34 called with the names pushed at 0x808a9cf and
  0x808ab7d, in `SV_Init`). VERIFIED. Both are live: an rcon
  change governed the next connect and the next `getinfo`. VERIFIED by
  capture.
- The reconnect search runs first and is unchanged: a connect from an
  address and qport (or port) that already holds a slot, live or zombie,
  takes that slot back whatever its password. VERIFIED by capture: a
  client in private slot 0 that reconnected from the same qport without a
  password got slot 0 again.
- A new client: `Info_ValueForKey(userinfo, "password")` (key at
  0x80d44e3) is compared with `sv_privatePassword` by `strcmp`
  (0x8085a26). Equal starts the free-slot search at 0, otherwise at
  `sv_privateClients->integer` (0x8085a36). The search takes the first slot
  whose state is 0, so a zombie is not free. None found sends
  `error\nEXE_SERVERISFULL` (0x80d44ec) and prints `Rejected a
  connection.` (0x80d4503) at developer level. INFERRED. There is no Q3
  local-address fallback for bots. VERIFIED by capture: with the two
  private slots free, six passwordless clients took slots 2..7 and a
  seventh got `error\nEXE_SERVERISFULL`; a client with `pp` then got slot 0,
  the next slot 1, the third `EXE_SERVERISFULL`. A `password wrong` client
  searches like a passwordless one.
- An empty `sv_privatePassword` equals the empty value a client without a
  `password` key reads, so such a client searches from 0. VERIFIED by
  capture: after rcon `sv_privatePassword ""`, a passwordless client got
  slot 1 (slot 0 held) and a `password wrong` one slot 2. Private slots
  therefore bite only with a non-empty private password.
- `password` is the same key `g_password` reads, so a private client on a
  passworded server needs the two to be equal. INFERRED.
- `SVC_Info` (0x808c1ac) counts `clients` over the slots from
  `sv_privateClients` up with state above 1, and sends `sv_maxclients` as
  `sv_maxclients - sv_privateClients`, unclamped. VERIFIED by capture:
  `clients 6 sv_maxclients 6` with six public and two private clients in;
  `sv_privateClients 12` read `sv_maxclients -4`. `getstatus` lists every
  player and carries `sv_privateClients` in its serverinfo. VERIFIED by
  capture.
- `SV_AddTestClient` searches from slot 0 (0x8087548), so test clients
  ignore the private slots. INFERRED. vcod's bots do the same.
- vcod: `svc_direct_connect` keeps its reconnect and zombie arms and
  searches a new client's slot from `Server::private_clients` unless the
  `password` matches; `svc_info` and the serverinfo string follow the
  cvars live. The same scenario on vcod-server gave the same slots, the
  same two refusals and the same `getinfo`.

### The serverinfo string follows every write

- `SV_Frame` tests `cvar_modifiedFlags & CVAR_SERVERINFO` once a frame
  and rewrites configstring 0 with `Cvar_InfoString(CVAR_SERVERINFO)`
  (map-cycle doc 4.3). The test reads the flag, not the writer, so a
  script `setCvar` of `sv_privateClients` or `sv_hostname` reaches
  configstring 0 and `getstatus` the same frame a console `set` would.
  INFERRED (Q3 lineage and the flag; the console half is VERIFIED by the
  capture above).
- `Cvar_InfoString` walks every cvar with flag 4 and leaves out the empty
  ones, so `sv_keywords` and `sv_mapname` (both flag 4, both empty on a
  stock server) never show. VERIFIED: the registry rows
  (`crates/server/src/cvars/registry.rs`) with `S` are exactly the stock
  capture's keys plus those two.
- `SVC_Info` reads `sv_hostname->string` for `hostname`, not a copy taken
  at load. INFERRED (Q3 lineage).
- vcod: `Server::serverinfo` builds the string from the running level's
  cvar table (every `S` cvar with a value, case-folded order, `sv_pure`
  from the pak lists), and `Server::flush_serverinfo` compares it with
  configstring 0 once a tick, after the script ran. `getinfo` reads the
  live `sv_hostname`.

### Ping limits (`SV_DirectConnect` 0x8085498)

- The challenge record is 0x2c bytes at `0x83b67f8`: address (+0),
  challenge (+0x14), `time` (+0x18), `pingTime` (+0x1c), `firstTime`
  (+0x20), the measured ping (+0x24) and `connected` (+0x28). VERIFIED
  (the writes at `0x8084e65`..`0x8084e9c`).
- `pingTime` is stamped when the `challengeResponse` goes out: at once for
  a LAN client under `net_lanauthorize 0` (`0x8084edc`), else when the
  authorize server's answer arrives (`0x8085208`). VERIFIED (code).
- At the connect, once the challenge matched: when the stored ping is 0 it
  becomes `svs.time - pingTime` (`0x8085795`), the server prints `Client %i
  connecting with %i challenge ping` (`0x80d43e0`) and marks the challenge
  connected. VERIFIED (code).
- Off the LAN only (`Sys_IsLANAddress` called at `0x80857ee`): with `sv_minPing`
  non-zero and the ping below it (as floats, `->value`), the client gets
  `error\nEXE_ERR_HIGH_PING_ONLY` and the console `Client %i rejected on a
  too low ping`; with `sv_maxPing` non-zero and the ping above it,
  `error\nEXE_ERR_LOW_PING_ONLY` and `Client %i rejected on a too high
  ping: %i`. The keys name what the server accepts, not what the client
  had. VERIFIED (strings `0x80d440d`, `0x80d4466`, `0x80d4440`,
  `0x80d44a0`; the comparisons at `0x808581b` and `0x8085880`).
- A connect that takes a slot clears the stored ping (`0x8085c16`), so a
  later connect on the same challenge measures again. VERIFIED (code).
- `SVC_Info` adds `minPing` and `maxPing` (`%i` of `->integer`) after
  `pure`, each only when non-zero, then `game` (`fs_game`, when set),
  `sv_allowAnonymous` and `pswrd` (the key pushes at `0x808c318` and
  `0x808c344`). VERIFIED (code).
- vcod: `svc_direct_connect` keeps the first connect's ping on the
  challenge and runs both tests off the LAN; `svc_info` writes the two
  keys, and `game` when `--set fs_game` named a mod (matched against a
  retail capture in `cod11-front-end.md` section 18, "vcod"). vcod has
  no authorize detour, so its ping is the plain `challengeResponse` to
  `connect` round trip.

### The challenge table (1.1d `SV_GetChallenge` 0x8084d90, `SV_DirectConnect` 0x8085498, `SV_DropClient` 0x8085cf4)

Read 2026-10-23 off the decompiles and the 1.1d disassembly; 1.5b, UO
1.51b and CoD2 1.3 beside it. The four keep Q3's table unchanged but for
the record size.

| | 1.1d | 1.5b | UO 1.51b | CoD2 1.3 |
|---|---|---|---|---|
| table, record | 0x83b67f8, 0x2c | 0x83ccde0, 0x54 | 0x84f7060, 0x54 | 0x84230dc, 0x74 |
| `SV_GetChallenge` | 0x8084d90 | 0x80889d0 | 0x80897d4 | 0x808d18e |
| `SV_DirectConnect` | 0x8085498 | 0x8089e7e | 0x808ac82 | 0x808e2aa |
| `SV_DropClient` | 0x8085cf4 | 0x808ac11 | 0x808ba15 | 0x808f02e |
| address compare | 0x8080dec | 0x808400f | 0x8084af7 | 0x806c54c |
| slot times | `svs.time` 0x83b67a4 | 0x83ccd88 | 0x84f7008 | `svs.time` 0x8423084 |
| challenge's xor | 0x83b67a4 | 0x83ccd84 | 0x84f7004 | 0x8423084 |

VERIFIED (each address read in its decompile: the 0x400 loop bound, the
stride, the writes).

- 1024 slots in every one (loop bound 0x400). VERIFIED. No slot expires:
  1.1d names the table's base only in `SV_GetChallenge`, the authorize
  reply (0x808514c), `SV_DirectConnect` and `SV_DropClient`, and none of
  them reads a slot's age but the authorize wait (`svs.time - firstTime
  < 5001`, which decides between asking the authorize server again and
  answering). VERIFIED (1.1d, every reference to 0x83b67f8). The other
  three have the same four users. INFERRED (their decompiles read for the
  three functions above only).
- `getchallenge`: the first slot with `connected` 0 and the same address
  is used again; on the way the loop keeps the slot with the lowest
  `time` (+0x18), the first of a tie, connected slots included. With no
  match that slot is overwritten: address, challenge `rand() << 16 ^
  rand() ^` the xor clock above, `time` and `firstTime` (+0x20) the slot
  clock, the stored ping (+0x24) and `connected` (+0x28) 0. A slot asked
  for again keeps its `time`, so the slot overwritten is the oldest
  written, not the longest unasked. VERIFIED (0x8084d90's loop and
  writes; the same writes at 0x80889d0, 0x80897d4, 0x808d18e).
- The answer-at-once path (`net_lanauthorize` 0 and a LAN address, or
  the authorize server timed out) stamps `pingTime` (+0x1c) on the slot
  clock and sends `challengeResponse`. VERIFIED.
- The address compare is `NET_CompareAdr` in all three places: the
  type, then for `NA_IP` the four bytes and the port (1.1d 0x8080dec
  compares the port at its end; 1.5b, UO and CoD2 go through
  `NET_CompareAdrSigned`, port first). The base compare (1.1d 0x8080e60,
  `NET_CompareBaseAdr`, string `NET_CompareBaseAdr: bad address type`)
  is not called here. VERIFIED (1.1d: `call 0x8080dec` at 0x8085702 in
  the connect's loop; the other three by name or string).
- `connect`: for an address that is not loopback (`NET_IsLocalAddress`,
  1.5b 0x808403c), the first slot, connected or not, whose address and
  challenge both match; none answers `error\nEXE_BAD_CHALLENGE`. Then
  the stored ping, if 0, becomes `slot clock - pingTime`, and
  `connected` is set. A connect that takes a slot clears the stored ping
  again (1.1d 0x8085c16, 1.5b and UO at their `connectResponse`, CoD2 at
  its). VERIFIED.
- `SV_DropClient`: the first slot with the client's address, connected
  or not, gets `connected` 0, and the loop stops there. VERIFIED. It is
  skipped when the word at `cl+0x10a40` (1.1d, 1.5b; CoD2 `cl+0x20c44`)
  is set, which vcod reads as the bot flag. INFERRED.
- 1.5b and UO time their slots on the unscaled clock (0x83ccd88, the
  snapshot pacing's, `cod15-protocol-diff.md` 12) and xor the challenge
  with the scaled one (0x83ccd84). VERIFIED (operands).
- vcod: `engine::challenge::Challenges` is the table for both servers,
  timed on `svs_time_ms()` (CoD1) and `svs.time` (CoD2); the draw is
  CoD1's `(rand() << 16) ^ rand() ^ sv_time_ms` (the game clock, 1.5's
  0x83ccd84), CoD2's the same on `svs.time`. Until 2026-10-23 CoD1's
  dropped a slot 60 s after it was last asked for, refreshed `time` on
  every ask, matched a connect on the IP alone, released on drop by IP
  and challenge, and measured the ping on `Instant`; CoD2's grew a list
  that `swap_remove`d the oldest. A connect from a port other than the
  one the challenge went to is now refused, as on retail.

### Message pacing (`SV_SendClientMessages` 0x809045c)

- Each frame, a client whose `nextSnapshotTime` has come gets either the
  next fragment of an unsent message or a new message from
  `SV_SendClientSnapshot` (0x808f844). VERIFIED (code).
- `SV_SendClientSnapshot` writes the server commands and the snapshot only
  for an active client or a zombie; then, for any client but a zombie,
  `SV_WriteDownloadToClient`; then `svc_EOF`. A downloading client, which
  is not active yet, gets messages holding the reliable acknowledge and its
  download blocks and nothing else. VERIFIED (code, the state tests at
  `0x808f855`, `0x808f8a1` and `0x808f92a`).
- After the send, `SV_SendMessageToClient` (0x808f680) sets
  `nextSnapshotTime`: for a loopback or LAN client `svs.time - 1`, so the
  next frame; otherwise `svs.time + max(snapshotMsec, (min(bytes, 1500) +
  48) * 1000 / rate)` with `rate` capped by a non-zero `sv_maxRate` (raised
  to 1000 if below), and at least a second for a client that is neither
  active nor downloading. VERIFIED (code).
- A fragmented message's later fragments go one per turn, each setting
  `nextSnapshotTime` to `svs.time + (min(bytes left, 1500) + 48) * 1000 /
  rate` with no LAN shortcut (`0x80904d9`..`0x8090546`). VERIFIED (code). Live: a
  loopback probe downloading from 1.1d got its blocks three to a message
  (the LAN rate, 99999, over 50 ms) and a message every 200-300 ms, one
  fragment per frame. VERIFIED (capture, 2026-10-09, on a loaded host).
- Live off the LAN, 2026-10-11: 1.1d on `mp_carentan` dm in one network
  namespace (203.0.113.1), a spectating `vcod --net-probe` in another
  (198.51.100.2) behind a logging relay, both joined by a veth pair, with
  `--probe-rate` / `--probe-snaps` setting the userinfo. VERIFIED (capture):
  - `rate 5000`, `snaps 20`: 19-21-byte snapshots 50 ms apart (the
    `snaps` floor; the rate's 14 ms is under it).
  - `rate 25000`, `snaps 10`: 100 ms apart, the `snaps` floor.
  - `rate 1000`, `snaps 30`: 20-byte snapshots 100 ms apart, the rate's
    68 ms rounded up to the next frame; the first snapshot (304 bytes) was
    followed 350 ms later, its own `(304 + 48) * 1000 / 1000`.
  - The first snapshot after the gamestate, 2026-10-12, spectator on
    `mp_carentan` dm: 293-304 bytes on retail, 566 on vcod until the change
    below, 293-294 after. Decoded, retail's (290 bytes) is five server
    commands, the playerstate from null (720 bits), twelve entities each
    1-2 bytes against its baseline and the two turrets 8-9 bytes each (their
    barrels moved since the baseline, `angles2[0]` -20 to -63 and -72), and
    one client. vcod sent no baselines, so each entity went out against
    null at 12-34 bytes. VERIFIED (capture). The fix is the map-cycle doc's
    3.3: vcod now baselines what it sends, after the settle frames, and
    its first snapshot decodes to the same 2448 bits as retail's.
  - The gamestate's fragments under each are in the next section.
- vcod: `Client::pace`, `Client::first_packet` and `Client::next_fragment`
  follow these rules for every message: the snapshot and download messages
  built in the frame loop and the gamestate (next section).

### Clients not in the world: the bare message and the 1 s keepalive

- `SV_SendClientMessages` (0x809045c) walks every slot whose state is not
  `CS_FREE` (`cmpl $0x0,(%ebx)` at `0x80904a0`), so a
  `CS_CONNECTED` or `CS_PRIMED` client gets a message on each of its turns.
  VERIFIED (code).
- For such a client `SV_SendClientSnapshot` (0x808f844) writes neither
  server commands nor a snapshot (state tests at `0x808f855` and `0x808f8a1`
  pass only 4 and 1), then the download, then `svc_EOF`. With no download
  the message is the 4-byte sequence, the 4-byte reliable acknowledge and
  one compressed `svc_EOF` byte: 9 bytes. VERIFIED (code).
- `SV_SendMessageToClient` (0x808f680) floors a client that is not
  `CS_ACTIVE` and not downloading (byte `+0x10a64`) at `svs.time + 1000`
  (`0x808f806..0x808f824`). A LAN client takes the `svs.time - 1` branch
  first and gets a message every frame. VERIFIED (asm).
- Every switch to `CS_ACTIVE` sets `nextSnapshotTime` to `svs.time`:
  `SV_ClientEnterWorld` (0x80877d8, store at `0x8087838`), the entering cmd
  in `SV_UserMove` (0x8086fa4, the store decompiled at its `CS_PRIMED`
  branch), the restart path of `SV_ExecuteClientMessage` (0x80872ec) and
  the bot add (0x808753c). `SV_DirectConnect` (0x8085498) does the same at
  connect. VERIFIED (code). So the first snapshot goes on the next frame,
  not after the keepalive interval.
- Live, 2026-10-10, 1.1d on `mp_carentan` with a logging relay that dropped
  the probe's sequenced packets for 3-3.5 s after `connectResponse` and
  again after the first gamestate fragment, so the client sat in
  `CS_CONNECTED` and then `CS_PRIMED`. VERIFIED (capture):
  - Loopback: a 9-byte message every 46-51 ms in both states, 63 of them
    before the gamestate.
  - Off the LAN (client 198.51.100.2, server 203.0.113.1, joined by a veth
    pair across two network namespaces): 9-byte messages 995-1003 ms apart
    while connected; the gamestate's second fragment 976 ms after the
    first, the rest 98-102 ms apart (the rate); one 9-byte message 51 ms
    after the last fragment, then 9-byte messages 1000 ms apart while
    primed. The entering cmd got its first snapshot 15 ms later.
- vcod: `Server::send_unentered` sends every connected or primed client its
  fragment, its download blocks or the bare message on its turn, paced by
  `Client::pace`; `enter_world` makes the client due at once. Against
  vcod-server the same two captures matched: 9-byte messages each frame on
  loopback; off the LAN 1000 ms apart in both states, the second gamestate
  fragment 926 ms after the first (vcod answers the packet at the next tick
  start, retail between frames; both add 1000 to the last frame's time),
  and the first snapshot one tick after the entering cmd. VERIFIED
  (capture, same relay). Before this change vcod sent a client neither,
  and off the LAN the first snapshot waited out the primed interval.
- Live, 2026-10-18, `--probe-edge watch` joining 1.1d on loopback:
  the gamestate is message 1 and the first
  snapshot message 2, since retail handles the connect and the client's
  first message as they arrive, between frames, and the client answers
  each before the next frame. VERIFIED (capture). vcod-server read
  packets only at a tick's start and sent the tick's output at its end,
  so its `connectResponse` left after the frame had already sent the new
  client a bare message 1, and the gamestate was 2 and the first snapshot
  3. `vcod-server`'s loop now handles a packet that arrives between ticks
  at once and sends what it answers; the same probe against it reads
  gamestate 1, first snapshot 2. VERIFIED (capture, 2026-10-18).
- vcod, 2026-10-19: a packet the loop finds queued at a tick's start (it
  came in while the last tick overran its slot) is answered there, before
  the tick runs, rather than with the tick's output; the between-ticks
  path already sent at once. VERIFIED, `cod_lnxded` decompile: retail's
  `Com_EventLoop` runs every queued packet's handler ahead of `SV_Frame`
  (the read schedule above), and `NET_SendPacket` (0x8080d28) writes the
  socket inside the handler: a loopback address goes to the loopback ring,
  any other but the bot and bad types to `Sys_SendPacket` (0x80c7914,
  `sendto` at 0x80c79ba), which the out-of-band senders 0x8080920 and
  0x8080a8c call the same way.

### The gamestate goes out fragmented (`SV_SendClientGameState` 0x8085eec)

- It first sends every fragment left of an earlier message, in a loop of
  `Netchan_TransmitNextFragment` (0x808dcf8, called at `0x8085f34`) until
  `unsentFragments` clears. They all go in that one call. VERIFIED (code).
- It builds the gamestate in a 16384-byte message buffer (`MSG_Init(...,
  0x4000)` at `0x8085fa7`). The write helpers (`MSG_WriteByte` 0x807f090,
  `MSG_WriteData` 0x807eef0) set the buffer's overflowed flag and drop the
  write when it is full. VERIFIED (code). The function never reads that
  flag before sending. VERIFIED (asm, `0x8085eec..0x8086168`). INFERRED:
  a gamestate past 16384 plain bytes reaches the client cut short, with
  no `svc_EOF`.
- It sends through `SV_SendMessageToClient` (0x808f680) like any message,
  so `nextSnapshotTime` follows "Message pacing" above. The client is
  `CS_PRIMED` by then, so off the LAN, and not downloading, the second
  fragment waits at least a second. VERIFIED (code).
- Off the LAN, the same 2026-10-11 namespace capture as "Message pacing"
  (six fragments, 5 x 1308 + 328 on the wire). VERIFIED (capture):
  - `rate 25000`: the second fragment 975 ms after the first, the rest
    98-104 ms apart (the rate's 62 ms rounded to the next frame).
  - `rate 5000`: 958 ms, then 346-353 ms apart (310 ms rounded up).
  - `rate 1000`: 1507 ms, then 1548-1552 ms apart, the last fragment
    (328 bytes) included. So each fragment's pace is taken off the bytes
    left *before* it went (`min(left, 1500) + 48`), as
    `0x80904d9`..`0x8090546` reads, and the first fragment's off the whole
    message, which at 1548 ms beats the one-second floor.
  - The first snapshot came 52 ms after the last fragment at every rate:
    the entering cmd makes the client due at once (next section).
  - vcod-server under the same three: the same gaps to within 3 ms (its
    gamestate was five fragments, 4 x 1308 + 1237). With the first map's
    baselines in it (2026-10-12) it is six, 5 x 1308 + 250, against
    retail's 5 x 1308 + 652. VERIFIED (capture). Those 400 bytes were
    the configstring 1 pak lists of the retail homepath's extra paks
    (326 bytes) and seven unsent baselines (77). vcod now writes the
    baselines (map-cycle doc 3.3).
- `Netchan_Transmit` (0x8080320) raises a `Com_Error` past 16384 bytes
  (`Netchan_Transmit: length = %i`, `0x80d3000`). Below 1300 bytes
  (`cmp 0x514`) a message goes in one packet. Otherwise it keeps the whole
  message as unsent fragments and sends the first one now. VERIFIED (code).
- Live, 2026-10-09: a loopback `vcod --net-probe` against 1.1d on
  `mp_carentan` got its gamestate as six fragments under one sequence
  (5 x 1300 + 645). The first came on the `connect` reply's heels, and
  the rest came 41, 56, 47, 48 and 51 ms apart, one per server frame.
  Before the gamestate the client got one 9-byte message, the
  `CS_CONNECTED` frame. VERIFIED (capture, a logging UDP relay between
  the probe and the server).
- vcod: `Server::send_gamestate` flushes the old fragments, sends the
  gamestate's first fragment and paces the client. The frame loop sends
  the rest one per turn. Against vcod-server, the same probe got the
  6718-byte gamestate as five fragments, one per frame. VERIFIED (capture,
  same relay). vcod logs a warning past 16384 plain bytes (32768 on
  United Offensive, whose buffer is twice 1.1d's, `coduo-diff.md` 7) and
  sends the gamestate whole.

### `snaps` (`SV_UserinfoChanged` 0x8086ab4)

- `snapshotMsec` (client field `0x528a8`) is 50 when the userinfo's
  `snaps` is empty or missing. Otherwise it is `1000 / snaps`, with
  `snaps` read by `strtol` and clamped to 1..30, so `snaps 30` gives 33,
  `0` or junk gives 1000, and anything past 30 gives 33. VERIFIED (asm
  `0x8086cad..0x8086d44`: the `cmp $0x1e` at `0x8086d20`, the 50 at
  `0x8086d44`).
- Two readers. `SV_SendMessageToClient` uses it to floor an off-LAN
  client's interval (`0x808f7d8`). `SV_WriteDownloadToClient` uses it to
  size a message at `rate * snapshotMsec / 1000 / 2048 + 1` blocks
  (`imul` at `0x80865d9`). A LAN client's interval ignores it. VERIFIED
  (code).
- Live, 2026-10-09: a loopback probe downloading `bonneville.pk3` from
  1.1d (LAN rate 99999) got download messages of 11394-11496 bytes with
  `snaps 10`, which is five blocks, and of 4541-4572 bytes with `snaps
  30`, which is two. vcod-server sent the same message sizes byte for
  byte under both values. VERIFIED (capture, relay, a scratch build of
  the probe that took `snaps` from the environment).
- vcod: `Client::snapshot_msec` reads it the same way. The pace and the
  download block count use it.

### `g_password` (`ClientConnect`, game.mp.i386.so 0x4246c)

Measured 2026-10-09 against `cod_lnxded` (1.1d) on a spare port with
`+set g_password secret`, a throwaway connectionless client (getinfo,
getstatus, getchallenge, a Huffman `connect` with a chosen userinfo) and
`vcod --net-probe` for the level-boundary drops.

- The check lives in the game module, not the engine. `ClientConnect`
  reads the userinfo's `ip` (key at 0x730a4) and skips the test when it is
  `localhost` (0x730a7). Otherwise it reads `password` (0x730cf) and denies
  when `g_password` is non-empty, is not `none` (0x730d8, `Q_stricmp`, so
  any case) and differs from the userinfo value by `strcmp`. The denial
  frees the entity and returns `GAME_INVALIDPASSWORD` (0x730dd). INFERRED
  from 0x425da..0x42666 (calls resolved with `tools/re/annotate_func.py`).
  There is no bot or first-time parameter, unlike Q3's.
- `SV_DirectConnect` puts `ip localhost` into the userinfo only when
  `NET_IsLocalAddress` (0x8080ed8: address type 0 or 2, bot or loopback)
  holds, else the address string (0x80858d3 and 0x808576c). A test client
  (`SV_AddTestClient`, zeroed address at 0x80875cf) is therefore exempt;
  a UDP client on 127.0.0.1 is not. INFERRED. VERIFIED by capture: a
  connect from 127.0.0.1 without a password, and one carrying its own
  `\ip\localhost`, were both refused.
- A refused connect gets the OOB `error\nGAME_INVALIDPASSWORD` (format
  `error\n%s` at 0x80d42c5, sent at 0x8085bd2) and the slot is freed; the
  server prints `Game rejected a connection: %s.` at developer level. The
  game verdict comes after the engine's own checks (challenge, ping,
  `sv_privatePassword`, `EXE_SERVERISFULL`). INFERRED from 0x8085baa..
  0x8085bf2. VERIFIED by capture: no `password` key, `wrong` and `Secret`
  each got exactly `error\nGAME_INVALIDPASSWORD`; `secret` got
  `connectResponse`.
- `g_password` is registered with flags 0 (`dump_cvars.py`), so it is live:
  an rcon `g_password` change showed in the next `getinfo` and governed the
  next connect with no map load. VERIFIED by capture.
- `getinfo` and `getstatus` send `pswrd 1` when `Cvar_VariableString
  ("g_password")` is non-empty, `0` otherwise (0x808c3ae, 0x808bf23).
  VERIFIED by capture: `secret` and `none` both read `pswrd 1`, `""` read
  `pswrd 0`. `none` reads as passworded in the browser yet admits a connect
  with no password. VERIFIED by capture.
- `SV_MapRestart_f` (0x8083f8f) and `SV_SpawnServer` (0x808a765) run
  `ClientConnect` again for every client on the server, and a denial goes
  through `SV_DropClient` with the same string. VERIFIED by capture: a
  probe joined with `g_password ""`, rcon set `secret`, and both
  `map_restart` and `map mp_harbor` dropped it with `w
  "GAME_INVALIDPASSWORD"`; the restart also printed `SV_MapRestart_f:
  dropped client 3 - denied!` per client.
- vcod (`Server::password_denied` in `crates/server/src/server.rs`) reads
  `g_password` from the level's cvar table and runs the same test in
  `svc_direct_connect` after the full-server check, and at both level
  boundaries before the script's reconnect. vcod's bots are exempt, as
  retail's test clients are.

## Pak checksums and pure servers

Measured 2026-10-09 against `cod_lnxded` (1.1d) on a spare port with
`fs_basepath` on a stock 1.1 install (`pak0`-`pak6`, `bonneville`, two
localized paks) and a scratch homepath, `vcod --net-probe` as the client.
Code facts come from the decompiles; each claim carries its own label.
vcod's port: `crates/common/src/pak_checksum.rs`, `Pk3Fs::paks` and
`search_paks` in `crates/common/src/pk3.rs`, `PakLists` in
`crates/server/src/configstrings.rs`, `PureCheck` in
`crates/server/src/server.rs`.

### The checksum

- `FS_LoadZipFile` (CoDMP.exe `0x42ac20`, called from `FS_AddGameDirectory`
  `0x42bd30`) walks the central directory in order and keeps the CRC-32 of
  every entry whose uncompressed size is not 0 (`unz_file_info+0x1c`); no
  names are filtered. VERIFIED.
- `pack+0x304` = `Com_BlockChecksum(crcs)` (`0x4445b0`) and `pack+0x308` =
  `Com_BlockChecksumKey(crcs, fs_checksumFeed)` (`0x444640`, the 4 key bytes
  fed ahead of the CRCs). Both are MD4 (`67452301`.. init, `0x5a827999` and
  `0x6ed9eba1` round constants) folded as `d0 ^ d1 ^ d2 ^ d3`, printed
  `%i`. lnxded has the same MD4. VERIFIED.
- The rule reproduces retail's numbers: `pak1`-`pak5` of the 1.1 install
  give `1265884747 616334813 918160098 -1825805837 77111478`, `pak6`
  `1252304247`, the same as both captures'. VERIFIED. The 1.1 install's
  `pak0` reads `-363972028` and the 1.5 install's `1048127331`: the two
  `pak0.pk3` files differ (md5 and size). VERIFIED.
- `pak6.pk3` holds `ui_mp_x86.dll`, `cgame_mp_x86.dll` and
  `game_mp_x86.dll` and nothing else. VERIFIED.

### The four cvars (`SV_SpawnServer`, map-cycle doc section 3 step 23)

| cvar | built by | entries | with `sv_pure 0` |
|---|---|---|---|
| `sv_paks` | `FS_LoadedPakChecksums` `0x8071600` | non-localized paks, `%i ` each | `""` |
| `sv_pakNames` | `FS_LoadedPakNames` `0x8071580` | same paks, bare name, space-joined | `""` |
| `sv_referencedPaks` | `FS_ReferencedPakChecksums` `0x80717a4` | every pak, `%i ` each | same |
| `sv_referencedPakNames` | `FS_ReferencedPakNames` `0x80716cc` | every pak, `<game>/<name>` | same |

- The table is VERIFIED from the code and from the capture below. An empty
  value leaves its key out of the systeminfo. VERIFIED (the `sv_pure 0`
  capture has no `sv_paks`).
- "Referenced" is every pak: Q3's `pack->referenced ||
  Q_stricmpn(gamename, BASEGAME)` test reads the address of CoD's
  four-byte `referenced[]` array (`cmp $0xfffffcf0` at `0x80717c7`), which is
  never null. VERIFIED for the code; that it is a bug is INFERRED. A map
  download nobody uses still lands in every client's download list.
- 1.5b and UO fixed it, so their referenced pair is a real subset:
  - 1.5b (`FS_ReferencedPakChecksums` 0x8075cf2, names 0x8075d9e): a pak
    whose `referenced` byte (`pack+0x310`) is set, or whose directory fails
    `Q_stricmpn(dir, "main", 4)`. VERIFIED (disassembly).
  - UO (0x807647e, names 0x8076564): set, or a directory that fails both
    `Q_stricmpn(dir, "main", 4)` and `Q_stricmpn(dir, fs_basegame,
    strlen(fs_basegame))`; `fs_basegame` defaults to `uo` (0x80dd585).
    VERIFIED (decompile). So a stock `uo/` or `main/` pak is listed only when
    a file was read from it, and a mod directory's paks always are.
  - The byte is set by `FS_FOpenFileRead` (1.1 0x805cc18, 1.5b 0x8061c40, UO
    0x8061a74) on a read out of the pak unless the name ends `.shader`,
    `.txt`, `.cfg`, `.config`, `.bot`, `.arena` or `.menu` or contains
    `levelshots`; UO adds `soundaliases`, 1.5b does not (its
    `soundaliases` string 0x80d8c37 is read only at 0x806ded5 and
    0x806f697, elsewhere). VERIFIED (decompile and disassembly). An
    existence check (no handle) sets nothing.
  - `FS_ClearPakReferences(0)` (UO 0x8065a4d) clears the bytes at the top of
    `SV_SpawnServer` and in `FS_Restart` (0x8065bbb); step 23 reads them
    after the game's init and the three settle frames, and `map_restart`
    never rebuilds the four cvars. VERIFIED (decompile).
  - Live with `fs_debug 1` (which prints `FS_FOpenFileRead: <file> (found in
    '<pak>')` per read), retail UO on mp_cassino read from exactly
    `pakuo07 pakuo04 pakuo03 pakuo01 pakuo00 pak9 pak8 pak5 pak4 pak0`, the
    capture's list; 1.5b on mp_carentan from `pak9 pak8 pak5 pak4 pak1
    pak0`, `pak1` for `soundaliases/*.csv` alone and `pak9` for
    `iw_sound2.csv` among others. VERIFIED.
- vcod's `Pk3Fs` keeps the same byte per pak (`flags_reference`, the 1.1
  list), vcod-server clears it before the map's BSP is read and builds the
  pair after the level's script load (`Server::level_paks`). vcod reads
  some files later or never, so on 1.5 and UO it makes the reads that flag
  a pak at that point: each indexed model's descriptor and LOD 0
  `xmodelparts` (retail's `G_ModelIndex` precache), and on 1.5 every
  `soundaliases/*.csv`; on UO the map's sound load spec; and on both each
  `#using_animtree`'s tree. The UO gates on mp_cassino (dm, ctf, dom, hq),
  mp_foy bas and mp_j5outpost and the 1.5 gate on mp_carentan match
  retail's referenced pair exactly, and
  `a_pak_holding_the_load_spec_or_an_animtree_is_referenced` lists a pak
  holding either file. VERIFIED (those gates).
- The load's reads after `FS_ClearPakReferences`, diffed file by file
  between retail UO's `fs_debug 1` log and vcod's `read <file> from <pak>`
  trace (2026-10-17, mp_cassino dm and mp_foy bas):
  - Every weapon file, weapon world and pickup model and vehicle model
    retail reads, vcod reads too. VERIFIED (live). Retail's flagged reads vcod lacked were
    `soundloadspecs/mp/<map>.csv` and `animtrees/flak88.atr` (with
    `xanim/void` from `pak0`, flagged anyway). `configure_mp.csv` is read
    before the spawn's clear, so it flags nothing. VERIFIED (live).
  - UO's spawn calls `0x806e35a(va("maps/mp/%s.bsp"), 2)` after
    `sv_serverid` (decompile, coduo_lnxded). With 1 or 2 it opens
    `soundloadspecs/mp/<map>.csv`, falling back to
    `soundloadspecs/mp/default.csv` when the map has none (`0x806e15e`),
    and reads the `soundaliases/` files the spec lists; with any other
    value it lists `soundaliases/*.csv` as 1.5 does. VERIFIED (decompile).
    1.5b and 1.1d hold no `soundloadspecs` string. VERIFIED (strings).
  - The animtree read follows the script compile: `Can't find
    animtrees/turret88.atr`, then `animtrees/flak88.atr`, then
    `animtrees/multiplayer.atr`, after `mp/playeranim.script` on mp_foy.
    VERIFIED (log). Each name is a `#using_animtree` of a loaded script
    (`_flak_gmi.gsc`'s `flak88`). INFERRED. All three servers hold
    `animtrees/%s.atr` (1.1d 0x8e904, 1.5b 0x98e04, UO 0x9ed44 file
    offsets). VERIFIED (strings). vcod touches the tree on 1.5 too; that
    1.5b reads it at the same point is INFERRED.
  - A scratch UO install with `uo/zzz_spec.pk3` (only
    `soundloadspecs/mp/mp_foy.csv`) and `uo/zzz_tree.pk3` (only
    `animtrees/flak88.atr`): retail on mp_foy bas lists `uo/zzz_tree
    uo/zzz_spec uo/pakuo07 uo/pakuo04 uo/pakuo03 uo/pakuo01 uo/pakuo00
    main/pak9 main/pak8 main/pak5 main/pak4 main/pak0`, and vcod-server,
    probed through `--net-probe`, the same string. VERIFIED (live).
- `sv_pure` defaults to `1` (`0x80d542c`). VERIFIED. With it on and no pak
  loaded the server prints `WARNING: sv_pure set but no PK3 files loaded`
  (`0x80d5540`). VERIFIED.
- Order is the search path's, highest priority first: the later game
  directory first, each directory's paks in reverse sorted order, and every
  directory's `localized_*` paks after all the others. `FS_AddGameDirectory`
  sorts with `mp_` read as `zz` and `localized_` as a space. VERIFIED by
  both captures (the homepath's `pak5..pak0` ahead of the basepath's
  `pak6 pak5..pak0 bonneville`, then the homepath's localized paks, then the
  basepath's).
- Capture (`sv_pure 1` before the map loads, stock 1.1 basepath), the pak
  part of configstring 1. VERIFIED:

```
\sv_pakNames\pak6 pak5 pak4 pak3 pak2 pak1 pak0 bonneville\sv_paks\1252304247 77111478 -1825805837 918160098 616334813 1265884747 -363972028 2000562497 \sv_pure\1\sv_referencedPakNames\main/pak6 main/pak5 main/pak4 main/pak3 main/pak2 main/pak1 main/pak0 main/bonneville main/localized_english_pak1 main/localized_english_pak0\sv_referencedPaks\1252304247 77111478 -1825805837 918160098 616334813 1265884747 -363972028 2000562497 -961133319 -1187400494 \sv_serverid\16\timescale\1
```

- A `+set sv_pure 1` placed after `+map` (as `tools/run_server.sh`'s extra
  arguments land) reads `sv_pure 1` in the systeminfo with no `sv_paks`: the
  lists were built under the 0 the map loaded with. VERIFIED. Such a server
  still drops a client without `cp`, since the drop reads the live cvar.

### The client side

- `CL_SystemInfoChanged` (`0x415eb0`) also forces every systeminfo key
  into a client cvar, whatever its flags, while no demo plays and no local
  server runs. VERIFIED (cod11-front-end.md 16, "The systeminfo sets
  client cvars", with the 1.5 and UO addresses).
- `CL_SystemInfoChanged` (`0x415eb0`) hands `sv_paks`/`sv_pakNames` to
  `FS_PureServerSetLoadedPaks` (`0x43c290`) and the referenced pair to
  `FS_ServerSetReferencedPaks` (`0x43c530`); both error `pak sum/name
  mismatch` when the counts differ. VERIFIED. A non-empty list prints
  `Connected to a pure server.` VERIFIED.
- `FS_PakIsPure` (`0x428ca0`) passes a pak when the list is empty or holds
  its checksum, names ignored; it gates `FS_FOpenFileRead`, so on a pure
  server any other pak is invisible. VERIFIED. Localized paks are never in
  `sv_paks`; that they stay readable is INFERRED (the client would lose its
  text otherwise).
- `CL_SendPureChecksums` (`0x40fac0`) sends a reliable `cp` (built as `Va `
  plus 13 and 15 on the first two bytes) after `CL_InitCGame` in
  `CL_DownloadsComplete` (`0x40ffb0`) and after a `vid_restart`, pure server
  or not. There is no serverId in it, unlike Q3 1.32. VERIFIED.
- Its body is `FS_ReferencedPakPureChecksums` (`0x43c050`, `"%i "` each):
  the keyed checksum of the first pak flagged for `cgame_mp_x86.dll`
  (`pack+0x312`), of the first flagged for `ui_mp_x86.dll` (`+0x311`), `@`,
  every non-localized pak with the general flag (`+0x310`), then
  `checksumFeed ^ (each general pak) ^ count`. VERIFIED.
- `FS_FOpenFileRead` sets the general flag for any read except `.shader`,
  `.txt`, `.cfg`, `.config`, `.bot`, `.arena`, `.menu` and paths containing
  `levelshots` (`0x429e3e`..). The three DLL flags match obfuscated names
  that decode to `qagame_mp_x86.dll`, `cgame_mp_x86.dll`, `ui_mp_x86.dll`.
  VERIFIED (strings); the decoding is INFERRED. The flags clear at
  `FS_Restart` and `vid_restart` (`0x42d170`). VERIFIED.
- `FS_ComparePaks` (`0x43b830`) skips stock paks (`FS_iwPak` `0x43b6b0`:
  `pak0`-`8`, `mp_pak`, `sp_pak`, `mp_bin`, `localized_*_pak`) and calls a
  referenced pak missing when no loaded pak has its checksum. A file of that
  name already on disk makes the download land as `<name>.%08x.pk3`, the
  checksum in lowercase hex, and adds ` (local file exists with wrong
  checksum)` to the `Need paks:` line. VERIFIED.

### The server's check

- `cp` is `SV_VerifyPaks_f` (`0x808674c`) in the ucmd table at
  `0x80e2f5c`, beside `userinfo`, `disconnect`, `vdr`, `download`,
  `nextdl`, `stopdl`, `donedl`, `retransdl`. VERIFIED. `vdr` (`0x8087b14`)
  resets `pureAuthentic` (`cl+0x528ac`) to 0; nothing else does after the
  connect, so a map change keeps the last verdict until the next `cp`.
  VERIFIED.
- The check, in order: the server's own paks must hold both client DLLs
  (`FS_FileIsInPAK`); more than five tokens; tokens 1 and 2 equal the cgame
  and ui pure checksums and do not start with `@`; token 3 is `@`; no
  duplicate among the general tokens; each is in `FS_LoadedPakPureChecksums`
  (`0x8071664`, the non-localized paks); `count ^ checksumFeed ^ (each)`
  equals the last token. Pass sets `pureAuthentic` 1, anything else 2.
  VERIFIED.
- `SV_ExecuteClientMessage` (`0x80872ec`) drops a client with
  `pureAuthentic` 2 under `sv_pure` with `EXE_UNPURECLIENTDETECTED` once the
  message's commands ran; `SV_UserMove` (`0x8086fa4`) drops one still at 0
  with `EXE_CANNOTVALIDATEPURECLIENT`, after the entering cmd put it in the
  world. VERIFIED (code). Live: a probe that sent no `cp` was dropped with
  `EXE_SERVERDISCONNECTREASON\x14EXE_CANNOTVALIDATEPURECLIENT` right after
  its gamestate, and one sending `cp <pak6> <pak6> @ <three paks> <key>`
  stayed Active. VERIFIED.
- So a pure server whose own paks lack the DLLs (the 1.5 install's server
  directory has no `pak6`) drops every client. INFERRED.

### Serving a download

- The ucmds `download` (`0x8087a64`: closes any transfer, keeps the name up
  to 64 bytes), `nextdl` (`0x8086168`), `stopdl` (`0x8087960`), `donedl`
  (`0x80879fc`: `SV_SendClientGameState`) and `retransdl` (`0x8087a2c`).
  VERIFIED.
- `SV_WriteDownloadToClient` (`0x8086290`) opens the file on the next
  message. The refusals, each `svc_download`, block 0, size -1 and the
  string with the file name after `\x15`: `EXE_CANTAUTODLGAMEPAK` for a
  stock pak (`FS_idPak(name, "main")`), `EXE_AUTODL_SERVERDISABLED` under
  `sv_allowDownload 0` (`EXE_AUTODL_SERVERDISABLED_PURE` with `sv_pure` on),
  `EXE_AUTODL_FILENOTONSERVER` when the file does not open or is empty.
  VERIFIED (strings `0x80d4838`, `0x80d48c2`, `0x80d48a0`, `0x80d4914`).
- The stock-pak test never fires: `FS_idPak` (`0x80709c0`, called at
  `0x80862e8`) gets the name as the client sent it, `.pk3` included, and
  compares it with `%s/pak%d`, which has none. INFERRED from the code; the
  1.5b server streamed a requested `main/pakb.pk3` (cod15-protocol-diff.md
  10, live). vcod serves a stock pak it lists too.
- It reads the file 0x800 bytes per block into an 8-slot window, adds an
  empty block once the file is read, and writes `block`, the size on block
  0, the length and the bytes. Per message it writes the client's rate over
  one snapshot interval in whole 2048-byte units plus one (`sv_maxRate`
  caps the rate). When every block in the window went out it waits; past
  1000 ms with no ack it starts again from the first unacked block.
  VERIFIED (code). So 1.1d's blocks are 2048 bytes; the 8192- and 11800-byte blocks in
  docs/protocol-1.1.md came off public servers. INFERRED.
- `nextdl <n>` with `n` the block due advances the window, or ends the
  transfer when that block is the empty one (`clientDownload: %d : file
  "%s" completed`); any other `n` drops the client with `broken download`.
  `retransdl <n>` with the block due rewinds the send cursor to it.
  VERIFIED (code).
- The file opens through `FS_SV_FOpenFileRead` (`0x806ffb8`): the name as
  sent, joined to `fs_homepath`, then `fs_basepath`, then `fs_cdpath` by
  `FS_BuildOSPath` (`0x8062d70`), and `fopen`ed. Nothing checks the
  extension, the pak lists or `..`. VERIFIED (code). Live, a loopback
  client on 1.1d asked for `main/game.mp.i386.so` and then
  `main/../main/game.mp.i386.so` and got all 581180 bytes of the game
  module both times (285 blocks each; the server logged `begining` for
  both names). VERIFIED (capture, 2026-10-09). So any client can read any
  file the server process can, `server.cfg` and its `rconPassword`
  included.
- The blocks ride the same per-client message as the snapshot ("Message
  pacing" above): a downloading client is not active, so its messages
  carry only the download, at its `nextSnapshotTime`.
- The client side paces its acks: `CL_ReadyToSendPacket` (CoDMP.exe
  `0x40b940`) sends no packet within 50 ms of the last while
  `downloadTempName` is set, and otherwise one per second until the state
  reaches primed. VERIFIED (code).

### vcod

- The client sends `cp` after each map load (the probe after each
  gamestate, touching the map's pak first), reopens its search path under a
  pure server's `sv_paks`, and downloads by checksum with retail's
  `name.%08x.pk3` rule (`net::download::candidates_for_map`).
- The server publishes all four cvars from its mounted paks and verifies
  `cp` the same way. `sv_pure` defaults to 0 (`--set sv_pure=1` turns it
  on) where retail's default is 1. Its systeminfo under `sv_pure 1` on the
  1.1 install matched the capture above byte for byte. VERIFIED. When a
  1.1 systeminfo's lists overflow `MAX_INFO_STRING`, vcod drops paks off
  their ends, the referenced pair first, instead of losing `sv_serverid`;
  1.5 and UO keep `BIG_INFO_STRING` ("Configstring 1, systeminfo").
- The server serves downloads the same way (`crates/server/src/download.rs`),
  limited on purpose to the paks it mounts and lists in
  `sv_referencedPakNames`: those are the only names a retail client ever
  asks for (`FS_ComparePaks` builds its list from them), and retail's open
  path hands out any file under its directories. A downloading client's
  blocks go out in the frame loop's per-client message at its
  `nextSnapshotTime`, alone while it is not in the world and after the
  snapshot once it is, with no server commands until it is active, and a
  fragmented message goes one fragment per turn. Against it the probe
  below fetched the same two paks with matching checksums, `n_degaulle` as
  `n_degaulle.e3668738.pk3` past a same-named file. VERIFIED.
- The client sends a packet every 50 ms while a download runs
  (`NetClient`), and the probe sends no usercmd until its downloads end
  and the next gamestate is in, so it no longer enters the world
  mid-download.
- `--net-probe --probe-download <dir>` takes the download path headless.
  Against the server above with `zzz_zfunmod` and `n_degaulle` added to its
  homepath, and a `zzz_zfunmod.pk3` copy of `pak6` already in the scratch
  dir, it fetched `main/zzz_zfunmod.pk3` into `zzz_zfunmod.1b7cedba.pk3`
  and `main/n_degaulle.pk3` under its own name; both files' checksums
  (`461172154`, `-479819976`) matched `sv_referencedPaks`, and `donedl`
  brought the gamestate back. VERIFIED.

## CD key authorization (the clients)

The document-level default above does not cover this section: every line
here was read off the Windows client binaries (1.1 `CoDMP.exe`, 1.5
`CoDMP.exe`, `CoDUOMP.exe` 1.51, `CoD2MP_s.exe` 1.3, `iw3mp.exe` 1.7) and
carries its own label. The server side (`getIpAuthorize`, the verdicts) is
in "Bans" above and in cod2-protocol.md 1.3.

| | 1.1 | 1.5 | UO 1.51 | CoD2 1.3 | CoD4 1.7 |
|---|---|---|---|---|---|
| `CL_RequestAuthorization` | 0x40f260 | 0x40fe60 | 0x410bb0 | 0x40d130 | 0x469b20 |
| authorize host | `codauthorize.activision.com` | same | `coduoauthorize.activision.com` | `cod2master.activision.com` | `cod4master.activision.com` |
| port (`BigShort` imm.) | 20500 (0x40f2fb) | 20500 (0x40fefb) | 20600 (0x410c41) | 20700 (0x40d1b0) | 20800 (0x469baa) |
| packet | `getKeyAuthorize %i %s` | same | same | `getKeyAuthorize %i %s PB %s` | same as CoD2 |
| key read from | file `main/codkey` | registry | registry | registry | registry |
| `getchallenge` | bare | bare | bare | `getchallenge 0 "%s"` | same as CoD2 |

All VERIFIED: the format strings, host strings and port immediates at those
addresses, and the `getchallenge` strings (1.1 0x567094, 1.5 0x5a66f8, UO
0x594300, CoD2 0x5a38a4, CoD4 0x6d09c4).

- `%i` is `cl_anonymous`'s integer (the cvar named at 1.1 0x56753c, CoD2
  0x5a3c20, CoD4 0x6d05d0's registration), VERIFIED. The `%s` after it is
  the key itself: the first 32 characters of `cl_cdkey` with all but
  `0-9a-zA-Z` dropped (the compare chain at 1.1 0x40f3c0, CoD2 0x40d260),
  VERIFIED. `cl_cdkey` holds the 16 key characters and a NUL, so the
  packet carries those 16, INFERRED (1.5 stores the NUL at 0x5bb8c4, UO at
  0x5c51dc). With `fs_restrict` set the word `demo` goes instead
  (1.1 0x40f379), VERIFIED.
- CoD2's and CoD4's `PB %s` is `CL_BuildMd5StrFromCDKey` (CoD2 0x40cfc0,
  CoD4 0x4699a0), the same string `getchallenge 0 "%s"` carries (calls at
  CoD2 0x40e3d6, CoD4 0x46b29c), VERIFIED. Its parts: the same character filter, a
  white-space trim (CoD2 0x40cf40), a case fold, MD5 from the state words
  `0x6f1cd602 0x226c74be 0xb31c088d 0x555a9639` (CoD2 0x40d053, CoD4
  0x469a37) in place of RFC 1321's with the bit count 0 over the string's
  `strlen`, and the 16 digest bytes printed `%02x` (0x5a3cc8), VERIFIED.
  That they run in that order is INFERRED (call order). CoD2 folds to upper case (`_strupr`, 0x58ce80:
  `a`..`z` less 0x20) and CoD4 to lower (0x688e3b: `A`..`Z` plus 0x20),
  VERIFIED. That the update and final steps are plain MD5 is INFERRED from
  the transform they call (CoD2 0x4bc3b0, which holds the constant
  0xd76aa478) and the 0x38/0x3f padding test in the final (0x4bce22).
- The key is checked first (`CL_CDKeyValidate`: 1.1 0x414260, 1.5
  0x415ab0, UO 0x416f40, CoD2 0x4b3b20, CoD4 0x4712e0): a CRC-16 over the
  16 key characters, each XORed in and shifted right 8 times with
  `0x14002` XORed in before the shift on an odd value (the reflected
  0xa001 polynomial, initial 0), printed `%04x` and compared without case
  to the 4-character checksum, VERIFIED (the `xor eax,0x14002` and the
  `cmp ecx,0x10` loop in all five). A mismatch ends the request with
  `Com_Error(1, "EXE_ERR_INVALID_CD_KEY")` (1.1 0x40f286), INFERRED
  (branch). A
  valid `main/codkey` passes this check, VERIFIED (checked without
  printing the key).
- When: `CL_CheckForResend`'s challenging arm calls it right before every
  `getchallenge` it sends, unless `net_lanauthorize` is 0 and
  `Sys_IsLANAddress` calls the server local (1.1 0x4105b5..0x4105df, 1.5
  0x4112b3, UO 0x4120da, CoD2 0x40e316, CoD4 0x46b1d7 off
  `net_lanauthorize`'s dvar at 0x14e88b0), INFERRED (branch order). It
  sends one datagram and waits for nothing: `getchallenge` follows in the
  same call (1.1 0x4105e4), INFERRED. The name is resolved once and kept
  (1.1 0x40f2a8 tests the cached port), INFERRED. CoD2 also re-sends it
  every 300000 ms while connected (0x40d310: state 8, `realtime -
  authRequestTime > 0x493e0`), INFERRED.
- `needcdkey` from the server (`CL_ConnectionlessPacket`, 1.1 0x410f00,
  CoD2 0x40f047): the connect screen's line becomes `EXE_AWAITINGCDKEYAUTH`
  and the key goes to the authorize server again; the challenge resends go
  on, INFERRED. `keyAuthorize` from the authorize server is matched and
  dropped (CoD2 Mac `CL_ConnectionlessPacket`), INFERRED. The refusals the
  server relays (`error\nEXE_ERR_CDKEY_IN_USE`, `error\nEXE_ERR_BAD_CDKEY`,
  "Bans" above) reach the client as any `error` does.
- Where the key lives. 1.1 reads `codkey` under `fs_basepath`'s `main/`
  (`Com_ReadCDKey` 0x436930, called with `main` at 0x42d0f3): the first
  16 bytes are the key and the next 4 its checksum, VERIFIED (the two
  `FS_Read` lengths, and a real key file). The file's tail is the comment lines
  `Com_WriteCDKey` (0x436bc0) writes after upper-casing both parts
  (`toupper` through the ctype `_LOWER` mask, 0x525f07), VERIFIED.
  `Com_AppendCDKey` (0x436a40, called with `fs_game` at 0x42d122) appends a
  mod directory's `codkey` the same way, VERIFIED. 1.5 reads value `codkey`
  under `HKLM\SOFTWARE\Activision\Call of Duty` (0x438ee0, 0x5a1f58), UO
  value `key` under `...\Call of Duty United Offensive` (0x43b070,
  0x598704, 0x598700), CoD2 `codkey` under `...\Call of Duty 2`
  (0x433237, 0x5a7f60), CoD4 `codkey` under `...\Call of Duty 4`
  (0x4fde77, 0x6e13ac), each a 21-byte string, key then checksum,
  VERIFIED. None of those four reads a file.
- The localized lines (`exe.str`): 1.1 `AWAITINGCDKEYAUTH` "Awaiting CD key
  authorization", `ERR_INVALID_CD_KEY` "CD Key not valid. Please enter CD
  Key in Multiplayer Options/Enter CD-Key menu ...", `ERR_CDKEY_IN_USE`
  "CD Key in use. Please try reconnecting later.", `ERR_BAD_CDKEY` "Bad CD
  Key."; CoD2 says "Key code" where 1.1 says "CD key", VERIFIED
  (`localized_english_pak1.pk3`, `localized_english_iw09.iwd`). CoD4's
  `localized_code_post_gfx_mp.ff` holds the same four names, VERIFIED.

vcod (`vcod_common::net::cdkey`) sends the same packet to the same host,
on its own socket and its own thread, for a server off the LAN, before
each `getchallenge`; `needcdkey` without a key ends the connect with
`EXE_ERR_INVALID_CD_KEY` (retail never sends one without a key), with a
key it sends the key again and a timeout reads `EXE_AWAITINGCDKEYAUTH`.
The key comes from `VCOD_CDKEY`, else on 1.1 and 1.5 from the install's
`main/codkey`; vcod reads no registry. `Com_AppendCDKey`'s mod key is not
read. CoD2's 5-minute re-send is not done.

Live, 2026-10-24, CoD2 1.3. Two public servers off the LAN
answered vcod's bare `getchallenge`, sent without a key, with
`\xff\xff\xff\xffneedcdkey` about 200 ms later, and vcod ended the
connect with `EXE_ERR_INVALID_CD_KEY` on the first one, VERIFIED. Retail
`cod2_lnxded` on mp_toujane inside `unshare -rnm` (the server at
203.0.113.1, an off-LAN client at 198.51.100.2, `/etc/hosts` bound over
to name every authorize host 198.51.100.9, where a stand-in logged each
datagram's command and length and answered `getIpAuthorize` with
`accept`): with the dummy key `AAAA-AAAA-AAAA-AAAA` the off-LAN join sent
one 74-byte `getKeyAuthorize 0 <16> PB <32 hex>` to port 20700 and the
server's `getIpAuthorize` grew from 48 to 84 bytes with the `PB` string
`getchallenge` now carries; without a key no `getKeyAuthorize` went out;
from loopback neither a key nor its absence sent anything to the stand-in
or the server's authorize. All four joins reached the snapshots and the
server logged each connect, VERIFIED.

## Raw captures

Two of them are in the repo now:
`crates/server/tests/fixtures/configstrings/mp_{pavlov,carentan}-dm.txt` are
the whole non-empty table off a retail `dm` server, one `<index> <value>` per
line, and `crates/server/tests/configstrings_ab.rs` diffs our table against
them slot by slot on both maps. Retake them with `tools/run_server.sh <map>`
in one shell and `cargo run -p vcod -- --net-probe 127.0.0.1:28960
--save-configstrings` in another; the fixture header records the settings the
capture depends on (`g_gametype dm`, `sv_maxclients 8`, `sv_pure 0`, stock
`scr_*` defaults) and changing any of them moves slots.

**Configstrings 0 and 1 are provenance, not measurement.** 0 carries the
hostname and `sv_maxclients` the capture ran with, 1 the pak name and
checksum lists, which vary with `sv_pure` and with whatever pk3s the
capturing homepath holds. Both are in `configstrings_ab.rs`'s
`STRUCTURAL_SKIP` for that reason and `crates/server/tests/connect.rs` covers
what the server writes there instead. A re-capture that changes those two
lines is not a regression.

The rest I kept locally; they are not in the repo. Regenerate them with the
commands above. What I captured on 2026-08-25, per map: the three
connectionless replies, the full `RUST_LOG=debug` probe log, the `cs[i] = ...`
lines on their own, and the raw gamestate message from `--save-snapshots`,
which leaves the committed fixture alone.
