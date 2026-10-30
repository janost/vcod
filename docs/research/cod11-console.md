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
console up does not close it: in game the handler opens the menu under it
and the console stays down (`cod11-front-end.md` section 13). vcod does the
same in a game; with no game up its Escape still closes the console.

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

## 5. Client cvars: mouse, fov, rate and snaps

VERIFIED, `CL_Init` (0x411e60) registers, with the default string at the
address given and the flags word: `sensitivity` `"5"` (0x5686b0) flags 1,
`cl_mouseAccel` `"0"` flags 1, `m_pitch` and `m_yaw` `"0.022"` flags 1,
`m_filter` `"0"` flags 1, the userinfo `name` `"Unknown Soldier"` flags 3,
`rate` (0x5667b4) `"5000"` (0x5667bc) flags 3 and `snaps` `"20"`
(0x5667b0) flags 3.
VERIFIED: the `name` row's push is at 0x41236b, and 1.5's and UO's
`CL_Init` register `name` (1.5 0x5a2e60, UO 0x59381c) `"Unknown Soldier"`
flags 3 the same. vcod's shell defaults `name` to it.

VERIFIED: the flag bits are Q3's. `Cvar_Set2` (0x439650) prints `%s is read
only.` on 0x40, `%s is write protected.` on 0x10 and `%s is cheat
protected.` on 0x200 while `sv_cheats`' integer is 0. `sv_cheats` is
registered at 0x4375c0 with `"0"` and flags 0x48. So 1 is `CVAR_ARCHIVE`,
2 is `CVAR_USERINFO` and 0x200 is `CVAR_CHEAT`.

VERIFIED, `cgame_mp_x86.dll`: the cvar table row at 0x300749e0 is `cg_fov`
`"80"` with flags 0x201, so `cg_fov` is archived and cheat-protected.
`xmodel-v14-format.md`, "The view fov", has the 80..160 clamp that reads it.

VERIFIED, `localized_english_pak0.pk3` `default_mp.cfg`: `set sensitivity
5`, `set m_pitch "0.022"`, `set m_filter 0`, `set cl_mouseAccel 0`. It sets
no `rate`, `snaps` or `cg_fov`.

VERIFIED, `CL_MouseMove` (0x40b240): the rate is `cl_mouseAccel * speed +
sensitivity` (values at cvar `+0x1c`) times the scale cgame hands over
(0x143a97c). Yaw takes `mx * rate * m_yaw` off `cl.viewangles[YAW]`
(0x143a9a4); pitch adds `my * rate * m_pitch` to 0x143a9a0. The scale is
stored by the cgame syscall switch (0x401df0), case 0x55, from its second
argument. INFERRED, off the branch on `0x1432a00 & 0xc000` (the mounted
`eFlags` bits): there the rate is the double `2.5` (0x5690b8) for yaw, and
pitch is `2 * my * m_pitch` with no sensitivity.

VERIFIED, `CL_MouseMove` (0x40b240): `CL_MouseEvent` adds the motion to
one of two slot pairs (`0x143a948`/`0x143a950`, index `0x143a958`). With
`m_filter` (cvar `0x161732c`, read as its integer at `+0x20`) 0 the
frame takes its own slot; otherwise the sum of both slots times 0.5
(`0x568e70`). The index then flips and the new slot is cleared, so a
filtered frame spends the mean of its motion and the last frame's. A
frame whose two scaled axes are both 0 does nothing more (`0x40b3e8`).

VERIFIED, same function: with `+strafe` held (its `active` word
`0x87a0f8`; `+strafe` 0x40a8c0) the horizontal motion goes to
`rightmove` instead of yaw: `rightmove + _ftol(mx * rate * m_side)`
(`m_side` cvar `0x1617320`, `_ftol` 0x538be0, which truncates), clamped to
-128..127 (`0x40b400`..`0x40b42a`). The vertical motion pitches the view
when `+mlook` is held (`0x87a2f0`) or `cl_freelook` (cvar `0x1617354`) is
non-zero, and `+strafe` is not (`0x40b4e6`..`0x40b507`); otherwise it goes
to `forwardmove - _ftol(my * rate * m_forward)` (`m_forward` cvar
`0x15ce840`), clamped the same way (`0x40b5c3`..`0x40b5f1`). On a mounted
gun `my` is doubled in place of the rate, so `cl_freelook` 0 there moves
by `2 * my * m_forward`.

VERIFIED: `CL_Init` registers `cl_freelook` `"1"` (`0x5685e4`),
`m_forward` and `m_side` `"0.25"` (`0x5668e8`) and `m_filter` `"0"`, all
flags 1. `+mlook` (0x40a450) sets `0x87a2f0`; `-mlook` (0x40a460) clears
it and, while `cl_freelook` is 0, stores `-(delta_angles[PITCH] *
360/65536)` (`0x14329c8` times `0x568f64`) into `cl.viewangles[PITCH]`
(`0x143a9a0`), Q3's `IN_CenterView`. VERIFIED: 1.5 (CoDMP.exe
`0x40bd60`, `+mlook` 0x40af60, `+strafe` 0x40b3d0) and UO (CoDUOMP.exe
`0x40c330`, `+mlook` 0x40b530, `+strafe` 0x40b990) read `m_filter` and
`cl_freelook` at the same two points with the same branches (their
decompilations).

vcod: `play::input::MouseAccum` keeps the two slots and `App::mouse_frame`
takes them once a frame; `MouseLook::moves` splits the motion, and
`PlayInput` adds the move counts to the next cmd built, as vcod builds
cmds on its own clock rather than one a frame. `-mlook` levels the pitch
over the newest snapshot's `delta_angles`.

VERIFIED, cgame: the fov function (0x30032e20) ends by storing `fov_x /
cg_fov` (0x30032fe2, into 0x3020b5f4), and the frame passes it to trap 0x55
(0x30033ede) with `cg_weaponSelect`, multiplied first by a second factor at
0x3020d5c0 when that is not 1. VERIFIED: that factor is the shellshock's
mouse scale; 0x3002f5b0 writes it off the active shock's record (`+0x70`)
and resets it to 1 when no shock runs (`cod11-shellshock.md`, "Mouse").
vcod multiplies it into the fov ratio (`main.rs`, `look_zoom`).

So the default turn is 5 x 0.022 = 0.11 degrees a count, the same both ways,
and a sight that zooms to half the fov halves it. vcod's
`play::input::MouseLook` and `console::shell` carry these. vcod keeps its
own default `name`. The client's userinfo carries `rate` and `snaps` from the
cvars and resends on a change. The probes keep 25000, so a retail server
never throttles a capture.

The options screens' cvars (`mss_volume`, `r_mode`, `r_fullscreen` and the
inert ones), `setfromcvar`, `exec` and `vid_restart` are in
`cod11-front-end.md`, section 14; `ui_load` and `ui_menuFiles` in its
section 1.

## 6. The talk button

VERIFIED, `0x40b600` (my `CL_CmdButtons`, called from the cmd builder
`0x40b720`): after the key buttons it ORs `buttons` 0x2 (`BUTTON_TALK`)
when the key-catcher word `0x155f2c4` is non-zero and
`cl_bypassMouseInput` (the cvar pointer at `0x142f604`) reads 0 as an
integer. INFERRED: its default, the string at `0x56871c`, is `"0"`, the
value the cmd builder also writes back into `cl_viewPitchCompensate`. The catcher
bits are 1 for the console (section 1), and INFERRED off the routing in
the mouse handler `0x40b0a0` (bit 2 sends a motion to the UI, bit 8 to
the cgame) and the key handler `0x40dc30` (bit 4 to the chat field
`0x40d380`): 2 for a menu, 4 for the `messagemode` line, 8 for the cgame.
So the console, a menu (the main menu over the game, a script menu) or
the chat line up sets the bit on every cmd, whatever keys were held.

VERIFIED, pak0's `ui_mp/wm_quickmessage.menu` and
`ui_mp/scriptmenus/quick{commands,responses,statements}.menu` set
`cl_bypassMouseInput` 1 on open and 0 on close, so a quick message menu
never sends the bit and the player keeps moving under it.

VERIFIED, 1.5 `CoDMP.exe` `0x40c130` (catcher word `0x15b8aa4`,
`cl_bypassMouseInput` at `0x1488824`) and UO `CoDUOMP.exe` `0x40c700`
(`0x4ad3d64`, `0x4e1cbdc`) make the same test and set the same 0x2.

What the server does with the bit (the masked cmd, `eFlags` 0x40000) is in
`cod15-movement.md` 5, "The talk button". vcod: `play::input::talking`
(the console open, `Ui::active`, the HUD's chat field; the bypass read
off the cvar) sets `PlayInput::talking` before each frame's cmds;
`play::loopback`'s `a_key_catcher_lights_the_talk_flag` sees the server
light `EF_TALK` for it. vcod has no cgame key catcher. The cgame draws
the bit as `headiconTalkBalloon` over a teammate's head
(`cod11-gametypes-re-bel.md` 7.3, "The second sprite").

## 7. `rcon`

VERIFIED, `CL_Init` (0x411e60) registers `rconPassword` with the default
`""` (0x559228) and flags 0x100 (Q3's `CVAR_TEMP`, never saved), and
`rconAddress` with `""` and flags 0. 1.5 `CoDMP.exe` and UO
`CoDUOMP.exe` register both with the same defaults and flags. There is no
`rcon_password` cvar; the name survives only in a message (below).

VERIFIED, the command `rcon` (name at 0x566570) is registered with the
handler 0x40f880 (`push 0x40f880` at 0x4126d7). What it does, read off
its disassembly:

- It tests `rconPassword`'s string pointer, not its first byte
  (`mov eax,[edx+4]; test eax,eax` at 0x40f8a1). VERIFIED. INFERRED: a
  cvar's string is never null, so `You must set 'rcon_password'
  before\nissuing an rcon command.\n` (0x5673fc) never prints, and an
  empty password goes out as it stands.
- It builds the message in a 0x400-byte buffer: four 0xff bytes, `rcon `
  (0x5673f4), the password, then for each argument from argv 1 a space
  (0x568a9c) and the argument, wrapped in `"` when it is empty or holds a
  byte at or below 0x20 (0x40f926..0x40f9cd). VERIFIED. So the line is
  rebuilt from its tokens, not sent as typed, the way the server rebuilds
  it again (`cod11-server-handshake.md`, "rcon").
- A message that reaches 0x400 bytes prints `rcon command too long\n`
  (0x5673dc) and sends nothing (0x40f9d3). VERIFIED.
- With `cls.state` (0x155f2c0) at 3 or more it sends to the connection's
  address (the 20 bytes at 0x15ef028), whatever `rconAddress` says
  (0x40f9f1). VERIFIED. INFERRED: 3 is the state the `connectResponse`
  branch of `CL_ConnectionlessPacket` (0x4109d0) sets, so a client still
  challenging or connecting uses `rconAddress`.
- Otherwise an empty `rconAddress` prints `You must either be
  connected,\nor set the 'rconAddress' cvar\nto issue rcon commands\n`
  (0x567388, one string) and sends nothing; a set one goes through
  `NET_StringToAdr` (0x449690) and a port of 0 becomes `htons(0x7120)`,
  28960 (0x40fa0c..0x40fa61). VERIFIED.
- It sends `strlen + 1` bytes, the NUL included (0x40fa66..0x40fa8f).
  VERIFIED.

VERIFIED, 1.5 `CoDMP.exe` 0x410490 and UO `CoDUOMP.exe` 0x4111c0 are the
same function: the pointer test, `rcon command too long` (1.5 0x5a6a3c, UO
0x594704), the not-connected message (0x5a69e8, 0x5946b0), the state
compare against 3 (`cls.state` at 0x15b8aa0, 0x4ad3d60) and the port
0x7120. The packet is the same on all three protocols.

The reply: `CL_ConnectionlessPacket`'s `print` branch (the word at
0x566e68, branch at 0x410db1) reads the rest of the packet
(`MSG_ReadString`, 0x444e00), copies up to 255 bytes into `clc`'s server
message (0x15ce888) and prints it with `%s` (0x568b34). VERIFIED. INFERRED
off the branch order: nothing on that path tests the state or the sender,
so a reply prints whether the client is connected or not.

vcod: `console::rcon` builds the packet and reads a `print` back. While
the connection is past `connectResponse` it goes out on the connection's
socket (`NetClient::send_oob`) and the reply comes back as a
`NetEvent::Print`, which `NetClient` raises for a `print` in any state.
Otherwise it goes to `rconAddress` from a socket of its own, which the
main loop polls for `print` packets in every mode, the main menu
included. A `rconAddress` that does not resolve sends nothing, where
retail's `NET_StringToAdr` leaves a bad address that sends nothing either
(INFERRED from Q3's `NET_SendPacket`, which drops `NA_BAD`).

CoD2 1.3 and CoD4 1.7 have neither cvar nor the not-connected string:
their `CL_Rcon_f` logs in (`rcon login`, `rcon host`). VERIFIED
(cod2-protocol.md 1.6, cod4-handshake.md 14.2). vcod runs theirs on
those games (`Session::login_rcon`).

## 8. vcod's config stamp

`vcod_mp.cfg` archives every `CVAR_ARCHIVE` cvar at its value, so a
value written while vcod's default was something else outlives the
change: an install vcod ran on before kept `r_aaSamples 4` after the
default became retail's 1 (cod11-client-perf.md 4, cod4-client-perf.md
19), and CoD2 kept `r_vc_depthprepass 0` from the opt-in builds
(cod2-client-perf.md 12, now unarchived and its line dropped).

vcod writes `vc_cfgVersion`, an archived cvar of its own, into the
config (`console::shell::CONFIG_VERSION`). A config whose first line is
vcod's header and that has no stamp reads as stamp 0; any other file,
1.7's `config_mp.cfg` included, reads as this build's and nothing in it
is reset. Once the config ran and the late cvars registered
(`r_aaSamples`, CoD2's and CoD4's options), `Shell::upgrade_config`
puts each `STALE_DEFAULTS` row past the stamp back to the cvar's current
default when the cvar holds that row's old value, then writes the
config with the stamp moved. A value the player picked stays, and so
does one set again after the upgrade.

| Stamp | Cvar | Old vcod default | Games | Now |
|---|---|---|---|---|
| 1 | `r_aaSamples` | 4 (CoD1 and UO until 68d97193, CoD4 until 36372025) | CoD1, CoD4 | 1 |
| 1 | `name` | `vcod` (until f5635f70) | all | `Unknown Soldier` |

The rows are vcod's own defaults. An old default that was retail's
has no row: `com_maxfps` 85 (vcod's for a day, then 0), `rate` 5000
(then 25000), `r_picmip` 1, `r_picmip2` 2 and `r_swapInterval` 0 (then
0, 0 and 1). The audit took every archived cvar in `console/shell.rs`
and each default `git log` shows it registered with; nothing else in
the client registers an archived cvar. A default change to an archived
cvar adds a row and bumps `CONFIG_VERSION`.
