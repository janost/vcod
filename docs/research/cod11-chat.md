# CoD 1.1 chat, game messages and the chat field

A chat line's path from a client's `say` (or a script's `sayAll`) to the other clients' screens, and how the client lays out chat, game messages and bold messages.

Sources: server facts from `game.mp.i386.so` (1.1d Linux, symbols present, file-relative addresses; Ghidra's export adds 0x10000) and `cod_lnxded`; client facts from `cgame_mp_x86.dll` (image base `0x30000000`) and `CoDMP.exe` (`0x00400000`), md5s in `cod11-hud-protocol.md`. Live captures: the retail 1.1d server with `vcod --net-probe --probe-say` and `client-probes/probe_say.gsc`.

## 1. Who calls `G_Say`

VERIFIED, `ClientCommand` (0x487ec):

| Command (string) | Handler |
|---|---|
| `say` (0x73f69) | `Cmd_Say_f` (0x47050), mode 0 |
| `say_team` (0x73f6d) | `Cmd_Say_f`, mode 1 |
| `tell` (0x73f85) | `Cmd_Tell_f` (0x47210) |
| `vsay`, `vsay_team` | `G_Voice` (`cod11-quick-chat.md`) |

INFERRED, off the branch order: all four come before the `pm_type == 5` return at 0x48909, so chat works during intermission.

VERIFIED, `Cmd_Say_f`: under two arguments it returns. Otherwise it joins `argv[1..]` with single spaces into a 0x3fe-byte buffer and calls `G_Say(ent, NULL, mode, text)`. VERIFIED live: `say a   b "c d" 100% ^1red` arrives as `a b c d 100% ^1red`, `say    spaced   ` as `spaced`; a bare `say` or `say_team` sends nothing.

VERIFIED, `SV_ExecuteClientCommand` (cod_lnxded 0x8086d58): `Cmd_TokenizeString` (0x805b398, calling 0x805adc0 with no token limit) splits a client command, so `say` sees the argv a console line or a query would. VERIFIED, the rules in 0x805adc0:

- A byte up to a space separates. The compares are on a signed `char`, so a byte past 127 separates too.
- A `"` opens a token running to the next `"` or the end. Neither comment form counts inside it.
- Outside quotes, `//` ends the line, also mid-token. `/*` skips to the first `*/` from its own `*` on (so `/*/` is a whole comment), or ends the line when there is none.
- 256 tokens at most.

The server's reader has already turned every byte past 127 into `.` and kept `%` (`docs/protocol-1.1.md`), so a high byte never reaches the tokenizer from a client.

VERIFIED live, retail 1.1d, 1.5b and UO 1.51b, a spectator probe's `--probe-say`, identical on all three (the `%` rows from the server log, since the client's own reader turns `%` into `.`):

| `say` text | Chat text |
|---|---|
| `http://x` | `http:` |
| `a /* b */ c` | `a c` |
| `a/*b` | `a` |
| `"x//y" z` | `x//y z` |
| `a /*/ b` | `a b` |
| `aé b` | `a.. b` |
| `a;b\c` | `a;b\c` |
| `a//` | `a` |
| `//a` | nothing sent |
| `a/b /c` | `a/b /c` |
| `a*/b` | `a*/b` |
| `"a b" "c` | `a b c` |
| `a%b`, `100% x` | `a%b`, `100% x` |

VERIFIED live, same servers: the console's `say` over rcon reads the line twice (handshake doc, "rcon"):

| rcon line | Reached clients as |
|---|---|
| `rcon <pw> say "q//r" s` | `console: q` |
| `"q r//t" s` | `console: q r//t s` |
| `http://x` | `console: http:` |
| `a/*/b` | `console: a b` |
| `//x` | nothing |

vcod's server (`game::say::tokenize`) gives the same line for every row of both tables.

VERIFIED, `Cmd_Tell_f`: `argv[1]` is a client number below `level.maxclients` whose entity is in use and has a client. The text is `argv[2..]` joined the same way. It logs `tell: %s to %s: %s`, then calls `G_Say(ent, target, 2, text)` and `G_Say(ent, ent, 2, text)`.

VERIFIED, `sayAll` (0x45884, method slot 19) and `sayTeam` (0x45920, slot 20): `Scr_ConstructMessageString` packs the arguments into a 0x3ff-byte buffer behind a leading `\x14`, then `G_Say(self, NULL, 0 or 1, buffer)`. VERIFIED live: `self sayAll("plain words")` reaches clients as `...^7\x14plain words`, `self sayAll(&"MPSCRIPT_WINS", self)` as `...^7\x14MPSCRIPT_WINS\x15vcod^7`.

## 2. `G_Say` (0x46c14)

VERIFIED off the function body, each branch confirmed by the live captures in 2.1:

- Mode 1 from a client whose `clientState.team` (`client+0x217c`) is neither 1 (axis) nor 2 (allies) becomes mode 0.
- The name is `client+0x21b4` (the userinfo name), `Q_CleanStr`ed into 64 bytes.
- Status prefix: team 3 gives `\x15(\x14GAME_SPECTATOR\x15)` (0x73989); else `ent->health` (`ent+0x230`) below 1 gives `\x15(\x14GAME_DEAD\x15)` (0x7399d); else `\x15` (0x739ac).
- The head, `Com_sprintf` into 0x80 bytes:

| Mode | Format | Colour |
|---|---|---|
| 1 | `%s(\x14%s\x15)%s%s: ` (0x73a04): prefix, `GAME_AXIS` for team 1 else `GAME_ALLIES`, name, `^7`. With a location `%s(\x14%s\x15)%s%s (\x14%s\x15): ` (0x739ee) | `5` |
| 0 | `%s%s%s: `: prefix, name, `^7` | `7` |
| 2 | `%s[%s]%s: `, or `%s[%s]%s (%s): ` with a location when the target is on the speaker's team | `3` |

- The text is `Q_strncpyz`ed into 0x96 bytes (149 characters).
- The line is `va("%s \"\x15%s%c%c%s\"", word, head, '^', colour, text)` (0x7397a), `word` being `i` (0x73976) for mode 1 and `h` (0x73978) otherwise, sent by `trap_SendServerCommand(client, 0, line)`.
- Recipients: with no target, every entity slot below `level.maxclients`; with one, that one. A recipient must be in use, have a client, be `CON_CONNECTED` (`client+0x20ec` == 2), pass `OnSameTeam` (0x64aec: teams equal and not 0) in mode 1, and, when the speaker's `sessionState` (`client+0x20d0`) is not 0 (playing), have a nonzero `sessionState` too. A dead or spectating speaker reaches only clients out of play.
- `G_LogPrintf` writes `say: %s: %s` or `sayteam: %s: %s`.

INFERRED: the location forms never appear on a stock map. `Team_GetLocationMsg` (0x647c0) walks `level.locationHead`, which only `target_location` entities fill, and a byte search over `pak*.pk3` finds that classname in no stock `.bsp`. vcod leaves them out.

VERIFIED, `trap_SendServerCommand` type 0 (cod_lnxded 0x808b680): dropped for a client that is not active (`state != 4`) or 32 or more commands behind its acknowledge. INFERRED: a chat line never reaches a client still loading, whatever the `CON_CONNECTED` test says.

### 2.1 Live lines

VERIFIED live, retail `tdm` on mp_carentan, three probes (axis alive, spectator, allies speaking), control bytes escaped:

| Command | Line | Reached |
|---|---|---|
| `say hello all` | `h "\x15\x15vcod^7: ^7hello all"` | all three |
| `say_team hello team` | `i "\x15\x15(\x14GAME_ALLIES\x15)vcod^7: ^5hello team"` | the speaker only (no teammate) |
| `say dead talk` after `kill` | `h "\x15\x15(\x14GAME_DEAD\x15)vcod^7: ^7dead talk"` | speaker, spectator; not the live axis |
| `say_team dead team` | `i "\x15\x15(\x14GAME_DEAD\x15)(\x14GAME_ALLIES\x15)vcod^7: ^5dead team"` | speaker |
| `tell 0 psst` (0 the live axis) | `h "\x15\x15(\x14GAME_DEAD\x15)[vcod]^7: ^3psst"` | speaker only |
| spectator's `say_team spec team` | `h "\x15\x15(\x14GAME_SPECTATOR\x15)vcod^7: ^7spec team"` | all |

VERIFIED live, `probe_say.gsc` on a playing allied speaker with an axis listener:

| Script state | Result |
|---|---|
| `sayTeam(&"QUICKMESSAGE_FOLLOW_ME")` | speaker only: `i "...(\x14GAME_ALLIES\x15)vcod^7: ^5\x14QUICKMESSAGE_FOLLOW_ME\x15"` |
| health 0, `sessionstate` playing | `GAME_DEAD` prefix, both |
| `sessionstate` `dead` or `spectator`, health 100 | no prefix, speaker only |
| `sessionteam` `spectator` | `GAME_SPECTATOR` prefix, `sayTeam` becomes an `h` to both |
| `sessionteam` `none` | `sayTeam` becomes a plain `h` |
| `pingPlayer` | nothing on the reliable stream |

vcod's server gives the same lines to the same clients in both recipes.

### 2.2 The netchan key and `%`

VERIFIED: the server-to-client scramble keys on the server's raw copy of the client's last command (`docs/protocol-1.1.md`, "XOR scramble"), and the server's `MSG_ReadString` keeps `%`. A client that substitutes `%` in its own copy of the key garbles every later message after a `say ... %s ...` (VERIFIED live, an older vcod client).

## 3. The client's handlers

VERIFIED, `CG_ServerCommand` (0x3002e0d0) on `argv[0][0]`:

| Command | Action |
|---|---|
| `h` | when `cg_teamChatsOnly` (0x3029824c, default 0) is 0: localize `argv[1]` (trap 0x39, "chat message"), play `player_talk` (0x301d5d78), copy 0x95 bytes, strip every `\x19` (0x3002df00), add to the chat (0x3002c920), print to the console |
| `i` | the same with no `cg_teamChatsOnly` test ("team chat message") |
| `e`, `f` | localize and bind-substitute (0x300229b0, "game message"), then trap 2, the game message window |
| `g` | play `game_message` (0x301d6130), then as `c` |
| `c` | localize and bind-substitute ("announcement message"), then trap 3, the bold message window |

INFERRED: `c`'s second argument (`2` from `announcement`) is never read; nothing in the case reads `argv[2]`.

VERIFIED: the stock aliases have `game_message` (a `null.wav`, in `pak1.pk3`'s `iw_sound.csv`) and no `player_talk`, so both are silent on a stock install.

### 3.1 Localizing a message (CoDMP.exe 0x4aa040)

INFERRED, off the loop:

- The string is cut at `\x14`, `\x15` and `\x16`. The first part, and every part after a `\x14`, is a localization key; a part after a `\x15` is literal.
- A key the table lacks comes back as itself, or as `^1UNLOCALIZED(^7%s^1)^7` when `loc_warnings` is set (0x4a9f80).
- In each part the first `%s` opens a slot; any later one is escaped. A part fills the first open slot before it, or is appended when there is none.
- A `\x16` after a separator (or standing as one) makes the next part's `%s` literal.

vcod: `Localized::message`.

VERIFIED, 0x300229b0's body: after localizing (trap `0x39`), the cgame finds the first `[{` with at least five bytes from it, then the first `}]` after it, and replaces the command between them with its key text, brackets kept. Only that first one is tried. No `}]`, a command with no key, or a result past 1023 bytes (`String too long to add key binding: %s`) returns the text unchanged. Callers: the centre print, the game and bold game messages, hudelem text and labels (`"hudelem string"`), the cursor hint string and the scoreboard's objective.

VERIFIED, the key text: `0x30046590` rescans a fixed table of 24-byte rows, filling each row's first two keys, from key 0 up, whose binding equals the row's command case-blind (`0x300464e0`, trap `0xb0`).

| Version | Table | Rows | Differences |
|---|---|---|---|
| 1.1 | `0x3006f150`..`0x3006f5ff` | 50 | |
| 1.5 | `0x30071150` | 50 | `messagesquad` where 1.1 has `messagemode4` |
| UO | `0x3008adb8` | 55 | adds `mp_quickmap`, `mp_purchase`, `weaponslot satchel`, `weaponslot binocular`, `+sprint` |

`0x30046860` finds the command's row case-blind. A command in no row, or a row with no key, reads unbound (`0x30046940` returns 0). The text is the key name (trap `0xac`), localized when the table has it (`KEY_SPACE` "Space"); two keys join by `" %s "` (`0x30060710`) of `KEY_OR`. 1.5's cgame (`0x30023c90`) and UO's (`0x3002d850`) keep the same pass (same string). vcod: `hud::bind_keys`, `hud::bind_commands`, the key text `Shell::key_text` (the stance hints, the vote line and the weapon strip use the same lookup).

### 3.2 The chat (0x3002c920, drawn by 0x300150b0)

VERIFIED, the cvars, all flags 1 (archived):

| Cvar | Default | Where |
|---|---|---|
| `cg_chatHeight` | `8` | vmCvar 0x301dc080; cgame table row 0x30074ed4 (1.5 0x30076e94, UO 0x30085734) |
| `cg_chatTime` | `12000` | vmCvar 0x30297ca0; row 0x30074ec4 (1.5 0x30076e84, UO 0x30085724) |
| `cg_teamChatsOnly` | `"0"` | exe (`CL_Init`, push at CoDMP.exe 0x412318) and cgame (row 0x30074f94, vmCvar 0x30298240) |

1.5 and UO keep the same defaults and flags.

VERIFIED, 0x3002c920's head: a `cg_chatHeight` or `cg_chatTime` integer below 1 zeroes the ring's head and tail and adds nothing. The draw (0x300150b0) returns at once on a height below 1.

INFERRED, the ring:

- At most `min(cg_chatHeight, 8)` lines of 0x10f bytes, each with an arrival time.
- A line wraps once it holds 90 characters (`> 0x59`, colour codes not counted), back at its last space. The text after that space is read again onto the next line, which opens with `^` and the last colour read.
- Lines beyond the height drop the oldest. Each frame drops at most one line older than `cg_chatTime`.

VERIFIED, 1.5's `CG_AddToTeamChat` (cgame 0x3002de50): the same 0x10f ring and `> 0x59` wrap, taking `^0`..`^9` as codes; UO's (0x30039390, ring at 0x3044ade8) likewise. VERIFIED, UO's continuation line opens with two codes: `^` and the first colour the line read, then `^` and the last one. The first starts as byte 0xff, which the renderer does not read as a code. VERIFIED: that first code is the message's, set on the first code read and never reset at a wrap. 1.5's continuation opens with the last code only, as 1.1's. vcod: `hud::chat::wrap` under `Rules::uo`.

vcod reads `cg_chatHeight`, `cg_chatTime` and `cg_teamChatsOnly` off its console cvars each frame (`hud::chat::Rules`, `hud::chat::takes`). With the height lowered under the lines held, vcod draws the newest ones only; retail's ring then indexes modulo the new height, which vcod does not model.

INFERRED, the draw, newest first, row `k` = -1, -2, ...:

- Alpha 1, or `(cg_chatTime - age) * 0.005` (0x30069510) inside the last 200 ms (0x30069514). A line at alpha 0 or less is skipped.
- A `hudColorBar` (0x301d5a98) strip at x 0, y `84 + 10k`, width the text width plus 24, height 10, in the viewer's team colour (`g_TeamColor_Axis`/`_Allies` off the viewer's client info, white otherwise, 0x3002a810) times 0.25 (0x3006950c), at alpha times 0.6 (0x30069508).
- The text at baseline `93 + 10k`, font 0, scale 0.208333 (0x3e555555). When the line has a `^7` past its first byte (0x30065348), the part before it draws in the team colour at x 8 and the rest in white from the part's width plus 8 (0x3006931c). Otherwise the whole line is white at x 8.

The chat sits top left, newest line lowest, in a band from y 4 to 84.

### 3.3 Game and bold message windows (CoDMP.exe)

VERIFIED, the cvars (0x408d70): `con_gamemessagetime` `5`, `con_boldgamemessagetime` `8` (seconds; times 1000 at 0x569090). VERIFIED, each window's record: a ring of 8 slots (start time, end time, console line), 3, 250, 250 and 500.

VERIFIED, the cgame's draw calls:

| Window | Trap | Position | Mode |
|---|---|---|---|
| game messages | 0x1a (CoDMP.exe 0x409e70) | x 6, y `345 - (cg_hudCompassSize - 1) * 160 + 12` (0x30069484, 0x30069480, 0x300693fc), alpha `cg_hudAlpha` | 2 (0x30017880) |
| bold | 0x1b (0x409e90) | x 320 (0x140), y 180 (0xb4) | 3 (0x30017940) |

INFERRED, off 0x408be0 and 0x409920:

- A new line takes the current slot with end time `now + life` and the slot advances. Of the next three slots (the oldest), any ending more than 500 ms away is cut to end in 500 ms, so a window holds five steady lines.
- While a line is younger than 250 ms the whole stack is pushed down by `round((1 - age / 250) * line height)`.
- From the newest, each live line moves up a line height (12, or 16 for mode 3) and draws at that y plus 12, white, alpha fading in over its first 250 ms and out over its last 500.
- Mode 2 draws at font 0, scale 0.25, left aligned; mode 3 at font 4, scale 1/3 (0x3eaaaaab), centred on x by its width (0x409870).

INFERRED, not measured: the `y + 12` is the text baseline, as the other text traps take it.

VERIFIED, the wrap. Trap 2 (0x401e52) and trap 3 (0x401fc2) cut the message at each `\n` (0x401ed7: a `\n` ends a line, an empty one too; nothing follows a trailing one) and hand each part to 0x408f80 with window 1 or 2, the life, the cgame's width and the running colour, which the call returns for the next part. The width is the trap's second argument:

| Cvar | vmCvar | Default | cgame row (1.1 / 1.5 / UO) |
|---|---|---|---|
| `cg_gameMessageWidth` | 0x301dfc20 | `70` | 0x300750c0 / 0x300770b0 / 0x30085950 |
| `cg_gameBoldMessageWidth` | 0x3020d700 | `60` | 0x300750d0 / 0x300770c0 / 0x30085960 |

Both archived (flags 1) on all three. A width of 0, or one over the console's line width (0x142ef54, `max(vidWidth, 640) / 8 - 2`, set at 0x4088f0), is clamped to that, so 70 and 60 hold. In 0x408f80, for windows 1 and 2:

- `len` is the part's length less `^0`..`^7` pairs, `\n` and `\r` (0x4aa6c0). Over the width, the line count is `ceil(len / width)` (0x408fd7) and the soft width `len / count` (0x408fec); otherwise the soft width is the width.
- A `^0`..`^7` pair sets the colour and writes nothing.
- Before every other character the rest of its word is counted: bytes above `0x20` from it, at most the width (0x409059). When that count is under the width and column plus count is over it, the line ends first (0x40907d). The count runs from each character, not the word's start, so a word longer than the width breaks where its last `width - 1` characters start: 100 letters at 70 go out as 31 and 69.
- A space at column 0 after any wrap is dropped (0x40909b). Any other character is written. The line ends when the column reaches the width, or reaches the soft width on a space (0x4090d8..0x4090eb), the space kept at the line's end.
- What is left ends a line at the end (0x40913f).

Each ended line takes a ring slot (0x408d00, 0x408be0) like a whole message. VERIFIED: 1.5's 0x409760 and UO's 0x409dc0 have the same shape but take `^0`..`^9` as colour pairs. INFERRED, not read: the window draw ignores the blanks after a line's text, so vcod measures a bold line without its trailing space.

vcod: `messages::wrap`, with `^0`..`^9` on 1.5 and UO (`hud::font::is_colour_code`, cod11-hud-protocol.md 7). The client registers both width cvars with the cgame's defaults and flags and reads them each frame (`messages::line_width` holds the clamp).

## 4. The chat field

VERIFIED, CoDMP.exe:

- `messagemode` (0x408440) and `messagemode2` open the field with team flag 0 or 1 (0x1430380), a width of 0x100 characters and 0x24c pixels, and a 16.0 character height.
- The cgame's 2D pass (0x30018810) draws it through trap 0x1e at y 100, right after the game and bold message windows.
- 0x409d80 draws `EXE_SAY` or `EXE_SAYTEAM` localized plus `:` (0x568610) at x 8, y + 16, font 0, scale 1/3, and the field after the label's width plus 8.
- Enter sends `say "` or `say_team "` (0x567988, 0x567994) with the buffer (0x40d40b).

VERIFIED, `exe.str`: `EXE_SAY` is `say`, `EXE_SAYTEAM` is `say_team`. The stock `ned_mp.cfg` and `wart_mp.cfg` bind `t` to `messagemode` and `y` to `messagemode2`; vcod uses the same keys.

## 5. `pingPlayer`

`cod11-hud-protocol.md`, "Compass friendlies": `eFlags` 0x80000 until `level.time + 3000`. vcod's server sets it (`builtins::client::ping_player`), and the `iCompassFriendInfo` half that carries it to a teammate's compass.
