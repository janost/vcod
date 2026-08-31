# CoD 1.1 HUD protocol: obituaries, scores, status configstrings, fonts

Everything below is read out of CoD 1.1 binaries, stock assets, or a live
capture against a running 1.1 server. Claims that could not be settled that way
are marked `UNVERIFIED` with the check that would settle them.

As in `docs/research/cod11-events-and-fx.md`, the authority for
client behaviour is `main/cgame_mp_x86.dll`, not the root `cgamex86.dll`. The
root DLL is the single-player module and its event enum diverges above id 172.

### Evidence sources

All binaries are from the 1.1 install unless stated otherwise.

| File | md5 | Used for |
|---|---|---|
| `cgame_mp_x86.dll` | `4912169a9eb22b404f95c52863a5feb6` | `CG_Obituary`, `CG_ServerCommand`, scores parser, configstring consumers |
| `CoDMP.exe` | `753fbcabd0fdda7f7dad3dbb29c3c008` | font `.dat` loader, text renderer, `^N` colour table |
| `game.mp.i386.so` (the 1.1d Linux dedicated server's game module) | `de8947beb6f86fbfb46f5adfaab3d3ed` | who writes each configstring, obituary event builder, scoreboard message builder. It has a symbol table, so function names below are the module's own |
| `pak5.pk3` | `0cb20baa66ddecc72ccb7f17b3062bb3` | `fonts/fontImage_*.dat`, `gfx/hud/*death*` art |
| `pak0.pk3` | | `weapons/mp/*` weapon defs |
| `cgame_mp_x86.dll` from the 1.5 install | `075e2af18aeaf2aeaf2a75ce22db683a` | 1.5 diff (icon names unchanged) |
| live: `51.195.89.86:28960`, mp_carentan TDM, 2026-08-24 | | `serverCommand` stream, gamestate |

Addresses are virtual: image base `0x30000000` for the cgame DLL, `0x00400000`
for `CoDMP.exe`, file-relative for the Linux `.so`. The cgame DLL and
`CoDMP.exe` facts come from their Ghidra decompilations (exported with
`tools/re/ExportDecomp.java`). The `.so` was read with `objdump -d -M intel`
plus `readelf -r` to resolve symbol references.

Recurring trap numbers (the cgame's syscall pointer is `0x30074898`):
`4` add-kill-message, `9` `Cvar_Set`, `0xb` `Cvar_VariableStringBuffer`,
`0xc` `Argc`, `0xd` `Argv`, `0x18` `SendClientCommand`, `0x4f` `GetGameState`,
`0x30` register-material (the tail of the loading-screen wrapper `0x30030b70`,
which is what the precache calls go through), `0x58` a second
register-material trap used for scoreboard status icons and levelshots.

---

## 0. CoD 1.1 server commands are single letters

`CG_ServerCommand` @ `0x3002e0d0` switches on `argv0[0]` and never compares
the whole token, so every server command in 1.1 MP is one character. This is why
grepping a capture for `scores` or `cs` finds nothing.

| Char | Handler | Meaning |
|---|---|---|
| `a` | `0x30037f20` | takes one int arg |
| `b` | `0x3002b920` | scoreboard (section 3) |
| `c` | | "announcement message" (big centre print) |
| `d` | `0x3002c6b0` | configstring update: `d <index> <string>` |
| `e`, `f` | | "game message" (print queue) |
| `g` | | "bold game message" |
| `h` | | "chat message" |
| `i` | | "team chat message" |
| `j`/`k`/`l` | `0x3002d940(0/1/2)` | three variants of one handler |
| `m` | | int arg, arms a 20 s (or 10 s if negative) timer |
| `n` | `0x3002ca60` | |
| `o`, `p`, `q` | | traps `0xc0`/`0xd4`, `0xd5`, `0x30031fc0` |
| `r` | `0x3002df30` | `CG_ReverbCmd` |
| `s` | `0x3002e040` | `CG_LocalSound` |
| `t` | `0x3002dae0` | open script menu: `t <script menu index>` (section 0.1) |
| `u` | `0x3002de60` | close script menus |
| `v` | `0x30030550` | set client cvar: `v <name> "<value>"` |

Live confirmation from the capture (`serverCommand N:` lines):

```
serverCommand 3: f "vcod ^7Connected"
serverCommand 4: d 0 \codextended\CoDExtended v20\...\g_gametype\tdm\...\mapname\mp_carentan\...
serverCommand 5: v g_scriptMainMenu "team_americangerman"
serverCommand 8: t 0
serverCommand 13: d 5 118
serverCommand 14: d 6 131
serverCommand 29: i "^1Revive:^7 Join our Discord server Cod1.net/^6Discord"
```

`d` carries the new configstring text as argv[1]; the cgame handler ignores it
and re-reads the applied table via `trap_GetGameState` (`0x4f`), exactly as Q3's
`CG_ConfigStringModified` does.

### 0.1 The script menu handshake

Measured on 2026-08-31 against the retail 1.1d dedicated server, dm on
mp_pavlov, with `--net-probe --save-playerstate`; the whole stream is in
`crates/server/tests/fixtures/playerstate/mp_pavlov-dm.txt`. A client that sends
`begin` gets these five commands back, in this order (`^U` is a literal 0x15
byte):

```
f "MPSCRIPT_CONNECTED^Uvcod^7"
v g_scriptMainMenu "team_russiangerman"
v scr_showweapontab "0"
t 0
v cg_objectiveText "DM_KILL_OTHER_PLAYERS^U"
```

INFERRED that the five come from `Callback_PlayerConnect`: `begin` is the notify
its `waittill` blocks on, and the `setClientCvar` and `openMenu` calls that
follow that wait in the stock dm script account for four of them.

`t`'s argument is an index into the script-menu configstring range, whose slot 0
is cs 1180: cs 1180 is `team_russiangerman` and cs 1181 `weapon_russian`, and
answering the team menu produces `v g_scriptMainMenu "weapon_russian"` then
`t 1`.

VERIFIED that `game.mp.i386.so` holds both format strings, `t %i` at
`0x731f8` and `v %s "%s"` at `0x73300`, which is why the name is bare and the
value quoted, and VERIFIED that the capture above matches those two shapes: a
bare integer after `t`, a bare name and a quoted value after `v`. INFERRED
that `t` is therefore the wire form of the script's `openMenu` (`0x453f4`)
and `v` of its `setClientCvar` (`0x446e0`), that those are the strings the
two pass to `trap_SendServerCommand`, and that `openMenu`'s `%i` is
`GScr_GetScriptMenuIndex(name)`'s return value, so the menu's name never
reaches the wire through `t`. Each is which value reaches which argument,
read off the instruction stream.

INFERRED that `setClientCvar` rewrites a `"` inside the value as `'` before
formatting it (`0x447b2`), so the value cannot close its own quoted argument:
that is a branch and its position in the instruction stream, and no capture
holds a value with a quote in it.

VERIFIED that `GScr_GetScriptMenuIndex` (`0x5c73c`) reads its candidates out
of configstrings `0x49c + i`, and that `0x49c` is 1180. VERIFIED that the
not-found path formats the script error `Menu '%s' was not precached`
(`0x771f2`). INFERRED, from the loop's compare-and-branch, that `i` runs
`0..=31` and that the returned index is the first slot whose text matches the
name.

The client answers with the `mr <serverId> <menu index> <response>` client
command, quoting back the index `t` named. Answering `t 0` with `allies` and the
`t 1` that follows with a weapon `maps\mp\gametypes\_teams::restrict` allows for
that menu's nationality spawns the player; `--save-playerstate` does exactly
that.

`Cmd_MenuResponse_f` (`0x486d8`) turns that command into a two-argument
`notify(player, "menuresponse", menu, response)`.

VERIFIED that the first argument is the menu's *name*, not the index the
client sent. The evidence is the cross-check, not the disassembly: the capture
above shows retail answering `mr <sid> 0 allies` with the weapon menu, and
`dm.gsc` (`pak5.pk3`) can only get there by comparing that first argument
against `game["menu_team"]` at `:245`, which `:142` builds as the name string
`"team_" + game["allies"] + game["axis"]`; `:340` hands the same argument back
to `openMenu`, and `:1102` has the script raise the notify itself with a menu
name in that position. An index would match nothing and the loop would spin.
INFERRED that the mechanism is a read of configstring `0x49c + index` back
into the buffer the notify argument is taken from (`0x48790`).

The rest of the handler is INFERRED too, read off the disassembly rather than
run live:

- an argument count other than 4 sends the notify with an empty menu name and
  the response `"bad"`, without reading the serverId at all (`0x486ed`);
- argv[1] is compared against the `sv_serverId` cvar and the handler returns
  without notifying when they differ (`0x4874a`);
- an index outside `0..=31` skips the configstring read, leaving argv[2]'s own
  digits as the menu argument (`0x4877c`).

---

## 1. `EV_OBITUARY` field mapping

`CG_EntityEvent` @ `0x3001dc10`, `case 0xc9` (201 = `EV_OBITUARY`) tail-calls
`CG_Obituary` @ `0x3001d6c0` with the `entityState_t *` still in `EAX`.

The handler's first three reads are the ints at entityState offsets `0x74`,
`0x78` and `0xa0`. It range-checks the first one against `0..63` and aborts with
`"CG_Obituary: target out of range"` (`0x30020750`) when it fails, which names
`+0x74` as the victim:

| entityState offset | Q3/CoD field | vcod netfield (`crates/common/src/net/fields_v1.rs`) | bits | Carries |
|---|---|---|---|---|
| 116 (`0x74`) | `otherEntityNum` | `otherEntityNum` | 10 | victim clientNum (0..63) |
| 120 (`0x78`) | `otherEntityNum2` | `attackerEntityNum` | 10 | attacker clientNum, or `1022` |
| 160 (`0xa0`) | `eventParm` | `eventParm` | 8 | means of death or weapon, see section 2 |

Offsets cross-check against CoDExtended `src/shared.h:643-654`
(`otherEntityNum //116`, `otherEntityNum2 //120`, `eventParm //160`).

The server side is one function, the GSC `obituary()` builtin, at `.so` `0x5a750`
(the only `push 0xc9` in the whole module). It calls `G_TempEntity` with event
`0xc9` (`0x5a78d`), stores script argument 0 (the victim entity number) at
`+0x74` (`0x5a7aa`), and stores argument 1 at `+0x78` only when its variable
type is 7 (entity) with subtype `0xd` (player) (`0x5a7bd`, `0x5a7cf`,
`0x5a7e0`); otherwise `+0x78` gets `0x3fe`, `ENTITYNUM_WORLD` = 1022
(`0x5a7e5`). It then sets `r.svFlags` at `+0xf4` to `8`, `SVF_BROADCAST`
(`0x5a7ec`).

Consequences for the killfeed:

- `attackerEntityNum == 1022` means "no player attacker" (world/environment).
  `CG_Obituary` reproduces this: any attacker outside `0..63` is forced to
  `0x3fe`, the attacker name string is emptied, and only the victim is drawn.
- `victim == attacker` is a suicide; `CG_Obituary` blanks the attacker name in
  that case too.
- The event is `SVF_BROADCAST`, so a spectator sees every obituary regardless of
  PVS.

Names and colours come from the cgame's clientinfo array at `0x3018bc0c`, stride
`0x448` bytes:

| Offset in clientinfo | Read as |
|---|---|
| `+0x00` | `infoValid` (row skipped if 0) |
| `+0x0c` | `name`, `strncpy(..., 0x1f)` |
| `+0x2c` | `team` |
| `+0x30` | `score` (written by the scores parser, section 3) |

Both names get the literal `"^7"` appended before use (the string at
`0x30065348`, written as a 2-byte store plus NUL). Each name's colour comes from
the team-colour helper at `0x3002a810`, called as `(team, vec3 *out)`:

```
team == 1 -> Cvar_VariableStringBuffer("g_TeamColor_Axis",   buf, 1024)
team == 2 -> Cvar_VariableStringBuffer("g_TeamColor_Allies", buf, 1024)
otherwise -> {1,1,1}
             then sscanf(buf, "%f %f %f", &out[0], &out[1], &out[2])
```

Team 1 is Axis, team 2 is Allies (cross-checked against the server in
section 5). Team 0 and 3 render white.

The final draw is trap `4` with nine arguments, in this order: attacker name,
attacker colour, victim name, victim colour, icon name, icon width (`1.4` or
`2.8`), a constant `1.4f` (`0x3fb33333`), and a third colour. So the row reads
attacker, icon, victim; all three colour vec4s default to white with alpha 1;
and the icon is passed by material name, not by handle.

If the local client is the victim, the attacker's name is also copied to
`0x3020c044` (the "killed by" name). If the local client is the attacker, a
console/print line is produced from one of two localized formats:

- `"CGAME_YOUKILLED\x15%s"` (`0x30064c3c`) for a normal kill
- `"CGAME_YOUKILLED\x15^1%%s^7 %s\x14%s"` + `"CGAME_TEAMMATE"` (`0x30064c50`)
  when the attacker's team is non-zero and equals the victim's team.

---

## 2. The icon rule

`eventParm` is 8 bits wide on the wire. Bit 7 selects which of two namespaces
the low 7 bits live in.

`CG_Obituary` tests the sign of the byte:

- Bit `0x80` set: the low 7 bits are a means-of-death enum value, and the
  icon comes from the switch below.
- Bit `0x80` clear: the value indexes `weaponDefs[]`. The `killIcon` string at
  weaponDef `+0x2e0` is used as the icon name when it is non-empty, and the
  `wideKillIcon` int at `+0x2e4` selects the 2.8 icon width; the switch then
  runs with value 0 and falls through its default, keeping the weapon icon.
  An empty `killIcon` falls back to `killIconDied`.

The two weapon-def field offsets are settled from the weapon-file parse table at
`0x30075c84` (triples of `{keyword, structOffset, type}`):

```
0x30066424 "killIcon"      offset 0x2e0  type 0 (string)
0x30066414 "wideKillIcon"  offset 0x2e4  type 5 (int)
```

and confirmed against the stock weapon files, e.g.
`pak0.pk3:weapons/mp/thompson_mp` has `killIcon\gfx/hud/hud@death_thompson.tga`,
`wideKillIcon\1`; `weapons/mp/colt_mp` has `killIcon\gfx/hud/hud@death_colt45.tga`,
`wideKillIcon\0`. The art sizes match the flag: `hud@death_thompson.dds` is
11088 bytes, `hud@death_colt45.dds` is 5616, exactly half.

`iconWidth` defaults to the constant `1.4f` at `0x30069764` and becomes `2.8f`
(`0x40333333`) only for wide weapon icons. The MOD branch never sets it, so
every MOD icon is 1.4 wide.

### Means-of-death enum

Recovered from the pointer table at `.so` file offset `0x7cda0`, 25 entries:

| # | Name | # | Name | # | Name |
|---|---|---|---|---|---|
| 0 | `MOD_UNKNOWN` | 9 | `MOD_MORTAR` | 18 | `MOD_LAVA` |
| 1 | `MOD_PISTOL_BULLET` | 10 | `MOD_MORTAR_SPLASH` | 19 | `MOD_CRUSH` |
| 2 | `MOD_RIFLE_BULLET` | 11 | `MOD_KICKED` | 20 | `MOD_TELEFRAG` |
| 3 | `MOD_GRENADE` | 12 | `MOD_GRABBER` | 21 | `MOD_FALLING` |
| 4 | `MOD_GRENADE_SPLASH` | 13 | `MOD_DYNAMITE` | 22 | `MOD_SUICIDE` |
| 5 | `MOD_PROJECTILE` | 14 | `MOD_DYNAMITE_SPLASH` | 23 | `MOD_TRIGGER_HURT` |
| 6 | `MOD_PROJECTILE_SPLASH` | 15 | `MOD_AIRSTRIKE` | 24 | `MOD_EXPLOSIVE` |
| 7 | `MOD_MELEE` | 16 | `MOD_WATER` | | |
| 8 | `MOD_HEAD_SHOT` | 17 | `MOD_SLIME` | | |

### Which deaths get the `0x80` flag

The server tags exactly seven of them. The `obituary()` builtin at `.so`
`0x5a7f6` takes the tag path for mod 7 or 8 (a `mod - 7 <= 1` unsigned compare)
and for mods 22, 21, 19, 16 and 17 (five equality compares), then ORs `0x80`
into the mod (`0x5a817`) and stores it as `eventParm` at `+0xa0` (`0x5a81a`).
Every other mod stores the weapon index at `+0xa0` instead (`0x5a822`).

`{7, 8, 16, 17, 19, 21, 22}` is precisely the case set of the client switch, so
the two halves agree.

### The table

| `eventParm` | Means of death | Icon name the client uses | Stock art |
|---|---|---|---|
| `0x87` | `MOD_MELEE` (7) | `killIconMelee` | `gfx/hud/death_melee.dds` |
| `0x88` | `MOD_HEAD_SHOT` (8) | `killIconHeadShot` | `gfx/hud/death_headshot.dds` |
| `0x90` | `MOD_WATER` (16) | `killIconDrown` | none ships |
| `0x91` | `MOD_SLIME` (17) | `killIconSlime` | none ships |
| `0x93` | `MOD_CRUSH` (19) | `killIconCrush` | `gfx/hud/death_crush.dds` |
| `0x95` | `MOD_FALLING` (21) | `killIconFalling` | `gfx/hud/death_falling.dds` |
| `0x96` | `MOD_SUICIDE` (22) | `killIconSuicide` | `gfx/hud/death_suicide.dds` |
| `0x97` | `MOD_TRIGGER_HURT` (23) | `killIconDied` | `gfx/hud/death_died.dds` |
| `0x00`..`0x7f` | weapon index | `weaponDefs[parm]->killIcon`, e.g. `gfx/hud/hud@death_mp40.tga` | ships per weapon |
| `0x00`..`0x7f` with empty `killIcon` | weapon index | `killIconDied` | `gfx/hud/death_died.dds` |
| any other `0x80`-flagged value | | `killIconDied` (switch default) | `gfx/hud/death_died.dds` |

Notes:

- `0x97` (`MOD_TRIGGER_HURT`) has a client case but the stock 1.1 server never
  sets bit 7 for mod 23, so a stock server never sends it. A mod could.
- `MOD_LAVA` (18) has neither a flag on the server nor a case on the client.
- The eight `killIcon*` names are precached at cgame init
  (`0x30020e9f`..`0x30020ee5`, each `RegisterMaterial(name, 5)` with the handle
  discarded), then passed to trap `4` by name.

UNVERIFIED: the `killIcon<Mod>` to image binding. No stock pk3 contains an
entry named `killIconMelee` (checked: full file listings of all nine
`main/*.pk3`, plus `zipgrep` over `pak0.pk3` and `pak5.pk3`, plus every
`fxshaders/*.shader` and every `ui_mp/*` file). The only plausible art is the
six `gfx/hud/death_*.dds` images whose names line up one-for-one with the six
MOD cases that have art, and 1.5's cgame uses the identical names. vcod maps
the MOD case straight to `gfx/hud/death_<mod>.dds` and draws nothing for
drown/slime. Check that would settle it: run the stock 1.1 MP client, die by
melee, and screenshot the killfeed row.

---

## 3. The `scores` server command

Command letter `b`, built by `DeathmatchScoreboardMessage` (`.so` `0x459c0`),
parsed by `0x3002b920`.

### Grammar

```
b <numRows> <axisScore> <alliesScore>{ <client> <score> <ping> <time> <statusIcon>}*
```

Server format strings (`.so` `0x737f0` and `0x737e0`):

```
"b %i %i %i%s"          outer
" %i %i %i %i %i"       one row, appended numRows times
```

The row push order at `0x45a65`..`0x45a78` (right-to-left on the stack) is
`sortedClients[i]`, `cl[+0x20e0]`, `ping`, `cl[+0x20e4]`, `cl[+0x20d8]`.

The client reads them back with `Argv(4 + 5*i + k)` into a six-int row
(`0x3020ba30`, stride `0x18`):

| Token | Client slot | Meaning |
|---|---|---|
| 1 | `row[0]` | clientNum; clamped to 0 if outside `0..63` |
| 2 | `row[1]` | score; also copied into `clientinfo[client].score` (`0x3018bc3c`) |
| 3 | `row[2]` | ping (`cl[+0x20c8]`); server sends `-1` when `cl[+0x20ec] == 1` (connecting), else clamps to `999` |
| 4 | `row[3]` | "time", `cl[+0x20e4]`, the Q3 `pers.enterTime` position |
| 5 | `row[5]` | status-icon index; if in `1..8` the client resolves configstring `20 + n` and replaces it with a material handle |

`row[4]` is not on the wire. The client fills it with
`clientinfo[client].team` and uses it to accumulate per-team row counts
(`0x3020ba20[team]`) and average ping (`0x3020ba10[team] /= count`), so the
scoreboard's team blocks are derived client-side from clientinfo, not sent.

Header tokens:

- `numRows` is `min(level.numConnectedClients, 64)`; the client clamps to `0x40`
  again.
- Token 2 is `level.teamScores[1]` = Axis, token 3 is `level.teamScores[2]` =
  Allies (proved in section 5). The client stores them at `0x3020ba04` /
  `0x3020ba08`; `-9999` means "no score" and renders as `"-"` (`0x300630c8`).

### Spectators

Spectators appear as ordinary rows. `DeathmatchScoreboardMessage` walks
`level.sortedClients[]` and emits every entry; the sort comparator (`.so`
`0x500ab`) treats `cl[+0x217c] == 3` (the spectator team) as a separate class
that sorts last, but nothing filters them out of the message. The client puts
them in team bucket 3, which the team-colour helper at `0x3002a810` renders
white. Server-side banner strings `g_ScoresBanner_Allies` / `_Axis` / `_None` /
`_Spectators` exist for the four buckets.

### Live example

The first 90 s probe capture, taken before vcod sent `score`, logged 49
`serverCommand` lines (42 `d`, 3 `v`, 2 `f`, 1 `i`, 1 `t`) and not one `b`.
Section 4 shows the server only sends `b` on request or at intermission, so
that is expected.

The 2026-08-24 evening live sweep held the Tab scoreboard open against a
populated server and confirmed the grammar above end to end on screen:
Axis/Allies sections with team icons and totals, score-sorted rows, a
Spectators section, `hud unk 0` throughout. No wire-level dump of a raw `b`
line was kept, so token 4's unit (below) is still open.

UNVERIFIED: the meaning and unit of token 4. `cl[+0x20e4]` has no write site
anywhere in the stock module's `.text` (`grep 0x20e4` finds three reads and no
store), and unlike Q3 the server sends it raw rather than as
`(level.time - enterTime)/60000`. `SortRanks` uses it as the ascending tiebreak
after score, which is the `pers.enterTime` role. Check: print a real `b` line
from a `score` request and compare token 4 against a known join time.

---

## 4. How `scores` is requested

Client console commands, registered by the cgame: `+scores`, `-scores`, `score`
(the three are the only score-ish strings in the DLL).

The scoreboard draw function at `0x3002b650` re-requests on a fixed interval
while the board is up: it keeps a last-request timestamp at `0x3020b9f8`, and
when `cg.time` has passed it by more than 2000 ms it resets the stamp and
issues trap `0x18` (`trap_SendClientCommand`) with the string `"score"`.

So the client sends the reliable client command `score` (no `cmd` prefix at
this layer; `trap_SendClientCommand` adds the framing) and repeats it every
2000 ms for as long as the scoreboard is being drawn.

Server side (`ClientCommand`, `.so` `0x488e3`): `Q_stricmp(argv0, "score") == 0`
leads to `DeathmatchScoreboardMessage(ent)`. That is one of only two callers.

The other caller is `SendScoreboardMessageToAllIntermissionClients` (`.so`
`0x50bf0`), which is gated three ways:

```
if (level[+0x20c] == 0) return;                 // dirty flag, set by setteamscore
for each connected client:
    if (cl[+0x20ec] != 2) continue;             // must be CON_CONNECTED
    if (ps[+0x4]  != 5)   continue;             // pm_type == PM_INTERMISSION
    DeathmatchScoreboardMessage(client);
level[+0x20c] = 0;
```

There is no periodic push during normal play. A spectator that wants a live
scoreboard has to send `score` itself, exactly as the stock client does, on the
same 2 s cadence.

---

## 5. Status configstrings in 1.1 MP

Every index below is a `trap_SetConfigstring` call site in the stock server
module, recovered by resolving the `R_386_PC32` relocations against
`trap_SetConfigstring` and reading the immediate pushed two slots earlier.
Client-side consumers are `CG_ConfigStringModified` @ `0x3002c6b0` and the
init path `0x3002c060`.

| CS | Written by | Contents | Client use |
|---|---|---|---|
| 0 | engine | serverinfo | `CG_ParseServerinfo` @ `0x3002bbb0` reads `sv_hostname`, `g_gametype`, `sv_maxclients`, `mapname` |
| 1 | engine | systeminfo | download/pure checks |
| 2 | `SP_worldspawn` | `"cod"` | game/module mismatch check |
| 3 | `SP_worldspawn` | `n\<ambienttrack>` | start ambient music |
| 4 | `SP_worldspawn` | worldspawn `message` | |
| 5 | `setteamscore` | Axis score | `0x301d24f4`; `-9999` renders `"-"` |
| 6 | `setteamscore` | Allies score | `0x301d24f8`; `-9999` renders `"-"` |
| 7 | `BG_SetupWeaponInfo` | all-weapon-name list | weapon table |
| 8 | `SaveRegisteredItems` | item bits | `0x30036150` |
| 11 | `SP_worldspawn` | `northyaw` | compass |
| 12 | `Cmd_Fogswitch_f`, `G_setfog` | fog params | `0x3002bcf0` |
| 13 | `SP_worldspawn` | level start time, `va("%i", level.startTime)` | `0x301d24f0` |
| 14 | `SP_worldspawn` | MOTD, `g_motd->string` (`g_motd + 0x10`) | |
| 15 | `Cmd_CallVote_f`, `CheckVote` | vote time (ms) | `0x301d21c0`, drives the `(t - cg.time)/1000` countdown in `0x30018120` |
| 16 | `Cmd_CallVote_f` | vote string | `0x301d21d0`, 255 bytes |
| 17 | `Cmd_CallVote_f`, `Cmd_Vote_f` | vote yes count | `0x301d21c4` |
| 18 | `Cmd_CallVote_f`, `Cmd_Vote_f` | vote no count | `0x301d21c8` |
| 20 | `G_InitGame` | match-state infostring; `Info_SetValueForKey(cs, "winner", "0")` | not handled by the cgame |
| 21..28 | script | player status icons (max 8) | material per index; the scores command's 5th token indexes here |
| 29..43 | script | player head icons | material per index |
| 44+ | `target_location_linkup` | location names | |
| 108..139 | `G_TagIndex` | attachment tag names | |
| 140..203 / 204..267 | writer not found in the stock module (no literal `trap_SetConfigstring` index in that range) | 64 pairs of (cvar name, cvar value) | `0x3002bc60` does `Cvar_Set(cs[140+i], cs[204+i])` for i < 64, stopping at the first empty name |
| 268..523 | `G_ModelIndex` | xmodel paths | |
| 524..779 | `G_SoundAliasIndex` | sound aliases | |
| 780..843 | `G_EffectIndex` | `.efx` paths | |
| 1100..1115 | `G_ShellShockIndex` | shellshock scripts | |
| 1180..1211 | menus | precached menus | |
| 1244+ | `G_LocalizedStringIndex` | localized strings; hudelem strings are `1244 + n` | |
| 1500+ | `G_ShaderIndex` | materials | |

This agrees with the block map in `crates/common/src/net/protocol.rs`, which
was built from a live gamestate dump on 2026-08-23. The live probe on
2026-08-24 reported `354 configstrings` set on mp_carentan, and `d 5 <n>` /
`d 6 <n>` arrived continuously during the TDM round with both counters
climbing (5: 117 to 129, 6: 126 to 153 over 90 s). All 42 `d` commands in that
window went to index 5 (13 times), 6 (28 times) or 0 (once); nothing else
changed mid-round.

### Which team score is which

`setteamscore` (`.so` `0x5b9dc`):

```
bx = Scr_GetConstString(0)
if (bx != scr_const[+4] && bx != scr_const[+8])
        Scr_Error("Illegal team string '%s'. Must be allies, or axis.")
if (bx == scr_const[+4]) { level[+0x200] = score; trap_SetConfigstring(6, va("%i", score)); }
else                     { level[+0x1fc] = score; trap_SetConfigstring(5, va("%i", score)); }
level[+0x20c] = 1;   // intermission scoreboard dirty flag
```

`GScr_LoadConsts` (`.so` `0x58550`) fills `scr_const+4` with
`Scr_AllocString("allies")` and `scr_const+8` with `Scr_AllocString("axis")`.
So CS 6 = allies, CS 5 = axis, and `level.teamScores[1]` is axis while
`level.teamScores[2]` is allies, matching the `g_TeamColor_Axis` (team 1) /
`g_TeamColor_Allies` (team 2) split the cgame uses in section 1.

`DeathmatchScoreboardMessage` pushes `level[+0x1fc]` before `level[+0x200]`, so
the `b` command's token 2 is axis and token 3 is allies, in the same order as
CS 5 then CS 6.

### Header vs. scoreboard totals can disagree in the same frame

A live capture (2026-08-24, `51.195.89.86` mp_chateau) caught the status
header, read straight off CS 5/6, showing `219 tdm 214` while the same
frame's `b` reply carried axis 221 / allies 231. Both configstrings and the
`b` reply are legitimate wire values. A `d` update for 5/6 and the server's
next `b` push (or the client's own periodic `score` re-request, section 4) do
not land in the same tick, so the two can transiently read a few points apart.

Ruling for vcod: the status header prefers the most recent `b` reply's
axis/allies totals once the Tab scoreboard has received one
(`Scoreboard::totals`, `status::read_status`'s `scoreboard_totals`
parameter), falling back to CS 5/6 only before any `b` has arrived. A `b`
reply is requested and answered inside the same round-trip, while CS 5/6 rely
on a `d` push reaching the client on its own schedule, so the `b` side is the
more current one when they disagree.

UNVERIFIED: whether the stock client's own header shows the same transient
disagreement, i.e. whether it also reads CS 5/6 directly for its header line
rather than caching the scoreboard's last reply. Check: open the stock 1.1 MP
client's `+scores` command against a server showing a fresh score change and
compare its header text to its own Tab scoreboard in the same frame; not yet
done.

### There is no round-timer configstring

Nothing writes a match/round clock to a configstring, and the cgame's clock
formatters do not read one. `0x3001ece0` (`"%i:%02i"` / `"%i:%02i:%02i"`) and
`0x3001ed60` (tenths) both take their value from `0x3001ec70`, which reads
`hudelem[0x17]`, an absolute millisecond timestamp, and picks a direction from
`hudelem[0]`:

| `hudelem[0]` | Value |
|---|---|
| 4 | `(t - cg.time) + 999`, count down, whole seconds |
| 5, 7, 9 | `cg.time - t`, count up |
| 6 | `(t - cg.time) + 99`, count down, tenths |
| 8 | `t - cg.time` |

The same struct carries `hudelem[0xb]` (a localized-string index, used as
configstring `0x4dc + n` = 1244 + n) and `hudelem[0x19]` (a numeric value). The
round clock is therefore a server-scripted HUD element, not a configstring,
and it is created by the gametype's GSC.

The live serverinfo confirms there is no `timelimit` key to fall back on:

```
\g_gametype\tdm\gamename\Call of Duty\...\mapname\mp_carentan\...\sv_maxclients\64\...
```

UNVERIFIED: how hudelems reach the client. `CG_Obituary`'s siblings read
them from a resident array, and vcod's `PLAYER_FIELDS` table has no `hudElem*`
entries, so the transport is unconfirmed. Check: decompile the cgame's hudelem
refresh (the caller of `0x3001ec70`, around `0x3001f5b4`) and match the source
buffer against the playerState netfield offsets in
`crates/common/src/net/fields_v1.rs:85-189`.

For the spectator HUD this is moot. `crates/client/src/hud/status.rs` shows
elapsed time since level start (`server_time - cs[13]`) instead of the round
clock. The 2026-08-24 live sweep confirmed the header ticking correctly
(`219 tdm 214 / 14:34`-style output) against two servers. The check above
still stands for anyone who later wants the real round countdown.

---

## 6. `fonts/fontImage_<size>.dat` layout

Six files ship in `pak5.pk3`, all exactly 20552 bytes:
`fontImage_{12,16,18,24,30,32}.dat`.

The engine loader is `0x004dee00` (`"fonts/fontImage_%i.dat"` at `0x004def0e`).
It hard-rejects any other size: the `FS_ReadFile` length must equal `0x5048`
(20552) exactly. The in-memory `fontInfo_t` stride is the same `0x5048` (the
font cache walks `ptr += 0x5048`, and the copy loop moves `0x1412` dwords =
20552 bytes).

Per glyph the loader byte-swaps 12 dwords and then copies 8 dwords raw,
advancing the source by `0x50`:

```
for each of 256 glyphs:
    dst[0..12]  = LittleLong(src[0..12])    // bytes 0x00..0x2f
    dst[12..20] = src[12..20]               // bytes 0x30..0x4f, shaderName
    src += 0x50; dst += 0x50
```

That is Q3's `glyphInfo_t` exactly: 12 four-byte fields plus `char shaderName[32]`
= 80 bytes, times 256 = 20480 = `0x5000`. The shader-registration pass right
after confirms the tail offsets. It walks `p = base + 48` (shaderName) with
stride `0x50` and stores the handle at `p - 4` (offset 44).

The field types differ from Q3 even though the layout does not. Decoded from
`fontImage_16.dat`:

| Offset | Type | `' '` | `'A'` | `'i'` | `'.'` | Reading |
|---|---|---|---|---|---|---|
| 0 | int | 0 | 11 | 12 | 2 | glyph height in px (= `(t2-t)*256`) |
| 4 | int | 0 | 12 | 4 | 4 | glyph width in px (= `(s2-s)*256`) |
| 8 | float | 1.0 | 12.0 | 13.0 | 3.0 | always height + 1 |
| 12 | float | 0.0 | -0.333 | +0.333 | +1.0 | horizontal bearing, scales with font size |
| 16 | float | 3.859 | 10.667 | 4.667 | 4.333 | advance / xSkip |
| 20 | int | 0 | 12 | 4 | 4 | `imageWidth` (duplicates +4) |
| 24 | int | 0 | 11 | 12 | 2 | `imageHeight` (duplicates +0) |
| 28,32,36,40 | float | | | | | `s, t, s2, t2` |
| 44 | int | 39 | 39 | 39 | 39 | glyph handle (overwritten at load) |
| 48..79 | char[32] | `fonts/fontImage_0_16.tga` | | | | shader name |

So Q3's `height/top/bottom/pitch/xSkip/imageWidth/imageHeight` block became
`height / width / (height+1) / bearing / advance / imageWidth / imageHeight`,
with three of the seven promoted from `int` to `float`. The `s/t/s2/t2/glyph/
shaderName` tail is unchanged from Q3.

### The extra 4 bytes

`20552 - 20480 = 72` bytes of header follow the glyph array; Q3's tail is
`float glyphScale` + `char name[64]` = 68. The extra 4 bytes are a second
float at offset 20484 (`0x5004`), sitting between `glyphScale` and `name[64]`:

```
0x0000 .. 0x4FFF   glyphInfo_t glyphs[256]      20480
0x5000             float glyphScale                 4
0x5004             float lineHeight                 4   <-- not in Q3
0x5008 .. 0x5047   char  name[64]                  64
                                                 -----
                                                 20552 = 0x5048
```

The loader treats them as two separate scalars. It byte-swaps `file+0x5000`
into `dst[0x1400]` and `file+0x5004` into `dst[0x1401]`, then block-copies 64
bytes into `dst[0x1402]` and immediately overwrites that with
`strncpy(dst + 20488, requestedPath, 0x3f)` plus a NUL at `0x5047`. In the
shipped files those 64 bytes are all zero, so the name only ever holds the path
the engine asked for.

Both floats are used by the text renderer at `0x004d7...`: the glyph size
scale is the caller's size argument times `font[0x5000]`, and a `'\n'` advances
`y` by the same size argument times `font[0x5004]`.

Values in the stock set:

| File | `+0x5000` glyphScale | `+0x5004` | `+0x5004 / glyphScale` | tallest glyph |
|---|---|---|---|---|
| `fontImage_12.dat` | 4.0 | 52.0 | 13.0 | 14 |
| `fontImage_16.dat` | 3.0 | 45.0 | 15.0 | 15 |
| `fontImage_18.dat` | 3.0 | 48.0 | 16.0 | 16 |
| `fontImage_24.dat` | 2.0 | 44.0 | 22.0 | 23 |
| `fontImage_30.dat` | 1.6 | 48.0 | 30.0 | 30 |
| `fontImage_32.dat` | 1.5 | 46.5 | 31.0 | 31 |

`glyphScale` is `48 / pointSize` for every size except 18 (which reuses the 16pt
scale of 3.0), i.e. the same "source font rendered at 48pt" convention Q3 uses.
The second float divided by `glyphScale` lands within one pixel of the tallest
glyph, so it reads as the line box height in design units.

UNVERIFIED: the second float's real name. Its use is settled (newline
advance, same scale factor as `glyphScale`); only the id-internal field name is
unknown, and nothing downstream needs it.

---

## 7. The `^N` colour palette

Handled entirely by the engine's text renderer at `0x004d7f13`, not by the
cgame. On a `^` the renderer looks at the next byte. If it is not `^` and lies
in `'0'..'7'` (`0x30..0x37`), the byte is consumed and the colour changes:
`^7` restores the caller's colour in full, any other code takes RGB from
`colorTable[b & 7]` (masked with `0x00ffffff`) and the alpha from the caller
(`callerAlpha << 24`).

Three things follow: only `^0`..`^7` are recognised (`^8`, `^9`, `^a` are printed
literally), `^^` escapes a caret, and alpha always comes from the caller;
`^N` only ever replaces RGB. `^7` is special-cased to restore all four
components of the caller's colour rather than reading `colorTable[7]`.

`colorTable` is eight RGBA dwords at `0x005405d8`, bytes in R,G,B,A order:

| Code | Bytes (R,G,B,A) | Hex | Name |
|---|---|---|---|
| `^0` | 0, 0, 0, 255 | `#000000` | black |
| `^1` | 255, 0, 0, 255 | `#FF0000` | red |
| `^2` | 0, 255, 0, 255 | `#00FF00` | green |
| `^3` | 255, 255, 0, 255 | `#FFFF00` | yellow |
| `^4` | 0, 0, 255, 255 | `#0000FF` | blue |
| `^5` | 0, 255, 255, 255 | `#00FFFF` | cyan |
| `^6` | 255, 0, 255, 255 | `#FF00FF` | magenta |
| `^7` | 255, 255, 255, 255 | `#FFFFFF` | white |

The float form of the same eight colours (Q3's `colorBlack` .. `colorWhite` /
`g_color_table`) is present byte-identically in all three modules:
`cgame_mp_x86.dll` @ `0x30060868`, `CoDMP.exe` @ `0x00541950`, and the server
module at file offset `0x79fa0`.

One CoD-specific wrinkle: the chat handlers (`h`/`i`) run every incoming string
through `0x3002df00`, which strips every byte `0x19` before display. Any
renderer vcod writes should do the same, and should strip `^N` pairs from a
string before measuring its width.

---

## UNVERIFIED summary

| # | Claim | Check that settles it |
|---|---|---|
| 1 | `killIcon<Mod>` material names bind to `gfx/hud/death_<mod>.dds` (section 2). No stock pk3 entry carries the name; the mapping is by name correspondence only. | Run the stock 1.1 MP client, take a melee death, screenshot the killfeed. |
| 2 | Scores token 4 is the Q3 `pers.enterTime` slot; unit unknown, and the field has no write site in the stock server module (section 3). | Log a real `b` line from a `score` request and compare token 4 against a known join time. |
| 3 | Resolved: the `b` grammar. The 2026-08-24 live sweep held the Tab scoreboard against a populated server and confirmed it on screen (section 3). No raw wire dump was kept, so token 4's unit (item 2) is still open. | n/a |
| 4 | Hudelem transport for the round clock (section 5). vcod's `PLAYER_FIELDS` has no `hudElem*` entries. Moot for vcod: the shipped header uses elapsed-since-level-start instead and was confirmed working live. | Decompile the caller of `0x3001ec70` (near `0x3001f5b4`) and match its source buffer against `crates/common/src/net/fields_v1.rs:85-189`, only if a real round countdown is wanted later. |
| 5 | The second font header float's id-internal field name (section 6). Its use is settled. | Nothing downstream depends on it. |
| 6 | Whether the stock client's own status header shows the same CS-5/6-vs-`b` transient disagreement observed live (section 5). vcod's header prefers the `b` reply once one has arrived. | Compare the stock client's header text to its own Tab scoreboard in the same frame against a server showing a fresh score change; not yet done. |
| 7 | Trap `4`'s official name (section 1). Its nine-argument signature is settled from the call site. | Decompile the engine's cgame syscall dispatcher and read case 4. |
