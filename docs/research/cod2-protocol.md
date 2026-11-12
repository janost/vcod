# Call of Duty 2 1.3 (protocol 118) against CoD 1.1 (protocol 1)

The CoD2 1.3 multiplayer wire protocol as a diff against `docs/protocol-1.1.md`. Where this doc is silent, 1.1's rule holds: the transport, the message-block Huffman model, the two-cursor `msg_t`, the snapshot frame and the generic delta codec carry over. Each section says which parts were checked unchanged and which moved.

Contents: 1 handshake, 2 netchan, 3 server messages, 4 client messages, 5 netfields and codec, 6 configstrings, 7 divergences list, 8 open questions, 9-10 live sessions, 11 pure and downloads, 12-13 entities and events, 14 vcod's server, 15 the wire against retail, 16 retail client on vcod-server, 17 server housekeeping, 18 the client on a bad link.

## Sources

- `cod2_lnxded`: the CoD2 1.3 Linux dedicated server (`pc_1.3_1_1`), stripped, game module linked in. Addresses without a module name are in it. Function names come from the name map transferred from the Mac build or, where the map has none, from what the function does; a name in the second group is INFERRED, its address is not.
- The Mac 1.3 MP binary (client and server in one, symbolized, with STABS struct layouts). Addresses marked "Mac" are in its i386 slice. The client half (`CL_*`, `CG_*`, the parsers) exists only there. For the server half the Linux address is the oracle, with the Mac one beside it where the Mac code was read.
- 1.1 tables and addresses come from `cod_lnxded` 1.1d, read with `tools/re/netfields.py` and `tools/re/netfield_diff.py`.

Section 9 is a live session against retail `cod2_lnxded` on loopback: the out-of-band replies, a plain-text connect, the gamestate and about 40 snapshots decoded to the last byte by a Python client written from this doc. Section 10 is vcod's own client (`crates/cod2`) on the same server. Section 11 is the pure check and downloads. Claims those sessions confirm say "VERIFIED live"; everything else is read out of the two binaries.

## 1. Connectionless packets and the handshake

Transport and framing are 1.1's: `FF FF FF FF`, then a text line. VERIFIED: `SV_PacketEvent` (0x80960e2, Mac 0x15b836) takes any datagram of 4 or more bytes starting with -1 as out of band.

### The commands a server answers

`SV_ConnectionlessPacket` (0x809594e, Mac 0x15ac8c) reads the first line with `MSG_ReadStringLine`, tokenizes it and compares `argv[0]` case-insensitively against, in order: `getstatus`, `v`, `getinfo`, `getchallenge`, `connect`, `ipAuthorize`, `rcon`, `disconnect`. VERIFIED (strings at 0x814b6e0 for `v` and 0x814b715 for `rcon`, the rest inline). A payload starting `pb_` goes to PunkBuster first (string at 0x814b6bf). VERIFIED. The OOB `disconnect` does nothing (the compare's result is discarded). VERIFIED. `v` (voice) is new, 1.7.

### 1.1 `getchallenge` / `challengeResponse`

The client sends `getchallenge 0 "<md5>"`, the second argument a 32-hex-digit string built by `CL_BuildMd5StrFromCDKey` (`CL_CheckForResend`, Mac 0x14ad5a; format string `getchallenge 0 "%s"`). VERIFIED. The 1.1 client sent a bare `getchallenge`.

`SV_GetChallenge` (0x808d18e) keeps 1024 challenge slots of 116 (0x74) bytes at 0x84230dc: the address (20 bytes, `netadr_t` is five words in this build), the challenge at +0x14, times, the GUID at +0x2c, and two 33-byte strings at +0x30 and +0x51. VERIFIED (the loop bound 0x400 and the stride). A new challenge is `rand() << 16 ^ rand() ^ svs.time`, as in 1.1. VERIFIED. When the request carries three tokens the third is copied into the slot's +0x51 string. VERIFIED. The table's rules (no expiry, the oldest written slot replaced, `NET_CompareAdr` 0x806c54c on the whole address in `SV_GetChallenge`, `SV_DirectConnect` and `SV_DropClient` 0x808f02e) are 1.1's, set side by side in `cod11-server-handshake.md`, "The challenge table"; vcod runs both games on `engine::challenge`.

The reply is `challengeResponse %i`, one integer. VERIFIED (format string inline). INFERRED, off the branches: it goes out at once only when `net_lanauthorize` is 0 and the address is a LAN one, or when the authorize server has not answered for 1200 s and this slot asked more than 7 s ago; otherwise the server sends `getIpAuthorize` to the master and answers only when `ipAuthorize` comes back (1.3).

### 1.2 `connect`

**Divergence from 1.1.** The client sends `connect "<userinfo>"` as plain text; 1.1 Huffman-compresses the payload from byte 12. VERIFIED: `CL_CheckForResend` (Mac 0x14ad5a) builds `connect "` + userinfo + `"` and hands it to `NET_OutOfBandData` (Mac 0x1588f6), which writes the four `FF` bytes and copies the payload with no Huffman step. `SV_DirectConnect` (0x808e2aa) reads the userinfo as `argv[1]` of the tokenized line. VERIFIED. A plain-text `connect` got `connectResponse` from retail. VERIFIED live (9).

The userinfo is the client's userinfo dvars plus `protocol` 118 (`%i` of 0x76), `challenge` and `qport`. VERIFIED (`Info_SetValueForKey` calls in `CL_CheckForResend`). The server's own test-client string (`connect "\cg_predictItems\1\cl_punkbuster\0\cl_anonymous\0\color\4\head\default\model\multi\snaps\20\rate\5000\name\bot%d\protocol\%d"`, inline in `cod2_lnxded`) shows the stock key set; `cl_punkbuster` is new against 1.1. VERIFIED.

`SV_DirectConnect` reads, in order:

- `protocol`, compared with 118 at 0x808e346. A mismatch answers `error\nEXE_SERVER_IS_DIFFERENT_VER\x15%s\n` (0x8149ec0) with `1.3` (0x8149eb7). VERIFIED.
- `challenge` and `qport`. VERIFIED. A reconnect from the same address and qport inside `sv_reconnectlimit` (0x849f75c) seconds of the last is ignored. VERIFIED (the compare and the dvar's registration).
- For a non-local address, the challenge slot whose address and challenge match; none answers `error\nEXE_BAD_CHALLENGE`. VERIFIED. The slot's GUID and the two 33-byte strings come over to the client here. VERIFIED.
- Off the LAN, `sv_minPing` and `sv_maxPing` against the challenge round trip: `error\nEXE_ERR_HIGH_PING_ONLY` and `error\nEXE_ERR_LOW_PING_ONLY`. VERIFIED (strings and the two dvars). New against 1.1, which only listed them in `getinfo`.
- `cl_punkbuster`, passed to `PbAuthClient`; a non-zero answer starting `error\n` is sent back verbatim. VERIFIED.
- `password` against `sv_privatePassword` (0x849f744); a match lets the client into the `sv_privateClients` (0x849f754) slots, which are otherwise skipped. VERIFIED (the compare and the loop start). A full server answers `error\nEXE_SERVERISFULL`. VERIFIED.
- `ClientConnect`'s refusal goes out as `error\n<reason>`; success as `connectResponse`, no arguments. VERIFIED.

A zero GUID is logged (`Connecting player #%i has a zero GUID`) and allowed. VERIFIED.

### 1.3 IP authorize

`SV_AuthorizeRequest` (0x808cfc6) sends the master `getIpAuthorize <challenge> <a.b.c.d> "<fs_game>" <sv_allowAnonymous>`, with ` PB "<string>"` appended when the client's `getchallenge` carried its third token. VERIFIED (both format strings). The master is `cod2master.activision.com`, port 20700 (`BigShort(0x50dc)` in `SV_GetChallenge`). VERIFIED.

`SV_AuthorizeIpPacket` (0x808da2a) takes `ipAuthorize <challenge> <verdict> <reason> <guid> <string>` from the master's address only, matching the slot by challenge. VERIFIED (argv 1..5). On `accept` it stores `atoi(argv[4])` as the slot's GUID and argv 5 in the +0x30 string, checks the permanent and temporary GUID ban lists, and sends `challengeResponse`. VERIFIED. A `demo` server (`fs_restrict`) answers `error\nEXE_ERR_NOT_A_DEMO_SERVER`. VERIFIED. The other verdicts map `CLIENT_UNKNOWN_TO_AUTH` and `BAD_CDKEY` to `needcdkey`, `INVALID_CDKEY` to `error\nEXE_ERR_CDKEY_IN_USE`, `BANNED_CDKEY` to `error\nEXE_ERR_BAD_CDKEY`, and anything else to `error\n<reason>`. VERIFIED.

vcod-server asks no authorize server and never takes this path. The client side (`getKeyAuthorize`) is in cod11-server-handshake.md, "CD key authorization (the clients)".

### 1.4 `getinfo`

`SVC_Info` (0x809537c) builds its string itself, in this order: `challenge` (argv 1), `protocol` (118), `hostname`, `mapname`, `clients` (only when non-zero), `sv_maxclients` (the public slots plus the private ones in use, only when positive), `gametype`, `pure` `1` (when `sv_pure` or a flag at 0x84a4480 is set), `minPing`, `maxPing` (when set), `game` (when `fs_game` is set), `sv_allowAnonymous` (when set), `con_disabled` (`sv_disableClientConsole`, when set), `pswrd` `1` (when `g_password` is set), `ff` (`scr_friendlyfire`, when non-zero), `kc` (`scr_killcam`, when non-zero), `hw` `1` always, `mod`, `voice` (`sv_voice`), `pb` (`sv_punkbuster`). VERIFIED (key strings at 0x814b649, 0x814b65e, 0x814b693, 0x814b6a2, 0x814b6a5, 0x814b5de, 0x814b6ae). The reply opens `infoResponse\n`. VERIFIED. `mod` is 1 when `sv_pure` is off, when `fs_game` is set, or when `sv_referencedIwdNames` names an iwd that `FS_iwIwd` does not count as stock; otherwise 0. INFERRED, off the branches.

Against 1.1's list, new: `con_disabled`, `ff`, `kc`, `hw`, `mod`, `voice`, `pb`; `pure` and `pswrd` are now present only when true.

### 1.5 `getstatus`

`SVC_Status` (0x8094c84) sends `statusResponse\n<serverinfo>\n<player lines>`: `Dvar_InfoString(0x404)` with `challenge`, `pswrd` (`0` or `1`) and `mod` set into it, and, with `fs_restrict` on, `sv_keywords` prefixed `demo `. VERIFIED. Each player line is `%i %i "%s"\n`: score (`G_GetClientScore`, 0 before the game is up), ping, name. VERIFIED. 1.1's shape, with the `mod` key added.

### 1.6 `rcon`

**Divergence from 1.1.** `SVC_RemoteCommand` (0x8097188; its commands are in 3.4.2) checks argv 1 against `rcon_password` (0x849f740, the `Dvar_RegisterString("rcon_password", ...)` site), Q3's name; 1.1's is `rconPassword`. VERIFIED. A second rcon less than 500 ms after the last one from anyone gets no answer, good password or bad (the global at 0x849fbf4). VERIFIED. Output goes back through `SV_FlushRedirect` (0x8097050) as `print\n<text>` packets of at most 1300 bytes. VERIFIED. A command starting `pb_sv_` also runs `PbServerForceProcess`. VERIFIED.

The refusals, each a `print` (0x8097188, strings at file offsets 0x103c95 and 0x103cc0), are CoD4 1.7's three (cod4-handshake.md 14.2). VERIFIED:

| case | reply |
|---|---|
| `rcon_password` empty | `The server must set 'rcon_password' for clients to use 'rcon'.` |
| password set, argv 1 empty | `You must log in with 'rcon login <password>' before using 'rcon'.` |
| wrong password | `Invalid password.` |

1.1's `Bad rconpassword.` and `No rconpassword set on the server.` are not in `cod2_lnxded`. VERIFIED.

The client side is CoD4's too. `CL_Rcon_f` (`CoD2MP_s.exe` 0x4b41d0, Mac 0x161566) reads no password or address dvar. VERIFIED:

- `rcon login <password>` (0x4b40f0) keeps the password in a static buffer (0x1492a24), refusing one of 24 bytes or more.
- `rcon logout` (0x4b4150) clears it or prints `Not logged in`.
- `rcon host <address>` (0x4b4170) sets the address, port 28960 when it names none.
- Any other line without a login prints `You need to log in with 'rcon login <password>' before using rcon.` (0x5b29a8).
- The packet is `rcon <password>` then each argument after a space, quoted as `Com_AddToString` quotes (flag 1, Mac 0x161566).
- It goes to the joined server when `cls.state` (0x609fe0) is 5 or more (`cmp ...,0x5; jl` at 0x4b4398), else to `rcon host`'s address, else it prints `Can't determine rcon target.  You can fix this by either:` (0x5b2938), `1) Joining the server as a player.` (0x5b2914), `2) Setting the host server with 'rcon host <address>'.` (0x5b28dc), one `Com_Printf` each.

1.1's `You must either be connected,\nor set the 'rconAddress' cvar\nto issue rcon commands` (CoDMP.exe 0x567388) and the strings `rconAddress` and `rconPassword` are not in `CoD2MP_s.exe`. VERIFIED. `CL_ConnectionlessPacket` (Mac 0x14c4c4) prints a `print` from any sender in any state. INFERRED (no state or address test on the branch).

vcod: the CoD2 client runs CoD4's rcon (`console::rcon::Cod4Rcon` through `Session::login_rcon`), and a joined session's `print`s reach the console. vcod-server's CoD2 engine prints the three replies above. vcod-server reads `--set rconPassword=` as `rcon_password` under `--game cod2` and `--game cod4` (`vcod_server::rcon::login_cvar_name`), a vcod convenience: retail's `+set rconPassword` makes a dvar nothing reads.

The heartbeat is `heartbeat COD-2\n` to `cod2master.activision.com` every 180 s while `dedicated` reads 2, and `heartbeat flatline\n` on the way down (`SV_MasterHeartbeat`, 0x8096ed6). VERIFIED. `dedicated` registers with default 2 (`Dvar_RegisterInt("dedicated", 2, 0, 2, 0x1040)`, the store to 0x84a8780). VERIFIED. `+set dedicated 1` on the command line does not stop it: the server still logged `Sending heartbeat to cod2master.activision.com` (9). VERIFIED live. Run a test server in a network namespace with no route out.

The heartbeat port is `PORT_MASTER` 20710 (`BigShort(0x50e6)` in `SV_MasterAddress`, 0x8096e0c), set only when the name holds no `:`; its `strstr(":", name)` has 1.1's swapped arguments. The name is a constant, with no `sv_master` dvars. VERIFIED. The same function sends the master `SVC_Status` every 600 s at `dedicated 2`. VERIFIED. The client's Internet refresh runs `globalservers 0 118 full empty` (`0x76`, or `debug_protocol` when set), and `CL_GlobalServers_f` sends `getservers <args past the master number>` to `cod2master.activision.com` on 20710 (`0xe650`, byte-swapped). VERIFIED (Mac strings and stores). A `getserversResponse` reads as 1.1's: `\` and six address bytes per server, `\EOT` closing every packet but the last, which ends `\EOF`. VERIFIED live.

vcod lists on and asks `master.cod.pm` instead (its operator allowed it), same port. `vcod-server --game cod2` sends `heartbeat COD-2` there at `dedicated 2` only (default 1, `crates/server/src/cod2`) and flatlines on `quit`; the 600 s status is 17.6. The browser asks `getservers 118 full empty` alone, never another protocol and never 0, at most once a second. The master answered `getservers 118` with 314 entries in three packets. VERIFIED capture.

### 1.7 Voice

Voice rides out of band. The client sends `FF FF FF FF`, the string `v` with its NUL, a short qport, a byte count, then per packet a length byte and the data (`CL_WriteVoicePacket`, Mac 0x187fac, through `NET_OutOfBandVoiceData`, which adds no compression). VERIFIED. `SV_VoicePacket` (0x8094b56) finds the client by address and qport. VERIFIED. The server sends `v`, a byte count, then per packet the talker's client number, a length byte and the data (`SV_SendClientVoiceData`, Mac 0x1bb770). VERIFIED. 1.1 has no voice.

### 1.8 Map change

`SV_SpawnServer` (0x8093520) sends 1.1's `loadingnewmap\n<map>\n<gametype>` to every client past `CS_CONNECTED` and sleeps 250 ms (`NET_Sleep(0xfa)`). VERIFIED (format string inline, the `2 <` state test, the constant). It adds `0x10` to the byte `sv_serverid` (`0x10` again when the high nibble wraps to 0) and sets each client whose `ClientConnect` accepts it to `CS_CONNECTED`. VERIFIED (the stores). Nothing else goes out: the gamestate is resent when the client's next message carries the old id (4.1). VERIFIED live (10: the client sent 16 until the gamestate with 32 came after `map mp_carentan`, and 49 after a `map_restart`). The client's `loadingnewmap` arm of `CL_ConnectionlessPacket` (Mac 0x14c4c4) compares the base address, reads the two lines, calls `CL_SetupForNewServerMap` and sets the connection state to 5. VERIFIED (the calls and the store).

`map_restart` is `SV_MapRestart(0)` (0x808be3e): `SV_SpawnServer` on the current `mapname` whatever the state, a full reload with a new gamestate. INFERRED, off the branch the 0 argument takes. `map_rotate` (`SV_MapRotate_f`, 0x808c132) ends in `map <name>`, or `fast_restart` when the rotation names no map. VERIFIED (the `map %s` format and the call).

### 1.9 `fast_restart`

`fast_restart` is `SV_MapRestart(1)`. VERIFIED (`Cmd_AddCommand` with the two wrappers). Its in-place arm runs when the server is not loading, `g_gametype` is unchanged and the argument is set; anything else falls back to `SV_SpawnServer`. INFERRED (the branch). The arm:

- adds 1 to the low nibble of `sv_serverid` only (`(id + 1 & 0xf) + (id & 0xf0)`);
- runs three 100 ms frames;
- for every client past `CS_CONNECTED`, queues `B` when `G_GetSavePersist()` is 0 and `n` when 1, calls `ClientConnect` and, for an active client, `SV_ClientEnterWorld`.

VERIFIED for the stores, the `0x42` / `0x6e` and the calls; their order is INFERRED. No gamestate goes out: the new id reaches the client as a `d 1` systeminfo update, which it must echo in its headers. VERIFIED live (10: 32 to 33 on dm; 16 to 17, 18, 19 over three sd rounds). The script's `map_restart(<persist>)` (`GScr_MapRestart`, 0x8116a3c) stores the persist flag and runs `fast_restart`. VERIFIED. An sd round restart arrived as `n` and an rcon `fast_restart` on dm as `B`. VERIFIED live.

### 1.10 Drops and timeouts

The client honours an OOB `disconnect` only when more than 2999 ms passed since its last sequenced packet, as 1.1's does, and an OOB `error` at once (`CL_ConnectionlessPacket`, Mac 0x14c4c4). INFERRED (the `2999 <` compare guards the arm). `cl_timeout` defaults to 200 s (`CL_InitOnceForAllClients`, Mac 0x1485e2: `Dvar_RegisterFloat("cl_timeout", 200.0, 0, 3600)`). VERIFIED. `CL_Frame` (Mac 0x14b182) drops a connection past state 4 after that long without a packet, counted over six frames, with `EXE_ERR_SERVER_TIMEOUT`. INFERRED (the compare and the `< 6`); the key VERIFIED (section 18.2). `clientkick` arrived as the server command `w "EXE_PLAYERKICKED"` and `quit` as `w "EXE_SERVERQUIT"`. VERIFIED live (section 10).

## 2. Netchan, Huffman and the scramble

### 2.1 Huffman

The tree is built from 1.1's and Q3's frequency table. VERIFIED: the 256 words at 0x8142d00, which `MSG_initHuffman` (0x806b12c) feeds through `Huff_addRef` into the trees at 0x81e9c60 and 0x81f0c7c, equal `MSG_HDATA` in `crates/common/src/net/huffman.rs` word for word. The whole-block model is unchanged: `SV_SendMessageToClient` (Mac 0x195788) copies the first 4 bytes and compresses the rest with `MSG_WriteBitsCompress` (0x8067f4e, Mac 0x17345e), a per-byte `Huff_offsetTransmit` walk with no length prefix; both sides decompress the whole block with `MSG_ReadBitsCompress` (0x8067fa2, Mac 0x1734c6) before reading an op. VERIFIED.

### 2.2 Message headers

Both are 1.1's shape. VERIFIED:

| direction | plain header | then |
|---|---|---|
| server to client | 4-byte `reliableAcknowledge` (`MSG_WriteLong` of `client+0x20840` opening `SV_SendClientGameState` and `SV_SendClientSnapshot`) | compressed block |
| client to server | 9 bytes: `serverId` byte, `messageAcknowledge` and `reliableAcknowledge` longs (`CL_WritePacket` Mac 0x188154; `SV_PacketEvent` 0x80960e2) | compressed block from byte 9 |

`SV_PacketEvent` drops a message whose `messageAcknowledge` is negative, and one whose `reliableSequence - reliableAcknowledge` is 128 or more (1.1: 64), setting the acknowledge to the sequence. VERIFIED. A datagram from an address and qport no client holds is answered with an OOB `disconnect`. VERIFIED.

### 2.3 Netchan framing

The packet header is 1.1's: a 4-byte sequence with bit 31 marking a fragment, then from the client a 2-byte qport (`Netchan_Transmit` 0x806bc6c and `Netchan_TransmitNextFragment` 0x806ba4c write it only for socket 0, the client). VERIFIED. `FRAGMENT_SIZE` stays 1300 (0x514), and a message whose length is a multiple of it still ends on a short fragment (the `!= 0x514` test at the end of `Netchan_TransmitNextFragment`). VERIFIED.

**Divergence from 1.1.** `fragmentStart` is a 4-byte long, not a short: `[u32 sequence|0x80000000][u16 qport, client only][u32 fragmentStart][u16 fragmentLength]`. VERIFIED: `Netchan_TransmitNextFragment` writes `MSG_WriteLong` then `MSG_WriteShort`; `Netchan_Process` (0x806be8a) reads `MSG_ReadLong` then `MSG_ReadShort`. The retail gamestate arrived in six fragments starting 0, 1300, ..., 6500. VERIFIED live (9). `MAX_MSGLEN` is 0x20000 (131072), up from 32768: `Netchan_Transmit` errors past it, and `Netchan_Process` drops a fragment that would grow the reassembly past it. VERIFIED.

### 2.4 The XOR scramble

The scramble is 1.1's, with the command ring doubled to 128 slots.

- Server to client: `SV_Netchan_Transmit` (0x8097626; the encoder at 0x80974d8, Mac 0x190872) scrambles from byte 4 with `key = challenge ^ outgoingSequence` (low byte), walking `lastClientCommandString` (`client+0x20844`). VERIFIED. The client decodes with its `reliableCommands[reliableAcknowledge & 127]` (`CL_Netchan_Decode`, Mac 0x18fea4, slot stride 0x400). VERIFIED.
- Client to server: `CL_Netchan_Transmit` (Mac 0x18ff40) scrambles from byte 9 with `key = challenge ^ serverId ^ messageAcknowledge`, walking `serverCommands[reliableAcknowledge & 127]`. VERIFIED. `SV_Netchan_Decode` (0x8097562, Mac 0x1907cc) runs the same from the read cursor after the header, keyed on `client+0x20810 & 0x7f`. VERIFIED. Both keys, over empty command strings, worked against retail. VERIFIED live (9).
- Both walk the string raw, `c << (i & 1)` with `i` the index within the scrambled part, wrapping at the NUL. VERIFIED (all four functions).

What changed is the string the server keys on. `MSG_ReadString` (Mac 0x1737fe) and `MSG_ReadCommandString` (0x8068522) map each byte through `I_CleanChar` (0x80b7edc, Mac 0x28700), which turns 0x92 into `'` and keeps every other byte. VERIFIED. 1.1's server reader also turned every byte over 127 into `.`. So a CoD2 client's copy of its last command keys the same as the server's after the 0x92 mapping alone. INFERRED.

## 3. Server to client messages

### 3.1 The op numbers

**Divergence from 1.1.** `svc_bad` is gone and every op moved down one. VERIFIED: the Mac `_svc_strings` table (0x34d280) reads `svc_nop`, `svc_gamestate`, `svc_configstring`, `svc_baseline`, `svc_serverCommand`, `svc_download`, `svc_snapshot`, `svc_EOF` for 0..7, and `CL_ParseServerMessage` (Mac 0x1d69fa) switches on 0 (no-op), 1 (`CL_ParseGamestate`), 4 (server command), 5 (`CL_ParseDownload`), 6 (`CL_ParseSnapshot`) and stops on 7. A retail gamestate and its snapshots parsed with these numbers to the last byte. VERIFIED live (section 9).

| op | 1.1 | CoD2 |
|---|---|---|
| `svc_nop` | 1 | 0 |
| `svc_gamestate` | 2 | 1 |
| `svc_configstring` | 3 | 2 |
| `svc_baseline` | 4 | 3 |
| `svc_serverCommand` | 5 | 4 |
| `svc_download` | 6 | 5 |
| `svc_snapshot` | 7 | 6 |
| `svc_EOF` | 8 | 7 |

`svc_configstring` and `svc_baseline` are only read inside a gamestate; at the top level they hit the `Illegible server message` error. VERIFIED. A snapshot message is the reliable acknowledge, every unsent server command, the snapshot, any download block and `svc_EOF`, in that order (`SV_SendClientSnapshot`, Mac 0x195f84). VERIFIED. On an overflowed message the server rebuilds it from the server commands alone and drops the client with `EXE_SERVERMESSAGEOVERFLOW` if that overflows too. VERIFIED. `SV_WriteSnapshotToClient` (0x80985c8) appends `sv_padPackets` (0x849f760) zero bytes, `svc_nop`s, after the client states. VERIFIED.

### 3.2 `svc_gamestate` (1)

`SV_SendClientGameState` (0x808f302, Mac 0x17d204) writes:

```
long  reliableAcknowledge            ; the plain header
<pending server commands>            ; svc_serverCommand (4) each
byte  1                              ; svc_gamestate
long  reliableSequence               ; serverCommandSequence
loop: byte 2, short index, bigstring ; every non-empty configstring, ascending
loop: byte 3, <10-bit number + entity delta from zero>   ; every baseline with a number
byte  7
long  clientNum
long  checksumFeed
byte  7                              ; the message's svc_EOF
```

VERIFIED. 1.1's body with the ops renumbered. `MAX_CONFIGSTRINGS` is 2048 and the client caps the gamestate's string data at 0x20000 bytes (`MAX_GAMESTATE_CHARS exceeded`, `CL_ParseGamestate`, Mac 0x1d5534). VERIFIED. A baseline number past 1023 is a client error. VERIFIED. After the gamestate the client runs `CL_RequestAuthorization` unless `net_lanauthorize` is 0 and the server is on the LAN. VERIFIED.

### 3.3 `svc_snapshot` (6)

The frame is 1.1's, no areamask:

```
long  serverTime
byte  deltaNum          ; 0 = from nothing
byte  snapFlags
<playerState delta>
<packet entities>       ; [10-bit number][entity delta]..., 1023 ends
<client states>         ; [1 bit][6-bit number][clientState delta]..., bit 0 ends
<sv_padPackets svc_nop bytes>
```

VERIFIED: `SV_WriteSnapshotToClient` (0x80985c8) writes the op, `svs.time`, the delta number and the flags, then the playerstate, then `0x8098308` (entities) and `0x809847c` (clients, through `MSG_WriteDeltaClient` 0x80698e8), and `CL_ParseSnapshot` (Mac 0x1d5cb2) reads them in that order with `MSG_ReadBits(10)` and `MSG_ReadBit` plus `MSG_ReadBits(6)`. `snapFlags` is built as in 1.1: the server-count bit, `| 1` when the client's last message was rate-limited, `| 2` while the client has never been active. VERIFIED. The server deltas only for a `CS_ACTIVE` client whose acknowledged frame is fewer than 29 frames old and whose entities are still in the ring. VERIFIED. Under `clc_move` the delta byte reads 1 every snapshot on loopback (the frame before), and the packet entities and client states merge against that frame's lists as 1.1's do, a new entity from its baseline and a new client state from zero. VERIFIED live (section 10). The client keeps 32 frames and 2048 entities and 2048 client states in its rings (`& 0x1f`, `& 0x7ff`). VERIFIED.

The entity, client and baseline deltas are 1.1's `MSG_ReadDeltaStruct` shape: a remove bit, a no-change bit, an `lc` byte, then `lc` fields through `MSG_ReadDeltaField` and the rest copied from the base (`MSG_ReadDeltaStruct` 0x8069d4a, Mac 0x174292). VERIFIED. The field codec under it changed; section 5.2.

### 3.4 `svc_serverCommand` (4)

`long sequence`, `string text`, 1.1's. VERIFIED. The ring is 128 slots on both ends (`(seq & 0x7f) * 0x400` on the client, `* 0x408` on the server). VERIFIED.

`SV_AddServerCommand` (0x8094750) keeps 1.1's queue rules at the new size: a client more than 64 commands behind, or not `CS_ACTIVE`, is queued only type-1 commands; a 129th unacknowledged command prints the queue, sends the client an OOB `disconnect`, defers a drop with `EXE_SERVERCOMMANDOVERFLOW` and stores `w "EXE_SERVERCOMMANDOVERFLOW"`. VERIFIED for the constants (0x40, 0x81) and the strings; which commands survive is INFERRED, off the branches, as in 1.1. The OOB `disconnect` is new.

The engine-side letters are 1.1's: `CL_GetServerCommand` (Mac 0x17a4c4) handles `B` and `n` (restart), `d` (configstring), `w` (drop; a reason `PB` reads as a PunkBuster kick) and `x`/`y`/`z` (big configstring chunks). VERIFIED. The cgame's (`CG_ServerCommand`, Mac 0x1e2ea4) switch on the first character. VERIFIED:

| letter | effect | 1.1 |
|---|---|---|
| `B` | `CG_MapRestart(0)` | same |
| `n` | `CG_MapRestart(1)` | same |
| `C <weapon>` | equipped offhand (`CG_SetEquippedOffHand`) | new |
| `D` | reverb off | new |
| `E` / `F` | channel volumes on / off | new |
| `G <n>` / `H <n>` | axis / allies team score | new |
| `I <n>` | menu notify | new |
| `J <n>` | reset player muting | new |
| `K` | close the in-game menu | new |
| `a <n>` | select weapon index | same letter |
| `b ...` | scoreboard: count (capped 64), two ints, then 5 ints a player (client, score, ping, deaths, status icon `1..8` into configstring `0x16 + n`) | reshaped |
| `c <text>` | bold announcement | |
| `d <index>` | configstring changed (handlers below) | same |
| `e`, `f` | game message | same |
| `g` | bold game message | same |
| `h`, `i` | chat, team chat (`\x19` stripped) | same |
| `o`, `p`, `q` | music play, music stop, sound fade | same |
| `r` | reverb (6 args) | new |
| `s <n>` | local sound, sound alias configstring `0x24e + n`, `n` in 1..256 | was a sound index |
| `t` | open script menu | same |
| `u` | close the script popups | same |
| `v <dvar> <value>` | client dvar from server | same |

The meanings of the `C` through `K` letters are read off the functions they call. INFERRED.

### 3.4.1 The scoreboard, chat and the client commands the game answers

`ClientCommand` (Mac 0x1a7684) compares the first argument, case folded, against `say`, `say_team`, `score`, `mr`, `give`, `take`, `god`, `demigod`, `notarget`, `noclip`, `ufo`, `kill`, `follownext`, `followprev`, `where`, `callvote`, `vote`, `setviewpos`, `entitycount` and `printentities`, in that order. VERIFIED. CoD2 still has `callvote` and `vote` (`Cmd_CallVote_f`, `Cmd_Vote_f`). VERIFIED. `kill` returns when `sessionstate` (gclient+0x26a8) is not 0 (playing), else runs `player_die(self, self, self, 100000, 12)`, 12 being `MOD_SUICIDE`. VERIFIED.

`score` is `Cmd_Score_f` (Mac 0x1a5186), which calls `DeathmatchScoreboardMessage` (Mac 0x1a4f98; `cod2_lnxded` 0x80fdf08). VERIFIED. It sends the asker `b <rows> <axis> <allies>` then ` %i %i %i %i %i` a row over `level.sortedClients` (level+0x21c, count level+0x218 capped at 64). VERIFIED. Per row:

| field | source |
|---|---|
| client number | |
| score | gclient+0x26b8 |
| ping | `SV_GetClientPing`; -1 while `pers.connected` (gclient+0x26c4) is 1, `CON_CONNECTING` |
| deaths | gclient+0x26bc |
| status icon | gclient+0x26b0, the 1-based index `ClientScr_SetStatusIcon` (Mac 0x1b9fcc) stores, configstring `0x16 + n` |

A row that would take the rows' text past 1024 bytes ends the list, and `<rows>` counts only those written. VERIFIED. The team scores are level+0x200 (axis, `G`) and level+0x204 (allies, `H`), as `GScr_SetTeamScore` (Mac 0x199fb6) writes them. VERIFIED. `SortRanks` (Mac 0x1ada9a) is 1.1's: a connecting client last, spectators (`sessionTeam` gclient+0x274c of 3) after the rest in slot order, then score down and deaths up. VERIFIED. `ClientConnect` sets `pers.connected` 1 and `ClientBegin` 2. VERIFIED (Mac 0x193a5e, 0x1928ae).

Live, two vcod probes on retail `mp_toujane` `dm`: `b 2 0 0 0 0 0 0 0 1 0 0 0 0` before any death, `b 2 0 0 0 0 0 0 0 1 -1 0 1 0` after client 1 sent `kill`. VERIFIED live. The ping reads 0 because `svs.time` steps a frame at a time and the probe acknowledges inside the frame. INFERRED. The killed client had respawned by the second reply, so its icon was 0 again.

vcod-server answers every command on the list (3.4.2). On the same probes it sent the same first reply and, after the `kill`, `b 2 0 0 0 0 0 0 0 1 -1 0 1 1`: the dead probe had not respawned, since vcod-server did not then exec `default_mp.cfg` and its `scr_forcerespawn 1` (4.2). It does now (15.4). vcod-server cuts the row text at 1024 bytes on CoD2 only (`scoreboard::text_capped`); CoD1's reply stays whole.

`h` and `i` go through `SEH_LocalizeTextMessage`, lose their `\x19`s, and go to `CG_AddToTeamChat` (Mac 0x1e1f70) and the console. VERIFIED. That function keeps 1.1's ring: `cg_chatHeight` lines (default 8, at most 8), each wrapped at 90 visible characters on the last space with the colour carried, timed by `cg_chatTime` (default 12000). VERIFIED. `CG_DrawChatMessages` (Mac 0x1cf0d0) draws them at `cg_hudChatPosition` (default 5, 150) in font 0 at 0.20833. VERIFIED. vcod's CoD2 HUD draws them there (cod2-hud.md 14). `messagemode` and `messagemode2` (`Con_ChatModePublic_f` Mac 0x15c680, `Con_ChatModeTeam_f` 0x15c6d0) open the chat field, 0 or 1 in `chat_team`; Enter sends `say` or `say_team` with the line. INFERRED, the send (1.1's field).

CoD2's localized strings number their slots `&&1` .. `&&9` (`SEH_LocalizeTextMessage`, Mac 0x31d6c): each part after the first fills the next number still in the text, and `&&0` is an error. VERIFIED. The scoreboard's count is `CGAME_SB_PLAYERS` "&&1 players" with the number as the part. VERIFIED (the string, `localized_english_iw*.iwd`).

### 3.4.2 The rest of `ClientCommand`, the votes and the console

The engine runs its own client commands first: `SV_ExecuteClientCommand` (lnxded 0x8090b14) compares argv 0, case for case (`strcmp`), against the `ucmds` table at 0x817d9e0: `userinfo`, `disconnect`, `cp`, `vdr`, `download`, `nextdl`, `stopdl`, `donedl`, `retransdl`, `wwwdl`, `muteplayer`, `unmuteplayer`. VERIFIED (the table's pointers). Anything else goes to `ClientCommand` once the level is loaded and the flood rule let it through. VERIFIED. That rule (`SV_ClientCommand`, 0x8090bac): on a client in the world with `sv_floodProtect` on, a command other than `team `, `score ` and `mr ` (prefixes, case folded) inside 800 ms of the last non-exempt one skips the game, and every non-exempt command pushes the window to now + 800. VERIFIED.

In `ClientCommand` (lnxded 0x8100e62) `say`, `say_team` and `score` come first; a client whose `pm_type` (gclient+4) is 5, the intermission, gets nothing for the rest. VERIFIED. Each reply below is the `e` print command to the asker unless said otherwise. VERIFIED, the format strings; the quoted replies VERIFIED live too (two vcod probes on retail `mp_toujane` `dm`, once under `map` and once under `devmap`).

| command | reply and effect |
|---|---|
| unknown | `e "GAME_UNKNOWNCLIENTCOMMAND\x15<argv 0 as sent>"` (`Foo bar` answered `...\x15Foo`) |
| `god`, `demigod`, `notarget` | `CheatsOk`, then `ent->flags` (gentity+0x174) `^=` 1, 2, 4 and `GAME_GODMODE_ON`/`_OFF`, `GAME_DEMI_GODMODE_ON`/`_OFF`, `GAME_NOTARGETON`/`OFF` |
| `noclip`, `ufo` | `CheatsOk`, then gclient+0x27ac or +0x27b0 toggled and `GAME_NOCLIPON`/`OFF`, `GAME_UFOON`/`OFF` |
| `kill` | nothing unless `sessionstate` is playing; god and demigod cleared, health 0, `player_die(.., 100000, MOD_SUICIDE)` |
| `follownext`, `followprev` | `Cmd_FollowCycle_f` (below); no reply |
| `where` | `e "\x15(%i %i %i)\n"`, `vtos` of the entity's `r.currentOrigin`, newline inside the quotes; any client, no cheat test |
| `callvote`, `vote` | below |
| `setviewpos x y z yaw` | `sv_cheats` only (no alive test): `GAME_CHEATSNOTENABLED`, or with other than four arguments `GAME_USAGE\x15: setviewpos x y z yaw`; else `TeleportPlayer` to z minus `viewHeightCurrent` (ps+0xf8), view (0, yaw, 0) |
| `entitycount` | under `sv_cheats` the server console's `entity count = <level.num_entities>`; nothing to the client |
| `printentities` | the server console's `%4i: '%s', origin: %f %f %f` per slot below `level.num_entities`, classname and origin, free slots `''` at 0 (`G_PrintEntities`, Mac 0x1b5b18) |
| `give`, `take` | `CheatsOk`, below |

`CheatsOk` (Mac 0x1a5190) answers `GAME_CHEATSNOTENABLED` while `sv_cheats` is 0 and `GAME_MUSTBEALIVECOMMAND` while the entity's health (gentity+0x194) is not above 0, a spectator's and a dead player's. VERIFIED, and live: a dead player's `god` and a spectator's `god` both drew `GAME_MUSTBEALIVECOMMAND`. The game's `g_cheats` is the engine dvar `sv_cheats` (`Dvar_RegisterBool("sv_cheats", 0, 0x1000)`, lnxded 0x8111795). VERIFIED.

The flags' effects: `G_Damage` (Mac 0x1c4272) passes over a client in `noclip` or `ufo` before the callback; `finishPlayerDamage` (Mac 0x1c2e32) applies the knockback, then returns for a god (`ent->flags & 1`) before the impact events and the health, and holds a demigod's damage to its health less 1. VERIFIED. `ClientEndFrame` (Mac 0x1a47a8) sets `pm_type` 2 under `noclip`, 3 under `ufo`, else the dead or living type, and clears the contents under either. VERIFIED. `notarget` is read by no MP code found. INFERRED.

`TeleportPlayer` (lnxded 0x810a708): the origin, plus 1 on z, `eFlags ^= 2` (the teleport bit), `SetClientViewAngle`, the entity rebuilt and relinked. VERIFIED. Live `setviewpos 100 200 300 90` followed by `where` a second later read `(98 328 -12)` with the probe walking off the landing.

`Cmd_Give_f` (Mac 0x1a6ac0, lnxded 0x80fe588) reads the arguments past the first as a count (`atoi`) and the whole line as the item: `all` does health, weapons and ammo; `health` sets the maximum (gclient+0x134) with a count of 0 or adds the count; `weapons` gives every weapon the slot rule allows; `ammo` adds 998 to each weapon with a full clip, or the count to the weapon in hand; `allammo <n>` the count to each; anything else is `G_FindItem` (an item's pickup or class name past the first 0x81 rows, then a weapon name) spawned and touched. VERIFIED, the branches. `Cmd_Take_f` (0x80fe9ca): `health` to 1 or down by the count to 1; `weapons` every weapon with its rounds, the hand emptied (`G_SelectWeaponIndex(.., 0)`); `ammo` every round, or the count off the weapon in hand with the clip covering what the reserve lacks; `allammo <n>` the same on each. VERIFIED. Live `give health 5` and `take all` raised the stock health script's `breathing_better` and `breathing_hurt`. VERIFIED live. vcod gives every weapon the level registered for `weapons` and fills `Add_Ammo` with 1.1's rule (reserve up to `maxAmmo`), which CoD2 is INFERRED to share.

#### The spectator follow

`Cmd_FollowCycle_f` (Mac 0x1a53b8, lnxded 0x80ff4ac) runs only for `sessionstate` spectator (gclient+0x26a8 = 2) with no forced follow (`spectatorclient`, gclient+0x26ac, negative). From `spectatorClient` (gclient+0x27a8, slot 0 when negative) it steps by the direction, wrapping at `sv_maxclients`, and takes the first slot `SV_GetArchivedClientInfo` (Mac 0x195a06) returns and `G_ClientCanSpectateTeam` (bit `1 << team` of the spectate mask gclient+0x2740 clear) allows, the start tried last. VERIFIED. At no archive delay `SV_GetArchivedClientInfo` wants the client `CS_ACTIVE` with `pm_flags` 0x800000, which `ClientEndFrame` keeps on every non-spectator, the dead included (`GetFollowPlayerState`, Mac 0x1a28d2). VERIFIED. Live, a spectator's `follownext` with one other player went onto it; a second `follownext` and a `followprev` stayed there, and the follow held through that player's death. VERIFIED live.

`SpectatorThink` (Mac 0x1a2a08), when following, not forced and free flight allowed: attack's press cycles forward, the sight button's (0x1000) back, melee (0x4) pressed or let go stops the follow. A following spectator runs no pmove. VERIFIED.

`SpectatorClientEndFrame` (Mac 0x1a2bc6). VERIFIED:

- A forced follow takes the script's client.
- A spectator not following with free flight refused (mask 0x10) cycles forward.
- A target that may still be followed has its playerstate copied over the spectator's, `pm_flags` 0x800000 off and 0x400000 on. A forced follow then clears 0x1000000 and 0x2000000; any other sets 0x1000000, and 0x2000000 too when free flight is allowed. `eFlags` 0x100000 stays the spectator's own. `HudElem_UpdateClient(ps, own number, 2)` refills the non-archived HUD elements with the spectator's own.
- Anything else runs `StopFollowing`.

Live the follower read `pm_flags` 0x3400000 over the copy and `clientNum` 0, the followed player's. VERIFIED live. `StopFollowing` (Mac 0x1a686c): after a copy, the spectator stands where an 8-unit capsule (mask 0x810011) swept from the followed eye toward 40 back and 10 up stops, looking 15 lower (`_DAT_08158eac` reads 15.0), with its own `clientNum`, `eFlags` without 0x300 and `pm_flags` without 0x400040. VERIFIED. vcod sends the followed player's playerstate as the snapshot builds it, not the copy at the follower's end frame; the two agree within a frame for a target numbered below the follower. INFERRED.

#### Votes

`g_oldVoting` (default 1) and `g_allowVote` (default 1) are game dvars, `g_voteAbstainWeight` a float of default 0.5 (lnxded 0x8111827..0x8111844). VERIFIED. Under `g_oldVoting` 0 the votes go to script (`call_vote`, `vote` notifies). Under 1 `Cmd_CallVote_f` (Mac 0x1a54dc) answers, in order: `GAME_VOTINGNOTENABLED`, `GAME_VOTEALREADYINPROGRESS`, `GAME_MAXVOTESCALLED` (gclient+0x2734 above 2, which nothing writes, INFERRED), `GAME_NOSPECTATORCALLVOTE`, `GAME_INVALIDVOTESTRING` for a `;` in any of the three arguments, and for an unknown kind `GAME_INVALIDVOTESTRING` then `GAME_VOTECOMMANDSARE\x15 map_restart, map_rotate, map <mapname>, g_gametype <typename>, typemap <typename> <mapname>, kick <player>, clientkick <clientnum>, tempBanUser <player>, tempBanClient <clientNum>`. VERIFIED. The kinds are 1.1's with `tempBanUser` and `tempBanClient` (`clientkick` and `tempBanClient` by slot, the other two by cleaned name, the last match winning); a passed `map_restart` runs `fast_restart`, `typemap` always wants its map on disk (1.5's rule). VERIFIED. The call sends everyone `GAME_CALLEDAVOTE\x15<name>`, ends at `level.time + 30000`, clears `eFlags` 0x100000 on every client and sets it on the caller, then configstrings 15 (the end), 16 (the text), 17 (yes, 1) and 18 (no, 0). VERIFIED, and live: `e "GAME_CALLEDAVOTE\x15vcodplay"`, `d 15 63150`, `d 16 GAME_VOTE_MAPRESTART`, `d 17 1`, `d 18 0` (a slot unchanged since the last vote is not resent); a kick vote's text read `GAME_VOTE_KICK\x15(1)vcodplay`.

`Cmd_CallVote_f`'s `map` and `typemap` texts are `GAME_VOTE_MAP\x15%s` (rodata 0x815924b) formatted with the argument as typed, the map's file name, not a display name; the cgame shows what configstring 16 holds. VERIFIED (the format string and its argument). vcod sends the same.

`Cmd_Vote_f` (Mac 0x1a5f8a) has no complaint arm: `GAME_NOVOTEINPROGRESS`, `GAME_VOTEALREADYCAST` (the voted bit), `GAME_NOSPECTATORVOTE`, else `GAME_VOTECAST`, the bit set and yes (first letter `y`, or second `Y` or `1`) or no counted into 17 or 18. VERIFIED, and live the caller's own `vote yes` drew `GAME_VOTEALREADYCAST`. `CheckVote` (Mac 0x1aeb2a) first runs a passed vote's command 3 s after the pass; while a vote runs a yes count of at least `n / 2 + 1` passes it and a no count above `n - (n / 2 + 1)` fails it, `n` the voting clients; at its end it passes when `(int)(ceil(abstained * g_voteAbstainWeight) + no) < yes`, else fails. Either way everyone gets `GAME_VOTEPASSED` or `GAME_VOTEFAILED` and then configstring 15 goes empty. VERIFIED, and live with two voters the second's `vote yes` drew `e "GAME_VOTECAST"`, `d 17 2`, `e "GAME_VOTEPASSED"`, `d 15 ` in one message and `B` 3.1 s later; a kick vote passed once its target's opponent left (`n` 1).

The `n` of `CheckVote` is `numVotingClients`, which `CalculateRanks` (lnxded 0x81097e6) counts as every client whose `pers.connected` (gclient+0x26c4) is 2 and whose session team (gclient+0x274c) is not 3, the spectators. INFERRED, the loop's two conditions. It has no bot test, so an `addTestClient` client counts as a voter that never votes. INFERRED, the same loop. vcod counts its `addTestClient` clients the same way. vcod's own bots (`--bots`, `addbot`) are left out, as on CoD1 (`cod11-votes.md`): with six of them a lone player's vote could only pass at its end on the abstainers' weight. That is vcod's own rule (`game::vote::voting_clients` over `RosterSlot::bot`). Under `g_oldVoting` 0 the engine counts nothing, and the stock 1.3 gametypes (`iw_07.iwd`, `iw_15.iwd`) never wait on `call_vote` or `vote`, so a bot cannot hold a script vote either. VERIFIED, the stock scripts searched.

A passed vote's command goes through `Cbuf_ExecuteText`, which splits on `;`: a `g_gametype` or `typemap` vote on the running map queues `g_gametype <type>; map_restart`, two commands. INFERRED, the buffer walk at lnxded 0x805feb6 (beside `Cbuf_ExecuteText` 0x805fe3c) ends a command at a `;` outside quotes or a newline. vcod's console drain splits it the same way (`console::split_commands`). Run as one line, the restart becomes `g_gametype`'s argument and the passed vote changes nothing.

The cgame (`CG_ConfigStringModified`, `CG_Draw2D` Mac 0x1d114a) keeps the four slots and draws 1.1's two lines while 15 is not 0: `%s(%i):%s` over `CGAME_VOTE`, the whole seconds to the end in server time and the text, at x 8 y 200, then `%s(%s):%i, %s(%s):%i` with the keys bound to `vote yes` and `vote no`, or `%s:%i, %s:%i` once `eFlags` 0x100000 is set, at y 210, font 0 at 0.20833, yellow. VERIFIED. The text's map names are swapped for their display names (`UI_GetMapDisplayNameFromPartialLoadNameMatch`) before the localize; vcod skips that and plays no sound on a change, as the cgame plays none. VERIFIED, the absence of a sound call.

#### The console

`SV_AddOperatorCommands` (Mac 0x172136) registers `heartbeat`, `onlykick`, `banUser`, `banClient`, `kick` and `tempBanUser` (both `SV_TempBan_f`), `tempBanClient`, `unbanUser`, `clientkick`, `status`, `serverinfo`, `systeminfo`, `dumpuser`, `map_restart`, `fast_restart`, `map`, `map_rotate`, `gameCompleteStatus`, `devmap`, `killserver`, and on a dedicated server `say` and `tell`. VERIFIED. Live through rcon:

- `status` prints `map: <mapname>`, `num score ping guid   name            lastmsg address               qport rate`, its dashes, then per client `%3i %5i %4i %6i <name>^7` padded to 16 drawn characters, `%7i ` the ms since its last packet, the address padded to 22 with its port printed as a signed short (`127.0.0.1:-31526`, 1.1's), `%5i` qport and ` %5i` rate; `CNCT` or `ZMBI` in place of the ping. VERIFIED. vcod prints the guid of 17.7: 0 on the LAN, as the cd-keyless probes read.
- `serverinfo` and `systeminfo` print `Server info settings:` and `System info settings:`, then each pair with the key padded to 20 and none past it (`bg_fallDamageMaxHeight480`). VERIFIED.
- `dumpuser <name>` prints `userinfo`, `--------` and the client's userinfo the same way; without a name `Usage: info <userid>`. VERIFIED.
- `say <text>` sends everyone `h "\x15console: <text>"`, the arguments joined by one space (`say hi  all` sent `hi all`), a leading quote and the last character dropped; `tell <slot> <text>` the same to one client in the world. Both print nothing back. VERIFIED.
- `kick`, `onlykick`, `tempBanUser`, `banUser` take a name (or `all`): `Usage: <cmd> <player name>\n<cmd> all = kick everyone`, `Player <name> is not on the server`; `clientkick`, `tempBanClient`, `banClient` a slot: `Bad slot number: x`, `Bad client slot: 9`, `Client <n> is not active`. VERIFIED. `kick`, `tempBanUser` and `tempBanClient` ban the guid for a while and `banUser`/`banClient` for good (17.7). The drop prints `<slot>:<name> EXE_PLAYERKICKED`, and everyone else gets `e "\x15<name>^7 \x14EXE_PLAYERKICKED"` and `J <slot>` (`SV_DropClient`, Mac 0x17e01a; a `\x14` when the reason localizes, nothing for `EXE_DISCONNECTED`). VERIFIED.
- `map <name>` without `maps/mp/<name>.d3dbsp` prints `Can't find map maps/mp/<name>.d3dbsp`. VERIFIED. An unknown command prints nothing back to rcon. VERIFIED live (`banana`).

vcod-server answers all of the above. `map` and `devmap` print the missing-map reply; `map`, `devmap`, `map_restart`, `fast_restart` and `map_rotate` run the map cycle (cod2-map-cycle.md). On the same recipe the cheat, vote, `where`, `setviewpos`, follow and console replies read as retail's (a disconnect's `MP_DISCONNECTED` goes out before `J`, 15.8), except the `Going to CS_ZOMBIE for <name>` line retail's `developer 1` adds to a kick's rcon reply. VERIFIED live.

### 3.5 `svc_download` (5)

1.1's block format with a redirect added. VERIFIED: `CL_ParseDownload` (Mac 0x1d59c8) reads a short block number; block -1 hands over to `CL_ParseWWWDownload` (Mac 0x1d57c8), which reads a URL string and two longs; block 0 reads a long size, negative meaning a refusal with a string reason; then a short length and that many bytes. A gap in the block numbers makes the client send `retransdl <n>`, new against 1.1; it acknowledges with `nextdl <n>` and aborts with `stopdl`. VERIFIED.

On the server (`SV_WriteDownloadToClient`, 0x808fdc2) blocks are 2048 bytes in a window of 8. VERIFIED (the `0x800` reads and the `< 8` window test). The window restarts from the oldest unacknowledged block once 1000 ms pass with nothing new acknowledged. INFERRED, off the `0x3e9` compare. With `sv_wwwDownload` on and a flag at `client+0x20e08` set (INFERRED: the client said it takes HTTP downloads), `SV_WWWRedirectClient` (0x808fc0c) writes op 5, short -1, the string `sv_wwwBaseURL/<file>`, a long file size and a long `sv_wwwDlDisconnected`. VERIFIED. The client answers with `wwwdl ack`, `wwwdl done`, `wwwdl fail`, `wwwdl chkfail` or `wwwdl bbl8r` (`SV_WWWDownload_f`, 0x808f97c; `CL_ParseWWWDownload`). VERIFIED. **Divergence from 1.1**, whose server streams a stock pak on request: a name `FS_iwIwd` takes for a stock iwd is refused with `cannot download IW iwd files`. VERIFIED. Which names those are is in section 11.5. The refusal block is op 5, short 0, long -1, the reason string (0x808fd64). VERIFIED.

## 4. Client to server messages

### 4.1 The clc ops

**Divergence from 1.1.** The op is 3 bits, not 2. VERIFIED: `SV_ExecuteClientMessage` (0x809114e, Mac 0x1802ac) reads `MSG_ReadBits(3)`, and `CL_WritePacket` (Mac 0x188154) writes `MSG_WriteBits(op, 3)`.

| op | meaning | body |
|---|---|---|
| 0 | `clc_move` | byte count, keyed usercmd deltas |
| 1 | `clc_moveNoDelta` | the same |
| 2 | `clc_clientCommand` | long sequence, string |
| 3 | `clc_EOF` | none |

VERIFIED (`SV_UserMove` called with 1 for op 0 and 0 for op 1; op 2 reads `MSG_ReadLong`, `MSG_ReadString`). A `clc_moveNoDelta` (1), a `clc_clientCommand` (2) and `clc_EOF` (3), each in 3 bits, were taken by the retail server. VERIFIED live (section 9). The client writes every unacknowledged reliable command, then one move op, then `clc_EOF`. VERIFIED. The server stops reading after a move op. VERIFIED (it returns after `SV_UserMove`). With `sv_pure` on, a move op drops the client with `EXE_UNPURECLIENTDETECTED` when the word at `client+0x6e6c0` (Mac offset) reads 2, and `SV_UserMove` drops it with `EXE_CANNOTVALIDATEPURECLIENT` when that word reads 0. VERIFIED. The word is the `cp` check's verdict (section 11). VERIFIED live.

Client commands keep 1.1's flood rules: `team `, `score ` and `mr ` are exempt, everything else past the first inside 800 ms is ignored when `sv_floodProtect` is on (`SV_ExecuteClientMessage`). VERIFIED. A gap in the command sequence drops the client with `EXE_LOSTRELIABLECOMMANDS`. VERIFIED. `SV_ExecuteClientMessage` keeps 1.1's serverId rules: commands are read only when the header's id equals `sv_serverid` or `client+0x20c68` (the download name, INFERRED) is non-empty; the same high nibble promotes a `CS_PRIMED` client and returns; a different one resends the gamestate once the client has acknowledged past it. VERIFIED.

### 4.2 `usercmd_t`

**Divergence from 1.1.** The struct is 28 bytes and lost `wbuttons`, `upmove` and `flags`; `buttons` is a 32-bit int and the offhand has a byte. VERIFIED, the Mac STABS `usercmd_s`:

```
i32 serverTime        ; +0
i32 buttons           ; +4
u8  weapon            ; +8
u8  offHandIndex      ; +9
i32 angles[3]         ; +12
i8  forwardmove       ; +24
i8  rightmove         ; +25
```

The button bits, from `CL_CmdButtons` (Mac 0x188ee2) and the `kb[]` entries the `IN_*` handlers press (`CL_InitInput`, Mac 0x1885ac): VERIFIED for `0x1` `+attack`, `0x4` `+melee`, `0x10` `+reload`, `0x20` `+usereload`, `0x40` `+leanleft`, `0x80` `+leanright`, `0x4000` `+binoculars`, `0x10000` `+frag`, `0x20000` `+smoke`. VERIFIED off `MSG_SetDefaultUserCmd` (0x80689a6, Mac 0x173736): `0x100` is set for a prone `eFlags`, `0x200` for a crouched one and `0x1000` for a non-zero `fWeaponPosFrac`. VERIFIED, each handler's `kb[]` slot against the slot whose active flag `CL_CmdButtons` tests (a `kbutton_t` is 0x14 bytes, its active flag at +0x10): `0x8` `+activate` (kb 0x190), `0x8000` `+holdbreath` (kb 0x12c; `+melee_breath` presses it and `+melee`'s 0x17c), `0x400` kb 0xc8, which `+moveup` and `+gostand` press (`IN_UpDown`, `IN_GoStandDown`). `0x40000` is set while the word at `cl+4` is non-zero and `cl_bypassMouseInput` is 0, and `0x800` while `cl+0x24780` is. VERIFIED, the tests; that they are talk (a key catcher) and the shellshock is INFERRED, from zk_libcod's names and 1.1's `wbuttons`. zk_libcod's `KEY_MASK_*` list agrees with every bit above.

What `CL_KeyMove` (Mac 0x18996e) adds, VERIFIED off its stores: held `+prone` (kb 0x1f4) sends 0x100 and held `+movedown` (kb 0xdc) 0x200, each with 0x2000; otherwise `cl_stance` (legacyHacks +8) 1 sends 0x200 and 2 sends 0x100, and 0x2000 is cleared. `gocrouch` sets `cl_stance` 1, `goprone` 2, and `+gostand` sets it 0 when it is not, else presses the jump (`IN_GoCrouch`, `IN_GoProne`, `IN_GoStandDown`). `togglecrouch`, `toggleprone`, `lowerstance`, `raisestance` and `+moveup` step it as 1.1's do (cod11-console.md 9; `IN_ToggleCrouch` Mac 0x187de2, `IN_ToggleProne` 0x187e12, `IN_LowerStance` 0x187d64, `IN_RaiseStance` 0x187da6, `IN_UpDown` 0x187732), and every handler that writes `cl_stance` first tests the held `+prone` and `+movedown` `active` bytes (kb 0x204 and 0xec) and does nothing when either is set. `+movedown` and `-movedown` (`IN_DownDown` 0x18943e, `IN_DownUp` 0x18948e) set `legacyHacks` +0xc, the forced-stance events' gate, to 1 while either key is held. VERIFIED, the decompile. While the snapshot's `pm_type` is 2, 3 or 4, held `+gostand` (kb 0xf0) also sends 0x400 (`CL_CmdButtons`). VERIFIED. The sight 0x1000 is sent when `cl.toggleADS` (cl +0xb, flipped by `toggleads`, cleared by `leaveads`) differs from held `+speed` (kb 0xb4), or while `PM_IsBinocularsADS` holds. VERIFIED. The move keys send 127 each (rodata 0x3262d0), scaled by the share of the frame they were held. VERIFIED.

`CL_CreateCmd` (Mac 0x189f0e) stamps `cl.serverTime`, at most 5000 past the snapshot, holds a frame's pitch change inside 90 degrees (0x3261b0), and takes `weapon` and `offHandIndex` from `cl` +0x24778 and +0x2477c, which the cgame sets every frame from `cg_weaponSelect` (cg +0x2be50) and the equipped offhand (cg +0x2be70) (`CG_DrawActive` Mac 0x1cefca, `CL_SetUserCmdValue`). VERIFIED. `CG_Respawn` (Mac 0x1e3c56) sets both from the playerstate's `weapon` and `offHandIndex`, and `CG_SetNextSnap` (Mac 0x1dee08) calls it when the playerstate's word at +0x140 or its `clientNum` changes. VERIFIED. That +0x140 is `stats[5]` is INFERRED from 1.1's layout. `weapnext` and `weapprev` (`CG_NextWeapon_f` Mac 0x1dc352) walk the two `weaponslots` (1 and 2) after `cg_weaponCycleDelay`, testing ammo and alt modes, and a new selection clears the sight toggle unless it is the old weapon's alternate (`CG_SelectWeaponIndex` Mac 0x1d7ef6, `CL_SetADS(0)`). VERIFIED.

vcod's client (`crates/client/src/play/cod2_input.rs`) sends all of the above except `+stance`'s hold timer (`IN_Stance_Down` 0x187c94), the binoculars' sight and the cycle's ammo and alt-mode tests. It builds one cmd every 8 ms of its clock (`play::cmds::CMD_MS`). Its stock binds on a CoD2 install are `default_mp.cfg`'s (`iw_00.iwd`): SHIFT `+melee_breath`, MOUSE2 `toggleads`, 1 and 2 `weapnext`, G and MOUSE3 `+frag`, 4 `+smoke`, B `+binoculars`, X `openscriptmenu ingame changeweapon`. CoD2 has no `weaponslot`: 1 and 2 both cycle, so either key swaps the rifle and the pistol. VERIFIED, the file.

`CG_CycleWeap` (Mac 0x1dbac4) does nothing unless `cg.snap` is set and the byte at `cg.snap + 0x1a` has 0x80; `CG_NextWeapon_f` / `CG_PrevWeapon_f` also want the short at `ps + 0xc` non-negative. VERIFIED, the tests. That these are `pm_flags` 0x800000 (set on every playing snapshot captured) and `pm_flags` 0x8000 clear is INFERRED from the layout (`ps` at `snap + 0xc`, `pm_flags` at `ps + 0xc`). vcod's cycle tests neither. The cycle walks `weaponslots` bytes 1 and 2 (`cg + 0x26118` is `ps + 0x554`, the field at 1364), then `CG_SelectFirstWeaponNotInSlot` for held weapons in no slot. VERIFIED.

The probe (`vcod --net-probe ... --game cod2 --probe-team allies`) pressed 2 and then turned the wheel up through the stock binds (`Shell::key_event`, the path `main.rs` routes a key down) on `mp_toujane`. VERIFIED live, both servers:

- Retail 1.3: the cmd's weapon went from 6 (`enfield_mp`) to 5 (`webley_mp`); `weaponstate` read 2 (dropping) for five snapshots, 1 (raising) for five, then 0 with `weapon` 5. The wheel brought `enfield_mp` back the same way.
- vcod-server: `enfield_mp` (4) to `webley_mp` (3) and back.

Trap in the window's input: winit hands a high-resolution wheel's notch on Wayland as several `MouseWheel` events (a `PixelDelta` per part, then the discrete notch's `LineDelta`; smithay-client-toolkit 0.19 binds `wl_seat` up to version 7). Turning every event into `MWHEELDOWN` ran `weapnext` up to eight times a notch, and an even count left the weapon as it was. `console::keys::WheelNotches` counts whole notches. That this caused the playtest's "nothing switches" is INFERRED; the windowed check is pending.

#### Script menus on a CoD2 client

`openscriptmenu <menu> <response>` (`CL_OpenScriptMenu_f`, Mac 0x1484fe) sends `cmd mr <sv_serverId> <index> <response>`, the index the first of configstrings 1246..1277 matching `<menu>` without case, -1 when none; it wants exactly two arguments and prints its usage otherwise. A menu's `scriptMenuResponse` (`Script_ScriptMenuResponse`, Mac 0x165c44) sends the same off its menu's name. VERIFIED. `v` lands in `CG_SetClientDvarFromServer` (Mac 0x1e2676): `cg_objectiveText` and `cg_drawHud` go to the cgame, `g_scriptMainMenu` is kept at `cg + 0x2adfc`, anything else is set as a dvar. VERIFIED. Esc in a game is `UI_SetActiveMenu` case 2 (Mac 0x15118e): close every menu, open the one `g_scriptMainMenu` names, else `main`. VERIFIED. The stock `ingame` menu's execKeys 1..5 answer `changeweapon`, `changeteam`, `muteplayer`, `callvote` and open `leavegame`; `_menus.gsc` (`iw_15.iwd`) answers `changeweapon` by opening the team's weapon menu, whose execKeys 1..7 answer its weapon file names (`weapon_british`: `sten_mp`, `enfield_mp`, `m1garand_mp`, `enfield_scope_mp`, `shotgun_mp`, `thompson_mp`, `bren_mp`), each button shown by `ui_allow_<weapon>`. VERIFIED, the files.

`t` is `CG_OpenScriptMenu` (Mac 0x1e216c): `CL_Popup` (0x17aeac) asks `UI_SetActiveMenu` 9 (10 with a non-empty third argument, the cursor parked at 639,479). VERIFIED. Cases 9 and 10 refuse while a menu has the focus and the active menu type (`uiInfo + 0x49c`, written by every other case) is neither, as CoD1's UI does. A refused `t` is kept; a different one waiting before it is given up with `cmd mr <its index> noop`, one of the same name left as it was; `CG_CheckOpenWaitingScriptMenu` (0x1e2314) offers it again each frame. VERIFIED. So a `t` never opens over the `ingame` menu Esc opened (type 2). INFERRED.

The stock `ingame` buttons send `scriptMenuResponse` without closing it (`iw_06.iwd` `ingame.menu`; Leave Game alone runs `close ingame`). VERIFIED, the files. `_menus.gsc` answers each with `closeMenu()`, `closeInGameMenu()`, `openMenu(...)`: `u`, `K` (`PlayerCmd_CloseInGameMenu`, 0x80fbed2, `"%c"` with 0x4b), `t <index>`. VERIFIED. The cgame's `K` arm runs `CL_CloseInGameMenu` (0x17afbe): with no fullscreen menu shown and active type 2, `UI_CloseFocusedMenu` (0x1518ae) closes every menu, which lets the `t` in. `u` (`CG_CloseScriptMenu`, 0x1e23ce) closes the same way only under type 9 and drops the waiting `t`. VERIFIED.

Live, the probe's `VCOD_PROBE_INGAME=<response>` sent `mr 16 0 <response>` 17 s in on mp_toujane dm, joined allies. Retail 1.3 and vcod-server both answered `u`, `K`, then `t 2` (`weapon_british`), `t 1` (`team_britishgerman`), `t 6` (`muteplayer`) and `t 5` (`callvote`) for changeweapon, changeteam, muteplayer and callvote, and a weapon pick's `mr` with `u`, `K`. VERIFIED live.

vcod's session (`play::cod2`) answers each script menu once with `--team` / `--weapon` (a restart or a gamestate rearms it). Every other `t`, every `u`, `K` and `v` go to the window's UI (`main.rs`, `App::cod2_script_menus`, through `frontend::cod2::menu_command` on CoD1's `play::join::Join`):

- the gamestate's menus load into the UI;
- a `t` opens its popup or waits (`frontend::cod2::offer_waiting` each frame);
- `K` is `Ui::close_in_game_menu`;
- Esc opens `g_scriptMainMenu`;
- `u` stays CoD1's Esc to the popup, which on the stock menus runs their `onEsc` `close <self>`.

`frontend::cod2`'s tests key the real `ingame` and `weapon_british` menus to `mr 16 0 changeweapon` and `mr 16 2 thompson_mp`, and click each `ingame` button through the captured `u`, `K`, `t` replies to the menu each opens. How the menus draw and take clicks in the window is a pending hand check.

Live, the probe sent X's `mr <id> 0 changeweapon` 17 s in; the server opened `t 2` (`weapon_british`) again; the probe answered `mr <id> 2 thompson_mp`; `kill` at 20 s respawned the player holding `thompson_mp`: weapon 11 on retail two seconds after the kill, weapon 9 on vcod-server. `--probe-weapon thompson_mp` spawned with it at once on retail. VERIFIED live.

Retail respawned without the use key: `FS_InitFilesystem` (0x80a325a) picks `default_localize_mp.cfg`, else `default_mp.cfg` (a 1.3 install ships only the second), and 0x8062036 runs `exec` on it, so `default_mp.cfg`'s `scr_forcerespawn 1` is what dm's `respawn()` reads. VERIFIED, the decompile and the file. vcod-server sets `default_mp.cfg`'s dvars at load too (`crates/server/src/cod2`, 15.4).

### 4.3 The usercmd delta

Keyed as in 1.1: `key = checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommands[reliableAcknowledge & 127], 32)` (`SV_UserMove` 0x8090e34, Mac 0x17fed2; `Com_HashKey` 0x80acaa4). VERIFIED. At most 32 cmds a message (`cmdCount > MAX_PACKET_USERCMDS`). VERIFIED. The first cmd deltas from `MSG_SetDefaultUserCmd` (0x80689a6), the rest each from the one before. VERIFIED. A cmd whose `weapon` or `offHandIndex` fails `BG_IsWeaponValid` (0x80ebf24) is given the playerstate's. VERIFIED.

`MSG_ReadDeltaUsercmdKey` (0x8068f9e, Mac 0x1757bc) reads, with a keyed `n`-bit field being `bits ^ (key & mask[n])` (masks `(1 << n) - 1` at 0x817d880) and a keyed angle a change bit then `short ^ key`, truncated to 16 bits:

1. `to = from` (28 bytes).
2. 1 bit: set, a byte added to `from.serverTime`; clear, a 32-bit `serverTime`.
3. Keyed bit: 0 returns the base cmd unchanged.
4. `to.buttons &= ~1`.
5. Keyed bit: 0 picks the compact branch, 1 the full one.

Compact: `key ^= serverTime`, then button bit 0 (keyed bit), `angles[0]`, `angles[1]`, and a change bit plus a keyed 4-bit forward/right code. Full: button bit 0, `angles[0]`, `angles[1]` and forward/right on the raw key, then `key ^= serverTime`, `angles[2]`, `buttons` bits 1..18 (change bit, 18 keyed bits, base `from.buttons >> 1`), `weapon` (change bit, 7 keyed bits) and `offHandIndex` (change bit, 7 keyed bits). VERIFIED. 1.1's full branch carried 6 button bits, a `wbuttons` byte, a 2-bit `up` and a 6-bit `weapon`; CoD2 drops `wbuttons` and `up` and widens the rest. The forward/right code is 1.1's (`> 10` sets bit 0, `< -10` bit 1, decoded to 127, -127 or 0; 0x8068b20, 0x8068b66). VERIFIED.

The base cmd (`MSG_SetDefaultUserCmd`): all zero, `weapon` and `offHandIndex` from the playerstate, `angles[0..1]` as `ANGLE2SHORT(viewangles) - delta_angles` (the 182.044 at 0x81414c4), `angles[2]` 0; and only when `pm_flags & 0x800000`, the stance and lean button bits off `eFlags` (8 prone, 4 crouch) and `leanf` (`0x40` below 0, `0x80` above), and `0x1000` off `fWeaponPosFrac`. VERIFIED for the stores and the `pm_flags` test.

## 5. Netfield tables and the delta codec

### 5.1 The tables

Read with `tools/re/netfields.py` at the VAs below; names come from the binary's own strings. VERIFIED for every row below, the "vs 1.1" column from `tools/re/netfield_diff.py` against `cod_lnxded` 1.1d. Wire order is table order. A bits value of `-100` or `-99` is a field codec, not a width (5.2).

| table | VA | fields | struct | 1.1 |
|---|---|---|---|---|
| entityState | 0x81417a0 | 59 | 240 B | 59 at 0x80d1760 |
| archivedEntity | 0x8141ae0 | 68 | | none |
| clientState | 0x8141f60 | 22 | 92 B | 22 at 0x80d2058 |
| playerState | 0x8142520 | 105 | 0x26a8 B | 103 at 0x80d229c |
| objective | 0x8142a20 | 6 | 28 B | 6 at 0x80de384 |
| hudelem | 0x8142b80 | 32 | 128 B | 28 at 0x80de3cc |

The struct sizes are VERIFIED off the `memset`s and strides (`0xf0` in `SV_SendClientGameState`, `0x5c` in `CL_ParseSnapshot`, `0x26a8` in `MSG_ReadDeltaPlayerstate`, `local * 7` words and `* 0x80` in the objective and hud readers).

The archived-entity table is server-side only: its reader and writer are called from `SV_ArchiveSnapshot` (Mac 0x194da8) and `SV_GetCachedSnapshotInternal` (Mac 0x1943dc), never from the snapshot writer. VERIFIED. INFERRED: it is the killcam's cached snapshots, and no client ever sees it (`cod2-killcam.md` 2).

The objective table is 1.1's, field for field. VERIFIED (same six names, offsets and bits).

#### entityState (59)

Same 59 fields as 1.1 (1.1's `_union.scale` is now `scale`), in a new order. Changed: `apos.trBase[0..2]` are `-100` (were floats), `weapon` 7 bits (was 6), `index` 10 bits (was 9).

| # | field | off | bits | vs 1.1 | # | field | off | bits | vs 1.1 |
|---|---|---|---|---|---|---|---|---|---|
| 0 | `pos.trTime` | 16 | 32 |  | 30 | `apos.trTime` | 52 | 32 |  |
| 1 | `pos.trBase[1]` | 28 | 0 |  | 31 | `solid` | 156 | 24 |  |
| 2 | `pos.trBase[0]` | 24 | 0 |  | 32 | `apos.trDelta[0]` | 72 | 0 |  |
| 3 | `pos.trDelta[0]` | 36 | 0 |  | 33 | `apos.trType` | 48 | 8 |  |
| 4 | `pos.trDelta[1]` | 40 | 0 |  | 34 | `animMovetype` | 224 | 4 |  |
| 5 | `angles2[1]` | 108 | 0 |  | 35 | `fTorsoHeight` | 228 | 0 |  |
| 6 | `apos.trBase[1]` | 64 | -100 | was 0 | 36 | `apos.trDelta[2]` | 80 | 0 |  |
| 7 | `pos.trDelta[2]` | 44 | 0 |  | 37 | `weapon` | 200 | 7 | was 6 |
| 8 | `pos.trBase[2]` | 32 | 0 |  | 38 | `index` | 140 | 10 | was 9 |
| 9 | `apos.trBase[0]` | 60 | -100 | was 0 | 39 | `apos.trDelta[1]` | 76 | 0 |  |
| 10 | `eventSequence` | 164 | 8 |  | 40 | `eventParms[0]` | 184 | 8 |  |
| 11 | `legsAnim` | 204 | 10 |  | 41 | `eventParms[1]` | 188 | 8 |  |
| 12 | `eType` | 4 | 8 |  | 42 | `eventParms[2]` | 192 | 8 |  |
| 13 | `eFlags` | 8 | 24 |  | 43 | `eventParms[3]` | 196 | 8 |  |
| 14 | `otherEntityNum` | 116 | 10 |  | 44 | `iHeadIcon` | 148 | 4 |  |
| 15 | `surfType` | 136 | 8 |  | 45 | `pos.trDuration` | 20 | 32 |  |
| 16 | `eventParm` | 160 | 8 |  | 46 | `iHeadIconTeam` | 152 | 2 |  |
| 17 | `scale` | 216 | 8 | 1.1 `_union.scale` | 47 | `time` | 84 | 32 |  |
| 18 | `clientNum` | 144 | 8 |  | 48 | `leanf` | 212 | 0 |  |
| 19 | `torsoAnim` | 208 | 10 |  | 49 | `attackerEntityNum` | 120 | 10 |  |
| 20 | `groundEntityNum` | 124 | 10 |  | 50 | `time2` | 88 | 32 |  |
| 21 | `events[0]` | 168 | 8 |  | 51 | `loopSound` | 132 | 8 |  |
| 22 | `events[1]` | 172 | 8 |  | 52 | `origin2[2]` | 100 | 0 |  |
| 23 | `events[2]` | 176 | 8 |  | 53 | `origin2[0]` | 92 | 0 |  |
| 24 | `angles2[0]` | 104 | 0 |  | 54 | `origin2[1]` | 96 | 0 |  |
| 25 | `events[3]` | 180 | 8 |  | 55 | `angles2[2]` | 112 | 0 |  |
| 26 | `apos.trBase[2]` | 68 | -100 | was 0 | 56 | `constantLight` | 128 | 32 |  |
| 27 | `pos.trType` | 12 | 8 |  | 57 | `apos.trDuration` | 56 | 32 |  |
| 28 | `fWaistPitch` | 236 | 0 |  | 58 | `dmgFlags` | 220 | 32 |  |
| 29 | `fTorsoPitch` | 232 | 0 |  |  |  |  |  |  |

#### playerState (105)

Changed: `viewangles[0..2]` are `-100`, `movementDir` -8 (was 8), `pm_flags` 27 bits (was 19), `weaponstate` 5 (was 4), `weapon` 7 (was 6). Removed from 1.1: `proneViewHeight`, `crouchViewHeight`, `standViewHeight`, `deadViewHeight`, `walkSpeedScale`, `runSpeedScale`, `proneSpeedScale`, `crouchSpeedScale`, `strafeSpeedScale`, `backSpeedScale`, `leanSpeedScale`, `friction`, `serverCursorHint`, `serverCursorHintString`, `serverCursorHintVal`, `fJumpPeak`, `iFoliageSoundTime`, `gunfx`. The mantle state, hold-breath, the view clamp, the offhand and the flinch are new.

| # | field | off | bits | vs 1.1 | # | field | off | bits | vs 1.1 |
|---|---|---|---|---|---|---|---|---|---|
| 0 | `commandTime` | 0 | 32 |  | 53 | `maxs[2]` | 1408 | 0 |  |
| 1 | `origin[1]` | 24 | 0 |  | 54 | `maxs[1]` | 1404 | 0 |  |
| 2 | `origin[0]` | 20 | 0 |  | 55 | `maxs[0]` | 1400 | 0 |  |
| 3 | `bobCycle` | 8 | 8 |  | 56 | `gravity` | 72 | 16 |  |
| 4 | `viewangles[1]` | 236 | -100 | was 0 | 57 | `damageTimer` | 144 | 16 | new |
| 5 | `origin[2]` | 28 | 0 |  | 58 | `cursorHint` | 1432 | 8 | new |
| 6 | `velocity[1]` | 36 | 0 |  | 59 | `mantleState.flags` | 1480 | 4 | new |
| 7 | `velocity[0]` | 32 | 0 |  | 60 | `flinchYaw` | 152 | 16 | new |
| 8 | `viewangles[0]` | 232 | -100 | was 0 | 61 | `fWaistPitch` | 1456 | 0 |  |
| 9 | `movementDir` | 156 | -8 | was 8 | 62 | `mantleState.timer` | 1472 | 32 | new |
| 10 | `velocity[2]` | 40 | 0 |  | 63 | `fTorsoPitch` | 1452 | 0 |  |
| 11 | `eventSequence` | 164 | 8 |  | 64 | `proneTorsoPitch` | 1420 | 0 |  |
| 12 | `legsAnim` | 124 | 10 |  | 65 | `holdBreathTimer` | 1464 | 16 | new |
| 13 | `aimSpreadScale` | 1492 | 0 |  | 66 | `jumpTime` | 112 | 32 |  |
| 14 | `weaponTime` | 52 | -16 |  | 67 | `viewangles[2]` | 240 | -100 | was 0 |
| 15 | `pm_flags` | 12 | 27 | was 19 | 68 | `foliageSoundTime` | 68 | 32 | new |
| 16 | `events[0]` | 168 | 8 |  | 69 | `weapons[1]` | 1352 | 32 |  |
| 17 | `events[1]` | 172 | 8 |  | 70 | `damageEvent` | 284 | 8 |  |
| 18 | `events[2]` | 176 | 8 |  | 71 | `damageDuration` | 148 | 16 | new |
| 19 | `events[3]` | 180 | 8 |  | 72 | `damageYaw` | 288 | 8 |  |
| 20 | `weapAnim` | 1488 | 10 |  | 73 | `proneDirection` | 1412 | 0 |  |
| 21 | `viewHeightCurrent` | 248 | 0 |  | 74 | `proneDirectionPitch` | 1416 | 0 |  |
| 22 | `torsoTimer` | 128 | 16 |  | 75 | `mantleState.yaw` | 1468 | 0 | new |
| 23 | `torsoAnim` | 132 | 10 |  | 76 | `mantleState.transIndex` | 1476 | 4 | new |
| 24 | `eFlags` | 160 | 24 |  | 77 | `fTorsoHeight` | 1448 | 0 |  |
| 25 | `fWeaponPosFrac` | 220 | 0 |  | 78 | `damagePitch` | 292 | 8 |  |
| 26 | `holdBreathScale` | 1460 | 0 | new | 79 | `jumpOriginZ` | 116 | 0 | new |
| 27 | `weaponstate` | 216 | 5 | was 4 | 80 | `pm_type` | 4 | 8 |  |
| 28 | `viewHeightTarget` | 244 | -8 |  | 81 | `viewlocked` | 1424 | 8 |  |
| 29 | `weaponDelay` | 56 | -16 |  | 82 | `weaponrechamber[0]` | 1372 | 32 |  |
| 30 | `legsTimer` | 120 | 16 |  | 83 | `vLadderVec[0]` | 100 | 0 |  |
| 31 | `viewHeightLerpTarget` | 256 | -8 |  | 84 | `weaponslots[4]` | 1368 | 32 |  |
| 32 | `groundEntityNum` | 96 | 10 |  | 85 | `weaponRestrictKickTime` | 64 | -16 | new |
| 33 | `pm_time` | 16 | -16 |  | 86 | `vLadderVec[1]` | 104 | 0 |  |
| 34 | `eventParms[3]` | 196 | 8 |  | 87 | `viewAngleClampRange[1]` | 280 | 0 | new |
| 35 | `eventParms[1]` | 188 | 8 |  | 88 | `viewAngleClampRange[0]` | 276 | 0 | new |
| 36 | `eventParms[0]` | 184 | 8 |  | 89 | `viewAngleClampBase[1]` | 272 | 0 | new |
| 37 | `eventParms[2]` | 192 | 8 |  | 90 | `weaponrechamber[1]` | 1376 | 32 |  |
| 38 | `weapon` | 212 | 7 | was 6 | 91 | `leanf` | 76 | 0 |  |
| 39 | `weapons[0]` | 1348 | 32 |  | 92 | `damageCount` | 296 | 7 |  |
| 40 | `viewHeightLerpDown` | 260 | 1 |  | 93 | `grenadeTimeLeft` | 60 | -16 |  |
| 41 | `weaponslots[0]` | 1364 | 32 |  | 94 | `deltaTime` | 1956 | 32 |  |
| 42 | `delta_angles[0]` | 84 | 16 |  | 95 | `shellshockTime` | 1500 | 32 |  |
| 43 | `delta_angles[1]` | 88 | 16 |  | 96 | `shellshockIndex` | 1496 | 4 |  |
| 44 | `cursorHintString` | 1436 | -8 | new | 97 | `shellshockDuration` | 1504 | 16 |  |
| 45 | `offHandIndex` | 208 | 7 | new | 98 | `vLadderVec[2]` | 108 | 0 |  |
| 46 | `clientNum` | 204 | 8 |  | 99 | `delta_angles[2]` | 92 | 16 |  |
| 47 | `viewlocked_entNum` | 1428 | 16 |  | 100 | `viewHeightLerpPosAdj` | 264 | 0 |  |
| 48 | `viewmodelIndex` | 228 | 8 |  | 101 | `mins[2]` | 1396 | 0 |  |
| 49 | `viewHeightLerpTime` | 252 | 32 |  | 102 | `viewAngleClampBase[0]` | 268 | 0 | new |
| 50 | `speed` | 80 | 16 |  | 103 | `adsDelayTime` | 224 | 32 | new |
| 51 | `mins[1]` | 1392 | 0 |  | 104 | `iCompassFriendInfo` | 1444 | 32 |  |
| 52 | `mins[0]` | 1388 | 0 |  |  |  |  |  |  |

#### clientState (22)

Same 22 fields, offsets and bits as 1.1 (`team` 2, `modelindex` 8, six `attachModelIndex` 8, six `attachTagIndex` 5, `name` as eight 32-bit words), in a new order: `team`, `name[0]`, `name[4]`, `modelindex`, `attachModelIndex[1]`, `attachModelIndex[0]`, `name[8]`..`name[28]`, `attachTagIndex[5]`, `attachTagIndex[0]`..`[4]`, `attachModelIndex[2]`..`[5]`.

#### hudelem (32)

| # | field | off | bits | vs 1.1 | # | field | off | bits | vs 1.1 |
|---|---|---|---|---|---|---|---|---|---|
| 0 | `y` | 8 | -99 | was 10 | 16 | `sort` | 120 | 0 |  |
| 1 | `type` | 0 | 4 |  | 17 | `text` | 116 | 8 |  |
| 2 | `color.rgba` | 32 | 32 |  | 18 | `font` | 20 | 4 |  |
| 3 | `x` | 4 | -99 | was 10 | 19 | `scaleStartTime` | 72 | 32 |  |
| 4 | `alignScreen` | 28 | 6 | new | 20 | `scaleTime` | 76 | 16 |  |
| 5 | `fontScale` | 16 | 0 |  | 21 | `fromHeight` | 68 | 10 |  |
| 6 | `materialIndex` | 60 | 8 | 1.1 `shaderIndex` | 22 | `label` | 48 | 8 |  |
| 7 | `width` | 52 | 10 |  | 23 | `fromWidth` | 64 | 10 |  |
| 8 | `height` | 56 | 10 |  | 24 | `moveStartTime` | 96 | 32 |  |
| 9 | `fadeStartTime` | 40 | 32 |  | 25 | `moveTime` | 100 | 16 |  |
| 10 | `fromColor.rgba` | 36 | 32 |  | 26 | `fromX` | 80 | -99 | was 10 |
| 11 | `fadeTime` | 44 | 16 |  | 27 | `fromY` | 84 | -99 | was 10 |
| 12 | `value` | 112 | 0 |  | 28 | `fromAlignScreen` | 92 | 6 | new |
| 13 | `time` | 104 | 32 |  | 29 | `fromAlignOrg` | 88 | 4 | new |
| 14 | `z` | 12 | -99 | new | 30 | `duration` | 108 | 32 |  |
| 15 | `alignOrg` | 24 | 4 | new | 31 | `foreground` | 124 | 1 | new |

Removed from 1.1: `alignX` and `alignY` (2 bits each), replaced by `alignOrg` and `alignScreen`. The names of entries 0, 3 and 14 are the one-letter strings `y`, `x` and `z` at 0x8142a68, 0x8142a7a and 0x8142ad9, too short for `netfields.py`'s name filter, which prints their addresses instead. VERIFIED.

### 5.2 The field codec

`MSG_ReadDeltaField` (0x8069960, Mac 0x173c78) and `MSG_WriteDeltaField` (0x80692dc, Mac 0x1778de) serve entities, client states, objectives and hud elements. Per field: a change bit (clear copies the base), then by `bits`. VERIFIED for every row:

| bits | wire after the change bit | 1.1 |
|---|---|---|
| 0 (float) | zero bit (clear = 0.0); then a bit: clear, 5 bits plus a byte, `- 0x1000`, as an integral float in [-4096, 4095]; set, a raw 32-bit float off the byte cursor | same |
| -99 | zero bit; then a bit: clear, 2 bits plus a byte, `- 0x200`, an integral float in [-512, 511]; set, a raw 32-bit float | new |
| -100 | one bit: clear = 0.0; set, a signed 16-bit short off the byte cursor, times 360/65536 (the 0.0054931640625 at Mac 0x326200); the writer sends `(short)(182.0444 * angle)` | new |
| -98 | zero bit; then 32 bits, unsigned (no table uses it) | new |
| any other | zero bit (clear = 0); then `|bits| & 7` raw bits low first, then whole bytes off the byte cursor; a negative `bits` sign-extends from bit `|bits| - 1` | 1.1 read negative widths unsigned |

So a `-100` field has no float fallback: an angle goes out quantized to 65536ths, where 1.1 sent the full float. A negative width now means signed. VERIFIED (the `| -1 << bits` after the read, `MSG_ReadDeltaField`). Retail baselines and uncompressed snapshots (playerstate, five array blocks, 27 entities, a client state), `-100` and `-99` fields included, decoded with this codec to the last byte. VERIFIED live (9).

### 5.3 The playerstate delta

`MSG_ReadDeltaPlayerstate` (0x806aaca, Mac 0x17478c) and `MSG_WriteDeltaPlayerstate` (0x806a200, Mac 0x17884a). An `lc` byte, then per field a change bit and, as in 1.1, no zero bit for floats or ints. VERIFIED:

- float: a bit; clear, the 13-bit integral float; set, 32 raw bits.
- `-100`: a bit; clear, 0.0; set, a short times 360/65536 (0x8068688, the constant at 0x8141430).
- int: `|bits| & 7` raw bits then bytes, sign-extended when `bits` is negative.

A playerstate deltaed from the acknowledged frame's, with `origin`, `velocity` and `viewangles` changing every snapshot, decoded to the last bit. VERIFIED live (section 10).

The trailing array blocks are 1.1's five in 1.1's order, at new offsets (struct `playerState_s` in the Mac STABS gives the names). VERIFIED:

1. `stats[6]` at 0x12c: gate bit, 6-bit mask, then `stats[0..2]` shorts, `stats[3]` 6 bits, `stats[4]` short, `stats[5]` byte.
2. `ammo[64]` at 0x144: a group gate, then four sub-blocks each a gate, a 16-bit mask and a short per set bit.
3. `ammoclip[64]` at 0x344: four sub-blocks with no group gate.
4. `objective[16]` at 0x5e4, stride 28: gate bit; per objective 3 raw bits of state then the six objective fields behind their own gate bit (0x8069c98).
5. hud: gate bit, then `hud.archival[31]` at 0x1728, then `hud.current[31]` at 0x7a8 (`MSG_ReadDeltaHudElems`, 0x806a0d0): a 5-bit count, then per element a 5-bit last-changed index `j` and fields `0..j`, the rest copied from the base; elements past the count are zeroed. Elements are 128 bytes.

## 6. Configstrings

What the game module and the cgame write where. VERIFIED off the `SV_SetConfigstring` / `SV_GetConfigstring` / `G_FindConfigstringIndex` call sites in `cod2_lnxded` and the configstring handlers in `CG_ServerCommand`'s `d` arm (Mac 0x1e2ea4). An index range scans from 1 when it goes through `G_FindConfigstringIndex` (0x811d300) or `G_ModelIndex` (0x811d49c): slot 0 of those ranges is never written. VERIFIED.

| index | content | writer |
|---|---|---|
| 0 | serverinfo, `Dvar_InfoString(0x404)` | 0x80969b0 |
| 1 | systeminfo, `Dvar_InfoString_Big(8)` | 0x80969b0 |
| 2 | `cod` | `SP_worldspawn` 0x811b56a |
| 3 | ambient track, `n\<track>` or empty | `SP_worldspawn`, `ambientPlay`/`ambientStop` |
| 4 | worldspawn `message` | `SP_worldspawn` |
| 5, 6 | axis, allies team score | 0x80f6818; the cgame stores them where `G`/`H` do |
| 7 | weapon names, space-separated, 1-based | 0x8106ec0 |
| 8 | registered items, one hex digit per 4 items | 0x8106f7c |
| 11 | `northyaw` | `SP_worldspawn`, `getNorthYaw` |
| 12 | fog, 7 floats | 0x80fe4bc |
| 13 | level start time, `%i` | `SP_worldspawn` |
| 14 | `g_motd` | `SP_worldspawn` |
| 15..18 | vote time, vote string, yes, no | `GScr_SetVote*`, `Cmd_CallVote_f` |
| 19, 20 | map and gametype display strings | `HECmd_SetMapNameString`, `HECmd_SetGameTypeString` |
| 22 | winning player / team | `GScr_SetWinningPlayer`, `GScr_SetWinningTeam` |
| 23..30 | status icons (8) | `GScr_PrecacheStatusIcon` 0x81134dc |
| 31..45 | head icons (15) | `GScr_PrecacheHeadIcon` 0x811367a |
| 110..141 | tag names (32) | 0x811d5cc |
| 142..237, 238..333 | server dvar mirror: 96 names, then 96 values | the cgame loop over 0x8e..0xed with `+ 0x60` |
| 334..589 | models (256) | `G_ModelIndex` |
| 590..845 | sound aliases (256) | `G_SoundAliasIndex` 0x811d668 |
| 846..909 | effects (64) | `G_EffectIndex` 0x811d600 |
| 910..1165 | effect tags (256) | `Scr_PlayFXOnTag` 0x81152d6 |
| 1166..1181 | shellshocks (16) | `G_ShellShockIndex` 0x811d634 |
| 1246..1277 | script menus (32) | `GScr_PrecacheMenu` 0x811334a |
| 1278..1309 | hint strings (32) | `G_GetHintStringIndex` 0x8112432 |
| 1310..1565 | localized strings (256) | `G_LocalizedStringIndex` 0x811d3fe |
| 1566..1693 | shaders / materials (128) | `G_ShaderIndex` 0x811d44a |

The labels in the content column for 5, 6, 8, 13, 110 and 910 are read off the writer's arguments and the cgame's handler. INFERRED. Everything else in the table is VERIFIED from the strings, dvar names or function names at the cited site. No writer for 9, 10, 21, 46..109 or 1182..1245 was found. INFERRED (none at the call sites read). `MAX_CONFIGSTRINGS` is 2048. VERIFIED (the `0x7ff` bound in `CL_ParseGamestate`, the `0x800` loop in `SV_SendClientGameState`).

Against 1.1 every range moved: 1.1's models start at 268 and its localized strings at 1244, its shaders at 1500. The 192-slot dvar mirror at 142 is the mirror `cod11-hud-protocol.md` reads at CS 140/204, at a new base.

## 7. Divergences from 1.1, in one place

1. `connect` is plain text; no Huffman on the userinfo (1.2).
2. `getchallenge` carries `0 "<cdkey md5>"`; an off-LAN challenge waits on the authorize server (1.1, 1.3).
3. `rcon_password`, not `rconPassword`; a global 500 ms rcon limit (1.6).
4. Out-of-band voice, `v` (1.7).
5. Server command and client command rings are 128 slots; the reliable window is 128 (2.2, 3.4).
6. `fragmentStart` is a long; `MAX_MSGLEN` is 0x20000 (2.3).
7. The server's string reader keeps bytes over 127 (2.4).
8. svc ops are renumbered down one, `svc_bad` gone (3.1).
9. `sv_padPackets` nops after each snapshot (3.1).
10. Download redirect to HTTP; `retransdl`; stock iwds refused (3.5).
11. clc ops are 3 bits (4.1).
12. `usercmd_t` is 28 bytes: int buttons, `offHandIndex`, no `wbuttons`, no `upmove` (4.2).
13. The field codec gains `-99` (10-bit int or float) and `-100` (16-bit angle) and reads negative widths signed (5.2).
14. Every configstring range moved (6).
15. `cp` carries no module checksums and opens with `@`; the pure lists are `sv_iwds` and kin (11).

## 8. Open questions

- The server command whose letter is a space (`  "MP_CONNECTED\x15<name>"`, 9) is `iPrintLn`'s own word, emptied by `va`'s two buffers (15.5). The retail client logs it as `Unknown client game command` (16.2). VERIFIED live. Closed.
- What the client sends in `getchallenge` when it has no CD key (a server build, a LAN game). Off loopback the client refuses to connect without one (16.1).
- The button bits marked INFERRED in 4.2, and whether 1.1's `+gostand` and stance toggles reach the wire any other way than through `0x100` and `0x200`.
- Configstrings 9, 10, 21, 46..109 and 1182..1245.
- `pm_flags` and `eFlags` bit meanings past the ones `cod2-movement.md` 12 names; the event and entity type numbering is section 13, and 12.1 names the client's entity types.
- The names of entity types 7, 8 and 9 (section 13 names them by what reads them).

## 9. The loopback session

Retail `cod2_lnxded` 1.3 on `mp_toujane` `dm` (`+set net_ip 127.0.0.1 +set net_port 28970 +set sv_punkbuster 0 +set sv_maxclients 8`) inside `unshare -rn`, loopback only, driven by a Python client written from this doc: Huffman codes from `vcod_common::net::huffman`, netfield tables from `tools/re/netfields.py`, the rest as sections 1 to 5 describe. Outside a namespace with the socket bound to 127.0.0.1 the heartbeats fail with `NET_SendPacket ERROR: Invalid argument`. Every result below is VERIFIED live.

- `getinfo xyz` with `sv_pure 1`: `infoResponse\n\challenge\xyz\protocol\118\hostname\CoD2Host\mapname\mp_toujane\sv_maxclients\8\gametype\dm\pure\1\kc\1\hw\1\mod\0\voice\0\pb\0`. With `sv_pure 0` the `pure` key is gone and `mod` reads 1: 1.4's key order, optional keys absent when unset, and its `mod` rule.
- `getstatus xyz`: `statusResponse\n` then the serverinfo in case-insensitive key order (`\sv_maxclients\8\sv_maxPing\0`), with `challenge`, `pswrd` 0 and `mod` appended, then a newline and no player lines.
- `getchallenge` and `getchallenge 0 "<32 hex>"` from loopback both came back at once as `challengeResponse <n>`, one integer, and the second got the same number as the first: the slot is found by address and reused.
- `connect "\protocol\118\challenge\<n>\qport\4660\name\vcodprobe\rate\25000\snaps\20\cl_punkbuster\0"` as plain text got `connectResponse`. The server logged a zero GUID, as 1.2 says it allows.
- A first sequenced message with `serverId` 0, `messageAcknowledge` 0 and a lone `clc_EOF` (3 in 3 bits), scrambled with `challenge ^ 0 ^ 0` over an empty command, pulled the gamestate: 7387 bytes in six fragments with 4-byte starts 0, 1300, ..., 6500 and a last length of 887 (the `sv_pure 1` run: 7685 bytes, 1185 last).
- The gamestate, descrambled with `challenge ^ sequence` and decompressed whole: `svc_gamestate` (1) first with no server command ahead of it, `serverCommandSequence` 0, 282 configstrings (op 2, ascending), 38 baselines (op 3), `svc_EOF` (7), `clientNum` 0, `checksumFeed`, and a final 7.
- `sv_serverid` in configstring 1 read 16 on the first map.
- A message with that `serverId` carrying `clc_moveNoDelta` (1) with one cmd (a 32-bit `serverTime`, then the keyed unchanged bit) and `clc_EOF` took the client from `CS_PRIMED` to `CS_ACTIVE` (server log). With `sv_pure 1` the same message dropped it with `EXE_CANNOTVALIDATEPURECLIENT`, as 4.1 says. A `clc_clientCommand` (2) `disconnect` dropped it with `EXE_DISCONNECTED`.
- Every snapshot after that decoded to its last byte: the reliable acknowledge, the queued server commands (op 4), op 6, `serverTime` stepping 50 ms, `deltaNum` 0 (no delta under `moveNoDelta`), `snapFlags` 4, a playerstate with `lc` 81, five array blocks, 27 entities to 1023, one client state, and `svc_EOF`, with nothing left over. Before the client was active (the `sv_pure 1` run) `snapFlags` read 6. The playerstate carried `viewangles[1]` 33.99719 (`-100`, a short times 360/65536), `pm_type` 4, `speed` 400, `pm_flags` 0x1000000, `stats[3]` 63 and `stats[5]` 1; the hud blocks carried `-99` `x`/`y` values (8.0, -12.0, 28.0).
- The first server commands: 31 `v <dvar> "<value>"`, then ` "MP_CONNECTED\x15vcodprobe^7"` (the first byte is a space, 0x20), `v ui_allow_weaponchange "0"`, `t 4` (script menu 4, `serverinfo_dm` at 1250), `v cg_objectiveText ""`, `v g_scriptMainMenu "ingame"` and two more `v`.

The configstrings, against section 6's table:

| index | captured |
|---|---|
| 0 | serverinfo (`\g_antilag\1\g_gametype\dm\gamename\Call of Duty 2...`) |
| 1 | systeminfo (`\bg_fallDamageMaxHeight\480...`, `sv_serverid` 16) |
| 2 | `cod` |
| 3 | `n\ambient_africa\t\0` |
| 7 | `defaultweapon_mp mg42_bipod_stand_mp frag_grenade_british_mp ...` |
| 8 | `fffff3000000000000000000000000000` |
| 11 | `0` |
| 12 | `0 1 0.00015 0.9 0.95 1 0` |
| 13 | `0` |
| 22 | `\winner\0` |
| 23, 24 | `hud_status_dead`, `hud_status_connecting` |
| 31 | `talkingicon` |
| 142..333 | dvar names from 142 (`bg_aimSpreadMoveSpeedThreshold`, `bg_bobAmplitudeDucked`, ...), their values from 238 (`11`, `0.0075`, `0.03`, ...) |
| 335..430 | `xmodel/defaultweapon`, `xmodel/prop_doorknob1`, ..., `xmodel/weapon_kar98_scoped` |
| 591.. | `weap_mg42_loop`, `weap_mg42_loop_plr`, ... |
| 847.. | `fx/explosions/flak88_explosion.efx`, ... |
| 1167 | `default` |
| 1246..1255 | `ingame`, `team_britishgerman`, `weapon_british`, `weapon_german`, `serverinfo_dm`, `callvote`, `muteplayer`, `quickcommands`, `quickstatements`, `quickresponses` |
| 1278, 1279 | `PLATFORM_HOLD_TO_USE`, `PLATFORM_HOLD_TO_DROP` |
| 1311..1318 | `PLATFORM_PRESS_TO_SPAWN`, `MP_AMERICAN`, ..., `MP_SLASH` |
| 1567..1575 | `mpflag_british`, `mpflag_german`, `mpflag_spectator`, `black`, `hudicon_british`, ... |

So the ranges filled through `G_FindConfigstringIndex` and `G_ModelIndex` (models, sound aliases, effects, shellshocks, localized strings, materials) start at their base plus 1, and the ones the script precache builtins fill (status icons, head icons, menus, hint strings) start at their base. Configstring 12 is seven numbers, as `CG_ParseFog`'s `sscanf` reads it. Nothing was written at 9, 10, 21, 46..109 or 1182..1245 on this map.

## 10. vcod's spectator

`crates/cod2/examples/spectate.rs` against the same retail server and map (`+set sv_pure 0`), inside `unshare -rn`, with a second instance (`--join --fly`) as a player. The client is written from sections 1 to 5: `crates/cod2/src/net/` holds the six netfield tables (generated from `tools/re/netfields.py` at the VAs of 5.1), the playerstate delta, the keyed usercmd delta and the parser; the field codec, message reader and netchan are vcod-common's (`FieldCodec::COD2`, `NetchanParams::COD2`: 4-byte fragment starts, 128-slot rings, a raw key string). Every result below is VERIFIED live. A 30 s rerun on the shared codec and netchan parsed 592 messages, 588 delta snapshots, with no parse error.

- A 62 s session: 1235 messages, 1228 of them delta snapshots against the frame acknowledged in the `clc_move` header, every one parsed to `svc_EOF` with nothing left. `snapFlags` 4, 28 entities, 2 client states, the spectator's own `origin` circling at z 456 and the other client's entity (number 0, `eType` 1) walking.
- Nothing is left past the parse: re-compressing the bytes the parse consumed reproduces the server's compressed block byte for byte, so the one decoded byte past `svc_EOF` is the Huffman walk over the block's zero padding bits.
- The server took `clc_move` (0) with one cmd per packet keyed as 4.3 says: the compact branch, `forward_move` 127 through the 4-bit code and `angles[1]` changing every cmd, moved the spectator forward and round. The first cmd of a packet deltas from `MSG_SetDefaultUserCmd` of the client's newest playerstate.
- Before the gamestate the server sent a 9-byte message every frame (sequence, `reliableAcknowledge`, one block byte), 1.1's bare message to a client not in the world.
- Without `+set sv_pure 0` the default server dropped a client that sent no `cp` with `EXE_CANNOTVALIDATEPURECLIENT` at its first `clc_move` (4.1). The client sends `cp` (11.4) and stays.
- `mr <serverId> <menu> <response>`, `<menu>` the script menu's index from 1246 (the handler at 0x8100d16 reads `argv[1]` against `sv_serverId` and the menu's configstring at `0x4de + n`), joined the second client: `mr 16 1 autoassign` to `team_britishgerman`, the server answered `t 2` (`weapon_british`), and `mr 16 2 enfield_mp` spawned it.

vcod's client (`vcod --net-probe ADDR --game cod2 --probe-team allies`, `play::cod2::Cod2Session`) ran the same lifecycle against `cod2_lnxded` with `sv_pure 1` and a `--join --fly` player beside it:

- The join is three menus in a row. The server opened `t 4` (`serverinfo_dm`); `mr 16 4 close` brought `t 1` (`team_britishgerman`), `mr 16 1 allies` brought `t 2` (`weapon_british`), and `mr 16 2 enfield_mp` spawned the client: the next playerstate read `pm_type` 0, and walking forward moved `origin` about 1000 units over the session. `axis` opens `t 3` (`weapon_german`); sd runs the same three (`serverinfo_sd`). VERIFIED live.
- rcon `map mp_carentan` sent an OOB `loadingnewmap\nmp_carentan\ndm`, then a new gamestate with `sv_serverid` 32 while the client still sent 16; after its `cp` the three menus came again under the new id (`team_americangerman`, `weapon_american`), and the mp_carentan snapshots parsed with no error. VERIFIED live.
- rcon `fast_restart` on dm sent `B` and a `d 1` with `sv_serverid` 33, then the three menus (persist 0 dropped the team). rcon `map_restart` sent `loadingnewmap` and a full gamestate for the same map, id 49. VERIFIED live.
- On sd with `scr_sd_roundlength 0.5` each round end sent `n` and moved the id 16, 17, 18, 19, and the client stayed spawned with no menu between rounds. VERIFIED live.
- rcon `clientkick 0` sent `w "EXE_PLAYERKICKED"`, rcon `quit` `w "EXE_SERVERQUIT"`; a server stopped with SIGSTOP timed the client out on a shortened `cl_timeout`. VERIFIED live.

`crates/cod2/tests/fixtures/mp_toujane-spectate.bin` is the first 40 server messages of such a session (`--save --save-messages 40`): the out-of-band exchange, sixteen bare messages, the gamestate in seven fragments and 23 snapshots, 22 of them deltas, with the client's packets between them. `crates/cod2/tests/replay.rs` decodes it to the last bit in CI.

## 11. Pure servers: `cp`, the iwd lists, downloads

`sv_pure` defaults to 1 (`SV_Init`, 0x8093adc: `Dvar_RegisterBool("sv_pure", 1, 0x100c)`). VERIFIED. So a stock server drops a client that never sends `cp`.

### 11.1 The iwd checksums

Each iwd's two checksums are 1.1's, over the same input. VERIFIED: the zip loader (0x80a0d5e, the call in `FS_AddIwdFilesForGameDirectory` 0x80a241c) collects the central directory CRC-32 of every entry whose uncompressed size is not 0 (`unz_file_info` +0x14 and +0x1c), stores `Com_BlockChecksum` (0x8067b02) of them at `iwd+0x304` and the keyed one (0x8067b5e: the feed's 4 bytes, then the CRCs) at `iwd+0x308`. The digest is MD4 folded by XOR of its four words: the init at 0x80670d4 sets MD4's (and MD5's) four state words, and the binary carries MD4's round constants `0x5a827999` and `0x6ed9eba1`. INFERRED from those constants. vcod's `pak_checksum` (1.1's MD4) reproduces the server's `sv_iwds` for all sixteen stock iwds (`crates/cod2/tests/pure.rs`). VERIFIED live. The feed is `fs_checksumFeed`, the gamestate's `checksumFeed`. VERIFIED (`SV_SpawnServer` passes its random feed to `FS_Restart`, and the gamestate carries it, section 3.2).

### 11.2 Which iwds count

A search path entry is 20 bytes: next, iwd, directory, a localized flag at +0xc and a language at +0x10. VERIFIED (`FS_AddIwdFilesForGameDirectory`, `FS_AddGameDirectory` 0x80a27ae). `localized_*.iwd` files, and every iwd of a localized folder, carry the flag. VERIFIED. Every list below skips flagged entries, so `localized_*` iwds are in no pure list on either side. VERIFIED (each walk tests `searchpath[3] == 0`). The `iw_*` iwds count like any other.

### 11.3 The systeminfo lists

`SV_SpawnServer` sets, after the map and game load (VERIFIED, the tail before `SV_SaveSystemInfo`):

| cvar | value | source |
|---|---|---|
| `sv_iwds` | unkeyed checksum (`+0x304`) of every non-localized iwd, `"%i "` each, search order (highest priority first); `""` when `sv_pure` is 0 | `FS_LoadedIwdChecksums` 0x806590a |
| `sv_iwdNames` | their bare names (`iw_15`), space separated; `""` when not pure | `FS_LoadedIwdNames` 0x8065982 |
| `sv_referencedIwds` | unkeyed checksum of every iwd a file was read from (`iwd+0x310`) or whose game directory is not `main`, localized ones included | `FS_ReferencedIwdChecksums` 0x8065a86 |
| `sv_referencedIwdNames` | the same as `<game>/<name>` | `FS_ReferencedIwdNames` 0x8065b32 |

With `sv_pure` on and no iwd loaded the server prints `WARNING: sv_pure set but no IWD files loaded`. VERIFIED. 1.1's `sv_paks`, `sv_pakNames`, `sv_referencedPaks` and `sv_referencedPakNames` are these four renamed. VERIFIED (strings at `SV_Init`). On the stock 1.3 install, `mp_toujane` `dm` (VERIFIED live): `sv_iwds` held the sixteen `iw_15` .. `iw_00` sums, `sv_iwdNames` `iw_15 iw_14 ... iw_00`, `sv_referencedIwds` six sums and `sv_referencedIwdNames` `main/iw_15 main/iw_13 main/iw_08 main/iw_07 main/iw_06 main/iw_04`.

The client hands `sv_iwds` and `sv_iwdNames` to `FS_PureServerSetLoadedIwds` (Mac 0x4706a, Linux 0x8065d9a), which keeps the pairs (a count mismatch is `iwd sum/name mismatch`) and prints `Connected to a pure server.` VERIFIED. That the client then reads only iwds on that list is INFERRED (1.1's lineage; the list is what `FS_CompareIwds` checks against).

What the load reads that flags an iwd, against vcod's (VERIFIED live, both maps of 15, `cod2::wire_ab`):

- `SV_SpawnServer` (0x8093520) calls `Com_LoadSoundAliases("maps/mp/<map>.d3dbsp", "all_mp", 2)` after `CM_LinkWorld` and before `SV_InitGameProgs`. VERIFIED. It lists `soundaliases/*.csv` and reads each through `FS_ReadFile` (0x80ae90c, `"soundaliases/%s"`). VERIFIED. That is what flags `iw_06`: its `soundaliases/iw_sound2.csv` (with `volumemodgroups.def`, which the first csv read loads once a process) is the only file of it a dedicated server opens; `iw_07`'s csvs and `iw_15`'s `iw_multiplayer2.csv` are flagged the same way. vcod: `Cod2Server::load` touches every `soundaliases/*.csv` before the level.
- The dedicated's xmodel surface loader (0x80c7436, reached from 0x80c7450) is a stub that returns 0 and reads nothing. VERIFIED. So no `xmodelsurfs/` read flags an iwd: `iw_14`, which holds only meshes, is never referenced, while `iw_13` (the same models' `xmodel/` and `xmodelparts/`) is. vcod loads whole models for collision and bones; `Pk3Fs::set_surfs_unflagged`, on for the CoD2 server, keeps those reads from flagging.

### 11.4 The `cp` command

**Divergence from 1.1.** No module checksums: CoD2 has no cgame or ui DLL, and the string opens with `@`. VERIFIED.

The client builds it after `CL_InitCGame`, in `CL_Vid_Restart_f` (Mac 0x14b466) and `CL_DownloadsComplete` (Mac 0x14d368), and queues it as a reliable command. VERIFIED. The name is hidden: the format is `"Va "` and the first two bytes are raised by 13 and 15, giving `cp `. VERIFIED (both Mac sites; `CoD2MP_s.exe` holds the same `"Va "` at 0x5a3b7c, pushed once at 0x40d68e). The body is `FS_ReferencedIwdPureChecksums` (Linux 0x8065c36, Mac 0x46f24). VERIFIED:

    cp @ <sum> <sum> ... <key>

- each `<sum>` is the keyed checksum (`+0x308`) of a non-localized iwd a file was read from (`+0x310`), `"%i "`, in search order;
- one more number, a global the client clears in `FS_PureServerSetLoadedIwds`, follows when it is non-zero; so never on a pure server. VERIFIED; what else sets it is not traced;
- `<key>` is `checksumFeed` XOR every sum XOR their count, `"%i "`, so the string ends in a space.

The server's handler is `SV_VerifyIwds_f` (0x8090534, Mac 0x17d714), the `cp` row of the client command table (`.data` 0x817d9f0, the pair `"cp"`, 0x8090534). VERIFIED. It passes when (VERIFIED, read off the function):

1. there are at least two arguments and the first starts with `@`;
2. no sum appears twice;
3. every sum is among the server's keyed checksums of its non-localized iwds (0x8065a0e, the keyed twin of `FS_LoadedIwdChecksums`);
4. the last number equals the server's feed XOR the sums XOR their count.

Pass writes 1 to `client+0x6e6c0`, fail 2; `vdr` (0x80907ba) writes 0. VERIFIED. There is no lower bound on the list: `cp @ <feed>` with no sums passes. VERIFIED live.

Live (`crates/cod2/examples/spectate.rs`, retail with `+set sv_pure 1` before the map, recipe in 10; VERIFIED live):

- With feed 494263159 and `mp_toujane.d3dbsp` read from `iw_04`, vcod sent `cp @ -1059352459 -575780093 `; the server took the client to `CS_ACTIVE` and kept it 30 s (591 messages, 588 delta snapshots) until the client's `disconnect`.
- The bare `cp @ <feed> ` also stayed to `CS_ACTIVE` (8 s run).
- A wrong key (`cp @ <!feed> `) dropped the client with `EXE_UNPURECLIENTDETECTED` at its next move, so the word at `+0x6e6c0` is this check's verdict.

vcod builds it with `vcod_cod2::net::pure::pure_command(fs, checksum_feed, systeminfo)`: the referenced non-localized iwds of the client's `Pk3Fs`, filtered to the ones `sv_iwds` lists when the server is pure, keyed with the gamestate's feed. The caller marks what the level read first (at least `maps/mp/<map>.d3dbsp`) and queues the string as a reliable command before its first `clc_move`. `verify_cp` is the server side.

### 11.5 Downloads

The client asks for what it lacks with `FS_CompareIwds` (Linux 0x80654a0, Mac 0x47b14) over `sv_referencedIwds` / `sv_referencedIwdNames` (VERIFIED: `Need iwds: %s`), skipping a name `FS_iwIwd` takes and any name containing `_svr_` (`FS_svrIwd` 0x8065450). VERIFIED. For a download it writes `@<game>/<name>.iwd@<game>/<name>.iwd`, the local name `<name>.<%08x sum>.iwd` when a file of that name exists with the wrong checksum. VERIFIED.

`FS_iwIwd(name, "main")` (0x8065324, Mac 0x46a42) is true for (VERIFIED):

- `main/iw_00` .. `main/iw_24`, compared whole by `FS_FilenameCompare`, so without the `.iwd`;
- a name whose part up to `localized_` is `main/localized_` and whose rest contains `_iw00` .. `_iw24`, case folded.

The server's download path (`SV_WriteDownloadToClient` 0x808fdc2) runs `FS_iwIwd(name, "main")` on the requested name, which carries `.iwd`, and refuses a hit with `clientDownload: %d : "%s" cannot download IW iwd files` and the reason `EXE_CANTAUTODLGAMEPAK\x15<name>` (op 5, short 0, long -1, the string). VERIFIED. So `main/localized_*_iwNN.iwd` is refused by the server, while `main/iw_NN.iwd` would pass the server's test (the whole-name compare against `main/iw_NN` fails) and is only kept off the wire by the client never asking for it. VERIFIED live (17.5: `download main/iw_00.iwd` got block 0 with size 162248089). Other refusals: `sv_allowDownload` 0 gives `EXE_AUTODL_SERVERDISABLED_PURE\x15<name>` with `sv_pure` on, else `EXE_AUTODL_SERVERDISABLED\x15<name>`; a missing file `EXE_AUTODL_FILENOTONSERVER\x15<name>`. VERIFIED.

HTTP redirect: `sv_wwwDownload` (0), `sv_wwwBaseURL` (""), `sv_wwwDlDisconnected` (0), all registered with 0x1001. VERIFIED. The client opts in with userinfo `cl_wwwDownload` non-zero (`SV_UserinfoChanged` 0x80907cc sets `client+0x20e08`). VERIFIED. The redirect's wire is 3.5, the `wwwdl` replies 17.5. The redirect skips the `FS_iwIwd` test, which runs only on the UDP path. VERIFIED (it returns before that branch).

vcod's CoD2 client downloads over UDP; 17.5 has the transfer and both sides.

## 12. Entities on the client

What the cgame draws for each snapshot entity, read in the Mac build's `CG_*` functions. vcod's reading is `crates/client/src/cod2_scene.rs` (what draws), `vcod_cod2::net::Frame::entity_place` (where, the one lerp the client runs) and `crates/cod2/src/trajectory.rs`.

### 12.1 `eType`

VERIFIED: `CG_ProcessEntity` (Mac 0x164bd4) switches on `eType` 0..9 and `Com_Error`s `Bad entity type` past that; `CG_CheckEvents` (Mac 0x1e614a) treats `eType > 10` as an event entity and dispatches `eType - 10` to `CG_EntityEvent`. 1.1's numbering (`ET_MOVER` 5, `ET_PORTAL` 6, `ET_FX` 10, `ET_TURRET` 11) does not carry over.

| eType | handler | draws |
|---|---|---|
| 0 | `CG_General` 0x164354 | the xmodel at `index` in the model range |
| 1 | `CG_Player` | the player path |
| 2 | `CG_Corpse` 0x1e78ea | the player path, off the client's info |
| 3 | `CG_Item` 0x164550 | `cg_items[index]`'s xmodel |
| 4 | `CG_Missile` 0x1646da | the `weapon`'s projectile model, from `time` on |
| 5 | none | nothing |
| 6 | `CG_ScriptMover` 0x164972 | inline model `index` when `solid` is 0xffffff, else the xmodel at `index` |
| 7 | `CG_SoundBlend` 0x16351c | nothing; a sound |
| 8 | inline | a looping effect, no model |
| 9 | inline | the xmodel at `index`, posed by `CG_mg42_DoControllers`, lit 32 units above its origin |

VERIFIED for the handlers, the model fields and the turret's `+ 32.0` (Mac 0x3263e8). The `eType` 9 = turret label is INFERRED: its tree comes from `CG_GetMG42Anims` (`CG_PreProcess_GetDObj`, Mac 0x163f12). Every model handler but the turret's skips the entity when `eFlags & 0x20` is set; the turret arm checks the same bit. VERIFIED. The model range is `G_ModelIndex`'s, 334 + `index` (section 6). VERIFIED live: on the fixture's mp_toujane the three `eType` 9 entities read `index` 39, `xmodel/weapon_mg42`, the two xmodel script movers 51, `xmodel/german_artillery_flak88_destroy`, and the three brush movers inline models 1, 16 and 18.

`bg_itemlist` rows 1 to 128 are the weapons by their CS 7 index: `BG_FindItemForWeapon` (Mac 0x69cd2) returns row `weapon`, and `BG_GetWeaponIndexForName` (Mac 0x7ede0) fills that row from the weapon def it loads, its model word included. VERIFIED. `G_FindItem` (Mac 0x69cea) searches the other items from row 0x81. VERIFIED. `bg_numItems` is 131 (`cod2_lnxded` rodata 0x8154ed8), and `bg_itemlist` (.data 0x81850c0, rows of 0x2c bytes: classname, pickup sound, two world models, icon, pickup name, quantity, `giType`, `giTag`) holds two rows past the weapons: 129 `item_health_small` (`xmodel/health_small`, `icons/iconh_small`, quantity 10) and 130 `item_health_large` (`xmodel/health_large`, quantity 50), both `giType` 3. VERIFIED. `CG_Item` `Com_Error`s on an `index` at or past `bg_numItems` and draws `cg_items[index]`'s first model, which `CG_RegisterItemVisuals` (Mac 0x1d9580) registers off the row's world model word. VERIFIED. vcod draws rows 129 and 130 as their two xmodels and anything past them as nothing.

### 12.2 `trType`

VERIFIED: `BG_EvaluateTrajectory` (Mac 0x6b262) has nine arms, 0..8: stationary, interpolate and 6 return `trBase`; 2 linear; 3 linear stopping at `trTime + trDuration`, never before `trTime`; 4 `sin(2 pi (t - trTime) / trDuration)` (the double pi at Mac 0x340c08); 5 gravity, `z - 400 t^2` (Mac 0x326290); 7 accelerate and 8 decelerate along `trDelta` at `|trDelta| / trDuration` (the 0.5 and -0.5 at Mac 0x326190 and 0x3261f4). 1.1's `TR_GRAVITY_LOW` and `TR_GRAVITY_FLOAT` are gone, so the accelerations moved down two. Naming 6 `TR_GRAVITY_PAUSED` is INFERRED from its place in 1.1's list.

`CG_CalcEntityLerpPositions` (Mac 0x163c40) lerps an entity between the two snapshots when its older state's `pos.trType` is 1, or 3 with a number under 64 (`CG_InterpolateEntityPosition`, Mac 0x1636d2: each state evaluated at its own snapshot's time, angles through `LerpAngle`). Anything else evaluates the older state's `pos` and `apos` at the render time, then `CG_AdjustPositionForMover` (Mac 0x163b00) adds what an `eType` 6 entity numbered 1..1021 in `groundEntityNum` moved between the older snapshot's time and the render time. VERIFIED.

### 12.3 Teleports and entities entering

`CG_SetNextSnap` (Mac 0x1dee08) copies each entity of the new snapshot into `cent->nextState` (`centity + 0xf0`; the older state sits at `+0`) and runs `CG_ResetEntity` (Mac 0x1deb10), which copies the new state over the older one and re-evaluates `lerpOrigin` and `lerpAngles` at the render time, when the entity was not in the snapshot before or when `(older eFlags ^ newer eFlags) & 2`. VERIFIED. Bit 2 (`EF_TELEPORT_BIT`, vcod's name) is flipped by `TeleportPlayer` (0x810a708), `PlayerCmd_setOrigin` (0x80fa838), `ClientScr_SetSessionState` (0x80f7efc), `ClientSpawn` (Mac 0x1930a6, which keeps `eFlags & 0x100002` and then flips it) and the body queue's slot reuse (0x811eaa4, `& 2 ^ 2` into the reused entity). VERIFIED. vcod's `Frame::entity_place` takes the newer state as the current one in both cases (`vcod_cod2::net::view::teleported`), so neither a lerped body nor a trajectory starts from a stale state. The playerstate arm of the same function builds the followed client's entity off the playerstate and resets it on the same bit test. VERIFIED. That the eye should therefore not lerp across a flip of the playerstate's bit 2 is INFERRED from that reset; `Frame::eye` holds the newer eye then.

### 12.4 Mounted guns

`CG_mg42_DoControllers` (Mac 0x1631ac) turns `tag_aim` and `tag_aim_animated` by (pitch, yaw, 0) and `tag_flash` by (`angles2[2]`, 0, 0) through `DObjSetControlTagAngles`. VERIFIED: the three tags are `scr_const` +0x9e, +0xa0 and +0x8c, allocated as `tag_aim` (0x1ac8f1e), `tag_aim_animated` (0x1ac8f20) and `tag_flash` (0x1ac8f0c) by `GScr_LoadConsts` (Mac 0x1cb694). Pitch and yaw are `nextState.angles2[0..2]` (`centity + 0x158`, `+0x15c`; `angles2` is entity state offset 0x68) passed through `LerpAngle(a, a, frac)`, so the newest state, never lerped. VERIFIED. When the predicted playerstate has `eFlags & 0x300` and its `viewlocked_entNum` (ps 0x594, `cg + 0x26158`) is the gun, pitch and yaw are instead `AngleSubtract` of the view angles (`cg + 0x285c8`, `0x285cc`) and the gun's `lerpAngles`. VERIFIED for the test and the subtraction; that those two `cg` words are the drawn view angles is INFERRED. The tree (`CG_GetMG42Anims`, Mac 0x1630e4) is `root` over the weapon's two anims, `XAnimSetCompleteGoalWeightKnobAll` blending to anim 2 when `eFlags & 0x40` and the gun is not the view's, else to 1, over 0.1 s. VERIFIED. The stock `mg42_bipod_stand_mp` names `idleAnim` `standMG42gun_aim_foward` and `fireAnim` `standMG42gun_fire_foward`. VERIFIED (iwd weapon file). vcod reads the two words as the weapon file's `idleAnim` and `fireAnim` (INFERRED from the stock file; `CG_GetMG42Anims` reads weapon def +0x18 and +0x20) and poses through CoD1's `turret::apply_controller`, the same three tags in the same order.

On the fixture's mp_toujane the three unmanned guns carry `angles2` (-81, 0, 0), (-57, 0, 0) and (-57, 0, 0). VERIFIED live. Turret info `+0x1c` is `dropPitch` (the Mac build's stabs type `turretInfo_s`: `inuse`, `flags`, `fireTime`, `arcmin[2]`, `arcmax[2]`, `dropPitch` at bit 224, `pitchCap` at bit 480). VERIFIED. `G_SpawnTurret` (Mac 0x1bc84a) stores -90 there (0xc2b40000). VERIFIED. `turret_think_init` (Mac 0x1bc1f2) then tries the pitches `i * -3.0` (literal at Mac 0x326564) for `i` 0..30. For each one it turns the vector from `tag_aim` to `tag_butt` (`scr_const` +0xa4, allocated as `tag_butt` at 0x1ac8f24) by that pitch about `tag_aim`, both tags in model space, places the turned vector through the entity's `AnglesToAxis`, and runs `G_LocationalTrace` from `tag_aim` to the turned butt under mask 0x811. The first pitch whose fraction is under 1 becomes `dropPitch`. VERIFIED. While no client holds the gun, `turret_think` (Mac 0x1bbce2) steers `angles2[0]` toward `dropPitch` and `angles2[1]` toward 0, 200 * 0.05 = 10 degrees a frame (the 0.05 at Mac 0x3262e4). VERIFIED for the targets and the rate.

So an unmanned gun tips back on its stand until its butt meets the world, barrel pitched up by `dropPitch`. The pitch is absolute in the entity's frame; negative is nose up, as `AnglesToAxis` reads it. VERIFIED. vcod's clip map reproduces the search: mp_toujane's three `misc_mg42` spots give -81, -57 and -57, the wire's values; mp_carentan's MG42s give -63 (ground floor) and -69 (upstairs window, the butt on the floor below the sill). VERIFIED (`cod2_scene` tests `retail_toujane_guns_rest_on_their_butts`, `carentan_mg42s_rest_on_their_butts`). `weapon_mg42`'s bipod mesh skins to `tag_pivot`, which the controllers leave alone; the gun's body skins to `tag_aim_animated` (the xmodel's vertex bone ids). VERIFIED. So the bipod stays level within the idle clip's 1 degree and only the gun's body turns. `CG_ProcessEntity`'s turret arm builds the model's axis from `lerpAngles` alone (`AnglesToAxis` of `centity + 0x1f8`), with no pitch of its own. VERIFIED.

`CG_Player` (Mac 0x1e6834) re-places a gunner off his turret: when the body has `eFlags & 0x300`, `otherEntityNum` (es 0x74) names an entity 64..1022 and the client's legs anim record carries flag 4, it reads the gun's `tag_weapon` and the gunner's turret anim tree's deltas and writes the body's `lerpOrigin` and `lerpAngles`, then traces it down (mask 0x2810011). VERIFIED for the conditions and the writes. vcod: `cod2_scene::gun_ridden` finds the gun (`eFlags & 0x300`, `otherEntityNum` 64..1022, an `eType` 9 in the snapshot), and `vcod_cod2::player::gunner_placement` hangs the body off the gun's `tag_weapon` through its turret anim group, the server's placement (`turretpose::place_gunner`). The trace down is left out: the snapshot's origin already carries the server's.

### 12.5 Corpses and the weapon in hand

`CG_Player` draws nothing when `eFlags & 0x20` or `eFlags & 0x20000` (dead); `CG_Corpse` (Mac 0x1e78ea) skips only 0x20. VERIFIED. A corpse poses off its own `clientInfo_t` (`cgs + 0x15594 + number * 0x4b8`), which `CG_ResetEntity` fills from its client's live one; with `eFlags & 0x80000` it also clones the live client's anim tree (`XAnimCloneAnimTree`), else it keeps its own tree. VERIFIED. `PlayerCmd_ClonePlayer` (Mac 0x1c3574) gives a new corpse the player's `eFlags` with bit 2 kept from the corpse slot and 0xa0000 set. VERIFIED. vcod: a dead player entity draws no body, and a fresh corpse new in a frame takes over its client's body state (`cod2_players`), so its death clip blends out of the live clip.

`BG_UpdatePlayerDObj` (Mac 0x182312) builds a body's DObj with `es.weapon`, or no weapon while `eFlags & 0x300`, and rebuilds it when that changes; `CG_CreateDObj` (Mac 0x14420c) appends the weapon's world model (`cg_weapons[w] + 0xbc`) on `cg_weaponrightbone` (`tag_weapon_right`), or `cg_weaponleftbone` (`tag_weapon_left`) while the client info's `+0x3f4` is set, which `CG_ProcessClientNoteTracks` (Mac 0x16341c) sets on an `anim_gunhand = "left"` note. VERIFIED. vcod hangs the CS 7 weapon's `worldModel` on `tag_weapon_right` and reads no gunhand note.

## 13. Entity types, events and vcod's descriptor

### Entity types

`CG_ProcessEntity` (Mac 0x164bd4) switches on `eType` 0..9 and errors `Bad entity type` past them. VERIFIED. 0..6 are named by the strings of the entity list printer just before `AddIP` (Mac 0x1cb286): `ET_GENERAL`, `ET_PLAYER`, 2 (`CG_Corpse`), `ET_ITEM`, `ET_MISSILE`, `ET_INVISIBLE`, `ET_SCRIPTMOVER`. VERIFIED. 7 calls `CG_SoundBlend`, 8 replays the effect configstring at `centity + 0x1c8` on a timer through `FX_PlayEffect`, and 9 is what `G_SpawnTurret` (Mac 0x1bc84a) stores, posed by `CG_mg42_DoControllers`. VERIFIED for the arms and the store; vcod's names for them (`ET_SOUND_BLEND`, `ET_FX`, `ET_TURRET`) are its own. 1.1 numbers these differently (`ET_MOVER` 5, `ET_PORTAL` 6, `ET_SCRIPTMOVER` 8, `ET_TURRET` 11).

`ET_EVENTS` is 10: `G_TempEntity` (Mac 0x1b7052) stores `event + 10` as the `eType`, and `CG_CheckEvents` (Mac 0x1e614a) takes `10 < eType` as an event entity carrying `eType - 10`. VERIFIED. 1.1's is 12.

### Events

The 199 `EV_*` names are the pointer table at `.data` 0x8184da0 in `cod2_lnxded` (strings from rodata 0x8153940), read in order; `crates/cod2/src/net/events.rs` is that table. VERIFIED. Each per-surface block holds 23 events (`DEFAULT` .. `ASPHALT`): footsteps run at 1, walk 24, prone 47, jump 70, landing 93, landing pain 116, then `EV_FOLIAGE_SOUND` 139 to `EV_STEP_VIEW` 143 and the weapon, binocular, impact and effect events to `EV_OBITUARY` 198. VERIFIED.

### Trajectories

`BG_EvaluateTrajectory` (0x80df5d4) has nine arms: 0, 1 and 6 hold `trBase`, 2 linear, 3 linear to `trTime + trDuration`, 4 sine, 5 gravity with `400 * t * t` off `z` (the float at 0x8154d38), 7 and 8 accelerate and decelerate with the 0.5 at 0x8154d3c. VERIFIED. 1.1's extra gravity arms are gone, and the accelerations moved from 9 and 10 to 7 and 8.

Which entities the cgame lerps is 12.2 and 18.5. A live player entity's `pos.trType` reads 3 (the fixture of 10). VERIFIED live.

### `map_restart`

`SV_MapRestart` (0x808be3e) queues `B` to each client when `G_GetSavePersist` is 0 and `n` otherwise, 1.5's choice. VERIFIED.

### vcod's `Cod2Wire` descriptor

`vcod_cod2::net::PROTOCOL_118` is a `Cod2Wire`, which implements vcod-common's `WireGame` beside CoD1's `Cod1Wire`:

- Shared and filled from this doc: `version` 118, `short_version` `1.3`, `game`, `max_configstrings` 2048, `field_codec` (5.2), `netchan` (2.3, 2.4), the three snapshot tables (5.1), `usercmd_weapon_bits` 7 (4.3), the HUD block (5-bit counts, 31 elements of 32 fields), `et_events` 10, `map_restart_command` `B`, and the configstring layout of section 6 (`CS_LAYOUT_118`: a block filled through `G_FindConfigstringIndex` or `G_ModelIndex` from its base plus one, a precache builtin's block from its base). The stock iwds are `FS_iwIwd`'s (0x8065324): `main/iw_00` .. `iw_24` and `localized_*`. VERIFIED.
- `max_weapons` 128 is INFERRED from the 7-bit `weapon` fields and configstring 8's 33 hex digits.
- `Cod2Wire` has no `*_to_v1` or `*_from_v1` maps: vcod keeps each game's own numbering, and the CoD2 client path reads `vcod_cod2::net::{events, flags}`. `Cod1Wire::ALL` and `by_version` stay CoD1's, so a CoD1 connection never falls back to 118.
- `Cod2Wire::as_cod1_shim` is the same wire as a `Cod1Wire`: 118's tables and layout, identity maps, and empty what only CoD1's server, item hints or pure check read (`cursor_hints` 0, `pure_dlls` empty since CoD2's client game is in the executable, no vehicle tables, no `^8`/`^9` colour codes). Two things still take it. `Defs::COD2` hands it to the shared game code as the configstring layout, the HUD block, `max_weapons` and the version tests (`is_v1`, `is_uo`, `Arith::for_protocol`). And until `Cod2Server` takes `EntityState2`, the CoD2 entities sit in vcod's `EntityState` over its table. Every number in those entities is CoD2's, built so (15.9); `cod2_wire::to_cod2_entity` copies them out and maps nothing.

### vcod's client on a CoD2 server

`vcod --connect` on a CoD2 install runs `crates/client/src/play/cod2.rs` over `vcod_cod2::net::Client`, not CoD1's `NetClient`: challenge, plain-text connect, the gamestate before the window opens, the map from serverinfo's `mapname`, then each frame the socket read, cmds built on the client clock (4.2) and sent in packets paced as 18.4 says, and the camera at the predicted eye. `vcod_cod2::net::NetView` keeps the live configstrings (`d`, and `x` `y` `z` chunk runs) and the snapshots the cgame could still read (18.5); its module doc is the interface the entity drawers read. `crates/cod2/tests/view.rs` replays the fixture of 10 through it.

`vcod --net-probe ... --game cod2 --probe-secs 62` (the same loop without a window) against retail on `mp_toujane`, `sv_pure 0`, beside a `spectate.rs --join --fly` player: 1242 messages, 1240 snapshots (1239 deltas), 1681 packets sent, no parse error; the spectator's eye flew from (434, 1448, 456) to (4535, 6264, 463) on its usercmds. VERIFIED live.

A slow load can outlast the server's `sv_timeout` if the session reads nothing while the window and its materials build. The session runs on a thread of its own from the gamestate to the first redraw, and again through any renderer build (`play::cod2::Pump`), reading snapshots and sending a cmd that holds the playerstate's view every 33 ms. The windowed client's whole frame is exercised by the CoD2 soak (`play::cod2_soak`, `tools/cod2_soak.sh`) and census (`play::cod2_census`); what it draws is a pending hand check.

Each frame the session hands `Frame` to `cod2_scene::Cod2Draw`: `place` resolves the non-player entities (12.1) and `players` the bodies (every `eType` 1 but the view's own client, and every corpse) with their `Character::from_client_state`, both placed by `Frame::entity_place`. The entity xmodels and posed body parts go to the renderer as one instance list, the brush models as submodel poses. After every gamestate the session queues the `cp` of 11.4 (the map's `.d3dbsp` touched, `pure_command`) and sends no cmd until it is queued.

Against retail on `mp_toujane` with `+set sv_pure 1` before the map, beside a `spectate.rs --join --fly` player (VERIFIED live):

- `vcod --net-probe 127.0.0.1:28970 --game cod2 --probe-secs 60` sent `cp @ <sum> <key>`, went to `CS_ACTIVE` and stayed 60 s: 1202 messages, 1200 snapshots (1199 deltas), no parse error, until its own `disconnect`. Every second the CPU half of the draw (`Cod2Draw::count`) built 5 entity xmodels (three `weapon_mg42` turrets, two flak 88 wrecks), 3 brush models and 1 body of 3 parts (the joined player's body, head and helmet).
- With `+set sv_timeout 10` and `VCOD_COD2_LOAD_STALL=25` (the probe's map load stretched to 25 s under the pump) the probe stayed connected: 500 messages read during the stall, no drop in the server log. A run without the pump was not made.

## 14. vcod's server

`vcod-server <map> --game cod2` runs `crates/server/src/cod2/` (`Cod2Server`, `engine::Engine<Cod2Game>`) beside CoD1's `Server`, through the same frame loop. It is written from sections 1 to 6 and 11; what it sends is checked against the retail captures (10, 15) and a retail client (16).

- The writers are `vcod_cod2::net::write`: `write_delta_entity`, `write_delta_client` and `write_delta_playerstate` (the mirror of 3.3 and 5.3: `lc` the last changed field plus one, the stats mask, the ammo blocks behind their gates, the objectives, the HUD count stopping at the first element whose `type` is 0 and each element's last changed field, 0 when none), `write_gamestate` and `write_snapshot`. Fed what the parser decoded from `crates/cod2/tests/fixtures/mp_toujane-spectate.bin`, they rebuild every server message of it, the gamestate and all 23 snapshots, byte for byte up to `svc_EOF` (`write::tests::retail_gamestate_and_snapshots_re_encode_byte_for_byte`). VERIFIED.
- The server netchan is vcod-common's `ServerNetchan` under `NetchanParams::COD2`: 4-byte fragment starts, the raw key string, a 128-slot command ring, and `transmit_eof` closing each block with CoD2's `svc_EOF` 7 (2.3, 2.4, 3.1).
- Connectionless: `getchallenge` is answered at once from a slot found again by address (1.1's LAN path; vcod asks no authorize server), `connect` reads the plain-text userinfo and checks `protocol` 118, the challenge of every address, the ping limits, the slots and `g_password` (1.2, 17.1), `getinfo` sends 1.4's keys in 1.4's order, `getstatus` configstring 0 with `challenge`, `pswrd` and `mod` and a line a client (1.5), and `rcon` checks `rcon_password` with 1.1's 500 ms window (1.6). No voice, no PunkBuster.
- Configstring 0 and 1 are built from the engine's dvars in case-folded order with the keys of section 9's capture; 1 carries `sv_iwds` and `sv_iwdNames` when pure (`sv_pure` defaults to 1, 11) and `sv_referencedIwds` / `sv_referencedIwdNames` over the iwds the map and script load read (11.3). On `mp_toujane` `dm` the lists name the sixteen `iw_15` .. `iw_00` and retail's six referenced iwds (11.3). A `cp` is checked with `verify_cp` (11.4), and a pure server drops a client that moves without one (`EXE_CANNOTVALIDATEPURECLIENT`) or with a bad one (`EXE_UNPURECLIENTDETECTED`), as 4.1 says.
- The rest of the table is the game's (`game::cod2::Cod2Level`, cod2-gsc-diff.md 5.2); changed slots go out as `d`. The level, its order and what the snapshots carry are 14.1.
- A client connects as a spectator at `mp_global_intermission` (else the first `mp_*_spawn`) until the script places it. A spectator's playerstate carries the fields of section 9's retail spectator: `pm_type` 4, `speed` 400, `pm_flags` 0x1000000, `eFlags` 2 once a spawn flipped it (0 from the connect to the first spawn, cod2-killcam.md 5), `viewlocked_entNum` 1023, the player box, `stats[3]` 63 and `stats[5]` 1. Snapshots carry `snapFlags` 4, or 6 before the client is active, and delta from the frame a `clc_move` acknowledged when it is under 29 messages old (3.3). Without game data (`load(None)`) there is no level, and the engine keeps bare spectators at the origin whose cmds only advance the clock.
- Housekeeping (pacing, zombies and timeouts, private slots, `g_password`, ping limits, downloads, the heartbeat, bans) is section 17. Shots, blasts, damage and deaths are the level's (`cod2-combat.md`); the snapshot carries its corpses (the body queue), its temp entities by scope, and its missiles past the cull (`SVF_BROADCAST`). The map cycle (`map`, `map_restart`, `fast_restart`, `map_rotate`) is `cod2-map-cycle.md`'s.

On loopback, `vcod-server mp_toujane --game cod2` with the stock 1.3 iwds took `examples/spectate.rs --secs 60 --fly` and `vcod --net-probe ... --game cod2 --probe-secs 60` at once: the example 1202 messages, 1199 delta snapshots; the probe 1203 messages, 1201 snapshots (1200 deltas), 0 parse errors; both sent a `cp` the server took, both entered the world and flew, and both left with `disconnect`. The probe's eye ended at (4535.9, 6263.9, 456); the same probe on retail ended at (4535, 6264, 463) (13). VERIFIED live. `cod2::tests` replays the same exchange in process, `vcod_cod2::net::Client` against `Cod2Server`, with no game data and, under `COD2_DIR`, a minute over `mp_toujane`.

### 14.1 Players

`Cod2Server` drives `Cod2Level` in `cod2_lnxded`'s order, read off the functions each item names. Calls and their order are read off control flow, so every ordering claim below is INFERRED unless it says otherwise.

- `SV_SpawnServer` (0x8093520): after `SV_InitGameProgs` three `SV_RunFrame`s, each 100 ms on, then `SV_CreateBaseline`, then `ClientConnect` again for clients carried over. INFERRED. vcod's `load` runs the three frames and takes the baselines from the object table's linked entities (`ScriptRuntime::baseline_entities`), triggers and `script_origin`s included as 1.1's pass does (INFERRED for CoD2). On `mp_toujane` `dm` that is 38.
- `SV_DirectConnect` calls `ClientConnect`; vcod's `admit` calls `Cod2Level::connect_at` with the userinfo's name. `SV_UserMove` (0x8090e34) enters the world (`SV_ClientEnterWorld`, `ClientBegin`) on a primed client's first move, before the pure check, then thinks each cmd whose `serverTime` is past `lastUsercmd`'s and not past the packet's newest (INFERRED); `SV_ClientThink` (0x8090dac) stores the cmd and, active, moves the lag time (0x80f5614) and runs `ClientThink` (0x80f65f8), which is `Cod2Level::client_think` (INFERRED). Cmds run as their packet arrives, between frames, as on CoD1. Not done: `SV_UserMove` putting the playerstate's weapon and offhand back in a cmd that names one the player lacks (0x80ebf24).
- `SV_Frame` (0x80969b0) calls, by name or string: `Dvar_InfoString(0x404)` into configstring 0, `Dvar_InfoString_Big(8)` into 1 and `SV_SetConfig(0x8e, 0x60, 0x100)` for the 0x100 dvars' block, each behind its modified bit; 0x8096952, which calls `SV_ResetSkeletonCache` and then 0x809676c per test client, a cmd of `G_random` buttons and moves (left zero while the client's killcam archive time is set) handed to `SV_ClientThink`; the pings 0x80963c8 (its `Giving %s a 999 ping` strings); `SV_RunFrame` 0x8096752, which is `SV_ResetSkeletonCache` and `G_RunFrame(svs.time)`; the archive 0x809b016; `SV_CheckTimeouts` 0x809657e (its `EXE_TIMEDOUT` and `Going from CS_ZOMBIE to CS_FREE`); `SV_SendClientMessages` 0x809bcce (its `SV_SendClientSnapshot` call); `SV_MasterHeartbeat("COD-2")`. VERIFIED. Their order, off the instruction order: the `sv_fps` residual and the time-wrap checks, configstrings 0, 1 and the 0x8e block, the test clients' cmds, the pings, then per owed frame `svs.time += frame` and `SV_RunFrame` with the archive between two owed frames, then the timeouts, the messages, the archive and the heartbeat. INFERRED. Against CoD1's order (AGENTS.md, "The server tick") the differences are the archive per frame and no zombie pass between the timeouts and the snapshots, the zombie clear being inside 0x809657e. INFERRED.
- `SV_Frame`'s two arms ahead of the frames. The pause arm, 0x80966ac: with `cl_paused` set, fewer than two clients past `CS_ZOMBIE` set `sv_paused` 1 and the call runs nothing, residual included. VERIFIED. It never fires on `cod2_lnxded`: `cl_paused` is registered with flag 0x1040, 0x40 being the read-only bit on which the set path at 0x80b2afa refuses a console or script set (`%s is read only.`), and the engine's only two writes set it 0. VERIFIED. vcod has no pause. The time-wrap arm: once a frame is owed, `svs.time` past 0x70000000 (21.7 days of uptime), or one of seven snapshot counters near `INT_MAX` (`svs.nextSnapshotEntities wrapped` and kin), calls 0x8060eb8 with `EXE_SERVERRESTARTTIMEWRAP` (or the counter's string) and `Cbuf_AddText("map <sv_mapname>")`, and runs no frame. VERIFIED, the tests and strings. 0x8060eb8 is `SV_Shutdown` (0x80942f8: every client past `CS_ZOMBIE` and not loopback gets `e "<reason>"` and a reliable `w "<reason>"` and a snapshot, twice; the masters' flatline, the game's shutdown, every client dropped, `svs` cleared, so `svs.time` restarts at 0) then `Com_Restart`. VERIFIED, the calls. Not done in vcod: no whole-engine restart for `Com_Restart` and the `svs` clear, and none of the seven counters. Its `time_ms` is an `i32` that wraps at 24.8 days.
- vcod's `engine::tick` over `Cod2Game` runs retail's order and keeps the killcam archive (between owed frames and after the snapshots) and vcod's own bots, which retail has not. Read off `VCOD_TICK_TRACE` on `bot_match`: configstrings 0, 1 and the dvar mirror's changes at the head; the bots once, `addTestClient`'s clients and vcod's own, on the clock before the step (CoD1's bots run there too); the pings; per owed frame the clock, `G_RunFrame` through `ClientEndFrame`, the identify pass and `CheckVote`, the test clients' install, the archive between frames, the level's commands; after the frames `SV_CheckTimeouts`, the messages, the archive, the heartbeat. VERIFIED, the trace; `cod2::tests::a_cod2_frame_runs_in_retails_order` pins it. Its ping is 1.1's average of acknowledge time less send time over the frame ring, both stamped with the frame clock (`level.time`). INFERRED, the clock (3.4.1).
- `G_RunFrame` (0x810a13a): the trigger queue, the script threads (0x808561a), the entity pass, `ClientEndFrame` for each client, `CheckVote` (INFERRED). `Cod2Level::run_frame` runs the script's frame (`ScriptRuntime::run_frame`), lands the spawns, the `setOrigin` and `setPlayerAngles` ops, and then `ClientEndFrame`'s playerstate stores (0x80f7516): a playing client gets `pm_flags` 0x800000 with 0x3000000 cleared, `gravity` `(int)g_gravity` and `speed` `g_speed`; a spectator the fields above. VERIFIED, the stores. Their order against the entity pass is not reproduced.
- The script's server commands (`iprintln`'s `f`, `H`/`G` scores, `C` offhand, `t` menus, `say`'s lines, `CheckVote`'s line) and configstring writes go out, in vcod, after each frame and after each client command that ran script, in the order they were written, as retail sends each as it is written. The dvar mirror's block waits for the next `SV_Frame`'s head, as `SV_SetConfig(0x8e, 0x60, 0x100)` does there. INFERRED, the order of the calls. Not reproduced: retail's mirror writes a dvar it has not mirrored yet into the first empty name slot (0x8092abe), VERIFIED, the loop. vcod's `Cvars::write_mirror` rewrites the block in sorted order. Client commands the level answers: `mr <serverId> <menu> <response>` (1.1's parse, `server::parse_menu_response`), `say`, `say_team`, `score` and `kill` (3.4.1).
- The playerstate (`Cod2Level::wire_playerstate`): pmove's fields (`PlayerState::to_wire`), `clientNum`, `viewlocked_entNum` 1023, `weapon` and `weapons[0..1]` the held mask, `weaponslots`, `offHandIndex`, the ammo and clip arrays the weapon builtins wrote, health in `stats[0]` and max health in `stats[2]`, and the HUD. `ClientEndFrame` copies the entity's health into `stats[0]`. VERIFIED. `stats[2]` is 1.1's slot and `weaponslots` packs four slot bytes a word. INFERRED.
- The HUD (`cod2_wire::hud_elems`) is `HudElem_UpdateClient` (0x8104882): the pool walked from slot 0, an element with a team kept for that team, one with an owner kept for that client, each copied into the archived or the current array by its `archived` flag, 31 a side. INFERRED, the walk and the filter. Types: text 1, value 2, the player name, map name and game type strings 3, 4 and 5, the shader 6 (`HECmd_SetShader` 0x8104022), the four timers 7..10 (`HECmd_SetTimer` .. `HECmd_SetTenthsTimerUp` hand 7..10 to 0x8104124) and the two clocks 11 and 12 (`HECmd_SetClock` and `HECmd_SetClockUp` hand them to 0x81041d8), VERIFIED. vcod's 1.1 numbers map onto these. `alignOrg` is `alignx << 2 | aligny` and `alignScreen` `horzalign << 3 | vertalign` (`vcod_cod2::hudelem`); `fromAlignOrg` and `fromAlignScreen` are sent equal to them (`cod2_wire::build_hud_elem`): a `moveOverTime` does not snapshot the old alignment.
- A playing client's body (`Cod2Level::player_entity`) is `ClientEndFrame`'s `g_smoothClients` arm (0x80f56f8; `Dvar_RegisterBool("g_smoothClients", 1, 0x1000)`): `pos.trType` 3 at the playerstate's origin and velocity from `commandTime` for 50 ms, `apos.trType` 1 at the view angles, both bases rounded to whole units (to nearest, INFERRED: no `fldcw`), `angles2[1]` the `movementDir`, `eFlags` the playerstate's with 0x20000 when dead and 0x40000 while sighted, `clientNum`, `groundEntityNum`, `leanf`, `weapon`, and the prone pitches. VERIFIED, the stores. The `eType` is 1 while `pm_flags & 0xc00000`, else 5 (both arms), which `ClientEndFrame`'s 0x800000 makes 1 for every playing client. INFERRED. The ring is the entity's own, fed from the playerstate's less the list at 0x815486c (16.3). `legsAnim` and `torsoAnim` are the playerstate's (`cod2_wire::player_entity`). Not done: the external `eventParm` and the prone lerp across a stance change.
- The object table's entities (`ScriptRuntime::packet_entities` over `PROTOCOL_118`, whose table order is CoD2's) are built in CoD2's numbers (`game::wire::build` under the profile's `cod2_wire::COD2_ENTITY_NUMBERS`): a script model, script brush model, `script_origin` or brush mover `eType` 6, a turret 9, a looped effect 8, an event entity `10 + n` with `n` and the ring's events through `cod2_wire::event_to_cod2` (15.2); `eFlags` 0x2 the teleport bit, 0x20 `EF_NODRAW`, 0x40 a gun's firing bit (12.1 and 12.4: the bits the cgame tests, INFERRED for the teleport and firing bits' sources), an item's none; a mover's `trType`s through `cod2_wire::tr_type_to_cod2` (15.9). An item's `index` stays the weapon's configstring 7 index (12.1). A looped effect's id rides `scale`, which the builder's `_union.scale` reaches through `EntityState::field_index`'s fallback (cod2-efx.md 6.6).
- The cull is `crate::world::in_snapshot` under 1.5's rule (areas connected, then the clusters of the link box against the eye's PVS row; broadcast entities always), over `MapWorld::visibility` of the d3dbsp, from the playerstate's origin plus `viewHeightCurrent`, and the viewer's own body never goes in its own snapshot (INFERRED, Q3's rule; `SV_AddEntitiesVisibleFromPoint` 0x8098b98 not read).
- The client state: `team` from `.sessionteam`, the name, `modelindex` and the six attachment pairs the script set (`ScriptRuntime::client_model_index`, `client_attachments`).

VERIFIED in process (`COD2_DIR`, `cod2::tests::two_cod2_players_join_spawn_and_see_each_other_on_mp_toujane`): two `vcod_cod2::net::Client`s connect to `Cod2Server` on `mp_toujane` `dm`, send their `cp`, answer the team menu (`allies`, `axis`) and the weapon menu `t` opens, spawn, and run 30 s. Each one's snapshots carry the other's body (`eType` 1) in 788 of about 800 snapshots, moving over 1000 units; each ends with `pm_type` 0, a weapon, `stats[0]` 100 and the script's HUD elements; no parse error, no script error at `developer 1`.

On loopback, `vcod-server mp_toujane --game cod2` (stock 1.3 iwds, pure) took `examples/spectate.rs --join --fly --secs 40` and `vcod --net-probe ... --game cod2 --probe-team allies --probe-secs 40` at once. Both sent a `cp` the server took, entered the world and spawned through the menus. The probe read 802 messages and 801 snapshots (800 deltas) with no parse error, played from its first second with a weapon in hand (`pm_type` 0, `weapon` 4), walked 610 units and drew the example's player as one body of 4 parts in every frame; the example printed the probe's entity 1 at `eType` 1 moving across the town, and read 801 messages, 799 deltas. Both left with `disconnect`. The server logged no script error. VERIFIED live. A retail client on this server is 16.

## 15. The whole wire against retail

Recipe: retail `cod2_lnxded` 1.3 (`dm`, `sv_pure 1`, `developer 1`, `sv_maxclients 8`, `+set g_gametype dm` before `+map`) on `mp_toujane` and `mp_carentan`, inside `unshare -rn`, with two `examples/spectate.rs` clients saving every datagram: a spectator 12 s after the server, and a `--join` player 3 s later (`mr <id> 1 autoassign`, then the nation's rifle). `examples/wire_dump.rs` prints a capture as text (configstrings, baselines, server commands, every snapshot's playerstate, ammo, clips and entities). The same two clients against `vcod-server --game cod2` gave the diff below; each item ends with what vcod does. The captures are `crates/server/tests/fixtures/cod2-wire/` (15.7 lists the gametype ones), and `cod2::wire_ab` replays the same two clients in process and pins what still differs (15.7). Every "live" claim below is VERIFIED live from those captures.

### 15.1 Entity numbers

`G_InitGame` (0x8109096) sets `level.num_entities` to 0x48. VERIFIED. `G_Spawn` (0x811e9d6) takes the head of the free list only when 0x811e988 passes it: `level.time - freetime > 499` (`cmp $0x1f3` / `jg` at 0x811e9ad) or `num_entities > 0x3fd`; otherwise it hands out `num_entities` and counts up, and errors at 0x3fe. VERIFIED. `G_FreeEntity` (0x811ee50) stamps `freetime` (+0x178) with `level.time` and appends a number above 71 to the list's tail. VERIFIED. 1.1's `G_Spawn` has no wait; with it vcod's CoD2 numbers ran low (258.. against 619..).

So every block of a map's entity string takes the next number in lump order whether its spawn keeps it or frees it, since the whole load runs at one `level.time`. VERIFIED live: a `dm.gsc` that logs `getEntArray()`'s `getEntityNumber()` and `classname` at `Callback_StartGameType` printed mp_toujane's blocks at 72.. (block 84 the `vase_break_remove` brush model, 111 the first `trigger_radius`, 616 the last block, a `trigger_use_touch`), with the 361 `misc_model` blocks' numbers taken and empty; the script's first spawns (two `script_origin`) at 617 and 618 and the 19 `loopfx` effects at 619..637. The body queue is 64..71 (0x811eaa4: the entity array plus 0x8c00, index mod 8). VERIFIED.

vcod: `ObjectTable::set_reuse_after(500)`, on for a CoD2 level, with the allocator's clock moved each frame (`ObjectTable::set_clock`). CoD1 keeps its immediate reuse. Both captures' baselines and snapshot entities carry retail's numbers.

### 15.2 Events in an entity's ring

vcod's shared builtins raise every event in 1.1's numbering (`playFxOnTag`, `playSound`, a turret's fire, a missile's bounce), on temp entities and in entity rings alike. `cod2_wire::event_to_cod2` maps one to CoD2's table (13, "Events"): the per-surface blocks and 139..143 sit at the same numbers in both (VERIFIED, the two tables), the small and large bullet hits go by cod2-efx.md 6.2, every other event by its `EV_*` name; an event CoD2 lacks goes out as none, with its parm. `game::wire::build` runs it on the ring of every object table entity as it builds it; a player's and a corpse's rings are CoD2 pmove's own, and a missile's CoD2's from the start (`Cod2Level::missile_entities`).

### 15.3 Weapons and the registrations at load

- `G_SetupWeaponDef` (0x8120d74), called at the head of `G_InitGame` before the spawn pass, registers `bg_itemlist`'s ammo rows (none in MP) and then `G_GetWeaponIndexForName("defaultweapon_mp")`. VERIFIED. So `defaultweapon_mp` is weapon 1 and `xmodel/defaultweapon` model 1 (configstring 335). VERIFIED live, both maps.
- `G_GetWeaponIndexForName` (0x8120dd0) registers a new weapon through 0x80ebc24 with `G_RegisterWeapon` as the callback only while the word at 0x864c39c is set, and otherwise only looks the name up (`BG_FindWeaponIndexForName`). VERIFIED for the branch; that the word is `level.initializing` is INFERRED. Stock `_weapons.gsc` takes every nation's pistol and grenade from a spawning player (`takeWeapon("colt_mp")` and kin); registering on lookup would add them at the first spawn and resend configstring 7.
- `G_RegisterWeapon` (0x8107216): the item's flag, the hint strings at def+0x56c and +0x570 through `G_GetHintStringIndex`, `G_ModelIndex` of the world model at def+0x1b4 (`xmodel/defaultweapon` swapped in for one that fails to load), then `G_ModelIndex` of def+0x38c. VERIFIED, the calls and their order. The four words are `useHintString`, `dropHintString`, `worldModel` and `projectileModel`: INFERRED from the live order, `PLATFORM_HOLD_TO_USE` and `PLATFORM_HOLD_TO_DROP` at 1278 and 1279 off `mg42_bipod_stand_mp`, then `xmodel/weapon_mk1grenade`, `xmodel/projectile_BritishGrenade` and on through the precached weapons from 411.
- Configstring 8 (0x8106f7c): one bit per `bg_itemlist` row over `bg_numItems` (131), four rows a hex digit, low bit first, lowercase. VERIFIED. Row 0's bit reads set on both maps, `fffff3...` for mp_toujane's 21 weapons and `fffff7...` for mp_carentan's 22 (33 digits). VERIFIED live; what sets row 0 is not traced.
- `G_SpawnTurret` (0x810c6b2) registers its `weaponinfo`, then `G_SoundAliasIndex` of the def's words at 0xb0, 0xb4, 0xb8 and 0xbc: `weap_mg42_loop`, `weap_mg42_loop_plr`, `weap_mg42_cooldown`, `weap_mg42_cooldown_plr` at 591..594 live, the file's `loopFireSound`, `loopFireSoundPlayer`, `stopFireSound`, `stopFireSoundPlayer`. VERIFIED. 1.1 indexed two.
- `SP_trigger_hurt` (0x811caba) reads its `sound` key and indexes nothing, so no `world_hurt_me`. VERIFIED.
- `SP_worldspawn` writes configstring 2 `cod` first (the string at 0x815e774), and `G_InitGame` sets configstring 22's `winner` to `0` (`Info_SetValueForKey`, the `0` at 0x815aa62). VERIFIED.

### 15.4 Dvars, the mirror and the startup config

- Configstrings 142..237 carry the mirrored dvars' names, 238..333 their values (6). The engine's are those registered with flag 0x100 (0x1180 in `BG_RegisterDvars` 0x80de830, 0x1100 in 0x81084a4). VERIFIED. Live, mp_toujane's block is those 62 plus the six `ui_*` that `_serversettings.gsc` and `dm.gsc` flag with `makeCvarServerInfo`, sorted case-folded. VERIFIED live. vcod: `Cvars::remirror` points the cvar table's mirror at CoD2's layout with `cod2::oob::MIRRORED`, and the server writes it after each script frame.
- The server execs `default_mp.cfg` at startup (`execing default_mp.cfg` in the server log), which sets the stock `scr_*` limits: `scr_dm_scorelimit` 50 where `dm.gsc`'s own default is 100, so `cg_objectiveText` reads `MP_GAIN_POINTS_BY_ELIMINATING\x1550`. VERIFIED live. vcod runs it under the command line's sets.
- A colour dvar keeps four bytes: `setCvar("g_TeamColor_Allies", ".25 .25 .75")` reads back `0.25098 0.25098 0.74902 0` (a missing component is 0). VERIFIED live. Rounding to the nearest byte is INFERRED from the values.
- `serverinfo`'s `gamename`, `protocol` and `shortversion` read `Call of Duty 2`, `118`, `1.3` (9). VERIFIED live. vcod's CoD2 server sends the same.

### 15.5 Server commands

- `Scr_ConstructMessageString` (0x8110720) packs a localized string behind a 0x14 when it is not first, and any other part (a player's `"%s^7"` name, a string, a number) behind a 0x15 when it follows a localized string or opens the message. VERIFIED. Against 1.1 there is no closing 0x15 and no letters arm, so `setCvar("g_TeamName_Allies", &"MPUI_BRITISH")` reads `MPUI_BRITISH`. VERIFIED live.
- `iPrintLn` (0x8110ad2) formats its word with `va("%c", 'f')` and hands it to 0x8110a60, whose packer calls `va` once per player argument and whose final `va("%s \"%s\"")` then formats into `va`'s next buffer of two (`va`, the index toggled through `Sys_GetValue(1)`). VERIFIED. With one player argument that is the word's own buffer, and the word reads empty: `iprintln(&"MP_CONNECTED", self)` arrives as ` "MP_CONNECTED\x15vcodspec^7"`. VERIFIED live (8): the client sees a command whose letter is a space. That two player arguments send the last even one's `"<name>^7"` as the word is INFERRED from the same buffers.
- `giveWeapon` (0x80f9b8c) sends `I "1"` after `G_GivePlayerWeapon` (0x8120b26) returns 1, which it does for a weapon the player lacked (not class 7 or 8); a first offhand inside it sends `C <index>` (0x8120e44) first. VERIFIED. A spawn's `I "1"`, `C 3`, `I "1"`, `I "1"` live: the grenade's give, then `switchToOffhand`'s second `C 3` (type 1) taking the unsent first one out and going on the end, as 1.1's `SV_AddServerCommand` squash does (`docs/protocol-1.1.md`). VERIFIED live, the four commands. That CoD2's squash (0x8094750, its find at 0x8094530) follows 1.1's rule is INFERRED from the function's shape. vcod squashes an unsent equal `C` or `a`.
- A notify resumes its waiters newest `waittill` first. On the first connect live, `_grenadeindicators`' `cg_hud*` lines came before `_weapons`' `ui_allow_*` and `_teams`' `ui_allow_joinauto`, the reverse of the order `dm.gsc` starts those three; on the second connect, after each waiter had parked again in that wake order, the order was `_teams`, `_weapons`, `_grenadeindicators`. VERIFIED live, both captures; the mechanism, a list the newest `waittill` heads, is INFERRED. 1.1's notify wakes in start order (cod11-gsc-language.md 9). vcod: `Vm::set_newest_waiter_first`, on for CoD2.

### 15.6 Entity and playerstate fields

- `SV_LinkEntity` (0x809c63a) packs a solid or body box (`contents & 0x2000001`) as `maxs.x`, `1 - mins.z` and `32 + maxs.z`, each clamped to 1..255, with 0xffffff for a brush model. VERIFIED. 1.1's middle byte is `-mins.z`. A standing player reads 0x66010f live. vcod packs a live body's from its playerstate box.
- `InitTrigger` (0x811c504) ORs `eFlags` 1; its callers are `SP_trigger_hurt` and three spawn functions at 0x811c69a, 0x811cb36 and 0x811ce16; `trigger_radius` never calls it. VERIFIED. Live: mp_toujane's `trigger_hurt` baseline carries `eFlags` 1, its `trigger_radius` ones none. Which classnames the three other callers spawn is not traced; vcod gives every brush trigger but a look-at one the bit.
- A spectator's `groundEntityNum` reads 0 (no ground trace runs for it). VERIFIED live. vcod sends 0, not pmove's 1023.
- A playing client's `viewmodelIndex` is its `setViewModel` model (103 on mp_carentan's axis player: configstring 437, `xmodel/viewmodel_hands_german`), and its `cursorHintString` -1, from `ClientEndFrame`'s use pass (0x8121bc6: 0, -1, 1023 into the three hint words). VERIFIED, the stores and the live values; a spectator's `cursorHintString` reads 0. VERIFIED live. vcod's gunner hint pass (`cod2_turret`) writes the playing arm's, and a spectator's goes out 0.
- A map entity's angles go out unsnapped. `G_ParseEntityFields` (0x811a7b0) ends with `G_SetOrigin` and `G_SetAngle(ent, ent+0x144)`, and `G_SetAngle` (0x811f426) copies the angles into `s.apos.trBase` (+0x3c) and `r.currentAngles` (+0x144); `SV_LinkEntity` (0x809c63a) then runs the angle snap on +0x144 alone. VERIFIED. So mp_toujane's first `trigger_radius` (111, `targetname` `trigger_landmark`, `angles "0 2.50448e-006 0"`) sends its yaw as a set field whose short is 0, which the parser reads back as the float bits 1, while script reads the snapped 0. VERIFIED live. vcod: a CoD2 link whose snap moves the `angles` field keeps the unsnapped value in `Entity::apos_base`, an `angles` write clears it, and the wire's stationary `apos.trBase` reads it first.
- `stats[5]` counts spawns: `ClientSpawn` (0x80f910e) keeps playerstate word 0x50 across its memset and stores it plus one. VERIFIED. Live, 1 for the spectator and 2 once the joiner spawned.
- Ammo and clip slots follow `clip_indices` over configstring 7 (cod2-weapons.md): `defaultweapon_mp`'s `enfield` clip takes 1, a weapon with no clip name shares slot 0. Live, mp_carentan's axis rifleman holds clips 12 (`frag_grenade_german_mp`, 3), 14 (`luger_mp`, 8) and 15 (`kar98k_mp`, 5) and ammo 14 and 15; `binoculars_mp`'s `clipSize` 0 leaves slot 0 empty. VERIFIED live. Trap: numbering the empty clip name as a name of its own, or flooring `clipSize` at 1, puts every slot one high and a 1 in slot 2.

### 15.7 The gate and what still differs

`cod2::wire_ab` (`COD2_DIR`) replays the fixtures and runs the same two clients against `Cod2Server` (hostname `CoD2Host`, `developer 1`, the script's RNG seeded so `menuAutoAssign`'s `randomInt(2)` draws retail's team). It diffs every configstring (0 and 1 key by key but `sv_serverid`), every baseline field, both clients' server commands up to retail's last, the spectator's last snapshot (every entity, a live body's clock and anim aside, and its playerstate) and the joined player's last playerstate with its stats, ammo and clips (place, clock and pmove fields aside). `WIRE_REPORT=1` prints each difference's two values; `WIRE_GT_DIR`, `WIRE_MAP`, `WIRE_GT`, `WIRE_SEED` and `WIRE_KILL` gate a capture outside the fixtures.

The gate replays four things off the retail capture rather than drawing them (the fourth, the drops' draws, below):

- The joiner's spawn spots: each snapshot whose `pm_type` turns 0 gives a spot and its yaw, handed to the `spawn` builtin as CoD1's `Server::pin_spawns` does. The script draws its pick off `irand` (lnxded 0x80aaada, `Scr_RandomInt` 0x8113ca4), seeded at load. VERIFIED, the calls.
- The death anim: `BG_AnimScriptEvent` (lnxded 0x80d96ee) picks `rand() % n` among the clause's commands, off the game's glibc `rand()`, seeded off the clock (cod2-combat.md 9.6). VERIFIED. vcod's `player_die` pick draws from the game's stream, not the script's RNG (`Cod2HostState::rand`, the same `Rc` as `Combat::rand`), and the gate pins retail's anim (`Cod2HostState::death_pins`) with the draw still made.
- The clock: each client reads its first snapshot at its twin's first snapshot's `serverTime`, the kill's `kill` goes out a frame before retail's first dead snapshot, and a client leaves 650 ms before its first snapshot plus its `--secs`, so `d 13`, a round's end and the drops land on retail's level times. That `spectate.rs` reads its first snapshot 650 ms after its start is INFERRED from the two kill runs (`--kill-at 5`, the first dead snapshot 4400 ms after the first). The gate also runs the queued console lines ahead of each frame, as `main.rs` does, so a script's `map_restart` happens.

The fixtures, all on the recipe of 15: `dm` on mp_toujane and mp_carentan; mp_toujane and mp_carentan under `tdm`, `sd`, `ctf` and `hq`; a `tdm` and an `sd` run whose joiner sends `kill` 5 s in (`spectate.rs --kill-at 5`, both clients `--secs 15 --save-messages 240`); mp_burgundy under `dm` and `sd`. Every one matches with an empty `GAPS` but the two kill runs'. VERIFIED live.

What the kill runs and mp_burgundy showed (VERIFIED live, the fixtures, unless labelled):

- The body queue's pick (lnxded 0x811eaa4) flips the slot's teleport bit, `eFlags = (old & 2) ^ 2`, before `PlayerCmd_ClonePlayer` (0x80fcc76) ORs in the player's flags less 0x2. VERIFIED, the stores. The tdm corpse in slot 64 reads 0xa0002. VERIFIED live. vcod: `BodyQueue::push_raw` flips the slot's toggle.
- `ClientThink_real` (lnxded 0x80f5d2e) sets `r.currentAngles` to (0, `ps.viewangles` yaw, 0) after `Pmove`. VERIFIED, the stores. The clone (`G_SetAngle(clone, player+0x144)`) and `Drop_Weapon` (Mac 0x1b165e, `G_SetAngle(item, player+0x144)`) take it, not the body's `apos`. VERIFIED, the calls. The tdm corpse's angles, turned onto the ground, match retail's bit for bit with the view's yaw (-144.305) and not the body's (-143.998). vcod writes the player's `angles` field after each think and the clone reads the view's yaw.
- The body's `pos.trBase` snap goes through an integer (15.6's `fistp`), so -0.953 goes out +0, not -0. VERIFIED live, the sd spawn at z -0.953; that the store back is a `fild` is INFERRED.
- `PM_Weapon` (lnxded 0x80f066e) runs the breath step (0x80ed084) once `pm_flags` 0x1000 is clear, and `ClientSpawn`'s think (100 ms) runs in `Pmove`'s 66 and 34 ms steps: empty hands' `holdBreathScale` tracks to 1 at `player_breath_gasp_lerp` 6, 0.396 then 0.519216, which the retail spawn's snapshot carries. VERIFIED, the calls and the live value. vcod runs the breath step on the spawn think.
- `SV_MapRestart` (lnxded 0x808be3e) sends `n`/`B` and calls `ClientConnect` only for a client whose state is above 1 (`CS_CONNECTED` and up), so a zombie (a drop still draining) gets nothing. VERIFIED. Trap: a `ClientConnect` for the dropped spectator's slot wakes the restarted level's `connecting` waiters first and swaps their order for the joiner's (15.5). vcod skips zombies there; the sd kill run's commands match retail's through the round's restart, `d 595` before `f` included.
- A `thread` call reads what the thread returned when it ran to its end without suspending, and undefined once it suspended. VERIFIED live (a probe `dm.gsc` on retail 1.3 storing each result in a dvar read over rcon: 7, a struct's field, undefined after `wait 0.05`; cod2-gsc-diff.md 3.5). `_fx::soundfx` writes `ent.delay` on `level thread createfx_showOrigin(..)`'s struct; reading undefined there fails and loses the rest of mp_burgundy's `main` (its fog, ambient, `_load` and `level.xenon`, so `_menus` fails too). vcod: `Vm::set_thread_returns`, on for CoD2.
- `ScrCmd_PlayLoopSound` (lnxded 0x8111e0c) sets `r.broadcastTime` to -1 and `ScrCmd_StopLoopSound` (0x8111e5a) to `level.time + 300`. `SV_AddEntitiesVisibleFromPoint` (0x8098b98) adds an entity with a live `r.broadcastTime` before it tests `SVF_NOCLIENT`, the client mask and the PVS. VERIFIED. So mp_burgundy's `soundfx` `script_origin` (897, `loopSound` 9) is in every snapshot. vcod: the loop sounds stamp `broadcast_time` on CoD2 as on 1.5, and `game::wire::build` sends a CoD2 entity with one past `SVF_NOCLIENT`.

- The corpse slides on its death anim's root motion: `G_RunCorpseMove` (lnxded 0x8107846) moves a resting corpse whose `XAnimCalcDelta` step is over a unit along its forward and left (cod2-combat.md 6.2). VERIFIED, the decompile. Both kill runs' corpses match retail's bit for bit. VERIFIED live, the gate.
- The dropped weapons fly at the player's angles with no roll, spin at `(50 c3, 40 c2, 60 c1)` off three `crandom()` draws, climb off a fourth, and land through CoD2's own `G_RunItem` (cod2-combat.md 6.3). VERIFIED, the decompile. The draws come off the clock-seeded `rand()`, so the gate reads each item's four back off its first snapshot in flight (`trDelta[2]` is `10 + 5 c`, `apos.trDelta` the spin) and pins them (`Cod2HostState::drop_pins`), the draws still made. VERIFIED, the gate.

Still differing, the kill runs' `GAPS` (VERIFIED live, the fixtures):

- The two dropped weapons' place. `Drop_Weapon` starts them at the posed body's tag (`G_DObjGetWorldTagMatrix`), and vcod's pose of the dying body is 0.01 off retail's on the tdm run (the tag at (298.07028, 642.31024, 44.839428) against (298.06473, 642.30334, 44.847702)) and two ulps of x off on the sd run. Their spin, angles at rest and flight match. VERIFIED live, the gate. That the rest is the idle clip's phase or the skeleton's float order, as in the hit pose's gaps (cod2-combat.md 9.5), is INFERRED.

Not covered: temp entities, and entity events past the spectator's last snapshot.

### 15.8 What the gametypes showed

Each fix below turned a gametype capture's difference into a match. VERIFIED live, the fixtures of 15.7, unless labelled.

- A player body's `iHeadIcon` and `iHeadIconTeam` are the script's `.headicon` / `.headiconteam` (`ClientScr_SetHeadIcon` 0x80f81d4 stores the 1-based index of `GScr_GetHeadIconIndex` 0x8113770 over configstrings 31..45; the team setter 0x80f82b6 stores none 0, axis 1, allies 2, spectator 3). VERIFIED, the stores. `tdm` sends `iHeadIcon` 2 (`headicon_german`) and team 1 for an axis player. vcod's CoD2 body copies them (`Cod2Level::player_entity`).
- `G_ShaderIndex` (0x811d44a) lowercases the name before `G_FindConfigstringIndex(.., 0x61e, 0x80, ..)`. VERIFIED. So `sd`'s `objective_add(0, "current", .., "objectiveA")` indexes `objectivea`, and `hudStopwatch` reads `hudstopwatch`. vcod: `GameHost::shader_name`, through `precacheShader`, `setShader`, the clocks and the objective icons.
- `SP_trigger_use` and `SP_trigger_use_touch` (0x811003e, 0x8110052, both 0x810fe50) never call `InitTrigger`: no `eFlags` 1. They store the hint string slot in `s+0xd8` (`scale` on the wire, 0xff for none) and the cursor hint in `s+0xdc` (`dmgFlags`, 2 `HINT_ACTIVATE` unless the `cursorhint` key names another). VERIFIED. The bombzones' baselines read `scale` 255, `dmgFlags` 2. The spawn table is at 0x8188a00. VERIFIED.
- `ScrCmd_Hide` (0x8112130) sets `r.svFlags` 0x800 and both `r.clientMask` words to -1, so no client's snapshot carries the entity; the entity pass (0x8109f5c) turns 0x800 into `eFlags` 0x20, which the baseline still carries. VERIFIED, the stores. `sd`'s hidden exploder models (452, 454, 460, 462) are in the baselines with `eFlags` 0x20 and in no snapshot. vcod drops a hidden entity from `Cod2Level::world_entities`.
- The script clock: `G_RunFrame` (0x810a13a) ends its thread passes with 0x80855e2, which adds 1 to the VM's time word (masked to 24 bits) once a frame, whatever the frame's length. VERIFIED. That the word counts 50 ms ticks, so the three 100 ms load frames of `SV_SpawnServer` leave the script 150 ms behind `level.time`, is INFERRED from the live order: `sd.gsc`'s `bombzones()` (`wait .2` before its `objective_add`) and the `trigger_use_touch` hint string it sets land before the spectator's gamestate on retail, and only with the lag on vcod. vcod: `ScriptRuntime::script_lag_ms`, 50 per load frame.
- `SV_DropClient`'s `ClientDisconnect` runs inside the drop, so a disconnect's `MP_DISCONNECTED` goes out ahead of the drop's `J <slot>` (17.3). VERIFIED live, the kill run. vcod runs the callback in the drop and flushes its commands before the `J`.

### 15.9 The entity numbers, against the shim

vcod-server builds each CoD2 entity in CoD2's numbers (14.1). An earlier design built them in 1.1's numbers over 118's table (`Cod2Wire::as_cod1_shim`) and renumbered them in `cod2_wire::to_cod2_entity`: `eType` by a table, ring events by name, `eFlags` through a list that kept 1.1's 0x1, 0x8 (teleport), 0x100 (`EF_NODRAW`) and 0x400 (`EF_FIRING`) as 0x1, 0x2, 0x20 and 0x40 and dropped every other bit. What that path lost against retail, the trap a renumbering layer reintroduces:

- A missile's `eFlags`. CoD2's missile code wrote CoD2's bits, which the list read as 1.1's and dropped. `fire_grenade` (0x810e68e) leaves a thrown grenade with 0x1000000, and `G_ExplodeMissile` (0x810d970) sets `eType` 0 and ORs `EF_NODRAW` 0x20 into `eFlags` (the store `param_1[2] | 0x20`). VERIFIED, the stores. The frag capture (`crates/cod2/tests/fixtures/combat/hit-throw.txt`) carries `eFlags` 0x1000000 on the grenade in flight and 0x20 on it once it went off. VERIFIED live. 0x1000000 sits above the field's 24 bits, but the delta's change and zero bits still go out for it (5.2), so the bytes differ. The renumbered path sent 0 in both states, wrong in all 69 flight and 7 spent frames of `cod2_combat_ab`'s `hit-throw` replay; vcod sends 0x1000000 and 0x20 and matches all 76 (`missile eFlags whole`).
- A mover's `trType`. The shared mover writes 1.1's `TR_ACCELERATE` 9 and `TR_DECCELERATE` 10; CoD2's `BG_EvaluateTrajectory` (0x80df5d4) has arms 0..8 only and `Com_Error`s (code 1, a drop) past them, and CoD2's own move and rotate setups (0x8118c4a under `ScriptMover_Move` 0x81193a8, 0x8118ffe under `ScriptMover_Rotate` 0x8119572) store 7 and 8. VERIFIED, the switch and the stores. `maps/mp/_load.gsc` runs `maps\mp\_shutter::main`, whose shutters swing with `rotateto` and `rotateroll` given an acceleration and a deceleration (`rottime * 0.5`), so every stock map with shutters sent 9 and 10. VERIFIED, the stock script. No retail capture holds a mover with an acceleration. vcod maps 8, 9 and 10 to 6, 7 and 8 as it builds (`cod2_wire::tr_type_to_cod2`); 1.1's 6 and 7, the low gravities, have no CoD2 type and nothing on CoD2 writes them.
- A missile's ring went out through a round trip, CoD2's events to 1.1's and back, which loses the 14 CoD2 events 1.1 has none for (the offhand, binocular, `EV_SOUND_ALIAS_AS_MASTER`, `EV_SHOTGUN_HIT`, `EV_BULLET_HIT_AP` and `EV_GRENADE_SUICIDE` ones). The missiles raise only the bounce and the explodes, which survive it. VERIFIED, `event_to_cod2` over all 198. The ring goes out as built.
- `_union.scale`, a looped effect's id: the 1.1 builder's name for the word CoD2's table calls `scale`. `EntityState::field_index` falls back from `_union.` names, so it goes out (cod2-efx.md 6.6).
- An item's 1.1 `eFlags` 16 was dropped by the list, which happened to be retail's: every item in the `cod2-wire` captures (934 states) carries `eFlags` 0. VERIFIED live. The builder writes 0 (`COD2_ENTITY_NUMBERS.item_eflags`).

Not part of the shim but read off the same capture: `fire_grenade` also stores `level.time + 50` in `time` (+0x54; 0x1d4c on a grenade whose `pos.trTime` is 0x1d1a), and `G_ExplodeMissile` sets `eFlags` 0x10000 with `time` and `time2` for a weapon with an explosion effect at def+0x394. VERIFIED, the stores. vcod writes neither.

## 16. Retail client on vcod-server

Recipe: retail `CoD2MP_s.exe` 1.3 under Wine in a nested compositor, in one `unshare -rn` namespace with the server, started as `CoD2MP_s.exe +set logfile 2 +set name c12player +exec c12.cfg +connect 127.0.0.1:28970 +vstr c12a`. The cfg's `c12a` loops `+forward`, `+attack`, `+left`, `+moveleft` and `wait`s, so the player walks, turns and fires with no input device. The server ran a copy of stock `dm.gsc` whose spectator arm notifies `menuresponse` with the team menu's `allies` 4 s after `begin` and the nation's rifle 3 s later, so the stock menus open on the client and the script answers them. An rcon sender issued `status`, `map_restart` and `map mp_carentan` on a schedule. Servers: retail `cod2_lnxded` 1.3 (`dm`, `sv_pure 1`, `developer 1`) as the control, and `vcod-server mp_toujane --game cod2 --gametype-script dm.gsc` with `sv_pure` 0 and 1, `--bots 1 --bots-shoot`. Screenshots every 15 s; 130 to 240 s a run, once with `+set developer 1` on each server.

### 16.1 The CD key

Without a CD key the client never sends a packet: it prints `127.0.0.1:28970 resolved to 127.0.0.1:28970` and stops with `Error during initialization: EXE_ERR_INVALID_CD_KEY`, on either server. VERIFIED live.

- `CL_Connect_f` (Mac 0x1627fc), for an address that `NET_IsLocalAddress` does not call local, runs a CRC-16 (polynomial 0xa001, initial 0) over the 16 characters of `cl_cdkey`, prints it `%04x` and compares it case-folded with the first 4 characters of `cl_cdkeychecksum`; a mismatch is `Com_Error(1, "EXE_ERR_INVALID_CD_KEY")`. VERIFIED (the loop, the format string and the error string). That only the client's own listen server skips the check is INFERRED from the `NET_IsLocalAddress` branch; `connect 127.0.0.1` takes it, and a `devmap` runs without a key. VERIFIED live.
- `CL_RequestAuthorization` (Mac 0x14ab04) opens with `CL_CDKeyValidate(cl_cdkey, cl_cdkeychecksum)` and the same `Com_Error` before it resolves `cod2master.activision.com`. VERIFIED.
- The Windows client reads the key from the value `codkey` under `HKLM\SOFTWARE\Activision\Call of Duty 2`. VERIFIED (both strings in `CoD2MP_s.exe`, beside `cl_cdkey`); that `cl_cdkey` is filled from it is INFERRED from the Mac client's `Com_ReadCDKey` (Mac 0x2f802), which reads `codkey`.

With a valid key in the prefix the check passes locally, and both servers took the client from loopback with no authorize step (a LAN address is answered at once, 1.1). The namespace has no route out. VERIFIED live.

The Windows client's authorize request, the `PB` hash and the `needcdkey` handling, and what vcod sends, are in cod11-server-handshake.md, "CD key authorization (the clients)", beside the other four clients: `getKeyAuthorize 0 <key> PB <hash>` to `cod2master.activision.com` port 20700 before each `getchallenge 0 "<hash>"` to a server off the LAN, the hash MD5 of the upper-cased key from Activision's own initial state (CoD2MP_s.exe 0x40cfc0). VERIFIED.

### 16.2 What the client did

On retail and on vcod-server, pure on and off, the client (VERIFIED live, its `console_mp.log` and the screenshots):

- got a challenge and `connectResponse`, loaded the gamestate, sent a `cp` (vcod: `cp ok` under `sv_pure 1`) and entered the world;
- opened the menus the script sent (`serverinfo_dm` on both, the weapon menu on vcod-server);
- spawned with the Enfield, 10 in the clip and 60 in reserve; walked, turned, fired and reloaded;
- printed no parse, snapshot or `CG_` error.

On vcod-server the bot fragged it (`K;` lines with `shotgun_mp`, `thompson_mp`, `frag_grenade_british_mp`). The client drew the obituary `Killed by [BOT] ...`, its body on the ground, the `KILLCAM` overlay with `Press USE [F] to respawn`, the killer's view, the `YOU` marker and the countdown, and respawned. `map_restart` reloaded the level on both (loading screen, new gamestate, fresh `cp`) and the script took the client back through the menus, as `SV_MapRestart(0)` does (1.8). `map mp_carentan` did the same with the American weapon menu. VERIFIED live.

With repeated lines and numbers folded, the client's logs against the two servers differed in one line: `Unknown client game command: MP_CONNECTED[BOT] ...^7` for the bot's connect, the line retail itself causes for every connect (15.5's empty `iPrintLn` word). The `challenge:` line's sign varies run to run on both (`rand() << 16` overflows). VERIFIED live.

### 16.3 Event only for the client's own ears

At `developer 1` the client logged `Event EV_STEP_VIEW just for client 0 was sent to other clients` 66 times and `EV_STANCE_FORCE_STAND` once in two minutes against vcod-server, about the bot's body, and never against retail. VERIFIED live. The cgame's `CG_EntityEvent` prints that line (`Com_DPrintf`, Mac, four call sites beside the `just for client` string) when a player entity carries such an event. VERIFIED.

`ClientEndFrame`'s `g_smoothClients` arm (0x80f56f8), like `BG_PlayerStateToEntityState` (0x80dfcc2), does not copy the playerstate's ring: for each event past the playerstate word `+0xc8` (the copy's mark) it runs `G_PlayerEvent`, then appends the event and its parm to the entity's own ring unless the event is in the -1 terminated list at rodata 0x815486c, and finally sets the mark to `eventSequence`. A mark ahead of the ring comes back to it first. VERIFIED, the loop and the stores. The list is 140, 141, 142, 143, 185, 186: `EV_STANCE_FORCE_STAND`, `EV_STANCE_FORCE_CROUCH`, `EV_STANCE_FORCE_PRONE`, `EV_STEP_VIEW`, `EV_BULLET_HIT_CLIENT_SMALL` and `EV_BULLET_HIT_CLIENT_LARGE`. VERIFIED, the words at 0x815486c and the event table of 13. So the entity's `eventSequence` counts only the copied events, apart from the playerstate's.

vcod: `game::cod2::BodyEvents` is the entity's ring and the mark, fed in `end_frame` before the body is built; both spawns (`ClientSpawn` and the follow copy's) zero the mark as the memset does. Copying the whole playerstate ring caused the lines above. That the entity's own count runs on across a spawn is INFERRED (no store to it read in `ClientSpawn`). Test: `game::cod2::tests::a_body_never_carries_the_playerstate_only_events`. A rerun logged none of the lines. VERIFIED live.

### 16.4 What the first round did not see

The first round did not cover the `cp bad` vcod logged under `sv_pure 0` on each entry, a download or pure mismatch (both servers ran stock iwds), or a kill by the retail client, the scoreboard and chat. The second round (16.5 to 16.10) covers them.

### 16.5 The second round

Same setup, against retail `cod2_lnxded` 1.3 (`developer 1`) and `vcod-server --game cod2`, each running a copy of stock `dm`, `tdm` or `sd` (retail took it as a loose file in its homepath) with one thread added to the spectator arm of `Callback_PlayerConnect` and one to `Callback_StartGameType`:

- the connect thread answers the stock menus 4 s and 7 s in, `allies` and the nation's rifle for a real client, `axis` and `kar98k_mp` for the test client; with the cvar `c15_bot 1` the first real player to spawn calls `addTestClient()`, and with `c15_aim 1` a thread puts the test client 200 units in front of the player whenever the two are 400 apart and turns the player's view onto its chest with `setPlayerAngles` every 50 ms;
- the start thread finds the two again after a `map_restart(1)` (they skip the menus then) by a `pers[]` mark on the test client.

The client ran `+exec c15.cfg +vstr c15fire`: fire tapped about every 3 s, each tap stepping a chain that sends `say c15hello`, `say_team c15team` and holds `+scores` twice for about 20 s. `crates/cod2/examples/spectate.rs` joined as a second client (`--join --say-at S --score-at S`), and an rcon sender issued `say` and `status`. vcod-server ran at `RUST_LOG=vcod_server::cod2=debug`, which logs every client command. VERIFIED live, unless a line says otherwise.

### 16.6 Test clients

`SV_AddTestClient` (0x80913da) finds a free slot, builds `connect "\cg_predictItems\1\cl_punkbuster\0\cl_anonymous\0\color\4\head\default\model\multi\snaps\20\rate\5000\name\bot%d\protocol\118"` with a static counter `botport` as the number and the port, runs `SV_DirectConnect` from a type-0 address (where `ClientConnect` runs), marks the client a test client (`client+0xae70c` = 1), sends it the gamestate and runs `SV_ClientEnterWorld`, all inside the call, and returns its entity. VERIFIED. The counter is never reset: on retail the test client added after a tdm map change was `bot1`. VERIFIED live.

- **Its cmds.** `SV_Frame`'s bots' pass (0x8096952) runs 0x809676c for every slot in use whose address type is 0. When the client has an entity and `G_GetClientArchiveTime` is 0, it builds a cmd from `G_random` draws in this order: buttons `|= 1` below 0.5, `|= 0x28` below 0.5; `forwardmove` 127 below 0.33, else -127 below 0.5; `rightmove` the same; each of the three view angles, below 0.33, `360 * G_crandom()`; the weapon byte is `ps.weapon`, the time 0. `SV_ClientThink` (0x8090dac) stores it and, for an active client, runs the game's `ClientThink`. VERIFIED, the calls and the constants 0.5, 0.33 and 360 at 0x814b8f8. That `ClientThink_real` lifts the time 0 to a second behind `level.time` is INFERRED (Q3's clamp; the store at 0x80f5614 VERIFIED). Live, a test client placed by script was 40 units off a moment later.
- **No server commands.** `SV_AddServerCommand` (0x8094750) adds nothing for a test client. VERIFIED.
- **Ping 999.** `SV_CalcPings` (0x80963c8) has no bot arm: a client with no acked frame reads 999 (`Giving bot0 a 999 ping - !count:`). VERIFIED, and live in `status` and the scoreboard. `status` prints its address as `00000000.000000000000:0`. VERIFIED live.

vcod's `addTestClient` on a CoD2 level:

- the builtin gives the slot a fresh CoD2 client state and roster entry before the connect callback and marks it begun after;
- the level gives it a `Cod2Player` before the frame's spawns (`Cod2Level::take_test_clients`);
- the engine installs an active client with retail's userinfo and a bot address (`Cod2Server::install_test_clients`), queues it no command, keeps `botport` across levels and reads its ping 999;
- the bots' pass thinks for it on retail's random cmd (`Cod2Level::test_client_cmd`, the level's `G_random`).

Traps: CoD1's path (a script entity with no engine client) leaves it out of `status` and every snapshot; queueing it commands drops it with `EXE_SERVERCOMMANDOVERFLOW` at an sd round end, since its ring is never acked. Test: `cod2::tests::a_cod2_test_client_joins_and_thinks_on_random_cmds`.

On both servers the retail client killed the test client again and again (`K;...;bot0;...;c15player;enfield_mp;135;MOD_RIFLE_BULLET;torso_lower`) and drew the obituary, and the test client respawned (on its random use presses, which stock dm respawns on: INFERRED). With line numbers and numbers folded, the client's console from `CL_InitCGame` on was the same against both servers in `dm`. VERIFIED live.

### 16.7 Scoreboard and chat

While `+scores` is held the client sends a bare `score` every 2 s (vcod's log: commands 6 to 9, 2 s apart), and both servers answer `b`. For the second client the two answers were byte for byte the same (`b 2 0 0 0 0 0 0 0 1 0 999 0 0`, the client and the test client). The screenshots show `3 players`, each name with score, deaths and ping, the test client at 999; the board is empty on either server for the instant before the first answer. VERIFIED live.

The client's `say` and `say_team`, the second client's `say` and an rcon `say` reached the client as `c15player^7: ^7c15hello`, `c15player^7: ^7c15team`, `c15probe^7: ^7one` and `console: c15rcon` on both servers. Retail logged `say;` for the `say_team` in `dm` and `sayteam;` in `sd`. VERIFIED live.

### 16.8 tdm and sd

**tdm with `scr_tdm_scorelimit 1`.** On retail the kill that reached the limit wrote `W;allies;0;c15player` and `L;axis;0;bot0` but no `K;` line; 10 s later `ExitLevel: executed` and, with no rotation, the map again, the players and the test client back through the menus. VERIFIED live. Stock tdm's `Callback_PlayerKilled` opens with `self endon("spawned")` and calls `checkScoreLimit()`, which waits `waittillframeend` and threads `endMap()`; `endMap` calls `spawnIntermission` on every player, whose `self notify("spawned")` ends the callback's thread while it sits under the `thread` call, before its `logPrint("K;...")`. A VM that runs such a thread on to its next suspend (CoD1's rule, cod11-gsc-language.md 10) writes the `K;` line. A CoD2 level stops it at once (`Vm::set_kill_mid_step`, test `sched::tests::cod2_a_thread_killed_by_a_nested_spawn_s_notify_stops_at_once`); vcod's tdm then logged `W;` and `L;` only and named the next level's test client `bot1`, as retail did. VERIFIED live.

**sd.** On both servers the test client joining the empty axis printed `MP_MATCHSTARTING` and the level restarted 7 s later (`map_restart(1)`); after it each kill by the client ended a round with `W;allies;0;c15player` and `L;axis;0;bot0`, and the next round began about 8 s on. The tenth win ended the map; with no rotation the map came back with the next test client `bot1`, and its joining restarted the match again, the clients rejoining as `J` lines in the order `bot1`, `bot0`, `c15player`, on both. VERIFIED live.

Two script divergences came out of the gametype copies. VERIFIED live:

- `anglesToForward(a) * 200` stopped retail's server: `pair '(1, 0, -0)' and '200' has unmatching types 'vector' and 'int'`. CoD2's `Scr_EvalMultiply` (0x807e290) takes two floats or two ints after the int-to-float promotion of 0x807d7aa, and anything else is `Scr_UnmatchingTypesError`. VERIFIED. A CoD2 level's `*` refuses a vector operand (`Op::MulNumber`, test `sched::tests::cod2_refuses_a_vector_times_a_number`); no stock CoD2 MP script multiplies one (they call `vectorScale`).
- A string literal running over a line break is a compile error on retail (`bad token '"'` at the opening quote). The CoD2 dialect refuses it (`lex::tests::a_cod2_string_may_not_span_lines`); no stock CoD2 script has one.

### 16.9 Pure, and the `cp` under `sv_pure 0`

The client held an extra `main/zzz_c15extra.iwd` (a changed `weapons/mp/enfield_mp`) in both runs. VERIFIED live, unless labelled.

- **`sv_pure 1`.** The client's `cp` listed 13 keyed sums, none the extra iwd's, and vcod logged `cp ok`; retail kept the client for the whole run. That the client read nothing from an iwd off the pure list is INFERRED (the sums, and 11.3).
- **`sv_pure 0`.** The client's `cp` listed 14 sums, then `42`, then the key; without the extra iwd 13 sums, `42` and the key (vcod's log, both runs). That the 14th is the extra iwd's is INFERRED from the count. The Linux build's file open (0x809f958, the one that prints `FS_FOpenFileRead: %s (found in '%s/%s')`) sets the global behind that number to `rand() + 1` when a file comes from a directory rather than an iwd, and `FS_PureServerSetLoadedIwds` (0x8065d9a) clears it only when the server's list is not empty, so never on an unpure server. VERIFIED (both functions). That the Windows client does the same, set off by the loose `.cfg` files it execs, and that 42 is the Windows CRT's first `rand()` plus one, is INFERRED. Rule 3 of 11.4 refuses the `42` (and the extra iwd's sum): `SV_VerifyIwds_f` checks against the keyed sums of every non-localized iwd (0x8065a0e), which reads no `sv_pure`, so retail writes 2 there as vcod's `cp bad` does. VERIFIED, read off both. Nothing acts on it unpure: `SV_ExecuteClientMessage` (0x809114e) and `SV_UserMove` test `sv_pure` before the verdict. VERIFIED. Both servers kept the client. The client lists no localized iwd: `FS_ReferencedIwdPureChecksums` skips them (0x8065c36, `searchpath+0xc`). VERIFIED.

### 16.10 Downloads

The servers held a non-stock iwd the client lacked, under `sv_pure 1`.

- **UDP.** `main/zzz_c15dl.iwd` (61179 bytes: a copy of stock `maps/mp/mp_toujane.gsc`, which the level read, and filler) with `sv_allowDownload 1`, the client at `rate 25000` and `cl_wwwDownload 0`: it printed `Need iwds: @main/zzz_c15dl.iwd@main/zzz_c15dl.iwd`, asked for it, acked blocks 0 to 29 and the empty block 30, loaded the map and spawned; vcod logged `cp ok`. The file was whole on disk after either server. With mp_dltest's 12975639-byte `z_dltest.iwd` at the profile's `rate 5000`, 150 s brought 1.0 MB from retail (which paces a LAN client on its rate, 17.2) and 3.0 MB from vcod (99999 under `dedicated 1`).
- **www.** With `sv_wwwDownload 1` and `sv_wwwBaseURL http://127.0.0.1:8000` (a local HTTP server over the game dir), both servers redirected to `http://127.0.0.1:8000/main/z_dltest.iwd`; the client answered `wwwdl ack`, fetched the 5125333 bytes with one `GET`, sent `wwwdl done`, took the gamestate again, loaded `mp_dltest` and spawned (vcod: `cp ok`).

The client's console from `Need iwds` on was the same against both servers, line for line with numbers folded, but for one `Replace existing download target file` in vcod's www run, caused by a partial file the UDP run before it left on disk. VERIFIED live.

## 17. Server housekeeping

What `cod2_lnxded` does around the game: who gets in, how fast messages go, when a client is let go, files, the master.

Recipe: retail and `vcod-server --game cod2`, both on `mp_toujane` `dm` inside `unshare -rn`. The server sits at 203.0.113.1 with loopback up; a child namespace at 198.51.100.2, joined by a veth, holds an off-LAN client and a stand-in for `cod2master.activision.com`'s address that answers every `getIpAuthorize` with `ipAuthorize <challenge> accept KEY_IS_GOOD 4242 ""` and logs what reaches it. Nothing leaves the namespace. The out-of-band side is a Python client sending plain-text `getinfo`, `getstatus`, `getchallenge` and `connect` from a fresh socket a step; the in-session side is `crates/cod2/examples/spectate.rs` (`--rate`, `--snaps`, `--password`, `--www`, `--log-packets`, `--silent-after`, `--linger`, `--say-burst`, `--download-dir`, `--download`) and `vcod --net-probe ... --game cod2`. `crates/server/tests/fixtures/cod2/oob-*.txt` hold the out-of-band replies; `cod2::house_tests` replays them on vcod's server byte for byte and gates the rest of this section, each test naming its rule.

### 17.1 Getting in

`SV_GetChallenge` (0x808d18e) finds a slot by full address among those not marked connected (+0x28), else takes the oldest by its time (+0x18) and makes a new challenge. VERIFIED. On the LAN path it stores `svs.time` at +0x1c (the ping time) and answers at once. VERIFIED. `Sys_IsLANAddress` (0x80d5638) is 10/8, 127/8, 169.254/16, 172.16/12, 192.168/16, or the first three octets of one of the host's own addresses (the table `NET_GetLocalAddress` fills). VERIFIED. 203.0.113.1, the server's own address in the namespace, was not LAN to it: the challenge went to the authorize server. VERIFIED live. Off the LAN the ping time is stamped when `ipAuthorize accept` comes back (0x808da2a). VERIFIED.

`SV_DirectConnect` (0x808e2aa), after the protocol (1.2), in order (VERIFIED, read off the function; the live results VERIFIED live):

1. A client slot with the same base address and the same qport or port whose `lastConnectTime` is under `sv_reconnectlimit` (3) seconds old makes the connect silently ignored.
2. Every address, 127.0.0.1 included, needs a challenge slot matching address and challenge: `NET_IsLocalAddress` is the loopback address type, not 127/8. Live, a loopback connect was logged `Client 4 connecting with 0 challenge ping from 127.0.0.1`.
3. The ping is `svs.time` less the ping time, kept in the slot (+0x24) once measured; the slot is marked connected. Off the LAN, `sv_minPing` refuses a lower ping with `error\nEXE_ERR_HIGH_PING_ONLY` and `sv_maxPing` a higher one with `error\nEXE_ERR_LOW_PING_ONLY`. Live with 100 and 300: a connect at once read ping 0 and was refused HIGH, one 400 ms after the challenge read 400 and was refused LOW, one 200 ms after read 200 and got in. The ping is on the frame clock (multiples of 50).
4. The same client again (base address, qport or port) takes its old slot.
5. Otherwise `password` against `sv_privatePassword` (`strcmp`, so an empty password matches an empty one) starts the slot search at 0, else at `sv_privateClients`; a zombie occupies its slot. None free: `error\nEXE_SERVERISFULL`. Live with `sv_maxclients 2`, `sv_privateClients 1`: an off-LAN client took slot 1, a public one then got `EXE_SERVERISFULL`, `password priv` took slot 0, and another `password priv` got `EXE_SERVERISFULL`.
6. `ClientConnect` (0x80f8e7a) refuses with `GAME_INVALIDPASSWORD` when the client is not a bot, `g_password` is set and not `none` (any case), and `strcmp(g_password, password)` differs; the engine sends it as `error\nGAME_INVALIDPASSWORD`. Live: no password, a wrong one and `SECRET` for `secret` all got it; `secret` got `connectResponse`.
7. `connectResponse`, then `SV_Heartbeat_f` when the clients past `CS_ZOMBIE` number 1 or `sv_maxclients`.

`getinfo` (`SVC_Info`, 0x809537c): `clients` counts the slots past `CS_ZOMBIE`, present only when non-zero; `sv_maxclients` is `sv_maxclients - (sv_privateClients - private slots in use)`, present only when positive. VERIFIED, and live: 1 with no one on (2 slots, 1 private), 2 with a private and a public client. `getstatus` (`SVC_Status`, 0x8094c84) lists every client past `CS_ZOMBIE` as `<score> <cl->ping> "<name>"`, 999 for one not yet in the world. VERIFIED live. Under `g_password` `getinfo` carries `pswrd\1` after `gametype`/`pure` and `getstatus` `pswrd\1`. VERIFIED live.

vcod: `oob.rs` follows 1 to 7 and both replies; it never asks the authorize server, so an off-LAN client's challenge is answered at once and its ping runs from that answer. `g_password`'s check sits in the engine's connect, before the level's `ClientConnect`. Every out-of-band reply of the two captures matched retail's bytes past the challenge number, live and in `cod2::house_tests`.

### 17.2 Pacing

`SV_UserinfoChanged` (0x80907cc): `rate` is 99999 for a LAN client while `dedicated` is not 2, else the userinfo's clamped to 1000..90000, 5000 when empty; `snapshotMsec` is 1000 over `snaps` clamped to 1..30, 50 when empty; `cl_wwwDownload` non-zero sets +0x20e08. VERIFIED. Each value is read by glibc's `atoi` (`rate` at 0x809088a, `snaps` 0x809090e, `cl_wwwDownload` 0x80909e2, all `call atoi@plt`), on i386 a saturating `strtol`, as 1.1d's `__strtol_internal` (cod11-server-handshake.md, "Rate"). VERIFIED (the calls). INFERRED, off glibc's `strtol` saturating at `LONG_MAX`: `rate 99999999999` reads 90000 and `snaps` of the same 33. vcod reads them with `vcod_common::text::atoi` on both servers. Retail's `dedicated` reads 2 whatever the command line sets (1.6), so on retail a LAN client always paces fragments and downloads on its userinfo `rate`. VERIFIED live: a loopback client with `rate 5000` got its gamestate fragments 350 ms apart.

`SV_RateMsec` (0x809aad0) is `(min(bytes, 1500) * 1000 + 48000) / rate`, the rate first lowered to a non-zero `sv_maxRate` (raised to 1000 at the least). VERIFIED. `SV_SendMessageToClient` (0x809aba2) after each message: a loopback or LAN address is due again next frame (`svs.time - 1`); anyone else at `svs.time + max(RateMsec(len), snapshotMsec)`, `rateDelayed` set when the rate was the larger, and at least a second on for a client not active and not downloading. VERIFIED. `SV_SendClientMessages` (0x809bcce) gives a client whose time came either its next fragment, its time moved on by `RateMsec(unsentLength - unsentFragmentStart)`, or a new message. VERIFIED. `SV_WriteSnapshotToClient` (0x80985c8) adds 1 to `snapFlags` while `rateDelayed`, and 2 unless the client had a snapshot while active. VERIFIED. `SV_SendClientSnapshot` (0x809adea) builds and writes a snapshot and the server commands only for an active client or a zombie; anyone else gets the acknowledge, any download, and `svc_EOF`. VERIFIED, and live: a primed client that sent no move was sent 9-byte messages a second apart.

Live, off the LAN (VERIFIED live): `rate 5000 snaps 20` got the first gamestate fragment at once, the next 989 ms later (not active), then one each 350 ms (309 ms rounded up to the 50 ms frame), then snapshots each 50 ms; `25000 snaps 10` fragments each 100 ms and snapshots each 100 ms; `25000 snaps 20` fragments each 100 ms, snapshots each 50 ms; `1000 snaps 30` fragments each 1550 ms. vcod's server (`engine::link::pace`, `Cod2Server::transmit`, `send_messages`) gave the same gaps against the same client (VERIFIED live) and `an_off_lan_client_is_paced_by_its_rate_and_snaps` pins the 350 ms and 100 ms. vcod's `dedicated` defaults to 1 (17.6), so its LAN clients take one fragment a frame at 99999.

### 17.3 Zombies and timeouts

`SV_DropClient` (0x808f02e) on a client not already a zombie: `SV_FreeClient` (the download closed, `ClientDisconnect`), state `CS_ZOMBIE` (1), the challenge slot of a non-bot's address unmarked; then, unless the reason is `EXE_DISCONNECTED`, `e "\x15<name>^7 \x14<reason>"` to everyone past `CS_CONNECTED` (`\x14` only when the reason is a localized key), `J <slot>` to them, and the client `w "<reason>"` (an unlocalized reason: `w "<name>^7 <reason>" PB`); a heartbeat when no client past `CS_ZOMBIE` is left. VERIFIED. `SV_PacketEvent` (0x80960e2) still runs a zombie's packets through the netchan and stops before `lastPacketTime`; a datagram from an address and qport no slot holds is answered with an out-of-band `disconnect`. VERIFIED.

`SV_CheckTimeouts` (0x809657e), once a `SV_Frame`, for each non-bot: a zombie whose `lastPacketTime` is older than `sv_zombietime` (2) seconds is freed; a client past `CS_ZOMBIE` silent longer than `sv_timeout` (240) seconds counts a frame, and on the sixth is dropped `EXE_TIMEDOUT` and freed at once, with no zombie turn; anyone else's count is reset. VERIFIED (defaults at `SV_Init` 0x8093adc).

Live (VERIFIED live): a loopback client's `disconnect` was followed by 40 messages a frame apart, the first carrying `w "EXE_DISCONNECTED"`, and the server logged `Going from CS_ZOMBIE to CS_FREE` 2 s after `Going to CS_ZOMBIE`; off the LAN by 2 messages a second apart. Under `sv_timeout 3` a client that went silent was sent messages until 3.2 s after its last packet, then nothing. vcod's server gave 41 and 2 messages and 3.2 s (VERIFIED live); `a_zombie_is_sent_its_reason_until_sv_zombietime` and `a_silent_client_times_out_with_no_zombie_turn` pin 40, 2 and 3250 ms.

The order of a frame's messages. `SV_SendClientMessages` (0x809bcce) runs once over the slots, in slot order. Any slot whose state is not 0 and whose time has come gets its next fragment or `SV_SendClientSnapshot` (0x809adea), then `SV_SendClientVoiceData`, in its own turn. There is no separate pass for zombies or for clients not in the world. VERIFIED (the loop's only conditions are `state != 0` and the time test). `SV_CheckTimeouts` (0x809657e) frees the stale zombies before it, as CoD1's three servers do (cod11-server-handshake.md, "The order of a frame's messages"). VERIFIED. Live, loopback `mp_toujane` in an `unshare -rnm` namespace, an `AF_PACKET` socket on `lo` logging the server's datagrams in send order (VERIFIED live):

- A probe held at `CS_CONNECTED` at slot 0 (a relay passed only its out-of-band packets; `status` read `CNCT`) and a probe in the world at slot 1: slot 0's 9-byte message went out first in all 309 frames after the join.
- A zombie at slot 0 (a probe's `disconnect`) and a probe at slot 1: the zombie's 159-162 byte message went out first in every one of its 40 frames.
- The only frames out of order held the packet handler's own sends on a connect, or an rcon reply.

### 17.4 Flood protection

`SV_ClientCommand` (0x8090bac) past `CS_PRIMED` with `sv_floodProtect` on and no local client: a command other than `team `, `score ` and `mr ` inside 800 ms of the last such one runs the engine's own commands but not the game's. VERIFIED (vcod's implements it, 3.4.1). Live: three `say`s 100 ms apart from a spectator reached the others as one line, the server logging `client text ignored` for the second and third. VERIFIED live, the same on vcod's server.

### 17.5 Downloads

The client commands (`.data` 0x817d9e0): `download <name>` closes any transfer and stores the name (0x808f940); `nextdl <n>` acks block `n` when it is the one due, else drops the client `broken download`, and the ack of the empty block closes the transfer (0x808f83c); `retransdl <n>` restarts the window at the block due (0x808f7fc); `stopdl` closes it (0x808f780); `donedl` sends the gamestate again (0x808f7d0). VERIFIED.

`SV_WriteDownloadToClient` (0x808fdc2), in every message but a zombie's, when a name is set and no `wwwdl ack` is pending: with `sv_wwwDownload` and the client's `cl_wwwDownload`, unless its last www transfer failed (which clears the flag), `SV_WWWRedirectClient` (0x808fc0c) opens the file for its size and writes op 5, short -1, `<sv_wwwBaseURL>/<name>`, the size, and `sv_wwwDlDisconnected` as a long, then clears the name; a file that does not open falls through. Otherwise `sv_allowDownload` 0 refuses (`EXE_AUTODL_SERVERDISABLED_PURE` with `sv_pure` on), `FS_iwIwd(name, "main")` refuses `EXE_CANTAUTODLGAMEPAK`, a file that does not open or is empty `EXE_AUTODL_FILENOTONSERVER`, each `<key>\x15<name>` behind op 5, short 0, long -1. Blocks are 2048 bytes, eight in flight past the acked one, the empty block after the last; a message carries `(rate * snapshotMsec / 1000 + 2048) >> 11` of them (1 at rate 0); a window sent out is sent again from the acked block once 1000 ms pass with no ack; block 0 carries the file's size before its length. VERIFIED. `SV_ExecuteClientMessage` (0x809114e) reads a downloading client's messages whatever their `serverId`. VERIFIED.

The transfer's speed is set by the fragments, not the window. `Netchan_Transmit` (0x806bc6c) sends a message of 1300 bytes or more as 1300-byte fragments, the first at once. VERIFIED (the `0x514` compares). `SV_SendClientMessages` (0x809bcce) gives a client whose time came either one fragment or a new message, and runs once a `SV_Frame` (0x80969b0) after the frames owed at `1000 / sv_fps`. VERIFIED. A fragment's wait is `SV_RateMsec` of the bytes left, with no LAN shortcut (0x809aad0 has none). VERIFIED. So any rate whose wait is under a frame moves one fragment a frame, at most 1300 bytes each 50 ms at `sv_fps 20`. INFERRED. At rate 90000 or 99999 and `snaps 20` a message carries three blocks, 6.2 KB of payload, sent as five full fragments and a sixth short one, six frames: 3 blocks each 300 ms, 20.5 KB/s. INFERRED (the arithmetic of the rules above). Live on loopback, the spectate example asking `--download main/z_dltest.iwd` (5125333 bytes) for 40 s with `snaps 20` (VERIFIED live): retail at `rate 90000` sent 396 blocks in 39.3 s, 20.2 KiB/s; vcod-server sent the same 396 in 39.3 s at its LAN rate 99999 (`dedicated` 1), and again at `dedicated 2` with `rate 90000`. Retail's `dedicated` is always 2 (17.2), so its loopback clients download at their userinfo rate. INFERRED. The Windows client under Wine, userinfo `rate 5000`, took 995328 bytes of the same file from retail in 150 s and 2994176 from vcod-server, whose LAN rate is 99999. VERIFIED live. 1.1d sends its fragments the same way (cod11-server-handshake.md, "Message pacing"). `a_lan_download_moves_a_fragment_a_frame` pins one datagram a frame and three blocks a message.

`wwwdl` (`SV_WWWDownload_f`, 0x808f97c): before a redirect, anything drops `PC_PATCH_1_1_UNEXPECTEDDOWLOADMESSAGE`; `ack` marks the ack; `bbl8r` drops `PC_PATCH_1_1_DOWNLOADDISCONNECTED`; before the ack anything else drops the unexpected reason; `done` clears the transfer; `fail` and `chkfail` clear it, mark the failure and send the gamestate again. VERIFIED (the strings `ack`, `done`, `fail` at 0x814a486, 0x814a502, 0x814a507).

The client (Mac): `CL_InitDownloads` (0x14d84c), under `cl_allowDownload` (registered 1 in `CL_InitOnceForAllClients`, Mac 0x1485e2), asks `FS_CompareIwds` (0x80654a0) for the referenced iwds that are neither `FS_iwIwd` (on the bare name, so `main/iw_00` is stock) nor `_svr_` and whose checksum no search path iwd has, and walks the `@remote@local` list: `CL_BeginDownload` (0x14d668) sends `download <remote>`; `CL_ParseDownload` (0x1d59c8) writes each block due, sends `nextdl <n>`, asks `retransdl <due>` for a block past it, raises `Com_Error` on a refusal, and on the empty block renames the file and goes to the next; `CL_DownloadsComplete` (0x14d368) sends `donedl` after a transfer, and with none sends the `cp` and enters the game. VERIFIED. A redirect (`CL_ParseWWWDownload`, 0x1d57c8) answers `wwwdl ack`, `wwwdl fail` when the HTTP transfer cannot start, `wwwdl bbl8r` with flag 1. VERIFIED.

Live, retail with a 63853-byte `main/zzz_vcodtest.iwd` (one stored copy of `maps/mp/gametypes/dm.gsc` and filler) in its homepath (VERIFIED live): `sv_referencedIwdNames` named `main/zzz_vcodtest` first, as the gametype script came from it; vcod's spectate example asked for it, took blocks 0 (size 63853) to 31 (365 bytes) and the empty block 32, each one message (a block a 100 ms over loopback at `rate 25000`, the 2100-byte message going as two fragments; off the LAN 150 ms), sent `donedl`, got the gamestate again and its `cp` was taken. `main/iw_00.iwd` was served (block 0 size 162248089), `main/localized_english_iw11.iwd` refused `EXE_CANTAUTODLGAMEPAK`, `main/nothere.iwd` `EXE_AUTODL_FILENOTONSERVER`, and `sv_allowDownload 0` `EXE_AUTODL_SERVERDISABLED_PURE`, or `EXE_AUTODL_SERVERDISABLED` with `sv_pure 0`. With `sv_wwwDownload 1` a client with `cl_wwwDownload 1` got the redirect `http:/main/zzz_vcodtest.iwd`, size 63853, flags 1 (`+set sv_wwwBaseURL http://...` reaches the engine as `http:`: the command line drops what follows `//`), answered `ack` and `fail`, got the gamestate and then the file over UDP; one without got the blocks; with `sv_allowDownload 0` the redirect still came first and the fallback was refused. vcod's server gave the same messages in every case (VERIFIED live), apart from the LAN timing of 17.2.

The client's side of a join to a server whose iwds it lacks (Mac `cod2mp_mac13_i386`):

- Where files go. `Sys_DefaultHomePath` (Mac 0xc4932) returns nothing, so `FS_RegisterDvars` (Mac 0x3285a) registers `fs_homepath` as `fs_basepath`'s value, the working directory (`Sys_DefaultInstallPath`, Mac 0xc493a), the install. VERIFIED (Mac). No Windows decompile was read; that the Windows client does the same is INFERRED (the Q3 lineage's Windows `Sys_DefaultHomePath` returns nothing). `FS_SV_FOpenFileWrite` (Mac 0x46304) builds `<fs_homepath>/<name>` with `FS_BuildOSPath`, creates the directories and opens it, with no check on the name's characters. VERIFIED. So a mod's iwd lands in `<install>/<fs_game>/` and any name the server references is written as given, `~`-padded ones too. INFERRED.
- The order. `CL_ParseGamestate` (Mac 0x1d5534) calls `CL_SystemInfoChanged` (0x1d535a), then `FS_ConditionalRestart(checksumFeed)` (0x37266), then `CL_InitDownloads` (0x14d84c). INFERRED (call order). `CL_SystemInfoChanged` hands `sv_iwds`/`sv_iwdNames` and `sv_referencedIwds`/`sv_referencedIwdNames` to the file system and, when no local server runs, sets every systeminfo key as a dvar, `fs_game` included (its 0x101c flags carry the systeminfo bit 0x8). VERIFIED (the calls and `Info_NextPair` loop). So the server's mod is on the search path before the iwds are compared. INFERRED.
- The list. `FS_CompareIwds` (Mac 0x47b14) passes over a referenced name that `FS_iwIwd(name, "main")` calls stock, one holding `_svr_`, and one whose checksum some search path iwd carries (whatever its name); for the rest it writes `@<name>.iwd@<local>`, `<local>` being `<name>.iwd`, or `<name>.%08x.iwd` (the listed checksum) when `<fs_homepath>/<name>.iwd` already opens. VERIFIED. With `cl_allowDownload` 0 it prints `WARNING: You are missing some files referenced by the server:` and `CL_InitDownloads` goes on to `CL_DownloadsComplete`, which loads the map. VERIFIED (the string, the call). A map that is not there then fails in `Com_LoadBsp` (Mac 0x11154e) with `EXE_ERR_COULDNT_LOAD\x15<path>` ("Couldn't load &&1"). VERIFIED.
- A transfer. `CL_ParseDownload` raises the refusal's text as `Com_Error(1, ...)`, which ends the connection with it. VERIFIED. A file it cannot create prints `Could not create %s`, sends `stopdl` and goes to the next file: the join goes on without it. VERIFIED (string and calls). The finished file is renamed into place with no checksum test. INFERRED (none in the function). After the last, `CL_DownloadsComplete` (0x14d368) runs `FS_Restart(checksumFeed)` and sends `donedl`; on the gamestate that brings, the list is empty and it runs `CL_InitCGame`, then sends `cp ` (stored as `Va `, two bytes shifted by 13 and 15) with `FS_ReferencedIwdPureChecksums`, three packets. VERIFIED.
- A redirect. `CL_ParseWWWDownload` (0x1d57c8): flag 2 opens the URL in a browser and quits; else `wwwdl ack`, `DL_BeginDownload` into `<fs_homepath>/<name>`, and `wwwdl fail` with `Failed to initialize download for '%s'` when it cannot start; flag 1 adds `wwwdl bbl8r` and the disconnected mode. VERIFIED. `CL_WWWDownload` (0x1625cc) on the end renames the file and sends `wwwdl done` (in the disconnected mode `reconnect`) and goes to the next file; on a failure `Download failure while getting '%s'` and `wwwdl fail` (in the disconnected mode a `Com_Error`). VERIFIED. `cl_wwwDownload` is registered 1 with flags 0x1003 in `CL_InitOnceForAllClients` (Mac 0x1485e2), so it rides the userinfo. VERIFIED (flags); the userinfo bit is INFERRED (Q3's `CVAR_USERINFO` 2).
- The connect screen. `UI_DisplayDownloadInfo` (Mac 0x156aa6) draws `EXE_DOWNLOADING` ("Downloading:"), the name with `(<n>%)`, an estimate or `EXE_ESTIMATING`, `(<got> EXE_OF <size> EXE_COPIED)` and the rate; `UI_ReadableSize` (0x151506) writes bytes up to 1024, KB up to 1 MB, MB past it. VERIFIED. MB's two decimals are INFERRED.

A public server's join (a playtest log, not a probe capture) referenced `veteran/rt` (the server's `fs_game`) and `main/~~~~...~` (54 tildes, a name that sorts last so the iwd loads over the others). Retail writes `<install>/veteran/rt.iwd` and `<install>/main/~~~...~.iwd`, restarts its file system over `veteran/`, sends `donedl`, and takes the gamestate again and its `cp`. INFERRED (from the functions above). Trap: a path check that allows only `main/` and `[A-Za-z0-9_.-]` refuses both names, and reporting that refusal as a drop reads as "the server dropped the connection" when the server dropped nothing.

vcod: the server side is `house::Download` (`DownloadMsg` writes the three shapes), the file read from `<fs_basepath>/<name>`, which `main.rs` sets to the game dir; it serves any `<dir>/<file>.iwd` that stays inside it (`house::servable`), where retail's `FS_SV_FOpenFileRead` serves any file. The client side is `vcod_cod2::net::download` (`missing_iwds`, `held_checksums`, `local_iwd_name`, `safe_iwd_path`, `Downloads`) and `play::cod2::Cod2Session`: the connect (`ConnectOpts`: `cl_allowDownload`, `cl_wwwDownload`, the install as homepath) returns once the gamestate's downloads are done and the gamestate after `donedl` is in (`Cod2Session::settled`); a www redirect is fetched over HTTP (`vcod_common::net::http`); `take_reopen` says when the search path must open again (a file landed, or `fs_game` moved), the app reopens it (`App::reopen_cod2_fs`) before the map loads, and the `cp` goes from the first frame after the load. vcod refuses only names that would leave the homepath (`.`/`..`, more than one directory, a backslash, a colon, a control character) and passes them over as `Could not create` does; a refusal ends the join with the server's text, localized in the popup. A file whose checksum is not the listed one is deleted and the join goes on without it, where retail keeps it. Flag 1 (`sv_wwwDlDisconnected`) is fetched connected, with no `bbl8r`; flag 2 ends the join. A gamestate for a new map mid-game downloads the same way while the old map stays drawn; its progress shows on the same line.

Live, `vcod --net-probe ... --game cod2` from an install copy without the files (VERIFIED live): `mp_dltest` is mp_toujane's `.d3dbsp`, `.gsc` and `.csv` renamed into `main/z_dltest.iwd` (5125333 bytes). Retail `cod2_lnxded` with `sv_pure 1`, `sv_allowDownload 1`: the probe asked for `main/z_dltest.iwd`, took it over UDP (about 250 s at `rate 25000` over loopback), reopened its search path, loaded the map, sent its `cp` and was kept. With `fs_game veteran`, `veteran/rt.iwd` and a 54-tilde iwd in `main/` beside it, the server referenced both, the probe wrote `<install>/veteran/rt.iwd` and `<install>/main/~~~...~.iwd`, reopened over `veteran`, and its `cp` was kept; the same with `sv_wwwDownload 1` and `sv_wwwBaseURL` at a local HTTP server fetched both over HTTP (`wwwdl ack`, `wwwdl done`) and was kept. vcod-server gave the same over UDP and over HTTP, and with `sv_allowDownload 0` the probe ended with `EXE_AUTODL_SERVERDISABLED_PURE\x15main/z_dltest.iwd` and no panic.

### 17.6 The master

`SV_MasterHeartbeat` (0x8096ed6) with `dedicated` 2: `heartbeat COD-2\n` each 180 s and `SVC_Status` to the master each 600 s, both to `cod2master.activision.com` port 20710 (`SV_MasterAddress` 0x8096e0c, `BigShort(0x50e6)`); `SV_Heartbeat_f` makes the next frame beat; `SV_MasterShutdown` (0x8097032) sends `heartbeat flatline\n`. VERIFIED. Live (VERIFIED live, the stand-in's log): the first frame sent the heartbeat and a `statusResponse` whose `challenge` was `mp_toujane` (`SVC_Status` reads `Cmd_Argv(1)`, the `map` command's argument still tokenized), and each client connect that made 1 or `sv_maxclients` clients sent another heartbeat.

`SVC_Status` (0x8094c84) is 1.5's reply, with `challenge` set to `Cmd_Argv(1)`. VERIFIED. Its `mod` test tokenizes `sv_referencedIwdNames` on a pure server without `fs_game`, so the next status reads that list's second word as `Cmd_Argv(1)`. INFERRED from the shared tokenizer.

vcod beats `master.cod.pm`'s CoD2 list instead, under `--set dedicated=2` only (`crate::master::Masters`, its own `dedicated` defaulting to 1). It sends the status every 600 s too (`Masters::status`, `STATUS_MS`), to the master the heartbeat resolved, with `challenge` the last tokenized line's first argument (the command line's map at startup, as the stand-in's log showed). Under the master's one packet a second the status of the first frame waits a second behind the heartbeat, where retail sends both together. `the_master_gets_a_status_every_ten_minutes` gates it against a loopback address only. `SV_Heartbeat_f`'s forced beat on a first or full server and on the last client's drop are the connect's and `drop_client`'s calls into it; `a_first_client_forces_a_heartbeat` gates the first.

### 17.7 Bans

The commands (`SV_AddOperatorCommands` 0x808cca6): `onlykick` (0x808c684, `SV_KickUser_f` alone), `kick` and `tempBanUser` (both `SV_TempBan_f` 0x808c55e), `tempBanClient` (`SV_TempBanNum_f` 0x808c6bc), `clientkick` (0x808c6a0, `SV_KickClient_f` alone), `banUser` (`SV_Ban_f` 0x808c5a8), `banClient` (`SV_BanNum_f` 0x808c5fc), `unbanUser` (`SV_Unban_f` 0x808c650). VERIFIED (the `Cmd_AddCommand` calls; `kick` is the string at 0x814994c). So 1.3's `kick` is a temporary ban, unlike 1.1's.

- The kick (`FUN_0808c316`): a loopback client gets `e "EXE_CANNOTKICKHOSTPLAYER"`; otherwise the name is copied and `I_CleanStr`'d, the client dropped with `EXE_PLAYERKICKED`, and its guid (`cl+0xae704`) returned. `kick all` kicks every client and returns 0. VERIFIED.
- `SV_TempBan_f` and `SV_TempBanNum_f`: with a non-zero guid, `%s (guid %i) was kicked for cheating` and the guid with `svs.time` into a 16-entry table at 0x844010c, the first free entry, else the oldest (`FUN_0808d72e`, `FUN_0808d6ce`). VERIFIED. The check (`FUN_0808d5ac`) refuses a non-zero guid with an entry whose age is not above `sv_kickBanTime * 1000.0` (float at 0x8149b7c), the dvar read at the check; `sv_kickBanTime` is a float, 300, 0..3600 (`SV_Init`). VERIFIED.
- `SV_BanClient` (0x808d75a): loopback as above; a guid of 0 prints `Can't ban user, GUID is 0`; a guid `SV_IsBannedGuid` finds prints `This GUID (%i) is already banned`; otherwise `ban.txt` opens for append and gets `%i %s\r\n`, the guid and the cleaned name, and the client is dropped with `EXE_PLAYERKICKED`. VERIFIED. 1.5b returns silently on a guid of 0 (cod11-server-handshake.md, "1.5 permanent bans"); 1.3 prints.
- `SV_IsBannedGuid` (0x808d630): 0 never; otherwise `ban.txt` through `FS_ReadFile`, each line's first `Com_Parse` token `atoi`'d against the guid, the rest of the line skipped. VERIFIED.
- `SV_Unban_f`: an argument count other than 2 prints `Usage: unban <client name>`, with no `Server is not running` check. `SV_UnbanClient` (0x808d886) prints nothing without a `ban.txt`; it cleans the name, cuts every line whose text past the guid and the blanks after it equals the name ignoring case up to a `\r` or `\n`, writes the file back whether or not anything went, and prints `unbanned %i user(s) named %s` or `no banned user has name %s` with the cleaned name. VERIFIED. 1.5b's walk, measured there, is the same shape.
- The checks run only at the authorize server's `accept` (`SV_AuthorizeIpPacket` 0x808da2a, 1.3): the guid is `atoi(argv[4])`, `ban.txt` first, then the table; a hit prints `rejected connection from permanently|temporarily banned GUID %i`, sends `error\n\x15You are permanently banned from this server` (0x8149d60) or `...temporarily...` (0x8149de0), and clears the challenge. VERIFIED. A LAN client (`net_lanauthorize` 0) and one the authorize server timed out on are answered without it and keep guid 0 (`Connecting player #1 has a zero GUID`). VERIFIED (`SV_GetChallenge` 0x808d18e; the line live).
- `addip`, `removeip` and `listip` are the game's (0x811bf64): they keep the `g_banIPs` dvar and a filter table nothing reads; 1.3 has no `G_FilterPacket` call. INFERRED (no other reference to the table at 0x8850e00).

VERIFIED live, retail on mp_toujane, an off-LAN client at 198.51.100.2 with the stand-in authorize answering `accept KEY_IS_GOOD 4242` and a LAN one, rcon 0.7 s apart:

| command | result |
|---|---|
| `status` | guid 4242 and 0 |
| `banClient 1` (LAN) | `Can't ban user, GUID is 0` |
| `banUser badguy` (on `^1Bad^7Guy`) | `0:^1Bad^7Guy EXE_PLAYERKICKED`; `ban.txt` reads `4242 BadGuy\r\n` |
| second `banUser badguy` | `Player badguy is not on the server` (`SV_FreeClient` 0x808e1ea empties the zombie's userinfo, so its name is gone) |
| next connect | `error\n\x15You are permanently banned from this server` |
| `unbanUser ^2BADGUY` | `unbanned 1 user(s) named BADGUY` |
| `unbanUser badguy` | `no banned user has name badguy`; `ban.txt` left empty |
| `kick cheat` | `0:cheat EXE_PLAYERKICKED`, then `cheat (guid 4242) was kicked for cheating`; next connect `error\n\x15You are temporarily banned from this server` |
| `sv_kickBanTime 1`, 1.5 s later | the connect goes through |
| `onlykick` | bans nothing |

`developer 1` adds `Going to CS_ZOMBIE for <name>` ahead of each kick's line.

vcod (`engine::bans`, shared with 1.5 and UO, whose `ban.txt` is this one; `oob.rs`): every command, reply and file line above. vcod asks no authorize server, so an off-LAN client's guid is its IPv4 address read as a big-endian `i32` (`engine::bans::guid_of`) and a LAN client's is 0, as on retail; `status`, the prints and `ban.txt` carry that number. The two checks run at `getchallenge`, which vcod answers at once, with retail's prints and refusals. `ban.txt` is the `--ban-file` path, read and written at every use as retail does; without the flag the file lives for the run (retail's is `<fs_homepath>/main/ban.txt`). The script's `getGuid` stays 0. Gates: `house_tests::ban_txt_refuses_a_banned_guid_until_unbanned`, `kick_bans_a_guid_for_sv_kick_ban_time`, and `engine/bans.rs`'s unit tests.

## 18. The client on a bad link

The client's own rules for its clock, its packets, a silent server and a userinfo change, from the Mac binary's `CL_*` and `CG_*`. The `cl` offsets are the Mac layout: `cl.snap` at +0x18 (`valid`, then `snapFlags` +0x1c, `serverTime` +0x20, `messageNum` +0x24, `ping` +0x2c, the playerstate from +0x34), `serverTime` +0x26f0, `oldServerTime` +0x26f4, `oldFrameServerTime` +0x26f8, `serverTimeDelta` +0x26fc, +0x2700 the snapshot time before the newest, `extrapolatedSnapshot` +0x2704, `newSnapshots` +0x2708. VERIFIED (the loads and stores in the functions below; the names follow Q3's order, INFERRED).

### 18.1 The clock

`CL_SetCGameTime` (Mac 0x17ba76) runs as 1.1's with no `cl_timeNudge`: CoD2 registers no such dvar. VERIFIED (no `cl_timeNudge` string in the binary). The time is `realtime + serverTimeDelta`, held at or past `oldServerTime`, and a frame with `realtime + delta >= snap.serverTime - 5` sets `extrapolatedSnapshot`, before the adjust. VERIFIED. The first snapshot sets the delta to `snap.serverTime - realtime` and `oldServerTime` to the snapshot's time, and consumes `newSnapshots`. VERIFIED. A snapshot older than the last frame's is `Com_Error` off loopback and a re-peg on it. VERIFIED (the `"localhost"` compare and the error string).

`CL_ParseSnapshot` (Mac 0x1d5cb2) copies the old `cl.snap.serverTime` to +0x2700 before it stores the new snapshot, and sets `newSnapshots`. VERIFIED. `CL_AdjustTimeDelta` (Mac 0x17b544) is not 1.1's. VERIFIED (the arithmetic below, read off the stores):

- The target is `newDelta = prevSnapTime - realtime - 5`: the time drawn as a snapshot comes in aims 5 ms short of the one before it, a full interval behind the newest. 1.1's is `snap.serverTime - realtime`.
- `short = newDelta - delta`. At or below 0 its size is what counts; above 0 the interval (`snap.serverTime - prevSnapTime`) is subtracted, all of it counting once the interval passes 500 ms, and a `short` inside one interval counts as nothing.
- Past 500 resets: the delta to the target, `serverTime` and `oldServerTime` to the snapshot's time (`<RESET>` under `cl_showTimeDelta`). Past 100 the delta moves halfway (`<FAST>`).
- Otherwise, with `com_timescale` 1 only: after an extrapolated frame the flag clears and the delta drops 2; else it moves 1 toward the target, either way. 1.1 adds 1 whatever the target.
- Nothing while a demo plays.

vcod: `ServerClock::cod2` / `update_cod2` (`crates/client/src/play/clock.rs`); `cod2_aims_one_snapshot_behind` pins each arm, `cod2_settles_a_snapshot_behind_under_jitter` the drift. The time a frame draws at is the cgame's (18.5), not the clock's alone.

### 18.2 Silence: Connection Interrupted, timeouts, drops

`CG_DrawDisconnect` (Mac 0x1cd848) reads the cmd `CL_GetCurrentCmdNumber() - 0x7f`, the oldest of the 128 kept, and draws when `cg.snap->ps.commandTime < cmd.serverTime <= cg.time`: `CGAME_CONNECTIONINTERUPTED` (sic; the English is `Connection Interrupted`) through `UI_DrawText` in font 0 at scale 0.5, x `(640 - width) / 2`, y 100, style 3, and unless `cg.time & 0x200`, the `net_disconnect` material at (296, 416), 48 by 48. VERIFIED. `CG_Draw2D` (Mac 0x1d114a) calls it after the board and the lagometer, past the intermission return and only while the HUD menus show. VERIFIED. Before 128 cmds were made that slot reads 0, so nothing draws. INFERRED.

`CL_Frame` (Mac 0x14b182): past state 4 a client with no packet for `cl_timeout` seconds (200) counts a frame in `cl` +0x14 and on the sixth `Com_Error`s `EXE_ERR_SERVER_TIMEOUT` ("Server connection timed out."); any other frame resets the count. VERIFIED. 1.1's `EXE_SERVERCONNECTIONTIMEDOUT` is not in CoD2's `exe.str`. VERIFIED (the iwd's string files). The OOB `disconnect` (1.10) errors `EXE_SERVER_DISCONNECTED`. VERIFIED (`CL_ConnectionlessPacket`, Mac 0x14c4c4). `CL_Reconnect_f` (Mac 0x147670) reconnects to `cls.servername`; a kick's `w` and a refused connect leave it set. INFERRED.

`CL_CreateCmd` (Mac 0x189f0e) stamps a cmd `cl.serverTime`, held to 5000 ms past `cl.snap.serverTime`, and one is made every frame. VERIFIED. So a client cut off goes on sending cmds at the held time.

VERIFIED live, retail on mp_toujane with the off-LAN setup of 17 (server at 203.0.113.1, vcod's probe at 198.51.100.2): the veth down for 6 s showed `interrupted` within the first second (the probe makes cmds faster than a 60 Hz frame), snapshots came back the second the link did, 120 sequence numbers counted lost, and nothing reset; down for 15 s under `VCOD_COD2_TIMEOUT=8` ended `EXE_ERR_SERVER_TIMEOUT` 8 s in. vcod-server gave the same.

vcod: `Cod2Session::interrupted`, drawn by `cod2_hud::connection_interrupted`; `timeout_frames`; a packet goes even with no new cmd, its newest cmd again, at most at the off-LAN pace. Trap: sending only on a new cmd stops the packets 5 s into an outage (the held clock), and the server hears nothing after the link comes back.

### 18.3 Userinfo

`CL_Frame` (Mac 0x14b182), past state 3 and not paused, sends `userinfo "<Dvar_InfoString(2)>"` on the reliable stream when a userinfo dvar changed (`dvar_modifiedFlags & 2`), once a frame. VERIFIED. `rate` (5000, 1000-25000), `snaps` (20, 1-30), `cl_maxpackets` (30, 10-100) and `cl_packetdup` (1, 0-5) are registered in `CL_InitOnceForAllClients` (Mac 0x1485e2) with flags 0x1003 for the first two and 0x1001 for the others. VERIFIED. The menus' `ui_setRate` (`UI_RunMenuScript`, Mac 0x1548e8) sets `cl_maxpackets` 30 and `cl_packetdup` 1 at the top rates, 15 and 2 in the band below and 15 and 1 under it. VERIFIED (the stores; the two thresholds are float constants not read here).

VERIFIED live, same setup, `ping=250,jitter=80,loss=15`: the probe's `snaps` went from 20 to 10 twelve seconds in and both servers dropped to 10 snapshots a second (8-10 counted under the loss).

vcod: `ConnectOpts::userinfo` / `with_userinfo` (`name`, `rate`, `snaps`, `cl_punkbuster`, `cl_wwwDownload`, `password` when set), `Cod2Session::set_userinfo` from the console's userinfo cvars and from a server `v` of one. vcod's default `rate` stays 25000 where the console has none.

### 18.4 Packets and the net line

`CL_SendCmdInternal` (Mac 0x18a032) makes a cmd each frame once primed and writes a packet when: not a demo; before state 7 at most once a second unless downloading; then every frame to a loopback or `Sys_IsLANAddress` server, else once `1000 / cl_maxpackets` ms passed since the last packet. VERIFIED. `CL_WritePacket` (Mac 0x188154) sends the cmds since packet `outgoingSequence - cl_packetdup - 1`, so each packet repeats the last one's, capped at 32 with `MAX_PACKET_USERCMDS` printed; `clc_move` deltas when `cl_nodelta` is off, the snapshot is valid, no demo waits and the newest message carried that snapshot, else `clc_moveNoDelta`. VERIFIED. Each packet records its realtime and its newest cmd's time; `CL_ParseSnapshot` sets `cl.snap.ping` to the realtime since the newest of the last 32 packets whose newest cmd the snapshot's `commandTime` covers, 999 when none. VERIFIED.

`CG_PredictPlayerState` (Mac 0x1ddafa) sets `cg.predictedErrorTime` to `cg.oldTime` (`cg` +0x284c0 from +0x25bb4), so the frame that finds a miss already draws part of it. VERIFIED (the store). vcod's predictor does the same. Trap: easing from the frame's own time draws none of the error on the miss frame and pops back on the next.

VERIFIED live, the off-LAN setup above, the probe joining allies and walking, 40 s each:

| Link (`VCOD_NETSIM`) | Server | Ping | Snapshots/s | Clock behind newest | Resets / fast | Corrections past 0.1 |
|---|---|---|---|---|---|---|
| 100 ms, 30 jitter, 5 % | retail | 96-137 | 16-20 | 26-38 ms | 0 / 0 | 0 |
| 100 ms, 30 jitter, 5 % | vcod-server | 109-139 | 18-20 | 17-32 ms | 0 / 0 | 1 |
| 250 ms, 80 jitter, 15 % | retail | 224-350 | 15-18 (8-11 at `snaps 10`) | 14-236 ms | 0 / 5 | 10 |
| 250 ms, 80 jitter, 15 % | vcod-server | 252-290 | 17-19 (6-10) | -48-204 ms | 0 / 3 | 3 |

vcod's F3 shows `net: ping P  snaps N/s  msgs N/s  out N/s  lost N (total)  frag N/s (total)  rate R snaps S [lan]`, the clock line and a `pred` line (`play::cod2_net::NetMeter`, `Cod2Session::debug_lines`); `--net-probe --game cod2` prints them each second as `link`. `VCOD_NETSIM` applies to a CoD2 session (`UdpTransport::from_socket`) and makes it off the LAN; `Cod2Session::disconnect` puts the datagrams the simulator still holds on the wire at once (`UdpTransport::drain`), so the three `disconnect`s are not left in its queue when the session ends.

### 18.5 The snapshots a frame draws between

The cgame keeps two snapshot buffers, `cg` +0x28 and +0x12de8, with `cg.snap` at `cg` +0x20 and `cg.nextSnap` at +0x24; `cgs.processedSnapshotNum` is `cgs` +0x2201c and `cg.latestSnapshotNum` `cg` +0x18. VERIFIED (the loads and stores in `CG_ProcessSnapshots`, Mac 0x1df762). `CG_DrawActiveFrame` (Mac 0x1d4a50) stores the frame's time to `cg.time` (`cg` +0x25bb0), calls `CG_AddLagometerFrameInfo`, and calls `CG_ProcessSnapshots` after it. VERIFIED (the stores and the call order in the listing).

`CL_GetSnapshot` (Mac 0x17a02c) `Com_Error`s on a number past `cl.snap.messageNum` and returns 0 for one 32 or more behind it, one whose slot in the 32-entry `cl.snapshots` ring is not valid, or one whose entities or clients the 2048-slot parse rings (`cl` +0x24750, +0x24754) have overwritten. VERIFIED (the compares: `< 0x20`, `*piVar1 != 0`, `< 0x800` twice). It copies at most 256 entities and 64 clients. VERIFIED (the 0x100 and 0x40 caps).

`CG_ProcessSnapshots` reads the next snapshot by raising `processedSnapshotNum` one at a time while it is below the latest, `CL_GetSnapshot` on each; a refused number is skipped. INFERRED (the loop). It asks only while it holds no snapshot or while `cg.nextSnap` is `cg.snap` (or `cg` +0xc is set, the frame's third argument, not traced). INFERRED. With no snapshot yet it takes the first not flagged `snapFlags & 2` as both `cg.snap` and `cg.nextSnap` through `CG_SetInitialSnapshot` and `CG_SetNextSnap`. INFERRED (the `& 2` test and the two calls). A read snapshot whose `snapFlags & 4` differs from `cg.nextSnap`'s starts it over the same way, and one whose server time is below `cg.nextSnap`'s is `Com_Error` `"CG_ProcessSnapshots: Server time went backwards"`. VERIFIED (the `& 4` test and the string). With nothing left to read, `cg.time` below `cg.snap`'s time is raised to it, and the function returns. INFERRED (the compare and the stores to +0x25bb0 and +0xe08f4). It returns once `cg.snap.serverTime <= cg.time < cg.nextSnap.serverTime`; else `cg.nextSnap` becomes `cg.snap`, its entities' next states copied over their current ones, and it loops. INFERRED (the two compares and the copies). So a time before `cg.snap` walks on to the newest and is held there, and `cg.snap` never goes back. INFERRED.

`CG_SetFrameInterpolation` (Mac 0x1633be) sets `cg.frameInterpolation` (`cg` +0x25ba8) to `(cg.time - snap.serverTime) / (nextSnap.serverTime - snap.serverTime)`, 0 when the two times are equal, and to 0 when that is negative; there is no clamp at 1. VERIFIED (the arithmetic). `CG_AddPacketEntities` (Mac 0x164fcc) walks `cg.nextSnap`'s entities, skipping the number `cg` +0x26158 holds and any whose `eType` is 10 or over. VERIFIED (the `+0x24` load, the compare and the `< 10` test). What +0x26158 names is not traced.

`CG_SetNextSnap` (Mac 0x1dee08) copies each of the new snapshot's entities into its `nextState` (centity +0xf0) and runs `CG_ResetEntity` (Mac 0x1deb10) on one not in the old `cg.nextSnap` or whose `eFlags & 2` differs from its current state's; `CG_ResetEntity` copies `nextState` over `currentState` (+0). VERIFIED (the `memcpy`s and the `^ ... & 2` test). It then runs `CG_CheckEvents` over the new snapshot's entities and `CG_TransitionPlayerState` on its playerstate. VERIFIED (the calls). So events fire when a snapshot becomes `cg.nextSnap`, not when it becomes `cg.snap`. INFERRED.

`CG_CalcEntityLerpPositions` (Mac 0x163c40) sends `pos.trType` 1 (`TR_INTERPOLATE`), and 3 (`TR_LINEAR_STOP`) for an entity numbered under 64, to `CG_InterpolateEntityPosition` (Mac 0x1636d2) with no other test. VERIFIED (the compares). That evaluates `currentState`'s trajectories at `cg.snap.serverTime` and `nextState`'s at `cg.nextSnap.serverTime` and lerps them by `frameInterpolation`, the angles through `LerpAngle`. VERIFIED. Every other entity evaluates `currentState` at `cg.time`, past the newest snapshot when the clock runs ahead, and rides its ground mover by the mover's move from `cg.snap.serverTime` to `cg.time` (`CG_AdjustPositionForMover`, Mac 0x163b00, the mover `eType` 6). VERIFIED.

vcod: `vcod_cod2::net::NetView` keeps every snapshot `CL_GetSnapshot` would still return (`NetView::push`, the 32-message and parse-ring windows), `NetView::process` runs the walk above once a frame from `Cod2Session::fly` and leaves a `Cursor` (`cg.snap`, `cg.nextSnap`, `processedSnapshotNum`), and `NetView::frame` hands out `snap` as `cg.nextSnap`, `prev` as `cg.snap` (`None` when they are one), `time_ms` as `cg.time` and `frac`. Events, the sound cues and the predicted playerstate ride the frame's `snap`; `Cod2Session::interrupted` reads its `prev`. `the_cgame_lerps_the_pair_around_its_time` and `lost_inactive_and_restarted_snapshots` pin the walk. Keeping only the newest two holds any drawn time before the older of them (the last column below).

Measured against vcod-server on mp_toujane tdm with 4 bots, the probe joining allies on loopback under `VCOD_NETSIM`, 40 s each. VERIFIED live. F3's `lerp` line counts frames whose time sat before the drawn pair (held) and past the newest snapshot (extrap), and what the newest two alone would have drawn:

| Link (`VCOD_NETSIM`) | Frames | Held | Extrapolated | Held with the newest two |
|---|---|---|---|---|
| none (LAN) | 15555 | 0 | 186 | 1848 (11.9 %) |
| 100 ms, 30 jitter, 5 % | 17309 | 0 | 1180 | 3309 (19.1 %) |
| 250 ms, 80 jitter, 15 % | 16014 | 0 | 1625 | 7280 (45.5 %) |

The extrapolated frames are the clock's (18.1) and the same under either rule.

### 18.6 The lagometer

`cg_drawLagometer` is a bool, default 0, flags 0x1001, registered in `CG_Init` (Mac 0x145214). VERIFIED (`Dvar_RegisterBool("cg_drawLagometer", 0, 0x1001)`). `CG_AddLagometerFrameInfo` (Mac 0x1cd7c0) stores `cg.time - cg.latestSnapshotTime` (`cg` +0x1c, the newest snapshot's time as the last `CG_ProcessSnapshots` read it) into a 128-entry ring. VERIFIED. `CG_AddLagometerSnapshotInfo` (Mac 0x1cd7f2) stores a snapshot's ping (`snapshot_t` +4) and `snapFlags` (+0) into a second 128-entry ring, or -1 for a number `CL_GetSnapshot` refused. VERIFIED. `CG_ProcessSnapshots` calls it on every number it asks for. VERIFIED (the calls after each `CL_GetSnapshot`).

`CG_Draw2D` (Mac 0x1d114a) draws it between the vote lines and `CG_DrawDisconnect` when `cg_drawLagometer` is set and `cgs` +0x22020 is 0. VERIFIED. That word is `com_sv_running` copied in `CG_Init`, so a client running its own server draws none. VERIFIED (the store). The placements, all with horizontal and vertical alignment 3 (right and bottom), VERIFIED (the constants at Mac 0x3265f0..0x326608, 0x326260, 0x326470):

- `UI_DrawHandlePic(-55, -140, 48, 48)` with the `lagometer` material (`cgs` +0x27bc0).
- 48 columns of `white` (`cgs` +0x27b9c), one unit wide at x `-7 - i`, newest first. A frame's offset times 0.0533333 (16 per 300 ms): above 0, yellow, at y `-124 - v` and `v` high, or y -140 and 16 high past 16; below 0, blue, at y -124, `-v` high at most 16; 0 draws nothing.
- 48 columns the same for the snapshots: a ping above 0 times 0.0266667 (24 per 900 ms), green, yellow when `snapFlags & 1`, at y `-92 - v` and `v` high, or y -116 and 24 high past 24; -1 a red column at y -116, 24 high; 0 nothing.
- `snc` through `CG_DrawBigDevString(-55, -140)` when `cg_nopredict` or `cg_synchronousClients` is set.

vcod: `play::cod2_net::Lagometer` keeps the two rings, filled in `Cod2Session::process_snapshots` with the ping `NetMeter` measured for each parsed snapshot (18.4); `cod2_hud::lagometer` draws them through `ScreenPlacement` when the console's `cg_drawLagometer` is set. vcod has neither `cg_nopredict` nor `cg_synchronousClients`, so `snc` never draws.
