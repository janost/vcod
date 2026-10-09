# The front end: main menu and server browser (CoD 1.1 MP)

What the retail main menu and server browser are made of, and the master
query behind the list. Sources: the stock menu files in the 1.1 paks
(`pak0.pk3`, `localized_english_pak0.pk3`), `ui_mp_x86.dll` 1.1 (image base
`0x40000000`), `CoDMP.exe` 1.1 (image base `0x00400000`; md5s in
`cod11-events-and-fx.md`), and live captures against
`codmaster.activision.com` and the retail 1.1d server on 2026-10-08 and
2026-10-09.
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
  (this host's firewall drops a broadcast's local copy): one
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

- vcod draws the main menu, the browser and its popups (sections 6-11),
  the options set (section 14), the quit popup and the error popup from the
  stock files with their layout, the main menu again over a game (section
  13), and refuses every other menu with a console line. A button whose
  script would close its own menu and then open a refused one is refused
  whole, so `Start New Server` leaves the main menu up.
- Ping pacing (32 `getinfo`s in flight, a 1.5 s timeout, 5 s for the master)
  is vcod's own; retail's numbers were not measured.
- Text is placed as RTCW's `Item_SetTextExtents` does: baseline at
  `rect.y + textaligny`, x at `textalignx` moved left by the width (right
  aligned) or half of it (centred); an owner draw with no label draws at
  `textalignx` whatever its alignment. INFERRED from the RTCW lineage, and
  the screenshots match the stock layout by eye.
- Not done: create server, mods, the CD key popup, the game type filter
  (`UI_JOINGAMETYPE` always prints `EXE_ALL`), `EXE_REFRESHTIME` and
  `ui_lastServerRefresh_*` (the line reads vcod's own count once a refresh
  ends), the Internet list's cache in `servercache.dat`, the scroll bar, the
  menu cursor image, keyboard focus on buttons, `focusColor`'s pulse and the
  map preview.

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
  screen), and `r_gamma`, read every frame and applied as retail's gamma
  ramp in a final pass (`cod11-gamma.md`). vcod registers `r_mode` -1 (its own window size) and
  `r_fullscreen` 0 instead of retail's 3 and 1.
- Stored but inert (archived, so a choice survives): `cl_freelook`,
  `m_filter`, `cg_drawCrosshair`, `cg_drawStatus`, the texture, picmip,
  `r_ignorehwgamma`, LOD, dynamic light, swap interval and NVIDIA fog cvars,
  `mss_khz`, `mss_3d_provider`, `cg_marks`, `cg_brass`, `cg_blood`.
  `snd_restart` and `setRecommended` are unknown commands. Binds to
  commands vcod lacks (`+lookup`, `+strafe`, `mp_QuickMessage`,
  `screenshotJPEG`, the stance toggles) are stored and print `Unknown
  command` when pressed.
- Not shown: the language picker (`cl_languagesavailable` reads 1, so its
  `hideCvar` hides it), NVIDIA fog (no `r_nv_fog_available`), Driver Info
  (no `developer`). The CD key button is refused.
