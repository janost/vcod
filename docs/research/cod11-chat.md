# CoD 1.1 chat, game messages and the chat field

How a chat line gets from a client's `say` (or a script's `sayAll`) to the
other clients' screens, and how the client lays out chat, game messages and
bold messages. Server facts are from `game.mp.i386.so` (1.1d Linux, symbols
present, file-relative addresses; Ghidra's export adds 0x10000) and
`cod_lnxded`; client facts from `cgame_mp_x86.dll` (image base
`0x30000000`) and `CoDMP.exe` (`0x00400000`), md5s in
`docs/research/cod11-hud-protocol.md`. Live captures were taken on
2026-10-06 against the retail 1.1d server with `vcod --net-probe
--probe-say` and `client-probes/probe_say.gsc`.

## 1. Who calls `G_Say`

VERIFIED, `ClientCommand` (0x487ec): `say` (string 0x73f69) calls
`Cmd_Say_f` (0x47050) with mode 0, `say_team` (0x73f6d) with mode 1,
`tell` (0x73f85) calls `Cmd_Tell_f` (0x47210). `vsay` and `vsay_team` go to
`G_Voice` (`docs/research/cod11-quick-chat.md`). INFERRED, off the branch
order: these four come before the `pm_type == 5` return at 0x48909, so chat
works during intermission.

VERIFIED, `Cmd_Say_f`: with fewer than two arguments it returns; otherwise
it joins `argv[1..]` with single spaces into a 0x3fe-byte buffer and calls
`G_Say(ent, NULL, mode, text)`. VERIFIED live: `say a   b "c d" 100% ^1red`
arrives as `a b c d 100% ^1red`, `say    spaced   ` as `spaced`, and a bare
`say` or `say_team` sends nothing.

VERIFIED, `Cmd_Tell_f`: `argv[1]` is a client number below
`level.maxclients` whose entity is in use and has a client; the text is
`argv[2..]` joined the same way; it logs `tell: %s to %s: %s` and calls
`G_Say(ent, target, 2, text)` then `G_Say(ent, ent, 2, text)`.

VERIFIED, `sayAll` (0x45884, method slot 19) and `sayTeam` (0x45920, slot
20): `Scr_ConstructMessageString` packs the arguments into a buffer behind
a leading `\x14` (0x3ff bytes), and `G_Say(self, NULL, 0 or 1, buffer)`.
VERIFIED live: `self sayAll("plain words")` reaches clients as
`...^7\x14plain words`, `self sayAll(&"MPSCRIPT_WINS", self)` as
`...^7\x14MPSCRIPT_WINS\x15vcod^7`.

## 2. `G_Say` (0x46c14)

All VERIFIED off the function body unless labelled; the live captures
below confirm every branch named.

- Mode 1 from a client whose `clientState.team` (`client+0x217c`) is
  neither 1 (axis) nor 2 (allies) becomes mode 0.
- The name is `client+0x21b4` (the userinfo name), `Q_CleanStr`ed into 64
  bytes.
- The status prefix: team 3 gives `\x15(\x14GAME_SPECTATOR\x15)` (0x73989);
  else `ent->health` (`ent+0x230`) below 1 gives `\x15(\x14GAME_DEAD\x15)`
  (0x7399d); else `\x15` (0x739ac).
- The head, `Com_sprintf` into 0x80 bytes:
  - mode 1: `%s(\x14%s\x15)%s%s: ` (0x73a04) with the prefix, `GAME_AXIS`
    for team 1 or `GAME_ALLIES` otherwise, the name and `^7`; colour `5`.
    With a location, `%s(\x14%s\x15)%s%s (\x14%s\x15): ` (0x739ee).
  - mode 0: `%s%s%s: ` with the prefix, the name and `^7`; colour `7`.
  - mode 2: `%s[%s]%s: `, or `%s[%s]%s (%s): ` with a location when the
    target is on the speaker's team; colour `3`.
- The text is `Q_strncpyz`ed into 0x96 bytes (149 characters).
- The line, `va("%s \"\x15%s%c%c%s\"", word, head, '^', colour, text)`
  (0x7397a), where `word` is `i` (0x73976) for mode 1 and `h` (0x73978)
  otherwise, goes out by `trap_SendServerCommand(client, 0, line)`.
- Recipients: with no target, every entity slot below `level.maxclients`;
  with one, that one. A recipient must be in use, have a client, be
  `CON_CONNECTED` (`client+0x20ec` == 2), pass `OnSameTeam` (0x64aec: both
  teams equal and not 0) in mode 1, and, when the speaker's `sessionState`
  (`client+0x20d0`) is not 0 (playing), have a `sessionState` that is not 0
  either. So a dead or spectating speaker reaches only clients out of play.
- `G_LogPrintf` writes `say: %s: %s` or `sayteam: %s: %s`.

INFERRED: the location forms never appear on a stock map.
`Team_GetLocationMsg` (0x647c0) walks `level.locationHead`, which only
`target_location` entities fill, and no `.bsp` in the stock paks contains
that classname (checked by a byte search over `pak*.pk3`). vcod leaves them
out.

VERIFIED, `trap_SendServerCommand`'s type 0 (cod_lnxded 0x808b680): a
type-0 command is dropped for a client that is not active (`state != 4`) or
already 32 or more commands behind its acknowledge. INFERRED: a chat line
never reaches a client still loading, independent of the
`CON_CONNECTED` test.

### 2.1 Live lines

Retail, `tdm` on mp_carentan, three probes (axis alive, spectator, allies
speaking), control bytes escaped:

| Command | Line | Reached |
|---|---|---|
| `say hello all` | `h "\x15\x15vcod^7: ^7hello all"` | all three |
| `say_team hello team` | `i "\x15\x15(\x14GAME_ALLIES\x15)vcod^7: ^5hello team"` | the speaker only (no teammate) |
| `say dead talk` after `kill` | `h "\x15\x15(\x14GAME_DEAD\x15)vcod^7: ^7dead talk"` | speaker, spectator; not the live axis |
| `say_team dead team` | `i "\x15\x15(\x14GAME_DEAD\x15)(\x14GAME_ALLIES\x15)vcod^7: ^5dead team"` | speaker |
| `tell 0 psst` (0 the live axis) | `h "\x15\x15(\x14GAME_DEAD\x15)[vcod]^7: ^3psst"` | speaker only |
| spectator's `say_team spec team` | `h "\x15\x15(\x14GAME_SPECTATOR\x15)vcod^7: ^7spec team"` | all |

`probe_say.gsc` (section 1) on a playing allied speaker with an axis
listener: `sayTeam(&"QUICKMESSAGE_FOLLOW_ME")` reached only the speaker as
`i "...(\x14GAME_ALLIES\x15)vcod^7: ^5\x14QUICKMESSAGE_FOLLOW_ME\x15"`;
health 0 with `sessionstate` playing gave the `GAME_DEAD` prefix and reached
both; `sessionstate` `dead` or `spectator` with health 100 gave no prefix
and reached only the speaker; `sessionteam` `spectator` gave the
`GAME_SPECTATOR` prefix and turned `sayTeam` into an `h` to both;
`sessionteam` `none` turned `sayTeam` into a plain `h`. `pingPlayer` sent
nothing on the reliable stream. vcod's server gives the same lines to the
same clients in both recipes.

### 2.2 The netchan key and `%`

VERIFIED: the server-to-client scramble keys on the server's copy of the
client's last command, raw (`docs/protocol-1.1.md`, "XOR scramble"), and
the server's `MSG_ReadString` keeps `%`. VERIFIED live: before vcod's
client stopped substituting `%` in its key, a probe that sent
`say ... %s ...` lost every later message to garbage.

## 3. The client's handlers

VERIFIED, `CG_ServerCommand` (0x3002e0d0) on `argv[0][0]`:

- `h`: when `cg_teamChatsOnly` (0x3029824c, default 0) is 0, localize
  `argv[1]` (trap 0x39, "chat message"), play `player_talk` (0x301d5d78),
  copy 0x95 bytes, strip every `\x19` (0x3002df00), add it to the chat
  (0x3002c920) and print it to the console.
- `i`: the same with no `cg_teamChatsOnly` test ("team chat message").
- `e`, `f`: localize and bind-substitute (0x300229b0, "game message"),
  then trap 2, the game message window.
- `g`: play `game_message` (0x301d6130), then as `c`.
- `c`: localize and bind-substitute ("announcement message"), then trap 3,
  the bold message window. INFERRED: `c`'s second argument (`2` from
  `announcement`) is never read; nothing in the case reads `argv[2]`.

VERIFIED: the stock sound aliases have `game_message` (a `null.wav`, in
`pak1.pk3`'s `iw_sound.csv`) and no `player_talk`, so both are silent on a
stock install.

### 3.1 Localizing a message (CoDMP.exe 0x4aa040)

INFERRED, off the loop: the string is cut at `\x14`, `\x15` and `\x16`.
The first part, and every part after a `\x14`, is a localization key; a
part after a `\x15` is literal. A key the table lacks comes back as the key
itself, or as `^1UNLOCALIZED(^7%s^1)^7` when `loc_warnings` is set
(0x4a9f80). In each part the first `%s` opens a slot; any later one is
escaped. A part fills the first open slot in what came before it, or is
appended when there is none. A `\x16` after a separator (or standing as
one) makes the next part's `%s` literal. vcod: `Localized::message`.

INFERRED, 0x300229b0: after localizing, the cgame replaces the first
`[{command}]` with the key bound to the command, keeping the brackets, and
leaves the text alone when the command is unbound. vcod fills
`[{+activate}]` with its use key F.

### 3.2 The chat (0x3002c920, drawn by 0x300150b0)

VERIFIED, the cvars: `cg_chatHeight` default `8` (vmCvar at 0x301dc080),
`cg_chatTime` `12000` (0x30297ca0). INFERRED, the ring: at most
`min(cg_chatHeight, 8)` lines of 0x10f bytes with an arrival time each; a
line wraps once it holds 90 characters (`> 0x59`, colour codes not
counted), back at its last space, and the text after that space is read
again onto the next line, which opens with `^` and the last colour read.
Lines beyond the height drop the oldest; each frame drops at most one line
older than `cg_chatTime`.

INFERRED, the draw, newest first, row `k` = -1, -2, ...:

- alpha 1, or `(cg_chatTime - age) * 0.005` (0x30069510) inside the last
  200 ms (0x30069514); a line at alpha 0 or less is skipped.
- a `hudColorBar` (0x301d5a98) strip at x 0, y `84 + 10k`, width the
  line's text width plus 24, height 10, in the viewer's team colour
  (`g_TeamColor_Axis`/`_Allies` read off the viewer's client info, white
  otherwise, 0x3002a810) times 0.25 (0x3006950c) at alpha times 0.6
  (0x30069508).
- the text at baseline `93 + 10k`, font 0, scale 0.208333 (0x3e555555):
  when the line has a `^7` past its first byte (0x30065348), the part
  before it in the team colour at x 8, then the rest in white from the
  part's width plus 8 (0x3006931c); otherwise the whole line white at x 8.

So the chat sits top left, the newest line lowest, in a band from y 4 to
84.

### 3.3 Game and bold message windows (CoDMP.exe)

VERIFIED, the cvars (0x408d70): `con_gamemessagetime` `5`,
`con_boldgamemessagetime` `8` (seconds; times 1000 at 0x569090). VERIFIED,
each window's record: a ring of 8 slots (start time, end time, console
line), 3, 250, 250 and 500.

VERIFIED, the cgame's draw calls: trap 0x1a (game messages, CoDMP.exe
0x409e70) at x 6, y `345 - (cg_hudCompassSize - 1) * 160 + 12` (0x30069484,
0x30069480, 0x300693fc), alpha `cg_hudAlpha`, mode 2 (0x30017880); trap
0x1b (bold, 0x409e90) at x 320 (0x140), y 180 (0xb4), mode 3
(0x30017940).

INFERRED, off 0x408be0 and 0x409920:

- a new line takes the current slot with an end time `now + life`, the
  current slot advances, and of the next three slots (the oldest) any whose
  end is more than 500 ms away is cut to end in 500 ms, so a window holds
  five steady lines;
- while a line is younger than 250 ms the whole stack is pushed down by
  `round((1 - age / 250) * line height)`;
- from the newest, each live line moves up a line height (12, or 16 for
  mode 3) and draws at that y plus 12, white, alpha fading in over its
  first 250 ms and out over its last 500;
- mode 2 draws at font 0, scale 0.25, left aligned; mode 3 at font 4,
  scale 1/3 (0x3eaaaaab), centred on x by its width (0x409870).

INFERRED, not measured: the `y + 12` is the text baseline, as the other
text traps take it.

## 4. The chat field

VERIFIED, CoDMP.exe: `messagemode` (0x408440) and `messagemode2` open the
field with team flag 0 or 1 (0x1430380), a 0x100 width in characters and
0x24c in pixels, and a 16.0 character height. The cgame's 2D pass (0x30018810) draws it through trap 0x1e at y
100, right after the game and bold message windows; 0x409d80 draws `EXE_SAY` or `EXE_SAYTEAM` localized plus
`:` (0x568610) at x 8, y + 16, font 0, scale 1/3, and the field after the
label's width plus 8. Enter sends `say "` or `say_team "` (0x567988,
0x567994) with the buffer (0x40d40b). VERIFIED, `exe.str`: `EXE_SAY` is
`say` and `EXE_SAYTEAM` is `say_team`. The stock `ned_mp.cfg` and
`wart_mp.cfg` bind `t` to `messagemode` and `y` to `messagemode2`; vcod
uses the same keys.

## 5. `pingPlayer`

`docs/research/cod11-hud-protocol.md`, "Compass friendlies": `eFlags`
0x80000 until `level.time + 3000`. vcod's server sets it, and the
`iCompassFriendInfo` half that carries it to a teammate's compass.
