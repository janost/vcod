# The front end: main menu and server browser (CoD 1.1 MP)

What the retail main menu and server browser are made of, and the master
query behind the list. Sources: the stock menu files in the 1.1 paks
(`pak0.pk3`, `localized_english_pak0.pk3`), `ui_mp_x86.dll` 1.1 (image base
`0x40000000`), `CoDMP.exe` 1.1 (image base `0x00400000`; md5s in
`cod11-events-and-fx.md`), and live captures against
`codmaster.activision.com` and the retail 1.1d server on 2026-10-08 and
2026-10-09.
Addresses are virtual; unexported function names are this doc's, after the Quake III
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
- `UI_LoadMenus` (`ui_mp_x86.dll` `0x400085b0`) reads the list named by
  `ui_menuFiles` (vmCvar row at `0x40036c9c`: default `ui_mp/menus.txt`,
  flags 0). A missing list prints `^3menu file not found: %s, using
  default` and falls back to `ui_mp/menus.txt`; that one missing too is
  fatal (`^1default menu file not found: ui_mp/menus.txt, unable to
  continue!`). VERIFIED (strings `0x4002f1a8`, `0x4002f160`, their xrefs
  at `0x400085e9`, `0x4000860c`).
- The list is read token by token until a bare `}`: a `loadmenu` token
  hands the following `{ ... }` block to `0x40008480`, which loads each
  file name in it until its `}`, and any other token (the opening `{`) is
  skipped. A `loadmenu` not followed by `{` ends the list. Then it prints
  `UI menu load time = %d milli seconds`. VERIFIED (asm: the `0x7d` test
  at `0x4000864e`, the `loadmenu` compare at `0x40008653`).
- Each name is first tried under a path built from `cl_language`, then as
  given (`0x400084f7..0x40008566`). VERIFIED (asm): `atoi` (0x40019b68)
  of the cvar; non-zero takes UI syscall 2 (CoDMP.exe 0x4a9a00), which
  returns the language name from the table at 0x571b30 (`english`,
  `french`, `german`, `italian`, `spanish`, `british`, `russian`,
  `polish`, `korean`, `taiwanese`, `japanese`, `chinese`, `thai`, `leet`;
  an index of 14 or more reads `english`). The path is the name's
  directory with its trailing `/` (0x400058b0), the language and `/`
  (`%s%s/` at `0x4002f1d4`), then the file name (0x40005870), so
  `ui_mp/main.menu` under 2 is `ui_mp/german/main.menu`. When that load
  (0x40008240) fails the name loads as given. `cl_language` is registered
  `"0"`, flags `0x21` (CoDMP.exe 0x4a99b0), and English is 0, so a stock
  install never tries the localized path. VERIFIED.
- `cl_language` latches. Flag `0x20` is Q3's `CVAR_LATCH`: `Cvar_Set2`
  (0x439650) tests it at `0x4397db`, compares the new value against the
  pending one (`+0xc`) when there is one, else the live one (`+0x4`),
  returns on a match, else prints `%s will be changed upon restarting.`
  (0x5622e4) and stores the value as the pending one. `Cvar_Get`
  (0x439350) takes a pending value when it meets an existing cvar
  (`0x439489..0x4394ac`), and `Cvar_WriteVariables` (0x43a100) writes the
  pending value over the live one for an archived cvar (`0x43a148`).
  VERIFIED (the disassembly). The registrations are startup's
  (`0x42d210` calls 0x4a9a60, which registers it at `0x4a9a6c`),
  `CL_Vid_Restart_f` (0x40fbe0, the `vid_restart` command at `0x41262f`;
  `Cvar_Get` at `0x40fd14`, then the language check 0x4a9b10 at
  `0x40fd6e`) and that check itself (`0x4a9b1f`). `snd_restart`
  (0x40fdf0) calls `CL_Vid_Restart_f` (`0x40fe9d`). VERIFIED (the calls).
  So a `cl_language` set in a session reaches the menus at the next
  `vid_restart`, `snd_restart` or start. `CL_Vid_Restart_f` shuts the UI
  down (`0x418e30` at `0x40fc9b`) every time, clears the UI's started
  flag (`0x155f3d4`), and `CL_StartHunkUsers` (`0x4115c0`) starts it again
  through `CL_InitUI` (`0x418f10`: load the VM, check the API version 7,
  `UI_INIT`). VERIFIED (the calls). So every `vid_restart` reloads every
  menu, under the new directory, and leaves none open. With no game up
  `SCR_DrawScreenField` (`0x416dd0`) then sends `UI_SET_ACTIVE_MENU` 1
  (main), after `S_StopAllSounds`, whenever the UI is not full screen and
  the state is 0. VERIFIED (the calls). The command refuses first, with
  `Listen server cannot video restart.`, while `sv_running` is set.
  VERIFIED (the test ahead of the string's print). 0x4a9b10 rewrites `cl_language` (forced, `Cvar_Set2` with 1)
  to the first language with localized assets when the chosen one has
  none (the table's second dwords at `0x571b34`). VERIFIED that it writes;
  INFERRED what the availability test reads. A script menu
  (`ui_mp/scriptmenus/<name>.menu`, 0x40008330) goes through the same two
  tries (`0x400083d6..0x4000844d`). VERIFIED (asm).
- `UI_LoadMenus`' second argument, when non-zero, zeroes the menu count
  (0x40018ac0 stores 0 to 0x401bf400, the count `Menus_FindByName` reads
  at 0x40010561) before the list loads; 0 appends. VERIFIED (asm).
- `_UI_Init` (0x4000d310, `vmMain` case 1 at `0x400076be`) sets
  `ui_menuFiles` back to `ui_mp/menus.txt` (`trap_Cvar_Set` at
  `0x4000d324`) before it reads the cvar at `0x4000d624`. So the UI always
  starts off the stock name, whatever a config set. It then loads
  `ui_mp/ingame.txt` with the reset argument 0 (`0x4000d64c`). VERIFIED
  (asm).
- `ui_load` (string `0x4002fafc`, `UI_ConsoleCommand` at `0x40006d55`)
  runs `UI_Load` (0x400086c0). It copies the name (field `+0x20`) of the
  menu `0x40016970` returns, reads `ui_menuFiles` (empty reads
  `ui_mp/menus.txt`), reloads that list with the reset argument 1, then
  calls `0x40010660` and `0x400134b0` with the saved name. VERIFIED (asm).
  INFERRED, from the shape it shares with Team Arena's `UI_Load`:
  `0x40016970` is `Menu_GetFocused` and the two calls are `Menus_CloseAll`
  and `Menus_ActivateByName`, so the focused menu comes back open off the
  new files and `ingame.txt` is not reloaded.
- Stock `menus.txt` names three files no 1.1 pak ships:
  `ui/options_view.menu`, `ui/options_defaults.menu` and
  `ui_mp/in_rec_restart.menu`; each costs a `^1menu file not found`
  line. VERIFIED (pak listings).
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
- Owner draws: `UI_NETSOURCE` (220), `UI_SERVERREFRESHDATE` (247) and
  `UI_JOINGAMETYPE` (253). The `UI_SERVERMOTD` item and both `mappreview`
  items (`UI_NETMAPCINEMATIC` (246) at `497 3 100 80`, and a filled
  rect over the same spot) sit inside `/* */` comments, so the stock
  browser draws no map preview. VERIFIED (`ui_mp/joinserver.menu` in
  `pak0.pk3`; no other menu in the 1.1 paks names `UI_NETMAPPREVIEW` or
  `UI_NETMAPCINEMATIC`). The localized strings the
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

1.5's menu has nine columns, with hardware, PunkBuster and mod icons and
marks; `cod15-protocol-diff.md` section 11 has its feeder.

The 1.2-1.4 patch pak `pak8.pk3` ships a `joinserver.menu` whose list box
has eight columns (`columns 8`, positions 2 22 41 306 420 484 548 568) and
whose header tabs read password, hardware, server, map, players, type, mod,
ping. VERIFIED off the asset. That this is its feeder's column order is
INFERRED off the header; vcod lays out only six and nine (`columns_for`),
so an eight-column list draws in 1.1's order. `paka.pk3` (1.5) keeps the
eight-column block commented out above its nine.

vcod keeps the layout on the browser (`Browser::columns`) and derives it
from the loaded list box whenever the UI takes a browser
(`Ui::set_browser`): at start with the favourites cache and when a UI
restart carries the old lists over. A browser built without it draws a
nine-column list in 1.1's six, shifted a column right (issue #3).

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
  port 20510 came back in one 378-byte packet:
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

## 6. Sources and the display list

- `ui_netSource` picks the list: 0 Local, 1 Internet, 2 Favorites. The
  `UI_NETSOURCE` owner draw prints `EXE_LOCAL`, `EXE_INTERNET` or
  `EXE_FAVORITES` from the pointer table at `0x40036ad8`. VERIFIED (the
  table's three pointers). The cvar is archived with default `0` (cvar
  table entry at `0x40036c8c`: name `0x4002f710`, default `"0"`, flags 1),
  so a first run opens the browser on Local. VERIFIED. A retail
  `config_mp.cfg` carries `seta ui_netSource "0"` and the four
  `ui_browserShow*` cvars at `"1"`. VERIFIED (the file).
- `UI_NetSource_HandleKey` (`0x40009b90`) steps the source up on keys 200,
  `0xd` and `0xbf` and down on 201, wraps it to 0..2, rebuilds the display
  list, starts a refresh unless the new source is Internet (`cmp` against 1
  before the call to `0x4000ea90`), and writes `ui_netSource`. INFERRED off
  the branches; that 200 and 201 are the mouse buttons is INFERRED from the
  click handling.
- `UpdateFilter` (compared at `0x4000ac82`), which `joinserver`'s `onOpen`
  runs, starts a refresh only when the source is Local (`test eax,eax` on
  `0x401c146c` at `0x4000ac97`), then rebuilds the display list. INFERRED.
- The refresh starter (`0x4000ea90`) first stores the date in
  `ui_lastServerRefresh_%i` (`0x4002f000`) as `%s %i, %i   %i:%02i` with the
  month from `EXE_MONTH_ABV_*`, then sends `localservers` for source 0
  (`0x4000eb5b`) and gives it 1000 ms (`DAT_401ea684 = now + 1000`), or
  5000 ms and `globalservers` for source 1; source 2 sends nothing. VERIFIED
  for the strings; the timings are INFERRED off the stores.
- `UI_SERVERREFRESHDATE` (`0x40008fc5`): while a refresh runs it prints
  `EXE_WAITINGFORMASTERSERVERRESPONSE` when `LAN_GetServerCount` (trap
  `0x4f`) is negative, else `EXE_GETTINGINFOFORSERVERS` with that count, the
  source's list length (`0x4000905d`..`0x4000906e`); after it, `EXE_REFRESHTIME`
  with `ui_lastServerRefresh_<source>` (`0x400090fa`..`0x4000913b`).
  INFERRED off the branches.
- `UI_BuildServerDisplayList` (`0x4000b800`) lists a server only if the
  engine has it visible and its ping is above 0, except on Favorites, which
  lists every entry (`0x4000b957`). Then, in order (INFERRED off the
  branches, each skip a `LAN_MarkServerVisible(source, n, 0)`, trap `0x52`):
  an `addr` of `000.000.000.000` (`0x4002e76c`) is dropped;
  `ui_browserShowEmpty` 0 drops `clients` 0 (`0x4000b9d4`);
  `ui_browserShowFull` 0 drops `clients` equal to `sv_maxclients`
  (`0x4000b9f5`); `ui_browserShowPassword` 0 drops `pswrd` non-zero
  (`0x4000ba2d`); `ui_browserShowNoPassword` 0 drops `pswrd` 0
  (`0x4000ba65`); then the game type filter (`gametype` against the
  `ui_joinGameType` entry) and the game filter (`game`; its table at
  `0x4002d700` holds only `EXE_ALL`). The four `ui_browserShow*` vmCvars
  are at `0x401c3980`, `0x401ef3c0`, `0x401efcc0`, `0x401c3860` (table
  entries `0x40036ddc`, `0x40036dcc`, `0x40036dec`, `0x40036dfc`), all
  archived with default `"1"`. VERIFIED (the table).
- The display flags: `UI_SHOW_FAVORITESERVERS` (4) shows an item only on
  Favorites and `UI_SHOW_NOTFAVORITESERVERS` (`0x1000`) only off it
  (`0x40009780`, the `& 4` and `& 0x1000` tests against `ui_netSource`).
  INFERRED. `joinserver.menu`'s `addFavorite` button carries the second flag
  but is `visible 0` and nothing shows it, so retail's browser has no Add to
  Favorites button; `delfavorite` (flag 4, `visible 1`) and `createFavorite`
  (`showCvar { "2" }` on `ui_netSource`) are the favourites controls.
  VERIFIED (the file); that a `visible 0` item stays hidden is INFERRED from
  the RTCW lineage.

### The game type filter

- `UI_LoadGameTypes` (`0x4000cf80`) builds two lists: the join list
  (count `0x401e33ec`, 8-byte entries from `0x401e33f0`: script name, then
  display name) and the game type list (count `0x401e32e8`, entries from
  `0x401e32ec`) that the create-server screen reads. The join list's
  entry 0 is `All` (`0x4002f0c8`) with an empty display name
  (`0x4002dfb9`). Then `trap_FS_GetFileList("maps/mp/gametypes", "gsc")`
  (`0x40019000`, strings `0x4002e3f4`, `0x4002e408`) fills both: a name
  starting with `_` is skipped (`cmp byte [esi],0x5f` at `0x4000d030`),
  `.gsc` (`0x4002e3ec`) is cut off the end, and the script name goes into
  both lists. VERIFIED (asm). A full list stops the loop with `Too many
  game type scripts found! Only loading the first %i` and 31 (`push 0x1f`
  at `0x4000d152`); the join list's cap of 32 (`cmp ...,0x20` at
  `0x4000d05a`, `0x4000d062`) is the one that bites, as it holds `All`
  too. VERIFIED (asm). None found prints `No game type scripts found in
  maps/mp/gametypes folder` (`0x4002e358`). VERIFIED.
- The display name is the first token (`0x400054f0`, `COM_Parse`'s shape)
  of `maps/mp/gametypes/<script>.txt` (`0x4002e390`); a missing file names
  the entry after its script. The loader is `0x40007c30`, which prints
  `^1menu file not found: %s, using default` (`0x4002f344`) on a miss.
  VERIFIED for the strings and the branch at `0x4000d0b6`; INFERRED that
  `0x40007c30` is Team Arena's `GetMenuBuffer`.
- Stock: `pak5.pk3` holds `maps/MP/gametypes/` with `bel`, `dm`, `re`,
  `sd`, `tdm` and the four `_` helpers; the five `.txt` files read
  `"BEL_BEHIND_ENEMY_LINES"`, `"DM_DEATHMATCH"`, `"RE_RETRIEVAL"`,
  `"SD_SEARCH_AND_DESTROY"` and `"TDM_TEAM_DEATHMATCH"`, references into
  `localizedstrings/english/<script>.str` (`dm.str`'s `DEATHMATCH` is
  "Deathmatch"). VERIFIED (the paks).
- The list runs in `FS_GetFileList`'s order. `FS_ListFilteredFiles`
  (CoDMP.exe `0x42b2a0`) walks the search path from `0x1407420`, newest
  pak first, and each pak's file table (`pak+0x31c`, 24-byte entries,
  count at `pak+0x30c`) in order, adding through `0x42b200`, which skips
  a name already listed; it never sorts. VERIFIED (decompile). That the
  file table keeps the zip's central-directory order is INFERRED (Q3's
  `FS_LoadZipFile`). A loose directory on the path is listed through
  `0x460770` only on an unpure path. VERIFIED (the branch). So a mod
  pak's game types come ahead of pak5's five, which are alphabetical in
  the zip (`unzip -l`). VERIFIED. `UI_LoadArenas`' `.arena` files go the
  same way. vcod reads both off `Pk3Fs::file_list`; it has no loose
  files.
- The UI also runs the loader from `UI_Load` (`0x4000871d`) and from the
  `loadGameInfo` menu script (`0x4002eb40`, call at `0x4000aa25`).
  VERIFIED (the calls).
- `ui_joinGametype` (vmCvar row `0x40036cbc`, integer at `0x401c374c`) is
  archived with default `"0"`. VERIFIED (the table). Its owner draw,
  `UI_DrawJoinGameType` (`0x400088b0`), resets the cvar to `0` when it reads
  below 0 or above the join count, then prints the entry's display name
  through the UI's localize call (`0x4000ec20`), or `EXE_ALL`
  (`0x4002f86c`) when that name is empty. VERIFIED (asm). The bound test
  is `jle` against the count, so a value equal to the count passes and
  reads one past the list. INFERRED; vcod reads it as All.
- `UI_JoinGameType_HandleKey` (`0x40009b10`) steps up on keys 200, `0xd`
  and `0xbf` and down on 201, wraps to `0 .. count-1`, writes the cvar
  (`%i`, `0x4002fa84`) and rebuilds the display list (`0x4000b800` with 1).
  It starts no refresh. VERIFIED (asm).
- In `UI_BuildServerDisplayList` the filter runs only when the selected
  entry's display name is non-empty (`0x4000baaa`): the server's
  `gametype` (`0x4002f9c0`) is compared to the entry's script name with
  `0x40005e00`, which calls `0x40005d60` with a limit of 99999, Q3's
  `Q_stricmp` over `Q_stricmpn`; a mismatch hides the server. VERIFIED for
  the calls; that the compare folds case is INFERRED from the Q3 shape.
  It does not strip colour codes, so a server reporting `^7dm` (seen live,
  section 5) passes only All.
- The localize call (`0x4000ec20`) returns the key itself for a reference
  the tables lack, unless `cl_languagewarnings` (vmCvar row `0x40036e48`,
  default `"0"`) is set, which prints `^3WARNING: Could not translate ui
  string "%s"` (`0x4002dfd8`) and wraps the key in `^1UNLOCALIZED(^7` ..
  `^1)^7`; `cl_languagewarningsaserrors` (`0x40036e58`) makes it an error.
  VERIFIED (the branches at `0x4000ec30`..`0x4000ed0f`, the table rows).

### The refresh date

- `UI_StartServerRefresh` (`0x4000ea90`) first reads the clock through
  `trap_RealTime` (`0x40019770`) and stores, for the current source, the
  cvar `ui_lastServerRefresh_<source>` (`0x4002f000`) as `%s %i, %i
  %i:%02i` (`0x4002e07c`; three spaces before the hour) over the month's
  localized abbreviation (table `0x40036aa8`, indexed by `tm_mon`), the
  day, `tm_year + 1900` (`0x76c`), the hour and the minute. VERIFIED (asm).
  Its argument then picks a full refresh or a re-ping
  (`0x4000eaf9`); both paths are dated. VERIFIED. That the time is local is
  INFERRED from Q3's `Com_RealTime` (`localtime`).
- The month table is `EXE_MONTH_ABV_JANUARY` .. `_DECEMBER`, but June is
  `EXE_MONTH_ABV_JUN` while `exe.str` defines `MONTH_ABV_JUNE` ("Jun"); every
  other month matches ("Jan", "Feb", ..). VERIFIED (the table and
  `localized_english_pak1.pk3`). So a June refresh reads `Refresh Time:
  EXE_MONTH_ABV_JUN 11, 2026   14:05` with `cl_languagewarnings` 0.
  INFERRED from the localize call above.
- The cvar is not in the vmCvar table and a retail `config_mp.cfg` holds
  none, so it lives for one run. VERIFIED (the table, the file). A source
  never refreshed this run prints `Refresh Time: ` alone. INFERRED.
- When the refresh ends, `UI_StopServerRefresh` (`0x4000e9b0`) prints to
  the console `%d servers listed in browser with %d players.`
  (`0x4002e0d0`) and, when the source holds more than it lists, `%d servers
  not listed (filtered out by game browser settings)` (`0x4002e090`).
  VERIFIED (the strings and pushes). Neither is drawn in the browser.

### The map preview

- `UI_FeederSelection` for `FEEDER_SERVERS` (`0x4000ccee`) reads the
  selected server's info, registers `levelshots/<mapname>` (`0x4002f9cc`,
  `0x4000cd2a`, through `0x400191c0` with 2, `trap_R_RegisterShaderNoMip`'s
  shape) into `0x401ec6b0`, and stops a server cinematic if one runs
  (`0x401ec6b4`). It never starts one: the Team Arena call that plays
  `<map>.roq` is gone. VERIFIED (asm).
- `UI_OwnerDraw`'s jump table (`0x40009700`, index bytes `0x40009740`,
  base 200) sends 221 (`UI_NETMAPPREVIEW`) to `0x40008b60` and 246
  (`UI_NETMAPCINEMATIC`) to `0x40008bb0`. VERIFIED. `0x40008bb0` clamps
  `ui_currentNetMap` (`0x4002f5bc`), draws the server cinematic when one
  runs, else falls through to `0x40008b60`. `0x40008b60` draws the
  registered levelshot when its handle is above 0, else
  `menu/art/unknownmap` (`0x4002f078`), over the owner draw's rect.
  VERIFIED (asm). INFERRED from the Q3 lineage: a missing image registers
  as 0, so a map without a levelshot draws `menu/art/unknownmap`.
- The 1.1 paks hold `levelshots/<map>.dds` (1024x1024 DXT1) for the twelve
  stock maps and `levelshots/unknownmap.jpg`, all in
  `localized_english_pak1.pk3`; no pak holds `menu/art/unknownmap`.
  VERIFIED (pak listings, the DDS header).
- Only a mod's menu can show any of this; the stock browser has no preview
  (section 4).

## 7. Local servers (`localservers`)

- `CL_LocalServers_f` (CoDMP.exe `0x413710`, registered at `0x428840` with
  the string `localservers`) prints `Scanning for servers on the local
  network...` (`0x5661b0`), zeroes the Local count (`0x155f400`) and its
  128 `0xb8`-byte entries from `0x155f404`, all but the `int` at `+0xac`
  (the loop at `0x413750` saves and restores it), then sends
  `\xff\xff\xff\xffgetinfo xxx` (`0x5661a0`) through `NET_SendPacket`
  (`0x4493e0`) to a netadr of type 3 (`NA_BROADCAST`, stored at
  `0x4137ac`) on port `htons(0x7120 + j)` for `j` 0..3 (`0x413792`,
  `cmp ebp,4`), in two rounds (`mov [esp+0x10],2`, `dec`/`jne` at
  `0x413872`). VERIFIED (the string, the port base and both loop counts).
  So eight packets: 28960, 28961, 28962, 28963, twice. That `NA_BROADCAST`
  goes to `255.255.255.255` is INFERRED from the Q3 lineage
  (`NetadrToSockadr`); the socket has `SO_BROADCAST` (the
  `UDP_OpenSocket: setsockopt SO_BROADCAST` warning string). VERIFIED for
  the string.
- Live, 2026-10-09, in a network namespace whose only interface is a dummy
  (a host firewall can drop a broadcast's local copy): one
  `getinfo xxx` broadcast to `255.255.255.255:29661` and `:29662` was
  answered by the retail 1.1d server and by vcod-server, each with its usual
  `infoResponse`; the retail one, started with `g_password secret`, sent
  `pswrd\1`. VERIFIED by capture (`lan_scan_live` in
  `crates/client/src/frontend/browser.rs` reran it on vcod's own scan).
- vcod: a Local refresh broadcasts the eight packets, lists every address
  that answers within 1000 ms (capped at 128), with the time from the
  broadcast as its ping. Retail adds broadcast answers in
  `CL_ServerInfoPacket` and pings them afterwards; the ping vcod shows is its
  own simplification.

## 8. Favourites (`servercache.dat`)

- `LAN_LoadCachedServers` (CoDMP.exe `0x417490`) reads `servercache.dat`:
  three `int` counts (Internet at `0x1565004`, favourites at `0x15c400c`,
  a third at `0x15c1008`), then an `int` size that must be `0x64c00`, then
  `0x5c000` bytes of Internet entries (`0x1565008`), `0x5c00` of favourites
  (`0x15c4010`) and `0x3000` more (`0x15c100c`); a wrong size zeroes the
  counts. `LAN_SaveServersToCache` (`0x417570`) writes the same seven
  fields. VERIFIED. So the file is 16 + `0x64c00` = 412688 bytes; the one
  in a 1.1 install, beside `CoDMP.exe`, is that size with all counts 0.
  VERIFIED (the file). The Local list is never cached. VERIFIED (not among
  the fields). That the third block is the master's address list (2048 of
  six bytes) is INFERRED from its size.
- An entry is `0xb8` bytes. `LAN_GetServerInfo` (`0x417a10`) reads it
  into an info string with the keys `hostname`, `mapname`, `clients`,
  `sv_maxclients`, `ping`, `minping`, `maxping`, `game`, `gametype`,
  `nettype`, `addr`, `sv_allowAnonymous` and `pswrd` (string refs
  `0x5663d0`..`0x566378`). VERIFIED. Which offset feeds which key, read off
  the load order, is INFERRED: a 20-byte netadr (`int` type, 4 is
  `NA_IP` and 2 loopback per `NET_AdrToString` `0x449150`; IP at +4; port
  big-endian at +18), `hostname` at `0x14`, `mapname` `0x34`, `game`
  `0x54`, `nettype` `0x74`, `gametype` `0x78` (32-byte strings), then
  `int`s `clients` `0x98`, `sv_maxclients` `0x9c`, `minping` `0xa0`,
  `maxping` `0xa4`, `ping` `0xa8`, `sv_allowAnonymous` `0xb0`, `pswrd`
  `0xb4`. `LAN_ResetPings` (`0x417600`) writes -1 at `0xa8`; `LAN_AddServer`
  sets 1 at `0xac`, the visible flag. INFERRED.
- The UI reaches the lists through traps (the `0x40036030` syscall
  pointer): `0x55` load the cache and `0x56` save it (CoDMP.exe dispatch
  `0x418314`, cases at `0x4187a0`, `0x4187ac`); the UI calls load from its
  init (`0x40007c30`) and save from its shutdown (`0x40007c20`). VERIFIED
  for the jump table cases and the call sites. So retail writes the file
  when the UI shuts down, not on each change.
- `LAN_AddServer` (trap `0x57`, `0x417640`): -1 when the list is full (128
  for favourites), -2 when `NET_StringToAdr` (`0x449690`) fails, 0 when the
  address is already listed (`NET_CompareAdr` `0x449230`), else copies the
  address and the name (`strncpy` of `0x1f` bytes) and returns 1. INFERRED
  off the branches. `LAN_RemoveServer` (trap `0x58`, `0x4177d0`) parses
  the address and closes the gap with `0xb8`-byte moves. INFERRED.
- The UI's favourite scripts (`UI_RunMenuScript`, VERIFIED for the string
  compares): `addFavorite` (`0x4000b01b`) acts off Favorites only, reads the
  selected server's `hostname` and `addr` (`0x4002e8c0`, `0x4002e8b8`) and
  adds them; `deleteFavorite` (`0x4000b0af`) acts on Favorites only and
  removes the selected server's `addr` when non-empty; `createFavorite`
  (`0x4000b13c`) acts on Favorites only and adds `ui_favoriteName` and
  `ui_favoriteAddress` (`0x4002e888`, `0x4002e874`). The add (`0x4000a4a0`)
  checks, in order: an empty name sets `ui_favorite_message` to
  `@EXE_FAVORITENAMEEMPTY`, an empty address `@EXE_FAVORITEADDRESSEMPTY`;
  then `LAN_AddServer(2, ...)`'s 0, -1, -2 and 1 give `@EXE_FAVORITEINLIST`,
  `@EXE_FAVORITELISTFULL`, `@EXE_BADSERVERADDRESS` and
  `@EXE_FAVORITEADDED`; each also prints the localized text with `%s\n`
  (`0x4002ede4`). VERIFIED: the strings and their pushes. INFERRED: the
  order of the checks.
  `fav_message_popmenu` shows `ui_favorite_message` as its text.
- vcod reads the favourites block at start and writes it back after every
  add and delete and at the end of a Favorites refresh, keeping every other
  byte of an existing file; with none it writes a 412688-byte file whose
  Internet and address lists are empty. An entry vcod cannot address (IPX,
  loopback) is kept as read. A reply's hostname, map, counts and ping go
  into the entry, as `CL_SetServerInfo` updates every list holding the
  address (INFERRED from the Q3 lineage).

## 9. Password popup

- `password_popmenu` is an `ITEM_TYPE_EDITFIELD` on cvar `password`,
  `maxchars 12`, and an OK button that only closes the popup. VERIFIED
  (`ui_mp/password.menu`). CoDMP.exe registers `password` with flags 2
  (`CVAR_USERINFO`) and default `""` (push at `0x4123ea`, between
  `cl_anonymous` and `cg_predictItems`). VERIFIED. vcod sends it as
  `\password\<value>` after `cg_predictItems`, the newest-first cvar walk's
  place (INFERRED from the registration order), and leaves the key out when
  empty.
- Live, 2026-10-09, retail 1.1d with `g_password secret`: vcod's
  `NetClient` with no password and with `wrong` was dropped with
  `GAME_INVALIDPASSWORD` (the `game_mp_x86.dll` string at `0x5b780`); with
  `secret` it got the gamestate and went active. VERIFIED by capture.
- vcod-server reports `pswrd` and checks `g_password` as retail does
  (`cod11-server-handshake.md`, "`g_password`").

## 10. Server info popup

- `serverinfo_popmenu`'s `onOpen` and Refresh run `uiScript ServerStatus`
  (compared at `0x4000acc0`), which copies the selected server's address
  (trap `0x50`) and rebuilds the status. VERIFIED for the compare. The list
  is `FEEDER_SERVERSTATUS` (13), `notselectable`, `elementheight 16`, four
  columns at 2, 60, 110, 155 capped at 20, 10, 10 and 25 characters.
  VERIFIED (`ui_mp/serverinfo.menu`).
- The engine sends `getstatus` with no argument (CoDMP.exe push of
  `0x566250` at `0x4133eb`), resent every `cl_serverStatusResendTime`
  (registered at default `"750"`, `0x566938`). VERIFIED. The UI gives up
  after `ui_serverStatusTimeOut` (default `"7000"`, table entry
  `0x40036e0c`). VERIFIED for the default; its use is INFERRED.
- `UI_GetServerStatusInfo` (`0x4000bcc0`) builds rows of four strings: the
  first `address` with the address in the last column, then one row per
  serverinfo pair (key first, value last), then, under 125 rows, a blank
  row, a header of `@EXE_SV_INFO_NUM`, `_SCORE`, `_PING`, `_NAME`, and one
  row per player: its index (`%d`), score and ping (split at spaces) and the
  rest of the line, quotes included; at most 128 rows. INFERRED off the
  loop. `UI_SortServerStatusInfo` (`0x4000bbb0`) then walks the table at
  `0x40036ec8` (key, label, yes/no flag: `sv_hostname`, `address`, `pswrd`
  (yes/no), `gamename`, `g_gametype`, `sv_pure` (yes/no), `mapname`,
  `shortversion`, `protocol`, `sv_maxping`, `sv_minping`, `sv_maxrate`,
  `sv_floodprotect`, `sv_allowanonymous`, `sv_maxclients`,
  `sv_privateclients`; VERIFIED), and for every row whose second column is
  empty and whose key matches, case-insensitively, swaps its key and value
  into the next row from the top, puts the label in the key column, and for
  a yes/no key prints `@EXE_YES` or `@EXE_NO` by the value's `atol`.
  INFERRED off the loop.

## 11. Filter and favourite popups

- `filter_popmenu` holds five `ITEM_TYPE_YESNO` items on
  `ui_browserShowEmpty`, `ui_browserShowFull`, `ui_browserShowPassword`,
  `ui_browserShowNoPassword` and `ui_browserShowTourney` (the last
  `visible 0`), and OK closes it. VERIFIED (`ui_mp/filter.menu`). A yes/no
  item draws `EXE_YES` or `EXE_NO` (pushed at `0x4001491b`,
  `0x40014922`). VERIFIED for the pushes. The filters apply at the next
  display list build (section 6).
- `createfavorite_popmenu` has two edit fields, `ui_favoriteName` and
  `ui_favoriteAddress` (`maxchars 30`), and OK runs `uiScript
  CreateFavorite`, closes the popup and opens `fav_message_popmenu`.
  VERIFIED (`ui_mp/createfavorite.menu`).
- vcod's popups use the options screens' edit field and yes/no item
  (section 14).

## 12. vcod's front end

- vcod loads the menu files `ui_mp/menus.txt` names off the search path,
  in its order, so a mod's own list and menus replace the stock ones
  (`vcod_common::ui_menu::menu_list`). `ui_load` reloads off the list
  `ui_menuFiles` names (a missing one falls back with retail's warning)
  and reopens the focused menu. A front-end restart sets the cvar back to
  its default, as `_UI_Init` does. A start or restart adds
  `ui_mp/ingame.txt`'s menus after the first list's, and `ui_load` drops
  them, as retail's reset does. Every menu file and script menu is tried
  under `cl_language`'s directory first (registered `"0"`, archived and
  latched: a set prints retail's line, the config gets the pending value,
  and startup or `vid_restart` applies it). Every `vid_restart` restarts
  the UI closed and rebuilds the script menu cache, as section 1 reads
  it; with no game up the main menu comes back. The server lists carry
  over, as retail's survive the restart in `servercache.dat`. While the
  server Dedicated No started runs, `vid_restart` prints retail's
  refusal. vcod has no `snd_restart` and no language-availability
  fallback. `mp_QuickMessage` opens `quickmessage`
  (`cod11-quick-chat.md`, "The quick message menu").
- It draws the main menu, the browser and its popups (sections 6-11),
  the options set (section 14), the quit popup and the error popup from the
  stock files with their layout, the main menu again over a game (section
  13), the Mods menu (section 17) and Start New Server with its game type
  pages (section 18). The stock menus it cannot run yet (CD key, single
  player, the credits and driver info pages, the language and record
  restarts, `multi_menu`, `Connect`, `auconfirm`) are refused with a
  console line; any other menu the list loaded opens, a mod's included. A
  button whose script would close its own menu and then open a refused or
  unknown one is refused whole, so the main menu stays up.
- Ping pacing (32 `getinfo`s in flight, a 1.5 s timeout, 5 s for the master)
  is vcod's own; retail's numbers were not measured.
- The Internet source asks `master.cod.pm` (its operator allowed it), not
  section 5's Activision host: `getservers <protocol> full empty` with the
  install's protocol alone (1 on 1.1, 6 on 1.5, both on port 20510; 22 on
  UO, port 20610), never 0, which lists every game. One request a refresh
  and at most one a second to a master host, process-wide
  (`net::master::master_send_allowed`); a list that comes in several
  packets is read whole, not asked again. A listed server whose
  `infoResponse` names another protocol is dropped. Live, 2026-10-22:
  `getservers 1` listed 49 servers in one packet, `6` 50, UO's `22` 131
  in two (the first ending `\EOT`, the last `\EOF`). VERIFIED capture.
- Text is placed as RTCW's `Item_SetTextExtents` does: baseline at
  `rect.y + textaligny`, x at `textalignx` moved left by the width (right
  aligned) or half of it (centred); an owner draw with no label draws at
  `textalignx` whatever its alignment. INFERRED from the RTCW lineage, and
  the screenshots match the stock layout by eye.
- The game type filter, the refresh date and the preview owner draws
  follow section 6. vcod keeps each source's date in the browser, not in
  a `ui_lastServerRefresh_<n>` cvar. A right click acts as a left click
  and steps the game type and source owner draws back (section 19). A missing `menu/art/unknownmap` draws
  nothing.
- Not done: the CD key popup, the Internet list's cache in
  `servercache.dat` and `focusColor`'s pulse
  (section 20). The server list and the map list draw retail's
  scroll bar (section 19); the Mods list does not.

## 13. Esc in a game

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

### The script menu's cursor

- Cases 10 and 11 run only when no menu has the focus or the focused one
  came from case 10 or 11 (`0x401bf42c` is 10 after either). Case 11
  writes 639, 479 into the display context's cursor (`+0xf4`, `+0xf8`;
  `0x4000dae7`, `0x4000daf1`), case 10 writes 320, 240 (`0x4000dafd`,
  `0x4000db07`); then both set the UI catcher, close every menu and open
  `ui_newScriptMenu`. Case 8 writes 639, 479 too. VERIFIED (asm).
- cgame's `t` handler (`cgame_mp_x86.dll` `0x3002dae0`) sets
  `ui_newScriptMenu` and `ui_newScriptMenuIndex` and asks for
  `UIMENU_SCRIPT_POPUP_NO_MOUSE` when the command has a third argument
  that is not empty, else `UIMENU_SCRIPT_POPUP`. VERIFIED (decompile).
  The game's `openMenuNoMouse` (`game.mp.i386.so` `0x454b4`, builtin
  table row 28) sends `t %i 1`; `openMenu` (`0x453f4`) sends `t %i`.
  VERIFIED (the builtin table and the format strings). So case 10 is
  `openMenu` and case 11 `openMenuNoMouse`. INFERRED (the names'
  order). No stock script calls `openMenuNoMouse`. VERIFIED (the pak
  scripts). VERIFIED (decompile of 0x454b4): its body is `openMenu`'s
  with the other format, and vcod-server has both builtins
  (`builtins::client::open_menu_no_mouse`).
- `UI_Refresh` (`vmMain` case 5, `0x40007b20`) paints the menus and then
  the cursor shader (`0x401e1f9c`) 32 by 32 at the cursor less 16 when
  the menu count (`0x400189a0` answers `0x401bf400`) is not 0, after
  `0x40007090(0)` and through `0x40006e30`, which sets no colour.
  VERIFIED (decompile). `0x40007090(0)` is Q3's `UI_SetColor(NULL)`, so
  the cursor draws white. INFERRED (Q3's `_UI_Refresh` has that order).
  The shader is the `assetGlobalDef` keyword `cursor` (string
  `0x4002f280`): `0x40008060` registers its name through `0x400191c0`
  (UI syscall `0x19`) into `0x401e1f9c`. VERIFIED (asm). Stock
  `main.menu` names `ui/assets/3_cursor3` (a 32 by 32 `.tga` with alpha in
  `pak0.pk3`), section 1. VERIFIED (the file).
- `SCR_DrawScreenField` (CoDMP.exe `0x416dd0`) calls UI `vmMain` 5 at
  its end whenever the UI catcher (bit 2 of `0x155f2c4`) is set, whatever
  the connection state and the console. VERIFIED (decompile). So the
  cursor shows over every menu, the front end's, a script menu and the
  quick message menus alike; with `cl_bypassMouseInput` 1 the mouse turns
  the view and the cursor stays where case 8 parked it, three quarters
  off the bottom-right corner. INFERRED (the combination).
  `CL_MouseEvent` (`0x40b0a0`) tests only the UI catcher and the bypass,
  not the console bit, so the mouse moves the cursor under the console
  too. VERIFIED (decompile). The mouse
  event (`vmMain` case 4, `0x4000d790`) moves the cursor by the delta,
  clamped to 0..640 and 0..480, and runs `Display_MouseMove` there.
  VERIFIED (decompile, asm). The client refreshes the UI while the UI
  catcher is set, as Q3's `SCR_DrawScreenField` does. INFERRED (Q3; the
  CoDMP.exe caller was not read). So a script menu shows the cursor at
  the screen's centre, and the no-mouse form parks it in the bottom-right
  corner, three quarters off screen. INFERRED (the combination). Whether the mouse moves the cursor or the
  view is `CL_MouseEvent`'s choice: the UI catcher sends it to the cursor
  unless `cl_bypassMouseInput` is 1 (`cod11-quick-chat.md`), so a team or
  weapon menu holds the mouse and the quick message menus, whose
  `onOpen` sets the bypass, leave it on the view. INFERRED (the
  combination).
- Where the deltas come from, CoDMP.exe. VERIFIED (decompile): `IN_Frame`
  (`0x461a80`) keeps the mouse active, through `0x461730`, and reads it
  every frame, through `0x461850`, unless the console bit of the catcher
  word (`0x155f2c4 & 1`) is set while `r_fullscreen` is 0, or the window
  is not active (`0x16bdd98`); otherwise it deactivates it (`0x4616b0`:
  `ClipCursor(NULL)` when windowed, `ReleaseCapture`, `ShowCursor(1)`).
  Activating (`0x4615e0`) centres the pointer in the window,
  `SetCapture`s, clips it to the window when windowed and hides it.
  `0x461850` reads `GetCursorPos`, puts the pointer back at the centre
  and queues event 3 with the offset from the centre. `CL_MouseEvent`
  (`0x40b0a0`) hands that offset to UI `vmMain` 4 as it is. INFERRED,
  from those three: a menu's cursor moves one unit of the 640x480 grid
  per pixel the OS pointer would have moved, whatever `sensitivity`,
  `m_yaw` or `m_pitch` say (they scale only the view, `0x40b130`), and
  with the console down in a window the cursor does not move at all.

### The menus it reaches

- `ui_mp/ingame.txt` (`pak0.pk3`) lists `ui_mp/ingame.menu` and five more
  `ingame_*` files, none of them in any 1.1 pak, then
  `ui_mp/wm_quickmessage.menu`, which `pak0.pk3` ships: menuDef
  `quickmessage`, `UI_SetActiveMenu` case 8, whose `execKey` 1-3 close it
  and open the `quickcommands`, `quickstatements` and `quickresponses`
  script menus. VERIFIED (pak listings, the files). The cgame command
  `mp_QuickMessage` is what reaches case 8 (`cod11-quick-chat.md`, "The
  quick message menu").
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
  but the UI plays from alias set 0 (section 20, "Keywords"), which
  CoDMP.exe's `CL_InitUI` (`0x418f10`) loads under the name `menu` before
  it loads the UI module (`0x433e90` with set 0, then `0x4602e0` with the
  dispatcher `0x418300`). The only code that frees set 0 (base
  `0x893dd8`, count `0x893de4`, buckets `0x8931d8`) is the function just
  before it, ending at `0x418f00`. VERIFIED (asm). So the menus' `play`
  commands find `mouse_over` and `mouse_click` over a game too. INFERRED
  (that function is the UI shutdown). `main`'s `onOpen` plays the looping
  `music_mainmenu` there as well; the cgame's end-of-frame call (syscall
  `0xd2`, `0x44e600`, `cod11-sound-system.md`) ends every looping voice
  not played again that frame, so over a running game the music ends the
  frame it starts. INFERRED (not heard).

### Where the script menus come from

- The cgame loads every script menu the server precached as a UI menu
  when it starts: `0x3002c060` walks configstrings `0x49c..0x4bb` (1180
  ..1211) and passes each non-empty name to syscall `0x79`, failing with
  `Could not load script menu file '%s'` through `0x30020750` (the
  function `CG_ConfigString: bad index` errors through) when it returns
  0. VERIFIED (decompile). Syscall `0x79` reaches UI `vmMain` 15
  (`0x40008330`), which reads `ui_mp/scriptmenus/<name>.menu`, under
  `cl_language`'s directory first, into the UI's menu list. INFERRED
  (the jump table entries and the path the function builds). So a script
  menu is a UI menu like `main`: the same parser, painter, focus, mouse
  and key code drive it, and a missing file ends the game.
- `vmMain` 15 hands the file to `UI_ParseMenu` (`0x40008240`), the
  parser `UI_LoadMenus` uses: each top-level token is compared with
  `assetGlobalDef` (string `0x4002f20c`), whose block goes to
  `Asset_Parse` (`0x40007cc0`, call at `0x400082bb`), then with `menudef`
  (`0x4002f204`), whose block goes to `Menu_New` (`0x400082dc`).
  VERIFIED (asm). So a script menu file's `assetGlobalDef` sets the UI's
  global assets, its `cursor` included, over what the list menus set.
  vcod merges a script menu's `cursor` and `itemFocusSound` into the
  UI's assets (`Ui::load_script_menus`).
- `0x30020750` formats its message and calls cgame syscall 1
  (`CG_ERROR`) with it. VERIFIED (decompile). CoDMP.exe's cgame
  dispatcher (`0x401df0`, jump table `0x4041f0`) sends case 1 to
  `Com_Error(1, "%s", message)` (`0x401e3f`; the format at `0x568b30`).
  VERIFIED (asm). Code 1 is `ERR_DROP`, which takes section 15's second
  path: `com_errorMessage` gets the text as written, as it is no
  localization key, and the game drops to `main` with `error_popmenu`.
  INFERRED (section 15 and `UI_SetActiveMenu` case 1). The text is
  `Could not load script menu file '<name>'` with a trailing newline
  (`cgame_mp_x86.dll` string at file offset `0x62c64`). VERIFIED.
- `scriptMenuResponse` sends `mr` with the menu's index looked up by
  name (`cod11-quick-chat.md`). Every stock team, weapon, `viewmap` and `callvote`
  menu's `onOpen` sends `open` and its `onClose` `close`, so a pick sends
  its response and then `close`; the stock gametypes skip both
  (`tdm.gsc:257`, `if(response == "open" || response == "close")
  continue;`). VERIFIED (the files).

### When a `t` opens its menu

- `UI_SetActiveMenu` records the id it ran in `0x401bf42c` for every
  case but 10 and 11, which write 10 (`0x4000dadb`); `vmMain` 8
  (`0x4000d800`) returns it. VERIFIED (asm). Cases 10 and 11 go on only
  when `Menu_GetFocused` finds no menu or the recorded id is 10 or 11
  (`0x4000da8b`..`0x4000da9e`); else they return 0. With a focused menu
  whose name (`+0x20`) matches `ui_newScriptMenu` (`0x40005e00`) they
  return 1 and change nothing (`0x4000dacd`). VERIFIED (asm). So a menu
  that Esc (case 2), the main menu (case 1) or `mp_QuickMessage` (case 8)
  put up refuses a `t`, and a menu a script popup's own `open main`
  brought up does not. INFERRED (the combination).
- cgame's `t` handler (`0x3002dae0`) asks the UI through syscall `0x7a`;
  on 0 it stores the menu in `ui_waitingScriptMenu`,
  `ui_waitingScriptMenuIndex` and `ui_waitingScriptMenuNoMouse`, first
  sending `cmd mr %s noop` with `ui_waitingScriptMenuIndex` for a menu
  already waiting there. `0x3002dd10` offers the waiting menu again and
  clears it once the UI takes it; the server command pass `0x3002e580`
  calls it first. VERIFIED (decompile). When the waiting menu's name
  matches the new one under `Q_stricmpn` (`0x3003e8f0`, length 99999)
  the handler returns at once (`0x3002dc3d`..`0x3002dc4f`): no `noop`,
  and the first `t`'s index and no-mouse flag stay. VERIFIED (asm). The
  `noop` line carries the index where the server id goes, so it reads
  `mr <index> noop`. INFERRED (the format string's one argument).
- A `t` whose index is outside 0..31, or whose configstring is empty,
  prints `Server tried to open a bad script menu index: %i` or `... a
  non-loaded script menu index: %i` through `0x30020890` and sends
  `cmd mr %i bad` with the index. VERIFIED (decompile). Like `noop`, the
  line has three words where the server reads four.
  INFERRED (the format).
- `u` (`0x3002de60`) calls syscall `0x7b` with `UIMENU_SCRIPT_POPUP` and
  again with `UIMENU_SCRIPT_POPUP_NO_MOUSE`, then empties the six script
  menu cvars. VERIFIED (decompile). CoDMP.exe's `0x7b` case
  (`0x4035d5`..`0x40367a`) sends the UI a down Esc (`vmMain` 3, `0x1b`,
  1) when `vmMain` 8 answers 10, either way. VERIFIED (asm). So `u`
  closes a script popup as Esc would, and nothing else.

### Esc and the fullscreen menus

`_UI_KeyEvent` (`0x4000d6f0`) takes an Esc ahead of `Menu_HandleKey`
(`0x4000d72d`): when `0x40016ab0` finds no menu on the stack with its
visible bit (`+0x48` & 4) and a non-zero `+0xbc`, it runs
`Menus_CloseAll` and returns. VERIFIED (asm). `+0xbc` is `fullscreen`:
`MenuParse_fullscreen` (`0x40018270`, the keyword table entry at
`0x40036940`) parses its int there. VERIFIED (asm, the table). No
script menu sets `fullscreen 1`, so Esc on one never runs its `onEsc`:
every menu closes through its `onClose`. `main` is `fullScreen
MENU_TRUE`, so over it Esc reaches the focused menu's `onEsc`. VERIFIED
(the files). Which of `ebx`/`edi` is the key and which the down flag is
INFERRED (the `0x1b` compare).

### `cl_bypassMouseInput` and the keys

`CL_KeyEvent` (`0x40dc30`) passes a press to the binds instead of the UI
while `cl_bypassMouseInput` is not 0 (`0x40dfd2` onwards): `MOUSE1`..
`MOUSE3` (200..202) only when it is 1; any other key when UI `vmMain` 14
(`0x418fb0` calls it; `0x4000a170`) answers 0. That answers 1 while
`0x401bf3f4` is set, else whether the focused menu has an `execKey` slot
(`+0xe8 + key * 4`) for the key; with no focused menu it answers 0 and
sets `cl_bypassMouseInput` back to 0. VERIFIED (decompile). Esc is taken
earlier (`0x40ddca`, section 13 above) and always reaches the UI.
INFERRED (control flow). So under the quick message menus only their
digit keys go to the menu and every other key, mouse buttons included,
reaches the binds. INFERRED (the combination).

### vcod

- The script menus load into the front end's `Ui` once the map is up
  (`Ui::load_script_menus` after the lists' menus; the cgame's start),
  again after a `vid_restart` or a mod switch in a live game, and are
  dropped with the rest by `ui_load`. A missing file prints `Could not
  load script menu file '<name>'` (no newline) as the game's drop: the
  connection ends and the main menu opens with the error popup, as at any
  drop. The first missing name is the one reported; retail's longjmp
  stops at it too. The drop also follows a `vid_restart` or a mod switch
  in a live game.
- `t` opens its menu through `Ui::open_script_popup`, the cases 10 and
  11 above: refused while a menu Esc, the main menu or the quick message
  menu opened has the focus, a no-op for the focused menu's own name,
  else the cursor moves, every menu closes and the menu opens. A refused
  `t` waits in `play::join::Join` and is offered again each frame; a `u`
  drops it. `u` is an Esc to the UI when the last open came from a `t`
  (`Ui::close_script_popup`). A refused `t` of another name replaces the
  waiting one and sends `mr <index> noop` for it; one of the same name
  changes nothing (`Join::wait`). A `t` naming no menu sends `mr <index>
  bad`. vcod-server hands either to the script as the menu `""` and the
  response `"bad"`, as any argument count but four reaches
  `Cmd_MenuResponse_f` (`parse_menu_response`, gsc language doc 6).
- `scriptMenuResponse` is `UiEffect::ScriptMenuResponse`; the app sends
  `mr <sv_serverid> <index> <response>` with the index looked up by name
  (`play::join::script_menu_response`), so `open` and `close` go out as
  retail sends them. The cvar tests read the server's `v` cvars and the
  140/204 mirror (`Ui::set_server_cvars`) before the console's.
  `WINDOW_STYLE_CVAR_SHADER` items paint their window with the shader
  their cvar names and nothing else (`viewmap`'s layout image), as
  `Item_Paint` (`0x40016470`) does: for style 6 it reads the cvar
  (display context `+0x68`) and registers it as the window's background
  (`+0xb4`, `0x4001674e`..`0x40016785`), paints the window
  (`0x4000f8f0`, which takes style 6 down style 3's shader path), and
  skips the per-type switch (the second `cmp [esi+0x34], 6` at
  `0x40016809`), so no text prints. VERIFIED (asm, decompile).
- A cvar shader naming no image draws nothing, in retail as in vcod.
  `RE_RegisterShaderNoMip` (CoDMP.exe `0x4fcae0`, refexport slot
  `0x14`) finds the shader as a 2D one (`0x4fc5c0` with -4) and returns
  0 when it came back as the default shader (flag 1 at `+0x4c`, set
  after `Couldn't find image for shader %s`). VERIFIED (decompile).
  `Window_Paint` (`0x4000f8f0`) takes style 6 to the stretch pic only
  when the background is not 0 (`cmp $0x6` at `0x4000fb18`, `test` of
  `+0xb4` at `0x4000fb23`, `je` past the draw). VERIFIED (asm). Style 3
  jumps to the same draw with no such test (`0x4000fa79`), and
  `RE_StretchPic` (`0x4ddd80`) takes handle 0 as `tr.shaders[0]`, so a
  style 3 window whose image is missing draws retail's default shader.
  INFERRED (handle 0 as the default shader, as in Q3). The default
  shader (`<default>`, built at CoDMP.exe `0x4fd780`) has one stage on
  the `*default` image (`0x16c4d58`, `R_CreateDefaultImage` `0x4f0380`)
  and no blend bits set. VERIFIED (decompile). INFERRED: it draws the
  image opaque whatever the stretch pic's colour. vcod draws a style 3
  window with no background, or whose image resolves to nothing, as
  that: the `*default` image with its alpha forced to 255, uncoloured
  (`hud::DEFAULT_SHADER`, `hud::default_if_missing`, `HudTex::Default`).
  Every other HUD quad with a missing image is still dropped. Of the
  stock 1.1 menus none hits the case: every style 3 window names an
  image the paks hold (`multi.menu`'s `menu_select` window, whose shader
  does not exist, is commented out). VERIFIED (pak scan, 2026-10-15).
- Esc in a live game with no menu up opens `g_scriptMainMenu`
  (`Ui::open_script_main`, case 2), whose Main Menu tab's `open main`
  brings up `main` with `cl_ingame` 1. When the server named no script menu
  vcod opens the in-game `main` directly; retail opens nothing there.
  M does what Esc does there; it is vcod's own.
- Esc while connecting or loading opens `main` with `cl_ingame` 0, as menu
  1 does.
- Esc with the console down in a game opens the same menus under it and
  leaves the console down. With no game up vcod still closes the console on
  Esc; retail reopens `main` under it.
- The mouse is released while a menu is up and captured again when Esc or
  Back to Game closes the last one. Usercmds keep going out with no keys
  held, and no bind fires while the menu has the keys.
- Any menu over a game takes the mouse unless `cl_bypassMouseInput` is
  1: the mouse is released, the view does not turn and the held keys let
  go. A `t` puts the cursor at the centre, or the bottom-right corner
  for `t <index> 1`. Esc and a menu's own `open` leave the cursor alone. With the bypass on (the quick message menus'
  `onOpen`) the mouse stays on the view, its buttons and wheel reach the
  binds, and of the keys only the focused menu's `execKey`s and Esc go to
  the menu (`Ui::wants_key`); vcod sets the bypass back to 0 whenever no
  menu is up. Closing the last menu captures the mouse again unless the
  console is down.
- With a menu up the UI draws the `assetGlobalDef` `cursor` shader 32
  units square and centred on the UI cursor, last, white (`Ui::build`).
  While a menu takes the mouse the OS pointer stays captured and hidden,
  as for the view, and its raw motion moves the UI cursor unscaled,
  clamped to 0..=640 and 0..=480 (`App::sync_capture`,
  `Ui::mouse_delta`); the OS pointer never moves it, so nothing has to
  warp and a `t` works on Wayland. With the console down in a window the
  pointer is let go and shows, and the UI cursor stays put; fullscreen
  keeps it captured and the motion still reaches the menus. Only where
  the platform refuses to capture does the OS pointer's position drive
  the cursor (`Ui::mouse_move`). winit's raw motion is the device's
  counts where retail read accelerated pointer pixels, so the speed can
  differ from retail's by the OS acceleration curve.

## 14. The options screens

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
  menu whose item it lands on. vcod sends clicks and mouse moves as
  section 20, "Which menu takes the mouse", reads them.

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

### How retail applies the Graphics, Performance and Sound pages

All CoDMP.exe 1.1 unless named. Which page writes which cvar is VERIFIED
from `ui/options_graphics.menu`, `ui/options_performance.menu`,
`ui/options_sound.menu`, `ui_mp/vid_restart.menu` and
`ui/snd_restart.menu` in `localized_english_pak0.pk3`: Graphics edits
`ui_r_*` shadows that the restart popup's Yes copies into `r_mode`,
`r_fullscreen`, `r_picmip2`, `r_picmip`, `r_texturemode`, `r_texturebits`
and `r_nv_fog_dist` before `vid_restart`; Sound edits `ui_mss_khz` and
`ui_mss_3d_provider`, copied into `mss_khz` and `mss_3d_provider` before
`snd_restart`; Performance writes `cg_marks`, `cg_brass`,
`r_dynamiclight`, `r_dlightQuality`, `r_swapinterval`, `cg_blood` and
`ai_corpsecount` directly, and `ui_lod` into `r_lodscale` / `r_lodbias` on
close. VERIFIED (the file): the Graphics page's lighting (`r_vertexlight`)
and geometric detail items are commented out, so no stock page offers
vertex lighting.

- Flags. VERIFIED, `R_Register` (0x4b3300): `r_picmip` "1", `r_picmip2`
  "2", `r_texturebits` "0" and `r_nv_fog_dist` "1" register 0x21 (archive
  and latch); `r_textureMode`, `r_dynamiclight`, `r_dlightQuality`,
  `r_swapInterval`, `r_lodbias`, `r_lodscale` and
  `r_ext_texture_filter_anisotropic` "0" register 1. VERIFIED, the Miles
  start-up (0x44fe60) registers `mss_khz` "44", `mss_bits`, `mss_stereo` and
  `mss_3d_provider` 0x21 and `mss_volume` 1. A latched value takes effect
  when its registration runs again: `vid_restart` re-runs `R_Register`,
  only `snd_restart` re-runs 0x44fe60 (INFERRED from the call order below).
- `r_picmip` / `r_picmip2`. VERIFIED, 0x4ea380: an image's picmip is 0
  unless its flags carry 2, else `r_picmip2` with flag 4 and `r_picmip`
  without, clamped to 0..3. VERIFIED, the stage `map` parser (0x4f3410)
  builds the image flags from the shader's: 4 from shader flag 0x200, 2
  unless shader flag 0x100, 1 (mipmapped) unless 0x80; the shader keywords
  set 0x80 for `nomipmaps`, 0x100 for `nopicmip` and 0x200 for `picmip2`
  (0x4fad13, 0x4fad32, 0x4fad51), so unlike Q3 `nomipmaps` leaves picmip
  on. VERIFIED, `R_FindShader` (0x4fc6f3): a shader looked up with
  lightmap index -1 starts with flag 0x200, so model shaders read
  `r_picmip2` ("Character Textures") and world ones `r_picmip` ("General
  Textures"). No stock shader uses `picmip2` or the per-map
  `picmip`/`nopicmip` tokens; HUD, menu and a few effect shaders say
  `nopicmip`. VERIFIED (shader scripts in `pak0`-`pak6`).
- How picmip shrinks an image. VERIFIED, 0x4ebf20 (uncompressed): the
  upload size is `width >> picmip` by `height >> picmip`, at least 1, and
  the image is halved by 0x4eb1d0 until it fits. VERIFIED, 0x4eb1d0's
  branch for the S3TC formats (0x83f0..0x83f3) returns the pointer past
  the current level (plus 0x4eaba0's size), so on the same 0x4ebf20 path
  a compressed image drops whole stored levels down to `width >> picmip`,
  at least 1. VERIFIED, 0x4ef590 drops levels only while both sides are 5
  or more; it is the texture-sheet path (images packed into a
  `*sheet%03i` image, 0x4ef8e0, through 0x4ef800). VERIFIED, 0x4ec730
  takes that path only for an image with flag 0x80 whose sides are both
  above 3 or whose format is `GL_RGBA`, and 0x4eeae0 clears 0x80 unless
  `r_optimize` ("1", 0x4b3300 block) and `r_optimizeTextures` ("2") are
  both non-zero. VERIFIED: the stage `map` parser's flag writes
  (0x4f342c..0x4f3639) set 1, 2, 4, 0x10, 0x20, 0x40 and 0x100 only and
  0x4f38c2 clears 0x80; the implicit-shader path passes 3 or 0x30
  (0x4fc82f..0x4fc83e). INFERRED: shader-stage and implicit images take
  0x4ebf20's path. VERIFIED: 0x80 is set at 0x4eee2d (in 0x4eed80, for an
  image without flag 0x100); which images reach it is not traced.
- `r_texturebits`. VERIFIED, 0x4ebe30, for an uncompressed image only (a
  compressed format returns at once): an image with any alpha byte below
  255 goes up as `GL_RGBA4` (0x8056) with 16, `GL_RGBA8` with 32 and
  generic `GL_RGBA` otherwise; an opaque one as `GL_RGB5` (0x8050),
  `GL_RGB8` or generic `GL_RGB`. Two branches by image flag (0x200: the
  desktop depth) are not traced to their images. INFERRED. VERIFIED,
  0x4ebffc: 0x4ebf20 calls 0x4ebe30 before its branch at 0x4ec004, so the
  internal format applies on the straight upload too. INFERRED: HUD and
  menu images take `r_texturebits` like world ones.
- The texture gamma on 2D images. INFERRED, from `cod11-gamma.md`
  section 3 (0x4eaa90 is called at 0x4ec124 on the resample and mip path
  only): a 2D image with flag 1 (a weapon icon, a `CG_RegisterGraphics`
  name, a scripted block without `nomipmaps`) or one picmip shrinks takes
  the texture gamma; one uploaded unmipped at its own size keeps its
  bytes. The rule is 0x4ebf20's, the same for every image.
- Compressed images without mipmaps. VERIFIED, 0x4ebdc0: a level goes up
  through `glCompressedTexImage2D` (pointer at 0x16c426c) for formats
  0x83f0..0x83f3 and `glTexImage2D` otherwise. VERIFIED: 0x4ebf20's level
  loop after the first upload (0x4ec193) runs only with flag 1. INFERRED:
  a DDS image without flag 1 (`nomipmaps`) goes up as level 0 alone,
  whatever levels the file carries, and samples `GL_LINEAR` (0x4ec07e).
  VERIFIED, 0x4eeae0: an image with flag 1 whose file has no levels
  (0x4edf20's out-flag) prints `image '%s' requested mipmaps, but they
  aren't available` and fails.
- `r_textureMode`. VERIFIED, `GL_TextureMode` (0x4ea530): the name is
  looked up without case in the six-row table at 0x5712f8 (`GL_NEAREST`,
  `GL_LINEAR`, the four `GL_*_MIPMAP_*`), each row a min and a mag filter;
  a miss prints `bad filter name` and changes nothing; a hit sets both on
  every image with flag 1. VERIFIED, 0x4de1db..0x4de1f8: the frame start
  calls it whenever the cvar's modified flag is set, so a change is live.
  VERIFIED, 0x4b2a40 (Q3's `GL_SetDefaultState` by its shape) calls it for
  each texture unit, so every renderer start applies it too.
  VERIFIED, 0x50abf0: `GLimp_Init` resets it to
  `GL_LINEAR_MIPMAP_NEAREST` when the renderer string changed since the
  last run.
- Mipmaps. INFERRED, 0x4ebf20's control flow: an image with flag 1
  uploads level 0, then, while either side is above 1, builds the next
  level in place through 0x4eb1d0 and uploads it; without flag 1 only
  level 0 goes up. INFERRED, the branch at 0x4ec07e: a flag-1 image's min
  and mag filters are the current `GL_TextureMode` pair (0x5712f0,
  0x5712f4), any other image's `GL_LINEAR` (0x2601) both.
- Mip filter cvars. VERIFIED, `R_Register` (0x4b3300): `r_simpleMipMaps`
  "1" flags 0x21, `r_weightMipMaps` "0" flags 0x20, `r_colorMipLevels`
  "0" flags 0x20.
- Mip filter. INFERRED, 0x4eb1d0's branches for `GL_RGBA`:
  `r_simpleMipMaps` 0 calls 0x4eac90 (Q3's `R_MipMap2` by its place), and
  otherwise `r_weightMipMaps` at or below 0 or above 1 takes the plain
  branch, so the defaults take it. VERIFIED, that branch's arithmetic: each
  channel of a next-level texel is the four bytes of its 2x2 block summed
  and shifted right by 2, truncating; when a side halves to 0 it averages
  consecutive texel pairs instead, summed and shifted right by 1. Picmip
  halving (above) calls the same 0x4eb1d0, so it truncates too.
- The weighted branch. VERIFIED, 0x4eb3a0..0x4eb3d1: it reads
  `r_weightMipMaps`'s float and takes the plain branch when it is at or
  below 0.0 (0x568e64) or above 1.0 (0x568e60). VERIFIED, 0x4eb3d9..0x4eb446:
  otherwise `k = (1.0 - w) * 0.25` (0x568ed0) is stored as a float, and each
  channel is `_ftol` (0x538be0, truncating) of `sum * k + top_left * w`, where
  `sum` is the 2x2 block's four bytes and `top_left` the block's first.
  INFERRED from the branch order: a side that halves to 0 averages pairs
  as the plain branch does.
- `r_simpleMipMaps 0`. VERIFIED, 0x4eb32f: the cvar's integer at 0 sends
  the level to 0x4eac90. VERIFIED, 0x4eac90's arithmetic: each texel of a
  `(w >> 1)` by `(h >> 1)` level is the 4x4 block from row and column
  `2i - 1` to `2i + 2`, weighted 1 2 2 1 by 1 2 2 1, coordinates masked by
  `w - 1` and `h - 1` (so they wrap), summed and divided by 36 unsigned;
  the result is copied over the input. INFERRED: with a side of 1 the level
  has no texels to write, so the copy is empty and the next level is the
  leading texels of the one above.
- `r_colorMipLevels`. VERIFIED, 0x4ebf20's mip loop calls 0x4ebd60 for
  each level while the cvar is non-zero; INFERRED from 0x4ebb90 (a blend
  of every texel toward a colour by its alpha) that it tints levels for
  debugging. vcod does not register it.
- Address modes. VERIFIED, 0x4ec490: after the upload it sets
  `GL_TEXTURE_WRAP_S` (0x2802) to `GL_CLAMP_TO_EDGE` (0x812f) when image
  flag 0x10 is set, else `GL_REPEAT` (0x2901), and `GL_TEXTURE_WRAP_T`
  (0x2803) by flag 0x20 the same way.
- The sky box. VERIFIED, `skyParms` (0x4fa320): each of the six far-box
  and six near-box sides loads `%s_%s.tga` through 0x4eeae0 with flags
  0x33 (pushes at 0x4fa3cd and 0x4fa52a), so sky images are mipmapped,
  take `r_picmip` and clamp whatever the block says.
- The sun. VERIFIED, 0x4e7930 (the sprite, `r_sunsprite_shader`) and
  0x4e7be0 (the flare, `r_sunflare_shader`, pushes at 0x4e7c34..0x4e7c38): both
  register a shader with lightmap index -3, the raw-image mip argument 0
  and image type 4 (0x4e7933..0x4e7937). INFERRED, 0x4fc5c0: the mip
  argument picks only an implicit shader's flags, so a scripted one takes
  its stage flags from its keywords. VERIFIED, `pak4.pk3 scripts/sun.shader`:
  all eight blocks say `nomipmaps`, `nopicmip` and `map clamp`, so the
  stock sun disc and flares sample `GL_LINEAR`, unmipped.
- 2D registrations. VERIFIED, CoDMP.exe's cgame syscall table
  (0x401df0, jump table 0x4041f0): trap 0x30 goes to 0x402b99, which calls
  refexport +0x10 (`RE_RegisterShader`, mipmapped), and trap 0x58 to
  0x402bd9, which calls +0x14 (`RE_RegisterShaderNoMip`). VERIFIED, cgame
  0x30030d81: the loading-screen wrapper 0x30030b70 ends in trap 0x30.
  VERIFIED (cgame decompilation): `CG_RegisterGraphics` (0x30020da0)
  registers through 0x30030b70 `gfx/2d/net.tga`, `gfx/2d/select`,
  `gfx/2d/backtile`, `gfx/hud/hud@objective_friendly.tga` and its `_chat`
  twin, and every scripted HUD material it names (stances, hints, kill
  icons, objectives); `CG_RegisterWeapon` (0x30034cf0) registers each
  weapon's `hudIcon` (+0x18c), `modeIcon` (+0x190), `ammoIcon` (+0x194)
  and `killIcon` (+0x2e0) the same way; configstrings 29..43 (head icons)
  too. Trap 0x58 takes `black`, `white`, the scoreboard arrows,
  levelshots, configstrings 21..28 (status icons) and 1501 on (script
  `precacheShader`). So an implicit 2D image, which takes its flags from
  the trap (flags 3, or 0x30 for `NoMip`, above), is mipmapped,
  picmipped and repeating when it is a weapon icon or one of the five
  `CG_RegisterGraphics` names, and unmipped, unpicmipped and clamped
  otherwise; a scripted 2D shader is mipmapped unless it says
  `nomipmaps`. VERIFIED (cgame decompilation): `CG_RegisterWeapon`
  returns at once for index 0 and for a weapon already registered
  (0x301a69dc), and is called with an index into configstring 7's table
  from the item walk 0x30036080 (an item of type 1 registers its weapon,
  0x30036132), which 0x30036150 runs over configstring 8's bits, and from
  0x30036cf0 (0x30036d71) and 0x300371f0 (0x30037289), which take a
  player's and the playerstate's weapon. INFERRED (by their shape): the
  last two are the third-person and view weapon draws. So every mipmapped
  weapon icon belongs to a weapon of configstring 7; one not marked in
  configstring 8 registers when first drawn. VERIFIED (shader scripts in `pak4`): the 2D blocks without
  `nomipmaps` are the four head icons (`hud.shader`), `lagometer` and the
  rest of `gfx.shader`, and `ui/assets/hudbar`, `hudbarlarge`,
  `hudbarmedium` and `sprites/destroy` (`ui_hud.shader`).
- Custom mip levels. VERIFIED, 0x4eb1d0's string use: it searches the
  image's name for `#` (0x5611b4), reads the digits after it and prints the
  number plus one and the rest of the name back with `"%i%s"` (0x54bf78),
  then loads that name through 0x4edf20, which builds `.dds`, `.tga` and
  `.jpg` names in place over a 4-character extension. VERIFIED (0x4edf20
  itself): a name longer than four characters whose fourth-last is `.`
  loses those four, then `.dds`, `.tga` and `.jpg` are tried in that order
  whatever extension the name gave. VERIFIED: 0x4eeae0, the image lookup
  that prints `image '%s' requested mipmaps, but they aren't available`,
  loads a 2D (`0xde1`) image it has not cached through it. INFERRED: so a script's `.tga` name loads a `.dds` of the
  same base when one exists (`assets::resolve_bundle_image`). VERIFIED,
  0x4eb1d0: the `"%i%s"` print writes over the name's digits in place
  (the name is 0x4ebf20's local copy, shared by its picmip and mip
  loops) before the load, so the next level looks for the number after
  whether this one was found or not. VERIFIED, the branches after the
  load: an image found at `(w + 1) >> 1` by `(h + 1) >> 1` is copied in
  as the level; found at another size it prints `WARNING: custom mipmap
  image '%s' should have dimensions %i x %i instead of %i x %i, refusing
  to load` and returns 0; not found, the box filter runs. VERIFIED, the
  refusal's path up: 0x4ebf20 returns 0 on a 0 from either loop, so does
  0x4ec490, and 0x4ec730 then frees the image (0x4ec430) and returns 0;
  0x4eeae0 hands that 0 to the shader stage, which prints `Couldn't load
  image '%s' in shader %s` and fails (0x4f3410), and a deferred load
  (0x4ee8a0) copies the `*default` image (0x16c4d58) over the image
  instead. INFERRED: so on 1.1 the whole image draws as the default
  image or its shader as `<default>`. VERIFIED: 1.5's CoDMP.exe
  (0x50ba60) has the same refusal; United Offensive's CoDUOMP.exe
  (0x505520) prints the warning and falls through to the filter for
  that level, with the name stepped on as 1.1 steps it. VERIFIED (pak listings): 158 stock files carry
  a `#` (foliage and wood skins, wire and fence textures, `...#0` with
  `#1`, `#2` and up beside them), each level half the size of the one
  above. VERIFIED (pak listings): United Offensive's `uo/pakuo01.pk3`
  ships `skins/foliage_masked@bushwallfill#01`, `bushwallsparse#01`,
  `deadtreehoriz#01` and `deadtreevert#01` at 1.1's level 0 size, and
  their next name `#2` is 1.1's `#2` in `main/pak3.pk3`, a quarter of the
  size: UO warns on each and filters all four whole chains, since level
  2 looks for `#3`. No 1.1 or 1.5 stock image reaches the refusal.
  INFERRED: the level is copied as loaded, so the texture gamma
  (`cod11-gamma.md` section 3) reaches level 0 and the levels filtered
  from it only.
- Model skins. VERIFIED: the xmodel surface registration (CoDMP.exe
  0x504200's loop) formats `skins/%s` with the surface's skin name and
  calls `R_FindShader` (0x4fc5c0) with -1, 1 and 7. VERIFIED: 0x4fc5c0
  copies the name up to its first `.`, and with no `@` in it and no
  script of that name, copies the full name, gives it `.tga`
  (0x44a5f0, 0x546a94) when it has no extension, and loads it through
  0x4eeae0 as a 2D (`0xde1`) image; with an `@` it passes the
  extensionless name to 0x4fc0c0, whose template stages load through
  0x4eed80, which calls 0x4eeae0 too. INFERRED: so a skin loads like
  any other image, `.dds` first over the extension the model gives
  (`assets::load_skin_image`). VERIFIED (pak listings): no stock skin
  ships under two extensions in the 1.1 paks or in UO's `main/` and
  `uo/` together, so the probe changes only third-party models.
- Effect images. VERIFIED: the efx parser's `shaders` field (0x49d8b0
  compares the name) goes to 0x49c8b0, which calls `[0x1432870]` with
  the name and 9 (0x49c8db). VERIFIED: 0x1432860 is the client's
  refexport copy, 0x39 dwords filled by `rep movs` at 0x411ac5, and the
  table 0x4b4a6a fills at 0xca26c8 holds 0x4fca80 (`RE_RegisterShader`)
  at +0x10 (the store at 0x4b4aa6) and 0x4fcae0
  (`RE_RegisterShaderNoMip`) at +0x14. VERIFIED: 0x4fca80 passes
  lightmap index -4 and 1 to 0x4fc5c0, 0x4fcae0 passes -4 and 0.
  INFERRED, 0x4fc5c0's implicit-shader branch: that argument picks image
  flags 3 (mipmapped, picmipped) or 0x30 (clamped, neither), and index -4
  leaves the `picmip2` shader flag clear. So an effect image is mipmapped
  and takes `r_picmip` unless its shader says `nomipmaps` or `nopicmip`.
- Anisotropy. VERIFIED, `GLW_InitExtensions` (0x509260, its last test,
  which prints `...ignoring GL_EXT_texture_filter_anisotropic` on one
  branch): both branches set `r_ext_texture_filter_anisotropic` to "0",
  and no stock page writes it.
- `r_dynamiclight`, `r_dlightQuality`: read every frame, light grid doc
  ("r_dynamiclight") and shader doc 3.
- `r_swapInterval`. VERIFIED, `GLimp_EndFrame` (0x50a610): when the
  modified flag is set it calls `wglSwapIntervalEXT` with the value, so it
  is live.
- `cg_blood`. VERIFIED, cgame 0x30039679: `CG_BulletHitWall` reads it
  (row at 0x30075430, storage 0x30298120); 0 swaps a flesh hit's effect,
  events doc, the surface type table. `cg_brass`. VERIFIED, cgame
  0x30038a30: the brass function returns at once with it 0.
- `mss_khz`. VERIFIED, 0x44ba30: 11, 22 and 44 open Miles' output at
  11025, 22050 and 44100 Hz; any other value prints `invalid value %i for
  mss_khz, using 22 khz instead` and takes 22050.
- `snd_restart`. VERIFIED, 0x40fdf0: refused with `Listen server cannot
  sound restart.` (0x5672f0) while `sv_running`, else 0x40fe0c, which
  fills a buffer through 0x4510f0, then calls 0x450130, the Miles start-up
  0x44fe60, `vid_restart` (0x40fbe0) and 0x451330 with that buffer.
  INFERRED from the order: the playing sounds saved, Miles shut down and
  started, the video restart, the sounds restored. The stream half is
  VERIFIED in the sound doc, section 10 ("Mixer rate and `snd_restart`"):
  streams resume at their position, the ambient is not saved.
- `configure_mp.csv` (`pak0.pk3`). VERIFIED, the string: 0x436d40 reads
  it. INFERRED, Q3's auto-config: it runs at the first start. VERIFIED,
  the file: per CPU MHz, system and video MB it lists `r_picmip`,
  `r_picmip2`, `r_mode`, `r_texturebits`, `r_dynamiclight`,
  `r_dlightQuality`, `r_lodscale`, `r_lodbias`, `r_swapInterval`,
  `r_textureMode`, `cg_brass` and `mss_khz`, and its top row (2000 MHz,
  256 MB, 256 MB) is `r_picmip` 0, `r_picmip2` 0, `r_swapInterval` 1,
  bilinear, `mss_khz` 44.

### What vcod backs

- Live: every bind, `sensitivity`, invert mouse (`m_pitch`), `name` (on
  closing Multiplayer Options), `rate`, `mss_volume` (the master volume,
  read every frame), and `r_mode` / `r_fullscreen`, which `vid_restart`
  and start-up apply to the window (section 21), and `r_gamma`, read every frame and applied as retail's gamma
  ramp in a final pass, and `r_ignorehwgamma`, latched at start-up and
  `vid_restart`, which moves `r_gamma` into the TGA and JPG textures at
  each map load (`cod11-gamma.md` section 4). `cg_drawCrosshair` and
  `cg_drawStatus` gate the HUD as the cgame does (`cod11-hud-protocol.md`,
  "Which views draw it"). vcod registers `r_mode` -1 (its own window size) and
  `r_fullscreen` 0 instead of retail's 3 and 1.
- The image settings (`crate::texture_settings`): `r_picmip`, `r_picmip2`
  and `r_texturebits` latch to `vid_restart`, which rebuilds the renderer
  when any of them (or `vc_hdr`) moved. World surfaces, world shader stage
  images, marks, the sky and the sun flare drop `r_picmip` levels, prop,
  player, weapon and viewmodel skins and entity shader stages
  `r_picmip2`, unless the shader says `nopicmip`; a path two shaders share
  keeps the first one's levels, as retail's image cache does. HUD and menu
  images take the 2D registration rules above: a weapon icon or a
  `CG_RegisterGraphics` image is mipmapped, takes `r_picmip` and repeats,
  a scripted shader follows its keywords and its first stage's
  `clampmap`, and any other image is unmipped and clamped. vcod's weapon
  icons are those of configstring 7's table at each gamestate (and none
  without one); it registers all of them up front where retail registers
  a weapon at its item or its first draw, so an icon some other path
  registers first (a `precacheShader`) is mipmapped where retail's image
  cache may keep it unmipped. A changed table or texture gamma reloads
  the HUD images. `r_texturebits 16` quantizes every uncompressed image,
  HUD and menu ones included, to 4:4:4:4 or 5:5:5 after the texture
  gamma, rounding to nearest (the driver's rounding is not in the
  binary). The texture gamma (`r_ignorehwgamma 1`) reaches an image only
  when it is mipmapped or picmip shrank it, as 0x4ebf20 above. `r_textureMode`
  picks the filters of two shared samplers (repeat and clamp) that every
  world, model, viewmodel, effect, sky box and sun disc image with
  mipmaps draws through; an image without them (one level) draws through a
  fixed `GL_LINEAR` pair beside them, picked in the shader by its level
  count. Images bind no sampler of their own, so a change seen at a frame
  start swaps the samplers and rebuilds the camera, viewmodel and HUD bind
  groups that hold them, in place as retail does. vcod's 16x anisotropy stays on only under
  `GL_LINEAR_MIPMAP_LINEAR`, the one mode wgpu allows it with; retail has
  none. Uncompressed images upload with retail's mip chain (the truncating
  box and the `#N` levels above) unless their shader says `nomipmaps`;
  a DDS image without mipmaps uploads its level 0 alone, so it samples
  `GL_LINEAR` like retail's. A compressed image's picmip stops while a
  side is 4 or less, the sheet path's rule: wgpu needs whole 4x4 blocks
  in level 0, where retail's plain path would go down to `width >>
  picmip`. A
  `#N` level of the wrong size falls back to the box as United
  Offensive's does (logged at debug, since stock UO skins hit it); 1.1
  and 1.5 fail the image there, which vcod does not port: no stock 1.1 or
  1.5 image reaches it. `r_simpleMipMaps 0` and `r_weightMipMaps` in (0, 1] swap the
  box for the kernel and the weighted step above, latched to
  `vid_restart`; `r_colorMipLevels` is not ported. Effect images take
  `r_picmip` and the chain like world ones. The sky box takes flags 0x33
  whatever its block says; the sun disc and flare follow their own
  blocks. The sun flare samples `GL_LINEAR` when unmipped (every stock
  flare) and through `r_textureMode`'s clamp sampler when a mod's block
  leaves it mipmapped.
- `r_dynamiclight` (0 none, 1 world and models, other models only) and
  `r_dlightQuality` (0 projects the dlight on the model's axes) are read
  every frame, `r_swapInterval` too (vsync on or off), `cg_blood` swaps
  the flesh hit for `flesh_hit_noblood.efx`, and `cg_marks` gates impact
  marks.
- `mss_khz` latches to `snd_restart`, which stops every voice, restarts
  the ambient, decodes sounds again and runs `vid_restart`. kira mixes at
  the device's rate, so vcod resamples each file above `mss_khz`'s rate
  down to it, linearly, at decode, and each stream (music, ambients,
  dialog) as it plays, since retail's streams mix through the same driver
  (sound doc, section 10). The ambient and the menu music start over as in
  retail; a dialog or quick-chat line playing at the restart ends instead
  of resuming.
- vcod registers `r_picmip` 0, `r_picmip2` 0 and `r_swapInterval` 1, the
  top row of `configure_mp.csv`, instead of the registered 1, 2 and 0. An
  older `vcod_mp.cfg` may still carry 1 and 2.
- `r_lodscale` / `r_lodbias` (Model Detail) feed the xmodel LOD pick
  every frame, as `RE_RenderScene` reads them (`xmodel-v14-format.md`,
  "LOD selection").
- `cl_freelook` and `m_filter` drive the mouse as `CL_MouseMove` reads
  them, with `+mlook` and `+strafe` (`cod11-console.md` section 5).
- Stored but inert (archived, so a choice survives): `r_nv_fog_dist`,
  `mss_3d_provider`, and the Multiplayer Options' Show Compass
  (`cg_drawCompass`) and Team Overlay (`cg_drawTeamOverlay`), which no
  retail binary reads either (`cod11-hud-protocol.md`, "Which views draw
  it").
  `setRecommended` is an unknown command. Binds to commands vcod lacks
  (`+lookup`, `screenshotJPEG`, the stance toggles) are stored
  and print `Unknown command` when pressed.
- Not shown: the language picker (`cl_languagesavailable` reads 1, so its
  `hideCvar` hides it), NVIDIA fog (no `r_nv_fog_available`), Driver Info
  (no `developer`). The CD key button is refused. `cl_language` itself
  latches to `vid_restart` and picks the menu files' language directory
  (section 1), but only the English localized paks ship with a stock
  install.

## 15. The error popup's text

`error_popmenu` prints cvar `com_errorMessage`. How a drop reason gets
there:

- `Com_Error` is `0x435ad0`. VERIFIED: one path calls
  `SEH_LocalizeTextMessage` (`0x4aa040`) on the formatted message with the
  label `"error message"` and copies the result back; the other calls
  `0x435a40`. INFERRED, off its compares on the code: codes 3, 4 and 5 take
  the first path, any other code the second.
- `0x435a40`: VERIFIED, it registers `com_errorMessage` and passes the
  message to `0x4aa040` with the label at `0x562e84` and a last argument of
  1. INFERRED, off the null test after that call: `com_errorMessage` gets
  the result, or the raw message when the result is null. INFERRED, off
  `0x4a9f80`: with that last argument 1, a part the table lacks makes the
  call return null, so a message that is not a key, or holds one unknown
  key, shows as sent.
- The `w` server command (case `0x77` of the server-command switch,
  `0x40168b`). VERIFIED: one branch pushes `0x568c0c`
  (`"EXE_SERVER_DISCONNECTED"`) and code 2 to `Com_Error`; the other passes
  the first argument and `0x568c24` (`"EXE_SERVERDISCONNECTREASON"`) each
  through the exe-string translator `0x4a9e50`, formats with `0x44ace0`
  (`va`) and calls `Com_Error` with code 2. INFERRED, off the compare on the
  argument count: a bare `w` takes the first branch. INFERRED, off
  `0x4a9e50`: a key the table lacks comes back as itself when `loc_warnings`
  is 0. VERIFIED, `localizedstrings/english/exe.str`:
  `SERVERDISCONNECTREASON` is "Server Disconnected - %s" and
  `SERVER_DISCONNECTED` "Server Disconnected". So `w "EXE_TIMEDOUT"` reads
  "Server Disconnected - Timed out", and a plain-text reason is filled in
  as sent.
- An OOB `error`. VERIFIED (`0x410df4`..`0x410e5d`; `0x561c08` is
  `"error"`, `0x566e58` `"server error"`): the handler passes the packet's
  text to `0x4aa040` with the label `"server error"` and 0, then calls
  `Com_Error(1, "%s", result)`. INFERRED, off the tests ahead of it: only
  while connecting or connected, and only from the server's address. The
  text opens on a key, so `GAME_INVALIDPASSWORD` reads "Invalid Password.".
  VERIFIED, cod_lnxded's bytes: the version refusal is
  `error\nEXE_SERVER_IS_DIFFERENT_VER\x15%s\n`, whose version is a literal
  part.
- An OOB `disconnect`. VERIFIED (`0x410620`..`0x410676`): the handler
  calls `Com_Error(1, "EXE_SERVER_DISCONNECTED")`. INFERRED, off its
  compares: only while the client state at `0x155f2c0` is not 0, from the
  server's address, and when the clock at `0x155f3e0` is at least 3000 ms
  past the stamp at `0x15ce868`. What writes the stamp is in the handshake
  doc, "Out-of-band `disconnect`".

vcod: `NetClient` emits `Dropped` with the reason in that form (`w <arg>`
becomes `EXE_SERVERDISCONNECTREASON\x14<arg>`, which
`Localized::message` resolves the same way), and the client runs
`Localized::message` over every reason before the popup, so its own English
reasons pass through. vcod honours an OOB `disconnect` under the same 3 s
guard (handshake doc, "Out-of-band `disconnect`"). Where a message mixes a known key with an unknown one,
vcod shows the known part translated; retail shows the whole raw. vcod-server sends `EXE_SERVER_IS_DIFFERENT_VER 1.1` with a
space, not `\x15`, so that reason shows unlocalized.

## 16. Modded servers

What a public 1.1 server sends a joining client beyond the stock set, and
what the client does with it. Sources: `CoDMP.exe` 1.1, the cgame for the
HUD side (`cod11-hud-protocol.md` section 9, "Mod items"), and a survey of
`codmaster.activision.com:20510` on 2026-10-09: `getservers 1 full empty`
listed 49 servers, all of which answered `getstatus`; vcod joined 13 of
them as a spectator for 10-25 s each.

### What the survey found

- 36 of 49 serverinfos carry `codextended` (CoDExtended v20/v21), 6 carry
  `iw1x`, the rest neither. `mod` reads 1 on three. Eleven carry
  `psv_powerserver`. VERIFIED (`getstatus` replies).
- Mods ride `main/zzz_*.pk3` and similar names in `sv_referencedPakNames`:
  CoDaM (`___CoDaM__CoD1.1__`, `__CoDaM_PowerServer`, ...), MiscMod, a
  fun mod (`zzz_zfunmod`: gametype scripts, `ui_mp/hud.menu`,
  `mp/playeranim.script`), a weapon menu (`zzz_nexus_weaponmenu_v2`: one
  script menu, `nexus_weapons`), zombies, jump and gungame maps. VERIFIED
  (systeminfos and the downloaded paks).
- Two of the 13 put `fs_game` in the systeminfo and name
  their paks `<fs_game>/<pak>`; the second ships a replacement front end
  (`ui_mp/main.menu`, `joinserver.menu` and twelve more). VERIFIED.
- Every one of the 13 systeminfos carries `cl_allowDownload`: 0 on six
  (three servers of one host, the first `fs_game` server above, an iw1x
  jump server, a pure LAN-party server), 1 on the rest. A server with it 0
  answered vcod's `download` with `EXE_AUTODL_SERVERDISABLED<pak>` and
  dropped it. VERIFIED (live).
- Two set `sv_wwwBaseURL` (HTTP downloads for patched clients); the stock
  1.1 client has no HTTP download and fetches over UDP. VERIFIED (no
  `sv_wwwBaseURL` string in `CoDMP.exe`).
- UDP downloads ran at 25-40 KB/s, about the `rate` 25000 the client
  sends. The busiest server references 20 non-stock paks. VERIFIED (live).
- Localized-string configstrings (1245..) on modded servers hold literal
  text (`"Press ^1FIRE ^7to vote"`), not table keys; the client prints a
  key the table lacks as itself. VERIFIED (configstrings).
- Two CoDaM servers had `ET_ITEM` entities with `index` 68, the
  `item_health` row of `bg_itemlist` (`cod11-items.md`), which stock maps
  never place: mod health drops. vcod draws rows 65-69 with their world
  models. VERIFIED (snapshots, two public servers).
- One server answered `connect` with two OOB `print`s
  (an advertising URL) and sent `connectResponse` 4 s after the
  challenge. VERIFIED (live).

### The systeminfo sets client cvars

- `CL_SystemInfoChanged` (`0x415eb0`) reads `sv_serverid`, then returns
  while a demo plays (`clc.demoplaying`, `0x15ef004`). VERIFIED (the
  compare). Otherwise a systeminfo `sv_cheats` whose `atol` is 0 (a
  missing key included) runs `Cvar_SetCheatState` (`0x439bd0`), which
  puts every `CVAR_CHEAT` (0x200) cvar off its reset string back on it.
  VERIFIED. The pure and referenced pak lists go to the filesystem next.
- Then, when `sv_running` is 0, the function walks the systeminfo with an
  inline `Info_NextPair` (one leading `\` skipped per pair, an empty key
  ends the walk, a key with no value reads `""`) and calls `Cvar_Set2`
  (`0x439650`) on each pair with `force` 1 (`mov eax, 1` at `0x415ff9`).
  VERIFIED (asm). `force` skips `Cvar_Set2`'s read-only (0x40), write
  protect (0x10), cheat (0x200, against `sv_cheats`) and latch (0x20)
  branches and drops a pending latched string; a cvar the client never
  registered is made with flags 0, not `CVAR_USER_CREATED` (0x80). VERIFIED
  (`0x439650`'s body). The set ORs the cvar's flags into
  `cvar_modifiedFlags` (`0x163b40c`), so a forced userinfo cvar goes back to
  the server and an archived one into the config. VERIFIED (the OR);
  INFERRED (the readers, Q3's). No key is refused: `fs_game`,
  `cl_allowDownload`, `rate`, `snaps`, `sv_serverid`, `sv_paks` and the
  rest all become client cvars.
- 1.5 and UO keep the function unchanged: CoDMP.exe 1.5 `0x4176f0` (its
  `Cvar_Set2` `0x43bbb0`, `mov eax, 1` at `0x417839`), CoDUOMP.exe 1.51
  `0x418b80` (`Cvar_Set2` `0x43dc50` takes `force` as a third argument,
  `push 0x1` at `0x418cad`). VERIFIED (asm).
- `sv_cheats` is registered `"0"`, flags 0x48 (`CVAR_SYSTEMINFO |
  CVAR_ROM`), in `Com_Init` (call at `0x437636`), so only a systeminfo moves
  it. VERIFIED.
- `fs_game` is registered with flags 0x18 (`CVAR_SYSTEMINFO | CVAR_INIT`)
  at `0x42cdb0`. VERIFIED. After `CL_SystemInfoChanged`, the gamestate
  parse (`0x416050`) restarts the filesystem (`0x42d2b0`) when no local
  server runs and `fs_game` was modified or the checksum feed changed, then
  runs `CL_InitDownloads` (`0x410240`). INFERRED (call order).
- `CL_Disconnect` (`0x40ef90`) does not touch `fs_game`, and the systeminfo
  only sets keys it carries, so retail stays in a mod until another server
  names one. INFERRED (its body).
- `cl_allowDownload` is registered "0" with flag 1 (archive) at
  `0x412152`. VERIFIED. `CL_InitDownloads`, with `sv_running` 0 and
  `cl_allowDownload` set, has `FS_ComparePaks` (`0x43b830`) list the
  missing paks and starts the first; otherwise it calls `FS_ComparePaks`
  for the names only and prints `WARNING: You are missing some files
  referenced by the server:\n%sYou might not be able to join the game\nGo
  to the settings menu to turn on autodownload, or get the file
  elsewhere\n\n` (string `0x5670b9`, its xref at `0x41039b`), then goes on
  to load. VERIFIED (string, xref); INFERRED (branches).
- `FS_ComparePaks` compares checksums, not names: a local pak with the
  right name and the wrong checksum is listed with ` (local file exists
  with wrong checksum)`, and the download is saved as `%s.%08x.pk3`.
  VERIFIED (strings in the function).
- No stock menu sets `cl_allowDownload`; `configure_mp.cfg` and
  `safemode_mp.cfg` in `pak0.pk3` set it "0". VERIFIED.

### vcod

- Every systeminfo key is forced into the client's cvars at the gamestate
  and at each `d 1`, as retail does, except during a demo
  (`Shell::apply_systeminfo`). vcod's client never runs a server in its
  own process, so the `sv_running` test always passes. A server's
  `cl_allowDownload 0` therefore lands in the client's archived cvar and
  stays after it leaves, as on retail; vcod's own default is 1. With
  downloads off it logs the missing paks and loads the map if it resolves.
- The search path follows the `fs_game` cvar, registered `""` and
  write-protected (`CVAR_INIT`) as retail's: at the gamestate the value
  the systeminfo forced layers `<game_dir>/<fs_game>` over the base
  directory (`Pk3Fs::open_layered`), before the download check; downloads
  land in either directory and never overwrite a file. A disconnect
  leaves the cvar, so the server's mod stays after it, as on retail; only
  the pure list is dropped.
- Paks are matched by checksum, with retail's
  `<name>.%08x.pk3` file name when a same-named pak is in the way, and a
  pure server's `sv_paks` limits the search path
  (docs/research/cod11-server-handshake.md, "Pak checksums and pure
  servers").
- A server's `fs_game` rebuilds the front end off the new search path at
  the gamestate, so a mod's replacement menus are what Esc opens in that
  game, and a mod's `ui_mp/menus.txt` decides which files load.
- Script menus run the front end's preprocessor (`#include` read off the
  search path, object-like `#define`s), so `visible MENU_TRUE` and
  `#define`d responses resolve. Function-like macros are not expanded.

## 17. The Mods menu

Sources: `ui_mp/mods.menu` and `ui_mp/main.menu` in `pak0.pk3`,
`ui_mp_x86.dll` 1.1 (base `0x40000000`) and `CoDMP.exe` 1.1. Each claim
carries its label.

### The menu

- `mods.menu` defines `mods_menu`: `onOpen { hide grpfinish; hide accept;
  uiScript loadMods }`, `onEsc { close mods_menu; open main }`, a list box
  `modlist` (`feeder FEEDER_MODS`, 9 in `menudef.h`; `elementheight 20`;
  `action { show accept }`; `doubleClick { play "mouse_click"; uiScript
  RunMod }`) and a hidden `@MENU_LAUNCH` button `accept` whose action is
  `uiScript RunMod`. VERIFIED (asset).
- `main.menu`'s `@MENU_MODS` button runs `open mods_menu; close
  options_multi` and has no `cvarTest`, so it shows in a game too. VERIFIED
  (asset).
- `UI_RunMenuScript` (`0x4000a4a0`): `LoadMods` (`0x4000ab0a`) calls
  `UI_LoadMods` (`0x40009e10`); `RunMod` (`0x4000ab6a`) sets `fs_game` to the
  selected entry's directory and appends `vid_restart;`; `Quake3`
  (`0x4000abca`) sets `fs_game` to `""` and appends `vid_restart;`. No stock
  menu calls `Quake3`. VERIFIED (asm).
- `UI_LoadMods` asks `trap_FS_GetFileList("$modlist", "", buf, 0x800)` and
  keeps at most 64 `name\0description\0` pairs. The feeder's text is the
  description when it is not empty, else the directory. VERIFIED (asm).

### The engine

- `FS_GetModList` (`0x43b030`, reached through `$modlist` at `0x5644d8`):
  the subdirectories of `fs_basepath` and `fs_cdpath`, without duplicates,
  `main` and `.`, that hold at least one `.pk3` in the base, CD or home
  path; the description is the first 48 bytes of `<dir>/description.txt`,
  else the directory name. VERIFIED (strings and the `fread` size);
  INFERRED (the loop).
- `fs_game` is `CVAR_SYSTEMINFO | CVAR_INIT` (0x18) everywhere it is
  registered. A console `set` refuses it (`%s is write protected.`), but
  the UI's `trap_Cvar_Set` and `CL_SystemInfoChanged` pass `force`.
  VERIFIED (asm).
- `vid_restart` (`CL_Vid_Restart_f`, `0x40fbe0`) refuses with `Listen
  server cannot video restart.` while a local server runs, shuts the cgame
  and the UI module down, restarts the filesystem when `fs_game` changed
  (`FS_Restart` `0x42d2b0`, which reads `default_mp.cfg` off the new path
  and, unless the game is the last one it ran, `config_mp.cfg`), restarts
  the renderer and loads the UI again (`CL_InitUI` `0x418f10`), so the
  menus are read off the mod's paks. VERIFIED (strings and calls);
  INFERRED (the order's effect).
- `FS_Startup` adds `main` and then `fs_game`, the later directory searched
  first, so a mod's `ui_mp/*.menu` replaces the stock file by name. A mod's
  `ui_mp_x86.dll` inside a pk3 is written out beside it and loaded
  (`Sys_LoadDll` `0x4633a0`). INFERRED (structure).
- A remote connect reloads the UI too: `CL_DownloadsComplete` (`0x40ffb0`)
  shuts it down and starts it again with the hunk users. INFERRED (calls).
- In a game `vid_restart` keeps the connection. With no local server it
  restarts the filesystem when `fs_game` was modified or the checksum
  feed changed (`FS_Restart` called at `0x40fd66`, so a pure server's list still applies), loads
  the UI again, and when the state is past challenging and not 7 or 8
  (`0x40fda5`..`0x40fdaf`) runs `CL_InitCGame` (`0x4049c0`) and
  `CL_SendPureChecksums` (`0x40fac0`). So switching mods from the in-game
  main menu reloads the menus and the cgame's assets under the new mod and
  re-sends `cp`; a pure server that does not list the mod's paks keeps
  them out of the search path. VERIFIED (calls); INFERRED (the state
  numbering, which is CoD's own).
- Before the restart `vid_restart` clears the whole hunk (`Hunk_Clear`
  `0x432090`, `Hunk_Clear: reset the hunk ok`). `CL_InitCGame` builds
  `maps/mp/%s.bsp` off the serverinfo's map and calls the cgame's
  `CG_Init` (`0x30022f90` in `cgame_mp_x86.dll`), which loads the
  `collision map` (syscall 0x1f) and then the world (syscall 0x2c) under
  their `LOADING... %s` lines and parses configstring 7's weapons again
  (`0x3000ef10`). VERIFIED (strings, calls).
- `CM_LoadMap` (`0x419c90`, `CM_LoadMap( %s, %i )`) reuses the loaded map
  for a client load only while a local server runs (the cvar
  `vid_restart` refuses on). VERIFIED (asm). Otherwise it zeroes the
  collision state (0xa15c bytes) and reads the file again through
  `0x42a8b0`, a `(name, &buffer) -> length` read off the search path.
  INFERRED (its shape). So an in-game mod switch reads the map off the
  mod's paks: a mod that ships its own `maps/mp/<map>.bsp` replaces the
  stock geometry and collision. INFERRED (the two above together).

### The cgame restart

- `CL_Vid_Restart_f` (`0x40fbe0`), while a cgame runs (`0x1617348`),
  calls its export `0x11` to save state into the hunk before the restart
  and, once `CL_InitCGame` has run, export `0x12` (`0x404fb0`) to hand it
  back. VERIFIED (calls). The 1.1 MP cgame's `vmMain` (`0x300203b0`)
  answers both with 0 and saves nothing. VERIFIED (the switch). 1.5's
  engine (`CoDMP.exe` `0x4107f0`) uses exports `0x12` and `0x13`, which
  its cgame's `vmMain` (`0x30021650`) answers with 0; UO's
  (`CoDUOMP.exe` `0x411560`) uses `0x13` and `0x14`, and its cgame's
  `vmMain` (`0x3002af00`) answers with 0 too. VERIFIED (calls and
  switches). So on all three nothing of the old cgame carries over.
  INFERRED (from the two above).
- `CL_ShutdownCGame` (`0x401a20`, called twice by the restart)
  `FreeLibrary`s the cgame module and zeroes its VM slot, and
  `CL_InitCGame` (`0x4049c0`) loads it again and calls `CG_Init` (export
  0) with `0x15de99c`, `0x15de9a4` and `0x15ce860`. VERIFIED (calls).
  Those are the current server message number, command sequence and
  client number, as Q3's `CL_InitCGame` passes. INFERRED. A freshly loaded
  DLL starts from its image, so everything the cgame keeps starts over:
  the chat ring (cod11-chat.md 3.2), the kick and landing dip, the
  prediction and its error, every entity's state, which the first
  snapshot resets as a load's does, and the weapon selection. INFERRED
  (the module reload).
- `CG_Init` parses configstring 7's weapons again as the configstring
  stands then, not as the gamestate sent it. INFERRED (the parse at
  `0x3000ef10` runs under `CG_Init`, and `CG_ConfigStringModified` has
  no case for 7).
- The restart opens with `0x44fa40(1)`, which ends every 2D and 3D
  sample and closes every stream: its flags 2, 4 and 8 keep the music
  slot, the ambient slots or the samples, and 1 sets none of them.
  VERIFIED (the decompilation). `CG_Init` starts the ambient again
  (cod11-sound-system.md, `snd_restart`). INFERRED. `snd_restart`
  restores its saved streams after the restart (`0x40fea7`), so they
  outlive this stop. VERIFIED (calls).
- The stop runs whether a game is up or not: the call sits after the
  listen server refusal and the save block, under no condition, so a
  `vid_restart` from the main menu ends its music and clicks too, before
  the reloaded UI opens the main menu again. VERIFIED (the
  decompilation). 1.5's restart calls `0x452c50(1)` and UO's
  `0x454dd0(1)` at the same spot. VERIFIED (calls); INFERRED (that
  they are the same stop, by position).
- `CL_InitCGame` and `CL_SendPureChecksums` run only past `CA_LOADING`
  (`0x40fda5`), so a `vid_restart` during a load restarts no cgame.
  VERIFIED (asm). 1.5 (`0x404ab0`, `0x4106d0`) and UO (`0x405310`,
  `0x411450`) keep the same tail; `0x404ab0` and `0x405310` carry
  `maps/mp/%s.bsp` and `CL_InitCGame: %5.2f seconds`. VERIFIED (calls,
  strings).

### A map change reloads the cgame

- Every gamestate goes through the same reload. `CL_ParseGamestate`
  (`0x416050`) ends in `CL_InitDownloads` (`0x410240`), which reaches
  `CL_DownloadsComplete` (`0x40ffb0`) once the downloads are done. That
  sets state 4, and with no local server (the cvar at `0x163a23c` the
  listen server check of `vid_restart` reads) calls `0x40ee30`, which runs `CL_ShutdownCGame`
  (`0x401a20`), the UI shutdown (`0x418e30`, the `FreeLibrary` of the UI
  module at `0x161747c`) and the renderer's shutdown with 0, then
  `0x432140`, which runs both shutdowns again, `0x457970`, `0x404fe0`,
  `Hunk_Clear` (`0x432090`) and `0x460420`; then `CL_InitCGame`
  (`0x4049c0`), `CL_SendPureChecksums` (`0x40fac0`) and three packets
  (`0x40ba50`). VERIFIED (calls). With a local server and a cgame already
  up it returns before `CL_InitCGame`. VERIFIED (the branch on
  `0x1617348`). This is Q3's `CL_DownloadsComplete` with
  `CL_FlushMemory` inlined. INFERRED.
- 1.5's `CL_DownloadsComplete` (`CoDMP.exe` `0x410bc0`: `0x40fb00`,
  whose first call is the cgame shutdown `0x401b70`, `0x434320`, then
  `CL_InitCGame` `0x404ab0`) and UO's (`CoDUOMP.exe` `0x411940`:
  `0x4104a0`, first call `0x401b70`, `0x435d20`, then `0x405310`) have
  the same shape. VERIFIED (calls; `0x401b70` `FreeLibrary`s the module
  at `0x18e2a28` on 1.5 and `0x4e19990` on UO).
- So a map change starts the cgame over as a `vid_restart` does: the
  chat ring and everything else above go. `CG_Init` takes the client's
  current command sequence (`0x15de9a4`), so the commands parsed while
  the map loaded, a chat line among them, never reach the new cgame.
  INFERRED (Q3's `CL_InitCGame` passes it, and the cgame starts its
  command walk there). A `map_restart` sends no gamestate
  (cod11-map-cycle.md section 4) and reloads nothing.

### A disconnect frees the cgame

- `0x4358a0`, the error handler's arm for codes 1, 2, 3 and 5 (a
  `disconnect`, a drop, a server `error`), calls `CL_Disconnect(1)`
  (`0x40ef90`), `CL_ShutdownCGame` (`0x401a20`), the UI shutdown
  (`0x418e30`) and the renderer's shutdown with 0, then `SV_Shutdown`
  (`0x459600`), both shutdowns again, `0x457970`, `0x404fe0`,
  `Hunk_Clear` (`0x432090`), `0x460420` and `CL_StartHunkUsers`
  (`0x4115c0`). VERIFIED (calls). So leaving a server unloads the cgame
  module and its chat ring with it, as a map change does. INFERRED (the
  module unload, as above).

### `snd_restart` with no game up

- `snd_restart`'s body (`0x40fe0c`) saves the streams (`0x4510f0`),
  shuts the sound system down and starts it (`0x450130`, `0x44fe60`),
  runs `CL_Vid_Restart_f` and then restores the streams (`0x451330`).
  VERIFIED (calls). The restart's UI shutdown clears `KEYCATCH_UI` (bit
  2 of `0x155f2c4`, `0x418e30`'s first store). VERIFIED (the store).
- The client frame's check at `0x411280`: with no `0x155f2c8` pending,
  state 0 (disconnected), no `KEYCATCH_UI` and no local server it calls
  `S_StopAllSounds(0)` (`0x44fa40`, which closes every stream) and then
  the UI's export 7 with 1, `UI_SetActiveMenu(UIMENU_MAIN)`. VERIFIED
  (decompile). This is Q3's `CL_Frame` check. INFERRED.
- So from the main menu the restored streams live until the next frame,
  which stops them and opens the main menu, whose `onOpen` starts its
  music from the top. INFERRED (the order of the two above; nothing
  between them sets bit 2 again was read). vcod's `enter_menu` stops
  every sound (`AudioSystem::on_gamestate("menu")`) before it opens the
  main menu, so it ends the same way, a frame earlier.

### Alt+Enter's release

- The window procedure sends `WM_SYSKEYUP` (0x105) through the byte
  table at `0x467050` (index 0) and the jump table at `0x467040` (entry
  0) to `0x466f1c`, the `WM_KEYUP` (0x101) arm, which maps the key
  (`0x466b70`) and queues a key up (`0x463580` with 1, the key, 0).
  VERIFIED (the tables' bytes, asm). Only the down at `0x466e52` is
  swallowed.
- `CL_KeyEvent` (`0x40dc30`) runs a key up whatever `keys[key].down`
  says: the `-` half of a `+` bind, then with `KEYCATCH_UI` the UI's
  key event (export 3) with down 0. VERIFIED (decompile). So the Enter's
  release reaches the menu on retail too, and vcod's `Ui::key_up`
  (`Menu_HandleKey`'s focus pass at the cursor) matches.

### vcod

- `mods_menu` opens from the stock button, lists `vcod_common::pk3::mod_list`
  (the install's directories other than `main` and `--mod-dir` with a pk3,
  sorted by name, at most 64) and draws each row's description or name.
  A click selects and shows Launch; Launch or a double click switches.
- The switch sets the player's `fs_game`, reopens the search path with the
  mod over the base directory, rebuilds the whole front end off it and
  opens the new main menu, as `vid_restart` brings retail's back.
- In a game the switch keeps the connection: the search path reopens under
  the server's pure list, the renderer's shaders, the HUD, sounds, effects
  and viewmodels reload, the front end is rebuilt closed and the game has
  the mouse again, and a live map sends `cp` once more. The map is read
  again off the new search path (`reread_live_map`): the world, its
  collision, the ambient and configstring 7's weapons, with a fresh
  entity scene and the first snapshot's resets, as the restarted cgame's.
  A `vid_restart` with the mod unchanged keeps the loaded map, which
  reads the same files. The next gamestate puts a server's own `fs_game`
  back.
- Every `vid_restart` in a live game, the Mods switch's included,
  restarts the cgame half (`App::restart_cgame`, `restart_live_cgame`):
  the live state starts fresh over the kept collision map and
  `cl.serverTime`, configstring 7 is parsed again, the view
  (`OnlineView::restart_cgame`: kick, landing, quake, shellshock; the
  `cg_shellshock` file the cvars hold stays), the selection, the
  prediction, the HUD (`Hud::restart_cgame`: message windows, chat,
  scoreboard, kill icons, vote and timeout lines), the effects and the
  marks start over, every sound stops and the ambient starts from the
  top (not after `snd_restart`, whose restored streams play on), and
  `cp` goes out again. Before the game is live nothing restarts.
- Leaving a server drops the chat ring with the rest of the HUD's game
  state (`leave_server_hud`), as retail's disconnect frees the cgame.
- A map load runs the same reset (`fresh_cgame`) once the new map is up,
  where `CL_DownloadsComplete` loads the cgame: the chat ring, the view
  and the prediction start over and the lines that came in during the
  load are dropped. The gamestate parse itself keeps the chat, so the
  loading screen still shows it.
- Every `vid_restart` but `snd_restart`'s stops every sound first, a
  game up or not; the offline viewer keeps its ambient. The Mods pick's
  restart stops them too.
- The pick forces the `fs_game` cvar, as `RunMod`'s `trap_Cvar_Set`
  does. A server that names no `fs_game` keeps whatever the cvar holds;
  one that names another replaces it, and leaving keeps the server's
  until the Mods menu or another server names one.
- While the server Dedicated No started runs, a pick prints `Listen
  server cannot video restart.` and only records the mod, as `RunMod`'s
  `fs_game` set before the refused `vid_restart`; leaving the game, or the
  next `vid_restart` with no game up, switches to it.
- vcod has no UI DLL, so a mod's `ui_mp_x86.dll` does nothing; only its
  menu files count.

## 18. Start New Server (`ui_mp/createserver.menu`)

### The menus

- `createserver.menu` holds three menuDefs: `createserver` (full screen,
  the backdrop), `createserver_op` (the server settings, Back, the Game
  Type Settings / Server Settings toggle and Start) and
  `createserver_maps` (the map preview and the map list). `createserver`'s
  `onOpen` runs `uiScript loadArenas`, opens `createserver_op`, then
  `createserver_maps`; its `onClose` closes both. The five
  `ui_mp/settings_<gametype>.menu` files hold each game type's page.
  VERIFIED (the files in `pak0.pk3`).
- Server settings: Game Type is owner draw `UI_NETGAMETYPE` (245); Server
  Name edits `sv_hostname` (32 chars); Dedicated is a multi on
  `ui_dedicated` (No 0, LAN 1, Internet 2); Pure Server a yes/no on
  `sv_pure`; Maximum Players, Minimum Ping, Maximum Ping and Maximum Rate
  are `ITEM_TYPE_NUMERICFIELD` (9) on `sv_maxclients`, `sv_minping`,
  `sv_maxping`, `sv_maxrate`; Password an edit field on `g_password`.
  The game type pages edit `scr_<gt>_scorelimit`, `scr_<gt>_timelimit`
  (and for `sd` / `re` the round limit, round length and grace period),
  `scr_drawfriend`, `scr_friendlyfire`, `scr_forcerespawn`, `g_allowvote`,
  `scr_allow_panzerfaust`, `scr_re_showcarrier`, and a `ui_allow_sniperrifles`
  shadow whose actions `setcvar` the three `scr_allow_*sniper*` /
  `scr_allow_springfield` cvars. VERIFIED (the files).
- Game Type Settings runs `openForGameType "settings_%s"` and Server
  Settings `closeForGameType "settings_%s"`; both buttons carry `cvar
  "ui_netGametypeName"`. VERIFIED (the file). The handlers (`0x40010770`,
  `0x400107f0`, names at `0x4002dca0` / `0x4002dc8c`, table at
  `0x40036068`) read the item's cvar (`item+0x114`) through the cvar
  string import, `va` it into the argument and open / close that menu
  (`0x400134b0` / `0x40010640`). VERIFIED (asm).
- `origin x y` adds to the rect parsed so far, and a `rect` after it
  replaces the whole rect: the game type item puts `origin` before
  `rect 10 158` and lands at 10 158, while the fields after it put
  `origin 20 8` after `rect -10 165` and stack 15 apart from 173. INFERRED
  from RTCW's `ItemParse_origin` and the stock layout it produces.

### The game type and map lists

- The game type list is `UI_LoadGameTypes`' second list (section 6, "The
  game type filter"): the same scripts without `All`. vcod reads both off
  `vcod_common::ui_menu::game_types`.
- `UI_LoadArenas` (`0x40007400`): every `.arena` under `mp`
  (`0x4002fa38` `mp`, `0x4002fa30` `.arena`), each file read whole
  (more than 8191 bytes is refused, `0x400072f0`) and parsed as `{ key
  value }` blocks into one info list of at most 64 (`0x40007170`, "Max
  infos exceeded"). Per info: `map`, `longname`, `levelshots/<map>`, and a
  game type bit mask from the words of `gametype` matched against the
  list with `Q_stricmp`; a missing or empty `gametype` sets every bit.
  VERIFIED (decompile). The stock list is `mp/CoDmaps.arena` in
  `pak4.pk3`; `mp_chateau`'s entry leaves out `sd`. VERIFIED.
- `loadArenas` (handler at `0x4000a98f`) runs `UI_LoadArenas`, then
  `0x4000b510` (sets `ui_netGameType` and `ui_netGameTypeName` to the
  list's entry for `g_gametype`, untouched when none matches), then
  `0x4000b5c0` (marks the maps whose mask has the current game type's
  bit), then selects row 0 of `FEEDER_ALLMAPS` (4) and runs `0x4000b610`
  (which picks the running server's map when the client is in state 6).
  VERIFIED (asm, decompile). The stock arena file lists eleven maps and
  the stock map list shows six rows, so it scrolls (section 19). VERIFIED
  (the files).
- `UI_NETGAMETYPE`'s paint (`0x40008820`) draws the localized display
  name of entry `ui_netGametype` (the vmCvar at `0x401c3620`, integer at
  `0x401c362c`), `EXE_ALL` when it is empty; an index past the count
  resets to 0. VERIFIED (decompile). Its key handler (`0x40009a60`)
  steps forward on `K_MOUSE1` (200), Enter and 0xbf, back on `K_MOUSE2`
  (201), wrapping, then sets `ui_netGameType`, `ui_netGameTypeName` (the
  short name) and `ui_currentNetMap` "0", re-marks the maps and selects
  row 0. VERIFIED (decompile).
- `FEEDER_ALLMAPS` rows are the marked maps' `longname`s in load order
  (`0x4000c6c0` returns the n-th marked info's `longname`). VERIFIED
  (decompile). A selection (`0x4000cc40`) sets `ui_mapIndex` and
  `ui_currentNetMap` to the info's index. VERIFIED (decompile).
- The cvars, rows `{vmCvar, name, default, flags}` in the table at
  `0x40036c28`.. : `ui_dedicated` "0" flags 1, `ui_netGametype` "0" flags
  1, `ui_netGametypeName` "" flags 1. VERIFIED. CoDMP.exe registers the
  server's own cvars in the client too (`0x459130`): `g_gametype` "dm"
  (0x24), `sv_hostname` "CoDHost" (5), `sv_maxclients` "20" (0x24),
  `sv_maxRate`, `sv_minPing`, `sv_maxPing` "0" (5), `sv_pure` "1" (0xc).
  VERIFIED (decompile and the strings at `0x55dda8`, `0x5667b0`,
  `0x56871c`, `0x5685e4`).

### `uiScript StartServer` (`0x4000a62b`)

- Sets `cg_thirdPerson` (`0x4002f380`) to "0", sets `dedicated`
  (`0x4002ecb8`) to `ui_dedicated`'s integer (`0x401eed0c`) clamped to
  0..2 (`0x40005840`), sets `g_gametype` to the selected game type's short
  name, and appends `wait ; wait ; map %s\n` (`0x4002eca0`) with the map
  of the info `ui_currentNetMap` selects. VERIFIED (asm). Everything else
  the server reads (`sv_hostname`, `scr_*`) the menus already set in the
  shared console. The Start button's script closes `createserver`,
  `joinserver` and `multi_menu` first. VERIFIED (the file).

### vcod

- `dedicated` is registered latched (flags 0x20) by `Com_Init`
  (`0x4375c0`). VERIFIED (decompile). `Com_Frame` (`0x437f40`) applies a
  latched change at the top of its next frame: a non-zero value runs
  `CL_Shutdown` (`0x4127f0`, "----- CL_Shutdown -----") and shows the
  dedicated console window; a zero value while a non-dedicated game runs
  prints "cannot become non-dedicated, since a non-dedicated game is
  already running" and puts the old value back. The client frame
  (`0x411280`) runs only while `dedicated` is 0. VERIFIED (decompile).
  StartServer's `wait ; wait` before `map` gives that frame room, so
  retail's Dedicated LAN and Internet turn the game into a dedicated
  server in the same process, and the player does not join it; Dedicated
  No is an in-process listen server the player joins over the loopback
  transport. INFERRED (the order).
- vcod has no listen server, on purpose: the server is always a separate
  `vcod-server` process. `StartServer` launches it from the directory of
  the running `vcod` with the map, `--gametype`, `--game-dir` /
  `--mod-dir` as the client has them, `--set fs_game=<dir>` for the Mods
  menu's choice (retail's in-process server shares the client's
  `fs_game`), `--set dedicated=<1..2>` (No runs as 1, so no heartbeat),
  `--hostname` and `--max-clients` from `sv_hostname` and
  `sv_maxclients`, and `--set name=value` for every other cvar the
  `createserver_op` and `settings_*` items edit or `setcvar` (the `ui_`
  shadows left out, empty values skipped). The port is the first free
  one from 28960. Output goes to `vcod-server.log` beside `vcod_mp.cfg`.
  A missing binary or a failed start shows the error popup.
- Dedicated LAN and Internet: the client stays in the front end, unlike
  retail's, whose window turns into the dedicated console. The child runs
  until the client exits or starts another; vcod kills it then, so it is
  never left behind.
- Dedicated No: the client sends the child a `getinfo` every 250 ms and
  connects to `127.0.0.1:<port>` as `connect` does once an `infoResponse`
  comes back. If the child exits first, or 60 s pass, the error popup
  names the status and the log. The host is an ordinary client of its
  server: it takes the first free public slot and needs `g_password` like
  anyone else, where retail's loopback client is exempt (handshake doc,
  "`g_password`").
- Dedicated No's child is killed when the client exits, starts another,
  connects elsewhere or disconnects, the last as retail:
  `CL_Disconnect_f` (`0x40f5f0`) raises
  `Com_Error(3, "EXE_DISCONNECTED_FROM_SERVER")`, and the frame's error
  handler (`0x435910`) sends codes 1, 2, 3 and 5 alike through `0x4358a0`,
  which runs `CL_Disconnect` (`0x40ef90`) and then `SV_Shutdown`
  (`0x459600`, "----- Server Shutdown -----"). VERIFIED (decompile). A
  connection that fails after the child died names the exit status and
  the log in the error popup.
- `cg_thirdPerson` is not set (vcod has none). The map preview
  (`UI_STARTMAPCINEMATIC`, 255) draws the selected map's
  `levelshots/<map>` the way the browser's preview does (section 6);
  retail's paint for 255 was not read. A left click steps the game type
  forward and a right click back, as `0x40009a60` does. The map list has
  retail's scroll bar and keys (section 19); the wheel steps the focus,
  as retail's does, and scrolls nothing.
- A game type page's `onEsc` (`open main; close createserver`) leaves
  any page but `settings_dm` open under `main`, since `createserver`'s
  `onClose` names only that one. vcod draws open menus in open order
  from the topmost full screen one, so the page stays hidden until it
  opens again; its items still take the mouse under `main`, as retail's
  do (section 20, "Which menu takes the mouse").
- `vcod-server` takes `--set fs_game=<dir>` as retail takes `+set
  fs_game`: the directory's paks go over the base's, the systeminfo
  carries `fs_game` after `bg_fallDamageMinHeight`, and `getinfo` adds
  `game` after `pure`. A retail 1.1d server with `+set fs_game mymod` and
  one pak in `mymod/` answered the same `getinfo` and sent the same
  systeminfo, byte for byte, as `vcod-server` with the same mod and
  `sv_pure 1` (2026-10-11). VERIFIED (live). Retail loaded its game module
  from `<fs_homepath>/mymod/` (`Sys_LoadDll` in its log). VERIFIED (live).
- A numeric field takes digits only. INFERRED from Q3's
  `Item_TextField_HandleKey`; not checked against `ui_mp_x86.dll`.

## 19. List boxes

Sources: `ui_mp_x86.dll` 1.1. The function names are RTCW's, matched by
shape.

### The scroll bar

- `Item_ListBox_Paint` (`0x400159b0`) draws a vertical list's bar on every
  list box, with no flag to turn it off: the up arrow at `x + w - 17`,
  `y + 1`, 16 square; the bar 16 wide and `h - 31` tall from 15 below
  that; the down arrow at the bar's foot less 1; then the thumb at
  `Item_ListBox_ThumbDrawPosition`, kept 17 above the down arrow. The
  handles are the assets `ui/assets/scrollbar_arrow_up_a.tga`,
  `scrollbar.tga`, `scrollbar_arrow_dwn_a.tga` and `scrollbar_thumb.tga`,
  which `AssetCache` (`0x400077f0`) registers into the display context
  at offsets `0x1e2c4`, `0x1e2d4`, `0x1e2c8` and `0x1e2d8`. VERIFIED
  (asm, the float table at `0x4002ff70`).
- `Item_ListBox_MaxScroll` (`0x400115a0`): the row count less `h /
  elementHeight` (truncated) plus 1, at least 0, so the last first row
  shows an empty row under the last. `Item_ListBox_ThumbPosition`
  (`0x400115f0`): `y + 17 + first * (h - 50) / max`, truncated, and `y +
  17` when `max` is 0. While the thumb is held
  `Item_ListBox_ThumbDrawPosition` (`0x400116a0`) draws it at the cursor
  less 8 inside its travel. VERIFIED (asm).
- `Item_ListBox_OverLB` (`0x40011840`) names the part under the cursor:
  the up arrow (`0x800`), the down arrow (`0x1000`), the thumb
  (`0x2000`), the bar above the thumb (`0x4000`), below it (`0x8000`), each
  16 wide at `x + w - 16`. VERIFIED (asm).
- `Item_ListBox_HandleKey` (`0x40011da0`) acts only with the cursor inside
  the list and the item focused. On `K_MOUSE1` or `K_MOUSE2` an arrow
  moves the first row by one and the bar above or below the thumb by a
  page (`h / elementHeight`), clamped to 0 and `MaxScroll`; anywhere else
  selects the row under the cursor (a second click inside 300 ms runs
  `doubleClick`). The keys move the selection and keep it in view: the
  up and down arrows (`0x9a`, `0x9b`, keypad `0xb7`, `0xbd`) by one, Page
  Up and Page Down (`0xa4`, `0xa3`, keypad `0xb8`, `0xbe`) by a page, Home
  and End (`0xa5`, `0xa6`, keypad `0xb6`, `0xbc`) to the ends. VERIFIED
  (decompile). A click on the thumb does nothing there. VERIFIED
  (decompile).
- `Menu_HandleKey` (`0x400136b0`) sends the wheel (`0xcd`, `0xce`) with
  the down and up arrows and Tab to the next and previous focus item
  (`0x40013240`, `0x40013160`) when the focused item's handler did not
  take the key. The list box handler takes neither, so the wheel never
  scrolls a list. VERIFIED (decompile).

### The capture: held arrows and the thumb drag

- `Item_HandleKey` (`0x40013040`): with no capture item (`0x401bf3e8`),
  a press of `K_MOUSE1`, `K_MOUSE2` or `K_MOUSE3` (200..202) runs
  `Item_StartCapture` (`0x40012dd0`) before the item's key handler; with
  one, any key event, the button's release included, clears the item, the
  function (`0x401bf3e0`) and its data (`0x401bf3e4`), starts no new
  capture, and the item's key handler still runs on a press. VERIFIED
  (decompile).
- `_UI_KeyEvent` (`0x4000d6f0`) hands every key event, press or release,
  keyboard, mouse button or wheel, to `Menu_HandleKey` (`0x400136b0`),
  which calls `Item_HandleKey` for the menu's focused item. VERIFIED
  (decompile). `CL_KeyEvent` (CoDMP.exe `0x40dc30`) drops a key repeat
  while the catcher holds neither the console (1) nor the message line
  (4) bits (`test 5`), so a repeat never reaches the UI. VERIFIED
  (decompile). `CL_CharEvent` (`0x40e200`) has no repeat test and hands
  the UI catcher `key | 0x400` (`K_CHAR_FLAG`), so typed characters and
  Backspace (char 8, which `Item_TextField_HandleKey` deletes on) repeat in
  an edit field while the arrows, Home, End and Del there do not. VERIFIED
  (decompile). So a keyboard key pressed or let go during a held arrow
  ends the hold, as does the wheel or the other button; a key already
  held when the arrow was pressed does not, through its repeats.
  INFERRED (the combination).
- `Item_StartCapture` for a list box: over an arrow (`0x1800`) it fills the
  scroll info at `0x401be340` with `nextScrollTime = realTime + 500`,
  `nextAdjustTime = realTime + 150` (`0x96`), `adjustValue = 500`, the key
  and the item, and captures with `0x40012b20`; over the thumb (`0x2000`)
  it stores the cursor as `xStart` / `yStart` and captures with
  `0x40012b90`. The page areas capture nothing. VERIFIED (asm). RTCW's
  start sets `nextAdjustTime` 500 on; 1.1 sets 150.
- The capture function runs once a frame at the head of the paint
  (`0x400189b0`). VERIFIED (decompile).
- `Scroll_ListBox_AutoFunc` (`0x40012b20`): when `realTime >
  nextScrollTime` it calls `Item_ListBox_HandleKey` with the stored key and
  force set and puts `nextScrollTime` `adjustValue` on; then when
  `realTime > nextAdjustTime` it puts that 150 on and takes 40 off
  `adjustValue` while it is over 20 (`cmp 0x14`, `add -0x28`). VERIFIED
  (asm). So a held arrow steps on the press, again past 500 ms (the gap
  is 380 by then), and the gap shrinks to 20 ms about 1.8 s in.
  INFERRED (the arithmetic).
- `Menu_HandleMouseMove` (`0x40016b20`, reached from `Display_MouseMove`
  `0x40018b40`) skips the items' mouse handling while a capture item is
  set, so the part flags (`0x48`) stay as they were at the press and the
  repeat steps that arrow wherever the cursor goes; force skips the
  cursor-inside test. VERIFIED (its `0x401bf3e8 == 0` test); INFERRED (the effect).
- `Scroll_ListBox_ThumbFunc` (`0x40012b90`), vertical: when the cursor's
  y differs from `yStart`, `startPos = ftol((cursorY - (y + 17) - 8) *
  MaxScroll / (h - 34 - 16))` clamped to 0..MaxScroll, and `yStart`
  takes the cursor (floats 17, 32, 2, 8, 16 at `0x4002ff70`,
  `0x4002ff78`, `0x4002feb8`, `0x4002fecc`, `0x4002ff74`). It then runs
  the same timed `HandleKey` as the arrows off the scroll info's stale
  times; with the flags frozen on the thumb that call returns at the
  `0x2000` test and does nothing. VERIFIED (asm). This is RTCW's mapping
  with the same constants.

### The wheel

- `Menu_HandleKey` (`0x400136b0`) runs `Menu_HandleMouseMove`
  (`0x40016b20`) at the cursor first, then the focused item's key
  handler, which takes neither `0xcd` nor `0xce`: the only places the
  module's decompile names either are `Menu_HandleKey`'s own switch
  cases. VERIFIED (decompile). `0xcd` (wheel down) goes with
  Tab, the down and right arrows to `Menu_SetNextCursorItem`
  (`0x40013240`), `0xce` (wheel up) with the up and left arrows to
  `Menu_SetPrevCursorItem` (`0x40013160`). VERIFIED (decompile).
- `Menu_SetNextCursorItem` walks the menu's `cursorItem` (`+0xc8`) up,
  wrapping once, and stops at the first item `Item_SetFocus`
  (`0x40011410`) takes; with `cursorItem` -1 it starts the walk at 1. It
  then runs `Menu_HandleMouseMove` at the item's `rect.x + 1`, `rect.y +
  1` (the float 1.0 at `0x4002feb4`), not at the cursor. VERIFIED (asm).
  `_Prev` walks down the same way, from `count - 2` when -1. VERIFIED
  (decompile).
- `Item_SetFocus` refuses an item with flag `0x10` (decoration) or `0x2`
  (the focus already), one without `0x4` (visible), and one its show or
  enable cvar test hides. A text item (type 0) takes the focus only with
  the cursor in it; otherwise the old focus comes back and it still
  answers 1 and moves `cursorItem`. Any other type takes it with
  `Menu_ClearFocus` (`0x4000fe70`), its `onFocus` and the focus sound
  (`0x124`, else the display context's `0x1e2fc`, `itemFocusSound`, which
  the stock `main.menu` comments out). VERIFIED (decompile).
- `Menu_HandleMouseMove` leaves (`0x40011cb0`, `mouseExit`) every item
  that has the mouse-over bit (`0x1`) and does not hold the point, and
  enters (`0x40011b80`, `mouseEnter` once per entry) and focuses every
  visible item that holds it, last item first. VERIFIED (decompile).
- `Menu_HandleKey` runs `Menu_HandleMouseMove` at the cursor for every
  key event, press or release, before anything else (the first call in
  `0x400136b0`), and the switch that steps the focus runs only on a press
  (`param_3 != 0` ahead of it). VERIFIED (decompile). The keys are Tab
  (9), down (`0x9b`), right (`0x9d`), keypad down (`0xbd`) and wheel down
  to `_Next`; up (`0x9a`), left (`0x9c`), keypad up (`0xb7`) and wheel up
  to `_Prev`. Shift does not reverse Tab. VERIFIED (decompile). The menu's
  own key table (`+0xe8 + key * 4`, `execKey`) is tried first. VERIFIED
  (decompile).
- `Menu_HandleMouseMove` ends with `Menu_ClearFocus` when it focused
  nothing and a focused item was found that does not hold the point, and
  `Menu_ClearFocus` leaves `cursorItem` alone. VERIFIED (decompile). That
  the point test reads the focused item's rect is INFERRED (the decompile
  lost the `this`). So a Tab press focuses and enters the next item, the
  item stays focused while the key is down, and the release takes the
  focus back to the item under the cursor or clears it; with the cursor
  on no item the next press walks on from there. INFERRED (the
  combination).
- `Item_ListBox_HandleKey`'s vertical half takes up and down but neither
  left nor right (`0x9c`, `0xb9`, `0x9d`, `0xbb` are the horizontal
  half's), so left and right over a list step the focus. It takes its keys
  with an empty list too: nothing checks the row count before the key
  switch. VERIFIED (decompile). Page Up and Page Down (`0xa4` / `0xb8`,
  `0xa3` / `0xbe`) move `cursorPos` by `h / elementHeight`, clamp it to the
  rows, pull `startPos` to keep it in view and run the feeder's selection
  (`+0x9c`); they never scroll without moving the selection. VERIFIED
  (decompile). The server list is a plain list box on `FEEDER_SERVERS`,
  so this is its Page Up / Down.
- `Item_TextField_HandleKey` (`0x40012690`) steps the focus itself on Tab,
  down and keypad down (`_Next`) and up and keypad up (`_Prev`), and moves
  the edit to the target when it is an edit field (types 4, 9, 15);
  otherwise the edit stays on the old field. VERIFIED (decompile).
- So a wheel notch over a list neither scrolls it nor moves its
  selection. It enters the next (or previous) item: that item's
  `mouseEnter` runs and the hovered item's `mouseExit`. The release that
  follows runs `Menu_HandleMouseMove` at the cursor again, which leaves
  the item and enters and focuses the hovered one again, so its
  `mouseEnter` runs a second time and `cursorItem` comes back. INFERRED
  (the combination; that Windows queues the wheel's press and release
  together is RTCW's `IN_MouseEvent`, not read in CoDMP.exe). Over the
  stock server list the next item is the `password` column header,
  whose `mouseEnter` plays `mouse_over`.

### Right click

- `Menu_HandleKey` takes `K_MOUSE2` (201) wherever it takes `K_MOUSE1`
  (200): the item under the cursor gets its key handler, then its action
  (the switch at `0x400138b6`). VERIFIED (decompile). The owner draw key
  handlers for `UI_NETGAMETYPE` (`0x40009a60`), `UI_JOINGAMETYPE`
  (`0x40009b10`) and `UI_NETSOURCE` (`0x40009b90`) step back on 201 and
  forward on 200, Enter and `0xbf`. VERIFIED (decompile). A yes/no item
  (`0x400122b0`) toggles and a multi item (`0x40012550`) steps forward on
  either. VERIFIED (decompile).

### vcod

- The server list (`FEEDER_SERVERS`), the map list (`FEEDER_ALLMAPS`) and
  the Mods list (`FEEDER_MODS`) draw the bar and scroll on its arrows, its
  page areas and a thumb drag (`crates/client/src/frontend/listbox.rs`).
  A held arrow repeats on `Scroll_ListBox_AutoFunc`'s clock, checked once
  a frame, until either button comes up; the thumb maps the cursor as
  `0x40012b90` does, on each mouse move. Any key event that is not a
  repeat ends the hold or the drag, a slider's too; a press during one
  starts no new one. With the cursor over the map list, the Mods list or
  the server list, the arrows, Home, End and the page keys move its
  selection (keypad ones too); elsewhere the up and down arrows step the
  focus. On the server list an arrow with nothing selected picks the
  first row, and Enter joins the selected server anywhere in the browser
  (both vcod's own; section 20).
- The wheel scrolls a list, unlike retail, on purpose: a row over the map
  or Mods list, half a page in a menu with the server list
  (`Ui::wheel_scroll`). Anywhere else a notch steps the menu's cursor item as
  `Menu_SetNextCursorItem` and `_Prev` do and runs the scripts above: the
  hovered item's `mouseExit`, the target's `mouseEnter` and `mouseExit`,
  the hovered item's `mouseEnter` (`Ui::wheel`). The top open menu takes
  it, as it takes keys. Tab, the arrows and the keypad's up and down step
  the same way when no list takes them (`Ui::key`), but the return to the
  cursor waits for the next key event, release or mouse move
  (`Ui::settle_focus`), so the target draws in the menu's focus colour
  while the key is down. The focus scripts, the focus sound and Tab in
  an edit field follow section 20.
- vcod's menus drop key repeats as `CL_KeyEvent` does; in an edit field
  typed text and Backspace still repeat (`Ui::key_repeat`).
- A right click runs the left click's path, with the three owner draws
  stepping back.

## 20. Focus, edit fields and menu keys

Sources: `ui_mp_x86.dll` 1.1 (image base `0x40000000`). The display
context the module calls through sits at `0x401c3ce0` (`_UI_Init`,
`0x4000d310`, fills it). VERIFIED (asm).

### Keywords

- The item keyword table pairs `onFocus` (`0x400367d8`) with
  `0x40017930`, which stores the script at item `+0x10c`; `leaveFocus`
  (`0x400367e4`) with `0x40017950` (`+0x110`); `focusSound`
  (`0x40036874`) with `0x40016fd0`, which stores what the display
  context's `+0xcc` (`0x400193b0`, UI syscall `0x26`) answers for the name
  at `+0x124`. VERIFIED (asm). `maxCharsGotoNext` (string `0x4002d414`,
  table entry `0x4003685c`) pairs with `0x40017b10`, which validates the
  item's type data and sets its `+0x14` to 1 (the pointer at item
  `+0x250`). VERIFIED (asm).
- The menu keyword table pairs `execKey` (`0x40036a84`) with
  `0x40018780`, which reads one character and stores the script at menu
  `+0xe8 + c * 4`, and `execKeyInt` (`0x40036a90`) with `0x400187c0`,
  which reads an integer for the same slot. VERIFIED (asm). A second
  block for a key replaces the first. INFERRED (one slot per key).
- `itemFocusSound` is an `assetGlobalDef` keyword (`0x40007fe8`, string
  `0x4002f298`): the name goes through the same `0x400193b0` into the
  display context's `+0x1e2fc` (`0x401e1fdc`). VERIFIED (asm). The stock
  `ui_mp/main.menu` comments it out.
- UI syscall `0x26` is case `0x26` of CoDMP.exe's UI dispatcher
  (`0x418300`, jump table `0x418c68`, `"Bad UI system trap: %i"` at
  `0x565614`): `0x432a80` with set 0, the sound alias lookup in the menu
  alias set (`cod11-sound-system.md` 1a), which answers the alias
  record's name pointer or 0. Case `0x27`, the play, is `0x44d9c0` with
  set 0: `0x434160` picks a row of the named alias in the menu set and
  `0x44d670` starts it on the listener's entity with no origin. VERIFIED
  (asm). So `focusSound` and `itemFocusSound` name aliases, not files,
  and play the alias `play` with the same name plays. A name the menu set
  lacks registers as 0, and `Item_SetFocus` then falls back to
  `itemFocusSound` (its `+0x124 != 0` test). INFERRED (the combination).
  The stock commented-out `itemFocusSound "sound/misc/menu2.wav"` is a
  file path, so it would register as 0 even uncommented. INFERRED (no
  alias carries a path as its name).
- Stock use: no front-end menu (`ui_mp/menus.txt`) has `onFocus`,
  `leaveFocus`, `focusSound` or `execKey`. The script menus carry
  `onFocus { play "mouse_over"; }` and `execKey` rows, and
  `wm_quickmessage.menu` three `execKey`s; those menus are the HUD's in
  vcod (`cod11-quick-chat.md`). VERIFIED (the pak files).

### Taking the focus

- `Item_SetFocus` (`0x40011410`) is section 19's; once it accepts an
  item it calls `Menu_ClearFocus` (`0x4000fe70`) first, which clears the
  focus bit on every item of the menu and runs every item's
  `leaveFocus`, focused or not. A text item under the cursor takes the
  focus with no `onFocus`; a text item away from it gives the focus back
  to the item `Menu_ClearFocus` returned and runs that item's `onFocus`
  again, with no sound. Any other item runs its `onFocus`. The sound is
  the item's `focusSound`, else `itemFocusSound`, played through the
  display context's `+0x84` (`0x400193d0`, UI syscall `0x27`). VERIFIED
  (decompile). `play` (`0x40011030`) hands its alias string to the same
  `+0x84`, so the focus sound plays the way `play` does. INFERRED (what
  syscall `0x26` answers was not read in CoDMP.exe).
- `Menu_HandleMouseMove` (`0x40016b20`) calls `Item_SetFocus` for each
  shown item holding the point after its `mouseEnter`, so the mouse
  focusing an item runs `onFocus` and the sound too; an item that has the
  focus is refused, so moving inside it runs nothing. VERIFIED
  (decompile). A Tab press and its release each focus an item: the
  target's `onFocus` and sound, then the hovered item's. INFERRED (the
  combination with section 19).
- `Menu_HandleMouseMove` makes two passes over the items, last first,
  skipping any without the visible or forced flag (`0x100004`) or failing
  its cvar test. In both passes an item off the point with the mouse-over
  flag runs `Item_MouseLeave` (`0x40011cb0`) and loses the flag. In the
  second, every item on the point runs `Item_MouseEnter` (`0x40011b80`)
  when it is visible and not fading out (`0x4000fed0`), which runs
  `mouseEnter` only when the flag was clear and sets it, and calls
  `Item_SetFocus` until one call answers 1. VERIFIED (decompile). So
  every overlapping item under the cursor has the mouse at once, a
  decoration included (`Item_SetFocus` refuses only those for the focus),
  and of two overlapping items that can take the focus the one without it
  takes it on every move, so the focus swaps between them as the mouse
  moves inside both. INFERRED (the combination).
- `Item_SetFocus` writes the item's index into the menu's `cursorItem`
  (menu `+200`) whenever it answers 1, the text item that hands the focus
  back included; the mouse entering an item does not. VERIFIED
  (decompile, `0x40011554`).
- `Menu_HandleMouseMove` returns before anything while a capture item
  (`0x401bf3e8`), a bind waiting for its key (`0x401bf3f0`) or an edit
  (`0x401bf3f4`) is set. VERIFIED (decompile).

### Which menu takes the mouse

- `Display_MouseMove` (`0x40018b40`), called with no menu from the mouse
  event (`0x4000d790`), takes the focused menu (`0x40016970`: the
  topmost open menu with flags `0x2` and `0x4`); when that menu has the
  popup flag (`0x200000`) only it runs `Menu_HandleMouseMove`. Otherwise
  every open menu runs it from the top of the stack down until one
  answers 1. VERIFIED (decompile). `Menu_HandleMouseMove` answers 1 only
  when `Item_SetFocus` took an item; it walks a visible menu, full
  screen or not, and nothing in the walk tests a menu's rect.
  VERIFIED (decompile).
- So with a menu that is no popup on top, the mouse reaches the menus
  under it wherever no item of the menus above takes the focus. That
  includes a menu a full screen menu hides: the paint starts at the
  topmost full screen menu (`0x400189b0`) but the walk does not, so with
  `main` over a game type page left open (section 18), the page's
  buttons under `main`'s background still run their `mouseEnter`
  (`play "mouse_over"`) and take the focus. An item that already has the
  focus answers 0 too, so a move inside the top menu's focused item
  reaches the menus under it. INFERRED (the combination).
- A click goes to the focused menu through `Menu_HandleKey`
  (`0x400136b0`) unless the menu is no popup and the cursor is off its
  rect (`0x4000fef0`); then `Menus_HandleOOBClick` (`0x40013500`) gives
  the focus and the key to the first open menu from the top for which
  `Menu_OverActiveItem` (`0x40018d70`) holds: the menu's rect holds the
  cursor and so does a visible item that is no decoration. VERIFIED
  (decompile). `main`'s rect is the whole screen, so no click reaches the
  page under it. INFERRED (the combination).

### The focused menu

- The menu flag `0x2` is the focus. `Menus_Activate` (`0x40013400`)
  clears it on every menu in the stack, pushes the menu (`0x400104f0`
  takes it out first, `0x400104a0`), sets `0x6` on it, runs `onOpen`
  (`+0xdc`), then runs `Display_MouseMove` at the cursor. VERIFIED
  (decompile). So a menu opening under a still cursor runs the mouse pass
  at once: the item there takes the mouse and the focus.
- `Menus_Close` (`0x40010600`) runs `onClose` (`0x400105c0`, while the
  menu is still visible), takes the menu off the stack and, only when it
  had the focus, sets the focus on the new top; then it clears `0x6` on
  the menu. VERIFIED (decompile).
- `_UI_KeyEvent` (`0x4000d6f0`) hands every key to `Menu_HandleKey` of
  `Menu_GetFocused` (`0x40016970`), the topmost menu with `0x2` and
  `0x4`, not to the top of the stack. VERIFIED (decompile).
- `Menus_HandleOOBClick` (`0x40013500`), from `Menu_HandleKey` for a
  press of `K_MOUSE1..3` (200..202) off a focused menu that is no popup
  (guard `0x401bf418` against recursion): with the menu's
  `outOfBoundsClick` flag (`0x20000`) it first runs its `onClose` and
  clears `0x6` without taking it off the stack; then, for the first menu
  from the top for which `Menu_OverActiveItem` holds, it clears `0x2` on
  every stack menu, sets `0x6` on that one, runs `Display_MouseMove` and
  that menu's `Menu_HandleMouseMove` at the cursor, and hands it the key
  through `Menu_HandleKey`. Nothing writes the stack (`0x4020d300`).
  VERIFIED (decompile). So the lower menu takes the focus where it stands,
  stays drawn under the top one, and takes the keys (Esc runs its
  `onEsc`) until a menu opens or the click goes back. INFERRED (the
  combination). No stock menu sets `outOfBoundsClick`
  (`vid_restart.menu` comments it out). VERIFIED (the pak files). The
  menu keyword table entry at `0x40036a18` pairs the string
  `outOfBoundsClick` (`0x4002d2a4`) with `0x40018620`, which ORs
  `0x20000` into the menu's flags (`+0x48`) and takes no argument.
  VERIFIED (asm).

### Edit fields

- `Menu_HandleKey` (`0x400136b0`) hands an edit's press to
  `Item_TextField_HandleKey` (`0x40012690`) for the edit item
  (`0x401bf3fc`) ahead of anything else. A 0 back ends the edit; a 1
  back returns, except for `K_MOUSE1..3` (200..202), which end the edit,
  run `Display_MouseMove` (`0x40018b40`) at the cursor and go on to the
  normal key path. VERIFIED (decompile).
- `Item_TextField_HandleKey` answers 0 for Enter (`0xd`, keypad `0xbf`)
  and Esc (`0x1b`), 1 for every other key. On Tab (9), down (`0x9b`) and
  keypad down (`0xbd`) it calls `Menu_SetNextCursorItem` (`0x40013240`),
  on up (`0x9a`) and keypad up (`0xb7`) `_Prev` (`0x40013160`), and when
  the item it lands on is an edit field (types 4, 9, 15) that item
  becomes the edit item; otherwise the edit stays where it was. The
  target's cursor position is not reset. Left and right move the edit
  cursor. VERIFIED (decompile).
- Since `Menu_HandleMouseMove` returns at once during an edit, the step's
  mouse pass at the target's corner and every mouse move do nothing:
  only `Item_SetFocus` runs (`leaveFocus`, `onFocus`, sound), the target
  keeps the focus colour, and the item under the cursor stays as it was
  until a click ends the edit. INFERRED (the combination).
- A typed character that brings the cursor to `maxChars`, or one typed
  there in overstrike mode, steps to the next item the same way when the
  edit data's `+0x14` is set, and carries the edit when that item is an
  edit field. VERIFIED (decompile). That flag is the one
  `maxCharsGotoNext` sets (section 20, "Keywords") is INFERRED (the
  keyword's pointer at item `+0x250` was not traced to the edit data the
  handler reads); only the stock `cdkey.menu` uses the keyword. Outside
  overstrike mode a character typed into a full field is dropped
  (`maxChars <= len` returns 1 before the insert). VERIFIED (decompile).

### Menu keys

- `Item_HandleKey` (`0x40013040`) starts a capture for `K_MOUSE1..3` and
  hands the key to the item's type handler only for list boxes (6), owner
  draws (8), sliders (10), yes/no (11), multi (12) and binds (13); every
  other item answers 0. A handler's 1 runs the item's `action`. VERIFIED
  (decompile).
- The keys each handler takes, VERIFIED (decompile of each): the list
  box (`0x40011da0`) `K_MOUSE1` and `K_MOUSE2` (200, 201) and its
  navigation keys, not Enter or `K_MOUSE3`; the owner draw goes through
  the display context's `+0x88` (`0x40011d70`), and the browser's game
  type and source handlers (`0x40009a60`, `0x40009b10`, `0x40009b90`)
  take 200, 201, Enter (13) and keypad Enter (191); the slider
  (`0x40012f30`), yes/no (`0x400122b0`) and multi (`0x40012550`)
  handlers take 200, 201, 202 and Enter (not keypad Enter), each only
  with the cursor on the item and its focus flag set; the bind handler
  (`0x40015410`) answers 1 to every key, and starts waiting for one on 200
  or Enter with the cursor on the item.
- `Menu_HandleKey` works on the menu's focused item (the last item with
  flag `0x2` in the item list at menu `+0x50c`), not on the item under the
  cursor. Past the handler and `execKey`: `K_MOUSE1` and `K_MOUSE2` on a
  text item (type 0) run its `action` when its text rect (`0x40013630`)
  holds the cursor, on an edit field (4, 9, 15) start the edit when the
  item's rect holds it, on anything else run the `action` when the rect
  holds it. Enter, keypad Enter (`0xbf`) and `K_MOUSE3` (`0xca`) share a
  case: an edit field starts its edit and anything else runs its
  `action`, with no cursor test. With no focused item all of them do
  nothing. VERIFIED (decompile, the switch at `0x400138b6`). So the middle
  button is Enter, and the menu's own mouse pass at the top of
  `Menu_HandleKey` decides the item a click acts on: of two overlapping
  items the one that takes the focus on that pass. INFERRED (the
  combination).
- That mouse pass notes the focused item only among the items it does
  not skip (the cvar test on item `+0x120`, `0x400112d0`), and clears the
  focus only through that note. So an item that a cvar test hides after
  it took the focus keeps it, and a click on its rect runs its `action`
  though it is not drawn. INFERRED (the combination with "Taking the
  focus").
- After the focused item's key handler declines a press,
  `Menu_HandleKey` runs the menu's `execKey` script for keys 1..255 when
  one is set, through `Item_RunScript` (`0x400111d0`) with a stack item
  whose parent is the menu, and returns; the switch with Tab, the arrows,
  Enter and Esc (`onEsc`) comes after it. VERIFIED (decompile). A
  character key event reaches the UI as `key | 0x400` (section 19), above
  255, so an `execKey` fires on the key number: letters lower case.
  INFERRED (`execKey "A"` can never fire). The numbers are CoDMP.exe's
  keynames table (`0x57b8d0`, name and number pairs): Tab 9, Enter 13,
  Esc 27, Space 32, Backspace 127, the arrows up, down, left, right
  154..157, Alt 158, Ctrl 159, Shift 160, Caps Lock 151, Pause 153, Ins
  161, Del 162, Page Down 163, Page Up 164, Home 165, End 166, F1..F12
  167..178, the keypad 182..199 (`KP_HOME` .. `KP_EQUALS` in the table's
  order, `KP_ENTER` 191), `MOUSE1..5` 200..204, `MWHEELDOWN` 205,
  `MWHEELUP` 206, `SEMICOLON` 59. VERIFIED (the table).
- `Item_ListBox_HandleKey` (`0x40011da0`) without force returns 0 unless
  the cursor is inside the list (`0x4000fef0`) and the list has the focus
  bit; `Menu_HandleKey` calls the handler of the focused item only, after
  `Menu_HandleMouseMove` at the cursor, which focuses the list under it.
  VERIFIED (decompile). So the server list takes its keys only with the
  cursor over it. INFERRED (the combination). The list box handler does
  not take Enter, and Enter on a focused item that is not an edit field
  runs its `action` (`0x40013140`); the stock server list has none, only
  `doubleClick { uiScript JoinServer }`, so Enter over it does nothing in
  retail. VERIFIED (decompile and `joinserver.menu`).

### Text rects and the focus colour

- `Item_Text_Paint` (`0x400144d0`, item types 0 and 1) takes the item's
  text, or its cvar's value when it has no text, localizes an `@` key
  through the display context's `+0x20`, then runs `Item_TextColor`
  (`0x40013df0`) and `Item_SetTextExtents` (`0x40013bb0`) and draws at the
  text rect's x and y (item `+0xb8`, `+0xbc`). VERIFIED (decompile).
- `Item_SetTextExtents` keeps the text rect at item `+0xb8..+0xc4` (x, y,
  w, h): w is the text's width (display context `+0x14`, UI syscall
  `0x43`), h the font's height at the item's scale (`+0x18`, syscall
  `0x44`), x is `textalignx` (`+0xd8`) less the width (right aligned) or
  half of it (centred), where an owner draw's width adds the owner draw's
  own (`+0xc8`), and y is `textaligny` (`+0xdc`). `ToWindowCoords`
  (`0x40013b70`) then adds the border size (`+0x44`) to both when the
  item has a border (`+0x38`), and the rect's x and y. A rect with a
  width already set is kept, except for a centred owner draw or cvar
  item. VERIFIED (decompile). Exactly: it returns at once when the
  rect's width (`ftol` of `+0xc0`) is not 0, unless the alignment is 3
  or it is centred (1) on an owner draw (type 8) or an item with a cvar
  (`+0x114`); type 4, 9 and 15 items centred on a cvar add the cvar's
  text width to the one the alignment subtracts. VERIFIED (decompile).
  `Item_SetScreenCoords` (`0x4000fd00`) zeroes the width and height
  (`mov %ecx,0xc0(%eax)` at `0x4000fd38`); it runs from
  `Menu_PostParse` (`0x4000fe30`) and the item move and transition
  commands only. VERIFIED (asm). So a plain text item keeps the rect,
  and the spot it draws at, of its first paint for the rest of the
  load, whatever its text becomes. INFERRED (the combination).
  So the rect stands on the baseline, holds
  the text alone and is set only by a paint; an item never painted keeps
  a zero rect. INFERRED (the combination). What syscall `0x44` measures
  in CoDMP.exe was not traced past the renderer table (`0x418990` calls
  through `0x1432930`).
- `Item_Text_Paint` runs `Item_SetTextExtents` on the whole text, then
  takes window flag `0x40000` to `0x40014250` (lines split at `\r`) and
  `0x80000`, `autowrapped`, to `0x40014000`. VERIFIED (decompile).
  `0x40014000` measures the whole text W; past the rect's width it aims
  each line at `ftol(W / ceil(W / rect.w))` (`0x4001a800` runs under
  control word `0x1b3f`, rounding up, data at `0x40035254`), else at
  `ftol(rect.w)`. VERIFIED (asm, data). It then walks the text a
  character at a time: a space, tab, `\n` or the end marks a break
  point and takes the width measured on the step before as the line's
  width, then the buffer is measured; the line goes out at the last
  break point when the buffer is past the rect's width with a break
  point in it, at `\n` or the end, or at a space with the buffer past the
  aim. A `\r` is stored as a space. Each line writes the text rect's x
  (`textalignx`, less the line width or half of it) and y (`textaligny`
  plus the font height and 5 a line), runs `ToWindowCoords` and draws
  there; the width and height stay `Item_SetTextExtents`'. VERIFIED
  (decompile). So the lines of a long text come out about even, and an
  autowrapped item's text rect is the last line's spot with the whole
  text's width. INFERRED (the combination).
- Which paints set the rect. `Item_Paint`'s switch (`0x40016470`,
  `0x40016746`..) calls `Item_Text_Paint` for types 0 and 1; the edit
  field paint (`0x40014600`, types 4, 9 and 15) calls it first whatever
  the item holds; the yes/no (`0x40014820`, type 11), multi
  (`0x400149d0`, 12), slider (`0x400150a0`, 10), bind (`0x40015230`, 13)
  and owner draw (`0x400160a0`, 8) paints call it only when the item has
  text (`+0xe8` non-zero). VERIFIED (decompile). `Item_Text_Paint`
  returns before `Item_SetTextExtents` when the item has neither text
  nor a cvar. VERIFIED (decompile).
- The numbers in `Item_SetTextExtents` are ints. The text width
  (display context `+0x14`) is stored with `fildl` into `+0xc0`, and the
  centred and alignment 3 shift is the sum `cltd; sub; sar 1`
  (`0x40013dab`..`0x40013db0`), a halving rounded toward zero. VERIFIED
  (asm). CoDMP.exe's `R_TextWidth` (`0x4df310`, the refexport table's
  `+0xcc`, stored at `0x4b4c68`) takes the widest line's advance sum
  times the scale and ends in `jmp _ftol` (`0x4df52d` to `0x538be0`), a
  truncation. VERIFIED (asm). That the UI's text width trap reaches it is
  INFERRED (the cgame's text draw `0x4df9a0` calls it too).
- Alignment 3. The keep test lets alignment 3 through (`cmpl $0x3` at
  `0x40013c28`), and its measure (`0x40013cf2`..`0x40013d1c`) adds the
  width of the text argument (`[esp+0x11c]`, the same string
  `Item_Text_Paint` passes) to the text's own, then halves the sum as for
  a centred item. VERIFIED (asm). So a `textalign 3` item sits like a
  right aligned one and is measured at every paint. INFERRED (the
  arithmetic). `ui_mp/menudef.h` names 3 `ITEM_ALIGN_CENTER2` and 15
  `ITEM_TYPE_UPREDITFIELD`; the one stock use of either in the 1.1 paks
  is `ui_mp/cdkey.menu`'s key fields, an `ITEM_TYPE_UPREDITFIELD` with
  `ITEM_ALIGN_CENTER2` and empty text. VERIFIED (pak0).
- `Item_SetFocus` (`0x40011410`) tests a text item (type 0) against that
  rect moved up by its height (`0x40011494`: `lea edx, [esi+0xb8]`, then
  `y - h`), not against the item's rect. VERIFIED (asm).
  `Item_CorrectedTextRect` (`0x40013630`) hands out the same rect, moved
  up only when its width is not 0, for `Menu_HandleKey`'s click on a text
  item. VERIFIED (decompile).
- `Item_TextColor` draws an item with the focus flag (`0x2` in the
  window flags) in the menu's `focusColor`, pulsing by `sin(realTime /
  75)`; without it, a blinking text style pulses and anything else takes
  its `forecolor`; a cvar-disabled item takes `disableColor` (menu
  `+0x4fc`). VERIFIED (decompile). So the focus colour follows the
  focus flag, not the mouse: of two overlapping items the one the last
  move handed the focus to is lit while the cursor stays on both, and a
  focus step lights its target. INFERRED (the combination).
- `Menus_HandleOOBClick` ends with the display context's `+0xc4` called
  with 0 when no menu is visible (`0x400134d0`). `_UI_Init` fills `+0xc4`
  (`0x401c3da4`) with `UI_Pause` (`0x4000d1a0`): with 0 it clears the UI
  catcher bit (syscall `0x30` read, `and eax, 0xfffffffd`, syscall `0x31`
  write), runs `Key_ClearStates` (syscall `0x2f`) and sets `cl_paused`
  0; with 1 it sets `cl_paused` 1 and the catcher to 2. VERIFIED (asm).
  So a click that closes the last visible menu through `outOfBoundsClick`
  hands the keys and the mouse back to the game. INFERRED (the
  combination).

### vcod

- `crates/common/src/ui_menu.rs` parses `onFocus`, `leaveFocus`,
  `focusSound`, `maxCharsGotoNext`, `execKey` and `execKeyInt`, and
  `assetGlobalDef`'s `itemFocusSound` (`parse_menu_file`; the last file
  that sets it wins). The front end keeps each menu's focused item
  (`Ui::focus`), the items with the mouse (`Ui::over`, every overlapping
  one) and the focused menu (`Ui::focus_menu`, set by an open, an
  off-menu click and a close as "The focused menu" says; the stack keeps
  its order). A mouse move walks the open menus as `Display_MouseMove`
  does (`Ui::track_cursor`, `Ui::menu_mouse_move_at`'s two passes), and
  so does every menu open, after its `onOpen`; a key event runs the
  focused menu's pass alone and goes to that menu; a Tab step's target
  takes the focus on the press, then the pass runs at its corner. A click
  goes where "Which menu takes the mouse" sends it (`Ui::click_menu`),
  moving the focus first when that is another menu. Taking the focus
  runs `Menu_ClearFocus`'s `leaveFocus` pass, the item's `onFocus` (not a
  text item's) and `UiEffect::FocusSound`: the client plays the item's
  `focusSound` when the menu set has that alias, else `itemFocusSound`
  when it has that one. `play` and the focus sounds pick from the menu
  set (`AudioSystem::play_menu`, the aliases filtered for `menu`) in the
  front end and over a game alike; over a game a looping row is dropped,
  as the reaper above ends it.
- A close runs `onClose` with the menu still open, then takes it off the
  stack (`Ui::close_menu`); an `onClose` that closes its own menu again
  does nothing there, where retail would recurse. A menu with
  `outOfBoundsClick` closes on a click off its rect (onClose, then
  hidden where it stands on the stack, `Ui::hidden`) before the click
  goes on as above. A hidden menu draws and takes nothing, gets no
  `onClose` from a later close or `Menus_CloseAll`, and an `open` shows
  it on top again. When a menu with the focus closes and the stack's
  top is a hidden menu, the focus goes there, as `Menus_Close` sets bit
  `0x2` on the top whatever its flags, and `Menu_GetFocused` then finds
  no menu with both bits, so no menu takes the keys. In retail the next
  key then takes the UI down (`_UI_KeyEvent` `0x4000d6f0` with no
  focused menu: catcher bit off, `Key_ClearStates` unless the bypass
  set its flag, `cl_paused` 0; VERIFIED, decompile) with the menus still
  flagged visible. vcod does the same (`Ui::unfocused_key`, from a key,
  a button or a release): the key goes nowhere, the UI stops drawing and
  taking input, the stack stays, and the next menu open brings the UI
  back with those menus still up. When a close leaves no menu up the
  UI is inactive, and
  the app takes the mouse back for the view as after any close; keys
  pressed while the menu was up never reached the binds, so there is
  nothing for `Key_ClearStates` to let go of.
- A press works on the focused menu's focused item (`Ui::press`), after
  the menu's mouse pass: the item's type handler where retail's takes the
  key (`Ui::item_takes`, the list above), else the menu's `execKey`,
  else the first two buttons act on the item when the cursor is on its
  rect (a text item's text rect) and Enter, keypad Enter and
  the middle button act on it anywhere: an edit field starts its edit,
  anything else runs its `action`. A focused item a cvar test now hides
  keeps the focus and still takes the click.
- An `execKey` runs on retail's key number (`console::keys::number`,
  which the bind lists sort by too): for a key after the list keys and
  before Esc, Enter and the focus keys; for `MOUSE1..5` (200..204) when
  the focused item's type handler declines the button, in place of the
  click; for `MWHEELDOWN` and `MWHEELUP` (205, 206) after vcod's list
  scroll, in place of the focus step.
- The server's script menus are UI menus too (`UI_SetActiveMenu` cases
  10 and 11, section 13), so their `play` lines (`onFocus { play
  "mouse_over"; }` and `play "mouse_click"` in each `action` and
  `execKey` of the team and weapon menus) go through the same `+0x84`
  into the menu set. INFERRED (the combination). vcod runs them in the
  same `Ui` (section 13, "Where the script menus come from"), so focus,
  clicks, `execKey`s and their sounds are the front end's.
- Each paint records a text item's text rect (`Ui::text_rect`) where it
  draws the text; the focus and the click test it, and an item not yet
  painted has none, so it takes neither. The rect keeps its first
  paint's width and spot under retail's rule above (cleared at a load);
  an autowrapped item breaks its lines as `0x40014000` does
  (`Painter::wrap_lines`) and leaves the rect on its last line. vcod
  takes the font's tallest glyph as the height, truncated. Every item
  whose paint runs `Item_Text_Paint` keeps a rect (`paints_text`,
  `text_extents`): text items and buttons, the edit fields, and a slider,
  yes/no, multi, bind or owner draw with text; their value or slider bar
  follows the kept rect's width. The text of every item moves by the
  border size when the item has a border. An item is drawn in the focus
  colour when it has the focus (`Ui::focus`), not when it is hovered;
  vcod does not pulse the colour.
- In an edit field Tab, down and up step the focus as above and carry the
  edit to an edit field (`Ui::edit_step`); the keypad's arrows do so only
  with Num Lock off, since with it on they type digits. Mouse moves
  change nothing during an edit; a click ends it, runs the held-back
  mouse pass and then clicks. With `maxCharsGotoNext` the character that
  fills the field steps on the same way; vcod has no cursor inside a
  field and no overstrike mode, so only that case is done.
- The server list takes the arrows, Home, End and the page keys only with
  the cursor over it. Enter joining the selected server anywhere in the
  browser (unless the focused item's handler takes Enter), and an arrow
  with nothing selected picking the first row, are vcod's own.

## 21. Video modes, fullscreen and wide windows

Retail's:

- `R_Register` (CoDMP.exe 0x4b3300) registers `r_mode` "3",
  `r_fullscreen` "1", `r_customwidth` "1600", `r_customheight` "1024"
  and `r_customaspect` "1", each with flags 0x21 (archive, latch).
  VERIFIED, the calls at 0x4b3718..0x4b3786 and the default strings at
  0x568730, 0x5685e4, 0x558064 and 0x55804c.
- The mode table at 0x5716c0 holds 13 rows of name, width, height and
  pixel aspect, the count at 0x571790: 320x240, 400x300, 512x384,
  640x480, 800x600, 960x720, 1024x768, 1152x864, 1280x1024, 1600x1200,
  2048x1536, 856x480 "(wide)" and 1920x1200 "(wide)", every pixel aspect
  1.0. VERIFIED, the table's bytes.
- `R_GetModeInfo` (0x4b0ec0) fails a mode below -1 or past the table and
  takes mode -1's size from `r_customwidth` and `r_customheight` and its
  aspect from `r_customaspect`. INFERRED, its branches.
- The options screen's Video Mode list is the stock
  `ui/options_graphics.menu` (`localized_english_pak0.pk3`), a
  `cvarFloatList` on `ui_r_mode`, so its rows are the menu file's, not the
  engine's. VERIFIED, the menu file.
- Alt+Enter: the window procedure's `WM_SYSKEYDOWN` (0x104) arm tests
  `wParam` 0xd at 0x466e52. VERIFIED, the compares at 0x466c65 and
  0x466e52. Unless `cls.state` (0x155f2c0) is 4, a map loading, it sets
  `r_fullscreen` to `!r_fullscreen->integer` (`Cvar_SetValue` 0x439960
  with the name at 0x55cd5c) and queues `vid_restart\n` (0x55bc8c).
  INFERRED, the branch and the call order.
- `gfxinfo` (0x4b2e40) prints "fullscreen" only when `r_fullscreen` is
  exactly 1. INFERRED, its index expression. Retail's fullscreen is a
  display mode change to the `r_mode` size.

vcod's (`crate::video`, `apply_video_mode` in `main.rs`):

- `r_mode` reads the same 13 rows. vcod registers `r_mode` -1 and
  `r_customwidth` / `r_customheight` 0, and -1 with either at 0 keeps the
  window's own size (1600x900 logical at first start), so a wide size is
  `r_mode -1` with the custom pair (`r_customaspect` is registered and
  ignored). None of them latches; `vid_restart` reads them.
- `r_fullscreen 1` is a borderless window over the monitor the window is
  on, at the desktop's resolution, whatever `r_mode` says. `r_fullscreen
  2` is exclusive: the monitor switches to its video mode closest to the
  `r_mode` size (the monitor's own size when `r_mode` names none): the
  same size at its highest refresh and colour depth, else the least
  difference in area, then in aspect. Wayland has no exclusive mode, so 2
  is borderless there. Any other non-zero value reads as 1.
- At start-up the window opens windowed at the asked size and goes full
  screen once it knows its monitor.
- Alt+Enter runs `r_fullscreen <!r_fullscreen>; vid_restart`, so 2 goes
  to windowed and windowed to 1, as retail's boolean flip.
- The 3D view, the HUD and the menus on a window that is not 4:3 are
  `vc_widescreen`'s (cod11-hud-protocol.md section 10, `camera::view_fov_x`).


## 22. The message of the day

A server has two motds and a retail client draws one of them. 1.1 is
`cgame_mp_x86.dll` and `ui_mp_x86.dll` from the 1.1 install, 1.5 their
1.5 builds, UO `uo_cgame_mp_x86.dll` and `uo_ui_mp_x86.dll`.

- `g_motd` goes out as configstring 14 (UO 15), written by
  `SP_worldspawn` (`cod11-map-cycle.md` 4.5). No client module reads it.
  1.1's cgame keeps the configstring offsets at `0x301cc0a0`; slot 14's
  word `0x301cc0d8` appears in no instruction (objdump) and nowhere in the
  decompile. VERIFIED. Every call to the getter `0x30021b00` takes a
  computed range index (`+0x20c`, `+0x4bc`, `+0x1c`), and
  `CG_ConfigStringModified` (`0x3002c6b0`) has cases for 0, 3, 5, 6, 8,
  11, 12, 13, 15..18 and ranges, none for 14. VERIFIED (decompile). 1.5's
  table is at `0x301cfb20` and its slot 14 word `0x301cfb58` appears in no
  instruction. VERIFIED. UO's `CG_ConfigStringModified` (`0x30038e70`)
  reads 14 as its start time and has no case for 15. VERIFIED (decompile).
- The UI reads configstrings through syscall `0x35` only for 0 (the
  serverinfo) and for the script menu names. 1.1: wrapper `0x40019510`,
  called with index 0 at `0x40008f12`, `0x4000b657` and `0x4000e6cc`;
  the display context's getter `0x4000ec00` (slot `0x401c3d54`, +0x74) is
  called only at `0x4001115d`, over `0x49c + i`. 1.5: index 0 at
  `0x40009322`, `0x4000bad7`, `0x4000eebc`; getter `0x4000f450` called
  only at `0x4001197d`, over `0x49c + i`. UO: index 0 pushed at
  `0x4000a42a`, `0x4000d6d3`, `0x40011063`; its getter is called only at
  `0x400141be`, over `0x535 + i`. VERIFIED (objdump). So configstring 14
  (UO 15) is drawn by nothing, and vcod draws it nowhere either.
- `g_motd` registers with no flags (`crates/server/src/cvars/registry.rs`,
  `tools/capture_cvars.py`), and the retail `-motd` configstring captures
  (`crates/server/tests/fixtures/configstrings/*-motd*.txt`) hold no motd
  key in serverinfo (0). VERIFIED. So `getinfo` and `getstatus`, built
  off the serverinfo cvars, carry none and the browser's server info
  popup (section 10) cannot show one. INFERRED.
- The motd a player reads is the server's `scr_motd`, through the
  140/204 cvar mirror (`cod11-hud-protocol.md`, the table's 140..203 row):
  - 1.1's `_teams.gsc` runs `makeCvarServerInfo("scr_motd", "")`, so the
    mirror carries `scr_motd` (180, value 244 in the capture). The four
    team menus (`ui_mp/scriptmenus/team_americangerman.menu`,
    `team_britishgerman`, `team_russiangerman`, `team_germanonly` in
    `pak0.pk3`) draw it in `text_motd`: rect
    `208 24 216 216`, `ORIGIN_MENUWINDOW`, `autowrapped`, `cvar
    "scr_motd"`, `UI_FONT_NORMAL`, `textscale .24`, `textaligny 20`,
    white, `decoration`. VERIFIED (the files, the capture).
  - 1.5's `_teams.gsc` (`pak9.pk3`) copies `scr_motd` into `ui_motd` and
    mirrors that (186, value 250), and re-copies it when `scr_motd`
    changes in its cvar loop; UO's (`pakuo01.pk3`) does the same (225,
    value 353). The menu is `serverinfo_<gametype>.menu` (1.5 `pak8.pk3`,
    UO `pakuo03.pk3`), the one the gametypes open before the team menu:
    `text_motd` at `12 66 190 213`, `ORIGIN_MENUWINDOW`, `autowrapped`,
    `cvar "ui_motd"`, `UI_FONT_NORMAL`, `textscale .24`, `textalignx 8`,
    `textaligny 16`, white, `decoration`, below `ui_hostname`. VERIFIED
    (the files, the captures).
  - The mirror is set by the cgame's `0x3002bc60` (UO `0x30038430`),
    which stops at the first empty name. VERIFIED (decompile). An empty
    `scr_motd` still has its name, so it mirrors as an empty value and the
    item prints nothing. vcod: `ClientCvars::get` and `::all` stop there
    too.
- The connect screen (`UI_DrawConnectScreen`, 1.1 `0x4000e4e0`) prints
  `Info_ValueForKey(updateInfoString, "motd")` (key `0x4002e148`, the
  `uiClientState_t` field at +0x40c, `0x4000e75b`) centred at x 320, y
  460, scale 0.5, white (`0x4002d7dc`), in every connection state.
  VERIFIED (asm). That string is the Activision motd service's reply, not
  the game server's: `CoDMP.exe` sends `getmotd` to
  `codmotd.activision.com` (`0x40f110`) and `CL_MotdPacket` (`0x410680`)
  copies the reply into `updateInfoString` and its `motd` into
  `cl_motdString`. VERIFIED (decompile). 1.5's and UO's executables
  carry no `getmotd` string (`cod15-protocol-diff.md`), so their connect
  screens (1.5 key ref `0x4000ef4b`, UO `0x400110f1`) print an empty
  line. INFERRED.
- `UI_SERVERMOTD`'s owner draw (1.1 `0x4000b800`) scrolls `cl_motdString`,
  but every stock `joinserver.menu` (1.1 `pak0`, 1.5 `pak8`, `paka`) has
  the item inside `/* */` (section 4). VERIFIED (the files).

vcod draws the script menus' `text_motd` as any cvar text item
(`Ui::label`, `Item_Text_AutoWrapped_Paint`'s wrap), off
`ClientCvars::all`, refreshed every frame a menu is up, so a live
`scr_motd` change reaches an open 1.5 or UO serverinfo menu once the
gametype's loop re-copies it. vcod has no retail connect screen and sends
no `getmotd`, so it draws neither the Activision motd nor `cl_motdString`.
The tests are `the_team_menu_shows_scr_motd_on_1_1` and
`the_serverinfo_menu_shows_ui_motd_on_1_5_and_uo`
(`crates/client/src/frontend/mod.rs`), on the retail captures.
