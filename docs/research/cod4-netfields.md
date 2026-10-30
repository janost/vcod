# CoD4 1.7: netfield tables, the delta codec, snapshots

What a 1.7 client reads after the configstrings: the netfield tables, the
per-field codes, the entity, client and playerstate readers, the end of the
gamestate and the snapshot. It continues `cod4-handshake.md`, which covers
the netchan, the scramble, the Huffman code and the configstring op. Code:
`crates/cod4/src/net/` (`fields.rs` and `events.rs` generated, `delta.rs`, `parse.rs`, `pure.rs`, `stats.rs`); gate:
`crates/cod4/tests/net_decode.rs`.

## Sources

- `cod4_lnxded` 1.7 (Linux dedicated server, ELF i386, stripped) and
  `iw3mp.exe` 1.7 (Windows client, image base 0x400000, md5
  8c40cde102a9d8297a8674cb149f96ed). Addresses with no module name are
  `cod4_lnxded`'s for the tables and `iw3mp.exe`'s for code. A claim
  labelled INFERRED that names no 1.7 address is not yet confirmed on
  1.7.
- Live captures, `crates/cod4/tests/fixtures/net/`: `handshake-1.7.txt`
  and `handshake-1.7-cmds.txt` (`cod4-handshake.md`), and
  `snapshots-1.7.txt`, 15 s of spectator snapshots on mp_crash
  (section 6), the playing pair `play-1.7-walk.txt` and `-spec.txt`
  (section 8) and the kill pair `play-1.7-kill-target.txt` and
  `-shooter.txt` (section 10).

## 1. The tables

`tools/re/cod4_netfields.py <binary>` finds them: a row is 16 bytes,
`{char *name; int offset; int bits; int changeHints}`; a table starts at a
row something points at whose name opens one of the known tables (`eType`,
`modelindex`, `commandTime`, `origin[0]`, `color.rgba`, `absmin[1]`) and
runs to the next table or the first row whose name pointer reads no
identifier. `--rust` writes `crates/cod4/src/net/fields.rs`, `--rows`
prints every row for a diff.

- Both binaries hold the same 1273 rows in the same order: `--rows` of
  `cod4_lnxded` and of `iw3mp.exe` diff empty. VERIFIED.

| Table | Rows | `cod4_lnxded` | `iw3mp.exe` |
|---|---|---|---|
| clientState | 24 | 0x8227a60 | 0x6baee0 |
| playerState | 141 | 0x8227be0 | 0x6bb060 |
| objective | 6 | 0x82284c0 | 0x6bb930 |
| hudelem | 40 | 0x8228520 | 0x6bb990 |
| archived entity (server only, not read here) | 69 | 0x8227600 | 0x6baa90 |
| entity, 13 tables | 59, helicopter 58, plane 60 | 0x8225080.. | 0x6b7aa0.. |

All VERIFIED.

The per-eType list is 18 `{table, count}` pairs at 0x8228be0 (`iw3mp.exe`
0x6bc010), found by the tool as the one run of 18 pairs whose pointers all
land on `eType` tables of the stated length. VERIFIED. The eType names are
INFERRED; the table each one points at is
VERIFIED:

| eType | Table | eType | Table |
|---|---|---|---|
| 0 general | general (0x8225080) | 9 loop fx | loop fx (0x8226e80) |
| 1 player | player (0x8225800) | 10 primary light | general |
| 2 player corpse | corpse (0x8225bc0) | 11 mg42 | general |
| 3 item | item (0x8226340) | 12 helicopter | helicopter (0x8229040, 58) |
| 4 missile | missile (0x8227240) | 13 plane | plane (0x82293e0, 60) |
| 5 invisible | general | 14 vehicle | vehicle (0x8225f80) |
| 6 script mover | script mover (0x8226700) | 15 vehicle collmap | general |
| 7 sound blend | sound blend (0x8226ac0) | 16 vehicle corpse | general |
| 8 fx | fx (0x8228c80) | 17 events | events (0x8225440) |

- The client's entity reader compares the eType with 0x11 and takes 0x11
  for any higher one (0x506f14), so every event eType reads with row 17's
  table. INFERRED.
- `changeHints` values: 0 on most rows; 1 on 157 entity rows and on
  `adsDelayTime`; 2 on 21 entity rows and six playerstate rows (five 1-bit
  fields and `shellshockDuration`); 3 on eight playerstate rows, origin,
  velocity, `bobCycle` and `movementDir`. VERIFIED.

## 2. The field codes

`MSG_ReadDeltaField` is `iw3mp.exe` 0x5067c0: it tests `changeHints` against
2 (0x5067f4), then compares `bits` with each special width in turn
(0x5068a5..0x506ab4). INFERRED. Its constants are VERIFIED where the table
says so; the semantics of each branch are INFERRED from that code. The codec in `delta.rs::read_field` follows it.

- A field whose `changeHints` is not 2 opens with a change bit; 0 keeps the
  base. INFERRED.

| bits | Read | Constant evidence |
|---|---|---|
| 0 (float) | bit 0: a zero, then its sign bit. Bit 1, then bit 1: a long XOR the base; bit 0: 5 bits plus a byte, XOR `(int)base + 4096`, minus 4096 | `shl 0x5`, `add 0x1000` at 0x50685b, 0x506865 VERIFIED |
| -89 | as 0 without the zero branch | INFERRED |
| -88 | a long XOR the base | INFERRED |
| -99 | bit 0: 0. Else bit 1: long XOR base; bit 0: 4 bits plus a byte, XOR `(int)base + 2048`, minus 2048 | `shl 0x4`, `add 0x800` at 0x506902, 0x50690c VERIFIED |
| -98 | 24-bit flags: bit 1, three bytes; bit 0, 5 bits naming one bit to toggle | INFERRED |
| -97 | a time: bit 1, a long; bit 0, the snapshot time minus 8 bits | `push 0x8` at 0x50696e VERIFIED |
| -96 | ground entity: bit 1, 1022; else bit 1, 0; else 2 bits plus a byte | INFERRED |
| -95 | 7 bits times 100 | `push 0x7`, `imul 0x64` at 0x5069de VERIFIED |
| -94, -93 | a byte (event, event parm) | INFERRED |
| -92, -91, -90 | origin x, y, z: bit 0, a 7-bit step minus 64 off the base; bit 1, 16 bits XOR `(int)base + 0x8000 - c`, where `c` is the map centre axis plus 0.5, truncated | 16 bits and 0x8000 at 0x50663e..0x50665e, centre at 0x956eb8 (x, y) and 0x956ec0 (z) VERIFIED |
| -100 | bit 0: 0; bit 1: a short angle | INFERRED |
| -87 | a short angle, no zero bit | INFERRED |
| -86 | 5 bits / 10 + 1.4 | `push 0x5` at 0x506a8c VERIFIED |
| -85 | HUD colour: bit 1, the base with alpha flipped to 0 or 0xff; bit 0, then bit 0, three RGB bytes; then 5 bits times 8 as alpha | INFERRED |
| other | bit 0: 0. Bit 1: `abs(bits) & 7` loose bits, then whole bytes, XOR the base under the width's mask; a negative width sign-extends | INFERRED |

- 1.7's origin code is 16 bits on the client's writer too: the move echo's
  writer at 0x509290 sends `(value - old + 64)` in 7 bits when it fits and
  otherwise `push 0x10` over the XOR biased by 0x8000 (0x509329..0x509351).
  VERIFIED (the constants above).
- The server's origin writer (`cod4_lnxded` 0x813decc) and the field
  writer's equality test for the three (0x813e6b1) take each value as
  `floor(f + 0.5f)` (`frndint` under round-down, then `fistp` under
  truncation): a half goes up, so the map's script model at x 1912.5
  goes out as 1913 and one at -521.5 as -521, where `fistp`'s ties to
  even sent 1912 and -522. VERIFIED (the disassembly, `cod4::wire_ab`'s
  baselines).
- `CS_MAPCENTER` is configstring 12, read with `sscanf("%f %f %f")`
  INFERRED; on mp_crash it reads `429 126.5 137`. VERIFIED live.

## 3. The struct readers

- Entity number (`MSG_ReadEntityIndex`): bit 1, last + 1; else, for 10-bit
  numbers, bit 0, a 4-bit step; bit 1, the whole 10 bits. A 6-bit client
  number skips the step form. INFERRED. `cmp edi,0xa` and
  `push 0x4` sit in it at 0x50672e and 0x506758. VERIFIED.
- Entity (`MSG_ReadDeltaEntityStruct`, around 0x506e10): remove bit, change
  bit, then the last-changed index in the bits 61 needs,
  6: `mov [esp+0x14],0x3d`, `bsr`, `xor 0x1f`, `sub` from 0x20 at 0x506e8e.
  VERIFIED. The eType reads first through the general table's row 0
  (`push 0x6b7aa0` before the call at 0x506f09), then rows 1 to lc - 1 of
  the eType's table, the rest kept from the base. INFERRED.
- Client state: remove bit, change bit, lc in 5 bits (24 rows), rows 0 to
  lc - 1. INFERRED.
- Hudelem: a 5-bit count in use, each with a 6-bit lc and rows 0 to lc
  (`push 0x5` 0x506ffb, `push 0x6` 0x507038, `cmp ecx,0x28` 0x507075).
  VERIFIED. Slots past the count still in use are cleared. INFERRED.
- Playerstate, in order: a bit saying origin and velocity are on the wire;
  lc in 8 bits (141 rows) and rows 0 to lc - 1, the `changeHints` 3 rows
  read against 0 when that bit is set; stats (bit, 5-bit mask: three
  shorts, 6 bits, a byte); ammo (bit, then four groups of bit plus a
  16-bit mask of shorts); ammo in clip (eight such groups); objectives
  (bit, then 16 of 3-bit state plus bit-gated six rows); HUD elems (bit,
  then archival and current, 31 each); weapon models (bit, 128 bytes).
  INFERRED. The capture parse through all of it lands on
  `svc_EOF` in every message (section 6). VERIFIED live.
- The server sends origin and velocity unless the client's echoed origin
  matches and its echo time equals `commandTime`. INFERRED, not yet confirmed
  on 1.7. The probe echoes an origin of 1e6 at time 0, and every
  snapshot carries them. VERIFIED live.

## 4. The end of the gamestate

After the configstring op (`cod4-handshake.md` 5) come baselines, each
`svc_baseline` (3), an entity number and an entity delta from zero; then
`svc_EOF`, `clientNum` as a long, `checksumFeed` as a long, and the
message's own `svc_EOF`. INFERRED. On `handshake-1.7.txt`: 62
baselines (eType 6 script movers and eType 0 general entities, numbers 77
to 367), clientNum 0, checksumFeed 995670164, and the message's last op
ends on its block's last byte. VERIFIED live.

## 5. Snapshots and the client's answer

- `svc_snapshot` (6): long serverTime, byte delta (0 none, else messages
  back), byte snapFlags, the playerstate, entities by number until 1023
  (the entity number codes reset to -1 first), then clients while a bit is
  set, each a 6-bit number and a client delta (reset to -1 again). Entities
  and clients merge with the delta frame's lists as Q3's do, a new entity
  deltaing from its baseline. INFERRED.
- The client's message: the 9-byte plain header (`serverId` byte,
  `messageAcknowledge`, `reliableAcknowledge`), then ops in 3 bits: 0
  move, 1 move without delta, 2 client command, 3 EOF. A move is a count
  byte, the usercmds, three longs of predicted origin and a long predicted
  server time. INFERRED. The 1.7 server takes it so: with
  `examples/cod4_probe.rs` answering each message, `commandTime` follows
  the sent cmd times to the end of the capture. VERIFIED live.
- A usercmd: bit 0 and a long serverTime (or bit 1 and a byte step), then
  a 1-bit "changed" flag XOR the key (section 8 has the rest). The key is
  `checksumFeed ^ messageAcknowledge ^ Com_HashKey(server command, 32)`,
  the command the one `reliableAcknowledge` names: `SV_UserMove`
  (0x8172560) XORs the word at 0x13e77f94, the client's +0x20e5c and the
  hash (0x8197b80, length 32) of its command slot `+0x20e54 & 127`
  (0x81725eb..0x8172633), after refusing a count past 32 (0x81725c3).
  INFERRED (the fields' meaning off the code).
- The op block is compressed with the same static tree and scrambled from
  byte 9 with key `challenge ^ serverId ^ messageAcknowledge`, walking the
  newest server command's text. INFERRED. The capture's 50
  server commands rotate that text and the server reads every message
  (commandTime keeps advancing). VERIFIED live.
- A move without delta (op 1) makes the next snapshot full; the probe sends
  op 0 once it holds a snapshot, and every later snapshot deltas, 1 to 4
  messages back. VERIFIED live.

## 6. The snapshot capture

`snapshots-1.7.txt`: `cargo run -p vcod-cod4 --example cod4_probe --
127.0.0.1:28960 out.txt --secs 15 --auth 127.0.0.2` against `cod4_lnxded
+set developer 0 +set sv_pure 0 ... +map mp_crash` inside the namespace of
`cod4-handshake.md` 7. 513 datagrams over 18.2 s, 91 KB of hex; 303 server
messages: one empty, the 12713-byte gamestate, then 301 snapshots (the
first 1987 bytes, the rest about 25). VERIFIED live.

- Every snapshot parses and ends on its block's last byte; serverTime steps
  50 ms; the playerstate's clientNum is 0, `pm_type` 4; 11 entities (the
  script movers near the spectator's spot, 336 60 260), and one client,
  index 0, team 3. VERIFIED live.
- Two server settings the capture needed. `sv_pure 1` (the default) drops
  a client that sent no `cp` command (`cod4-handshake.md` 8 has the
  check and the command, which the probe now sends). With all-zero
  stats the stock `_globallogic.gsc` kicks the client on connect: it
  checks `getstat(205 + 10 * i)` for i 0..4 on an online game, and
  `level.onlineGame` is hard-coded true (both in the rawfile of the same
  name in `common_mp.ff`). VERIFIED. The console showed `was kicked for
  cheating` after 38 snapshots. VERIFIED live. The probe now sends a
  fresh profile's stats (section 10, "The stats block").

## 7. The writer and the re-encode gate

`delta.rs` writes every field code, entity, client, HUD elem, objective
and playerstate delta, and `parse.rs`'s `Client::write` puts a parsed
server message back together; `clmsg.rs` does the same for the client's
ops. The gate (`tests/net_decode.rs`'s `walk`) re-encodes every message
of every committed capture and compares the compressed block byte for
byte, both directions. All seven captures pass: `handshake-1.7.txt`,
`handshake-1.7-cmds.txt`, `snapshots-1.7.txt`, the two of section 8 and
the two of section 10. VERIFIED live.

The writer's forms follow `MSG_WriteDeltaField` and its helpers,
INFERRED; that every capture
re-encodes with them VERIFIES them on 1.7 for the codes the captures use.
The server's field writer is `cod4_lnxded` 0x813e20a, its per-code
dispatch at 0x813e287..0x813e339. What its forms test, read off that
code:

- Codes 0, -89 and -99 take the short form only for a whole `to`
  (`cvttss2si`, then `fild` and compared) that is not -0 and lies in
  -4096..4095 (-2048..2047 for -99). Code 0 also wants the base's int
  in that range (0x813e3ea..0x813e3fa); -89 (0x813e611) and -99
  (0x813e555) do not test the base. VERIFIED (the disassembly).
- Code 0 sends a zero, either sign, in its zero form; -99's zero form is
  +0 only, and its -0 goes out as a long (0x813e4f4). VERIFIED (the
  disassembly).
- -86 sends `floor((f - 1.4f) * 10 + 0.5f)` (0x813e923, `frndint` under
  round-down). VERIFIED (the disassembly).
- The origin's long form rounds the map centre as `(int)(c + 0.5f)` in
  float (0x813df9a..0x813dfb3). VERIFIED (the disassembly).
- A float out of an int's range or NaN converts to 0x80000000
  (`cvttss2si`; the client's `_ftol2` at `iw3mp.exe` 0x68b360 is
  `cvttsd2si`), which vcod's codec copies (`delta::cvtt`). VERIFIED (the
  disassembly); no capture carries such a base.

What the two states a reader holds do not fix, the parse records:

- Each struct delta's change bits, row by row up to its last-changed
  count (`delta::Sent`), and for an entity or client whether it went out
  removed, unchanged or changed (`delta::Wire`). The server compares its
  own unquantized state, and a playerstate sends rows by its own rule
  (origin and velocity by the first bit), so the bits are kept, not
  rebuilt. INFERRED, not yet confirmed on 1.7.
- `MSG_WriteDeltaPlayerstate` (0x813f586, called from 0x816b930) sends
  the `-87` view angles only with `otherFlags` 2 (`testb $0x2,0x14`),
  an `eFlags` 2 flip (the two states' +0xb0 XORed), a
  `viewlocked_entNum` (+0x59c) other than 0x3ff or `pm_type` 5, tested
  inline for the last-sent count (0x813fa4a..0x813fa81) and again for
  the row writes (0x813faba..0x8140c05). VERIFIED (the disassembly). A
  set byte +0xd of its first argument skips the test and sends them;
  that this is the archived snapshot cod4-pmove.md names is INFERRED.
- A `-85` HUD colour's short form, the alpha toggled between 0 and 0xff
  (`delta::Change::Toggled`). In `play-1.7-walk.txt` HUD elems whose
  alpha reads 0xf8 (the long form's 31 x 8) go to 0 through the toggle:
  the server tested its own alpha, 0xff, not what the wire carried.
  VERIFIED live.
- The playerstate's objective and HUD blocks: whether each was sent and,
  per objective and elem, which rows (`delta::PsWire`).
- The playerstate's stats mask (`PsWire::stats`). At the target's
  respawn in `play-1.7-kill-target.txt` (message 132) the mask read 0xb:
  `stats[3]`, 6 bits on the wire, went out holding the 63 the client
  already had. VERIFIED live. The server compares its own ints, so a
  change above the 6 bits still sets the mask; that the field is the
  identified client and goes from one out-of-range value to another
  is INFERRED (`STAT_IDENT_CLIENT_NUM`). The ammo, clip and
  weapon model masks are still rebuilt from the two states; every
  capture so far agrees with that.
- The gamestate's op order and each configstring's index form. A string
  goes with a 12-bit index when it does not follow the last slot or when
  it is empty: every empty string in `handshake-1.7.txt` carries one,
  consecutive or not. VERIFIED live (the re-encode). That the writer
  does so for a default slot emptied on the server is INFERRED.
- A usercmd's time form, its branch and each field's flag
  (`clmsg::CmdWire`). `CmdWire::natural` picks them as `iw3mp.exe`'s
  writer does (section 8).

Strings read raw, NUL to NUL: a server command, a configstring and a
client command each re-encode only that way. VERIFIED live.

## 8. The playing capture

`play-1.7-walk.txt` and `play-1.7-spec.txt`: three probes on `cod4_lnxded
+set g_gametype war +set sv_pure 0 ... +map mp_crash` in the namespace of
`cod4-handshake.md` 7, the probe's recipe in its header. `mate` (an idle
allies teammate, not committed) and `spec` start together; `walker`
joins a second later on allies, picks `assault_mp,0`, and plays its
script from the spawn for 42 s. `walker`: 842 server and 438 client
messages; `spec`: 902 and 469. VERIFIED live.

- Joining a team: `mr <sv_serverid> <menu> <response>` (the format
  `cmd mr %i %i %s` at `iw3mp.exe` 0x6d1050), `<menu>` the menu's slot
  past `CS_SCRIPT_MENUS` 1970 INFERRED: `mr 16 8 allies`
  (`team_marinesopfor` at 1978), then `mr 16 15 assault_mp,0`
  (`changeclass` at 1985). The server answered with `t 10` and the
  spawn. VERIFIED live. The response's form comes from the stock
  `_menus.gsc` and `_class.gsc` (`strtok(response, ",")`), rawfiles in
  `common_mp.ff`. VERIFIED.
- The usercmd (section 5) as `cod4_lnxded`'s reader (0x813928e, called
  from `SV_UserMove` at 0x817269d) takes it: a time bit and a byte step or
  a long, a keyed changed bit, a keyed branch bit; the short branch keys
  on `key ^ serverTime` and carries attack, two angle shorts and a 4-bit
  move mask; the long one keys attack, the angle shorts and the mask on
  `key`, then folds the time in for angle 2, 20 bits of buttons above
  attack, 7-bit weapon and offhand, two location bytes under button
  0x10000 and the melee yaw short and 8-bit distance under 0x4. Each field
  is a flag bit and, when set, the value XOR the key; an unsent field
  keeps the base's, and the moves always come back from the mask (127,
  -127 or 0 a side). VERIFIED (the bit masks 0x826e250, 0x826e290,
  0x826e25c and 0x826e260 and the button tests); the order INFERRED (the
  reader's flow). The base of a message's first cmd is built from the
  server's playerstate (0x8130ab0, just before). `iw3mp.exe`'s writer
  (0x505c60) sends a byte step when the new time is 0 to 255 ms past the
  base (0x505c75); nothing past the time when the attack bit, the two
  angles and the move masks match and so does everything the long branch
  adds but the location; the short branch when only those four differ;
  else the long one, each flag set when its field differs. A location
  change alone is never sent. INFERRED (the writer's compares,
  0x505cf9..0x505e07). The probe sends three cmds a packet, the first long, the
  rest the fewest bits off it (441 long, 67 short, 802 unchanged in
  `walk`), and the server played them: the origin moved with the
  forward and right keys, `pm_flags` and `eFlags` went through the jump,
  crouch and prone, the rifle fired, the frag flew, and the `say` line
  reached the console log. VERIFIED live. The button bits
  (`net::clmsg::buttons`) the script uses did what they say: attack,
  prone, crouch, jump and the frag here, the sight key and the secondary
  offhand in `cod4-combat.md` 3.2 and 10.1. VERIFIED live.
- The server-to-client scramble keyed on a non-empty client command:
  each server message unscrambles with the text of the client command
  its `reliableAcknowledge` names, `mr ...` and `say ...` included.
  VERIFIED live.
- Entities seen (eType, snapshot samples in `walk`): 1 player 789, 4
  missile 66 (one, number 315, weapon 40, the frag), 6 script mover 9240,
  0 general 7, and 78, a temp entity (17 + 61) three times. Every table
  these use re-encodes. VERIFIED live. No item, corpse, vehicle or fx
  entity appeared.
- Events. On the walker's own playerstate: 6, 14, 26, 32, 33, 34, 73, 76,
  99; on its body as the spectator sees it: 26, 73, 76, 99. VERIFIED
  live. Their 1.7 names are in section 9.
- Every snapshot still carried origin and velocity: the probe echoes an
  origin of 1e6. VERIFIED live.
- `cod4::wire_ab` joins the three probes to vcod-server's `war` in process
  and diffs the walker's and the spectator's server commands, the gamestate
  and the walker's spawn frame against these two captures (`cod4-gsc.md`
  7).

## 9. Event names

`tools/re/cod4_events.py` reads the event name table out of either
binary and writes `crates/cod4/src/net/events.rs` (`EVENT_NAMES`,
`event_name`). The table is an array of 135 string pointers, `EV_NONE`
first and `EV_LANDING_PAIN_PAINTEDMETAL` (134) last: `iw3mp.exe`
0x7294f0, `cod4_lnxded` 0x826ac20, the two equal entry for entry.
VERIFIED. The client indexes it by the event number: `CG_EntityEvent`
loads `[esi*4+0x7294f0]` and prints it after `CG_EntityEvent:%s`
(0x6c8544) at 0x435d88. VERIFIED.

The capture's ids by name (`walk_capture_events_have_1_7_names`), each
VERIFIED off the table:

| id | name | id | name |
|---|---|---|---|
| 6 | `EV_STANCE_FORCE_STAND` | 34 | `EV_SWITCH_OFFHAND` |
| 14 | `EV_RESET_ADS` | 73 | `EV_FOOTSTEP_RUN` |
| 26 | `EV_FIRE_WEAPON` | 76 | `EV_JUMP` |
| 32 | `EV_PREP_OFFHAND` | 99 | `EV_LANDING_ASPHALT` |
| 33 | `EV_USE_OFFHAND` | 61 (temp entity) | `EV_EARTHQUAKE` |

The five ids round 3 had guessed (26, 32, 33, 73, 76) hold on
1.7. VERIFIED. The walker's temp entity 17 + 61 is the frag's
earthquake. VERIFIED (the name; that the frag raised it is INFERRED from
its timing). `cargo run -p vcod-cod4 --example cod4_event_dump --
<capture>` prints a capture's events by name.

## 10. The kill capture

`play-1.7-kill-target.txt` and `play-1.7-kill-shooter.txt`, cut at
20 s: `cod4_lnxded +set sv_pure 1 +set g_gametype war +set
scr_player_maxhealth 30 ... +devmap mp_crash` in the namespace of
`cod4-handshake.md` 7. `target` joins allies with `--script idle --goto
"207 2291 296"`; a second later `shooter` joins axis with `--script
shoot --goto "123 2110 300" --iwds <install>/main`. Each sends
`setviewpos` half a second after its spawn (`devmap` turns cheats on),
which puts them about 200 units apart; the shooter aims down its sights
at the target's chest and taps attack every 250 ms. Both send `cp`
first (`cod4-handshake.md` 8). The target: 348 server and 382 client
messages; the shooter: 349 and 382. VERIFIED live.

- The target was hit twice, 50 ms apart: two `EV_BULLET_HIT_CLIENT_LARGE`
  temp entities (eType 17 + 43), the first with `damageEvent` 1 and
  `damageCount` 56; the second came with the corpse and the obituary,
  and the playerstate went to `pm_type` 7. VERIFIED live. Without
  `scr_player_maxhealth 30` the target lived through nine hits spread
  over 15 s (an earlier run, rounds landing about 2 s apart). VERIFIED
  live.
- The shooter saw the same rounds as `EV_BULLET_HIT` (17 + 41) on its own
  entity numbers, never as hits on a client: `sv_clientSideBullets` 1
  hands the shooter its own impact. VERIFIED live (the event split);
  the reason is INFERRED from the systeminfo key.
- The death frame carried the corpse, eType 2, entity 64 (the body queue
  starts there, as on CoD1), at the target's feet; the dropped rifle, an
  item, eType 3, entity 320, `index` 27, falling to rest in 350 ms; and
  the obituary, a temp entity 17 + 66 with `otherEntityNum` 0 (the
  victim), `attackerEntityNum` 1 (the shooter) and `eventParm` 27. 27 is
  the shooter's `ps.weapon` (the M16 of `assault_mp,0`). VERIFIED live.
  That `eventParm` is the weapon and not a means of death is INFERRED
  from that match.
- The killcam followed: from 3 s after the death the target's snapshots
  replayed the shooter's view (`clientNum` 1, the shooter's temp
  entities again), then the target respawned about 4500 units away with
  `pm_type` 0 and the rifle back. VERIFIED live.
- Every message of both re-encodes (`kill_captures_decode_and_reencode`)
  once the stats mask is kept (section 7). VERIFIED live.
- Entity types seen across every capture: 0 general, 1 player, 2 corpse,
  3 item, 4 missile, 6 script mover, and events 17 + {41, 43, 61, 66}.
  Never seen: 5 sound blend, 7 fx, 8 loop fx, 9 helicopter, 10 plane, 11
  vehicle (the eType numbers are the per-type table's order, section 1).
  VERIFIED live. Nothing in these short war rounds spawned a helicopter
  or a plane; a run where a player earns killstreak rewards, or a gsc
  that spawns them, would show those tables on the wire.

### The stats block

The client sends 0x2000 bytes in seven `stats` packets
(`cod4-handshake.md` 3), the buffer `LiveStorage` keeps at `iw3mp.exe`
0xcc18c90 (`net::stats`):

| bytes | content |
|---|---|
| 0..4 | CRC-32 (`Com_BlockChecksumKey32`, key 0) of bytes 4..0x2000 |
| 4..2004 | byte stats 0..1999, stat `i` at `4 + i` |
| 2004..7996 | int stats 2000..3497, stat `i` at `4 * i - 5996`, little-endian |
| 7996..0x2000 | unused |

- The offsets: `LiveStorage_StatsInit` (0x4fc550) stores byte stat 201
  at 0xcc18d5d (base + 4 + 201) and int stat 3000 at 0xcc1a404 (base +
  4 * 3000 - 5996); its range reset (0x4fb120) indexes bytes off
  0xcc18c94 below 2000 and words off 0xcc17524 + 4 * i below 3498.
  VERIFIED. The checksum word and its range follow
  `LiveStorage_WriteChecksumToBuffer`, INFERRED; the server took the
  probe's block either way.
- A fresh profile. With no `mpdata` the client zeroes the buffer
  (`LiveStorage_NoStatsFound`, 0x579770, which prints `No stats found,
  zeroing out stats buffer` from 0x6f5c10) and calls
  `LiveStorage_StatsInit`. VERIFIED (the call at 0x5797b4). StatsInit
  zeroes the challenge, weapon and perk ranges, stores the five class
  loadouts and sets stats 3000 and 3002 to 9; stat 299 is then the
  `stat_version` dvar's default, 10 (`mov ecx, 0xa` at 0x57a0ea in its
  registration). VERIFIED. `tools/re/cod4_default_stats.py` walks those
  stores out of `iw3mp.exe` into `net::stats::DEFAULT_STATS`, 54 non-zero
  stats. VERIFIED.
- The five custom class slots, ten stats each from 200, 210, .. 240
  (the stores are assault, spec ops, heavy gunner, demolitions, sniper,
  INFERRED): the stock `_class.gsc` (rawfile in
  `common_mp.ff`) reads `200 + 10 * i + 1` the primary, `+ 2` its
  attachment, `+ 3` the secondary, `+ 4` its attachment, `+ 5` .. `+ 7`
  the three specialties, `+ 8` the special grenade, `+ 9` the camo.
  VERIFIED. A fresh profile's `+ 5` values are 191, 184, 176, 186 and
  176. VERIFIED.
- So `_globallogic.gsc`'s integrity check (`getstat(205 + 10 * i)` non-zero
  for i 0..4) passes on a fresh profile. VERIFIED live: both kill probes
  sent `Stats::fresh()` and neither was kicked.
- The probe recomputes the checksum over the filled block
  (`Stats::seal`). Retail's fresh buffer carries the checksum of the
  zeroed buffer until the profile is first written; which one a
  first-time client sends is not settled. INFERRED (the order
  in `LiveStorage_NoStatsFound`).

### The stats file

vcod keeps its own stats file,
`players/profiles/<profile>/[<fs_game>/]vcod_mpdata`, the 0x2000-byte
block stored plain (`play::cod4_profile`): read at each join, sent in
the `stats` packets, written back two seconds after the server's `N`
commands change it and at the disconnect. Retail's `mpdata` is neither
read nor written.

## 11. Still open

- A playerstate without origin and velocity (bit 0) needs the client's
  own prediction history. The server sends bit 0 only for a delta
  snapshot, with `sv_clientArchive` on, when the predicted origin the
  move op carried is within 0.1 units of the playerstate's and its time
  equals `commandTime`; the hint-3 rows then go unsent. The client fills
  origin, velocity, view angles, `bobCycle` and `movementDir` from a
  ring of what it predicted, keyed by the cmd's server time, saved as
  each move goes out (`CL_SavePredictedOriginForServerTime`,
  `CL_GetPredictedOriginForServerTime`, newest at or before the time).
  INFERRED. The reader keeps the base's values there; the probe
  sends an origin of 1e6, so every capture has bit 1. A vcod client needs
  that ring before it reports its real predicted origin.
- Sound blend, fx, loop fx, helicopter, plane and vehicle entities
  (section 10).
- The spectator stayed a free spectator; following a player is untried.
- The ammo, clip and weapon model masks are rebuilt, not recorded
  (section 7).

## 12. The client's watch

`vcod --game cod4 --capture <file>` and `--connect` (`play::cod4`,
`cod4_client`) read these captures and a live server the way a spectator
watches; the replay gate (`play::cod4::tests`) runs both play captures
through it. The clock and lerp it draws with are `cod4-client-view.md`'s.

- The weapon list in configstring 2258 is 1-based: weapon `n` is its
  `n`th name, 0 none. VERIFIED: the kill capture's frag (missile 315)
  carries weapon 40, the list's 40th name `frag_grenade_mp`; the
  shooter's `ps.weapon` and the obituary's `eventParm` 27 are the 27th,
  `m16_gl_mp`. That `assault_mp,0`'s first weapon is the M16 with the
  grenade launcher is INFERRED from that name.
- A dropped weapon item's `index` is its weapon number: item 320's
  `index` 27 is the dead target's `ps.weapon`. VERIFIED.
- A player or corpse names its client in `clientNum`; the body is
  `CS_MODELS` (830) plus that client's `clientState.modelindex`
  (`body_mp_usmc_assault`, `body_mp_arab_regular_assault`). VERIFIED.
  The zone's body xmodels hold their vertices in model space in the bind
  pose (z 0 to 63, the arms out to 21 either side). VERIFIED (mp_crash's
  zone). The head rides `attachModelIndex`; the posed body and head are
  `cod4-player-anims.md`'s.
- mp_crash's script movers (eType 6) are the destructible sedans:
  `solid` 0, `index` a model slot (`vehicle_80s_sedan1_*_destructible_mp`),
  not brush models. VERIFIED.
- The client's own view is not in its snapshots (the view angles go out
  only while the view is locked, section 7): the replay takes its client's
  newest usercmd angles over `delta_angles`. VERIFIED: the shooter's view
  so built points within 5 degrees of the target in 472 of the 534
  frames the target is drawn (`shooter_view_is_its_cmds_aim`).
- The target's snapshots carry the obituary's temp entity twice, at the
  death and again in the killcam. VERIFIED.
- Live: `vcod --net-probe 127.0.0.1:28960 --game cod4` joined the retail
  server (`sv_pure 1`, war, mp_crash, the namespace of `cod4-handshake.md`
  7) with every iwd's pure sum, stayed 35 s (595 snapshots, 20 a second)
  beside a walking probe, and its `disconnect` freed the slot (the server
  printed `EXE_DISCONNECTED`). VERIFIED live. That run also showed
  `EV_PLAY_FX_ON_TAG`, `EV_SOUND_ALIAS` and `EV_LANDING_GRASS`. VERIFIED
  live.
- A live join plays (`play::cod4_input`): a cmd a frame on the newest
  snapshot's time plus its age, packets every 33 ms with the cmds built
  since, the weapon byte the client's selection (the spawn's weapon,
  `weapnext` over the held primaries, the server's `a <n>`), never an
  echo of `ps.weapon`, and the offhand the spawn's or the server's `C
  <n>`. The button bits section 8 did not exercise follow
  `CL_CmdButtons` and `CL_KeyMove`. INFERRED. A spawn or any
  other change of the own playerstate's `delta_angles` moves the
  client's view by the delta's change, which leaves the cmd's angles
  where they were, as retail's unchanged `cl.viewangles` do
  (`CL_FinishMove` writes them alone; `IN_CenterView` is the only
  writer that reads the delta). INFERRED. Taking the
  playerstate's viewangles instead snapped the view toward the stale
  angles section 7's rule leaves on the wire (the last teleport's) each
  time `PM_UpdateViewAngles`' pitch clamp pushed a pitch past 85 into the
  delta: `cod4_live_pitch_held_past_the_limit_stays_at_the_limit` on
  vcod-server ended with the camera at 36 degrees before the fix and
  holds it at 85 after. VERIFIED (the test).
