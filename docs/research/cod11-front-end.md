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

- vcod draws the main menu, the browser, the quit popup and the error popup
  from the stock files with their layout, and refuses every other menu with
  a console line. A button whose script would close its own menu and then
  open a refused one is refused whole, so `Options` leaves the main menu up.
- `New Favorite` (`open createfavorite_popmenu`) opens the console with
  `connect ` typed, which is the LAN and favourite path.
- Ping pacing (32 `getinfo`s in flight, a 1.5 s timeout, 5 s for the master)
  is vcod's own; retail's numbers were not measured.
- Text is placed as RTCW's `Item_SetTextExtents` does: baseline at
  `rect.y + textaligny`, x at `textalignx` moved left by the width (right
  aligned) or half of it (centred); an owner draw with no label draws at
  `textalignx` whatever its alignment. INFERRED from the RTCW lineage, and
  the screenshots match the stock layout by eye.
- Not done: the options screens, create server, mods, password, server
  info, filters, favourites storage, LAN scanning (`localservers`), the
  in-game Esc menu (`cl_ingame` 1), the scroll bar, the menu cursor image,
  keyboard focus on buttons, `focusColor`'s pulse and the map preview.
