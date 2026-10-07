# The client console (CoD 1.1 MP)

What the retail client's drop-down console does and how it looks, read out
of `CoDMP.exe` 1.1 (image base `0x00400000`, md5 in
`cod11-events-and-fx.md`) and pak4's `scripts/gfx.shader`. Addresses are
virtual. The function names are mine, after the Quake III functions whose
shape they share; `CoDMP.exe` has no symbols. Floats are read out of
`.rdata` at the address given. vcod's implementation is
`crates/client/src/console/`.

## 1. Toggle and focus

`toggleconsole` is registered at `0x408a90` (VERIFIED, the string at
`0x5686f0` passed to the command registrar `0x428840` with `0x4083a0`).
Beside it the same function registers `messagemode`, `messagemode2`,
`messagemode3`, `clear` and `condump`, and the cvars `scr_conspeed`
(default `"3"`, the string at `0x568730`), `con_debug` and `con_restricted`
(VERIFIED).

The key handler at `0x40dc30` (my `CL_KeyEvent`) compares the key number
against `0x60` (`` ` ``) and `0x7e` (`~`) ahead of the bind lookup and calls
`0x4083a0` on a press of either (VERIFIED, the two compares and the call at
the top of the function). So both keys open the console whatever they are
bound to; the stock `config_mp.cfg` binds both to `toggleconsole` as well.
INFERRED, off the branch order: an auto-repeat of either key is dropped while
the console or a message field holds the keys.

`0x4083a0` clears the input field and flips bit 1 of the key-catcher word
`0x155f2c4` (VERIFIED, the 64-dword clear of `0x142f65c` and the `xor 1`).
INFERRED: while that bit is set the key handler routes keys to the console
field instead of the binds, which is what stops game input. Escape with the
console up does not close it: in game the handler opens the menu over it
(INFERRED, the `0x1b` branch reaching `0x460480` with 7). vcod has no menu,
so Escape closes the console instead.

## 2. Entering a line

The console field's key handler is `0x40d050`. On Enter (key `0x0d` or
keypad Enter `0xbf`):

- VERIFIED, the format strings: the line is echoed as `]%s\n` (`0x5679c8`),
  and a line with no leading `\` or `/` is rewritten as `\%s` (`0x5679d0`)
  first when the client is not in game.
- INFERRED, off the branches at `0x40d22b`..`0x40d2b7`: a line with a leading
  `\` or `/` runs as a command without that character; in game (`cls.state`
  at `0x155f2c0` equal to 6) any other non-empty line runs as
  `cmd say <line>` (the string `cmd say ` at `0x5679b4`), which is chat; an
  empty line does nothing. Out of game, every line is a command, because of
  the rewrite above.
- VERIFIED: history is a 32-entry ring (`& 0x1f` on the index at
  `0x40d2cd`), each entry a copy of the 0x47-dword field record; the field
  text is 256 bytes (the 0x40-dword clears).

Tab (key 9) calls `0x40ce80` (VERIFIED, the compare and call), which is the
completion. INFERRED from the `    %s\n` format at `0x5679d4` beside it: a
prefix with several matches lists them, four spaces in.

The rest is Q3's shape and vcod follows it: an unknown command is forwarded
to the server while connected, except a `+` or `-` one, and prints
`Unknown command "%s"` (VERIFIED, the string) otherwise. `Not connected to a
server.` and `usage: connect [server]` are VERIFIED strings; which command
prints which is INFERRED from Q3.

## 3. Drawing

`0x40a1e0` (my `Con_DrawConsole`) draws through `0x409f30` (my
`Con_DrawSolidConsole`) with the fraction 1.0 when the client state word
`0x155f2c0` is 0 and the key catcher has neither bit 2 nor bit 8 set, and
with the sliding fraction `0x142ef60` otherwise (VERIFIED, the `0x3f800000`
argument and the two state compares). INFERRED: state 0 is disconnected, so
with no menu up a disconnected client shows a full-screen console. vcod
shows it whenever there is neither a map nor a server.

The slide, at `0x40a260` (VERIFIED, the arithmetic): the target is 0.5 while
key-catcher bit 1 is set and 0 otherwise, and the fraction moves toward it
by `frametime * scr_conspeed * 0.001` a frame (`0x568e6c` is 0.001), clamped
at the target. With the default 3 the console is fully down in 1/6 s.

`0x409f30`, with `w` and `h` the window's size in pixels:

- VERIFIED: `0x416810` scales x by `w * 0.0015625` (`0x568fe4`, 1/640) and
  y by `h * 0.00208333` (`0x568fe0`, 1/480), so the rectangles are placed on
  a 640x480 grid.
- VERIFIED: the background is the `console` shader, registered at
  `0x411540` beside `white`, drawn from (0, 0) 640 units wide. pak4's
  `scripts/gfx.shader` defines `console` as `$whiteimage` with
  `rgbGen constLighting ( 0.15 0.15 0.15 )` and no blend: an opaque dark
  grey.
- VERIFIED: a separator follows, the `white` shader at width 640.0
  (`0x44200000`) and height 2.0 (`0x40000000`), under the colour (0, 0, 0,
  0.6) (`0x3f19999a`).
- INFERRED, the FPU arguments being invisible in the decompile: as in Q3,
  the background is `frac * 480 - 2` units tall, skipped below 1 unit, and
  the separator sits right under it.
- VERIFIED: text goes through `0x416a30`, `0x416aa0` and `0x416980`, which
  pass font handle 5 and a scale of `0.3333 * 480 / h` (`0x568fe8`), and
  add 16.0 (`0x568ec8`) to the row's y. `0x416aa0` also passes a cell of
  `640 / w * 8` (`0x568ef4` is 8.0), so a row is laid out one glyph per
  8-pixel cell. Handle 5 is `consoleFont`, `fontImage_18`
  (`cod11-hud-protocol.md`, section 8, "Font slots"). INFERRED: the y the
  text call receives is the baseline, as in Q3's UI text, and the scale
  times the font's own `glyphScale` is its size in window pixels, since the
  coordinates were converted from pixels to the 640x480 grid first.
- VERIFIED, pak5's `fonts/fontImage_18.dat`: every printable glyph is an
  8x16 image with a `glyphScale` of 3, so a third of it fills an 8x16 pixel
  cell: the console's font is a fixed 8x16 one.
- VERIFIED, the integer arithmetic: with `lines` the console's height in
  pixels, the version string sits at x `w - len * 8`, row y `lines - 18`;
  the input line's row is `lines - 32`; the scrollback's first row is
  `lines - 48`, or `lines - 64` when scrolled back, in which case the
  `lines - 48` row draws a marker every 4 columns from x 8; rows step 16
  pixels up. The column count is `max(w, 640) / 8 - 2` (`0x408860`).
- INFERRED from Q3's `Con_DrawSolidConsole`: the prompt `]` is at x 8, the
  field at x 16, scrollback rows at x 8, and the marker is `^`.
- VERIFIED: the colour is reset to white (`0x5419c0`, four 1.0s) before the
  scrollback, and the stored text cells start as `0x0720`, a space in
  colour 7.

## 4. Binds, `cl_run` and the ads key

VERIFIED: `bind`, `unbind`, `unbindall` and `bindlist` are registered beside
the strings `bind <key> [command] : attach a command to a key` (`0x5678f0`),
`"%s" isn't a valid key` (`0x567924`), `"%s" = "%s"` (`0x5678e0`) and
`"%s" is not bound` (`0x5678cc`). The key names are the `KEY_*` table and its
short twin at `0x567a34`..`0x5682ec`. The config writer's formats are
`bind %s "%s"` (`0x5678b0`) and `seta %s "%s"` (`0x562134`), after an
`unbindall` (`0x5678c0`); a cvar is printed as `"%s" is:"%s^7"
default:"%s^7"` (`0x56225c`) and `cvarlist` ends on `%i total cvars`.

VERIFIED: the key handler formats `-%s %i %i` on a key-up whose bind starts
with `+` and queues it (the call at the top of the release branch of
`0x40dc30`). So a `+` command gets its `-` twin on release.

VERIFIED: `cl_run` is registered at `0x411e60` with the default
`"1"` (`0x5685e4`) and flag `0x100`. The `+speed` handler loads the button
record at `0x87a100` (`0x40a8a1`); its `active` word is `0x87a110`. The
usercmd builder sets `buttons` bit `0x10`, ads, when `cl_run`'s integer
equals that `active` word and clears it otherwise (`0x40aefb`..`0x40af0e` and
again at `0x40b130`). So `+speed` is the ads key, held to aim at the default,
and `toggle cl_run` turns it into a toggle. vcod mirrors this in
`play::input::ClRun`.
