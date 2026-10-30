# CoD2 1.3 fonts, menus and hudelem placement

CoD2 1.3's font files, its menu parser's keywords, and where the cgame
places a hudelem. vcod's side: `vcod_cod2::font`,
`vcod_common::ui_menu::parse_menu_file_in`,
`crates/client/src/hud/cod2elem.rs`.

Module: the Mac 1.3 multiplayer build, thinned i386 slice
`cod2mp_mac13_i386` (md5 `b30966627b680de20bf35603c562a3f6`), named by its
own C++ symbols. Addresses are that slice's virtual addresses unless a
module is named. Constants come from its `__literal4` section
(`llvm-objdump -s`). Assets: a stock 1.3 install (`iw_00.iwd` holds
`fonts/`; `iw_06`, `iw_07` and `iw_15` the menus; `iw_13`
`materials/fonts/*`).

## 1. Fonts

`fonts/` holds six game fonts with no extension (`smallFont`,
`normalFont`, `consoleFont`, `bigFont`, `boldFont`, `extraBigFont`) and
three developer ones (`smallDevFont`, `bigDevFont`, `qerFont`). VERIFIED
(`iw_00.iwd`).

`ui_mp/main.menu`'s `assetGlobalDef` registers the six under CoD1's
`fonts/fontImage_<size>.dat` sizes. VERIFIED (the file). vcod maps a size
to its file by this list (`font::name_for_size`).

| size | font |
|---|---|
| 12 | small |
| 16 | normal |
| 18 | console |
| 24 | big |
| 30 | bold |
| 32 | extraBig |

`R_LoadFont` (0xfb970) reads the file into a hunk block 4 bytes longer than
the file: bytes 0..16 at +0, the rest at +20. It stores the glyph pointer
at +16 and turns +0 (name) and +12 (material) from file offsets into
pointers; the material goes through `Material_RegisterHandle`. VERIFIED
(the two `FS_Read` calls and the stores). The file is `Font_s` with its
pointers as offsets:

| off | type | field |
|---|---|---|
| 0 | u32 | offset of the font's name (`fonts/normalFont`) |
| 4 | i32 | `pixelHeight` |
| 8 | i32 | `glyphCount` |
| 12 | u32 | offset of the material name (`fonts/gamefonts`) |
| 16 | | `glyphCount` glyphs of 24 bytes, then the two strings |

VERIFIED (every stock font; `normalFont`'s 0xfe glyphs end at 0x17e0, its
name's offset). The six game fonts name material `fonts/gamefonts`, colour
map `images/gamefonts.iwi`. VERIFIED (the files). The `fonts/gamefonts.tga`
beside them is source art: nothing loads that path. INFERRED.

A glyph, as `RB_DrawTextCmd` (0xda28c) reads it. VERIFIED (the loads).

| off | type | field |
|---|---|---|
| 0 | u16 | `letter` |
| 2 | i8 | `x0`, added to x, scaled |
| 3 | i8 | `y0`, added to the baseline y, scaled |
| 4 | u8 | `dx`, the advance |
| 5 | u8 | `pixelWidth` |
| 6 | u8 | `pixelHeight` |
| 8..0x14 | f32 x4 | `s0 t0 s1 t1`, passed to `RB_DrawStretchPic` |

`R_GetCharacterGlyph` (0xec2d8): a letter in 0x20..0x7f is glyph
`letter - 0x20`. Any other letter is binary-searched from glyph 0x60 on. A
letter not found, or any letter in a font of 0x60 glyphs or fewer, draws
glyph 14 (`.`). VERIFIED.

Scale: `R_NormalizedTextScale` (0xec548) is `48 * scale / pixelHeight`
(0x326270 is 48.0). VERIFIED. vcod builds its CoD1 `Font` from a CoD2 one
with `glyph_scale = 48 / pixelHeight` and `max_height` the tallest ascent
(`-y0`) of 0x20..0x7e, so its callers' top-of-text puts the baseline where
retail's draw has it (`hud::font::load_font`). INFERRED. The console font
is 16 pixels, so the console's third-of-glyph-scale draw is one texel a
pixel. INFERRED.

## 2. Menus

### Keywords

`Item_Parse` (0x1a92d4) and `Menu_Parse` refuse a keyword missing from
their hash tables with `unknown menu item keyword %s` / `unknown menu
keyword %s`. VERIFIED (the `PC_SourceError` strings). The tables are
`itemParseKeywords` (0x34c8a0, 65 entries) and `menuParseKeywords`
(0x34cbc0, 32), null-ended 12-byte `{name, handler, next}` records.
VERIFIED (dumped by symbol).

Against CoD1 (VERIFIED, the tables; arguments from each handler's
`PC_ReadTokenHandle` calls):

| change | keywords |
|---|---|
| `cvar` family renamed | `dvar`, `dvarTest`, `showDvar`, `hideDvar`, `enableDvar`, `disableDvar`, `focusDvar`, `dvarFloat`, `dvarStrList`, `dvarFloatList`, `dvarEnumList` |
| new item keywords | `textfile`, `textsavegame`, `textfont`, `mouseEnterText`, `mouseExitText`, `accept`, `special`, `cinematic` |
| new menu keywords | `blurWorld` (float, refused below 0), `fadeInAmount`, `legacySplitScreenScale` (no argument), `soundLoop` |

`enableDvar`, `disableDvar`, `showDvar`, `hideDvar` and `focusDvar` all
store their block at item +0x2cc and set a flag at +0x2d0, so an item
carries one dvar test whatever the keyword. VERIFIED. vcod keeps the show
and hide gates (`CvarGate`) and parses the other three as skipped blocks.

| keyword | flag | handler |
|---|---|---|
| `enableDvar` | 1 | 0x1a89be |
| `disableDvar` | 2 | 0x1a89f4 |
| `showDvar` | 4 | 0x1a8a2a |
| `hideDvar` | 8 | 0x1a8a60 |
| `focusDvar` | 0x10 | 0x1a8a96 |

`PC_Rect_Parse` (0x1a9dec) reads four floats, then up to two integers
through `PC_ReadLineHandle`, which reads only from the same line: the
horizontal and vertical alignment, 0 when absent. VERIFIED. The stock
menus spell them `HORIZONTAL_ALIGN_*` / `VERTICAL_ALIGN_*` from
`ui/menudefinition.h`, where `HORIZONTAL_ALIGN_DEFAULT` is itself
`#define`d as `HORIZONTAL_ALIGN_SUBLEFT`; the preprocessor expands a
define's body again. VERIFIED (the header; the stock menus parse only so).
vcod's tokens carry no lines, so it takes up to two integer tokens after
the four (no keyword is a number). `ITEM_TYPE_DVARENUM` is 13 and
`ITEM_TYPE_BIND` moves to 14. VERIFIED (`ui/menudefinition.h`).

The script commands (`commandList`, 0x34b260, 28 entries; count at
0x34b240): `fadein fadeout show hide setcolor open openforgametype
closeforgametype close ingameopen ingameclose setbackground setitemcolor
setfocus setfocusbydvar transition setdvar exec execnow
execOnDvarStringValue execOnDvarIntValue execOnDvarFloatValue
execNowOnDvarStringValue execNowOnDvarIntValue execNowOnDvarFloatValue
play orbit scriptmenuresponse`. VERIFIED. vcod's script splitter maps
`setdvar` and the two `execOnDvar*Value` it runs to CoD1's names
(`ui_menu::script_commands`).

Every file `ui_mp/menus.txt`, `ingame.txt` and `hud.txt` load, and every
other `ui_mp/**/*.menu`, parses in vcod with none of the errors above
(`stock_cod2_menus_parse_without_errors`). The front end and `hud.menu`
read through the install's grammar.

### Missing list entries

The stock lists name eleven files no 1.3 iwd holds under any case or
directory. VERIFIED (the iwd directories).

| list | missing |
|---|---|
| `ui_mp/menus.txt` (iw_06, iw_15) | `ui/options_view.menu`, `ui/options_defaults.menu`, `ui/options_credits.menu`, `ui/rec_restart.menu`, `ui_mp/in_rec_restart.menu` |
| `ui_mp/ingame.txt` (iw_06) | `ui_mp/ingame.menu`, `ingame_controls`, `ingame_options`, `ingame_system`, `ingame_leave`, `ingame_callvote` (only `ui_mp/wm_quickmessage.menu` ships) |

`UI_LoadMenus` (0x1ad780) hands each name to `UI_ParseMenuInternal`
(0x1acac4), which prints `\tLoading '%s'...` and returns 0 when
`PC_LoadSourceHandle` finds no file; `UI_LoadMenus` drops the result.
VERIFIED. `^3WARNING: menu file not found` belongs to the list file itself,
and `UI_LoadMenu`'s (0x1ad942) to one named file. VERIFIED. Retail skips a
missing list entry without a warning; vcod's `load_menus` logs it at trace.

### Enum, decimal and file-name items

`ITEM_TYPE_DVARENUM` (13) names its enum in `dvarEnumList`, which
`ItemParse_dvarEnumList` (0x1a9bd4) keeps at item +0x2ec. VERIFIED.
`Item_Paint` (0x16fe0e, the 13 arm at 0x1704d6) prints:

- `<dvarEnumList not set>` with no list;
- `<not an enum dvar>` when the named dvar is not of type 6;
- nothing for an empty enum;
- else the entry the item's own dvar names: `atoi` of its string when in
  range, else the entry it spells under `I_stricmp`, else entry 0.

VERIFIED. `Item_HandleKey` (0x16d720) steps to the next entry on the first
and third mouse buttons, Enter and two more keys (200, 0xd, 0xca, 0xa3,
0x9d), and to the previous on the second button (0xc9, 0xa4, 0x9c),
wrapping. It sets the item's dvar to `%i` of the index when it moved.
VERIFIED. The graphics menu steps `ui_r_mode` over `r_mode` and
`ui_r_displayrefresh` over `r_displayRefresh` this way; Apply copies them
back with `setfromdvar`. VERIFIED (`ui/options_graphics.menu`,
`ui/options_graphics.cfg`). vcod's shell stores an in-range index or an
entry's name set on an enum dvar as the entry's name
(`Shell::set_enum_domain`) and keeps any other value, as it does vcod's own
`r_mode` -1 (the window's size). INFERRED: retail's enum refuses such a
value.

`r_mode` and `r_displayRefresh` are enums over the display's modes
(`R_BeginRegistration`, 0xcb5f6): the adapter's modes sorted by width,
height and refresh; each size of at least 640 x 480 once as `%ix%i`; each
refresh once, in that order, as `%i Hz`; both registered with flags
0x2021. VERIFIED. The defaults, read off the loops: the first size, the
first refresh of 60 Hz or more. INFERRED. vcod lists the primary monitor's
modes the same way at window creation (`video::cod2_mode_lists`) and keeps
the config's `r_mode`; a `WxH` value sizes the window (`video::video_mode`).

`ITEM_TYPE_DECIMALFIELD` (17) paints as an edit field
(`Item_TextField_Paint`, 0x16994a) whose text is its float dvar through
`Com_LocalizedFloatToString(value, 2)` (0x2f8b0: `%.*f`, the point a comma
for languages 1..4, 6, 7 and 14). VERIFIED. The look menu's
`sensitivity_textdisplay`, a decoration beside the slider, is one.
VERIFIED (`ui/options_look.menu`).

`ITEM_TYPE_VALIDFILEFIELD` (16) is an edit field that takes only
`I_isforfilename`'s characters (0x2942c: letters, digits, `_`, `-`).
VERIFIED. `accept` (`ItemParse_accept`, 0x1a881c, item +0x2b4) is the
script that Enter or keypad Enter (0xd, 0xbf) runs in an edit field
(`Item_TextField_HandleKey`, 0x16ee5e). VERIFIED. vcod reads a file-name
field as an edit field with the filter (`UiItem::file_name_only`) and runs
`accept` on Enter.

### Placement and blur

`Item_SetScreenCoords` (0x192612) offsets an item's rect by the menu's
origin and takes the item's alignment pair, or the menu's when the item's
is `0 0`. VERIFIED (the stores at +0x10 and +0x14). Every paint goes
through `CL_DrawStretchPic` (0x17b962), `CalcScreenPlacement` on all four
numbers with that pair. `UI_DrawText` (0x151caa) places the text's origin
and normalized scale the same way, flooring the origin to whole pixels.
VERIFIED. vcod places each rect once per window size and maps it back onto
the 4:3 column's grid (`frontend::cod2`), so CoD1's paint, focus and click
code runs unchanged. Text keeps the column's scale under every alignment,
so `FULLSCREEN` text on a wide window is narrower than retail's. INFERRED
(from the placement rules above).

`Menu_PaintAll` (0x00170a4a) zeroes the display context's blur radius
(`+0x28`) each frame. `Menu_Paint` (0x0017077c) folds each painted menu's
`blurWorld` (`menuDef + 0x240`) into it as `sqrt(blur^2 + radius^2)`.
VERIFIED. There is no lerp: the radius is there from the menu's first
painted frame. INFERRED (no other store to `+0x28` in the UI code).

`CL_GetMenuBlurRadius` (0x00172970) hands the radius to the view only when
bit 8 of `cl + 4` is set, `cls + 0x110` is not 0 and `*clc` is not 1, else
0. VERIFIED (the three tests). That these are the UI's key catcher, a game
up and `demoplaying` is INFERRED (the bit is Q3's `KEYCATCH_UI`; the fields
were not traced). vcod's `Ui::blur_world` keeps the key-catcher test and
its caller the demo test; the renderer's blur is `cod2-materials.md` 9.13.
The 19 stock `blurWorld` lines are all 5.0, in 15 files under
`ui_mp/scriptmenus/` (`ingame`, `callvote`, `muteplayer`, the five
`serverinfo_*`, three `team_*`, four `weapon_*`). VERIFIED (grep of the
iwds).

## 3. Hudelem placement

Script fields and the wire table: `cod2-gsc-diff.md` 2.3 and
`cod2-protocol.md` 5.1. `x`, `y`, `z` are floats, `alignOrg` =
`alignx << 2 | aligny`, `alignScreen` = `horzalign << 3 | vertalign`, plus
`foreground`.

`CG_GetHudElemInfo` (0x1e047c) and `CG_Draw2dHudElems` (0x1e11b8):

- Font: `default` is `UI_GetFontHandle(0, 0.25 * fontScale)`, `bigfixed`
  handle 4 at `0.5 * fontScale`, `smallfixed` handle 5 (the console font)
  at `fontScale / 3` (0x3261bc 0.25, 0x326190 0.5, 0x326640 1/3).
  VERIFIED. The height is `UI_TextHeight`: the font's `pixelHeight` times
  its normalized scale, truncated to an integer. VERIFIED (0x14f0d0).
- `CG_AlignHudElemX/Y` (0x1e0178, 0x1e01c2): align 1 adds `-0.5 * size`
  (0x3261f4 is -0.5), 2 subtracts the size, anything else adds nothing.
  This runs on grid units, before the screen placement. VERIFIED.
- `CalcScreenPlacement` (0x18ed96) maps x by `horzalign` (table below); y
  the same by `vertalign`, SUBTOP with no offset. VERIFIED.
- `SetScreenScaling` (0x18eb38) sets the column `h * 4/3 / pixelAspect`
  (0x3263cc 4/3), the scales `column / 640` and `h / 480`, and
  `subScreenLeft = 0.5 * (w - column)`. VERIFIED. That `cls + 0x2b46f8` is
  the pixel aspect is INFERRED. vcod's CoD2 placement takes square pixels
  and the whole window as the safe area.
- A move (`moveTime > 0`) places the `from*` position with its own
  `fromAlignOrg` / `fromAlignScreen`, places the current one, and lerps the
  placed points by `(time - moveStartTime) / moveTime`; at or before the
  start it draws the from point. VERIFIED.
- The draw sorts archived then current elements by `sort` with `qsort` and
  draws those whose `foreground` equals the pass's argument; type 13 is
  skipped there. Text kinds are types 1-5 and 7-10 (mask 0x7be), material
  6 (0x40). VERIFIED.

| `horzalign` | name | x |
|---|---|---|
| 0 | SUBLEFT | `x * scale + subScreenLeft` |
| 1 | LEFT | `+ viewable min` |
| 2 | CENTER | `+ 0.5 * viewport` |
| 3 | RIGHT | `+ viewable max` |
| 4 | FULLSCREEN | `x * w / 640` |
| 5 | NOSCALE | unchanged |
| 6 | TO640 | `x * 640 / column` |
| 7 | CENTER_SAFEAREA | the middle of the viewable span |

vcod: `hud::cod2elem` places and draws text and material elements by these
rules. `cod2_hud.rs` feeds it the snapshot's hudelems on a CoD2 server; the
CoD4 HUD reuses its `ScreenPlacement`.

## 4. The front end

What bare `vcod --game cod2` needs past the CoD1 front end
(`frontend::cod2`, `crates/client/src/cod2_front.rs`).

- `CL_LocalServers_f` (0x14a138) sends `getinfo xxx` to the broadcast
  address on ports 0x7120..0x7123 (28960..28963), two rounds, as 1.1 does.
  VERIFIED. vcod's Local refresh is 1.1's (`net::master::LAN_PORTS`).
- `UI_FeederItemText` (0x152e8a) fills the server list's eleven columns
  (table below). VERIFIED. Column 3 walks the map list for an entry whose
  bsp name is `mapname` under `I_stricmp` and prints that entry's display
  name, the `.arena` `longname`, else `mapname`. VERIFIED. vcod keys the
  layout off the list box's column count (`browser::COLUMNS_V118`) and
  prints the arena's `longname` (`Ui::list_cell`).
- `UI_Init` (0x1506ba) registers the `hw` icons by value: 0
  `server_hardware_unknown`, 1 `_linux_dedicated`, 2 `_win_dedicated`, 3
  `_mac_dedicated`, 4 nothing, 5 `_win_listen`, 6 `_mac_listen`. VERIFIED.
- `SVC_Info` (`cod2_lnxded` 0x809537c) sends `pure` only on a pure server,
  then `hw`, `mod`, `voice` and `pb` always. VERIFIED.
- `UI_RunMenuScript` (0x1548e8) adds `openMenuOnDvar`, `openMenuOnDvarNot`,
  `closeMenuOnDvar` and `closeMenuOnDvarNot` `<dvar> <value> <menu>`. A
  missing argument prints `invalid ...`; a dvar that does not exist prints
  `cannot find dvar`; otherwise the menu opens (or closes) when `I_stricmp`
  of the value and the dvar's string agrees under the plain form, disagrees
  under the `Not` one. VERIFIED. The main menu's `onOpen` asks for
  `player_profile` this way on `com_playerProfile` "" and on
  `ui_playerProfileCount` not 1 (4.1); the graphics Apply opens
  `vid_restart_popmenu_listen` on `g_reloading` 0.
- `snd_volume` is the options' volume (`SND_Init`, 0x4b496: 1.0 in 0..1,
  archived). VERIFIED. vcod's mixer reads it on a CoD2 install in place of
  `mss_volume`.
- The options menus run `exec "setfromdvar <dest> <source>"`, CoD2's
  `setfromcvar` (`Dvar_SetFromDvar_f`, 0x5018c, added by
  `Dvar_AddCommands`, 0x509c0). VERIFIED.
- The browser's localized strings (`EXE_NETSOURCE`, `EXE_REFRESHTIME`)
  carry `&&1` (cod2-protocol.md 3.4.1), not `%s`. VERIFIED
  (`localized_english_iw*.iwd`).
- Esc in a game opens `g_scriptMainMenu`, which the stock gametypes set to
  `ingame` with `setClientCvar` (`maps/mp/gametypes/tdm.gsc`). VERIFIED
  (the script). The server sends it as `v g_scriptMainMenu "ingame"`
  (cod2-protocol.md 9). Until it arrives vcod falls back to the main menu
  over the game.

| col | content |
|---|---|
| 0 | `pswrd` |
| 1 | the `hw` icon |
| 2 | `hostname` (copied into 20 bytes; the address while the ping is 0) |
| 3 | `mapname`, or the name a matching map entry holds |
| 4 | `clients (sv_maxclients)` |
| 5 | `gametype` (`?` when empty) |
| 6..9 | `voice`, `pure`, `mod`, `pb` (each `X` when non-zero) |
| 10 | the ping (`...` while 0) |

Live check in one loopback-only namespace (`unshare -rn`, a broadcast
route on `lo`): retail `cod2_lnxded` (1.3, `sv_pure 1`, `mp_toujane` dm)
and `vcod-server --game cod2` (`mp_toujane` tdm) both answered vcod's Local
broadcast and its `getstatus`. A front-end connect
(`Cod2Session::connect_in_background`, no `Join`) to each saw `t` open
`serverinfo_*`, answered `mr <id> 4 close`, then `u` and `t` for
`team_britishgerman` (`mr <id> 1 allies`), then `weapon_british`
(`mr <id> 2 enfield_mp`), and spawned. VERIFIED.

### 4.1 Player profiles

A profile is a directory under the mod directory's `players/`; the active
one's name is the first token of `players/active.txt`. VERIFIED
(`Com_InitPlayerProfiles`, 0x111aa6, and the strings at the
`Com_BuildPlayerProfilePath` calls).

At start `Com_InitPlayerProfiles` registers `ui_playerProfileAlreadyChosen`
(0 in 0..1, flags 0x1200, reset value changed to 1) and `com_playerProfile`
("", 0x1040). When `active.txt` names a directory (`I_stricmp` over
`FS_ListFiles("players", "/")`) it sets `com_playerProfile` to it and runs
the startup configs with `players/<name>/config_mp.cfg`; else with none.
VERIFIED. `UI_Init` (0x1506ba) adds `ui_playerProfileCount` (0, 0x1040),
`ui_playerProfileSelected` ("", 0x1040) and `ui_playerProfileNameNew` ("",
0x1000). VERIFIED.

The profile scripts of `UI_RunMenuScript` (0x1548e8). Each VERIFIED off its
arm.

| script | effect |
|---|---|
| `addPlayerProfiles` | lists the directories again, sorted ascending (`uiInfo + 900` set; `UI_PlayerProfilesQsortCompare`, 0x14f878, is `I_stricmp`, negated while that flag is 0); selects row 0 in every open menu (`Menu_SetFeederSelection(..., 0x18, 0)`); sets `ui_playerProfileCount` |
| `sortPlayerProfiles` | flips the flag, sorts again, selects row 0 |
| `selectActivePlayerProfile` | selects the row showing `com_playerProfile` |
| `createPlayerProfile` | clears `ui_playerProfileNameNew` (31 bytes); `profile_create_too_many_popmenu` at 64 profiles, `profile_exists_popmenu` for a name taken under `I_stricmp`, else `Com_NewPlayerProfile` (0x111c08) makes the directory (`profile_create_fail_popmenu` when it cannot) and the name joins the list, sorted, count set, its row selected |
| `deletePlayerProfile` | `Com_DeletePlayerProfile` (0x111802) removes `ui_playerProfileSelected`'s tree and clears `com_playerProfile` when it was the active one (`profile_delete_fail_popmenu` on failure); the list drops it, `ui_playerProfileSelected` clears when none is left, the count is set |
| `loadPlayerProfile` | `Com_ChangePlayerProfile` (0x111936) of `ui_playerProfileSelected`: for another existing profile writes `players/active.txt`, runs `disconnect`, resets every dvar, sets `com_playerProfile`, runs the startup configs with the profile's `config_mp.cfg`, and queues `snd_restart` when a latched dvar changed |

`UI_FeederSelection` (0x15020e) for feeder 0x18 sets
`ui_playerProfileSelected` to the row's name. VERIFIED.
`FEEDER_PLAYER_PROFILES` is 0x18. VERIFIED (`ui/menudefinition.h`). In the
stock `ui/playerprofile.menu`, `player_profile` closes at once under
`ui_playerProfileAlreadyChosen` 1 and opens `profile_create_popmenu` with
no profile; its list's double click and OK load the selection. VERIFIED
(the menu).

vcod (`frontend::profiles`) runs the same scripts over `<mod dir>/players`
on a CoD2 install only; a front end without a profile directory runs none.
Its config stays its own file, `players/<name>/vcod_mp.cfg` beside
retail's `config_mp.cfg`: read at start (the mod directory's `vcod_mp.cfg`
seeds a profile that has none yet) and written from then on.
`loadPlayerProfile` writes `active.txt`, sets `com_playerProfile` and runs
the profile's file, or writes the current settings there when it has
none. vcod resets no dvar and does not disconnect; the stock menus offer
the load from the main menu only (`player_profile_ingame_warning` in a
game).
