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
  matches and its echo time equals `commandTime`. VERIFIED (the
  instructions): `MSG_WriteDeltaPlayerstate` (`cod4_lnxded` 0x813f586)
  with no base (`test edx,edx; je` at 0x813f5a0) zeroes a playerstate
  on its stack (`memset` of 0x2f64 bytes at 0x8140cf4..0x8140d0d), takes
  it as the base (0x8140d12) and comes back to the same test at
  0x813f5a8, so a full snapshot leaves origin out too. With the
  context's byte +0xd clear (the live writer `SV_WriteSnapshotToClient`
  clears it at 0x8179c93; the writer at 0x816b6b4 sets it, which then
  writes bit 1 at 0x81409bb) and `sv_clientArchive` on (0x13f181e0,
  copied off the dvar at 0x13f18204 by 0x81789a0), it takes the squared
  distance, on the x87 stack, from the client's +0x63c (the move's three
  floats, stored by `SV_UserMove` at 0x817270a..0x817273e, after the
  cmds and before the state checks) to the playerstate's origin (+0x1c);
  at or under the float 0.01 (0x8209644, so 0.1 units; `fucomip`, `jb`
  at 0x813f601..0x813f60b, so a NaN fails) and with the client's +0x648
  (the move's time, 0x8172752) equal to `commandTime`
  (0x813f60d..0x813f615) it writes bit 0 (0x8140e13..0x8140e1e), else
  bit 1. `sv_clientArchive` is a bool, on by default: `SV_Init` registers
  it with 1 and flags 0 (0x8173944..0x8173968; the name at 0x8231957,
  "Have the clients archive data to save bandwidth on the server" at
  0x8231fb8). The probe echoes an origin of 1e6 at time 0, and every
  snapshot carries them. VERIFIED live. vcod-server makes the same test
  (`origin_echoed`, the f64 sum standing in for the x87 one), and
  `cod4_live_a_predicted_walk_lets_the_server_leave_origin_out` has
  vcod's client walking on it get nearly every snapshot without origin.
  VERIFIED (the test).

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
- The server's read of that header (`cod4_lnxded` 0x8177d62..0x8177e32,
  after `Netchan_Process` at 0x8177d59): the `serverId` byte into the
  client's +0xa0d28 (0x8177d6d), `messageAcknowledge` into +0x20e5c
  (0x8177d7e), and a negative one (`js` at 0x8177d86) prints `Invalid
  reliableAcknowledge message from %s - reliableAcknowledge is %i`
  (0x82325f0, with the old +0x20e54) and drops the message, no field
  changed past +0x20e5c (0x8177ded..0x8177e14). Then `reliableAcknowledge`
  into +0x20e54 (0x8177d95); when +0x20e50 minus it is over 0x7f (`sub`,
  `cmp 0x7f`, `jle` at 0x8177da1..0x8177da8) it prints `Out of range
  reliableAcknowledge message from %s - cl->reliableSequence is %i,
  reliableAcknowledge is %i` (0x823263c), stores +0x20e50 over +0x20e54
  (0x8177dd0..0x8177dd6) and drops the message. Else the ops are read
  (0x8178530 at 0x8177e32). VERIFIED (the instructions and the strings).
  The window test is a signed 32-bit difference, so an acknowledge past
  `reliableSequence` or near `i32::MIN` passes it. vcod-server
  (`engine::conn::incoming`) drops a negative `messageAcknowledge` as
  retail does, and also one of a message never sent (vcod's). A
  `reliableAcknowledge` 128 or more behind takes retail's clamp:
  `reliableSequence` stored over it and the message dropped
  (`Link::reliable_ack_behind`). One past `reliableSequence` passes, as
  retail's signed test lets it, and the next message's command walk
  starts past it (none sent). vcod takes the difference in 64-bit math,
  so an acknowledge near `i32::MIN` counts as 128 behind and is clamped
  where retail's wrapping test lets it in and walks 2^31 commands
  (SEC-001). The clamp commits before the message is read, and it keeps
  the netchan sequence as `Netchan_Process` (0x8177d59) took it, within
  vcod's 32-message window; the address still waits for the read
  (cod2-protocol.md 2.2).
- A usercmd: bit 0 and a long serverTime (or bit 1 and a byte step), then
  a 1-bit "changed" flag XOR the key (section 8 has the rest). The key is
  `checksumFeed ^ messageAcknowledge ^ Com_HashKey(server command, 32)`,
  the command the one `reliableAcknowledge` names: `SV_UserMove`
  (0x8172560) XORs the word at 0x13e77f94, the client's +0x20e5c and the
  hash (0x8197b80, length 32) of its command slot `+0x20e54 & 127`
  (0x81725eb..0x8172633), after refusing a count past 32 (0x81725c3,
  `cmdCount > MAX_PACKET_USERCMDS`) or under 1 (0x81725bd, `cmdCount <
  1`): each prints and returns before a cmd is read, after the delta
  message is set. INFERRED (the fields' meaning off the code).
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
  check and the command, which the probe sends). With all-zero
  stats the stock `_globallogic.gsc` kicks the client on connect: it
  checks `getstat(205 + 10 * i)` for i 0..4 on an online game, and
  `level.onlineGame` is hard-coded true (both in the rawfile of the same
  name in `common_mp.ff`). VERIFIED. The console showed `was kicked for
  cheating` after 38 snapshots. VERIFIED live. The probe sends a
  fresh profile's stats (section 10, "The stats block").

## 7. The writer and the re-encode gate

`delta.rs` writes every field code, entity, client, HUD elem, objective
and playerstate delta, and `parse.rs`'s `Client::write` puts a parsed
server message back together; `clmsg.rs` does the same for the client's
ops. The gate (`tests/net_decode.rs`'s `walk`) re-encodes every message
of every committed capture and compares the compressed block byte for
byte, both directions. Every capture it walks passes:
`handshake-1.7.txt`, `handshake-1.7-cmds.txt`, `snapshots-1.7.txt`,
`play-1.7-fly.txt` (the free spectator), the two of section 8 and the
two of section 10. VERIFIED live.

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
- `MSG_WriteDeltaPlayerstate` (0x813f586, called from 0x816b930)
  considers the `-87` view angles only with `otherFlags` 2 (`testb
  $0x2,0x14`), an `eFlags` 2 flip (the two states' +0xb0 XORed), a
  `viewlocked_entNum` (+0x59c) other than 0x3ff or `pm_type` 5, tested
  inline for the last-sent count (0x813fa4a..0x813fa81) and again for
  the row writes (0x813faba..0x8140c05). VERIFIED (the disassembly). A
  set byte +0xd of its first argument skips the test; that this is the
  archived snapshot cod4-pmove.md names is INFERRED. A considered row
  sets the last-sent count without a compare (0x813fa87), but its write
  goes through the field writer 0x813e20a, which compares the two words
  and writes a 0 bit for an equal one (0x813e268, 0x813e47b). VERIFIED
  (the disassembly). So a teleport sends only the view rows that moved:
  the kcent shooter's `setviewpos` frame (42100) sends its yaw, its pitch
  and roll kept. VERIFIED live
  (`net_decode::a_teleport_sends_only_the_view_rows_that_changed`). The
  compare is against the server's own base, the view the server holds,
  not the one the client last read, so a client's view can stay on a
  stale value: the killcam of cod4-combat.md 15. INFERRED (the gate's
  match). vcod sent every considered row until the kcent gate.
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
allies teammate, its capture not kept: nothing is read off it) and
`spec` start together; `walker`
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
  server's playerstate (0x8130ab0, just before), each angle `(int)` of the
  stored float `(viewangles - delta_angles) * 182.04445f + 0.5f`, masked to
  16 bits (0x8130adc..0x8130b29), as the client writes its own
  (cod4-client-view.md 14.1). VERIFIED (the instructions). `iw3mp.exe`'s writer
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

The walker's temp entity 17 + 61 is the frag's
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
  VERIFIED live. The planes and helicopter are in the later hardpoint
  captures (`play-1.7-hardpoint.txt`, `play-1.7-planefx.txt`; eType 13 in
  cod4-gsc.md 3.2), which the hardpoint gates read; this gate does not
  re-encode them.

### 10.1 The stats block

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

### 10.2 The stats file

vcod keeps its own stats file,
`players/profiles/<profile>/[<fs_game>/]vcod_mpdata`, the 0x2000-byte
block stored plain (`play::cod4_profile`): read at each join under the
`fs_game` the server's `connectResponse` names (cod4-front-end.md 6), sent in
the `stats` packets, written back two seconds after the server's `N`
commands change it and at the disconnect. Retail's `mpdata` is neither
read nor written.

- Retail's `N <index> <value>` (`CG_ServerCommand`'s arm at `iw3mp.exe`
  0x44c42d) reads both words with `atoi` and stores through 0x579c70 into
  the `LiveStorage` buffer at 0xcc18c90; before the stats are fetched
  (0xcc1ac90 clear) it prints `Tried to set stat index %i before we have
  obtained player stats` (0x6f5ca8) and drops it. VERIFIED (the
  disassembly).
- The `stats` packets read that same buffer, 0x4d8 bytes per packet from
  0xcc18c90 (0x46af8d), and a zeroed one while the stats are not
  fetched (0x46af9d). VERIFIED (the disassembly). So a map change's
  `requeststats` sends the block as the game's `N`s left it. vcod's
  session hands its block to the connection each frame
  (`Conn::set_stats`). Gate:
  `cod4_live_map_change_sends_the_stats_the_server_set` (vcod-server's
  `map mp_crash` after a `N 2301 300`: the new level ranks the player 3
  off the 300 XP).
- The main menu reads the file too, and Create a Class writes its picks
  there (cod4-front-end.md 4, 4.1).
- The chain on vcod-server: a ranked match (`sv_pure 1`) runs
  `_rank.gsc`'s `giveRankXP`, whose `setStat`s go out as `N`
  (cod4-gsc.md 7.3). Gate:
  `stat_tests::cod4_ranked_xp_unlocks_create_a_class` (300 XP: stat 2301
  300, rank 3, 260 at 2).

## 11. Still open

- Re-encoding sound blend, fx, loop fx, helicopter, plane and vehicle
  entities (section 10).
- A spectator following a player in these captures: the only follow on
  the wire is the class-less walker's forced follow (cod4-gsc.md 7).
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
- A live join plays (`play::cod4_input`): a cmd a frame stamped
  `cl.serverTime` (section 13), packets every 33 ms with the cmds built
  since and the last packet's again (section 14), the weapon byte the client's selection (the spawn's weapon,
  `weapnext` over the held primaries, the server's `a <n>`), never an
  echo of `ps.weapon`, and the offhand the spawn's or the server's `C
  <n>`. The button bits section 8 did not exercise follow
  `CL_CmdButtons` and `CL_KeyMove`. INFERRED. A spawn or any
  other change of the own playerstate's `delta_angles` moves the
  client's view by the delta's change, which leaves the cmd's angles
  where they were, as retail's unchanged `cl.viewangles` do
  (`CL_FinishMove` writes them alone; `IN_CenterView` is the only
  writer that reads the delta). INFERRED.
- Trap: taking the playerstate's viewangles instead snaps the view toward
  the stale angles section 7's rule leaves on the wire (the last
  teleport's) each time `PM_UpdateViewAngles`' pitch clamp pushes a pitch
  past 85 into the delta. `cod4_live_pitch_held_past_the_limit_stays_at_the_limit`
  on vcod-server holds the camera at 85 (36 with the playerstate's
  angles). VERIFIED (the test).

## 13. The predicted origin

A playerstate without origin and velocity (bit 0) takes them off
the client's own prediction history. The server sends bit 0, for a
delta or a full snapshot, with `sv_clientArchive` on, when the
predicted origin the move op carried is within 0.1 units of the
playerstate's and its time equals `commandTime` (section 3). VERIFIED
(the instructions).

- **The order.** Active, the cgame's frame (0x452a3b..0x452a57) runs
  `CL_CreateNewCommands` (0x463e00) and then the prediction (0x447980),
  whose tail is `CL_SetUserCmdOrigin` (0x447b36) and, when
  `CL_ReadyToSendPacket` (0x463e60) allows, `CL_WritePacket`
  (0x463f50 at 0x447b47); before state 9 `CL_Frame` makes and sends
  them itself after `CL_SetCGameTime` (0x46c9f6..0x46ca22). So each
  packet carries the state predicted through its newest cmd. A cmd is
  stamped `cl.serverTime`, at most 5000 past the newest snapshot
  (0x463a73..0x463aa5: `cmp 0x1388`, `jle`, else snapshot + 0x1388),
  and `CL_CreateNewCommands` makes one each call with no test of the
  time (0x463e15..0x463e53). VERIFIED (the instructions); the
  functions' names are INFERRED.
- **The send.** `CL_SetUserCmdOrigin` (`iw3mp.exe` 0x45ad30) stores
  `cl.serverTime` (0xc628d0) at 0xc84fd4 and the cgame's predicted
  origin, velocity, view angles, `bobCycle` and `movementDir` beside it
  (0xc84fa4..0xc84fcc); the cgame calls it off its predicted
  playerstate (0x447b36, origin 0x794490, velocity 0x79449c, view
  0x79457c) just before `CL_WritePacket` (0x447b47). The move op
  writes the origin and that time (0x4641cd..0x464231), and the packet
  adds the row to `cl.clientArchive` unless the newest holds the same
  time (0x46423e..0x46428c; cod4-demos.md 4). VERIFIED (the
  instructions).
- **The read.** `MSG_ReadDeltaPlayerstate` (0x507199..0x5072fd) reads
  the first bit (0x50719b); set, each `changeHints` 3 row reads against
  0 when the caller asks (0x5071e0..0x5071f7). Clear, it calls
  0x472d00 (`CL_GetPredictedOriginForServerTime`, 0x507280) with the
  new `commandTime` (+0x0) and the playerstate's origin (+0x1c),
  velocity (+0x28), view angles (+0x108), `bobCycle` (+0x8) and
  `movementDir` (+0xac): walking back from the ring's write index
  (`cl` +0x696cc) less 1 over at most 256 rows, the first whose time
  is at or before `commandTime` (`jle` at 0x472d3f), printing
  `Couldn't find exact match for servertime %i, using servertime %i`
  (0x6d25e0) when it is not equal, and copying its fields out. None
  found prints `Unable find origin sent, delta not going to work`
  (0x6e2bd8) and copies those five fields off the base (0x50728c..
  0x5072fa). VERIFIED (the instructions and the strings).
- vcod: the ring is `Client::archive` (`ArchiveRing::at_or_before`,
  `fill_unsent`), and `Client::parse` fills a bit-0 playerstate off it
  (`a_playerstate_sent_without_origin_takes_the_archive_row`); a
  demo's archive records write it, so a retail demo whose recorder
  took bit-0 snapshots plays his predicted origins, as 1.7 plays them
  (INFERRED: no such demo is here). A live session stamps its cmds
  `cl.serverTime` with the cap (`Cod4Session::cmd_time`), predicts to
  that time, and its scene sends the due packet after the prediction
  with the predicted row (`Cod4Session::archive_row`, `send_packet`),
  which `Conn::send` writes into the move and adds to the ring. A
  packet no scene sent goes out at the next step with the far origin
  at time 0. vcod's client makes no cmd on a millisecond it already
  stamped, where 1.7 makes one the server then skips; either way the
  server thinks the same cmds. vcod-server leaves origin out as
  0x813f586 does (section 3).
- vcod's in-process clients: a row with no prediction behind it is
  the newest snapshot's origin, velocity, `bobCycle` and `movementDir`,
  its view the newest cmd's angles over `delta_angles`, at the newest
  cmd's time (`Archive::unpredicted`, the live client's own when it
  predicts nothing). `bot_match`'s CoD4 observer sends and files it, so
  its playerstates go without origin while it stands. VERIFIED (release,
  `GAME=cod4 NETDIGEST=600 TICKS=6000`, mp_crash `war`, master
  37fc04040 then this, 2026-11-23): the matches play the same (seed 1
  49 kills, 181 hits; seed 2 39 kills, 163 hits, both runs) and only
  the observer's datagrams change: `net` seed 1 8d980153d0bde743 →
  6a136093f2e819fb, seed 2 f3eb2a13b4fa8fb2 → a64d95f215e39b91; over
  seed 1's first 600 ticks its 518 datagrams fall from 49885 bytes to
  41912, the first at tick 95 (68 → 52). The test harness's `Probe`
  sends it only with `echo_origin`
  (`a_still_client_echoing_its_origin_gets_playerstates_without_it`):
  its A/B twins replay captures whose client sent the far origin, and
  `cod4_probe` keeps sending that, since a playerstate without bit 0
  leaves a capture no way to read its velocity and view.
- A spawn's think puts `commandTime` at the level's time, which runs
  ahead of `cl.serverTime`, so the cmds of the first 100 to 150 ms
  after a spawn fall under it and the server skips them, a press with
  them. INFERRED (Q3's `ClientSpawn` shape; vcod-server's spawn,
  measured live in `cod4_live_every_server_round_fires_once`).

## 14. The packet's cmds: `cl_packetdup`

Which cmds a 1.7 client packet carries, how often one goes, and how the
server reads the repeats.

- **The dvars.** `CL_Init` registers `cl_maxpackets` (0x6d14f8) as an
  int, default 30 (`mov ebx,0x1e` at 0x46df21), range 15 (`lea
  ebp,[ebx-0xf]`, 0x46df30) to 100 (0x46df3a), described
  `Maximum number of packets sent per frame` (0x6d14cc), its pointer
  stored at 0xc5f8e0 (0x46dfa5); then `cl_packetdup` (0x6d1524), default
  1 (0x46dfaa), range 0 (`xor ebp,ebp`, 0x46dfaf) to 5 (0x46dfb8),
  `Enable packet duplication` (0x6d1508), pointer at 0xc5f8d8
  (0x46e02c). VERIFIED (the instructions and the strings). Each passes
  1 and 5 beside the name (0x46df89, 0x46e006); that 1 is the archive
  flag is INFERRED.
- **The count.** `CL_WritePacket` (0x463f50) takes the outgoing
  sequence (0x934e9c) less `cl_packetdup` (+0xc of 0xc5f8d8) less 1,
  masks it with 0x1f, and subtracts that packet row's first word (rows
  of 12 bytes from 0xcc9000) from `cl.cmdNumber` (0xcc5ff8):
  0x46407f..0x46409b. Over 32 it prints `MAX_PACKET_USERCMDS`
  (0x6cf6b4) and sends 32 (0x4640a2..0x4640b3); under 1 it writes no
  move and goes to `clc_EOF` (0x4640bd..0x4640c0, to 0x464298).
  VERIFIED (the instructions).
- **The op and the count byte.** Op 0 when the word at 0x934e7c is 0,
  the bool dvar at 0x956cfc is clear, 0xc5f938 is set and 0x914e1c
  equals 0xc5f944, else op 1 (0x4640e1..0x464117); then the count as a
  byte (0x464131). VERIFIED (the instructions). That those are
  `clc.demowaiting`, `cl_nodelta`, `cl.snap.valid` and
  `serverMessageSequence == cl.snap.messageNum` is INFERRED (Q3's
  shape).
- **The cmds.** The key is `Com_HashKey` (0x564bf0) of the server
  command slot `0x914e20 & 0x7f` (1024-byte slots from 0x914e28) XOR
  0x8f4e10 XOR 0x914e1c (0x464145..0x46416c, section 5). The loop
  writes cmd `(cmdNumber - count + i + 1) & 0x7f` (32-byte rows from
  0xcc4ff8) for i from 0 through 0x505c60, each off the one it wrote
  before (0x464180..0x4641b8), the first off the base 0x505b70 builds
  from the playerstate at 0xc5f954 (0x463fa6..0x463fb9). VERIFIED (the
  instructions). So the packet carries the newest `count` cmds, oldest
  first: the repeats lead.
- **The packet row.** After the compress, row `sequence & 0x1f` takes
  the realtime (0x956e98) at +8, the last written cmd's `serverTime` at
  +4 and `cl.cmdNumber` at +0 (0x4642ec..0x46431c), and 0x8f4ce8 the
  realtime (0x46432e), before `Netchan_Transmit` (0x508100 at
  0x46439a). VERIFIED (the instructions). So with `cl_packetdup` N a
  packet carries every cmd made since the packet N + 1 sequences back
  went out, at most 32, and a packet without a move still takes a row;
  at the default 1 it repeats the previous packet's cmds. VERIFIED (the
  instructions above).
- **A gamestate restarts it.** The gamestate op (0x473ce0, called at
  0x474816 from the message parse 0x474650) clears 0x1b1bdc bytes from
  0xc5f930 (`memset` 0x67c4a0 at 0x473d0e..0x473d1a) when its first
  argument is under 1 (`cmp ebp,1`, `jge` at 0x473cff..0x473d0c), and
  `CL_PacketEvent` passes 0 (0x46c532). That block holds `cl.cmdNumber`
  (+0x666c8), the cmds (+0x656c8) and the packet rows (+0x696d0).
  VERIFIED (the instructions); that 0xc5f930 is `cl` is INFERRED (the
  archive index `cl` +0x696cc is 0xcc8ffc, section 13).
- **`cl_maxpackets`.** `CL_ReadyToSendPacket` (0x463e60) sends nothing
  while 0x934e74 is set or the state (0xc5f900) is 1 or 2; with the byte
  at 0xc5b088 set, nothing under 50 ms since 0x8f4ce8
  (0x463e86..0x463ea3); outside states 8 and 9 without it, nothing
  under 1000 ms (0x463ea9..0x463ec0). Then it sends every frame when
  the server's address type (0x934eac) is 2 or 0x578180 returns
  non-zero for that address (0x463ec2..0x463f0a); else once the
  realtime is at least 1000 / `cl_maxpackets` past the previous
  sequence's row (+8 at 0xcc9008, `idiv` at 0x463f2d, `setge`).
  VERIFIED (the instructions). That these are a demo playing, a
  download, loopback and `Sys_IsLANAddress` is INFERRED. So
  `cl_maxpackets` paces only a server off the LAN, and the repeats
  count packets, not time.
- **The server.** `cod4_lnxded`'s `SV_UserMove` (0x8172560) loop skips
  a cmd later than the packet's last (`jg` at 0x8172814) or at or
  before the client's +0x20e68 (`jle` at 0x817281f) and runs
  0x816deca (`SV_ClientThink`) on the rest. VERIFIED (the
  instructions). The CoD4x 21.3 release's `SV_UserMove`
  (`cod4x18_dedrun` 0x112820, named by its symbols) skips the same two
  at 0x112a42..0x112a50 before `SV_ClientThink` (0x112a5e). VERIFIED
  (the instructions). So each repeated cmd runs once.
- vcod: `Conn::send` keeps the ring (`CmdRing`: the newest 128 cmds,
  the count made, a row per sequence) and writes each move off it on
  both dialects; a gamestate clears it. `Cod4Session::set_packet_dup`
  takes the console's `cl_packetdup` (default 1, clamped to 0..5).
  vcod-server's `user_move` skips as `SV_UserMove` does. Tests:
  `each_packet_resends_the_cmds_of_the_last_packet_dup_packets` (the
  count, the order, the cap, both headers) and
  `cod4_live_airstrike_confirm_survives_a_lost_packet` (the first
  packet carrying the location confirm dropped at the server, the
  selection still ends; at `cl_packetdup` 0 the map stays up). VERIFIED
  (the tests).
- vcod departs: the first cmd still goes on the long branch with every
  field (`CmdWire::full`), not off the playerstate base, so its bits
  differ from 1.7's wherever the base matches a field; the server reads
  the same cmd. INFERRED (the code). The send rate and the move op follow
  1.7 since 14.1 and 14.2.

### 14.1 The send rate

- **The gate.** `CL_ReadyToSendPacket` (0x463e60) returns 0 while
  0x934e74 is set or the state (0xc5f900) is 1 or 2
  (0x463e60..0x463e80); with the byte at 0xc5b088 set, under 50 ms past
  0x8f4ce8 (0x463e86..0x463ea3); in a state other than 8 or 9 without
  that byte, under 1000 ms past 0x8f4ce8 (0x463ea9..0x463ec0). Past
  those it returns 1 when the server's address type (0x934eac) is 2
  (0x463ec2..0x463ed4) or 0x578180 returns non-zero for the 20-byte
  address at 0x934eac (0x463ed5..0x463f0a); else the realtime
  (0x956e98) less the `+8` word of packet row `(0x934e9c - 1) & 0x1f`
  (12-byte rows from 0xcc9000) at least `1000 / cl_maxpackets` (`idiv`
  of 0x3e8 by +0xc of 0xc5f8e0, `setge`, 0x463f0c..0x463f37). VERIFIED
  (the instructions). That these are `clc.demoplaying`,
  `clc.downloadName`, `clc.lastPacketSentTime`, `CA_PRIMED` and
  `CA_ACTIVE`, `NA_LOOPBACK` and `Sys_IsLANAddress` is INFERRED.
- **No other gate.** The bool dvar at 0x8f4cd8 only prints the count as
  `(%i)` (0x6cf6cc) before the op (0x4640c6..0x4640de). VERIFIED (the
  instructions). That it is `cl_showSend` is INFERRED.
- **Who asks.** The frame's tail (0x46c9a0) asks and calls
  `CL_WritePacket` when the state at the frame's start is 5 to 8 (`add
  edi,-5; cmp edi,3; ja` at 0x46ca11..0x46ca17, the calls at
  0x46ca19..0x46ca22); in state 9 the function at 0x474da0 asks after
  0x452780 returns 0 (0x474e07..0x474e10); 0x447b3e..0x447b47 asks too.
  `CL_WritePacket` itself sends nothing while 0x934e74 is set or in
  state 1, 2 or 6 (0x463f6a..0x463fa0). Once the cgame is up, 0x46a8d0
  calls `CL_WritePacket` three times with no gate (0x46aae5..0x46aaef).
  VERIFIED (the instructions).
- **The LAN.** 0x578180 passes the address to 0x578120, which answers 1
  for type 0, 2 or 5, and for type 4 when the first octet is 10 or 127,
  169 with 254 next, 172 with the second's high nibble 1, or 192 with
  168 next (0x578120..0x57817a); else 0x578180 compares the first three
  octets with each of the 0xcc17a9c 4-byte entries at 0xcc17aa0
  (0x5781bb..0x5781f2). VERIFIED (the instructions). 0x578850 fills that
  table: WSOCK32 ordinals 57 and 52, `Hostname: %s` (0x6f58c8), `Alias:
  %s` (0x6f58d8), then for an address type 2 list at most 16 entries
  (`cmp eax,0x10` at 0x5788f0), each printed `IP: %i.%i.%i.%i`
  (0x6f55f4). VERIFIED (the instructions and the strings). That the
  ordinals are `gethostname` and `gethostbyname` is INFERRED (Winsock's
  numbering). vcod-server already ran the same test for cod2_lnxded
  0x80d5638 and cod4_lnxded 0x81d59c8 (`engine::link::Lan::Cod2`), which
  now calls the shared `is_lan_cod2`. INFERRED (the code).
- **The clock it reads.** 0x8f4ce8 takes the realtime with each packet
  `CL_WritePacket` sends (0x46432e) and each stats packet (0x46b013;
  stats go 100 ms apart, `cmp edx,0x64` at 0x46aeed), and -9999
  (0xffffd8f1) on any `statResponse` (the name 0x6d0c1c compared at
  0x46b9b5, the store at 0x46ba43; a mask of 0 sets state 5 first at
  0x46ba21), on `requeststats` (0x6d0d48 at 0x46bf9a; state 6 at
  0x46bfcd, the store at 0x46c007), after state 6 at 0x46b98a (0x46b9a1)
  and at 0x4695c5. `loadingnewmap` (0x6d0d28 at 0x46bea6) sets state 5
  (0x46bf74). A gamestate clears the packet rows (section 14). VERIFIED
  (the instructions).
- So in the world a LAN or loopback server gets a packet every frame and
  one off the LAN at most `cl_maxpackets` a second (30 at the default:
  33 ms); before the gamestate, and from `loadingnewmap` to the next
  one, a packet goes a second after the last, the first at once after
  the stats; while downloading 50 ms after the last and the LAN test
  besides. VERIFIED (the instructions above).
- vcod: `Conn::ready_to_send` (`SendClock`): in the world is a gamestate
  with no `loadingnewmap` since; `requeststats` and `statResponse` clear
  `last_sent`; the rows' times sit in `CmdRing`, which a gamestate
  clears; the LAN test is `vcod_common::net::lan::is_lan_cod2` on the
  server's address at the join, against the host's interfaces rather
  than its hostname's addresses (`lan.rs`). The live frame
  (`play::cod4::cmd`), the gamestate wait and the download loop
  (`play::cod4::join`) send only when it says so, on 1.7 and CoD4x 21;
  `cl_maxpackets` comes from the console each frame, clamped to 15..100.
  Tests: `a_lan_server_gets_every_frame_and_one_off_it_cl_maxpackets`,
  `a_conn_paces_its_packets_by_the_server_and_its_state`,
  `cod2_lan_is_the_private_blocks_and_the_hosts_slash_24`. VERIFIED (the
  tests).
- vcod departs: no three packets once the cgame is up; a map change's
  stats go in one burst and moves still go while they do (state 6); the
  join's download loop paces at the default `cl_maxpackets`; vcod's own
  frame asks, not the cgame's draw. CoD4x 21's client gate is taken as
  1.7's (its client binary was not read). INFERRED (the code).

### 14.2 The move op

- **The test.** Op 0 (`clc_move`) when 0x934e7c is 0, the bool dvar at
  0x956cfc is clear, 0xc5f938 is non-zero and 0x914e1c equals 0xc5f944;
  else op 1 (0x4640e1..0x464117). VERIFIED (the instructions).
- **What it compares.** `CL_PacketEvent` stores each sequenced message's
  sequence in 0x914e1c (0x46c4f1) and puts the old one back for an
  overflowed message (0x46c550). The snapshot parse (0x473710) clears
  the new snapshot at 0xd90bcf0 (0x473723), stores 0x914e1c as its
  `+0xc` (0x47374c..0x473757), clears 0x934e7c and sets the new
  snapshot's first word for a snapshot with no base
  (0x4737d1..0x4737e5), and copies the 0x2f94 bytes over 0xc5f938 only
  when that first word is set (`je` at 0x47399d, the copy at
  0x4739d9..0x4739f2). VERIFIED (the instructions). That these are
  `clc.serverMessageSequence`, `cl.snap.valid`, `cl.snap.messageNum`,
  `clc.demowaiting` and `cl_nodelta` is INFERRED (Q3's shape).
- So the delta op goes only when the newest message read carried a valid
  snapshot; after a message without one (a gamestate, which also clears
  `cl`) or with an invalid one, the next packet asks for a full frame.
  VERIFIED (the instructions above).
- vcod: `Conn::send` sends op 0 when the newest snapshot's `message_num`
  is the newest message read (`ack`) and `no_delta` is clear, on both
  headers. Test:
  `the_delta_move_needs_the_snapshot_in_the_newest_message`. VERIFIED
  (the test).
- **`cl_nodelta`.** 0x4643f0 (called from `CL_Init` at 0x46d501)
  registers `cl_nodelta` (0x6cf9f0) through 0x56c350 as type 0 with flags
  0, the default byte 0, limits 0 and 0, described `The server does not
  send snapshot deltas` (0x6cf9c4), and stores it at 0x956cfc
  (0x465b95..0x465bd0). VERIFIED (the instructions and the strings). The
  test above reads its `+0xc` as a byte (0x4640f0). VERIFIED (the
  instructions). That type 0 is a bool is INFERRED (the byte read;
  `cl_stanceHoldTime` beside it passes 5 with the int 300).
- vcod: the console registers `cl_nodelta` "0" with no flags on a CoD4
  install (`COD4_OPTION_DVARS`) and `Cod4Session::set_packet_rates`
  hands it to `Conn::set_nodelta` each frame; set, every move is
  `clc_moveNoDelta`. Test:
  `the_delta_move_needs_the_snapshot_in_the_newest_message`. VERIFIED (the
  test).

### 14.3 A snapshot off a base the client no longer holds

- **The refusals.** For a base above 0, `CL_ParseSnapshot` (0x473710)
  stops the message when ring slot `base & 0x1f` (0x2f94-byte rows from
  0xcc9180) is not valid (`Delta from invalid frame (not supposed to
  happen!).` 0x6d2710, 0x473842), holds another message (`Delta frame
  too old.` 0x6d2748, 0x47386a), or when 0xc84f58 less the slot's +0x2f88
  is over 0x780 (`Delta parseEntitiesNum too old.` 0x6d2760, 0x47389f) or
  0xc84f5c less its +0x2f8c is (`Delta parseClientsNum too old.`
  0x6d2784, 0x4738c1). Each returns before the playerstate, with the
  message's +0x1c over its +0x14 and its first word set (cod4-demos.md 4).
  VERIFIED (the instructions and the strings).
- **The counters.** 0x472e00 stores `cl` +0x25628 (0xc84f58) in the new
  snapshot's +0x2f88 and adds 1 to both for each entity it puts in the
  snapshot (0x472e0c..0x472e12, 0x472fb8..0x472fbe), the ring masked with
  0x7ff in 0xf4-byte rows (0x472e53); 0x4732c0 does the same for the
  clients with +0x2562c (0xc84f5c) and +0x2f8c, 0x7ff in 0x64-byte rows
  (0x4732d5..0x4732db, 0x47343b, 0x473319). VERIFIED (the instructions).
  So 0x780 is the 2048-row rings less 128. A gamestate clears both with
  `cl` (section 14).
- **What follows.** `CL_PacketEvent` then rolls `serverMessageSequence`
  and `reliableAcknowledge` back (cod4-handshake.md 10, "An illegible
  message rolls back"). `cl.snap` is not touched: the copy over 0xc5f938
  comes after the refusals (0x4739d9..0x4739f2). VERIFIED (the
  instructions). So the next move acknowledges the message before, and
  goes as `clc_move` when that one carried `cl.snap` (14.2); nothing sets
  `demowaiting`. VERIFIED (the instructions above).
- vcod: `Client::parse_snapshot` fails with `Illegible` for a base its
  ring lacks or whose entities or clients sit more than 0x780 behind its
  counts (`RingSlot`, which a gamestate clears), and `Conn::poll` rolls
  both acknowledges back; the newest snapshot stays the last one read.
  Test: `a_base_past_the_parse_rings_reach_is_illegible`. VERIFIED (the
  test). vcod counts a snapshot's entities and clients once it is read
  whole; retail counts each as it goes, so a message that overflows
  mid-list moves retail's counts and not vcod's. INFERRED (the code).
