# CoD 1.1 HUD protocol: obituaries, scores, status configstrings, fonts

Everything below is read out of CoD 1.1 binaries, stock assets, or a live
capture against a running 1.1 server. Claims not settled that way are marked
`UNVERIFIED` with the check that would settle them.

As in `docs/research/cod11-events-and-fx.md`, the client authority is
`main/cgame_mp_x86.dll`, not the root `cgamex86.dll` (the single-player
module, whose event enum diverges above id 172).

### Evidence sources

All binaries are from the 1.1 install unless stated otherwise.

| File | md5 | Used for |
|---|---|---|
| `cgame_mp_x86.dll` | `4912169a9eb22b404f95c52863a5feb6` | `CG_Obituary`, `CG_ServerCommand`, scores parser, configstring consumers |
| `CoDMP.exe` | `753fbcabd0fdda7f7dad3dbb29c3c008` | font `.dat` loader, text renderer, `^N` colour table |
| `game.mp.i386.so` (1.1d Linux server's game module) | `de8947beb6f86fbfb46f5adfaab3d3ed` | who writes each configstring, obituary event builder, scoreboard message builder; it has a symbol table, so its function names are its own |
| `pak5.pk3` | `0cb20baa66ddecc72ccb7f17b3062bb3` | `fonts/fontImage_*.dat`, `gfx/hud/*death*` art |
| `pak0.pk3` | | `weapons/mp/*` weapon defs, `ui/assets/reticle_q.tga` |
| `pak4.pk3` | | `scripts/ui_hud.shader`, `scripts/hud.shader` |
| `cgame_mp_x86.dll` from the 1.5 install | `075e2af18aeaf2aeaf2a75ce22db683a` | 1.5 diff (icon names unchanged) |
| live: a public TDM server, mp_carentan, 2026-08-24 | | `serverCommand` stream, gamestate |

Addresses are virtual: base `0x30000000` for the cgame DLL, `0x00400000`
for `CoDMP.exe`, file-relative for the Linux `.so`. The cgame DLL and
`CoDMP.exe` facts come from Ghidra decompilations (`tools/re/ExportDecomp.java`);
the `.so` was read with `objdump -d -M intel` and `readelf -r`.

Recurring cgame trap numbers (syscall pointer `0x30074898`):

| trap | call |
|---|---|
| `4` | add kill message |
| `9` | `Cvar_Set` |
| `0xb` | `Cvar_VariableStringBuffer` |
| `0xc`, `0xd` | `Argc`, `Argv` |
| `0x18` | `SendClientCommand` |
| `0x4f` | `GetGameState` |
| `0x30` | register material (`RE_RegisterShader`, mipmapped; tail of the loading-screen wrapper `0x30030b70` the precache calls go through) |
| `0x58` | `RE_RegisterShaderNoMip`: scoreboard status icons, levelshots, script `precacheShader` materials (`cod11-front-end.md` section 14, "2D registrations") |

---

## 0. CoD 1.1 server commands are single letters

`CG_ServerCommand` @ `0x3002e0d0` switches on `argv0[0]` and never compares
the whole token, so every 1.1 MP server command is one character. Grepping a
capture for `scores` or `cs` finds nothing.

| Char | Handler | Meaning |
|---|---|---|
| `a` | `0x30037f20` | takes one int arg |
| `b` | `0x3002b920` | scoreboard (section 3) |
| `c` | | "announcement message", the bold message window: `c "<message>" 2` from `announcement` / `clientAnnouncement` (docs/research/cod11-gametypes-re-bel.md 3, cod11-chat.md 3) |
| `d` | `0x3002c6b0` | configstring update: `d <index> <string>` |
| `e`, `f` | | "game message", the game message window (cod11-chat.md 3.3) |
| `g` | | "bold game message": `game_message` sound, then as `c` |
| `h` | | "chat message", from `G_Say` (cod11-chat.md) |
| `i` | | "team chat message", from `G_Say` |
| `j`/`k`/`l` | `0x3002d940(0/1/2)` | three variants of one handler |
| `m` | | int arg, arms a 20 s (or 10 s if negative) timer |
| `n` | `0x3002ca60` | |
| `o`, `p`, `q` | | traps `0xc0`/`0xd4`, `0xd5`, `0x30031fc0` -> `0xd7` |
| `r` | `0x3002df30` | `CG_ReverbCmd` |
| `s` | `0x3002e040` | `CG_LocalSound` |
| `t` | `0x3002dae0` | open script menu: `t <script menu index>` (section 0.1) |
| `u` | `0x3002de60` | close script menus: the bare letter, no argument |
| `v` | `0x30030550` | set client cvar: `v <name> "<value>"` |

From the live capture (`serverCommand N:` lines):

```
serverCommand 3: f "vcod ^7Connected"
serverCommand 4: d 0 \codextended\CoDExtended v20\...\g_gametype\tdm\...\mapname\mp_carentan\...
serverCommand 5: v g_scriptMainMenu "team_americangerman"
serverCommand 8: t 0
serverCommand 13: d 5 118
serverCommand 14: d 6 131
serverCommand 29: i "^1Server:^7 <advert text>"
```

`d` carries the new configstring text as argv[1]; the cgame ignores it and
re-reads the applied table via `trap_GetGameState` (`0x4f`), as Q3's
`CG_ConfigStringModified` does.

`v` sets the client's own cvar, the one the console, HUD and config read;
there is no separate store for server-set cvars. VERIFIED: 1.1's `v` arm
copies argv[1] and passes it with argv[2] to `0x30030550`, trap 9; UO's arm
calls `0x3003d570`, also trap 9. VERIFIED: CoDMP.exe's cgame trap
dispatcher (`0x401e22`, table `0x4041f0`) sends trap 9 to `0x402331`, which
calls `Cvar_Set2` (`0x439650`) with `eax` 1, the `force` path: no
read-only, cheat or latch check, and an unregistered name is made with no
flags (cod11-console.md 5). INFERRED that UO's engine does the same (no
CoDUOMP.exe trap table read). INFERRED, off Q3's `cvar_modifiedFlags`: a `v`
of a userinfo cvar sends a userinfo change, one of an archived cvar rewrites
the config. The 140/204 mirror's writer (`0x3002bc60`, section 5) is a trap
9 too. vcod keeps the mirror apart (`play::cvars::ClientCvars`) and lets a
name the server set with `v` this connection win over it.

### 0.1 The script menu handshake

Measured 2026-08-31 against the retail 1.1d server, dm on mp_pavlov, with
`--net-probe --save-playerstate`; the stream is in
`crates/server/tests/fixtures/playerstate/mp_pavlov-dm.txt`. A client that
has entered the world (its first usercmd after the gamestate, not any
command it sends; `docs/protocol-1.1.md`, "Entering the world") gets these
five commands, in order (`^U` is a literal 0x15 byte):

```
f "MPSCRIPT_CONNECTED^Uvcod^7"
v g_scriptMainMenu "team_russiangerman"
v scr_showweapontab "0"
t 0
v cg_objectiveText "DM_KILL_OTHER_PLAYERS^U"
```

INFERRED that the five come from `Callback_PlayerConnect`: its `waittill`
blocks on `begin`, which the engine raises from `ClientBegin` inside
`SV_ClientEnterWorld`, and the `setClientCvar` and `openMenu` calls after
that wait in the stock dm script account for four of them.

`t`'s argument indexes the script-menu configstring range, slot 0 at cs
1180: cs 1180 is `team_russiangerman`, cs 1181 `weapon_russian`. Answering
the team menu produces `v g_scriptMainMenu "weapon_russian"`, then `t 1`.

VERIFIED: `game.mp.i386.so` holds the three formats, `t %i` at `0x731f8`,
`v %s "%s"` at `0x73300`, and the lone `u` the `closeMenu` builtin
(`0x45574`) passes to `trap_SendServerCommand`, at `0x73204`. Hence the bare
name, the quoted value, and no argument on `u`. VERIFIED: the capture
matches the first two shapes. INFERRED, off which value reaches which
argument in the instruction stream: `t` is the wire form of `openMenu`
(`0x453f4`) and `v` of `setClientCvar` (`0x446e0`), and `openMenu`'s `%i` is
`GScr_GetScriptMenuIndex(name)`'s return, so the name never reaches the wire
through `t`.

INFERRED (a branch and its position; no capture holds a quote):
`setClientCvar` rewrites a `"` in the value as `'` before formatting it
(`0x447b2`), so the value cannot close its own quoted argument.

VERIFIED: `GScr_GetScriptMenuIndex` (`0x5c73c`) reads its candidates from
configstrings `0x49c + i` (1180), and the not-found path formats the script
error `Menu '%s' was not precached` (`0x771f2`). INFERRED, from the loop's
compare-and-branch: `i` runs `0..=31` and the index returned is the first
slot whose text matches.

The client answers with `mr <serverId> <menu index> <response>`, quoting the
index `t` named. Answering `t 0` with `allies` and the following `t 1` with
a weapon `maps\mp\gametypes\_teams::restrict` allows for that nationality
spawns the player; `--save-playerstate` does that.

`Cmd_MenuResponse_f` (`0x486d8`) turns the command into a two-argument
`notify(player, "menuresponse", menu, response)`.

VERIFIED by cross-check that the first argument is the menu's name, not the
index: retail answers `mr <sid> 0 allies` with the weapon menu, and `dm.gsc`
(`pak5.pk3`) can only get there by comparing that argument against
`game["menu_team"]` at `:245`, which `:142` builds as `"team_" +
game["allies"] + game["axis"]`; `:340` hands the same argument back to
`openMenu`, and `:1102` raises the notify itself with a menu name there. An
index would match nothing. INFERRED that the mechanism is a read of
configstring `0x49c + index` into the buffer the notify argument comes from
(`0x48790`).

The rest of the handler is INFERRED off the disassembly:

- an argument count other than 4 sends the notify with an empty menu name
  and response `"bad"`, without reading the serverId (`0x486ed`);
- argv[1] is compared with `sv_serverId`, and the handler returns without
  notifying when they differ (`0x4874a`);
- an index outside `0..=31` skips the configstring read, leaving argv[2]'s
  own digits as the menu argument (`0x4877c`).

---

## 1. `EV_OBITUARY` field mapping

`CG_EntityEvent` @ `0x3001dc10`, `case 0xc9` (201 = `EV_OBITUARY`)
tail-calls `CG_Obituary` @ `0x3001d6c0` with the `entityState_t *` in `EAX`.

Its first three reads are the ints at entityState `0x74`, `0x78` and `0xa0`.
It range-checks the first against `0..63` and aborts with `"CG_Obituary:
target out of range"` (`0x30020750`), which names `+0x74` the victim:

| entityState offset | Q3/CoD field | vcod netfield (`crates/common/src/net/cod1/fields_v1.rs`) | bits | Carries |
|---|---|---|---|---|
| 116 (`0x74`) | `otherEntityNum` | `otherEntityNum` | 10 | victim clientNum (0..63) |
| 120 (`0x78`) | `otherEntityNum2` | `attackerEntityNum` | 10 | attacker clientNum, or `1022` |
| 160 (`0xa0`) | `eventParm` | `eventParm` | 8 | means of death or weapon, section 2 |

The offsets match CoDExtended's `src/shared.h:643-654` (`otherEntityNum
//116`, `otherEntityNum2 //120`, `eventParm //160`).

The server side is the GSC `obituary()` builtin at `.so` `0x5a750` (the only
`push 0xc9` in the module). It calls `G_TempEntity` with event `0xc9`
(`0x5a78d`), stores script argument 0 (victim entity number) at `+0x74`
(`0x5a7aa`), and stores argument 1 at `+0x78` only when its type is 7
(entity) with subtype `0xd` (player) (`0x5a7bd`, `0x5a7cf`, `0x5a7e0`);
else `+0x78` gets `0x3fe`, `ENTITYNUM_WORLD` = 1022 (`0x5a7e5`). It sets
`r.svFlags` (`+0xf4`) to `8`, `SVF_BROADCAST` (`0x5a7ec`).

So:

- `attackerEntityNum == 1022` is no player attacker. `CG_Obituary` forces
  any attacker outside `0..63` to `0x3fe`, empties the attacker name and
  draws only the victim.
- `victim == attacker` is a suicide; the attacker name is blanked too.
- The event is `SVF_BROADCAST`, so a spectator sees every obituary
  regardless of PVS.

Names and colours come from the cgame's clientinfo array at `0x3018bc0c`,
stride `0x448`:

| Offset in clientinfo | Read as |
|---|---|
| `+0x00` | `infoValid` (row skipped if 0) |
| `+0x0c` | `name`, `strncpy(..., 0x1f)` |
| `+0x2c` | `team` |
| `+0x30` | `score` (written by the scores parser, section 3) |

Both names get a literal `"^7"` appended (string `0x30065348`, a 2-byte
store plus NUL). Each name's colour comes from the team-colour helper at
`0x3002a810`, `(team, vec3 *out)`:

```
team == 1 -> Cvar_VariableStringBuffer("g_TeamColor_Axis",   buf, 1024)
team == 2 -> Cvar_VariableStringBuffer("g_TeamColor_Allies", buf, 1024)
otherwise -> {1,1,1}
             then sscanf(buf, "%f %f %f", &out[0], &out[1], &out[2])
```

Team 1 is Axis, team 2 Allies (cross-checked against the server in section
5). Teams 0 and 3 render white.

The draw is trap `4` with nine arguments: attacker name, attacker colour,
victim name, victim colour, icon name, icon width (`1.4` or `2.8`), a
constant `1.4f` (`0x3fb33333`), and a third colour. The row reads attacker,
icon, victim; the three colour vec4s default to white, alpha 1; the icon
goes by material name, not handle.

If the local client is the victim, the attacker's name is copied to
`0x3020c044` (the "killed by" name). If it is the attacker, a centre print
comes from one of two localized formats:

- `"CGAME_YOUKILLED\x15%s"` (`0x30064c3c`) for a normal kill;
- `"CGAME_YOUKILLED\x15^1%%s^7 %s\x14%s"` + `"CGAME_TEAMMATE"`
  (`0x30064c50`) when the attacker's team is non-zero and equals the
  victim's.

### The centre print

Read with `objdump -d -M intel`, 1.1 `cgame_mp_x86.dll` unless named.

- VERIFIED: `CG_Obituary` compares the attacker with `cg.snap->ps.clientNum`
  (`[0x301e2160] + 0xb8`, `0x3001d985`), so "local" is the drawn
  playerstate: a 1.1 or 1.5 spectator following the killer sees the line.
  INFERRED, off the branch at `0x3001d968`: a suicide and a world attacker
  (`0x3fe`) never print.
- VERIFIED: the `%s` is the victim's clientinfo name, cut to 31 bytes
  (`strncpy(..., 0x1f)`, `0x3001d861`) plus `"^7"`. The format goes through
  `va` (`0x3003eb40`), then `CG_PriorityCenterPrint` (`0x300159e0`) with
  priority 1 (`EAX`), y 400.0 (`0x43c80000`) and character width 9.6
  (`0x4119999a`), pushed at `0x3001d9c4`..`0x3001d9d4`.
- VERIFIED, `localized_english_pak1.pk3`
  `localizedstrings/english/cgame.str`: `YOUKILLED` is `"You killed %s"`,
  `TEAMMATE` `"TEAMMATE"`; 1.5's `localized_english_pak5.pk3` and UO's
  `localized_english_pakuo01.pk3` carry the same. So a kill reads `You
  killed Bob`, a team kill `You killed ^1TEAMMATE^7 Bob`.
- VERIFIED: `0x300159e0` returns without printing while a print is up
  (`[0x3020c488]` non-zero) with a priority (`[0x3020c898]`) above the new
  one. It passes the text through `0x300229b0` with `"Center Print"`
  (`0x30065308`), the `[{command}]` pass over a localized message, and
  copies at most 1023 bytes to `0x3020c494`. INFERRED, off its loop at
  `0x30015a40`: once a line has run 75 (`0x4b`) characters its next space
  becomes `\n`. VERIFIED: it stores `cg.time` (`[0x30207148]`) plus 2000
  (`0x7d0`) as the start (`0x3020c488`), y and the character width through
  `_ftol` (`0x3020c490`, `0x3020c48c`), and counts lines from 1, one per
  `\n` and one per literal `\` `n` pair (`0x3020c894`).
- VERIFIED: `CG_DrawCenterString` (`0x30015b40`) fades with `CG_FadeColor`
  (`0x30019a30`) over `cg_centertime * 1000` (vmCvar `0x301dbf60`, default
  `"5"`, flags `0x200`, row at `.data` `0x30074d60`). `CG_FadeColor`
  returns nothing once `cg.time - start` reaches the total and fades over
  the last 100 ms (`0x64`, times 0.01) in white times `cg_hudAlpha`. With
  the 2000 added to the start, the line holds 7 s in all.
- VERIFIED: the draw takes font 0 at the character width times 0.03125
  (`0x30069504`; 0.28125 for 9), reads the height off trap `0x35` as an
  int, and starts at `y - height * 0.5 * lines - height` (`0x3006930c`
  0.5), each line `height * 1.2` (`0x30069500`) below the last. Each line
  is at most 75 bytes, its width trap `0x34` (an int), at x `(640 - width)
  * 0.5` (`0x300694fc`), drawn by trap `0x36` with style 3. INFERRED: y is
  the baseline, as for every trap `0x36` draw (section 8).
- VERIFIED: `CG_Draw2D` (`0x30018810`) returns for `pm_type` 5 before the
  print, and calls `CG_DrawCenterString` (`0x300188ed`) only when
  `0x3002b650` returns 0. INFERRED: `0x3002b650` is `CG_DrawScoreboard` (it
  fades and clears the killed-by name at `0x3002b681`), so the print hides
  under the scoreboard.
- VERIFIED: nothing prints for the victim. The killed-by name feeds only
  owner draw 50 (`CG_KILLER` in `ui_mp/menudef.h`, `"Fragged by %s"` at
  `0x300630b8`, read at `0x30022701`), and no stock `.menu` in `pak*.pk3`
  uses `CG_KILLER`.
- VERIFIED, 1.5 `cgame_mp_x86.dll`: the same print at `0x3001ec44`
  (`CG_PriorityCenterPrint` `0x30016a70`), y 400, width 9.6, the same draw
  constants (`0x3006b6dc` 1000, `0x3006b774` 0.03125, `0x3006b590` 0.5,
  `0x3006b770` 1.2, `0x3006b76c` 640), the 75 wrap, the 2000 and
  `cg_centertime` `"5"`.
- VERIFIED, UO `uo_cgame_mp_x86.dll`: the print at `0x3002259e`
  (`0x30019050`) sits at y 390.0 (`0x43c30000`), and `CG_Obituary` skips it
  when the snapshot's `pm_flags` (`[0x30459160] + 0x18`, playerstate +12)
  carry `0x40000`, UO's follow bit (`0x3002255e`): a UO follower sees no
  line. Draw constants, wrap, 2000 and `cg_centertime` `"5"` match 1.1.

vcod: `crates/client/src/hud/centerprint.rs` and `obituary_center_print` in
`hud/mod.rs`. It keeps no priority, since the obituary is the only caller.

### 1.1 Where the kill line goes

Read with `objdump -d -M intel` of 1.1 `CoDMP.exe` and the decompilations
of 1.5 `CoDMP.exe` and UO `CoDUOMP.exe`.

- VERIFIED: trap 4 is case 4 of the cgame syscall switch (`0x401e22`, jump
  table `0x4041f0`, entry `0x402132`). It reads its nine arguments in
  section 1's order and writes them into the console line buffer as one
  line (`0x409590` writes text, `0x409490` a colour).
- VERIFIED, the line: when the attacker name's first byte is non-zero,
  three colour cells 10, 11, 12 with the attacker colour's r, g, b times
  255 (`0x4021ca`, `0x568ec0` is 255.0), the name at colour 7, then `" "`
  (`0x568a9c`); then cells 13, 14, 15 with the third colour (`0x4021f3`);
  a cell `0x1000 | _ftol(width * 32)` and a cell `0x1100 | _ftol(1.4 *
  32)` (`0x568ee4` is 32.0, `0x402201`, `0x402212`); the icon's material
  name at colour `0x12`; `" "`; cells 10..12 with the victim colour; the
  victim name at colour 7.
- VERIFIED: `0x409590` with a colour of 0 or more skips every `^0`..`^7`
  pair without changing colour and drops `\n` and `\r`. So each name draws
  whole in its team colour, `CG_Obituary`'s own `^7` gone.
- VERIFIED: the line ends through `0x408d00` with `edx` 1 (`0x4022bf`),
  whose switch (`0x408c90`, case 1) adds it to the window record at
  `0x142efcc`, which trap 0x1a draws (`0x409e70`): the game message window
  of `cod11-chat.md` 3.3, lower left above the compass, x 6, font 0, scale
  0.25. Its life is `con_gamemessagetime` times 1000 (`0x402188`), the same
  5 s as an `iPrintLn`, and it takes a ring slot like any game message.
- VERIFIED: 1.5 `CoDMP.exe` has the same case (`0x4022b5` writes the
  `0x1100` cell, the line ends through `0x4094e0` with `edx` 1). UO
  `CoDUOMP.exe`'s case 4 writes the `0x1100` cell at 0x4022e3 and ends the
  line through 0x409b40 with `edx` 1 (0x402337), the game window. UO's
  `CG_Obituary` calls trap 4 with the same nine arguments and adds a MOD
  `0x1a` row, `gfx/icons/hud@bino_owned`.
- VERIFIED, the draw (refexport slot `0xdc`, `0x4dfa60`; the refexport is
  copied at `0x411ac5` from the table `0x4b4a30` fills at `0xca26c8`): the
  line splits at the first cell of colour 13, `0x10`, `0x11` or `0x12`
  (`0x4df5b0`). Colours 10..12 set the running text colour's rgb (times
  1/255, `0x568ec4`); alpha stays the window's. Each text run draws at `x +
  advance` and adds its int width (`0x4df310`). The icon cells
  (`0x4df750`) give width and height as `scale * 48 * code * 0.03125`
  (`0x568ed4`, `0x568ea0`) and the colour from 13..15. The icon draws at `x
  + advance`, top `baseline - (scale * 38.4 + h) * 0.5` (`0x5690cc`,
  `0x568e70`), and adds its width to the advance.
- INFERRED, from those codes at the game window's scale 0.25: an icon is
  16.5 units square, a wide one 33.375 by 16.5, centred 4.8 above the
  baseline. Nothing pads the icon; the spaces either side are the line's.
- VERIFIED: `CL_InitCGame` (after `"CL_InitCGame: %5.2f seconds\n"`)
  empties the game, bold and third message windows, so a new gamestate
  drops every kill line.
- VERIFIED: `CL_GetServerCommand`'s `n` case (1.1 `CoDMP.exe` 0x4017c3,
  index 10 of the byte table at 0x401824 off `'d'`, jump table 0x401808)
  calls 0x4087b0, which zeroes the rings of the game window (0x142efcc) and
  window 2 (0x142f050, the bold window; trap 3, 0x401fc2, writes it), then
  zeroes window 3 (0x142f0d4) itself, before the cgame sees the command.
  The switch at 0x408c90 maps window 0 to 0x142f5a8, 1 to 0x142efcc, 2 to
  0x142f050, 3 to 0x142f0d4. VERIFIED: 1.5's (`CoDMP.exe` 0x401620, calling
  0x408ce0) and UO's (`CoDUOMP.exe` 0x401630, calling 0x409340) send `B` and
  `n` to the same case. So a `map_restart` empties the game and bold
  windows, kill lines included. vcod: `Hud::on_map_restart`.

vcod: `Hud::on_game_event` builds a `messages::KillLine` and pushes it into
the game message window; `messages::kill_line` draws it. There is no
separate kill feed.

---

## 2. The icon rule

`eventParm` is 8 bits on the wire. `CG_Obituary` tests the byte's sign:

- Bit `0x80` set: the low 7 bits are a means-of-death value, and the icon
  comes from the switch below.
- Bit `0x80` clear: the value indexes `weaponDefs[]`. The `killIcon` string
  at weaponDef `+0x2e0` is the icon name when non-empty, and the
  `wideKillIcon` int at `+0x2e4` selects the 2.8 width. The switch then runs
  with 0 and falls through its default, keeping the weapon icon. An empty
  `killIcon` falls back to `killIconDied`.

The two offsets come from the weapon-file parse table at `0x30075c84`
(`{keyword, structOffset, type}` triples):

```
0x30066424 "killIcon"      offset 0x2e0  type 0 (string)
0x30066414 "wideKillIcon"  offset 0x2e4  type 5 (int)
```

The stock files agree: `pak0.pk3:weapons/mp/thompson_mp` has
`killIcon\gfx/hud/hud@death_thompson.tga`, `wideKillIcon\1`;
`weapons/mp/colt_mp` has `killIcon\gfx/hud/hud@death_colt45.tga`,
`wideKillIcon\0`. The art matches: `hud@death_thompson.dds` is 11088 bytes,
`hud@death_colt45.dds` 5616, half.

`iconWidth` defaults to `1.4f` (`0x30069764`) and becomes `2.8f`
(`0x40333333`) only for wide weapon icons. The MOD branch never sets it, so
every MOD icon is 1.4 wide.

### Means-of-death enum

From the pointer table at `.so` file offset `0x7cda0`, 25 entries:

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

The server tags seven. The `obituary()` builtin at `.so` `0x5a7f6` takes
the tag path for mod 7 or 8 (a `mod - 7 <= 1` unsigned compare) and for mods
22, 21, 19, 16 and 17 (five equality compares), ORs `0x80` into the mod
(`0x5a817`) and stores it as `eventParm` at `+0xa0` (`0x5a81a`). Every other
mod stores the weapon index there (`0x5a822`). `{7, 8, 16, 17, 19, 21, 22}`
is the client switch's case set, so the two halves agree.

### The table

| `eventParm` | Means of death | Icon name the client uses | Stock art |
|---|---|---|---|
| `0x87` | `MOD_MELEE` (7) | `killIconMelee` | `gfx/hud/death_melee.dds` |
| `0x88` | `MOD_HEAD_SHOT` (8) | `killIconHeadShot` | `gfx/hud/death_headshot.dds` |
| `0x90` | `MOD_WATER` (16) | `killIconDrown` | `gfx/hud/death_died.dds` |
| `0x91` | `MOD_SLIME` (17) | `killIconSlime` | `gfx/hud/death_died.dds` |
| `0x93` | `MOD_CRUSH` (19) | `killIconCrush` | `gfx/hud/death_crush.dds` |
| `0x95` | `MOD_FALLING` (21) | `killIconFalling` | `gfx/hud/death_falling.dds` |
| `0x96` | `MOD_SUICIDE` (22) | `killIconSuicide` | `gfx/hud/death_suicide.dds` |
| `0x97` | `MOD_TRIGGER_HURT` (23) | `killIconDied` | `gfx/hud/death_died.dds` |
| `0x00`..`0x7f` | weapon index | `weaponDefs[parm]->killIcon`, e.g. `gfx/hud/hud@death_mp40.tga` | ships per weapon |
| `0x00`..`0x7f` with empty `killIcon` | weapon index | `killIconDied` | `gfx/hud/death_died.dds` |
| any other `0x80`-flagged value | | `killIconDied` (switch default) | `gfx/hud/death_died.dds` |

- `0x97` has a client case, but the stock server never sets bit 7 for mod
  23. A mod could.
- `MOD_LAVA` (18) has neither a server flag nor a client case.
- The eight `killIcon*` names are precached at cgame init
  (`0x30020e9f`..`0x30020ee5`, each `RegisterMaterial(name, 5)`, handle
  discarded), then passed to trap `4` by name.

VERIFIED, the name-to-image binding: each name is a shader in
`pak4.pk3:scripts/hud.shader` (1.5's `pak8.pk3` has the same blocks) whose
one stage is `map clamp gfx/hud/death_<name>.tga`, `blendFunc blend`, vertex
colour and alpha. `killIconDrown`, `killIconSlime` and `killIconDied` all
map `gfx/hud/death_died.tga`, so a drown or slime death draws the generic
icon. VERIFIED: 1.5's switch (0x3001e9f5) has 1.1's eight cases. UO's is in
`coduo-diff.md` 12.

vcod: `killfeed::mod_icon` names each case's material, per protocol, and
the HUD resolves it through the shader scripts' image map like any HUD
material.

---

## 3. The `scores` server command

Letter `b`, built by `DeathmatchScoreboardMessage` (`.so` `0x459c0`), parsed
by `0x3002b920`.

### Grammar

```
b <numRows> <axisScore> <alliesScore>{ <client> <score> <ping> <time> <statusIcon>}*
```

Server formats (`.so` `0x737f0`, `0x737e0`):

```
"b %i %i %i%s"          outer
" %i %i %i %i %i"       one row, appended numRows times
```

The row push order at `0x45a65`..`0x45a78` (right to left on the stack) is
`sortedClients[i]`, `cl[+0x20e0]`, `ping`, `cl[+0x20e4]`, `cl[+0x20d8]`.

The client reads them with `Argv(4 + 5*i + k)` into a six-int row
(`0x3020ba30`, stride `0x18`):

| Token | Client slot | Meaning |
|---|---|---|
| 1 | `row[0]` | clientNum; clamped to 0 outside `0..63` |
| 2 | `row[1]` | score (`cl[+0x20e0]`, the `score` script field, 8416); also copied into `clientinfo[client].score` (`0x3018bc3c`) |
| 3 | `row[2]` | ping (`cl[+0x20c8]`); `-1` when `cl[+0x20ec] == 1` (connecting), else clamped to `999` |
| 4 | `row[3]` | deaths: `cl[+0x20e4]`, the `deaths` script field (8420 in the client field table, `cod11-gsc-object-model.md`), which every stock `Callback_PlayerKilled` increments |
| 5 | `row[5]` | status icon; in `1..8` the client resolves configstring `20 + n` to a material handle |

`row[4]` is not on the wire. The client fills it with
`clientinfo[client].team` and accumulates per-team row counts
(`0x3020ba20[team]`) and average ping (`0x3020ba10[team] /= count`), so the
team blocks come from clientinfo, not the wire.

Header tokens:

- `numRows` is `min(level.numConnectedClients, 64)`; the client clamps to
  `0x40` again.
- Token 2 is `level.teamScores[1]` (Axis), token 3 `level.teamScores[2]`
  (Allies) (section 5). The client stores them at `0x3020ba04` /
  `0x3020ba08`; `-9999` means no score and renders `"-"` (`0x300630c8`).

### Spectators

Spectators are ordinary rows. `DeathmatchScoreboardMessage` walks
`level.sortedClients[]` and emits every entry. The sort comparator (`.so`
`0x500ab`) sorts `cl[+0x217c] == 3` (the spectator team) last, but nothing
filters them out. The client puts them in team bucket 3, which the
team-colour helper `0x3002a810` renders white. Server-side banner strings
`g_ScoresBanner_Allies` / `_Axis` / `_None` / `_Spectators` cover the four
buckets.

### Live example

The first 90 s probe capture, before vcod sent `score`, logged 49
`serverCommand` lines (42 `d`, 3 `v`, 2 `f`, 1 `i`, 1 `t`) and no `b`: the
server sends `b` only on request or at intermission (section 4).

The 2026-08-24 live sweep held the Tab scoreboard open on a populated
server and showed the grammar end to end: Axis/Allies sections with team
icons and totals, score-sorted rows, a Spectators section, `hud unk 0`
throughout.

VERIFIED, the comparator's reads and compares (`.so` 0x50099-0x50109), in
order: `cl[+0x20ec] == 1` on either side (a connecting client sorts last);
`cl[+0x217c] == 3` on either side (a spectator after a non-spectator, two
spectators by `gclient_t` address, which is slot order); `cl[+0x20e0]`
(score), greater first; `cl[+0x20e4]` (deaths), fewer first; 0 on a full
tie. VERIFIED, the retail round-restart target capture
(`crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-target.txt`,
`b 2 0 1 1 0 0 0 0 0 -1 1 1 0`): the 0-score row leads the -1-score one
though its slot is higher.

VERIFIED: token 4 is `deaths`. `cl[+0x20e4]` has no write site in the stock
module's `.text` (`grep 0x20e4` finds three reads, no store) because the
script writes it: the client field table registers `deaths` at 8420 =
`0x20e4` and `score` at 8416 = `0x20e0` (`cod11-gsc-object-model.md`, the
client field dump). INFERRED: `SortRanks`' ascending tiebreak after score is
"fewer deaths ranks higher", not Q3's `pers.enterTime`. A retail client
shows it in the deaths column (a hand check saw minutes there when vcod
sent the Q3 reading).

VERIFIED: retail's `b` line carries `0` in both team slots of both hit
captures (`crates/server/tests/fixtures/playerstate/mp_carentan-dm-hit-target.txt`
and the `tdm` pair, `cod11-combat.md` section 9). INFERRED:
`level.teamScores[]` starts zeroed, and `-9999` is a value a gametype writes
for itself, not the default of an unwritten total.

**As implemented** (`GameHost::scoreboard`, `crates/server/src/game/scoreboard.rs`):
one row per online client in `SortRanks`' order (connecting last, then
spectators by `sessionteam`, score descending, deaths ascending, slot). The
score, deaths and status icon come from the client's `.score`, `.deaths` and
`.statusicon` script fields, the icon resolved to its 1-based slot in
`CsRange::StatusIcon`. The team totals are the level's `setTeamScore`
values, axis then allies. Ping is the link's `cl->ping`, `-1` while
connecting, capped at 999. With no script running every row is zero
(`server.rs` `scoreboard`).

### 3.1 How the cgame draws the board

Addresses are 1.1 `cgame_mp_x86.dll`; constants are floats in its `.rdata`.
Coordinates are the 640x480 virtual screen. Text goes through trap `0x36` in
font 0 at a baseline, width through `0x34`, font height through `0x35`;
colour codes in names pass through.

**Entry and gates.**

- VERIFIED, `0x3002b650`: returns 0 without drawing when `0x3029944c` is
  set. With `+scores` held (`0x3020c030`) the alpha is 1, else
  `0x30019a30(100, 0x3020c038)`, the 100 ms fade from the stamp `-scores`
  writes.
- VERIFIED, `-scores` (`0x30013cc0`) and the `n` handler (`0x3002ca60`):
  when held, clear `0x3020c030` and stamp `0x3020c038` with `cg.time`.
- VERIFIED, `+scores` (`0x30013ce0`): when more than 2000 ms passed since
  the last request (`0x3020b9f8`) it sends `score` and restamps, and if the
  board was down also zeroes the row count (`0x3020b9fc`) and the scroll
  position (`0x3020c03c`). Either way it sets `0x3020c030`. A press inside
  the 2 s sends nothing.
- VERIFIED: the draw sends `score` every 2000 ms (section 4), calls the
  frame `0x30029ac0` and the table `0x3002b410`, and returns 1.
- VERIFIED, `0x30018530`: the intermission draw (`pm_type` 5) restamps
  `0x3020c038` every frame and calls `0x3002b650`, so the board is up all
  intermission without the key.
- VERIFIED, the 2D pass `0x30018810`'s two calls of `0x3002b650`: while it
  returns 1 the cgame skips `0x30018550` (the spectator's `+attack` /
  `+melee` hints), `0x30018710` (`"spectator follow string"`), `0x30015b40`
  (centre print), `0x30017880` and `0x30017940` (game and bold message
  windows, `cod11-chat.md` 3.3) and the trap `0x1c` and `0x1d` draws
  (`0x300179f0`, `0x30017a80`). INFERRED: chat (trap `0x1e`) stays, drawn
  at `LAB_30018913` outside both gates.

**Frame.** VERIFIED, `0x30029ac0`: `black` at alpha `0.55 * a`
(`0x300696f8`) over (120, 22, 400, 430) (`0x30069690`, `0x30069560`,
`0x30069528`, `0x300696f4`); `white` at `0.1 * a` (`0x3006953c`) over the six
rects of the table at `0x30060d88`: (123, 25, 394, 2), (123, 447, 394, 2),
(123, 27, 2, 420), (515, 27, 2, 420), (125, 51, 390, 1), (125, 432, 390, 1).

- VERIFIED, title row: the game type's name (trap `0x7e` on `g_gametype`,
  localized through trap `0x39` `"scoreboard gametype display"`) at x 129,
  scale 0.41 (`0x3ed1eb85`), baseline `51 - (24 - h) * 0.5` (`0x300696f0`,
  `0x3006930c`), `h` trap `0x35`'s height; then the map (`mapname` without
  `maps/`, `mp/`, `.bsp`) right-aligned to end at `511 - 4` (`0x300696e8`).
  The map's scale starts at 0.41 and drops by 0.025 (`0x300696ec`) while
  the two widths plus 4 pass 386 (`0x182`) and the scale is above 0.075
  (`0x30069648`); the game type keeps 0.41. INFERRED: trap `0x7e` returns
  the game type's display key, the first token of
  `maps/mp/gametypes/<g_gametype>.txt` that `UI_LoadGameTypes` reads
  (`TDM_TEAM_DEATHMATCH`). vcod uses that list and falls back to the raw
  `g_gametype`.
- VERIFIED, footer: `sv_hostname` (`0x301d1ffc`, filled by
  `CG_ParseServerinfo` `0x3002bbb0`) at x 129 and trap `0x7f`'s string
  right-aligned to `511 - 4`, both at a scale from 0.2 (`0x3e4ccccd`) down
  by 0.01 (`0x300693f4`) on the same 386 rule, baseline `447 - (14 - h) *
  0.5` (`0x300696e4`). A trap `0x7f` string equal to `"0.0.0.0:0"`
  (`0x30062e84`) is replaced by `CGAME_LISTENSERVER` ("Listen Server").
  INFERRED: trap `0x7f` is the server's `ip:port`. vcod prints the address
  its socket is connected to.

**Objective.** VERIFIED, `0x3002a1e0` with y 52 (`0x42500000`): nothing when
`cg_objectiveText` (`0x301db670`) is empty. Else y += 4; the localized text
(trap `0x39` `"scoreboard objective info"`) is cut greedily at the last
space before a line reaches 375 (`0x177`) units at scale 0.24
(`0x3e75c28f`), each line at x 129, baseline `y + 9` (`0x30069760`), 12
apart (`0x300693fc`); a `\n` or literal `\` `n` breaks a line, and a space
opening a line is skipped. Then y += 2, `white` at `0.1 * a` over (125, y,
390, 1), and y + 1 is returned. INFERRED: the levelshot and loading bar
between the text and the rule draw only while `0x301e2160` is 0 (before
the first snapshot), never over a live board.

**Columns.** VERIFIED, the five 16-byte rows at `0x30060d38` (kind, share of
the table's width, header key, alignment 2 = right):

| Kind | Shows | Share | Header | Align |
|---|---|---|---|---|
| 4 | status icon | 0.05 | | left |
| 0 | name | 0.41 | | left |
| 1 | score | 0.16 | `CGAME_SB_SCORE` | right |
| 2 | deaths | 0.20 | `CGAME_SB_DEATHS` | right |
| 3 | ping | 0.18 | `CGAME_SB_PING` | right |

VERIFIED, `0x3002a650` at `y + 4` (`0x300693dc`): headers with a key at
scale 0.3 (`0x3e99999a`), baseline +10.5 (`0x300695b8`), right-aligned in
their column, white; returns y + 14 (`0x300696e0`). Columns start at x 129,
each `share * width` wide.

**Width.** VERIFIED, `0x3002b410` and `0x3002a740`: the board asks 10
(`0x300693e4`), or 66 (`0x30069404`) when axis or allies has a row, plus 28
(`0x30069400`) for each of the none and spectator blocks with one, plus 12 a
row. The table is 382 wide (`0x43bf0000`) when that fits between the
objective's bottom and 432 (`0x300693f8`), else 374 (`0x43bb0000`) with the
scroll bar.

**Order.** VERIFIED, `0x3002b410`: when axis or allies has a row, the leader
by the `b` totals (axis `0x3020ba04`, allies `0x3020ba08`) goes first, the
other second, a tie keeping the last frame's first (`0x3007497c`, 2 = allies
in the image); each block is followed by 4. Then team 0's block if it has a
row, then the spectators'. VERIFIED, `0x3002b920`: a row's team is its
client's if the client has info (`0x3018bc0c`), else 0; each team's average
ping is its rows' ping sum over its row count, 0 when either is under 1.
VERIFIED, `0x3002b0d0`: rows draw in the `b` line's order, so the server's
sort is the board's; a row whose client has no info is skipped.

**Banner.** VERIFIED, `0x3002a970`: the block stops at `y + 24 > 432`
(`0x3006940c`, `0x300693f8`). The banner is the material in
`g_ScoresBanner_None`, `_Axis`, `_Allies` or `_Spectators`, white at `a`
over (129, y, width, 32). Its title sits at x 133 (`0x43050000`), baseline
`y + 18` (`0x300694d8`), scale 0.32 (`0x3ea3d70a`), in the team's colour
(`0x3002a810`: `g_TeamColor_Axis` / `_Allies` read `"%f %f %f"`, clamped to
0..1, white for teams 0 and 3). For team 0 it is `CGAME_SB_PLAYER` (count 1,
`0x30062df4`) or `CGAME_SB_PLAYERS` with the count; for the others `"%s
(%s)"` of `g_TeamName_Axis` / `_Allies` (localized) or `CGAME_SPECTATORS`
and the count. Axis and allies also print the team's `b` total and average
ping (`"%i"`) in the score and ping columns, same baseline, scale and
colour. Rows start at `y + 24`.

**Row.** VERIFIED, `0x3002ace0`: stops at `y + 12 > 432`. The local client's
row (`0x301e2144`, `CG_Init`'s client number, not a followed player's) gets
`white` in the team colour at `0.2 * a` (`0x300693d4`) over (129, y, width,
12); otherwise every second drawn row of a block gets `black` at `0.15 * a`
(`0x300695c0`), the first plain. The icon is 13x13 (`0x41500000`) at the
column's x, white. Text columns draw at scale 0.22 (`0x3e6147ae`), baseline
`y + 10.5`, white at `a`, cut a character at a time from the end until the
text fits. A spectator's (team 3) score and deaths are blank. Returns y +
12. VERIFIED, `0x3002b920`: a status icon 1..8 names configstring `20 + n`
(UO 1..16, `21 + n`), registered with trap `0x58`.

**Scrolling.** VERIFIED: every banner and row is one item. Items under the
scroll position (`0x3020c03c`) are skipped without moving y; the first item
that does not fit sets `0x3020c040` and nothing after it draws.

- VERIFIED, `0x30027180`: while `+scores` is held, a key-down of 206, 164
  or 184 (`MWHEELUP`, `PGUP`, `KP_PGUP`) lowers the position by
  `cg_scoreboardScrollStep` (record `0x30075110`, default `"3"`, archived)
  to no less than 0; 205, 163 or 190 (the downs) raise it through
  `0x3002b720`, only when `0x3020c040` is set, to at most the row count
  minus 1. Both are eaten. VERIFIED, `0x3002b410`: after the draw the
  position is held to the item count minus 1.
- VERIFIED, `0x3002b170` (list top `t`, item count `n`, items reached `r`,
  position `s`): `black` at `0.5 * a` (`0x3006930c`) over (505, t, 8, `432
  - t - 1`); a thumb of `white` at `0.25 * a` (`0x3006950c`), x 506
  (`0x300695b0`), 6 wide (`0x30069314`), from `t + 1 + s / n * k` where `k`
  is the track height less 2, `k * (r - s) / n` tall when `r - s > 1` and
  `n > 1`, else `k`. INFERRED: `k` reads the height left on the x87 stack by
  the track's draw. VERIFIED: with `s > 0` the 16x16 (`0x41800000`)
  `hudScoreboardScroll_UpArrow` at (521, t) and `_UpKey` at (521, t + 18);
  with `r < n` `_DownArrow` at (521, 415) and `_DownKey` at (521, 397)
  (`0x44024000`, `0x43cf8000`, `0x43c68000`).

**1.5 and United Offensive.** VERIFIED, 1.5 `cgame_mp_x86.dll`
(`0x3002af40`..`0x3002cbe0`) and UO `uo_cgame_mp_x86.dll`
(`0x300361d0`..`0x30037e90`): the same functions, constants, column table
(`0x30062d38`, `0x30071a60`) and frame rects (`0x30062d88`, `0x30071ab0`);
the traps renumber (UO's register is `0x59`, its game type and address
`0x80` and `0x81`). Both add an entry gate (1.5 `0x3002cae0`, UO
`0x30037d90`): no board while the `timescale` vmCvar's integer is 0
(records `0x30076ed0`, `0x30085770`), a 1.5 match timeout
(`cod15-protocol-diff.md`). UO's status icons run 1..16 (`0x30038060`).

**As implemented** (`crates/client/src/hud/scoreboard.rs`): all of the
above, in vcod's centred 640x480 space rather than retail's stretched one.
`cg_hudAlpha` is not applied to the fade. Not drawn: the loading levelshot
and bar in the objective step.

---

## 4. How `scores` is requested

The cgame registers `+scores`, `-scores` and `score` (the only score-ish
strings in the DLL).

The scoreboard draw at `0x3002b650` re-requests while the board is up: it
keeps a last-request stamp at `0x3020b9f8`, and once `cg.time` passes it by
more than 2000 ms it resets the stamp and issues trap `0x18`
(`trap_SendClientCommand`) with `"score"`. So the client sends the reliable
command `score` (no `cmd` prefix at this layer) every 2000 ms while the
board draws.

Server side (`ClientCommand`, `.so` `0x488e3`): `Q_stricmp(argv0, "score")
== 0` calls `DeathmatchScoreboardMessage(ent)`, one of two callers. The
other is `SendScoreboardMessageToAllIntermissionClients` (`.so`
`0x50bf0`), gated three ways:

```
if (level[+0x20c] == 0) return;                 // dirty flag, set by setteamscore
for each connected client:
    if (cl[+0x20ec] != 2) continue;             // must be CON_CONNECTED
    if (ps[+0x4]  != 5)   continue;             // pm_type == PM_INTERMISSION
    DeathmatchScoreboardMessage(client);
level[+0x20c] = 0;
```

There is no periodic push during play. A spectator wanting a live board
sends `score` itself on the same 2 s cadence, as the stock client does.

---

## 5. Status configstrings in 1.1 MP

Every index below is a `trap_SetConfigstring` call site in the stock server
module, recovered by resolving the `R_386_PC32` relocations against
`trap_SetConfigstring` and reading the immediate pushed two slots earlier.
Client consumers are `CG_ConfigStringModified` @ `0x3002c6b0` and the init
path `0x3002c060`.

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
| 15 | `Cmd_CallVote_f`, `CheckVote` | vote time (ms): 1.1's end in level time, 1.5's ms left (`cod11-votes.md`) | `0x301d21c0`, drives the `(t - cg.time)/1000` countdown in `0x30018120` |
| 16 | `Cmd_CallVote_f` | vote string | `0x301d21d0`, 255 bytes |
| 17 | `Cmd_CallVote_f`, `Cmd_Vote_f` | vote yes count | `0x301d21c4` |
| 18 | `Cmd_CallVote_f`, `Cmd_Vote_f` | vote no count | `0x301d21c8` |
| 20 | `G_InitGame` | match-state infostring; `Info_SetValueForKey(cs, "winner", "0")` | not handled by the cgame |
| 21..28 | script | player status icons (max 8) | material per index; the scores command's 5th token indexes here |
| 29..43 | script | player head icons | material per index |
| 44+ | `target_location_linkup` | location names | |
| 108..139 | `G_TagIndex` | attachment tag names | |
| 140..203 / 204..267 | the engine, not the game module (`cod11-map-cycle.md` 3.2 reads the writer at `0x808b148` in `cod_lnxded`), so no `trap_SetConfigstring` index here | 64 pairs of (cvar name, cvar value) | `0x3002bc60` does `Cvar_Set(cs[140+i], cs[204+i])` for i < 64, stopping at the first empty name |
| 268..523 | `G_ModelIndex` | xmodel paths | |
| 524..779 | `G_SoundAliasIndex` | sound aliases | |
| 780..843 | `G_EffectIndex` | `.efx` paths | |
| 1100..1115 | `G_ShellShockIndex` | shellshock scripts | |
| 1180..1211 | menus | precached menus | |
| 1244+ | `G_LocalizedStringIndex` | localized strings; hudelem strings are `1244 + n` | |
| 1500+ | `G_ShaderIndex` | materials | |

This matches the block map in `crates/common/src/net/cod1/mod.rs`, built
from a live gamestate dump on 2026-08-23. The 2026-08-24 live probe reported
`354 configstrings` on mp_carentan; `d 5 <n>` / `d 6 <n>` arrived all
through the TDM round, both climbing (5: 117 to 129, 6: 126 to 153 over 90
s). All 42 `d` commands in that window went to 5 (13), 6 (28) or 0 (once).

VERIFIED, `CG_ConfigStringModified` @ `0x3002c6b0`: index 8 jumps to
`0x30036150` (`0x3002c70a`), the gamestate's item walk, so a mid-map CS 8
update registers the newly marked items' weapons (through `0x30036080` to
`CG_RegisterWeapon` `0x30034cf0`). INFERRED, from the compares at
`0x3002c70a`-`0x3002c927`: CS 7 has no case, so the weapon table is read
only at the gamestate. VERIFIED: the cgame's weapon setup (`0x3000ef10`)
parses configstring 7 from its gamestate copy (string data `0x301ce0a0`,
offset slot `0x301cc0bc`) when syscall 199 says the weapon memory is not
loaded. It is called from the gamestate init (`0x3002310b`) and from
`CG_MapRestart` (`0x3002ca91`), the latter only behind a non-zero
`sv_running` (`0x301d1fd8`, stored from the cvar at `0x30020652`) after
syscall 200 releases the memory. INFERRED: a client of a remote server
builds its weapon table once per gamestate, and a mid-map `d 7` changes
nothing until the next load.

vcod builds `LivePhase::weapons` at the gamestate and ignores a
`ConfigstringChanged(7)`. It reruns its viewmodel prewarm on each CS 8
update (`prewarm_viewmodels`, `crates/client/src/main.rs`); cached rigs cost
a lookup, and weapon files, models and clips are shared by name across rigs
(`viewmodel::RigCache`).

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
`Scr_AllocString("allies")` and `scr_const+8` with `"axis"`. So CS 6 is
allies, CS 5 axis, `level.teamScores[1]` axis and `level.teamScores[2]`
allies, matching the cgame's `g_TeamColor_Axis` (team 1) /
`g_TeamColor_Allies` (team 2) split in section 1.
`DeathmatchScoreboardMessage` pushes `level[+0x1fc]` before `level[+0x200]`,
so `b`'s token 2 is axis and token 3 allies, the CS 5, CS 6 order.

### Header vs. scoreboard totals can disagree in the same frame

A live capture (2026-08-24, a public TDM server, mp_chateau) caught vcod's
status header, read off CS 5/6, at `219 tdm 214` while the same frame's `b`
reply carried axis 221 / allies 231. Both are legitimate wire values: a `d`
for 5/6 and the next `b` (or the client's own `score` re-request, section 4)
do not land in the same tick, so the two can read a few points apart.

vcod's header prefers the latest `b` reply's totals once the scoreboard has
one (`Scoreboard::totals`, `status::read_status`'s `scoreboard_totals`),
falling back to CS 5/6 before any `b`. A `b` reply is requested and answered
in one round trip, while CS 5/6 wait on a `d` push, so `b` is the more
current. The header is vcod's own, drawn for spectators only (`hud/mod.rs`);
retail draws team totals only on the scoreboard (section 3.1).

### There is no round-timer configstring

Nothing writes a match or round clock to a configstring, and the cgame's
clock formatters read none. `0x3001ece0` (`"%i:%02i"` / `"%i:%02i:%02i"`)
and `0x3001ed60` (tenths) both take their value from `0x3001ec70`, which
reads `hudelem[0x17]`, an absolute millisecond timestamp, and picks a
direction from `hudelem[0]`:

| `hudelem[0]` | Value |
|---|---|
| 4 | `(t - cg.time) + 999`, count down, whole seconds |
| 5, 7, 9 | `cg.time - t`, count up |
| 6 | `(t - cg.time) + 99`, count down, tenths |
| 8 | `t - cg.time` |

The same struct carries `hudelem[0xb]` (a localized-string index, read as
configstring `0x4dc + n` = 1244 + n) and `hudelem[0x19]` (a numeric value).
The round clock is a hudelem the gametype's GSC creates. The live serverinfo
has no `timelimit` key to fall back on:

```
\g_gametype\tdm\gamename\Call of Duty\...\mapname\mp_carentan\...\sv_maxclients\64\...
```

vcod draws the clock as the hudelem it is (`hud/hudelem.rs`). Its spectator
header (`hud/status.rs`) shows elapsed time since level start
(`server_time - cs[13]`).

### How a hudelem reaches the client

Hudelems travel in the playerstate, in the two 31-entry arrays of array
block 5, past the 103 scalar fields, so `PLAYER_FIELDS` has no `hudElem*`
entry. Wire layout, field order and widths: `docs/protocol-1.1.md`, "Block
5". This is the server half.

VERIFIED, `game.mp.i386.so`: `HudElem_UpdateClient` (0x4be8c) takes a
`gclient_t`, an entity number and a two-bit selector, clears the selected
arrays (`bzero` of 0xd90 bytes at `ps+0x1338` and `ps+0x5A8`, 0x4bea7 and
0x4bec8), then walks all 0x400 `g_hudelems` records and copies 0x1c dwords
of each survivor into the next free slot. A record is skipped when its
`type` (`+0x0`) is zero, when its team (`+0x74`) is non-zero and differs
from `gclient+0x217c` (the `clientState.team` `.sessionteam` writes), or
when its owner (`+0x70`) is neither `0x3ff` nor the entity number passed.
`archived` (`+0x78`) picks `ps+0x1338` when set, `ps+0x5A8` when clear; each
array stops at 31.

VERIFIED: `G_RunFrame` (0x50a80) makes that call once per in-use client per
frame with both arrays selected, and `SpectatorClientEndFrame` (0x408bc)
with only `ps+0x5A8` right after copying the followed player's playerstate
whole. INFERRED from the two sites: `archived` marks the elements of the
playerstate a follower inherits (the community's `hud.archival` /
`hud.current`).

VERIFIED: `G_UpdateHudElemsToClients` (0x5121c) is the same loop and nothing
calls it (no relocation names it); the live path is `G_RunFrame`'s copy.

So the round clock is a `hudelem_t` in `ps+0x1338`. The connect-time frame
decoded in `docs/protocol-1.1.md` holds one: type 4, `x` 320, `y` 460, both
alignments 1, `font` 1, `fontScale` 1.0, colour `0xFFFFFFFF`, `time`
1800000, field for field `dm.gsc`'s `startGame`.

### A client's elements die with it

VERIFIED, `game.mp.i386.so` with relocations resolved
(`tools/re/annotate_func.py`): `ClientDisconnect` (0x42aac) calls
`HudElem_ClientDisconnect` (0x4c0ec) at 0x42be1 with the player's
`gentity_t`, then `Scr_PlayerDisconnect` (0x5c9ec, running
`CodeCallback_PlayerDisconnect`) at 0x42bed, then `G_FreeEntity` at
0x42bfc.

VERIFIED, `HudElem_ClientDisconnect`: a loop over all 0x400 records, 0x7c
apart, that skips `type` (`+0x0`) 0, compares the owner (`+0x70`) with the
entity's first dword (`s.number`), and on a match calls `Scr_FreeHudElem`
(0x4c11e) and stores 0 to `type` (0x4c123). INFERRED: a `newClientHudElem`
record is freed with its client; a `newHudElem` / `newTeamHudElem` one
(owner `0x3ff`) never is.

VERIFIED, `HudElem_DestroyAll` (0x4c150): the same walk without the owner
test, then a `bzero` of the whole 0x1f000-byte pool; its only caller is
`G_ShutdownGame` (relocation at 0x4ff53). INFERRED: a level boundary empties
the pool, which vcod gets by building the script runtime afresh.

VERIFIED, live, 2026-10-06, retail 1.1d on mp_pavlov with
`client-probes/probe_hud_disconnect.gsc` as the gametype and one `vcod
--net-probe` client that connected and left (`games_mp.log`):

```
  3:06 PROBE connect 0 own 1 shared 1
  3:13 PROBE disconnect_callback own 0 shared 1
  3:13 PROBE next_frame own 0 shared 1
```

The client's own element is gone when the callback runs; the shared one
survives. vcod-server prints the same three lines
(`crates/server/tests/hud.rs`,
`a_disconnect_frees_the_clients_own_elements`). INFERRED, the trap: without
the free, a stock `dm` client leaving while dead keeps its `respawntext`
(the disconnect kills `waitRespawnButton`'s threads, so
`removeRespawnText` never runs), and the next client in the slot has the
owner's entity number and is sent the element.

---

## 6. `fonts/fontImage_<size>.dat` layout

Six files ship in `pak5.pk3`, all 20552 bytes:
`fontImage_{12,16,18,24,30,32}.dat`.

The engine loader is `0x004dee00` (`"fonts/fontImage_%i.dat"` at
`0x004def0e`). It rejects any other size: the `FS_ReadFile` length must be
`0x5048` (20552). The in-memory `fontInfo_t` stride is the same `0x5048`
(the font cache walks `ptr += 0x5048`; the copy loop moves `0x1412` dwords).

Per glyph the loader byte-swaps 12 dwords and copies 8 raw, advancing the
source by `0x50`:

```
for each of 256 glyphs:
    dst[0..12]  = LittleLong(src[0..12])    // bytes 0x00..0x2f
    dst[12..20] = src[12..20]               // bytes 0x30..0x4f, shaderName
    src += 0x50; dst += 0x50
```

That is Q3's `glyphInfo_t`: 12 four-byte fields plus `char shaderName[32]` =
80 bytes, times 256 = 20480 = `0x5000`. The shader-registration pass after
it walks `p = base + 48` (shaderName) at stride `0x50` and stores the handle
at `p - 4` (offset 44).

The field types differ from Q3. Decoded from `fontImage_16.dat`:

| Offset | Type | `' '` | `'A'` | `'i'` | `'.'` | Reading |
|---|---|---|---|---|---|---|
| 0 | int | 0 | 11 | 12 | 2 | glyph height in px (= `(t2-t)*256`) |
| 4 | int | 0 | 12 | 4 | 4 | glyph width in px (= `(s2-s)*256`) |
| 8 | float | 1.0 | 12.0 | 13.0 | 3.0 | `top`: baseline to glyph top, px |
| 12 | float | 0.0 | -0.333 | +0.333 | +1.0 | horizontal bearing, scales with font size |
| 16 | float | 3.859 | 10.667 | 4.667 | 4.333 | advance / xSkip |
| 20 | int | 0 | 12 | 4 | 4 | `imageWidth` (duplicates +4) |
| 24 | int | 0 | 11 | 12 | 2 | `imageHeight` (duplicates +0) |
| 28,32,36,40 | float | | | | | `s, t, s2, t2` |
| 44 | int | 39 | 39 | 39 | 39 | glyph handle (overwritten at load) |
| 48..79 | char[32] | `fonts/fontImage_0_16.tga` | | | | shader name |

Q3's `height/top/bottom/pitch/xSkip/imageWidth/imageHeight` became `height /
width / top / bearing / advance / imageWidth / imageHeight`, three of seven
now `float`. The `s/t/s2/t2/glyph/shaderName` tail is Q3's.

`top` is `height + 1` only on glyphs that end on the baseline. VERIFIED
(1.1 `pak5.pk3` `fontImage_16.dat`): `P` height 11, top 12; `p`, `y`, `g`,
`q` height 11, top 9; `,` height 5, top 3; `Q` height 14, top 12. VERIFIED,
`CoDMP.exe`: the text renderer `0x004d7d60` draws each glyph quad at `y -
scale * glyph[+8]`, `y` the baseline it is passed, sized `imageWidth` by
`imageHeight` (+20, +24). A renderer that bottom-aligns glyphs on `height`
lifts every descender to the baseline: `p` reads `P`, `y` `Y`, `g` `8`.

The shader name is per glyph, and the bigger fonts span several atlas
pages. VERIFIED (1.1 `pak5.pk3`, the +48 names over all 256 records):

| font | pages |
|---|---|
| `fontImage_12`, `_16`, `_18` | `fonts/fontImage_0_<size>.tga` only |
| `fontImage_24` | 151/104 over `_0_24`, `_1_24` |
| `fontImage_30` | 92/98/65 over `_0_30`, `_1_30`, `_2_30` |
| `fontImage_32` | 85/86/84 over three pages; `A`..`T` on `_0_32`, `U`..`Z` and `o` on `_1_32` |

One record per font has an empty name. Drawing every glyph from page 0
garbles the 24, 30 and 32 point fonts; vcod reads the page per glyph
(`hud/font.rs`, `Font::glyph_page`).

### The extra 4 bytes

`20552 - 20480 = 72` bytes follow the glyph array; Q3's tail is `float
glyphScale` + `char name[64]` = 68. The extra 4 are a second float at 20484
(`0x5004`), between `glyphScale` and `name[64]`:

```
0x0000 .. 0x4FFF   glyphInfo_t glyphs[256]      20480
0x5000             float glyphScale                 4
0x5004             float lineHeight                 4   <-- not in Q3
0x5008 .. 0x5047   char  name[64]                  64
                                                 -----
                                                 20552 = 0x5048
```

The loader byte-swaps `file+0x5000` into `dst[0x1400]` and `file+0x5004`
into `dst[0x1401]`, block-copies 64 bytes into `dst[0x1402]`, then
overwrites them with `strncpy(dst + 20488, requestedPath, 0x3f)` plus a NUL
at `0x5047`. The shipped files' 64 bytes are all zero, so the name only ever
holds the requested path.

The text renderer at `0x004d7...` uses both floats: the glyph scale is the
caller's size times `font[0x5000]`, and `'\n'` advances `y` by the size
times `font[0x5004]`.

| File | `+0x5000` glyphScale | `+0x5004` | `+0x5004 / glyphScale` | tallest glyph |
|---|---|---|---|---|
| `fontImage_12.dat` | 4.0 | 52.0 | 13.0 | 14 |
| `fontImage_16.dat` | 3.0 | 45.0 | 15.0 | 15 |
| `fontImage_18.dat` | 3.0 | 48.0 | 16.0 | 16 |
| `fontImage_24.dat` | 2.0 | 44.0 | 22.0 | 23 |
| `fontImage_30.dat` | 1.6 | 48.0 | 30.0 | 30 |
| `fontImage_32.dat` | 1.5 | 46.5 | 31.0 | 31 |

`glyphScale` is `48 / pointSize` except for 18 (which reuses 16's 3.0), Q3's
"rendered at 48pt" convention. The second float over `glyphScale` lands
within a pixel of the tallest glyph: the line box height in design units.

UNVERIFIED: the second float's real name. Its use (newline advance, scaled
like `glyphScale`) is settled; nothing needs the name.

---

## 7. The `^N` colour palette

The engine's text renderer handles it at `0x004d7f13`, not the cgame. On a
`^` it looks at the next byte. If that is not `^` and lies in `'0'..'7'`
(`0x30..0x37`), it is consumed and the colour changes: `^7` restores the
caller's colour in full; any other code takes RGB from `colorTable[b & 7]`
(masked `0x00ffffff`) and alpha from the caller (`callerAlpha << 24`).

So only `^0`..`^7` are codes on 1.1 (`^8`, `^9`, `^a` print literally; 1.5
and UO add `^8` and `^9`, below), `^^` escapes a caret, and `^N` only ever
replaces RGB. `^7` restores all four components of the caller's colour
instead of reading `colorTable[7]`.

`colorTable` is eight RGBA dwords at `0x005405d8`, bytes R,G,B,A:

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

The float form (Q3's `colorBlack` .. `colorWhite` / `g_color_table`) is
byte-identical in all three modules: `cgame_mp_x86.dll` @ `0x30060868`,
`CoDMP.exe` @ `0x00541950`, the server module at file offset `0x79fa0`.

The chat handlers (`h`/`i`) pass every incoming string through
`0x3002df00`, which strips every `0x19` byte before display. A renderer
should do the same, and strip `^N` pairs before measuring width.

### `^8` and `^9` on 1.5 and UO

VERIFIED, the renderer: CoDMP.exe 1.5's text draw (0x4bab80..0x4babdf) and
CoDUOMP.exe's (0x4c0581) take `^` followed by `'0'..'9'` (`0x30..0x39`) as a
code. `^7` restores the caller's colour, as on 1.1; any other code goes
through 0x4ba8e0 (UO 0x4c02f0), and the caller's alpha byte is written over
the result (0x4babd3, UO 0x4c05d4).

VERIFIED, 0x4ba8e0 / 0x4c02f0: digits 0..7 read `colorTable` (1.5 0x57ed54,
UO 0x590e9c, 1.1's eight dwords), `8` the dword at 0x199a2f8 (UO 0x489a5b8),
`9` the one at 0x199a2fc (UO 0x489a5bc).

VERIFIED, those two dwords: 0x405440 (UO 0x405cb0) reads
`g_TeamColor_Allies` (string 0x5a7fe0, UO 0x58e694) into the floats at
0x1493bf0 (UO 0x49637ac) and `g_TeamColor_Axis` (0x5a7fcc, UO 0x58e680) into
0x1493c00 (UO 0x49637bc) through 0x4052f0 (UO 0x405b50), then packs Allies
into the `^8` dword and Axis into `^9`. 0x405b50 looks the cvar up
(0x43d8f0): missing or empty gives 1, 1, 1; else `sscanf("%f %f %f")`
(0x58e6a8) writes the floats it scans and leaves the rest. Each component is
clamped to 0..1 (0x5b9b3c 0.0, 0x5b9b38 1.0); alpha is 1. VERIFIED, the
caller: the client frame calls 0x405440 at 0x412762 (UO 0x405cb0 at
0x4138bf, in 0x413870), so the colours follow the cvars every frame.

INFERRED, off that caller and the renderer: the codes belong to the exe,
not the server, so a 1.5 or UO front end takes `^8`/`^9` before any game,
white while neither cvar exists. Whether a mirrored value outlives the
disconnect is not measured. vcod draws the front end's `^8`/`^9` white on a
1.5 or UO install and literal on 1.1 (`hud::font::set_front_end_colours`).

VERIFIED, the cvars' source: no cgame registers them (1.1, 1.5 and UO only
read them, `trap_Cvar_VariableStringBuffer` at cgame 0x3002a85d, 1.5
0x3002bced, UO 0x30036f66). The game module registers both, `0.5 0.5 1` and
`1 0.5 0.5`, with flag 0x800 (1.5b's table rows 70 and 71, UO's 84 and 85,
`tools/re/dump_cvars.py`), which puts them in the 140/204 mirror, so a
client in a game holds the server's. VERIFIED: CoDMP.exe 1.1 has neither
name; its renderer prints `^8`/`^9`.

VERIFIED, the writers agree: 1.5's console and message writer (0x409760, UO
0x409dc0), its length count (UO 0x470ee0) and its obituary line writer
(0x409e60, UO 0x40a4a0) take `^0`..`^9`, as do 1.5's and UO's
`CG_AddToTeamChat` (cgame 0x3002de50, UO 0x30039390; cod11-chat.md 3.2).

vcod: `hud::font::set_team_colours`, called with
`Protocol::team_colour_codes` and the client cvars at each HUD build and
with no cvars at a gamestate; `hud::font::is_colour_code`, which every text
path asks.

---

## 8. How the cgame draws a hudelem

Read out of `cgame_mp_x86.dll` (1.1). Element fields are named by their
`hudelem_t` offsets, which `docs/protocol-1.1.md` "Block 5" maps to the
wire.

### Order

VERIFIED: `0x3001f920` holds the bound 31 (`0x1f`), the element stride 112
(`0x1c` dwords) and the snapshot playerstate's `+0x1338` and `+0x5A8`, reads
each element's `type`, and calls the CRT `qsort` (`0x3004b350`) with
comparator `0x3001f8e0`. VERIFIED: the comparator loads the float at `+0x6c`
(`sort`) of both elements and returns -1, 1 or 0. INFERRED, off
`0x3001f920`'s two loops: the list is the archived array, then the current
one, each stopping at its first `type` 0, and `qsort` sorts it ascending by
`sort`. INFERRED, off `0x3001f980`'s loop: the elements draw in that order.
INFERRED: the CRT `qsort` is not stable, so equal `sort`s come in
unspecified order; vcod sorts stably, archived first, one of the orders
retail can produce.

### Font slots

VERIFIED, the constants `0x3001f120` loads: 0.25 (`0x3006950c`), the font
height trap `0x35`, 0.33333334 (`0x30069548`), and the immediates 16 and 16,
16 and 8, stored to the setup record's `+0x28` and `+0x2c`; the scale goes
to `+0x24`. INFERRED, off its switch on `font`:

| slot | text scale | height | `+0x2c` |
|---|---|---|---|
| 0 | `fontScale * 0.25` | trap `0x35` | 0 |
| 1 | `fontScale * 0.33333334` | 16 | 16 |
| 2 | `fontScale * 0.33333334` | 16 | 8 |

VERIFIED: `0x3001ec10`, the text width, multiplies trap `0x3a`'s result by
`+0x2c`, and has a second path that calls trap `0x34` with the record's
`+0x20` and `+0x24`. INFERRED, off its compare of `+0x2c` with 0: a fixed
slot takes the first path, trap `0x3a` counts characters, and each is `+0x2c`
wide, so slots 1 and 2 are 16x16 and 8x16 cells whose width ignores
`fontScale`. INFERRED: slots 0, 1, 2 are the `default`, `bigfixed` and
`smallfixed` names of the script's `font` field
(`cod11-gsc-object-model.md`, "HUD element fields"). VERIFIED (pak
listing): `pak5.pk3` ships only `fonts/fontImage_{12,16,18,24,30,32}`, no
fixed-width atlas.

VERIFIED: `0x3001f120` stores a font handle at `+0x20`: 0 for slot 0, 4 for
slot 1, 5 for slot 2; `0x3001f490` passes `+0x20`, `+0x24` and `+0x2c` to
text trap `0x36` as its third, fourth and seventh arguments. VERIFIED,
`CoDMP.exe`: trap `0x36` goes to the refexport's `0x004df570`, which calls
`0x004dfc10` with its seventh argument ninth; `0x004dfc10` stores that at
word 6 of render command 6, and the command loop passes word 6 to the text
renderer `0x004d7d60` as its eighth argument. INFERRED, off `0x004d7d60`'s
glyph loop: when that argument is not 0 (`0x00568e64`, 0.0) every glyph
advances by it and shifts right by `(cell - scale * advance) * 0.5`
(`0x00568e70`, 0.5) on top of its bearing, so a fixed slot is a proportional
font laid out one glyph per cell, centred. VERIFIED: the renderer takes its
font from the refimport hook `0x004118f0` with the handle and scale; the
hook truncates the scale times 100 (`0x00568eb0`, `_ftol2` at `0x00538be0`)
and calls the UI module's `vmMain` with command `0x10`.

VERIFIED, `ui_mp_x86.dll` (1.1, base `0x40000000`): `vmMain` (`0x400076a0`)
case `0x10` (`0x40007782`) multiplies the integer by 0.01 (`0x40030020`) and
calls `0x400079e0`. The asset parser (`0x40007d30`..`0x40007f2b`) stores
`font`, `smallFont`, `bigFont`, `extraBigFont`, `boldFont` and
`consoleFont` at `0x401c3dec`, `0x401c8e34`, `0x401cde7c`, `0x401d2ec4`,
`0x401d7f0c`, `0x401dcf54`; pak0's `ui_mp/main.menu` registers them at 16,
12, 24, 32, 30 and 18. VERIFIED, the UI's cvar records: `ui_smallFont` 0.25
(value at `0x401eee28`), `ui_bigFont` 0.4 (`0x401c1e88`), `ui_extraBigFont`
0.55 (`0x401c2428`). INFERRED, off `0x400079e0`'s compares, `s` the scale
times the float at `0x401c3dc0`:

| handle | font |
|---|---|
| 2 | `bigFont` |
| 3 | `smallFont` |
| 4 | `smallFont` for `s <= 0.25`, `font` below 0.4, `boldFont` from 0.4 |
| 5 | `consoleFont` |
| other | `smallFont` for `s <= 0.25`, `font` below 0.4, `bigFont` from 0.4, `extraBigFont` from 0.55 |

INFERRED: `0x401c3dc0` is the display context's `yscale` (screen height
over 480), from its place ahead of the asset block as in Q3's
`displayContextDef_t`.

So `smallfixed` draws `fontImage_18` in 8-unit cells, `bigfixed` picks by
drawn size in 16-unit cells, and the default slot also picks its atlas by
drawn size. vcod does all three with the screen height in place of
`yscale`, and reads the default slot's height as its tallest glyph at the
element's scale. INFERRED: trap `0x3a` counts the characters the renderer
draws, colour codes excluded; vcod measures a fixed-slot string that way.

### What each type prints

VERIFIED, strings and constants: a non-zero `label` (`+0x2c`) is looked up
as configstring `0x4dc + n` through the localize call tagged `"hudelem
string"`; the value format is `"%g"` (`0x30064b28`); the timer formatters
are `0x3001ece0` (`"%i:%02i"`, `"%i:%02i:%02i"`) and `0x3001ed60`
(`"%i:%02i.%i"`, `"%i:%02i:%02i.%i"`). INFERRED, off the type switch in the
setup `0x3001f120`:

| type | prints |
|---|---|
| 1 | its `text` (`+0x68`) configstring |
| 2 | `value` (`+0x64`) through `"%g"` |
| 4, 5 | whole-second clock |
| 6, 7 | tenths clock |
| 3, 8, 9 | nothing; width from `0x3001ee20` |

The clocks are fed by `0x3001ec70` (section 5, "There is no round-timer
configstring").

INFERRED, off `0x3001f090`'s loop: with both label and text non-empty they
merge into one string, the label up to a `%s`, the text in its place, then
the rest of the label; a label with no `%s` prefixes the text.

INFERRED, off `0x3001f7e0`: a non-empty label is drawn by `0x3001f490`
ahead of the type switch, for every type, and the position then moves right
by the label's width (`+0x14`) before the text, the shader (`0x3001f6f0`)
or the clock (`0x3001f520`). INFERRED, off `0x3001ef00` and the tail of
`0x3001f120`: the element aligns by the label's width plus the text's or
shader's. So a shader or clock with a label is laid out label first, the
pair aligned as one box; for the text types the merge empties the label, so
it rides in the text.

### Size, position and colour

INFERRED, off `0x3001ee20` and `0x3001ee90`: a shader element's width is
its `width` (`+0x30`), or the font height when 0. While `0 < scaleTime`
(`+0x48`) and `cg.time - scaleStartTime` (`+0x44`) is below it, the width
runs linearly from `fromWidth` (`+0x3c`, again the font height when 0) to
that; height likewise off `+0x34` and `+0x40`. VERIFIED: the stock S&D
progress bar is `setShader("white", 0, 8)` then `scaleOverTime(planttime,
barsize, 8)` (`maps/MP/gametypes/sd.gsc` in `pak5.pk3`). INFERRED, from the
two: that bar grows from the font height, not 0.

VERIFIED: `0x3001ef50` reads the dword at `+0x10` and the float at `+0x28`
of the structure `ecx` points at, and `0x3001f120` passes it `esi`
(`0x3001f2d9`), the setup record. INFERRED, off its branches: it returns the
font height (`+0x28`) instead of the element's height when that dword is
non-zero and the element's height is not above it; the box is placed by
that height. VERIFIED: `0x3001f120` stores the label string's pointer at
`+0x10` (the localize result at `0x3001f1f0` or the empty string
`0x3006193c` at `0x3001f1f7`), and `0x3001f090` writes `0x3006193c` there
again after a merge. INFERRED: the pointer is never null, so the floor
applies to every element, whatever its `font`. INFERRED, off `0x3001f6f0`
(type 3) and `0x3001f520` (types 8, 9): the shader draws at the unfloored
`0x3001ee90` height `h`, at the record's `y` plus `(H - h) * 0.5` for
`alignY` 1 and `H - h` for 2, `H` the floored height, which cancels the
floor. INFERRED: so the floored height moves only the label (`0x3001f490`
places its text inside `H` by the font height the same way), and a shader
aligns by its own height.

INFERRED, off `0x3001efb0` and `0x3001f020`: the position is `x` (`+0x4`)
and `y` (`+0x8`), moved from `fromX` (`+0x4c`) and `fromY` (`+0x50`) the
same way over `moveTime` (`+0x58`) from `moveStartTime` (`+0x54`). `alignX`
(`+0x14`) 1 subtracts half the element's width, 2 all of it; `alignY`
(`+0x18`) the same with the height. VERIFIED: the half is `0x3006930c`, 0.5.

INFERRED, off the tail of `0x3001f120`: while `0 < fadeTime` (`+0x28`) and
`cg.time - fadeStartTime` (`+0x24`) is below it, each byte lane runs
linearly from `fromColor` (`+0x20`) to `color` (`+0x1c`), scaled by
`0x30069420` to 0..1. VERIFIED: that constant is 1/255. Lane `+0x1c` is red,
`+0x1f` alpha, as the server's getters read them
(`cod11-gsc-object-model.md`, "HUD element fields").

INFERRED: none of the three tweens clamps a start time ahead of `cg.time`;
the fraction goes negative and extrapolates past `from`. vcod clamps it to
0..1.

### The two clocks

VERIFIED: `0x3001f520` registers the element's material and a second one
named after it with `"Needle"` (`0x30064b20`) appended, reads `duration`
(`+0x60`), and holds `0x300695d0` (1.0922667, `65536 / 60000`). INFERRED,
off its branch on `duration` and its arithmetic: the timer value is scaled
by 360 over `duration`, or by `0x300695d0` when `duration` is 0, and
truncated to a 16-bit angle (`ANGLE2SHORT`) before it turns the needle.
INFERRED: the face draws at the element's rect and the needle over it, one
revolution per `duration` ms, or per minute without one. The on-screen turn
direction is not measured.

---

## 9. The native player HUD

The layout is VERIFIED: pak0 `ui_mp/hud.menu` and the `COMPASS_*` defines
in `ui_mp/menudef.h`. What each owner draw does is read out of
`cgame_mp_x86.dll` (1.1), labelled per claim. Rects are virtual 640x480,
menu origin plus item offset.

| Item | Rect | Art | Ownerdraw |
|---|---|---|---|
| cursor hint | 300, 325, 40x40 | per hint | `CG_CURSORHINT` 72 |
| stance | 100, 434.375, 40x40 | `hudStance{Stand,Crouch,Prone}` | `CG_PLAYER_STANCE` 20 |
| weapon name back | 242.5, 431, 320x20 | `gfx/hud/hud@weaponnameback.tga` | 82 |
| ammo back | 557.5, 421.625, 80x40 | `gfx/hud/hud@ammocounterback.tga` | 6 |
| weapon mode | 537.5, 430.375, 20x20 | the weapon's `modeIcon` | `CG_PLAYER_WEAPON_MODE_ICON` 83 |
| weapon name | 242.5, 446, 320x30, textscale .3 | | 81 |
| ammo text | 570, 444.625, 55x40, textscale .21 | | `CG_PLAYER_AMMO_VALUE` 5 |
| health back | 501, 460, 130x12 | `gfx/hud/hud@health_back.tga` | `CG_DRAW_SHADER` |
| health bar | 502, 461, 128x10, forecolor 0.7 0.4 0 | `gfx/hud/hud@health_bar.tga` | 89 |
| health cross | 488, 460, 12x12 | `gfx/hud/hud@health_cross.tga` | `CG_DRAW_SHADER` |
| compass back, face | -25, 345, 160x160 | `hud@compassback`, `hud@compassface` | 84 |
| compass highlight | -25, 345, 160x160 | `hud@compasshighlight` | 85 |
| compass needle | 35, 395, 40x40 | `hud@compass_arrow` | 85 |
| compass friendlies | -25, 345, 160x160 | `hud@objective_friendly`, `hud@objective_friendly_chat` | `CG_PLAYER_COMPASS_FRIENDS` 88 |
| objective pointers | -25, 345, 160x160 | | 86 |

VERIFIED, the dispatch: the owner draw switch subtracts 4 from the id at
`0x30026bdf` and indexes the byte table at `0x300270f0`, then the jump table
at `0x3002706c`:

| id | lands on | calls |
|---|---|---|
| 5 | `0x30026c1e` | `0x30025ab0` |
| 20 | `0x30026d2a` | `0x30023f50` |
| 72 | `0x30026c36` | `0x300251f0` |
| 81 | `0x30026edb` | `0x30023c30` |
| 82 | `0x30026f00` | `0x30023d50` |
| 83 | `0x30026f26` | `0x30023e90` |
| 84 | `0x30026f3e` | `0x30024800` |
| 85 | `0x30026f5b` | `0x30025120` |
| 86 | `0x30026f78` | `0x30024d20` |
| 88 | `0x30026fd8` | `0x30013540` |
| 89 | `0x30026f95` | `0x300248c0` |

INFERRED: those are each id's handler; the claims below are attributed so.

VERIFIED, cvar table defaults (`{vmCvar_t *, name, default, flags}` records
at `0x30074aa4`..`0x30074c54`): `cg_hudDamageIconTime` 2000,
`cg_hudDamageIconWidth` 128, `cg_hudDamageIconHeight` 64,
`cg_hudDamageIconOffset` 32, `cg_hudCompassSize` 1.0,
`cg_hudCompassMaxRange` 1024, `cg_hudCompassMinRange` 0,
`cg_hudCompassMinRadius` 0, `cg_hudObjectiveMaxHeight` 70,
`cg_hudObjectiveMinHeight` -70, `cg_hudObjectiveMinAlpha` 1,
`cg_cursorHints` 3, `cg_crosshairAlpha` 1.0 (value at `0x301db788`),
`cg_crosshairAlphaMin` 0.7 (`0x301df208`), `cg_crosshairDynamic` 0. vcod
uses these values as constants.

VERIFIED, the weapon-file fields the HUD reads (`{name, offset, type}`
records around `0x30075558`):

| field | offset | field | offset |
|---|---|---|---|
| `displayName` | `+0x8` | `hipSpreadStandMin` | `+0x23c` |
| `modeName` | `+0x6c` | `hipSpreadDuckedMin` | `+0x240` |
| `reticleCenter` | `+0xe8` | `hipSpreadProneMin` | `+0x244` |
| `reticleSide` | `+0xec` | `hipSpreadMax` | `+0x248` |
| `reticleCenterSize` | `+0xf0` | `hipReticleSidePos` | `+0x264` |
| `reticleSideSize` | `+0xf4` | `clipOnly` | `+0x2d4` |
| `reticleMinOfs` | `+0xf8` | `wideListIcon` | `+0x2d8` |
| `hudIcon` | `+0x18c` | `adsAimPitch` | `+0x344` |
| `modeIcon` | `+0x190` | `adsCrosshairInFrac` | `+0x348` |
| `ammoIcon` | `+0x194` | `adsCrosshairOutFrac` | `+0x34c` |

The scope's keys are under "Scope overlay".

### Which views draw it

VERIFIED, `0x30018810`, the 2D pass: it returns early on `cg_draw2D` 0
(`0x301db42c`), else branches on the snapshot playerstate's `pm_type`
(`cg.snap + 0x10`):

| `pm_type` | calls |
|---|---|
| 5 | `0x30018530`, then returns |
| 4 | `0x30018070`, `0x30016f70`, `0x300150b0`; the hudelem pass `0x3001f980` with `cg_drawStatus` (`0x301d9ecc`) set |
| other | `0x30016760` (crosshair, which calls the turret reticle `0x30016610`), `0x30016f70`, `0x30037790` when below 6, `0x300150b0`; with `cg_drawStatus` set `0x3004a6f0`, `0x30017470`, `0x3001f980` |

INFERRED: `0x3004a6f0` paints the cgame's visible menus (`hud.menu`'s). So
a free-flying spectator (4) and the intermission (5) draw no menu HUD, a
dead player (6, 7) draws it without a crosshair, and a follower, whose
playerstate is its target's copy with the target's `pm_type`
(`cod11-spectator-follow.md`), draws the target's health, ammo, stance and
compass. vcod draws it on those terms, off the snapshot's playerstate
without prediction while following, and rebases the hit-direction feedback
when the playerstate's `clientNum` changes.

VERIFIED, the cvar rows: `cg_drawStatus` at `0x30074a44` binds
`0x301d9ec0`, `cg_drawCrosshair` at `0x30074aa4` binds `0x301d9020`, both
default `"1"`, flags 1 (archive). INFERRED, from Q3's `vmCvar_t`: each
`integer` sits at `+0xc`, `0x301d9ecc` and `0x301d902c`. VERIFIED, the
loads of `0x301d9ecc`: `0x3001887c` and `0x300188a0` in the 2D pass, and
`0x30026bb0`, the first instruction of `CG_OwnerDraw`. VERIFIED, the loads
of `0x301d902c`: `0x30016613` (turret reticle `0x30016610`), `0x30016827`
(crosshair `0x30016760`) and `0x30016f70`.

INFERRED, off the branches after those loads: `CG_OwnerDraw` returns at
once on `cg_drawStatus` 0, so with the 2D pass's two tests
`cg_drawStatus 0` hides every owner draw (health, ammo, weapon name,
stance, compass, cursor hint), the `hud.menu` pass and the hudelems, and
leaves the crosshair, scope overlay, hit-direction icons, chat and
obituaries. `0x30016760` returns on `cg_drawCrosshair` 0 after the scope
overlay's call, `0x30016610` draws only when it is not 0, and `0x30016f70`
returns only when it is negative. INFERRED, off `0x30016f70`'s test of
`cg_drawCrosshairNames` (`0x301e0bec`) and its name lookup: that function
draws the name of the player under the crosshair, which vcod does not draw.

vcod reads both cvars as integers each frame (`hud::DrawToggles`):
`cg_drawCrosshair 0` drops the weapon crosshair and the mounted reticle,
`cg_drawStatus 0` the native HUD, the cursor hint and the hudelems.

Multiplayer Options (`ui_mp/options_multi.menu`) also offers Show Compass
(`cg_drawCompass`, Off/On) and Team Overlay (`cg_drawteamoverlay`,
Off/Short/Long). VERIFIED:

- CoDMP.exe's client cvar block registers `cg_drawCompass` default `"1"`
  (`0x5685e4`) and `cg_drawTeamOverlay` default `"2"` (`0x5685c4`), flags 1,
  and discards the pointers.
- `cg_drawCompass` is in none of `cgame_mp_x86.dll`, `ui_mp_x86.dll` or any
  stock menu but that one; the exe's decompilation names it only at the
  registration.
- The cgame's `cg_drawTeamOverlay` row (`0x30074ef0`, binding `0x301ad060`,
  default `"2"`, flags 1) is the only reference to that `vmCvar_t`; no
  instruction loads `0x301ad060`..`0x301ad06f`.
- 1.5's CoDMP.exe and UO's CoDUOMP.exe register both the same way and
  discard the pointers. Neither string is in 1.5's or UO's `ui_mp_x86.dll`
  or in UO's cgame but for `cg_drawTeamOverlay`'s row, whose `vmCvar_t`
  (1.5 cgame `0x301b0c00`, row `0x30076eb0`; UO `0x304218a0`, row
  `0x30085750`) is referenced nowhere else (a byte search for each of its
  four words).

INFERRED: both options are inert in all three, so the compass always draws
and there is no team overlay. vcod registers both at retail's defaults so
the menu keeps the choice, and reads neither.

### Crosshair

INFERRED, off `0x30016760` and `0x3000fa50`: the hip spread in degrees is
`min + (max - min) * aimSpreadScale / 255`, `min` the prone minimum when
`pm_flags & 1`, the ducked one when `& 2`, else the standing one, or a
blend of two while the eye's stance leg runs (`0x3000fa50` is the cgame's
`BG_GetMinSpreadForWeapon`, `cod11-combat.md` 2.1), times a sight shrink
factor. Each arm sits `spread * 640 / fov_x` virtual pixels off the centre
horizontally and `spread * 480 / fov_y` vertically, floored at
`reticleMinOfs`. VERIFIED: the numerators are `0x300694fc` (640.0) and
`0x3006973c` (480.0), and the fov is an `fpatan` times the double at
`0x300693c8`, 114.59. INFERRED: that is `360 / pi`, so the fov is in
degrees.

INFERRED, same function: with the sight fraction `f` above 0, `t = f - (1 -
X)` for `X` = `adsCrosshairInFrac` or `adsCrosshairOutFrac` (picked by the
flag at `0x30209458`), and when `t > 0` the shrink is `1 - 0.5 * t / X`;
the arms' travel and both images' sizes take it. At `f` 1 nothing draws.
INFERRED: `adsAimPitch` drops the reticle by `480 / fov_y * adsAimPitch *
t` in the same arm; stock MP files do not set it and vcod leaves it out.

VERIFIED: `0x30036bd0` is the only writer of `0x30209458` (1 at
`0x30036c63`, 0 at `0x30036c75`). It reads the held weapon's `+0x2cc`
(`aimDownSight` in the field table, `0x30075c4c`), the float `0x30207214`,
and its own copy of it at `0x30209454`. VERIFIED: the snapshot
interpolation writes `0x30207214` from the float at `+0xc4` of the two
snapshots it lerps. INFERRED: that is `fWeaponPosFrac` (playerstate
`+0xb8`) behind the snapshot's 12-byte header. INFERRED, off
`0x30036bd0`'s branches: for an `aimDownSight` weapon, when the fraction
has left 0 or 1 since the last call, the flag becomes 1 if it moved up and
0 if down, and the copy updates; otherwise the flag holds. So the
in-fraction applies from the moment the sight starts rising until it next
starts falling, prone and mid-reload included. INFERRED, off
`0x30016760`'s test: 1 picks `adsCrosshairInFrac`.

VERIFIED: `0x30016760` tests the dword at `0x302071dc` against `0xc000` at
`0x300167b1`, compares `0x302074d4` with `0x3ff` at `0x300167c0`, and
tail-jumps to `0x30016610` at `0x300167d6`. INFERRED: `0x302071dc` is
`eFlags`, `+0x80` into the playerstate copy whose `+0xb8` is
`0x30207214`, and `0xc000` its mounted-gun bits. INFERRED, off those
branches: on a mounted gun the weapon reticle is not drawn; `0x30016610`
draws the turret's reticle, or nothing when `0x302074d4` is `0x3ff`
(`cod11-turrets.md` section 14.4). vcod draws that in place of the weapon
reticle.

INFERRED, off the four-arm loop: the arm table is top, right, bottom, left,
direction `(0,-1) (1,0) (0,1) (-1,0)`, corner offset in arm sizes
`(-0.5,-1) (0,-0.5) (-0.5,0) (-1,-0.5)`, a one-pixel nudge `(0,-1)` on the
top arm and `(-1,0)` on the left, pulled in by `hipReticleSidePos` arm
sizes. The bottom and left arms draw the image flipped (`t` 1 to 0), the
right and left turned 90 degrees. Arm and centre sizes skip the screen
scale the travel gets, so they are window pixels. The turn's direction is
not measured.

VERIFIED: `0x30016760` multiplies `1 - [0x30207534] * 0x30069420` (1/255)
by the return of `0x30015fe0` and by `cg_crosshairAlpha` (`0x301db788`),
and compares the product with `cg_crosshairAlphaMin` (`0x301df208`) at
`0x30016bb3`. VERIFIED: the snapshot interpolation writes `0x30207534`
from the float at `+0x3e4` of the two snapshots. INFERRED: that is the
playerstate's `aimSpreadScale` (`+0x3d8`, `docs/protocol-1.1.md`) behind the
12-byte header, lerped. INFERRED, off the compare: the arms' alpha is
`max((1 - aimSpreadScale / 255) * cg_crosshairAlpha,
cg_crosshairAlphaMin)`, so at the defaults they fade from 1 to 0.7, reached
at `aimSpreadScale` 76.5; the centre image takes `cg_crosshairAlpha` times
the same return, unfloored. INFERRED: `0x30015fe0` returns 1 whenever its
first call (`0x30015f20`) returns 0; its other branch draws the scope
overlay and returns `1 - frac` (next section), so both alphas fade over the
zoom tail, the centre to nothing and the arms to `cg_crosshairAlphaMin`.
vcod reads `aimSpreadScale` off the prediction or the newest snapshot, not
lerped.

### Scope overlay

VERIFIED, the weapon field table (`0x30075dc8`..`0x30075e10`):
`adsZoomFov` `+0x218`, `adsZoomInFrac` `+0x21c`, `adsZoomOutFrac` `+0x220`,
`adsOverlayShader` `+0x224` (type 0), `adsOverlayReticle` `+0x228` (type
10), `adsOverlayWidth` `+0x22c`, `adsOverlayHeight` `+0x230`. VERIFIED: the
name table at `0x300754f0` reads `none`, `crosshair`, `FG42`,
`Springfield`, `Gewehr43`. INFERRED: type 10 stores the name's index, 0 to
4.

VERIFIED, pak0's `weapons/mp/*`: five files set the keys, all
`adsOverlayShader ui/assets/reticle_circle_quarter` at 220 by 220:
`springfield_mp` with reticle `Springfield`; `kar98k_sniper_mp`,
`mosin_nagant_sniper_mp`, `fg42_mp`, `fg42_semi_mp` with `FG42`. VERIFIED:
pak4's `scripts/ui_hud.shader` maps that material to `map clamp
ui/assets/reticle_q.tga`, `blendFunc blend`. VERIFIED: pak0's 64x64
`reticle_q.tga` is opaque black except a quarter disc about its
bottom-right corner, radius about 32 texels, at alpha 25 of 255.

INFERRED, off `0x30015f20`: the overlay is up when the held weapon names a
shader or a reticle, `fWeaponPosFrac` (`0x30207214`) is not 0, and `frac =
(f - (1 - X)) / X` is above 0.01, `X` `adsZoomInFrac` with the direction
flag `0x30209458` set, else `adsZoomOutFrac`; the division is skipped while
`f - (1 - X)` is not above 0. VERIFIED: `0x300693f4` reads 0.01. INFERRED,
off `0x30015fe0`: the colour it sets before the image is `(1, 1, 1, 1)`, so
the overlay snaps on with no fade, at `f` 0.584 on the rise for
`kar98k_sniper_mp`'s 0.42.

INFERRED, off `0x30016760`: the crosshair draw calls `0x30015fe0` before
testing `cg_drawCrosshair` (`0x301d902c`), so only its earlier returns
(`0x30207158` set, a mounted gun, no weapon) skip the overlay.

INFERRED, off `0x30015fe0` and `0x30015d70`: the overlay's centre is the
refdef's middle plus an offset. With `v` the forward of the pitch at
`0x3020cb80` and yaw at `0x3020cb84`, and `d` its dot with the refdef axis
at `0x302095a0`, the offset is `-320 * (v . axis[1]) / (tan(fov_x / 2) * d)`
across and `-240 * (v . axis[2]) / (tan(fov_y / 2) * d)` down, times
`screenXScale` and `screenYScale` (`0x301d1fc4`, `0x301d1fc8`), and 0
unless `d` and both refdef fovs (`0x3020958c`, `0x30209590`) are above 0.
VERIFIED: `0x300694f8` reads -320.0, `0x300694f4` -240.0, `0x3006946c`
0.0087266 (`pi / 360`). INFERRED, off `0x300371f0`: for an `aimDownSight`
weapon with `fWeaponPosFrac` not 0 the two angles come from `0x3003c810`,
else from the refdef's view angles (`0x302095cc`, `0x302095d0`), which
centres the overlay. INFERRED: `0x3003c810` is the gun's angles composed
onto the view, the cgame's copy of the server's aim block
(`cod11-combat.md` 15), so the overlay sits where the shot goes and drifts
with the idle sway. vcod runs `pmove::aim::gun_angles` on the replay's
playerstate, or on a followed player's snapshot playerstate at the render
clock, and projects its forward through the drawn fovs.

VERIFIED, `0x300371f0` at `0x300373a3`..`0x30037480`: the block
`0x30012bf0` (the cgame's `BG_CalculateWeaponAngles`) reads is built on the
stack as `{ps, [0x3020c9f0], [0x30207144] * 0.001, ..., [0x30207148] -
ps[+0x20cc], [0x3020c9a8] - ps[+0x20cc] or 0 when [0x3020c9a8] is 0,
[0x3020c9e4], [0x3020c9e8], ...}`; `0x300127e0` reads words 7 to 10 of it,
and its constants are `0x30069524` 100.0, `0x30069528` 400.0, `0x3006930c`
0.5, `0x300693d0` 0.75. INFERRED, off its branches: `0x300127e0` is the
gun's damage kick, a no-op on a zero word 8, playing out `word 7 - word 8`
ms through the same envelope and factors as the server's `0x39ce8`
(`cod11-combat.md` 15). VERIFIED: every write to `0x3020c9a8`,
`0x3020c9e4` and `0x3020c9e8` except the zeroing in `0x30028a70`
(`0x30028b2e`..`0x30028b39`) is in `0x300287f0`. INFERRED: words 7 and 8
are `cg.time` and the hit's time on one base, so the offset cancels, and
`0x3020c9e4`/`0x3020c9e8` are the kick along and across the view.

VERIFIED, `0x300287f0` is called with `(damageYaw, damagePitch,
damageCount)` off a playerstate's `+0xe8`, `+0xec`, `+0xf0` at
`0x30028d2c` and `0x3002fc5e`, each behind a compare of `+0xe4`
(`damageEvent`) across two playerstates and a test of `+0xf0` against 0.
INFERRED: those are the new and previous snapshot's, so a hit is a changed
`damageEvent` with a non-zero count. VERIFIED: the kick is `damageCount *
0x300693d4` (0.2), raised to `0x30069414` (5.0) below that and capped at
90.0 (`0x3006947c`). INFERRED, off the branch at `0x30028854`: yaw and
pitch both 255 store 0 at `0x3020c9e8` and `-kick` at `0x3020c9e4`.
VERIFIED, the other arm: each byte is divided by `0x30069384` (255.0) and
multiplied by `0x30069374` (360.0), the pair goes through two `fsincos`
into `(cp*cy, cp*sy, -sp)`, `0x3020c9e8` gets `-kick` times its dot with
the refdef's `viewaxis[1]` (`0x302095ac`) and `0x3020c9e4` `kick` times its
dot with `viewaxis[0]` (`0x302095a0`). VERIFIED: `0x30028a58` stores
`cg.snap`'s `serverTime` (`[0x301e2160] + 8`) into `0x3020c9a8`. INFERRED:
the client rebuilds the kick from the feedback bytes, scaling by
`damageCount` where the server's own kick scales by `aimSpreadScale`
(`cod11-combat.md` 6), and reads a byte as `/ 255` where the server wrote
it as `* 256 / 360`. vcod does the same in `Hud::feed_damage` ("Damage
direction", "When the feed runs"), stamping the kick with the sway's clock
at the frame the new `damageEvent` is first seen, where retail stamps the
snapshot's `serverTime`.

INFERRED, off `0x30015fe0`'s calls to `0x300310f0` (a wrapper of the
stretch-pic trap `0x49`: x, y, w, h, s1, t1, s2, t2, material): with `w`
and `h` the overlay size times the screen scales and `(cx, cy)` the
centre, the image draws four times, `w` by `h`: at `(cx - w, cy - h)` with
corners `(0,0)-(1,1)`, at `(cx, cy - h)` mirrored in `s`, at `(cx - w, cy)`
mirrored in `t`, at `(cx, cy)` mirrored in both. Black bands follow in the
same material: left `(0, 0, cx - w, H)` when `cx - w > 0` and right `(cx +
w, 0, W - cx - w, H)` when `cx + w < W`, at `s` 0 and `t` 0 to 1; top `(cx
- w, 0, 2w, cy - h)` and bottom `(cx - w, cy + h, 2w, H - cy - h)` under
the same tests, at `t` 0 and `s` 0 to 1. `W` and `H` are the refdef's size
(`0x30209584`, `0x30209588`). INFERRED, off the weapon setup falling
through to `0x300358f5`: the material at `0x301a6aa4 + 0x198 * weapon` is
`adsOverlayShader` registered with trap `0x58`.

INFERRED, off UO's `uo_cgame_mp_x86.dll` `0x300195e0` (its `0x30015fe0`,
gated by `0x30019520`) and its calls to the stretch-pic wrapper
`0x3003e0f0`: UO draws the image once, `w` by `h` at `(cx - w/2, cy - h/2)`
with corners `(0,0)-(1,1)`, then the same four bands around that box (left
and right full height at `s` 0, top and bottom its width at `t` 0).
VERIFIED: `0x3007bce8` reads 0.5. VERIFIED, `uo/pakuo04.pk3`'s
`weapons/mp/springfield_mp`: `adsOverlayShader
gfx/reticle/scope@scope_overlay_us.dds`, `adsOverlayReticle none`, 520 by
400; `pakuo00.pk3`'s image is a 1024 by 1024 DDS of the whole scope.
VERIFIED: 1.5's `springfield_mp` keeps `ui/assets/reticle_circle_quarter`
at 220 by 220. INFERRED: 1.5's cgame keeps 1.1's four quarters. vcod picks
the layout by protocol (`hud::scope::Layout`); drawn as quarters, the UO
image would show four whole scopes.

INFERRED, same function, the reticle, drawn over the image whether or not
the shader is set:

| reticle | draws |
|---|---|
| `crosshair` | the weapon's `reticleCenter` material, `reticleCenterSize` times the screen scales, centred |
| `FG42`, `Gewehr43` | in `(0, 0, 0, 1)`: a `hudSoftLine` post `(cx - 1, cy, 3, 0.9h)` and two `hudSoftLineH` bars `(cx - 0.9w, cy - 1, 0.75w, 3)` and `(cx + 0.15w, cy - 1, 0.75w, 3)` |
| `Springfield` | a cross, `(cx - 1, cy - 0.9h, 3, 1.8h)` and `(cx - 0.9w, cy - 1, 1.8w, 3)` |

The 1 and the 3 are window pixels. VERIFIED: `0x300695ec` reads 0.9,
`0x300693d0` 0.75, `0x300695e8` 1.8, `0x300695c0` 0.15; `0x301d5a88` and
`0x301d5a8c` are stored at `0x3002303d` and `0x30023047` from the
registrations of `hudSoftLine` and `hudSoftLineH`. VERIFIED: pak4's
`scripts/hud.shader` gives both `rgbGen vertex` and `alphaGen vertex`.

INFERRED, off `0x300371f0` and `0x30036cf0`: the gun's add flag is cleared
when `cg_drawGun` (`0x301dbaec`) is 0, or is not 2 and `0x30015f20` returns
1; with it clear `0x30036cf0` adds neither the gun's refEntity (trap
`0x3d`) nor its `tag_flash` effect. So under the overlay the viewmodel and
the first-person muzzle flash vanish. VERIFIED: the cvar table names
`cg_drawGun` for the `vmCvar_t` at `0x301dbae0`, integer at `+0xc`.

INFERRED, off `0x300172f0`: while `0x30015f20` returns 1, the hit-direction
icons are skipped unless `cg_hudDamageIconInScope` (`0x301e0acc`, default
0) is set, and are then centred on the overlay's offset. INFERRED, off
`0x30018810`'s call order: the hit icons (`0x300172f0`) draw before the
`pm_type` branch, then the crosshair with its overlay (`0x30016760`),
`0x30016f70`, `0x30037790`, `0x300150b0`, then with `cg_drawStatus` the
`hud.menu` items (`0x3004a6f0`), `0x30017470` and the hudelems
(`0x3001f980`). So the menu HUD and the hudelems paint over the overlay's
black bands. vcod draws the overlay first, under them, as retail.

INFERRED, with "Which views draw it": a follower's playerstate carries its
target's `pm_type` and `fWeaponPosFrac`, so a spectator following a scoped
player gets the overlay and the zoom. vcod draws a followed player's view
weapon, zoom and overlay off the snapshot's playerstate.

vcod scales the overlay by the window height on both axes, as the rest of
its HUD, where retail scales x by `width / 640`: on a window wider than 4:3
the circle stays round and the side bands widen.

### Health

INFERRED, off `0x300248c0`: the share is `stats[0] / stats[2]` clamped to
0..1, 0 when either is 0; the bar is cropped (texture `s` 0 to the share),
not squeezed. Above half the forecolor's red and blue scale by `2 * (1 -
share)`; at or below half its green becomes `(share + 0.2) * green + 0.3`.
INFERRED: a red tail from the share to the share last shown holds one
frame, then drains by `0x30069490` per ms, and resets when the
playerstate's client number changes. VERIFIED, the constants: `0x300693d4`
0.2, `0x30069454` 0.3, `0x30069490` 0.0012.

### Weapon name and ammo

INFERRED, off `0x30023c30` and `0x30023d50`: the name is the localized
`displayName`, or `"%s / %s"` with the localized `modeName` when that is
not blank, right-aligned 28 units in from the rect's right edge; the
backdrop is the text's width plus 36, right-aligned on the same edge.
VERIFIED: `0x30069400` reads 28.0, `0x30069584` 36.0.

INFERRED, off `0x30025ab0` and `0x3000f920`: a non-`clipOnly` weapon prints
`ammoclip[clipIndex]` through `"%2i"` at the rect's left, `"|"` centred,
and the reserve through `"%i"` right-aligned; a `clipOnly` weapon prints
its clip alone, centred. Both counts cap at 999. VERIFIED, the strings:
`0x3006526c`, `0x30063148`, `0x3006567c`. INFERRED, off `0x3000f920`: a
weapon with a shared ammo cap sums the cap's weapons the player holds. vcod
prints the weapon's own.

INFERRED, off the guard both `0x30023c30` and `0x30023d50` open with: the
name and its backdrop draw only while `0x30019a30` returns a colour.
VERIFIED, `0x30019a30` (`ecx` the length, `edx` the start): null when the
start is 0 or `cg.time - start` is not below the length; else white, alpha
1, or `left * 0.01` (`0x300693f4`) when `left` (length less time since
start) is below 100, times the global at `0x301e14e8`, `cg_hudAlpha` (record
`0x30074ae0`, default `"1.0"`). VERIFIED: both callers pass 1800 (`0x708`)
and the stamp at `0x3020c920`.

VERIFIED, the seven stores of `cg.time` (`0x30207148`) to `0x3020c920`:

| at | in |
|---|---|
| `0x30037f28` | the weapon select `0x30037f20`, which the `a` server command calls (`0x3002e139`) |
| `0x300380a2`, `0x3003811c` | the two weapon-cycle commands, only when `cg.time` less the stamp is at least `cg_weaponCycleDelay` (record `0x30074c10`, integer `0x301df9ec`, default `"0"`) |
| `0x30038342` | the weapon slot command, same gate |
| `0x30028aa3` | `0x30028a70` |
| `0x3001dabc` | `0x3001da80`, an item pickup event for a weapon (row type 1 at `0x30076310`) while the selection (`0x301cbb0c`) is 0 |
| `0x300387fa` | the cycle's fallback, when it leaves the selection unheld and non-zero |

VERIFIED: `0x30028a70` is reached from the two first-snapshot setups (`jmp`
at `0x3002fa8f`, `call` at `0x3003037f`) and from the snapshot
transition's `call` at `0x300300cd`, taken when `0x30207154` is set or the
new snapshot's `stats[5]` (snapshot `+0x114`) or `clientNum` (`+0xb8`)
differs from the old one's (`0x30030009`..`0x300300b8`). INFERRED: that is
Q3's `CG_Respawn`, run on a new spawn count (`docs/protocol-1.1.md`,
`stats[5]`) or a new followed client. VERIFIED: the name and backdrop show
the selection's weapon (`0x301cbb0c`) when it is held, else the
playerstate's.

VERIFIED, `0x3001da80` (Q3's `CG_ItemPickup`, item index in `eax`): it
stores the item at `0x3020c914` and `cg.time` at `0x3020c918` and
`0x3020c91c`, then, for a weapon row while the selection is 0, stamps the
name and sets `cg_weaponSelect` to the row's weapon (`+0x4`, through
`"%i"`). VERIFIED: its one caller (`0x3001e0ff`) is the event switch's arm
for 146, 147 and 148 (`EV_ITEM_PICKUP`, `_QUIET`, `EV_AMMO_PICKUP`), taken
when the snapshot playerstate's `pm_flags` has `0x50000` and the event's
entity is its `clientNum`. VERIFIED: nothing else references `0x3020c914`
or `0x3020c91c`, and the only other store to `0x3020c918` is a clear at
`0x3002ca9d`. INFERRED: 1.1 keeps Q3's pickup bookkeeping but draws no
pickup notice; the pickup's only visible effects are the weapon name and
the autoswitch out of empty hands.

vcod stamps on a change of `(clientNum, stats[5])`, the first playerstate
included, on every weapon-select bind (`weaponslot`, `weapnext`,
`weapprev`), on the `a` command and on a weapon pickup with the selection
at 0, which also selects the weapon (`PlayInput::picked_up`). It fades the
name and its backdrop by that rule (`WeaponNameFade`). The selection is
`PlayInput::selected`, retail's `cg_weaponSelect` with every write
`cod11-combat.md` 9.8 lists; the name, mode icon and ammo count read it as
that section's readers do (`PlayerView`'s `name_weapon`, `ammo_weapon`).

### Weapon mode icon

VERIFIED: the weapon setup registers the def's `modeIcon` (`+0x190`) at
`0x30035cab` and stores it at `+0x11c` of the weapon's `0x198`-byte record
from `0x301a6940` (`0x30035cb3`). `0x30023e90` reads that slot for the
selection's weapon when the selection is carried and below the weapon
count, else for the weapon at `0x30209484` (`cod11-combat.md` 9.8), and
draws it over the item's rect in its forecolor when not 0. VERIFIED: it
skips the fade test `0x30019a30` when the playerstate's weapon
(`0x3020720c`) is not 0 (`0x30023e90`..`0x30023eab`). INFERRED: the icon
draws whenever a weapon with a `modeIcon` is held, and while the name fades
in with empty hands. VERIFIED, pak0's `weapons/mp/*`: the select-fire pairs
name one, `hud@weaponmode_full.tga` on `bar_mp`, `fg42_mp`, `mp44_mp`,
`ppsh_mp`, `thompson_mp`, `hud@weaponmode_semi.tga` on their `_semi` and
`bar_slow_mp` variants; every other weapon leaves it blank.

### The weapon-slot strip

VERIFIED: `0x30037790` has one caller, `0x30018896` in the 2D pass
`0x30018810`, after the crosshair (`0x30016760`) and `0x30016f70` and before
`0x300150b0`, taken when the snapshot's `pm_type` is below 4. The pass
returns early on `cg_draw2D` 0 and three unidentified flags (`0x301e214c`,
`0x301e2150`, `0x3020cbdc`). `cg_drawStatus` does not gate it. VERIFIED,
inside: `dt` is `cg.time` less `0x3020c944`, which takes `cg.time` each
call; it returns when the predicted `pm_type` (`0x30207160`) is 6 or more,
and when `0x30019a30(1800, 0x3020c920)` returns null, so it shares the
weapon name's stamp and fade. VERIFIED: `0x3002ca60` zeroes the slot scales
and `0x3020c944`.

VERIFIED, the layout (`.rdata` 32.0 `0x30069408`, 2.0 `0x30069308`, 0.01
`0x300693f4`, 64.0 `0x300695a0`), in 640x480 units, from x 632, slots 5
down to 1 of the predicted playerstate's `weaponslots` (`0x30207470`):

- a slot empty or not in `ps.weapons` (`0x30207468`) takes 32 and its
  scale goes to 0;
- a held weapon whose `hudIcon` registered nothing (record `+0x118` from
  `0x301a6940`, set in `0x30034cf0` only for a non-empty `hudIcon`) draws
  `hudNoWeaponIcon` (`0x301d5acc`) 32 square at y 8 in grey and takes 32,
  scale to 0, no key;
- the slot `0x3000f5c0(cg_weaponSelect, 1)` names grows its scale by `dt *
  0.01` to 1 and draws white, `(scale + 1) * 32` high and twice that wide
  for a `wideListIcon` (def `+0x2d8`); every other slot shrinks at the same
  rate and draws grey (0.5) at its own scale;
- each drawn slot puts its key under its left edge, then a 2-unit gap.

The icon is the slot's own weapon's. VERIFIED, `0x30037620`: the key text
is `weaponslot <name>`'s binding (`0x30046590` rescans the bind table each
call; names from `0x30075504`): one key's localized name, two joined by
`KEY_OR`, or `KEY_UNBOUND`. It draws with trap `0x36` in font 5
(`consoleFont`) at 0.25 with 6.0 as the fixed cell (section 8), black at
`(x + 1, 14.6)` then white at `(x, 13.6)`, alpha the fade's. INFERRED: the
y is the baseline, as vcod draws it.

VERIFIED, the selected slot of a weapon with more than one mode on its
`altWeapon` chain: the other modes draw grey under the box's centre, the
first at `y = h + 10` (10.0 `0x300693e4`), each further one 34 below
(`0x300694e8`), 32 high and 32 or 64 wide. A switch from a mode to the mode
it names sets `0x30096c20` to +1 (else -1), which runs back to 0 at `dt /
150` (`0x300696d4`); while non-zero the mode in hand slides from the row
below up to y 8 growing to full size, and the mode left slides down,
shrinking. INFERRED: a backward slide (-1), which no stock chain produces,
reads the colour's alpha off the stack as its first row; vcod copies that.

vcod: `hud::weapstrip`, pinned by its three tests, laid out in vcod's
centred 640x480 space where retail stretches 640x480 over the window
(`0x301d1fc4`, `0x301d1fc8`), as every native HUD piece in vcod. A look by
eye is pending.

United Offensive's strip, `uo_cgame_mp_x86.dll` `0x30046bb0`:

- VERIFIED: it walks slots 7 down to 1 (`iStack_68` starts at 7, the scale
  pointer at `0x3048ae6c` stops past `0x3048ae54`), so the binoculars (7)
  sit rightmost from x 632, then the satchel (6), then 5..1. Slot scales
  are `0x3048ae50 + slot * 4`, the clock `0x3048ae70`.
- VERIFIED: the layout constants match 1.1's: 632.0, 32.0 (`0x3007bdd0`),
  0.01 (`0x3007bdb4`), the 2.0 gap (`0x3007bce4`), the mode rows' 10.0 and
  34.0 (`0x3007bda4`, `0x3007bf20`), 64.0 (`0x3007c000`), the slide's 1/150
  (`0x3007c188`). The key text (`0x30046a40`) prints `weaponslot %s` off
  the eight-name table `0x30084310` at `(x + 1, 8 - 4 + 9.6 + 1)` black,
  then `(x, 13.6)` white (4.0 `0x3007be40`, 9.6 `0x3007c080`), 1.1's 14.6
  and 13.6.
- VERIFIED: a negative `dt` reads 0 (`jns`, `0x30046bce`); 1.1 keeps the
  sign.
- VERIFIED: the 2D pass calls it at `0x3001c06f` only when the snapshot's
  `pm_type` is below 6 and neither 4 nor 5, and `cg_drawStatus` (`vmCvar`
  `0x30450a40`, integer `0x30450a4c` tested at `0x3001c066`; registration
  row at file offset `0x85290`) is non-zero. 1.1 does not read
  `cg_drawStatus` for the strip.

vcod: `StripView::uo` walks 7..1, the slot lookup takes 1..7, a backward
clock reads 0, and on UO the strip hides with `cg_drawStatus` 0
(`weapstrip::tests::uo_draws_the_satchel_and_binocular_slots`). A look by
eye is pending.

### Weapon slots

The `weaponslot` command, per protocol (all three cgames load at
0x30000000).

- 1.1 `cgame_mp_x86.dll`. VERIFIED: the name table at `.rdata`
  `0x30075504` holds six pointers: `none`, `primary`, `primaryb`, `pistol`,
  `grenade`, `smokegrenade` (`0x30066db4`, `0x30066d54`, `0x30066d48`,
  `0x30066d40`, `0x30066d94`, `0x30066d30`). VERIFIED: the lookup
  `0x3000f220` compares the argument with the six through `_stricmp`
  (`0x300581b0`) and returns the index, 0 for none. INFERRED, the handler
  `0x300382f0`'s instruction order: the gates (a snapshot, predicted
  `pm_flags` without 0x4000, snapshot `pm_flags` 0x40000, `cg.time` less
  the name stamp `0x3020c920` at least `cg_weaponCycleDelay`), then the
  stamp takes `cg.time` (`0x30038342`); only then is argument 1 read and
  looked up. A 0 from the lookup falls back to `atol` (`0x3004afde`). A
  result outside 1..5 (`jle`, `cmp 6; jge`) returns, as does an empty slot
  byte of `ps.weaponslots` (`0x30207470 + n`) or a weapon outside
  `ps.weapons` (`0x30037f00`). Otherwise it selects through `0x30037f20`.
  No refusal prints anything, and a refused slot still restamps the weapon
  name, which the strip shares.
- 1.5 `cgame_mp_x86.dll` (protocol 6). VERIFIED: the same six names at
  `0x30077504`, the lookup `0x30010000`, the handler `0x30039c00` with the
  same order and the 1..5 bound (`0x30039c86`). Its own-view bit is 1.5's
  `pm_flags` 0x20000 (`0x30039c1c`), which vcod's decoder maps onto 1.1's
  0x40000.
- UO `uo_cgame_mp_x86.dll` (protocol 22). VERIFIED: eight names at
  `0x30084310`, 1.1's six then `satchel` (`0x300765fc`) and `binocular`
  (`0x300765f0`). The lookup `0x300110b0` scans all eight. The handler
  `0x30047750` bounds the slot at 1..7 (`cmp 8; jge`, `0x300477e8`) and
  adds one gate before the stamp: with the predicted `eFlags` 0x100000 (a
  seat, `0x3004776c`) it goes on only when the snapshot's `vehType` (ps
  +0x618) is 1 and `vehPos` (ps +0x614) is 3,
  `BG_AllowPlayerWeaponAtVehiclePos` inlined. VERIFIED: the slot lookup
  `0x30011460` takes slot types 3..7 at their own byte (1.1's `0x3000f5c0`
  stops at 5). The cycle wraps over seven slots, `(slot + dir + 6) % 7 + 1`
  (`0x300479d8`); its first-slot walk (`0x30047820`) runs 1..7 forward or
  7..1 back. The stock UO binds use `weaponslot
  satchel|binocular|smokegrenade` (`coduo-diff.md` 15).

vcod: the console's `weaponslot` (`console::shell::weapon_slot`) takes UO's
eight names, case-folded, falling back to `atol`, and always queues the
slot, as retail stamps before it looks. `PlayInput::weapon_slot` refuses 0,
a slot past the protocol's last (`Held::uo_slots`) and a UO seat that does
not fire the rider's weapon (`Held::seat_refuses_slot`, read off the
snapshot's `eFlags` where retail reads the predicted one). The cycle and
slot lookup take UO's seven slots. Trap: vcod once refused
`smokegrenade`, `satchel`, `binocular` and every number, with a usage line
retail lacks; `play::loopback::every_slot_name_switches_the_held_weapon`
runs each name through the shell, the input and vcod-server. On UO the
satchel and binoculars move the server's `ps.weapon` to slots 6 and 7, and
the strip draws them.

The other binds, read off the disassembly (INFERRED, instruction order):

- UO's `weapnext` `0x300474c0` and `weapprev` `0x30047550`: with the
  scoreboard held (`0x3048a554`) they scroll it, as 1.1's (`0x3020c030`)
  do. Else: a snapshot, predicted `pm_flags` without 0x4000, then predicted
  `eFlags` (`0x30483248`, ps `0x304831c4` + 0x84) 0x100000 sends
  `nextvehslot` / `prevvehslot` (trap 0x18) and returns, before the
  own-view bit (snapshot `pm_flags` 0x80000), the cycle delay against the
  stamp `0x3048ae44` and the stamp; the cycle `0x30047960` runs after the
  stamp.
- UO's `weapalt` `0x30047400`: a snapshot, predicted `pm_flags` without
  0x4000, the own-view bit, then with predicted `eFlags` 0x106000 (a seat
  or a mounted gun) it goes on only when the predicted `vehType`
  (`0x304837dc`, ps + 0x618) is 1 and `vehPos` (`0x304837d8`, ps + 0x614) is
  3 (`0x30047429`); then the cycle delay, the `altWeapon` lookup (def
  `+0x36c`), the `CGAME_THIS_WEAPON_HAS_NO_ALTERNATE` line, the carried
  check and the select `0x30047390`, which stamps. 1.1's `0x30037f90` has
  the same order with predicted `eFlags` 0xc000 (`0x302071dc`) alone.
- VERIFIED: UO's prediction (`0x30035800`) copies its base playerstate
  from `0x30459164`, the snapshot after `cg.snap` (`0x30459160`), 0x1141
  dwords at `+0xc`. INFERRED: that is `cg.nextSnap`, the newest snapshot
  the clock has reached, so the predicted `eFlags` the seat gates read are
  the newest snapshot's (pmove never writes the seat bit), and `cg.snap`'s
  own-view bit and `vehType` are a snapshot older.
- VERIFIED decompile: 0x30035800 tests `cg.snap`'s `pm_flags` (+0x18)
  against 0x40000, UO's follow bit, and calls 0x300354b0, which copies its
  playerstate from `0x30459164` + 0xc and lerps origin and view between
  `cg.snap` and it. INFERRED: 0x300354b0 is the follow branch, and a
  follower's drawn playerstate fields (`eFlags`, the seat,
  `viewlocked_entNum`) are the newest snapshot's, as vcod reads them; only
  the choice between predicting and following is `cg.snap`'s, a snapshot
  late at the start or end of a follow. vcod chooses off the newest
  snapshot's `clientNum`: for that snapshot retail would run our cmds on
  the followed player's playerstate, which vcod does not copy.

vcod: `PlayInput::select_allowed` runs the view and seat gates before the
cycle delay and the name stamp; a select they pass stamps even when its
slot is empty or out of range, as retail's. `weapalt` refuses in a UO seat
that does not fire the rider's weapon. vcod reads the predicted gates off
the newest snapshot (retail's prediction base), and the own-view bit and
`weaponslot`'s `vehType` and `vehPos` off `cg.snap`, the older snapshot of
the drawn pair (`Held::cg_snap`; 1.1 reads `cg.snap` `0x301e2160` + 0x18
at `0x3003830c`, UO `0x30459160` + 0x624, + 0x620 and + 0x18 at
`0x30047778`..`0x30047792`, VERIFIED disassembly). Reading them off the
newest snapshot instead is a snapshot early at a spawn, a follow or a seat
change. `play::replay::the_jeep_drivers_binds_wait_for_cg_snap` replays
`*-uo-jeep.bin`: the binds refuse on the frames before the first spawned
snapshot (14100) turns `cg.snap`'s bit on; the `nextvehslot` branch runs
first, off the newest snapshot. `play::input::tests::select_gates_run_before_the_stamp`
and `uo_weapalt_refuses_in_a_seat` pin both.

### Stance

VERIFIED: `0x30023f50` reads the dword at `0x30207168` and keeps its low
two bits at `0x30074988`. VERIFIED: `0x30028a70` copies `0x834` dwords from
`cg.snap + 0xc` (`0x301e2160`), the snapshot's playerstate, to
`0x3020715c`. INFERRED: that dword is the playerstate's `+0xc`,
`pm_flags`, in the copy the cgame predicts from. VERIFIED:
`hudStanceStand`, `hudStanceCrouch` and `hudStanceProne` are registered to
`0x301d5cf8`, `0x301d5cfc`, `0x301d5d00` (`0x30020faa`, `0x30020fbb`,
`0x30020fcf`). INFERRED, off the branch before the draw: prone on bit 1,
crouch on bit 2, standing otherwise.

VERIFIED: `0x30023f50` stores the time at `0x30074984` when those bits
differ from the kept ones or the time is earlier than the stored one, and
-1 instead whenever `cg_hudStanceHintPrints` (integer `0x301dac4c`,
default 0) is 0 (`0x30023f50`..`0x30023fa2`); both statics start at -1.
VERIFIED, the constants: 0.001 (`0x300693c0`), 0.8 (`0x30069458`), and the
cvars `cg_hudStanceFlash_r`, `_g`, `_b` at 1.0, 1.0, 0.3 (values at
`0x301dbd28`, `0x30298008`, `0x301df0e8`). INFERRED, off the tail: for 1000
ms from the stored time `hudStanceFlash` (`0x301d5d04`) draws over the icon
in that colour, each lane clamped to 0..1, at alpha `(stored + 1000 - now)
* 0.001 * 0.8`. VERIFIED: pak0's `configure_mp.cfg` and `safemode_mp.cfg`
set `cg_hudStanceHintPrints` 1. INFERRED: a stock install runs with it on,
which vcod assumes. VERIFIED: the icon's x adds `(cg_hudCompassSize - 1) *
112` (`0x30069738`), 0 at the default.

VERIFIED, the key hints (`0x300240e8`..`0x30024589`): while `cg.time` is
below the stored time plus 3000 (`0xbb8`), the function fills three 4-by-6
tables of command names on the stack, one per stance, and the labels
`CGAME_STANCEHINT_JUMP`, `_STAND`, `_CROUCH`, `_PRONE` (`0x30063238`,
`0x30063220`, `0x30063208`, `0x300631f0`). Read out of the stores:

| Stance | Jump | Stand | Crouch | Prone |
|---|---|---|---|---|
| standing (`[esp+0x114]`) | `+gostand`, `+moveup` | | `gocrouch`, `togglecrouch`, `lowerstance`, `+movedown` | `goprone`, `+prone` |
| crouched (`[esp+0xb4]`, bit 2) | | `+gostand`, `raisestance`, `+moveup` | | `goprone`, `lowerstance`, `toggleprone`, `+prone` |
| prone (`[esp+0x54]`, bit 1) | | `+gostand`, `toggleprone` | `gocrouch`, `togglecrouch`, `raisestance`, `+movedown`, `+moveup` | |

VERIFIED: it refreshes the binding table (`0x30046590`, the 50 `{command,
..., key1, key2}` rows from `0x3006f150`, each key the first two key
numbers whose binding matches case-insensitively, `0x300464e0`), then per
row keeps the first command `0x30046810` reports bound (key1 not -1) and
counts the rows kept. The first baseline is the rect's `y + h * 0.5 - 1.5`
(`0x3006930c`, `0x300693ac`), moved down half a text height (trap `0x35` at
the item's font and scale) for one row and up half a height plus 1.5 for
three. Each kept row prints at the rect's right edge (`x + w`, with the
compass-size shift) the localized label formatted with `0x30046940`'s key
text, and the next baseline moves down a height plus 1.5. VERIFIED,
`0x30046940`: the key text is `KEY_UNBOUND` with no key, the first key's
name, and with two `"%s %s"` of `KEY_OR` and the second's name appended.
VERIFIED: the colour is the item's forecolor, alpha 1 below the stored time
plus 2000 (`0x7d0`) and `(stored + 3000 - now) * 0.001` after. VERIFIED,
`localized_english_pak1.pk3`'s `cgame.str`: the labels read `Press [%s] to
jump`, `to stand`, `to crouch`, `to go prone`. INFERRED: with the stock
binds (Space `+gostand`, C `gocrouch`, Ctrl `goprone`) a standing player
sees three lines, crouched or prone two.

VERIFIED, the prone-blocked notice (`0x30023ff5`..`0x300240e5`): when the
predicted `pm_flags` has `0x8000` and the static at `0x3007498c` is below
`cg.time`, it becomes `cg.time + 1500` (`0x5dc`). While it is ahead,
`CGAME_PRONE_BLOCKED` (`0x30063250`, "Prone Blocked") prints centred on x
320 (`0x300695e4`, less half the text width) at y 270 (`0x43870000`) with
alpha `|sin((until - now) * 0.00066667 * 540 * pi / 180)|` (`0x30069734`,
`0x30069730`, `0x300693a8`, `0x30069494`): 0.36 degrees a millisecond,
three blinks in the 1.5 s.

vcod prints both off the flash's stamp (`stance_hints`, `ProneBlocked`),
the key text through the console's binds (`Shell::key_text`), and reads
`0x8000` off the predicted playerstate while predicting, else off the
snapshot's `pm_flags`. Its pmove raises the bit as retail's does
(`cod11-mantle.md`, "Prone Blocked"). All three draw before the icon, as on
retail. INFERRED: the notice, the hints and the flash live in the stance
owner draw, and the compass spring in the compass one, so `cg_drawStatus`
0 hides them and freezes their stamps and the spring; the weapon name's
stamp is set outside the draw and keeps running. vcod does the same.

vcod reads the icon and the flash off `pm_flags`' two bits, the replay's
while predicting. Its first frame counts as a change, as the -1 start makes
it on retail.

### Compass

VERIFIED: `hud.menu` lists back (84), highlight (85), face (84), needle
(85), friendlies, objective pointers (86). INFERRED: a menu draws its items
in file order, so the highlight is under the face.

INFERRED, off `0x30024800` and `0x30019bd0`: the back and face turn by the
view yaw less `northyaw` (configstring 11), smoothed by a spring; the
highlight and needle do not turn (`0x30025120`). INFERRED: both 84 items
call the spring each frame, and the second call finds no time passed.

VERIFIED, the spring `0x30019bd0`, which `0x30024800` calls before its draw:

- The target is `SHORT2ANGLE(ANGLE2SHORT(viewangles[YAW] (0x302095d0) -
  northyaw (0x3020d09c)) & 0xffff)` (`0x30069380` 182.04445, `0x3006937c`
  360/65536, through the truncating `_ftol` at `0x3005a890`).
- With the last time at `0x300eef2c` ahead of `cg.time`, or more than 500
  ms (`0x300694b8`) behind, it stores the time, sets the drawn yaw
  (`0x3020d0a0`) to the target and the speed (`0x3020d0a4`) to 0.
- Otherwise it stores the time, takes `delta = AngleSubtract(drawn,
  target)` (`0x3003c310`, into -180..180) and walks the elapsed time in
  steps of at most 5 ms, `dt` the step times 0.001 (`0x300693c0`). When
  `|delta| < 0.25` and `|speed| < 1.0` (doubles `0x30069430`, `0x30069328`)
  it snaps to the target with speed 0 and returns. Else `delta` becomes
  `vel * dt + delta` wrapped through `ANGLE2SHORT` into 0..360, less 360
  above 180 (`0x30069370`, `0x30069374`); the speed loses `dt * 1000`
  (`0x30069478`) when `delta > 0` and gains it when `delta < 0`; it loses
  `2 * speed * dt`; then, with the speed above 0, it loses `speed * dt *
  3.5` (`0x300694c4`) when `delta > 0` and `dt`, and becomes 0 if that took
  it below 0; with the speed at or below 0 the mirror image (`delta < 0`,
  `+ dt`). A speed it did not zero is clamped to ±30000 (`0x300694c0`,
  `0x300694bc`).
- After the last step the drawn yaw is the wrapped `delta + target`.
  `0x30024800` hands it to the turned pic call `0x30019330`.

VERIFIED: the pointer yaw at `0x3020d0a8`, which the friendlies and
objectives turn by, comes from a second spring at `0x30019f00` that runs
only when `cg_hudCompassSpringyPointers` (record `0x30074b30`, integer
`0x301da46c`) is set, else it is the view yaw. The default is `"0"`, and
pak0's `configure_mp.cfg` and `safemode_mp.cfg` set it 0. INFERRED: at
stock settings the face swings and settles while the pointers track the
view. vcod runs the face spring (`CompassSpring`) on the HUD clock and
turns the pointers by the view yaw.

INFERRED, off `0x30024d20`: only objectives in state 4 draw. The target is
the objective's origin, or its entity's when `entNum` is not `0x3ff`. The
20x20 icon's centre is `(55 - sin a * r, 425 - cos a * r)` for the bearing
`a` off the view yaw, `r = 43.75 * clamp(distance / 1024)` at the cvar
defaults. VERIFIED: `0x300695fc` reads 43.75, `0x30069448` 20.0. INFERRED:
the icon is the objective's configstring `1500 + icon` material with its
extension dropped and `_up` or `_down` appended when the target is more
than 70 units above or below the view. VERIFIED: `0x30024c40` holds the
suffixes `""`, `"_up"`, `"_down"`. Which suffix goes with which side, and
that `a` grows to the left, are INFERRED from the geometry, not measured.

### Compass friendlies

VERIFIED, `0x30013540`: it returns unless the client info of
`cg.nextSnap`'s playerstate `clientNum` (`0x301e2164`, `+0xb8`) is valid and
its team (`0x3018bc38`, stride `0x448`) is neither 0 nor 3. It then walks
the snapshot's entities (count `+0x20dc`, numbers from `+0x20e4`, stride
`0xf0`). For each whose `0x228`-byte `cg_entities` record reads 1 at
`0x3020dc74` (`0x300135a4`), has bit 1 clear at `0x3020dc78`
(`0x300135b3`) and has a valid client info on the same team, it stamps a
64-entry, 20-byte table at `0x3020d0b0` with `cg.time` and the record's
`0x3020dd78`, `0x3020dd7c` and `0x3020dd88`; when `0x3020dc78` has
`0x80000` (`0x300135e1`) and the slot's flash time (`+0x10`) is not ahead
of `cg.time`, it sets that to `cg.time + 3000` (`0x30013620`). INFERRED:
the two reads are `eType` (1, `ET_PLAYER`) and `eFlags` (bit 1, dead), and
the stamp is the interpolated origin's x and y and the interpolated yaw.

VERIFIED, `game.mp.i386.so`: `PlayerCmd_pingPlayer` (`0x550a4`) ORs 8 into
the playerstate's `eFlags` byte `+0x82` (the `0x80000` bit) and stamps
`level.time + 3000` at client `+0x2268`; `ClientEndFrame` clears the bit
once that time has passed. VERIFIED, pak5's `_teams.gsc`: the quick-chat
commands call `self pingPlayer()` after `sayTeam`.

VERIFIED, `G_GetNonPVSFriendlyInfo` (`0x42c30`; Ghidra's import places it
at `0x52c30`): it returns 0 for a viewer whose `sess.team` (client
`+0x217c`) is 0 or 3 (`0x42c50`, `0x42c59`). It walks 64 slots from `last
+ 1` modulo 64, `last` its third argument (0 when that reads `0x3ff`,
`0x42c65`), and skips a slot whose entity is not in use (`+0x160`,
`0x42ca0`), whose `s.eType` is not 1 (`0x42cad`), whose `s.eFlags` has bit
1 (`0x42cba`), with no client (`+0x158`) or a client of another team
(`0x42cd8`), or for which `trap_InSnapshot(eye, s.number)` (`0x42cf2`,
syscall `0x2f`) answers non-zero. INFERRED: the first slot left is the
answer, so the field cycles through every out-of-view teammate, one per
frame.

VERIFIED, the packing (`0x42d02`..`0x42f19`):

- each offset is the teammate's `r.currentOrigin` (`+0x134`, `+0x138`) less
  the eye's, plus 0.5 (`0x7311c`), stored by `fistp` under a control word
  ORed with `0xc00`, so truncated;
- an offset above 1024 gives a factor `1024.0 / offset` (`0x73120`), one
  below -1022 `-1022.0 / offset` (`0x73124`); when either factor is below
  1 the axis with the larger factor is multiplied by the smaller one and
  truncated (`0x42df9`..`0x42e5e`);
- both are clamped to -1022..1024 (`0x42e64`..`0x42e98`) and packed as
  `(offset + 2) / 4 + 255` with C's truncating division
  (`0x42ea3`..`0x42eda`), x at bit 6 and y at bit 15, 9 bits each;
- the client number (`s.number & 0x3f`) fills bits 0..5;
  `r.currentAngles[YAW]` (`+0x144`) times 256/360 (`0x73128`), truncated,
  the top byte.

VERIFIED: `ClientEndFrame` passes the playerstate origin with
`viewHeightCurrent` (`ps + 0xd0`) added to z, through `G_AddLean`
(`0x411c2`..`0x411e8`), and client `+0x2264` as `last` (`0x411ed`), stores
the answer at `iCompassFriendInfo` (playerstate `+0x3c0`, `0x41201`), and
writes `answer & 0x3f` back to `+0x2264`, or `0x3ff` for 0 (`0x4120e`,
`0x41240`). With a non-zero answer it sets the playerstate's `eFlags`
`0x100000` when that teammate's `s.eFlags` has `0x80000`, else clears it
(`0x4121d`..`0x41230`); a zero answer leaves the bit. `ClientSpawn` zeroes
the client up to `+0x22c4` (`bzero` at `0x42804`), covering `+0x2264` and
the playerstate.

VERIFIED, `SV_inSnapshot` (cod_lnxded `0x8087b90`, syscall `0x2f`'s
handler): it returns 0 when the entity's `+0xf0` is 0 or its `+0xf4` has
bit 1; 1 when `+0xf4` has `0x18`; 1 when the `svEntity`'s cluster count is
0; else 0 unless one of two area tests (`0x8053f44`) passes and one of the
entity's clusters is set in the eye's row. INFERRED: those are `r.linked`,
`SVF_NOCLIENT`, the broadcast flags and the snapshot cull's own PVS test,
so the answer is whether a snapshot built from the eye would carry the
entity, except that an entity in no cluster counts as carried.

VERIFIED, 1.5b's and UO's `SV_inSnapshot` (1.5b `0x808d5d2`,
`coduo_lnxded` `0x808e3a9`, syscall `0x32`'s handler; `trap_InSnapshot`
game.mp.uo `0x844e0`): after the same `r.linked`, `SVF_NOCLIENT` and `0x18`
tests it returns 1 for a non-zero `r.broadcastTime` (1.5b `+0xfc`, UO
`+0x100`), whatever its value; then 0 unless the eye's area connects to
one of the entity's two, 0 for an entity in no cluster, the cluster test,
and last the fog (`cod15-protocol-diff.md` 15). Unlike 1.1, an entity in no
cluster counts as not carried, and the area test comes first. VERIFIED
disassembly: only the snapshot builder clears `r.broadcastTime`, on the
first snapshot it builds whose `svs.time` is past it (`coduo_lnxded`
`0x8096d53`, 1.5b `0x8095dc6`). INFERRED, from that and the end frame
running before the snapshots: a sounded teammate is off the compass for one
end frame longer than he rides snapshots. VERIFIED decompile: on UO
`G_GetNonPVSFriendlyInfo` (`0x55762`) and `G_GetNonPVSTankInfo`
(`0x9bf6d`) are its only callers.

VERIFIED live, `client-probes/probe_snapcull` on mp_carentan, 2026-10-20,
retail 1.1d and UO 1.51b (`tests/fixtures/entities/snapcull-carentan-*.txt`,
gated by `uocull_ab`):

- slot 0 at (976, -864) reads allied slot 1 at (-992, 2328), out of its
  PVS, as `0x00ff9881` on both;
- slot 1 held 1000 units under the map (no cluster, area -1) reads 0 on
  1.1 and stays `0x00ff9881` on UO;
- after `playSound` on slot 1 the word reads 0 for the rest of the run on
  1.1 (`SVF_BROADCAST`, and slot 1 rides slot 0's snapshots); on UO it
  reads 0 from the sound's frame through the frame 350 ms later, the one
  whose snapshot first leaves slot 1 out, and is back 400 ms after the
  sound.

VERIFIED live, the same probe rerun 2026-10-20 on 1.1d, 1.5b and UO 1.51b
with four more steps (`snapcull-carentan-{11,15,uo}.txt`; 1.5b's gated by
`cull15_ab`):

- 1.5b reads every row above as UO does (no-cluster teammate kept at
  `0x00ff9881`, sounded one at 0 for 400 ms);
- slot 1 set at (1120, 432, 0), in slot 0's PVS about 1300 units off: 0 on
  all three;
- under `setCullFog(0, 1000, ...)`: `0x00ffc6c1` on 1.5b and UO (the fog
  test hides him); 0 on 1.1, where slot 1 still carries the earlier
  `SVF_BROADCAST`, so 1.1's row says nothing about fog;
- on 1.5b and UO a `playSound` inside the fog reads 0 for 400 ms, as
  without one: the broadcast time is tested before the fog. `setExpFog`
  puts the word back to 0;
- slot 1's own entity list empties under the fog on 1.5b and UO (all it
  was sent lies past 1000 units) and keeps all 22 entities on 1.1, whose
  builder has no fog test.

VERIFIED, `BG_PlayerStateToEntityState` (`0x2cbe8`): `s.eType` is 1 when
the playerstate's `pm_flags` has `0x10000` or `0x40000`, else 7.

VERIFIED live, `client-probes/probe_compass` on retail mp_harbor
2026-10-07, two allied probes, slot 0 standing at (-7352, -7976), slot 1
set down at spawns 2 to 13:

| slot 1 at | slot 0 read |
|---|---|
| (-8136, -7712) | `0x00a08f01` |
| (-8120, -8224) | `0x00611001` |
| (-7216, -8784) | `0x001b4841` |
| (-9104, -8712) | `0x004a8001` |
| (-6784, -7416), (-7562, -7344) | 0 (slot 1 in its snapshot) |

Slot 1 read slot 0 as `0x005f70c0` from (-8136, -7712) and `0x00ffd340`
from (-7680, -9056), y clamped to 1024 and x scaled from 328 to 312. On the
frame `setPlayerAngles` turned slot 1 to 45, 270 and 180 the top byte read
`0x20`, `0xc0` and `0x80`, and 0 again the frame after, when the probe's
own cmd angles took over. The `0x100000` bit rose on slot 0 one frame after
slot 1's `pingPlayer` and fell one frame after slot 1's own `0x80000`, 3 s
later: slot 0's end frame runs before slot 1 copies its playerstate into
its entity. The field read 0 while slot 1 was axis, came back with it on
allies, stayed through a `suicide()` that left slot 1 `playing` (`pm_type`
0), went to 0 a frame after its `sessionstate` became `dead` (`pm_type` 6),
and stayed 0 with it a spectator. The raw lines are in the probe README's
recipe; the packings are `crates/server/src/compass.rs`'s test.

VERIFIED, the cgame's read (`0x30013646`..`0x300137a5`): a non-zero
`+0x3c0` stamps slot `info & 0x3f`, decodes each offset as `field * 4 -
0x3fc`, tests both against 1024.0 (`0x44800000`) and -1020.0
(`0xc47f0000`), takes the yaw as the signed top byte times 1.40625
(`0x30069600`), and arms the 3 s flash on the playerstate's `eFlags`
`0x100000` (`0x3001378c`). INFERRED, off the two arms: an offset at either
clamp stores the pair normalised (`0x30039d50`) as a bare direction; else
it stores the playerstate's origin plus the offsets plus a lean offset
(`0x3003f4e0`, with 16.0 and 20.0), which vcod leaves out.

VERIFIED, the draw loop (`0x30013830`..`0x30013b9f`) at the defaults
(`cg_hudCompassSize` 1, `cg_hudCompassMinRange` 0, `cg_hudCompassMaxRange`
1024, `cg_hudCompassMinRadius` 0, `cg_hudObjectiveMinAlpha` 1): it resets a
slot whose time is ahead of `cg.time` to 0, skips a slot older than 800 ms
and the viewer's own `clientNum`, and tests the stored x and y against 1.0
(`0x30069328`, a double). INFERRED, off its arithmetic: a stored point sits
as an objective does, `43.75 * clamp(distance / 1024)` from the compass
centre along its bearing off the sprung compass yaw (`0x3020d0a8`,
`0x30019f00`); a stored direction sits at the full 43.75; the icon is `10 *
cg_hudCompassSize` (`0x300693e4`) square; alpha 1. VERIFIED:
`gfx/hud/hud@objective_friendly.tga` and
`gfx/hud/hud@objective_friendly_chat.tga` are registered to `0x301d5d14`
and `0x301d5d18` (`0x30021024`, `0x30021035`). VERIFIED: while the flash
time is ahead, `(flash - cg.time) % 500 >= 250` (`0x30013b20`..`0x30013b32`)
picks the chat icon through the plain pic call `0x300192d0`; otherwise the
friendly icon goes through the turned pic call `0x30019330` with the angle
`ANGLE2SHORT(viewangles[YAW] (0x302095d0) - yaw)` in degrees. INFERRED: the
icon turns as the compass face does, by the viewer's yaw less the
teammate's.

vcod draws the snapshot's teammates off their interpolated origins with the
snapshot's yaw, and the packed one, with the compass turned by the view yaw
(no spring). vcod-server writes `iCompassFriendInfo` and the `0x100000` bit
from each playing or dead client's end frame (`crate::compass`,
`compass_friend` in `server.rs`). Its candidates are the clients whose
`sessionstate` is `playing` and whose sim is a live player, what the probe
saw retail answer for. It reads every teammate's state as of this frame,
where retail reads a higher slot's as its last end frame left it, and skips
the area check, which stock maps never fail.

### Cursor hint

VERIFIED, the registrations: `hintActivate`, `hintNoActivate`, `hintDoor`,
`hintNoDoor`, `hintMg42`, `hintHealth`, `hintLadder`, `hintFriendly` at
`0x301d5ad8` to `0x301d5af4`, four bytes apart; the weapon setup stores a
material at `0x301d5af4 + 4 * weapon` and another at `0x301d5bf4 + 4 *
weapon`. INFERRED, off the setup's branches: the first is the weapon's
`hudIcon`, the second its `ammoIcon`, each `hintActivate` when the key is
blank.

VERIFIED, the stores and the draw's early exit:

| | 1.1 | 1.5 | UO |
|---|---|---|---|
| setup | `0x30034cf0`: `hintActivate` (`0x301d5ad8`) for a blank key at `0x30035c80`, the icon at `0x30035c71`, the ammo slot at `0x30035cdc` | `0x30037602`, `0x30037657`/`0x30037666` | `0x300446d7`/`0x300446e6` (`0x3044b720 + 4 * weapon`), `0x3004473b`/`0x3004474a` (`0x3044b920`) |
| draw loads the hint's slot | `0x3002523f` | `0x300265af` off `0x301d9658` | `0x300303e8` off `0x3044b6f8` |
| leaves on 0 | `0x30025255` | `0x300265c5` | `0x300303fe` |

The exit comes before the icon and the sentence. INFERRED: only
`CG_RegisterWeapon` writes a weapon's slots, so a weapon never set up (no
def) keeps 0 and its hint draws nothing; vcod draws nothing for a weapon
with no def (`hud::player::hint_material`, `hud::uo::hint_material`).
INFERRED, off `0x300251f0` indexing that run from `0x301d5ad0` by the hint:
hints 2..8 are the named ones in order, 9 is `hintFriendly`, `9 + w` weapon
`w`'s hud icon and `73 + w` its ammo icon, matching the server's encoding
(`cod11-items.md` 2.3); 0 and 1 draw nothing. VERIFIED: `hintNoDoor` is not
in pak4's `scripts/hud.shader`, which has `hintDoorLocked` instead.
INFERRED: that hint has no art on retail either.

INFERRED, same function: with `cg_cursorHints` 3 the icon's alpha pulses as
`(sin(cg.time * 0.0066667) + 1) / 2`; a `wideListIcon` weapon's icon draws
twice as wide, shifted left by half the rect. A hint string (configstring
`1212 + serverCursorHintString`, `cod11-gsc-object-model.md`) is localized,
its `[%s]` filled with the use key's binding, and printed at textscale .21
centred on the rect, baseline one text height above it. VERIFIED:
`0x300696d4` reads 0.0066667, `0x30063188` is `"[%s]"`. VERIFIED, the fill
(`0x30025792`..`0x30025899`): the string goes through the `[{command}]`
pass with `"Hint String"`; a copy is case-folded (`0x3003e9b0`) and
searched for `[%s]`; the original up to the `[`, `+activate`'s key text
(`0x3006186c`, through `0x30046940`) or, when unbound, `KEY_USE` localized
(`0x30063160`, key.str `"use"`), and the original from the `]` on are
joined.

The cgame's own sentences. VERIFIED, `0x300251f0` and `cgame.str`:

| branch | strings |
|---|---|
| weapon | `CGAME_PICKUPNEWWEAPON` ("Hold USE [%s] to pick up the %s", `0x3002548c`); `CGAME_SWAPWEAPONS` ("Hold USE [%s] to swap") with `CGAME_FOR` and `"%s %s %s %s"` (`0x300253c1`..`0x30025410`) |
| ammo | `CGAME_PICKUPAMMO` ("Press USE [%s] to pickup the %s ammo", `0x3002560d`) |
| hint 7 | `CGAME_PICKUPHEALTH` ("Press USE [%s] to pickup health", `0x30025942`) |

Each takes `+activate`'s key text from `0x30046940`, which writes the
localized `KEY_UNBOUND` for an unbound command (no `KEY_USE` fallback on
these paths). VERIFIED: the names come from `0x301a69e4 + 0x198 * w`, the
field the weapon name draws (`0x30023c30`).

INFERRED, off the branches: a weapon hint (10..73, weapon `hint - 9`) and
an ammo hint (74..137, weapon `hint - 73`) never read the server's string;
hint 7 reads it first and prints its own sentence only when none is sent.
For a weapon, `0x3000f680` (weapon, predicted ps) returns a free slot: for
slot type 1 or 2 (def `+0x78`) `weaponslots[1]` or `[2]` when empty, for
3..5 the type's own slot, else 0. With one, the pick-up string has the key
text spliced between its brackets (an unfolded `strstr` of `"[%s]"`,
`0x3002549d`) and then formats the name. Without one, the old weapon is the
predicted `ps.weapon` (`0x3020720c`) when its slot type is the new
weapon's, else `weaponslots[type]` (`0x30207470`); when that is the hinted
weapon the function leaves (`0x300253bb`, `0x30025426`) before the icon
draw at `0x300259d7`, so nothing draws. Otherwise the line is the swap
string formatted with the key text, the old name, `for` and the new name.
Ammo splices and formats as the pick-up does; health formats the key text
in. Each line is placed as the server's string is. VERIFIED: 1.5's
`0x30026560` holds the same compares (`0x3002669b`, `0x30026c96`) and
string loads; UO's `0x300303a0` the same with its numbering (weapons
11..138 at `0x300304eb`, ammo 139..266 at `0x3003078d`, health 8 at
`0x30030ae9`; slot types 1..7 in `0x30011520`). vcod:
`hud::player::hint_text`.

### Damage direction

VERIFIED: `0x3002faa0` and `0x30028d00` pass `0x300287f0` the
playerstate's `damageYaw`, `damagePitch` and `damageCount`, and each reads
`damageEvent` from the current and previous playerstate and `damageCount`
from the current one. INFERRED, off each site's compares: the call is made
when the event differs from the previous playerstate's and the count is not
0, the increment the server makes (`cod11-combat.md`).

INFERRED, off `0x300287f0`: yaw and pitch both 255 add no icon; otherwise
the oldest of eight slots takes the time and `damageYaw / 255 * 360` plus a
random jitter of up to 10 degrees either way. VERIFIED, `0x300287f0`..: the
slot's yaw is `SHORT2ANGLE(ANGLE2SHORT((rand() / 32768.0 - 0.5) * 20.0 +
yaw))`, `rand` at `0x3004b189`, constants `0x30069330` (32768.0),
`0x3006930c` (0.5), `0x30069448` (20.0), `0x30069380`, `0x3006937c`. vcod
draws it with MSVC's `rand` sequence from its own seed.

**The red flash.** VERIFIED, `0x30017250`, called from the 2D pass
`0x30018810` at `0x3001884a`, before the hit icons (`0x300172f0`): with a
snapshot up and `cg.time` short of `0x3020c9e0`, it fills (-10, -10, 650,
490) on the 640 grid through `CG_FillRect` (`0x30018d10`, the `white`
shader `0x301d5a84`) in (0.2, 0, 0) at alpha `min(5, |(end - cg.time) *
0.002 * [0x3020c9e4]|) * 0.2 * 0.7` (`0x300694ec` 0.002, `0x30069414` 5.0,
`0x300693d4` 0.2, `0x300694f0` 0.7). VERIFIED: `0x300287f0` stores `cg.time
+ 500` at `0x3020c9e0` after either arm, so every hit flashes, at a
strength set by the kick along the view, 0 for a hit square from the side.
INFERRED: a hit from in front or behind with a kick of 20 holds 0.7 until
125 ms are left and is gone by the 500th. vcod draws it
(`DamageIndicators::build_flash`).

**When the feed runs.** VERIFIED, `0x3002fc0d`..`0x3002fc5e`, the tail of
`0x3002faa0`: when any of `[0x301e2148]`, the new snapshot's `pm_flags &
0x10000`, `[0x301e172c]` or `[0x301e1bac]` is set, `damageEvent` (`+0xe4`)
of the new and old snapshot's playerstate are compared and `0x300287f0` is
called on a change with a non-zero count, then `0x30028740` and
`0x30028be0`; nothing tests `clientNum`, `eFlags` or `pm_type` first.
VERIFIED: `0x30028d00`, called at `0x30029a3f` near the end of
`0x300294f0` (`CG_PredictPlayerState`, `cod11-movers.md`), makes the same
compare and call on the playerstates in `eax` and `esi`. INFERRED: the four
flags are demo playback, `PMF_FOLLOW`, `cg_nopredict` and
`cg_synchronousClients` (Q3's `CG_TransitionSnapshot`); the `eax`/`esi` pair
is the predicted playerstate and the one before it; both sites are
`CG_TransitionPlayerState` inlined. So every frame's prediction, or a
followed snapshot's transition, feeds a hit, mounted, dead or with the 2D
pass off, and a follow switch onto a player whose `damageEvent` differs
feeds that player's last hit. VERIFIED, `0x300287f0`: the kick's dots read
`0x302095a0`..`0x302095b4`, the refdef's `viewaxis[0]` and `[1]`
(`0x30209594` is `refdef.vieworg`, turrets doc 14.3). INFERRED: the
transition runs before `CG_CalcViewValues` rebuilds the axis, so a hit
splits along the view drawn the frame before, kick, quake and turret
included. vcod feeds it from `Hud::feed_damage` every frame before the
snapshot's events, on the newest snapshot's playerstate, along the last
drawn view (`Hud::drawn_view`; the playerstate's view before the first
frame); the first playerstate seen is the only baseline.

INFERRED, off `0x300172f0`: each slot younger than `cg_hudDamageIconTime`
draws `hudHitDirection` over `(-64, 32)` to `(64, 96)` about the screen
centre, turned by the view yaw less the slot's yaw, alpha `min(1, 2 - 2 *
age / time)`. So an icon below the crosshair is a hit from behind.
VERIFIED: the centre is `0x300695e4` (320.0) and `0x300695e0` (240.0). The
turn's sense is inferred from that geometry, not measured.

VERIFIED, `0x300172f0`: the box comes from cvars: half of
`cg_hudDamageIconWidth` (128, vmCvar `0x301dafa0`) either side,
`cg_hudDamageIconOffset` (32, `0x301d9920`) to offset plus
`cg_hudDamageIconHeight` (64, `0x301d9da0`) below the centre, corners
stored `(-w/2, off)`, `(w/2, off)`, `(w/2, off+h)`, `(-w/2, off+h)`; the
defaults are the cvar table rows beside each name. VERIFIED: `0x30017445`
pushes the texture coordinates `0x30060fb4`, `(1,1) (0,1) (0,0) (1,0)` in
that corner order, to `0x300194f0`, which turns each corner `(x cos - y
sin, x sin + y cos)` and hands corners, coordinates and shader to trap
0x4c. INFERRED (trap 0x4c pairs corner i with coordinate i): the art goes
on turned half round, its top row on the far edge. The art
(`gfx/hud/hit_direction.dds` in `pak0.pk3`, shader `hudHitDirection`) is
wide at the top and narrows down, so the wide end faces away from the
crosshair. VERIFIED: 1.5's `cgame_mp_x86.dll` (`075e2af1...`, array
`0x30062fb4` pushed at `0x300184d5`) and UO's `uo_cgame_mp_x86.dll`
(`d83491ca...`, `0x30071854` pushed at `0x3001aad5`) carry the same array
and the same 128/32/64/2000 defaults.

### Mod items

- `0x30022ba0` registers `cg_hudFiles` with default `ui_mp/hud.txt` (trap
  9) and reads it back (trap 0xb). VERIFIED (strings). Its `loadMenu`
  entries load through `0x30022420`, `0x30022350` and `0x30022260`, which
  falls back to `ui_mp/testhud.menu`. INFERRED (call shape). Stock
  `hud.txt` names only `ui_mp/hud.menu`. VERIFIED (`pak0.pk3`).
- `0x3004a6f0` paints every loaded menu whose window flags carry 4
  (`WINDOW_VISIBLE`) and whose owner-draw-flag test passes: the menu's
  window, then each item through `0x30047fb0`. INFERRED (RTCW's
  `Menu_PaintAll` shape). An item with no `text` and a `cvar` prints the
  cvar's value, as RTCW's `Item_Text_Paint` does. INFERRED (lineage).
- Every item of stock `hud.menu` is an owner draw, drawn natively by vcod
  (above). VERIFIED (`pak0.pk3`). A mod's replacement adds plain items:
  `zzz_zfunmod.pk3` (a public server, 2026-10-09) adds menus `fm_corners`
  and `fm_announce` whose `type 1` items read `scr_fm_tl_d`, `scr_fm_bl_d`,
  `scr_fm_mk` and the like, set per client with `v` (seen live: `v
  scr_fm_tl_d "^3King of the Day: ..."`). VERIFIED.
- vcod (`crates/client/src/hud/hudmenu.rs`) loads `hud.txt`'s menus and
  paints the visible items of visible menus that are not owner draws:
  `WINDOW_STYLE_FILLED` and `WINDOW_STYLE_SHADER` backgrounds, then the
  text or the cvar's value (a `v`, else the 140/204 mirror), placed as the
  front end places text (`cod11-front-end.md` section 12), with `cvartest`
  gates, right after the native items on the views that draw the native
  HUD. `textstyle`, `textfont`, borders and item scripts are ignored.

### What vcod does not draw

| Retail piece | Where | vcod |
|---|---|---|
| The packed out-of-view teammate's lean offset | `0x3003f4e0` | left out |
| `adsAimPitch`, shared ammo caps | above | left out, each noted above |
| `cg_drawCrosshairNames`' name under the crosshair | `0x30016f70` | left out |

---

## 10. vcod: wide windows

vcod's own design, not retail's. Retail draws its 2D layer on the 640x480
grid stretched over the window, so anything but 4:3 squashes every HUD
piece and menu sideways. vcod keeps the grid's aspect and places each piece
(`crate::hud::hudelem::Virtual` and `Anchor`).

What it rests on from retail:

- VERIFIED, `ui_mp/hud.menu` (`pak0.pk3`): the stock HUD is plain grid
  rects with no alignment keyword: stance icon at 100,434.375; the
  "weaponinfo" menu spanning 0..640 with the weapon name at 242.5..562.5
  and the ammo counter at 557.5..637.5; health block at 488..631; compass
  at the left; cursor hint at 300,325.
- VERIFIED, the stock gametype scripts (`maps/MP/gametypes/*.gsc`,
  `pak5.pk3`): hudelems carry only `x`, `y`, `alignX`, `alignY`. The clock,
  killcam titles, respawn and spawn texts sit at 305 or 320 centred; the
  bomb and flag progress bar is `alignX "left"` at `320 - barsize / 2`; the
  killcam bars and the black screen are `x 0` shaders 640 wide; `bel`'s
  point counter and its back sit at 505-520, and a right-aligned elem at
  635.

vcod's rules:

- The grid scales to fit the window, centred (`Virtual::new`): by height on
  a window wider than 4:3 (bars at the sides), by width on a narrower one
  (bars above and below).
- With `vc_widescreen 1` (default) a piece retail draws at a screen side
  moves to that side of the window by the side bar's width
  (`Virtual::left`, `Virtual::right`); a piece in the middle stays in the
  box. With `vc_widescreen 0` every piece stays in the box.
- A rect spanning the whole grid width (`hudelem::full_width`: from 0 or
  less to 640 or more) or height stretches over the whole window at any
  `vc_widescreen`: the damage flash, killcam bars, a script's black screen,
  a menu backdrop or fade.
- A piece with no fixed side (a hudelem, a mod's `hud.menu` item) keeps to
  the third of the grid its drawn span's middle is in (`Anchor::of_span`:
  left of 213.3, right of 426.7). An elem's span is its box after `alignX`,
  so the left-aligned progress bar from 192 to 448 stays centred and `bel`'s
  counter at 505 moves right.

The native pieces:

| Piece | Grid position | Keeps to |
|---|---|---|
| Compass, its friends, objectives and UO vehicle marks | -25..135 | left |
| Stance icon, its flash, the stance hints, UO's stance and fatigue | 100..140 and right of it | left |
| UO seat status (in the stance's place) | 100..140 | left |
| Health block (1.1 and UO) | 488..631 | right |
| Weapon name, its back, fire mode, ammo counter and back | 242.5..637.5 | right, as one block |
| Weapon slot strip | from 632 leftwards, top | right |
| Game messages (obituaries, `print`) | from 6, low | left |
| Chat lines and the chat field | from 0 and 8 | left |
| Vote and complaint lines | from 8 | left |
| Crosshair, turret reticle, UO seat crosshair, cursor hint, damage direction icons | centre | centre |
| Prone-blocked text, centre print, bold messages, match timeout text | centred on 320 | centre |
| Scoreboard | 129..511 | centre |
| Damage flash | -10..650 by -10..490 | whole window |
| Scope overlay | window px | already window-wide (`hud::scope::build`) |
| vcod's spectator status header | window px | already window-wide |

The weapon name and ammo move as one block though the name's rect reaches
past the middle, since its back butts onto the ammo backdrop.

Menus, script menus included, draw in the centred box with no edge
anchoring (`frontend::Painter`); their full-width rects stretch as above,
and the cursor maps from window pixels through the box, clamped to its
edges (`Virtual::grid_point`). The raw mouse motion moving it is unscaled,
as on retail (cod11-front-end.md section 13).

The 3D view's fov follows the same cvar (`camera::view_fov_x`, Hor+: every
fov the game names is a 4:3 view's width, and a wider view keeps that
vertical fov). The crosshair's side arms read the fov a 4:3 view as tall
would be wide, at 4:3 retail's `640 / fov_x` (`hud::player::arm_offset`).

## UNVERIFIED summary

| # | Claim | Check that settles it |
|---|---|---|
| 1 | Resolved: the `killIcon<Mod>` names bind to `gfx/hud/death_<name>.tga` through `scripts/hud.shader` (section 2). | n/a |
| 2 | Resolved: scores token 4 is `deaths`, written by script (section 3). | n/a |
| 3 | Resolved: the `b` grammar, confirmed on screen against a populated server's Tab scoreboard (section 3). | n/a |
| 4 | Resolved: hudelems travel in the playerstate's array block 5, filled per client by `HudElem_UpdateClient` (section 5); the cgame's draw is section 8. | n/a |
| 5 | The second font header float's id-internal field name (section 6). Its use is settled. | Nothing downstream depends on it. |
| 6 | Moot: the status header is vcod's own; the stock client draws team totals only on the scoreboard (section 5). | n/a |
| 7 | Trap `4`'s official name (section 1). Its signature and engine case are read (section 1.1). | The name is in no binary read so far. |
