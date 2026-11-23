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

### 1.1 Text styles

Module: `CoD2MP_s.exe` 1.3 and `gfx_d3d_mp_x86_s.dll` 1.3 (md5s in 4.2),
read with `objdump` (2026-11-23); addresses are theirs.

- The renderer's text draw (`gfx_d3d_mp_x86_s.dll` `0x100329c0`, run for
  render command 0x15, queued at `0x10021700` and `0x1000c030`, executed
  at `0x100330b0`) takes the text, a char limit, the font, x, y, an x and
  a y scale (window px per font texel), the packed colour, the style, the
  cursor position and the cursor glyph. VERIFIED (asm: the loads from the
  command at `0x100330b0`..`0x100330e5`, the scales multiplying each
  glyph's `x0`, `y0`, `pixelWidth` and `pixelHeight` at
  `0x10032b24`..`0x10032c27`). Which argument is which past the scales is
  INFERRED from their use below.
- It first takes half a scale off x and y (`0x100329c3`..`0x10032a0d`,
  0.5 at `0x1019bd38`): every glyph sits half a texel up and left.
  VERIFIED (asm).
- Styles 3 and 6 draw each glyph first in the shadow colour at x and y plus
  `(style != 3) + 1`, the same scales, then the glyph
  (`0x10032b3b`..`0x10032bc6`, `setne cl; inc ecx`). The offset is added
  as an integer to window coordinates, so 1 or 2 px whatever the
  resolution. The shadow colour is the packed colour masked to its alpha
  byte (`shr 0x18; shl 0x18`, `0x100329f7`..`0x10032a04`): black at the
  text's alpha. Every other style draws no shadow. VERIFIED (asm). The
  packed colour is `b, g, r, a` from the float colour times 255
  (`0x1002c4e0`). VERIFIED (asm). The Mac build's doubled quarter-pixel
  draws (cod2-hud.md 13) are not in this renderer.
- A `^N` code takes no glyph and does not count against the char limit
  (`0x10032a4c`..`0x10032ae2`); `^7` and a digit past 9 restore the call's
  colour, others set the palette's with the call's alpha. VERIFIED (asm).
- `UI_DrawText` (`CoD2MP_s.exe` `0x5322c0`) places x and y through
  `ScrPlace_ApplyRect` (`0x44be80`), rounds both to whole px (adding 2^-30,
  `0x5c3f20`, then `fistp`, `0x5322ee`..`0x532329`) and calls
  `R_AddCmdDrawText` (the import at `0x68a31c`) with its style argument.
  VERIFIED (asm). The y is the baseline: the renderer adds each glyph's
  `y0` to it. VERIFIED (`0x10032b28`, `0x10032bbb`).
- Who passes which style to `UI_DrawText`, VERIFIED (the pushed operand,
  `tools/re`-style stack walk over every call to `0x5322c0`): 3 from the
  chat lines (`0x4c79ae`, the function reading `cg_chatHeight`), the
  connection-interrupted line (`0x4c7c01`), the centre print (`0x4c8545`,
  reading `cg_centertime`), the crosshair names (`0x4c99c8`), the
  spectator line (`0x4cb56d`), the vote lines (`0x4cb6fb`..`0x4cb7ce`), the
  follow keys and `CGAME_FOLLOWING` (`0x4cba44`, `0x4cbb8d`, `0x4cbbfd`),
  the loading screen (`0x4d3639`), the scoreboard's title, footer,
  objective, headers and banners (`0x4d9067`..`0x4da19f`); the scoreboard
  row passes a register (`0x4da64f`, `(scale >= 0.16) * 3` per
  cod2-hud.md 13). The HUD owner draws (`0x4c3ae0` ammo, `0x4c43b0`
  stance, `0x4c5c20`, `0x4c5f10`, `0x4c6050` hints) pass one of their
  arguments, and `Item_OwnerDraw_Paint` pushes the item's `+0x290`
  (`0x5417d4`, `0x5418bb`), which `textstyle` (`MenuParse` entry
  `0x5dd4ac`, handler `0x544c30`) sets. VERIFIED (asm). That the pushed
  `+0x290` reaches those arguments is INFERRED. The menu items' paints
  (`0x53f6d4`, `0x53f8f5`, `0x53fa1d`, `0x53fb30`, `0x53fdfd`,
  `0x53ffcf`, `0x54015f`, `0x540b60`, `0x54136f`, `0x541468`) push
  `+0x290` too. VERIFIED (asm).
- Text drawn past `UI_DrawText`, unrounded: a hudelem's text
  (`0x4dd8e0`, `R_AddCmdDrawText` with 3 pushed at `0x4dd9b8`), the say
  line's label (`0x406d00`, 3 at `0x406dd4`) and field (`Field_Draw`
  `0x409b00`, 3 stored at `0x409c67`), and the message windows' lines
  (`0x4066b0`, 3 pushed after the colour pointer at `0x406836` into the
  console text draw `0x68a328`). VERIFIED (asm). That those 3s are the
  style argument is INFERRED (their place after the colour, as in
  `UI_DrawText`'s own call).

vcod: `hud::font::TextStyle::Cod2 { style, snap }` draws all of it;
`snap` is `UI_DrawText`'s rounding. The menus pass the item's `textstyle`
(`Painter::text`), the HUD owner draws theirs (`cod2_player_hud::Ctx::text`),
the fixed draws 3 (`TextStyle::COD2`), the hudelems, the say line and the
message windows 3 unrounded. The cursor glyph draws once without a shadow
(4.2.1).

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
game). Create, delete and load refuse a name that is not one plain
directory name (a separator, `.`, `..`, a `:`, a control character); the
type-16 field types none.

### 4.2 Edit fields and the paint order

Module: `CoD2MP_s.exe` 1.3 (md5 `6183be2d1ab494a480423eb9af3d358b`) and
`gfx_d3d_mp_x86_s.dll` (md5 `2402bf2072f9255b85e3d23297d1d0ed`), read with
`objdump` (2026-11-23); addresses are theirs. 1.1's side is
cod11-front-end.md 20.5.1 and 18.4.1.

- An edit starts as 1.1's does: one path zeroes the edit data's
  `paintOffset` (`+0x1c`) and the item's `cursorPos` (`+0x2dc`), the other
  `cursorPos` only, and both set the editing flag (`0x19a1c94`), the edit
  item (`0x19a1c9c`) and overstrike, the int behind the pointer at
  `0x5cdd28`, to 1 (`0x53d510`..`0x53d541`, `0x53ed92`..`0x53edb5`).
  Insert (`0xa1`, keypad `0xc0`) flips it (`0x53d950`..`0x53d96e`).
  `Item_TextField_Paint` picks `_` in overstrike, `|` else (`and
  dl,0xe3; add dl,0x7c`, `0x53fd88`..`0x53fd9f`), and paints from
  `paintOffset`, at most `maxPaintChars` (`+0x18`, 0 for no limit,
  `0x53fd41`..`0x53fd61`). VERIFIED (asm).
- The cursor blinks on the other phase: the renderer's cursor draw
  (`gfx_d3d_mp_x86_s.dll` `0x10032860`, called from the text loop at
  `0x10032c61` and after it at `0x10032cdb`) skips the glyph when
  `(time / 256) & 1` (`sar eax,0x8; test al,0x1; jne`,
  `0x10032861`..`0x10032875`), with no base taken off. VERIFIED (asm).
  That the time is the engine's millisecond clock (the pointer at
  `0x101d49e0`) is INFERRED.
- The key handler moves the cursor unlike 1.1's: Left steps back over a
  `^` colour code in one go (`0x53da72`..`0x53dabb`), End calls a
  paint-offset fixup (`0x53d550`, from `0x53da4c`), and types 16, 17 and
  18 have character tests of their own (`0x53d6e9`..`0x53d764`). VERIFIED
  (asm). 4.2.1 has the handler; vcod runs it (`Ui::edit_key_cod2`).
- `Menu_PaintAll` (`0x542740`, from `UI_Refresh` `0x532553`) paints the
  menus off the stack (stack at the UI context's `+0x230`, count `+0x270`)
  that carry the visible flag (`+0xe8 & 4`), in load order
  (`0x54277b`..`0x542973`), then the stack from its topmost menu with
  `fullscreen` (`+0x214`, `MenuParse_fullscreen` `0x543b00`, keyword
  entry `0x5dd1cc`) up (`0x542979`..`0x5429da`), as 1.1 does. VERIFIED
  (asm). vcod paints CoD2's stack the same way (`Ui::painted`).

#### 4.2.1 The edit handler

`Item_TextField_HandleKey` (`CoD2MP_s.exe` `0x53d590`), read with
`objdump` (2026-11-23). The item's type is at `+0x270`, its dvar at
`+0x2c0`, `cursorPos` at `+0x2dc`, the edit data (`0x547080`: item
`+0x2ec` for types 0, 4, 9, 10, 11, 14, 16, 17 and 18) holds `maxChars`
`+0x10`, `maxCharsGotoNext` `+0x14`, `maxPaintChars` `+0x18` and
`paintOffset` `+0x1c`. VERIFIED (asm).

- The edit types (`0x53d4d0`, its byte table at `0x53d4fc`): 4, 9, 16, 17
  and 18. VERIFIED (asm). They are `ui/menudefinition.h`'s edit, numeric,
  valid-file, decimal and upper-case fields (`iw_07.iwd`). VERIFIED (the
  file). vcod reads 16 as an edit field with `file_name_only` and 18 as
  CoD1's 15 (`ui_menu::cod2::item_type`).
- The buffer: the dvar's string into a zeroed 1024 bytes (`0x3ff` copied),
  `len` its length; a `len` past a non-zero `maxChars` is cut there with a
  NUL written (`0x53d5c5`..`0x53d61a`). No edit data or no dvar answers 0.
  VERIFIED (asm).
- The cursor step (`0x546da0`, `n` in `ecx`): backward, each step takes one
  character, but a `^` and a digit before the cursor pass as a pair that
  counts nothing (`0x546dad`..`0x546def`); forward, the pairs at the cursor
  pass first, then the step stops at the NUL or after `n` characters, so a
  forward step also passes the codes after it (`0x546df1`..`0x546e30`).
  The pair test is `^` then a byte that is not NUL, not `^`, and in
  `0`..`9`. VERIFIED (asm). Left (`0x9c`, `0xb9`) and Backspace's move run
  the backward step inline (`0x53da72`..`0x53dabb`, `0x53d686`..`0x53d6e4`).
- The fixup (`0x53d550`) after every cursor move: a `paintOffset` past the
  cursor comes down to it; else with a non-zero `maxPaintChars` it rises to
  the backward step of `maxPaintChars` from the cursor. VERIFIED (asm).
- Char 8 (Backspace): with the cursor past 0, `memmove` the rest one left
  over the character before it (`0x53d63c`..`0x53d656`); the dvar is set
  either way and read back, the cursor takes a backward step from where it
  stood, and the fixup runs (`0x53d659`..`0x53d6e4`, `0x53dabd`). VERIFIED
  (asm).
- Other characters (`0x53d6e9`..`0x53d775`): type 16 drops one
  `I_isforfilename` (`0x44a610`: letters, digits, `_`, `-`) refuses; any
  type drops a character under `0x20`; type 9 ends the edit (answers 0) on
  a non-digit; type 17 ends it on a non-digit other than the decimal point
  (`0x435610`: `,` for the languages 1, 2, 3, 4, 6, 7 and 14, else `.`),
  stores that point as `.`, and does not count points; type 18 goes
  through the CRT's `toupper` (`0x57b7d5`). VERIFIED (asm). That
  `0x11972ec`'s `+8` is the language number is INFERRED; vcod takes `.`
  only.
- Insert (overstrike 0): `len` 255, or `len` at a non-zero `maxChars`,
  drops it; else `memmove` from the cursor one right
  (`0x53d786`..`0x53d7b5`). Overstrike: a cursor at a non-zero `maxChars`
  drops it, stepping the focus on with `maxCharsGotoNext` (the edit moves
  to an edit field there, its cursor kept; `0x53d87f`..`0x53d8c6`).
  VERIFIED (asm).
- The character goes at the cursor, the dvar is set and read back, the
  cursor takes one forward step and the fixup runs (`0x53d7c0`..`0x53d81a`).
  At a non-zero `maxChars` with `maxCharsGotoNext` the focus steps on, the
  next item's `cursorPos` goes to 0 and the edit moves there when it is an
  edit field (`0x53d81f`..`0x53d874`). The character then falls into the
  key compares below, as 1.1's does (20.5.1 of cod11-front-end.md); vcod
  does not do this. VERIFIED (asm).
- Keys (`0x53d8d8`..`0x53da31`): Del (`0xa2`, `0xc1`) `memmove`s the rest
  over the character under a cursor under `len` and sets the dvar, no
  fixup (`0x53daec`); Right (`0x9d`, `0xbb`) one forward step and the fixup
  (`0x53dada`); Home (`0xa5`, `0xb6`) zeroes the cursor and `paintOffset`;
  End (`0xa6`, `0xbc`) puts the cursor at `len` and runs the fixup only
  (`0x53da40`), so with `maxPaintChars` 0 the text stays painted, unlike
  1.1; Insert (`0xa1`, `0xc0`) flips overstrike (`0x53d960`). Tab, `0x9b`,
  `0xbd` step the focus on and `0x9a`, `0xb7` back, the edit moving to an
  edit field (`0x53d97f`..`0x53d9f6`); Enter (`0xd`, `0xbf`) runs `accept`
  (item `+0x2b4`, `0x53da16`) and answers 0, as Esc (`0x1b`) does. VERIFIED
  (asm).
- `Item_TextField_Paint` (`0x53fb80`): type 17 with a float dvar paints
  the value formatted at two places (`0x435640`, `eax` 2), whatever the
  edit did to the dvar's text, else the dvar's string; both from
  `paintOffset` (`0x53fbc8`..`0x53fc1f`, `0x53fd41`..`0x53fd61`). VERIFIED
  (asm). The renderer counts `maxChars` in glyphs: a colour code takes none
  (`gfx_d3d_mp_x86_s.dll` `0x10032a30`, the count dropped at `0x10032c86`
  and skipped by the code arms). VERIFIED (asm). It draws the cursor glyph
  once, in the text's current colour and with no shadow, at the pen spot of
  the glyph whose byte index (codes count 2) is the cursor, or after the
  last (`0x10032c3e`..`0x10032c61`, `0x10032ca9`..`0x10032cdb`,
  `0x10032860`). VERIFIED (asm).

vcod (`Ui::edit_key_cod2`, `Ui::painted_field`) follows all of it but the
fall-through, the comma point, and a cursor between a `^` and its digit,
which vcod's paint still draws.
