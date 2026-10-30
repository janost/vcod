# Quick chat (`vsay`, the `j`/`k`/`l` server commands)

How CoD 1.1 MP carries quick chat end to end, and why it is dead code on
stock installs. Evidence comes from three modules: `game.mp.i386.so`
(1.1d Linux dedicated, full symbols - server side), `game_mp_x86.dll` 1.1
(same code, per `cod11-sound-system.md`), and `cgame_mp_x86.dll` 1.1
(image base `0x30000000`; md5s in `cod11-events-and-fx.md`). Addresses are
virtual. Nothing here has been observed on the wire from a stock server;
every fact is read from the binaries and labelled where it is only
inferred. The live captures so far (see `cod11-sound-system.md`) came from
modded servers that bypass this whole mechanism with `EV_SOUND_ALIAS`.

## Server side

`G_Voice` (`game.mp.i386.so`, symbol `G_Voice@@Base` @ `0x473b8`; Windows
`game_mp_x86.dll` at the same VA):

- Spam gate first: a per-entity accumulator at `ent+0x308` compared against
  level time (`0x1e8`), capped near 30 s (`cmp $0x752f`); exceeding it sends
  the `GAME_SPAMPROTECT` notice (string @ `0x73a86`) instead.
- The chat scope is the caller's mode argument (`0x10(%ebp)`): `1` selects
  the letter `"k"` (@ `0x73a48`), `2` selects `"l"` (@ `0x73a4a`),
  anything else `"j"` (@ `0x73a4c`).
- It then formats `"%s %d %d %d %s %i %i %i"` (string @ `0x73a4e`) and sends
  it as a reliable server command. Push order at `0x474db..0x4752f`: the
  letter, three ints taken from the entity fields, the category string
  argument, and three more ints that are the speaker origin xyz each loaded
  as float and converted with `fistpl` (`flds 0x20/0x1c/0x18(%ebx)`).

So one command looks like:

```
j <int> <int> <int> <category> <origin_x> <origin_y> <origin_z>
```

with every numeric field printed as a decimal integer and re-read by the
client through `atol`. The client uses argv 1 as the scope echo (its value
selects the chat-line printf format and the team-only gate), argv 3 as the
speaking client index, and argv 5-7 as the speaker origin; argv 2's meaning
was not pinned down.

## Client dispatch and handling

All addresses below are `cgame_mp_x86.dll` 1.1.

- Dispatch: console-string cases `'j'` `'k'` `'l'` (`0x6a/6b/6c`) call
  `FUN_3002d940(0/1/2)` (call sites `0x30026437..0x30026443` in the export).
- `FUN_3002d940` @ `0x3002d940`: reads argv 1-3 via trap_Argv (syscall `0xd`)
  through `atol` (@ `0x3004afde`), keeps argv 4 as the category string, and
  converts argv 5-7 back to floats (`fild`/`fstp` pairs). If global
  `DAT_301ad3cc` is set, it drops the auto-taunt categories early:
  `kill_insult` (@ `0x30062a54`), `taunt` (@ `0x30062a4c`),
  `death_insult`, `kill_gauntlet`, `praise`. What sets `DAT_301ad3cc` was
  not traced; the strings are the Q3 voice-chat category names.
- `FUN_3002d770` @ `0x3002d770` (`__thiscall`; ECX points at a vec3 the
  caller assembled on its own stack from argv 5-7 - the speaker origin from
  the command, not entity state): clamps argv 3 to `0..63` as the speaking
  client index,
  bails if `clientinfo[idx].field_0x00 == 0` (per-client structs stride
  `0x448` from `0x3018bc0c`), then picks the voice table:
  default `EBX = 0x302f7d80` (the axis table), switched to `0x30340ec8`
  (allies) when `clientinfo[idx].field_0x2c != 1` - i.e. team value `1`
  means Axis, matching the HUD convention (team 1 Axis / 2 Allies). A mode
  check passes when the mode argument is `1` or global `DAT_3029824c == 0`.
- Selection, `FUN_3002d390` @ `0x3002d390` (EBX = table base, stack args =
  category string plus three out-pointers): scans up to
  `count = *(table+0x44)` categories, records starting at `table+0x48` with
  stride `0x1244`; the match test is `FUN_3003e8f0(ECX = record)` returning
  0 on hit. On the first hit it draws `rand()` modulo the variant count at
  `record+0x40` and returns:
  - `record+0x44 + j*4` - the registered sound handle (array A),
  - `record+0x1144 + j*4` - the head-icon material handle (array B),
  - `record+0x144 + j*0x40` - pointer into an inline per-variant struct
    area whose purpose is unknown in 1.1 (the parser never writes it).
- Queueing: the entry `{pos.xyz, category copy char[149], client index,
  handles...}` lands in a ring at `DAT_305407c0` indexed by `DAT_3020d094`
  masked to 32 slots. Drain, `FUN_3002d5d0` @ `0x3002d5d0`: at most one
  entry per 1000 ms (`DAT_3020d08c = time + 1000`).
- Presentation, `FUN_3002d470` @ `0x3002d470`: copies the head-icon handle
  into the speaker's clientinfo (drawn over their head elsewhere) and
  prints `": <text>"` to the console. The text itself goes through the
  localizer `FUN_3002d670` @ `0x3002d670` (syscall `0x38` lookup), which
  falls back to wrapping the raw key in `#UNLOCALIZED# ... #`. The chat-line
  printf formats in `FUN_3002d770` are `"%s %s(%s): %c%c%s"` (global),
  `"(%s)%s(%s): %c%c%s"` (mode 1) and `"[%s]%s[%s]: %c%c%s"` (mode 2).

## The `.voice` file format

Parsed by `FUN_3002cbd0` @ `0x3002cbd0`; called once at init from
`FUN_30020a00` @ `0x30020a00`:

```
FUN_3002cbd0("mp/axis_chat.voice",   &DAT_302f7d80, 0x40);
FUN_3002cbd0("mp/allies_chat.voice", &DAT_30340ec8, 0x40);
```

Mechanics, all VERIFIED against the decompilation and asm:

- Read through the fs traps (`0xf` open/size, `0x10` read, `0x12` free);
  files larger than `0x4000` bytes are rejected ("voice chat file too
  large").
- Tokens come from the shared tokenizer `FUN_3003da70` (`GetToken`,
  Q3-style). Category and brace comparisons are case-insensitive.
- Grammar:

```
<gender>                      "male" @0x30062be4 -> table+0x40 = 0,
                              "female" @0x30062bec -> 1; anything else that
                              still matches a third check -> 2, else the
                              parse aborts ("expected gender not found")
<category>                    repeated until 64 categories (arg 3)
{
    <soundAlias> [<headIconShader>]
    ...                       up to 64 entries, until "}"
}
```

The head-icon slot is a **same-line** read: every other token comes from
`GetToken(allowLineBreaks = 1)` but the slot after a sound alias uses
`GetToken(0)` (second call site in `FUN_3002cbd0`, flag 0 vs 1 elsewhere).
An entry therefore pairs within one line - `alias` alone, or
`alias headicon`; an `}` in that slot is pushed back (`UngetToken`) and the
default icon applies. Two aliases on one line make the second one the
first's head icon, not a new entry; tokens for categories and braces flow
across lines freely.

- Each `soundAlias` is registered with syscall `0xbf`, stored into array A.
  The same syscall registers known sound aliases like
  `mp_announce_g_twominutes` right next door in `FUN_30020a00`, so it is
  `S_RegisterSound` (INFERRED name, verified usage).
- The optional second token becomes array B via `FUN_30030b70(name, 2)`
  (INFERRED register-shader/material); missing or empty defaults to
  `headiconVoiceChat`. When the token after a sound alias is `}`, it is
  pushed back (`UngetToken`) and the default icon applies.
- Per-category record layout (stride `0x1244`): match data `[0x00..0x40]`,
  entry count u32 at `+0x40` (incremented in place while parsing),
  array A at `+0x44` (up to 64 dwords), inline structs `+0x144..0x1143`,
  array B at `+0x1144` (up to 64 dwords). `0x44 + 64*4 = 0x144` and
  `0x1144 + 64*4 = 0x1244` exactly.

The aliases these files name are the `american_*` / `british_*` /
`russian_*` / `german_*` rows of `dialog_mp.csv` (see
`cod11-sound-system.md` section on quick chat).

## Negative result: stock installs ship no `.voice` data

Every `.pk3` in both local installs (CoD 1.1 `main/`, Deluxe Edition
`main/` and `uo/`, including all `localized_english_pak*.pk3`) lists zero
`*.voice` entries (checked with a zipfile walk over every pak), and a
loose-file sweep finds none either. On a stock client `FUN_30020a00`
therefore logs "^1voice chat file not found: mp/axis_chat.voice" twice,
both tables stay empty, `FUN_3002d390` always returns 0, and `FUN_3002d770`
returns without touching the queue. Stock quick chat prints nothing, plays
nothing, shows no head icon. Any populated stock server can send `j/k/l`
commands all day; nothing happens client-side unless a mod ships `.voice`
files.

This makes "wire it like retail" an inert path on stock data, which vcod
accepts deliberately (full parity). The alternative - shipping our own
`.voice` files built from `dialog_mp.csv` - would be new game data, not RE,
and is out of scope.

A live sweep against three populated public servers (2026-08-26, one
TDM, one S&D and one more, 75-90 s each) sent no `j`/`k`/`l` command at
all. All three are
moddedicated servers (`g_scriptMainMenu` sets `team_*`, `i`/`f`/`v`/`t`/
`n`/`h`/`s` commands, `MPSCRIPT_*`/`scr_fm_*` text), which relay voice
lines through `s <idx>`/events instead. The stock `j`/`k`/`l` path remains
unobserved live and is not exercised by the current public-server
population.

## The quick message menu (`mp_QuickMessage`)

This is the stock path that does reach players: the `V` menu, whose rows
make the server's gametype script play a sound alias and send a team chat
line. It never uses `j`/`k`/`l`.

- `mp_QuickMessage` is a cgame console command: the command table row at
  `cgame_mp_x86.dll` `0x30061078` pairs the string at `0x30061904` with
  `0x300140e0`. VERIFIED (the table bytes). CoDMP.exe holds no such
  string, so the engine never forwards it to the server. VERIFIED (string
  sweep). `default_mp.cfg` (`localized_english_pak0.pk3`) binds it:
  `bind v "mp_QuickMessage"`. VERIFIED.
- `0x300140e0` reads `cg.snap` (`0x301e2160`), tests `pm_flags` at
  `+0x18` (the playerstate starts at `+0xc`) against `0x40000`, and only
  when that bit is set calls cgame syscall `0x7a` with the string
  `UIMENU_WM_QUICKMESSAGE` (`0x300655c0`). VERIFIED (the disassembly at
  `0x300140e0..0x300140fe`). So the menu opens only with the client's own
  view (`cod11-player-clip.md` 6.2: playing or dead, never a spectator or
  a follow). INFERRED, from that bit's meaning.
- CoDMP.exe's cgame syscall switch (`0x401e16`, jump table `0x4041f0`,
  entry `0x7a` is `0x4034f9`) handles `0x7a`: with `cls.state`
  (`0x155f2c0`) 6, no demo playing (`0x15ef004`) and a non-null name, a
  match on `UIMENU_WM_QUICKMESSAGE` (`0x568b08`) sends the UI VM command 7
  with 8, and `UIMENU_WM_AUTOUPDATE` (`0x568af0`) sends 7, 9. VERIFIED
  (the disassembly at `0x4034f9..0x403562`). The compares that follow,
  against `UIMENU_SCRIPT_POPUP` and `UIMENU_SCRIPT_POPUP_NO_MOUSE`, were
  not read.
- `UI_SetActiveMenu` case 8 (`ui_mp_x86.dll` `0x4000da11`) puts the UI
  cursor at 639, 479, sets the UI key catcher (`0x400194d0` with 2), runs
  `Menus_CloseAll` (`0x40010660`) and opens `quickmessage` (`0x4002e2cc`,
  through `0x400134b0`). VERIFIED (the disassembly); the function names
  are INFERRED from `cod11-front-end.md` section 1.
- The menu is `ui_mp/wm_quickmessage.menu` (`pak0.pk3`), loaded through
  `ui_mp/ingame.txt`. `fullscreen 0`, no `onEsc`; `onOpen` sets
  `cl_bypassMouseInput` 1 and `onClose` 0, so the mouse keeps turning the
  view while the keys go to the menu. Its four text rows are decorations
  with no action; each is followed by `execKey "1"` .. `"3"`, which run
  `close quickmessage; open quickcommands` (`quickstatements`,
  `quickresponses`). VERIFIED (the file).
- `quickcommands.menu`, `quickstatements.menu` (keys 1-8) and
  `quickresponses.menu` (keys 1-7) in `ui_mp/scriptmenus/` have the same
  shape: decoration rows, each followed by `execKey "<n>" {
  scriptMenuResponse "<n>"; close <self> }`. VERIFIED (the files). The
  stock gametypes `precacheMenu` all three (`tdm.gsc:167-169`), so they sit
  in the script-menu configstrings.
- `scriptMenuResponse` (`ui_mp_x86.dll` `0x40011140`) finds the index by
  name: the first of configstrings `0x49c + i`, `i` 0..31, that matches
  the menu's name (`0x40005e00`), -1 when none does, then executes `cmd mr
  %i %i %s` (`0x4002dce4`) with `sv_serverid`, the index and the
  response. VERIFIED (the disassembly).
- The server side is script: `tdm.gsc:449-454` routes the three menus to
  `_teams.gsc` `quickcommands` / `quickstatements` / `quickresponses`,
  which pick a nationality's alias and a `&"QUICKMESSAGE_*"` string, refuse
  a spectator or a player inside the 2 s `spamdelay`, and call
  `doQuickMessage` (`_teams.gsc:1629`): a playing player gets the
  `headicon@quickmessage` head icon, `playSound(alias)`, then `sayTeam`
  and `pingPlayer` (or `sayAll` with `level.QuickMessageToAll`).
  VERIFIED (the scripts).
- Esc closes the quick menus without a response: they have no `onEsc`.
  INFERRED from Team Arena's `_UI_KeyEvent`, which runs `Menus_CloseAll`
  on Esc when no fullscreen menu is visible.

## vcod behaviour and deliberate divergences

- `mp_QuickMessage` (bound to `V` by default, as `default_mp.cfg` does)
  opens `quickmessage` when the game is live and the newest snapshot's
  `pm_flags` has `0x40000` (`Ui::open_quick_message`, case 8). The menu
  is the front end's own copy from `ui_mp/ingame.txt`, `cl_language`'s
  directory first, drawn from the file at `32 256`; its `execKey`s open
  the quick-chat script menus the cgame loaded, whose `execKey`s send
  `mr` with the index looked up by name. Of the keys only those digits
  and Esc reach the menus; every other key and the mouse reach the binds,
  as `cl_bypassMouseInput` lets them (`cod11-front-end.md` section 13,
  "`cl_bypassMouseInput` and the keys"). Retail draws the cursor parked
  at `639 479`; vcod draws none.
- After a `ui_load` retail's `ingame.txt` menus are gone, so
  `Menus_ActivateByName` finds no `quickmessage` (0x400134b0 returns on a
  missing name, `cod11-front-end.md` section 13) and `mp_QuickMessage`
  only closes the open menus. vcod asks the UI whether the menu is loaded
  and, when it is not, closes every menu through its `onClose` and opens nothing; a
  mod whose own `menus.txt` lists a `quickmessage` keeps it, as retail
  does. Case 8 still sets the UI key catcher with no menu up. Retail's
  `_UI_KeyEvent` (0x4000d6f0, `vmMain` case 3), when `Menu_GetFocused`
  (0x40016970) finds nothing, reads the catcher (syscall `0x30`), clears
  bit 2 (`and eax, 0xfffffffd` at `0x4000d757`) and writes it back
  (syscall `0x31`), runs syscall `0x2f` unless `0x401bf428` is set (it is
  set only when a focused menu saw `cl_bypassMouseInput` 1), and sets
  `cl_paused` 0; it does nothing else with the key. VERIFIED (asm).
  `CL_UISystemCalls` (CoDMP.exe `0x418300`, jump table at `0x418c68`)
  sends `0x30` to a read of the catcher word `0x155f2c4`, `0x31` to
  `Key_SetCatcher` (`0x4180a0`, which keeps the console bit) and `0x2f` to
  `Key_ClearStates` (`0x40e260`). VERIFIED (asm). `Key_ClearStates`
  zeroes `anykeydown` (`0x142f75c`) and, for each of the 256 keys (12
  bytes from `0x142f780`) still down, calls `CL_KeyEvent` with down 0 and
  time 0 before clearing its down flag and repeat count. VERIFIED (asm).
  With bit 2 already off, each such release runs the key's `-` command
  when its bind starts with `+` (`0x40defc`); the stray key itself is down
  by then, so its own release goes out too. INFERRED (control flow).
- `CL_KeyEvent` (CoDMP.exe `0x40dc30`) hands a press to the UI (VM call
  3) and nothing else when the catcher has bit 2, unless
  `cl_bypassMouseInput` passes it on; Esc reaches the UI the same way
  (`0x40ddca`). ` and ~ go to the console first. A key repeat with the
  catcher at 2 returns early. VERIFIED (decompile). `CL_MouseEvent`
  (`0x40b0a0`) sends mouse motion to the UI with bit 2 set and
  `cl_bypassMouseInput` not 1, so the view does not turn. VERIFIED
  (decompile). `quickmessage`'s `onOpen` is what sets the bypass, and it
  never ran, so the stray catcher takes the next key press, mouse buttons
  and the wheel included, and holds the mouse until then. INFERRED (the
  combination).
- vcod: a failed `mp_QuickMessage` sets a stray catcher on the
  connection's menu state (`play::join::Join`). The next press (keyboard,
  mouse button or wheel; not the console key, not a repeat) is dropped
  and lets go of held keys (`clear_held_keys`: the movement and button
  state, and the scoreboard a held `+scores` keeps up), and mouse look is
  ignored until then. A script menu that opens first takes the catcher
  over.
- A new gamestate takes the catcher down. `CL_ParseGamestate`
  (`0x416050`) ends in `CL_InitDownloads` (`0x410240`), which reaches
  `CL_DownloadsComplete` (`0x40ffb0`) once the downloads are done. With
  `sv_running` 0 that runs `0x40ee30`, whose `CL_ShutdownUI` (`0x418e30`)
  clears bit 2 (`and 0xfffffffd`) and shuts the UI down, and
  `CL_StartHunkUsers` (`0x4115c0`) starts it again with no menu open.
  VERIFIED (decompile). With a local server the map load's
  `CL_MapLoading` (`0x40ee60`) sets the whole catcher word to 0 first.
  VERIFIED (decompile). vcod clears the stray catcher on every gamestate
  and closes the in-game menus with it, taking the mouse back for the
  view.

- Parser: `crates/common/src/voicechat.rs`, mirrors the grammar above
  (gender, categories, optional head icon, 64/64 caps, case-insensitive
  braces and category names). Loaded lazily from the game dir; a missing
  file warns once and leaves quick chat inert, exactly like retail.
- Table choice by speaker team: team 1 (Axis) -> axis file, everything
  else -> allies file, mirroring the `clientinfo[idx]+0x2c != 1` switch.
- Variant pick: random among the category's variants, like
  `rand() % count`.
- Cadence: at most one queued line per 1000 ms, like `FUN_3002d5d0`.
- Text: the raw category string is displayed, not localized - vcod has no
  `.str` localization engine, and retail's own fallback for a missing key
  is the raw key wrapped in `#UNLOCALIZED#`. Documented divergence.
- Head icon above the speaker: not implemented (needs renderer/entity
  work); the material name is parsed and kept.
- The suppression list (`kill_insult` etc. gated on `DAT_301ad3cc`) is not
  replicated; its enabling condition was not traced.
