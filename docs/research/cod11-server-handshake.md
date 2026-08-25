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
and pins the stock values. Only 179/243 is map-dependent. `scr_motd` (180) has
an empty value, which is also the loop's stop condition on the client.

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

## Raw captures

I kept the captures locally; they are not in the repo. Regenerate them with
the commands above. What I captured on 2026-08-25, per map: the three
connectionless replies, the full `RUST_LOG=debug` probe log, the `cs[i] = ...`
lines on their own, and the raw gamestate message from `--save-snapshots`,
which leaves the committed fixture alone.
