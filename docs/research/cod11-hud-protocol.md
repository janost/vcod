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
| `pak0.pk3` | | `weapons/mp/*` weapon defs, `ui/assets/reticle_q.tga` |
| `pak4.pk3` | | `scripts/ui_hud.shader`, `scripts/hud.shader` |
| `cgame_mp_x86.dll` from the 1.5 install | `075e2af18aeaf2aeaf2a75ce22db683a` | 1.5 diff (icon names unchanged) |
| live: a public TDM server, mp_carentan, 2026-08-24 | | `serverCommand` stream, gamestate |

Addresses are virtual: image base `0x30000000` for the cgame DLL, `0x00400000`
for `CoDMP.exe`, file-relative for the Linux `.so`. The cgame DLL and
`CoDMP.exe` facts come from their Ghidra decompilations (exported with
`tools/re/ExportDecomp.java`). The `.so` was read with `objdump -d -M intel`
plus `readelf -r` to resolve symbol references.

Recurring trap numbers (the cgame's syscall pointer is `0x30074898`):
`4` add-kill-message, `9` `Cvar_Set`, `0xb` `Cvar_VariableStringBuffer`,
`0xc` `Argc`, `0xd` `Argv`, `0x18` `SendClientCommand`, `0x4f` `GetGameState`,
`0x30` register-material (`RE_RegisterShader`, mipmapped; the tail of the
loading-screen wrapper `0x30030b70`, which is what the precache calls go
through), `0x58` `RE_RegisterShaderNoMip`, used for scoreboard status icons,
levelshots and script `precacheShader` materials (`cod11-front-end.md`
section 14, "2D registrations").

---

## 0. CoD 1.1 server commands are single letters

`CG_ServerCommand` @ `0x3002e0d0` switches on `argv0[0]` and never compares
the whole token, so every server command in 1.1 MP is one character. This is why
grepping a capture for `scores` or `cs` finds nothing.

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

Live confirmation from the capture (`serverCommand N:` lines):

```
serverCommand 3: f "vcod ^7Connected"
serverCommand 4: d 0 \codextended\CoDExtended v20\...\g_gametype\tdm\...\mapname\mp_carentan\...
serverCommand 5: v g_scriptMainMenu "team_americangerman"
serverCommand 8: t 0
serverCommand 13: d 5 118
serverCommand 14: d 6 131
serverCommand 29: i "^1Server:^7 <advert text>"
```

`d` carries the new configstring text as argv[1]; the cgame handler ignores it
and re-reads the applied table via `trap_GetGameState` (`0x4f`), exactly as Q3's
`CG_ConfigStringModified` does.

`v` sets the client's own cvar, the one the console, the HUD and the
config read; there is no separate store for server-set cvars. VERIFIED:
the `v` arm of 1.1's `CG_ServerCommand` copies argv[1] and passes it with
argv[2] to `0x30030550`, which is trap 9; UO's arm calls `0x3003d570`,
also trap 9. VERIFIED: CoDMP.exe's cgame trap dispatcher (`0x401e22`,
table at `0x4041f0`) sends trap 9 to `0x402331`, which calls `Cvar_Set2`
(`0x439650`) with `eax` 1, the `force` path: no read-only, cheat or latch
check, and an unregistered name is made with no flags (cod11-console.md
5). INFERRED that UO's engine does the same (no CoDUOMP.exe trap table was
read). INFERRED, off Q3's `cvar_modifiedFlags`: a `v` of a userinfo cvar
sends a userinfo change and one of an archived cvar rewrites the config.
The 140/204 mirror's writer (`0x3002bc60`, section 5) is a trap 9 too;
vcod keeps the mirror apart (`play::cvars::ClientCvars`) and lets a name
the server set with `v` this connection win over it.

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

| entityState offset | Q3/CoD field | vcod netfield (`crates/common/src/net/cod1/fields_v1.rs`) | bits | Carries |
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
centre print is produced from one of two localized formats:

- `"CGAME_YOUKILLED\x15%s"` (`0x30064c3c`) for a normal kill
- `"CGAME_YOUKILLED\x15^1%%s^7 %s\x14%s"` + `"CGAME_TEAMMATE"` (`0x30064c50`)
  when the attacker's team is non-zero and equals the victim's team.

### The centre print

Read with `objdump -d -M intel` (the Ghidra project was not on hand), 1.1
`cgame_mp_x86.dll` unless named.

- VERIFIED: `CG_Obituary` compares the attacker with
  `cg.snap->ps.clientNum` (`[0x301e2160] + 0xb8`, `0x3001d985`), so "local"
  is the drawn playerstate: a 1.1 or 1.5 spectator following the killer
  sees the line too. INFERRED, off the branch at `0x3001d968`: a suicide
  (attacker equals victim) and a world attacker (`0x3fe`) never print.
- VERIFIED: the `%s` is the victim's clientinfo name, cut to 31 bytes
  (`strncpy(..., 0x1f)`, `0x3001d861`) with `"^7"` appended. The format goes
  through `va` (`0x3003eb40`), then `CG_PriorityCenterPrint` (`0x300159e0`)
  with priority 1 (`EAX`), y 400.0 (`0x43c80000`) and character width 9.6
  (`0x4119999a`), pushed at `0x3001d9c4`..`0x3001d9d4`.
- VERIFIED, `localized_english_pak1.pk3` `localizedstrings/english/cgame.str`:
  `YOUKILLED` is `"You killed %s"`, `TEAMMATE` is `"TEAMMATE"`; 1.5's
  `localized_english_pak5.pk3` and UO's `localized_english_pakuo01.pk3` carry
  the same two lines. So a kill reads `You killed Bob`, a team kill
  `You killed ^1TEAMMATE^7 Bob`.
- VERIFIED: `0x300159e0` returns without printing while a print is up
  (`[0x3020c488]` non-zero) and its priority (`[0x3020c898]`) is above the
  new one. It passes the text through `0x300229b0` with `"Center Print"`
  (`0x30065308`), the `[{command}]` pass over a localized message, copies at
  most 1023 bytes to `0x3020c494`, and wraps: INFERRED, off its loop at
  `0x30015a40`, once a line has run 75 (`0x4b`) characters its next space
  becomes `\n`. VERIFIED: it stores `cg.time` (`[0x30207148]`) plus 2000
  (`0x7d0`) as the print's start (`0x3020c488`), y and the character width
  through `_ftol` (`0x3020c490`, `0x3020c48c`), and counts lines from 1, one
  per `\n` and one per literal `\` `n` pair (`0x3020c894`).
- VERIFIED: `CG_DrawCenterString` (`0x30015b40`) fades with `CG_FadeColor`
  (`0x30019a30`) over `cg_centertime * 1000` (vmCvar `0x301dbf60`, default
  `"5"`, flags `0x200`, the table row at `.data` `0x30074d60`). `CG_FadeColor`
  returns nothing once `cg.time - start` reaches the total and fades over the
  last 100 ms (`0x64`, times 0.01) in white times `cg_hudAlpha`. With the
  2000 added to the start, the line holds 7 s in all.
- VERIFIED: the draw takes font 0 at the character width times 0.03125
  (`0x30069504`, 0.28125 for 9), reads the height off trap `0x35` as an int,
  and starts at `y - height * 0.5 * lines - height` (`0x3006930c` 0.5),
  each line `height * 1.2` (`0x30069500`) below the last. Each line is at
  most 75 bytes, its width trap `0x34` (an int), at x `(640 - width) * 0.5`
  (`0x300694fc`), drawn by trap `0x36` with style 3. INFERRED: the y is the
  baseline, as for every trap `0x36` draw (section 8).
- VERIFIED: `CG_Draw2D` (`0x30018810`) returns for `pm_type` 5 before the
  print, and calls `CG_DrawCenterString` (`0x300188ed`) only when
  `0x3002b650` returns 0. INFERRED: `0x3002b650` is `CG_DrawScoreboard`
  (it fades and clears the killed-by name at `0x3002b681`), so the print
  hides under the scoreboard.
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
  carries `0x40000`, UO's follow bit (`0x3002255e`): a UO follower sees no
  line. The draw constants, the wrap, the 2000 and `cg_centertime` `"5"`
  match 1.1.

vcod: `crates/client/src/hud/centerprint.rs` and `obituary_center_print` in
`hud/mod.rs`. It keeps no priority, since the obituary is the only caller.

### 1.1 Where the kill line goes

Read with `objdump -d -M intel` of 1.1 `CoDMP.exe` and the Ghidra
decompilations of 1.5 `CoDMP.exe` and UO `CoDUOMP.exe`.

- VERIFIED: trap 4 is case 4 of the cgame syscall switch (`0x401e22`, jump
  table `0x4041f0`, entry `0x402132`). It reads its nine arguments off the
  argument block in the order section 1 lists and writes them into the
  console line buffer as one line, as `0x409590` writes text and
  `0x409490` a colour.
- VERIFIED, the line it writes: when the attacker name's first byte is
  non-zero, three colour cells 10, 11, 12 carrying the attacker colour's
  r, g, b times 255 (`0x4021ca`, `0x568ec0` is 255.0), the name at colour 7,
  then `" "` (`0x568a9c`); then cells 13, 14, 15 with the third colour
  (`0x4021f3`); a cell `0x1000 | _ftol(width * 32)` and a cell
  `0x1100 | _ftol(1.4 * 32)` (`0x568ee4` is 32.0, `0x402201`, `0x402212`);
  the icon's material name at colour `0x12`; `" "`; cells 10..12 with the
  victim colour; the victim name at colour 7.
- VERIFIED: `0x409590` called with a colour of 0 or more skips every `^0` to
  `^7` pair without changing the colour, and drops `\n` and `\r`. Each name
  therefore draws whole in its team colour, `CG_Obituary`'s own `^7` gone.
- VERIFIED: the line ends through `0x408d00` with `edx` 1 (`0x4022bf`),
  whose switch (`0x408c90`, case 1) adds it to the window record at
  `0x142efcc`, the one trap 0x1a draws (`0x409e70`): the game message window
  of `cod11-chat.md` 3.3, lower left above the compass at x 6, font 0, scale
  0.25. Its life is `con_gamemessagetime` times 1000 (`0x402188`), the same
  5 s as an `iPrintLn`, and it takes a ring slot like any game message.
- VERIFIED: 1.5 `CoDMP.exe` has the same case: `0x4022b5` writes the
  `0x1100` cell and the line ends through `0x4094e0` with `edx` 1, as 1.1's
  does. VERIFIED for UO: `CoDUOMP.exe`'s case 4 writes the `0x1100` cell
  at 0x4022e3 and ends the line through 0x409b40 with `edx` 1
  (0x402337), the game window. UO's `CG_Obituary` calls trap 4 with the same
  nine arguments and adds a MOD `0x1a` row, `gfx/icons/hud@bino_owned`.
- VERIFIED, the draw (refexport slot `0xdc`, `0x4dfa60`; the refexport is
  copied at `0x411ac5` from the table `0x4b4a30` fills at `0xca26c8`): the line splits at the first cell
  of colour 13, `0x10`, `0x11` or `0x12` (`0x4df5b0`), colours 10..12 set the
  running text colour's rgb (times 1/255, `0x568ec4`) and the alpha stays the
  window's. Each text run draws at `x + advance` and adds its width, an int
  (`0x4df310`). The icon cells (`0x4df750`) give width and height as
  `scale * 48 * code * 0.03125` (`0x568ed4`, `0x568ea0`) and the colour from
  13..15. The icon draws at `x + advance`, top
  `baseline - (scale * 38.4 + h) * 0.5` (`0x5690cc`, `0x568e70`), and adds
  its width to the advance.
- INFERRED, from the codes above at the game window's scale 0.25: an icon
  is 16.5 units square, a wide one 33.375 by 16.5, centred 4.8 units above
  the text baseline. Nothing pads the icon: the spaces either side are the
  line's own.
- VERIFIED: `CL_InitCGame` (after the `"CL_InitCGame: %5.2f seconds\n"`
  print) empties the game, bold and third message windows, so a new
  gamestate drops every kill line.
- VERIFIED: `CL_GetServerCommand`'s `n` case (1.1 `CoDMP.exe` 0x4017c3,
  index 10 of the byte table at 0x401824 off `'d'`, jump table 0x401808)
  calls 0x4087b0, which zeroes the rings of the game window (0x142efcc)
  and window 2 (0x142f050, the bold window: trap 3, 0x401fc2, writes
  window 2), then zeroes window 3 (0x142f0d4) itself
  before the cgame sees the command. The switch at 0x408c90 maps window
  0 to 0x142f5a8, 1 to 0x142efcc, 2 to 0x142f050 and 3 to 0x142f0d4.
  VERIFIED: 1.5's (`CoDMP.exe` 0x401620, calling 0x408ce0) and UO's
  (`CoDUOMP.exe` 0x401630, calling 0x409340) take `B` and `n` into the
  same case. So a `map_restart` empties the game and bold windows, kill
  lines included. vcod: `Hud::on_map_restart`.

vcod: `Hud::on_game_event` builds a `messages::KillLine` and pushes it into
the game message window; `messages::kill_line` draws it. There is no
separate kill feed.

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
| `0x90` | `MOD_WATER` (16) | `killIconDrown` | `gfx/hud/death_died.dds` |
| `0x91` | `MOD_SLIME` (17) | `killIconSlime` | `gfx/hud/death_died.dds` |
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

VERIFIED, the `killIcon<Mod>` to image binding: each name is a shader in
`pak4.pk3:scripts/hud.shader` (1.5's `pak8.pk3` carries the same blocks)
whose one stage is `map clamp gfx/hud/death_<name>.tga`, `blendFunc blend`,
vertex colour and alpha. `killIconDrown`, `killIconSlime` and
`killIconDied` all map `gfx/hud/death_died.tga`, so a drown or slime death
draws the generic icon. VERIFIED: 1.5's `cgame_mp_x86.dll` switch
(0x3001e9f5) has 1.1's eight cases. United Offensive's is in
`coduo-diff.md` 12.

vcod: `killfeed::mod_icon` names the material each case names, per
protocol, and the HUD resolves it through the shader scripts' image map
like any HUD material.

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
ranks higher", not Q3's `pers.enterTime` tiebreak. A retail client
renders it in the scoreboard's deaths column (a hand check saw minutes there
when vcod sent the Q3 reading).

### 3.1 How the cgame draws the board

Addresses are 1.1 `cgame_mp_x86.dll`; constants are floats read out of its
`.rdata`. Coordinates are the 640x480 virtual screen. Text goes through
trap `0x36` in font 0 at a baseline, width through `0x34`, font height
through `0x35`; colour codes in names pass through.

**Entry and gates.** VERIFIED, `0x3002b650`: returns 0 without drawing
when `0x3029944c` is set; with `+scores` held (`0x3020c030`) the alpha is
1, otherwise `0x30019a30(100, 0x3020c038)`, the 100 ms fade from the
stamp `-scores` writes. VERIFIED, `-scores` (`0x30013cc0`) and the `n`
handler (`0x3002ca60`): when held, clear `0x3020c030` and stamp
`0x3020c038` with `cg.time`. VERIFIED, `+scores` (`0x30013ce0`): when more
than 2000 ms passed since the last request (`0x3020b9f8`) it sends `score`
and restamps, and if the board was down also zeroes the row count
(`0x3020b9fc`) and the scroll position (`0x3020c03c`); either way it sets
`0x3020c030`. A press inside the 2 s sends nothing. VERIFIED: it sends `score`
every 2000 ms (section 4), then calls the frame `0x30029ac0` and the table
`0x3002b410`, and returns 1. VERIFIED, `0x30018530`: the intermission
draw (`pm_type` 5) restamps `0x3020c038` with `cg.time` every frame and
calls `0x3002b650`, so the board is up all intermission without the key.
VERIFIED, the 2D pass `0x30018810`'s two calls of `0x3002b650`: while
it returns 1 the cgame skips `0x30018550` (the spectator's
`+attack` / `+melee` hints), `0x30018710` (`"spectator follow string"`),
`0x30015b40` (the centre print), `0x30017880` and `0x30017940` (the game
and bold message windows, `cod11-chat.md` 3.3) and the trap `0x1c` and
`0x1d` draws (`0x300179f0`, `0x30017a80`). INFERRED: chat (trap `0x1e`)
stays, drawn at `LAB_30018913` outside both gates.

**Frame.** VERIFIED, `0x30029ac0`: `black` at alpha `0.55 * a`
(`0x300696f8`) over (120, 22, 400, 430) (`0x30069690`, `0x30069560`,
`0x30069528`, `0x300696f4`); `white` at `0.1 * a` (`0x3006953c`) over the
six rects of the table at `0x30060d88`: (123, 25, 394, 2), (123, 447, 394,
2), (123, 27, 2, 420), (515, 27, 2, 420), (125, 51, 390, 1), (125, 432,
390, 1). VERIFIED: the title row draws the game type's name (trap `0x7e`
on `g_gametype`, localized through trap `0x39` `"scoreboard gametype
display"`) at x 129, scale 0.41 (`0x3ed1eb85`), baseline `51 - (24 - h) *
0.5` (`0x300696f0`, `0x3006930c`) with `h` trap `0x35`'s height; then the
map, `mapname` with `maps/`, `mp/` and `.bsp` stripped, right-aligned to
end at `511 - 4` (`0x300696e8`). The map's scale starts at 0.41 and drops
by 0.025 (`0x300696ec`) while the two widths plus 4 pass 386 (`0x182`) and
the scale is above 0.075 (`0x30069648`); the game type keeps 0.41.
INFERRED: trap `0x7e` returns the game type's display key, the first token
of `maps/mp/gametypes/<g_gametype>.txt` that `UI_LoadGameTypes` reads
(`TDM_TEAM_DEATHMATCH`); vcod uses that list and falls back to the raw
`g_gametype`. VERIFIED: the footer draws `sv_hostname` (`0x301d1ffc`,
filled by `CG_ParseServerinfo` `0x3002bbb0`) at x 129 and trap `0x7f`'s
string right-aligned to `511 - 4`, both at a scale from 0.2 (`0x3e4ccccd`)
down by 0.01 (`0x300693f4`) on the same 386 rule, baseline `447 - (14 -
h) * 0.5` (`0x300696e4`); a trap `0x7f` string equal to `"0.0.0.0:0"`
(`0x30062e84`) is replaced by `CGAME_LISTENSERVER` ("Listen Server").
INFERRED: trap `0x7f` is the server's `ip:port`. vcod prints the address
its socket is connected to.

**Objective.** VERIFIED, `0x3002a1e0` with y 52 (`0x42500000`): nothing
when `cg_objectiveText` (`0x301db670`) is empty. Otherwise y += 4; the
localized text (trap `0x39` `"scoreboard objective info"`) is cut greedily
at the last space before a line reaches 375 (`0x177`) units at scale 0.24
(`0x3e75c28f`), each line at x 129, baseline `y + 9` (`0x30069760`), 12
apart (`0x300693fc`); a `\n` or a literal `\` `n` breaks a line, and a
space opening a line is skipped. Then y += 2, `white` at `0.1 * a` over
(125, y, 390, 1), and y + 1 is returned. INFERRED: the levelshot and
loading bar between the text and the rule draw only while `0x301e2160` is
0, before the first snapshot, never over a live board.

**Columns.** VERIFIED, the five 16-byte rows at `0x30060d38` (kind, share
of the table's width, header key, alignment 2 = right):

| Kind | Shows | Share | Header | Align |
|---|---|---|---|---|
| 4 | status icon | 0.05 | | left |
| 0 | name | 0.41 | | left |
| 1 | score | 0.16 | `CGAME_SB_SCORE` | right |
| 2 | deaths | 0.20 | `CGAME_SB_DEATHS` | right |
| 3 | ping | 0.18 | `CGAME_SB_PING` | right |

VERIFIED, `0x3002a650` at `y + 4` (`0x300693dc`): the headers with a key
at scale 0.3 (`0x3e99999a`), baseline +10.5 (`0x300695b8`), right-aligned
in their column, white; it returns y + 14 (`0x300696e0`). Columns start at
x 129 and each takes `share * width`.

**Width.** VERIFIED, `0x3002b410` and `0x3002a740`: the board asks 10
(`0x300693e4`), or 66 (`0x30069404`) when axis or allies has a row, plus
28 (`0x30069400`) for each of the none and spectator blocks that has one,
plus 12 a row; the table is 382 wide (`0x43bf0000`) when that fits
between the objective's bottom and 432 (`0x300693f8`), else 374
(`0x43bb0000`) with the scroll bar.

**Order.** VERIFIED, `0x3002b410`: when axis or allies has a row, the
leader by the `b` totals (axis `0x3020ba04`, allies `0x3020ba08`) goes
first and the other second, a tie keeping the last frame's first
(`0x3007497c`, 2 = allies in the image); each block is followed by 4. Then
team 0's block if it has a row, then the spectators'. VERIFIED,
`0x3002b920`: a row's team is its client's if the client has info
(`0x3018bc0c`), else 0, and each team's average ping is its rows' ping sum
over its row count, 0 when either is under 1. VERIFIED, `0x3002b0d0`:
rows draw in the order the `b` line carries them, so the server's sort is
the board's; a row whose client has no info is skipped.

**Banner.** VERIFIED, `0x3002a970`: the block stops at `y + 24 > 432`
(`0x3006940c`, `0x300693f8`). The banner is the material in
`g_ScoresBanner_None`, `_Axis`, `_Allies` or `_Spectators`, drawn white at
`a` over (129, y, width, 32). Its title, at x 133 (`0x43050000`), baseline
`y + 18` (`0x300694d8`), scale 0.32 (`0x3ea3d70a`), in the team's colour
(`0x3002a810`: `g_TeamColor_Axis` / `_Allies` read `"%f %f %f"` and
clamped to 0..1, white for teams 0 and 3), is `CGAME_SB_PLAYER` (count 1,
`0x30062df4`) or `CGAME_SB_PLAYERS` with the count, for team 0 alone, and
`"%s (%s)"` of `g_TeamName_Axis` / `_Allies` (localized) or
`CGAME_SPECTATORS` and that count for the others. Axis and allies also
print the team's `b` total and average ping (`"%i"`) in the score and ping
columns, same baseline, scale and colour. Rows start at `y + 24`.

**Row.** VERIFIED, `0x3002ace0`: stops at `y + 12 > 432`. The local
client's row (`0x301e2144`, `CG_Init`'s client number, not a followed
player's) gets `white` in the team's colour at `0.2 * a` (`0x300693d4`)
over (129, y, width, 12); otherwise every second drawn row of a block gets
`black` at `0.15 * a` (`0x300695c0`), the first row plain. The icon is
13x13 (`0x41500000`) at the column's x, white. The text columns draw at
scale 0.22 (`0x3e6147ae`), baseline `y + 10.5`, white at `a`, cut a
character at a time from the end until the text fits its column; a
spectator's (team 3) score and deaths are blank. The row returns y + 12.
VERIFIED, `0x3002b920`: a status icon 1..8 names configstring `20 + n`
(UO 1..16, `21 + n`), registered with trap `0x58`.

**Scrolling.** VERIFIED: every banner and row counts as one item. Items
under the scroll position (`0x3020c03c`) are skipped without moving y; the
first item that does not fit sets `0x3020c040` and nothing after it draws.
VERIFIED, `0x30027180`: while `+scores` is held a key-down of 206, 164 or
184 (`MWHEELUP`, `PGUP`, `KP_PGUP`) lowers the position by
`cg_scoreboardScrollStep` (record `0x30075110`, default `"3"`, archived)
to no less than 0, and 205, 163 or 190 (the downs) raise it through
`0x3002b720`, only when `0x3020c040` is set, to at most the row count
minus 1; both are eaten. VERIFIED, `0x3002b410`: after the draw the
position is held to the item count minus 1.
VERIFIED, `0x3002b170` (with the list's top `t`, the item count `n`, the
items reached `r` and the position `s`): `black` at `0.5 * a`
(`0x3006930c`) over (505, t, 8, `432 - t - 1`); a thumb of `white` at
`0.25 * a` (`0x3006950c`), x 506 (`0x300695b0`), 6 wide (`0x30069314`),
from `t + 1 + s / n * k` where `k` is the track's height less 2, `k * (r -
s) / n` tall when `r - s > 1` and `n > 1`, else `k`. INFERRED: `k` reads
the height left on the x87 stack by the track's draw. VERIFIED: with `s >
0` the 16x16 (`0x41800000`) `hudScoreboardScroll_UpArrow` at (521, t) and
`_UpKey` at (521, t + 18); with `r < n` `_DownArrow` at (521, 415) and
`_DownKey` at (521, 397) (`0x44024000`, `0x43cf8000`, `0x43c68000`).

**1.5 and United Offensive.** VERIFIED, 1.5 `cgame_mp_x86.dll`
(`0x3002af40`..`0x3002cbe0`) and UO `uo_cgame_mp_x86.dll`
(`0x300361d0`..`0x30037e90`): the same functions with the same constants
and the same column table (`0x30062d38`, `0x30071a60`) and frame rects
(`0x30062d88`, `0x30071ab0`); the traps renumber (UO's register is
`0x59`, its game type and address `0x80` and `0x81`). Both add a gate to
the entry (1.5 `0x3002cae0`, UO `0x30037d90`): no board while the
`timescale` vmCvar's integer is 0 (records `0x30076ed0` and `0x30085770`),
a 1.5 match timeout (`cod15-protocol-diff.md`). UO's status icons run 1..16
(`0x30038060`).

**As implemented** (`crates/client/src/hud/scoreboard.rs`). All of the
above, in vcod's centred 640x480 space rather than retail's stretched one.
`cg_hudAlpha` is not applied to the fade. Not drawn: the loading levelshot
and bar in the objective step.

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
| 15 | `Cmd_CallVote_f`, `CheckVote` | vote time (ms): 1.1's end in level time, 1.5's ms left (`cod11-votes.md`) | `0x301d21c0`, drives the `(t - cg.time)/1000` countdown in `0x30018120` |
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

This agrees with the block map in `crates/common/src/net/cod1/mod.rs`, which
was built from a live gamestate dump on 2026-08-23. The live probe on
2026-08-24 reported `354 configstrings` set on mp_carentan, and `d 5 <n>` /
`d 6 <n>` arrived continuously during the TDM round with both counters
climbing (5: 117 to 129, 6: 126 to 153 over 90 s). All 42 `d` commands in that
window went to index 5 (13 times), 6 (28 times) or 0 (once); nothing else
changed mid-round.

VERIFIED, `CG_ConfigStringModified` @ `0x3002c6b0`: index 8 jumps to
`0x30036150` (`0x3002c70a`), the same item walk the gamestate runs, so a
mid-map CS 8 update registers the newly marked items' weapons (the walk
reaches `CG_RegisterWeapon` `0x30034cf0` through `0x30036080`). INFERRED,
from the compares at `0x3002c70a`-`0x3002c927`: CS 7 has no case there,
so the weapon table is read only at the gamestate. VERIFIED: the cgame's
weapon setup (`0x3000ef10`) parses configstring 7 out of its gamestate copy
(string data `0x301ce0a0`, offset slot `0x301cc0bc`) when syscall 199 says
the weapon memory is not already loaded; it is called from the gamestate's
init (`0x3002310b`) and from `CG_MapRestart` (`0x3002ca91`), the latter
only behind a non-zero `sv_running` (`0x301d1fd8`, stored from the cvar
at `0x30020652`) after syscall 200 releases the memory.
INFERRED: a client of a remote server builds its weapon table once per
gamestate, and a mid-map `d 7` changes nothing until the next map load.
vcod builds `LivePhase::weapons` at the gamestate and ignores a
`ConfigstringChanged(7)`. vcod re-runs its
viewmodel prewarm on each CS 8 update (`prewarm_viewmodels`,
`crates/client/src/main.rs`); rigs already cached cost a lookup, and parsed
weapon files, models and clips are shared by name across rigs
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
`Scr_AllocString("allies")` and `scr_const+8` with `Scr_AllocString("axis")`.
So CS 6 = allies, CS 5 = axis, and `level.teamScores[1]` is axis while
`level.teamScores[2]` is allies, matching the `g_TeamColor_Axis` (team 1) /
`g_TeamColor_Allies` (team 2) split the cgame uses in section 1.

`DeathmatchScoreboardMessage` pushes `level[+0x1fc]` before `level[+0x200]`, so
the `b` command's token 2 is axis and token 3 is allies, in the same order as
CS 5 then CS 6.

### Header vs. scoreboard totals can disagree in the same frame

A live capture (2026-08-24, a public TDM server, mp_chateau) caught the status
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

### A client's elements die with it

VERIFIED, `game.mp.i386.so` with relocations resolved
(`tools/re/annotate_func.py`): `ClientDisconnect` (0x42aac) calls
`HudElem_ClientDisconnect` (0x4c0ec) at 0x42be1 with the player's
`gentity_t`, then `Scr_PlayerDisconnect` (0x5c9ec, which runs
`CodeCallback_PlayerDisconnect`) at 0x42bed, then `G_FreeEntity` at 0x42bfc.

VERIFIED, `HudElem_ClientDisconnect`: a loop over all 0x400 `g_hudelems`
records, 0x7c bytes apart, that skips a record whose `type` (`+0x0`) is 0,
compares its owner (`+0x70`) with the entity's first dword (`s.number`), and on
a match calls `Scr_FreeHudElem` (0x4c11e) and stores 0 to `type` (0x4c123).
INFERRED: a `newClientHudElem` record is freed with its client, and a
`newHudElem` / `newTeamHudElem` one (owner `0x3ff`) never is.

VERIFIED, `HudElem_DestroyAll` (0x4c150): the same walk without the owner
test, then a `bzero` of the whole 0x1f000-byte pool; its only caller is
`G_ShutdownGame` (relocation at 0x4ff53). INFERRED: a level boundary empties
the pool, which vcod gets from building the script runtime afresh.

VERIFIED, live, 2026-10-06, retail 1.1d dedicated server on mp_pavlov with
`client-probes/probe_hud_disconnect.gsc` as the gametype and one
`vcod --net-probe` client that connected and left (`games_mp.log`):

```
  3:06 PROBE connect 0 own 1 shared 1
  3:13 PROBE disconnect_callback own 0 shared 1
  3:13 PROBE next_frame own 0 shared 1
```

The client's own element is already gone when the callback runs; the shared
one survives. vcod's server printed the same three lines after the fix.

INFERRED: without the free, a stock `dm` client that leaves while dead keeps
its `respawntext`. The disconnect kills `waitRespawnButton`'s threads, so
`removeRespawnText` never runs, and the next client into the slot has the
owner's entity number and is sent the element. That was vcod's bug until
2026-10-06 (`crates/server/tests/hud.rs`,
`a_disconnect_frees_the_clients_own_elements`).

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
| 8 | float | 1.0 | 12.0 | 13.0 | 3.0 | `top`: baseline to glyph top, px |
| 12 | float | 0.0 | -0.333 | +0.333 | +1.0 | horizontal bearing, scales with font size |
| 16 | float | 3.859 | 10.667 | 4.667 | 4.333 | advance / xSkip |
| 20 | int | 0 | 12 | 4 | 4 | `imageWidth` (duplicates +4) |
| 24 | int | 0 | 11 | 12 | 2 | `imageHeight` (duplicates +0) |
| 28,32,36,40 | float | | | | | `s, t, s2, t2` |
| 44 | int | 39 | 39 | 39 | 39 | glyph handle (overwritten at load) |
| 48..79 | char[32] | `fonts/fontImage_0_16.tga` | | | | shader name |

So Q3's `height/top/bottom/pitch/xSkip/imageWidth/imageHeight` block became
`height / width / top / bearing / advance / imageWidth / imageHeight`,
with three of the seven promoted from `int` to `float`. The `s/t/s2/t2/glyph/
shaderName` tail is unchanged from Q3.

`top` is `height + 1` only on glyphs that end on the baseline. VERIFIED
(1.1 `pak5.pk3` `fontImage_16.dat`): `P` reads height 11, top 12; `p`, `y`,
`g` and `q` read height 11, top 9; `,` height 5, top 3; `Q` height 14, top 12.
VERIFIED, `CoDMP.exe`: the text renderer `0x004d7d60` draws each glyph quad
at `y - scale * glyph[+8]`, `y` being the baseline it is passed, with
`imageWidth` and `imageHeight` (+20, +24) as its size. A renderer that
bottom-aligns glyphs on `height` lifts every descender to the baseline and
draws `p` as `P`, `y` as `Y` and `g` as `8`.

The shader name is per glyph, and the bigger fonts span several atlas
images. VERIFIED (1.1 `pak5.pk3`, the +48 names over all 256 records):
`fontImage_12`, `_16` and `_18` name only `fonts/fontImage_0_<size>.tga`;
`fontImage_24` splits 151/104 over `_0_24` and `_1_24`; `fontImage_30`
splits 92/98/65 over `_0_30`, `_1_30` and `_2_30`; `fontImage_32` splits
85/86/84 over three pages, with `A`..`T` on `_0_32` and `U`..`Z` and `o` on
`_1_32`. One record per font has an empty name. A renderer that draws every
glyph from page 0 garbles the 24, 30 and 32 point fonts; vcod reads the page
per glyph (`hud/font.rs`, `Font::glyph_page`).

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
literally; 1.5 and UO add `^8` and `^9`, below), `^^` escapes a caret, and alpha always comes from the caller;
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

### `^8` and `^9` on 1.5 and UO

VERIFIED, the renderer: CoDMP.exe 1.5's text draw (0x4bab80..0x4babdf)
and CoDUOMP.exe's (0x4c0581) take a `^` followed by `'0'..'9'`
(`0x30..0x39`), not `^`, as a code. `^7` restores the caller's colour in
full, as on 1.1; any other code goes through 0x4ba8e0 (UO 0x4c02f0), and
the caller's alpha byte is written over its result (0x4babd3, UO
0x4c05d4).

VERIFIED, 0x4ba8e0 / 0x4c02f0: digits 0..7 read `colorTable` (1.5
0x57ed54, UO 0x590e9c, the same eight dwords as 1.1's), `8` reads the
dword at 0x199a2f8 (UO 0x489a5b8) and `9` the one at 0x199a2fc (UO
0x489a5bc).

VERIFIED, those two dwords: 0x405440 (UO 0x405cb0) reads
`g_TeamColor_Allies` (string 0x5a7fe0, UO 0x58e694) into the floats at
0x1493bf0 (UO 0x49637ac) and `g_TeamColor_Axis` (0x5a7fcc, UO 0x58e680)
into 0x1493c00 (UO 0x49637bc) through 0x4052f0 (UO 0x405b50), then packs
each into bytes, Allies into the `^8` dword and Axis into the `^9` one.
0x405b50 looks the cvar up (0x43d8f0); a cvar that is missing or empty
gives 1, 1, 1, otherwise `sscanf("%f %f %f")` (0x58e6a8) writes over the
floats it scans and leaves the rest at their last value. Each component
is clamped to 0..1 (constants 0x5b9b3c 0.0, 0x5b9b38 1.0) and alpha is 1.
VERIFIED, the caller: the client frame calls 0x405440 at 0x412762 (UO
0x405cb0 at 0x4138bf, in 0x413870), so the colours follow the cvars
every frame.

INFERRED, off that caller and the renderer: the codes belong to the
exe, not to the server, so a 1.5 or UO client's front end takes `^8`/`^9`
before any game, in white while neither cvar exists. Whether a mirrored
value outlives the disconnect is not measured; vcod draws the front end
white on a 1.5 or UO install and literal on a 1.1 one
(`hud::font::set_front_end_colours`).

VERIFIED, where the cvars come from: no cgame registers them (1.1, 1.5
and UO only read them, `trap_Cvar_VariableStringBuffer` at cgame
0x3002a85d, 1.5 0x3002bced, UO 0x30036f66). The game module registers
both, `0.5 0.5 1` and `1 0.5 0.5`, with flag 0x800 (1.5b's table row 70
and 71, UO's 84 and 85, `tools/re/dump_cvars.py`), which puts them in
the 140/204 mirror, so a client in a game holds the server's. VERIFIED,
CoDMP.exe 1.1 has neither name; its renderer prints `^8`/`^9`.

VERIFIED, the writers agree: 1.5's console and message writer (0x409760,
UO 0x409dc0), its length count (UO 0x470ee0) and its obituary line writer
(0x409e60, UO 0x40a4a0) take `^0`..`^9`, as do 1.5's and UO's
`CG_AddToTeamChat` (cgame 0x3002de50, UO 0x30039390; cod11-chat.md
3.2).

vcod: `hud::font::set_team_colours`, called with
`Protocol::team_colour_codes` and the client cvars at each HUD build and
with no cvars at a gamestate, and `hud::font::is_colour_code`, which
every text path asks.

---

## 8. How the cgame draws a hudelem

Everything in this section is read out of `cgame_mp_x86.dll` (1.1). The
element fields are named by their offsets in `hudelem_t`, which
`docs/protocol-1.1.md` "Block 5" maps to the wire.

### Order

VERIFIED: `0x3001f920` holds the bound 31 (`0x1f`), the element stride 112
(`0x1c` dwords) and the snapshot playerstate's `+0x1338` and `+0x5A8`, reads
each element's `type` word, and calls the CRT `qsort` (`0x3004b350`) with the
comparator `0x3001f8e0`. VERIFIED: the comparator loads the float at `+0x6c`
(`sort`) of both elements and holds the return values -1, 1 and 0.
INFERRED, off `0x3001f920`'s two loops and their exits: the list is the
archived array then the current one, each stopping at its first `type` 0,
and it is what `qsort` sorts. INFERRED, off the comparator's compare: the
order is ascending by `sort`. INFERRED, off `0x3001f980`'s loop: the elements
are drawn in that order.
INFERRED: the CRT `qsort` makes no stability promise, so the order of two
elements with equal `sort` is unspecified; vcod sorts stably, archived first, which is one of the orders
retail can produce.

### Font slots

VERIFIED, the constants `0x3001f120` loads: 0.25 (`0x3006950c`), the
engine's font-height trap `0x35`, 0.33333334 (`0x30069548`), and the
immediates 16 and 16, and 16 and 8, stored to the setup record's `+0x28` and
`+0x2c`; the scale goes to `+0x24`. INFERRED, off its switch on `font`: slot
0 takes a text scale of `fontScale * 0.25`, the trap's height and a `+0x2c`
of 0; slots 1 and 2 take `fontScale * 0.33333334`, a height of 16, and a
`+0x2c` of 16 (slot 1) or 8 (slot 2). VERIFIED: `0x3001ec10`, the text
width, multiplies the result of trap `0x3a` by `+0x2c`, and has a second
path that calls trap `0x34` with the record's `+0x20` and `+0x24`. INFERRED,
off its compare of `+0x2c` with 0: a fixed slot takes the first path, trap
`0x3a` counts the string's characters, and its text is `+0x2c` wide per
character, so `+0x2c` is the
fixed cell's width, and the two fixed slots are 16x16 and 8x16 cells whose
width does not take `fontScale`. INFERRED: slots 0, 1 and 2 are the
`default`, `bigfixed` and `smallfixed` names the script's `font` field takes
(`cod11-gsc-object-model.md`, "HUD element fields"). VERIFIED from the pak listing: `pak5.pk3`
ships only `fonts/fontImage_{12,16,18,24,30,32}`, no fixed-width atlas.

VERIFIED: `0x3001f120` stores a font handle at the record's `+0x20`: 0 for
slot 0, 4 for slot 1 and 5 for slot 2, and `0x3001f490` passes `+0x20`,
`+0x24` and `+0x2c` to text trap `0x36` as its third, fourth and seventh
arguments. VERIFIED, `CoDMP.exe`: the cgame's trap `0x36` goes to the
refexport's `0x004df570`, which calls `0x004dfc10` with its seventh argument
ninth; `0x004dfc10` stores that at word 6 of render command 6, and the
command loop passes word 6 to the text renderer `0x004d7d60` as its eighth
argument. INFERRED, off `0x004d7d60`'s glyph loop: when that argument is
not 0 (`0x00568e64`, 0.0) every glyph advances by it and is shifted right
by `(cell - scale * advance) * 0.5` (`0x00568e70`, 0.5) on top of its
bearing, so a fixed slot is a proportional font laid out one glyph per
cell, centred in it. VERIFIED: the renderer takes its font from the
refimport hook `0x004118f0` with the handle and the scale; that hook
truncates the scale times 100 (`0x00568eb0`, the CRT `_ftol2` at
`0x00538be0`) and calls the UI module's `vmMain` with command `0x10`.

VERIFIED, `ui_mp_x86.dll` (1.1, image base `0x40000000`): `vmMain`
(`0x400076a0`) case `0x10` (`0x40007782`) multiplies the integer it is
passed by 0.01 (`0x40030020`) and calls `0x400079e0`. The asset parser
(`0x40007d30`..`0x40007f2b`) stores `font`, `smallFont`, `bigFont`,
`extraBigFont`, `boldFont` and `consoleFont` at `0x401c3dec`, `0x401c8e34`,
`0x401cde7c`, `0x401d2ec4`, `0x401d7f0c` and `0x401dcf54`, and pak0's
`ui_mp/main.menu` registers them at 16, 12, 24, 32, 30 and 18. VERIFIED,
the UI's cvar records: `ui_smallFont` 0.25 (value at `0x401eee28`),
`ui_bigFont` 0.4 (`0x401c1e88`), `ui_extraBigFont` 0.55 (`0x401c2428`).
INFERRED, off `0x400079e0`'s compares, with `s` the scale times the float
at `0x401c3dc0`: handle 2 is `bigFont`, 3 `smallFont`, 5 `consoleFont`;
handle 4 is `smallFont` for `s <= 0.25`, `font` below 0.4 and `boldFont`
from there; any other handle is `smallFont` for `s <= 0.25`,
`extraBigFont` from 0.55, `bigFont` from 0.4 and `font` between. INFERRED:
`0x401c3dc0` is the display context's `yscale`, the screen height over 480,
from its place ahead of the asset block as in Q3's `displayContextDef_t`.

So `smallfixed` draws `fontImage_18` in 8-unit cells and `bigfixed` picks
by the drawn size in 16-unit cells, and the default slot too picks its
atlas by the drawn size. vcod does all three, with the height of the
screen in place of `yscale`; it reads the default slot's height as its
tallest glyph at the element's scale. INFERRED: trap `0x3a` counts the
characters the renderer draws, colour codes excluded, which is how vcod
measures a fixed-slot string.

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

INFERRED, off `0x3001f7e0`: a non-empty label is drawn by `0x3001f490`
ahead of the type switch, for every type, and the draw position then moves
right by the label's width (`+0x14`) before the text, the shader
(`0x3001f6f0`) or the clock (`0x3001f520`) is drawn. INFERRED, off
`0x3001ef00` and the tail of `0x3001f120`: the width the element is aligned
by is the label's width plus the text's or the shader's. So a shader or clock
with a label is laid out label first, the pair aligned as one box; for the
text types the merge above leaves the label empty, so it rides in the text.

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
named after it with `"Needle"` (`0x30064b20`) appended, reads `duration`
(`+0x60`), and holds the constant `0x300695d0` (1.0922667, `65536 / 60000`).
INFERRED, off its branch on `duration` and the order of its arithmetic: the
element's timer value is scaled by 360 over `duration`, or by `0x300695d0`
when `duration` is 0, and truncated to a 16-bit angle (`ANGLE2SHORT`) before
it turns the needle.
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

VERIFIED, the dispatch tables: the ownerdraw switch subtracts 4 from the id at
`0x30026bdf` and indexes the byte table at `0x300270f0` and then the jump
table at `0x3002706c`; for ids 5, 20, 72, 81, 82, 84, 85, 86 and 89 those
tables land on `0x30026c1e`, `0x30026d2a`, `0x30026c36`, `0x30026edb`,
`0x30026f00`, `0x30026f3e`, `0x30026f5b`, `0x30026f78` and `0x30026f95`,
whose calls are `0x30025ab0`, `0x30023f50`, `0x300251f0`, `0x30023c30`,
`0x30023d50`, `0x30024800`, `0x30025120`, `0x30024d20` and `0x300248c0`;
ids 83 and 88 land on `0x30026f26` and `0x30026fd8`, which call
`0x30023e90` and `0x30013540`.
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
`+0x344`, `adsCrosshairInFrac` `+0x348`, `adsCrosshairOutFrac` `+0x34c`,
and the scope's keys listed under "Scope overlay".

### Which views draw it

VERIFIED, `0x30018810`, the 2D pass: it returns early on `cg_draw2D` 0
(`0x301db42c`), and otherwise branches on the snapshot playerstate's
`pm_type` (`cg.snap + 0x10`): 5 calls `0x30018530` and returns; 4 calls
`0x30018070`, `0x30016f70` and `0x300150b0`, then the hudelem pass
`0x3001f980` when `cg_drawStatus` (`0x301d9ecc`) is set; any other value calls
`0x30016760` (the crosshair, which calls the turret reticle `0x30016610`),
`0x30016f70` and `0x30037790` when it is below 6, then `0x300150b0`, and with
`cg_drawStatus` set `0x3004a6f0`, `0x30017470` and `0x3001f980`. INFERRED:
`0x3004a6f0` paints the cgame's visible menus, which are `hud.menu`'s, so a
free-flying spectator (4) and the intermission (5) draw no menu HUD, a dead
player (6, 7) draws it without a crosshair, and a follower, whose
playerstate is its target's copy with the target's `pm_type`
(`cod11-spectator-follow.md`), draws the target's health, ammo, stance and
compass. vcod draws it on those terms, off the snapshot's playerstate
without prediction while following, and rebases the hit-direction
feedback when the playerstate's `clientNum` changes.

VERIFIED, the cgame's cvar table rows (`{vmCvar_t *, name, default,
flags}`): `cg_drawStatus` at `0x30074a44` binds `0x301d9ec0` and
`cg_drawCrosshair` at `0x30074aa4` binds `0x301d9020`, both default `"1"`
with flags 1 (archive). INFERRED, from Q3's `vmCvar_t` layout: each
`integer` sits at `+0xc`, `0x301d9ecc` and `0x301d902c`. VERIFIED, the
cgame's loads of `0x301d9ecc`: `0x3001887c` and `0x300188a0` in the 2D pass,
and `0x30026bb0`, the first instruction of `CG_OwnerDraw`. VERIFIED, the
loads of `0x301d902c`: `0x30016613` (the turret reticle `0x30016610`),
`0x30016827` (the crosshair `0x30016760`) and `0x30016f70`.

INFERRED, off the branches after those loads: `CG_OwnerDraw` returns at
once on `cg_drawStatus` 0, so with the 2D pass's two tests `cg_drawStatus 0`
hides every owner draw (health, ammo, weapon name, stance, compass, cursor
hint), the `hud.menu` pass and the hudelems, and leaves the crosshair, the
scope overlay, the hit-direction icons, the chat and the obituaries.
INFERRED, same: `0x30016760` returns on `cg_drawCrosshair` 0 after the scope
overlay's call, `0x30016610` draws only when it is not 0, and `0x30016f70`
returns only when it is negative. INFERRED, off `0x30016f70`'s test of
`cg_drawCrosshairNames` (`0x301e0bec`) and its name lookup: that function
draws the name of the player under the crosshair, which vcod does not draw.

vcod reads both cvars as integers each frame (`hud::DrawToggles`):
`cg_drawCrosshair 0` drops the weapon crosshair and the mounted reticle,
`cg_drawStatus 0` the native HUD, the cursor hint and the hudelems.

The Multiplayer Options page (`ui_mp/options_multi.menu`) also offers Show
Compass (`cg_drawCompass`, Off/On) and Team Overlay (`cg_drawteamoverlay`,
Off/Short/Long). VERIFIED, CoDMP.exe's client cvar block: it registers
`cg_drawCompass` default `"1"` (`0x5685e4`) and `cg_drawTeamOverlay`
default `"2"` (`0x5685c4`), both flags 1, and discards the returned
pointers. VERIFIED: the string `cg_drawCompass` is in none of
`cgame_mp_x86.dll`, `ui_mp_x86.dll` or any stock menu but that one, and
the exe's decompilation names it only at the registration. VERIFIED: the
cgame's row for `cg_drawTeamOverlay` (`0x30074ef0`, binding `0x301ad060`,
default `"2"`, flags 1) is the only reference to that `vmCvar_t`; no
instruction loads `0x301ad060`..`0x301ad06f`. VERIFIED: 1.5's CoDMP.exe
and UO's CoDUOMP.exe register both the same way and discard the pointers
(their decompilations), neither string is in 1.5's or UO's `ui_mp_x86.dll`
or in UO's cgame but for `cg_drawTeamOverlay`'s row, and that row's
`vmCvar_t` (1.5 cgame `0x301b0c00`, row `0x30076eb0`; UO cgame
`0x304218a0`, row `0x30085750`) is referenced nowhere else in the file (a
byte search for each of its four words). INFERRED: both options are inert
in all three, so the compass always draws and there is no team overlay.
vcod registers both at retail's defaults so the menu's choice is kept, and
reads neither.

### Crosshair

INFERRED, off `0x30016760` and `0x3000fa50`: the hip spread in degrees is
`min + (max - min) * aimSpreadScale / 255`, `min` being the prone minimum when
`pm_flags & 1`, the ducked one when `& 2` and the standing one otherwise, or
a blend of two of them while the eye's stance leg runs (`0x3000fa50` is the
cgame's `BG_GetMinSpreadForWeapon`, `cod11-combat.md` 2.1), and
it is multiplied by a sight shrink factor. Each arm then sits
`spread * 640 / fov_x` virtual pixels off the centre horizontally and
`spread * 480 / fov_y` vertically, floored at `reticleMinOfs`. VERIFIED: the
two numerators are `0x300694fc` (640.0) and `0x3006973c` (480.0), and the fov
is an `fpatan` times the double at `0x300693c8`, which reads 114.59.
INFERRED: that is `360 / pi`, so the fov is in degrees.

INFERRED, same function: with the sight fraction `f` above 0, `t = f - (1 -
X)` for `X` = `adsCrosshairInFrac` or `adsCrosshairOutFrac`, picked by the
flag at `0x30209458`, and when `t > 0` the shrink is `1 - 0.5 * t / X`;
the arms' travel and both images' sizes take it. At `f` 1 nothing is drawn.
INFERRED: `adsAimPitch` drops the reticle by `480 / fov_y * adsAimPitch * t`
in the same arm; stock MP files do not set it and vcod leaves it out.

VERIFIED: `0x30036bd0` is the only writer of `0x30209458`, storing 1 at
`0x30036c63` and 0 at `0x30036c75`; it reads the held weapon's `+0x2cc`,
which the weapon field table names `aimDownSight` (`0x30075c4c`), the float
`0x30207214`, and its own copy of that float at `0x30209454`. VERIFIED: the
snapshot interpolation writes `0x30207214` from the float at `+0xc4` of the
two snapshots it lerps between. INFERRED: that is `fWeaponPosFrac` (the
playerstate's `+0xb8`) behind the snapshot's 12-byte header. INFERRED, off
`0x30036bd0`'s branches: for a weapon with `aimDownSight`, when the fraction
has left 0 or 1 since the last call, the flag becomes 1 if it moved up and 0
if it moved down, and the copy is updated; otherwise the flag holds. So the
in-fraction applies from the moment the sight starts rising until it next
starts falling, prone and mid-reload included. INFERRED, off `0x30016760`'s
test of the flag: 1 picks `adsCrosshairInFrac`.

VERIFIED: `0x30016760` tests the dword at `0x302071dc` against `0xc000` at
`0x300167b1`, compares
`0x302074d4` with `0x3ff` at `0x300167c0`, and has a tail jump to
`0x30016610` at `0x300167d6`. INFERRED: `0x302071dc` is `eFlags`, `+0x80`
into the playerstate copy whose `+0xb8` is `0x30207214`, and `0xc000` its
mounted-gun bits. INFERRED, off those branches: on a mounted gun
the weapon reticle is not drawn; `0x30016610` draws the turret's reticle, or
nothing when `0x302074d4` is `0x3ff`. What `0x30016610` draws is in
`cod11-turrets.md` section 14.4; vcod draws that in place of the weapon
reticle.

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
returns 0; its other branch draws the scope overlay and returns `1 - frac`
(next section), so both alphas fade out over the zoom tail, the centre to
nothing and the arms to `cg_crosshairAlphaMin`. vcod reads `aimSpreadScale`
off the prediction or the newest snapshot, not lerped.

### Scope overlay

VERIFIED, the cgame's weapon field table (`{name, offset, type}` records
`0x30075dc8`..`0x30075e10`): `adsZoomFov` `+0x218`, `adsZoomInFrac`
`+0x21c`, `adsZoomOutFrac` `+0x220`, `adsOverlayShader` `+0x224` (type 0),
`adsOverlayReticle` `+0x228` (type 10), `adsOverlayWidth` `+0x22c`,
`adsOverlayHeight` `+0x230`. VERIFIED: the name table at `0x300754f0`
reads `none`, `crosshair`, `FG42`, `Springfield`, `Gewehr43`. INFERRED:
type 10 stores the index of the name, 0 to 4.

VERIFIED, pak0's `weapons/mp/*`: five files set the keys, all with
`adsOverlayShader ui/assets/reticle_circle_quarter` at 220 by 220:
`springfield_mp` with reticle `Springfield`, and `kar98k_sniper_mp`,
`mosin_nagant_sniper_mp`, `fg42_mp` and `fg42_semi_mp` with `FG42`.
VERIFIED: pak4's `scripts/ui_hud.shader` maps that material to `map clamp
ui/assets/reticle_q.tga` with `blendFunc blend`. VERIFIED: pak0's 64x64
`reticle_q.tga` is opaque black except for a quarter disc about its
bottom-right corner, radius about 32 texels, at alpha 25 of 255.

INFERRED, off `0x30015f20`: the overlay is up when the held weapon names a
shader or a reticle, `fWeaponPosFrac` (`0x30207214`) is not 0, and
`frac = (f - (1 - X)) / X` is above 0.01, `X` being `adsZoomInFrac` with
the direction flag at `0x30209458` set and `adsZoomOutFrac` without it; the
division is skipped while `f - (1 - X)` is not above 0. VERIFIED:
`0x300693f4` reads 0.01. INFERRED, off `0x30015fe0`: the colour it sets
before the image is `(1, 1, 1, 1)`, so the overlay does not fade in; it
snaps on, at `f` 0.584 on the rise for `kar98k_sniper_mp`'s 0.42.

INFERRED, off `0x30016760`: the crosshair draw calls `0x30015fe0` ahead of
its test of `cg_drawCrosshair` (`0x301d902c`), so only its earlier returns
(`0x30207158` set, a mounted gun, no weapon) skip the overlay.

INFERRED, off `0x30015fe0` and `0x30015d70`: the overlay's centre is the
refdef's middle plus an offset. With `v` the forward of the pitch at
`0x3020cb80` and the yaw at `0x3020cb84`, and `d` its dot with the refdef
axis at `0x302095a0`, the offset is `-320 * (v . axis[1]) / (tan(fov_x / 2)
* d)` across and `-240 * (v . axis[2]) / (tan(fov_y / 2) * d)` down, times
`screenXScale` and `screenYScale` (`0x301d1fc4`, `0x301d1fc8`), and 0 unless
`d` and both refdef fovs (`0x3020958c`, `0x30209590`) are above 0.
VERIFIED: `0x300694f8` reads -320.0, `0x300694f4` -240.0 and `0x3006946c`
0.0087266 (`pi / 360`). INFERRED, off `0x300371f0`: for an `aimDownSight`
weapon with `fWeaponPosFrac` not 0 those two angles come from
`0x3003c810`, otherwise from the refdef's view angles (`0x302095cc`,
`0x302095d0`), which leaves the overlay centred. INFERRED: `0x3003c810` is
the gun's angles composed onto the view, the cgame's copy of the server's
aim block (`cod11-combat.md` 15), so the overlay sits where the shot goes
and drifts with the idle sway. vcod runs `pmove::aim::gun_angles` on the
replay's playerstate, or on a followed player's snapshot playerstate at the
render clock, and projects its forward through the drawn fovs.

VERIFIED, `0x300371f0` at `0x300373a3`..`0x30037480`: the block
`0x30012bf0` (the cgame's `BG_CalculateWeaponAngles`) reads is built on the
stack as `{ps, [0x3020c9f0], [0x30207144] * 0.001, ..., [0x30207148] -
ps[+0x20cc], [0x3020c9a8] - ps[+0x20cc] or 0 when [0x3020c9a8] is 0,
[0x3020c9e4], [0x3020c9e8], ...}`; `0x300127e0` reads words 7 to 10 of it,
and its constants are `0x30069524` 100.0, `0x30069528` 400.0, `0x3006930c`
0.5 and `0x300693d0` 0.75. INFERRED, off its branches: `0x300127e0` is the
gun's damage kick, a no-op on a zero word 8, which plays out `word 7 - word
8` ms through the same envelope and factors as the server's `0x39ce8`
(`cod11-combat.md` 15). VERIFIED: the
writes to `0x3020c9a8`, `0x3020c9e4` and `0x3020c9e8` other than the zeroing
in `0x30028a70` (`0x30028b2e`..`0x30028b39`) are all in `0x300287f0`.
INFERRED: words 7
and 8 are `cg.time` and the hit's time on one base, so the offset cancels,
and `0x3020c9e4`/`0x3020c9e8` are the kick along and across the view.

VERIFIED, `0x300287f0`, called with `(damageYaw, damagePitch,
damageCount)` off a playerstate's `+0xe8`, `+0xec`, `+0xf0` at
`0x30028d2c` and `0x3002fc5e`, each behind a compare of `+0xe4`
(`damageEvent`) across two playerstates and a test of `+0xf0` against 0.
INFERRED: those are the new and the previous snapshot's, so a hit is a
changed `damageEvent` with a non-zero count. VERIFIED, `0x300287f0`: the kick is
`damageCount * 0x300693d4` (0.2), replaced by `0x30069414` (5.0) below it
and by 90.0 (`0x3006947c`) above that. INFERRED, off the branch at
`0x30028854`: yaw and pitch both 255 store 0 at `0x3020c9e8` and `-kick`
at `0x3020c9e4`. VERIFIED, the other arm: each byte is divided by
`0x30069384` (255.0) and multiplied by `0x30069374` (360.0), the pair goes
through two `fsincos` into `(cp*cy, cp*sy, -sp)`, `0x3020c9e8` gets `-kick`
times its dot with the refdef's `viewaxis[1]` (`0x302095ac`) and
`0x3020c9e4` `kick` times its dot with `viewaxis[0]` (`0x302095a0`).
VERIFIED: `0x30028a58`
stores `cg.snap`'s `serverTime` (`[0x301e2160] + 8`) into `0x3020c9a8`.
INFERRED: the client rebuilds the kick from the feedback bytes; it scales
by `damageCount` where the server's own kick scales by `aimSpreadScale`
(`cod11-combat.md` 6), and reads a byte as `/ 255` where the server wrote
it as `* 256 / 360`. vcod does the same in `Hud::feed_damage` ("Damage
direction", "When the feed runs"), stamping the kick with the clock the
sway runs on at the frame the new `damageEvent` is first seen, where
retail stamps the snapshot's `serverTime`.

INFERRED, off `0x30015fe0`'s calls to `0x300310f0` (a plain wrapper of the
stretch-pic trap `0x49`: x, y, w, h, s1, t1, s2, t2, material): with `w`
and `h` the overlay size times the screen scales and `(cx, cy)` the centre,
the image is drawn four times, `w` by `h`, at `(cx - w, cy - h)` with
corners `(0,0)-(1,1)`, at `(cx, cy - h)` mirrored in `s`, at `(cx - w, cy)`
mirrored in `t`, and at `(cx, cy)` mirrored in both. Black bands follow, in
the same material: left `(0, 0, cx - w, H)` when `cx - w > 0` and right
`(cx + w, 0, W - cx - w, H)` when `cx + w < W`, both at `s` 0 and `t` 0 to
1; top `(cx - w, 0, 2w, cy - h)` and bottom `(cx - w, cy + h, 2w, H - cy -
h)` under the same tests, at `t` 0 and `s` 0 to 1. `W` and `H` are the
refdef's size (`0x30209584`, `0x30209588`). INFERRED, off the weapon setup
that falls through to `0x300358f5`: the material at `0x301a6aa4 + 0x198 * weapon`
is `adsOverlayShader` registered with trap `0x58`.

INFERRED, off UO's `uo_cgame_mp_x86.dll` `0x300195e0` (its copy of
`0x30015fe0`, gated by `0x30019520`) and its calls to the stretch-pic
wrapper `0x3003e0f0`: UO draws the image once, `w` by `h` at `(cx - w/2,
cy - h/2)` with corners `(0,0)-(1,1)`, then the same four bands around
that box (left and right full height at `s` 0, top and bottom its width at
`t` 0). VERIFIED: `0x3007bce8` reads 0.5. VERIFIED, `uo/pakuo04.pk3`'s
`weapons/mp/springfield_mp`: `adsOverlayShader
gfx/reticle/scope@scope_overlay_us.dds`, `adsOverlayReticle none`, 520 by
400; `pakuo00.pk3`'s image is a 1024 by 1024 DDS of the whole scope.
VERIFIED: 1.5's `springfield_mp` keeps `ui/assets/reticle_circle_quarter`
at 220 by 220. INFERRED: 1.5's cgame keeps 1.1's four quarters. vcod
picks the layout by protocol (`hud::scope::Layout`); drawn as quarters, the
UO image showed four whole scopes.

INFERRED, same function, the reticle, drawn over the image whether or not
the shader is set: `crosshair` draws the weapon's `reticleCenter` material,
`reticleCenterSize` times the screen scales, centred. `FG42` and `Gewehr43`
draw in `(0, 0, 0, 1)` a `hudSoftLine` post `(cx - 1, cy, 3, 0.9h)` and two
`hudSoftLineH` bars `(cx - 0.9w, cy - 1, 0.75w, 3)` and `(cx + 0.15w, cy -
1, 0.75w, 3)`. `Springfield` draws a cross, `(cx - 1, cy - 0.9h, 3, 1.8h)`
and `(cx - 0.9w, cy - 1, 1.8w, 3)`. The 1 and the 3 are window pixels.
VERIFIED: `0x300695ec` reads 0.9, `0x300693d0` 0.75, `0x300695e8` 1.8 and
`0x300695c0` 0.15, and `0x301d5a88` and `0x301d5a8c` are stored at
`0x3002303d` and `0x30023047` from the registrations of `hudSoftLine` and
`hudSoftLineH`. VERIFIED: pak4's `scripts/hud.shader` gives both `rgbGen
vertex` and `alphaGen vertex`.

INFERRED, off `0x300371f0` and `0x30036cf0`: the gun's add flag is cleared
when `cg_drawGun` (`0x301dbaec`) is 0, or is not 2 and `0x30015f20` returns
1, and with it clear `0x30036cf0` adds neither the gun's refEntity (trap
`0x3d`) nor its `tag_flash` effect. So under the overlay the viewmodel and
the first-person muzzle flash vanish. VERIFIED: the cvar table names
`cg_drawGun` for the `vmCvar_t` at `0x301dbae0`, whose integer is `+0xc`.

INFERRED, off `0x300172f0`: while `0x30015f20` returns 1, the hit-direction
icons are skipped unless `cg_hudDamageIconInScope` (`0x301e0acc`, default 0)
is set, and are then centred on the overlay's offset. INFERRED, off
`0x30018810`'s call order: the hit icons (`0x300172f0`) draw before the
pm_type branch, then the crosshair with its overlay (`0x30016760`),
`0x30016f70`, `0x30037790` and `0x300150b0`, then with `cg_drawStatus` the
`hud.menu` items (`0x3004a6f0`), `0x30017470` and the hudelems
(`0x3001f980`). So the menu HUD and the script's hudelems paint over the
overlay's black bands. vcod draws the overlay first, under them, as retail.

INFERRED, the same order with section "Which views draw it": a follower's
playerstate carries its target's `pm_type` and `fWeaponPosFrac`, so a
spectator following a scoped player gets the overlay, and the zoom with
it. vcod draws a followed player's view weapon, zoom and overlay off the
snapshot's playerstate.

vcod scales the overlay by the window height on both axes, as the rest of
its HUD, where retail scales x by `width / 640`: on a window wider than 4:3
the circle stays round and the side bands widen.

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

INFERRED, off the guard both `0x30023c30` and `0x30023d50` open with: the
name and its backdrop draw only while `0x30019a30` returns a colour.
VERIFIED, `0x30019a30` (`ecx` the length, `edx` the start): it returns null
when the start is 0 or `cg.time - start` is not below the length; otherwise
white with alpha 1, or `left * 0.01` (`0x300693f4`) when `left`, the length
less the time since the start, is below 100, times the global at
`0x301e14e8`, `cg_hudAlpha` (record `0x30074ae0`, default `"1.0"`).
VERIFIED: both callers pass 1800 (`0x708`) and the stamp at `0x3020c920`.

VERIFIED, the seven stores to `0x3020c920`, each of `cg.time`
(`0x30207148`): `0x30037f28` in the weapon select `0x30037f20`, which the
`a` server command calls (`0x3002e139`); `0x300380a2` and `0x3003811c`, the
two weapon-cycle commands, and `0x30038342`, the weapon slot command, each
only when `cg.time` less the stamp is at least `cg_weaponCycleDelay`
(record `0x30074c10`, integer at `0x301df9ec`, default `"0"`);
`0x30028aa3` in `0x30028a70`; `0x3001dabc` in `0x3001da80`, an item pickup
event for a weapon (row type 1 at `0x30076310`) while the selection
(`0x301cbb0c`) is 0; and `0x300387fa`, the cycle's fallback, when it leaves
the selection unheld and non-zero. VERIFIED: `0x30028a70` is reached from
the two first-snapshot setups (`jmp` at `0x3002fa8f`, `call` at
`0x3003037f`) and from the snapshot transition's `call` at `0x300300cd`,
taken when `0x30207154` is set or the new snapshot's `stats[5]` (snapshot
`+0x114`) or `clientNum` (`+0xb8`) differs from the old one's
(`0x30030009`..`0x300300b8`). INFERRED: that is Q3's `CG_Respawn`, run on a
new spawn count (`docs/protocol-1.1.md`, `stats[5]`) or a new followed
client. VERIFIED: the name and the backdrop show the selection's weapon
(`0x301cbb0c`) when it is held, else the playerstate's.

VERIFIED, `0x3001da80` (Q3's `CG_ItemPickup`, item index in `eax`):
it stores the item at `0x3020c914` and `cg.time` at `0x3020c918` and
`0x3020c91c`, then, for a weapon row while the selection is 0, stamps the
name and sets `cg_weaponSelect` to the row's weapon (`+0x4`, through
`"%i"`). VERIFIED: its one caller (`0x3001e0ff`) is the event switch's arm
for 146, 147 and 148 (`EV_ITEM_PICKUP`, `_QUIET`, `EV_AMMO_PICKUP`), taken
when the snapshot playerstate's `pm_flags` has `0x50000` and the event's
entity is its `clientNum`. VERIFIED: nothing else references `0x3020c914`
or `0x3020c91c`, and the only other store to `0x3020c918` is a clear at
`0x3002ca9d`. INFERRED: 1.1 keeps Q3's pickup bookkeeping but draws no
pickup notice from it; the pickup's only visible effects are the weapon
name and the autoswitch out of empty hands.

vcod stamps on a change of `(clientNum, stats[5])`, the first playerstate
included, on every weapon-select bind (`weaponslot`, `weapnext`,
`weapprev`), on the `a` command and on a weapon pickup with the selection
at 0, which also selects the weapon (`PlayInput::picked_up`). It fades the
name and its backdrop with that rule (`WeaponNameFade`). The selection is
`PlayInput::selected`, retail's `cg_weaponSelect` with every write
`cod11-combat.md` 9.8 lists; the name, the mode icon and the ammo count read
it as that section's readers do (`PlayerView::name_weapon`,
`PlayerView::ammo_weapon`).

### Weapon mode icon

VERIFIED: the weapon setup registers the def's `modeIcon` (`+0x190`) at
`0x30035cab` and stores it at `+0x11c` of the weapon's `0x198`-byte record
from `0x301a6940` (`0x30035cb3`); `0x30023e90` reads that slot for the
selection's weapon when the selection is carried and below the weapon count,
else for the weapon at `0x30209484` (`cod11-combat.md` 9.8), and draws it
over the item's rect in its forecolor when it is not 0. VERIFIED: it skips
the fade test `0x30019a30` when the playerstate's weapon (`0x3020720c`) is
not 0 (`0x30023e90`..`0x30023eab`). INFERRED: the icon draws whenever a
weapon with a `modeIcon` is held, and while the name fades in with empty
hands.
VERIFIED, pak0's `weapons/mp/*`: the select-fire pairs name one,
`hud@weaponmode_full.tga` on `bar_mp`, `fg42_mp`, `mp44_mp`, `ppsh_mp` and
`thompson_mp`, `hud@weaponmode_semi.tga` on their `_semi` and `bar_slow_mp`
variants; every other weapon leaves it blank.

### The weapon-slot strip

VERIFIED: `0x30037790` has one caller, `0x30018896` in the 2D pass
`0x30018810`, after the crosshair (`0x30016760`) and `0x30016f70` and before
`0x300150b0`, taken when the snapshot's `pm_type` is below 4; the pass
itself returns early on `cg_draw2D` 0 and three unidentified flags
(`0x301e214c`, `0x301e2150`, `0x3020cbdc`). `cg_drawStatus` does not gate
it. VERIFIED, inside: `dt` is `cg.time` less `0x3020c944`, which takes
`cg.time` on every call; it returns when the predicted `pm_type`
(`0x30207160`) is 6 or more, and when `0x30019a30(1800, 0x3020c920)`
returns null, so it shares the weapon name's stamp and fade (above).
VERIFIED: `0x3002ca60` zeroes the slot scales and `0x3020c944`.

VERIFIED, the layout (`.rdata` 32.0 `0x30069408`, 2.0 `0x30069308`, 0.01
`0x300693f4`, 64.0 `0x300695a0`), in 640x480 units: from x 632, slots 5
down to 1 of the predicted playerstate's `weaponslots` (`0x30207470`):
- a slot that is empty or not in `ps.weapons` (`0x30207468`) takes 32 and
  its scale goes to 0;
- a held weapon whose `hudIcon` registered nothing (the record `+0x118`
  from `0x301a6940`, set in `0x30034cf0` only for a non-empty `hudIcon`)
  draws `hudNoWeaponIcon` (`0x301d5acc`) 32 square at y 8 in grey and takes
  32, scale to 0, no key;
- the slot `0x3000f5c0(cg_weaponSelect, 1)` names grows its scale by
  `dt * 0.01` to 1 and draws white, `(scale + 1) * 32` high and twice that
  wide for a `wideListIcon` (def `+0x2d8`); every other slot shrinks at the
  same rate and draws grey (0.5) at its own scale;
- each drawn slot puts its key under its left edge, then 2 units of gap.
The icon is the slot's own weapon's. VERIFIED, `0x30037620`: the key text is
`weaponslot <name>`'s binding (`0x30046590` rescans the bind table on every
call; names from `0x30075504`), one key's localized name, two joined by
`KEY_OR`, or `KEY_UNBOUND`; it is drawn with trap `0x36` in font 5
(`consoleFont`) at 0.25 with 6.0 as the fixed cell (section 8), black at
`(x + 1, 14.6)` then white at `(x, 13.6)`, alpha the fade's. INFERRED: the
y is the text's baseline, as vcod draws it.

VERIFIED, the selected slot of a weapon with more than one mode on its
`altWeapon` chain: the other modes draw grey under the box's centre, the
first at `y = h + 10` (10.0 `0x300693e4`) and each further one 34 below
(`0x300694e8`), 32 high and 32 or 64 wide. A switch from a mode to the mode
it names sets `0x30096c20` to +1 (else -1), which runs back to 0 at `dt /
150` (`0x300696d4`); while it is non-zero the mode in hand slides from the
row below up to y 8 growing to full size, and the mode left slides down,
shrinking. INFERRED: a backward slide (-1), which no stock chain produces,
reads the colour's alpha off the stack as its first row; vcod copies that.

**vcod drew no strip.** Added as `hud::weapstrip`, pinned by its three
tests. vcod lays it out in its centred 640x480 space, where retail
stretches 640x480 over the whole window (`0x301d1fc4`, `0x301d1fc8`); every
native HUD piece in vcod does the same. Pending a look by eye.

United Offensive's strip, `uo_cgame_mp_x86.dll` `0x30046bb0`:

- VERIFIED: it walks slots 7 down to 1 (`iStack_68` starts at 7, the scale
  pointer at `0x3048ae6c` and stops past `0x3048ae54`), so the binoculars
  (slot 7) sit rightmost from x 632, then the satchel (6), then 5..1. The
  slot scales are `0x3048ae50 + slot * 4`, the clock `0x3048ae70`.
- VERIFIED: the layout constants read the same values as 1.1's: 632.0,
  32.0 (`0x3007bdd0`), 0.01 (`0x3007bdb4`), the 2.0 gap (`0x3007bce4`),
  the mode rows' 10.0 and 34.0 (`0x3007bda4`, `0x3007bf20`), 64.0
  (`0x3007c000`), the slide's 1/150 (`0x3007c188`). The key text
  (`0x30046a40`) prints `weaponslot %s` off the eight-name table
  `0x30084310` at `(x + 1, 8 - 4 + 9.6 + 1)` black, then `(x, 13.6)`
  white (4.0 `0x3007be40`, 9.6 `0x3007c080`), as 1.1's 14.6 and 13.6.
- VERIFIED: one difference inside: a negative `dt` reads 0 (`jns`,
  `0x30046bce`); 1.1 keeps the sign.
- VERIFIED: the 2D pass calls it at `0x3001c06f` only when the snapshot's
  `pm_type` is below 6 and neither 4 nor 5, and `cg_drawStatus`
  (`vmCvar` `0x30450a40`, its integer `0x30450a4c` tested at `0x3001c066`;
  the registration row at file offset `0x85290` names it) is non-zero.
  1.1 does not read `cg_drawStatus` for the strip.

**vcod drew UO's strip over slots 5..1 only.** `StripView::uo` now walks
7..1, the slot lookup takes 1..7, a backward clock reads 0, and on UO the
strip hides with `cg_drawStatus` 0. `weapstrip::tests::uo_draws_the_satchel_and_binocular_slots`
pins the layout. Pending a look by eye.

### Weapon slots

The `weaponslot` command, per protocol. Each line names its module (all
three cgames load at 0x30000000).

- 1.1 `cgame_mp_x86.dll`. VERIFIED: the name table at `.rdata`
  `0x30075504` holds six pointers: `none`, `primary`, `primaryb`, `pistol`,
  `grenade`, `smokegrenade` (the pointers resolve to `0x30066db4`,
  `0x30066d54`, `0x30066d48`, `0x30066d40`, `0x30066d94`, `0x30066d30`).
  VERIFIED: the lookup `0x3000f220` compares the argument against the six
  with `_stricmp` (`0x300581b0`) and returns the index, 0 for none. Read
  off the handler `0x300382f0` (INFERRED, its instruction order): the
  gates (a snapshot, predicted `pm_flags` without 0x4000, snapshot
  `pm_flags` 0x40000, `cg.time` less the name stamp `0x3020c920` at least
  `cg_weaponCycleDelay`), then the stamp takes `cg.time`
  (`0x30038342`). Only after that does it read argument 1 and look it up.
  A 0 from the lookup falls back to `atol` (`0x3004afde`). A result
  outside 1..5 (`jle`, `cmp 6; jge`) returns. So does an empty slot byte
  of `ps.weaponslots` (`0x30207470 + n`) or a weapon outside `ps.weapons`
  (`0x30037f00`). Otherwise the handler selects through `0x30037f20`. It
  prints nothing on any refusal, and a refused slot still restamps the
  weapon name, which the strip above shares.
- 1.5 `cgame_mp_x86.dll` (protocol 6). VERIFIED: the same six names at
  `0x30077504`, the lookup `0x30010000`, the handler `0x30039c00` with the
  same order and the 1..5 bound (`0x30039c86`). Its own-view bit is 1.5's
  `pm_flags` 0x20000 (`0x30039c1c`), which vcod's decoder maps onto 1.1's
  0x40000.
- United Offensive `uo_cgame_mp_x86.dll` (protocol 22). VERIFIED: eight
  names at `0x30084310`, 1.1's six then `satchel` (`0x300765fc`) and
  `binocular` (`0x300765f0`). The lookup `0x300110b0` scans all eight. The
  handler `0x30047750` bounds the slot at 1..7 (`cmp 8; jge`,
  `0x300477e8`). It adds one gate before the stamp: with the predicted
  `eFlags` 0x100000 (a seat, `0x3004776c`) it goes on only when the
  snapshot's `vehType` (ps +0x618) is 1 and its `vehPos` (ps +0x614)
  is 3, `BG_AllowPlayerWeaponAtVehiclePos` inlined. VERIFIED: the slot
  lookup `0x30011460` takes slot types 3..7 at their own byte (1.1's
  `0x3000f5c0` stops at 5). The cycle wraps over seven slots,
  `(slot + dir + 6) % 7 + 1` (`0x300479d8`). Its first-slot walk
  (`0x30047820`) runs 1..7 forward or 7..1 back. The stock UO binds use
  `weaponslot satchel|binocular|smokegrenade` (`coduo-diff.md` 15).

vcod: the console's `weaponslot` (`console::shell::weapon_slot`) takes
UO's eight names, case-folded, falling back to `atol`. It always queues
the slot, as retail stamps before it looks. `PlayInput::weapon_slot`
refuses 0, a slot past the protocol's last (`Held::uo_slots`) and a UO
seat that does not fire the rider's weapon (`Held::seat_refuses_slot`,
read off the snapshot's `eFlags` where retail reads the predicted one).
The cycle and the slot lookup take UO's seven slots too. **vcod refused
`smokegrenade`, `satchel`, `binocular` and every number, and printed a usage
line retail does not have.** `play::loopback::every_slot_name_switches_the_held_weapon`
runs each name through the shell, the input and vcod-server. On UO the
satchel and binoculars move the server's `ps.weapon` to slots 6 and 7.
The strip above draws them (UO's strip).

The other binds, read off the disassembly (INFERRED, instruction order):

- UO's `weapnext` `0x300474c0` and `weapprev` `0x30047550`: with the
  scoreboard held (`0x3048a554`) they scroll it, as 1.1's (`0x3020c030`)
  do. Otherwise: a snapshot, predicted `pm_flags` without 0x4000, then
  predicted `eFlags` (`0x30483248`, ps `0x304831c4` + 0x84) 0x100000
  sends `nextvehslot` / `prevvehslot` (trap 0x18) and returns, before the
  own-view bit (snapshot `pm_flags` 0x80000), the cycle delay against the
  stamp `0x3048ae44` and the stamp itself; the cycle `0x30047960` runs
  after the stamp.
- UO's `weapalt` `0x30047400`: a snapshot, predicted `pm_flags` without
  0x4000, the own-view bit, then with predicted `eFlags` 0x106000 (a seat
  or a mounted gun) it goes on only when the predicted `vehType`
  (`0x304837dc`, ps + 0x618) is 1 and `vehPos` (`0x304837d8`, ps + 0x614)
  is 3 (`0x30047429`); then the cycle delay, then the `altWeapon` lookup
  (def `+0x36c`), the `CGAME_THIS_WEAPON_HAS_NO_ALTERNATE` line, the
  carried check and the select `0x30047390`, which stamps. 1.1's
  `0x30037f90` has the same order with predicted `eFlags` 0xc000
  (`0x302071dc`) alone.
- VERIFIED: UO's prediction (`0x30035800`) copies its base playerstate
  from `0x30459164`, the snapshot after `cg.snap` (`0x30459160`), 0x1141
  dwords at `+0xc`. INFERRED: that is `cg.nextSnap`, the newest snapshot
  the clock has reached, so the predicted `eFlags` the seat gates read are
  the newest snapshot's (pmove never writes the seat bit), and
  `cg.snap`'s own-view bit and `vehType` are one snapshot older.
- VERIFIED decompile: 0x30035800 tests `cg.snap`'s `pm_flags` (+0x18)
  against 0x40000, UO's follow bit, and calls 0x300354b0, which copies
  its playerstate from `0x30459164` + 0xc and lerps the origin and view
  between `cg.snap` and it. INFERRED: 0x300354b0 is the follow branch,
  and a follower's
  drawn playerstate fields (`eFlags`, the seat, `viewlocked_entNum`) are
  the newest snapshot's, as vcod reads them; only the choice between
  predicting and following is `cg.snap`'s, a snapshot late at the start
  or end of a follow. vcod chooses off the newest snapshot's `clientNum`:
  for that snapshot retail would run our cmds on the followed player's
  playerstate, which vcod does not copy.

vcod: `PlayInput::select_allowed` runs the view and seat gates before the
cycle delay and the name stamp; a select they pass stamps even when its
slot is empty or out of range, as retail's. **vcod stamped the weapon
name for every select, off its own view and in a refusing seat too.**
`weapalt` refuses in a UO seat that does not fire the rider's weapon
(**vcod let it switch modes there**). vcod reads the predicted
gates off the newest snapshot, retail's prediction base, and the own-view
bit and `weaponslot`'s `vehType` and `vehPos` off the older snapshot of
the drawn pair, `cg.snap` (`Held::cg_snap`; 1.1 reads `cg.snap`
`0x301e2160` + 0x18 at `0x3003830c`, UO `0x30459160` + 0x624, + 0x620
and + 0x18 at `0x30047778`..`0x30047792`, VERIFIED disassembly). **vcod
read those off the newest snapshot, a snapshot early at a spawn, a
follow or a seat change.** `play::replay::the_jeep_drivers_binds_wait_for_cg_snap`
replays `*-uo-jeep.bin`: the binds refuse on the frames before the first
spawned snapshot (14100) turns `cg.snap`'s bit on. The `nextvehslot` branch already
ran first and off the newest snapshot. `play::input::tests::select_gates_run_before_the_stamp`
and `uo_weapalt_refuses_in_a_seat` pin both.

### Stance

VERIFIED: `0x30023f50` reads the dword at `0x30207168` and keeps its low two
bits at `0x30074988`. VERIFIED: `0x30028a70` copies `0x834` dwords from
`cg.snap + 0xc` (`0x301e2160`), the snapshot's playerstate, to `0x3020715c`.
INFERRED: that dword is the playerstate's `+0xc`, `pm_flags`, in the copy
the cgame predicts from. VERIFIED: `hudStanceStand`, `hudStanceCrouch` and
`hudStanceProne` are registered to `0x301d5cf8`, `0x301d5cfc` and
`0x301d5d00` (`0x30020faa`, `0x30020fbb`, `0x30020fcf`). INFERRED, off the
branch ahead of the draw: the icon is the prone one on bit 1, the crouch
one on bit 2, the standing one otherwise.

VERIFIED: `0x30023f50` stores the time at `0x30074984` when those bits
differ from the kept ones or the time is earlier than the stored one, and
-1 instead whenever `cg_hudStanceHintPrints` (integer at `0x301dac4c`,
default 0) is 0 (`0x30023f50`..`0x30023fa2`); both statics start at -1.
VERIFIED, the constants: 0.001 (`0x300693c0`), 0.8 (`0x30069458`), and the
cvars `cg_hudStanceFlash_r`, `_g`, `_b` at 1.0, 1.0, 0.3 (values at
`0x301dbd28`, `0x30298008`, `0x301df0e8`). INFERRED, off the tail: for
1000 ms from the stored time `hudStanceFlash` (`0x301d5d04`) is drawn over
the icon in that colour, each lane clamped to 0..1, at alpha
`(stored + 1000 - now) * 0.001 * 0.8`. VERIFIED: pak0's
`configure_mp.cfg` and `safemode_mp.cfg` set `cg_hudStanceHintPrints` 1.
INFERRED: a stock install runs with it on, which vcod assumes. VERIFIED: the
icon's x adds `(cg_hudCompassSize - 1) * 112` (`0x30069738`), 0 at the
default.

VERIFIED, the key hints (`0x300240e8`..`0x30024589`): while `cg.time` is
below the stored time plus 3000 (`0xbb8`), the function fills three
4-by-6 tables of command names on the stack, one per stance, and the labels
`CGAME_STANCEHINT_JUMP`, `_STAND`, `_CROUCH`, `_PRONE` (`0x30063238`,
`0x30063220`, `0x30063208`, `0x300631f0`). Read out of the stores, row by
row (jump, stand, crouch, prone):

| Stance | Jump | Stand | Crouch | Prone |
|---|---|---|---|---|
| standing (`[esp+0x114]`) | `+gostand`, `+moveup` | | `gocrouch`, `togglecrouch`, `lowerstance`, `+movedown` | `goprone`, `+prone` |
| crouched (`[esp+0xb4]`, bit 2) | | `+gostand`, `raisestance`, `+moveup` | | `goprone`, `lowerstance`, `toggleprone`, `+prone` |
| prone (`[esp+0x54]`, bit 1) | | `+gostand`, `toggleprone` | `gocrouch`, `togglecrouch`, `raisestance`, `+movedown`, `+moveup` | |

VERIFIED: it refreshes the binding table (`0x30046590`, the 50 `{command,
..., key1, key2}` rows from `0x3006f150`, each key the first two key numbers
whose binding matches case-insensitively, `0x300464e0`), then for each row
keeps the first command `0x30046810` reports bound (key1 not -1) and counts
the rows kept. The first baseline is the rect's `y + h * 0.5 - 1.5`
(`0x3006930c`, `0x300693ac`), moved down by half a text height
(trap `0x35` at the item's font and scale) for one row and up by half a
height plus 1.5 for three; each kept row prints at the rect's right edge
(`x + w`, with the compass-size shift) the localized label formatted with
`0x30046940`'s key text, and the next baseline moves down a height plus
1.5. VERIFIED, `0x30046940`: the key text is `KEY_UNBOUND` with no key, the
first key's name, and with two `"%s %s"` of `KEY_OR` and the second's name
appended. VERIFIED: the colour is the item's forecolor with alpha 1 below
the stored time plus 2000 (`0x7d0`) and `(stored + 3000 - now) * 0.001`
after. VERIFIED, pak `localized_english_pak1.pk3`'s `cgame.str`: the four
labels read `Press [%s] to jump`, `to stand`, `to crouch`, `to go prone`.
INFERRED: with the stock binds (Space `+gostand`, C `gocrouch`, Ctrl
`goprone`) a standing player sees three lines, one crouched or prone two.

VERIFIED, the prone-blocked notice (`0x30023ff5`..`0x300240e5`): when the
predicted playerstate's `pm_flags` has `0x8000` and the static at
`0x3007498c` is below `cg.time`, it becomes `cg.time + 1500` (`0x5dc`);
while it is ahead, `CGAME_PRONE_BLOCKED` (`0x30063250`, "Prone Blocked") is
printed centred on x 320 (`0x300695e4`, less half the text width) at y 270
(`0x43870000`) with alpha `|sin((until - now) * 0.00066667 * 540 * pi /
180)|` (`0x30069734`, `0x30069730`, `0x300693a8`, `0x30069494`): 0.36
degrees a millisecond, three blinks in the 1.5 s.

vcod prints both off the same stamp as the flash (`stance_hints`,
`ProneBlocked`), the key text through the console's binds
(`Shell::key_text`), and reads `0x8000` off the predicted playerstate while
predicting, else off the snapshot's `pm_flags`. Its pmove raises the bit as
retail's does (`cod11-mantle.md`, "Prone Blocked"). All three draw before
the icon, as retail's do.
INFERRED: the notice, the hints and the flash all live in the stance owner
draw, and the compass spring in the compass one, so `cg_drawStatus` 0
hides them and freezes their stamps and the spring; the weapon name's
stamp is set outside the draw and keeps running. vcod does the same.

vcod reads the icon and the flash off `pm_flags`' two bits, the replay's
while predicting. Its first frame counts as a change, as the -1 start
makes it in retail.

### Compass

VERIFIED: `hud.menu` lists the compass items back (84), highlight (85),
face (84), needle (85), friendlies, objective pointers (86). INFERRED: a menu
draws its items in file order, so the highlight is under the face.

INFERRED, off `0x30024800` and `0x30019bd0`: the back and face turn by the view
yaw less `northyaw` (configstring 11), smoothed by a spring; the highlight and
the needle do not turn (`0x30025120`).

INFERRED: both 84 items call the spring each frame, and the second call
finds no time passed. VERIFIED, the spring, `0x30019bd0`, which
`0x30024800` calls ahead of its draw: the target is `SHORT2ANGLE(ANGLE2SHORT(
viewangles[YAW] (0x302095d0) - northyaw (0x3020d09c)) & 0xffff)`
(`0x30069380` 182.04445, `0x3006937c` 360/65536, through the truncating
`_ftol` at `0x3005a890`). With the last time at `0x300eef2c` ahead of
`cg.time`, or more than 500 ms (`0x300694b8`) behind, it stores the time,
sets the drawn yaw (`0x3020d0a0`) to the target and the speed
(`0x3020d0a4`) to 0. Otherwise it stores the time, takes `delta =
AngleSubtract(drawn, target)` (`0x3003c310`, into -180..180) and walks the
elapsed time in steps of at most 5 ms, `dt` the step times 0.001
(`0x300693c0`): when `|delta| < 0.25` and `|speed| < 1.0` (doubles
`0x30069430`, `0x30069328`) it snaps to the target and speed 0 and returns;
else `delta` becomes `vel * dt + delta` wrapped through `ANGLE2SHORT` into
0..360 and less 360 above 180 (`0x30069370`, `0x30069374`); the speed loses
`dt * 1000` (`0x30069478`) when `delta > 0` and gains it when `delta < 0`;
it loses `2 * speed * dt`; then, with the speed above 0, it loses `speed *
dt * 3.5` (`0x300694c4`) when `delta > 0` and `dt`, and becomes 0 if that
took it below 0; with the speed at or below 0 the mirror image, `delta < 0`
and `+ dt`; a speed it did not zero is clamped to ±30000 (`0x300694c0`,
`0x300694bc`). After the last step the drawn yaw is the wrapped `delta +
target`. VERIFIED: `0x30024800` hands that yaw to the turned pic call
`0x30019330`.

VERIFIED: the pointer yaw at `0x3020d0a8`, which the friendlies and the
objectives turn by, comes from a second spring at `0x30019f00` that runs
only when `cg_hudCompassSpringyPointers` (record `0x30074b30`, integer at
`0x301da46c`) is set, and is the view yaw otherwise; the cvar table's
default is `"0"` and pak0's `configure_mp.cfg` and `safemode_mp.cfg` set
it 0. INFERRED: at stock settings the face swings and settles while the
pointers track the view. vcod runs the face spring (`CompassSpring`) on
the HUD clock and turns the pointers by the view yaw.

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

### Compass friendlies

VERIFIED, `0x30013540`: it returns unless the client info of
`cg.nextSnap`'s playerstate `clientNum` (`0x301e2164`, `+0xb8`) is valid
and its team (`0x3018bc38`, stride `0x448`) is neither 0 nor 3. It then
walks the snapshot's entities (count `+0x20dc`, numbers from `+0x20e4`,
stride `0xf0`) and, for each whose `0x228`-byte `cg_entities` record
reads 1 at `0x3020dc74` (`0x300135a4`) and has bit 1 clear at `0x3020dc78`
(`0x300135b3`), with a valid client info on the same team, stamps a
64-entry, 20-byte table at `0x3020d0b0` with `cg.time` and the record's
`0x3020dd78`, `0x3020dd7c` and `0x3020dd88`, and, when `0x3020dc78` has
`0x80000` (`0x300135e1`) and the slot's flash time (`+0x10`) is not ahead
of `cg.time`, sets that to `cg.time + 3000` (`0x30013620`). INFERRED: the
two reads are `eType` (1, `ET_PLAYER`) and `eFlags` (bit 1, dead), and the
stamp is the interpolated origin's x and y and the interpolated yaw.

VERIFIED, `game.mp.i386.so`: `PlayerCmd_pingPlayer` (`0x550a4`) ORs 8
into the playerstate's `eFlags` byte `+0x82`, the `0x80000` bit, and stamps
`level.time + 3000` at client `+0x2268`; `ClientEndFrame` clears the bit
once that time has passed. VERIFIED, pak5's `_teams.gsc`: the quick-chat
commands call `self pingPlayer()` after `sayTeam`.

VERIFIED, `G_GetNonPVSFriendlyInfo` (`0x42c30`; Ghidra's import places it
at `0x52c30`): it returns 0 for a viewer whose `sess.team` (client
`+0x217c`) is 0 or 3 (`0x42c50`, `0x42c59`). VERIFIED: it walks 64 slots
from `last + 1` modulo 64, `last` being its third argument and 0 standing in
when that reads `0x3ff` (`0x42c65`), and skips a slot whose entity is not in
use (`+0x160`, `0x42ca0`), whose `s.eType` is not 1 (`0x42cad`), whose
`s.eFlags` has bit 1 (`0x42cba`), which has no client (`+0x158`) or whose
client's team differs (`0x42cd8`), or for which `trap_InSnapshot(eye,
s.number)` (`0x42cf2`, syscall `0x2f`) answers non-zero. INFERRED: the first
slot left is the answer, so the field cycles through every out-of-view
teammate, one per frame.
VERIFIED, the packing (`0x42d02`..`0x42f19`): each offset is the
teammate's `r.currentOrigin` (`+0x134`, `+0x138`) less the eye's, plus 0.5
(`0x7311c`), stored by `fistp` under a control word ORed with `0xc00`, so
truncated; an offset above 1024 gives a factor `1024.0 / offset`
(`0x73120`) and one below -1022 `-1022.0 / offset` (`0x73124`); when either
factor is below 1 the axis with the larger factor is multiplied by the
smaller one and truncated (`0x42df9`..`0x42e5e`); both are then clamped to
-1022..1024 (`0x42e64`..`0x42e98`) and packed as `(offset + 2) / 4 + 255`
with C's truncating division (`0x42ea3`..`0x42eda`), x at bit 6 and y at bit
15, 9 bits each; the client number (`s.number & 0x3f`) fills bits 0..5 and
`r.currentAngles[YAW]` (`+0x144`) times 256/360 (`0x73128`), truncated, the
top byte. VERIFIED: `ClientEndFrame` passes the playerstate origin with
`viewHeightCurrent` (`ps + 0xd0`) added to z, through `G_AddLean`
(`0x411c2`..`0x411e8`), and client `+0x2264` as `last` (`0x411ed`), stores
the answer at `iCompassFriendInfo` (playerstate `+0x3c0`, `0x41201`), and
writes `answer & 0x3f` back to `+0x2264`, or `0x3ff` for 0 (`0x4120e`,
`0x41240`). VERIFIED: with a non-zero answer it sets the playerstate's
`eFlags` `0x100000` when that teammate's `s.eFlags` has `0x80000` and
clears it otherwise (`0x4121d`..`0x41230`); a zero answer leaves the bit as
it was. VERIFIED: `ClientSpawn` zeroes the client up to `+0x22c4`
(`bzero` at `0x42804`), which covers `+0x2264` and the playerstate.
VERIFIED, `SV_inSnapshot` (cod_lnxded `0x8087b90`, syscall `0x2f`'s
handler): it returns 0 when the entity's `+0xf0` is 0 or its `+0xf4` has
bit 1, 1 when `+0xf4` has `0x18`, 1 when the `svEntity`'s cluster count is
0, and otherwise 0 unless one of two area tests (`0x8053f44`) passes and
one of the entity's clusters is set in the eye's row. INFERRED: those are
`r.linked`, `SVF_NOCLIENT`, the broadcast flags and the snapshot cull's
own PVS test, so the answer is whether a snapshot built from the eye would
carry the entity, except that an entity in no cluster counts as carried.
VERIFIED, 1.5b's and United Offensive's `SV_inSnapshot` (1.5b `0x808d5d2`,
`coduo_lnxded` `0x808e3a9`, syscall `0x32`'s handler; `trap_InSnapshot`
game.mp.uo `0x844e0`): after the same `r.linked`, `SVF_NOCLIENT` and `0x18`
tests it returns 1 for a non-zero `r.broadcastTime` (1.5b `+0xfc`, UO
`+0x100`), whatever its value; then 0 unless the eye's area connects to
one of the entity's two, 0 for an entity in no cluster, the cluster test,
and last the fog (`cod15-protocol-diff.md` 15). Against 1.1 an entity in
no cluster now counts as not carried, and the area test comes first.
VERIFIED disassembly: only the snapshot builder clears it, on the first
snapshot it builds whose `svs.time` is past it (`coduo_lnxded`
`0x8096d53`, 1.5b `0x8095dc6`). INFERRED, from that and the end frame
running before the snapshots: a sounded teammate is off the compass for
one end frame longer than he rides snapshots. VERIFIED decompile: on UO
`G_GetNonPVSFriendlyInfo` (`0x55762`) and `G_GetNonPVSTankInfo`
(`0x9bf6d`) are its only callers.

VERIFIED live, `client-probes/probe_snapcull` on mp_carentan, 2026-10-20,
retail 1.1d and UO 1.51b (`tests/fixtures/entities/snapcull-carentan-*.txt`,
gated by `uocull_ab`): slot 0 at (976, -864) reads allied slot 1 at
(-992, 2328), out of its PVS, as `0x00ff9881` on both. Held 1000 units under
the map (no cluster, area -1), slot 1 reads 0 on 1.1 and stays `0x00ff9881`
on UO. After `playSound` on slot 1 the word reads 0 for the rest of the run
on 1.1 (`SVF_BROADCAST`, and slot 1 rides slot 0's snapshots); on UO it
reads 0 from the sound's frame through the frame 350 ms later, the one
whose snapshot first leaves slot 1 out, and the word is back 400 ms after
the sound.
VERIFIED live, the same probe rerun 2026-10-20 on retail 1.1d, 1.5b and UO
1.51b with four more steps (`snapcull-carentan-{11,15,uo}.txt`; 1.5b's
gated by `cull15_ab`): 1.5b reads every row above as UO does, the
no-cluster teammate kept at `0x00ff9881` and the sounded one at 0 for
400 ms. Slot 1 is then set at (1120, 432, 0), in slot 0's PVS about 1300
units off, and the word reads 0 on all three. Under `setCullFog(0, 1000,
...)` it reads `0x00ffc6c1` on 1.5b and UO, the fog test hiding him, and
stays 0 on 1.1, where slot 1 still carries the earlier `SVF_BROADCAST`, so
1.1's row says nothing about a fog. On 1.5b and UO a `playSound` inside the
fog reads 0 for 400 ms, as without one: the broadcast time is tested before
the fog. `setExpFog` puts the word back to 0. Slot 1's own list empties
under the fog on 1.5b and UO (all it was sent lies past 1000 units) and
keeps all 22 entities on 1.1, whose builder has no fog test.
VERIFIED, `BG_PlayerStateToEntityState` (`0x2cbe8`): `s.eType` is 1 when
the playerstate's `pm_flags` has `0x10000` or `0x40000` and 7 otherwise.

VERIFIED live, `client-probes/probe_compass` on retail mp_harbor
2026-10-07, two allied probes, slot 0 standing at (-7352, -7976) and slot 1
set down at spawns 2 to 13: slot 0 read `0x00a08f01` with slot 1 at
(-8136, -7712), `0x00611001` at (-8120, -8224), `0x001b4841` at
(-7216, -8784), `0x004a8001` at (-9104, -8712), and 0 at (-6784, -7416) and
(-7562, -7344), where slot 1 was in its snapshot. Slot 1 read slot 0 as
`0x005f70c0` from (-8136, -7712) and `0x00ffd340` from (-7680, -9056), its
y clamped to 1024 and its x scaled from 328 to 312. On the frame
`setPlayerAngles` turned slot 1 to 45, 270 and 180 the top byte read
`0x20`, `0xc0` and `0x80`, and 0 again the frame after, when the probe's
own cmd angles took over. The `0x100000` bit rose on slot 0 one frame after
slot 1's `pingPlayer` and fell one frame after slot 1's own `0x80000`, 3 s
later: slot 0's end frame runs before slot 1 copies its playerstate into
its entity. The field read 0 while slot 1 was on axis, came back with it on
allies, stayed through a `suicide()` that left slot 1 `playing`
(`pm_type` 0), went to 0 a frame after its `sessionstate` became `dead`
(`pm_type` 6), and stayed 0 with it a spectator. The raw lines are in the
probe's README section's recipe; the packings are
`crates/server/src/compass.rs`'s test.

VERIFIED, the cgame's read of it (`0x30013646`..`0x300137a5`): a non-zero
`+0x3c0` stamps slot `info & 0x3f`, decodes each offset as
`field * 4 - 0x3fc`, tests both against 1024.0 (`0x44800000`) and -1020.0
(`0xc47f0000`), takes the yaw as the signed top byte times 1.40625
(`0x30069600`), and arms the 3 s flash on the playerstate's `eFlags`
`0x100000` (`0x3001378c`). INFERRED, off the two arms: an offset at either
clamp stores the pair normalised (`0x30039d50`) as a bare direction;
otherwise it stores the playerstate's origin plus the offsets plus a lean
offset (`0x3003f4e0`, with 16.0 and 20.0), which vcod leaves out.

VERIFIED, the draw loop (`0x30013830`..`0x30013b9f`), at the defaults
(`cg_hudCompassSize` 1, `cg_hudCompassMinRange` 0, `cg_hudCompassMaxRange`
1024, `cg_hudCompassMinRadius` 0, `cg_hudObjectiveMinAlpha` 1): it resets
a slot whose time is ahead of `cg.time` to 0, skips a slot older than 800
ms and the viewer's own `clientNum`, and tests the stored x and y against
1.0 (`0x30069328`, a double). INFERRED, off its arithmetic: a stored point
sits as an objective does, `43.75 * clamp(distance / 1024)` from the
compass centre along its bearing off the sprung compass yaw (`0x3020d0a8`,
`0x30019f00`); a stored direction sits at the full 43.75; the icon is
`10 * cg_hudCompassSize` (`0x300693e4`) square; the alpha is 1. VERIFIED:
`gfx/hud/hud@objective_friendly.tga` and
`gfx/hud/hud@objective_friendly_chat.tga` are registered to `0x301d5d14`
and `0x301d5d18` (`0x30021024`, `0x30021035`). VERIFIED: while the flash
time is ahead, `(flash - cg.time) % 500 >= 250` (`0x30013b20`..`0x30013b32`)
picks the chat icon through the plain pic call `0x300192d0`; otherwise the
friendly icon goes through the turned pic call `0x30019330` with the angle
`ANGLE2SHORT(viewangles[YAW] (0x302095d0) - yaw)` in degrees. INFERRED: the
icon turns the way the compass face does, by the viewer's yaw less the
teammate's.

vcod draws the snapshot's teammates off their interpolated origins with
the snapshot's yaw, and the packed one, with the compass turned by the view
yaw (no spring). vcod's server writes `iCompassFriendInfo` and the
`0x100000` bit from each playing or dead client's end frame
(`crate::compass`, `compass_friend` in `server.rs`), its candidates the
clients whose `sessionstate` is `playing` and whose sim is a live player,
which is what the probe saw retail answer for; it reads every teammate's
state as of this frame, where retail reads a higher slot's as its last end
frame left it, and skips the area check, which stock maps never fail.

### Cursor hint

VERIFIED, the registrations: `hintActivate`, `hintNoActivate`, `hintDoor`,
`hintNoDoor`, `hintMg42`, `hintHealth`, `hintLadder` and `hintFriendly` are
stored at `0x301d5ad8` to `0x301d5af4`, four bytes apart, and the weapon
setup stores a material at `0x301d5af4 + 4 * weapon` and another at
`0x301d5bf4 + 4 * weapon`. INFERRED, off the weapon setup's branches: the
first is the weapon's `hudIcon` and the second its `ammoIcon`, each
`hintActivate` when the key is blank. VERIFIED, the setup (`0x30034cf0`)
stores `hintActivate` (`0x301d5ad8`) for a blank key at `0x30035c80` and
the registered icon at `0x30035c71`, the ammo slot at `0x30035cdc`; 1.5's
at `0x30037602` and `0x30037657`/`0x30037666`, UO's at
`0x300446d7`/`0x300446e6` (`0x3044b720 + 4 * weapon`) and
`0x3004473b`/`0x3004474a` (`0x3044b920`). VERIFIED: the draw loads the
hint's slot (`0x3002523f`; 1.5 `0x300265af` off `0x301d9658`, UO
`0x300303e8` off `0x3044b6f8`) and leaves when it reads 0 (`0x30025255`,
1.5 `0x300265c5`, UO `0x300303fe`), ahead of the icon and the sentence.
INFERRED: only `CG_RegisterWeapon` writes a weapon's slots, so the slot of
a weapon never set up (no def) stays 0 and its hint draws nothing; vcod
draws nothing for a weapon with no def (`hud::player::hint_material`,
`hud::uo::hint_material`). INFERRED, off
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
`0x300696d4` reads 0.0066667 and `0x30063188` is `"[%s]"`. VERIFIED, the
fill (`0x30025792`..`0x30025899`): the string goes through the
`[{command}]` pass with `"Hint String"`, a copy is case-folded
(`0x3003e9b0`) and searched for `[%s]`, and the original up to the `[`,
`+activate`'s key text (`0x3006186c`, through `0x30046940`) or, when that
reads unbound, `KEY_USE` localized (`0x30063160`, key.str `"use"`), and the
original from the `]` on are joined.

The cgame's own sentences. VERIFIED, `0x300251f0` and `cgame.str`: the
weapon branch loads `CGAME_PICKUPNEWWEAPON` ("Hold USE [%s] to pick up
the %s", `0x3002548c`) and `CGAME_SWAPWEAPONS` ("Hold USE [%s] to swap")
with `CGAME_FOR` and `"%s %s %s %s"` (`0x300253c1`..`0x30025410`), the
ammo branch `CGAME_PICKUPAMMO` ("Press USE [%s] to pickup the %s ammo",
`0x3002560d`) and the hint 7 branch `CGAME_PICKUPHEALTH` ("Press USE [%s]
to pickup health", `0x30025942`); each takes `+activate`'s key text from
`0x30046940`, which writes the localized `KEY_UNBOUND` for an unbound
command (no `KEY_USE` fallback on these paths). VERIFIED: the names come
from `0x301a69e4 + 0x198 * w`, the field the weapon name draws
(`0x30023c30`).

INFERRED, off the branches: a weapon hint (10..73, weapon `hint - 9`) and
an ammo hint (74..137, weapon `hint - 73`) never read the server's
string; hint 7 reads it first and prints its own sentence only when none
is sent. For a weapon, `0x3000f680` (weapon, predicted ps) returns a free
slot: for slot type 1 or 2 (def `+0x78`) `weaponslots[1]` or `[2]` when
empty, for 3..5 the type's own slot, else 0. With one, the pick-up string
has the key text spliced between its brackets (an unfolded `strstr` of
`"[%s]"`, `0x3002549d`) and then formats the name. Without one the old
weapon is the predicted `ps.weapon` (`0x3020720c`) when its slot type is
the new weapon's, else `weaponslots[type]` (`0x30207470`); when that is
the hinted weapon the function leaves (`0x300253bb`, `0x30025426`) ahead
of the icon draw at `0x300259d7`, so nothing is drawn. Otherwise the line
is the swap string formatted with the key text, the old name, `for` and
the new name. Ammo splices and formats as the pick-up does; health formats
the key text in. Each line is placed as the server's string is. VERIFIED:
1.5's `0x30026560` holds the same compares (`0x3002669b`, `0x30026c96`)
and string loads; UO's `0x300303a0` the same with its numbering (weapons
11..138 at `0x300304eb`, ammo 139..266 at `0x3003078d`, health 8 at
`0x30030ae9`; slot types 1..7 in `0x30011520`). vcod:
`hud::player::hint_text`.

### Damage direction

VERIFIED: `0x3002faa0` and `0x30028d00` pass `0x300287f0` the
playerstate's `damageYaw`, `damagePitch` and `damageCount`, and each reads
`damageEvent` from the current and the previous playerstate and
`damageCount` from the current one. INFERRED, off the compares each call site
makes on those reads: the call is made when the event differs from the
previous playerstate's and the count is not 0, so a changed event with a
non-zero count is a hit, which is the increment the server makes
(`cod11-combat.md`).

INFERRED, off `0x300287f0`: yaw and pitch both 255 add no icon; otherwise the
oldest of eight slots takes the time and `damageYaw / 255 * 360` plus a random
jitter of up to 10 degrees either way. VERIFIED, `0x300287f0`..: the slot's
yaw is `SHORT2ANGLE(ANGLE2SHORT((rand() / 32768.0 - 0.5) * 20.0 + yaw))`,
`rand` at `0x3004b189`, the constants `0x30069330` (32768.0), `0x3006930c`
(0.5), `0x30069448` (20.0), `0x30069380` and `0x3006937c`. vcod draws it
with MSVC's `rand` sequence from its own seed.

**The red flash.** VERIFIED, `0x30017250`, called from the 2D pass
`0x30018810` at `0x3001884a`, ahead of the hit icons (`0x300172f0`):
with a snapshot up and `cg.time` short of `0x3020c9e0`, it fills
(-10, -10, 650, 490) on the 640 grid through `CG_FillRect`
(`0x30018d10`, the `white` shader `0x301d5a84`) in (0.2, 0, 0) at alpha
`min(5, |(end - cg.time) * 0.002 * [0x3020c9e4]|) * 0.2 * 0.7`
(`0x300694ec` 0.002, `0x30069414` 5.0, `0x300693d4` 0.2, `0x300694f0`
0.7). VERIFIED, `0x300287f0` stores `cg.time + 500` at `0x3020c9e0` after
either arm, so every hit flashes, and the strength is the kick along the
view, 0 for a hit square from the side. INFERRED: a hit from in front or
behind with a kick of 20 holds 0.7 until 125 ms are left and fades out by
the 500th. vcod draws it (`DamageIndicators::build_flash`).

**When the feed runs.** VERIFIED, `0x3002fc0d`..`0x3002fc5e`, the tail
of `0x3002faa0`: when any of `[0x301e2148]`, the new snapshot's
`pm_flags & 0x10000`, `[0x301e172c]` or `[0x301e1bac]` is set,
`damageEvent` (`+0xe4`) of the new and the old snapshot's playerstate are
compared and `0x300287f0` is called on a change with a non-zero count,
then `0x30028740` and `0x30028be0`; nothing tests `clientNum`, `eFlags`
or `pm_type` first. VERIFIED: `0x30028d00`, called at `0x30029a3f` near
the end of `0x300294f0` (`CG_PredictPlayerState`, `cod11-movers.md`),
makes the same compare and call on the playerstates in `eax` and `esi`.
INFERRED: the four flags are demo playback, `PMF_FOLLOW`, `cg_nopredict`
and `cg_synchronousClients` (Q3's `CG_TransitionSnapshot`), the `eax`/`esi`
pair is the predicted playerstate and the one before it, and both are
`CG_TransitionPlayerState` inlined: every frame's prediction, or a
followed snapshot's transition, feeds a hit, mounted, dead or with the 2D
pass off, and a follow switch onto a player whose `damageEvent` differs
feeds that player's last hit. VERIFIED, `0x300287f0`: the kick's dots
read `0x302095a0`..`0x302095b4`, the refdef's `viewaxis[0]` and `[1]`
(`0x30209594` is `refdef.vieworg`, turrets doc 14.3). INFERRED: the
transition runs before `CG_CalcViewValues` rebuilds the axis, so a hit
splits along the view drawn the frame before, kick, quake and turret
included. vcod feeds it from `Hud::feed_damage` every frame ahead of the
snapshot's events, on the newest snapshot's playerstate, along the last
drawn view (`Hud::drawn_view`; the playerstate's view before the first
frame); the first playerstate seen is the only baseline.

INFERRED, off `0x300172f0`: each slot younger than `cg_hudDamageIconTime` draws
`hudHitDirection` over `(-64, 32)` to `(64, 96)` about the screen centre,
turned by the view yaw less the slot's yaw, with alpha
`min(1, 2 - 2 * age / time)`. So an icon below the crosshair is a hit from
behind. VERIFIED: the centre is `0x300695e4` (320.0) and `0x300695e0`
(240.0). The sense of the turn is inferred from that geometry, not measured.

VERIFIED, `0x300172f0`: the box comes from cvars, not constants: half of
`cg_hudDamageIconWidth` (128, vmCvar at `0x301dafa0`) either side,
`cg_hudDamageIconOffset` (32, `0x301d9920`) to offset plus
`cg_hudDamageIconHeight` (64, `0x301d9da0`) below the centre, the corners
stored `(-w/2, off)`, `(w/2, off)`, `(w/2, off+h)`, `(-w/2, off+h)`. The
defaults come from the cvar table rows beside each name. VERIFIED:
`0x30017445` pushes the texture coordinates `0x30060fb4`, which read
`(1,1) (0,1) (0,0) (1,0)` in that corner order, to `0x300194f0`, which turns
each corner `(x cos - y sin, x sin + y cos)` and hands corners, coordinates
and shader to trap 0x4c. INFERRED (trap 0x4c pairs corner i with coordinate
i): the art goes on turned half round, its top row on the far edge. The art
(`gfx/hud/hit_direction.dds` in `pak0.pk3`, shader `hudHitDirection`) is
wide at the top and narrows down, so the wide end faces away from the
crosshair. VERIFIED: 1.5's `cgame_mp_x86.dll` (`075e2af1...`, array
`0x30062fb4` pushed at `0x300184d5`) and UO's `uo_cgame_mp_x86.dll`
(`d83491ca...`, `0x30071854` pushed at `0x3001aad5`) carry the same array
and the same 128/32/64/2000 defaults.

### Mod items

- `0x30022ba0` registers `cg_hudFiles` with the default `ui_mp/hud.txt`
  (trap 9) and reads it back (trap 0xb). VERIFIED (strings). The file's
  `loadMenu` entries are loaded by `0x30022420`, `0x30022350` and
  `0x30022260`, which falls back to `ui_mp/testhud.menu`. INFERRED (call
  shape). Stock `hud.txt` names only `ui_mp/hud.menu`. VERIFIED
  (`pak0.pk3`).
- `0x3004a6f0` paints every loaded menu whose window flags carry 4
  (`WINDOW_VISIBLE`) and whose owner-draw-flag test passes: the menu's
  window, then each item through `0x30047fb0`. INFERRED (shape of RTCW's
  `Menu_PaintAll`). An item with no `text` and a `cvar` prints the cvar's
  value, as RTCW's `Item_Text_Paint` does. INFERRED (lineage).
- Every item of stock `hud.menu` is an owner draw, which vcod draws
  natively (above). VERIFIED (`pak0.pk3`). A mod's replacement adds plain
  items: `zzz_zfunmod.pk3` (a public server, 2026-10-09) adds menus
  `fm_corners` and `fm_announce` whose `type 1` items read `scr_fm_tl_d`,
  `scr_fm_bl_d`, `scr_fm_mk` and the like, which the server sets per
  client with `v` (seen live: `v scr_fm_tl_d "^3King of the Day: ..."`).
  VERIFIED.
- vcod (`crates/client/src/hud/hudmenu.rs`) loads `hud.txt`'s menus and
  paints the visible items of visible menus that are not owner draws:
  `WINDOW_STYLE_FILLED` and `WINDOW_STYLE_SHADER` backgrounds, then the
  text or the cvar's value (a `v`, else the 140/204 mirror), placed as the
  front end places text (`cod11-front-end.md` section 12), with
  `cvartest` gates. It paints them right after the native items on the
  views that draw the native HUD. `textstyle`, `textfont`, borders and
  item scripts are ignored.

### What vcod does not draw

| Retail piece | Where | vcod |
|---|---|---|
| The packed out-of-view teammate's lean offset | `0x3003f4e0` | left out |
| The weapon name of a pending selection, the pickup stamp | `0x30023c30`, `0x3001da80` | the playerstate's weapon, no pickup stamp |
| `adsAimPitch`, shared ammo caps | above | left out, each noted above |

---

## 10. vcod: wide windows

This section is vcod's own design, not retail's. Retail draws its 2D layer
on the 640x480 grid stretched over the whole window, so on anything but 4:3
every HUD piece and menu is squashed sideways. vcod keeps the grid's
aspect and decides where each piece goes. The code is
`crate::hud::hudelem::Virtual` and `Anchor`.

What it rests on from retail:

- VERIFIED, `ui_mp/hud.menu` (`pak0.pk3`): the stock HUD is plain grid
  rects with no alignment keyword: the stance icon at 100,434.375, the
  "weaponinfo" menu spanning 0..640 with the weapon name at 242.5..562.5
  and the ammo counter at 557.5..637.5, the health block at 488..631, the
  compass at the left, the cursor hint at 300,325.
- VERIFIED, the stock gametype scripts (`maps/MP/gametypes/*.gsc`,
  `pak5.pk3`): hud elems carry only `x`, `y`, `alignX`, `alignY`. The
  clock, the killcam titles, the respawn and spawn texts sit at 305 or 320
  centred; the bomb and flag progress bar is `alignX "left"` at
  `320 - barsize / 2`; the killcam bars and the black screen are `x 0`
  shaders 640 wide; `bel`'s point counter and its back sit at 505-520,
  and a right-aligned elem at 635.

vcod's rules:

- The grid is scaled to fit the window and centred (`Virtual::new`): by
  the height on a window wider than 4:3, with bars at the sides, by the
  width on a narrower one, with bars above and below.
- With `vc_widescreen 1` (the default) a piece retail draws at a side of
  the screen moves to that side of the window by the side bar's width
  (`Virtual::left`, `Virtual::right`); a piece in the middle stays in the
  box. With `vc_widescreen 0` every piece stays in the box.
- A rect that spans the whole grid width (`hudelem::full_width`: from 0 or
  less to 640 or more) or height stretches over the whole window, at any
  `vc_widescreen`: the damage flash, killcam bars, a script's black screen,
  a menu backdrop or fade.
- A piece with no fixed side, a hud elem or a mod's `hud.menu` item,
  keeps to the third of the grid its drawn span's middle is in
  (`Anchor::of_span`: left of 213.3, right of 426.7). An elem's span is its
  box after `alignX`, so the left-aligned progress bar from 192 to 448
  stays centred and `bel`'s counter at 505 moves right.

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

The weapon name and ammo move as one block though the name's rect
reaches past the middle, since its back butts onto the ammo backdrop.

Menus, the script menus included, draw in the centred box with no edge
anchoring (`frontend::Painter`); their full-width rects stretch as above,
and the cursor maps from window pixels through the box, clamped to its
edges (`Virtual::grid_point`). The raw mouse motion moving it is unscaled,
as retail's (cod11-front-end.md section 13).

The 3D view's fov follows the same cvar (`camera::view_fov_x`, Hor+:
every fov the game names is a 4:3 view's width, and a wider view keeps
that vertical fov). The crosshair's side arms read the fov a 4:3 view as
tall would be wide, which at 4:3 is retail's `640 / fov_x`
(`hud::player::arm_offset`).

## UNVERIFIED summary

| # | Claim | Check that settles it |
|---|---|---|
| 1 | `killIcon<Mod>` material names bind to `gfx/hud/death_<mod>.dds` (section 2). No stock pk3 entry carries the name; the mapping is by name correspondence only. | Run the stock 1.1 MP client, take a melee death, screenshot the killfeed. |
| 2 | Resolved: scores token 4 is `deaths`, written by script (section 3). | n/a |
| 3 | Resolved: the `b` grammar, confirmed on screen against a populated server's Tab scoreboard (section 3). | n/a |
| 4 | Resolved: the hudelem transport is the playerstate's array block 5, filled per client by `HudElem_UpdateClient` (section 5, "How a hudelem reaches the client"), and what the cgame draws from it is section 8. | n/a |
| 5 | The second font header float's id-internal field name (section 6). Its use is settled. | Nothing downstream depends on it. |
| 6 | Whether the stock client's own status header shows the same CS-5/6-vs-`b` transient disagreement observed live (section 5). vcod's header prefers the `b` reply once one has arrived. | Compare the stock client's header text to its own Tab scoreboard in the same frame against a server showing a fresh score change; not yet done. |
| 7 | Trap `4`'s official name (section 1). Its nine-argument signature is settled from the call site. | Decompile the engine's cgame syscall dispatcher and read case 4. |
