# Quick chat (`vsay`, the `j`/`k`/`l` server commands)

How CoD 1.1 MP carries quick chat end to end, and why the `j`/`k`/`l` path is dead on stock installs. The stock quick message menu (`V`) is a separate, script-driven path; see "The quick message menu".

Sources, addresses virtual:

- `game.mp.i386.so` (1.1d Linux dedicated, full symbols), server side.
- `game_mp_x86.dll` 1.1, the same code (`cod11-sound-system.md`).
- `cgame_mp_x86.dll` 1.1, image base `0x30000000`; md5s in `cod11-events-and-fx.md`.

Default label for the first four sections: VERIFIED, read from the binaries; exceptions are labelled. No stock server has been seen sending `j`/`k`/`l` on the wire. The live captures in `cod11-sound-system.md` came from modded servers that bypass this path with `EV_SOUND_ALIAS`.

## Server side

`G_Voice` (`game.mp.i386.so`, `G_Voice@@Base` at `0x473b8`; `game_mp_x86.dll` at the same VA):

- Spam gate first: a per-entity accumulator at `ent+0x308` compared against level time (`0x1e8`), capped near 30 s (`cmp $0x752f`). Past it, the `GAME_SPAMPROTECT` notice (string at `0x73a86`) goes out instead.
- The caller's mode argument (`0x10(%ebp)`) picks the letter: `1` gives `"k"` (`0x73a48`), `2` gives `"l"` (`0x73a4a`), anything else `"j"` (`0x73a4c`).
- It formats `"%s %d %d %d %s %i %i %i"` (`0x73a4e`) and sends it as a reliable server command. Push order at `0x474db..0x4752f`: the letter, three ints from entity fields, the category string argument, then the speaker origin xyz, each loaded as float and converted with `fistpl` (`flds 0x20/0x1c/0x18(%ebx)`).

One command:

```
j <int> <int> <int> <category> <origin_x> <origin_y> <origin_z>
```

Every numeric field prints as a decimal integer, which the client reads back with `atol`. The client takes argv 1 as the scope echo (it picks the chat-line format and the team-only gate), argv 3 as the speaking client index and argv 5-7 as the speaker origin. argv 2's meaning is unknown.

## Client dispatch and handling

All addresses are `cgame_mp_x86.dll` 1.1.

- Dispatch: console-string cases `'j'` `'k'` `'l'` (`0x6a/6b/6c`) call `FUN_3002d940(0/1/2)` (call sites `0x30026437..0x30026443` in the export).
- `FUN_3002d940` (`0x3002d940`): reads argv 1-3 via trap_Argv (syscall `0xd`) through `atol` (`0x3004afde`), keeps argv 4 as the category string, converts argv 5-7 back to floats (`fild`/`fstp` pairs). When global `DAT_301ad3cc` is set it drops the auto-taunt categories early: `kill_insult` (`0x30062a54`), `taunt` (`0x30062a4c`), `death_insult`, `kill_gauntlet`, `praise` (the Q3 voice-chat category names). What sets `DAT_301ad3cc` is not traced.
- `FUN_3002d770` (`0x3002d770`, `__thiscall`; ECX points at a vec3 the caller built on its stack from argv 5-7, so the origin comes from the command, not entity state):
  - clamps argv 3 to `0..63` as the speaking client index;
  - bails if `clientinfo[idx].field_0x00 == 0` (clientinfo stride `0x448` from `0x3018bc0c`);
  - picks the voice table: `EBX = 0x302f7d80` (axis) by default, `0x30340ec8` (allies) when `clientinfo[idx].field_0x2c != 1`. Team `1` is axis, as on the HUD (1 axis, 2 allies);
  - a mode check passes when the mode argument is `1` or global `DAT_3029824c == 0`.
- Selection, `FUN_3002d390` (`0x3002d390`; EBX the table base; stack args the category string and three out-pointers): scans up to `count = *(table+0x44)` categories, records from `table+0x48` at stride `0x1244`. The match test `FUN_3003e8f0(ECX = record)` returns 0 on a hit. On the first hit it draws `rand()` modulo the variant count at `record+0x40` and returns:

| Out | Address | What |
|---|---|---|
| A | `record+0x44 + j*4` | the registered sound handle |
| B | `record+0x1144 + j*4` | the head-icon material handle |
| C | `record+0x144 + j*0x40` | a pointer into an inline per-variant area the 1.1 parser never writes |

- Queueing: the entry `{pos.xyz, category copy char[149], client index, handles...}` goes into a 32-slot ring at `DAT_305407c0`, indexed by `DAT_3020d094`. The drain, `FUN_3002d5d0` (`0x3002d5d0`), takes at most one entry per 1000 ms (`DAT_3020d08c = time + 1000`).
- Presentation, `FUN_3002d470` (`0x3002d470`): copies the head-icon handle into the speaker's clientinfo (drawn over the head elsewhere) and prints `": <text>"` to the console. The localizer `FUN_3002d670` (`0x3002d670`, syscall `0x38` lookup) falls back to the raw key wrapped in `#UNLOCALIZED# ... #`. The chat-line formats in `FUN_3002d770`:

| Mode | Format |
|---|---|
| global | `"%s %s(%s): %c%c%s"` |
| 1 | `"(%s)%s(%s): %c%c%s"` |
| 2 | `"[%s]%s[%s]: %c%c%s"` |

## The `.voice` file format

`FUN_3002cbd0` (`0x3002cbd0`) parses it, called once at init from `FUN_30020a00` (`0x30020a00`):

```
FUN_3002cbd0("mp/axis_chat.voice",   &DAT_302f7d80, 0x40);
FUN_3002cbd0("mp/allies_chat.voice", &DAT_30340ec8, 0x40);
```

VERIFIED against the decompilation and asm:

- Read through the fs traps (`0xf` open/size, `0x10` read, `0x12` free). A file over `0x4000` bytes is rejected ("voice chat file too large").
- Tokens come from the shared Q3-style tokenizer `FUN_3003da70` (`GetToken`). Category and brace compares are case-insensitive.
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

- The head-icon slot is a same-line read. Every other token comes from `GetToken(allowLineBreaks = 1)`; the slot after a sound alias uses `GetToken(0)` (the second call site in `FUN_3002cbd0`). An entry is `alias` or `alias headicon` on one line. Two aliases on one line make the second the first's head icon. Category and brace tokens flow across lines.
- Each `soundAlias` is registered with syscall `0xbf` into array A. INFERRED: `0xbf` is `S_RegisterSound`, since `FUN_30020a00` registers known aliases such as `mp_announce_g_twominutes` through it right beside.
- The optional second token becomes array B via `FUN_30030b70(name, 2)` (INFERRED: register shader/material). Missing or empty, it defaults to `headiconVoiceChat`. A `}` in that slot is pushed back (`UngetToken`) and the default icon applies.
- Per-category record (stride `0x1244`):

| Offset | Content |
|---|---|
| `0x00..0x40` | match data |
| `+0x40` | entry count u32, incremented in place while parsing |
| `+0x44` | array A, up to 64 dwords |
| `+0x144..0x1143` | inline structs |
| `+0x1144` | array B, up to 64 dwords |

`0x44 + 64*4 = 0x144` and `0x1144 + 64*4 = 0x1244` exactly.

The aliases these files name are the `american_*` / `british_*` / `russian_*` / `german_*` rows of `dialog_mp.csv` (`cod11-sound-system.md`, quick chat).

## Negative result: stock installs ship no `.voice` data

VERIFIED (a zipfile walk over every pak plus a loose-file sweep): no stock `.pk3` of 1.1 `main/`, 1.5 `main/` or UO `uo/`, the `localized_english_pak*.pk3` included, holds a `*.voice` file. INFERRED, off the code above: a stock client logs "^1voice chat file not found: mp/axis_chat.voice" twice from `FUN_30020a00`, both tables stay empty, `FUN_3002d390` always returns 0 and `FUN_3002d770` returns without queueing. Stock quick chat prints nothing, plays nothing and shows no head icon, whatever `j`/`k`/`l` a server sends, unless a mod ships `.voice` files.

vcod keeps this inert path for parity. Building `.voice` files from `dialog_mp.csv` would be new game data, out of scope.

VERIFIED live: a sweep of three populated public servers (TDM, S&D and one more, 75-90 s each) saw no `j`/`k`/`l` command. All three ran mods (`g_scriptMainMenu` set to `team_*`; `i`/`f`/`v`/`t`/`n`/`h`/`s` commands; `MPSCRIPT_*`/`scr_fm_*` text) that relay voice lines through `s <idx>` and events. The stock `j`/`k`/`l` path is unobserved live.

## The quick message menu (`mp_QuickMessage`)

The stock path that does reach players: the `V` menu, whose rows make the gametype script play an alias and send a team chat line. It never uses `j`/`k`/`l`.

- VERIFIED (the table bytes): `mp_QuickMessage` is a cgame console command; the table row at `cgame_mp_x86.dll` `0x30061078` pairs the string at `0x30061904` with `0x300140e0`. VERIFIED (string sweep): CoDMP.exe has no such string, so the engine never forwards it to the server. VERIFIED: `default_mp.cfg` (`localized_english_pak0.pk3`) has `bind v "mp_QuickMessage"`.
- VERIFIED (disassembly, `0x300140e0..0x300140fe`): `0x300140e0` reads `cg.snap` (`0x301e2160`), tests `pm_flags` at `+0x18` (the playerstate starts at `+0xc`) against `0x40000`, and only with that bit set calls cgame syscall `0x7a` with `UIMENU_WM_QUICKMESSAGE` (`0x300655c0`). INFERRED, from that bit's meaning (`cod11-player-clip.md` 6.2): the menu opens only on the client's own view, playing or dead, never as a spectator or a follower.
- VERIFIED (disassembly, `0x4034f9..0x403562`): CoDMP.exe's cgame syscall switch (`0x401e16`, jump table `0x4041f0`, entry `0x7a` at `0x4034f9`), with `cls.state` (`0x155f2c0`) 6, no demo playing (`0x15ef004`) and a non-null name, sends UI VM command 7 with 8 on a match of `UIMENU_WM_QUICKMESSAGE` (`0x568b08`), and 7, 9 on `UIMENU_WM_AUTOUPDATE` (`0x568af0`). The compares after, against `UIMENU_SCRIPT_POPUP` and `UIMENU_SCRIPT_POPUP_NO_MOUSE`, are not read.
- VERIFIED (disassembly): `UI_SetActiveMenu` case 8 (`ui_mp_x86.dll` `0x4000da11`) puts the UI cursor at 639, 479, sets the UI key catcher (`0x400194d0` with 2), runs `Menus_CloseAll` (`0x40010660`) and opens `quickmessage` (`0x4002e2cc`, through `0x400134b0`). INFERRED: the function names, from `cod11-front-end.md` section 1.
- VERIFIED (the file): the menu is `ui_mp/wm_quickmessage.menu` (`pak0.pk3`), loaded through `ui_mp/ingame.txt`. `fullscreen 0`, no `onEsc`; `onOpen` sets `cl_bypassMouseInput` 1 and `onClose` 0, so the mouse keeps turning the view while keys go to the menu. Its four text rows are decoration; each is followed by `execKey "1"` .. `"3"`, running `close quickmessage; open quickcommands` (`quickstatements`, `quickresponses`).
- VERIFIED (the files): `quickcommands.menu`, `quickstatements.menu` (keys 1-8) and `quickresponses.menu` (keys 1-7) in `ui_mp/scriptmenus/` share that shape: decoration rows, each followed by `execKey "<n>" { scriptMenuResponse "<n>"; close <self> }`. The stock gametypes `precacheMenu` all three (`tdm.gsc:167-169`), so they sit in the script-menu configstrings.
- VERIFIED (disassembly): `scriptMenuResponse` (`ui_mp_x86.dll` `0x40011140`) takes the first of configstrings `0x49c + i`, `i` 0..31, matching the menu's name (`0x40005e00`), -1 when none does, and executes `cmd mr %i %i %s` (`0x4002dce4`) with `sv_serverid`, the index and the response.
- VERIFIED (the scripts): `tdm.gsc:449-454` routes the three menus to `_teams.gsc` `quickcommands` / `quickstatements` / `quickresponses`. These pick a nationality's alias and a `&"QUICKMESSAGE_*"` string, refuse a spectator or a player inside the 2 s `spamdelay`, and call `doQuickMessage` (`_teams.gsc:1629`): a playing player gets the `headicon@quickmessage` head icon, `playSound(alias)`, then `sayTeam` and `pingPlayer` (or `sayAll` with `level.QuickMessageToAll`).
- INFERRED, from Team Arena's `_UI_KeyEvent`, which runs `Menus_CloseAll` on Esc when no fullscreen menu is visible: Esc closes the quick menus without a response, as they have no `onEsc`.

## vcod behaviour and deliberate divergences

### The menu

- `mp_QuickMessage` (bound to `V`, as `default_mp.cfg` does) opens `quickmessage` when the game is live and the newest snapshot's `pm_flags` has `0x40000` (`Ui::open_quick_message`, case 8). The menu is the front end's own copy from `ui_mp/ingame.txt`, `cl_language`'s directory first, drawn from the file at `32 256`. Its `execKey`s open the quick-chat script menus the cgame loaded, whose `execKey`s send `mr` with the index looked up by name.
- Only those digits and Esc reach the menus; every other key and the mouse reach the binds, as `cl_bypassMouseInput` lets them (`cod11-front-end.md` section 13, "`cl_bypassMouseInput` and the keys"). Retail draws the cursor parked at `639 479`; vcod draws none.

### `mp_QuickMessage` after a `ui_load`

After a `ui_load` retail's `ingame.txt` menus are gone, so `Menus_ActivateByName` finds no `quickmessage` (0x400134b0 returns on a missing name, `cod11-front-end.md` section 13) and `mp_QuickMessage` only closes the open menus. Case 8 still sets the UI key catcher with no menu up. Retail then:

- VERIFIED (asm): `_UI_KeyEvent` (0x4000d6f0, `vmMain` case 3), when `Menu_GetFocused` (0x40016970) finds nothing, reads the catcher (syscall `0x30`), clears bit 2 (`and eax, 0xfffffffd` at `0x4000d757`), writes it back (syscall `0x31`), runs syscall `0x2f` unless `0x401bf428` is set (set only when a focused menu saw `cl_bypassMouseInput` 1), and sets `cl_paused` 0. It does nothing else with the key.
- VERIFIED (asm): `CL_UISystemCalls` (CoDMP.exe `0x418300`, jump table `0x418c68`) maps `0x30` to a read of the catcher word `0x155f2c4`, `0x31` to `Key_SetCatcher` (`0x4180a0`, which keeps the console bit) and `0x2f` to `Key_ClearStates` (`0x40e260`).
- VERIFIED (asm): `Key_ClearStates` zeroes `anykeydown` (`0x142f75c`) and, for each of the 256 keys (12 bytes each from `0x142f780`) still down, calls `CL_KeyEvent` with down 0 and time 0, then clears its down flag and repeat count.
- INFERRED (control flow): with bit 2 already off, each such release runs the key's `-` command when its bind starts with `+` (`0x40defc`). The stray key is down by then, so its own release goes out too.
- VERIFIED (decompile): `CL_KeyEvent` (CoDMP.exe `0x40dc30`) hands a press to the UI (VM call 3) and nothing else when the catcher has bit 2, unless `cl_bypassMouseInput` passes it on. Esc reaches the UI the same way (`0x40ddca`). ` and ~ go to the console first. A key repeat with the catcher at 2 returns early.
- VERIFIED (decompile): `CL_MouseEvent` (`0x40b0a0`) sends mouse motion to the UI with bit 2 set and `cl_bypassMouseInput` not 1, so the view does not turn.
- INFERRED (the combination): `quickmessage`'s `onOpen` sets the bypass and never ran, so the stray catcher takes the next press, mouse buttons and the wheel included, and holds the mouse until then.
- VERIFIED (decompile): a new gamestate takes the catcher down. `CL_ParseGamestate` (`0x416050`) ends in `CL_InitDownloads` (`0x410240`), which reaches `CL_DownloadsComplete` (`0x40ffb0`) once downloads are done. With `sv_running` 0 that runs `0x40ee30`, whose `CL_ShutdownUI` (`0x418e30`) clears bit 2 (`and 0xfffffffd`) and shuts the UI down; `CL_StartHunkUsers` (`0x4115c0`) restarts it with no menu open. With a local server the map load's `CL_MapLoading` (`0x40ee60`) zeroes the whole catcher word first.

vcod asks the UI whether `quickmessage` is loaded. When it is not, vcod closes every menu through its `onClose`, opens nothing and sets a stray catcher on the connection's menu state (`play::join::Join`). A mod whose own `menus.txt` lists a `quickmessage` keeps it, as on retail. The next press (keyboard, mouse button or wheel; not the console key, not a repeat) is dropped and lets go of held keys (`clear_held_keys`: the movement and button state, and the scoreboard a held `+scores` keeps up); mouse look is ignored until then. A script menu that opens first takes the catcher over. Every gamestate clears the stray catcher and closes the in-game menus, giving the mouse back to the view.

### `j`/`k`/`l` and `.voice`

- Parser: `crates/common/src/voicechat.rs`, the grammar above (gender, categories, optional head icon, 64/64 caps, case-insensitive braces and category names). One divergence: retail aborts on a gender token that fails all three checks; vcod accepts any as `Gender::Other`.
- `crates/client/src/quick_chat.rs` loads the tables lazily from the game dir (reloaded after a download reopens the search path). A missing file leaves quick chat inert, as on retail. A file over `0x4000` bytes is refused.
- Table by speaker team: team 1 (axis) the axis file, anything else the allies file (the `clientinfo[idx]+0x2c != 1` switch).
- Variant: random among the category's variants (`rand() % count`).
- Queue: 32 entries, at most one line drained per 1000 ms (`FUN_3002d5d0`).
- Text: the raw category string, not localized. Retail's own fallback for a missing key is the raw key wrapped in `#UNLOCALIZED#`.
- Head icon above the speaker: not drawn. The material name is parsed and kept.
- The suppression list (`kill_insult` and the rest, gated on `DAT_301ad3cc`) is not replicated; its enabling condition is not traced.
