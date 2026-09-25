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
| `u` | `0x3002de60` | close script menus: the bare letter, no argument |
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
`crates/server/tests/fixtures/playerstate/mp_pavlov-dm.txt`. A client that has
entered the world -- which is the first usercmd after the gamestate, not any
command it sends (`docs/protocol-1.1.md`, "Entering the world") -- gets these
five commands back, in this order (`^U` is a literal 0x15 byte):

```
f "MPSCRIPT_CONNECTED^Uvcod^7"
v g_scriptMainMenu "team_russiangerman"
v scr_showweapontab "0"
t 0
v cg_objectiveText "DM_KILL_OTHER_PLAYERS^U"
```

INFERRED that the five come from `Callback_PlayerConnect`: `begin` is the notify
its `waittill` blocks on -- the engine raises it from `ClientBegin`, inside
`SV_ClientEnterWorld` -- and the `setClientCvar` and `openMenu` calls that
follow that wait in the stock dm script account for four of them.

`t`'s argument is an index into the script-menu configstring range, whose slot 0
is cs 1180: cs 1180 is `team_russiangerman` and cs 1181 `weapon_russian`, and
answering the team menu produces `v g_scriptMainMenu "weapon_russian"` then
`t 1`.

VERIFIED that `game.mp.i386.so` holds all three format strings, `t %i` at
`0x731f8`, `v %s "%s"` at `0x73300` and the lone `u` the `closeMenu` builtin
(`0x45574`) passes to `trap_SendServerCommand`, at `0x73204`. That is why the
name is bare and the value quoted, and why `u` never carries an argument.
VERIFIED that the capture above matches the first two shapes: a
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
| 4 | `row[3]` | deaths: `cl[+0x20e4]` is the client's `deaths` script field (offset 8420 in the client field table, `cod11-gsc-object-model.md`), which every stock gametype's `Callback_PlayerKilled` increments; token 2's `cl[+0x20e0]` is `score` (8416) the same way |
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

VERIFIED, the comparator's reads and compares (`.so` 0x50099-0x50109): it
tests `cl[+0x20ec] == 1` on either side first (a connecting client sorts
last), then `cl[+0x217c] == 3` on either side (a spectator sorts after a
non-spectator, and two spectators order by their `gclient_t` address, which
is slot order), then `cl[+0x20e0]` (score) with the greater first, then
`cl[+0x20e4]` (deaths) with the fewer first, and returns 0 on a full tie.
VERIFIED, from the retail round-restart target capture
(`crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-target.txt`,
`b 2 0 1 1 0 0 0 0 0 -1 1 1 0`): the 0-score row leads the -1-score one
although its slot is the higher.

**As implemented** (`Server::scoreboard`, `crates/server/src/server.rs`). One
row per online client, in that order, ties in slot order. The score comes from the client's own `.score` script
field and the icon from `.statusicon`, resolved to its 1-based slot within
`CsRange::StatusIcon`; both are where every stock gametype writes them. The
team totals go out as `0 0`, hardcoded: no `setTeamScore` builtin exists yet,
so there is nowhere for a gametype's own totals to live. VERIFIED: `0` is
what retail's `b` line carries in both slots of both hit captures
(`crates/server/tests/fixtures/playerstate/mp_carentan-dm-hit-target.txt` and
the `tdm` pair, and `cod11-combat.md` section 9). INFERRED: `level.teamScores[]`
therefore starts zeroed, and the `-9999` sentinel is a value a gametype has to
write for itself rather than the default an unwritten total holds.
`ping` is 0 rather than retail's clamp, the netchan
keeping no round-trip estimate. Token 4 is the client's `deaths` script
field, read the same way as `score`.

VERIFIED: token 4 is `deaths`. `cl[+0x20e4]` has no write site in the stock
module's `.text` (`grep 0x20e4` finds three reads and no store) because the
script writes it: the client field table registers `deaths` at offset 8420 =
`0x20e4` and `score` at 8416 = `0x20e0`, the token 2 source
(`cod11-gsc-object-model.md`, the client field dump). INFERRED: `SortRanks`'s
use of it as the ascending tiebreak after score is therefore "fewer deaths
ranks higher", not the Q3 `pers.enterTime` tiebreak this section used to
read into the slot. A retail client renders it in the scoreboard's deaths
column, which is where a hand check on 2026-09-03 saw the minutes vcod used
to send.

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
| 140..203 / 204..267 | the engine, not the game module: `docs/research/cod11-map-cycle.md` 3.2 reads the writer at `0x808b148` in `cod_lnxded`, which is why no `trap_SetConfigstring` index in that range appears here | 64 pairs of (cvar name, cvar value) | `0x3002bc60` does `Cvar_Set(cs[140+i], cs[204+i])` for i < 64, stopping at the first empty name |
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

### How a hudelem reaches the client

They travel in the playerstate, in the two 31-entry arrays of array block 5,
which is why the `PLAYER_FIELDS` netfield table has no `hudElem*` entry: the
block sits past the 103 scalar fields and is not in that table at all. The
wire layout, the field order and the widths are in `docs/protocol-1.1.md`,
"Block 5"; what follows is the server half.

VERIFIED, from `game.mp.i386.so`: `HudElem_UpdateClient` (0x4be8c) takes a
`gclient_t`, an entity number and a two-bit selector, clears the selected
array or arrays (`bzero` of 0xd90 bytes at `ps+0x1338` and `ps+0x5A8`,
0x4bea7 and 0x4bec8), then walks all 0x400 `g_hudelems` records and copies
0x1c dwords of each survivor into the next free slot. A record is skipped
when its `type` (`+0x0`) is zero, when its team (`+0x74`) is non-zero and
differs from `gclient+0x217c` -- the same `clientState.team` `.sessionteam`
writes -- or when its owner (`+0x70`) is neither `0x3ff` nor the entity
number passed in. `archived` (`+0x78`) picks `ps+0x1338` when set,
`ps+0x5A8` when clear, and each array stops at 31 elements.

VERIFIED that `G_RunFrame` (0x50a80) makes that call once per in-use client
per frame with both arrays selected, and that `SpectatorClientEndFrame`
(0x408bc) makes it with only `ps+0x5A8` selected immediately after copying
the followed player's playerstate wholesale. INFERRED from those two call
sites: `archived` marks the elements that belong to the playerstate a
follower inherits, which is where the community's `hud.archival` /
`hud.current` names come from.

VERIFIED that `G_UpdateHudElemsToClients` (0x5121c) is the same per-client
loop and that nothing calls it: no relocation in the module names it, so the
live path is `G_RunFrame`'s own copy of the loop.

So the round clock above is a `hudelem_t` in `ps+0x1338`, and the connect-time
frame decoded in `docs/protocol-1.1.md` is one on the wire: type 4, `x` 320,
`y` 460, both alignments 1, `font` 1, `fontScale` 1.0, colour `0xFFFFFFFF`
and `time` 1800000, which is `dm.gsc`'s `startGame` field for field.

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

## 8. How the cgame draws a hudelem

Everything in this section is read out of `cgame_mp_x86.dll` (1.1). The
element fields are named by their offsets in `hudelem_t`, which
`docs/protocol-1.1.md` "Block 5" maps to the wire.

### Order

VERIFIED: `0x3001f920` walks two runs of 31 pointers at stride 112, the first
from the snapshot playerstate's `+0x1338` and the second from its `+0x5A8`,
compares each element's `type` word against 0, and hands the list it built to
the CRT `qsort` (`0x3004b350`) with the comparator at `0x3001f8e0`. VERIFIED:
the comparator loads the float at `+0x6c` (`sort`) of both elements, subtracts
them and returns -1, 1 or 0. INFERRED, off those branches: the list is the
archived array then the current one, each stopping at its first `type` 0, and
it is sorted ascending by `sort`; `0x3001f980` then draws it in that order.
`qsort` is not stable, so the order of two elements with equal `sort` is
unspecified; vcod sorts stably, archived first, which is one of the orders
retail can produce.

### Font slots

VERIFIED, the constants `0x3001f120` loads per slot: `font` 0 takes a text scale of `fontScale * 0.25`
(`0x3006950c` reads 0.25) and a height from the engine's font-height trap
(`0x35`); `font` 1 and 2 take `fontScale * 0.33333334` (`0x30069548`) and the
immediates 16 and 16 (slot 1) or 16 and 8 (slot 2). INFERRED: those are the
`default`, `bigfixed` and `smallfixed` names the script's `font` field takes
(`cod11-gsc-object-model.md`, "HUD element fields"), and the 16s are the fixed
cell's height in virtual pixels. VERIFIED from the pak listing: `pak5.pk3`
ships only `fonts/fontImage_{12,16,18,24,30,32}`, no fixed-width atlas. vcod
draws the two fixed slots with a loaded font at retail's `fontScale / 3`, and
reads the default slot's height as its tallest glyph at the element's scale.

### What each type prints

VERIFIED, the strings and constants: a non-zero `label` (`+0x2c`) is looked up
as configstring `0x4dc + n` through the localize call tagged `"hudelem
string"`; the value format is `"%g"` (`0x30064b28`); the timer formatters are
`0x3001ece0` (`"%i:%02i"`, `"%i:%02i:%02i"`) and `0x3001ed60` (`"%i:%02i.%i"`,
`"%i:%02i:%02i.%i"`). INFERRED, off the type switch in `0x3001f120`, the
element-setup function: type 1 prints its `text` (`+0x68`) configstring,
type 2 prints `value` (`+0x64`) through `"%g"`, types 4 and 5 go through the
whole-second formatter and 6 and 7 through the tenths one, each fed by
`0x3001ec70` (section 5, "There is no round-timer configstring"), and types 3,
8 and 9 print nothing and take their width from `0x3001ee20`.

INFERRED, off `0x3001f090`'s loop: when both the label and the text are
non-empty they are merged into one string, the label copied up to a `%s`,
the text in its place, then the rest of the label; a label with no `%s`
prefixes the text.

### Size, position and colour

INFERRED, off `0x3001ee20` and `0x3001ee90`: a shader element's width is its
`width` (`+0x30`), or the font height when that is 0, and while
`0 < scaleTime` (`+0x48`) and `cg.time - scaleStartTime` (`+0x44`) is below it
the width runs linearly from `fromWidth` (`+0x3c`, again the font height when
0) to that; height the same off `+0x34` and `+0x40`. VERIFIED: the stock S&D
progress bar is `setShader("white", 0, 8)` then `scaleOverTime(planttime,
barsize, 8)` (`maps/MP/gametypes/sd.gsc` in `pak5.pk3`). INFERRED, from the
two together: that bar grows from the font height, not from 0.

VERIFIED: `0x3001ef50` reads the dword at `+0x10` and the float at `+0x28`
of the structure `ecx` points at, and `0x3001f120` passes it `esi`
(`0x3001f2d9`), the setup record it fills. INFERRED, off `0x3001ef50`'s
branches: it returns the font height (`+0x28`) in place of the element's
height when that dword is non-zero and the element's height is not above
it; that height is what the box is placed by. VERIFIED: `0x3001f120` stores
the label string's pointer at the record's `+0x10`, the localize result at
`0x3001f1f0` or the empty string `0x3006193c` at `0x3001f1f7`, and
`0x3001f090` writes `0x3006193c` there again after a merge. INFERRED, from
those stores: that pointer is never null, so the floor applies to every
element, whatever its `font`. INFERRED, off `0x3001f6f0` (type 3) and
`0x3001f520` (types 8 and 9): the shader is drawn at the unfloored
`0x3001ee90` height `h`, at the record's `y` plus `(H - h) * 0.5` for
`alignY` 1 and `H - h` for 2, where `H` is the floored height, which cancels
the floor. INFERRED: the floored height therefore only moves the label
(`0x3001f490` places its text inside `H` by the font height the same way),
and a shader is aligned by its own height.

INFERRED, off `0x3001efb0` and `0x3001f020`: the position is `x` (`+0x4`) and
`y` (`+0x8`), moved from `fromX` (`+0x4c`) and `fromY` (`+0x50`) the same way
over `moveTime` (`+0x58`) from `moveStartTime` (`+0x54`); `alignX` (`+0x14`) 1
then subtracts half the element's width and 2 all of it, and `alignY`
(`+0x18`) does the same with the height. VERIFIED: the half is `0x3006930c`,
which reads 0.5.

INFERRED, off the tail of `0x3001f120`: while `0 < fadeTime`
(`+0x28`) and `cg.time - fadeStartTime` (`+0x24`) is below it, each byte lane
runs linearly from `fromColor` (`+0x20`) to `color` (`+0x1c`), and the result
is scaled by `0x30069420` to 0..1. VERIFIED: that constant reads 1/255.
Lane `+0x1c` is red and `+0x1f` alpha, as the server's getters read them
(`cod11-gsc-object-model.md`, "HUD element fields").

None of the three tweens clamps a start time ahead of `cg.time`; the
fraction goes negative and extrapolates past the `from` value. INFERRED.
vcod clamps the fraction to 0..1.

### The two clocks

VERIFIED: `0x3001f520` registers the element's material and a second one
named after it with `"Needle"` (`0x30064b20`) appended, and scales the
element's timer value by 360 over `duration` (`+0x60`), or by `0x300695d0`
(1.0922667, `65536 / 60000`) when `duration` is 0, before `ANGLE2SHORT`.
INFERRED: the face is drawn at the element's rect and the needle over it,
turned one revolution per `duration` ms, or per minute without one. Which way
the needle turns on screen is not measured.

---

## 9. The native player HUD

The layout file is VERIFIED: pak0 `ui_mp/hud.menu` and the `COMPASS_*`
defines in `ui_mp/menudef.h`. What each ownerdraw does is read out of
`cgame_mp_x86.dll` (1.1), labelled per claim below. Rects are virtual 640x480,
menu origin plus item offset.

| Item | Rect | Art | Ownerdraw |
|---|---|---|---|
| cursor hint | 300, 325, 40x40 | per hint | `CG_CURSORHINT` 72 |
| stance | 100, 434.375, 40x40 | `hudStance{Stand,Crouch,Prone}` | `CG_PLAYER_STANCE` 20 |
| weapon name back | 242.5, 431, 320x20 | `gfx/hud/hud@weaponnameback.tga` | 82 |
| ammo back | 557.5, 421.625, 80x40 | `gfx/hud/hud@ammocounterback.tga` | 6 |
| weapon name | 242.5, 446, 320x30, textscale .3 | | 81 |
| ammo text | 570, 444.625, 55x40, textscale .21 | | `CG_PLAYER_AMMO_VALUE` 5 |
| health back | 501, 460, 130x12 | `gfx/hud/hud@health_back.tga` | `CG_DRAW_SHADER` |
| health bar | 502, 461, 128x10, forecolor 0.7 0.4 0 | `gfx/hud/hud@health_bar.tga` | 89 |
| health cross | 488, 460, 12x12 | `gfx/hud/hud@health_cross.tga` | `CG_DRAW_SHADER` |
| compass back, face | -25, 345, 160x160 | `hud@compassback`, `hud@compassface` | 84 |
| compass highlight | -25, 345, 160x160 | `hud@compasshighlight` | 85 |
| compass needle | 35, 395, 40x40 | `hud@compass_arrow` | 85 |
| objective pointers | -25, 345, 160x160 | | 86 |

VERIFIED, the dispatch tables: the ownerdraw switch subtracts 4 from the id at
`0x30026bdf` and indexes the byte table at `0x300270f0` and then the jump
table at `0x3002706c`; for ids 5, 20, 72, 81, 82, 84, 85, 86 and 89 those
tables land on `0x30026c1e`, `0x30026d2a`, `0x30026c36`, `0x30026edb`,
`0x30026f00`, `0x30026f3e`, `0x30026f5b`, `0x30026f78` and `0x30026f95`,
whose calls are `0x30025ab0`, `0x30023f50`, `0x300251f0`, `0x30023c30`,
`0x30023d50`, `0x30024800`, `0x30025120`, `0x30024d20` and `0x300248c0`.
INFERRED: those are each id's handler, which is how the claims below are
attributed.

VERIFIED, the defaults in the cgame's cvar table: `cg_hudDamageIconTime` 2000,
`cg_hudDamageIconWidth` 128, `cg_hudDamageIconHeight` 64,
`cg_hudDamageIconOffset` 32, `cg_hudCompassSize` 1.0,
`cg_hudCompassMaxRange` 1024, `cg_hudCompassMinRange` 0,
`cg_hudCompassMinRadius` 0, `cg_hudObjectiveMaxHeight` 70,
`cg_hudObjectiveMinHeight` -70, `cg_hudObjectiveMinAlpha` 1,
`cg_cursorHints` 3, `cg_crosshairAlpha` 1.0, `cg_crosshairAlphaMin` 0.7 and
`cg_crosshairDynamic` 0 (the entries sit at `0x30074aa4`..`0x30074c54`, name
then default string; each is a `{vmCvar_t *, name, default, flags}` record,
so `cg_crosshairAlpha`'s value is at `0x301db788` and
`cg_crosshairAlphaMin`'s at `0x301df208`).
vcod uses these values as constants.

VERIFIED, the weapon-file fields the HUD reads, from the cgame's weapon field
table (`{name, offset, type}` records around `0x30075558`): `displayName`
`+0x8`, `modeName` `+0x6c`, `reticleCenter` `+0xe8`, `reticleSide` `+0xec`,
`reticleCenterSize` `+0xf0`, `reticleSideSize` `+0xf4`, `reticleMinOfs`
`+0xf8`, `hudIcon` `+0x18c`, `modeIcon` `+0x190`, `ammoIcon` `+0x194`,
`hipSpreadStandMin` `+0x23c`, `hipSpreadDuckedMin` `+0x240`,
`hipSpreadProneMin` `+0x244`, `hipSpreadMax` `+0x248`, `hipReticleSidePos`
`+0x264`, `clipOnly` `+0x2d4`, `wideListIcon` `+0x2d8`, `adsAimPitch`
`+0x344`, `adsCrosshairInFrac` `+0x348`, `adsCrosshairOutFrac` `+0x34c`.

### Crosshair

INFERRED, off `0x30016760` and `0x3000fa50`: the hip spread in degrees is
`min + (max - min) * aimSpreadScale / 255`, `min` being the prone minimum when
`pm_flags & 1`, the ducked one when `& 2` and the standing one otherwise, and
it is multiplied by a sight shrink factor. Each arm then sits
`spread * 640 / fov_x` virtual pixels off the centre horizontally and
`spread * 480 / fov_y` vertically, floored at `reticleMinOfs`. VERIFIED: the
two numerators are `0x300694fc` (640.0) and `0x3006973c` (480.0), and the fov
is an `fpatan` times the double at `0x300693c8`, which reads 114.59.
INFERRED: that is `360 / pi`, so the fov is in degrees.

INFERRED, same function: with the sight fraction `f` above 0, `t = f - (1 -
X)` for `X` = `adsCrosshairInFrac` or `adsCrosshairOutFrac` (picked by a flag
this reading did not name), and when `t > 0` the shrink is `1 - 0.5 * t / X`;
the arms' travel and both images' sizes take it. At `f` 1 nothing is drawn.
INFERRED: `adsAimPitch` drops the reticle by `480 / fov_y * adsAimPitch * t`
in the same arm; stock MP files do not set it and vcod leaves it out.

INFERRED, off the four-arm loop: the arm table is top, right, bottom, left,
direction `(0,-1) (1,0) (0,1) (-1,0)`, corner offset in arm sizes
`(-0.5,-1) (0,-0.5) (-0.5,0) (-1,-0.5)`, a one-pixel nudge `(0,-1)` on the top
arm and `(-1,0)` on the left, pulled in by `hipReticleSidePos` arm sizes; the
bottom and left arms draw the image flipped (`t` 1 to 0) and the right and
left ones turned 90 degrees. The arm and centre sizes are passed to the draw
call without the screen scale the travel gets, so they are window pixels.
Which way the 90-degree turn goes is not measured.

VERIFIED: `0x30016760` multiplies
`1 - [0x30207534] * 0x30069420` (1/255) by the return of `0x30015fe0` and by
`cg_crosshairAlpha` (`0x301db788`), and compares the product with
`cg_crosshairAlphaMin` (`0x301df208`) at `0x30016bb3`. VERIFIED: the snapshot
interpolation writes `0x30207534` from the float at `+0x3e4` of the two
snapshots it lerps between. INFERRED: that is the playerstate's
`aimSpreadScale` (`+0x3d8`, `docs/protocol-1.1.md`) behind the snapshot's
12-byte header, lerped. INFERRED, off the compare: the arms' alpha is
`max((1 - aimSpreadScale / 255) * cg_crosshairAlpha, cg_crosshairAlphaMin)`,
so at the defaults they fade from 1 to 0.7, reached at `aimSpreadScale` 76.5;
the centre image takes `cg_crosshairAlpha` times the same return, unfloored.
INFERRED: `0x30015fe0` returns 1 whenever its first call (`0x30015f20`)
returns 0, and its other branch draws the sight overlay; vcod takes the
return as 1 and does not draw the overlay. vcod reads `aimSpreadScale` off
the prediction or the newest snapshot, not lerped.

### Health

INFERRED, off `0x300248c0`: the share is `stats[0] / stats[2]` clamped to
0..1, and 0 when either is 0; the bar is cropped, texture `s` 0 to the share,
not squeezed. Above half the forecolor's red and blue are scaled by
`2 * (1 - share)`, at or below half its green becomes `(share + 0.2) * green +
0.3`. INFERRED: a red tail from the share to the share last shown holds one
frame, then drains by `0x30069490` per ms, and resets when the playerstate's
client number changes. VERIFIED, the constants: `0x300693d4` reads 0.2,
`0x30069454` 0.3 and `0x30069490` 0.0012.

### Weapon name and ammo

INFERRED, off `0x30023c30` and `0x30023d50`: the name is the localized
`displayName`, or `"%s / %s"` with the localized `modeName` when that is not
blank, right-aligned 28 units in from the rect's right edge; the backdrop is
the text's width plus 36 wide, right-aligned on the same edge. VERIFIED:
`0x30069400` reads 28.0 and `0x30069584` 36.0.

INFERRED, off `0x30025ab0` and `0x3000f920`: a weapon that is not `clipOnly`
prints `ammoclip[clipIndex]` through `"%2i"` at the rect's left, `"|"`
centred, and the reserve through `"%i"` right-aligned; a `clipOnly` weapon
prints its clip alone, centred. Both counts cap at 999. VERIFIED, the three
strings: `0x3006526c`, `0x30063148` and `0x3006567c`. INFERRED, off
`0x3000f920`: a weapon with a shared ammo cap sums the cap's weapons the
player holds instead. vcod prints the weapon's own.

### Stance

INFERRED, off `0x30023f50`: prone on bit 1 and crouch on bit 2 of a stance
word, standing otherwise, and `hudStanceFlash` drawn over it for a second
after a change. vcod reads the stance from `eFlags` (`0x40` prone, `0x20`
crouch) and leaves the flash out.

### Compass

VERIFIED: `hud.menu` lists the compass items back (84), highlight (85),
face (84), needle (85), friendlies, objective pointers (86). INFERRED: a menu
draws its items in file order, so the highlight is under the face.

INFERRED, off `0x30024800` and `0x30019bd0`: the back and face turn by the view
yaw less `northyaw` (configstring 11), smoothed by a spring; the highlight and
the needle do not turn (`0x30025120`). vcod turns the face without the spring.

INFERRED, off `0x30024d20`: only objectives in state 4 are drawn; the target
is the objective's origin, or its entity's when `entNum` is not `0x3ff`; the
20x20 icon's centre is `(55 - sin a * r, 425 - cos a * r)` for the bearing
`a` off the view yaw, with `r = 43.75 * clamp(distance / 1024)` at the cvar
defaults above. VERIFIED: `0x300695fc` reads 43.75 and `0x30069448` 20.0.
INFERRED: the icon is the objective's configstring `1500 + icon` material
with its extension dropped and `_up` or `_down` appended when the target is
more than 70 units above or below the view. VERIFIED: `0x30024c40` holds the
three suffix strings `""`, `"_up"` and `"_down"`. Which suffix goes with which
side, and that `a` grows to the left, are INFERRED from the geometry, not
measured.

### Cursor hint

VERIFIED, the registrations: `hintActivate`, `hintNoActivate`, `hintDoor`,
`hintNoDoor`, `hintMg42`, `hintHealth`, `hintLadder` and `hintFriendly` are
stored at `0x301d5ad8` to `0x301d5af4`, four bytes apart, and the weapon
setup stores a material at `0x301d5af4 + 4 * weapon` and another at
`0x301d5bf4 + 4 * weapon`. INFERRED, off the weapon setup's branches: the
first is the weapon's `hudIcon` and the second its `ammoIcon`, each
`hintActivate` when the key is blank. INFERRED, off
`0x300251f0` indexing that run from `0x301d5ad0` by the hint: hint 2..8 are
the named ones in order, 9 is `hintFriendly`, `9 + w` weapon `w`'s hud icon
and `73 + w` its ammo icon, which agrees with the server's encoding
(`cod11-items.md` 2.3); 0 and 1 draw nothing. VERIFIED: `hintNoDoor` is not
in pak4's `scripts/hud.shader`, which has `hintDoorLocked` instead. INFERRED:
that hint has no art in retail either.

INFERRED, same function: with `cg_cursorHints` 3 the icon's alpha pulses as
`(sin(cg.time * 0.0066667) + 1) / 2`; a `wideListIcon` weapon's icon is drawn
twice as wide, shifted left by half the rect. A hint string (configstring
`1212 + serverCursorHintString`, `cod11-gsc-object-model.md`) is localized,
its `[%s]` filled with the use key's binding, and printed at textscale .21
centred on the rect with its baseline one text height above it. VERIFIED:
`0x300696d4` reads 0.0066667 and `0x30063188` is `"[%s]"`. The weapon hints'
own composed sentence is not modelled.

### Damage direction

VERIFIED: `0x3002faa0` and `0x30028d00` call `0x300287f0` with the
playerstate's `damageYaw`, `damagePitch` and `damageCount`, and each call site
compares `damageEvent` with the previous playerstate's and `damageCount` with
0. INFERRED, off those two compares: a changed event with a non-zero count is
a hit, which is the increment the server makes (`cod11-combat.md`).

INFERRED, off `0x300287f0`: yaw and pitch both 255 add no icon; otherwise the
oldest of eight slots takes the time and `damageYaw / 255 * 360` plus a random
jitter of up to 10 degrees either way. VERIFIED: the jitter's constants are
`0x30069448` (20.0) and `0x30069330` (32768.0). vcod leaves the jitter out.

INFERRED, off `0x300172f0`: each slot younger than `cg_hudDamageIconTime` draws
`hudHitDirection` over `(-64, 32)` to `(64, 96)` about the screen centre,
turned by the view yaw less the slot's yaw, with alpha
`min(1, 2 - 2 * age / time)`. So an icon below the crosshair is a hit from
behind. VERIFIED: the centre is `0x300695e4` (320.0) and `0x300695e0`
(240.0). The sense of the turn is inferred from that geometry, not measured.

---

## UNVERIFIED summary

| # | Claim | Check that settles it |
|---|---|---|
| 1 | `killIcon<Mod>` material names bind to `gfx/hud/death_<mod>.dds` (section 2). No stock pk3 entry carries the name; the mapping is by name correspondence only. | Run the stock 1.1 MP client, take a melee death, screenshot the killfeed. |
| 2 | Scores token 4 is the Q3 `pers.enterTime` slot; unit unknown, and the field has no write site in the stock server module (section 3). | Log a real `b` line from a `score` request and compare token 4 against a known join time. |
| 3 | Resolved: the `b` grammar. The 2026-08-24 live sweep held the Tab scoreboard against a populated server and confirmed it on screen (section 3). No raw wire dump was kept, so token 4's unit (item 2) is still open. | n/a |
| 4 | Resolved: the hudelem transport is the playerstate's array block 5, filled per client by `HudElem_UpdateClient` (section 5, "How a hudelem reaches the client"), and what the cgame draws from it is section 8. | n/a |
| 5 | The second font header float's id-internal field name (section 6). Its use is settled. | Nothing downstream depends on it. |
| 6 | Whether the stock client's own status header shows the same CS-5/6-vs-`b` transient disagreement observed live (section 5). vcod's header prefers the `b` reply once one has arrived. | Compare the stock client's header text to its own Tab scoreboard in the same frame against a server showing a fresh score change; not yet done. |
| 7 | Trap `4`'s official name (section 1). Its nine-argument signature is settled from the call site. | Decompile the engine's cgame syscall dispatcher and read case 4. |
