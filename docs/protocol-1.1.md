# Call of Duty 1.1 network protocol

This is everything I worked out about the CoD 1.1 wire protocol while building the spectate client. I wrote it for the next person implementing a 1.1 server (or client) from scratch. It's id Tech 3 under the hood, so if you know the Quake 3 / RTCW netcode most of this is "Q3, except…", and the exceptions are where all the pain is. I've flagged every place CoD 1.1 departs from the RTCW GPL source, because those are exactly the spots where a straight Q3 port breaks.

Everything here was verified two ways: against the RTCW/Q3 GPL sources as the structural reference, and byte-exact against a real `cod_lnxded` 1.1d server (protocol number **1**, `shortversion 1.1`) with live packet captures. Binary offsets below are into the stripped `cod_lnxded` 1.1d ELF. They're breadcrumbs for re-deriving a field, not a stable ABI.

## Transport

UDP. Two packet classes, told apart by the first 4 bytes:

- **Connectionless (out-of-band):** first 4 bytes are `0xFF 0xFF 0xFF 0xFF`, followed by a plaintext command string. Used for the handshake and server queries.
- **Netchan (connected):** first 4 bytes are a little-endian sequence number (never `0xFFFFFFFF`). Everything after the handshake.

## Connectionless packets

Format: `FF FF FF FF`, then an ASCII command word, a space, then command-specific payload. Replies use the same framing.

Queries are plaintext both ways, no compression:

- `getinfo <arg>` returns `infoResponse\n\<key\value…>`, short server info.
- `getstatus <arg>` returns `statusResponse\n\<key\value…>\n<player lines>`, the full info string. This is where I read `protocol\1`, `mapname\mp_carentan`, `sv_maxclients`, and the rest. Confirm the protocol number here rather than trusting folklore.

Both key sets are stable, by different routes. `SVC_Info` (cod_lnxded `0x808c1ac`) builds its string itself rather than dumping cvars, in this order: `challenge` (the caller's argument, echoed verbatim; vcod's server sanitizes it first, taking the first token, stripping `\` and `"` and capping it at 32 bytes, `challenge_arg` in `crates/server/src/server.rs`), `protocol`, `hostname`, `mapname`, `clients`, `sv_maxclients`, `gametype`, `pure`, `sv_allowAnonymous`, `pswrd`. `minPing`, `maxPing` and `game` are appended only when `sv_minPing`/`sv_maxPing`/`fs_game` are set. `SVC_Status` (`0x808bd50`) instead sends the serverinfo cvar string, configstring 0 byte for byte, with `challenge` and `pswrd` appended, then a newline, then one `<score> <ping> "<name>"` line per connected client. Its keys are in ascending **case-insensitive** name order, the way `Cvar_InfoString` walks the cvar list: the capture reads `\sv_maxclients\8\sv_maxPing\0\sv_maxRate\0`, which case-sensitive ASCII order would have put the other way round.

Both replies, captured verbatim from a stock server on two maps, are in `docs/research/cod11-server-handshake.md`, along with `challengeResponse` (one integer, freely negative, no Q3 second field).

Handshake:

1. Client sends `getchallenge`.
2. Server sends `challengeResponse <challenge>`. The 1.1 binary carries both `challengeResponse %i` and `challengeResponse %i %i` format strings. Only the first integer is the challenge, so ignore anything after it.
3. Client sends `connect "<userinfo>"`. The userinfo is Huffman-compressed (see below), not plaintext. The command word `connect ` stays in the clear so the server can match on it before decompressing. Everything from the opening `"` onward, byte 12 of the packet, is compressed.
4. Server replies `connectResponse`, or `error\n<reason>` (for example `EXE_SERVER_IS_DIFFERENT_VER 1.1` if the userinfo is malformed or the protocol key is wrong).

The reject tokens, verbatim from the binary, are localized keys the client looks up: `EXE_SERVER_IS_DIFFERENT_VER` (followed by ` 1.1` on the wire), `EXE_BAD_CHALLENGE` and `EXE_SERVERISFULL` (no underscores) go out at connect time as OOB `error\n<TOKEN>`. `EXE_LOSTRELIABLECOMMANDS`, `EXE_DISCONNECTED` and `EXE_TIMEDOUT` are the drop reasons; a connected client gets those in the `w` server command (see svc_serverCommand below). `connectResponse` carries no arguments. The gamestate is sent on the client's first netchan message, because the fresh slot has `gamestateMessageNum = -1` and its `serverId` (0) mismatches (`SV_ExecuteClientMessage`, cod_lnxded `0x80872ec`).

**Divergence from RTCW, #1.** RTCW sends the connect userinfo as plaintext. CoD 1.1 sends it Huffman-compressed from byte 12. A plaintext `connect` gets no `connectResponse`, because the server decompresses unconditionally and reads garbage. The userinfo I send:

```
\cg_predictItems\1\cl_anonymous\0\handicap\100\color\4\head\default\model\multi\snaps\20\rate\25000\name\vcod\protocol\1\qport\<qport>\challenge\<challenge>
```

`protocol`, `qport`, `challenge`, `name`, `rate`, and `snaps` are the keys that matter.

## Huffman coding

CoD 1.1 uses the exact Q3/RTCW `msg_hData[256]` frequency table, the one in `qcommon/msg.c`. I confirmed this by decoding real packets with a tree built from that table: 246 configstrings came out legible, with no extraction from the binary needed. The tree is the standard Q3 static Huffman. Build it once from the frequency table via the adaptive-insert (`Huff_addRef`) sequence, then freeze it. It never adapts during traffic.

`Huff_Compress` (`0x807f03c`) walks the source byte by byte through `offset_transmit` on the static tree. **Divergence from RTCW, #2.** RTCW's `Huff_Compress`/`Decompress` write a 2-byte big-endian length prefix. CoD 1.1's compressed *message* blocks have no length prefix, so the decoder runs until the input is exhausted. The connectionless `connect` payload compression does behave like the classic prefixed form; the per-message block compression below does not. Watch the distinction.

## The message-block model (the big one)

**Divergence from RTCW, #3, and the one that cost me the most time.** RTCW Huffman-codes each byte inside `MSG_WriteBits`, interleaving sub-byte raw bits with per-byte Huffman codes in a single stream. CoD 1.1 does not. The server builds the entire message as a plain byte buffer, then `Huff_Compress`es the whole thing as one block. `SV_Netchan_Transmit` (`0x808f680`) copies the first 4 bytes verbatim, then compresses everything from byte 4 onward.

Here's why it matters. A per-byte Huffman decode and a whole-block decompress produce identical bytes only for byte-aligned reads. They diverge at the first sub-byte field. In practice everything up to the first `svc_baseline` (configstrings, the leading opcodes) is byte-aligned, so a per-byte decoder looks correct right up until the first entity delta's 10-bit entity number, then silently desyncs. Decode the whole block up front, then read the plaintext with a two-cursor reader: a byte cursor `readcount` and a bit cursor, resyncing the bit cursor to the byte boundary whenever it lands on one. This mirrors the engine's own `msg_t`.

### Message headers

**Server to client.** 4 plain little-endian bytes are the `reliableAcknowledge`, then the Huffman-compressed block from byte 4. **Divergence from RTCW, #4.** RTCW Huffman-codes the reliableAck into the stream; CoD 1.1 leaves it plain. The proof: the first 4 payload bytes of a gamestate are `00 00 00 00`. Four Huffman `0x00` symbols are 2 bits each, so 8 bits total; they cannot occupy 32 bits, so the long can't be in the coded stream.

**Client to server.** A 9-byte plain header, then `scramble(Huff_Compress(ops))` from offset 0 of the compressed output. **Divergence from RTCW, #5.** The header is `serverId` as a single byte (`MSG_ReadByte`, not a long), plus `messageAcknowledge` (4-byte long), plus `reliableAcknowledge` (4-byte long), for 9 bytes. RTCW uses a 4-byte serverId and compresses the whole header with the ops. The server reads the 9 plain bytes (`SV_PacketEvent`, entry `0x808c9ca`, the read at `0x808c9d1`), then decodes and decompresses from byte 9. `SV_PacketEvent` drops the whole message when `messageAcknowledge < 0` or when `reliableSequence - reliableAcknowledge > 63`, the RTCW lost-reliable-commands check sized to the 64-slot ring. `sv_serverid` is itself kept as a byte on the server (see map_restart below), so this round-trips.

## Netchan framing

Standard Q3 netchan. The sequence number is the first 4 bytes, little-endian. `FRAGMENT_BIT = 1<<31` on the sequence marks a fragment. `FRAGMENT_SIZE = 1300`.

- Client to server packet: `[u32 sequence][u16 qport][payload]`. The `qport` is the client's chosen port id (I use `0x2000 | (pid & 0x0fff)`).
- Server to client packet: `[u32 sequence][payload]`, no qport.
- Fragmented: the sequence has `FRAGMENT_BIT` set, followed by `[u16 fragmentStart][u16 fragmentLength]`. A fragment whose length is not `FRAGMENT_SIZE` terminates the set; reassemble in order. When the message length is an exact multiple of `FRAGMENT_SIZE` the sender appends a zero-length fragment as the terminator (RTCW `net_chan.c:152`, and the retail server does the same). The gamestate is always fragmented. Mine came as 6 packets (5×1300 + 404) under one sequence. Drop stale and duplicate sequences; a fragment for a new sequence resets the reassembly buffer. The max reassembled message is 32768 (`MAX_MSGLEN`).

The sequence-number header bytes are plain; the payload is the compressed and scrambled block described above.

## XOR scramble

On top of the compression, connected-message payloads are XOR-scrambled. This is the Q3 "netchan encode/decode" (`CL_Netchan_Encode`/`Decode`; server side `SV_Netchan_Decode` `0x808de60`). CoD 1.1 keeps this layer intact. Port it byte for byte. The parity of the byte index matters (`c << (i & 1)`).

- Decode key (server to client, client decoding): `key = challenge ^ sequence` (low byte), keyed further on the client's own last reliable command, the one the message's `reliableAcknowledge` names (see below). The server side is RTCW's `SV_Netchan_Encode` (`sv_net_chan.c:43`) unchanged, walking `lastClientCommandString` with `challenge ^ outgoingSequence`. Decode starts at packet byte `4 + CL_DECODE_START`, which is byte 8, i.e. byte 0 of the compressed block, since the plain `reliableAcknowledge` occupies payload bytes 0 to 3.
- Encode key (client to server): `key = challenge ^ serverId ^ messageAcknowledge`, keyed on `serverCommands[reliableAcknowledge & 63]`. The scramble covers the **compressed op block from its byte 0**, which is packet byte 15 (4 sequence + 2 qport + 9 header). There is no `CL_ENCODE_START` offset into the block: the plain 9-byte header sits before it and is never scrambled. The server's `SV_Netchan_Decode` (`0x808de60`) walks the block with `i` starting at 0, so the parity that `c << (i & 1)` depends on is the parity within the block, not within the packet.
- The running key advances through the keyed command string, wrapping at its NUL, and substitutes any byte over 127 or `'%'` with `'.'`. The server-side `SV_Netchan_Decode` does a bare NUL-wrap with no `%` substitution, so match each side to its own reference.

Because the key depends on `serverCommands[seq & 63]`, you have to store every received `svc_serverCommand` string in a 64-entry ring indexed by its sequence. The encode/decode key reads that exact slot. Get this wrong and everything past the first server command decodes to garbage.

Server to client, the key string is the server's `lastClientCommandString`, the last reliable *client* command it executed, and the client walks its own copy of that command out of `reliableCommands[reliableAcknowledge & 63]`, the slot the message's plain acknowledge names. Both sides run the `'%'`/high-ascii substitution here, so a copy that came back through a `read_string` that already neutered those bytes and a raw copy produce the same key stream.

Every message a client sends carries at least a `clc_EOF`, so its compressed block is never empty and the scramble always applies, header-only keepalives included. VERIFIED live: vcod scrambles unconditionally and the retail 1.1d server accepts every message. Whether a retail *client* keeps RTCW's `cursize <= CL_ENCODE_START` early-out and can emit an unscrambled short message is untested. That is one of the things the retail-client check in `docs/research/cod11-server-handshake.md` would settle.

## Server to client message body (svc)

After decompression, the body is a sequence of ops, the same enum values as Q3:

```
svc_bad=0 svc_nop=1 svc_gamestate=2 svc_configstring=3 svc_baseline=4
svc_serverCommand=5 svc_download=6 svc_snapshot=7 svc_EOF=8
```

The first message is usually a leading `svc_serverCommand`, then the gamestate.

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

`checksumFeed` is `(rand() << 16) ^ rand() ^ millis`, drawn once per map load in `SV_SpawnServer` (cod_lnxded `0x808a3e0`); it seeds the usercmd key (see clc below). `MAX_CONFIGSTRINGS = 2048`. Configstring 0 is the serverinfo (`\mapname\…\g_gametype\…`), 1 is the systeminfo (`sv_serverid`, `sv_paks`, and so on). My capture had 246 configstrings and 19 baselines. Two base indices confirmed from `game.mp.i386.so` write sites: `CS_MODELS` is **268** (the `xmodel/…` path list runs 269 to 523, pinned off `G_ModelIndex`), and tag names for attachments are **108** (`attachTagIndex`, off `G_TagIndex`). Configstring **44** is not a per-client base at all. It is the locations list (`target_location_linkup`). CoD 1.1 has no per-client configstring; per-client visuals (body model, six attachment model/tag pairs, team, name) travel in the snapshot's clientState stream instead (see below, and `docs/research/clientstate-wire-format.md`).

`docs/research/cod11-server-handshake.md` has what the retail server puts in that table before any player exists: the whole configstring list for an empty `mp_pavlov` and `mp_carentan`, which entries are map-dependent, the baseline counts, and the fact that the stock server sends `svc_gamestate` with no leading `svc_serverCommand`. The `getinfo`/`getstatus`/`getchallenge` replies are there too.

Writing one is the reader run backwards, and it is worth pinning that way: vcod's writer re-encodes the retail capture into the identical byte string, ops and all (`crates/common/src/net/gamestate.rs`, test `writer_reproduces_the_captured_gamestate_byte_for_byte`). Three things that test forced out into the open. Every server message ends with a `svc_EOF` byte that `SV_Netchan_Transmit` appends after the last op, exactly as Q3's does: the capture carries it right after `checksumFeed`. The retail client reads it and stops, and without it reads the next byte as an op and drops with `CL_ParseServerMessage: Illegible server message` (VERIFIED live 2026-08-25: vcod's server omitted it, `CoDMP.exe` loaded the map and then dropped with that error; vcod's own client stops after the gamestate body and never noticed). After the `svc_EOF`, the `Huff_Compress` block ends on a partial byte, and decompressing it decodes the compressor's trailing pad bits into one more whole symbol, so the plain buffer runs one byte past the terminator; length has to come from what the reader consumed, never from the decompressed buffer. And a float field's zero flag does not round-trip naively (see Entity delta below).

Configstring **7** (`BG_SetupWeaponInfo`'s all-weapon-names list) is **space**-separated, confirmed against `crates/common/tests/fixtures/net/gamestate.bin`: `"bar_mp bar_slow_mp bren_mp colt_mp enfield_mp fg42_mp fg42_semi_mp fraggrenade_mp kar98k_mp kar98k_sniper_mp luger_mp …"`. Each name is a weapon file (`weapons/mp/<name>`), and `entityState.index` names it **1-based**: `index` N is `list[N-1]`.

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

RTCW writes an areamask (a length byte plus that many bytes) between `snapFlags` and the playerstate. CoD 1.1 goes straight into the playerstate delta. A phantom areamask read overruns every snapshot. This was the single change that made real frames decode.

Packet entities are repeated `[GENTITYNUM_BITS(10)-bit entity number][delta]`, terminated by `ENTITYNUM_NONE = 1023`. A remove is signalled by a bit after the number. An entity with no base in the delta-referenced snapshot deltas from its baseline, the one from the gamestate. `MAX_GENTITIES = 1024`, `ENTITYNUM_WORLD = 1022`.

Keep a ring of recent snapshots (I use 32) so any `deltaNum` reference resolves. If the base is missing or too old, the snapshot is invalid and gets dropped.

After the packet entities comes a third delta-compressed stream, `clientState_t`, CoD 1.1's replacement for Q3's per-client `CS_PLAYERS` configstring (`SV_WriteSnapshotToClient` `0x808e1fd`..`0x808e24f`):

```
<clients>: repeated [1 bit][6-bit clientIndex][clientState delta]
bit 0   client terminator
```

Each entry's delta body is the same generic per-field codec packet entities use (a removed bit, a delta-follows bit, a raw `lc` byte, then delta fields), just against the 22-field `clientState` table (team, body `modelindex`, six `attachModelIndex`/`attachTagIndex` pairs, and the 32-byte `name`) instead of the entity table. A removed delta means that client disconnected; a client the section doesn't mention at all carries forward unchanged from the base frame, exactly like an unmentioned packet entity. See `docs/research/clientstate-wire-format.md` for the full field table and struct layout.

### Which entities a client is sent

A client is not sent every entity: the list depends on where it stands.
Measured on 2026-09-01 with `tools/run_server.sh mp_carentan` (dm, stock
defaults) and `--net-probe --probe-pvs`, joining through the stock menus.

- A lone probe standing at its spawn was sent 6 of the map's entities, and 12
  after walking ~700 units; the six new ones were a second `mg42_bipod` and the
  `bookshelvewide`/`ammo_panzerfaust_box3` group around it at [810, 2274].
  VERIFIED live.
- That group crossed in and out of a second probe's list four times in 20 s as
  it walked back and forth over ~70 units near [1000, 700], its distance to the
  group holding at ~1600 units throughout, so the gate is a boundary in the map
  rather than a radius. VERIFIED live.
- Whether a client entity is culled the same way is UNVERIFIED: two probes
  ~2400 units apart were sent each other's entity (`eType` 1, `clientNum` 0 and
  1) continuously while both were connected, and the one removal seen was the
  other probe disconnecting.
- The 1.1d dedicated binary holds the strings `SV_BuildClientSnapshot: bad
  gEnt` and `CMod_LoadLeafs: cluster exceeded`, so it has Q3's snapshot builder
  and loads leaf clusters (VERIFIED). That a cluster PVS is what gates the list
  is INFERRED from those two names alone; nothing here rules out a cell/portal
  walk instead.

#### The box an entity links with

An entity's clusters come from the box it is linked with, never from its origin
alone: a turret's or a prop's origin usually sits inside the geometry it stands
on and reads as a solid leaf.

`SV_LinkEntity` (cod_lnxded 0x8090ad3) holds two blocks that write the abs box.
The first (0x8090b60-0x8090bb8) stores `origin[i] + r.mins[i]` into `absmin`
(+0x11c) and `origin[i] + r.maxs[i]` into `absmax` (+0x128), a component at a
time. VERIFIED. The second (0x8090bbe-0x8090c13) loads 1.0, stores each
`absmin` component back a unit lower and each `absmax` component back a unit
higher. VERIFIED. That the two compose, leaving a linked entity's box at
`origin + r.mins - 1` to `origin + r.maxs + 1`, is INFERRED: it rests on the
blocks' order and on nothing between them touching those fields. An entity
whose own box is a point therefore still links with `origin +/- 1`.

A third block (0x8090b0f-0x8090b5c) replaces the box with `RadiusFromBounds` of
it, cubed about the origin. That it applies only to a rotated bmodel is
INFERRED, from the `r.bmodel` test (+0xfc, 0x8090ac6) and the three
`r.currentAngles` comparisons (+0x140) guarding it. None of the entity types
below is a bmodel, and for a point box the two branches agree regardless.

What the game module leaves in `r.mins`/`r.maxs`. Offsets from here to the end
of this subsection are into `main/game.mp.i386.so`, the 1.1d dedicated server's
game module, not into `cod_lnxded`:

| entity | `r.mins` | `r.maxs` | written by | |
|---|---|---|---|---|
| `ET_ITEM` | (-1, -1, -1) | (1, 1, 1) | `G_SpawnItem` 0x4e6ed | VERIFIED |
| `ET_TURRET` | (-32, -32, 0) | (32, 32, 56) | `G_SpawnTurret` 0x52f75 | VERIFIED |
| `ET_SCRIPTMOVER` (`script_model`) | (0, 0, 0) | (0, 0, 0) | nothing does | INFERRED |

That last row is the one to justify, and the model a script model draws has
nothing to do with it.

No instruction in `SP_script_model` (0x60ff4), `InitScriptMover` (0x60214) or
the `setmodel` entity setter (0x5dabc) writes `r.mins` (+0x100) or `r.maxs`
(+0x10c). VERIFIED. `G_SetModel` (0x67020) does not either: it resolves the
model's `CS_MODELS` slot and writes the `modelindex` byte at +0x175, nothing
more. VERIFIED. Its `+0x10c` is the `CS_MODELS` configstring base, which
collides numerically with the `r.maxs` offset and means something else
entirely. VERIFIED.

The module has no trap that returns an xmodel's bounds: `trap_XModelGet` hands
back a handle, and the only other xmodel entry points are `trap_XModelExists`,
`trap_XModelNumBones`, `trap_XModelGetBoneNames` and `trap_XModelDebugBoxes`.
VERIFIED. Scripts cannot resize a mover either: there is no `setsize` builtin,
and `solid`/`notsolid` write only `r.contents`. VERIFIED. `G_FreeEntity`
bzeroes the 0x314-byte slot before it is reused (0x66b97), so the fields read
zero on a recycled entity as much as a fresh one. VERIFIED.

A census of every store to +0x100 or +0x10c in the module finds them paired as
vector writes in `ClientThink_real`, `ClientSpawn`, `LaunchItem`, `G_SpawnItem`,
`G_RunFrame`, `G_SpawnTurret`, `Blocked_DoorRotate` and one static in the
player-command block. VERIFIED. That none of those is reachable from a script
mover is INFERRED, from the call graph out of `SP_script_model`. Two further
stores, in `TeamplayInfoMessage` (0x6493b, 0x64acd), go through a base loaded
from +0x158. VERIFIED. That +0x158 is the `client` pointer, making them
`gclient_t` fields rather than an entity's box, is INFERRED.

`script_brushmodel` shares `ET_SCRIPTMOVER` but is not this case:
`SP_script_brushmodel` (0x60fb8) calls `trap_SetBrushModel` ahead of
`InitScriptMover`. VERIFIED. That this is what gives it real bounds, leaving
its box the brush's rather than a point, is INFERRED. Every `eType` 8 entity in
the traces we hold carries a model configstring index (56 on carentan, 26 on
pavlov), so all of them are `script_model`s. VERIFIED. Nothing we have captured
pins the brushmodel case.

#### What a map entity looks like

VERIFIED from the traces, and self-consistent with the shipped item list: a
placed weapon is `eType` 3 (`ET_ITEM`) with `index` a **1-based index into
configstring 7**, not a model index. Carentan's group at [388, -792] is four
entities with `index` 23, which is `panzerfaust_mp`, standing beside one
`eType` 8 (`ET_SCRIPTMOVER`) whose `index` 56 is the model
`xmodel/ammo_panzerfaust_box3`. Four panzerfausts and their ammo box. The
mounted MG is `eType` 11 with `index` 58, the model `xmodel/mg42_bipod`.

So `index` means one thing per `eType`, and reading a model index onto an item
gives a bookshelf where a panzerfaust stands. `ET_ITEM` reusing configstring 7
is the same 1-based weapon index `entityState.weapon` carries.

The static fields the traces show alongside those: `eFlags` 16 and
`groundEntityNum` 1022 (`ENTITYNUM_WORLD`) on the items, `clientNum` 254 on
them and 0 on the scriptmover and the turret, and `solid` 0 on all of them.
`groundEntityNum` on the scriptmover reads 0 for about a minute after a map
load and 1023 (`ENTITYNUM_NONE`) from then on, which is why a trace has to be
taken from a settled server.

#### What a player entity looks like

A client is sent no entity for itself, so a single client's capture never holds
one; this comes from two probes on one retail server, the second capturing what
it was told about the first (`--net-probe --save-entities --capture-tag
players`, committed as
`crates/server/tests/fixtures/entities/mp_carentan-dm-players.txt`).

VERIFIED from that capture: `eType` 1, `index` 0, `clientNum` the slot,
`eFlags` 16, `solid` 6684943, `weapon` the 1-based configstring 7 index, and
`groundEntityNum` 1022 or 1023. The body model is not on the entity: it rides
the `clientState` roster's `modelindex`.

**A player travels as a trajectory, not a point.** `pos.trType` 3 with
`pos.trTime`, `pos.trDuration` 50 and a `pos.trDelta`, so the receiving client
carries the motion between snapshots instead of stepping to each one.
`apos.trType` is 1 and `apos.trBase[1]` is the body yaw; a player entity
carries no pitch.

INFERRED: `solid` 6684943 decodes as `(maxs[2] + 32) << 16 | -mins[2] << 8 |
half width`, which is 0x66 = 102 = 70 + 32, 0x01 and 0x0f = 15 -- the pmove box
standing, one unit deep and 15 wide. One value against one known box, so the
packing is a reading and not a measurement.

The capture also carries `legsAnim`, an event ring with its `eventParms`, and
`angles2[1]`, a second yaw on every player that is not the view yaw. None of
those is identified here.

#### The rule, out of the binary

`SV_BuildClientSnapshot` is at `0x808f130` (it pushes the
`SV_BuildClientSnapshot: bad gEnt` string at `0x80d63e0` from `0x808f270`), and
the entity selection it calls at `0x808e298` is Q3's
`SV_AddEntitiesVisibleFromPoint` in shape. VERIFIED, all of the following read
out of the 1.1d dedicated binary: the loop bounds `sv.num_entities` at
`0x83b6684`; the gentity accessor at `0x8089258` and the parallel server-side
record at `0x8089288`; the collision helpers at `0x8053d34` (point to leaf),
`0x804bca4` (leaf area), `0x804bc8c` (leaf cluster), `0x8053cfc` (cluster PVS
row) and `0x8053f44` (areas connected); the gentity offsets `+0xf0`, `+0xf4`
and `+0xf8`; the server-record offsets `+0x118`, `+0x11c`, `+0x15c`, `+0x160`
and `+0x164`; and the immediates `0x01`, `0x18`, `0x0800`, `0x2000` and
`0x400`.

INFERRED, since every clause below is a branch condition, the per-entity rule
is: skip an entity whose `+0xf0` is zero; skip one whose `+0xf4` has `0x01`
set; with `0x0800` set send it only to the client in `+0xf8`, and with `0x2000`
set send it to every client except that one; never send a client its own
entity, which is the entity whose number equals `ps.clientNum`; send
unconditionally if `+0xf4` has either bit of `0x18`; otherwise require the
entity's area to connect to the client's and at least one of its `+0x118`
clusters to be set in the client's PVS row, with a scan up to `+0x15c` when the
cluster list did not settle it. The list stops at `0x400` entries.

#### What links an entity, out of the game module

`game.mp.i386.so` keeps its symbols, so the callers are countable rather than
guessable. VERIFIED: `trap_LinkEntity` (`0x63a40`, engine syscall `0x32`) has 65
call sites in 51 named functions. The static ones, the ones a map has before
anybody plays it, are `SP_script_model`, `SP_script_brushmodel`,
`SP_script_origin`, `SP_func_rotating`, `SP_func_leaky`, `SP_misc_spawner`,
`use_corona`, the five `SP_trigger_*`, `InitMover`, `InitMoverRotate`,
`G_SpawnItem` with `FinishSpawningItem`, and `G_SpawnTurret`. The rest link at
runtime: items being touched or respawned, movers reaching a point, missiles,
temp entities, and clients through `G_RunClient` and `ClientThink_real`.

VERIFIED, from the same module: every `SP_trigger_*` and `InitTrigger` writes
`1` to the entity's `+0xf4`, which is the bit the selection loop skips on;
`InitMover`, `InitMoverRotate`, `InitScriptMover` and `G_SpawnTurret` write
`0x80`; `ClientConnect` writes `0x200`; `fire_grenade` and `fire_rocket` write
`0x88`.

INFERRED, from those two together: a trigger is linked and never sent, and what
a map puts on the wire before anyone plays is its script models, its brush
models, its script origins, its movers, its items and its turrets. That matches
what the traces caught -- `eType` 3 script models, `eType` 8 items, `eType` 11
turrets, and nothing else on either map.

Two consequences worth stating plainly. The client's own entity being skipped
is the same fact stage 4's playerstate fixture records from the other side, and
it is why an entity-list diff cannot see anything about a client's own entity.
And an entity reaches nobody until something links it: `+0xf0` gates every
other test.

The consequence for a capture: an entity-list fixture is a fixture *of a
position*. Two captures taken at different spawns on one map disagree for
reasons that have nothing to do with which entities exist.

### svc_serverCommand (5)

`long sequence`, then `bigstring text`. Dedupe by sequence, and store in the 64-slot ring, because the scramble key needs it. Then dispatch on the leading token. CoD 1.1 server commands are single letters (`CG_ServerCommand` switches on `argv0[0]`; the full table is section 0 of `docs/research/cod11-hud-protocol.md`). The ones the engine side of a client has to act on:

- `d <index> <text>` is the configstring update (Q3's `cs`). The text is not quoted and runs to the end of the line, spaces included (`d 12 0 1 0.00025 0.32 0.36 0.4 0`, `d 0 \codextended\CoDExtended v20\...`), so take everything after the index verbatim. Index 1 is the systeminfo: re-read `sv_serverid` from it (see map_restart below).
- `n` is `map_restart`: the server has re-entered every connected client into the world.
- `w "<reason>"` is the per-client drop notice, and it is a reliable command like any other: `SV_DropClient` (cod_lnxded `0x8085cf4`) queues it before it frees the slot. The reason is a localized key; vcod's server sends `EXE_DISCONNECTED`, `EXE_TIMEDOUT` and `EXE_LOSTRELIABLECOMMANDS`. Inferred from the server disassembly; no capture of mine contains one, because every live session ended with the client leaving. vcod's server sends it, and vcod's client drops the connection with the reason it carries.
- The bare word `disconnect` runs the other way: a leaving client sends it, either out of band (`FF FF FF FF disconnect`, handled at `0x808c827`) or as a reliable client command. It is not the server's drop notice.
- `h`/`i` chat and team chat, `e`/`f`/`g` prints, `b` the scoreboard reply, and the rest are cgame-level; `docs/research/cod11-hud-protocol.md` covers them.

Sound-related letters, all cgame-level, from section 9 of `docs/research/cod11-sound-system.md`:

- `s <idx>` is `playLocalSound`: play `CS_SOUNDS + idx` non-positionally. It is addressed to one client, and a spectator does receive its own (the SD announcer arrives this way); sounds addressed to the followed player are not forwarded.
- `o <alias>`, `p <fade ms>` and `q %f %i` are `musicPlay`, `musicStop` and `soundFade`. No stock MP gametype or map script calls them and neither live capture saw one.
- `j <scope> <int> <clientNum> <category> <x> <y> <z>` is quick chat (`vsay`): the letter encodes the scope (`j` global, `k`/`l` team), argv 4 names a category (`kill_insult`, `praise`, ...) and the trailing three fields are the speaker origin cast to int. `G_Voice` (cod_lnxded `0x473b8`, format string at `0x73a4e`) sends it; the client resolves the category through `mp/*_chat.voice` tables, which no stock install ships - see `docs/research/cod11-quick-chat.md`.

### map_restart and sv_serverid

`sv_serverid` is one byte with two fields: `SV_SpawnServer` (cod_lnxded `0x808a598`) adds `0x10` per map load (`movzbl`, so it wraps at 256 and never exceeds a byte, which is why the 1-byte header below round-trips), and `SV_MapRestart_f` (`0x8083eb0`) bumps the low nibble modulo 16 and toggles `SNAPFLAG_SERVERCOUNT` (`xorb $4`). The counter starts at zero, so a freshly started server's first map runs at `0x10`. That is inferred from the add, and it is the value vcod's server uses; the populated capture's `\sv_serverid\18` is a server several loads and restarts in. Round-based gametypes (SD) restart the map between rounds, so the id changes every round.

`SV_MapRestart_f` then, for every client at `CS_CONNECTED` or above, queues the `n` server command, calls `ClientConnect` again, and re-enters `CS_ACTIVE` clients with `SV_ClientEnterWorld` (`0x80877d8`: state stays `CS_ACTIVE`, `deltaMessage = -1`, `ClientBegin`, which respawns a spectator at the intermission point, usually a floating camera spot). The new systeminfo goes out as `d 1 ...` right after `n`.

`SV_ExecuteClientMessage` (`0x80872ec`) compares the header serverId with `sv_serverid`. Equal: normal processing. High nibble differs (a map change): resend the gamestate if the client's `messageAcknowledge` is past the gamestate message, then return. Only the low nibble differs (a restart): a `CS_PRIMED` client is promoted to `CS_ACTIVE`, anything else returns without reading a single op. So a client that keeps sending the pre-restart id after a round restart is silently ignored: no moves, no reliable commands, and no further snapshots at all, until the client's own snapshot-silence timeout drops it. VERIFIED live 2026-08-24 on an SD server (`51.68.172.126:1337`): with the stale id nothing arrived for the 30 s after `n` and the client timed out; with the id re-read from `d 1`, snapshots resumed within a second of `n` (delta frames, spectator back at its spawn) and moves applied again.

### svc_download (6)

The RTCW UDP download protocol, unchanged in flow. The client asks with a reliable `download <name>.pk3` (names from `sv_referencedPakNames` in the systeminfo, gamedir prefix included, e.g. `main/n_dufresne.pk3`); the server streams blocks inside its regular messages:

```
short block           ; 0-based, in order
if block == 0:
  long  fileSize      ; -1 = refusal, then a string with the reason
short chunkLen        ; 0 = EOF
byte  data[chunkLen]
```

Ack each accepted block with a reliable `nextdl <block>`; the server's send window retransmits until acked. A zero-length block ends the file. `stopdl` aborts, and a final `donedl` makes the server re-send the gamestate. **Divergence from RTCW, #7.** `MAX_DOWNLOAD_BLKSIZE` is 8192, not RTCW's 2048 (observed live from the retail server; a 2048 cap rejects every real block). Download rate is governed by `sv_dl_maxRate`, and one message can carry several blocks. Stock paks (`main/pak0`..`pak9`, `localized_*`) are refused server-side.

## Delta field encoding

Field tables are arrays of `{ char *name; int offset; int bits }` (12 bytes per entry on i386), the Q3 `netField_t` layout. The 1.1 binary is stripped but keeps every field-name string, so you can recover the tables by finding the name pointers in `.data` and walking the 12-byte stride. I recovered 59 entity fields (table at `0x080d1760`) and 103 playerState fields (table at `0x080d229c`). Wire field order equals array order, so never sort them.

Field semantics: `bits == 0` is a float field, and a positive `bits` is an integer width. Here's a gotcha. The playerState table contains negative widths, for example `-16` and `-8`. These are not sign flags on the wire. CoD 1.1 reads them as plain unsigned of width `|bits|` (`read_delta_playerstate` `0x807e2f0`, integer read at `0x807e5d7`, no `sar`). I verified that `weaponTime` (`-16`) written as `0xffff` reads back as `65535`. Take the absolute value for the width and read unsigned.

There are two incompatible bit-field codings in the binary. They agree below 8 bits and diverge at and above 8: one for entity numbers and small counts, and `read_packed_bits` (`0x807cb03`) for delta field values. Use the same one the engine uses per site, or you desync on the first wide field.

### Entity delta (svc_baseline and packet entities)

This follows Q3's `MSG_ReadDeltaEntity`: a "last changed field" index, then per field a change bit. A changed field reads its width. Float fields read a zero-flag bit, then either a truncated 13-bit int-float or a full 32-bit value. Floats and the `lc` byte are raw bytes at the byte cursor.

The zero flag is where a writer has to be careful. "Changed, and the value is zero" reached from a `+0.0` base means the sender's value was `-0.0`: the C compares the two slots as `int`s before it compares floats, so `+0.0` against `+0.0` is not a change at all and only the other zero gets there. The retail server emits exactly this: `mp_carentan`'s baselines for entities 253 and 256 carry `apos.trBase[0] == -0.0`, and a reader that stores `0.0f` (as the C client does) cannot reproduce the message it just read. Recovering `-0.0` in `read_float_field` is what makes the round trip byte-exact.

### PlayerState delta

Like the entity delta but with no entity number, and (divergence) no zero-flag in the main loop. The entity delta has one; the playerstate delta does not. After the scalar fields come five array blocks (stats, ammo, HUD), each gated by a changed-array bitmask. Block 1 is a gate plus a 6-bit mask. Block 2 is a group-gate with `MSG_ReadShort` (byte-aligned) masks and short elements. Block 3 is ungated. Block 4 is 16 weapons, each a 3-bit value plus a gate plus 6 generic delta fields. Block 5 is a 5-bit outer count plus a 5-bit inner field count. There's a 34-entry HUD field table at `0x80de384` in `.data` for one of these blocks.

Missing zero flag, same `-0.0` trap. The change bit still means the C saw two different `int`s, but a changed float goes out through the integral path as a truncated 13-bit value, so `-0.0` reads back `+0.0` and the difference is gone. A reader that keeps the decoded `+0.0` re-encodes the field as unchanged and shortens `lc` with it. VERIFIED against the retail 1.1d dedicated server on mp_carentan: as a spectator's move ends, `velocity[2]` rides three frames as changed-but-zero, and retail's `lc` runs out to field 54 to carry it while the last field with a real change is 15 (frames 134 to 136 of `crates/common/tests/fixtures/net/snapshots-delta.bin`; storing `+0.0` gives `lc == 16` and a frame six bytes short). Recovering `-0.0` when the decoded value equals the base is the same move `read_float_field` makes for entities.

The array blocks are themselves gated against the base frame, so they are full on an uncompressed frame and empty once the client is deltaing. VERIFIED across the two committed captures: all 25 uncompressed frames carry them (42 primitives each) and none of the 399 delta frames does, a spectator's stats and HUD never changing (`crates/common/tests/fixtures/net/snapshots.bin` and `snapshots-delta.bin`). Skipping them keeps the parse aligned but loses the frame, so vcod records the primitives it consumed (`PlayerState::arrays`) and replays them verbatim; nothing decodes them into fields, and nothing pins their meaning beyond the widths that keep the stream aligned.

### Trajectories (`trType`/`pos`/`apos`)

Entity movement/rotation is a Q3-shaped `trajectory_t` (`trType`, `trTime`, `trDuration`, `trBase`, `trDelta`) carried in the `pos` and `apos` netfield groups, evaluated with RTCW's `BG_EvaluateTrajectory` maths (LINEAR, LINEAR_STOP, SINE, GRAVITY variants, ACCELERATE/DECCELERATE; STATIONARY/INTERPOLATE/GRAVITY_PAUSED just return `trBase`, matching snapshot-interpolated fields). **Divergence, observed not derived from source.** Real CoD 1.1 servers send `TR_SINE` with `trDuration == 0` on `ET_ITEM` pickups (thousands of samples across two populated servers). RTCW's reference divides `dt / trDuration` unconditionally in that branch, which is a guaranteed NaN here. Guard `trDuration == 0` in the `TR_SINE` branch by returning `trBase`, the same fallback as `STATIONARY`. Anything else makes every dropped/spawned item invisible.

## Client to server message body (clc)

clc opcodes are 2-bit. After the 9-byte plain header:

```
clc_clientCommand: 2-bit op, long sequence, string   (reliable commands: chat, "disconnect", team, etc.)
clc_move / clc_moveNoDelta: 2-bit op, byte count, <delta usercmds>
clc_EOF
```

`usercmd_t` layout, verified against the game's own struct:

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

Delta-encode against the previous usercmd (`MSG_WriteDeltaUsercmdKey`). The move message is keyed: `key = checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommands[reliableAcknowledge & 63], 32)`. There's a compact-move fast path where `forwardmove = 127` and friends encode as flag bits. `Com_HashKey` is at `0x806810c`; the compact-move mask table (`1,3,7,15,31,63,127`) is in `.data` around `0x80de304`.

The reader is `MSG_ReadDeltaUsercmdKey` (cod_lnxded `0x807b7f8`) and it has two branches. Both start with `to = *from` (so anything not transmitted keeps the base cmd's value), a serverTime preamble (1 bit; set = an 8-bit delta from the base, clear = a 32-bit absolute), a keyed "changed" bit that returns the base cmd unchanged when it equals `key & 1`, and a keyed branch bit: equal to `key & 1` picks the compact branch, otherwise the full-field one. The mask table at `0x80de300` is `mask[n] = (1 << n) - 1` (`0x80de304` = 1, `0x80de308` = 3, `0x80de310` = 0xf, `0x80de318` = 0x3f), and a keyed `n`-bit field is `read_bits(n) ^ (key & mask[n])`; a keyed angle is a raw short off the byte cursor XORed with the whole key and truncated to 16 bits; a keyed byte XORs with `key & 0xff`. Nothing is called out of line: `MSG_ReadBits`, `MSG_ReadByte`, `MSG_ReadShort` and `MSG_ReadLong` are all inlined, so both cursors show in the disassembly, loose bits off `msg->bit` (offset `0x14`) and whole bytes off `msg->readcount` (`0x10`).

Preamble, both branches:

| VA | Wire | Effect |
|---|---|---|
| `0x807b80a` | none | `to = *from` (`rep movsl` x6); every field not transmitted keeps the base value |
| `0x807b812` | 1 bit | set: a byte at the byte cursor, `to.serverTime = from.serverTime + byte`; clear: 32-bit absolute (`0x807b866`) |
| `0x807b88a` | 1 bit | `== key & 1` returns at `0x807c0d9`, the cmd is the base cmd unchanged |
| `0x807b8e6` | none | `to.buttons &= 0xfe` |
| `0x807b900` | 1 bit | `== key & 1` selects the compact branch, otherwise the full branch at `0x807bba0` |

Compact branch (`0x807b956`), the one vcod's `write_delta_usercmd` emits when nothing the branch omits differs from the previous sent cmd (upmove, weapon, wbuttons and the upper button bits are full-branch only). `key ^= to.serverTime` first, then:

| VA | Field | Wire | Keyed with |
|---|---|---|---|
| `0x807b967` | `buttons` bit 0 | 1 bit | `key & 1` |
| `0x807b9bd` | `angles[0]` | change bit, then i16 | `key` |
| `0x807ba1e` | `angles[1]` | change bit, then i16 | `key` |
| `0x807bac0` | `forward`/`right` | change bit, then 4 bits | `key & 0xf` |

It returns at `0x807bb81`. `angles[2]`, `up`, `wbuttons`, `weapon` and `flags` cannot be expressed in this branch.

Full-field branch (`0x807bba0`). The serverTime is mixed into the key only after the forward/right code (`0x807bde1`), so the first four fields are keyed with the raw key and the last five with the mixed one:

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

`flags` (offset 7) is never transmitted by either branch. `to.buttons` is rebuilt, not patched: bit 0 comes from the first field, and `0x807be45` clears bits 1-7 again before the 6-bit field is ORed back in shifted left by one (`add %al,%al`). The movement axes are never analog on the wire: each is a 2-bit code with a +/-10 deadzone (`> 10` sets bit 0, `< -10` sets bit 1) that decodes to 127, -127 or 0 (`0x807bb52` for forward/right, `0x807c013` for up), and the same derivation runs on the base cmd to produce the default, so a base `forward = 100` reads back as 127. Forward and right share one nibble (forward in bits 0/1, right in bits 2/3).

The angle change bits are not vestigial: VERIFIED live 2026-08-26, a retail 1.1 client sent roughly 624 moves in a 3-minute session with a cleared change bit on pitch or yaw, keeping the previous sent cmd's angle instead of announcing it. Each one flashes the view to zero for a frame unless the server decodes it the same way retail does, against its persistent last received cmd for that client (`cl->lastUsercmd`). The chain restarts with each gamestate, because the client wipes its own state -- the cmd ring with it -- as it parses one (`CL_ParseGamestate` calls `CL_ClearState`, Quake III Arena `code/client/cl_parse.c` and `code/client/cl_main.c`); vcod's server clears its stored base in `SV_SendClientGameState` for that reason.

Transcribed from disassembly. Widths and order VERIFIED live 2026-08-25: a retail 1.1 client's moves parsed against vcod's server for a whole session with no truncation error (`docs/research/cod11-server-handshake.md`, "Retail client check"). Field values are still unchecked, since the server parses and discards them.

### Usercmd input bits

Which bit means which key. Measured 2026-09-01 by logging `buttons`, `wbuttons` and `upmove` off a retail 1.1 client connected to vcod's server while each key was pressed in turn, and cross-checked against CoDExtended's bot input path (`src/gsc_bots.c`, which sets one mask per verb) and its `KEY_MASK_*` defines (`src/shared.h`). The two agree everywhere they overlap.

| Verb | Field | Value |
|---|---|---|
| move forward/back, right/left | `forwardmove`, `rightmove` | +/-127 |
| jump | `upmove` | +127 |
| crouch | `wbuttons` | `0x80` |
| prone | `wbuttons` | `0x40` |
| lean left | `wbuttons` | `0x10` |
| lean right | `wbuttons` | `0x20` |
| reload | `wbuttons` | `0x08` |
| fire | `buttons` | `0x01` |
| ads | `buttons` | `0x10` |
| melee | `buttons` | `0x20` |
| use | `buttons` | `0x40` |

Three things a Q3 reading gets wrong:

- **`upmove` is not a jump axis.** A crouched or prone client holds `upmove` at -127 for as long as it stays down, so only a positive `upmove` is a jump. Q3 reads -127 as crouch; CoD has a `wbuttons` bit for that and sends both.
- **The stance bits are level states, not key edges,** and they are mutually exclusive. The client owns the toggle -- crouch is a tap in the retail binds, not a hold -- and what reaches the wire is the resulting stance, held. One capture holds `0x80` for 18 seconds across a crouch, with `0x40` replacing it for the 6 seconds spent prone.
- **`buttons` `0x10` is ads, where Q3's `BUTTON_WALKING` is `0x10`.** There is no walk bit at all: CoD 1 has a single move speed, so nothing in a capture varies with it, and no `KEY_MASK` in the reference names one. `pm_walkSpeedScale` still exists engine-side (the stage 4 playerstate capture pins it at 0.4), but no client input reaches it.

`buttons` `0x02` was seen live, briefly, twice at spawn and once well after; nothing pressed in the capture accounts for it and it is left unidentified. `usercmd_t`'s own comment lists "console, chat" among the `buttons` meanings (CoDExtended `src/shared.h`), which is the likeliest home for it. INFERRED.

### Entering the world

A client is only sent snapshots once it's CS_ACTIVE, and that requires the server to accept a `clc_move`. Header-only keepalives keep the connection alive but won't promote you. This is the gate between "connected" and "receiving game state," and it's where I was stuck longest.

**`begin` is not a client command.** The engine's client command table has nine entries and none of them is `begin`: `userinfo`, `disconnect`, `cp`, `vdr`, `download`, `nextdl`, `stopdl`, `donedl`, `retransdl` (the `ucmds` table in CoDExtended's `src/sv_client.c`, whose entries are the retail server's own function addresses; Quake III Arena's table in `code/server/sv_client.c` is the same nine minus `retransdl`). A retail client never sends one, so a server that waits for it waits forever: the client loads the map to 100%, starts the ambient, and sits on the loading screen with no snapshot ever arriving.

The trigger is **the first usercmd after the gamestate**: `SV_UserMove` promotes a CS_PRIMED client before the block's cmds are applied. The document default does not cover the rest of this paragraph -- CoD's own `SV_ClientEnterWorld` is at cod_lnxded `0x80877d8`, but none of what follows was measured against a running one. It is a Q3/ioq3/RTCW-MP source read, each bullet naming its own, and vcod implements it on that basis:

- The cmd passed in is the *first* of the block, `SV_ClientEnterWorld( cl, &cmds[0] )`, and entry copies it into `cl->lastUsercmd` (Quake III Arena `code/server/sv_client.c`, `SV_UserMove` and `SV_ClientEnterWorld`; ioq3's copy is the same call with a null-cmd branch).
- The execute loop then skips every cmd whose `serverTime <= cl->lastUsercmd.serverTime`, so the entering cmd is never itself simulated (same file, `SV_UserMove`'s per-cmd loop).
- The spawn's `delta_angles` are taken against the entering cmd's angles rather than against zero. `ClientSpawn` pulls the client's stored cmd into `pers.cmd` with `trap_GetUsercmd` on the line before it calls `SetClientViewAngle`, which subtracts `pers.cmd.angles` from the spawn angle (RTCW-MP `src/game/g_client.c`); the trap returns `svs.clients[n].lastUsercmd` (RTCW-MP `src/server/sv_game.c`, `SV_GetUsercmd`), which entry has just set to `cmds[0]`.

Entry is also where the game module is called: `SV_ClientEnterWorld` ends in `ClientBegin` (the `GAME("ClientBegin")` call in CoDExtended's `src/sv_client.c`). That is the notify `Callback_PlayerConnect`'s `waittill("begin")` blocks on, and it is an engine-to-game call, not a command any client sends. The gsc side of it is section 0.1 of `docs/research/cod11-hud-protocol.md`.

The one other promotion path is the restart, above: a CS_PRIMED client whose serverId differs only in the low nibble goes to CS_ACTIVE with no entering cmd. ioq3 passes a null cmd on its equivalent path and zeroes `lastUsercmd` (`code/server/sv_ccmds.c`, `SV_MapRestart_f`); vcod does the same.

## Spectator

On a DM server the client auto-spawns as `GAME_SPECTATOR`, no team command needed. Send usercmds (movement axes plus `ANGLE2SHORT` view angles) every frame; the server flies the spectator and the position comes back in the playerState. It is a true noclip: the position integrates straight off the velocity with no trace, as Q3's `PM_NoclipMove` does. **This is a divergence from the RTCW lineage**, which sends `PM_SPECTATOR` to `PM_FlyMove` and so collides through `PM_StepSlideMove` (`bg_pmove.c:3927`), with `PM_NOCLIP` a separate pm_type. VERIFIED live 2026-08-28 by A/B with a retail client: against a retail server a spectator clips through the wires strung between lamp posts, through decoration cars (the ones by the allied spawn on mp_carentan), and through walls and the ground; against a vcod server that still collided, every one of them blocked. `forward = 127` moved my playerstate origin about 636 units over 2 seconds and stopped when I stopped sending it, which is the cheapest end-to-end proof the move path works.

### Spectator view angles: `delta_angles`, not `viewangles`

A retail server does not maintain `ps.viewangles` for a spectator. VERIFIED from both committed captures: `viewangles[0]` and `viewangles[1]` are 0.0 on every frame of both, while `delta_angles[1]` holds 16384 (= 90 degrees, the spawn yaw) throughout. The probe that took those captures sends `cmd.angles = [0,0,0]` and never moves its view, so if the server were running RTCW's `PM_UpdateViewAngles` (`ps.viewangles[i] = SHORT2ANGLE(cmd.angles[i] + ps.delta_angles[i])`) the capture would read 90 degrees, not 0.

The angles live in the `cmd.angles` / `delta_angles` pair instead. `delta_angles` is set once at spawn as `ANGLE2SHORT(spawn_angle[i]) - cmd.angles[i]` (RTCW `SetClientViewAngle`, `g_client.c:437`), the client subtracts it when building each usercmd, and the client owns its view between snapshots. vcod's own client already does the subtraction (`crates/common/src/net/mod.rs`).

vcod's server implements the RTCW scheme: `ClientSim::spectator`/`become_player` set `delta_angles` at spawn from `ANGLE2SHORT(spawn_angle) - cmd.angles`, `ClientSim::step` reads `SHORT2ANGLE(cmd.angles + delta_angles)` rather than the raw cmd angle, and `to_wire` no longer writes `viewangles`. All three landed in one commit deliberately: getting one or two without the third rotates the move basis, since the client always subtracts `delta_angles` when it builds a cmd (`crates/common/src/net/mod.rs`) -- a server that sets the delta but keeps reading the raw cmd angle, for instance, has the client compensating for an offset the server never applies, so the client walks at an angle to where it looks.

Four more fields a retail spectator's playerstate carries that vcod's does not: `bobCycle` 12, `eventSequence` 1, and the pair `events[0]` 143 / `eventParms[0]` 136. `crates/common/examples/snapshot_timing.rs` prints all of them from the captures. They belong to the spectator path and not to a player: all four read 0 in the retail *player* captures stage 4 took (`crates/server/tests/fixtures/playerstate/*.txt`), so vcod's zeros are right for a spawned player and the four spectator values stay unestablished.

### A spawned player's playerstate: what stage 4 settled

Three fields a walking player carries that a spectator does not, all measured off `crates/server/tests/fixtures/playerstate/mp_pavlov-dm.txt` and its `mp_carentan` twin and reproduced by `ClientSim::to_wire`:

- `pm_flags` 262144. Bit 18, the third member of the view-source group whose other two (0x10000 free spectator, 0x20000 following) `cod11-events-and-fx.md` already lists. `ClientEndFrame` sets it for a client looking out of its own body and both other arms clear it, which is why the spectator captures read 0.
- `serverCursorHintString` 255. The field is signed 8 bits and retail stores -1, a no-hint sentinel, from `G_CheckForCursorHints`.
- `viewmodelIndex` 52 on `mp_pavlov` and 82 on `mp_carentan`: the model configstring index of the hands viewmodel the nationality's character script asked for, 1-based on base 268. It is derived here, not pinned — `setViewmodel` allocates through the same model indexer the configstring gate diffs, so the number falls out of the table.

The addresses and the labels for all three are in `docs/research/cod11-gsc-object-model.md`, section 20.

One field remains unestablished: `legsAnim` 634, which is animation index 122 with `ANIM_TOGGLEBIT` set. The server picks it through the animscript state machine, which vcod has no equivalent of, so `playerstate_ab.rs` carries it as the gate's one recorded gap.

### The view-height lerp, and fields a settled capture cannot see

The eye eases between stances while the collision box snaps, and the client predicts the ease itself. Four playerstate fields carry it: `viewHeightCurrent` (float, the eye now), `viewHeightTarget` (signed byte, the stance's height), `viewHeightLerpDown` (1 bit, set when the eye is on its way down) and **`viewHeightLerpTime`**, which is the `serverTime` the lerp started at and reads 0 once it settles.

That last one is the trap. VERIFIED live 2026-09-01 by tracing the retail server per snapshot through a crouch: `viewHeightLerpTime` read 86666 while `serverTime` ran 86700, 86750, 86800 with the eye at 57.92, 52.64, 44.72, and dropped to 0 at 86850 with the eye settled at 40. Every value it takes outside a transition is 0, so a capture of a settled pose -- which is what both playerstate gates take -- pins it at 0 and proves nothing. A server that always sends 0 tells the client the lerp began at time zero; the client's prediction snaps its eye straight to the target and the next snapshot drags it back, once per snapshot, for as long as the transition lasts. It reads as the camera shaking for about half a second on every stance change.

The general shape is worth naming, because two vcod bugs came out of it: **a field that only holds a value mid-transition is invisible to a settled capture.** `leanf` and this one are both of that kind. Trace per snapshot through the transition, not after it.

### `ps.commandTime` and client prediction

`ps.commandTime` must carry the `serverTime` of the last usercmd the server actually simulated for that client, and nothing else. A client replays every usercmd newer than it on top of the snapshot's playerstate, so a `commandTime` naming a moment the server never simulated silently drops that slice of the client's own input from its prediction, every frame.

VERIFIED live 2026-08-28 against the retail 1.1 client, both directions of the experiment. vcod's server clamped `commandTime` up to `serverTime - sv_fps_interval`, which put it 11-24 ms past the newest cmd it had simulated on 100% of 801 traced frames; retail rendered smooth movement with a view that juddered at snapshot rate, because position is predicted while the view angles were being reset from the stale snapshot 20 times a second. Reporting the true last-simulated cmd time made `commandTime - last_simulated` exactly 0 on all 446 frames of the confirming trace and the judder went away.

The lead (`serverTime - commandTime`) is a consequence, not a target: it is however far behind the client's own clock runs. The committed captures, taken with vcod's probe against the retail server, show 0-34 ms (mean 16); a retail client against vcod's server shows 63-74 ms, because a retail client deliberately runs its `serverTime` estimate behind so it always has frames to interpolate between. Clamping the lead to hide that difference is what caused the bug. `crates/common/examples/snapshot_timing.rs` prints the capture side of this; `vcod-server --trace` prints the live side.

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
- `12`: global fog, `<near> <far> <density> <r> <g> <b> <fadeTime ms>`, written by the game dll's gsc builtins `setCullFog(near, far, r, g, b, fade)` and `setExpFog(density, r, g, b, fade)`. Every stock MP map sets one at load. The density slot selects the mode: `>= 1` means linear farclip fog across near..far, below 1 means GL_EXP at that density (RTCW-MP tr_main.c R_SetFog; its client adds +0.1 to the same slot before the GL call, so the wire value itself stays un-offset — INFERRED for exp-fog maps, VERIFIED at "1" on two setCullFog captures). Linear fog leaves the sky visible and unfogged (VERIFIED live on mp_ship against retail: clouds stay in view — CoD diverges from RTCW's drawsky=false here) and clears to the fog colour; exp fog keeps the sky and fogs it. Fade time drives a lerp between same-mode fogs; cross-mode changes snap.
- `524..779`: sound alias names, `CS_SOUNDS`, 256 slots with index 0 unused. `EV_SOUND_ALIAS`'s `eventParm` and `entityState.loopSound` both index it. The server fills it lazily at first use (`G_SoundAliasIndex`), so resolve a name at play time and follow `d` updates inside the block; snapshotting it at connect misses most of it.

Player entities are identifiable by `entity number < MAX_CLIENTS (64)`; the client slot equals the entity number. Don't try to key on `eType`. CoD's `ET_PLAYER` and `ET_GENERAL` are both 0, so a low number is the reliable discriminator.

## Divergences from RTCW/Q3, in one place

If you're porting a Q3/RTCW server, these are the things that will bite:

1. `connect` userinfo is Huffman-compressed from byte 12, not plaintext.
2. Message-block compression has no 2-byte length prefix; decode until the input is exhausted.
3. The whole message is built plain and block-compressed, not per-byte Huffman inside the bit writer.
4. `reliableAcknowledge` (server to client) is a plain u32 at bytes 0 to 3, outside the compressed stream.
5. The client-to-server header is 9 plain bytes with a 1-byte serverId, not a compressed 4-byte one.
6. Snapshots have no areamask.
7. PlayerState negative field widths are unsigned (width only, no sign extension), and the playerState delta has no zero-flag in its main loop, so a changed `-0.0` float decodes as `+0.0` and only `lc` betrays that the field was sent at all.
8. Real servers send `TR_SINE` trajectories with `trDuration == 0` on item pickups, which divides by zero under the literal RTCW `BG_EvaluateTrajectory` port; guard it to return `trBase`.
9. Server commands are single letters: the configstring update is `d <index> <text>` with unquoted text to end of line, not `cs <index> "<text>"`, and `map_restart` arrives as `n`.
10. `sv_serverid` is a byte (`0x10` per map load from a zero start, low nibble per `map_restart`) and a stale low nibble makes the server drop the whole client message, snapshots included, instead of Q3's "ignoring pre map_restart" path that keeps serving them.
11. The client-to-server scramble covers the compressed op block from its byte 0, packet byte 15, with no `CL_ENCODE_START` offset into it; the 9 plain header bytes in front are never scrambled and the parity `c << (i & 1)` counts from the block's own start.
12. The per-client drop notice is the reliable command `w "<reason>"` (`SV_DropClient` `0x8085cf4`), not Q3's `disconnect "<reason>"`. Bare `disconnect` only travels client to server.
13. `MAX_RELIABLE_COMMANDS` is 64, not RTCW's 256. CoDExtended's `shared.h:135` and the `& 63` masks in `SV_UserMove` (cod_lnxded `0x8087043`) agree. Both reliable rings, and the scramble key that indexes them, are sized off it.
14. `MSG_ReadString` AND `MSG_ReadBigString` both map `%` -> `.`, 0x92 -> `'`, and every other byte over 127 -> `.` (CoDMP.exe `0x444e00`/`0x444e60`; Q3/RTCW keep high bytes in the big reader). This is wire compatibility, not cosmetics: the usercmd delta key is `checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommands[reliableAcknowledge & 63], 32)`, `Com_HashKey` (`0x806810c`) multiplies raw sign-extended bytes with no substitution of its own, and the server hashes the string it queued while the client hashes the string it read. Only the read-time mapping keeps the two byte streams identical. A client that keeps high bytes (worse, re-encoded as two-byte UTF-8) computes a wrong key whenever the acked server command carries one - a chat line with a high-byte name - and the server then misreads the keyed framing bits of every move, so usercmds are dropped or garbled until an ASCII-clean command rotates into the slot. Live symptom: `ps.commandTime` frozen for 20+ s stretches, spectator movement applying seconds late or not at all, on busy servers only.

15. A spectator noclips. RTCW-MP sends `PM_SPECTATOR` to `PM_FlyMove`, which collides through `PM_StepSlideMove`; CoD 1.1 integrates the position straight off the velocity with no trace, like Q3's separate `PM_NOCLIP`. VERIFIED live 2026-08-28 by A/B with a retail client against a retail server: wires between lamp posts, decoration cars and the map's own walls and ground all pass straight through. vcod collided until it did not (`spectator_move`, `crates/common/src/pmove.rs`).

Everything else holds: the huffman table, the svc/clc opcodes, netchan fragmentation, the XOR scramble structure, and the delta-encoding shape are all Q3 as documented in the RTCW/Q3 GPL sources.

## What vcod's server does that retail does not

Anti-abuse behaviour only; the wire format is unchanged (`crates/server/src/server.rs`).

- Connectionless queries (`getinfo`, `getstatus`, `getchallenge`) and the out-of-band `disconnect` are rate limited the way ioquake3 does it: a per-source-address bucket (10 burst, one back per second, 1024 addresses tracked) and a global reply bucket (10 per 100 ms). Excess requests are dropped silently. Retail answers every one, which makes `getstatus` a reflection amplifier.
- A netchan message is parsed in full (header checks, `serverId`, every command and every usercmd) before anything about the client is updated. Retail commits the sequence number, the address and the timeout stamp first, so one spoofed packet with a client's IP and qport and a huge sequence number stalls that client until it times out.
- A `connect` that matches a live client's address and qport may replace it only when it carries that client's challenge, or when the slot has been silent for `sv_reconnectlimit` (3 s). Retail hands the slot over on the address match alone.
- Challenges expire after 60 s.

