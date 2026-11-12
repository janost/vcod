# CoD4 1.7: the front end

The main menu a bare `vcod` opens on a CoD4 install, and the screens behind
it. The menus are the `ui_mp` zone's `menuDef_t`s, read by the zone walk
(`vcod_cod4::assets::ui`, `vcod_cod4::scriptmenu`) and run by the same script
menu engine the server's menus use in a game (`play::cod4_menus`,
docs/research/cod4-hud.md 12), stacked; `cod4_front` holds the front end
and `App`'s glue. `cargo run -p vcod-cod4 --example menu_dump -- ui_mp
<menu>` prints a menu's items, scripts and `soundName`. Gates: the
`cod4_front_*` tests in the client (`cod4_front.rs`, `play/cod4_menus/front.rs`),
which drive the front end through its keys and mouse over the install's
zones (`cod4_front_join_game_joins_a_lan_server_and_drops_back_to_the_menu`
against vcod-server on loopback), and the `*_renders_*offscreen` ones, the
real renderer drawing the main menu and the join menu headless (they
draw nothing without a Vulkan, DX12 or Metal adapter).

Sources: the `ui_mp`, `code_post_gfx_mp` and `common_mp` zones of the 1.7
install, read by the zone walk, and the 1.7 binaries. Each claim carries
its own label; one labelled INFERRED that names no 1.7 address is not yet
confirmed on 1.7.

## 1. The zones

- The front end's assets come from `code_post_gfx_mp`,
  `localized_code_post_gfx_mp`, `ui_mp`, `common_mp` and
  `localized_common_mp`, loaded in that order. VERIFIED (iw3mp.exe
  0x46ccb0 stores the six names `code_post_gfx_mp`, `ui_mp`, `common_mp`,
  `localized_code_post_gfx_mp`, `localized_common_mp` and `mod` when a
  mod is loaded; 0x5f3c00 loads the two code zones, then the rest from
  `ui_mp` on in a second batch). The in-game load at 0x46cd60 stores the
  same names with no `ui_mp`. VERIFIED. Under a mod, `mod.ff` joins the
  first batch after the code zones, ahead of `ui_mp` and `common_mp`
  (0x5f3c37..0x5f3c62). VERIFIED. Whether its assets win over the later
  zones' rests on the asset table's rule for a name loaded twice, not
  read here. INFERRED. vcod walks the five in that order and a mod's
  `mod.ff` last, a later zone's asset over an earlier's
  (`vcod_cod4::hud::FRONT_ZONES`, `HudAssets::load_front`).
- `ui_mp` holds the main menu (`main`, `main_text`), its screens
  (`createserver`, `mods_menu`, `pc_join_unranked`, `player_profile`,
  `main_options`, `main_controls`, `menu_challenges`, the `options_*`
  pages) and its popups (`quit_popmenu`, `single_popmenu`, the
  `profile_*_popmenu`s, `password_popmenu`, `serverinfo_popmenu`,
  `filter_popmenu`, the favourites' popups), 290 menus with the create a
  class ones. VERIFIED (the zone walk). `error_popmenu` and `connect` are in
  `code_post_gfx_mp`. VERIFIED (the walk).
- `main`, `main_text` and `createserver`, among 17 `ui_mp` menus, carry
  `soundName` (`menuDef_t` +224) `music_mainmenu_mp`. VERIFIED (the walk).
  `Menus_Open` plays a menu's `soundName` as it opens. INFERRED. vcod plays it (section 8).

## 2. The menu stack

- `Menus_Open` takes the focus from every open menu (the focused item's
  `leaveFocus` runs), takes the menu off the stack if it is on it and puts
  it on top (16 deep), focuses it and runs its `onOpen`. `Menus_Close` runs
  the menu's `onClose` (`Menu_RunCloseScript`, when the menu is visible),
  takes it off the stack wherever it is, and gives the focus to the
  topmost visible menu left if it had it. `Menus_CloseAll` runs
  `Menus_Close` on every loaded menu in load order. INFERRED.
- `Menu_PaintAll` paints the stack bottom up from the topmost `fullScreen`
  menu (`menuDef_t` +160), and, with no fullscreen menu up, every visible
  loaded menu off the stack first. INFERRED. vcod paints
  the stack only (`ScriptMenus::drawn`), over black.
- vcod keeps one stack for the front end and the server's menus in a
  game alike (`ScriptMenus::push_open`, `close_named`); a server's `t`
  starts a fresh one with its menu, and `u`, `L` and a `disconnect` clear
  it (cod4-handshake.md 12.4). Its `close_all` runs each open menu's `onClose` bottom up and
  leaves nothing open, the menus those scripts opened included; retail's
  load-order walk can leave one a script opened up.
- `UI_SetActiveMenu(UIMENU_MAIN)` opens `main`, then `error_popmenu` over
  it when `com_errorMessage` is set and is not `;`. INFERRED. vcod closes every menu first
  (`Cod4Front::open_main`).
- `main` draws nothing: its `onOpen` opens `main_text`, runs `uiScript
  addPlayerProfiles`, opens `player_profile` when `com_playerProfile` reads
  `""` and when `ui_playerProfileCount` is not 1, runs `uiScript
  stopRefresh` and sets `ui_showEndOfGame` 0. VERIFIED (the walk, `onOpen`).
- A `uiScript` takes the words its handler parses: `openMenuOnDvar`,
  `openMenuOnDvarNot`, `closeMenuOnDvar` and `closeMenuOnDvarNot` three
  (0x5459e0), `update` one (0x546b2c), `ServerSort` and `setPbClStatus`
  an int each (0x5467aa, 0x546b70), the rest none, `sortPlayerProfiles`
  included (it flips the sort, 0x546241); the next word starts the next
  command. VERIFIED (`UI_RunMenuScript`, iw3mp.exe 0x545bc0, through
  `tools/re/cod4_menu_commands.py`). A command in `commandList` (0x6b4940,
  42 rows) reads what its handler does: one word for `fadein`, `fadeout`,
  `showMenu`, `hideMenu`, `setbackground`, `ingameopen`, `ingameclose`,
  `setfocusbydvar`, `openforgametype` and `closeforgametype`, three for
  the `scriptMenuRespondOnDvar*` three, none for `focusfirst`,
  `feederTop`, `feederBottom` and `getautoupdate`; `setcolor` a word and
  up to four numbers, `setitemcolor` two and up to four; the stat
  commands a list in parentheses or bare values, `statClearPerkNew` one,
  `statClearBitMask` two (0x54cc90), `statsetusingtable` an index and a
  `tableLookup` expression. VERIFIED (the same dump: the handlers' calls
  to the word readers 0x54b510, 0x571400 and 0x558f40). vcod skips the
  commands it does not run by those counts (`scriptmenu::COMMANDS`).
  `main_text`'s
  scripts run commands with no `;` between them (`"uiScript" "closeJoin"
  "close" "self"` in `pc_join_unranked`'s `onEsc`). VERIFIED (the walk).

## 3. Profiles

- A profile is a directory `players/profiles/<name>/` under the install
  root, its config `config_mp.cfg` there; `players/profiles/active.txt`
  names the active one, whose first word `Com_SetInitialPlayerProfile`
  reads into `com_playerProfile` at start when that directory exists.
  INFERRED.
- `main`'s `onOpen` opens `player_profile` when `com_playerProfile`
  reads `""` or `ui_playerProfileCount` is not 1; `player_profile`'s
  opens `profile_create_popmenu` when the count is 0, whose `onClose`
  opens `profile_must_create_popmenu` while it is still 0, whose
  `onClose` opens the create popup again. VERIFIED (the walk, the four
  menus' `onOpen` / `onClose`). The create popup's name field
  (`ui_playerProfileNameNew`, `ITEM_TYPE_VALIDFILEFIELD`) runs `uiScript
  createPlayerProfile` and `close self` on Enter (its `onAccept`) and on
  OK. VERIFIED (the walk). `profile_del_sure_popmenu`'s Yes runs
  `deletePlayerProfile`, then `openMenuOnDvar ui_playerProfileCount 0
  profile_must_create_popmenu`. VERIFIED (the walk).
- `UI_CreatePlayerProfile` takes `ui_playerProfileNameNew`'s first 31
  bytes and clears the dvar; it opens `profile_create_too_many_popmenu`
  at 64 profiles, `profile_exists_popmenu` for a listed name in any case,
  `profile_create_fail_popmenu` when `Com_NewPlayerProfile` cannot make
  `players/profiles/<name>/`; else the name is listed, sorted, counted
  and picked (`UI_SelectPlayerProfileIndex`, every visible menu's feeder
  24). `UI_DeletePlayerProfile` removes `ui_playerProfileSelected`'s
  directory tree (`Com_DeletePlayerProfile`, which clears
  `com_playerProfile` when it was the active one) or opens
  `profile_delete_fail_popmenu`; the row after it is picked, or none.
  `UI_LoadPlayerProfile` runs `Com_ChangePlayerProfile` on
  `ui_playerProfileSelected`: for another profile whose directory exists
  it writes the name to `players/profiles/active.txt`, disconnects,
  resets every dvar and runs `Com_SetPlayerProfile`, which sets
  `com_playerProfile`, runs the profile's `config_mp.cfg` and sets `name`
  to the profile's name when `name` is empty (registered `""`). INFERRED.
- vcod runs those scripts as written (`play::cod4_menus::front`): the
  list is `players/profiles/`' directories (`front::profile_list`, feeder
  24, `player_profile`'s list; VERIFIED, the walk's `feeder`), a pick
  sets `ui_playerProfileSelected`, create and delete make and remove the
  directory, and a load hands the client `front::Request::LoadProfile`.
  The client (`cod4_front::change_profile`) writes `active.txt`, runs the
  profile's `vcod_mp.cfg`, else its `config_mp.cfg`, gives `name` the
  profile's name when it has no config or an empty `name`, and writes
  `vcod_mp.cfg` there at once. It resets no dvar first: what the config
  does not set keeps its value. With no profile on disk the main menu
  asks for one as 1.7's does; vcod has no stand-in profile. A deleted
  profile that held the config sends the config back to
  `main/vcod_mp.cfg` until another loads. A name with a path separator
  is refused (the file field cannot type one).
- vcod's config for a CoD4 install with an active profile is
  `players/profiles/<name>/vcod_mp.cfg`, read from 1.7's `config_mp.cfg`
  there until vcod has written its own; with none, `main/vcod_mp.cfg`.
  vcod never writes `config_mp.cfg`. Every archived change writes the
  config, so the one on disk is current when the client exits.

## 4. The main menu's rows

`main_text`'s rows, top down, each an item whose `action` plays
`mouse_click` and opens its screen. VERIFIED (the walk, each row's
`textExp` and `action`). Status is vcod's.

| Row | Opens | Its scripts and dvars | vcod |
|---|---|---|---|
| Auto-Update (only with `cl_updateAvailable`) | `auconfirm` | `getautoupdate` | hidden; vcod sets no `cl_updateAvailable` |
| Join Game | `pc_join_unranked` over `main_text` | `UpdateFilter`, `RefreshServers`, `RefreshFilter`, `ServerSort`, `JoinServer`, `ServerStatus`, `addFavorite`, `closeJoin`; feeder 2 | the server browser (cod4-handshake.md 12); Join Server joins behind the connect screen (section 7) |
| Start New Server | `close self`, `createserver` | `loadArenas`; `ui_netGameType` (owner draw 245), the map list (feeder 4, sets `ui_currentNetMap`), `ui_dedicated` (multi No / LAN / Internet), `sv_hostname`, `sv_maxclients`, `sv_minping`, `sv_maxping`, `g_password` (edit fields), `sv_voice`, `scr_teambalance`, `g_allowvote`, `sv_punkbuster` (yes/no); the game type pages `settings_quick_<gametype>`; Start and the list's double click run `StartServer` | game type, map and Dedicated work; the edit fields and yes/no rows set their dvars in the UI's table (`front::ui_table_dvar`); Start starts `vcod-server --game cod4` with them (section 5) |
| Select Profile | `player_profile` | `addPlayerProfiles`, `selectActivePlayerProfile`, `loadPlayerProfile`, `sortPlayerProfiles`, `createPlayerProfile`, `deletePlayerProfile`; feeder 24; `ui_playerProfileAlreadyChosen`, `com_playerProfile` | lists, picks, creates, deletes and loads (section 3) |
| Create a Class | `pc_cac_popup` (`execnow statset 260 1` first) | unlocked when `stat(260) >= 1`, else the greyed row | reads the profile's `vcod_mpdata`: greyed on a fresh profile, open from rank 3 (below); the editor's stat commands run (4.1) |
| Rank & Challenges | `menu_challenges` | `statGetInDvar` rows, `ui_tier_num`, `ui_table_slot` | opens and draws off the profile's stats |
| Controls | `main_controls`, its pages `options_look`, `options_move`, `options_shoot`, `options_misc`, `controls_multi` | the bind rows and their dvars | the bind rows and the mouse options work on vcod's console as in a game (cod4-hud.md 12.6); the rest greyed |
| Options | `main_options`, its pages `options_graphics`, `options_graphics_texture`, `options_sound`, `options_voice`, `options_game`, `options_multi` | the option dvars, `vid_restart_popmenu`, `snd_restart_popmenu` | the controls with a vcod cvar (`r_gamma`, `sm_enable`, `snd_volume`, `fx_marks`, `cl_allowDownload`, the player name, the mouse) work; the rest greyed (section 9) |
| Mods | `close self`, `mods_menu` | `loadMods`, feeder 9, `RunMod`, `ClearMods` | lists `mods/` and runs one (section 6) |
| Single Player | `single_popmenu` | `startsingleplayer` | the popup works; Yes does nothing (no single player) |
| Quit | `quit_popmenu` | Yes runs `uiScript quit`, No and Esc `close self` | quits |

- Create a Class's rows: the button (`MPUI_CREATE_A_CLASS`) shows when
  `stat(260) >= 1`, the greyed copy and its `MPUI_DESC_CREATE_A_CLASS_LOCKED`
  hint when it is below 1, and the `specialty_new` mark when it is 1 among
  other tests. VERIFIED (the walk). Stat 260 is `feature_cac` in
  `mp/statstable.csv` (row `197,260,feature,PERKS_CREATE_A_CLASS,
  feature_cac`), and `mp/ranktable.csv`'s rank 3 (`lcpl1`, 270 XP to
  480) unlocks it. VERIFIED (the `code_post_gfx_mp` tables). `_rank.gsc`'s
  `unlockFeature` sets the stat to 2 ("newly unlocked") through
  `setStat`. VERIFIED (the rawfile). The row's `statset 260 1` clears the
  mark.
- `statset <index> <value>` (`iw3mp.exe` 0x579d50, registered at
  0x579f9e) takes two arguments read with `atoi`, else prints `statset
  usage: statset <index> <value>` (0x6f5d34). Indices 256..279 store at
  once; 200..250 pass 0x4fc440 first, which `Com_Error`s `trying to set
  invalid create-a-class stat.  You can't set stat %d.` (0x6e0fc0)
  outside 200..249; any other index stores 0 and `Com_Error`s anything
  else with `Trying to set a server-only stat %d.` (0x6e1004). The store
  (0x579ce0) reads the stat first and writes a changed one into the
  `LiveStorage` buffer (0x579c70), raising the dirty flag 0xcc1ac91.
  VERIFIED (the disassembly); the two error calls' severity is INFERRED.
  The menus' 1610 `statset`s use 202..249, 260 and 270..279 only.
  VERIFIED (`menu_dump`). vcod (`ScriptMenus::stat_set`) stores the same
  ranges, checks 200..249 as 4.1 says, and refuses the rest with the
  message and no drop.
- vcod's front end reads the active profile's `vcod_mpdata` each time the
  main menu comes up and when a profile loads or is deleted
  (`Cod4Front::reload_stats`), a fresh profile's block without one, and
  writes a `statset` back to it two seconds on or as a join starts. A
  game's `N`s reach the same file (cod4-netfields.md, "The stats file").
  Gate: `cod4_front_create_a_class_unlocks_off_the_profiles_stats`.

`main_text`'s `onOpen` runs `selectStringTableEntryInDvar
mp/didyouknow.csv 0 didyouknow`. VERIFIED (the walk). vcod picks the row
off the clock; retail picks one at random. INFERRED.

### 4.1 Create a Class

`pc_cac_popup` opens one `menu_cac_<class>` per custom class
(`assault`, `specops`, `heavygunner`, `demolitions`, `sniper`: stats 200,
210, .. 240, cod4-netfields.md 10), whose popups pick the class's
weapons, attachments, camo, perks and special grenade, rename it
(`<class>_pc_rename`, an edit field on `customclass<n>`) and reset it to
a stock class's template. VERIFIED (`menu_dump ui_mp`, 290 menus).
`ui_mp`'s scripts run 2285 `statsetusingtable`s, 1900 `statClearBitMask`s and 230
`statClearPerkNew`s, and their console lines `statset`,
`statGetInDvar`, `set`, `setFromLocString`, `updategamerprofile`,
`uploadstats`, `wait` and `closemenu`. VERIFIED (counts off the dump).

- The three stat commands are `commandList` rows (0x6b4940): handlers
  0x54c940, 0x54d060 and 0x54cee0. VERIFIED
  (`tools/re/cod4_menu_commands.py`). Each reads its words through
  0x54b510: an opening `(` is skipped and each argument past the `,`s
  before it, then the `)`s are read and the word after them handed back
  (`Com_UngetToken`, 0x570c70, whose `UngetToken called twice` is
  0x6f434c). VERIFIED (the disassembly). The word compares (0x571a90,
  length 0x7fffffff) ignore case. INFERRED (the menus write both
  `tablelookup` and `tableLookup`). The stat writes go through
  `LiveStorage_SetStat` (0x579c70): a byte stat under 2000 outside 0..255
  is a `Com_Error` (`Trying to set index %i (which is a byte value) to
  invalid value %i`, 0x6f5cf0), a word to 3497, nothing past; a changed
  stat raises the dirty flag 0xcc1ac91. VERIFIED. Before the stats are
  fetched (0xcc1ac90 clear) each prints `Tried to set stat index %i
  before we have obtained player stats` (0x6f5ca8) and writes nothing.
  VERIFIED.
- `statSetUsingTable ( <index> , tableLookup ( <table> , <column> ,
  <value> , <ret> ) )`: `atoi` of the index; the words `tablelookup`,
  `,` and `(` skipped to the table's name, found with
  `DB_FindXAssetHeader(0x20, ..)` (0x489570); `atoi`s of the column and
  the return column; `StringTable_Lookup` (0x569b30) and `atoi` of its
  string. VERIFIED. The lookup walks the rows from the last
  (`StringTable_LookupRowNumForValue`, 0x569aa0) and returns column
  `ret` of the first match, `""` with no match, no table (`Unable to find
  the lookup table in the fastfile, aborting lookup`, 0x6f3ac0) or a
  column out of range. VERIFIED. Index 2400..2402 stores at once (the
  menus' scratch: the picked weapon's stat number, read back with
  `statGetInDvar` for `statClearBitMask`); any other outside 200..249 is
  a `Com_Error` (`statsetusingtable should only be used for
  create-a-class.  You can't set stat %d`, 0x6ebc48); 200..249 pass
  0x4fc440 below. VERIFIED. A camo row passes its return column as
  `"11"`, a quoted word, which `atoi` reads the same. VERIFIED (the
  dump).
- `statClearBitMask <index dvar> <mask dvar>` (helper 0x54cc90): each
  word names a dvar (0x56b5d0), read as its word +0xc for types 5 and 6,
  else `atoi` of the string there, 0 when there is no such dvar; the
  stat at the first loses the second's bits. No range check past
  `LiveStorage_SetStat`'s. VERIFIED. Types 5 and 6 are the int and enum
  dvars. INFERRED. The menus clear 65536 off a weapon's stat as it is
  picked or left, and a camo's bit (16777216 for Desert, 0 for None) as
  it is picked. VERIFIED (the dump).
- `statClearPerkNew ( <perk> )` (helper 0x54ce00): the perk's row in
  `mp/statstable.csv` (0x6ebcd4) by column 4, its column 1 the stat;
  outside 150..298 a `Com_Error` (`statClearPerkNew: Invalid perk index
  %d for %s`, 0x6ebce8); a stat with neither of the bits 1 and 2 a
  `Com_Error` (`statClearPerkNew: perk %s[%d] isn't unlocked.`,
  0x6ebd18); else the stat becomes `(stat & ~2) | 1`. VERIFIED. So 1
  is the perk unlocked and 2 new. INFERRED (the command's name and
  message).
- The create-a-class check, 0x4fc440 (`statset` and `statsetusingtable`
  on 200..249): outside 200..249 a `Com_Error` (`trying to set invalid
  create-a-class stat`, 0x6e0fc0). Then by the index's last digit (jump
  table 0x4fc52c): 1 wants a primary, 3 a secondary (a primary too when
  the class's perk 2, its stat `+ 6`, is 166, Overkill; 0x4fc3d0), 5, 6,
  7 a perk of that slot, 8 a special grenade; 0, 2, 4 and 9 (the class's
  first stat, the attachments, the camo) are not checked. The value must
  be a row of the item table at 0x724bb8 (61 rows of number, rank,
  slot bits 1 primary, 2 secondary, 4 perk 1, 8 perk 2, 0x10 perk 3,
  0x20 grenade; `tools/re/cod4_cac_items.py`), else `No create-a-class
  item found at index %d` (0x6e0f08); its rank at most the profile's,
  else `Create-a-class item %d is too high level %d < %d.` (0x6e0f34);
  its slot bits the slot's, else `Create-a-class item %d is not the
  expected type for this slot (0x%x & 0x%x) == 0.` (0x6e0f68). Each a
  `Com_Error` with code 1. VERIFIED (the disassembly). The "too high"
  call pushes the item and the compare's result, not the two ranks.
  VERIFIED; what it prints past them is not checked.
- The profile's rank (0x4749d0) is a binary search of
  `mp/rankTable.csv` (0x6d2ad0) for the row whose columns 2 and 7 hold
  stat 2301 (XP), 0 XP before the stats are fetched; its column 0.
  VERIFIED (the disassembly).
- `setfromlocString <dvar> <text..>` (0x501d90, registered at 0x5028a7):
  fewer than two arguments print `USAGE: setFromLocalizedString
  <variable> <string>` (0x6e241c), a name 0x569df0 refuses `invalid
  variable name: %s` (0x6e2250); the words from the second joined, a
  leading `@`
  localized (`SEH_LocalizeTextMessage`, 0x538d30) when the key has a
  non-empty string; then `Dvar_SetFromStringByName` (0x56d110).
  VERIFIED. The templates name the class `@CLASS_CUSTOM<n>`.
- `uploadStats` (0x579f60 calling 0x579b40): with the stats fetched and
  dirty, writes the profile's `mpdata` and prints `Successfully wrote
  stats data` (0x6f5c88), else `Unable to write stats: %s.` (0x6f5c6c).
  VERIFIED. The class pages' Esc runs it. `updategamerprofile` names no
  command in 1.7's PC build (no such string in `iw3mp.exe`). VERIFIED.
- `LiveStorage_StatsInit` names the five classes for a profile with no
  stats: `customclass<n>` set to the localized `CLASS_SLOT<n>`
  (0x6e0ecc, 0x6e0ed8, ..; 0x4fc5b3 on), else 0x56d0a0 with the key and
  flags 1. VERIFIED. 0x56d0a0 registers the string dvar archived.
  INFERRED (`Dvar_RegisterString`'s arguments).

vcod (`play::cod4_menus::cac`, `vcod_cod4::scriptmenu::Cmd`): the three
commands, `statset`'s check, `setFromLocString` and `uploadstats` as
above, each `Com_Error` logged with its message and the stat left
alone, no drop. `updategamerprofile` is not run; `closemenu` is not run
(only `saving_popup`, which no menu opens, runs it). `uploadstats`
writes `vcod_mpdata` at the next frame, in the front end and in a game.
`customclass1`..`5` are vcod's archived cvars (the config keeps the
names); a profile loaded with no stats file takes the `CLASS_SLOT<n>`
names once. The server reads the class off the stats the join sent
(cod4-gsc.md, "The stats, the loadout, the perks"). Gates:
`cac::tests` (each command and refusal),
`cod4_front_create_a_class_picks_reach_the_profile` (the editor's
picks, the new marks, the file, the page drawn again) and
`cod4_front_create_a_class_spawns_with_its_picks` (vcod-server's
`custom1` spawns with the AK-47, the perk and the flash grenade the
editor picked).

## 5. Start New Server

- `uiScript StartServer` sets `dedicated` to `ui_dedicated` and
  `g_gametype` to the picked game type, then runs `wait; wait; map
  <mapLoadName>` of `ui_currentNetMap`'s map; with PunkBuster asked of the
  server but not the client it opens `startpb_popmenu` instead.
  INFERRED.
- The page's rows: the game type owner draw (245) on the Game Mode row,
  Dedicated a multi on `ui_dedicated`, Server Name and Password edit
  fields, Maximum Players and the pings numeric fields, the yes/no rows,
  the map list and the map preview (254) of the picked map's
  `loadscreen_<map>`; how each draws is cod4-hud.md 12.4. VERIFIED (the
  zone walk). vcod keeps the server settings in the UI's table with no
  value until typed; retail's fields show the dvars' registered values.
  INFERRED (`Item_TextField_Paint` reads the dvar).
- vcod has no listen server: the request (`front::Request::StartServer`)
  starts `vcod-server --game cod4 <map> --gametype <gametype> --set
  dedicated=<n>` beside `vcod`, with each settings dvar that has a value,
  its output in `vcod-server.log` beside the config, as CoD1's and CoD2's
  front ends do (`frontend::create`). Dedicated No (0) joins it once it
  answers `getinfo`; LAN and Internet leave the client in the menu.
  Leaving the game stops a server the client joined this way.

## 6. Mods

- `FS_GetModList` lists the directories under `<homepath>/mods`, each shown
  as the first 0x30 bytes of its `description.txt`, else its name.
  `RunMod` sets `fs_game` to `mods/<name>` and runs `vid_restart`;
  `ClearMods` clears it. INFERRED.
- vcod lists the install's `mods/` (any case), and a launch reloads the
  front end's zones and the search path under the mod (its `mod.ff` last,
  cod4-fastfile.md 13). `fs_game` is the session's only: vcod does not
  write it to a config. `mods_menu`'s Launch Without Mods row is visible
  only under a mod. VERIFIED (the walk, its `visible when`).

## 7. Joins, games and errors

- Connect from the front end (`connect <addr>` on the console, the
  browser's join, Start New Server's) closes the menus and runs the
  handshake and the map's load behind the connect screen, as `--connect`
  does (`App::connect_cod4`, `cod4_loading::Loader`, cod4-hud.md 18); the
  loaded game takes the frame (`App::cod4_join_frame`) and the front end
  waits for it to end. A refused join or a map that fails to load comes
  back to the main menu with `error_popmenu` saying why
  (`App::cod4_join_failed`), a front end made for a client that started on
  `--connect`. The reason is localized as `SEH_LocalizeTextMessage`
  does it: a 0x15 separates an argument taken as it stands, which fills
  the string's `&&1` (`EXE_SERVER_IS_DIFFERENT_VER\x151.7` reads
  `Server is a different version:` and `1.7` on the next line,
  INFERRED; the string VERIFIED in
  `localized_code_post_gfx_mp`). vcod: `play::cod4::localize_message`.
- `CL_KeyEvent`'s Esc with no key catcher switches on the connection
  state (iw3mp.exe 0x468125, index byte table 0x46844c, jump table
  0x46843c): `CA_CINEMATIC` and `CA_LOGO` stop the cinematic,
  `CA_CONNECTING`, `CA_CHALLENGING` and `CA_CONNECTED` do nothing,
  `CA_SENDINGSTATS`, `CA_LOADING` and `CA_PRIMED` open the main menu
  once the UI has started (0x4681b8), `CA_ACTIVE` the in-game menu.
  VERIFIED (the disassembly). `CL_Connect_f` clears the key catchers, so
  on the connect screen retail's Esc does not leave the server: the
  console's `disconnect` does.
  INFERRED.
- vcod's Esc on the connect screen calls the join off, and so does the
  console's `disconnect` (`App::cancel_cod4_join`): the worker's reads
  fail at once (`conn::Join::cancel`, `conn::Cancelled`), its socket
  closes, and the main menu comes back with no error. The Esc is vcod's
  own; retail's waits out the join. A join past its gamestate (the map
  loading) says `disconnect` once the load ends, and the server frees
  the slot then; one cancelled before it leaves the slot to the server's
  timeout, since `disconnect` rides a message keyed off the gamestate.
  Gates: `cod4_connect_cancel_ends_a_join_to_a_silent_server`,
  `cod4_connect_cancel_after_the_gamestate_frees_the_server_slot`
  (vcod-server on loopback).
- In a game Esc opens `g_scriptMainMenu` (cod4-hud.md 12.5). Leave Game's
  Yes runs `disconnect`, which leaves the server for the main menu.
- A server's drop (`w`, an out-of-band `disconnect`, the timeout) or a map
  that fails to load ends the game for the main menu with `error_popmenu`
  over it: `com_errorTitle` `MENU_NOTICE` for the server's, `MENU_ERROR`
  for anything else, translated, and `com_errorMessage` the reason
  (`Com_SetErrorMessage`). INFERRED.
  `error_popmenu` draws the two dvars; its Exit and Esc run `uiScript
  clearError`, close it and open `main`. VERIFIED (the walk).
- The browser's Join Server (`take_join`) goes through the same join,
  the `password` dvar in the userinfo (`JoinArgs::password`).
- CoD4x servers are hidden. This is vcod's own filter; retail 1.7 has
  none and lists them. A row is CoD4x when its `infoResponse` carries
  `shortversion` (CoD4x's `x21`; 1.7's `getinfo` sends none) or its
  `statusResponse` a `protocol` other than 6 or a `version` starting
  `CoD4 X` (cod4-handshake.md 13; `ServerInfo::cod4x`,
  `Status::is_cod4x`). `UI_BuildServerDisplayList`'s port leaves such
  rows out of all three sources unless `ui_browserShowCoD4x` (a vcod
  cvar, archived, default 0) is 1; then the name column reads
  `[CoD4x] <hostname>`, since the join is refused at the challenge.
  The mark is not a `serverInfo_t` field, so `servercache.dat` does not
  keep it: a cached favourite shows until its ping answers. vcod joins
  stock 1.7 servers only; playing on CoD4x servers is not supported.
- Left: the Create a Class editor's stat commands (section 4), and the
  CD key menus (`cdkey_menu`, `verifyCDKey`, `getCDKey`).

## 8. Sounds and music

- The front end's sounds are `play <alias>` in a menu script
  (`Script_Play`) and a menu's `soundName`, both through
  `UI_PlayLocalSoundAliasByName` on the UI's sound system, the
  `soundName` on `Menus_Open` and on every `Menu_Paint` of the menu. INFERRED.
- `ui_mp`'s scripts play `mouse_over` (`mouseEnter`, `onFocus`) and
  `mouse_click` (`action`). VERIFIED (the walk). Those aliases and
  `music_mainmenu_mp` are in `localized_code_post_gfx_mp`'s alias lists.
  VERIFIED (the zone's strings). The music's file is on the iwds'
  search path. VERIFIED (`cod4_front_plays_its_menu_sounds_and_music`).
- vcod builds the front end's alias table from `code_post_gfx_mp`,
  `localized_code_post_gfx_mp` and `ui_mp` (and a mod's `mod.ff`), their
  loaded sounds on the search path's overlay (`cod4_fly::front_sounds`),
  each time the front end comes up over a game's table. A `play` plays
  its alias at the listener (`AudioSystem::play_local`); the topmost
  drawn menu's `soundName` (`ScriptMenus::music`) runs on the music slot,
  started again whenever the slot is idle, and stopped once no drawn menu
  names one or a game starts.

## 9. The Options pages

- `ui_multiplayer` is registered 1 with flags 0x40 (`DVAR_ROM`), its
  description "True if the game is multiplayer": iw3mp.exe 0x5492fb
  stores the bool 1, 0x54931c pushes 0x40, 0x54931f the name at
  0x6eb5c4, then `Dvar_RegisterNew` (0x56c350). VERIFIED (the
  disassembly). vcod's menus read it as 1 (`ScriptMenus::dvar`).
- The Options pages of `ui_mp` and `common_mp` gate rows on it: with 1
  `main_options` shows Multiplayer Options where single player's Cheats
  stands, and `options_game` hides Subtitles and Enable Crosshair
  (`visible when !dvarBool(ui_multiplayer)`). VERIFIED (the walk, the
  items' `visible` expressions; `cod4_options_rows_follow_ui_multiplayer`).
- `options_multi` copies `name` into `ui_name` on open and back on close
  (`exec "setfromdvar ..."`). VERIFIED (the walk). vcod's Player Name
  field edits `ui_name` and the close sets the client's `name`.
- The rows that set a vcod cvar: Brightness (`r_gamma`), Shadows
  (`sm_enable`), Bullet Impacts (`fx_marks` as `cg_marks`), Master Volume
  (`snd_volume`), Allow Downloading (`cl_allowDownload`), Player Name,
  Controls' mouse rows, Ragdoll (`ragdoll_enable`, cod4-player-anims.md
  10), and the rows below (`controls::VCOD_CVARS`).
  Greyed, with no vcod cvar behind them: Dual Video Cards,
  Sound EQ, Sound Quality, Speaker Config, Voice Chat and its level,
  Subtitles and PunkBuster. Aspect Ratio and the texture page are in
  9.3, Number of Corpses too. 1.7's Options has no field of view, draw FPS
  or per-channel volume row.

### 9.1 The video rows and their restart

- `options_graphics`' `onOpen` runs `exec options_graphics.cfg` and the
  video restart popup's Yes `exec options_graphics_set.cfg`, then
  `exec vid_restart`. Both configs are in `iw_00.iwd`: the first is
  `setfromdvar ui_<d> <d>` for `r_mode`, `r_picmip`, `r_picmip_bump`,
  `r_picmip_spec`, `r_aspectratio`, `r_rendererpreference`,
  `r_picmip_manual`, `r_vsync`, `r_aasamples`, `sc_enable` and
  `r_displayrefresh` (the `texturemode` and `texturebits` lines
  commented out), the second the same copies back. VERIFIED (the walk,
  the iwd's files). So Video Mode, Refresh Rate, Aspect Ratio,
  Anti-aliasing and Sync Every Frame edit `ui_r_*` copies and apply only
  through the popup. vcod's menus run the two configs line by line
  (`cod4_menus::MENU_CFGS`) and hand `vid_restart` to the console.
  VERIFIED (`cod4_graphics_page_applies_the_video_mode`).
- Video Mode and Refresh Rate are `ITEM_TYPE_DVARENUM` rows over the enum
  dvars `r_mode` and `r_displayRefresh`. 1.7 builds their entries off the
  adapter's modes: `%ix%i` (0x6ff518, pushed at 0x5f42f0) for each size
  of at least 640 x 480 (the compares at 0x5f4305, 0x5f4316), `%i Hz`
  (0x6ff53c, 0x5f43f5), registered through 0x56c130 at 0x5f43b8 and
  0x5f44b4, the second with flags 0x221. VERIFIED (the disassembly).
  That the order is sorted as CoD2's (`video::cod2_mode_lists`) is
  INFERRED. vcod builds both off the primary monitor on a CoD4 install as
  on CoD2, again off the window's monitor once the window is up and at
  each `vid_restart` (before the first window a Wayland session may name
  no monitor, which left both rows empty and greyed). A Wayland output
  reports only the mode in use, so a display with one size lists the
  common desktop sizes that fit inside it as well
  (`video::cod2_mode_lists`); that list is vcod's, not the adapter's. A
  row steps its copy to the next entry as `%i`
  (`Item_DvarEnum_HandleKey`, INFERRED), and `r_mode` set to an
  index names the entry. After the `vid_restart` the window takes the
  size; under exclusive fullscreen (`r_fullscreen 2`) the monitor's mode
  of that size nearest the refresh rate (`video::closest_mode_at`).
- `r_aaSamples` is registered at 0x62b1cb, an int (type 5) with flags
  0x21, a default and minimum of 1 (`ebx`, set to 1 at 0x629df3 and
  pushed as both) and a maximum of 16 (`mov eax, 0x10` at 0x62b1a1); `r_vsync` at 0x62b20e, a bool with flags
  0x21. VERIFIED (the disassembly). Anti-aliasing's Off, 2x and 4x set 1,
  2 and 4; vcod's CoD4 default is retail's 1, not CoD1's 4
  (cod4-client-perf.md 19); vcod's scene has 1 or 4 samples (`renderer::aa_samples`), so
  2x draws 4x, and the `vid_restart` rebuilds the renderer when the count
  moves. Sync Every Frame sets vcod's `r_swapInterval`, read every frame,
  so it applies at once, before the popup's restart.

### 9.2 The rows vcod reads live

Each registers through `Dvar_RegisterNew` (0x56c350) at the address its
row gives. vcod registers them on a CoD4 install
(`Shell::register_cod4_options`) and reads them every frame.

- Glow, `r_glow_allowed` (0x62c5dc). VERIFIED (the disassembly). Off,
  the level's glow is not drawn (`R_UsingGlow`). INFERRED. vcod drops the vision set's glow.
- Specular Map, `r_specular` (0x62a7d2, a bool, flags 0). VERIFIED (the
  disassembly). Off, the runtime technique remap drops the specular
  variant (`Material_GetRemappedFeatures_RunTime`, every frame ). INFERRED. vcod drops a
  lit world or static model set's specular (`cod2_world.wgsl`); an entity
  model's set keeps it.
- Number of Dynamic Lights, `r_dlightLimit` (0x62b34d, an int, flags 0).
  VERIFIED (the disassembly). The menu's Off, Low and Normal set 0, 2 and
  4. VERIFIED (the walk). The renderer keeps that many of the scene's
  lights, the most important (`R_GetVisibleDLights`).
  INFERRED. vcod keeps the
  nearest that many of the effects' lights.
- Model Detail, `r_lodScaleRigid` (0x62a29c). VERIFIED (the
  disassembly). Normal sets 1, Low 2, and the page's `onClose` then sets
  `r_lodScaleSkinned`, `r_lodBiasSkinned` and `r_lodBiasRigid` (4, -200,
  -100 for Low; 1, 0, 0 for Normal). VERIFIED (the walk). The LOD
  distance is scaled by it (`R_SetLodOrigin`). INFERRED. vcod has one LOD scale
  and reads the rigid pair for every model.
- Water Detail, `r_drawWater` (0x62b5e5). VERIFIED (the disassembly).
  Low (0) stops the water's animation, its height map held at its last
  time (`R_UploadWaterTexture`). INFERRED. vcod holds the CoD4 waters' build time.
- Texture Filtering, `r_texFilterMipMode` (0x62a005). VERIFIED (the
  disassembly). Automatic, Bilinear and Trilinear set `Unchanged`,
  `Force Bilinear` and `Force Trilinear`. VERIFIED (the menu's strings).
  The sampler state reads it every frame (`R_SetSampler`).
  INFERRED. vcod forces
  `GL_LINEAR_MIPMAP_NEAREST` or `GL_LINEAR_MIPMAP_LINEAR` over
  `r_textureMode`.
- Show Blood, `cg_blood` (0x43d984, default 1, flags 1). VERIFIED (the
  disassembly). Off, a bullet's flesh hit plays
  `impacts/flesh_hit_noblood` (cod4-effects.md 6).
- Depth of Field, `r_dof_enable` (0x62cc46: a bool, default 1, flags 1,
  "Enable the depth of field effect"; `ebx` holds 1 from 0x629df3 and
  gives both the default byte and the flags). VERIFIED (the
  disassembly). A yes/no row (item type 11). VERIFIED (the walk). Off,
  the renderer zeroes the frame's depth of field (`R_SetDepthOfField` ). INFERRED. vcod draws none (cod4-client-view.md 12).
- Soften Smoke Edges, `ui_zfeather`, a multi row of No (0) and Yes (1).
  VERIFIED (the walk). `options_graphics`' `onOpen` sets it off
  `r_zfeather` (0 or 1) and to 2 when `r_depthPrepassModels` is 1; its
  `onClose` sets `r_zfeather 0` or `1` with `r_depthPrepassModels 0`.
  VERIFIED (the walk). 1.7 registers `r_zFeather` (0x62b970: a bool,
  default 1, flags 1, "Enable z feathering (fixes particles clipping
  into geometry)"). VERIFIED (the disassembly). No `r_depthPrepassModels`
  string is in iw3mp.exe, only `r_depthPrepass` (0x7063ac), so the menu's
  `set` makes a plain dvar of it. VERIFIED (the strings). vcod keeps
  `ui_zfeather` in the UI's table and `r_zFeather` as a cvar, so the
  close applies it (cod4-effects.md 14).
- Enable Console, `monkeytoy` (0x45d0be: a bool, default 0, flags 1,
  "Restrict console access"). VERIFIED (the disassembly). Yes sets 0, No
  1. VERIFIED (the walk). The key handler reads it
  at 0x467fb9: set, and with the console not up (key catcher 1 clear),
  the console key is ignored. VERIFIED (the disassembly). No config in
  `code_post_gfx_mp`, `localized_code_post_gfx_mp`, `common_mp` or
  `ui_mp` sets it. VERIFIED (their rawfiles). vcod's \` key does the
  same on a CoD4 install.

### 9.3 Aspect ratio, the texture page, corpses and dual video cards

- `r_aspectRatio` is registered at 0x62d358 through `Dvar_RegisterEnum`
  (0x56c600): default index 0, flags 0x21, the names at 0x71c5f0 `auto`,
  `standard`, `wide 16:10`, `wide 16:9`, its description "Screen aspect
  ratio.  Most widescreen monitors are 16:10 instead of 16:9." VERIFIED
  (the disassembly). The Aspect Ratio row is a `dvarStrList` over its
  `ui_r_aspectratio` copy (`@MENU_AUTO`, `@MENU_STANDARD_4_3`,
  `@MENU_WIDE_16_10`, `@MENU_WIDE_16_9`). VERIFIED (the walk). Like the
  video mode it applies through the video restart popup.
- `R_StoreWindowSettings` (0x5f3500) switches on the enum through the
  table at 0x5f36b8: `standard` stores 4:3 (0x70bb6c), `wide 16:10` 1.6
  (0x70bb74), `wide 16:9` 16:9 (0x70bb70) as `aspectRatioWindow`
  (0xcc9d0f8). `auto` takes the adapter's native size in fullscreen
  (when known) and the window's otherwise, rounds `h * 16.0 / w`
  (0x70b1f8) and compares it with 10: equal is 1.6, below 16:9, above
  4:3. VERIFIED (the disassembly). It then sets the `wideScreen` dvar off
  4:3, stores `sceneHeight * aspect / sceneWidth` as the scene's pixel
  aspect (0xcc9d0fc) and 1.0 as the display's pixel aspect when not
  fullscreen (0xcc9d100). VERIFIED (the disassembly).
- What the aspect does to the view and the HUD, and vcod's
  `vc_windowAspect`, are section 10.
- The picmip dvars register in `R_RegisterDvars`: `r_picmip_manual`
  (0x62a609, a bool, default 0, flags 1, "If 0, picmip is set
  automatically.  If 1, picmip is set based on the other r_picmip
  dvars."), `r_picmip` (0x62a64a), `r_picmip_bump` (0x62a68b) and
  `r_picmip_spec` (0x62a6cc), each an int (type 5) of default 0, range
  0 to 3, flags 1. VERIFIED (the disassembly). Archived, not latched.
- `options_graphics_texture`'s `onOpen` copies the four into `ui_r_*`;
  Texture Quality (`ui_r_picmip_manual`: Automatic 0, Manual 1) and the
  three resolution rows (Low 3, Normal 2, High 1, Extra 0) edit the
  copies, none of them behind a `dvarTest` in the MP zone. Its Apply
  closes the page and opens `apply_picmip_popmenu`, whose `onOpen`
  `execnow`s the four `setfromdvar`s back, `exec`s
  `wait; wait; r_applyPicmip` and reopens `options_graphics`. VERIFIED
  (the walk, `menu_dump ui_mp`).
- `r_applyPicmip` (name at 0x707794) runs `R_ApplyPicmip` (0x5f2c80):
  `R_SetPicmip` (0x617520), then a walk of asset type 4 with 0x5f2c20,
  which reads each texture of the asset (count at +0x3a, 12-byte entries
  at +0x44), skips semantic 0xb, and calls 0x617310 on an image whose
  byte +0x1e is 3 and +0xa is 0 when 0x616d40's picmip differs from its
  byte +0x8. VERIFIED (the disassembly).
- So it reloads every file-loaded material image that allows picmip at
  its new level, water maps aside, without a `vid_restart` (the fields:
  category, `noPicmip`, `picmip`). INFERRED.
- `R_SetPicmip` (0x617520): with `r_picmip_manual` set it copies the
  three dvars; without it it picks levels off the card's texture memory
  (0, 0, 0 from 450 MB, 0, 0, 1 from 300, 0, 1, 1 from 200, else 1, 1,
  1), raises each to 2 at 384 MB of system memory or less and to 1 at
  640 or less, and writes the three back through `Dvar_SetInt`
  (0x56c900). With `r_specular` off the specular level is 3. VERIFIED
  (the disassembly).
- The picmip an image takes by semantic (0x616d40, the byte table at
  0x616da0): colour (2) and water (11) maps `r_picmip`, normal maps (5)
  `r_picmip_bump`, specular maps (8) `r_picmip_spec`, anything else
  none, clamped to 0..3. VERIFIED (the disassembly). Loading an `.iwi`
  (0x64244b) refuses picmip when the header's flags hold 0x3
  (`NOPICMIP`, `NOMIPMAPS`) or its shorter side is under 32, then reads
  the file only up to the level that picmip starts at (the header's
  per-picmip file size). VERIFIED (the disassembly). The stock iwds hold
  252 images flagged `NOPICMIP` alone, mipmapped and 32 or more a side
  (`blackhawk_body_color` among them).
- vcod registers the four at those defaults, `r_picmip` no longer
  latched, and `r_applyPicmip` rebuilds the renderer when the levels
  moved, as a `vid_restart` that moves an image setting does
  (`App::apply_picmip`); a `vid_restart` reads them too. Automatic
  writes 0, 0, 0 back: vcod reads every machine as 1.7's top tier. The
  world's colour maps drop `r_picmip` levels, its normal and specular
  maps `r_picmip_bump` and `r_picmip_spec`, under the `.iwi` header's
  refusal (`texture_settings::iwi_picmip`); detail maps none. Models keep
  every level. A block-compressed map loses its top levels; a bitmap one
  is halved by vcod's box filter where 1.7 reads the file's smaller level.
- `r_texFilterAnisoMin` (0x629ff0: an int, default 1, range 1 to 16,
  flags 1, "Minimum anisotropy to use for texture filtering (overridden
  by max)") and `r_texFilterAnisoMax` (0x629f6a: an int, default 16,
  range 1 to 16, flags 1, "Maximum anisotropy to use for texture
  filtering"). VERIFIED (the disassembly; `ebx` holds 1 from 0x629df3).
  The texture page's Anisotropy row is a slider (type 10, 1 to 16) on
  `r_texFilterAnisoMin` itself, not a copy. VERIFIED (the walk).
- `RB_UpdateBackEndDvarOptions` (0x6154e0) calls `R_SetTexFilter`
  (0x631740) when either anisotropy dvar, `r_texFilterDisable`,
  `r_texFilterMipMode` or `r_texFilterMipBias` was modified, so they
  apply live. `R_SetTexFilter` caps `r_texFilterAnisoMax` at the card's
  limit, below 2 is 1, caps `r_texFilterAnisoMin` at that, and a linear
  mipped sampler takes the anisotropic filter when the result is not 1;
  a material's 2x and 4x samplers take the larger of it and 2 or 4
  under the maximum. VERIFIED (the disassembly). vcod's samplers take the
  minimum as their anisotropy each frame (`TextureFilter::cod4_aniso`),
  on trilinear sampling only, the one wgpu allows it with: under
  Texture Filtering's Bilinear the row has no effect.
- Number of Corpses (`ai_corpsecount`: Tiny 3 to Insane 64) is single
  player's: its row is `visible when !dvarBool(ui_multiplayer)`, and
  iw3mp.exe holds neither an `ai_corpseCount` nor a `cg_corpseCount`
  string. VERIFIED (the walk, the strings). vcod hides it as 1.7 does.
- Dual Video Cards is `r_multiGpu` (0x62a139: a bool, default 0, flags
  1, "Use multiple GPUs"), read at 0x5f4815 and 0x5f73d6 in the device
  setup. VERIFIED (the disassembly). vcod renders on one adapter and
  keeps the row greyed.

## 10. The view's aspect and the HUD's scale

How 1.7 turns `r_aspectRatio` and the window into the view's fov and the
2D layer's placement, measured for a 3840x1600 (2.4:1) display.

### 10.1 What 1.7 does

- `R_StoreWindowSettings` (0x5f3500) fills `vidConfig` (0xcc9d0e0, 12
  dwords): the scene's width and height at +0 and +4 (from the window
  parms' +0x14, +0x18), the display's at +8 and +0xc (+0x1c, +0x20), the
  refresh at +0x10, `isFullscreen` at +0x14 (the parms' byte +8),
  `aspectRatioWindow` at +0x18 (0xcc9d0f8), the scene's pixel aspect at
  +0x1c (0xcc9d0fc) and the display's at +0x20 (0xcc9d100). VERIFIED
  (the disassembly).
- `aspectRatioWindow` is one of three ratios. The jump table at
  0x5f36b8 sends `auto` to 0x5f3553, `standard` to 0x5f35d7 (4:3,
  0x70bb6c), `wide 16:10` to 0x5f35af (1.6, 0x70bb74) and `wide 16:9`
  to 0x5f35c4 (16:9, 0x70bb70). `auto` takes the adapter's native size
  (0xcc9a414, 0xcc9a418) when fullscreen and known (0xcc9a410), the
  display size otherwise, and rounds `h * 16.0 / w` (16.0 at 0x70b1f8,
  `fistp` after adding 2^-30): 10 is 16:10, below 10 is 16:9, above it
  4:3. VERIFIED (the disassembly). At 3840x1600 that is 6.67, which
  rounds to 7: 16:9. 2560x1080 and 3440x1440 round to 7 too. No setting
  gives 2.4. VERIFIED (the arithmetic).
- The scene's pixel aspect is `sceneHeight * aspect / sceneWidth`. The
  display's is 1.0 windowed, and fullscreen `aspect * [0xcc9a420] /
  [0xcc9a41c]`, the native height over its width. VERIFIED (the
  disassembly at 0x5f3645..0x5f36ac). Fullscreen at the native 3840x1600
  under `auto` it is 16/9 * 1600/3840 = 0.741. VERIFIED (the
  arithmetic).
- `CL_InitCGame`-side code copies `vidConfig` to `cls.vidConfig`
  (0xc5afec, `rep movs` of 12 dwords at 0x46cb0e) and hands the cgame
  `aspectRatioWindow` (0xc5b004) as `cgs.viewAspect` (0x74a918), with
  the display's size as `cgs.viewWidth` and `viewHeight` (0x74a910,
  0x74a914), at 0x43fff4. VERIFIED (the disassembly). The UI takes the
  same three at 0x548fef (0xcaee224, 0xcaee21c, 0xcaee220). VERIFIED.
- `CG_UpdateFov` (0x450510): `tanHalfFovY = tan(fov * 0.0174533 * 0.5)
  * 0.75` (0x70b2d0, 0x70ace8, 0x70b0c8) at 0x797614 and `tanHalfFovX =
  tanHalfFovY * viewAspect` at 0x797610. VERIFIED (the disassembly).
  `cg_fov` is a 4:3 view's horizontal fov: the vertical fov is that
  view's at every setting and the horizontal one widens with
  `viewAspect` (Hor+ off 4:3). `cg_fov 65` is 51.08 degrees tall, 80.69
  wide at 16:9, 74.80 at 16:10 and 65 at 4:3. VERIFIED (the arithmetic).
- The view projects at `viewAspect` across the whole window, so a window
  of another shape stretches it: at 3840x1600 `auto` and `wide 16:9`
  draw the same 16:9 view 1.35 times too wide, `wide 16:10` 1.5 times,
  `standard` 1.8 times. INFERRED (the refdef's viewport is the window and
  its fov the two tangents). On a 2.4:1 display Aspect Ratio's Automatic
  and 16:9 rows look the same.
- `R_StoreWindowSettings` sets `wideScreen` (the dvar pointer at
  0x1476f14, registered at 0x4ff10b with "True if the game video is
  running in 16x9 aspect, false if 4x3.", 0x6e1ca0): `auto` sets it for
  16:10 and 16:9 and clears it for 4:3 (`ecx` 1 at 0x5f35b5 and
  0x5f35ca, 0 at 0x5f35e3), a fixed setting through its own store
  (0 for `standard`, 1 for the wide two), and an index past 3 keeps the
  stored aspect and sets it unless that equals 4:3 (0x5f35fa..0x5f360d), and 0x5f3614 writes `1` or
  `0` (0x6d8f08, 0x6d0f4c) into it. VERIFIED (the disassembly). Of the
  menus in `common_mp`, `ui_mp` and the post-gfx zones only `xpbar`
  reads it: 12 items under `wideScreen` (its backing `0 -8 854 8`, the
  fill `720` wide with `rectW` `854 * ...`, and the `hud_xpticker480ws`
  ticks, 151 wide from 0 to 765) and 12 under `!wideScreen` (`640`,
  `540`, `hud_xpticker480`), all `LEFT` and `BOTTOM` (1 3). VERIFIED
  (`menu_dump`). So at 16:9 the backing spans the 854-unit screen, the
  whole foot, and at 4:3 the 640-unit one. INFERRED (the two together).
- `ScrPlace_SetupFloatViewport` (0x477420) places the 2D layer: its 4:3
  column is `h * 4/3 / displayPixelAspect` wide (1.333 at 0x70af40,
  0xc5b00c), no wider than the viewport, centred (`subScreenLeft`, +0x40,
  `0.5 * (w - column)`); the across scale is `column / 640` (640.0 at
  0x70b518), the down one `h / 480` (0x70ae90). VERIFIED (the
  disassembly). `ScrPlace_ApplyX` (0x477500, table at 0x4775a0): `LEFT`
  adds `realViewableMin` (+0x30), `CENTER` half the viewport (+0x28),
  `RIGHT` `realViewableMax` (+0x38), `FULLSCREEN` scales by +0x8, `NOSCALE`
  passes, `TO640` scales by +0x10, `CENTER_SAFEAREA` adds the viewable
  middle, anything else `subScreenLeft`. VERIFIED (the disassembly). The
  viewable area is the viewport inset by the safe area, 0 on PC (0x477280
  starts from `0.0 * 0.5`). INFERRED.
- So windowed the HUD keeps square pixels and only the world stretches;
  fullscreen at 3840x1600 the 4:3 column is 2880 wide, stretched 1.35
  times like the world, its sides at the screen's edges. INFERRED (the
  two above together).
- `r_aspectRatio`'s flags 0x21 latch it to the next `vid_restart`
  (section 9.3). `CL_Vid_Restart_f` (0x46a180) prints `Listen server
  cannot video restart.` (0x6d06ac) and returns while the dvar at
  0x1435d60's `current` byte is set. VERIFIED (the disassembly). That
  dvar is `sv_running`. INFERRED (its use). The in-game Graphics row
  is greyed then (cod4-hud.md 12.6), so on a listen server the setting
  can change only from the main menu.

### 10.2 vcod

- `camera::Cod4Screen::of` picks 1.7's aspect and display pixel aspect
  (fullscreen: `aspect * h / w` off the window, which vcod's borderless
  fullscreen sizes to the desktop); the frame projects every CoD4 view at
  it (`Renderer::set_view_aspect`) with `CG_UpdateFov`'s Hor+ fov
  whatever `vc_widescreen` says, and `ScreenPlacement::cod4`
  (`hud::cod2elem`) places the HUD, menus, hints and the loading screen
  by `ScrPlace_SetupFloatViewport`'s column, in fullscreen too and under
  `vc_widescreen 0` alike.
- A joined Dedicated No server stands in for the listen server: its
  menus read `sv_running` 1 (`ScriptMenus::listen_server`), so the
  in-game Graphics row greys as 1.7's does (it does not open a page
  whose `vid_restart` the server would refuse). Leave Game
  opens `popup_endgame` then, whose Yes is `exec disconnect` as
  `popup_leavegame`'s is. VERIFIED (`menu_dump common_mp
  popup_endgame`).
- `vc_windowAspect 1` (vcod's, archived, live, default 0): the view
  projects at the window's own aspect with the same `CG_UpdateFov`
  (`cg_fov 65` at 2.4:1 is 97.82 wide, 51.08 tall), so nothing
  stretches; the HUD keeps square pixels, its 4:3 column centred, and
  places as 1.7 places it in a window of that size
  (`ScrPlace_ApplyX`, 0x477500): `LEFT` and `RIGHT` at the window's
  edges, `CENTER` on its middle, `FULLSCREEN` across the whole window.
  INFERRED (the two bullets on 0x477420 and 0x477500 in 10.1, with the
  window's own aspect for the picked one). Trap: `LEFT` and `RIGHT` in a
  centred box of the picked aspect (at 3840x1600 under `auto` a 16:9 box
  from x 497.8 to 3342.2) put the compass 471 px in and the scoreboard,
  host name line included, 497.8 px right of the middle. VERIFIED (the
  gate below, run on that box). The board centres on `virtualViewableMax - Min`, the window's
  width on the grid, and places `LEFT` (`cod4_scoreboard::layout`), so
  only an origin at the window's edge centres it. INFERRED (the
  layout's arithmetic).
- `FULLSCREEN` stays the window's under `vc_windowAspect 1`, not the
  picked aspect's box: the `low health` overlay (`healthoverlay`, 0 0
  640 480, `common_mp`) and `killcam`'s two 640x112 bars (0 0 and 0
  368) are `FULLSCREEN` pieces that a box would cut off short of the
  window's sides. VERIFIED (`menu_dump common_mp`). The scoreboard's
  header lines (`line_horizontal_scorebar`, 0 4 640 2 and kin, `align 4
  1`) span the window with them.
- The HUD's boxes on vcod-server's war on mp_crash, a joined player,
  board held (`play::cod4::tests::hud_menus::
  cod4_live_hud_places_by_the_window_under_window_aspect`,
  `HUD_RECTS_DUMP=<path>` writes every texture's box). VERIFIED (the
  gate's dump, x0..x1 in pixels):

  | piece | 3840x1600 full, 1.7 | 3840x1600 `vc_windowAspect 1` | 1920x1080 either |
  |---|---|---|---|
  | `minimap_background` | -36.0..526.5 | -26.7..390.0 | -18.0..263.3 |
  | `scorebar_backdrop` | 18.0..594.0 | 13.3..440.0 | 9.0..297.0 |
  | `hud_us_grenade` (weapon info) | 3660.0..3768.0 | 3706.7..3786.7 | 1830.0..1884.0 |
  | the board, host line included | 835.5..3004.5 | 1116.7..2723.3 | 417.8..1502.3 |
  | `hud_xpticker480` | 0..2880.0 | 0..2133.3 | 0..1440.0 |

  Under `vc_windowAspect 1` every box at 3840x1600 is 1.7's windowed one
  at that size, at 1920x1080 1.7's own; with it 0 every box is 1.7's
  fullscreen one. The board's middle is 1920.0. The weapon info's frag
  icon is 36 grid units in from the right edge at both sizes. (The XP
  bar's rows are the `wideScreen` ones below.)
- Without `wideScreen` set the HUD draws `xpbar`'s 640-unit set at
  every aspect: 1440 of 1920 at 16:9, 2133 of 3840 under
  `vc_windowAspect 1` at 3840x1600. VERIFIED (the gate's dump with it
  unset). The menus read it off the frame's screen
  (`camera::Cod4Screen::wide_screen`, by the picked aspect as
  `R_StoreWindowSettings` does), which moves `vc_windowAspect 0` toward
  1.7: at 1920x1080 the backing is 0..1921.5 and the ticks are the `ws`
  set. Nothing else reads it, so every other box with
  `vc_windowAspect 0` is unchanged. VERIFIED (the dump, diffed).
- Under `vc_windowAspect 1` a HUD menu's strip widens to the window: a
  `LEFT` item from x 0 or less spanning the screen its pieces were made
  for (640 units, or 854 under `wideScreen`), and every item of the
  menu with the same alignment pair, scale across by the window's
  aspect over the picked one (`Cod4Screen::strip_scale`, 2.4 / (16/9) =
  1.35 at 3840x1600; never below 1), x and width alike, so the fill
  keeps its share of the bar and the ticks stay along it
  (`cod4_hud::menus::strip_align`). vcod's. In `common_mp` only `xpbar`
  has one: its rank icon items (`1 1`) keep their size. VERIFIED
  (`menu_dump`). `scorebars`, the obituaries and the kill feed draw no
  strip. The SUBLEFT backdrops of the option and class menus (`-120 0
  880 480`, `-107 0 854 480`, align `0 0`) are menus, not HUD, and keep
  the 4:3 column's frame.
- The XP bar's boxes with `wideScreen` (`HUD_RECTS_DUMP`, x0..x1 in
  pixels). VERIFIED (the gate's dump):

  | piece | 3840x1600 full, 1.7 | 3840x1600 windowed, 1.7 | 3840x1600 `vc_windowAspect 1` | 1920x1080 either |
  |---|---|---|---|---|
  | backing (`white`, 8 high) | 0..3843.0 | 0..2846.7 | 0..3843.0 | 0..1921.5 |
  | `hud_xpticker480ws` | 0..4122.0 | 0..3053.3 | 0..4122.0 | 0..2061.0 |

  The gate asserts the backing reaches the window's right edge at
  1920x1080 and at 3840x1600 fullscreen with either setting, and that
  every other box under `vc_windowAspect 1` is 1.7's windowed one.
- Live, vcod-server's dm on mp_crash and a joined client in a nested
  3840x1600 display (`tools/cod4_perf.sh`, lavapipe,
  `RUST_LOG=vcod::cod4_client=debug` logs each change): windowed
  1920x1080 `auto` 16:9, fov 80.69 x 51.08, column 240..1680; fullscreen
  3840x1600 `auto` 16:9, 80.69 x 51.08, column 480..3360; `standard`
  4:3, 65 x 51.08, column 0..3840; `wide 16:10` 1.6, 74.80 x 51.08,
  column 320..3520; `vc_windowAspect 1` 2.4, 97.82 x 51.08, column
  853.3..2986.7, sides 497.8 and 3342.2 (the HUD box below, since
  dropped). VERIFIED (the client's log).
  Gates: `camera::tests::cod4_screen_at_ultrawide_and_16_9`,
  `hud::cod2elem::tests::cod4_placement_follows_the_display_pixel_aspect`,
  `cod4_menu::tests::cod4_options_rows_follow_sv_running_and_developer`.
- vcod's fullscreen `auto` reads the window's size, not the adapter's
  native one; the safe area stays the whole window.
