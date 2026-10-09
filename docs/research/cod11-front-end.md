# The front end: main menu and server browser (CoD 1.1 MP)

What the retail main menu and server browser are made of, and the master
query behind the list. Sources: the stock menu files in the 1.1 paks
(`pak0.pk3`, `localized_english_pak0.pk3`), `ui_mp_x86.dll` 1.1 (image base
`0x40000000`), `CoDMP.exe` 1.1 (image base `0x00400000`; md5s in
`cod11-events-and-fx.md`), and live captures against
`codmaster.activision.com` and the retail 1.1d server on 2026-10-08.
Addresses are virtual; function names are mine, after the Quake III
functions whose shape they share. vcod's implementation is
`crates/client/src/frontend/`, `crates/common/src/ui_menu.rs` and
`crates/common/src/net/master.rs`.

## 1. Which files

- `ui_mp/menus.txt` lists the menu files the UI loads at startup, in order:
  `ui_mp/main.menu`, the options set (`ui_mp/options.menu`, eleven
  `ui/options_*.menu`, `ui_mp/options_multi.menu`), the restart popups,
  `ui/quit.menu`, `ui/error.menu`, `ui_mp/multi.menu`,
  `ui_mp/joinserver.menu`, `ui_mp/createserver.menu`, `ui_mp/cdkey.menu`,
  `ui_mp/mods.menu`, `ui_mp/connect.menu`, `ui_mp/password.menu`,
  `ui_mp/single_player.menu`, `ui_mp/serverinfo.menu`,
  `ui_mp/createfavorite.menu`, `ui_mp/filter.menu`, two more popups and the
  five `settings_<gametype>.menu`. VERIFIED (the file in `pak0.pk3`).
- The `ui/` files (`quit.menu`, `error.menu`, `options*.menu`) ship in
  `localized_english_pak0.pk3`, not `pak0.pk3`. `ui_mp/in_rec_restart.menu`,
  which `menus.txt` names, is in neither. VERIFIED (pak listings).
- `main.menu` opens with an `assetGlobalDef` naming the six fonts the UI
  registers (the font slots in `cod11-hud-protocol.md` section 8) and the
  cursor `ui/assets/3_cursor3`. VERIFIED.
- Two different `menudef.h` files exist: `ui/menudef.h` (`main.menu` and
  `ui/*.menu` include it) and `ui_mp/menudef.h` (`joinserver.menu` includes
  it). They disagree on flag values: `UI_SHOW_NOTFAVORITESERVERS` is `0x40`
  in the first and `0x1000` in the second; `UI_SHOW_FAVORITESERVERS` is `0x4`
  in both. VERIFIED. A parser has to resolve `#define`s per including file.
- `menu_background.menu` is not a menu: it is two `itemDef`s
  (`ui_mp/assets/main_back_top_mp.tga` over `0 0 640 320`,
  `main_back_bottom_mp.tga` over `0 320 640 160`) and a right-aligned
  `cvar "shortversion"` text at `630 472`, `#include`d inside both the
  `main` and `joinserver` menuDefs. VERIFIED.

## 2. The main menu (`ui_mp/main.menu`, menuDef `main`)

`fullScreen 1`, rect `0 0 640 480`, `focusColor UI_FOCUS_COLOR`
(`.98 .96 .39 1`). `onOpen` closes `mods_menu` and `options_multi`, plays
`music_mainmenu` and runs `uiScript stopRefresh`; `onESC` closes the same two
and runs `ingameclose main`. VERIFIED.

The buttons, all `ITEM_TYPE_BUTTON`, `textscale .4`, `textaligny 14`, at x
385 and 15 tall, in rect order (VERIFIED):

| y | text | shown when | action |
|---|---|---|---|
| 190 | `@MENU_BACKTOGAME` | `cl_ingame` 1 | `ingameclose main; close ...; close main` |
| 190 | `@MENU_JOIN_GAME` ("Join a Game") | `cl_ingame` 0 | `close mods_menu; close options_multi; close main; open joinserver` |
| 220 | `@MENU_DISCONNECT` | `cl_ingame` 1 | `exec "disconnect"` |
| 220 | `@MENU_START_NEW_SERVER` | `cl_ingame` 0 | `close main; ...; open createserver` |
| 250 | `@MENU_MULTIPLAYER_OPTIONS` | always | `close mods_menu; open options_multi` |
| 280 | `@MENU_OPTIONS` | always | `close main; ...; open options_menu; open options_look` |
| 310 | `@MENU_MODS` | always | `open mods_menu; close options_multi` |
| 340 | `@MENU_SINGLE_PLAYER` | always | `open single_popmenu` |
| 370 | `@MENU_QUIT` | always | `open quit_popmenu` |
| 430 | `@MENU_AUTO_UPDATE` | `cl_updateavailable` 1 | `open auconfirm` |

Every action starts with `play "mouse_click"` and every button's
`mouseEnter` plays `mouse_over`. Both aliases are in `iw_sound.csv` with
loadspec `menu`, as is `music_mainmenu` (`music/mainmenu1.mp3`, streamed,
looping). VERIFIED (`pak1.pk3`).

`quit_popmenu` (`ui/quit.menu`) is a popup at `204 160 235 135`; its Yes
runs `close main; close quit_popmenu; uiScript "quit"`, its No and Esc
`close quit_popmenu; open main`. `error_popmenu` (`ui/error.menu`, popup at
`158 80 320 320`) shows `cvar "com_errorMessage"` as an `autowrapped`
centred text at `textscale .25`, runs `uiScript clearError` on close and goes
back to `main` on Esc. VERIFIED.

## 3. Menu scripts

The script words the front-end files use: `play`, `open`, `close`, `show`,
`hide`, `setitemcolor`, `setcvar`, `exec`, `ingameclose`, `uiScript`.
VERIFIED (the files). `setcvar`, `exec`, `ingameclose`, `execKey` and the
`execOnCvar*Value` family are strings in `ui_mp_x86.dll` (`0x4002dbe0`,
`0x4002dbd8`, `0x4002dc5c`, `0x4002d280`, `0x4002db94`). VERIFIED.
INFERRED from the RTCW-MP lineage (`ui_shared.c`, `Item_RunScript`): a
script is a token stream where each command takes a fixed number of
arguments, `;` is optional, and a word the table lacks goes to the UI
module's `UI_RunMenuScript` with the rest of the stream. Stock files rely on
that: `joinserver`'s `onEsc` is `uiScript closeJoin` and `close joinserver`
with no `;` between them. VERIFIED (the file).

`uiScript` names the stock browser and main menu use, each a string in
`ui_mp_x86.dll` (VERIFIED): `RefreshServers` (`0x4002eb24`, compared at
`0x4000aa43`), `RefreshFilter` (`0x4002eb14`), `UpdateFilter`
(`0x4002ea40`), `ServerSort` (`0x4002e988`), `JoinServer` (`0x4002ea00`,
compared at `0x4000ad6c`), `closeJoin` (`0x4002ea70`), `addFavorite`
(`0x4002e8cc`), `quit` (`0x4002e9cc`), `StopRefresh` (`0x4002ea50`; the menu
spells it `stopRefresh`), `clearError` (`0x4002eb64`). `JoinServer` formats
`connect %s` (`0x4002e9f4`, pushed at `0x4000adc8` and `0x4000ae04`).
VERIFIED. An unknown name logs `unknown UI script %s` (`0x4002e7bc`, pushed
at `0x4000b4d7`). VERIFIED.

## 4. The browser (`ui_mp/joinserver.menu`)

- The list is an `ITEM_TYPE_LISTBOX` with `feeder FEEDER_SERVERS` (2) at
  `19 145 600 302` inside a menu at `1 0`, `elementheight 15`,
  `textscale .25`, `textaligny -1`, `outlinecolor .19 .3 .2 .45` and six
  columns at x offsets 2, 21, 286, 400, 464, 548 with character caps 20,
  40, 20, 10, 10, 20; `doubleClick { uiScript JoinServer }`. VERIFIED.
- The column headers are buttons whose actions run `uiScript ServerSort 0`
  .. `5`. The `Refresh List` button runs `uiScript RefreshServers`, `Quick
  Refresh` `RefreshFilter`, `Join Server` (at `510 453`) `close` four
  menus then `uiScript JoinServer`. VERIFIED.
- Owner draws: `UI_NETSOURCE` (220), `UI_SERVERREFRESHDATE` (247),
  `UI_JOINGAMETYPE` (253), `UI_NETMAPCINEMATIC` (246); the
  `UI_SERVERMOTD` item is commented out. VERIFIED. The localized strings the
  UI prints for them are in `exe.str`: `EXE_NETSOURCE` "Source:     %s",
  `EXE_INTERNET`, `EXE_ALL`, `EXE_WAITINGFORMASTERSERVERRESPONSE`,
  `EXE_GETTINGINFOFORSERVERS` "Getting info for %d servers (ESC to cancel)",
  `EXE_REFRESHTIME` "Refresh Time: %s". VERIFIED (the `.str` file and the key
  strings at `0x4002f058`, `0x4002f884`, `0x4002f86c`, `0x4002efb0`,
  `0x4002efd4`, `0x4002efa0`).
- The rate picker is an `ITEM_TYPE_MULTI` on `rate` with a
  `cvarFloatList` of 2500, 3000, 4000, 5000 and 25000. VERIFIED.

### Server list columns

The feeder's text function switches on the column through the jump table at
`0x4000cb58` (`jmp [esi*4+0x4000cb58]` at `0x4000c8ad`, six entries).
VERIFIED. Before the switch it reads the server's `ping` key and converts it
to an integer (`0x4000c88c`..`0x4000c89c`). VERIFIED. Per column, INFERRED
off the branches:

| col | text |
|---|---|
| 0 | `X` (`0x4002e44c`) when `pswrd` (`0x4002e764`) is non-zero, else nothing |
| 1 | `hostname` when the ping is above 0, else `addr` |
| 2 | `mapname` (`0x4002e7b4`) |
| 3 | `%s (%s)` (`0x4002e444`) over `clients` and `sv_maxclients` |
| 4 | `gametype` (`0x4002f9c0`), `?` when empty |
| 5 | the ping when above 0, else `...` (`0x4002e43c`) |

## 5. Master query

- `RefreshServers` with source 1 (Internet) runs `globalservers 0 %s full
  empty` (`debug_protocol` when set, `0x4000ebb9`) or `globalservers 0 %d
  full empty` (`0x4002e02c`, pushed at `0x4000ebe4`) with the `protocol`
  cvar read at `0x4000ebd2`; source 0 runs `localservers` (`0x4000eb5b`).
  VERIFIED for the strings and pushes; the source branch (`cmp eax,1` at
  `0x4000eb8b` on `0x401c146c`) is INFERRED.
- `CL_GlobalServers_f` (`0x413890`, registered as `globalservers`) prints
  its usage when given fewer than three arguments, resolves
  `codmaster.activision.com` (`0x566120`, pushed at `0x4138d6`) with port
  `0x501e` (20510, pushed at `0x4138e4`), and formats `getservers %s`
  (`0x566110`, pushed at `0x41392c`). VERIFIED. The master name is a
  constant: no cvar picks it. INFERRED from the push. The arguments after
  the master number go into the request, so the packet is
  `\xff\xff\xff\xffgetservers 1 full empty`. INFERRED from the format and the
  argument loop; the live master answers exactly that request (below).
- Live, 2026-10-08: `getservers 1 full empty` to `codmaster.activision.com`
  (`185.34.107.179:20510`) came back in one 378-byte packet:
  `\xff\xff\xff\xffgetserversResponse\n\0`, then 50 entries of `\`, four
  address bytes and a big-endian port, then `\EOF`. VERIFIED by capture.
- `CL_ServersResponsePacket` (`0x4107b0`) is called from the connectionless
  dispatch after an 18-character compare against `getserversResponse`
  (`0x566e34`, loaded at `0x410e9c`). VERIFIED. INFERRED, off its loop: it
  skips to each `\`, takes six bytes as address and port, requires a `\`
  after them, and stops at 256 entries, at `\EOT`, or with fewer than seven
  bytes left; the list is capped at `0x800` servers overall. The master's
  `\EOF` is caught by the length check, not by the `EOT` compare.
- Each server is then asked `getinfo xxx` (`0x5660e0`, pushed at `0x413da7`
  and `0x413fcb`). VERIFIED. The `infoResponse` handler reads `clients`,
  `hostname`, `sv_maxclients`, `gametype`, `game`, `nettype`, `minping`,
  `maxping`, `sv_allowAnonymous` and `pswrd` (the string refs at
  `0x412aa1`..`0x412b0d`). VERIFIED. A packet from a different protocol logs
  `Different protocol info packet: %s` (`0x566354`, pushed at `0x412c7a`).
  VERIFIED.
- Live `infoResponse` from public servers, 2026-10-08: `\challenge\xxx
  \protocol\1\hostname\...\mapname\...\clients\N\sv_maxclients\N\gametype\...
  \pure\0\sv_allowAnonymous\0\pswrd\0|1`, some with a `codextended\v20` pair
  after `pure`. Hostnames carry `^N` colour codes and raw bytes below 0x20
  and above 0x7f; one gametype was `^7dm`. VERIFIED by capture. `getstatus`
  returns the full serverinfo and player lines (the `serverinfo` popup's
  data). VERIFIED by capture.
- The retail 1.1d server on a local port answered vcod's `getinfo`, showed
  in the list among the master's 50, and `JoinServer` on it connected and
  loaded the map. VERIFIED 2026-10-08.

## 6. vcod's front end

- vcod draws the main menu, the browser, the options set (section 8), the
  quit popup and the error popup from the stock files with their layout,
  the main menu again over a game (section 7), and refuses every other menu
  with a console line. A button whose script would close its own menu and
  then open a refused one is refused whole, so `Start New Server` leaves the
  main menu up.
- `New Favorite` (`open createfavorite_popmenu`) opens the console with
  `connect ` typed, which is the LAN and favourite path.
- Ping pacing (32 `getinfo`s in flight, a 1.5 s timeout, 5 s for the master)
  is vcod's own; retail's numbers were not measured.
- Text is placed as RTCW's `Item_SetTextExtents` does: baseline at
  `rect.y + textaligny`, x at `textalignx` moved left by the width (right
  aligned) or half of it (centred); an owner draw with no label draws at
  `textalignx` whatever its alignment. INFERRED from the RTCW lineage, and
  the screenshots match the stock layout by eye.
- Not done: create server, mods, the CD key popup, password, server
  info, filters, favourites storage, LAN scanning (`localservers`), the
  scroll bar, the menu cursor image, keyboard focus on buttons,
  `focusColor`'s pulse and the map preview.

## 7. Esc in a game

### The client's key handler

`CL_KeyEvent` (`0x40dc30`) handles a press of Esc (`0x1b`) at `0x40ddca`
ahead of the binds. The key-catcher word is `0x155f2c4` (bit 1 console, 2
UI, 4 message field, 8 cgame), the connection state `0x155f2c0`, the
cgame VM `0x1617348`, the UI VM `0x161747c`; `0x460480` is the VM call with
the VM in `eax`. VERIFIED (the disassembly at `0x40ddca`..`0x40deb2`). The
branches, in order, each INFERRED off its test and jump:

1. Message field up (bit 4): `0x40d380` with the key, which drops the chat
   line. INFERRED.
2. cgame catcher (bit 8): the bit is cleared and the cgame VM is called with
   8, 0 (Q3's `CG_EVENT_HANDLING`, `CGAME_EVENT_NONE`). INFERRED.
3. UI catcher (bit 2): the UI VM gets 3, `0x1b`, down (`UI_KEY_EVENT`), so
   the open menu's `onEsc` runs. INFERRED.
4. State 6 (`CA_ACTIVE`, set at `0x404d00`): with a
   demo playing (`0x15ef004`) the UI gets 7, 1; otherwise 7, 2 when the cvar
   `cl_serverloadwaiting` (`0x566684`, registered at `0x412566`, held at
   `0x1617304`) is 0, else 7, 1. 7 is `UI_SET_ACTIVE_MENU`. INFERRED.
5. States 7 and 8 (cinematics): `0x40f5f0`, `0x44fa40(0)`, then 7, 1.
   INFERRED.
6. Any other state: 7, 1 when the UI VM is loaded. There is no disconnect
   on this path, unlike Q3's. INFERRED.

The console bit is not tested: with the console down in a game, Esc still
reaches step 3 or 4. The UI's `Key_SetCatcher` trap (UI syscall `0x31`,
jump table `0x418c68` entry `0x418730`) calls `0x4180a0`, which ORs the
console bit back in (VERIFIED). So the menu opens under the console, the
console stays down and keeps the keys (bit 1 is tested before bit 2 for
other keys at `0x40dfd2`). INFERRED.

`cl_ingame` is set to `1` at `0x411220` when the state is 6 and to `0`
otherwise (VERIFIED, the two `Cvar_Set` calls). The UI's `ingameclose` and
`ingameopen` (`0x40010930`, `0x40010900` in `ui_mp_x86.dll`) act only when
the display context's call at `+0xa8` returns non-zero (VERIFIED); that is
syscall `0x65` (`0x40019830`), which returns `cls.state == 6` (`0x418c1b`
in `CoDMP.exe`). VERIFIED.

### `UI_SetActiveMenu` (`ui_mp_x86.dll` `0x4000d810`)

`vmMain` (`0x400076a0`) sends command 7 there (jump table `0x400077a8`,
entry 7 is `0x4000770f`). It switches through the table at `0x4000dbcc`
(VERIFIED). What each case does, INFERRED off its calls and pushes:

| id | case | does |
|---|---|---|
| 0 | `0x4000d861` | clears the UI catcher bit, sets `cl_paused` `0`, closes every menu |
| 1 | `0x4000d8a9` | sets the UI catcher, opens `main`, then `error_popmenu` when `com_errorMessage` is not empty |
| 2 | `0x4000d9c3` | sets the UI catcher, closes every menu (`0x40010660`), opens the menu the cvar `g_scriptMainMenu` (`0x4002e2dc`) names |
| 3, 4 | `0x4000d95d`, `0x4000d990` | `needcd`, `badcd` |
| 5 | `0x4000d92a` | `team` |
| 8 | `0x4000da11` | `quickmessage` |
| 9 | `0x4000da62` | `autoupdate` |
| 10, 11 | `0x4000da8b` | the server's script menu (`ui_newScriptMenu`) |

So Esc in a live game does not open `main`: it opens the gametype's script
menu, which the stock scripts keep in `g_scriptMainMenu` through
`setClientCvar` (`tdm.gsc` sets the team menu on connect and a weapon menu
once a team is picked). Case 2 does not write `cl_paused` (`0x4002e9b4`);
case 0 sets it to `0` (VERIFIED, the push at `0x4000d878`). Other code in
the module sets it to `1` (pushes at `0x4000ae43`, `0x4000d1ad`); what pauses
a client connected to a remote server, if anything, is not measured. The
server's game keeps running either way. An empty or
unknown `g_scriptMainMenu` opens nothing: `0x400134b0` looks the name up
(`0x40010560`) and returns when it is missing. VERIFIED.

### The menus it reaches

- `ui_mp/ingame.txt` lists `ui_mp/ingame.menu` and five more `ingame_*`
  files; none of them is in any 1.1 pak. VERIFIED (pak listings).
  `menus.txt` is the list the UI loads.
- Every stock team and weapon script menu carries a `button_mainmenu` tab,
  `@MPMENU_MAIN_MENU`, whose action is `play "mouse_click"; close <self>;
  open main`. Their `onEsc` is `scriptMenuResponse "close"; close <self>`.
  `callvote.menu` has an `open main` too; `viewmap` and the quick-chat menus
  do not. VERIFIED (the files in `pak0.pk3`).
- `main` with `cl_ingame` 1 shows Back to Game (`ingameclose main` and
  closes) and Disconnect (`exec "disconnect"`) where Join a Game and Start
  New Server stand; Multiplayer Options, Options, Mods, Single Player and
  Quit stay. Its `onEsc` is `ingameclose main`, so Esc closes it in a game
  and does nothing with no game up. VERIFIED (the file, section 2).
- The `menu` loadspec rows (`mouse_click`, `mouse_over`, `music_mainmenu`)
  are left out of a map's alias set (`cod11-sound-system.md` section 1d),
  so in a game the menus' `play` commands find no alias. INFERRED; whether
  retail keeps the menu set loaded beside the map's is not measured.

### vcod

- Esc in a live game with no script menu open runs `Join::open_main`
  (`g_scriptMainMenu`), drawn as vcod's keyboard list, whose "Main Menu"
  row opens `main` with `cl_ingame` 1. When the server named no script menu
  vcod opens the in-game `main` directly; retail opens nothing there.
- Esc while connecting or loading opens `main` with `cl_ingame` 0, as menu
  1 does.
- Esc with the console down in a game opens the same menus under it and
  leaves the console down. With no game up vcod still closes the console on
  Esc; retail reopens `main` under it.
- The mouse is released while a menu is up and captured again when Esc or
  Back to Game closes the last one. Usercmds keep going out with no keys
  held, and no bind fires while the menu has the keys.

## 8. The options screens

### Files and layout

- `menus.txt` loads `ui_mp/options.menu` (not `ui/options.menu`, which
  differs only in 130-wide button rects), eleven `ui/options_*.menu`,
  `ui_mp/options_multi.menu`, `ui_mp/vid_restart.menu` and
  `ui/snd_restart.menu`. Two of the eleven, `ui/options_view.menu` and
  `ui/options_defaults.menu`, are in no pak. VERIFIED (pak listings). vcod
  loads the files that exist except `options_credits` and
  `options_driverinfo`.
- `options_menu` is full screen with the main backdrop; its right-hand
  buttons (Controls: Look, Move, Shoot, Interact, Set Default Controls;
  System: Graphics, Sound, Performance, Optimal System Settings; Back) each
  close every page and open one. `Interact` and `Save & Interact` share a
  rect and split on `ui_multiplayer` 1 / 0; `Driver Info` shows only with
  `developer` 1. The main menu's Options runs `open options_menu; open
  options_look`; Multiplayer Options opens `options_multi` over the main
  menu without closing it. VERIFIED (the files).
- Every page is a 360 x 325 window at `OPTIONS_WINDOW_POS` 5 75 (from
  `ui/menudef.h`; `ui_mp/menudef.h` has the same position with
  `OPTIONS_CONTROL_SIZE` 350 12 instead of 350 13). Controls are right
  aligned at `textalignx` 165. VERIFIED.
- The two menus are both open and both take clicks. INFERRED from RTCW's
  `Menus_HandleOOBClick`: a click outside the focused menu goes to the open
  menu whose item it lands on. vcod hit-tests the open menus top down and
  stops below a popup.

### Item types

`ui/menudef.h` numbers them: editfield 4, ownerdraw 8, slider 10, yes/no
11, multi 12, bind 13. VERIFIED. The stock options files use 49 binds, 24
multis, 16 yes/nos, 6 sliders and one edit field. VERIFIED (counted).

- Bind (`ui_mp_x86.dll`): the commands a bind item can change are the
  50-record `g_bindings` table at `0x40036130` (24 bytes each: command,
  two default keys, the two current keys, a spare), `+scores` through
  `screenshotJPEG`; vcod's `options::BIND_COMMANDS` copies it. VERIFIED.
  `BindingFromName` (`0x40014cf0`) answers `KEY_UNBOUND` for a command not
  in the table or with no key, else the first key's name, translated, and
  for a second key `" %s "` (`0x4002d6d8`) around the translated `KEY_OR`
  and the second name: "W or Up Arrow". VERIFIED. Key names come from
  `key.str` (`KEY_SPACE` "Space", `KEY_MOUSE1` "Left Mouse"). VERIFIED (the
  file). Which two keys show when more are bound follows the key-number
  order of the scan that fills the table. INFERRED from Q3's
  `Controls_GetKeyAssignment`.
- `Item_Bind_HandleKey` (`0x40015410`), VERIFIED off the compares and
  stores: Enter (`0x0d`) or `K_MOUSE1` (`0xc8`) over the item starts the
  wait. While waiting, a character event is ignored, Escape (`0x1b`) ends
  the wait, `` ` `` (`0x60`) is ignored, Backspace (`0x7f`) unbinds both
  keys. Any other key is first removed from every table entry (a second
  slot cleared, a first slot taking the second's key), then becomes the
  command's first key if it has none, its second if the first differs and
  the second is free, and otherwise both old keys are unbound (`setBinding`
  with `""`, `0x4002dfb9`) and it becomes the only key. `0x40014c30`
  (`Controls_SetConfig`) writes the table back and the wait ends.
- The status owner draw `UI_KEYBINDSTATUS` (250) prints `EXE_KEYWAIT`
  ("Waiting for new key. Press ESCAPE to cancel or BACKSPACE to clear")
  while waiting and `EXE_KEYCHANGE` ("Press ENTER or CLICK to change")
  otherwise (`0x40008e21`..`0x40008e43`). VERIFIED. The pages show it only
  while a bind is hovered (`show keyBindStatus` / `hide`). VERIFIED (the
  files).
- Slider: `cvarFloat "name" default min max`. `Item_Slider_Paint`
  (`0x400150a0`) draws `ui/assets/slider2.tga` 96 x 16 at the item's top,
  8 past the label's end (the rect's left with no label), and
  `ui/assets/sliderbutt_1.tga` 10 x 20, 2 above it, centred on
  `x + 6 + 84 * (clamp(value) - min) / (max - min)` (`0x40011740`, the
  constants 6.0 at `0x4002fec4`, 84.0 at `0x4002fec8`). VERIFIED.
  `Item_Slider_HandleKey` (`0x40012f40`) on a click sets
  `(cursor - x) / 96 * (max - min) + min` formatted `%f` (`0x4002db50`)
  without a clamp; the drag function (`0x40012d30`) clamps the cursor to
  the bar first. VERIFIED. The hit rect the click needs is INFERRED from Q3
  (half a thumb left of the bar to its right end).
- Yes/no: the value is `EXE_YES` / `EXE_NO` (strings at `0x4002db30`,
  `0x4002db28`). VERIFIED. A click sets `!value`. INFERRED from Q3.
- Multi: `cvarFloatList` or `cvarStrList` (comma-separated pairs in the
  stock files). The value shown is the matching entry's label, nothing when
  none matches, and a click moves to the next entry, the second when none
  matched. INFERRED from Q3's `Item_Multi_Setting` and
  `Item_Multi_FindCvarByValue`, which answers 0 for no match.
- Edit field: `maxChars` 32, `maxPaintChars` 18 on the name. The cvar is
  written on every key. INFERRED from Q3. vcod keeps its cursor at the end
  and inserts; Q3 starts an edit at position 0 in overstrike mode, which
  was not checked against CoD.

### Scripts and cvars

- `uiScript` names the options files call, each a string in
  `ui_mp_x86.dll`: `loadControls` (`0x4002eb70`), `update` (`0x4002e844`),
  `getLanguage` (`0x4002e7f4`), `verifyLanguage` (`0x4002e7e4`). VERIFIED.
  `update ui_mousePitch` (`0x4000a377`) sets `m_pitch` to 0.022
  (`0x3cb43958`) when the toggle is 0 and -0.022 otherwise. At load the UI
  sets `ui_mousePitch` to "1" when `m_pitch` is below 0.0 and "0"
  otherwise (`0x4000d66c`..`0x4000d697`). VERIFIED. vcod derives
  `ui_mousePitch` from `m_pitch` the same way on every read.
- `execOnCvarIntValue` / `execOnCvarFloatValue` (strings in the dll, see
  section 3) take a cvar, a value and a command; `options_performance`
  uses them to map `r_lodscale` to `ui_lod` on open and back to
  `r_lodscale` / `r_lodbias` on close. VERIFIED (the file). Their
  semantics are INFERRED from the names and that use.
- `setfromcvar` is a CoDMP.exe command (registered at `0x43a670` with the
  handler `0x43a070`): `usage: setfromcvar <variable> <variablein>`
  (`0x56216c`) with fewer than three arguments, else it sets the first
  cvar to the second's string, `""` when the second does not exist.
  VERIFIED. The pages copy live cvars into `ui_*` shadows on open
  (`ui_name` from `name`, `ui_r_mode` from `r_mode`) and back on close or
  on the restart popups' Yes. VERIFIED (the files).
- `vid_restart` and `snd_restart` are CoDMP.exe commands (`0x40fbe0`,
  `0x40fdf0`) and `exec` prints `execing %s` / `couldn't exec %s`.
  VERIFIED (registration and strings). Reset Controls runs
  `exec default_mp.cfg`, which ships in `localized_english_pak0.pk3` and
  opens with `unbindall`. VERIFIED.
- Defaults, VERIFIED at their registrations: CoDMP.exe `mss_volume` "0.8"
  flags 1, `mss_khz` "44" flags 0x21, `mss_3d_provider` "Miles Fast 2D
  Positional Audio", `r_mode` "3" and `r_fullscreen` "1" flags 0x21,
  `r_picmip` "1", `r_picmip2` "2", `r_texturebits` "0", `r_textureMode`
  "GL_LINEAR_MIPMAP_NEAREST", `r_gamma` "1.0", `r_ignorehwgamma` "0",
  `r_lodscale` "1", `r_lodbias` "0", `r_dynamiclight` "1",
  `r_dlightQuality` "1", `r_swapInterval` "0", `r_nv_fog_dist` "1",
  `cl_freelook` "1", `m_filter` "0", `cl_languagesavailable` "0" (set to
  the language count later); cgame's table: `cg_drawCrosshair`,
  `cg_drawStatus`, `cg_marks`, `cg_brass`, `cg_blood` all "1" flags 1
  (rows at `0x30074aa0`, `0x30074a40`, `0x30074c80`, `0x30074c70`,
  `0x30075430`).

### What vcod backs

- Live: every bind, `sensitivity`, invert mouse (`m_pitch`), `name` (on
  closing Multiplayer Options), `rate`, `mss_volume` (the master volume,
  read every frame), and `r_mode` / `r_fullscreen`, which `vid_restart`
  and start-up apply to the window (Q3's mode table; borderless full
  screen). vcod registers `r_mode` -1 (its own window size) and
  `r_fullscreen` 0 instead of retail's 3 and 1.
- Stored but inert (archived, so a choice survives): `cl_freelook`,
  `m_filter`, `cg_drawCrosshair`, `cg_drawStatus`, the texture, picmip,
  gamma, LOD, dynamic light, swap interval and NVIDIA fog cvars,
  `mss_khz`, `mss_3d_provider`, `cg_marks`, `cg_brass`, `cg_blood`.
  `snd_restart` and `setRecommended` are unknown commands. Binds to
  commands vcod lacks (`+lookup`, `+strafe`, `mp_QuickMessage`,
  `screenshotJPEG`, the stance toggles) are stored and print `Unknown
  command` when pressed.
- Not shown: the language picker (`cl_languagesavailable` reads 1, so its
  `hideCvar` hides it), NVIDIA fog (no `r_nv_fog_available`), Driver Info
  (no `developer`). The CD key button is refused.
