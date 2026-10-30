# Call of Duty 1.1 network protocol

The CoD 1.1 wire protocol, for anyone writing a 1.1 server or client from scratch. It is id Tech 3: read it as "Q3, except". Every place 1.1 departs from the RTCW GPL source is flagged, because those are the spots where a straight Q3 port breaks. The full list is the last section but one.

Sources: the RTCW/Q3 GPL sources as the structural reference, and byte-exact captures against a retail `cod_lnxded` 1.1d server (protocol **1**, `shortversion 1.1`). Binary offsets are into the stripped `cod_lnxded` 1.1d ELF unless a module is named. They are breadcrumbs for re-deriving a field, not a stable ABI.

Patch 1.5 speaks protocol 6: this protocol with a wider, renumbered `pm_flags` and a few new server commands (`docs/research/cod15-protocol-diff.md`).

## Transport

UDP. The first 4 bytes tell the two packet classes apart:

- **Connectionless (out-of-band):** `0xFF 0xFF 0xFF 0xFF`, then a plaintext command string. Handshake and queries.
- **Netchan (connected):** a little-endian sequence number (never `0xFFFFFFFF`). Everything after the handshake.

## Connectionless packets

`FF FF FF FF`, an ASCII command word, a space, the payload. Replies use the same framing. Queries are plaintext both ways:

- `getinfo <arg>` returns `infoResponse\n\<key\value...>`, short server info.
- `getstatus <arg>` returns `statusResponse\n\<key\value...>\n<player lines>`, the full info string (`protocol\1`, `mapname\mp_carentan`, `sv_maxclients` and the rest). Read the protocol number here.

Both key sets are stable:

- `SVC_Info` (`0x808c1ac`) builds its own string, in this order: `challenge`, `protocol`, `hostname`, `mapname`, `clients`, `sv_maxclients`, `gametype`, `pure`, `sv_allowAnonymous`, `pswrd`. `minPing`, `maxPing` and `game` are appended only when `sv_minPing`/`sv_maxPing`/`fs_game` are set. `challenge` is `Cmd_Argv(1)` of the query line as `MSG_ReadStringLine` read it, and a bare query sends no `challenge` key (`docs/research/cod11-server-handshake.md`, "The challenge key"). vcod tokenizes the line the same way (`oob_argv`, `crates/server/src/server.rs`).
- `SVC_Status` (`0x808bd50`) sends the serverinfo cvar string, configstring 0 byte for byte, with `challenge` and `pswrd` appended, a newline, then one `<score> <ping> "<name>"` line per connected client. Keys are in ascending **case-insensitive** order, as `Cvar_InfoString` walks the cvar list: the capture reads `\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0`, which case-sensitive ASCII order would reverse.

Both replies, captured verbatim on two maps, are in `docs/research/cod11-server-handshake.md`, with `challengeResponse` (one integer, freely negative, no Q3 second field).

Handshake:

1. Client sends `getchallenge`.
2. Server sends `challengeResponse <challenge>`. The binary carries both `challengeResponse %i` and `challengeResponse %i %i`. Only the first integer is the challenge; ignore the rest.
3. Client sends `connect "<userinfo>"`. The word `connect ` stays in the clear so the server can match it. Everything from the opening `"` (packet byte 12) on is Huffman-compressed.
4. Server replies `connectResponse` (no arguments), or `error\n<reason>`, for example `EXE_SERVER_IS_DIFFERENT_VER\x151.1` for a malformed userinfo or a wrong protocol key.

The reject tokens are localized keys the client looks up:

- At connect time, as OOB `error\n<TOKEN>`: `EXE_SERVER_IS_DIFFERENT_VER` (format `error\nEXE_SERVER_IS_DIFFERENT_VER\x15%s\n` at file offset 574208 of `cod_lnxded`, VERIFIED; the client splits key from argument on `\x15`, `docs/research/cod11-front-end.md` section 15), `EXE_BAD_CHALLENGE`, `EXE_SERVERISFULL` (no underscores).
- Drop reasons, sent to a connected client in the `w` server command (svc_serverCommand below): `EXE_LOSTRELIABLECOMMANDS`, `EXE_DISCONNECTED`, `EXE_TIMEDOUT`.

The gamestate goes out on the client's first netchan message: a fresh slot has `gamestateMessageNum = -1` and its `serverId` (0) mismatches (`SV_ExecuteClientMessage`, `0x80872ec`).

`rcon <password> <command>` answers in `print\n<text>` packets. The password cvar is `rconPassword`, not Q3's `rcon_password`. A `dedicated 2` server sends the master `heartbeat COD-1\n` every 180 s and `heartbeat flatline\n` on the way down. Both, and the `CS_ZOMBIE` window a dropped client gets, are in `docs/research/cod11-server-handshake.md`, "Housekeeping".

**Divergence from RTCW, #1.** RTCW sends the connect userinfo as plaintext. CoD 1.1 Huffman-compresses it from byte 12. A plaintext `connect` gets no `connectResponse`: the server decompresses unconditionally and reads garbage. vcod's userinfo:

```
\cg_predictItems\1\cl_anonymous\0\handicap\100\color\4\head\default\model\multi\snaps\20\rate\25000\name\vcod\protocol\1\qport\<qport>\challenge\<challenge>
```

The keys that matter are `protocol`, `qport`, `challenge`, `name`, `rate` and `snaps`.

## Huffman coding

CoD 1.1 uses the exact Q3/RTCW `msg_hData[256]` frequency table from `qcommon/msg.c`. A tree built from it decoded real packets into 246 legible configstrings, with nothing extracted from the binary. Build the standard Q3 static tree once through the adaptive-insert (`Huff_addRef`) sequence, then freeze it. It never adapts during traffic.

`Huff_Compress` (`0x807f03c`) walks the source byte by byte through `offset_transmit` on the static tree. **Divergence from RTCW, #2.** RTCW's `Huff_Compress`/`Decompress` write a 2-byte big-endian length prefix. CoD 1.1's compressed *message* blocks have none: decode until the input runs out. The connectionless `connect` payload does use the classic prefixed form; the per-message blocks below do not.

## The message-block model (the big one)

**Divergence from RTCW, #3, and the costliest one.** RTCW Huffman-codes each byte inside `MSG_WriteBits`, interleaving sub-byte raw bits with per-byte codes in one stream. CoD 1.1 builds the whole message as a plain byte buffer, then `Huff_Compress`es it as one block. `SV_Netchan_Transmit` (`0x808f680`) copies the first 4 bytes verbatim and compresses everything from byte 4.

A per-byte Huffman decode and a whole-block decompress give the same bytes only for byte-aligned reads. Everything up to the first `svc_baseline` (configstrings, the leading opcodes) is byte-aligned, so a per-byte decoder looks right until the first entity delta's 10-bit entity number, then silently desyncs. Decompress the whole block first, then read the plaintext with two cursors, a byte cursor `readcount` and a bit cursor, resyncing the bit cursor to the byte boundary whenever it lands on one. This mirrors the engine's `msg_t`.

### Message headers

**Server to client.** 4 plain little-endian bytes of `reliableAcknowledge`, then the compressed block from byte 4. **Divergence from RTCW, #4.** RTCW Huffman-codes the reliableAck into the stream; CoD 1.1 leaves it plain. Proof: a gamestate's first 4 payload bytes are `00 00 00 00`. Four Huffman `0x00` symbols take 2 bits each, 8 bits in all, so they cannot fill 32 bits.

**Client to server.** A 9-byte plain header, then `scramble(Huff_Compress(ops))` from byte 0 of the compressed output. **Divergence from RTCW, #5.** The header is `serverId` as one byte (`MSG_ReadByte`), `messageAcknowledge` (long) and `reliableAcknowledge` (long). RTCW uses a 4-byte serverId and compresses the header with the ops. The server reads the 9 plain bytes (`SV_PacketEvent`, entry `0x808c9ca`, the read at `0x808c9d1`), then decodes and decompresses from byte 9. `SV_PacketEvent` drops the message when `messageAcknowledge < 0` or `reliableSequence - reliableAcknowledge > 63`, RTCW's lost-reliable-commands check sized to the 64-slot ring. The server keeps `sv_serverid` as a byte too (map_restart below), so this round-trips.

## Netchan framing

Standard Q3 netchan. The sequence number is the first 4 bytes, little-endian. `FRAGMENT_BIT = 1<<31` on it marks a fragment. `FRAGMENT_SIZE = 1300`.

- Client to server: `[u32 sequence][u16 qport][payload]`. The `qport` is the client's port id; vcod uses `0x2000 | (pid & 0x0fff)`.
- Server to client: `[u32 sequence][payload]`, no qport.
- Fragmented: `FRAGMENT_BIT` set, then `[u16 fragmentStart][u16 fragmentLength]`. A fragment shorter than `FRAGMENT_SIZE` ends the set; reassemble in order. A message whose length is an exact multiple of `FRAGMENT_SIZE` gets a zero-length terminator fragment (RTCW `net_chan.c:152`; retail does the same). The gamestate is always fragmented, for example 6 packets (5x1300 + 404) under one sequence. Drop stale and duplicate sequences; a fragment of a new sequence resets the buffer. The largest reassembled message is 32768 (`MAX_MSGLEN`).

The sequence header bytes are plain; the payload is the compressed, scrambled block.

## XOR scramble

Connected payloads are also XOR-scrambled: Q3's netchan encode/decode (`CL_Netchan_Encode`/`Decode`; server side `SV_Netchan_Decode` `0x808de60`), intact in 1.1. Port it byte for byte. The byte index parity matters (`c << (i & 1)`).

- Server to client (client decoding): `key = challenge ^ sequence` (low byte), keyed further on the client's own reliable command that the message's `reliableAcknowledge` names. The server side is RTCW's `SV_Netchan_Encode` (`sv_net_chan.c:43`) unchanged, walking `lastClientCommandString` with `challenge ^ outgoingSequence`. Decoding starts at packet byte `4 + CL_DECODE_START` = 8, byte 0 of the compressed block, since the plain `reliableAcknowledge` fills payload bytes 0 to 3.
- Client to server: `key = challenge ^ serverId ^ messageAcknowledge`, keyed on `serverCommands[reliableAcknowledge & 63]`. The scramble covers the **compressed op block from its byte 0**, packet byte 15 (4 sequence + 2 qport + 9 header). There is no `CL_ENCODE_START` offset into the block; the plain header before it is never scrambled. `SV_Netchan_Decode` (`0x808de60`) walks the block with `i` from 0, so the parity is within the block, not the packet.
- The running key advances through the keyed string, wrapping at its NUL, with no byte substitution on either side. VERIFIED: the retail client's decode (CoDMP.exe `0x414780`), the server's `SV_Netchan_Transmit` scramble (`0x808dc74`) and `SV_Netchan_Decode` (`0x808de60`) all walk their string raw. Q3's `'%'`/high-ascii substitution is not in 1.1.

The key reads `serverCommands[seq & 63]`, so store every received `svc_serverCommand` in a 64-entry ring by sequence. Get this wrong and everything past the first server command decodes to garbage.

Server to client, the key string is the server's `lastClientCommandString`: the last reliable *client* command it executed. The client walks its own copy out of `reliableCommands[reliableAcknowledge & 63]`. The server's copy is what its `MSG_ReadString` (`0x807f320`, byte map `0x8083394`) produced: 0x92 read as `'`, every other byte over 127 as `.`, and `%` kept, unlike the client's reader. VERIFIED: `SV_ClientCommand` (`0x8086e08`) copies the string it read into `lastClientCommandString` (`client+0x10640`) after executing it, and the scramble walks that copy. So the client must map its raw copy the same way before keying, as vcod's decode does. VERIFIED live 2026-10-06 on retail 1.1d: without that mapping, a vcod client that sent `say ... %s ...` decoded every later message as garbage (`snapshot overran the message`), and its next client command reached the server mis-keyed.

Every client message carries at least a `clc_EOF`, so its block is never empty and the scramble always applies, header-only keepalives included. VERIFIED live: vcod scrambles unconditionally and retail 1.1d accepts every message. Whether a retail *client* keeps RTCW's `cursize <= CL_ENCODE_START` early-out and can send an unscrambled short message is untested; the retail-client check in `docs/research/cod11-server-handshake.md` would settle it.

## Server to client message body (svc)

After decompression the body is a sequence of ops, Q3's enum values:

```
svc_bad=0 svc_nop=1 svc_gamestate=2 svc_configstring=3 svc_baseline=4
svc_serverCommand=5 svc_download=6 svc_snapshot=7 svc_EOF=8
```

A message can open with a `svc_serverCommand` before the gamestate.

### svc_gamestate (2)

```
long  serverCommandSequence
loop:
  svc_configstring(3): short index, bigstring value
  svc_baseline(4):     GENTITYNUM_BITS(=10)-bit entity number, delta-from-null entityState
until svc_EOF(8)
long  clientNum
long  checksumFeed
```

- `checksumFeed` is `(rand() << 16) ^ rand() ^ millis`, drawn once per map load in `SV_SpawnServer` (`0x808a3e0`). It seeds the usercmd key (clc below).
- `MAX_CONFIGSTRINGS = 2048`. Configstring 0 is the serverinfo (`\mapname\...\g_gametype\...`), 1 the systeminfo (`sv_serverid`, `sv_paks` and so on). One capture held 246 configstrings and 19 baselines.
- Base indices pinned from `game.mp.i386.so` write sites: `CS_MODELS` is **268** (the `xmodel/...` list runs 269 to 523, off `G_ModelIndex`), attachment tag names **108** (`attachTagIndex`, off `G_TagIndex`). Configstring **44** is the locations list (`target_location_linkup`), not a per-client base.
- CoD 1.1 has no per-client configstring. Body model, six attachment model/tag pairs, team and name travel in the snapshot's clientState stream (below, and `docs/research/clientstate-wire-format.md`).

`docs/research/cod11-server-handshake.md` has what retail puts in the table before any player exists: the full configstring list for an empty `mp_pavlov` and `mp_carentan`, which entries depend on the map, the baseline counts, and the fact that the stock server sends `svc_gamestate` with no leading `svc_serverCommand`. The `getinfo`/`getstatus`/`getchallenge` replies are there too.

The writer is the reader run backwards. vcod's re-encodes the retail capture into the identical byte string (`crates/common/src/net/gamestate.rs`, `writer_reproduces_the_captured_gamestate_byte_for_byte`). That test pinned three things:

- Every server message ends with a `svc_EOF` byte that `SV_Netchan_Transmit` appends after the last op, as Q3's does; the capture has it right after `checksumFeed`. The retail client reads it and stops. Without it the client reads the next byte as an op and drops with `CL_ParseServerMessage: Illegible server message`. VERIFIED live 2026-08-25: `CoDMP.exe` loaded the map, then dropped with that error, when vcod's server left it out.
- The `Huff_Compress` block ends on a partial byte. Decompressing it turns the trailing pad bits into one more whole symbol, so the plain buffer runs a byte past the terminator. Take the length from what the reader consumed, never from the decompressed buffer.
- A float field's zero flag does not round-trip naively (Entity delta below).

Configstring **7** (`BG_SetupWeaponInfo`'s weapon name list) is **space**-separated, per `crates/common/tests/fixtures/net/gamestate.bin`: `"bar_mp bar_slow_mp bren_mp colt_mp enfield_mp fg42_mp fg42_semi_mp fraggrenade_mp kar98k_mp kar98k_sniper_mp luger_mp ..."`. Each name is a weapon file (`weapons/mp/<name>`). `entityState.index` names it **1-based**: `index` N is `list[N-1]`.

### svc_snapshot (7)

**Divergence from RTCW, #6.** CoD 1.1 snapshots have no areamask. `SV_WriteSnapshotToClient` (`0x808df7f`) writes:

```
long  serverTime
byte  deltaNum        ; 0 = uncompressed (delta from nothing); else base = messageNum - deltaNum
byte  snapFlags
<playerState delta>
<packet entities>
<client states>
```

RTCW writes an areamask (a length byte plus that many bytes) between `snapFlags` and the playerstate. CoD 1.1 goes straight to the playerstate delta. A phantom areamask read overruns every snapshot; dropping it is what made real frames decode.

Packet entities repeat `[GENTITYNUM_BITS(10)-bit entity number][delta]` and end at `ENTITYNUM_NONE = 1023`. A bit after the number signals a remove. An entity with no base in the referenced snapshot deltas from its gamestate baseline. `MAX_GENTITIES = 1024`, `ENTITYNUM_WORLD = 1022`.

`snapFlags`:

| bit | name | set when | evidence |
|---|---|---|---|
| 4 | `SNAPFLAG_SERVERCOUNT` | `svs.snapFlagServerBit`, toggled by every map load and restart, so the first map already reads 4 | VERIFIED, the load of `0x83b67a8` at `0x808dfac`; live 2026-10-12, retail's first snapshot on the first map read 4 on loopback, VERIFIED (capture) |
| 1 | `SNAPFLAG_RATE_DELAYED` | the client's last message hit its rate (`cl+0x10b18`) | VERIFIED, the `or $0x1` at `0x808dfbe` |
| 2 | `SNAPFLAG_NOT_ACTIVE` | the word at `cl+4` is 0 | VERIFIED, the `or $0x2` at `0x808dfe3` |

- `cl+4`: `CS_ACTIVE` stores 1 (state test at `0x808dfc3`), and any state but `CS_ZOMBIE` stores 0 (`0x808dfd1`); a zombie keeps it. VERIFIED. INFERRED: a zombie that had been active keeps 1, so its snapshots carry no 2.
- `cl+0x10b18`: `SV_SendMessageToClient` (`0x808f680`) sets it off the LAN when the rate interval `(min(bytes, 1500) + 48) * 1000 / rate` is at or past `snapshotMsec`, and clears it when the floor wins; a LAN client leaves it alone. VERIFIED. Live 2026-10-12, the namespace capture of the handshake doc's "Message pacing" at `rate 1000`, `snaps 30`: `snapFlags` 5 off the LAN, 4 on loopback. VERIFIED (capture). vcod: `Link::rate_delayed` (`crates/server/src/engine/slot.rs`).
- `SV_SendClientSnapshot` (`0x808f844`) builds a snapshot only for state 4 (`CS_ACTIVE`) or 1 (`CS_ZOMBIE`), the two compares at its head. VERIFIED. INFERRED: the store of 0 at `0x808dfd1` is never reached, `cl+4` is 1 from a client's first active snapshot on, and only a zombie dropped before it was ever sent one (a kick or a disconnect while loading) carries the 2. Live 2026-10-12, loopback: `--probe-edge zombie` (a `disconnect` before the first usercmd) read `snapFlags` 6 on all 40 zombie snapshots, and `--probe-edge zombie-active` (a `disconnect` after 5 s in the world) read 4. VERIFIED (capture; handshake doc, "Zombie slots"). vcod: `Link::was_active`, set on the first active snapshot; `Server::send_zombie` ors in the 2 without it (`a_zombie_never_active_carries_snapflag_not_active`).

The delta base is the frame the client last acked only for a `CS_ACTIVE` client. Anything else, a zombie included, gets `deltaNum` 0 and a frame written from nothing. VERIFIED (the `cmpl $0x4` on the state at `0x808df11` takes the no-base arm).

Keep a ring of recent snapshots (`PACKET_BACKUP`, 32) so any `deltaNum` resolves. A snapshot whose base is missing or too old is invalid and dropped.

After the packet entities comes a third delta stream, `clientState_t`, which replaces Q3's per-client `CS_PLAYERS` configstring (`SV_WriteSnapshotToClient` `0x808e1fd`..`0x808e24f`):

```
<clients>: repeated [1 bit][6-bit clientIndex][clientState delta]
bit 0   client terminator
```

Each delta body uses the generic per-field codec packet entities use (a removed bit, a delta-follows bit, a raw `lc` byte, then delta fields) over the 22-field `clientState` table: team, body `modelindex`, six `attachModelIndex`/`attachTagIndex` pairs, and the 32-byte `name`. A removed delta means that client disconnected. A client the section does not mention carries forward from the base frame, like an unmentioned packet entity. Field table and struct layout: `docs/research/clientstate-wire-format.md`.

### Which entities a client is sent

A client is not sent every entity: the list depends on where it stands. Measured 2026-09-01 with `tools/run_server.sh mp_carentan` (dm, stock defaults) and `--net-probe --probe-pvs`, joining through the stock menus.

- A lone probe at its spawn was sent 6 of the map's entities, and 12 after walking ~700 units. The six new ones were a second `mg42_bipod` and the `bookshelvewide`/`ammo_panzerfaust_box3` group around it at [810, 2274]. VERIFIED live.
- That group crossed in and out of a second probe's list four times in 20 s as it walked back and forth over ~70 units near [1000, 700], its distance to the group holding at ~1600 units. The gate is a boundary in the map, not a radius. VERIFIED live.
- Whether a client entity is culled the same way is UNVERIFIED. Two probes ~2400 units apart were sent each other's entity (`eType` 1, `clientNum` 0 and 1) for as long as both were connected; the one removal seen was the other probe disconnecting.
- A dead client is sent to nobody. Its entity leaves the other clients' lists the frame after the kill and returns on the respawn frame; the body queue's corpse stands in for it meanwhile. VERIFIED live, 2026-09-05, six lives. `ClientEndFrame` sets `SVF_NOCLIENT` every frame on a client whose `sessionstate` is dead (`docs/research/cod11-combat.md` 5.4).
- The 1.1d binary holds `SV_BuildClientSnapshot: bad gEnt` and `CMod_LoadLeafs: cluster exceeded`, so it has Q3's snapshot builder and loads leaf clusters (VERIFIED). "The rule, out of the binary" below reads the test itself.

#### The box an entity links with

An entity's clusters come from the box it links with, never its origin alone: a turret's or a prop's origin usually sits inside the geometry it stands on and reads as a solid leaf.

`SV_LinkEntity` (0x8090ad3) has two blocks that write the abs box:

- 0x8090b60-0x8090bb8 stores `origin[i] + r.mins[i]` into `absmin` (+0x11c) and `origin[i] + r.maxs[i]` into `absmax` (+0x128), a component at a time. VERIFIED.
- 0x8090bbe-0x8090c13 loads 1.0 and stores each `absmin` component a unit lower and each `absmax` component a unit higher. VERIFIED.

INFERRED, from the blocks' order and nothing between them touching those fields: a linked entity's box is `origin + r.mins - 1` to `origin + r.maxs + 1`. An entity whose own box is a point still links with `origin +/- 1`.

A third block (0x8090b0f-0x8090b5c) replaces the box with `RadiusFromBounds` of it, cubed about the origin. INFERRED: it applies only to a rotated bmodel, from the `r.bmodel` test (+0xfc, 0x8090ac6) and the three `r.currentAngles` comparisons (+0x140) guarding it. None of the types below is a bmodel, and for a point box both branches agree.

What the game module leaves in `r.mins`/`r.maxs`. Offsets from here to the end of this subsection are into `main/game.mp.i386.so`, the 1.1d dedicated server's game module:

| entity | `r.mins` | `r.maxs` | written by | |
|---|---|---|---|---|
| `ET_ITEM` | (-1, -1, -1) | (1, 1, 1) | `G_SpawnItem` 0x4e6ed | VERIFIED |
| `ET_TURRET` | (-32, -32, 0) | (32, 32, 56) | `G_SpawnTurret` 0x52f75 | VERIFIED |
| `ET_SCRIPTMOVER` (`script_model`) | (0, 0, 0) | (0, 0, 0) | nothing does | INFERRED |
| `ET_MISSILE` | (0, 0, 0) | (0, 0, 0) | nothing does | VERIFIED |

`ET_MISSILE` has no box. VERIFIED: `fire_grenade` (`0x543AC`) writes neither `r.mins` (+0x100) nor `r.maxs` (+0x10c), and `G_RunMissile` (`0x53FCC`) traces the frame's move with `trap_LocationalTrace`, which takes no mins/maxs. INFERRED: a grenade in flight is traced as a point (`docs/research/cod11-combat.md` sections 11 and 12).

The `script_model` row, and why the model it draws does not matter:

- No instruction in `SP_script_model` (0x60ff4), `InitScriptMover` (0x60214) or the `setmodel` entity setter (0x5dabc) writes `r.mins` (+0x100) or `r.maxs` (+0x10c). VERIFIED.
- `G_SetModel` (0x67020) resolves the model's `CS_MODELS` slot and writes the `modelindex` byte at +0x175, nothing more. VERIFIED. Its `+0x10c` is the `CS_MODELS` configstring base, which collides numerically with the `r.maxs` offset and means something else. VERIFIED.
- No trap returns an xmodel's bounds: `trap_XModelGet` hands back a handle, and the other xmodel entry points are `trap_XModelExists`, `trap_XModelNumBones`, `trap_XModelGetBoneNames` and `trap_XModelDebugBoxes`. VERIFIED.
- Scripts cannot resize a mover: there is no `setsize` builtin, and `solid`/`notsolid` write only `r.contents`. VERIFIED.
- `G_FreeEntity` bzeroes the 0x314-byte slot before reuse (0x66b97), so the fields read zero on a recycled entity too. VERIFIED.
- A census of every store to +0x100 or +0x10c finds them paired as vector writes in `ClientThink_real`, `ClientSpawn`, `LaunchItem`, `G_SpawnItem`, `G_RunFrame`, `G_SpawnTurret`, `Blocked_DoorRotate` and one static in the player-command block. VERIFIED. INFERRED, from the call graph out of `SP_script_model`: none is reachable from a script mover. Two more stores, in `TeamplayInfoMessage` (0x6493b, 0x64acd), go through a base loaded from +0x158. VERIFIED. INFERRED: +0x158 is the `client` pointer, so they are `gclient_t` fields, not an entity's box.

`script_brushmodel` shares `ET_SCRIPTMOVER` but differs: `SP_script_brushmodel` (0x60fb8) calls `trap_SetBrushModel` ahead of `InitScriptMover`. VERIFIED. INFERRED: that gives it the brush's bounds, not a point. Every `eType` 8 entity in the entity traces carries a model configstring index (56 on carentan, 26 on pavlov), so all of them are `script_model`s. VERIFIED. A `script_brushmodel` arrives as `eType` 8 with `solid` `0xffffff` and `index` its inline model number (`docs/research/cod11-movers.md` section 14). VERIFIED.

#### The clusters a link keeps

Q3's caps, in 1.1d and UO alike (VERIFIED decompile, `cod_lnxded` 0x80908b0, `coduo_lnxded` 0x8099dee):

- `CM_BoxLeafnums` lists at most 128 leafs (the 0x80 argument), solid ones included, walking a straddled node's front child first (UO 0x80573e6). Its last-leaf output is the last leaf with a cluster the walk reached, listed or not (UO 0x8057259 updates it before the full-list check).
- The link keeps the first 16 clusters among the listed leafs, repeats included (`MAX_ENT_CLUSTERS`, the 0x10 break). When it stops on the 16th it stores the last leaf's cluster as `lastCluster`, else 0.
- The snapshot test (UO 0x8096b97) tries the 16 against the viewer's PVS row. With none seen and `lastCluster` non-zero it scans cluster numbers from the 16th up to `lastCluster`: stopping on a seen cluster below `lastCluster` sends the entity, stopping on `lastCluster` itself hides it, running past it sends it. So an entity past 16 clusters is hidden only when `lastCluster` is the first seen number of the range.

vcod: `mapworld::Visibility::link_clusters` and `link_visible` (`crates/common/src/mapworld.rs`).

The walk's side test is Q3's x87 `BoxOnPlaneSide`, which objdump reads where Ghidra does not: `cod_lnxded` 1.1d 0x80c8634, 1.5b 0x80d5f94, `coduo_lnxded` 0x80cb294. VERIFIED, the instructions: it jumps on the plane's sign bits (`+0x11`, a table of eight arms) to sum the box's two corners along the normal on the x87 stack, compares the first against `dist` (`+0xc`) and adds 1 unless it is below, then adds 2 when the second is below (0x80c87e9..0x80c8802). There is no axial case, and the leaf walk calls it on every node with no type check (1.1d 0x8053e1d, 0x8054006, 0x80541a6; 1.5b 0x805760a; UO 0x805743e). The sign bits are set for a normal component below 0 (1.1d's plane loader, 0x804af70..0x804af8c, and `SetPlaneSignbits` 0x8067204). INFERRED, off those: the farthest corner at or past the plane puts the box in front and the nearest strictly short of it puts it behind, an axial plane included. A box whose high side lies on an axial plane is on both sides, where Q3's C version (`p->dist >= emaxs` gives the back only) is not. vcod's `mapworld::box_on_plane_side` takes the sums in f64.

1.5 and United Offensive add a broadcast time and a fog cull to this test (`docs/research/cod15-protocol-diff.md` 15).

#### What a map entity looks like

VERIFIED from the traces, and consistent with the shipped item list: a placed weapon is `eType` 3 (`ET_ITEM`) with `index` a **1-based index into configstring 7**, not a model index. Carentan's group at [388, -792] is four entities with `index` 23 (`panzerfaust_mp`) beside one `eType` 8 (`ET_SCRIPTMOVER`) whose `index` 56 is `xmodel/ammo_panzerfaust_box3`: four panzerfausts and their ammo box. The mounted MG is `eType` 11 with `index` 58, `xmodel/mg42_bipod`.

So `index` means one thing per `eType`. Reading a model index off an item draws a bookshelf where a panzerfaust stands. `ET_ITEM`'s index is the same 1-based weapon index `entityState.weapon` carries.

Static fields in the same traces: `eFlags` 16 and `groundEntityNum` 1022 (`ENTITYNUM_WORLD`) on the items, `clientNum` 254 on them and 0 on the scriptmover and the turret, `solid` 0 on all.

`groundEntityNum` on the scriptmover and the turret reads 0 until the client in slot 0 leaves, and 1023 (`ENTITYNUM_NONE`) from then on.

- VERIFIED live (retail 1.1d, `mp_carentan` `dm`, 2026-10-17): a spectator that never joined read 0 on all four for 280 s of a fresh server. On another fresh server the first `--save-entities` run read 0 and later runs 1023 from their first sample.
- VERIFIED, `game.mp.i386.so` 1.1d: `ClientDisconnect` (0x42aac) calls `G_FreeEntity` on the client (0x42bfc), and `G_FreeEntity` (0x66948) walks every entity in use (`inuse`, +0x160, 0x66ad1) and stores 0x3ff into a `s.groundEntityNum` (+0x7c) equal to the freed number (0x66b0e, 0x66b13).
- VERIFIED, 1.5b and UO: their `G_FreeEntityRefs` (0x6d1cc, 0x79a06), which `G_FreeEntity` calls, makes the same store (0x6d27b, 0x79ab5; `docs/research/cod11-turrets.md` 8).
- INFERRED: the scriptmover and the turret carry `G_Spawn`'s zeroed 0, which names entity 0, so slot 0's departure turns them. A trace is therefore taken after one client has come and gone, and `entities_ab.rs` replays such a client before its own.

#### What a missile looks like

VERIFIED live (retail 1.1d, `mp_carentan` `tdm`, 2026-09-05, the `--save-grenade` and `--probe-grenade` captures in `crates/server/tests/fixtures/playerstate/`): a thrown `fraggrenade_mp` is `eType` 4 (`ET_MISSILE`) with this field set and nothing else. It is the only `eType` 4 entity either capture saw, two captures of one weapon on one map. `weapon_grenadelauncher_fire` builds a missile the same way and nothing found so far calls it (`docs/research/cod11-combat.md` 11.3), so "the only one a stock MP server sends" is not measured.

| field | in flight | at rest | on the explode frame |
|---|---|---|---|
| `eType` | 4 | 4 | **0** |
| `eFlags` | 0 | 0 | **256** |
| `pos.trType` | 5 (`TR_GRAVITY`) | 0 (`TR_STATIONARY`) | 0 |
| `apos.trType` | 2 (`TR_LINEAR`) | 0 | 0 |
| `weapon` | configstring 7 index | same | same |
| `index` | 0 | 0 | 0 |
| `events[]` | 177 per bounce | | 178, parm the trace normal |

- `pos.trTime`, `trBase` and `trDelta` are rewritten in place at each bounce, so a client cannot cache the first segment.
- `apos.trDelta` is the tumble, a per-throw draw of roughly `(720, 0, 360)` degrees per second give or take 45. It goes stationary with the yaw and roll the tumble reached and a pitch of 0.
- The `eType` flip is the trap for a reader filtering on 4: the explode event rides the missile's own ring on the frame its `eType` has already become 0.
- `index` is 0, unlike `ET_ITEM`'s: a missile's model rides `s.eFlags 0x03000000`, above the 24-bit netfield, and never travels. VERIFIED: the 2026-10-17 and 2026-10-19 recaptures of `mp_carentan-tdm-grenade.txt` print `eFlags` 16777216 on every in-flight and resting frame. The field arrives non-zero with 24 zero bits, and vcod's reader marks that with a bit above the width (`above_width` in `crates/common/src/net/codec.rs`, "PlayerState delta"). INFERRED: the model bits are what make the int non-zero.

VERIFIED from the same captures: `grenadeTimeLeft` takes exactly two values on a stock 1.1 MP server, 0 and the held weapon's `fuseTime` in ms (4000 for the frag). The pullback sets it, the throw clears it, nothing decrements it in between: no cook countdown, no pin, no auto-throw. The `-grenade-shooter.txt` pair alone carries 233 retail traces and 499 vcod ones with no third value.

#### A turret's `angles2[0]` is where its barrel came to rest

Both of carentan's mounted MGs carry a non-zero `angles2[0]` in every sample of every run: -63 on the one at [-500, 1896, 175], -72 on the one at [1712, 1830, 8]. It is the barrel's pitch, which the turret measures against the map at spawn.

VERIFIED, all read out of `game.mp.i386.so`:

- `G_SpawnTurret` (0x52c84) writes zero to the three `angles2` floats at `+0x68`, `+0x6c` and `+0x70` (0x52fd2) and -90 (rodata `0x75a10`) to `+0x1c` of the turret record it allocates, then sets `turret_think_init` as the think and `level.time + 100` as nextthink.
- `turret_think_init` (0x5268c) calls `G_DObjGetLocalTagMatrix` (0x673e0) for `tag_aim` (rodata `0x75882`) and `tag_butt` (`0x75910`), `AnglesToAxis` (0x3ef3c), `MatrixTransformVector` (0x3e220), `MatrixTransformVector43` (0x3e2e8), and `trap_LocationalTrace` (0x63948) with mask 0x11. Its loop counter is compared against 30, its per-step angle is the counter times -3.0 (rodata `0x7591c`), and its one store to `+0x1c` writes that angle.
- `turret_think` (0x5328c) is a 50 ms think that calls the helper at 0x524cc with a two-float vector holding `+0x1c` and 0.
- The helper reads `angles2[0]` and `angles2[1]`, calls `AngleSubtract` on each against that vector, scales its per-step limit by 0.05 (rodata `0x7590c`), takes the limit from 200.0 (rodata `0x75904`) when its third argument is zero (what `turret_think` passes), and leaves the step it could not take in `angles2[2]`.
- `turret_controller` (0x53348) hands (`angles2[0]`, `angles2[1]`, 0) to `G_DObjSetControlTagAngles` for `tag_aim` and `tag_aim_animated`, and (`angles2[2]`, 0, 0) for `tag_flash`.

INFERRED, control flow: the sweep runs 31 steps from 0 to -90 degrees and stops at the first whose trace does not reach its end. So `+0x1c` holds the pitch at which the segment from `tag_aim` to the swung `tag_butt` first meets the world, and -90 survives a sweep that hits nothing or a model missing either tag.

INFERRED, from what the swept segment is: `tag_butt` is the stock, so an unmanned MG42 tips its barrel up until its stock is in the dirt.

INFERRED, control flow: `turret_think` walks `angles2[0]` toward that pitch by at most 10 degrees a frame, so a -72 turret settles eight frames after its first think and holds, which is why every trace reads the same value.

VERIFIED, the retail gamestate fixture: both carentan turrets' baselines read `angles2[0]` -20. INFERRED, from that and the two facts below: the baseline pass runs after the load's three settle frames (map-cycle doc, section 3 step 21), so `turret_think_init` on the first (100 ms past the spawn) moved nothing and the two after it took one step each. vcod: the `TurretRecord::init_at` field, `sweep_rest_pitch` (`crates/server/src/game/spawn.rs`) and `slew_home` (`crates/server/src/game/turret.rs`).

VERIFIED by reproduction: replaying the sweep against `xmodel/mg42_bipod`'s `tag_aim` (-2.9806, 0.0611, 12.8727) and `tag_butt` (-49.6281, 0.0000, 10.9540) on mp_carentan's collision gives exactly -63 and -72, with a ray that stops on `CONTENTS_SOLID` alone. A ray that also stops on playerclip gives -60 for the second turret, so retail's 0x11 does not cover playerclip (0x10000 in this engine).

#### What a player entity looks like

A client is sent no entity for itself, so this comes from two probes on one retail server, the second capturing what it was told about the first (`--net-probe --save-entities --capture-tag players`, committed as `crates/server/tests/fixtures/entities/mp_carentan-dm-players.txt`).

VERIFIED from that capture: `eType` 1, `index` 0, `clientNum` the slot, `eFlags` 16, `solid` 6684943, `weapon` the 1-based configstring 7 index, `groundEntityNum` 1022 or 1023. The body model is not on the entity; it rides the `clientState` roster's `modelindex`.

**A player travels as a trajectory.** `pos.trType` 3 with `pos.trTime`, `pos.trDuration` 50 and a `pos.trDelta`, so the receiving client carries the motion between snapshots. `apos.trType` is 1 and `apos.trBase[1]` is the body yaw; a player entity carries no pitch.

**A player's `pos.trBase` is whole units, truncated.**

- `BG_PlayerStateToEntityStateExtrapolate`, called with its snap argument 1 from `ClientThink_real` and `ClientEndFrame`, stores each axis through `fistp` under the control word or'd with 0xc00 (round toward zero). VERIFIED, 1.1d `game.mp.i386.so` 0x2d1e5..0x2d217, 1.5b 0x1cca9, UO 1.51b `game.mp.uo.i386.so` 0x212f5..0x2133d.
- VERIFIED, captures: every `eType` 1 entity in the committed 1.1 (`*-turret-hit.bin`, `*-panzer.bin`), 1.5 (`*-1.5.bin`, `*-15-turret.bin`) and UO (`*-uo.bin`, `*-uo-tank.bin`, `*-uo-turret.bin`) snapshot pairs has whole `trBase` values, 3313 entity states, none fractional. A UO `client-probes/probe_minefield` victim falling from z 64 reads 63, 60, 55, 50 where its playerstate held 63.97 and below.
- The corpse `cloneplayer` builds and a `script_model` spawned at the player's `origin` keep the fraction (`r.currentOrigin` is `ps.origin` again once the cmd's link is done, UO 0x50c91 `ClientThink_real`). vcod truncates and keeps the fraction in the same places.

**`solid`.** `SV_LinkEntity` (cod_lnxded `0x80908b0`) packs `(maxs.z + 32) << 16 | -mins.z << 8 | maxs.x`, each byte truncated and clamped to `[1, 255]`, and 0 unless `r.contents` holds 0x2000000 or 0x1. VERIFIED, the stores, the float 32.0 at `0x80d6648` and the compares against 1 and 255; the branches are INFERRED. The server's player box starts at the feet (`mins.z` 0), so the middle byte is the clamp's 1, not a box one unit deep; a client decoding `mins.z = -zd` puts the box floor one unit below the feet. VERIFIED, the player-clip capture: 6684943 standing, 5374223 crouched, 4063503 prone, the packing of `maxs.z` 70, 50 and 30 with `maxs.x` 15. When it is written and what a client does with it: `docs/research/cod11-player-clip.md`, sections 4 and 5.

**`angles2[1]` is the player's `movementDir`.** VERIFIED, 1.1d `game.mp.i386.so`: `BG_PlayerStateToEntityStateExtrapolate` loads `ps+0x7c` and stores it as a float into `es+0x6c` at `0x2d06d`. `BG_PlayerStateToEntityState`, the non-extrapolating twin, does the same at `0x2cd4c` with an unsigned-byte fixup, subtracting 256.0 from a value above 128. VERIFIED, `cod_lnxded`: the playerstate netfield table at `0x080d229c` names `ps+0x7c` `movementDir`, 8 bits; the entity table at `0x080d1760` names `es+0x6c` `angles2[1]`.

It is the yaw the legs move along, off the view yaw, in whole degrees, so a client can turn a strafing player's legs while the torso faces the view. `PM_SetMovementDir` (`0x2e970`, `game.mp.i386.so`) computes it; RTCW-MP's `bg_pmove.c` function of the same name is the same code with a different cap. INFERRED, all off that function's control flow:

- 0 unless a move key is held, `groundEntityNum` is not 1023 and the frame's displacement exceeds `pml.frametime * 5`.
- Otherwise `AngleDelta(vectoyaw(origin - pml.previous_origin), viewangles[YAW])`, truncated to an int, folded by 180 when `forwardmove < 0`, capped at +-90 (`0x2e9d2`).
- Prone: `AngleDelta(ps.proneDirection, viewangles[YAW])` (`0x2e98d`).
- Ladder: `vectoyaw(ps.vLadderVec) + 180` (`0x2e9f6`); `PM_LadderMove` writes the same value itself at `0x33df0` with a 75-degree cap.

VERIFIED against retail with held inputs (`--net-probe --save-motion` on `mp_carentan`, poses the committed motion fixture lacks; nothing in that fixture moves the field off zero):

- Running forward into a wall and scraping along it: `movementDir` 42 with view yaw -42.85 and velocity along +x. That is the displacement angle truncated, not the key's direction.
- Backpedalling: 254 against a 178.50-degree displacement, `trunc(178.50) + 180` folded to -2 and read back as an unsigned byte.
- Three prone poses: 0, 197 and 83 against a `proneDirection - viewangles[YAW]` of 0.00, -60.00 and 83.13.

`movementDir` is an 8-bit playerstate field, so a negative one arrives as its unsigned byte, which the 256.0 subtraction in `BG_PlayerStateToEntityState` undoes; `angles2[1]` carries the signed value.

The two-probe capture does not line up. Of its four player samples one agrees (`angles2[1]` 2 against a 2.4-degree gap between `pos.trDelta` and `apos.trBase[1]`), one is airborne and reads 0 as the formula predicts, and two disagree (3 against 64, 27 against 34). Its route is not reproducible and the walked client's keys at each station are unknown, so it is not evidence either way; the held-input run is.

The capture also carries `legsAnim` and an event ring with its `eventParms`. Neither is identified here.

#### The rule, out of the binary

`SV_BuildClientSnapshot` is at `0x808f130` (it pushes the `SV_BuildClientSnapshot: bad gEnt` string at `0x80d63e0` from `0x808f270`). The entity selection it calls at `0x808e298` is Q3's `SV_AddEntitiesVisibleFromPoint` in shape. VERIFIED, all read out of the 1.1d dedicated binary:

- the loop bound `sv.num_entities` at `0x83b6684`;
- the gentity accessor at `0x8089258` and the parallel server-side record at `0x8089288`;
- the collision helpers at `0x8053d34` (point to leaf), `0x804bca4` (leaf area), `0x804bc8c` (leaf cluster), `0x8053cfc` (cluster PVS row) and `0x8053f44` (areas connected);
- the gentity offsets `+0xf0`, `+0xf4`, `+0xf8`; the server-record offsets `+0x118`, `+0x11c`, `+0x15c`, `+0x160`, `+0x164`;
- the immediates `0x01`, `0x18`, `0x0800`, `0x2000`, `0x400`.

INFERRED, since every clause is a branch condition, the per-entity rule:

1. Skip an entity whose `+0xf0` is zero.
2. Skip one whose `+0xf4` has `0x01` set.
3. With `0x0800` set, send it only to the client in `+0xf8`; with `0x2000` set, to every client but that one. "The client" is the snapshot's `ps.clientNum`, a followed or killcam player's number rather than the slot's (`docs/research/cod11-events-and-fx.md` section 2).
4. Never send a client its own entity, the one whose number equals `ps.clientNum`.
5. Send unconditionally if `+0xf4` has either bit of `0x18`.
6. Otherwise require the entity's area to connect to the client's and at least one of its `+0x118` clusters to be set in the client's PVS row, with a scan up to `+0x15c` when the cluster list did not settle it.

The list stops at `0x400` entries.

VERIFIED decompile, the same function and its 1.5b (`0x8095c30`) and UO (`coduo_lnxded` `0x8096b97`) twins: all of the above runs only when the eye's leaf has an area of 0 or more. An eye in a solid leaf (area -1) is sent no entity at all, broadcast and temp entities included. VERIFIED live on 1.1d and UO 1.51b (`client-probes/probe_snapcull`, mp_carentan, 2026-10-20): a player held 1000 units under the map got empty entity lists for as long as he was there.

Areas:

- `CM_AreasConnected` (`0x8053f44`) is false for area -1 and otherwise compares the two areas' flood numbers.
- VERIFIED decompile, UO (`0x805793e`, `0x80579a3`): the floods follow area portals whose counts start at 0 and change only through `trap_AdjustAreaPortalState`, which the game calls from its binary movers (`Reached_BinaryMover`, game.mp.uo `0x721ae`).
- VERIFIED, every stock map's entity lump on 1.1, 1.5 and UO: no MP map has a `func_door` or any other `func_` class, so each area is its own flood.
- An entity's two areas are the first area its link's leafs carry and the last other one (UO `SV_LinkEntity` `0x8099dee`).
- Every stock 1.1 MP map's leafs carry area -1 or 0. UO's mp_berlin, mp_ponyri, mp_rhinevalley and mp_uo_stanjel and 1.5's mp_bocage have small leaf groups in areas 1 to 7 (VERIFIED, lump 21). INFERRED: an entity only in one of those reaches only an eye in the same area; no stock spot measured.

vcod-server ports both tests (`world::Viewer`, `mapworld::LinkClusters::areas`).

#### What links an entity, out of the game module

`game.mp.i386.so` keeps its symbols, so the callers can be counted. VERIFIED: `trap_LinkEntity` (`0x63a40`, engine syscall `0x32`) has 65 call sites in 51 named functions.

- Static (linked before anybody plays): `SP_script_model`, `SP_script_brushmodel`, `SP_script_origin`, `SP_func_rotating`, `SP_func_leaky`, `SP_misc_spawner`, `use_corona`, the five `SP_trigger_*`, `InitMover`, `InitMoverRotate`, `G_SpawnItem` with `FinishSpawningItem`, and `G_SpawnTurret`.
- Runtime: items touched or respawned, movers reaching a point, missiles, temp entities, and clients through `G_RunClient` and `ClientThink_real`.

VERIFIED, the same module, the stores to the entity's `+0xf4`:

| writer | value |
|---|---|
| every `SP_trigger_*`, `InitTrigger` | `1` (the bit the selection skips) |
| `InitMover`, `InitMoverRotate`, `InitScriptMover`, `G_SpawnTurret` | `0x80` |
| `ClientConnect` | `0x200` |
| `fire_grenade`, `fire_rocket` | `0x88` |

INFERRED, from the two together: a trigger is linked and never sent, and what a map puts on the wire before anyone plays is its script models, brush models, script origins, movers, items and turrets. That matches the traces: `eType` 8 script models, `eType` 3 items, `eType` 11 turrets, nothing else on either map.

Consequences:

- The client's own entity being skipped is the same fact stage 4's playerstate fixture records from the other side, and it is why an entity-list diff cannot see a client's own entity.
- An entity reaches nobody until something links it: `+0xf0` gates every other test.
- An entity-list fixture is a fixture *of a position*. Two captures at different spawns on one map disagree for reasons unrelated to which entities exist.

#### Corpses, temp entities and the numbers vcod gives them

Beside the map's own entities a snapshot carries the body queue's corpses and the events raised that frame.

A corpse is culled like any other entity, with the player's link box. A temp entity carries one event. Retail's `obituary` sets `r.svFlags` to 8, `SVF_BROADCAST` (`.so 0x5a7ec`, `docs/research/cod11-hud-protocol.md` section 1), which sends it to every client regardless of PVS. VERIFIED: the dm hit capture's shooter, 2000 to 4500 units from the victim with no line of sight, received every obituary (`docs/research/cod11-combat.md` section 8.3). vcod handles the three cases the flag word spells: a broadcast temp entity skips the cull, one withheld from a single client is dropped for that client and culled for the rest, and one addressed to a single client reaches only it, culled.

Entity numbers, one label each:

- Clients take 0..63. VERIFIED, `MAX_CLIENTS` is 64.
- The body queue takes 64..71. VERIFIED as retail's own range: `G_SpawnPlayerClone` indexes `&g_entities[64 + bodyQueIndex]` with the index advanced `& 7` (`docs/research/cod11-combat.md` section 5.2).
- Map and script entities run from 72 up. VERIFIED: `G_InitGame` sets `level.num_entities` to 72 whatever `sv_maxclients` is.
- Temp entities take their numbers off the same free list as script spawns and ride every snapshot until the entity pass frees them, more than 300 ms past their event. VERIFIED: `G_TempEntity` (0x67938) calls `G_Spawn` (0x67947), and `client-probes/probe_entnum` on retail put a `playFx` between two spawns' numbers (`docs/research/cod11-combat.md` 14.7, "Entity numbers"). A client keys a fired event entity on `(eType, eventParm)` per number and forgets a number only once it leaves the snapshot. A freed number is out of the snapshot of the frame that frees it, so its next event reads as new.

### svc_serverCommand (5)

`long sequence`, then `bigstring text`. Dedupe by sequence and store in the 64-slot ring, which the scramble key reads. Then dispatch on the leading token. CoD 1.1 server commands are single letters (`CG_ServerCommand` switches on `argv0[0]`; full table in section 0 of `docs/research/cod11-hud-protocol.md`). The ones the engine side of a client acts on:

- `d <index> <text>` is the configstring update (Q3's `cs`). The text is unquoted and runs to the end of the line, spaces included (`d 12 0 1 0.00025 0.32 0.36 0.4 0`, `d 0 \codextended\CoDExtended v20\...`), so take everything after the index verbatim. Index 1 is the systeminfo: re-read `sv_serverid` from it (map_restart below).
- `n` is `map_restart`: the server has re-entered every connected client into the world.
- `w "<reason>"` is the per-client drop notice, a reliable command like any other: `SV_DropClient` (`0x8085cf4`) queues it and the slot goes to `CS_ZOMBIE`, whose messages carry it. The reason is a localized key: `EXE_DISCONNECTED`, `EXE_TIMEDOUT`, `EXE_LOSTRELIABLECOMMANDS` (a gap in the client's command sequence), `EXE_SERVERCOMMANDOVERFLOW` (the queue below), `EXE_PLAYERKICKED`. VERIFIED live: zombie captures read `w "EXE_DISCONNECTED"` in each message (`docs/research/cod11-server-handshake.md`, "Zombie slots"). vcod's server sends it the same way, and vcod's client drops the connection with the reason it carries.
- The bare word `disconnect` runs the other way: a leaving client sends it, out of band (`FF FF FF FF disconnect`, which retail matches at `0x808c827` and ignores, as vcod's server does) or as a reliable client command. It is not the server's drop notice. VERIFIED, CoDMP.exe 1.1 decompile: the function at `0x40ef90` passes `"disconnect"` to `0x40e2f0` and holds three calls to `0x40ba50` beside it. INFERRED from control flow and the Q3 shape: `CL_Disconnect` queues the command and writes three packets, only while the connection-state global at `0x155f2c0` is above 2, so one lost datagram does not hold the slot until `sv_timeout`. vcod's `NetClient::disconnect` sends three too.
- `h`/`i` chat and team chat, `e`/`f`/`g` prints, `b` the scoreboard reply and the rest are cgame-level (`docs/research/cod11-hud-protocol.md`).

Sound letters, all cgame-level, from section 9 of `docs/research/cod11-sound-system.md`:

- `s <idx>` is `playLocalSound`: play `CS_SOUNDS + idx` non-positionally. It is addressed to one client. A spectator receives its own (the SD announcer arrives this way); sounds addressed to the followed player are not forwarded.
- `o <alias>`, `p <fade ms>` and `q %f %i` are `musicPlay`, `musicStop` and `soundFade`. No stock MP gametype or map script calls them and no live capture saw one. vcod's server sends all three from the builtins and its client handles them (sound doc section 10). `q`'s format ends in a newline (`.rodata` `0x77568`), which the tokenizer drops.
- `j <scope> <int> <clientNum> <category> <x> <y> <z>` is quick chat (`vsay`). The letter encodes the scope (`j` global, `k`/`l` team), argv 4 names a category (`kill_insult`, `praise`, ...) and the last three fields are the speaker origin cast to int. `G_Voice` (`0x473b8`, format string at `0x73a4e`) sends it. The client resolves the category through `mp/*_chat.voice` tables, which no stock install ships (`docs/research/cod11-quick-chat.md`).

### The server command queue

Retail does not send a server command when it is issued. `SV_AddServerCommand` (`0x808b680`) queues it in the client's 64-slot ring, and the next message carries every command past `reliableAcknowledge`. A command still waiting can be replaced by a later one or dropped. Q3 has no equivalent. `client_t` offsets (stride `0x5a8fc`): state `+0x0`, `reliableSequence` `+0x1060c`, `reliableAcknowledge` `+0x10610`, `reliableSent` `+0x10614`, slot `n` text at `+0x40c + n * 0x408` (1024 bytes), its time at `+0x80c`, its type at `+0x810`.

- **Type.** Every caller passes a type with the text. VERIFIED, the pushes ahead of each `trap_SendServerCommand` call in `game.mp.i386.so` (69 sites) and each `SV_SendServerCommand` (`0x808b900`) call in cod_lnxded:
  - type 0: `c`, `e`, `f`, `g`, `h`, `i`, `j`/`k`/`l` (`G_Voice`), `s`;
  - type 1: `a`, `b`, `d` (`SV_SetConfigstring`, `0x8089d0f`), `m`, `n` (`0x80d39f1`), `o`, `p`, `q`, `r`, `t`, `u`, `v`, `w`.

  No letter appears with both. vcod derives the type from the letter (`CmdKind::of`, `crates/server/src/client.rs`).
- **Bots.** VERIFIED: `0x808b68c` returns at once when `client_t + 0x5a8f4` is set, and the one store to that field is the `1` at `0x80876a3` in `SV_AddTestClient` (the `bot%d` connect string). INFERRED: a test client is queued nothing. vcod's bots read the ring themselves, so the rules below skip them.
- **Not ready.** VERIFIED: `0x808b6aa` compares `reliableSequence - reliableAcknowledge` with `0x1f`, and `0x808b6af` the state with 4. INFERRED, off those branches: when the client is more than 31 commands behind or not `CS_ACTIVE`, the loop at `0x808b6d1` closes up the unsent part of the queue (`reliableSent + 1` on) over every type-0 slot, and a type-0 command being added returns without queueing (`0x808b733`). So a print sent to a connecting client is lost, and a cvar is not.
  - VERIFIED live, `client-probes/probe_squash` on retail 2026-10-07: an `iprintln` before `waittill("begin")` never arrived; the `setClientCvar` beside it did.
  - VERIFIED live for `CS_PRIMED`, 2026-10-20 on 1.1d, 1.5b and UO 1.51b: `client-probes/probe_primedprint` on mp_carentan, the client's sequenced packets dropped for 3 s after its gamestate (`crates/server/tests/fixtures/handshake/primed-print.txt`). Of a self `iprintln`, a broadcast `iprintln` and a `setClientCvar` every quarter second until `begin`, no print arrived and every cvar did: the one queued while connected inside the gamestate message, the rest with the first snapshot.
  - vcod matches (`crates/server/tests/connect.rs`, `prints_sent_while_primed_are_dropped_and_cvars_kept`).
- **Squash.** VERIFIED: `0x808b580` (called at `0x808b748`) scans `reliableSent + 1 ..= reliableSequence`. It skips a slot whose type is 0 (`0x808b5a6`) or whose first letter differs or is `x`, `y` or `z` (`0x808b5c6`, the big configstring chunks), returns the slot on a `strcmp` match of the rest (`0x808b5e4`), and otherwise jumps through the table at `0x80d5820`: `a`, `b`, `o`, `p`, `q`, `r` and `t` land on the return (`0x808b65e`), `d` and `v` on a comparison of the first space-delimited token after the letter (`0x808b606`), everything else on the next slot. INFERRED, off `0x808b74d`..`0x808b7be`: a found slot is removed by shifting the later ones down and the new command takes the last slot, so `reliableSequence` does not move; with none found it goes up by one.
  - So an unsent `d <n>` or `v <name>` is replaced by a later one for the same index or name, an unsent `t`, `b` or `a` by any later one, and anything by the same text. A command already carried by a message (`reliableSent`, raised by the snapshot writer at `0x808f917` and `0x808faf8`) never is.
  - VERIFIED live, the same probe. One frame of `setClientCvar("sq_a", 1)`, `("sq_a", 2)`, `("sq_b", 1)`, `("sq_c", 1)`, two `iprintln("sq dup")`, two `closeMenu()`, `("sq_d", 1)`, `iprintln("sq between")`, `("sq_d", 2)` arrived as `v sq_a "2"`, `v sq_b "1"`, `v sq_c "1"`, `f "sq dup"` twice, one `u`, `f "sq between"`, `v sq_d "2"`. `("sq_a", 3)` and `("sq_a", 4)` half a second apart both arrived.
- **Overflow.** VERIFIED: `0x808b7ea` compares `reliableAcknowledge` with `reliableSequence - 65`. On equality it prints the queue (`===== pending server commands =====`), calls `0x8087880` with `EXE_SERVERCOMMANDOVERFLOW` (`0x80d58c2`) and stores `w "EXE_SERVERCOMMANDOVERFLOW"` (`0x80d58dc`) in the slot. VERIFIED: `0x8087880` writes its argument to `client_t + 0x8` when the state is not 1 and that field is still 0. INFERRED: a drop deferred to later in the frame, so the client is dropped once a 65th unacked command would overwrite the oldest, and its last message carries the notice. vcod drops at once (`Queued::Overflow` in `send_server_command`), through the same zombie path as any drop, so the notice still rides the zombie's messages.

vcod's server queues the same way and flushes the queue with each snapshot and gamestate.

### map_restart and sv_serverid

`sv_serverid` is one byte with two fields:

- `SV_SpawnServer` (`0x808a598`) adds `0x10` per map load (`movzbl`, so it wraps at 256 and fits the 1-byte header below).
- `SV_MapRestart_f` (`0x8083eb5..0x8083ece`) bumps the low nibble modulo 16 with `(id & 0xf0) + ((id + 1) & 0xf)`.
- Both toggle `SNAPFLAG_SERVERCOUNT` (`xorb $4` on the byte at `0x83b67a8`, `0x808a516` and `0x8083eba`).
- The high nibble skips zero on wrap. VERIFIED: the first `add al, 0x10` is at `0x808a59e` and stores at `0x808a5a3`; a `testb $0xf0` on the same byte sits at `0x808a5ab` with a `jne` to `0x808a5bc`; a second `add eax, 0x10` at `0x808a5b4` stores at `0x808a5b7`. INFERRED, off that test and jump: the second add runs only when the first left the high nibble zero, so after the first load the high nibble cycles 1 to 15.
- The counter starts at zero, so a fresh server's first map runs at `0x10`. INFERRED from the add; vcod's server uses it. The populated capture's `\sv_serverid\18` is a server several loads and restarts in.

Round-based gametypes (SD) restart the map between rounds, so the id changes every round.

A map change is announced out of band and the gamestate is pulled, not pushed:

- VERIFIED: `SV_SpawnServer` has a client loop at `0x808a2c0..0x808a30c` with a `cmp state, 2` at `0x808a2c6`, a connectionless send of `loadingnewmap\n<map>\n<gametype>` inside it (`NET_OutOfBandPrint`, format string `0x80d5471`), and a call to `0x80c786c(250)` after the loop at `0x808a324`. INFERRED: the line goes to every client at `CS_PRIMED` or above, and the 250 ms wait follows the last of them, before anything is torn down.
- VERIFIED: the function calls no `SV_SendClientGameState` (`0x8085eec`); the binary has three calls, all reached from `SV_ExecuteClientMessage` or the `donedl` handler.
- VERIFIED: it writes configstrings 0 and 1 while `sv.state` still reads 1 (`0x808a8c9`, `0x808a8e1`), and `SV_SetConfigstring` has a `cmp sv.state, 2` at `0x8089c6c` and a `cmp sv.restarting, 0` at `0x8089c75` ahead of its per-client loop.
- INFERRED, from those three: neither write reaches a client. A client learns of the change from the OOB line, and the gamestate arrives only after its next message reaches the new server carrying the old serverId. Full read-out: `docs/research/cod11-map-cycle.md`.

`SV_MapRestart_f` then, for every client at `CS_CONNECTED` or above, queues `n`, calls `ClientConnect` again, and re-enters `CS_ACTIVE` clients with `SV_ClientEnterWorld` (`0x80877d8`: state stays `CS_ACTIVE`, `deltaMessage = -1`, `ClientBegin`, which respawns a spectator at the intermission point, usually a floating camera spot). The new systeminfo goes out as `d 1 ...` right after `n`.

`SV_ExecuteClientMessage` (`0x80872ec`) compares the header serverId with `sv_serverid`:

- Equal: normal processing.
- High nibble differs (a map change): resend the gamestate if the client's `messageAcknowledge` is past the gamestate message, then return.
- Only the low nibble differs (a restart): a `CS_PRIMED` client is promoted to `CS_ACTIVE`; anything else returns without reading an op.

So a client that keeps sending the pre-restart id after a round restart is silently ignored: no moves, no reliable commands, no snapshots, until its own snapshot-silence timeout drops it. VERIFIED live 2026-08-24 on a public SD server: with the stale id nothing arrived for 30 s after `n` and the client timed out; with the id re-read from `d 1`, snapshots resumed within a second (delta frames, spectator back at its spawn) and moves applied again.

### svc_download (6)

RTCW's UDP download protocol, same flow. The client asks with a reliable `download <name>.pk3` (names from `sv_referencedPakNames` in the systeminfo, gamedir prefix included, e.g. `main/n_dufresne.pk3`). The server streams blocks inside its regular messages:

```
short block           ; 0-based, in order
if block == 0:
  long  fileSize      ; -1 = refusal, then a string with the reason
short chunkLen        ; 0 = EOF
byte  data[chunkLen]
```

- Ack each block with a reliable `nextdl <block>`; the server's send window retransmits until acked. A zero-length block ends the file. `stopdl` aborts; a final `donedl` makes the server resend the gamestate.
- **Divergence from RTCW, #7.** The client caps no block size, unlike RTCW's 2048. `CL_ParseDownload` (CoDMP.exe 0x004162a0) reads the length as a short and copies that many bytes into a 16 KiB stack buffer with no bound check, filling 0xff past the message's end. VERIFIED (decompile). Public servers send 8192- and 11800-byte blocks (observed live); the 1.1d Linux server sends 2048-byte blocks (`docs/research/cod11-server-handshake.md`, "Serving a download"). vcod takes up to 16384 and drops a block that runs past its message.
- There is no `sv_dl_maxRate`. 1.1d fills each message with the client's rate over one snapshot interval and sends it at the client's `nextSnapshotTime`, one fragment per frame (`docs/research/cod11-server-handshake.md`, "Message pacing"), so one message can carry several blocks.
- A client never asks for a stock pak: `FS_ComparePaks` skips what `FS_idPak` names (`main/pak0`..`pak8`, `mp_pak*`, `sp_pak*`, `mp_bin`, `localized_*_pak*`; 1.5 `pak0`..`pakf` and `_svr_` paks, `docs/research/cod15-protocol-diff.md` section 10). The server's own `FS_idPak` check sees the name with `.pk3` and never matches, so retail serves a stock pak that is asked for, and so does vcod-server.

## Delta field encoding

Field tables are arrays of `{ char *name; int offset; int bits }` (12 bytes per entry on i386), Q3's `netField_t`. The 1.1 binary is stripped but keeps every field-name string, so the tables come back by finding the name pointers in `.data` and walking the 12-byte stride: 59 entity fields (table at `0x080d1760`) and 103 playerState fields (table at `0x080d229c`). Wire order is array order; never sort them.

`bits == 0` is a float field; a positive `bits` is an integer width. The playerState table also has negative widths (`-16`, `-8`). They are not sign flags: CoD 1.1 reads them as plain unsigned of width `|bits|` (`read_delta_playerstate` `0x807e2f0`, integer read at `0x807e5d7`, no `sar`). `weaponTime` (`-16`) written as `0xffff` reads back `65535`. Take the absolute value and read unsigned.

The binary has two incompatible bit-field codings that agree below 8 bits and part at 8 and above: one for entity numbers and small counts, and `read_packed_bits` (`0x807cb03`) for delta field values. Use the one the engine uses at each site, or the first wide field desyncs.

### Entity delta (svc_baseline and packet entities)

Q3's `MSG_ReadDeltaEntity`: a "last changed field" index, then a change bit per field. A changed field reads its width. A float reads a zero-flag bit, then either a truncated 13-bit int-float or a full 32-bit value. Floats and the `lc` byte are raw bytes at the byte cursor.

The zero flag needs care in a writer. "Changed, and zero" against a `+0.0` base means the sender's value was `-0.0`: the C compares the two slots as `int`s before it compares floats, so `+0.0` against `+0.0` is no change and only the other zero gets there. Retail emits exactly this: `mp_carentan`'s baselines for entities 253 and 256 carry `apos.trBase[0] == -0.0`. A reader that stores `0.0f` (as the C client does) cannot reproduce the message it read. Recovering `-0.0` in `read_float_field` makes the round trip byte-exact.

### PlayerState delta

Like the entity delta with no entity number and (divergence) no zero flag in the main loop. After the scalar fields come five array blocks the 103-entry table does not cover ("The five array blocks" below).

Same `-0.0` trap without the flag. The change bit still means the C saw two different `int`s, but a changed float goes out through the integral path as a truncated 13-bit value, so `-0.0` reads back `+0.0`. A reader that keeps `+0.0` re-encodes the field as unchanged and shortens `lc`. VERIFIED against retail 1.1d on mp_carentan: as a spectator's move ends, `velocity[2]` rides three frames as changed-but-zero, and retail's `lc` runs to field 54 to carry it while the last real change is field 15 (frames 134 to 136 of `crates/common/tests/fixtures/net/snapshots-delta.bin`; storing `+0.0` gives `lc == 16` and a frame six bytes short). Recover `-0.0` when the decoded value equals the base, as `read_float_field` does for entities.

Integer fields have the same trap one level up. The C compares whole ints and sends only the low `|bits|`, so a field whose two ints differ only above its width comes back equal to the base behind a set change bit, and an entity integer whose low bits are 0 comes back 0 behind a set zero flag. VERIFIED on retail UO 1.51b (protocol 22, same codec): the follow landing in `snapshots-uo-flame.bin` (frame 523) sends `delta_angles[1]` 0x3555 over a base of 0x3555, and the smoke grenade's first frame in `snapshots-uo-smoke.bin` (message 150) sends `eFlags` with the zero flag set and 24 packed zero bits. vcod's readers set bits above the width until the value differs from the base and from 0 (`above_width`, `crates/common/src/net/codec.rs`); the writer masks them off, so the frame re-encodes byte for byte.

The array blocks are gated against the base frame: full on an uncompressed frame, empty once the client is deltaing and nothing changed. VERIFIED across the two committed captures: all 25 uncompressed frames carry them (42 primitives each) and none of the 399 delta frames does, a spectator's stats and HUD never changing (`crates/common/tests/fixtures/net/snapshots.bin` and `snapshots-delta.bin`). Skipping them keeps the parse aligned but loses the data, so vcod keeps them all in `PlayerState::arrays`: blocks 1 to 3 decode into `stats`, `ammo` and `ammoclip` and re-encode gated against the base, block 4 into `objectives` and block 5 into `hud_archived`/`hud_current`.

#### The five array blocks

Two functions carry the layout. Addresses are in `cod_lnxded` 1.1d unless they name `game.mp.i386.so` (the 1.1d MP game module, full dynamic symbols, module-relative addresses, image base 0, as `docs/research/cod11-gsc-object-model.md` reads it). The reader is the tail of `MSG_ReadDeltaPlayerstate` (`0x807e2f0`), `0x807e7b3..0x807eeb6`. The writer is the tail of `MSG_WriteDeltaPlayerstate` (`0x807d0f8`), `0x807d893..0x807e2ef`. VERIFIED: the two agree element for element. INFERRED, from that: a captured frame round-trips.

`playerState_t` is `0x20D0` bytes for delta purposes: the writer `bzero`s exactly that many for a null base (`0x807d116`). VERIFIED. Offsets below are into that struct. Member names are CoDExtended's `playerState_t` (`shared.h`), whose `weapons[0]` at 780, `weaponslots[0]` at 788, `viewangles[0]` at 192, `weapon` at 176 and `weaponstate` at 180 match the netfield table's offsets. VERIFIED.

Three blocks name themselves. With `cl_shownet` at 4 (cvar cached at `0x8355214`, registered at `0x80813e0`) the reader prints `PS_STATS` (`0x80d2e35`) before block 1, `PS_AMMO` (`0x80d2e42`) before block 2 and `PS_AMMOCLIP` (`0x80d2e4a`) before block 3. VERIFIED. Blocks 4 and 5 print nothing; their names below come from what writes the offsets.

**Block 1, `stats[6]` at `0xF4`.** One gate bit; if set, six raw bits low to high select the elements that follow, read in element order. VERIFIED (reader `0x807e7eb`..`0x807ea0a`, writer `0x807d893`..`0x807da44`). Widths are per element:

| element | offset | wire | meaning | label for the meaning |
|---|---|---|---|---|
| `stats[0]` | `0xF4` | 16-bit, byte-aligned, sign-extended | health | VERIFIED |
| `stats[1]` | `0xF8` | 16-bit, byte-aligned, sign-extended | dead yaw (the angle a corpse's camera settles on) | INFERRED |
| `stats[2]` | `0xFC` | 16-bit, byte-aligned, sign-extended | max health | VERIFIED |
| `stats[3]` | `0x100` | 6 raw bits, low to high, unsigned | entity number of the teammate the compass/HUD is naming, `-1` for none | VERIFIED |
| `stats[4]` | `0x104` | 16-bit, byte-aligned, sign-extended | that teammate's health | VERIFIED |
| `stats[5]` | `0x108` | 8-bit, byte-aligned, unsigned | spawn counter | VERIFIED |

Evidence per meaning, all in `game.mp.i386.so`:

- `stats[0]`: `Scr_SetHealth` (`0x5c68c`) writes `ps+0xF4` at `0x5c6b0`; `Pickup_Health` (`0x4d400`) writes it at `0x4d569`. VERIFIED.
- `stats[1]`: `PM_UpdateViewAngles` (`0x32d7c`) compares `ps+0xF8` against the immediate 999 and, on a match, stores `SHORT2ANGLE(cmd->angles[1] + ps->delta_angles[1])` there; `LookAtKiller` (`0x4a938`) stores an angle in the same slot. VERIFIED. INFERRED: it is RTCW's `stats[STAT_DEAD_YAW]`, whose `bg_pmove.c:3385` is the same 999 sentinel in the same function.
- `stats[2]`: the `maxhealth` script-field setter (`0x41a78`) stores `sess.maxHealth` (`gclient+0x2150`) into `ps+0xFC` at `0x41add`, and `ClientSpawn` (`0x4268c`) stores it again at `0x4284f`. VERIFIED. INFERRED, off sequencing and branches: the second store lands after the respawn memset (`0x42804`), and the setter clamps `ps+0xF4` down to `sess.maxHealth` and copies the result into `gentity->health` (`gentity+0x230`).
- `stats[3]` and `stats[4]`: `TeamplayInfoMessage` (`0x64910`) has two pairs of stores to `+0x100` and `+0x104`, both off a base loaded from `gentity+0x158`. One pair carries a traced entity number and that entity's `gentity->health` (`0x64acd`, `0x64ad9`), the other the immediates `-1` and `0` (`0x6493b`, `0x6494b`). It also compares a traced entity number against 63 (`0x64a83`). VERIFIED. INFERRED: the `+0x158` base is the `gclient_t` and `ps` sits at `gclient+0`, so those stores are `stats[3]` and `stats[4]` (same footing as "The box an entity links with"; `sess.maxHealth` at `gclient+0x2150`, past `sizeof(playerState_t)` `0x20D0`, puts `ps` at 0). INFERRED, from the compare and the branches: which pair runs, and so the reading "the teammate the HUD is naming, and their health".
- `stats[5]`: `ClientSpawn` loads `ps+0x108` at `0x427f8`, memsets the client at `0x42804`, and stores an incremented copy back at `0x42837`. VERIFIED, the three instructions. INFERRED, from the order: a spawn counter carried across the memset on purpose.

Two consequences of the widths. A `stats[3]` of `-1` goes out as six set bits and arrives as 63, so the wire cannot tell "nobody" from client 63; INFERRED. `stats[5]` is a byte, so the spawn counter wraps at 256; INFERRED.

**Block 2, `ammo[64]` at `0x10C`. Block 3, `ammoclip[64]` at `0x20C`.** Each is four sub-blocks of 16 elements. Element `i` of sub-block `s` lands at `0x10C + (s*64) + i*4` for block 2 (`0x807eaf1`) and `0x20C + (s*64) + i*4` for block 3 (`0x807ec03`). VERIFIED. A sub-block is one gate bit; if set, a byte-aligned 16-bit changed mask, then one byte-aligned 16-bit element per set bit, low to high, sign-extended into the `int` slot. VERIFIED (`0x807ea85`..`0x807eb3d`), off the reader and writer alone: every gate is 0 in both committed captures (the decoded frame below), so no round trip exercises the non-empty path.

Block 2 has a group gate ahead of its four sub-gates; block 3 does not. The group gate's `jae` (`0x807ea45`) lands on block 3's first sub-gate, so block 3's four gate bits are always on the wire. VERIFIED for the branch target; INFERRED that block 3 is ungated. An element is 16 bits, so a count above 32767 does not survive the round trip; INFERRED.

**Block 4, `objective[16]` at `0x3E8`, stride 28.** One gate bit for the block. Then per objective: three raw bits low to high stored unconditionally at the objective's `+0` (`0x807ed72`, index arithmetic `i*28` at `0x807ed6b`), one gate bit, and if set six generic delta fields read through the shared field reader `0x807c904` off entries 0..5 of the 34-entry field table at `0x80de384` (`0x807edc9`). VERIFIED. With the per-objective gate clear the six fields are copied from the base (`0x807ee00`). VERIFIED.

The six entries carry their names and offsets in `.data`; `python3 tools/re/dump_field_table.py cod_lnxded 0x80de384 34` prints them:

| table entry | member | offset in `objective_t` | wire |
|---|---|---|---|
| none | `state` | `+0` | 3 raw bits, always present |
| 0 | `origin[0]` | `+4` | float |
| 1 | `origin[1]` | `+8` | float |
| 2 | `origin[2]` | `+12` | float |
| 3 | `icon` | `+24` | 12 bits |
| 4 | `entNum` | `+16` | 10 bits |
| 5 | `teamNum` | `+20` | 4 bits |

VERIFIED. Wire order is the table's, not the struct's: `icon` precedes `entNum` and `teamNum`.

The values the game module writes there and its per-client filter are read out of `game.mp.i386.so` in `docs/research/cod11-gsc-object-model.md` 23.3, with an address for each:

- `state`: 0 for `"empty"`, 2 for `"invisible"`, 4 for `"current"`, the three names the state parser accepts. VERIFIED. Also 1, which no name maps to and only `objective_current` writes. VERIFIED. INFERRED, off that builtin's branches: the 1 demotes every unmarked slot that read 4, and is the "active" state.
- `entNum`: `0x3ff` on both `objective_add` and `objective_delete`, so an objective is on the wire with no entity until `objective_onentity` names one. VERIFIED.
- `teamNum`: 0 for `"none"`, 1 for `"axis"`, 2 for `"allies"`; `objective_add` stores 0 to it. VERIFIED. INFERRED, off that builtin's branches: the store is on every path, so every call resets it.
- `icon`: `G_ShaderIndex`'s answer, so it indexes configstring `1500 + n`, the range the `shaderIndex` hudelem field uses. VERIFIED.
- The copy into a client's block tests the record's state against 0, compares its `teamNum` with that client's `clientState.team`, and stores state 0 in place of the record. VERIFIED. INFERRED, off the branches between those sites: 0 goes out whenever the record's state is 0, or its `teamNum` is non-zero and differs from the client's team. The other team's objective reaches a client as an empty slot, not as nothing.

**Block 5, two 31-entry `hudelem_t` arrays, stride 112.** One gate bit for the block; then the array at `ps+0x1338`, then the one at `ps+0x5A8`, each with the element cap 31 passed in (`0x807ee76`, `0x807ee90`). VERIFIED. CoDExtended calls `ps+0x5A8` `hud.current[31]` and `ps+0x1338` `hud.archival[31]`; those names are the community's and UNVERIFIED. Only the offsets and the wire order come from the binary.

One array (`0x807cf5c`) is a 5-bit count `n`, then for element `i` in `0..n` a 5-bit last-changed field index `j` and `j+1` delta fields off table entries `6+k` for `k` in `0..=j`. VERIFIED. Fields `j+1..28` are copied from the base (`0x807d073`), which fixes the per-element field count at 28 and the table's total at 34. VERIFIED. There is no element index on the wire: element `i` is the array's `i`-th (`i*112`, `0x807d03e`). VERIFIED.

Entries 6..33 of the table are the `hudelem_t` fields, in wire order:

| entry | member | offset | wire | entry | member | offset | wire |
|---|---|---|---|---|---|---|---|
| 6 | `color.rgba` | 28 | 32 bits | 20 | `fromColor.rgba` | 32 | 32 bits |
| 7 | `type` | 0 | 4 bits | 21 | `fadeStartTime` | 36 | 32 bits |
| 8 | `fontScale` | 12 | float | 22 | `fadeTime` | 40 | 16 bits |
| 9 | `y` | 8 | 10 bits | 23 | `scaleStartTime` | 68 | 32 bits |
| 10 | `x` | 4 | 10 bits | 24 | `scaleTime` | 72 | 16 bits |
| 11 | `alignY` | 24 | 2 bits | 25 | `fromHeight` | 64 | 10 bits |
| 12 | `alignX` | 20 | 2 bits | 26 | `value` | 100 | float |
| 13 | `time` | 92 | 32 bits | 27 | `label` | 44 | 8 bits |
| 14 | `font` | 16 | 4 bits | 28 | `fromWidth` | 60 | 10 bits |
| 15 | `text` | 104 | 8 bits | 29 | `moveStartTime` | 84 | 32 bits |
| 16 | `shaderIndex` | 56 | 8 bits | 30 | `moveTime` | 88 | 16 bits |
| 17 | `width` | 48 | 10 bits | 31 | `fromX` | 76 | 10 bits |
| 18 | `height` | 52 | 10 bits | 32 | `fromY` | 80 | 10 bits |
| 19 | `sort` | 108 | float | 33 | `duration` | 96 | 32 bits |

VERIFIED. `text` and `label` are 8 bits wide, so they are indices, not strings: both are localized-string indices into configstring `1244 + n` (`game.mp.i386.so`'s `setText` 0x4c62e and the `label` field setter 0x4c422 both call `G_LocalizedStringIndex`). `shaderIndex` is `G_ShaderIndex`'s answer at 0x4b5f9, configstring `1500 + n`. VERIFIED.

Which array an element lands in, and which client gets it, `HudElem_UpdateClient` (`game.mp.i386.so` 0x4be8c) decides once per client per frame. `G_RunFrame` calls it for every in-use client with both arrays selected (0x50a80, argument 3). It clears both arrays, walks `g_hudelems` from slot 0, and copies 112 bytes per surviving record into the next free slot of one of them. VERIFIED:

- a record with a zero `type` is free and skipped;
- a record whose team field (`+0x74`) is non-zero and differs from the client's `clientState.team` is skipped;
- a record whose owner (`+0x70`) is neither `0x3ff` (`ENTITYNUM_NONE`) nor this client's entity number is skipped;
- `archived` (`+0x78`) picks `ps+0x1338` when set and `ps+0x5A8` when clear;
- both arrays stop at 31.

`SpectatorClientEndFrame` makes the same call with only `ps+0x5A8` selected (0x408bc, argument 2), right after copying the followed player's whole playerstate, so a following spectator inherits the archived half and gets its own unarchived one. VERIFIED, the call. INFERRED, off those two call sites: that is what `archived` means.

The count is the leading run of records with a non-zero `type` in the array being written, capped at 31 (0x807cd8e), and the reader clears every element past the count (`bzero` at 0x807d0e9) instead of keeping it from the base. VERIFIED. A destroyed element leaves the screen by the array being rebuilt from the top and the count dropping. Per element the writer picks the index of the last field that differs from the base, 0 when none does, so an unchanged element still costs its 5 bits and one clear change bit (0x807ce85). VERIFIED. The block's single gate is one `memcmp` over both arrays at once, which are contiguous (0x1b20 bytes from `ps+0x5A8`, 0x807e233). VERIFIED.

**As implemented.** `crates/common/src/net/msg.rs` decodes block 5 into `PsArrays::hud_archived` and `hud_current`, each a `Vec<HudElem>` of 28 raw field values in the wire order above, and re-encodes it with the same gate, count and last-changed rules. `crates/server/src/game/wire.rs`'s `hud_elems` is the filter `HudElem_UpdateClient` runs, over `crate::game::entity::HudState` plus the element's script fields. The `fadeOverTime`, `scaleOverTime` and `moveOverTime` methods (`crates/server/src/game/builtins/hud.rs`) fill entries 20..25 and 28..33.

##### A decoded frame

Message 4 of `crates/common/tests/fixtures/net/snapshots.bin` (uncompressed, `mp_carentan`, a lone spectator who never joined a team) carries 42 primitives, the same 42 as every uncompressed frame in both committed captures. VERIFIED. In order:

1. Block 1 gate `1`, mask `0b101000`: only `stats[3]` and `stats[5]` are present, `stats[3] = 63` (the 6-bit "no teammate") and `stats[5] = 1` (first spawn). The four absent elements are zero on a spectator who never had health. VERIFIED.
2. Block 2's group gate `0`, then block 3's four sub-gates `0 0 0 0`: no ammo, no clip. VERIFIED.
3. Block 4's gate `0`: no objectives on this map and gametype. VERIFIED.
4. Block 5's gate `1`. The array at `ps+0x1338` has count 1, and its element 0 announces `j = 8`, so entries 6..14 follow: `color.rgba = 0xFFFFFFFF`, `type = 4`, `fontScale = 1.0` (integral float, biased 13-bit `4097`), `y = 460`, `x = 320`, `alignY = 1`, `alignX = 1`, `time = 1800000`, `font = 1`. The array at `ps+0x5A8` has count 0. VERIFIED.

That is all 42 primitives, which a server must reproduce bit for bit to keep a retail client's parse aligned.

##### How `ammo[]` and `ammoclip[]` are indexed

Neither array is indexed by weapon. Both are indexed by a per-weapon-def integer, in two separate namespaces. `BG_WeaponAmmo(ps, weapon)` (`game.mp.i386.so` `0x3ab9c`) returns `ps->ammoclip[weapDef->clipIndex] + ps->ammo[weapDef->ammoIndex]`, reading `weapDef+0x1A8` against base `0x20C` and `weapDef+0x1A0` against base `0x10C`. VERIFIED. `PM_WeaponAmmoAvailable` (`0x3abe8`), `PM_WeaponClipEmpty` (`0x3aee0`) and `PM_WeaponUseAmmo` (`0x3aeac`) all use `weapDef+0x1A8` against `0x20C`. VERIFIED. INFERRED: firing spends the clip, not the reserve, so `ammoclip[]` is the loaded magazine and `ammo[]` the reserve behind it.

The indexes are assigned once at weapon registration, by name, deduplicated:

- `BG_SetupAmmoIndexes` (`0x3600c`) indexes the weapon-def pointer array at `0x7c91c`, which `BG_SetupWeaponInfo` fills with a `0x100`-byte (64-pointer) allocation at `0x366b1`, against the bound at `0x7c918`. It passes the def's ammo name (`weapDef+0x19C`) to `Q_strlwr`, `Q_stricmp`s it against a name table at `0xa96e0`, has a store that appends to that table and bumps the count at `0xa98e0` (`0x3616c`, `0x36188`), and stores a slot number into `weapDef+0x1A0` (`0x36098`, `0x36181`). VERIFIED. INFERRED, from the loop and the branch: the walk runs weapon indices from 1 up, and the append is on the miss path, so each distinct name takes one slot.
- `BG_SetupClipIndexes` (`0x36370`) does the same with the clip name (`weapDef+0x1A4`), the name table at `0xa9b20`, the count at `0xa9d20` and the result in `weapDef+0x1A8`. VERIFIED.
- `BG_SetupSharedAmmoIndexes` (`0x361ac`) does the same for the shared-ammo-cap name (`weapDef+0x1B4`, table `0xa9900`, count `0xa9b00`, result `weapDef+0x1B8`, default `-1`). VERIFIED. That index addresses no playerstate array.

The first index handed out is 1; slot 0 of both arrays goes unused. VERIFIED, off the spawn line of `crates/server/tests/fixtures/playerstate/mp_carentan-tdm-hit-target.txt`, a retail capture of a stock allies join: `clip=3:7,6:3,10:15 ammo=3:56,10:400` is `colt_mp`, `fraggrenade_mp` and `m1carbine_mp` one slot above where a 0-based walk over configstring 7 puts them. `crates/server/src/weapons.rs` reads that line back in `the_first_index_handed_out_is_one`.

Each pass also stores a parallel integer beside the name (max ammo at `weapDef+0x1AC`, clip size at `weapDef+0x1B0`) and carries a `Com_Error` call with format strings `Max ammo mismatch for "%s" ammo: ...` (`0x720e0`) and `Clip Size mismatch for "%s" clip: ...` (`0x721c0`). VERIFIED. INFERRED, from the compare guarding it: the call fires when two weapon files give one name different values.

`BG_SetupWeaponInfo` (`0x36674`) runs the three passes back to back (`0x369b2`, `0x369b7`, `0x369bc`) after it has read the weapon-file list, sorted it with `compare_weaponfile_names` and published it as configstring 7. VERIFIED. INFERRED, from the call order: a name's index is decided by the first weapon in configstring-7 order that names it, the same order `entityState.weapon` indexes 1-based.

Both arrays are 64 long, capping the distinct ammo names and the distinct clip names at 64 each, independently. VERIFIED.

### Trajectories (`trType`/`pos`/`apos`)

Entity movement and rotation are a Q3-shaped `trajectory_t` (`trType`, `trTime`, `trDuration`, `trBase`, `trDelta`) in the `pos` and `apos` netfield groups, evaluated with RTCW's `BG_EvaluateTrajectory` maths. **Divergence, #8.** The arithmetic is RTCW's; the enumeration is not. `BG_EvaluateTrajectory` (`game.mp.i386.so` 0x2c600) and `BG_EvaluateTrajectoryDelta` (0x2c8e8) both open `cmp $0xa,<trType>; ja Com_Error` over eleven-entry jump tables at 0x700a8 and 0x7012c, with the error string `BG_EvaluateTrajectory: unknown trType: %i` at 0x70060. VERIFIED. The valid range is 0..10 with no holes:

| idx | name | evaluates |
|---|---|---|
| 0 | `TR_STATIONARY` | `trBase` |
| 1 | `TR_INTERPOLATE` | `trBase` |
| 2 | `TR_LINEAR` | `base + delta*dt` |
| 3 | `TR_LINEAR_STOP` | linear, `dt` clamped to `trDuration` and to >= 0 |
| 4 | `TR_SINE` | `base + delta * sin(2*pi*(t-trTime)/trDuration)` |
| 5 | `TR_GRAVITY` | x,y linear; `z = base.z + delta.z*dt - 400*dt*dt` |
| 6 | `TR_GRAVITY_LOW` | the same with 120 |
| 7 | `TR_GRAVITY_FLOAT` | `z -= 80*dt`, a single `dt` |
| 8 | `TR_GRAVITY_PAUSED` | `trBase`; the delta arm is `Com_Error` |
| 9 | `TR_ACCELERATE` | accelerate along the normalised delta |
| 10 | `TR_DECCELERATE` | the same negated |

VERIFIED: the arithmetic per case and the `.rodata` constants (0x70098 = 400.0, 0x7009c = 120.0, 0x700a0 = 80.0, 0x70120 = 800.0, 0x70124 = 240.0, 0x70128 = 160.0), and the cgame's own `trType == 5` arm (`cgame_mp_x86.dll` 0x30005470, the 400.0 at 0x30069528). INFERRED: the names, from RTCW's enumeration minus `TR_LINEAR_STOP_BACK`, `TR_SPLINE` and `TR_LINEAR_PATH`. That makes index 4 the `fsin` branch instead of RTCW's `Com_Error` hole and leaves index 8 as the one value handled in evaluate and fatal in delta.

Trap: codextended's `shared.h:461-476` has `TR_SINE = 5`, but it is a paste of the RTCW-MP header with no CoD1 verification. Codextended never uses it numerically; it resolves the game binary's `BG_EvaluateTrajectory` at runtime and writes raw literals (`src/script.c:1177`, `bolt->s.pos.trType = 5;`). "`TR_SINE` with `trDuration == 0`" samples are `TR_GRAVITY` read one index high: index 5 never touches `trDuration`, and 0 is what a launched item carries.

## Client to server message body (clc)

clc opcodes are 2 bits. After the 9-byte plain header:

```
clc_clientCommand: 2-bit op, long sequence, string   (reliable commands: chat, "disconnect", team, etc.)
clc_move / clc_moveNoDelta: 2-bit op, byte count, <delta usercmds>
clc_EOF
```

`usercmd_t`, checked against the game's struct:

```
i32 serverTime
u8  buttons        ; attack, use, ads, etc.
u8  wbuttons       ; lean left/right, reload
u8  weapon
u8  flags
i32 angles[3]      ; ANGLE2SHORT-style: (deg * 65536/360) & 0xffff
i8  forwardmove
i8  rightmove
i8  upmove
u8  (padding)
```

Delta-encoded (`MSG_WriteDeltaUsercmdKey`) against a base built from the playerstate, not the previous usercmd ("The base cmd is built from the playerstate" below). The move message is keyed: `key = checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommands[reliableAcknowledge & 63], 32)`. `Com_HashKey` is at `0x806810c`. A compact-move path encodes `forwardmove = 127` and friends as flag bits; its mask table (`1,3,7,15,31,63,127`) is in `.data` around `0x80de304`.

The reader is `MSG_ReadDeltaUsercmdKey` (`0x807b7f8`), with two branches. Both start with `to = *from` (anything not sent keeps the base value), a serverTime preamble, a keyed "changed" bit that returns the base cmd unchanged when it equals `key & 1`, and a keyed branch bit: `key & 1` picks the compact branch, otherwise the full one. The mask table at `0x80de300` is `mask[n] = (1 << n) - 1` (`0x80de304` = 1, `0x80de308` = 3, `0x80de310` = 0xf, `0x80de318` = 0x3f). A keyed `n`-bit field is `read_bits(n) ^ (key & mask[n])`; a keyed angle is a raw short off the byte cursor XORed with the whole key, truncated to 16 bits; a keyed byte XORs with `key & 0xff`. `MSG_ReadBits`, `MSG_ReadByte`, `MSG_ReadShort` and `MSG_ReadLong` are all inlined, so both cursors show in the disassembly: loose bits off `msg->bit` (offset `0x14`), whole bytes off `msg->readcount` (`0x10`).

Preamble, both branches:

| VA | Wire | Effect |
|---|---|---|
| `0x807b80a` | none | `to = *from` (`rep movsl` x6); every field not transmitted keeps the base value |
| `0x807b812` | 1 bit | set: a byte at the byte cursor, `to.serverTime = from.serverTime + byte`; clear: 32-bit absolute (`0x807b866`) |
| `0x807b88a` | 1 bit | `== key & 1` returns at `0x807c0d9`, the cmd is the base cmd unchanged |
| `0x807b8e6` | none | `to.buttons &= 0xfe` |
| `0x807b900` | 1 bit | `== key & 1` selects the compact branch, otherwise the full branch at `0x807bba0` |

Compact branch (`0x807b956`). `key ^= to.serverTime` first, then:

| VA | Field | Wire | Keyed with |
|---|---|---|---|
| `0x807b967` | `buttons` bit 0 | 1 bit | `key & 1` |
| `0x807b9bd` | `angles[0]` | change bit, then i16 | `key` |
| `0x807ba1e` | `angles[1]` | change bit, then i16 | `key` |
| `0x807bac0` | `forward`/`right` | change bit, then 4 bits | `key & 0xf` |

It returns at `0x807bb81`. `angles[2]`, `up`, `wbuttons`, `weapon` and `flags` cannot be expressed here, so they come from the base. vcod's writer never uses this branch (below).

Full branch (`0x807bba0`). The serverTime is mixed into the key only after forward/right (`0x807bde1`), so the first four fields use the raw key and the last five the mixed one:

| VA | Field | Wire | Keyed with | Value when the change bit is clear |
|---|---|---|---|---|
| `0x807bba0` | `buttons` bit 0 | 1 bit, no change bit | `key & 1` | none |
| `0x807bc06` | `angles[0]` | change bit, then i16 | `key` | `from.angles[0]` |
| `0x807bc6b` | `angles[1]` | change bit, then i16 | `key` | `from.angles[1]` |
| `0x807bd08` | `forward`/`right` | change bit, then 4 bits | `key & 0xf` | bucket of `from.forward`/`from.right` |
| `0x807bde4` | `angles[2]` | change bit, then i16 | `key` (mixed) | `from.angles[2]` |
| `0x807be5e` | `buttons` bits 1-6 | change bit, then 6 bits | `key & 0x3f` | `from.buttons >> 1` |
| `0x807bef9` | `wbuttons` | change bit, then a byte | `key & 0xff` | `from.wbuttons` |
| `0x807bf80` | `up` | change bit, then 2 bits | `key & 3` | bucket of `from.up` |
| `0x807c041` | `weapon` | change bit, then 6 bits | `key & 0x3f` | `from.weapon` |

- `flags` (offset 7) is never sent by either branch.
- `to.buttons` is rebuilt, not patched: bit 0 comes from the first field, and `0x807be45` clears bits 1-7 again before the 6-bit field is ORed back in shifted left by one (`add %al,%al`).
- The movement axes are never analog on the wire. Each is a 2-bit code with a +/-10 deadzone (`> 10` sets bit 0, `< -10` sets bit 1) that decodes to 127, -127 or 0 (`0x807bb52` for forward/right, `0x807c013` for up). The same derivation runs on the base cmd for the default, so a base `forward = 100` reads back as 127. Forward and right share one nibble (forward in bits 0/1, right in bits 2/3).

The angle change bits are used. VERIFIED live 2026-08-26: a retail 1.1 client sent roughly 624 moves in a 3-minute session with a cleared change bit on pitch or yaw, keeping the base's angle. Each one flashes the view to zero for a frame unless the server's base carries the angle too. Retail builds it from the playerstate's view angles less `delta_angles`; vcod's server decodes against its stored last received cmd for that client, which holds the same angle. The chain restarts with each gamestate, because the client wipes its state, cmd ring included, as it parses one (`CL_ParseGamestate` calls `CL_ClearState`, Quake III Arena `code/client/cl_parse.c` and `code/client/cl_main.c`). vcod's server clears its stored base when it sends a gamestate for that reason (`send_gamestate`, `crates/server/src/server.rs`).

Widths and order VERIFIED live 2026-08-25: a retail 1.1 client's moves parsed on vcod's server for a whole session with no truncation error (`docs/research/cod11-server-handshake.md`, "Retail client check"). The values feed vcod's pmove, and the A/B gates replay retail captures' cmds through it (`moves_ab`, `predict_ab`).

### The base cmd is built from the playerstate

The `from` cmd the first usercmd of a message decodes against is not the last cmd the server received. `SV_UserMove` (`0x8086fa4`) builds it fresh for every message: it fetches the client's game-side playerstate (`0x8089270`, the client number in), hands it with a 24-byte stack cmd to `0x807f5c0` (`0x80870a8`, that function's only caller), and passes the cmd as the base to the first `MSG_ReadDeltaUsercmdKey` (`0x80870e3`). Each later cmd of the message decodes against the one before it (`0x808710d`). VERIFIED, the calls and the argument flow. What the builder writes, VERIFIED store by store, INFERRED for the conditions (branch reads):

| cmd field | value | from |
|---|---|---|
| everything | 0 | `bzero` of the 24 bytes (`0x807f5d5`) |
| `weapon` (+6) | `ps.weapon` | `ps+0xb0` (`0x807f5e7`) |
| `angles[0]`, `angles[1]` | `ANGLE2SHORT(ps.viewangles[i]) - ps.delta_angles[i]`, 16-bit | `ps+0xc0`, `ps+0x48` (`0x807f5f0`-`0x807f61d`), 65536/360 at `0x80d2e60` |
| `wbuttons` `0x40` (prone) | when `ps.eFlags & 0x40` | `ps+0x80` (`0x807f635`), only under `pm_flags & 0x40000` (`0x807f629`) |
| `wbuttons` `0x80` (crouch) | otherwise when `ps.eFlags & 0x20` | `0x807f640` |
| `up` (+0x16) | -127 | with either stance bit (`0x807f648`) |
| `wbuttons` `0x20` / `0x10` (lean right / left) | `ps.leanf > 0` / `< 0` | `ps+0x40` (`0x807f64e`-`0x807f670`) |
| `buttons` `0x10` (ads) | when `ps.fWeaponPosFrac != 0` | `ps+0xb8` (`0x807f676`-`0x807f688`) |

`angles[2]`, `forward`, `right`, the fire, melee and use bits and the reload bit are never in the base, so a compact cmd reads them as 0 and the base's stance, lean, sight and weapon as whatever the playerstate says. The client builds the same base from its predicted state, so the two agree without either storing the last cmd, and the change bits compare against what the server already knows.

What that costs a client that chains from its own last sent cmd, measured 2026-09-05 with `--net-probe --save-ads` against retail: the sight release went out once as a full-branch cmd and lowered the fraction for one step (`1.0` to `0.96`). Every compact cmd after it, carrying no upper button bits, took the base's `0x10` off the still non-zero fraction and raised the sight again. With the full branch forced on every cmd the fraction ramped to 0 over the weapon's `adsTransOutTime` exactly. VERIFIED, both runs. vcod's writer therefore always sends the full branch with the absolute serverTime (`write_delta_usercmd`, `crates/common/src/net/msg.rs`).

vcod's server still decodes against the last cmd it received (`parse_move`, `crates/server/src/server.rs`). That matches retail's base whenever the playerstate says what the last cmd asked for, which is every case measured: a retail client that holds or releases the sight, changes stance or leans sends the full branch until its predicted state catches up, and by then the stored cmd and the state agree. It would not agree after a state the server changed on its own, such as a `delta_angles` rewrite outside a spawn (`setPlayerAngles`) or a stance the script forced.

The same loop validates the weapon byte: after each decode it calls game export 14 with `cmd.weapon` (`0x80870f8`) and, when that returns 0, overwrites the byte with `ps.weapon` (`0x8087104`-`0x808710a`). VERIFIED, the call and the store; what export 14 tests is not read.

### Usercmd input bits

Which bit means which key. Measured 2026-09-01 by logging `buttons`, `wbuttons` and `upmove` off a retail 1.1 client connected to vcod's server while each key was pressed in turn, cross-checked against CoDExtended's bot input path (`src/gsc_bots.c`, one mask per verb) and its `KEY_MASK_*` defines (`src/shared.h`). They agree everywhere they overlap.

| Verb | Field | Value |
|---|---|---|
| move forward/back, right/left | `forwardmove`, `rightmove` | +/-127 |
| jump | `upmove` | +127 |
| crouch | `wbuttons` | `0x80` |
| prone | `wbuttons` | `0x40` |
| lean left | `wbuttons` | `0x10` |
| lean right | `wbuttons` | `0x20` |
| reload | `wbuttons` | `0x08` |
| stance from a held `+prone` / `+movedown` | `wbuttons` | `0x02`, beside the stance bit (CoDMP.exe `0x40ae90`; `cod11-mantle.md`, "The forced-stance events") |
| no key: held while the playerstate's shellshock runs | `wbuttons` | `0x04` (CoDMP.exe `0x40b675`; `cod11-shellshock.md`, "Movement") |
| fire | `buttons` | `0x01` |
| ads | `buttons` | `0x10` |
| melee | `buttons` | `0x20` |
| use | `buttons` | `0x40` |

Three things a Q3 reading gets wrong:

- **`upmove` is not a jump axis.** A crouched or prone client holds `upmove` at -127 for as long as it stays down, so only a positive `upmove` is a jump. Q3 reads -127 as crouch; CoD has a `wbuttons` bit for that and sends both.
- **The stance bits are level states, not key edges,** and mutually exclusive. The client owns the toggle (crouch is a tap in the retail binds) and the wire carries the resulting stance, held. One capture holds `0x80` for 18 seconds across a crouch, with `0x40` replacing it for the 6 seconds spent prone.
- **`buttons` `0x10` is ads, where Q3's `BUTTON_WALKING` is `0x10`.** There is no walk bit: CoD 1 has one move speed, nothing in a capture varies with it, and no `KEY_MASK` names one. `pm_walkSpeedScale` exists engine-side (the stage 4 playerstate capture pins it at 0.4), but no client input reaches it.

`buttons` `0x02` is Q3's `BUTTON_TALK`. `CL_FinishMove` (`CoDMP.exe` 0x40b65b-0x40b671) sets it whenever `cls.keyCatchers != 0` and `cl_bypassMouseInput` is 0, so it marks a client with the console, a menu or the chat line up. VERIFIED, the store and its two tests. Seen live twice at spawn (the team menu) and once well after. A server can read it as "not looking at the game"; while it is set, `+activate` never reaches `CL_ButtonBits`.

### Entering the world

A client is sent snapshots only once it is `CS_ACTIVE`, and that takes a `clc_move` the server accepts. Header-only keepalives keep the connection alive but do not promote.

**`begin` is not a client command.** The engine's client command table has nine entries, none of them `begin`: `userinfo`, `disconnect`, `cp`, `vdr`, `download`, `nextdl`, `stopdl`, `donedl`, `retransdl` (the `ucmds` table in CoDExtended's `src/sv_client.c`, whose entries are the retail server's own function addresses; Quake III Arena's in `code/server/sv_client.c` is the same nine minus `retransdl`). A retail client never sends one. A server that waits for it waits forever: the client loads the map to 100%, starts the ambient, and sits on the loading screen with no snapshot.

**`cp` carries no serverId.** Q3 1.32's `CL_SendPureChecksums` writes `cp <serverId> ...`; CoD 1.1's is `cp <cgame> <ui> @ <paks...> <key>`, sent after every map load whether or not the server is pure. A pure server drops a client that entered the world without one (`EXE_CANNOTVALIDATEPURECLIENT`) or whose list fails (`EXE_UNPURECLIENTDETECTED`). Format and check: `docs/research/cod11-server-handshake.md`, "Pak checksums and pure servers".

The trigger is **the first usercmd after the gamestate**: `SV_UserMove` promotes a `CS_PRIMED` client before the block's cmds run. CoD's `SV_ClientEnterWorld` is at `0x80877d8`, but none of the following was measured against a running server. It is a Q3/ioq3/RTCW-MP source read, each bullet naming its source, and vcod implements it on that basis:

- The cmd passed in is the *first* of the block, `SV_ClientEnterWorld( cl, &cmds[0] )`, and entry copies it into `cl->lastUsercmd` (Quake III Arena `code/server/sv_client.c`, `SV_UserMove` and `SV_ClientEnterWorld`; ioq3's is the same call with a null-cmd branch).
- The execute loop skips every cmd whose `serverTime <= cl->lastUsercmd.serverTime`, so the entering cmd itself is never simulated (same file, `SV_UserMove`'s per-cmd loop).
- The spawn's `delta_angles` are taken against the entering cmd's angles, not zero. `ClientSpawn` pulls the stored cmd into `pers.cmd` with `trap_GetUsercmd` on the line before `SetClientViewAngle`, which subtracts `pers.cmd.angles` from the spawn angle (RTCW-MP `src/game/g_client.c`). The trap returns `svs.clients[n].lastUsercmd` (RTCW-MP `src/server/sv_game.c`, `SV_GetUsercmd`), which entry just set to `cmds[0]`.

Entry also calls the game module: `SV_ClientEnterWorld` ends in `ClientBegin` (the `GAME("ClientBegin")` call in CoDExtended's `src/sv_client.c`). That is the notify `Callback_PlayerConnect`'s `waittill("begin")` blocks on, an engine-to-game call, not a command a client sends. The gsc side is section 0.1 of `docs/research/cod11-hud-protocol.md`.

The other promotion path is the restart (above): a `CS_PRIMED` client whose serverId differs only in the low nibble goes to `CS_ACTIVE` with no entering cmd. ioq3 passes a null cmd on its equivalent path and zeroes `lastUsercmd` (`code/server/sv_ccmds.c`, `SV_MapRestart_f`); vcod does the same.

### How long a cmd is simulated for

A cmd's `serverTime` is the moment the client's own prediction has reached, and the server integrates the whole span from `ps.commandTime` to it. A gap left by a hitching client is chopped into steps, not clamped away.

The engine does none of the timing. `SV_UserMove` (`0x8086fa4`) reads a cmd count, rejects a message whose count is below 1 or above `MAX_PACKET_USERCMDS`, and per cmd skips the ones at or before `cl->lastUsercmd.serverTime` and the ones past the block's last cmd. VERIFIED: `cmp $0x20` at `0x8087020` (`MAX_PACKET_USERCMDS` is 32, as in Q3), the `jle` at `0x8087286` against `cl+0x10624` (the stale test is `<=`), the `jg` at `0x808727b` against the block's last cmd, and the strings `"cmdCount < 1"` (`0x80d4aa3`) and `"cmdCount > MAX_PACKET_USERCMDS"` (`0x80d4ac0`). INFERRED, off the branch targets: a stale cmd costs only itself, while a bad count abandons the message. VERIFIED: no compare in `0x8086fa4`..`0x80872e9` reads a server clock, so the engine neither detects nor bounds a gap.

The rule is in the game module's `ClientThink_real` (`game.mp.i386.so` `0x3fee0`), one cmd at a time:

| step | what | where |
|---|---|---|
| clamp the cmd clock ahead | `serverTime = min(serverTime, level.time + 200)` | `0x3ff05`-`0x3ff13`, `add $0xc8` |
| clamp it behind | `serverTime = max(serverTime, level.time - 1000)` | `0x3ff15`-`0x3ff23`, `add $0xfffffc18` |
| the span | `msec = serverTime - ps.commandTime` | `0x3ff29` |
| drop a stale cmd | `msec < 1` returns | `0x3ff31`, `test`/`jg` |
| clamp the span | `msec = min(msec, 200)` | `0x3ff52`-`0x3ff5e`, `cmp $0xc8` |

VERIFIED: every immediate in the table, and that `level.time` is `level + 0x1e8` (the relocations at `0x3ff06` and `0x3ff16` name `level`).

The last row is a trap: it is not the dt clamp. That `msec` is a local. `ClientThink_real` copies the usercmd verbatim into `pm.cmd` (24 bytes, `0x40073`), and the clamped local goes only to the weapon-position and spread code (`0x40204`, `0x4023a`). INFERRED, off the dataflow from the clamp to its two readers.

`Pmove` (`0x3447c`) integrates the move, Q3's chop loop unchanged:

- `finalTime = pm->cmd.serverTime`; `finalTime < ps->commandTime` returns (`0x3448e`).
- `finalTime > ps->commandTime + 1000` snaps `ps->commandTime` to `finalTime - 1000` (`0x34492`, `0x3449b`): arrears past a second are dropped, which also caps the loop at 16 iterations.
- While `ps->commandTime != finalTime`: `msec = min(finalTime - ps->commandTime, 66)` (`0x344d3`, `0x344d8`; under `pmove_fixed`, `min(..., pmove_msec)`, `0x344c5`), `pm->cmd.serverTime = ps->commandTime + msec` (`0x344e4`), call `PmoveSingle` (`0x344e8`), and force `cmd.upmove` to 20 while `PMF_JUMP_HELD` is set (`0x344f8`).
- `PmoveSingle` clamps its own `pml.msec` to `[1, 200]` (`0x3404c`, `0x3405f`) and writes `ps->commandTime = pm->cmd.serverTime` (`0x34074`).

VERIFIED: every immediate above and the store at `0x34074`. So 66 is a chop granularity, not a discard: a 600 ms gap is ten `PmoveSingle` calls, and on exit `ps.commandTime` holds the cmd's own `serverTime`, which puts the client's replay boundary where the client put it.

`pmove_fixed` and `pmove_msec` exist in CoD 1.1 MP, registered in `gameCvarTable` with defaults `0` and `8`; `pmove_msec` is clamped to `[8, 33]` (`0x3ff68`-`0x3ff92`, `.rodata` strings `"8"`, `"pmove_msec"` and `"33"` at `0x72d1c`, `0x72d1e`, `0x72d29`). VERIFIED. Both default off, so a stock server runs the 66 ms chop.

**What vcod does.**

- `replay_moves` (`crates/server/src/server.rs`) runs the chop, the 1000 ms arrears bound and the `PMF_JUMP_HELD` `upmove` force of 20 (`pmove::cmd::chop` in `vcod-common`; `docs/research/cod11-mantle.md`, "Jumps"). It leaves `last_processed_st`, which goes out as `commandTime`, at the cmd's own clock.
- `crates/server/tests/hitching_client.rs` pins it: a spectator silent for 600 ms that then sends one cmd covers the whole gap, about 250 units (a single clamped step covered 16).
- `replay_moves` holds each cmd's clock to the two `level.time` clamps before the stale test and the chop. `level.time` is the last frame's, the one a packet between two frames runs on.
- `enter_world` seeds `commandTime` from the entering cmd's clock held to the same window, so a handshake cmd stamped far ahead cannot leave it past every later cmd.
- The clamps are what turn a server stall into a `commandTime` stuck 200 ms past the stalled frame (`docs/research/cod11-player-clip.md` 8.12).

The vcod client predicts on the same rule. Each frame it rebuilds the sim playerstate from the newest snapshot and replays every sent cmd with a `serverTime` past that snapshot's `commandTime` through the server's own per-cmd step (`pmove::cmd`), chop and arrears bound included (`crates/client/src/play/predict.rs`). A history that no longer reaches back to `commandTime` draws the interpolated snapshots instead of replaying from the wrong base. A correction a new snapshot brings is eased out over 100 ms:

- Retail's cgame registers `cg_errordecay` with default `"100"`: the cvar-table entry at `cgame_mp_x86.dll` `0x30074e04` points at the name string at `0x30064434` and the default at `0x30064990`. VERIFIED. Its `vmCvar_t` is at `0x301e1f00` (the dword before the table entry), so its value is `0x301e1f08` and its integer `0x301e1f0c`. VERIFIED.
- In `CG_PredictPlayerState` (cgame `0x300294f0`) a correction counts only when its length exceeds the float at `0x3006953c`, 0.1 (the compare at `0x300297ed`). VERIFIED, the constant and the compare.
- What is left of the old error is scaled by `(cg_errordecay - (cg.time - errorTime)) / cg_errordecay`, floored at 0, the new correction added on, and `errorTime` set to the dword at `0x3020714c` (the store at `0x300298ea`), the one after `cg.time`'s `0x30207148`, which is `cg.oldTime`. INFERRED, the scaling off control flow and the two names off Q3's `cg_t` order.
- The view (`0x300333b0`) adds the error times the same factor and drops it once the factor leaves (0, 1). INFERRED off control flow. So the ease starts a frame in, not at full strength.

vcod does the same with local ms in place of `cg.time`; the clock below keeps the two within a couple of ms. The equivalence gate is `crates/server/tests/predict_ab.rs`: it steps the server's `ClientSim` and the predictor side by side through the wire codec and compares the fields prediction draws from.

## Spectator

On a DM server the client auto-spawns as `GAME_SPECTATOR`; no team command is needed. Send usercmds (movement axes plus `ANGLE2SHORT` view angles) every frame. The server flies the spectator and the position comes back in the playerState. `forward = 127` moved the playerstate origin about 636 units over 2 seconds and stopped when the key stopped, the cheapest end-to-end proof the move path works.

**The flight collides**, as RTCW's does. `PmoveSingle`'s `pm_type` 4 arm (`game.mp.i386.so` 0x341c8) runs `PM_CheckDuck` (0x316f4) and `PM_FlyMove` (0x2ee6c), and `PM_FlyMove` ends in `PM_StepSlideMove(0)` (0x2f02d). VERIFIED.

- The hull is the spectator's own: `PM_CheckDuck`'s `pm_type` 4 arm stores (-8, -8, -8) and (8, 8, 16) into `pm.mins`/`pm.maxs` (rodata 0x70bd0..0x70bd8, stores at 0x31715..0x3173f). VERIFIED.
- `SpectatorThink` (0x3fab8) hands `Pmove` a capsule trace and `pm.tracemask` 0x800011 (0x3fbd8): no playerclip, no bodies. VERIFIED.
- A spectator is never ground traced and its `groundEntityNum` reads 0 on the wire, so `PM_StepSlideMove`'s entry gate (`!= 0x3ff` at 0x3507f) lets every move through. A blocked move steps up in the air, raises `EV_STEP_VIEW` and, past 3 units, pushes `bobCycle` on (0x357ad). INFERRED from those tests.
- Live 2026-10-12, `mp_carentan` dm and tdm, the probe flying +x from the intermission spot: on dm it stepped 8.125 onto a window sill at x 1016.8 with `EV_STEP_VIEW` parm 136 and `bobCycle` 12. On both it stopped at x 1019.875, 8.125 short of the caulk behind the shutters. VERIFIED (capture). vcod: `pmove::spectator_move`, pinned by `a_spectator_steps_onto_the_sill_retail_s_did`.
- Trap: a 2026-08-28 by-eye A/B with a retail client reported the spectator passing through wires, cars, walls and the ground. Wires and cars are static models, which `trap_Trace` does not test, and the 16-unit hull fits gaps a player cannot. The wall and ground reports did not reproduce: both 2026-10-12 server captures block.

**The -0 velocity.** Pressed into the caulk, retail's spectator reads velocity (+0, +0, -0) on every snapshot, bits `00000000 00000000 80000000`, through the 2 s push and after it. VERIFIED (capture, 2026-10-12, the same flight with each snapshot's velocity bits printed). INFERRED, from these functions and the retail bits:

- The -0 comes out of `PM_SlideMove` (0x347c0). Its plane list starts with the velocity's own direction, `VectorNormalize2` of (v, +0, +0).
- A head-on wall clips the velocity to a small backwards x, and the clip against that first plane sends it into the two-plane arm. That arm takes `CrossProduct` (0x3d86c) of the wall's normal and the velocity's direction, two opposed vectors, so the cross is zero.
- `VectorNormalize` (0x3d8d8) leaves a zero vector as it is (the `fucomp` against 0 at 0x3d909 skips the stores), and the crease scaled by its dot with the velocity keeps the cross's -0 in z.
- The z is -0 only because the wall's normal is (-1, +0, +0).

The brush clip (`cod_lnxded` 0x8054e90) builds an axial side's normal by zeroing three floats and storing -1 or 1 on the axis (0x8054fd1..0x8054fe9). VERIFIED. Two traps vcod hit: negating the axis gives (-1, -0, -0), and returning +0 for a zero normalize loses the -0. Either makes the velocity read (0, 0, 0), and the zombie message that writes this playerstate comes out 4 bytes short of retail's (handshake doc, "Zombie slots"). vcod follows retail (`x87::normalize_in_place`, `brush_side_planes`), and the sill test pins the bits. The -0 must also survive the idle frames after the push, which it does because an idle `PM_FlyMove` adds nothing (below).

**`PM_FlyMove`'s wish** (0x2ee8b..0x2f026), from the disassembly. VERIFIED (code) for each step:

- `PM_CmdScale` (0x2e5bc) of the cmd. A zero scale makes the wish (+0, +0, +0).
- Otherwise each component is `scale * forward[i] * forwardmove + scale * right[i] * rightmove`, with `forward` and `right` from `PmoveSingle`'s `AngleVectors` of `ps.viewangles` (0x34120), pitch included. `scale * upmove` is then added to z.
- z then loses 512 when `wbuttons & 0x20` (lean right, `cmd+5`, read as `pm+9`) is set and gains 256 when `wbuttons & 0x10` (lean left) is (0x2ef19..0x2ef4a). Both are raw units outside the scale, and they run on the zero-scale path too (its jump lands on 0x2ef19), so a lean key alone flies a spectator: right down at 512, left up at 256, both down at 256. The two terms stay on the x87 stack and round once, at the store.
- `VectorNormalize` gives the wish direction and speed. The accelerate returns when the speed minus the velocity's dot with the direction is at or under 0. Otherwise it adds `8 * frametime * max(wishspeed, 100)` (rodata 0x708e4 and 0x708e8), capped at that difference, along the direction. A non-`ENTITYNUM_NONE` ground entity multiplies the add by `1 / ps+0x37c`, which `ClientSpawn` sets to 1.

`PM_Friction` (0x2e460), which `PM_FlyMove` calls first, gives a spectator only its `pm_type` 4 arm (0x2e56d): the drop is speed times 5 (rodata 0x70874) times `frametime`, with no `pm_stopspeed` floor. The floored arm (100 at 0x70868, 5.5 at 0x70870) needs `pml.walking`, which a spectator never has. Under speed 1 it zeroes x and y and returns, leaving z. VERIFIED (code).

Live 2026-10-12, `--probe-edge lean-fly` on `mp_carentan` dm from the intermission spot, no other key: lean right read -512 on the wire and drove the spectator into the floor below, lean left climbed at a steady 256, both together ramped to -256. From 0.9565565 at rest, cmds at 22461, 22478, 22495 | 22512, 22529, 22546 | 22561, 22577, 22594 under both bits gave `velocity[2]` -100.164536, -172.55359 and -225.02026 (bits `c2c8543e`, `c32c8db8`, `c3610530`). VERIFIED (capture). That ramp fits a friction of speed times 5 bit for bit and misses with a stop-speed floor (-92.64 after the first snapshot). vcod: `pmove::spectator_move` takes `wbuttons` and runs both (`spectator_lean_bits_fly_down_512_and_up_256`); the same probe against vcod-server read the same per-snapshot velocities, within the phase of the cmd clock.

INFERRED from those steps: a spectator flies where it looks, half a key flies at half the 400, and with no key the velocity is untouched. Live 2026-10-12: retail's per-snapshot ramp from rest was 144.35, 222.58, 320.24, 392.36, 400. Stepping friction and the add over the snapshot's ~16 ms cmds reproduces the second step to 0.2 with an accelerate of 8, and misses by 13 with Q3's walk accelerate of 9. VERIFIED (capture). vcod: `pmove::spectator_move` takes the raw cmd axes and runs this wish, lean terms included (`spectator_flies_along_the_view_at_the_key_s_speed`).

A spectator who was a player reads `groundEntityNum` 0 too. `ClientSpawn` (0x4268c) zeroes the whole `gclient_t` (`__bzero` of 0x22c4 bytes at 0x42804) and copies back only the persistent halves, so the last ground trace does not survive. INFERRED from that call. Live 2026-10-12, `mp_carentan` dm: a probe that joined allies stood at 1022 (`ENTITYNUM_WORLD`) for 12 s, then answered the team menu with `spectator` (`--probe-say '12:mr {sid} 0 spectator'`) and read 0 on every snapshot after. VERIFIED (capture). vcod read the same in the same run: `ClientSim::to_wire` writes `groundEntityNum` for a player only. VERIFIED (capture).

### Client commands are flood-protected

`SV_ClientCommand` (`cod_lnxded` 0x8086e08) gates every client command on `sv_floodProtect` before the game sees it. VERIFIED, out of the function:

- Three exemptions, `strncmp` against `"team "` (length 5), `"score "` (6) and `"mr "` (3) (0x8086eb3-0x8086ee9, strings at 0x80d4a5b, 0x80d4a61 and 0x80d4a68, each with the trailing space).
- The window is `svs.time + 800`, written to `client+0x10b08` (0x8086f5f-0x8086f64).
- The drop compares `svs.time` against that field (0x8086f17-0x8086f23) behind three other tests: `com_cl_running->integer == 0`, `client->state > 3` and `sv_floodProtect->integer != 0` (0x8086efc-0x8086f15).
- A dropped command is logged as `client text ignored for %s: %s` (0x80d4a80).

INFERRED, from that control flow:

- A command matching none of the three prefixes both opens the window and is subject to it; an exempt one does neither.
- An active client's non-exempt command inside the window reaches `SV_ExecuteClientCommand` with `clientOK` false, which runs the engine's own `ucmds` entry if it is one and skips the game's `ClientCommand` otherwise.
- A bare `score` is *not* exempt: its NUL fails the compare against the space.

VERIFIED, `crates/server/tests/fixtures/netchan/mp_carentan-sd-roundrestart-target.txt` and two later pairs: the probe sends `score` every 2 s and `kill` every 45 s. Retail killed it on the first and third `kill` and ignored the second and fourth, each of which landed 300 to 700 ms after a `score`. After each accepted `kill` the next `score` went unanswered (the 4 s gaps between `b` lines at `ms=21571` and `ms=111970`), the window the `kill` had opened. vcod's `client_command` applies the same rule (`FLOOD_WINDOW_MS`), and `crates/server/tests/flood_protect.rs` measures it.

### View angles: `delta_angles` always, `viewangles` the sum below `pm_type` 5

Retail maintains `ps.viewangles` for a player and a spectator alike, as `SHORT2ANGLE(cmd.angles[i] + delta_angles[i])` per axis.

- VERIFIED, `game.mp.i386.so`: `PM_UpdateViewAngles` (0x32d7c) compares `pm_type` against 5 at 0x32d8e and returns on equality (0x32d91). Above 5 it takes a dead arm that writes no `ps+0xc0`. Below, a three-axis loop adds `cmd.angles` to `delta_angles` (`ps+0x48`) and stores the scaled sum at `ps+0xc0` (0x32e70), which is `viewangles` (netfield offset 192).
- VERIFIED: `PmoveSingle` calls it at 0x340fc, and no branch ahead of that call targets past it.
- INFERRED: every `pm_type` below 5, the spectator's 4 included, has `viewangles` rewritten from each cmd; the intermission camera (5) keeps whatever its spawn's `SetClientViewAngle` wrote; a dead player keeps what it died with.

Evidence on the wire:

- VERIFIED, both committed spectator captures: `viewangles[0]` and `viewangles[1]` are 0.0 on every frame, while `delta_angles[1]` holds 16384 (90 degrees, the spawn yaw). That is the sum: the probe builds its cmds with `NetClient::send_frame`, which subtracts `delta_angles` (`crates/common/src/net/mod.rs`), so a probe holding a view of 0 sends -16384. INFERRED: an earlier reading, that a spectator's `viewangles` is not maintained, assumed the probe sent raw zeros.
- VERIFIED, the retail follow runs (`docs/research/cod11-spectator-follow.md` 9): the frame a sight press stopped a follow read pitch 15, `StopFollowing`'s `SetClientViewAngle` (0x46be2) output, and the frames after it 0 once the probe had subtracted the new delta. An unwritten field would have read 0 on the stop frame too.
- VERIFIED, `mp_carentan-dm-mapchange.txt` and the follow doc's intermission run: every `pm_type` 5 frame reads `viewangles` 0, 90, the intermission point's own angles.

For a **spawned player** the sum is measured directly:

- VERIFIED, the `--save-ads` capture on `mp_carentan` under `tdm` with `kar98k_sniper_mp` (`crates/server/tests/fixtures/playerstate/mp_carentan-tdm-ads-sniper.txt`, recaptured 2026-10-18): `delta_angles[1]` reads 32768 and `viewangles[1]` 90.0, the spawn point's yaw, on every one of its 355 `!trace` lines. Those pin the wire cmd yaw at 49152: `ANGLE2SHORT(90)` is 16384, and `16384 - 32768` is 49152 modulo 65536. The 2026-10-17 take, from another spawn, read 24576 and 45.0 on all 356 lines, the same wire yaw.
- The capture's `!input yaw=0` is not the wire value: that column records the script's yaw on top of the held facing, and `NetClient::send_frame` subtracts `delta_angles` from every axis before the cmd goes out. INFERRED, from the probe's control flow: the 45 is what `hold_view_yaw` produces, reading the spawn's facing off `delta_angles` and holding it.
- VERIFIED, the first capture (in git at `c52e9606`): the same 24576 under `viewangles[1]` -45.0 on all 355 lines, a wire yaw of 32768, from a `hold_view_yaw` that held the view of the first snapshot it saw, the spectator's.
- VERIFIED, both motion captures: the field tracks the cmd rather than pinning the spawn angle. `mp_carentan-dm-motion.txt` reads 135.0 at every held pose and moves by exactly the turn steps' 60 degrees, to 75.0 at `turn_left` and -165.0 at `turn_right`; `mp_pavlov-dm-motion.txt` is the same shape off 45.0.
- VERIFIED, vcod reproduces it: `the_scoped_sight_and_view_match_retail_on_mp_carentan` (`crates/server/tests/playerstate_combat_ab.rs`) compares all three axes against that capture with no tolerance, sample by sample, replaying the capture's own cmd angles.

Two retail *player* captures read `viewangles` 0 and are not counterexamples. `mp_carentan-dm.txt` and `mp_pavlov-dm.txt` came from a probe built on vcod's client, which subtracts `delta_angles` when it builds each cmd, so the sum really is zero there. The `--save-ads` and `--save-motion` probes hold the spawn view instead (`hold_view_yaw`, `crates/client/src/probe.rs`), which is why their sum is the spawn yaw. The same subtraction is why `playerstate_ab.rs`'s `check_spawn_shape` expects zero on every axis.

The angles live in the `cmd.angles` / `delta_angles` pair: `delta_angles` is set once at spawn as `ANGLE2SHORT(spawn_angle[i]) - cmd.angles[i]` (RTCW `SetClientViewAngle`, `g_client.c:437`), the client subtracts it when building each usercmd, and the client owns its view between snapshots. `viewangles` is the sum the server hands back. A client that replays at least one usercmd recomputes it and never reads the wire value.

vcod's server implements the RTCW scheme in three parts:

- `ClientSim::spectator`/`become_player` set `delta_angles` at spawn from `ANGLE2SHORT(spawn_angle) - cmd.angles`;
- `ClientSim::step` reads `SHORT2ANGLE(cmd.angles + delta_angles)`, not the raw cmd angle;
- `to_wire` writes the sim's `viewangles` in every mode; only a player's or a spectator's step moves it.

Change the first two together: the client always subtracts `delta_angles` when it builds a cmd (`crates/common/src/net/mod.rs`), so a server that sets the delta but reads the raw cmd angle has the client compensating for an offset the server never applies, and the client walks at an angle to where it looks. The `viewangles` write is pinned by a pair of tests in `crates/server/src/spectate.rs`, one per path.

A retail spectator's playerstate in the committed spectator captures also carries `bobCycle` 12, `eventSequence` 1 and `events[0]` 143 (`EV_STEP_VIEW`) with `eventParms[0]` 136: the marks of one air step like the sill step above. `crates/common/examples/snapshot_timing.rs` prints them. vcod's spectator step raises the same event and cycle (`a_spectator_steps_onto_the_sill_retail_s_did`). All four read 0 in the retail *player* captures stage 4 took (`crates/server/tests/fixtures/playerstate/*.txt`).

### A spawned player's playerstate: what stage 4 settled

Four fields where a walking player's frame differs from a spectator's, all reproduced by `ClientSim::to_wire`. The first three were measured off `crates/server/tests/fixtures/playerstate/mp_pavlov-dm.txt` and its `mp_carentan` twin; `viewangles` reads 0 in those two for the reason above and was measured off the sight and motion captures:

- `pm_flags` 262144. Bit 18, the third member of the view-source group whose other two (0x10000 free spectator, 0x20000 following) `cod11-events-and-fx.md` lists. `ClientEndFrame` sets it for a client looking out of its own body, and both other arms clear it, which is why the spectator captures read 0.

  The other bits a moving player carries, and where each is written, are in `docs/research/cod11-combat.md`: 0x1 prone, 0x2 the crouch latch, 0x8 held jump, 0x10 the ladder, 0x40 the backpedal latch, and the ADS pair 0x20 and 0x80 (section 1.13). The ADS bits are the one place the wire word is not a reading of the usercmd: **the sight bit `buttons 0x10` reaches nothing directly.** `PM_UpdateAimDownSightFlag` gates it: a dead, airborne, raising, holstering or swinging player, and a weapon with no `aimDownSight`, all clear 0x20 whatever the button does, and `ps.fWeaponPosFrac` and the animscript's `ads` condition both read the gated flag. 0x80 rides on 0x20 but drops while prone and for the whole of a reload. Both retail motion captures read `pm_flags 0x400A0` at their one ADS pose and 0x40000 or 0x40002 everywhere else.
- `viewangles[0..2]`, `SHORT2ANGLE(cmd.angles[i] + delta_angles[i])` per axis, the same sum a spectator's carries. VERIFIED off `mp_carentan-tdm-ads-sniper.txt` and both `-motion.txt` captures (numbers in the section above).
- `serverCursorHintString` 255. The field is signed 8 bits and retail stores -1, a no-hint sentinel, from `G_CheckForCursorHints`.
- `viewmodelIndex` 52 on `mp_pavlov` and 82 on `mp_carentan`: the model configstring index of the hands viewmodel the nationality's character script asked for, 1-based on base 268. It is derived, not pinned: `setViewmodel` allocates through the same model indexer the configstring gate diffs, so the number falls out of the table.

Addresses and labels for these are in `docs/research/cod11-gsc-object-model.md`, section 20.

`legsAnim` 634 is animation index 122 with `ANIM_TOGGLEBIT` (512) set. vcod's server picks it through its animscript port, and `playerstate_ab.rs` compares the anim channels by index with the toggle masked off, its `GAPS` list now empty (`docs/research/player-model-anim-system.md`, "The restart toggle").

### The view-height lerp, and fields a settled capture cannot see

The eye eases between stances while the collision box snaps, and the client predicts the ease itself. Four playerstate fields carry it: `viewHeightCurrent` (float, the eye now), `viewHeightTarget` (signed byte, the stance's height), `viewHeightLerpDown` (1 bit, set while the eye goes down) and **`viewHeightLerpTime`**, the `serverTime` the lerp started at, 0 once it settles. A lerp between standing and prone is two legs through the crouch height, each stamped when it starts, with `viewHeightLerpTarget` naming the leg's end (`docs/research/cod11-mantle.md`, "The eye through a stance change").

`viewHeightLerpTime` is the trap. VERIFIED live 2026-09-01 by tracing retail per snapshot through a crouch: it read 86666 while `serverTime` ran 86700, 86750, 86800 with the eye at 57.92, 52.64, 44.72, and dropped to 0 at 86850 with the eye settled at 40. Outside a transition it is always 0, so a capture of a settled pose, which is what both playerstate gates take, pins it at 0 and proves nothing. A server that always sends 0 tells the client the lerp began at time zero: the client's prediction snaps the eye to the target and the next snapshot drags it back, once per snapshot, for the whole transition. It looks like the camera shaking for about half a second on every stance change.

The general rule, which two vcod bugs came out of: **a field that only holds a value mid-transition is invisible to a settled capture.** `leanf` and this one are both of that kind. Trace per snapshot through the transition, not after it.

### `ps.commandTime` and client prediction

`ps.commandTime` must carry the `serverTime` of the last usercmd the server actually simulated for that client, and nothing else. A client replays every newer usercmd on top of the snapshot's playerstate, so a `commandTime` naming a moment the server never simulated silently drops that slice of the client's input from its prediction, every frame.

VERIFIED live 2026-08-28 against the retail 1.1 client, both directions. With `commandTime` clamped up to `serverTime - sv_fps_interval`, it sat 11-24 ms past the newest simulated cmd on 100% of 801 traced frames, and retail drew smooth movement with a view that juddered at snapshot rate: position was predicted while the view angles were reset from the stale snapshot 20 times a second. Reporting the true last-simulated time made `commandTime - last_simulated` exactly 0 on all 446 frames of the confirming trace, and the judder went away.

The lead (`serverTime - commandTime`) is a consequence, not a target: it is however far behind the client's own clock runs. The committed captures (vcod's probe against retail) show 0-34 ms (mean 16); a retail client against vcod's server shows 63-74 ms, because a retail client runs its `serverTime` estimate behind on purpose so it always has frames to interpolate. Clamping the lead to hide that difference caused the bug. `crates/common/examples/snapshot_timing.rs` prints the capture side; `vcod-server --trace` prints the live side.

### The client's clock

Retail draws everything, and stamps every usercmd, at one time: `cl.serverTime`.

- `CL_SetCGameTime` (CoDMP.exe `0x404d60`) sets it each frame to `cls.realtime + cl.serverTimeDelta - cl_timeNudge`, with the nudge clamped to +-30 and the result never below last frame's. VERIFIED off the decompile: realtime at `0x155f3e0`, the delta at `0x1434a70`, the time at `0x1434a64`, the floor at `0x1434a68`, `cl_timeNudge` at `0x1617308`.
- It is the time the cgame draws at (`vmMain(3, cl.serverTime, ...)` at `0x404bc0`) and the time `CL_FinishMove` (`0x40b690`) stamps a cmd with, capped at the newest snapshot's serverTime plus 5000. VERIFIED.
- A frame whose `realtime + delta` reaches the newest snapshot's serverTime minus 5 sets `cl.extrapolatedSnapshot` (`0x1434a74`). VERIFIED.
- When snapshots arrived since the last frame, `CL_AdjustTimeDelta` (`0x404bf0`) compares the delta with the newest snapshot's (`snap.serverTime - realtime`). More than 500 ms apart resets the delta to it and the time to the snapshot's (printing `<RESET> ` under `cl_showTimeDelta`). More than 100 averages the two (`<FAST> `). Anything closer moves it 2 ms back when the flag is set, clearing it, else 1 ms forward. VERIFIED, the thresholds 0x1f5 and 0x65 and the +1/-2. The slow path runs only at `com_timescale` 0 or 1. VERIFIED.
- `CL_FirstSnapshot` (`0x404d00`) seeds the delta from the first snapshot, and runs again when a snapshot's time goes below the last frame's (`0x1434a6c`) outside a demo. VERIFIED.

All of it is Q3's `cl_cgame.c` with the RESET and FAST constants unchanged.

The clock settles where a third of the snapshot intervals touch the newest snapshot minus 5: each one costs 2 ms and each clean one gains 1. Interpolation runs between the two newest snapshots, 25-30 ms behind the newest on average at 20 snapshots a second, and the few frames that run past the newest draw players where it put them. INFERRED from the rule; measured below.

vcod's client runs the same clock (`crates/client/src/play/clock.rs`) for entities, the HUD and the cmd clock. It counts a new snapshot by message number, as `cl.newSnapshots` is set for every snapshot parsed: a 1.5 match timeout sends snapshots whose serverTime stands still, and the clock stands with them (`docs/research/cod15-protocol-diff.md` section 12). INFERRED, Q3's `CL_ParseSnapshot`. `cl_timeNudge` is registered `"0"`, `CVAR_TEMP` (0x100), as `CL_Init` (`0x411e60`) has it. VERIFIED, the registration.

`CL_CreateNewCommands` (CoDMP.exe `0x40b8d0`) builds one cmd a frame once `cls.state` is past 4. It takes `frame_msec` as the frame's `com_frameTime` step clamped to 200 (used only to scale the frame's input), bumps `cl.cmdNumber` and stores `CL_CreateCmd`'s (`0x40b720`) cmd at `cmdNumber & 63`, stamped by `CL_FinishMove` as above. VERIFIED (decompile). So a hitch or a forward RESET reaches the server as one cmd whose time steps forward by the whole gap. vcod ticks cmds every 8 ms (`play::cmds::CmdClock`); a frame owing more than 200 ms builds one cmd at its newest tick instead of a burst. The 200 ms bound is vcod's own, borrowed from `frame_msec`'s clamp; retail has no tick to collapse.

Measured 2026-10-09 with `vcod --net-probe ADDR --probe-clock --probe-cmd-ms 8` (frame time 8 ms), which runs both clocks on the same arrivals. "Old vcod" is the clock vcod had before: 100 ms behind the newest snapshot, re-anchored on every arrival (an early one stepped the view forward, a late one froze it), cmds stamped off the newest snapshot plus elapsed time. "Behind" is newest snapshot time minus drawn time, min/mean/max ms; "past" the frames drawn at or past the newest snapshot; "held" the frames whose drawn time did not move; "jerk" the mean and largest |drawn step - local step| in ms.

| Link | Snapshot gaps | Clock | Behind | Past | Held | Jerk |
|---|---|---|---|---|---|---|
| retail 1.1d, loopback, 30 s | 50.0 +-4.6, max 74 | retail | -43/26.7/57 | 2.4% | 0% | 0.23/2 |
| | | old vcod | 37/79.0/100 | 0% | 0.9% | 0.57/26 |
| retail 1.1d, `VCOD_NETSIM=ping=100,jitter=20,loss=2`, 60 s | 51.0 +-12.1, max 141, 23 lost | retail | -86/27.9/70 | 5.0% | 0% | 0.23/2 |
| | | old vcod | -33/77.2/100 | 0.1% | 4.9% | 1.34/26 |
| vcod-server (debug build), same netsim, 60 s | 51.5 +-14.2, max 134, 35 lost | retail | -70/28.5/85 | 6.1% | 0% | 0.22/2 |
| | | old vcod | -26/76.2/100 | 0.2% | 4.7% | 1.38/40 |
| public TDM server over the internet, 40 s | 50.8 +-13.9, max 167, 13 lost | retail | -159/26.2/107 | 5.7% | 0% | 0.24/53 |
| | | old vcod | -59/76.4/100 | 0.2% | 3.9% | 1.07/122 |

VERIFIED, those runs. The public server's one 53 ms step is a FAST adjust right after the connect.

### The server's frame cadence

vcod-server runs its frames on a fixed grid (`Schedule` in `crates/server/src/main.rs`): slot `n` starts `n * 50` ms after the first, a tick runs every frame owed at its start, and the loop waits on the socket until the next slot. A late wake or a long tick moves only that snapshot; the next goes out on its own slot. A stall over 5 s restarts the grid. INFERRED from `SV_Frame`'s residual loop, map-cycle doc 4.7.

The +-14.2 netsim spread in the table above is the netsim's own 20 ms jitter, not a noisy server cadence: under the same conditions the two servers match. Measured 2026-10-20 with `vcod --net-probe ADDR --probe-clock` (release client), each server on mp_carentan alone on loopback, on a machine at load 45 of 30 cores. `--probe-cmd-ms` is the probe's poll period, which quantizes the arrival times it reports.

| Server | Probe | Run | Snapshot gaps, ms |
|---|---|---|---|
| retail 1.1d | `--probe-cmd-ms 8` | 40 s | 50.0 +-5.5, max 66 |
| vcod-server (release) | `--probe-cmd-ms 8` | 40 s | 50.0 +-4.1, max 68 |
| vcod-server (debug) | `--probe-cmd-ms 8` | 40 s | 50.0 +-5.5, max 69 |
| retail 1.1d | `--probe-cmd-ms 1` | 30 s | 50.0 +-2.4, max 56 |
| vcod-server (release) | `--probe-cmd-ms 1` | 30 s | 50.0 +-1.5, max 71 |
| retail 1.1d | `VCOD_NETSIM=ping=100,jitter=20,loss=2`, 8 ms | 60 s | 50.9 +-12.0, max 114, 20 lost |
| vcod-server (release) | same | 60 s | 50.9 +-11.6, max 139, 20 lost |

VERIFIED, those runs. The debug server's ticks took 3.5-6 ms (`--trace`) and its cadence still matched retail's. Two more 1 ms runs each, with the grid in `Schedule`: vcod +-1.1 and +-1.0, retail +-2.4 both times. One earlier vcod run read +-8.0 with a 217 ms gap and 5 snapshots "lost": at load 60 the process went unscheduled for a quarter second, and the next tick ran the five owed frames under one snapshot, as `SV_Frame` does. A gap of that kind is the host, not the loop.

### The client's send rate

`CL_ReadyToSendPacket` (CoDMP.exe `0x40b940`) sends a packet every frame when `0x464be0` passes the server address, and otherwise one per `1000 / cl_maxpackets` ms. `cl_maxpackets` is clamped to 15..100, default `"30"`, archived. VERIFIED, the clamp strings at `0x568564`/`0x568550` and the registration in `CL_Init` (`0x411e60`). `0x464be0` is `Sys_IsLANAddress`: it passes loopback, `127.0.0.1` and addresses matching the table at `0x8e3c78`, the host's interfaces. INFERRED off control flow and Q3's function.

Cmds are still built every frame. A packet carries every cmd since the one sent `cl_packetdup` packets before the last (`CL_WritePacket`, `0x40ba50`: `cl.cmdNumber - outPackets[(outgoingSequence - cl_packetdup - 1) & 31].p_cmdNumber`, at most 32 with the `MAX_PACKET_USERCMDS` print). `cl_packetdup` is clamped to 0..5, default `"1"`, archived. VERIFIED. So one lost packet loses nothing, and two in a row lose the first one's new cmds.

vcod's client does the same (`crates/client/src/play/cmds.rs`, `CmdRing`), with `Sys_IsLANAddress`'s class match against the host's interface addresses (`vcod_common::net::lan`), as its server does.

### Simulating a bad link

`VCOD_NETSIM="ping=100,jitter=20,loss=2"` in the client's environment delays every datagram it sends or receives by half the ping, plus or minus half the jitter, and drops `loss` percent in each direction (`crates/common/src/net/netsim.rs`). Datagrams keep their order. It applies to `vcod --connect` and `--net-probe` alike, and makes the server count as off the LAN for the send rate.

## Constants worth having

| Constant | Value |
|---|---|
| protocol number | 1 |
| `GENTITYNUM_BITS` | 10 |
| `MAX_GENTITIES` | 1024 |
| `ENTITYNUM_NONE` | 1023 |
| `ENTITYNUM_WORLD` | 1022 |
| `MAX_CONFIGSTRINGS` | 2048 |
| `MAX_CLIENTS` | 64 |
| `FRAGMENT_SIZE` | 1300 |
| `MAX_MSGLEN` | 32768 |
| entity netfields | 59 |
| playerState netfields | 103 |
| `entityState_t` size | 240 bytes |
| `MAX_RELIABLE_COMMANDS` (both reliable rings) | 64 (`& 63`) |

Configstring indices a client has to know by number:

- `3`: map ambient, `n\<alias>\t\<time>`, set by the gsc `ambientPlay`. Re-sent as `d 3` on every round restart, so it is not a connect-time-only string.
- `12`: global fog, `<near> <far> <density> <r> <g> <b> <fadeTime ms>`, written by the game dll's gsc builtins `setCullFog(near, far, r, g, b, fade)` and `setExpFog(density, r, g, b, fade)`. Every stock MP map sets one at load.
  - The density slot selects the mode: `>= 1` is linear farclip fog across near..far, below 1 is GL_EXP at that density (RTCW-MP tr_main.c R_SetFog). RTCW's client adds +0.1 to the same slot before the GL call, so the wire value stays un-offset. VERIFIED at "1" on two setCullFog captures and at `0 1 1e-05 0 0 0 0` for mp_chateau's `setExpFog(0.00001, 0, 0, 0, 0)`, whose near and far are the constants 0 and 1 (docs/research/cod11-gametypes-re-bel.md 2).
  - Linear fog leaves the sky visible and unfogged and clears to the fog colour. VERIFIED live on mp_ship against retail: clouds stay in view, where RTCW sets drawsky=false. Exp fog keeps the sky and fogs it.
  - The fade time drives a lerp between fogs of the same mode; a change of mode snaps.
- `524..779`: sound alias names, `CS_SOUNDS`, 256 slots with index 0 unused. `EV_SOUND_ALIAS`'s `eventParm` and `entityState.loopSound` both index it. The server fills it lazily at first use (`G_SoundAliasIndex`), so resolve a name at play time and follow `d` updates inside the block; a snapshot of it at connect misses most of it.

Player entities are the ones with `entity number < MAX_CLIENTS (64)`; the client slot equals the entity number. Do not key on `eType`: CoD's `ET_PLAYER` and `ET_GENERAL` are both 0.

## Divergences from RTCW/Q3, in one place

What bites a Q3/RTCW server port:

1. `connect` userinfo is Huffman-compressed from byte 12, not plaintext.
2. Message-block compression has no 2-byte length prefix; decode until the input runs out.
3. The whole message is built plain and block-compressed, not per-byte Huffman inside the bit writer.
4. `reliableAcknowledge` (server to client) is a plain u32 at bytes 0 to 3, outside the compressed stream.
5. The client-to-server header is 9 plain bytes with a 1-byte serverId, not a compressed 4-byte one.
6. Snapshots have no areamask.
7. PlayerState negative field widths are unsigned (width only, no sign extension), and the playerState delta has no zero flag in its main loop, so a changed `-0.0` float decodes as `+0.0` and only `lc` shows the field was sent.
8. `trType_t` is not RTCW's. CoD 1.1 has no `TR_LINEAR_STOP_BACK`, `TR_SPLINE` or `TR_LINEAR_PATH`, so every value from 4 up sits one index lower: `TR_SINE` 4, `TR_GRAVITY` 5, up to `TR_DECCELERATE` 10, and 11 or above is a `Com_Error` (`.so` 0x2c600). An item or a corpse arrives under 5, so a port that keeps RTCW's numbering reads gravity as a sine, divides by the `trDuration` 0 those entities carry, and draws every one frozen where it was launched.
9. Server commands are single letters: the configstring update is `d <index> <text>` with unquoted text to end of line, not `cs <index> "<text>"`, and `map_restart` arrives as `n`.
10. `sv_serverid` is a byte (`0x10` per map load from a zero start, low nibble per `map_restart`), and a stale low nibble makes the server drop the whole client message, snapshots included, where Q3's "ignoring pre map_restart" path keeps serving them.
11. The client-to-server scramble covers the compressed op block from its byte 0, packet byte 15, with no `CL_ENCODE_START` offset into it. The 9 plain header bytes in front are never scrambled, and the parity `c << (i & 1)` counts from the block's own start.
12. The per-client drop notice is the reliable command `w "<reason>"` (`SV_DropClient` `0x8085cf4`), not Q3's `disconnect "<reason>"`. Bare `disconnect` only travels client to server.
13. `MAX_RELIABLE_COMMANDS` is 64, not RTCW's 256. CoDExtended's `shared.h:135` and the `& 63` masks in `SV_UserMove` (`0x8086fa4`, the masks at `0x8086fe7` and `0x8087060`) agree. Both reliable rings, and the scramble key that indexes them, are sized off it.
14. `MSG_ReadString` and `MSG_ReadBigString` both map `%` to `.`, 0x92 to `'`, and every other byte over 127 to `.` (CoDMP.exe `0x444e00`/`0x444e60`; Q3/RTCW keep high bytes in the big reader). This is wire compatibility, not cosmetics. The usercmd key is `checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommands[reliableAcknowledge & 63], 32)`; `Com_HashKey` (`0x806810c`) multiplies raw sign-extended bytes with no substitution of its own; the server hashes the string it queued and the client the string it read. Only the read-time mapping keeps the two byte streams identical. A client that keeps high bytes (worse, re-encoded as two-byte UTF-8) computes a wrong key whenever the acked server command has one, such as a chat line with a high-byte name. The server then misreads the keyed framing bits of every move, so usercmds are dropped or garbled until an ASCII-clean command rotates into the slot. Live symptom: `ps.commandTime` frozen for 20+ s stretches, spectator movement applying seconds late or not at all, on busy servers only.
15. A spectator flies in its own hull, (-8, -8, -8) to (8, 8, 16), under `pm.tracemask` 0x800011, with no playerclip and no bodies. Its `groundEntityNum` reads 0, so `PM_StepSlideMove` steps it in the air. RTCW-MP's spectator flies in the player's box ("Spectator" above).
16. A server command waits in a queue for the client's next message. An unsent one is replaced by a later `d`/`v` for the same index or name, a later `a`/`b`/`o`/`p`/`q`/`r`/`t`, or the same text. Prints and other type-0 commands are dropped for a client not yet active or more than 31 behind ("The server command queue").
17. The usercmd delta base is built from the playerstate, not stored. Q3's `SV_UserMove` decodes the first cmd of a message against a zeroed cmd and its client writes against one. CoD 1.1's server builds the base from the client's playerstate (weapon, view angles less `delta_angles`, stance and lean bits, the sight bit off `fWeaponPosFrac`), and each later cmd of the message chains from the one before ("The base cmd is built from the playerstate").

Everything else holds: the Huffman table, the svc/clc opcodes, netchan fragmentation, the XOR scramble structure and the delta-encoding shape are Q3's, as the RTCW/Q3 GPL sources document them.

## What vcod's server does that retail does not

Anti-abuse checks, two timing differences, a LAN test and a default; the wire format is unchanged (`crates/server/src/server.rs`, `crates/server/src/main.rs`).

- Connectionless queries (`getinfo`, `getstatus`, `getchallenge`) are rate limited as ioquake3 does it: a per-source-address bucket (10 burst, one back per second, 1024 addresses tracked) and a global reply bucket (10 per 100 ms). Excess requests are dropped silently. Retail answers every one, which makes `getstatus` a reflection amplifier.
- A netchan message is parsed in full (header checks, `serverId`, every command and every usercmd) before anything about the client is updated. Retail commits the sequence number, the address and the timeout stamp first, so one spoofed packet with a client's IP and qport and a huge sequence number stalls that client until it times out.
- A `connect` that matches a live client's address and qport may replace it only when it carries that client's challenge, or when the slot has been silent for `sv_reconnectlimit` (3 s). Retail hands the slot over on the address match alone.
- A packet that arrives between ticks is handled on arrival, and its replies (a `connectResponse`, a gamestate, an rcon print) go out at once, as retail's event loop does. One that arrives while a tick runs is handled once the tick ends, or at the next tick's start if the tick overran its slot, on the moment retail's 5 ms socket poll would have handled it: rcon's 500 ms window reads its arrival (or the end of an earlier packet's handling that ran past it), and a move message's ack takes the time of the newest frame that had gone out when it arrived (`docs/research/cod11-server-handshake.md`, "rcon" and "Pings"). Its replies leave as soon as it is handled, before the next tick (`handle_queued` in `crates/server/src/main.rs`), as `NET_SendPacket` writes the socket inside retail's handler.
- Clients' cmds run at the tick, not as they arrive. Retail's `SV_UserMove` calls the game's `ClientThink` per cmd as each client's message is parsed (`0x80872cb`; the call is VERIFIED, that it is `ClientThink` is INFERRED), between game frames, on the previous frame's `level.time`. `replay_moves` runs every packet queued since the last tick in the order the server executed it, a packet's `kill` ahead of its cmds and each cmd's shots inside it, so the order is retail's; only the clock differs (`docs/research/cod11-combat.md`, section 16).
- `Sys_IsLANAddress`, which decides whether a client's `rate` reads 99999 or its clamped userinfo value, matches against the host's interface addresses where retail's server matches against what its hostname resolves to (`docs/research/cod11-server-handshake.md`, "Rate").
- `dedicated` defaults to 1 (no master heartbeat), where retail's default of 2 heartbeats, so dev runs stay off the public list. `--set dedicated=2` opts in.
