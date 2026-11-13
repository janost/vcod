# Call of Duty 4 1.7: the handshake against the retail Linux server

What a CoD4 1.7 client sees from `cod4_lnxded` between `getinfo` and the end of the gamestate, measured live on loopback, and what vcod-server sends in its place.

## Sources

- `cod4_lnxded` from the 1.7 Linux server package: ELF i386, stripped, built `Jun 28 2008` (string at 0x8219c14), console banner `CoD4 MP 1.7 build linux-i386 Jun 28 2008`. Addresses without a module name are in it. An INFERRED claim that names no 1.7 address is not yet confirmed on 1.7. The package also holds `cod4_lnxded-bin`, a different build dated `May 20 2008`; only `cod4_lnxded` was run.
- `iw3mp.exe` 1.7 for the client half (image base 0x400000).
- Two captures, written by `tools/cod4_handshake_probe.py` and read by `cargo run -p vcod-common --example cod4_capture`: `crates/cod4/tests/fixtures/net/handshake-1.7.txt` (the full walk, a refused connect included) and `handshake-1.7-cmds.txt` (an earlier run whose gamestate carries two server commands; cut at 3.3 s). Each line is `<ms> <c2s|s2c|auth> <peer> <hex>`, every datagram raw. Facts read from them are VERIFIED live.

## 1. Running the retail server

- It needs 32-bit glibc (`ld-linux.so.2`, `libdl`, `libpthread`, `libm`, `libc` from the system's lib32). The package ships `libstdc++.so.6` and `libgcc_s.so.1`; `LD_LIBRARY_PATH` must point at them. VERIFIED.
- The package holds only the 1.7 patch payload. Game data comes from a full 1.7 install: a symlink tree with `main/` (`iw_00..13.iwd`, the localized iwds) and `zone/` (`zone/english/*.ff`) beside the binary and its two libraries, passed as `fs_basepath`. `fs_homepath` points at an empty writable directory. VERIFIED.
- Command line: `+set fs_basepath <tree> +set fs_homepath <home> +set net_ip 127.0.0.1 +set net_port 28960 +set dedicated 1 +set sv_punkbuster 0 +set logfile 2 +set sv_maxclients 8 +map mp_crash`. It loads `code_post_gfx_mp`, `localized_code_post_gfx_mp`, `ui_mp`, `common_mp`, `localized_common_mp`, unloads `ui_mp` and loads `mp_crash` in 2-5 s. VERIFIED (its console).
- Without PunkBuster's files the server logs `Error: Unable to initialize punkbuster.  Punkbuster is disabled` and carries on. VERIFIED.
- `developer 1` refuses every connect: `SV_DirectConnect` logs `Clients can't connect if the server is in developer mode` (0x8217ed0) and answers `error\nGAME_INVALIDSERVER` (0x8217f4b). VERIFIED live. Run at `developer 0`.
- Isolation. `+set dedicated 1` is refused (`dedicated is read only.`; section 15, "The server's own address"), so the server runs at `dedicated 2` and heartbeats: it resolves `cod4master.activision.com` (0x822325c) and sends `heartbeat COD-4\n` (`COD-4` at 0x8232326) to port 20810. VERIFIED live. In a bare `unshare -rn` the lookup still reached systemd-resolved's socket and resolved the real address; only the send failed (`Network is unreachable`). VERIFIED live. The recipe (section 7) uses `unshare -rnm`, a hosts file bind-mounted over `/etc/hosts` mapping the master to 127.0.0.2, and a tmpfs over `/run/systemd/resolve`. The heartbeat then lands on 127.0.0.2:20810, where the probe logs it (`auth` lines), and nothing leaves the namespace. VERIFIED live.

## 2. `getinfo` and `getstatus`

- `getinfo xxx` answers `infoResponse\n\challenge\xxx\protocol\6\hostname\CoD4Host\mapname\mp_crash\sv_maxclients\8\gametype\war\pure\1\kc\1\hw\1\mod\0\voice\0\pb\0` (142 bytes with the `FF` header): CoD2 1.3's key set and order (`cod2-protocol.md` 9) with `protocol` 6. VERIFIED live.
- `getstatus xxx` answers `statusResponse\n`, the serverinfo with `challenge\xxx`, `pswrd\0` and `mod\0` appended, and an empty player list (372 bytes). It carries `gamename\Call of Duty 4`, `protocol\6` and `shortversion\1.7`. VERIFIED live.
- **Protocol 6, shortversion 1.7.** A `connect` with `protocol\7` gets `error\nEXE_SERVER_IS_DIFFERENT_VER\x151.7\n` (string at 0x8230d4a); `protocol\6` gets `connectResponse`. VERIFIED live.
- The default gametype is `war` and `sv_maxRate` 5000. VERIFIED live.

## 3. The handshake

The datagrams of `handshake-1.7.txt` in order, sizes including the four `FF` bytes or the netchan header. VERIFIED live.

| ms | dir | bytes | content |
|---|---|---|---|
| 0 | c2s | 15 | `getinfo xxx` |
| 1 | s2c | 142 | `infoResponse` |
| 1 | c2s | 17 | `getstatus xxx` |
| 7 | s2c | 372 | `statusResponse` |
| 7 | c2s | 53 | `getchallenge 0 "<32 hex>"` |
| 14 | s2c | 32 | `challengeResponse <int>` |
| 14 | c2s | 174 | `connect "<userinfo, protocol 7>"` |
| 18 | s2c | 42 | `error\nEXE_SERVER_IS_DIFFERENT_VER\x151.7\n` |
| 18 | c2s | 174 | `connect "<userinfo, protocol 6>"` |
| 23 | s2c | 20 | `connectResponse ` |
| 24..58 | c2s | 1253 x6, 765 | `stats` 0..6 |
| 30..63 | s2c | 19-20 | `statResponse 126, 124, 120, 112, 96, 64, 0` |
| 63 | c2s | 15 | netchan: seq 1, qport, serverId 0, messageAck 0, reliableAck 0, no body |
| 69..2867 | s2c | 1310 x9, 1250 | gamestate, seq 1, ten fragments |
| 113.. | c2s | 15 | the same empty message every 50 ms |
| 3164.. | s2c | 9 | a message of `svc_EOF` alone, about every 50 ms |

- **Challenge.** The client sends `getchallenge 0 "<md5 hex>"` (`getchallenge 0 "%s"`, `iw3mp.exe` 0x6d09c4). VERIFIED. The server answered a loopback client at once, with no authorize round trip. VERIFIED live. The 32 hex digits became the client's GUID (`Going from CS_FREE to CS_CONNECTED for  (num 0 guid "<the same hex>")`). VERIFIED live. The authorize strings are present (`getIpAuthorize %i %i.%i.%i.%i "%s" %i` at 0x823070c), yet no `getIpAuthorize` reached 127.0.0.2:20800 in either run. VERIFIED live. That a loopback client skips the authorize step is INFERRED from that silence.
- **CD key hash.** The `<md5 hex>` is `CL_BuildMd5StrFromCDKey` (`iw3mp.exe` 0x4699a0): MD5 of the lower-cased key from Activision's own initial state, the hash CoD2 takes over the upper-cased key. VERIFIED. The key goes as `getKeyAuthorize 0 <key> PB <hash>` to `cod4master.activision.com` port 20800 (`CL_RequestAuthorization` 0x469b20). VERIFIED (the strings and the port). It goes only to a server off the LAN, before each challenge, and the client waits for no answer (`CL_CheckForResend` 0x46b1d7). INFERRED. cod11-server-handshake.md, "CD key authorization (the clients)", sets the five clients side by side. Without a key vcod sends md5("vcodprobe"), which a server checking no key accepts.
- **Connect.** `connect "<userinfo>"` is plain text after the `FF` bytes, as on CoD2: the server read an uncompressed packet. VERIFIED live. The probe's userinfo is the server's own test-client string (`connect "\cg_predictItems\1\cl_punkbuster\0\cl_anonymous\0\color\4\head\default\model\multi\snaps\20\rate\5000\name\bot%d\protocol\%d\qport\%d"`, in both binaries) with `rate` 25000, `name`, `protocol`, `challenge` and `qport` set. VERIFIED. `connectResponse ` ends in a space: the format is `connectResponse %s` (0x8231410) with `fs_game` empty. VERIFIED.
- **Stats.** After `connectResponse` the client sends seven OOB packets `FF FF FF FF "stats\0" [i16 qport][u8 index][data]`: 1240 data bytes for 0..5, 752 for 6, 0x2000 in all. The server answers each with `statResponse <mask>` (0x823114c), the mask of indices still missing: 126 after index 0, 0 after 6. VERIFIED live, all-zero stats accepted (the stock scripts then kick the client; layout and a fresh profile's contents in `cod4-netfields.md` 10). The console logs `Received packet %i of stats data` (0x823079c) per packet. VERIFIED. The 1.7 server took this layout and the 1240-byte split, which VERIFIES both on 1.7.
- **Gamestate trigger.** The gamestate goes out only once all seven stats packets are in, and only in answer to a netchan message: an empty message (the 9-byte header alone) at 63 ms brought the first fragment 6 ms later. VERIFIED live. A netchan message before any stats gets `requeststats` (0x82310a8). INFERRED from `SV_SendClientGameState` (not triggered here).
- **Resend.** A later empty message whose `messageAcknowledge` passes the gamestate's sequence, with the stale `serverId` 0, makes the server send the whole gamestate again. VERIFIED live (a run that acked each new sequence got the gamestate every 3.15 s, six times in 20 s). The probe holds its ack at the gamestate's sequence.
- **Rate.** On loopback, with the probe's `rate` 25000 and the default `sv_maxRate` 5000, fragments came 350 ms apart after the first two. VERIFIED live. CoD1 skips rate pacing for LAN addresses; 1.7 paced these fragments. A loopback client's whole messages still go a frame apart whatever their size: the walker's 359-byte spawn frame and its next snapshot went out 50 ms apart at 5000. VERIFIED live (cod4-pmove.md 15). That `SV_SendMessageToClient`'s LAN arm gives that spacing is INFERRED. Why 350 ms and not the 271 ms Q3's rate formula gives for 1310 bytes at 5000 is not settled. Off the LAN the second fragment waits a second (section 15).
- **Idle messages.** After the gamestate, with the client on the old `serverId`, the server sends a 9-byte datagram (sequence, `reliableAcknowledge`, one block byte decoding to `svc_EOF`) about every 50 ms. VERIFIED live. In `handshake-1.7-cmds.txt` one such message went out as sequence 1 during the stats phase, and the gamestate followed as sequence 2. VERIFIED live.

## 4. Netchan, scramble and Huffman

- **Netchan.** CoD2's. Server to client: a 4-byte sequence, bit 31 marking a fragment; a fragment carries a 4-byte start and a 2-byte length; fragments are 1300 bytes, the last shorter. VERIFIED live: `Netchan` with `NetchanParams::COD2` reassembles the gamestate (`handshake-1.7.txt`: 9 x 1300 + 1240 = 12940 bytes). Client to server: 4-byte sequence, 2-byte qport, then the 9-byte plain header (`serverId` byte, `messageAcknowledge`, `reliableAcknowledge`). VERIFIED live (the server took the probe's 15-byte packets for the client on that qport).
- **Scramble.** CoD2's on the server's side: the first 4 bytes (`reliableAcknowledge`) plain, the rest XORed from a key seeded `challenge ^ sequence` (low byte) that walks the last client command, here empty. VERIFIED live: `Netchan::decode` with an empty key string gives a block that Huffman-decodes cleanly. This capture's client messages were all empty, so it exercises neither the walk over a non-empty command nor the client-to-server key (`challenge ^ serverId ^ messageAcknowledge`); both are covered since (section 6).
- **The Huffman code is not CoD2's.** The weight table is 256 little-endian words at 0x82287a0 (file offset 0x1e07a0), starting 274054, 68777, 40460; the same 1 KB sits in `iw3mp.exe` at 0x6bbc10. VERIFIED. The CoD1/CoD2 `MSG_HDATA` (250315, 41193, ...) is in neither binary. VERIFIED.
- **Server tree.** CoD1 and CoD2 feed the weights through the adaptive `Huff_addRef`; 1.7's tree is static. `cod4_lnxded` never sorts: the function at 0x812af70, called by the init that prints `Huffman Took %d Milliseconds` (0x8130a5a, string 0x82246ed), stores all 513 nodes of the finished tree as constants, 20 bytes each `{first, second, parent, weight, symbol}`. The NYT leaf (symbol 256, weight 0) is at state +0x414, leaf `s` at +0x428 + 20 `s`, the internal nodes (symbol 257) after them, the root at +0x2c14 behind the state's word at +8. VERIFIED. `tools/re/cod4_huffman.py` replays those stores and prints every symbol's code; the leaf weights are the table's, and `net::huffman::CODES` is its output, a step to a node's first link reading 0. VERIFIED (every capture decodes and re-encodes through it, cod4-netfields.md 7). The NYT's 11-bit code `10111101100` ends a decode. The codes form a Huffman code for the weights plus the NYT (`codes_are_a_huffman_code_for_the_weights`). VERIFIED. Tied weights (155 and 205 at 3889, 228 and 231 at 4683) take the server tree's codes; a build from the weights would have to reproduce `iw3mp.exe`'s sort to match.
- **Client tree.** `iw3mp.exe` builds its tree at run time (0x504ef0): the 256 leaves in symbol order, `qsort` (0x671320) by weight alone (comparator 0x504ed0 subtracts the two weights), the NYT (made at 0x504e60) merged with the first, then each round a `qsort` of the rest and the first two merged, lighter first. VERIFIED (the constants and calls). That this unstable sort lands on the server's tree is INFERRED: a stock 1.7 client plays on the Linux server. Every server message in the captures decodes and re-encodes through the server's codes (cod4-netfields.md 7). VERIFIED live.
- **Bit reads.** After decompression `MsgReader` reads unchanged: bytes off the byte cursor, single bits and the 12-bit index off the bit cursor. VERIFIED live: the configstring block parses to its count with every string intact.

## 5. The gamestate

`handshake-1.7.txt`'s gamestate is 12940 bytes reassembled, 12936 after the plain `reliableAcknowledge`, 12730 decompressed. VERIFIED live.

```text
long  reliableAcknowledge (plain)      ; 0
<svc_serverCommand ops>                ; none here; 2 in handshake-1.7-cmds.txt
byte  1                                ; svc_gamestate
long  serverCommandSequence            ; 0 (2 in the -cmds run)
byte  2                                ; svc_configstring, once
short count                            ; 479 (0x01df)
count x { bit 1 = previous + 1, or bit 0 + 12-bit index; BigString }
byte  3                                ; svc_baseline, at decompressed byte 11715
...                                    ; baselines: not decoded
```

- The `-cmds` run's server commands are op 4, long sequence, string: `v ui_3dwaypointtext "1"` and `v ui_deathicontext "1"`. VERIFIED live.
- One configstring op: a short count, then per string a 1-bit "next index" flag or a 12-bit index, then the string. 42 of the 479 strings carry an explicit index. VERIFIED live.
- Whole ranges are missing: models 933..1150, every sound alias (nothing from 1220 to 1598), gaps among the localized strings. Some slots go out as explicit empty strings (`cs 350`, `351`, `852`, `896`, `897`, `1993`, `1994`, `1996`, `2101..2110`, `2271..2273`). VERIFIED live. See "Configstring defaults".
- The table's content column is VERIFIED live. Its last column, the slot's range, is INFERRED.

| Index | 1.7 content | Name |
|---|---|---|
| 0 | serverinfo (`shortversion\1.7`) | serverinfo |
| 1 | systeminfo: `sv_serverid\16`, `sv_iwds`/`sv_iwdNames` (14 iwds), `sv_referencedFFNames\code_post_gfx_mp mp_crash common_mp` and their checksums, `sv_clientSideBullets\1`, `sv_voiceQuality\3` | systeminfo |
| 2 | `cod` | game version |
| 9 | `500 0.000198042 0.501961 0.501961 0.45098 0` | fog |
| 11 | `619800` | level start time |
| 12 | `429.000000 126.500000 137.000000` | map centre |
| 19 | `\winner\0` | |
| 36..124 | dvar names (`friction`, `jump_height`, `player_sprintTime`, `ui_hostname`, ...) | codinfo names (20-147) |
| 148..252 | their values (`11`, `0.0075 0.0075`, `faction_128_usmc`, `CoD4Host`, ...) | |
| 310..381 | localized string names (`MP_HALFTIME`, `QUICKMESSAGE_*`, `OBJECTIVES_WAR_HINT`) | localized 309-820 |
| 821 | `n\ambient_crash\t\0` | ambient |
| 822 | `0` | north yaw |
| 823 | `"compass_map_mp_crash" 2735.000000 2528.000000 -1959.000000 -2166.000000` | minimap |
| 824, 825 | `"mp_crash" 2000`, `"default_night" 1000` | vision sets |
| 827 | `map_artillery_selector` | |
| 831..1219 | xmodels (`defaultweapon`, the sedans, `body_mp_*`, `head_mp_*`, `vehicle_mi24p_hind_desert`) | models 830-1341 |
| 1599..1630 | effects (`fire/jet_afterburner`, `explosions/tanker_explosion`) | effects 1598 |
| 1955..1957 | `flashbang`, `frag_grenade_mp`, `damage_mp` | shellshocks 1954 |
| 1970..1990 | menus (`callvote`, `scoreboard`, `class_marines`, `quickresponses`) | menus 1970 |
| 2003..2013 | materials (`faction_128_usmc`, `headicon_opfor`) | materials 2002 |
| 2258 | the weapon list, space-separated, `defaultweapon_mp` first, `cobra_20mm_mp` last | weapon files 2258 |
| 2259, 2260 | `hud_status_dead`, `hud_status_connecting` | status icons 2259 |
| 2267..2273 | `headicon_american`, `faction_128_arab`, `talkingicon`, three empty | head icons 2267 |
| 2314 | `ffffffffffffffffffffffffffff3000` | items 2314 |

Every index in the capture sits inside its kind's range; the highest is 2314. VERIFIED live.

The range bounds come from the indexers in `cod4_lnxded`. VERIFIED (the constants).

| Kind | First slot / count | Where |
|---|---|---|
| sound aliases | 1342 / 256 | `G_FindConfigstringIndex` (0x80cae70, overflow string 0x82200d0) called at 0x80cb114 |
| effects | 1598 / 100 | 0x80cb17f |
| effect tags | 1698 / 256 | 0x80bdfa5 |
| shellshocks | 1954 / 16 | 0x80cb14a |
| materials | 2002 / 256 | 0x80cb202 |
| localized strings | 309 / 512 | 0x80cb266 |
| tags | 2282 / 32 | 0x80cb1b2 |
| models | index + 830 (0x80cac45), 512 (0x80cad3d, `G_ModelIndex: overflow` 0x821ff06) | |
| menus | scan 1970 up to 2002 | 0x80bcc6c |
| head icons | scan 2267 up to 2282 | 0x80bc62e, 0x80bc664 |
| status icons | 2259..2266, unrolled | 0x80bc95d..0x80bca91 |
| mirrored dvars (flag 0x100) | names 20..147, values 148..275 (128 each, value 128 past its name) | 0x8175017..0x817502e, again 0x8176f7b; `SV_SetConfigValueForKey` 0x81741c8, value slot 0x8174378, overflow `Overflow at config string start value of %i` 0x8232054 |
| `callvote` time (`%i %i`), string, yes, no | 13..16 | 0x80ae074..0x80ae0d2 |
| worldspawn `message` / `g_motd` (dvar named at 0x8219cf9) | 3 / 10 | 0x80c82a5, 0x80c82bd |

`SV_SetConfigstring` (0x8173e54) refuses an index above 2441 (compare at 0x8173e60, `SV_SetConfigstring: bad index %i` at 0x82320d4) and `SV_SendClientGameState` (0x816e10a) loops to 2442, so `MAX_CONFIGSTRINGS` is 2442. VERIFIED. The hint strings are under "Configstring defaults".

### Configstring defaults

- `constantConfigStrings` is a table of 16-byte rows `{int slot; char *string; int hash; int lowercaseHash}`, ascending, ended by a row of slot 0: `cod4_lnxded` 0x826e4c0, `iw3mp.exe` 0x7209b0, `cod4_lnxded-bin` 0x826e320. Each holds 832 rows, slots 20 to 2273; the three are equal row for row and the hash words are 0 in the files. VERIFIED (`cargo run -p vcod-cod4 --example cod4_constcs -- <cod4_lnxded> <iw3mp.exe>`).
- The rows cover dvar names 20..108 (`bg_aimSpreadMoveSpeedThreshold` .. `stopspeed`), localized strings 350..351 and 390..741, models 852..910 and 933..1150 (`weapon_*`), menus 1968..1996, materials 2020..2125, and 2271..2273 (a weapon list and the two status icons). VERIFIED.
- `SV_SendClientGameState` (0x816e10a) compares a slot past 820 with its default case-blind: the `cmpl $0x334` at 0x816e2e2 and 0x816e3f2 picks 0x81aa3b2 over `strcmp`. `SV_SetConfigstring` (0x8173e54) interns a string past 820 through a different string call (0x8173eca). VERIFIED (the constants). The rule is INFERRED (the branches).
- In `handshake-1.7.txt` no slot holds its own default (from 821 compared case-blind, below exactly), and every empty string sits on a slot with a default. VERIFIED live (`configstring_defaults_fill_the_slots_left_out`). So a slot left out holds its default and an empty string clears one. `Gamestate::configstring` fills the rest from `net::constant_cs`.
- 1.7's server keeps its weapon list at 2258 and status icons at 2259..2260 and clears 2271..2273, where the table puts them. VERIFIED live.
- Hint strings are 32 slots from 277: the hint index stored on an entity (+0xd4) is the first of `0x115 + i` holding the string or empty (0x80c6270, `i` below 0x20 at 0x80c6267); a 33rd string is `Too many different hintstring values. Max allowed is %i different strings` (0x821f224). VERIFIED (the constants).
- The name indexers start one past their block's base, so the base slot stays empty: `G_FindConfigstringIndex` (0x80cae70) scans from 1 (0x80caeb6), `G_ModelIndex` (0x80cac52) from 1 below 512 (0x80caccc..0x80cace6). VERIFIED (the constants). vcod-server's `CS_LAYOUT_COD4` starts those blocks there.

## 6. Not decoded, and what is still open

Closed since this capture:

- Baselines, `svc_EOF`, `clientNum` and `checksumFeed` (`cod4-netfields.md` 4).
- `crates/cod4/examples/cod4_probe.rs` sends `sv_serverid` back in keyed move messages and captures snapshots (`cod4-netfields.md` 5, 6).
- Both scramble directions over non-empty commands (`cod4-netfields.md` 5, 8).
- `constantConfigStrings` from both binaries (section 5, "Configstring defaults").
- `requeststats` and the authorize round trip off the LAN: section 15.
- The pure check: section 8.

Still open: the rate pacing on loopback (section 3) and why the first two fragments go out together.

## 7. Recipe

With the symlink tree of section 1 and the server's console on stdout:

```sh
unshare -rnm bash -c '
  mount --bind <hosts file mapping cod4master.activision.com to 127.0.0.2> /etc/hosts
  mount -t tmpfs none /run/systemd/resolve
  ip link set lo up
  cd <tree> && LD_LIBRARY_PATH=. ./cod4_lnxded +set fs_basepath <tree> +set fs_homepath <home> \
    +set net_ip 127.0.0.1 +set net_port 28960 +set dedicated 1 +set sv_punkbuster 0 \
    +set logfile 2 +set sv_maxclients 8 +map mp_crash &
  sleep 35
  python3 -I <checkout>/tools/cod4_handshake_probe.py 127.0.0.1:28960 out.txt --auth 127.0.0.2
  kill $!'
cargo run -p vcod-common --example cod4_capture -- out.txt --strings
```

## 8. The pure check

`sv_pure 1` is the retail default. The probe answers it as the client does and holds a pure server's world (`play-1.7-kill-*.txt`, `cod4-netfields.md` 10).

- **Iwd checksum.** `sv_iwds` lists one number per non-localized iwd, in `sv_iwdNames` order (`iw_13` first): the CRC-32 (reflected, polynomial 0xedb88320, `~0` in and out) of the CRC-32 words of the iwd's non-empty entries, little-endian, in central directory order. VERIFIED: the install's `iw_00..13.iwd` give the 14 numbers in `handshake-1.7.txt` (`iwd_checksums_match_the_server`). It is `Com_BlockChecksumKey32` with key 0; the function name is INFERRED. CoD1 and CoD2 take an MD4 over the same words (`vcod_common::pak_checksum`); 1.7 does not.
- **Pure checksum.** The same CRC seeded with the gamestate's `checksumFeed` (`~feed` in place of `~0`), as `FS_LoadZipFile` keys it. VERIFIED live: the shooter's `cp` carried the 14 numbers keyed that way and the server kept it in the world; the test above checks the capture's `cp` against the install.
- **Command.** `CL_SendPureChecksums` (`iw3mp.exe` 0x46a0f0) writes `"Va "` (0x6d06a4), appends `FS_ReferencedIwdPureChecksums`, then adds 13 and 15 to the first two bytes, so the reliable command reads `cp @ <sum> ... <key> `. VERIFIED (the two `add BYTE PTR` at 0x46a125 and 0x46a12a). The `@`, the `"%i "` numbers and the key (`checksumFeed` XOR every sum XOR their count) follow `FS_ReferencedIwdPureChecksums`. INFERRED. The server's check below VERIFIES the key's shape on 1.7.
- **Server.** `cp` is the first row of the client command table at 0x826d398 (`"cp"` at 0x82314af, handler 0x817059c). The handler needs `@` as argument 1 (0x81705c4), parses the rest with `strtol`, refuses a repeated sum (0x8170656), loads its own list from `FS_LoadedIwdPureChecksums` (0x817067e), refuses a sum not in it (0x8170719), then XORs `checksumFeed` (0x13e77f94) with the sums and the count and compares the last argument (0x817074d). A pass sets `pureAuthentic` (client offset 0x804fc) to 1, a failure to 2. INFERRED (read off the handler). It reads no fastfile checksum; `sv_referencedFFCheckSums` serves only the client's own check. INFERRED (no FF read on that path).
- **No sum needed.** `cp @ <checksumFeed>` (count 0) passes the same test. VERIFIED live: the kill capture's target sent only that and stayed in.
- **Without `cp`.** `SV_UserMove` (0x81727b8) drops a client whose `pureAuthentic` is still 0 on a pure server with `EXE_CANNOTVALIDATEPURECLIENT` (0x8231470), past the `CS_PRIMED` arm; one at 2 goes with `EXE_UNPURECLIENTDETECTED` (0x823148d, check at 0x8172a6d). INFERRED (the branches). The first drop is VERIFIED live: a `--no-cp` probe beside the pure one got `w "EXE_CANNOTVALIDATEPURECLIENT"` as its first server command after the gamestate, and the console printed `0:nocp EXE_CANNOTVALIDATEPURECLIENT`. The server drops such a client; it does not hold it out of the world and resend the gamestate.
- `net::pure` holds the checksum, the command (`pure_command` reads an install's `main/`) and the server's check (`verify_cp`). `cod4_probe` sends `cp` as reliable command 1 on the first gamestate (`--iwds <main>` for the real sums, the bare key without it).

## 9. What vcod-server's 1.7 server sends

`vcod-server <map> --game cod4` (`crates/server/src/cod4/`) answers the out-of-band half of `handshake-1.7.txt` byte for byte, except the challenge's value, two `rand()` draws off the level's stream (`cod4::oob_ab`, cod4-combat.md 10.9). The capture holds one challenge. One draw pair at an unknown `svs.time` matches about one seed in 256 (64 draw offsets times 2^18 values of `svs.time` over 2^32), so even a minute of wall-clock milliseconds holds some 230 candidate seeds and `recover_game_seed` cannot pin this run's; the gate compares the reply's prefix and form, and `cod4::wire_ab::the_challenges_come_off_the_levels_rand` checks the draws on the walk run's recovered seed. INFERRED (the search's arithmetic). With an install it runs the level (`cod4-gsc.md` 7): the gamestate carries the script's configstrings, the dvar mirror and the baselines of the script's entities, and a client connects, picks a team and class and spawns as on retail (`cod4::wire_ab`). Building that gamestate brought out these facts:

- Configstring 1 ends `\fs_game\\` (two backslashes) with `fs_game` empty, after `timescale`, on all four captures. VERIFIED live. `SV_SaveSystemInfo` appends `\fs_game\` explicitly when the dvar is empty. INFERRED.
- A pure-off server's configstring 1 has no `sv_iwds` or `sv_iwdNames` key (`play-1.7-spec.txt`, `sv_pure\0`). VERIFIED live.
- Configstring 11 reads `619800` on three captures from two server runs with different checksum feeds, so it does not follow the server's clock. VERIFIED live. Slot 11 is `CS_GAMEENDTIME`: 619800 is 19800 plus `war`'s 600000 ms time limit, the script's game end time, not a level start time. INFERRED.
- Configstring 12, the map centre, is set by the script through `GScr_SetMapCenter`. INFERRED. vcod's level writes it through `setMapCenter`. The box centre of mp_crash's `mp_tdm_spawn*` origins is x 429, y 126.5, the capture's x and y; its z is 171 where the capture reads 137. VERIFIED (the zone's entities against `handshake-1.7.txt`). The z gap fits `_spawnlogic.gsc` dropping each spawn point to the ground before the box. INFERRED.
- `SV_SendClientGameState` leaves out a slot holding its `constantConfigStrings` default, sends an explicit-index empty string for a slot with a default that holds `""`, and sends a slot with no default only when non-empty. INFERRED. The table starts all empty at each spawn (`SV_SpawnServer`). INFERRED. So vcod's bare level, with no script to precache, sends all 832 default slots as explicit empties. The retail client and `vcod_cod4::net` read that back cleanly. VERIFIED (the join gate below).
- `SV_DropClient` tells a client in the world its reason as `w "<reason>"`, one still loading as an out-of-band `disconnect <reason>`, and everyone else `e <name>^7 <reason>` and `K <slot>`. INFERRED. Only the `w` is VERIFIED live (section 8).
- 1.7's `SV_DropClient` (0x817096e) quotes the others' line: `%c "\x15%s^7 %s%s"` (0x8231236) with 0x65, the name, `\x14` (0x8231227) when `SEH_StringEd_GetString` (0x817f4b0) finds the reason, else `""` (0x822b5d8), and the reason (`EXE_LEFTGAME`, 0x8231229, for `EXE_DISCONNECTED`). VERIFIED (disassembly at 0x8170a12..0x8170a54 and 0x8170b55..0x8170b82). The cgame's `e` reads only its first argument (`CG_DeployServerCommand`, `iw3mp.exe` 0x44be40). INFERRED. Trap: the unquoted `%c %s^7 %s%s` shows a kicked `[BOT] Major Lag` as `[BOT]` in the kill feed. VERIFIED (the live test below, run on the unquoted line). The lookup goes through the localize assets (`DB_FindXAssetHeader` type 0x16, 0x817f36f). The dedicated server's only localized zone is `localized_code_post_gfx_mp`, so vcod marks a reason that zone holds (`Cod4Server::code_string`). INFERRED (the server install's `zone/english` lists no other localized zone). `cod4_live_kick_message_keeps_the_whole_name` kicks a bot through the console and reads the client's message through the HUD's localizer.
- Reconnects. `SV_DirectConnect` (0x8171eb3) frees a reconnecting client's old slot when its state is above `CS_ZOMBIE`, through the call `SV_DropClient` makes after its `Going to CS_ZOMBIE` line (0x81709cd, `SV_FreeClient`: `ClientDisconnect` and `CodeCallback_PlayerDisconnect`); a zombie's ran at its drop. VERIFIED (the disassembly; the `%s:reconnect. same qport` print is 0x8171e9d). vcod does the same. Skipping it leaves the old player in `level.players` beside the new one, and the stock scripts err on it every frame (`cod4::disconnect_tests`).
- Pacing follows section 3. A snapshot leaves out what `hide` hid and culls nothing by position (cod4-pmove.md 15).

Gates:

| Gate | Checks |
|---|---|
| `cod4::oob_ab` | the replies, configstrings 0, 2 and the mirror against the capture after 30 s of the level; with `$COD4_DIR` the slots the running level writes too: 1, the fog (9), the game end time (11), the map centre (12) and `\winner\0` (19), every slot below the localized strings retail holds |
| `cod4::wire_ab` | three clients join `war` in process (`cod4-gsc.md` 7); the walker replays its captured cmds (cod4-pmove.md 15.1) and every snapshot's HUD elements (cod4-hud.md 5.1) |
| `cod4::prematch_ab` | |
| `cod4::disconnect_tests` | clients leave, one reconnects into its live slot; no script error |
| `cod4::join_tests` | a `vcod_cod4::net::conn` client joins a bare level on loopback; every datagram decodes and re-encodes; no `cp` on a pure server reads `w "EXE_CANNOTVALIDATEPURECLIENT"` |
| `COD4_REENCODE=<capture> cargo test -p vcod-cod4 --test cod4 an_env_named` | a `cod4_probe` run against the binary |

## 10. A server that drops us or goes away

- **Timeout.** `CL_CheckTimeout` (0x46c590) counts a frame in 0xc5f934 when `cls.realtime - clc.lastPacketTime` passes `cl_timeout` seconds (`cls.state`, 0xc5f900, 8 and on) or, in states 4..7, `cl_connectTimeout`. Any other frame clears the count; the sixth errors `EXE_ERR_SERVER_TIMEOUT` (0x6d0e10). VERIFIED (0x46c5b5..0x46c672). `cl_timeout` registers at 40 s and `cl_connectTimeout` at 200, both in 0..3600 (`CL_Init`, 0x46d75d, 0x46d80e; floats at 0x6c6304 and 0x70b774). VERIFIED (the code). CoD2's `cl_timeout` was 200 (cod2-protocol.md 18.2).
- **`w`.** `CL_CGameNeedsServerCommand`'s `w` (0x45b1d9): with a third argument `PB`, `EXE_SERVERDISCONNECTREASON` around the second; with no argument `EXE_SERVER_DISCONNECTED`; else `CL_DisconnectError` (0x46b4e0), the same wrap of the translated reason. Each is a `Com_Error` that leaves the server. VERIFIED (the code). vcod-server's `quit` (and Ctrl-C) sends each client `w "EXE_SERVERQUIT"` (`drop_client`).
- **OOB `disconnect`.** `CL_DisconnectPacket` (0x46b510) honours one only from the server's address, past state 3 and 3000 ms (0xbb8) after the last sequenced packet: its reason through `CL_DisconnectError`, or `EXE_SERVER_DISCONNECTED`. VERIFIED (the code).
- **vcod.** `Cod4Session::take_dropped` reports all three drops; `localize_drop` reads the reason from the install's strings. The client logs it and returns to the CoD4 main menu with the reason in `error_popmenu` (`App::leave_cod4_game`). Gates: `cod4_live_w_drops_with_its_reason`, `cod4_live_silence_interrupts_then_times_out` (the second with `timeout` cut to 3 s).

## 11. Downloads

Read off `cod4_lnxded`'s and `iw3mp.exe`'s disassembly and measured live with `cod4_probe --download` against retail serving a mod (`+set fs_game mods/modwarfare`, `sv_pure 1`, `sv_allowDownload` 1 and 0). The raw captures hold the mod's bytes and stay out of the tree; `crates/cod4/tests/fixtures/net/download-1.7.txt` is the decoded message list (block numbers, sizes and lengths, the commands, the refusals).

### The server

- **Commands.** The client command table at 0x826d398 holds, after `cp` and `vdr`, `download` (handler 0x8172068), `nextdl` (0x81721c6), `stopdl` (0x81723d2), `donedl` (0x816fd00), `retransdl` (0x81707aa) and `wwwdl` (0x8170ffa). VERIFIED (the table).
- `download <name>` closes any transfer and copies the name, 64 bytes with its NUL. VERIFIED (0x8172068). `nextdl <n>` drops the client with `broken download` (0x8231452) unless `n` is the block due (+0x212f0); the block due past an empty block closes the transfer (`clientDownload: %d : file "%s" completed`), any other moves the window on. VERIFIED (0x81721c6). `donedl` closes the transfer and jumps into `SV_SendClientGameState`. VERIFIED (0x816fd00..0x816fd5d).
- The gamestate `donedl` brings needs its own `cp`. A probe that sent `cp` on the first gamestate and moved after `donedl`'s without one was dropped with `EXE_CANNOTVALIDATEPURECLIENT`; one that answered the second gamestate with `cp` again entered the world. VERIFIED live.
- **Dvars.** Server: `sv_allowDownload` 1, `sv_wwwDownload` 0, `sv_wwwBaseURL` "", `sv_wwwDlDisconnected` 0 (0x8173319, 0x81738c9..0x817393a). Client: `cl_allowDownload` and `cl_wwwDownload` both 1 (`iw3mp.exe` 0x46e251, 0x46e2b8). VERIFIED (the code). `cl_wwwDownload` registers with flags 3 (archive and userinfo, 0x46e304), so the server reads the client's www flag from the userinfo. VERIFIED (the code).
- **Order of checks.** `SV_WriteDownloadToClient` (0x816f774), on its first call for a name, in order. VERIFIED (the code).
  1. `sv_allowDownload` off (0x816f7b4): refuse with `EXE_AUTODL_SERVERDISABLED_PURE` (0x8230904) when `sv_pure` is set (0x816f7f5), else `EXE_AUTODL_SERVERDISABLED` (0x82311b5).
  2. `EXE_CANTAUTODLGAMEPAK` (0x82311d2) for a name `FS_iwIwd(name, "main")` (0x81286c2) passes.
  3. Open the file; `EXE_AUTODL_FILENOTONSERVER` (0x82311eb) when missing or empty.
  4. With `sv_wwwDownload` on and the client's www flag (+0x21444), the redirect.

  CoD2's server tests the redirect before the refusals; 1.7 does not.
- `FS_iwIwd` is CoD2's: `<base>/iw_00` .. `iw_24` compared whole, or a `localized_` name holding `_iw00` .. `_iw24` (`%s/iw_%02d`, 0x8224396). VERIFIED (the code). So `main/iw_00` is refused but `main/iw_00.iwd`, the name a client sends, is served: the probe fetched 131 blocks of it before it stopped. VERIFIED live.
- **Refusal.** `svc_download` (5), a short 0, a long -1 and `<key>\x15<name>` (0x816f5f2). VERIFIED live: every refusal of the three kinds read that way, `EXE_AUTODL_SERVERDISABLED_PURE\x15mods/modwarfare/z_modwarfare.iwd` among them. The client reads the block as a long (`CL_ParseDownload`, `iw3mp.exe` 0x474440; `MSG_ReadLong` 0x5056f0, `MSG_ReadShort` 0x505690), so it sees -65536, then a block size of -1, logs `Expected block 0, got -65536` and leaves the rest of the message to the op loop. INFERRED (read off the code; no retail client ran). `vcod_cod4::net::download` reads the server's shape whole.
- **Block.** `svc_download` (5), the block number as a long, on block 0 the file's size as a long, the block's size as a short, then at most 2048 bytes (0x816fb84..0x816fbdd); an empty block past the last byte ends the file. Q3 writes the block number as a short. VERIFIED live: the mod's 178745-byte iwd came as blocks 0..87 of 2048 bytes, block 87 of 573 and an empty block 88, byte for byte the server's file.
- **Pacing.** Eight blocks in flight (the `cmp 7`s at 0x816f8e7, 0x816f93e); a resend from the last acknowledged block after 1000 ms without a `nextdl` (0x816fb5f). Blocks per message are the client's rate (capped by `sv_maxRate` past 999) times its snapshot interval over 2048, rounded up past a whole one (0x816fa21..0x816fa93). VERIFIED (the code). At `rate 25000`, `snaps 20` and `sv_maxRate` 5000 that is one block a message. Each 2 KB message went as two fragments 50 ms apart, the next message 250 ms after the first, so the iwd took 22 s (about 8 KB/s) on loopback. VERIFIED live.
- **Redirect** (`SV_WWWRedirectClient`, 0x816f674): `svc_download` (5), a long -1, the URL (`%s/%s` of `sv_wwwBaseURL` and the file, 0x820e5d6), the file's size as a long and `sv_wwwDlDisconnected` as a long. The console prints `Redirecting client '%s' to %s` (0x82308b8); the file is closed and the name cleared. VERIFIED (the code). After a failed www transfer (+0x21450) the next message writes nothing and clears the flag; blocks start on the one after. VERIFIED (0x816fc72). The client answers `wwwdl ack`, `bbl8r`, `done` or `fail` (`iw3mp.exe` strings 0x6d2914, 0x6d28e8, 0x6d2520, 0x6d2554). VERIFIED (the strings).

### What the client asks for

- `sv_referencedIwdNames` listed `main/iw_11 mods/modwarfare/z_modwarfare`; `sv_referencedFFNames` `code_post_gfx_mp mp_crash common_mp mods/modwarfare/mod`, with `sv_referencedFFCheckSums` `89196 35287381 13517381 34310`: the fastfiles' sizes (mod.ff came as 34310 bytes). VERIFIED live.
- The client fetches each referenced iwd under `fs_game` whose checksum it lacks and each referenced fastfile there whose size differs, as `<name>.iwd` and `<name>.ff`, never a `main` iwd (`server_file_compare`). INFERRED. The probe's `auto` list, built by that rule, was `mods/modwarfare/z_modwarfare.iwd` and `mods/modwarfare/mod.ff`, and retail served both. VERIFIED live.
- **Fastfile compare** (0x5038f0) walks `sv_referencedFFNames` and the sizes. INFERRED (read off the code).

  | Name | Local path |
  |---|---|
  | starts `mods` (`Q_strnicmp` 4, 0x50395b), longer than `fs_game` plus one | `<base>\<fs_game>\<rest>.ff` (`%s\%s\%s.ff`, 0x6d385c), rest past `fs_game` and its slash |
  | starts `usermaps` (8) | `%s\%s\%s\%s.ff` (0x6d387c) |
  | any other (stock) | `%s\zone\%s\%s.ff` (0x6d3848) |

  The size is `GetFileSize` of that file (0x48b940). A mod or usermap mismatch goes on the list as `@<name>.ff@<name>.ff`, remote and local names the same (`@` at 0x6e2840, `.ff` at 0x6e2888), printed with `(local file exists with wrong filesize)` (0x6e288c) when the file is there; a stock mismatch returns 2 (0x503b93). The name is used as it came: nothing escapes it or strips a colour code on the way to the request or the file. INFERRED (no string or escape call between the list and `DL_BeginDownload`; libwww sends the path as given).

### The client's www half

- `CL_ParseDownload`'s block -1 (`iw3mp.exe` 0x474270..0x474439) reads the URL into `cls.downloadName` (0xc5b188), then the size (0x14b8c30) and the flags (0xc5b6a8) as longs. VERIFIED (the code).
- Flags bit 2 (`test al,0x2` at 0x474308) sends `wwwdl bbl8r` (0x47432c) and `quit` (0x6d28e0) after a call at 0x5769e0, the Q3 lineage's `Sys_OpenURL`. INFERRED (the branch; the call's role from the lineage).
- Otherwise it logs `Server redirected download: %s`, sends `wwwdl ack`, sets the www download flag (0xc5b6a4) and starts the fetch into the OS path of the requested name under `fs_homepath` (0x474381..0x4743db). A fetch that does not start sends `wwwdl fail`, clears the flag and logs `Failed to initialize download for '%s'` (0x4743e7..0x47440b). Then flags bit 1 (`test 0x1` at 0x474413) sends `wwwdl bbl8r` and sets the www disconnect flag (0xc5b6a0). INFERRED (the branches).
- The fetch is libwww's (`DL_BeginDownload` 0x500ae0, `HTLoadToFile failed` at 0x6e20e0) on an `http` URL, with a 30000 ms event timeout (0x500b61) and `user:pass@` parsed into basic auth (0x500b8b..0x500c8b). VERIFIED (the strings and the timeout constant).
- `CL_WWWDownload` (0x4729c0) runs it each frame through `DL_DownloadLoop` (0x500e10). INFERRED (the branches):
  - Done (1): the spool renamed into place (0x472a4b; a copy and a delete when the rename fails), the names cleared, then `reconnect` (0x6d2514) if disconnected (unless 0x8f4ccc is set) or `wwwdl done` if connected; either way the flag cleared and the next file asked for (0x46aba0).
  - Failed, disconnected: a disconnect and `Download failure while getting '%s'` as an error (0x472b01..0x472b16). Failed, connected: that line printed, `wwwdl fail`, the flag cleared (0x472b28..0x472b3f).
- The server answers `fail` with the gamestate and the client asks for the file again on it, over UDP. INFERRED (the server's fallback flag is consumed by the next `download`; no retail client ran against an HTTP server).

### No file check

- Neither end checks a file. `CL_WWWDownload`'s done arm renames the spool (0x472a4b, `rename` at 0x671ac6) and asks for the next; `CL_ParseDownload`'s empty block closes the file, renames it (0x474614) and does the same (0x474640). VERIFIED (the code).
- The only check is `CL_InitDownloads` (0x46ac60), called by `CL_ParseGamestate` at the end of every gamestate (0x47423e). VERIFIED (the call). It calls the compare (0x503ba0) with the download flag from `cl_allowDownload`. Off: a missing file ends the join with `\x15You are missing some files referenced by the server` (0x6d0910). On: the list goes out as `Need files: %s` (0x6d09a4) and the first `download` starts. A stock fastfile that differs ends it with `\x15%s is different from the server` (0x6d0980). INFERRED (the branches). So a file that still differs after its download is asked for again on `donedl`'s gamestate, without end. INFERRED.

### vcod

- **Server** (`crates/server/src/cod4/download.rs`) serves a `.iwd` or `.ff` under its install root at the name asked for, checks in the order above, the blocks in the snapshot message after the snapshot. It serves no other kind (retail serves any file). Its `wwwdl` drop texts are the strings at 0x6e6768 and 0x6e67c4 in `iw3mp.exe`'s server half, INFERRED as the reasons.
- **Client** (`vcod_cod4::net::download::Downloads`) writes into the install root at the referenced path and keeps the file unchecked, as 1.7 does. It compares again on every gamestate and ends the join when a file it landed is asked for again (`<name> is different from the server after its download`; retail fetches it again on each gamestate). On a refusal it ends the join with the reason. It sends `cl_wwwDownload\1` in the userinfo when `cl_wwwDownload` is on.
- **Redirect.** `vcod_cod4::net::http::HttpFetch` fetches it: a plain HTTP/1.0 GET on its own thread with the 30 s timeout while the netchan runs. Connected: `wwwdl ack`, then `wwwdl done` and the next file, or `wwwdl fail` and the file again over UDP on the server's gamestate. Disconnected: `wwwdl bbl8r` and a fresh join once the file lands (`Cod4Session::connect_with`, at most 16 joins). Differences from retail: no `user:pass@` URL, no HTTP redirect followed, and a start that fails with bit 1 set falls back to UDP where retail sends `wwwdl bbl8r` after its `wwwdl fail`.
- **Progress.** The file in flight goes out as `vcod_cod4::net::download::DownloadProgress` (name, bytes, size) through `Fetch::progress` for the connect screen. Each file's last report is its whole size off `DownloadEvent::Finished`, since a loopback fetch can land before a look reads it.
- **Gates.** `cod4_live_www_*` in `crates/client/src/play/cod4.rs`: vcod-server under a generated mod and a loopback HTTP server; the fetch, the fallback after a 404, the disconnected fetch and its reconnects, each ending in a spawn; a slow fetch, on the wall, costs the client's clock nothing. `cod4::download_tests`: a generated iwd and fastfile from a bare level, the join on the next gamestate, the refusal. `blocks_match_the_retail_capture`: the fixture's blocks.
- Live, the client fetched ModWarfare's iwd and `mod.ff` from retail and from vcod-server (`--set fs_game=mods/modwarfare`) on loopback, both byte for byte, the iwd in 22 s off each, and joined.

## 12. The server browser

Read off `iw3mp.exe`'s disassembly and data; addresses are in it. Function names are INFERRED; the facts are the 1.7 binary's. Live checks ran on loopback and in section 7's namespace only, against vcod-server, retail `cod4_lnxded` and a fake master on 127.0.0.1; no public master or server was contacted.

### 12.1 The three lists

- `ui_netSource` (registered at 0x541d5b: int, default 1, 0..2, `The network source where: 0:Local 1:Internet 2:Favourites`) picks the list. VERIFIED (the code). Names `EXE_LOCAL`, `EXE_INTERNET`, `EXE_FAVORITES` (table 0x723df0). VERIFIED.

  | Source | List | Count | Rows | Limit from |
  |---|---|---|---|---|
  | 0 Local | 0x956ec8 | 0x956ec4 | 128 | `MAX_OTHER_SERVERS hit, dropping infoResponse`, 0x470ece |
  | 1 Internet | 0x95bcdc | 0x95bcd8 | 20000 | 0x95bcdc..0xc5585c |
  | 2 Favorites | 0xc55860 | 0xc5585c | 128 | `servercache.dat`'s reader, 0x47645a |

  `UI_StartServerRefresh` (0x54aaf0) and `CL_UpdateVisiblePings`'s pick (0x470380) read them. Each row is a 156-byte `serverInfo_t` (stride 0x9c). VERIFIED (the code).
- `CL_SetServerInfo` (0x470ad0) fills a row from `infoResponse`, each number through `atoi` into a byte unless the table says otherwise. VERIFIED (the code).

  | Offset | Field |
  |---|---|
  | +0 | `netadr_t`, 20 bytes: type (4 for `NA_IP`, 0x471668), IPv4 address at +4, port in network order at +8 |
  | +0x14 | `sv_allowAnonymous` |
  | +0x15 | `pswrd` |
  | +0x16 | `pure` |
  | +0x17 | `con_disabled` |
  | +0x18 | `nettype` |
  | +0x19 | `clients` |
  | +0x1a | `sv_maxclients` |
  | +0x1b | the refresh's mark |
  | +0x1c..+0x22 | `ff`, `kc`, `hc`, `od`, `hw`, `mod`, `voice` |
  | +0x23 | `pb > 0` |
  | +0x24 | read by the cache writer (12.5); what raises it is not found |
  | +0x26, +0x28 | `minping`, `maxping` (shorts) |
  | +0x2a | ping (short); -1 on a new row (0x47172b) |
  | +0x2c | ping send time (0x470455) |
  | +0x30 | set to 1 |
  | +0x31 | `hostname` (32 bytes) |
  | +0x51 | `mapname` (32) |
  | +0x71 | `game` (24) |
  | +0x89 | `gametype` (16) |

### 12.2 The wire

All VERIFIED (the code).

- **Refresh.** `UI_StartServerRefresh` (0x54aaf0) first writes `ui_lastServerRefresh_<source>` as `%s %i, %i   %i:%02i` (0x6eb858): localized month (`EXE_MONTH_ABV_JANUARY`.., table 0x723dc0), day, year, hour, minute. A quick refresh (argument 0) then resets the source's pings (0x475560: ping -1, no send time) and stops. A full one marks every row of the source (+0x1b = 1, 0x54abf5), resets the pings, then Local runs `localservers` (0x6eb86c), Internet `globalservers 0 6 full empty` (0x6eb89c; `globalservers 0 %s full empty` with `debug_protocol` when that dvar is set, 0x54ac4d), Favorites nothing more.
- **`localservers`** (`CL_LocalServers` 0x470220) prints `Scanning for servers on the local network...`, empties the Local list keeping each row's mark, and broadcasts `getinfo xxx` (0x6d1e9c) to ports 28960 (0x7120) to 28963, the whole round twice.
- **`globalservers`** (`CL_GlobalServers_f` 0x471820, `usage: globalservers <master# 0-1> <protocol> [keywords]`) resolves the literal `cod4master.activision.com` (0x6d0554) at port 20810 (0x514a), prints `Requesting servers from the master...` and sends `getservers %s` (0x6d2138) with the protocol and each keyword after a space, plus ` demo` when `fs_restrict` is set. `masterServerName` (same name, `Master server name for listing public inet games`), `masterPort` (20810), `authServerName` and `authPort` (20800) are registered at 0x4febc1..0x4fecbf, but the query reads neither master dvar: nothing loads their pointers (0x1476f04, 0x1476ef4) outside the registrations.
- **Master's answer** (`CL_ServersResponsePacket` 0x4714b0, matched as the 18 bytes `getserversResponse`, 0x46bd96): from the first `\`, four address bytes and a big-endian port per server, each followed by `\`; at most 256 a packet; it stops at `\EOT` or `\EOF` (0x4715c9) or with fewer than seven bytes left. A new address joins the Internet list, which stays sorted; a known one is skipped (0x4713c0).
- **Pings** (`CL_UpdateVisiblePings` 0x4703c0, only while `cls.state` 0xc5f900 is 0): round-robin from a static cursor (0x95bcc8) over the source's rows. A marked row with ping -1 and no send time gets the time and a `getinfo xxx`, up to `cl_maxppf` a frame (0x4704a4; int, default 5, 1..10, registered at 0x46e4df, read through 0x8f4cc8). The refresh is busy while a frame sends one or for 2000 ms (0x7d0) after the newest send; that is the only timeout. `cl_maxPing` (800) is read nowhere in the browser.
- **`infoResponse`** (`CL_ServerInfoPacket` 0x470ce0): a `protocol` other than 6 (or `debug_protocol`) prints `Different protocol info packet: %s` and is dropped (0x470d93). Otherwise the row with that address in the source being pinged takes the info and the ping, now minus its send time (0x470dec), uncapped. On the Local source an unlisted address is added, up to 128.
- **`getstatus`** (0x6d2348, no argument) goes from `CL_ServerStatus` (0x4722ef); the info popup asks again every 500 ms until answered (0x548212).

### 12.3 The join menu

`pc_join_unranked` in `ui_mp` holds the list (feeder 2), owner draws 220 (source), 253 (game type filter) and 247 (refresh line), the column buttons (`uiScript ServerSort <n>`), Refresh List, Quick Refresh, New Favorite (shown when `ui_netSource` is 2), Del. Favorite, Password, Server Info and Join Server; its `onOpen` runs `UpdateFilter`. Its Add to Favorites item has no `WINDOW_VISIBLE` and never draws. The popups are `password_popmenu` (edit field on `password`, 12 characters), `serverinfo_popmenu` (feeder 13), `createfavorite_popmenu` (edit field on `ui_favoriteAddress`), `fav_message_popmenu` (`ui_favorite_message`), `del_fav_popmenu`, `filter_popmenu` and `joinpb_popmenu`. VERIFIED (the zone, `menu_dump`).

- **Scripts** (`UI_RunMenuScript`). VERIFIED (the code).

  | Script | Address | Does |
  |---|---|---|
  | `RefreshServers` | 0x546186 | full refresh of the current source |
  | `RefreshFilter` | 0x5461c4 | quick refresh |
  | `UpdateFilter` | 0x54654e | full refresh of a Local source, rebuild the list, pick its first row |
  | `ServerStatus` | 0x5464dc | ask the picked row's address for its status |
  | `JoinServer` | 0x5465aa | set `cg_thirdPerson` 0; open `joinpb_popmenu` for a PunkBuster server when `cl_punkbuster` is off, else `connect %s` to the picked row; reads no password (the `password` dvar rides the userinfo) |
  | `ServerSort` | 0x546789 | set the column; the same column again flips the direction |
  | `addFavorite` | 0x5468e8 | not on Favorites: add the picked row's host name and address |
  | `deleteFavorite` | 0x5469b0 | Favorites only: remove the picked row |
  | `createFavorite` | 0x546a77 | Favorites only: add `ui_favoriteAddress` named `default`, refresh |
  | `closeJoin` | 0x546410 | stop a running refresh, else close `joinserver` and open `main` |

  Each add sets `ui_favorite_message` (0x545820) to `@EXE_FAVORITEADDED`, `@EXE_FAVORITEINLIST`, `@EXE_FAVORITELISTFULL` (128), `@EXE_BADSERVERADDRESS`, or for an empty name or address `@EXE_FAVORITENAMEEMPTY` / `@EXE_FAVORITEADDRESSEMPTY`.
- **Owner draws.** VERIFIED (the code).
  - 220 draws `EXE_NETSOURCE\x14<name>` localized (0x543a60) and steps the source on a click (back on MOUSE2), refreshing in full unless the new source is Internet (0x544d00).
  - 253 draws `ui_joinGametype`'s game type, `EXE_ALL` for entry 0, and steps it over "All" then the zone's game types (0x544c80, at most 32).
  - 247 draws `EXE_WAITINGFORMASTERSERVERRESPONSE` while the master is awaited, `EXE_GETTINGINFOFORSERVERS` with `&&1` as `<sent>/<total>` while pinging, else `EXE_REFRESHTIME` with `ui_lastServerRefresh_<source>` (0x543ef0).
- **Shown list** (`UI_BuildServerDisplayList` 0x5472b0): a marked row with a ping above 0, or any marked favourite (0x5474c8), then the filters, each a UI dvar (0x54205b..0x542342), and the game type filter's name against `gametype`. VERIFIED (the code).

  | Dvar | Default | Filter |
  |---|---|---|
  | `ui_browserShowEmpty` | 1 | 0 hides `clients` 0 |
  | `ui_browserShowFull` | 1 | 0 hides a full one |
  | `ui_browserShowPassword` | -1 | else `pswrd` equal |
  | `ui_browserShowPure` | 1 | hides `pure` 0 |
  | `ui_browserShowDedicated` | 0 | 1 needs `hw` 1..3 |
  | `ui_browserMod` | 0 | else `mod` equal |
  | `ui_browserFriendlyfire`, `Killcam`, `Hardcore`, `OldSchool`, `ShowPunkBuster` | -1 | else `ff`, `kc`, `hc`, `od`, `pb` equal |

- **Sort** (`LAN_CompareServers` 0x475b20): columns 0 `pswrd`, 1 `hw` (0 last), 2 host name without colours, case-blind, 3 the map's display name, 4 `clients`, 5 the game type's display name, 6 `voice`, 7 `pure`, 8 `mod`, 9 `pb`, 10 ping. Ties go by ping, then `gametype`, then host name; the direction negates. VERIFIED (the code). The starting column and direction are not found.
- **Feeder 2 columns** (`UI_FeederItemText` 0x5483e0). VERIFIED (the code).

  | Col | Text |
  |---|---|
  | 0 | `X` with a password |
  | 1 | `hw` icon: `server_hardware_unknown`, `_linux_dedicated`, `_win_dedicated`, `_mac_dedicated`, `_win_listen`, `_mac_listen` for 0, 1, 2, 3, 6, 7 (0x549183) |
  | 2 | the address with no ping, else the host name cut to 37 |
  | 3 | the map's display name |
  | 4 | `<clients> (<max>)` |
  | 5 | the game type's display name or `?` |
  | 6, 7, 9 | voice, pure, PunkBuster as `X` |
  | 8 | `X` when `mod` is 0 |
  | 10 | `...` with no ping, else the ping |

- **Feeder 13** (`UI_BuildServerStatus` 0x548160): four columns. First `address` and the status's pairs: those the table at 0x723e00 names, in its order with its label, then the rest. The table's 25 rows: `sv_hostname` (`@EXE_SV_INFO_SERVERNAME`), `address`, `pswrd` yes/no, `gamename`, `g_gametype` as its display name, `sv_pure`, `mapname` as its display name, `shortversion`, `protocol`, `sv_maxping`, `sv_minping`, `sv_maxrate`, `sv_floodprotect`, `sv_allowanonymous`, `sv_maxclients`, `sv_privateclients`, `scr_friendlyFire`, `fs_game`, `mod`, `scr_killcam`, `g_antilag`, `g_compassShowEnemies`, `sv_voice`, `sv_punkbuster`, `sv_disableClientConsole`. Then a blank row, the header `@EXE_SV_INFO_NUM`, `_SCORE`, `_PING`, `_NAME`, and a row per player: index, score, ping, name. VERIFIED (the table and the code). That the unnamed pairs follow the named ones is INFERRED.

### 12.4 vcod

- `vcod_cod4::net::browser` holds the wire (`getservers`, the master's answer, `ServerInfo` as 12.1 reads it, `Status`) and `servercache.dat` (`ServerCache`, `Record`).
- The client's `play::cod4_menus::browser` runs 12.2 and 12.3 on the script menus. The front end (`cod4_front::Cod4Front`) reads `servercache.dat` from the install root at start (`Browser::with_cache`), pumps the browser once a frame (`pump_browser`), hands `take_join`'s address and `password` dvar to the join (`App::connect_cod4`, the connect screen's `cod4_loading::Loader`), and passes typed keys to an edit field (`type_text`).
- A script's `open` stacks the new menu over the open one and `close` uncovers it (`Menus_OpenByName`, INFERRED); a server's `t`, `u` and `L` replace or clear the stack. An edit field takes typed text after a click, Enter or `setfocus`; Backspace drops a character and Enter runs its `onAccept` (`Item_TextField_HandleKey`, INFERRED).
- Departures from retail:
  - The Internet source asks `masterServerName` and `masterPort`, registered with vcod's master (`master.cod.pm` 20810, with its operator's permission; AGENTS.md "Public servers"), where 1.7 asks `cod4master.activision.com`; at most once a second per host.
  - vcod writes only the favourites block and its count of `servercache.dat`, leaving the Internet block as retail last wrote it.
  - No PunkBuster: `joinpb_popmenu`'s Yes joins without it.
  - The Join's password goes in the userinfo as `password` (`Cod4Session::connect_password`).
- Nothing asks the master until Refresh List runs on the Internet source: Internet is `ui_netSource`'s default, `UpdateFilter` refreshes only Local, and stepping the source onto Internet refreshes nothing (12.3).
- Gates: `cod4_join_game_lists_a_lan_server_and_joins_it_with_its_password` (vcod-server in process: Local, the row's columns and icon, Server Info, the password popup, Join with and without the password), `cod4_favorites_and_a_fake_masters_list` (New Favorite, the cache file, Internet through a fake master on loopback), `cod4_browser_against_a_named_server` (`COD4_BROWSER_SERVER`, skipped without it).
- Live: retail `cod4_lnxded` bound to 0.0.0.0 in the namespace, with 10.9.9.1/24 on `lo` and a vcod probe joined, answered the Local broadcast to 10.9.9.255:28960. The row read `^2retail ^7lnxded`, Crash, `1 (8)`, Team Deathmatch, a 16 ms ping, `hw` 1, `pure` 1, `kc` 1; the status listed the labelled keys, `ui_maxclients` 32 unlabelled, and the probe as `0 0 24 vcodprobe`. VERIFIED live (the test above with `COD4_BROWSER_SERVER` and `COD4_BROWSER_BROADCAST`).

### 12.5 `servercache.dat`

- `LAN_LoadCachedServers` (0x4764d0, called from the UI's init at 0x549250) and `LAN_SaveServersToCache` (0x476540, the tail of the UI's shutdown at 0x543681). VERIFIED (the code).
- Layout: the int 3, the Internet count (refused at 20000 or more), the Favorites count (refused at 128 or more), the int 0x2fe980, then 20000 Internet rows (0x2f9b80 bytes) and 128 favourite rows (0x4e00 bytes): 3139984 bytes. A file whose words or reads differ is refused and both lists start empty (0x476521). VERIFIED (the code).
- Before writing, the saver drops each Internet row whose +0x24 byte is 3 or more and each address listed twice, then sorts the list (0x476592). VERIFIED (the code).

## 13. CoD4x servers

Most public CoD4 servers run CoD4x, a rebuilt 1.7 server. One ran on mp_crash with the stock 1.7 `main/` and `zone/` inside `unshare -rnm` (section 7's recipe, its update host also mapped to 127.0.0.2), joined with `cod4_probe` and `vcod --game cod4 --net-probe` on loopback. Everything below is what it sent on the wire.

- **Telling one apart.** `getinfo` answers `protocol\6`, so 1.7's browser lists it, beside `shortversion\x21` and `build\1`; retail sends `shortversion` only in `getstatus`, as `1.7` (section 2). VERIFIED live. `getstatus` carries `protocol\21`, `shortversion\-`, a `version` starting `CoD4 X`, `build`, `branch`, `revision` and a `_CoD4 X Site` key. VERIFIED live. `getchallenge 123` is answered `challengeResponse 1357851985 123 0 xproto 18`, where retail sends `challengeResponse <int>` (section 3). VERIFIED live.
- **A 1.7 client never plays there.** A protocol 6 connect logged `Have to fix up old client which reports version 6` and `Going from CS_FREE to CS_ZOMBIE`; the server sent `updgetchallenge <n> noguid` to its update host (127.0.0.2:27953) every 3 s, and no `connectResponse` came. VERIFIED live.
- **vcod.** `Conn::join` refuses a `challengeResponse` carrying `xproto` with `This server runs CoD4x (xproto N) ...` before `connect`, so nothing reaches the server's update path and no mod is fetched (`vcod_cod4::net::conn::cod4x_proto`). VERIFIED live: both joins above ended with that text. The refusal is vcod's own. Retail has none and would walk on into whatever the server sends: a 1.7 client reads no `xproto`, and fetches and renames a redirected file unchecked (section 11). INFERRED, read off `iw3mp.exe`. The browser hides such rows by the same keys (`ServerInfo::cod4x`, `Status::is_cod4x`, cod4-front-end.md). Playing on a CoD4x server needs its protocol 21 and what it adds past the `xproto` handshake; vcod speaks none of it.

## 14. The console: `status`, `getstatus`'s players, `say`, `tell`, `kick`

Read off the `cod4_lnxded` 1.7 disassembly; no retail console run was diffed against ours.

- **`status`.** `SV_Status_f` (0x816c658) prints `map: %s`, the header, the separator `--- ----- ---- ---...` (0x8230328), then a line per slot not free. VERIFIED.

  | Format | Column |
  |---|---|
  | `%3i ` | slot |
  | `%5i ` | score |
  | `CNCT ` (state 2), `ZMBI ` (state 1), else `%4i ` | ping, capped at 9999 |
  | `%32s ` | GUID, right aligned |
  | `%s^7` | name, padded with blanks to 16 by its raw length |
  | `%7i ` | `lastmsg` |
  | | address, padded with blanks to 22 |
  | `%5i` | qport |
  | ` %5i` | rate |

- **`lastmsg`** is `svs.time` (0x90b4204) minus the slot's `lastPacketTime` (client +0x2145c, read at 0x816c82c), both on the server's clock, so a frame hitch or a `sv_fps` change moves it with the server, not the wall clock. VERIFIED. vcod prints `sv_time_ms` minus `last_packet_ms` (`cod4_status_lastmsg_reads_the_server_clock`).
- **Score** is `G_GetClientScore` (0x80b384e): `level.clients[n]` +0x2f78, the script's `score`. `SVC_Status` (0x81764a2) prints it as the first number of each `%i %i "%s"` player line (0x823228a), for each client past state 1. VERIFIED.
- **`say` and `tell`.** `say` (`SV_ConSay_f`, 0x816d328, registered at 0x816d46f) needs an argument. `tell` (0x816cca6, registered at 0x816d4a7) needs two: the slot number (`strtol`) of a client in state 4, and the text. Each writes `console: ` then `Cmd_ArgsBuffer` from its first text argument, cuts a leading `"` together with the last character, and sends `%c "\x15%s"` (0x82300c7) with `h`, `SV_SendServerCommand`'s ignorable type 0, to everyone or to that client. VERIFIED.
- **`kick`, `onlykick`, `tempBanUser`** share a helper (0x816ca14). Any argument count but two prints `Usage: %s <player name>\n%s all = kick everyone\n` (0x8230394). The name lookup (0x816c1b6) prints `Player %s is not on the server` (0x823026c) when nobody has the name; then a name of `all` (`Q_stricmp`, 0x8210717) drops every slot not free. VERIFIED.
- **`ban.txt`** (0x82310d4) is kept by `banUser` (0x816cbaa), `banClient` and `unbanUser`. `SV_BanClient` (0x8171360) refuses the host (`e EXE_CANNOTKICKHOSTPLAYER`), a client with no GUID (`Can't ban user, GUID is unknown`) and one already listed (`This GUID (%s) is already banned`); otherwise it appends `%s %s\r\n` (GUID, cleaned name) and drops the client with `EXE_PLAYERKICKED`. VERIFIED. `unbanUser` takes one argument (`Usage: unban <client name>`, 0x82300d0) into `SV_UnbanClient` (0x816ea9c), which prints `unbanned %i user(s) named %s` (0x82310dc) or `no banned user has name %s` (0x82310fa), CoD2 1.3's strings. VERIFIED. Its line match is taken to be CoD2's (`engine::bans::BanFile::unban`). INFERRED.
- **Ban check.** `SV_IsBannedGuid` (0x816ec34) reads `ban.txt` whole: an empty GUID is never banned, else each line's first token is `strcmp`'d against it. `SV_GetChallenge` asks it (0x816f13b) ahead of the temporary bans and refuses with the log line `rejected connection from permanently banned GUID "%s"` (0x82307c0) and the OOB `error\n\x15You are permanently banned from this server` (0x82307f8). VERIFIED. The four strings have no reference outside `SV_GetChallenge` (0x816f05a), so the connect checks no ban. VERIFIED. A `cod4_probe` whose GUID `ban.txt` listed got that `error` 2 ms after its `getchallenge` and no `challengeResponse`. VERIFIED live (section 15). vcod-server keeps the file at `--ban-file` (in memory without it) and refuses at the `getchallenge` too.

### 14.1 The rest of the command table

A retail run (`cod4_lnxded` under `unshare -rnm`, mp_crash `war`, two `cod4_probe` clients, the commands over rcon; cod4-server-gaps.md, "The retail run") answered each of these as written here.

- **The table.** `Cmd_AddCommand` (0x81116b4) registers 67 names and `Cmd_Init` inlines four more (`wait`, `cmdlist`, `exec`, `vstr`; `Cmd_AddCommand: %s already defined` at 0x8222858 has five references). VERIFIED. A dedicated server's `cmdlist` printed 63 of them, newest first, then `63 commands`. VERIFIED live. vcod's `cmdlist` prints the ones its console runs, in that order (`cod4::oob::COMMANDS`).
- **`kick` bans, `onlykick` does not.** `kick` and `tempBanUser` are one handler (0x816cb4a, both rows of the server command table); `onlykick` is 0x816cb36. Both call the helper, `onlykick` with no name buffers. With a client found, 0x816cb4a prints `%s (guid "%s") was kicked for cheating` (0x82303c4) and calls 0x816e808 with the GUID: the brief ban `sv_kickBanTime` holds. VERIFIED. `kick bravo` printed `0:bravo EXE_PLAYERKICKED` (`SV_DropClient`'s own line, under the redirect) then the cheating line. VERIFIED live.
- The helper's drop (0x816c96e) refuses a loopback client with `%c "EXE_CANNOTKICKHOSTPLAYER"` (0x823005a) and drops anyone else with `EXE_PLAYERKICKED` (0x8230078). VERIFIED.
- **`dumpuser`** (0x816cc0a): `Server is not running.` without a level (0x822ffd4), `Usage: info <userid>` (0x82300a7) unless it has one argument, the name lookup's refusal, else `userinfo` (0x82300bd), `--------` (0x82319db) and `Info_Print` of the client's userinfo (client +0x650). VERIFIED. Each pair printed as the key padded to 20, then the value; the probe's userinfo read `cg_predictItems`, `cl_punkbuster`, `cl_anonymous`, `color`, `head`, `model`, `snaps`, `rate`, `name`, `protocol`, `challenge`, `qport`. VERIFIED live.
- **`heartbeat`** (0x816c180) stores 0x80000000 in `svs.nextHeartbeatTime` (0x13e636b0), so the next frame beats. VERIFIED. It printed nothing. VERIFIED live.
- **`killserver`** (0x816c2f2) calls 0x8123602 with `EXE_SERVERKILLED` (0x822ffc3), which runs `SV_Shutdown` (0x81743a8). VERIFIED. The process then stayed up, answered neither rcon (`status`, `dumpuser`, `map_restart`, `quit`) nor `getstatus` or `getinfo`, and the log took `ShutdownGame:`. VERIFIED live. vcod drops every client with the reason, sends each its last message at once, shuts the level down and answers nothing after (`Cod4Server::kill_server`).
- **Dvar commands.** `toggle` on a 0/1 dvar flips it silently; on any other with no value list it prints `'toggle' with no arguments makes no sense for dvar '%s'` (file 0x1dc0f0). `togglep` prints `%s toggled to %s`. VERIFIED live. An unknown dvar prints `toggle failed: dvar '%s' not found.` and a bare `toggle` `USAGE: %s <variable> <optional value sequence>`. VERIFIED (the strings, file 0x1dc0c8 and 0x1dc098). `setu vtest 5` made a dvar reading `5` with default `5`; `reset vtest` printed nothing. VERIFIED live. `setfromdvar`, `reset`, `set`/`seta`/`sets`/`setu` print their `USAGE:` lines (file 0x1dc174, 0x1dbf43, 0x1dc28c..0x1dc2ec). VERIFIED.
- **`exec`** (`Cmd_Exec_f`, 0x81121a2) takes one argument (else `exec <filename> : execute a script file`, 0x822287c) and adds `.cfg` when it has no extension (0x8237ac3). `config_mp.cfg` (0x8222788) is read off the disk only, and a miss prints nothing (0x81124a7..0x81124bb). Any other name is first a rawfile of the loaded zones while `useFastFile` (0x88a53f0, registered 1 at 0x8121d28) is set and the database is up (0x1411e148): `execing %s from fastfile` (0x82227ac). Else `FS_ReadFile`: `execing %s from disk` (0x8222796), or `couldn't exec %s` (0x82227c6) with the argument as typed through `Com_PrintError` (channel 1). VERIFIED. The run printed `execing ragdoll.cfg from fastfile` with a loose `ragdoll.cfg` in the home path's `main/` (its dvar stayed unset), `^1Error: couldn't exec nothere` and `^1Error: couldn't exec nothere.txt`. VERIFIED live. vcod reads the zones' rawfiles, then the disk (`Cod4Server::exec_file`).
- **`path`** listed the search path: the home path's `main/` iwds, its directory, the base path's iwds (`iw_13` down to `iw_00`), its directory, `main_shared`, `players`, then the `localized_*` iwds. VERIFIED live. So a cfg in a home path iwd beat a loose one beside it (`vt_both.cfg` read `iwd`), and a loose home file beat the base path's iwds. VERIFIED live. vcod walks the `fs_game` directory then `main/`, home then base, each directory's iwds ahead of its files, the localized iwds last (`pk3::read_search`).
- **`writeconfig vtest.cfg`** printed `Writing vtest.cfg.` (file 0x1db4e3) and wrote `players/vtest.cfg` under the home path: `// generated by Call of Duty, do not modify` (file 0x1dba00), a `seta <name> "<value>"` line per archived dvar in case-folded order, then `con_hidechannel *; con_showchannel dontfilter error ...` on one line. VERIFIED live. A name without an extension takes `.cfg` (`writeconfig vw2` wrote `players/vw2.cfg`), a subdirectory is made (`sub/vw3.cfg`), and any argument count but one prints `Usage: writeconfig <filename>` (file 0x1dba30). VERIFIED live. A dvar waiting on a latch is written with the waiting value: `exec default_mp_gamesettings` (its `sv_maxclients 24`) printed `sv_maxclients will be changed upon restarting.` and the next file read `seta sv_maxclients "24"`. VERIFIED live. vcod keeps no latch on its CoD4 dvars, so it writes the line but not the warning.
- **Archived dvars.** A dedicated server archives 41 dvars (the file's lines). `seta` archives the dvar it sets, a registered one too: after `seta vnew 7` and `seta g_inactivity 5` both had lines, `set vplain 3` none. VERIFIED live. vcod registers the 41 with retail's values and spelling (`oob::ARCHIVED`), clearing the archive flag CoD1's table gives others.
- **`writedefaults vd1.cfg`** wrote `main/vd1.cfg` (the home path's game directory, not `players/`): the header and 142 `set <name> "<value>"` lines, over a set of dvars no flag column of `dvarlist` picks out. VERIFIED live.
- **`dvar_bool <name> <default>`, `dvar_int`, `dvar_float <name> <default> <min> <max>`** make a dvar the query then prints with `Domain is 0 or 1`, `Domain is any integer from 0 to 10`, `Domain is any number from 0 to 1`; a wrong count prints `USAGE: %s <name> <default>` (file 0x1dbef9) or `USAGE: %s <name> <default> <min> <max>` (file 0x1dc1a4). VERIFIED live. vcod keeps the domain beside its dvar (`oob::Domain`).
- **`setdvartotime vtime`** set `vtime` to `59642`, the process's milliseconds; a bare one prints `USAGE: set <variable>`. VERIFIED live. vcod writes the server clock there. **`touchFile vtouch.txt`** printed and wrote nothing; a bare one prints `Usage: touchFile <file>`. VERIFIED live.
- **Listings.** `dvarlist` printed 566 dvars as flag columns, name and value, then `%i total dvars` (`dvarlist g_lo*` the two `g_log` dvars); `dvardump` the same under a `DVAR DUMP` rule; `dir` its usage or `Directory of players`; `fdir *.cfg` 126 files out of every iwd and directory; `meminfo` the hunk and the zones. VERIFIED live. vcod's table holds its own dvars, not 1.7's 566.
- **`statSet 1 2`** on the dedicated server shut the level down (`ShutdownGame:` in the log) and nothing answered after. VERIFIED live.
- **`gameCompleteStatus`** printed `Sending gameCompleteStatus to cod4master.activision.com`. VERIFIED live.
- **Not run by vcod's console:** `writedefaults`, `dvarlist`, `dvardump`, `setfromlocString`, `statSet`, `statGetInDvar`, `readStats`, `uploadStats`, `meminfo`, `path`, `dir`, `fdir`, `fullpath`, `con_showchannel`, `con_hidechannel`, `con_channellist`, `con_visiblechannellist`, `in_restart`, `phys_go`, `phys_stop`, `snd_refreshVolumeModGroups`, `net_dumpprofile`, `scriptUsage`, `stringUsage`, `setPerk`, `gameCompleteStatus` (no master contacted). Client, debug or developer commands, or ones needing state vcod does not keep (the whole dvar registry, file system handles).

### 14.2 rcon

1.7 keeps no client password dvar: the client logs in. Read off the `iw3mp.exe` and `cod4_lnxded` 1.7 disassembly; no retail session was diffed against ours.

- **`CL_Rcon_f`** (`iw3mp.exe` 0x471af0) takes `rcon <command>`; under two arguments it prints `USAGE: rcon <command> <options...>` (0x6d21e0). Its first argument is compared case-blind (0x571a90) with `login`, `logout` and `host`. VERIFIED.
- **`rcon login <password>`** (0x471a10) keeps the password in a static buffer (0xe11558). Any count but three tokens prints `USAGE: rcon login <password>`; a password of 24 bytes or more prints `rcon password must be %i characters or less` with 24 (`cmp eax,0x18; jb`, so 24 itself is refused). VERIFIED. `rcon logout` clears it, or prints `Not logged in` when empty. VERIFIED.
- **`rcon host <address>`** (0x471a80) parses into a static `netadr_t` (0xe11570), port 28960 (`push 0x7120`, `htons`) when none is named; a bad one prints `bad host address`. VERIFIED. vcod forgets the host on a bad address, taken to be what the failed parse leaves. INFERRED.
- **Sending.** Any other line with no password prints `You need to log in with 'rcon login <password>' before using rcon.` (0x6d2220) and sends nothing. VERIFIED. Else the text is `rcon ` (0x6d2264), the password, then each token from the first, one space apart (0x69d8d0), quoted when empty or holding a byte at or below a space (0x500620); a text that reaches 0x400 bytes prints `rcon commands are limited to %i characters` with 1023 and sends nothing. VERIFIED.
- **Target.** The server's address (0x934eac) when `cls.state` (0xc5f900) is 5 (`CA_CONNECTED`) or past, else the host when its type is not 1; else three lines: `Can't determine rcon target.  You can fix this by either:`, `1) Joining the server as a player.`, `2) Setting the host server with 'rcon host <address>'.`. VERIFIED. It goes out of band with its NUL (0x508d70). VERIFIED. The answers come back as `print` packets on the connection's socket, which `CL_ConnectionlessPacket` prints in any state. INFERRED.
- **Server.** `SVC_RemoteCommand` (`cod4_lnxded` 0x817818a) drops a request within 499 ms of the last one it took (`cmp ecx,0x1f3; jle`), as CoD1 does. The password is the second token (or empty) and matches when `rcon_password` is set and `strcmp` equal. VERIFIED. It logs `Rcon from %s:\n%s\n` or `Bad rcon from %s:\n%s\n` with the third token, then under the redirect answers. VERIFIED.

  | Case | Answer |
  |---|---|
  | `rcon_password` unset | `The server must set 'rcon_password' for clients to use 'rcon'.` (0x8232838) |
  | wrong password sent | `Invalid password.` (0x82327ee) |
  | empty password | `You must log in with 'rcon login <password>' before using 'rcon'.` (0x8232878) |
  | good | rebuild the tokens from the third, one space apart, quoted as the client quotes them; run the line when under 0x400 bytes |

- The dvar is `rcon_password` (`Password for the rcon command`, 0x823177c). VERIFIED (the strings). CoD1's `rconPassword` sets a dvar nothing reads, so rcon stays off; vcod-server warns of it.

### 14.3 Test clients, `kick all` and the bans: a retail run

`cod4_lnxded` 1.7 under `unshare -rnm` (section 7), stock `war` on mp_crash, `sv_pure 0`, `+set developer_script 1` so `_dev.gsc`'s `/# #/` block starts `addTestClients`, then `set scr_testclients 2` over rcon beside one `cod4_probe` player. `developer 1` refuses every real client (`Clients can't connect if the server is in developer mode`). There a test client dies in `_rank.gsc`'s `player setStat( 251, ... )`: the builtin (0x80c54dd) refuses a test client while `developer_script` is set, with the misleading `setstat: developer_script is false.` (0x821ef1c), a fatal script error at `developer 1`. At `developer 0` the console printed no error and the bots stayed. VERIFIED live.

- `SV_AddTestClient` (0x8172c7e) connects `bot%d` (0x8230f38, `rate` 5000, `snaps` 20) from an address of type 0 whose port is a counter (0x8c0ff80). VERIFIED. Its connect takes no GUID: the console printed `Connecting player #1 has an unknown GUID` and `Going from CS_FREE to CS_CONNECTED for  (num 1 guid "")`. VERIFIED live.
- `status` leaves a bot's GUID column blank; its row read `  1     0   999 <32 blanks> bot0^7  3650 00000000.000000000000:0    1  5000` (ping 999, the type 0 address, qport 1, rate 5000). VERIFIED live. vcod's bots are its own, not test clients; they keep an empty GUID too and print their own ping, address and rate.
- `banUser bot0` and `banClient` of a bot's slot printed `Can't ban user, GUID is unknown` and left it in: past the host check, `SV_BanClient` (0x8171360) tests the GUID's first byte (client +0xa0d00, 0x8171376) before anything else. VERIFIED (the code and live). 1.7 cannot ban a bot, and vcod refuses it the same way.
- `banClient 0` on the player printed `0:alpha EXE_PLAYERKICKED`: `SV_DropClient`'s line reaches the redirect on a ban too. VERIFIED live. vcod prints it too.
- `kick all` with the player in the world printed `Player all is not on the server`, `0:alpha EXE_PLAYERKICKED`, `1:bot0 EXE_PLAYERKICKED` and `2:bot1 EXE_PLAYERKICKED`, and all three slots went to `ZMBI`. In a second run, after `banClient 0` had dropped the player, his zombie slot printed nothing. VERIFIED live. The loop (0x816cadf..0x816cb2d) walks `sv_maxclients` slots and calls the drop helper on each not free. The helper refuses only an address of type 2 (`NA_LOOPBACK`, the listen server's own client, 0x816c97b), so a dedicated server's `kick all` drops every player and every bot. VERIFIED (the code).

### 14.4 The stdin console

Read off the `cod4_lnxded` 1.7 disassembly, 2026-11-12. The shape is CoD1's and CoD2's (cod11-server-handshake.md, "The stdin console"; cod2-protocol.md 17.8).

- `Sys_ConsoleInputInit` (0x81d4616) ignores `SIGTTIN` and `SIGTTOU` (0x81d4643, 0x81d4657). It registers `ttycon` as a bool dvar, default 1, flags 0x10, described `use tty console` (0x81d467b). VERIFIED.
- A call at 0x81d4690 sets `ttycon` to false through the setter the not-a-tty branch calls at 0x81d46ec. VERIFIED.
- That call is unconditional and precedes the read of `ttycon` at 0x81d469c, so the tty mode never starts. `Started tty console` (0x823e194) and `stdin is not a tty` (0x823e164) never print. INFERRED.
- `Sys_ConsoleInput` (0x81d44f2) and its copy inlined in `Sys_GetEvent` (0x81d49d2, called from 0x8123654) run a zero-timeout `select` on fd 0 (0x81d4aac) and a `read` of up to 256 bytes into 0x14119140 (0x81d4af2), the last byte read overwritten with a NUL. VERIFIED.
- They read while `stdin_active` (0x8273740, 1 in the file) is set, and EOF clears it (0x81d4afb). The line is copied and queued as an event (0x81d4b31..0x81d4b63), as 1.1 does. INFERRED.

vcod-server under `--game cod4` reads stdin as for 1.1. Each line runs through `Cod4Server::exec`, the dispatch rcon uses, and its output goes to the log. Gate: `stdin_console::tests::cod4_runs_a_stdin_line`.

## 15. Off the LAN: the authorize round trip, `requeststats`, pacing

The run: `cod4_lnxded` at 203.0.113.1:28960 in a rootless user, net and mount namespace, the clients at 198.51.100.2 in a child net namespace joined by a veth pair (`tools/offlan_capture.sh`'s layout), so neither address is private or shares three octets with the other. The hosts file mapped `cod4master.activision.com` to 127.0.0.2, where a listener logged every datagram and answered none. `sv_pure 0`, `scr_war_timelimit 0`, `+set dedicated 1` (refused; the server ran at 2, see "The server's own address"). A script sent `getchallenge 0 "<hex>"` every 3 s for 1290 s. Then `cod4_probe` joined as a spectator, `rcon map mp_crash` went in 25 s into its join, a second script challenged off the LAN 15 s after that spawn, and a third from the server's own address. vcod-server ran the same timeline with a 30 s first script.

### The challenge off the LAN

- `SV_GetChallenge` (0x816f05a) finds or makes the address's slot, runs the bans (section 14), copies the hex into the slot, then asks `Sys_IsLANAddress` (0x81d59c8), or `Sys_IsLANAddress_IgnoreSubnet` (0x81d55a0) at `dedicated 2`, which `cod4_lnxded` always is; it asks neither when `net_lanauthorize` (0x892fe94) is set. A LAN address gets `challengeResponse` at once. VERIFIED (the code).
- Off the LAN, with `svs.authorizeAddress` unset, it resolves `cod4master.activision.com` (0x822325c) on port 20800 (0x5140); a failed lookup prints `Couldn't resolve address` (0x823117f) and returns with no answer. VERIFIED (the code). The `Resolving %s` and `%s resolved to %i.%i.%i.%i:%i` lines came on the console. VERIFIED live. That a failed lookup marks the address bad (type 1, 0x816f24c), so the next challenge goes on to the timeout test, is INFERRED.
- It then sends `getIpAuthorize <challenge> <a.b.c.d> "<fs_game>" <sv_allowAnonymous> PB "<hex>"` (0x82306dc) to that address and answers the client nothing: one per `getchallenge`, the slot's challenge repeated (`getIpAuthorize 551485082 198.51.100.2 "" 0 PB "0123..."`), 415 in the run. VERIFIED live. Without a hex the form drops `PB "%s"` (0x823070c). VERIFIED (the string).
- **Timeout** (0x816f30f..0x816f5c8). Only when all three hold does it print `authorize server timed out` (0x8231199, developer only), stamp the slot's ping time and send `challengeResponse`: `svs.time` more than 1200000 ms past the last word from the master's host (0x13e776e0), more than 7000 ms past the slot's first `getchallenge` (+0x20), and the client not the master's own address (`NET_CompareAdr`, 0x813c13c). VERIFIED (the code).
- 0x13e776e0 takes `svs.time` where the client array is allocated (0x817489d, 0x8174ab1) and when any of four connectionless commands comes from the master's host (0x816eff6, called at 0x8177830, 0x8177905, 0x817797f, 0x8177aeb). INFERRED (the code).
- The first script's first `challengeResponse` came 1201 s after the server started, then one every 3 s. VERIFIED live. A `map mp_crash` did not restart the 20 minutes: the script that challenged 15 s after that spawn was answered at its fourth `getchallenge`, 9 s in. VERIFIED live. The probe joining past the 20 minutes was answered on its fourth too (0, 3, 6, then 9 s). VERIFIED live.
- So a 1.7 server whose authorize server is silent lets nobody in from off the LAN for its first 20 minutes, then each client after 7 s of challenging. INFERRED (the two bullets above). `cod4_probe` gives up after 30 s: the early join ended `Error: no challengeResponse`. VERIFIED live.
- The server's own public address counted as off the LAN: the script sent from 203.0.113.1 got no answer in its 6 s and the listener logged `getIpAuthorize ... 203.0.113.1`, though the console printed `IP: 203.0.113.1`. VERIFIED live. The next subsection explains why.
- The heartbeat goes out at the server's start (`heartbeat COD-4\n` to :20810, 0.8 s in). VERIFIED live. The run's `+set dedicated 1` did not take, so it ran at `dedicated 2`, the only value `cod4_lnxded` has. vcod-server sends none below `dedicated 2`, as for every game it hosts, and keeps its own `dedicated` settable with 1 as the default (AGENTS.md, Running).
- vcod-server never asks the authorize server (the CD key check is skipped on purpose): every address gets `challengeResponse` at once, which is what retail sends once its authorize server answers `accept`. Off the LAN its first answer came within a millisecond. VERIFIED live.

### The server's own address

Read with gdb on the same layout (retail under `unshare -rnm`, breakpoints on the two tests and the table's build).

- `dedicated` is `Dvar_RegisterEnum` (0x81a2130) at 0x8121c96: names `listen server`, `dedicated LAN server`, `dedicated internet server` (0x826df40), default 2, flags 0x40, `True if this is a dedicated server` (0x8223780). VERIFIED (the code). `+set dedicated 1` on the command line prints `dedicated is read only.` and the value stays 2: `[dvar+0xc]` read 2 at 0x816f1fd on every `getchallenge`. VERIFIED live.
- So `SV_GetChallenge` always takes `Sys_IsLANAddress_IgnoreSubnet` (0x81d55a0) unless `net_lanauthorize` (0x892fe94, `Dvar_RegisterBool` default 0, `Authorise CD keys when using a LAN` 0x8224b40) is set. It is true for address types 2 and 5, and for type 4 (IP) in 10/8, 127/8, 169.254/16 (`0xfea9`), 172.16/12 and 192.168/16 (`0xa8c0`), nothing else; it has no table. VERIFIED (the code). gdb saw it called with 203.0.113.1 on each of three challenges, `Sys_IsLANAddress` never, and each went on to `Resolving cod4master.activision.com`. VERIFIED live. A public address, the host's own included, is off the LAN for the challenge on every `cod4_lnxded`.
- `Sys_IsLANAddress` (0x81d59c8) has the same prefix rules, then compares the first three octets with each entry of the local table. The table is built at startup (0x81d576c, called at 0x81d5e58): `gethostname`, `gethostbyname`, `Hostname: %s` (0x823e2a1), an `IP: %i.%i.%i.%i` (0x823e290) per IPv4 address, written from entry 1 (entry 0 stays 0.0.0.0), at most 15. VERIFIED (the code). The run's table held 0.0.0.0 and 203.0.113.1, count 2. VERIFIED live. That the address came from nss's `myhostname` (the hosts file had no line for the hostname) is INFERRED. A hostname resolving to 127.0.1.1 alone, as a stock Debian `/etc/hosts` has it, leaves the host's own /24 out. INFERRED (the code).
- Its callers: `SV_UserinfoChanged`'s rate (0x817037d: a LAN client gets 99999 only below `dedicated 2`, 0x817038f, so never on `cod4_lnxded`; else the userinfo's `rate`, 5000 when empty), `SV_DirectConnect`'s ping limits (0x8171847) and `SV_SendMessageToClient`'s pacing (0x8178b3e). A connected client's `getIpAuthorize` resend (0x8172bb7, `sending getIpAuthorize for %s` 0x82306bc) takes the challenge's split. VERIFIED (the code).
- vcod-server answers every challenge (no authorize), so the subnet-blind test decides nothing there. Its ping limits and pacing take `Lan::Cod2`, the subnet test, with the host's interface addresses for the table (`vcod_common::net::lan::local_addresses`, the choice CoD1's port made for the 127.0.1.1 case). It never gives the LAN rate, as retail does not. VERIFIED (the code).

### The connect off the LAN

- `SV_DirectConnect` takes the slot's first ping (+0x3c, else `svs.time` minus the ping time), prints `Client %i connecting with %i challenge ping from %s` (0x8230dd0) and marks the slot connected. For an address `Sys_IsLANAddress` refuses (0x8171847) it applies `sv_minPing` (0x13ed7c00, `error\nEXE_ERR_HIGH_PING_ONLY`, `Client %i rejected on a too low ping`) and `sv_maxPing` (0x13ed7c04, `error\nEXE_ERR_LOW_PING_ONLY`, `Client %i rejected on a too high ping: %i`). VERIFIED (the code). vcod-server applies both the same way (`cod4_min_ping_refuses_only_off_the_lan`).

### `requeststats` off the LAN

- Past the 20 minutes the probe got `connectResponse` 6 ms after its `connect`; its seven `stats` packets were answered in 4-15 ms each, `statResponse 126` down to 0, as on loopback. VERIFIED live.
- `map mp_crash` sent `loadingnewmap\nmp_crash\nwar`, and 428 ms later `requeststats\n`; the probe's netchan messages drew eight more, and its repeated packets 22 `statResponse 0`. VERIFIED live. One `requeststats` per message while no stats packet has come is `SV_SendClientGameState`'s (the reference at 0x816e616). INFERRED.
- vcod-server sent the same counts: 1 `loadingnewmap`, 9 `requeststats`, six falling masks and 22 `statResponse 0` for 28 `stats` packets, its first `requeststats` 382 ms after `loadingnewmap`. VERIFIED live.

### Pacing a client out of the world

- `SV_SendMessageToClient` (0x8178aff..0x8178bed): a loopback or LAN address (0x8178b3e) is due the next frame. Anyone else is due `(min(bytes, 1500) * 1000 + 48000) / rate` ms on, `rate` capped by `sv_maxRate` past 999, at least `snapshotMsec`. A client not `CS_ACTIVE` (4) with no download (`downloadName`, +0x212a4) waits at least a second (0x8178bd1..0x8178bed). VERIFIED (the code).
- Both gamestates' second fragment left 957 ms after the first, the rest about 350 ms apart. VERIFIED live. vcod-server waits the same second (`cod4_an_idle_off_lan_client_waits_a_second`): its second fragments left 976 and 991 ms after the first, the rest 350 ms apart. VERIFIED live. Without that wait it sent the second 336 ms after the first.
- A client in the world more than 28 messages past its last acknowledge is paced by a random back-off instead, LAN or not (`cod4-server-frame.md` 12). VERIFIED (the code).

### A stand-in authorize server

- `SV_AuthorizeIpPacket` (0x816fd62) takes a packet only from the authorize server's address (`SV_AuthorizeIpPacket: not from authorize server` 0x82309e4 otherwise), finds the slot by argv 1, the challenge (`SV_AuthorizeIpPacket: challenge not found` 0x8230a18), and stamps the slot's time. VERIFIED (the code and strings). Argv 2 is `deny` (0x8231222), `demo` (0x8231296) or `accept` (0x82312a7); argv 3 the reason (`CLIENT_UNKNOWN_TO_AUTH`, `BAD_CDKEY`, `INVALID_CDKEY`, `BANNED_CDKEY`); past a `deny`, argv 5 must equal the slot's `getchallenge` hex (`strcmp` at 0x816ffb8) or it answers `error\nEXE_ERR_BAD_CDKEY` and prints `rejecting connection due to mismatched GUID` (0x8230a44). An `accept` sends `challengeResponse %i` (0x823115c) when the slot holds no client yet (+0x28). INFERRED (control flow).
- A stand-in at 127.0.0.2:20800 that answered each `getIpAuthorize` with `ipAuthorize <challenge> accept KEY_IS_GOOD 0 <hex>`, the request's challenge and hex, let `cod4_probe` in from 198.51.100.2 30 s after the start: `challengeResponse` came 13 ms after its first `getchallenge`. VERIFIED live. `tools/cod4_offlan_capture.sh` runs that stand-in, so an off-LAN run waits out no 20 minutes.
