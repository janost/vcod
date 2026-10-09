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
argument verbatim. `protocol` is **1**. `gametype` is the short name, not
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

Either way the vcod server must keep systeminfo under 1024 bytes and
`sv_serverid` has to survive the trim, because `SV_ExecuteClientMessage` gates
every client message on it.

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

## Housekeeping: master heartbeat, rcon, zombie slots

Measured 2026-10-07 against `cod_lnxded` (1.1d) with `tools/run_server.sh`
on a spare port, a UDP listener standing in for the master, `vcod
--net-probe` as the client, and `rcon status` polled 0.5-0.6 s apart for the
zombie timing. Each claim carries its own label; the scripts were throwaway.
vcod's port is `crates/server/src/master.rs`, `crates/server/src/rcon.rs`
and the zombie half of `Server::drop_client`.

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
  again on retail with `developer 1` (`SV packet 185.34.107.179:20510 :
  getchallenge`, then `: getstatus`).
- Retail answers that `getchallenge` by resolving
  `codauthorize.activision.com` and logging `sending getIpAuthorize for
  185.34.107.179:20510` (0x8084d90), not with a `challengeResponse`.
  VERIFIED by log. On the machine these captures ran on, `/etc/hosts` points
  `codauthorize.activision.com` at `127.0.1.2`, so that request never left
  the host and no authorize reply came back. VERIFIED. Public DNS resolves it
  to `185.34.107.179`, the master's own address. VERIFIED (`dig @1.1.1.1`).
- Neither server got listed. Retail (`dedicated 2`, one heartbeat, stopped
  with `rcon quit` after about 60 s) was missing from `getservers 1 full
  empty` at 15, 35 and 60 s, among 48 listed servers. vcod's server (one
  heartbeat and its flatline) was missing at 12 and 32 s. VERIFIED
  2026-10-07. With both unlisted there was no behaviour to copy. Whether the
  missing authorize round trip, the address or something else keeps a
  server off the list is still open. UNVERIFIED. The next test is retail
  from a host whose `codauthorize` lookup reaches the real server.
- vcod's binary starts at `dedicated 1`, not retail's 2. That is deliberate:
  dev and agent runs stay off the public list unless they opt in with
  `--set dedicated=2`.

### rcon (`SVC_RemoteCommand`, 0x808c404)

- The password cvar is `rconPassword` (registered at 0x8d699, flags 0x100);
  Q3's `rcon_password` does not exist. Cvar names fold case, so
  `+set rconpassword x` works. VERIFIED.
- The whole packet is tokenized; argv 1 is the password, argv 2 on the
  command. INFERRED from the `Cmd_Argv` calls.
- Rate limit: one request per 500 ms server-wide, whatever the password or
  source. A request arriving within 499 ms of the last answered one is
  dropped with no reply, and a dropped request does not restart the window.
  VERIFIED by capture: a second socket 5 ms after the first got nothing, a
  good password 450 ms after an answered one got nothing, and one 200 ms
  after a dropped one was answered. The check (`cmp eax,0x1f3` at
  0x808c423, skipped while the last time is still 0) runs before the
  password test. INFERRED.
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
  server commands, then a snapshot built from nothing, since
  `SV_BuildClientSnapshot` (0x808f130) skips a zombie and the ring slot keeps
  whatever frame it held. INFERRED from 0x809045c and 0x808f844.
- A zombie's packets still go through the netchan and update its message
  and reliable acks; they are not executed and do not move its last-packet
  time (`cmp [piVar2],1` before the store at 0x808ca44). INFERRED.
- `sv_zombietime` defaults to 2 (0x80d56cf). VERIFIED. The slot is freed
  once its last-packet time is more than `sv_zombietime` seconds old.
  VERIFIED by capture: after `clientkick` (which stamps that time) the row
  read `ZMBI` with 500, 1050 and 1550 ms at 0.51, 1.03 and 1.53 s, and was
  gone at 2.04 s. After a client's own disconnect the last `ZMBI` row read
  1550 ms and the next poll, 510 ms later, found the slot free.
- A timeout drop frees the slot at once (`*piVar3 = 0` after the drop in
  0x808cbc0), so the timed-out client is never sent its `w`. INFERRED.
- A zombie is not free for a new connect, but a connect from the same
  address and qport (or port) reconnects into it. INFERRED from Q3's
  `SV_DirectConnect`, which this one shares the shape of; not measured.
- vcod's zombie repeats the last frame it sent in place of retail's stale
  ring slot; both carry the command sequence that gets the `w` executed.

### Console commands over rcon

Measured 2026-10-08 the same way (retail on a spare port, `rconPassword pw`,
one `vcod --net-probe` for the commands that need a client). Reply strings
VERIFIED by capture unless marked. `SV_AddOperatorCommands` (0x8084a3c)
registers `heartbeat`, `kick`, `banUser`, `banClient`, `clientkick`,
`status`, `serverinfo`, `systeminfo`, `dumpuser`, `map_restart`, `map`,
`map_rotate`, `gameCompleteStatus`, `devmap`, `killserver`, `scriptUsage`,
`stringUsage`, and `say` only when `dedicated` is non-zero. VERIFIED (string
table at 0x8084a3c's calls). vcod ports all but `gameCompleteStatus` and
the two usage dumps.

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
  systeminfo lacks the `sv_referencedPaks` pair (see "Configstring 1").
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
  `g_scriptMainMenu`. VERIFIED. The filter matches the whole name without
  case (`cvarlist G_SPEED` printed `g_speed`, `cvarlist foo` only `foo`)
  and takes `*` (`*_debugMove`). VERIFIED. vcod's `Com_Filter` has no
  `[...]` sets.
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
  elapsed msec. INFERRED.
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
  `Server::handle_packet_at` stamps a move message's ack with the frame time
  of the newest frame whose messages had gone out at arrival
  (`Server::frame_sent`), modelling retail's frame as instantaneous. A
  packet left over past `MAX_PACKETS_PER_FRAME` from an earlier tick takes
  the last frame's time. A vcod bot reads 0, Q3's rule; retail has no bots.
- Measured 2026-10-08 on loopback, mp_harbor, one `--net-probe`, 20
  `status` polls 1 s apart: retail (port 29561) read 0-3 (`1 0 0 1 1 1 3 0 0
  0 0 0 0 1 1 0 0 0 0 0`); vcod's debug build (29562) read 21-50 before the
  change and 0-3 after (`3 0 1 1 0 1 0 1 0 0 1 0 1 0 0 3 0 0 0 0`) with the
  tick still at 11-19 ms mean. VERIFIED by capture.

### Rate (`SV_UserinfoChanged`, 0x8086ab4)

- `cl->rate`, which `status` prints, is 99999 when `Sys_IsLANAddress`
  (0x80c72f8) holds and `dedicated` is not 2. Otherwise it is the
  userinfo's `rate` through `strtol`, clamped to 1000..90000, or 5000 when
  the key is empty or missing. INFERRED from control flow. VERIFIED by
  capture: a loopback probe sending `rate 25000` read 99999 under
  `dedicated 1`; under `dedicated 2` (the 2026-10-07 captures above) it read
  25000.
- `Sys_IsLANAddress` takes loopback and compares an IPv4 address against
  the host's own interface addresses by class (A: first octet, B: two, C:
  three, plus the 172.16/12 and 192.168/16 cases). INFERRED. vcod takes
  loopback and the RFC 1918 ranges instead of enumerating interfaces.

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
- vcod: no authorize detour, so the list is vcod's own (`crate::bans`,
  `--ban-file` to keep it). `getchallenge` from a banned address off the
  LAN gets `error\nEXE_ERR_BAD_CDKEY`; a LAN address is never refused, as
  on retail. The reply skips the two `Resolving` lines and nothing goes to
  the authorize server.

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
