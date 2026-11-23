# CoD4x 21.3: what its server speaks to a client

What a CoD4x dedicated server sends a 1.7 client and a protocol 21 client,
measured on loopback against the project's release binary, and what vcod's
client would need to play on one. `cod4-handshake.md` 13 holds the first
pass (October 2026); this doc extends it with the release binary and goes
past the challenge.

## Sources

- The binary: `cod4x18_dedrun` from the CoD4x server project's GitHub
  release 21.3 (published 2026-11-14), 4764524 bytes, md5
  `543d59bdc5109556460d4562a7481681`, sha256
  `e23593123e84631a31b0324012e806feb49c7fc1e161c3086ac80545062ed02f`.
  ELF i386 PIE, not stripped (symbol names are the binary's own). Banner
  `CoD4 X - linux-i386-custom build 1231 Nov 14 2026`; its `getstatus`
  names revision `63f3e6ac9abf15528a002249e66e9f9d0505f08c`. VERIFIED.
- Addresses are the ELF's link-time virtual addresses (base 0). Code holds
  absolute data addresses (`R_386_RELATIVE` text relocations); only libc
  calls go through `R_386_PC32`, which `tools/re/annotate_func.py`
  resolves. VERIFIED.
- Evidence rule for this doc: the release binary, its disassembly and
  loopback captures of clients against it. No CoD4x source code in any form
  was read. The project's user-facing README (features, setup) was read.
- Captures: `tools/cod4x_capture.sh` (section 16), each line
  `<ms> <c2s|s2c|udp|tcp> <peer> <hex>` (`tools/lo_capture.py`). The
  captures stay under the gitignored `tmp/cod4x/`; section 16 says how to
  remake each. A claim marked "VERIFIED live" names the capture.

## 1. Running the release

- Data: the 1.7 install's `main/*.iwd` and `zone/` beside the binary, as
  for `cod4_lnxded` (`cod4-handshake.md` 1). It needs no other library
  than 32-bit `libc`, `libm` and `libstdc++` (`readelf -d`). VERIFIED.
- At boot it tries HTTPS for its `globalconfig.cfg` URL (0x2a7b6c) on
  `raw.githubusercontent.com` (the console logs `mbedtls_ssl_handshake
  returned NET - Connection was reset by peer`), prints its auto-update
  banner (`Current version: 1.8`, `Current subversion: 21.3`, `Current
  build: 1231`) and sends `heartbeat COD-4\n` to
  `cod4master.activision.com`:20810. VERIFIED live (`cap-noauth1`'s
  console and capture; the namespace's hosts file maps every name to
  127.0.0.2).
- It looks for `steam_api.so` (0x2bb092) and for a zone
  `cod4x_patchv2` (0x2c8b9c, loaded by `R_Init` 0x12f4de beside
  `common_mp` unless the mod has its own `mod.ff`). The release ships
  neither; the server logs both missing and runs on. VERIFIED live.
- `sv_pure` defaults to 0 (`getinfo`'s `pure\0`), where 1.7's is 1.
  VERIFIED live.
- A server whose update host or Steam library is reachable may behave
  differently from this one (sections 5 and 15). INFERRED.

## 2. getinfo, getstatus, getchallenge

The answers, each VERIFIED live (`cap-oob-noauth1`, `run-oob-auth`):

- `getinfo xxx`: `infoResponse` with `voice mod hw od hc ki ff pswrd
  shortversion build pure gametype sv_maxclients g_humanplayers clients
  mapname hostname protocol challenge`, in that order. `protocol` is `6`,
  `shortversion` is `x21`, `build` is `1231`. 1.7 sends `kc` where CoD4x
  sends `ki`, and no `shortversion` (`cod4-handshake.md` 2).
- `getstatus xxx`: serverinfo with `version\CoD4 X - linux-i386-custom
  build 1231 Nov 14 2026`, `shortversion\-`, `build`, `branch\master`,
  `revision`, `_CoD4 X Site\http://cod4x.ovh`, `protocol\21`,
  `g_mapStartTime` and `uptime`, then the players.
- `getchallenge 0 "<hex>"`: `challengeResponse <c> <n> 0 xproto 18`. `c`
  is `NET_CookieHash` of the sender's address, so a second request from
  the same address gets the same value; `n` is the request's first
  argument through `strtol`. Format `challengeResponse %d %d 0 xproto 18`
  at 0x10e700, in `SV_GetChallenge` 0x10e6b0. VERIFIED (the instructions
  at 0x10e6dc..0x10e708) and live.
- The answers are the same with `sv_noauth` 0 and 1 (`cap-oob-noauth1`
  against `run-oob-auth`, all keys equal but the start time). Nothing in
  them says whether the server authenticates clients. VERIFIED live.

## 3. connect: what each protocol number gets

`SV_DirectConnect` (0x111580) checks the challenge cookie (0x111606,
`error\nNo or bad challenge for address.\n`), reads `protocol` (0x11164b)
and compares it with the dvar `protocol` (registered at 0x11dff0 by
`Cvar_RegisterInt`, pointer 0xe7e21c0; its value off
`Sys_GetCommonVersion`, live 21). VERIFIED (the instructions at
0x1117bd..0x1117e1, 0x111dd0..0x111dec, 0x1120bb..0x1120c8).

| protocol | Path | Reply |
|---|---|---|
| 21 | the game | `connectResponse ` (with `fs_game`), `Going from CS_FREE to CS_CONNECTED` |
| 7 or less | "Have to fix up old client" (0x2c1484): a zombie relayed to the update host (section 4) | none, then after 18 s the update-backend error |
| 8 | refused | `error\nThis server requires protocol version: 21\nPlease install the unofficial CoD4X-update ...` (0x2c14b8) |
| 9..20 | refused | `... To update to protocol version 21 please look in CoD4X serverlist ...` (0x2c15e0) |
| 22 and up | refused | `... Please restart CoD4 ... {OOBErrorParser protocolmismatch CoD4X- 21}` (0x2c1538) |

The rows for 6, 7, 8, 20 and 21 are VERIFIED live (`run-oob-auth`); the
22 row is VERIFIED (the instructions) only.

- Retail `cod4_lnxded` answers a protocol 21 connect with
  `error\nEXE_SERVER_IS_DIFFERENT_VER\x151.7\n`. VERIFIED live
  (`cap-retail`).
- Password, bans and the full-server queue (`Server is full. %i players
  wait before you.`) answer protocol 21 and old clients alike. INFERRED
  (the strings in `SV_DirectConnect`).

## 4. A 1.7 client on a CoD4x server

- A protocol 6 connect makes a zombie slot (`Going from CS_FREE to
  CS_ZOMBIE`, 0x111d22) and calls `SV_ConnectWithUpdateProxy` (0x11b7f0),
  which resolves `sv_updatebackendname` (default `cod4update.cod4x.ovh`,
  registered at 0x11e833) and sends it `updgetchallenge <n> noguid`, then
  `updconnect "<info>"` with the keys `challenge`, `rtnchallenge`,
  `clchallenge`, `name`, `clremote`, `qport`, `protocol`. VERIFIED (the
  strings and the calls at 0x11b915..0x11bb02). One `updgetchallenge`
  went to 127.0.0.2:27953 for each connect the client resent, 3 s apart.
  VERIFIED live (`cap-legacy`).
- From then on the zombie's packets go to the update host prefixed with a
  long (`SV_PassToUpdateProxy` 0x11b740) and the host's come back through
  `SV_ReceiveFromUpdateProxy` (0x11b670, a packet whose first long is -2,
  0x121eb7). VERIFIED (the instructions). What the host sends the 1.7
  client (the CoD4x client update) was not observed: no host answered.
- With no answer for 18 s (`cmp ecx,0x4650` at 0x111d74) the next connect
  gets `error\nCan not connect to server because the update backend is
  unavailable\nTo join this server you have to install the required
  update manually.\nPlease visit www.cod4x.ovh/clupdate` and the slot is
  freed. VERIFIED live (`cap-legacy`: the error at 18.02 s).
- So a retail 1.7 client does not play on a CoD4x server as 1.7: it is
  handed to the update host or refused. INFERRED (the bullets above).
- vcod's join (`vcod_cod4::net::conn`, the client's own, run as the
  `cod4_probe` example) stops at the challenge: `This server runs CoD4x
  (xproto 18) ...`. VERIFIED live (`cap-vcod`).

## 5. Client authentication

Scope: this doc records where the server enforces authentication, enough
to detect it. It does not describe or reimplement the exchange.

- `SV_InitSApi` (0x100184) registers `sv_noauth` (bool, default 0, flags
  0x10, `Start the server without client authentication. Unofficial
  clients will be able to join.`). Set, it prints `Running without client
  authentication` and loads nothing; clear, it loads `steam_api.so` and
  takes its `Init` entry. VERIFIED (the instructions at
  0x10019b..0x1003ba). Flag 0x10 is `DVAR_INIT` (command line only), not
  serverinfo. INFERRED.
- The gamestate waits on `SV_ConnectSApi` (0x10049f): with no library
  entry it returns 1, otherwise the library's answer for the slot.
  `SV_SendClientGameState` sends nothing past that call while it returns
  0. VERIFIED (the instructions at 0x10049f..0x1004cd and
  0x11091f..0x110937). So the enforcement lives in a library the release
  does not ship, and an unauthenticated client on such a server would see
  its stats request (7.1) and then no gamestate. INFERRED.
- The side channel (section 7) carries the library's traffic (`cmd` 8,
  0x35448) and an anti-cheat module's (`cmd` 12, `SV_PassNETMessageXAC`)
  and screenshots (`cmd` 0x866). VERIFIED (the dispatch in
  `SV_ExecuteReliableMessage` 0x110fc0). Out of scope.
- This run had no `steam_api.so`, so `sv_noauth` 0 and 1 behaved the same
  past the console line. VERIFIED live (`cap-noauth0`, `cap-noauth1`).

## 6. The protocol 21 join

`cap-noauth1`, a `cod4x_probe` client from `getchallenge` to its spawn.
The rows, each VERIFIED live:

| ms | Dir | What |
|---|---|---|
| 4002 | both | `getchallenge`, `challengeResponse`, `connect` (protocol 21), `connectResponse ` |
| 4011 | s2c | netchan message 1: the two `v` server commands 1.7 also sends |
| 4022 | c2s | side channel: a bare acknowledge |
| 4052 | c2s | netchan message 1: the 16-byte header (section 8) and `clc_EOF` |
| 4061 | s2c | side channel fragment 0: the stats request (7.1) |
| 4322 | c2s | side channel fragments 0..6: the stats, mode 2 |
| 4711..7011 | s2c | side channel fragments: the gamestate, 52112 bytes in 44 fragments of 1200, about one per 50 ms |
| 7062 | s2c | the first snapshot message |

- The client is in the world after its first move with the current
  `sv_serverid`, as on 1.7 (`cod4-netfields.md` 5): the team and class
  answers (`mr`) spawned it and `say` came back. VERIFIED live.
- Past the gamestate every message of the 20 s capture (461) went through
  vcod's 1.7 parser to its last byte with the change of section 10.
  VERIFIED live.

## 7. The reliable side channel

Protocol 21 adds a second sequenced stream beside the netchan, on the same
socket, that carries the stats, the gamestate, downloads and the
authentication and anti-cheat traffic.

- Packet: `long -16`, `short qport`, `long sequence` (-1 for a bare
  acknowledge), `long acknowledge` (the next sequence the sender expects),
  `byte 0`, a selective-acknowledge list (`byte n`, at most 3, then `n`
  pairs of shorts), `short window`, `short length`, then the data. Written
  by `ReliableMessagesTransmitNextFragment` (0xd56b4), read by
  `ReliableMessagesReceiveNextFragment` (0xd5918) after `SV_PacketEvent`
  routes a -16 (0x121ee2). VERIFIED (the instructions) and live: the bare
  acknowledge is the 20 bytes `f0ffffff a40f ffffffff 00000000 00 00
  1400 0000`.
- A fragment holds at most 1200 bytes (0x4b0, 0xd5aca). The window was 20
  both ways. The sender repeats an unacknowledged fragment or a bare
  acknowledge 350 ms (0x15e, 0xd57ec) after the last. VERIFIED (the
  instructions) and live.
- The stream holds messages, each `long length` then that many bytes
  (`SV_SendReliableServerCommand` 0x10eea0 writes `size - 4` over the first
  long). A message's body opens with `long cmd`. The data is plain: no
  Huffman code and no scramble. VERIFIED (the instructions at
  0x10eea0..0x10eed2, 0x1114fc..0x111520) and live.

| cmd | Direction | Body |
|---|---|---|
| 1 | s2c | the gamestate (section 9) |
| 5 | both | a download (7.2) |
| 9 | s2c | stats request: `byte 0` (`SV_RequestStats` 0x10eef0) |
| 9 | c2s | stats (7.1) |
| 8, 0x35448 | c2s | authentication library data (section 5) |
| 12 | c2s | anti-cheat module data |
| 0x866 | c2s | a screenshot |
| 0x2c49 | c2s | ignored |

The c2s rows are `SV_ExecuteReliableMessage`'s dispatch (0x110fed..0x1110ab),
VERIFIED (the instructions); any other cmd prints `Unknown clientcommand`.
The s2c 1 and 9 rows are VERIFIED live.

### 7.1 Stats

- `SV_SendClientGameState` asks for stats once (`SV_RequestStats`, state
  byte at client +0xa33a1 set to 0xff) and sends no gamestate until they
  came. VERIFIED (the instructions at 0x10ef09..0x10ef6f, 0x11092d).
- The answer, `SV_ReceiveStats_f` (0x10f5b0): `long 9`, `byte mode`.
  Mode 0 marks the stats received with none. Modes 1 and 2 carry `long
  0x2000` and 0x2000 bytes; mode 1 is read and dropped, mode 2 is
  AES-128-CBC (libtomcrypt's `rijndael_*`), the key the connection's
  challenge as four little-endian copies, the IV the four words
  0xeb62114f 0x66996144 0x7341cfa4 0xb9551299. A length other than
  0x2000 drops the client (`Received stats data of invalid length`).
  VERIFIED (the instructions at 0x10f5eb..0x10f81a).
- Mode 0 joined, and the stock scripts kicked the client at connect
  (`Player kicked by scriptadmin`) on its empty stats. A fresh 1.7
  profile's block (`net::stats::Stats::fresh`, `cod4-netfields.md` 10.1)
  in mode 2 played. VERIFIED live (`cap-noauth1`; the mode 0 run is not
  kept).
- The 1.7 out-of-band `stats` packets still get `statResponse`
  (`SV_ReceiveStats` 0x10ece0). INFERRED for a protocol 21 client: the
  capture sent none.

### 7.2 Downloads

- Client to server, `cmd 5` then a sub-op byte through the table at
  0x2c1e60: 0 begin (`SV_BeginDownloadX_f`), 1 done, 2 stop, 3 the blocks
  received (`SV_SelectDownloadBlocksX_f`), 4 www failed, 5 www done, 6
  `SV_WWWDownload_BBL8R_f`, 7 www checksum failed. VERIFIED (the table and
  `SV_ExecuteDownloadCmd` 0x110ef0).
- Server to client, `cmd 5` then: 1 begin (`long` size and
  `SV_WriteChecksumInfo`), 3 an error string, 0 a block (`long` position,
  `short` length, the bytes), 2 a www redirect (`SV_WWWRedirect`: the
  `%s/%s` URL string and two longs). VERIFIED (the writes in
  `SV_WriteDownloadToClient` 0x1102c0 and `SV_WWWRedirect`).
- No download ran in the captures. 1.7's netchan download commands
  (`cod4-handshake.md` 11) do not appear in this path. INFERRED.

## 8. Netchan, scramble, Huffman, the client header

- Server to client: 1.7's netchan, scramble and static Huffman code
  (`cod4-handshake.md` 4). Every server message of `cap-noauth1` and
  `cap-two` unscrambled and Huffman-decoded with vcod's 1.7 code.
  VERIFIED live.
- `Huffman_InitMain` (0xcb50a) stores a finished 513-node tree as
  constants, as 1.7's builder does. VERIFIED (the instructions). Its
  weights are not compared row by row; the live decode is the check.
- Client to server: `long sequence`, `short qport`, then a plain header of
  four longs, `serverId`, `messageAcknowledge`, `reliableAcknowledge`,
  `configDataAcknowledge` (16 bytes where 1.7 has 9 with a one-byte
  `serverId`), then the scrambled Huffman block. `SV_PacketEvent` reads
  the four (0x121f1b..0x121f7c) and keeps the largest config acknowledge.
  VERIFIED (the instructions) and live: the server took the probe's
  messages, spawned it and echoed its `say`.
- The block's scramble is 1.7's: the low bytes of `serverId`, the
  challenge and `messageAcknowledge` XORed for the seed, keyed on the
  server command the client acknowledges (`SV_Netchan_Decode` 0x122711).
  VERIFIED (the instructions at 0x122726..0x12278b).
- `SV_ExecuteClientMessage` (0x112b70) tests `(sv.serverId ^ header
  serverId) & 0xffffff00` (0x112bea..0x112bf5): non-zero means the
  client still holds an old gamestate, and the gamestate is sent again
  once its `messageAcknowledge` passes the one it came in. VERIFIED (the
  instructions). The ops are 1.7's: 3-bit `clc_move`, `clc_moveNoDelta`,
  `clc_clientCommand`, `clc_EOF` (0x112c0a..0x112c73). VERIFIED (the
  instructions) and live.

## 9. The gamestate

- It goes as side channel `cmd 1`, not as a netchan message. Before it
  the netchan carries the pending server commands and `svc_EOF`. VERIFIED
  (`SV_SendClientGameState` 0x110a99..0x110b6e) and live.
- Body after `long 1` (`SV_WriteGameState` 0x11eb70), each VERIFIED (the
  instructions) and live in `cap-noauth1`:
  - `byte 1`, `long serverCommandSequence`;
  - `byte 2`, `long count`, then `count` times `long index` and a big
    string. Every slot of 0..4882 (0x1313 slots) is sent, empty ones too:
    4883 in the capture;
  - `byte 3` and an entity delta per baseline, 1.7's format but for
    section 10 (62 baselines);
  - `byte 11` client config per connected client (section 11);
  - `byte 7`, then `long configDataSequence`, `long clientNum`, `long
    checksumFeed`, `long 0`. The feed equalled the console's
    `SV_SpawnServer checksum feed`.
- 1.7 sends a `short` count with a 1-bit or 12-bit index and leaves out a
  slot holding its `constantConfigStrings` default (`cod4-handshake.md`
  5.1). Its mp_crash gamestate also holds 62 baselines. VERIFIED
  (`handshake-1.7.txt`).
- The gamestate was 52108 bytes against 12730 decompressed on 1.7.
  VERIFIED live. It goes through a 0x20000-byte buffer (`MSG_Init` at
  0x110aaf). VERIFIED (the instructions).

## 10. The delta codec

- The netfield tables are 1.7's: every row of the entity (all 18 eType
  tables through `netFieldList` 0x364b80), player, client, objective,
  hudelem and archived-entity tables has 1.7's offset, bits and change
  hint. Only names differ (`netname` for `name`,
  `lerp.u.vehicle.team` for `teamAndOwnerIndex`). VERIFIED (each table
  dumped by its symbol and diffed against `tools/re/cod4_netfields.py
  --rows` of `cod4_lnxded`).
- The origin codes -92, -91, -90 (entity `lerp.pos.trBase`, hudelem x, y,
  z) are a whole float off the byte cursor: `MSG_WriteOriginFloat`
  (0xd26d8) and `MSG_WriteOriginZFloat` (0xd26ee) call `MSG_WriteFloat`
  (0xd1786, four bytes at `cursize`), and the field writer's table at
  0x2b761c sends `bits + 100` = 8, 9, 10 there (0xd2bf5, 0xd2c0b). 1.7
  sends 7 or 16 bits against `CS_MAPCENTER` (`cod4-netfields.md` 2).
  VERIFIED (the instructions). Baseline 77 on mp_crash reads -235.1,
  1394.7, 230.2 where 1.7's rounds to -235, 1395, 230. VERIFIED live
  (`cod4x_probe --gamestate` on `cap-noauth1`'s `rel-2-1.bin`,
  `--baseline17` on `handshake-1.7.txt`).
- Every other code reads as 1.7's: with only the origin arm changed,
  vcod's 1.7 reader took all 461 messages of `cap-noauth1` to their last
  byte. VERIFIED live. The change used for that measurement, in
  `delta.rs`'s `Code::read`, was one arm reading the long:
  `Code::Origin(_) if origin_floats() => r.read_long() as u32,` with
  `origin_floats()` an env switch; it is not on the branch.
- `MSG_WriteAngle16` (0xd1c89) truncates `f * 65536 / 360`
  (`cvttss2si`), where 1.7's server adds 0.5 first; the reader is the
  same. VERIFIED (the instructions); baseline 77's yaw 101.7609 against
  1.7's 101.7664 shows it live.

## 11. Client config data (svc 11)

- `SV_WriteClientConfigInfo` (0x1234e2): `byte 11`, `long
  configDataSequence` (left out in the gamestate, where the argument is
  -1), `byte clientNum`, the player's name string (client +0x2b8) and a
  second string (client +0x2dc). VERIFIED (the instructions). The second
  string was empty in every capture; that it is a clan tag is INFERRED.
- `SV_BeginClientSnapshot` (0x123849) writes, after the server commands of
  every snapshot message, one `svc 11` per config entry newer than the
  client's `configDataAcknowledge`, from a ring of 4096 (`and 0xfff`),
  once the client has acknowledged its gamestate (state byte +0xa33a2
  above 1). VERIFIED (the instructions at 0x123558..0x1235ed,
  0x12389f..0x1238ad).
- An acknowledge more than 4095 behind the sequence, or ahead of it, stores
  `svs.configDataSequence - MAX_CONFIGDATACACHE >= cl->configDataAcknowledge`
  (0x2c6b00) or `svs.configDataSequence < cl->configDataAcknowledge`
  (0x2c6b4c) at client +0x380 and sends nothing. VERIFIED (the
  instructions at 0x12358e, 0x12359d). That +0x380 is a drop reason is
  INFERRED.
- Live (`cap-two`): when a second probe joined, the first's messages
  opened `0b 02000000 01 "vcodx1002" "" 06 ...` and kept doing so while it
  sent `configDataAcknowledge` 1. vcod's 1.7 parser stops at op 11
  (illegible). VERIFIED live.

## 12. Configstrings

- The slots 1.7 uses keep their numbers and contents on mp_crash:
  serverinfo 0, systeminfo 1, fog 9, map centre 12, dvar names from 20,
  values from 148, localized strings from 309, models from 830, effects
  from 1598, menus from 1970, materials from 2002, the weapon list 2258.
  VERIFIED live (`cap-noauth1`'s gamestate against `handshake-1.7.txt`).
- Slot 2 reads `cod21 ` where 1.7's reads `cod`. VERIFIED live.
- The table runs to 4883 slots; 2442..4882 were all empty here. What CoD4x
  puts past 2441 is not known. VERIFIED live (the empties).
- Later changes come as 1.7's `d <index> <string>` server commands
  (`d 824`, `d 251`, `d 11`, `d 1343`). VERIFIED live.
- Systeminfo carries `sv_serverid` as a plain number (257 in
  `cap-noauth1`) and `sv_referencedFFNames` without
  `cod4x_patchv2` here (section 15). VERIFIED live.

## 13. Server commands

The capture's server commands were all 1.7's (`v`, `f`, `H`, `G`, `t`, `u`,
`L`, `J`, `a`, `C`, `d`, `s`, `h`, `w`). VERIFIED live. Commands CoD4x adds
for its own client were not looked for; open (section 17).

## 14. The server browser

- A CoD4x row looks like a 1.7 row to 1.7's browser: `infoResponse`'s
  `protocol` is 6. VERIFIED live. vcod tells it apart by `shortversion`
  in `infoResponse` and `protocol`/`version` in `statusResponse`
  (`vcod_cod4::net::browser::info_is_cod4x`, `Status::is_cod4x`; the
  front end's filter `ui_browserShowCoD4x`, `cod4-front-end.md` 7). 21.3's
  answers still match those keys. VERIFIED live.
- `kc` (killcam) is `ki` in CoD4x's `infoResponse`, so vcod's killcam
  column and filter read 0 on every CoD4x row. VERIFIED live.
- No answer says whether the server needs authentication (section 2).
  VERIFIED live.

## 15. What CoD4x servers rely on client-side

- The CoD4x client update: a 1.7 client reaches it only through a
  server's update proxy (section 4). The project's README says the update
  is proprietary and Windows-only. vcod cannot take it and has no use for
  it on an `sv_noauth 1` server. INFERRED (the README and section 4).
- `cod4x_patchv2.ff`: a server that has it loads it with the common zones
  (`R_Init` 0x12f4de). Whether it then appears in `sv_referencedFFNames`
  and the client must hold the same zone is not measured: the release
  does not ship it and nothing here could fetch it. INFERRED; open.
- `xbase_00.iwd`: the server reads its certificates from it
  (`InitCA Attempting to read certificates from file main/xbase_00.iwd`).
  Whether clients are asked for it is not known. INFERRED; open.
- A mod server's assets come through the downloads of 7.2. INFERRED.

## 16. Tools and captures

- `tools/cod4x_capture.sh SERVER_BIN OUTDIR [CLIENT_CMD]` runs the server
  (or 1.7's `cod4_lnxded`) in a rootless user, net, mount and pid
  namespace, the install bind-mounted read-only, every master and update
  name mapped to 127.0.0.2, and `tools/lo_capture.py` logging `lo`. It
  waits for `getinfo` to answer, runs `CLIENT_CMD` with `$ADDR`, then
  stops everything. `PORT` (28990), `MAP`, `CAP_SECS`, `SERVER_SETS`.
- `cargo run -p vcod-cod4 --example cod4x_probe -- HOST:PORT OUTDIR` is
  the protocol 21 client of section 6; `--gamestate FILE` decodes a saved
  side channel gamestate offline, and `--baseline17 CAPTURE N` prints a
  1.7 capture's baseline `N` beside it.
- The captures, each `COD4_DIR=<1.7 install>` and the release binary as
  `BIN`:

  | Capture | `SERVER_SETS` | `CLIENT_CMD` |
  |---|---|---|
  | `run-oob-auth` | | `oob_walk.py $ADDR 6 7 8 20 21 22` |
  | `cap-oob-noauth1` | `+set sv_noauth 1` | `oob_walk.py $ADDR 21` |
  | `cap-legacy` | | `legacy_walk.py $ADDR 30` (a 1.7 join, connect every 3 s) |
  | `cap-vcod` | | the `cod4_probe` example, `--secs 5` |
  | `cap-noauth1`, `cap-noauth0` | `+set sv_noauth 1` / `0` | `cod4x_probe $ADDR <dir> --secs 20 --stats fresh --cmd '2 mr {sid} {menu:team_marinesopfor} autoassign' --cmd '3.5 mr {sid} {menu:changeclass} assault_mp,0' --cmd '8 say hello'`, with the origin change of section 10 built in |
  | `cap-two` | `+set sv_noauth 1` | two `cod4x_probe`s, `--qport 1001` and, 6 s later, `--qport 1002` |
  | `cap-retail` | (`cod4_lnxded` as `BIN`) | `oob_walk.py $ADDR 21 6` |

  `oob_walk.py` and `legacy_walk.py` are throwaway scripts beside the
  captures (getchallenge and connect per protocol; a 1.7 join resending
  `connect`).

## 17. Still open

- What an authenticating server (Steam library loaded) does to a protocol
  21 client that sends no authentication data: waits forever, drops it,
  or tells it why. Only a server with that library shows it.
- The CoD4x-only server commands and side channel messages a stock server
  sends its own client during play, past the ones seen here.
- `cod4x_patchv2.ff` and `xbase_00.iwd` on a real install (section 15).
- A download through 7.2 and a `sv_pure 1` server's `cp` on protocol 21.
- What the configstrings past 2441 hold on a server that uses them.

## 18. Design proposal

Target: vcod's client joins and plays on CoD4x servers that run `sv_noauth
1` (or with no Steam library), the protocol 21 wire above. Out of scope:
client authentication, the anti-cheat and screenshot traffic, the CoD4x
client update, and vcod-server speaking protocol 21.

Each stage ends with its gate green; the hours are agent-hours.

1. **Protocol 21 codec** (4-6 h). A wire choice in `vcod_cod4::net`
   (1.7 or CoD4x 21) that the delta reader, gamestate parser, snapshot
   parser and client writer take: origin codes as floats (10), the side
   channel gamestate (9), `svc 11` in snapshots and the
   `configDataAcknowledge` it drives (11), the 16-byte client header (8),
   4883 configstring slots without defaults (9, 12). Gate: committed
   fixtures cut from `cap-noauth1` and `cap-two` (the side channel
   messages and the netchan messages) decode to their last byte and
   re-encode byte for byte, as `handshake-1.7.txt` does now; the 1.7
   fixtures stay green.
2. **Side channel and join** (6-8 h). `Conn::join` sends protocol 21 on an
   `xproto` challenge, runs the side channel (fragments, acknowledges,
   window, resend), answers the stats request and takes the gamestate
   from it. Gate: an ignored live test against the release binary under
   `tools/cod4x_capture.sh` (an env var names the binary, as `COD4_LNXDED`
   does) that joins, picks a team and class, spawns and leaves, with the
   message order of `cap-noauth1`; the 1.7 join gates unchanged.
3. **Refusal and browser** (3-4 h). A server that never sends the
   gamestate after the stats (an authenticating one, section 5) ends the
   join with a plain reason after a timeout; the browser lists CoD4x rows
   by default, marks them, reads `ki` for the killcam column. Gate: a
   live test against the release with the side channel's gamestate
   withheld (a relay that drops `cmd 1`), and the browser test's fake
   master listing a CoD4x row.
4. **Downloads** (4-6 h). 7.2's download over the side channel and its
   www redirect, through the existing `download` module's file rules.
   Gate: a live test downloading a mod's iwd from the release binary
   running `fs_game mods/<mod>`, its bytes against the file.

Questions for the owner:

- Reach. The 2026-11-12 scan found no `getstatus` advertising `sv_noauth`,
  and section 2 shows none would: the dvar is not serverinfo. The only
  test is a join attempt: an authenticating server would leave the client
  without a gamestate after its stats (section 5, INFERRED). How many
  public CoD4x servers run without authentication is unknown, possibly
  none. Is a client that joins only those worth stages 1-4?
- Stats. Stock gametypes kick a client with no stats, so playing needs
  mode 2, an AES-128-CBC block keyed on the connection's challenge (7.1).
  This is the stats transport, not authentication, but it is a cipher
  CoD4x added. In scope?
- Measuring reach needs one join attempt against public CoD4x servers,
  which the project's rules forbid today. Allow a one-off, listed set?
- `cod4x_patchv2.ff`: if real servers reference it, a vcod client without
  the CoD4x install cannot pass the fastfile check. Find out from a real
  server's systeminfo (`getstatus` does not show it) before stage 2?
