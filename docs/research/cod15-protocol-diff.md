# CoD 1.5 (protocol 6) against 1.1 (protocol 1), client side

What a 1.1 client changes to join a patch 1.5 server. In short, 1.5 is 1.1's wire with one wider netfield, a renumbered top of `pm_flags`, a new protocol number, three more userinfo keys and four new server commands. Huffman, netchan, the message codec, the entity and clientState tables, the event table and the configstring layout did not move.

Contents: 1 queries, 2 handshake and PunkBuster, 3 codec, 4 netfields, 5 `pm_flags`, 6 eFlags and events, 7 configstrings, 8 server commands, 9 pmove, 10 paks, 11 browser, 12 match timeout, 13 server side, 14 HTTP downloads, 15 snapshot culls, then what vcod does.

## Sources

| Binary | md5 | Role |
|---|---|---|
| `cod_lnxded` 1.5b | `d3ac406b33acf2c9278a813d5f011b46` | retail Linux server; netfield tables, query and connect strings |
| `main/game.mp.i386.so` 1.5b | `343f99cd67b79ac74aeaa5261f63c011` | game module with symbols; `pm_flags`, configstrings, server commands |
| `CoDMP.exe` 1.5 | `4bdf293d8e6fb32208d1b0942a1ba6bc` | client engine; message codec, userinfo cvars |
| `main/cgame_mp_x86.dll` 1.5 | `075e2af18aeaf2aeaf2a75ce22db683a` | client game; server commands, events |
| `ui_mp_x86.dll` 1.5 (`pakb.pk3`) | `90274612d492da05d56b4a02cf684423` | UI module; server list columns, the PunkBuster join popup |
| `main/game_mp_x86.dll` 1.5 | `d396f23231a113f292c171443c403074` | game module the Windows server below loads |
| `pb/pbsv.dll` (1.5 install) | `faf14ec5f413e822e090e3fb704ac25d` | PunkBuster server, v1.076 |

The 1.1 counterparts are those listed in `AGENTS.md` and `cod11-events-and-fx.md`. Addresses in `game.mp.i386.so` are `nm` addresses (Ghidra's import of either module sits 0x10000 higher). 1.5 `CoDMP.exe` and cgame addresses are virtual at their image bases.

Method:
- `tools/re/netfields.py` and `tools/re/dump_field_table.py` on both servers.
- Ghidra exports of 1.5 `CoDMP.exe`, `cgame_mp_x86.dll` and `game.mp.i386.so`, diffed function by function against the 1.1 exports with addresses and locals renamed away.
- Live traffic between `vcod --net-probe` and the 1.5b server (`COD_VERSION=1.5 tools/run_server.sh`, loopback), captures dated 2026-10-13. The 1.5b binary needs a 32-bit `libstdc++.so.5`; the script header says where it looks.
- PunkBuster has no Linux module here. Its measurements ran 1.5 `CoDMP.exe` as a dedicated server under wine (display drivers disabled) inside `unshare -rn`, loopback only, so PunkBuster reached no outside server. `+set logfile 2` puts its console in `main/console_mp.log`.

## 1. Protocol number and queries

- `getinfo` and `getstatus` both report `protocol\6`; `getstatus` reports `shortversion\1.5` (live capture, VERIFIED).
- `infoResponse` keys, in order: `challenge`, `protocol`, `hostname`, `mapname`, `clients` (only when nonzero), `sv_maxclients`, `gametype`, `pure` (only when 1), `pswrd` (only when 1), `kc`, `hw`, `pb`, `mod`. Live capture with 0 and 1 clients, `g_password` set and cleared, `sv_pure` 0 and 1, VERIFIED.
- 1.1 always sends `clients`, `pure` and `pswrd`, has no `kc`, `hw`, `pb` or `mod`, and 1.5 drops `sv_allowAnonymous` (VERIFIED against the 1.1 capture in `cod11-server-handshake.md`). vcod's `ServerInfo` reads a missing key as 0, which is what each means.
- `kc` tracks `scr_killcam` (INFERRED from the values seen beside it).
- `pb` is 1 with PunkBuster running and 0 without (live, the Windows server with and without `+set sv_punkbuster 1`, VERIFIED).
- `hw` is 1 from the Linux 1.5b server and 2 from the Windows one (live, VERIFIED): the index of the 1.5 UI's hardware icons (section 11).
- `mod` read 1 from both servers on stock paks (live, VERIFIED). Section 13 has what sets it on 1.5b.
- `challengeResponse <n>`: one integer, as on 1.1 (live capture, VERIFIED). Both format strings survive at 0x80ddeb8 and 0x80ddea0 (VERIFIED).
- A connect with the wrong `protocol` key gets `error\nEXE_SERVER_IS_DIFFERENT_VER\x151.5\n` (live capture against a protocol-1 connect, VERIFIED; token at 0x80de2c6).
- The master query is `getservers <protocol> full empty` in both, the protocol coming from the UI's `globalservers %d %d full empty` (string in both `ui_mp_x86.dll`s, VERIFIED). The master's answer to `getservers 6` is not measured here.

## 2. Handshake and userinfo

| Cvar | Default | Flags | Registered | Userinfo position |
|---|---|---|---|---|
| `cl_guid` | `unknown` | 0x42 | `CL_Init` (1.5 `CoDMP.exe` 0x413380), between `cl_anonymous` and `password`, VERIFIED | between `password` and `cl_anonymous`, after `\cl_punkbuster\0`, INFERRED |
| `cl_punkbuster` | 0 | 0x43 | same, VERIFIED | same, INFERRED |
| `cl_wwwDownload` | `1` (string at 0x5a7c54) | 3 | `CL_Init`, before `name` and `rate`, VERIFIED | after `name`, at the end, INFERRED |

The userinfo positions follow from the newest-first info string walk, as for 1.1's order.

- Nothing in 1.5 `CoDMP.exe` sets `cl_guid` (VERIFIED: its only reference is the registration). PunkBuster sets it (INFERRED).
- The 1.5b server logs `Going from CS_FREE to CS_CONNECTED for vcod (num 0 guid 0)` for that userinfo and accepts it (live, VERIFIED).
- 1.1 `CoDMP.exe` has no `cl_wwwDownload` or `wwwdl` string (VERIFIED). Section 14 covers what it switches on.
- Connectionless commands: 1.1's minus the MOTD. `getmotd`, `cl_motd` and `codmotd.activision.com` are gone from 1.5 `CoDMP.exe`. `challengeResponse`, `connectResponse`, `infoResponse`, `statusResponse`, `getserversResponse`, `keyAuthorize`, `needcdkey`, `loadingnewmap`, `print`, `disconnect` and `updateResponse` are in both (string sets, VERIFIED).
- CD key authorization uses the same `getKeyAuthorize %i %s` and `getIpAuthorize %i %i.%i.%i.%i %s %i` strings (VERIFIED). vcod's client sends `getKeyAuthorize` as on 1.1 (`net::cdkey`); `vcod-server` sends neither.
- `sv_punkbuster`, `cl_punkbuster` and the `pbsv.so` loader strings are new in 1.5b (VERIFIED).

### PunkBuster

- `sv_punkbuster` is read-only once registered. The Linux 1.5b server prints `sv_punkbuster is read only.` for `+set sv_punkbuster 1` and runs without PunkBuster (live, VERIFIED; no `pbsv.so` here). The Windows server loads `pb/pbsv.dll` and prints `PunkBuster Server (v1.076 | A1319 C1.074) Enabled` for the same `+set` (live, VERIFIED).
- `pb_sv_enable` on a running server answers `PunkBuster Server: This edition of PunkBuster Server cannot be enabled while the game server is running.` (live, rcon, VERIFIED).
- With PunkBuster on, a connect with `cl_punkbuster 0`, or with neither `cl_punkbuster` nor `cl_guid`, gets one `error\nPunkBuster must be Enabled before joining this server. PunkBuster can be Enabled/Disabled in the Multiplayer Options menu. Visit www.evenbalance.com for PunkBuster support.\n` and nothing else (live, VERIFIED).
- The text comes from `pbsv.dll`. `SV_DirectConnect` (1.5 `CoDMP.exe` 0x457380) calls the PunkBuster hook at `0x14604b4` when set and sends its return when that starts `error\n` (decompile, INFERRED).
- Without PunkBuster the same server takes `cl_punkbuster` 0, 1 or absent and logs `guid 0` for each (live, VERIFIED).
- 1.5's UI checks first. `JoinServer` (ui_mp_x86.dll 1.5 0x4000b124) opens `joinpb_popmenu` (`paka.pk3` `ui_mp/pb_popmenus.menu`) when the server's PunkBuster byte is 1 and `cl_punkbuster` is 0, else runs `connect %s` (disassembly, INFERRED from the branches).
- The byte comes from UI trap 0x62. The engine function ending at 1.5 `CoDMP.exe` 0x419ad6 returns offset 0x20 of a server entry, where 0x414145 stores the `pb` key (disassembly, VERIFIED).
- The popup's Yes runs `setPbClStatus 1` and `uiScript joinserver` (the menu file, VERIFIED).

vcod on protocol 6 sends `cl_punkbuster 0` and `cl_guid unknown`, the 1.5 defaults with PunkBuster off, and never claims PunkBuster. Its `joinserver` skips the popup, whose Yes it cannot honour, and connects. A PunkBuster server then refuses with its text, which the error popup shows. The browser marks PunkBuster servers on 1.5's menus (section 11).

## 3. Message codec, Huffman, netchan

- Every function of 1.1 `CoDMP.exe` between 0x443000 and 0x449000 (52 functions: bit and byte readers and writers, the usercmd, entityState and clientState deltas, the Huffman coder) has a 1.5 twin whose decompile is identical once addresses and locals are renamed (VERIFIED, normalised decompile diff).
- The one exception: the playerState reader and writer (1.1 0x447860 / 0x446830, 1.5 0x44a630 / 0x449600) differ only in the size of the zeroed or copied playerState, 0x834 dwords against 0x833 (VERIFIED, same diff).
- The server's snapshot writer (1.1 0x45c2b0, 1.5 0x4613e0) has the same shape and passes a null base for a full frame in both (VERIFIED, call sites at 1.5 0x46154b).
- The 1.5 capture pair in `crates/common/tests/fixtures/net/` re-encodes byte for byte through vcod's 1.1 writer given 1.5's tables (`protocol_6_capture_reencodes_and_maps_pm_flags`, VERIFIED).

## 4. Netfield tables

`cod_lnxded` 1.5b holds the tables at 0x080db860 (entityState, 59), 0x080dbfe0 (clientState, 22) and 0x080dc560 (playerState, 103), and the objective and HUD-element tables from 0x080e9a80 (VERIFIED, `dump_field_table.py`). Against 1.1d's 0x080d1760, 0x080d2058, 0x080d229c and 0x080de384:

| Entry | 1.5 | 1.1 | Label |
|---|---|---|---|
| entityState, clientState, objective, HUD element | same name, offset, width | | VERIFIED |
| playerState [12] `pm_flags` | 21 bits | 19 bits | VERIFIED |
| playerState [76], offset 104 | `fJumpOriginZ` | `fJumpPeak`, same width | VERIFIED |
| playerState [86] `deltaTime` | offset 8392 | 8396 (struct layout only, not on the wire) | VERIFIED |

`CoDMP.exe` 1.5's own copies (0x580080 onward) differ from 1.1's in the same two playerState entries and nowhere else (VERIFIED, `netfields.py`). A 1.1 decoder reading a 1.5 snapshot desyncs on the first frame that changes `pm_flags` ("overran the message" warnings in the first live run, VERIFIED) and reads the rest correctly.

## 5. `pm_flags`

The low sixteen bits keep their 1.1 meaning except 0x2000. The bits above were renumbered. vcod rewrites a protocol 6 value into 1.1's layout on decode (`flags::pm_flags_v6_to_v1`, called through `Cod1Wire::pm_flags_to_v1`), so nothing past the decoder sees the difference.

| 1.5 | 1.1 | Meaning | Evidence |
|---|---|---|---|
| 0x0001..0x1000, 0x4000, 0x8000 | same | prone, crouch latch, jump held, ladder, ADS, back run, ADS walk, knockback, damage, 0x400, respawned, melee latch, lean, prone blocked | the same bit set and tested in the same functions of both modules (`PM_UpdateViewAngles` 0x1, 0x10, 0x8000; `PM_UpdateLean` 0x4000; `StopFollowing` 0x10020; the melee latch 0x1000 beside `PM_AddEvent(164)` at 1.1 0x38f68 and 1.5 0x301a4), INFERRED |
| 0x0004 | 0x0004 | 1.1's `PMF_PRONE_DIVE`; 1.5 never sets it | no write of 0x4 to `pm_flags` in the 1.5 export, INFERRED |
| 0x2000 | none | jump penalty: set with 0x8 by the jump (0x1f882) and by prone entry (0x24037), timed by `pm_time` up to 1800 ms, scaling friction (0x1eab3) and the next jump's speed (`cod15-movement.md` 2) | decompile, INFERRED; set on every takeoff in the moves capture, VERIFIED |
| 0x10000 | 0x10000 | following a player | `SpectatorThink` and `StopFollowing` in both, INFERRED |
| 0x20000 | 0x40000 | own view, a live client in its own body | `ClientEndFrame` (1.1 0x40e98, 1.5 0x3ab2a) sets it on the playing branch where 1.1 sets 0x40000; `GetFollowPlayerState`, `StuckInClient` and `BG_PlayerStateToEntityState` (1.1 0x2cbe8 tests 0x50000, 1.5 0x1c449 0x30000) test it, INFERRED; every frame of a spawned player in the live capture reads 0x20000, VERIFIED |
| 0x40000 | none | a spectator may change whom it follows | `SpectatorClientEndFrame` (1.5 0x39edd) sets it on a free follow and clears it on a forced one, INFERRED; a spectator in the live capture reads 0x40000, VERIFIED |
| 0x80000 | none | a following spectator may leave to free flight (`G_ClientCanSpectateTeam(4)`) | same function, INFERRED |
| 0x100000 | none | `disableWeapon` | the method table at file offset 0x82da4 pairs `disableweapon` with 0x408e0, which sets it, and `enableweapon` with 0x40983, which clears it, VERIFIED |
| (FOLLOW without 0x40000) | 0x20000 | forced follow | 1.1's `SpectatorClientEndFrame` (0x40760) sets 0x20000 on the branch where 1.5 clears 0x40000 and 0x80000, INFERRED |

- 1.1 never sets 0x2000, so the rewrite keeps it in place for the predictor's 1.5 rules.
- 0x100000 lands on `flags::PMF_WEAPON_DISABLED`, above 1.1's 19 bits.
- The follow permissions drop on the client's way in. The server keeps them as `PMF_MAY_CYCLE` and `PMF_MAY_FREE` and maps all three back on the way out (`flags::pm_flags_v1_to_v6`, section 13).

## 6. eFlags, entity types, events

- eFlags: the same bits set and cleared at the same sites (pingPlayer 0x80000, the friend ping 0x100000 in `ClientEndFrame`, talk 0x40000 off the usercmd's button 2 in `PmoveSingle`, the vote bit 0x20000 kept across a follow in `SpectatorClientEndFrame`, the firing bit 0x400), INFERRED. The netfield is 24 bits in both (VERIFIED).
- Entity types: players arrive as 1 and the carentan MG42s as 11, as on 1.1 (live capture, VERIFIED).
- A manned turret sends its arcs in `leanf` and `origin2`, and the gunner's view is held inside them (VERIFIED, `cod11-turrets.md` 14.3).
- A shot's client side is 1.1's. The cgame's `CG_CalcMuzzlePoint` (0x3003ab90) starts tracers and the whizby at the shooter's `tag_flash`, and the ridden body's at its unbobbed eye plus lean. It has no counterpart of the server's untruncated, bobbed `CalcMuzzlePoint` (VERIFIED, `cod11-events-and-fx.md` 6).
- A turret round's impact names the gun (live capture, VERIFIED, `cod11-turrets.md` 6.3), and its trace passes the gun, not the gunner (`Bullet_Fire` 0x6dfeb, VERIFIED, same section).
- `CG_FireWeapon` (0x3003a2a0) starts the fire sound at the shooter's `tag_flash`, as 1.1's (0x30038b70) and UO's (0x30047d20) do (`cod11-sound-system.md` 8, VERIFIED).
- Events: the `EV_*` name tables of 1.1 and 1.5 `cgame_mp_x86.dll` (202 entries) and `game_mp_x86.dll` are identical (`evtab.py`, VERIFIED). No event id needs remapping.

## 7. Configstrings

- The indexers walk the same bases. `G_FindConfigstringIndex` callers pass 16, 32, 64, 108, 128, 256, 524, 780, 844, 1100, 1244 and 1500, and `G_ModelIndex` reads from 0x10c (268) (1.5 game module, VERIFIED).
- A gamestate from each server on mp_carentan dm puts every block where 1.1 does: the cvar mirror at 140..203 and 204..267, models from 269, menus from 1180, localized strings from 1245 (live captures, VERIFIED).
- New: 1628 and 1629, the match timeout (section 12), written by `Cmd_MatchTimeout_f`, `Cmd_MatchTimein_f` and `CheckMatchTimeout` (0x4fea8) through `trap_SetConfigstring(0x65c, ..)` and `(0x65d, ..)` (VERIFIED). 1.1's cgame reads nothing there (its `CG_ConfigStringModified` ends its last block at 1627, VERIFIED), so vcod reads them on either protocol.
- The stock 1.5 gametypes register `serverinfo_<gametype>` as the first script menu, open it before the team menu, and open the team menu once it answers `close` (`maps/MP/gametypes/dm.gsc` in `pak9.pk3`, `ui_mp/scriptmenus/serverinfo_dm.menu` in `pak8.pk3`, VERIFIED).
- The live capture has `serverinfo_dm` at 1180 and the team menu at 1181, one later than 1.1 (VERIFIED). A `t <index>` names the menu by that index, not by `g_scriptMainMenu`, which still names the team menu (live, VERIFIED).
- The weapon list in configstring 7 on carentan dm is the same 32 names (live captures, VERIFIED).

## 8. Server commands

1.5's `CG_ServerCommand` (cgame 0x3002f660, 1.1 0x3002e0d0) handles 1.1's lowercase set unchanged and adds four (decompile, VERIFIED):

| Cmd | What it does | Evidence | vcod |
|---|---|---|---|
| `B` | `CG_MapRestart(0)` (0x3002df90). 1.5b's `map_restart` sends it in place of 1.1's `n`: `d 13`, `d 12`, `d 3`, `B`, then `d 1` with the new `sv_serverid`. `n` now calls `CG_MapRestart(1)`; the argument only decides whether the script menus reset | live, VERIFIED; the argument's meaning INFERRED | treats `B` as `n` |
| `C` | `record %s-%s` under `cg_autodemo` 1 | strings VERIFIED; the gate INFERRED from the branches | runs it (`play::demos`, `cod11-demos.md` 6) |
| `D` | `stoprecord` under `cg_autodemo` 1 | same | runs it |
| `E` | `screenshotJPEG %s-%s` under `cg_autoscreenshot` 1 | same | runs it (`screenshot::auto_command`, `cod11-demos.md` 10.5) |

- The arguments are the player's `name` stripped to letters and digits (0x3002f470) and the string from cgame trap 0xf4 (decompile, INFERRED). `C` puts the name first; `E` puts the stamp first, `screenshotJPEG <stamp>-<name>` (VERIFIED, the pushes at 0x3002faa5..0x3002fab9; `cod11-demos.md` 10.5). vcod's second argument is the local `yyyyMMdd-HHmmss`, trap 0xf4 at CoDMP15.exe 0x46af40 (VERIFIED).
- The 1.5b game module sends `C`, `D`, `E` reliably to one client from the player methods `autoDemoStart`, `autoDemoStop` and `autoScreenshot` (0x411bb, 0x41295, 0x4136f; their usage errors name them, VERIFIED).

## 9. Pmove

1.5 changed movement the client predicts (exported symbol sets of both modules, VERIFIED):
- the jump penalty of section 5;
- removal of `bg_duck2prone_time`, `bg_prone2duck_time`, `bg_prone_softyawedge` and the prone dive;
- the bob cvars moved into the game module (`cg_bobAmplitude*`, `cg_bobMax`, C variables `bg_bob*`, mirrored to clients);
- `BG_WeaponFireRecoil`.

`cod15-movement.md` has the function by function diff and the captures. vcod's predictor runs 1.5's rules on a protocol 6 connection (`pmove::MoveRules`).

The weapon channel under the combat script matches 1.5b sample for sample when `playerstate_combat_ab` replays retail's logged cmds on a protocol 6 server, with no gap (VERIFIED, gate, `mp_carentan-dm-combat-15.txt`, captured 2026-10-19). Capture: `--save-combat --capture-tag 15` beside `COD_VERSION=1.5 tools/run_server.sh mp_carentan`, the carbine, a walk, single shot, sustained fire, reload, crouched and prone shots. Compared: the shots, `weaponstate` runs, `weapAnim` and `torsoAnim` indices, torso flips, `weaponDelay` peak, sight fraction, spread counter and view, the walk included. A second take, `-combat-15-stall`, whose walk the probe's stall turn steered 45 then 90 degrees off its start, passes the same way off the logged view (`cod11-combat.md` 1.13; VERIFIED, gate).

## 10. Paks and pure servers

- Stock pak names are hexadecimal in 1.5. `FS_idPak` (1.5 `CoDMP.exe` 0x43e310) walks `pak0`..`pakf`, `mp_pak*` and `sp_pak*` with `%x`; 1.1's (0x43b6b0, `%s/pak%d`) walks `pak0`..`pak8` (decompile, VERIFIED). The 1.5 install ships `pak0`..`pak6`, `pak8`, `pak9`, `paka` and `pakb`.
- 1.1 and 1.5 `pak0.pk3` differ, so a client needs a 1.5 install to join a pure 1.5 server. From one, vcod's `cp` passes (live, `sv_pure 1`, VERIFIED). From a 1.1 install the server drops it with `EXE_SERVERDISCONNECTREASON\x14EXE_UNPURECLIENTDETECTED` (live, VERIFIED).
- A non-pure 1.5 server takes a client on a 1.1 install (live probe, VERIFIED), but references `pak8` and `pak9`, which that install lacks and where the 1.5 menus (`serverinfo_dm`) live. On tdm it references `main/pakb main/pak9 main/pak8 main/pak5 main/pak4 main/pak1 main/pak0` (live, VERIFIED).
- 1.5's `FS_ComparePaks` (`CoDMP.exe` 0x43e510) skips a name `FS_idPak` calls stock and one whose lower-cased name holds `_svr_` (0x43e490). 1.1's (0x43b830) skips only `FS_idPak`'s (decompile, INFERRED from the branches).
- `FS_idPak` compares the name with `%s/mp_bin`, `%s/pak%x`, `%s/mp_pak%x`, `%s/sp_pak%x` (1.1 `%d`) and a `localized_` name's rest with `_pak%x` (format strings VERIFIED; the loop bounds 0x10 and 9 INFERRED). So a 1.5 client never asks for a stock pak, whatever its install holds (INFERRED).
- `SV_WriteDownloadToClient` passes the download name, `.pk3` included, to `FS_idPak` (1.5b `cod_lnxded` 0x808b92a, name at client+0x10a64; disassembly, VERIFIED). The `EXE_CANTAUTODLGAMEPAK` refusal therefore never matches a stock pak (INFERRED). A 1.1 install on the 1.5b server that asked for `main/pakb.pk3` got `clientDownload: 0 : beginning "main/pakb.pk3"` and the stream (live, VERIFIED). 1.1d's download path calls its `FS_idPak` the same way (0x80709c0 on client+0x10a64; decompile, INFERRED).

vcod's client skips what the connection's protocol calls stock (`PakRules::id_pak_numbers` and `skips_svr_paks` on the `Transport`, `net::download::is_stock_pak`) and warns once per load which referenced stock paks no local pak matches by checksum. `vcod-server` serves a listed stock pak to a client that asks, on either protocol, as retail does.

## 11. Server browser

1.5's `joinserver.menu` (`paka.pk3`) lists nine columns where 1.1's lists six (menu files, VERIFIED). 1.5's `UI_FeederItemText` (ui_mp_x86.dll 1.5 0x4000ce00) switches on the column through the nine-entry jump table at 0x4000d22c (disassembly, VERIFIED). Per column, off the branches (INFERRED):

| col | 1.5 | 1.1 |
|---|---|---|
| 0 | `X` when `pswrd` | same |
| 1 | the `hw` icon, `ui_mp/assets/server_hardware_*` in the order unknown, linux_dedicated, win_dedicated, mac_dedicated, win_listen, mac_listen for 0..5; none outside | name |
| 2 | `hostname` when the ping is above 0, else `addr` | map |
| 3 | `mapname` | players |
| 4 | `%s (%s)` over `clients`, `sv_maxclients` | game type |
| 5 | `gametype`, `?` when empty | ping |
| 6 | `ui_mp/assets/punkbusterlogo` when `pb` | |
| 7 | `X` when `mod` | |
| 8 | the ping, `...` when 0 | |

- An icon cell returns empty text and a shader handle. 1.1's `Item_ListBox_Paint` (0x400159b0) draws a handle as a square of the column's width at `x + 4 + pos`, `y - 1 + elementHeight / 2` (decompile, INFERRED). 1.5's list box doing the same is INFERRED.
- `ServerSort <n>` sorts on column `n` in both. 1.5's `LAN_CompareServers` (`CoDMP.exe` 0x419c00) switches on keys 0..8 to `pswrd`, `hw`, hostname, mapname, clients, gametype, `pb`, `mod` and ping (disassembly, INFERRED from the jump table's targets).
- 1.5's master query asks `getservers 6`, so its list holds 1.5 servers only (INFERRED from section 1). No column marks a server's version (menu files, VERIFIED).

vcod reads the layout off the loaded menu's list box (nine columns mean 1.5's), fills the cells as above and sorts the extra columns by value. It lists 1.1 and 1.5 servers together and marks neither, as retail has no column for it.

## 12. Match timeout

1.5 adds the `matchtimeout` and `matchtimein` client commands (`ClientCommand` 0x46341, the `Q_stricmp` strings, VERIFIED). They work when `g_timeoutsAllowed` and `g_timeoutLength` are above 0; a team player calls one for its team, with `g_timeoutBank` and `g_timeoutRecovery` beside them (`Cmd_MatchTimeout_f` 0x45a91, decompile, INFERRED).

Measured on 1.5b, tdm, `g_timeoutsAllowed 2`, `g_timeoutLength 15000`, `g_timeoutRecovery 5000`, a probe on axis (live, VERIFIED unless marked):

- `matchtimeout` brings `cs 1628 "15000"`, `cs 1629 "PATCH_1_5_TIMEOUT_CALLED\x14GAME_AXIS"` and `e "PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\x15vcod"`. The cvar mirror's `timescale` value (227) changes; configstring 1 does not. The game sets `timescale` to `0` (the `trap_Cvar_Set` arguments, VERIFIED).
- Snapshots keep coming at 20 a second, all at the serverTime of the call, 43600 for 13 s.
- `matchtimein` brings `cs 1628 "5000"`, `cs 1629 "PATCH_1_5_TIMEOUT_ENDING\x15"` and `e "PATCH_1_5_TIMEIN_CALLED_PLAYERNAME\x15vcod"`. 5 s later `cs 1628 ""` arrives and serverTime moves again.
- `CheckMatchTimeout` sends `e "PATCH_1_5_TIMEOUT_EXPIRED"` and the recovery's 1628 when the length runs out, and sets `timescale 1` with an empty 1628 when the recovery does (decompile, INFERRED).

The cgame:

- `CG_ConfigStringModified` (0x3002db40) keeps 1628 as `atol` plus cgame trap 6 and clears a flag. 1629 goes through trap 0x39 (`"timeout string"`) into a 256-byte buffer. `CG_Init`'s reader (0x30024290) sets that flag for a nonzero 1628 (decompile, INFERRED).
- Trap 6 is `Sys_Milliseconds` in 1.1's `CoDMP.exe` (jump table 0x4041f0 to 0x4659d0, `timeGetTime`; VERIFIED). 1.5's being the same is INFERRED.
- `CG_Draw2D` (0x300199e0) calls the banner (0x300195c0) right after the vote, past the intermission (`pm_type` 5) return (decompile, INFERRED).
- The banner formats `"%s: %i \n%s"` over `CGAME_PAUSED`, the whole seconds left and the message, or `"%s\n%s"` with the flag set, and draws it at ((640 - width) / 2, (480 - height) / 2) in font 0 at 0.5, white, style 6 (strings and constants VERIFIED; which format runs when INFERRED from the branch). `CGAME_PAUSED` is "Paused" in 1.5's `cgame.str` (asset, VERIFIED).

The client's clock (`docs/protocol-1.1.md`, "The client's clock") runs `CL_AdjustTimeDelta` on each snapshot parsed, frozen or not, as Q3's `CL_ParseSnapshot` sets `cl.newSnapshots` for every valid one (INFERRED from the lineage). Over a frozen serverTime the adjusts pull the delta back as fast as local time pushes it, and the drawn time stops about 150 ms past the snapshot's (INFERRED, simulating the rule at 8 ms frames).

vcod draws the banner (`hud::timeout`), the line break as a second line under the first, its shadow vcod's usual 1 px one. It counts a new snapshot by message number, so the drawn time holds through a timeout. A 1.1 install lacks `CGAME_PAUSED` and the `PATCH_1_5_*` strings (`localized_english_pak5.pk3`) and shows the keys.

### The server's half

The game module (`game.mp.i386.so` 1.5b; `nm` addresses):

- `level` holds the timeout from 0x1744: the allied and axis banks (ms left), the timeouts each side called, the running timeout's length, its start (`trap_Milliseconds`), the recovery's end and the calling side (decompile of 0x45a91, VERIFIED). `G_InitGame` (0x4f10a) zeroes `level` and fills both banks from `g_timeoutBank`, so a map load or `map_restart` resets all of it (VERIFIED).
- `Cmd_MatchTimeout_f` (0x45a91) tests, in order (strings and branch order VERIFIED; refusals go to the caller alone):
  1. `g_timeoutsAllowed` and `g_timeoutLength` above 0, else `PATCH_1_5_TIMEOUT_NOTENABLED`;
  2. the caller playing on a side, else `MUSTBEPLAYING`;
  3. no timeout or recovery running, else `ALREADYINPROGRESS`;
  4. the side allies or axis, else `INVALIDGAMETYPE`;
  5. its count under `g_timeoutsAllowed`, else `MAXCALLED\x14<side>`;
  6. its bank above 0, else `MAXTIMEUSED\x14<side>`.

  Then the count goes up, the length is the smaller of the bank and `g_timeoutLength`, `timescale` goes to `0`, 1628 and 1629 are set and `e "PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\x15<name>"` goes to everyone.
- `Cmd_MatchTimein_f` (0x45f42) refuses with `NOTENABLED`, `NONEINPROGRESS`, `TIMEIN_MUSTBEPLAYING`, or `TIMEIN_WRONGTEAM` for a side other than the caller's. Otherwise the elapsed time comes off the bank, the recovery starts (`g_timeoutRecovery`), 1628 and 1629 change and `TIMEIN_CALLED_PLAYERNAME` goes to everyone. Both arms of its side test charge the allied bank (0x1744), whichever side called (decompile, VERIFIED); vcod keeps that.
- `CheckMatchTimeout` (0x4fea8) runs every frame at the end of `G_RunFrame`, after `CheckTeamStatus` and `CheckVote` (decompile, VERIFIED).
- `G_IsInMatchTimeout` (0x51043) is `level.frametime == 0` (VERIFIED). Its two callers are the turret think's fire block (0x52305), skipped whole, cooldown included, and its loop sound (0x523fd), which stops with the stop alias on every frame its timer still runs (decompile, INFERRED from the branches).

The engine (`cod_lnxded` 1.5b; virtual addresses):

- 1.5b keeps two clocks where 1.1d has `svs.time` alone: 0x83ccd88 and 0x83ccd84. `SV_Frame` (0x80942a1) steps the first by the frame (`1000 / sv_fps`) and the second by `Com_ModifyMsec` (0x80716b2) of it, per frame, then runs the game frame (disassembly, VERIFIED). The snapshot's `serverTime` is the scaled one and the snapshot pacing the other (live capture below, VERIFIED).
- 1.5's `Com_ModifyMsec` sets a negative `timescale` to `0`, always scales by it, truncates, and bumps a result under 1 to 1 only when `timescale` is not 0. 1.1d skips the scale at 0, so a 1.1 `timescale 0` runs at full speed (disassembly of 0x80716b2 against 1.1d's 0x806e32c, VERIFIED).
- `map` and `map_restart` set a `timescale` at or under 0 to 1 (0x80876aa, 0x8087777, VERIFIED). A restart in a timeout leaves 1628 and 1629 standing, since the table survives a restart and `G_InitGame` writes neither (INFERRED).

Live, 1.5b against `vcod-server` on the same paks, tdm, `g_timeoutsAllowed 2`, `g_timeoutLength 15000`, `g_timeoutRecovery 5000`, one axis probe sending `matchtimeout` twice, `say hi`, `matchtimein` twice (2026-10-14, the same sequence on both, VERIFIED):

- The call brings `d 1628 15000`, `d 1629 PATCH_1_5_TIMEOUT_CALLED\x14GAME_AXIS\x15`, the `e` line, and only then `d 227 0`, the mirror's `timescale` row, which the engine flushes at the top of the next `SV_Frame`.
- The second call gets `e "PATCH_1_5_TIMEOUT_ALREADYINPROGRESS"`, the second timein `e "PATCH_1_5_TIMEOUT_NONEINPROGRESS"`.
- `serverTime` stands through the timeout and the recovery (13250 on ours, 30200 on 1.5b, 19 s), snapshots still 20 a second. The recovery's end brings `d 1628 ""` then `d 227 1`, and `serverTime` moves on from where it stood.

vcod:
- `Server::sv_time_ms` is the game clock and `Server::svs_time_ms()` the network's (pacing, pings, timeouts, the flood window, downloads, heartbeats). On a protocol 6 server `Server::game_step_ms` is `Com_ModifyMsec`.
- The game half is `game::timeout`. The commands run inside the packet and send their configstrings at once, ahead of the `e` lines, as `trap_SetConfigstring` does. The frame check's go out before the frame's own configstring diff for the same reason.
- `crates/server/tests/timeout_15_ab.rs` replays a second 1.5b capture (`fixtures/timeout/mp_carentan-tdm-timeout-1.5.txt`: a call, a refused call and the timein a second apart) and holds ours to its command stream and its 142 snapshots at one `serverTime` (ours 140; the capture's commands left on the probe's wall clock).

## 13. Server side

What a server does differently to host 1.5 clients. `cod_lnxded` 1.5b addresses are virtual; `game.mp.i386.so` 1.5b ones are `nm` addresses. Live comparisons: the 1.5b server on loopback (`COD_VERSION=1.5 tools/run_server.sh`, a basepath of the eleven stock 1.5 paks and the localized ones) against `vcod-server` on the same paks, 2026-10-13.

### One protocol per process

Each retail binary checks the connect's `protocol` against its own and nothing else (`SVC_DirectConnect`, 0x8089ebc, INFERRED), so a 1.5b server takes 1.5 clients only and 1.1d 1.1 clients only. vcod does the same: one protocol per process, picked by `--set protocol=<1|6>` or by the install (6 when `main/pak0.pk3` has 1.5's checksum, 1048127331 as the 1.5b server's `sv_referencedPaks` lists it, VERIFIED). Mixing both on one server would need two script sets and two pak lists; no retail server does it.

### Collision arithmetic

VERIFIED, the `.comment` strings: 1.1d's `cod_lnxded` is `GCC: (GNU) 2.95.2` and 1.5b's `GCC: (GNU) 3.2.3`, unoptimized, storing every float variable 1.1d keeps on the x87 stack. On mp_carentan (the same bsp in both installs) the two put cracks in different places along a patch's facet seams, and a round 1.1d stops goes through on 1.5b. Its terrain point clip is UO's (0x8055763, as `coduo_lnxded` 0x8055595).

vcod's 1.5 server clips patches and terrain in 1.5b's arithmetic (`CollisionWorld::set_arith`; `bsp-ibsp59-format.md`, "Patch collision on 1.5 and UO"). Its brush clip keeps 1.1d's, which leaves a brush hit a bit or two off 1.5b's fraction.

### `getinfo` (`SVC_Info`, 0x8092a74)

The key strings, each referenced in turn from 0x8092a74, are VERIFIED. Each value and condition is read off its branch:

| Key | Value | When | Evidence |
|---|---|---|---|
| `challenge`, `protocol` (6), `hostname`, `mapname` | as 1.1 | always | INFERRED |
| `clients` | occupied slots, private ones included | non-zero | INFERRED |
| `sv_maxclients` | `sv_maxclients - (sv_privateClients - private slots in use)` | above 0 | INFERRED |
| `gametype` | `g_gametype` | always | INFERRED |
| `pure` | `sv_pure->integer` | non-zero | INFERRED |
| `minPing`, `maxPing` | as 1.1 | non-zero | INFERRED |
| `game` | `fs_game` | non-empty | INFERRED |
| `sv_allowAnonymous` | the integer | non-zero | INFERRED |
| `pswrd` | `1` | `g_password` non-empty | INFERRED |
| `ff`, `kc`, `timeoutsAllowed` | the string of `scr_friendlyfire`, `scr_killcam`, `g_timeoutsallowed` | its `atoi` non-zero | INFERRED |
| `hw` | `1`, a constant in the Linux binary (0x8092e6f) | always | VERIFIED |
| `pb` | `sv_punkbuster->integer` | always | INFERRED |
| `mod` | see below | always | INFERRED |

- 1.1 counts `clients` from `sv_privateClients` up and sends `sv_maxclients` less every private slot (`cod11-server-handshake.md`, "Private slots"). 1.5 counts all of them and gives back the private slots in use (the two loops from 0x8092a8e, INFERRED).
- `mod` starts 0 and goes to 1 when `sv_pure` is 0, when `fs_game` is set, or when a name in `sv_referencedPakNames` fails `FS_IdPak(name, "main")` (0x8092ecf..0x8092f6c, INFERRED from control flow).
- 1.5's `FS_idPak` (0x80754ea) accepts `mp_bin` and `pak`, `mp_pak`, `sp_pak` or `localized_<language>_pak` with one hex digit (format strings `%s/mp_bin`, `%s/pak%x`, `%s/mp_pak%x`, `%s/sp_pak%x`, `%s/localized_`, `_pak%x`, VERIFIED).
- vcod's reply to `getinfo xyz` on the 1.5 paks is byte for byte the 1.5b server's (live, VERIFIED).

### `getstatus` (0x8092538)

The serverinfo string, then `challenge`, `con_disabled` (the `sv_disableClientConsole` integer, only when non-zero), `pswrd` (`1` or `0`, always) and `mod`, then 1.1's `%i %i "%s"` player lines (format at 0x80dfe4b) (INFERRED from the branches; strings VERIFIED). vcod's reply matches 1.5b's byte for byte on the same paks (live, VERIFIED).

### Connect

- `SVC_DirectConnect` keeps 1.1's refusal strings (`EXE_BAD_CHALLENGE`, `EXE_ERR_HIGH_PING_ONLY`, `EXE_ERR_LOW_PING_ONLY`, `EXE_SERVERISFULL`, `error\n%s` for the game's verdict; the string set referenced from 0x8089ebc on, VERIFIED). It tests them in 1.1's order and changes only the version it names (INFERRED).
- It reads `cl_punkbuster` (an `atoi`) and `cl_guid` from the userinfo (0x808a46a, VERIFIED). Both go with the address to 0x80c191c, whose non-null answer is either relayed when it starts `error\n` or drops the connect silently (0x808a52b..0x808a59e, INFERRED). That this is PunkBuster's authorization is INFERRED from the arguments and `sv_punkbuster`.
- The guid in the log line `Going from CS_FREE to CS_CONNECTED for %s (num %i guid %i)` comes from the challenge entry (0x808a1d0), which only the CD key authorize answer fills (INFERRED). 1.5b logs `guid 0` for a LAN client (live, VERIFIED).
- vcod accepts the two keys and ignores them.

### Cvars

`tools/capture_cvars.py` against 1.5b, diffed with 1.1d's capture (VERIFIED):
- `protocol 6`, `shortversion 1.5`, `gamename "Call of Duty"` (1.1 `main`); `fs_game` gains `S`, `g_allowVote` gains `AL`.
- New rows: the `g_allowVote*` family, `g_autodemo`, `g_autoscreenshot`, `g_deadChat`, `g_maxDroppedWeapons`, the four `g_timeout*` (`g_timeoutsallowed` serverinfo), `sv_console_lockout`, `sv_disableClientConsole`, `sv_kickBanTime`, `sv_packet_info`, `sv_punkbuster` (`SRA`), the three `sv_www*`, and the `ui_*` mirrors of the vote, friendly fire, killcam and timeout settings.
- The game module also registers `scr_friendlyfire` and `scr_killcam` (`SRA`) and `scr_drawfriend` (`RA`) (`cvarlist scr_`, VERIFIED). That is why 1.5's serverinfo carries `g_timeoutsallowed`, `scr_friendlyfire`, `scr_killcam` and `sv_punkbuster` where 1.1's does not (gamestate fixture's configstring 0, VERIFIED).
- 1.5 registers no `sv_master*` row; vcod keeps them for its heartbeat.

The 140/204 mirror on 1.5 drops `bg_duck2prone_time`, `bg_prone2duck_time` and `bg_prone_softyawedge`, adds the four `cg_bob*` rows and `timescale`, and sorts by folded name: `ui_allow_thompson` sits at 180 and `ui_allowVote` at 181, which byte order would swap (`gamestate-1.5.bin`, VERIFIED).

Configstring 1 drops `timescale` and adds `sv_console_lockout`, `sv_disableClientConsole` and the `ui_*` rows, in folded-name order (gamestate fixture, VERIFIED). `G_SetUICvars` (0x4ec08) copies `g_allowvote`, the twelve `g_allowVote*`, `scr_drawfriend`, `scr_friendlyfire`, `scr_killcam` and the four `g_timeout*` into them (decompile, VERIFIED); `G_InitGame` calls it at every level load (INFERRED).

With `sv_pure 1` both servers send the same `sv_pakNames` and `sv_paks`, `pakb` down to `pak0` (live, VERIFIED). Unlike 1.1, 1.5b's `sv_referencedPakNames` names only the paks the level read a file from, plus any outside `main`, and its systeminfo is bounded by `BIG_INFO_STRING`, not `MAX_INFO_STRING`. vcod builds both the same way (`cod11-server-handshake.md`, "The four cvars" and "Configstring 1, systeminfo").

### Script builtins the stock 1.5 gametypes call

The 1.5 scripts (`pak9`, `paka`, `pakb`) call seven player methods and one entity method 1.1 lacks. The player method table is at 0x82ca0, 54 entries (VERIFIED).

- `getGuid` (0x4110b): `trap_GetGuid`, the engine's guid (0 without an authorize server, see Connect) (decompile, VERIFIED).
- `disableWeapon` (0x408e0) and `enableWeapon` (0x40983) set and clear `pm_flags` 0x100000 (decompile, VERIFIED). Pmove tests it beside the ladder's 0x10 at the weapon change (0x2f7be) and the raise (0x2e43d) and holsters the same way (INFERRED from control flow). `sd.gsc` brackets the plant and the defuse with them.
- `allowSpectateTeam(team, allowed)` (0x40f93) keeps a mask at `gclient+0x2174`: `none` 1, `axis` 2, `allies` 4, `freelook` 0x10 (the `scr_const` strings at +0xf8, +8, +4 and +0x100 are `none`, `axis`, `allies` and `freelook`, VERIFIED). Allowed clears the bit, refused sets it (INFERRED). `G_ClientCanSpectateTeam(client, team)` (0x3a36d) is `!(mask >> team & 1)`, team 4 being free flight (VERIFIED). `_teams::SetSpectatePermissions` calls it on every team change.
- What the mask does (INFERRED from control flow):
  - `SpectatorThink` (0x389eb) lets the sight edge end a follow only with free flight allowed, and gives the spectator `ps.speed` 0 without it.
  - `Cmd_FollowCycle_f` (0x42aef) skips a client whose team is refused.
  - `SpectatorClientEndFrame` (0x39edd) drops a follow of a refused team, and cycles a spectator following nobody without free flight onto someone.
  - The follow frame carries 0x40000 for a follow the script did not force, and 0x80000 beside it with free flight allowed. A free spectator carries 0x40000 when it may watch team 0, 1 or 2 (section 5).
- `autoDemoStart`, `autoDemoStop` (0x411bb, 0x41295) under `g_autodemo` and `autoScreenshot` (0x4136f) under `g_autoscreenshot` send the reliable `C`, `D` or `E` (strings at 0x781d4, 0x781f2, 0x78223, VERIFIED) and return 1. With the cvar at 0 they do nothing and return nothing (INFERRED).
- `getAttachIgnoreCollision(index)` (0x5ceeb), and `attach`'s third argument:
  - `G_EntAttach` (0x6b9b9) keeps it as one bit per attachment at `gentity+0x17a` (decompile, VERIFIED). `_utility::saveModel` and `loadModel` carry it across the killcam.
  - `G_DObjUpdate` (0x6b71c) and `BG_UpdatePlayerDObj` (0x1afc9) hand the bit to the engine beside each attached model (VERIFIED), as the fourth word of each 16-byte model record `trap_DObjCreate` takes (UO's `G_DObjUpdate` 0x87df9, VERIFIED decompile).
  - The engine's `DObjCreate` (1.5b `cod_lnxded` 0x80d12e6, UO `coduo_lnxded` 0x80c667a) ORs `1 << i` into the DObj's `+0x18` mask for each flagged record i (1.5b 0x80d146c, UO 0x80c6800). The hit-box walk (1.5b 0x80d2000, UO 0x80c7394, the walk of `cod11-combat.md` 3.1) ANDs its model index into that mask (1.5b 0x80d215d, UO 0x80c74f1) (VERIFIED decompile and disassembly).
  - INFERRED, off the branch on that AND: a flagged model's boxes are never tested, while its bones are still walked, so the models after it keep their parents.
  - INFERRED: the mask is set by record index and read by the index the model was stored at. These differ only after `DObjCreate` drops a record for a missing tag.
  - The mask's one-bit accessor (1.5b 0x80d01ce, UO 0x80c5562) has no direct call in either server binary (VERIFIED disassembly).
  - No stock 1.5 script passes true. vcod's hit rig zeroes such a model's boxes (`hitrig::Assembly::ignore_collision`).
- `takeWeapon` (1.1 0x4493c, 1.5 0x3d749) is in both modules: the weapon's reserve and clip zeroed, then `BG_TakePlayerWeapon` (decompile, VERIFIED). No 1.1 stock script calls it; 1.5's `_teams::givePistol` does on every spawn.

With those, a protocol 6 server on the 1.5 paks builds 1.5b's whole configstring table for mp_carentan dm, slot for slot, the referenced pak pair included (`crates/server/tests/configstrings_15_ab.rs` against `gamestate-1.5.bin`, VERIFIED; runs when `COD_DIR` is a 1.5 install).

### Chat

1.5's `G_Say` (0x42dc9) differs from 1.1's (0x46c14) in four places (decompile, VERIFIED; strings at 0x786bd..0x787d0):

- The status ahead of the name: `\x15(\x14GAME_SPECTATOR\x15)` on the spectator team, else `\x15` and the side's colour (`^9` axis, `^8` allies, nothing on neither), then `(\x14GAME_DEAD\x15)` when the session state is not playing. 1.1 tests health instead and colours nothing.
- `say_squad` (`ClientCommand` 0x46341, `Cmd_Say_f` 0x434b7 with mode 3): a speaker in squad `squad_alpha` or `squad_bravo` (the `sessionsquad` client field, set by 0x3b515) gets `%s(\x14<side>\x15 \x14PATCH_1_5_SQUADALPHA\x15)%s%s: ` in colour 5 as an `h` line to `InSameSquad` (0x69987) clients. Any other speaker's squad say becomes a team say, without the team say's own fall back to a plain say.
- `G_SayTo` (0x42c8b) lets a speaker out of play reach a playing client when `g_deadChat` is not 0.
- The log lines (`say;%d;%d;%s;%s`) carry the guid.

Live, 1.5b against `vcod-server`, tdm: an axis probe saying `say_squad`, `say_team`, `kill`, two plain says and a `say_squad` while dead, an allied probe listening, `g_deadChat 1` set over rcon between the two dead says (2026-10-14, identical on both, VERIFIED):

    i "\x15\x15^9(\x14GAME_AXIS\x15)vcod^7: ^5sq"
    h "\x15\x15^9(\x14GAME_DEAD\x15)vcod^7: ^7dead1"
    i "\x15\x15^9(\x14GAME_DEAD\x15)(\x14GAME_AXIS\x15)vcod^7: ^5dsq"

Only `dead2`, said after `g_deadChat 1`, reached the living allied probe. No stock 1.5 script sets `sessionsquad` (`pak9`..`pakb`, VERIFIED).

The field (1.5b setter 0x3b515, getter 0x3b5e4; UO's hooks 0x43803 and 0x438d2, `dump_script_fields.py`, carry the same error string and were not decompiled):

- VERIFIED (decompile): a `u16` at `gclient+0x217c` holding 0, 1 or 2. The setter takes a const string and stores 1 or 2 for two of them and 0 for a third. Anything else is the script error `'%s' is an illegal sessionsquad string. Must be squad_alpha, squad_bravo, or none.` It then calls `ClientUserinfoChanged`. `ClientConnect` (0x4c680) stores 0.
- INFERRED, off the error's wording and `sessionteam`'s use of the same `scr_const` slot for `none`: 1 is `squad_alpha`, 2 `squad_bravo`, 0 `none`, which the getter reads back.
- VERIFIED (decompile and the format at 0x787d0): a squad say from a speaker in squad 1 or 2 is `%s(\x14<side>\x15 \x14PATCH_1_5_SQUAD<ALPHA|BRAVO>\x15)%s%s: ` in colour 5, an `h` line (`G_SayTo` 0x42c8b picks `i` for mode 1 only), to clients `InSameSquad` (0x69987) finds: on the speaker's team (`OnSameTeam`) and in his squad, which is not 0. UO's strings and `InSameSquad` are the same.

vcod-server keeps the field on protocols 6 and 22 (`GameHost::client_squad`); on 1.1 the name is a plain script variable.

### PunkBuster on the Linux server

The 1.5b Linux server has no `pbsv.so` here and runs without PunkBuster (section 2). Live against it (2026-10-14, VERIFIED):

- `+set sv_punkbuster 1` on the command line prints `sv_punkbuster is read only.` and `getinfo` answers `pb 0`. `+set protocol 5` is refused the same way; `+set shortversion 9` is not.
- `set`, `seta` and a bare `sv_punkbuster 1` over rcon answer `sv_punkbuster is read only.`
- `pb_sv_enable`, `pb_sv_ver`, `pb_sv_kick 0` and any other unknown command answer an empty `print`.
- An out-of-band `PB_` packet gets nothing.

`vcod-server` on 1.5 answers each the same way: `Server::set_cvar` refuses `sv_punkbuster`, the console refuses the read-only cvar, and an unknown command prints nothing over rcon.

### Spawn events

- Retail 1.5b's spawn sends no event. A probe joining dm or tdm, and one respawned by `scr_forcerespawn` after a `kill`, reads `eventSequence` 0 and an empty ring from its first playing snapshot. The death frame reads `EV_DEATH` and `EV_RAISE_WEAPON` (2026-10-14, VERIFIED). `vcod-server` sends the same.
- The `--probe-moves` run over `client-probes/probe_prone` drains the same events on both (41, then 87 and 110 per jump, 18).
- The dead playerstate's `weaponstate` is no 1.5 difference. It is 1 when one cmd reached the server behind the `kill` inside its frame and 0 when two or more did, on 1.1d, 1.5b and UO alike, and ours follows the same rule (2026-10-17, VERIFIED, run: `--probe-kill-cmds 1,3`, `kill_cmds_ab.rs`; `cod11-combat.md` 9.2, "The death frame's `weaponstate`").

### Missiles

- 1.5b's `G_RunMissile` sends every trace through `G_MissileTrace` (0x54a18), `trap_LocationalTrace` under `bulletPriorityMap` as 1.1's traces are. It opens with a fall test 1.1 lacks: a resting missile whose ground is not the world falls once nothing is 1.5 below it. Addresses and the live capture are in `cod11-combat.md` 12.6.
- VERIFIED there, live: the throw leaves from z 36.148 where 1.1's leaves from 37.0 on the same spot.
- 1.5's `CalcMuzzlePoint` (0x7e91f) starts from the unsnapped origin, adds the walk bob and never truncates. Shots, rockets, throws and the melee swing all take it (`cod11-combat.md` 2.1, "1.5 and United Offensive"), and so does vcod-server's on protocols 6 and 22.

### Not modelled

- The parts of 1.5's movement `cod15-movement.md` leaves out. The rest runs on a protocol 6 server for every client, bots included, as the 1.5 game module's one pmove does (`MoveRules::V1_5`, through `MoveRules::for_protocol`). The wire carries the jump penalty's 0x2000 and `pm_time`, the stepped `viewHeightTarget` and the `eFlags` stance that follows it. The bots' nav graph builds and walks under the server's rules (`NavJob::start` with `Server::move_rules`).
- Live, the `--probe-moves` script against `vcod-server` on the 1.5 paks jumps 39.7, 39.5, 16.4, 18.1, 40.8, 16.2, 17.8 and 39.3 units. The 1.5b capture (`mp_carentan-dm-slope-8ms-moves-1.5.txt`) jumps 40.3, 39.3, 16.6, 18.1, 40.6, 16.7, 18.1 and 39.7, and 1.1d about 40 every time. Penalty bit on 400 snapshots against 1.5b's 399 (2026-10-13, VERIFIED).
- PunkBuster: refused as the Linux 1.5b server refuses it (above).
- Votes, including the kinds the `g_allowVote*` rows gate, run on 1.5 (`game::vote`, `cod11-votes.md`). HTTP download redirects run too (section 14).

## 14. HTTP downloads

1.5 and United Offensive 1.51 add the www download redirect: a server with `sv_wwwDownload 1` hands a client that says `cl_wwwDownload 1` a URL instead of streaming the pak over `svc_download`. UO's server and client run the same code (`coduo-diff.md` 23); this section covers both.

Sources: 1.5b `cod_lnxded` disassembly, Ghidra exports of UO `coduo_lnxded` and of 1.5 and UO `CoDMP.exe` / `CoDUOMP.exe`, and live runs on loopback (2026-10-15): the 1.5b and UO servers on a copy of the stock paks plus `nsf_mp_j5outpost.pk3` (a third-party map, run as `mp_j5outpost` dm), `python3 -m http.server` serving the same pak, and `vcod --net-probe ... --probe-download <scratch dir>`.

### The server

- The redirect is one `svc_download` op: byte 6, short -1, string `"%s/%s"` of `sv_wwwBaseURL` and the requested name (`.pk3` included), long file size, long flags (UO `SV_WriteDownloadToClient` 0x808c559, decompile, VERIFIED; 1.5b's writer at 0x808b6fc writes the same ops, disassembly, VERIFIED). Flag bit 0 is `sv_wwwDlDisconnected != 0` (VERIFIED, same function). Nothing in either server sets bit 1 (INFERRED: the one write of the flags long).
- Live, with `sv_wwwBaseURL "http://127.0.0.1:31072"`, both servers log `Redirecting client 'vcod' to http://127.0.0.1:31072/main/nsf_mp_j5outpost.pk3` and send `http://127.0.0.1:31072/main/nsf_mp_j5outpost.pk3`, 953499, flags 0. With `sv_wwwDlDisconnected 1` UO sends flags 1. The 1.5b and UO messages are byte for byte the same (VERIFIED, `crates/common/tests/fixtures/net/www-redirect-1.5.bin`, `www-redirect-uo.bin`, `www-redirect-uo-discon.bin`, each the decoded message: the `u32` reliable acknowledge then the Huffman ops).
- The engine's command line cuts a token at `//`, so `+set sv_wwwBaseURL http://host` leaves `http:` and the URL arrives as `http:/main/...` (live, VERIFIED). Quote the value (`'"http://host:port"'` through the shell).
- `SV_UserinfoChanged` reads the client's `cl_wwwDownload`: a non-empty value whose `atoi` is not 0 sets client+0x10c04 on 1.5b (0x808c70e, disassembly, VERIFIED) and client+0x10b04 on UO (0x808d2c7, decompile, VERIFIED).
- With `sv_wwwDownload` on and that flag clear, both servers call a stub that returns 1 (1.5b 0x808b89f, UO 0x808c54f, VERIFIED). On that 1 they return before the `Client '%s' is not configured for www download` print, which is never reached (INFERRED from the branch). Then they differ:
  - UO opens the pak only after the www branch, so it sends nothing, ever. Live, it logged `clientDownload: 0 : begining "main/nsf_mp_j5outpost.pk3"` on every frame, 4586 times, and the client timed out at 0 bytes (VERIFIED). A vcod client that does not send `cl_wwwDownload` stalls this way on a UO server.
  - 1.5b opens the pak into client+0x10aa4 before the www branch when `sv_allowDownload` is on and the name is not an id pak (0x808b95d), and the next frame finds the handle open and streams blocks (disassembly, INFERRED). Live, a client without `cl_wwwDownload` got the UDP transfer (VERIFIED). The same open precedes a redirect, so a 1.5b server can stream blocks between the redirect and the client's `wwwdl ack` (INFERRED; on loopback the ack won the race).
- The server writes the redirect every frame until `wwwdl ack` sets client+0x10c0c (UO); after that the function returns at once (decompile, INFERRED from the early return).
- `wwwdl` (UO 0x808c298), decompile, INFERRED from the branches; the subcommand strings are in both servers, VERIFIED:

  | Subcommand | Effect |
  |---|---|
  | any, before a redirect | drop with `GMI_EXE_UNEXPECTEDWWWDOWLOADMESSAGE` |
  | `ack` | marks the transfer; a second prints `WARNING: dupe wwwdl ack` |
  | `bbl8r` | drop with `GMI_EXE_DOWNLOADDISCONNECTED` (live: `0:vcod GMI_EXE_DOWNLOADDISCONNECTED` and a zombie, VERIFIED) |
  | `done`, after an ack | clears the download name |
  | `fail`, `chkfail`, after an ack | clear the name, set the fallback flag (+0x10c10), resend the gamestate (0x808bc48) |
  | anything else | drop |

- UO's fallback branch clears the flag and returns through the same stub, so the next frame redirects again: on UO a failed www download never falls back to UDP (decompile, INFERRED). VERIFIED live 2026-10-20, connected (`sv_wwwDlDisconnected 0`), the wrong pak behind the URL (`crates/server/tests/fixtures/handshake/www-refuse-uo.txt`): after `chkfail` the server resent the gamestate, logged `clientDownload: 0 : begining` on two messages and redirected on the second. After the client's `wwwdl fail`, the same again. No block went over UDP.
- `wwwdl` is the last row of UO's ucmd table, after `retransdl` (0x80f4fc8, string 0x80e4c1b, handler 0x808c298; VERIFIED). Subcommands compare case-blind (`Q_stricmp`, 0x8086a5a; VERIFIED). A `download` closes the transfer and keeps the name (0x808c25d) but touches none of the three www flags, so they outlive a transfer (decompile, INFERRED from the stores).
- Live on loopback (UO, `mp_j5outpost`, 2026-10-15) the server took a `wwwdl chkfail` after the ack and printed `WARNING: client 'vcod' reports that the redirect download for 'main/nsf_mp_j5outpost.pk3' had wrong checksum.` and `you should check your download redirect configuration.` (VERIFIED). The redirect itself prints through `Com_DPrintf`, so `Redirecting client` is not in a `developer 0` log (VERIFIED, the run's log).

### The client

- `CL_ParseDownload` (UO `CoDUOMP.exe` 0x418f90; 1.5 0x417d80 with the redirect in 0x417b10) takes block -1 as the redirect while no www transfer runs. It keeps the requested name and reads URL, size and flags; on a repeat it only eats the three fields (decompile, INFERRED from the branches).
- Flag 2 opens the URL in a browser (1.5: `ShellExecuteA "open"`) and sends `wwwdl bbl8r`. Otherwise it sets `cl_downloadSize`, sends `wwwdl ack`, refuses a name on its bad-checksum list (`refusing redirect to %s by server (bad checksum)`, `wwwdl fail`), starts the transfer (`Failed to initialize download for '%s'`, `wwwdl fail`), and on flag 1 sends `wwwdl bbl8r` and marks the transfer disconnected (decompile, INFERRED from the branches; the quoted strings are in both executables, VERIFIED).
- The transfer is libwww (`\Projects\Cod\Src\3rdParty\libwww` paths in `CoDUOMP.exe`). Its protocol modules are HTTP, FTP, Gopher, News and File, with no TLS module (strings, VERIFIED; no `https` support is INFERRED).
- `CL_WWWDownload` (UO 0x413420) holds `wwwdl done`, `wwwdl fail` and `Download failure while getting '%s'` (decompile, VERIFIED).
- On success it renames the temp file into the requested name and, connected, sends `wwwdl done`; disconnected, it calls 0x42bc10, the reconnect in ET's lineage. On failure, connected, it prints the failure and sends `wwwdl fail`; disconnected, it drops with that text (decompile, INFERRED from the branches). A connected success also appends `@<name>` to a redirected list (`ERROR: redirectedList overflow`, decompile, VERIFIED).
- Neither client sends `wwwdl chkfail`: the string is in the 1.5 and UO servers and in neither `CoDMP.exe` 1.5 nor `CoDUOMP.exe` (strings, VERIFIED).
- The checksum is checked later. `CL_WWWBadChecksum` (UO 0x4136e0, 1.5 0x412570) prints `WARNING: file %s obtained through download redirect has wrong checksum` and `this likely means the server configuration is broken` for a name on the redirected list, and adds it to the bad-checksum list `refusing redirect` reads (decompile, VERIFIED strings).
- Its one caller is `FS_ComparePaks` (UO 0x440630), in the branch that builds the "missing files" warning, for a referenced pak whose name exists with another checksum; it deletes that file (decompile, INFERRED). `CL_InitDownloads` (UO 0x411c20) takes that branch only with autodownload off or refused by the server. With it on, the same pak is asked for again under `<name>.<checksum %08x>.pk3` (INFERRED from the branches). So a retail client keeps a wrong pak from a redirect and either fails the pure check or fetches it again.

### What vcod does

- On protocols 6 and 22 the userinfo ends `\cl_wwwDownload\<value>`. The client cvar `cl_wwwDownload` (default 1, archived, userinfo, as `CL_Init` registers it) sets the value; a change once connected goes out in a `userinfo` command. With 0 a 1.5b server streams the pak over UDP and a UO one sends nothing (above).
- `NetClient::parse_www_redirect` reads block -1. A redirect on protocol 1, one with no download asked for, or one with flag 2 drops with a reason. Otherwise it sends `wwwdl ack` and fetches the URL over plain `http://` (`net::http`: HTTP/1.0 on a thread, Location hops followed, other schemes refused) into the UDP path's spool, which validates the zip and renames it. `wwwdl done` or `wwwdl fail` follows. UDP blocks during a www transfer are ignored.
- With flag 1 it sends `wwwdl bbl8r`, keeps fetching through the drop, and starts the handshake over when the pak lands. `donedl` then waits for the gamestate the reconnect brings, and a further pak is asked for once it arrives.
- Live on loopback the probe fetched the pak on both servers, connected and disconnected (UO), and its checksum matched `sv_referencedPaks` (2026-10-15, VERIFIED).
- Every finished download, www or UDP, is checked against its `sv_referencedPaks` checksum before it is renamed into place (`net::download::Download`); a mismatch deletes it. Over www a connected client then sends `wwwdl chkfail`, which retail's server takes as a failure. Retail's client never sends it; vcod does because the server's handler exists and the alternative is to load a pak the server did not list. Live against the UO server with the wrong pak at the URL: `chkfail`, the server's warning, nothing left in the download directory (2026-10-15, VERIFIED).
- The name then goes on a bad-checksum list, as `CL_WWWBadChecksum` puts it on retail's, and the transfer starts over inside the net client. The gamestate the server resends after `chkfail` re-asks for the pak and is not reported to the loader, which still waits on that download. A disconnected transfer reconnects and asks again the same way.
- A redirect of a listed name is acked and refused with `wwwdl fail`, retail's order, and its repeats are eaten until the next gamestate. On 1.5b (and vcod-server on protocol 6) the fallback flag sends the second request over UDP; that transfer is checked too, and a second mismatch drops. A UO server redirects again after the refusal, which would loop forever, so a second redirect of a refused name drops (`net::tests::a_www_pak_with_the_wrong_checksum_is_asked_for_again_over_udp`, `a_redirect_of_a_bad_checksum_pak_is_refused_once_then_left`).
- Live on loopback (2026-10-15, `mp_j5outpost` dm, another pak behind the URL under the map pak's name, `--probe-download`), VERIFIED:
  - 1.5b, connected: the probe said `chkfail`, the server printed its warning (name already cleared: `redirect download for ''`), logged `clientDownload: 0 : beginning` for the re-request and streamed the pak over UDP without a second redirect. It landed with the listed checksum and the probe went Active.
  - UO with `sv_wwwDlDisconnected 1`: the wrong pak, the reconnect, a redirect the probe refused with `wwwdl fail`, a second redirect at once, then the probe left. UO does not fall back to UDP.
- Rerun 2026-10-20, connected on both (VERIFIED, captures): 1.5b took `chkfail` and streamed the re-request over UDP, 467 block lines, one HTTP GET (`crates/server/tests/fixtures/handshake/www-chkfail-15.txt`). UO redirected again, the probe acked and refused it with `wwwdl fail`, UO redirected the next request again and the probe left, one HTTP GET (`www-refuse-uo.txt` beside it). Retail's 1.5 client reaches the same UDP transfer by a longer road: it keeps the wrong pak, deletes it at the next gamestate's `FS_ComparePaks` and asks again (INFERRED, above).
- `--probe-download` reopens its search path over the install and the download directory after each pak lands, and sends `cp` from it. Live on a pure UO server (`sv_pure 1`) after the redirect, `cp` passed and the probe stayed in for 20 s (2026-10-15, VERIFIED).

### vcod-server

- On protocols 6 and 22 `sv_wwwDownload`, `sv_wwwBaseURL` and `sv_wwwDlDisconnected` are read per message. A client whose userinfo `cl_wwwDownload` is non-zero gets the redirect on every message until `wwwdl ack`, then nothing until `done`, `fail` or `chkfail` (`download::ServerDownload::write`, `download::wwwdl`).
- The message is retail's byte for byte: the three fixtures above come out of `ServerDownload::write` unchanged (gate `the_redirect_is_retails_byte_for_byte`).
- `fail` and `chkfail` resend the gamestate. `bbl8r` drops with `GMI_EXE_DOWNLOADDISCONNECTED`, logged live as `0:vcod GMI_EXE_DOWNLOADDISCONNECTED`, as retail's.
- One choice differs from UO's server and follows 1.5b: a client without `cl_wwwDownload` gets the pak over UDP rather than nothing. A retail UO client sees no change, since it sends `cl_wwwDownload 1`.
- The request after a failed www transfer goes over UDP on protocol 6 and, on 22, waits a message and is redirected again, as each retail server does (`WwwConfig::udp_fallback`). `www_download` gates both against the captures above (`a_chkfail_on_1_5_falls_back_to_udp`, `a_refused_redirect_on_uo_is_redirected_again_until_the_client_leaves`). Live on loopback (2026-10-20), vcod-server on the UO install, connected, the wrong pak at the URL: the probe saw retail's sequence, one GET, three redirects, `fail` and the leave (VERIFIED).
- Live on loopback (2026-10-15): vcod-server on the UO install with `mp_j5outpost`, `sv_pure 1` and the pak behind `python3 -m http.server`. The probe was redirected, fetched it, passed `cp` and stayed in. With `sv_wwwDlDisconnected 1` it was dropped, fetched, reconnected and passed (VERIFIED). `www_download` drives the same over an in-process transport.

## 15. What the snapshot builder culls

1.5b's and United Offensive's `SV_AddEntitiesVisibleFromPoint` (`cod_lnxded` 1.5b 0x8095c30, `coduo_lnxded` 0x8096b97) and their `SV_inSnapshot` (1.5b 0x808d5d2, UO 0x808e3a9) add two tests to 1.1's. The compass side, measured live on all three, is `cod11-hud-protocol.md` 9. 1.1d's builder makes no game call (VERIFIED: no `vmMain` 0x17 call in the 1.1d decompile, and 1.1d's `vmMain` has no case past 0x14).

- **The broadcast time.**
  - Before the cluster test the builder reads a gentity field after `r.singleClient` (1.5b `+0xfc`, UO `+0x100`, `r.broadcastTime`). Non-zero and either negative or not yet passed by `svs.time`, the entity is sent unculled; passed, it is zeroed (VERIFIED disassembly, 1.5b 0x8095d80..0x8095dc6; UO decompile).
  - The game writes it in three builtins and nowhere else (VERIFIED decompile, every store to the field): `playSound` the level time plus 300 (1.5b 0x5d836, UO 0x68abb), `playLoopSound` -1 (0x5d8a9, 0x68b2e), `stopLoopSound` the level time plus 300 (0x5d908, 0x68b8d).
  - None of them touches `svFlags`, so 1.1's permanent `SVF_BROADCAST` and its `SVF_NOCLIENT` clear (`cod11-sound-system.md` 9) are gone: a sounded `script_origin` stays hidden.
  - INFERRED: `DAT_084f7004`, what the time is compared with, is `svs.time`; the same global dates the archived frames.
- **The fog cull.**
  - The builder asks the game for `vmMain` 0x17 (1.5b 0x8095c79, UO 0x8096be0), which answers `level` `+0x2204` (1.5b) or `+0x2208` (UO) (VERIFIED decompile, `vmMain` 1.5b 0x4d888, UO 0x56ac0).
  - `G_setfog` (UO 0x4a59a, 1.5b 0x41b0a) sets that to `FLT_MAX`, then reads the fog configstring with `"%f %f %f %f %f %f %f"`. With all seven read and the density (third) at least 1.0, it stores `((far - near) + near)` squared (VERIFIED decompile; 1.0 at UO 0x9e95c, file bytes). `setCullFog` writes density 1 and `setExpFog` its density under 1, so only cull fog culls.
  - The builder reads `FLT_MAX` (UO 0x80e66f4, file bytes) as 0, and 0 as no cull. Otherwise, after the clusters pass, it drops an entity whose `r.absmin`/`r.absmax` box is farther than that from the view point (0x806ab7e: per axis, the smaller squared distance to the box's two faces, counted only where the box lies wholly to one side).
  - VERIFIED live, UO 1.51b, `client-probes/probe_uocull` on mp_berlin, 2026-10-16: `setCullFog(0, 500, ...)` took every entity past 500 units off the viewer's list on that frame, and `setExpFog` put them back. vcod-server matches it frame for frame (`uocull_ab`).
- **A UO sound blend** is tested as its vehicle (`coduo-diff.md` 9).

Measured live on 1.5b, `client-probes/probe_cull15` on mp_carentan, 2026-10-16 (`tests/fixtures/entities/cull15-carentan.txt`, one `--probe-edge watch` viewer at (-584, 1872, 8), script models carrying `xmodel/weapon_mp40`). Each item VERIFIED live:

- A script model out of the PVS rides from the frame of its `playSound` and leaves 350 ms later (seven snapshots at `sv_fps` 20). Sounded twice 200 ms apart it leaves 550 ms after the first: the second stamp replaces the first.
- A sounded `script_origin` never rides, out of the PVS (1872, 928) or in it (-1136, 1504).
- `playLoopSound` brings an out-of-PVS model in for as long as it loops; `stopLoopSound` keeps it 350 ms more.
- Under `setCullFog(0, 1000, ...)` the in-PVS model 2100 units off leaves on the fog's frame and the one 490 off stays. A model moved from 2100 off to 560 off (`origin` write) comes in on that frame; one moved from 490 off to 2470 off goes on it.
- The broadcast time skips the fog test too: a model out of the PVS and past the fog rides 350 ms after `playSound` and for as long as `playLoopSound` loops, as without fog.
- `setExpFog` brings every culled entity back on its frame.

vcod-server matches every row, timing included (`cull15_ab`). Trap: a 1.5 server must get its own `defs::Defs::of(proto)` set. Handed 1.1's, which carries `PROTOCOL_V1`, `playSound` runs 1.1's permanent `SVF_BROADCAST` (`cod11-sound-system.md` 9) and shows the script origins.

## What vcod does with this

Client:
- `net::protocol::PROTOCOL_V6` beside `PROTOCOL_V1`: version, short version, the tables (1.1's with the wider `pm_flags`, `net::fields_v6`) and the `pm_flags` rewrite. A `NetClient` holds one; `NetClient::protocol` hands it to everything that decodes.
- The client connects as its install's protocol and, refused with `EXE_SERVER_IS_DIFFERENT_VER` naming a version it speaks, starts the handshake over in that one. The browser sends `getservers` for both.
- `B` restarts like `n`. The join probe and `--team` close `serverinfo_<gametype>` and resolve a `t` index through the menu configstrings.
- Stock paks follow the protocol (section 10), the browser fills 1.5's columns (section 11), and the match timeout draws its banner and leaves the clock standing (section 12). PunkBuster servers refuse vcod with their own text (section 2).
- `C` and `D` record under `cg_autodemo` (section 8).
- Prediction on a protocol 6 connection runs 1.5's movement (`cod15-movement.md`, "What vcod does with this").
- www-redirected downloads go over HTTP with checksum checks (section 14).
- The net probe's scripted modes read through the connection's protocol and need `--capture-tag` to write off 1.1 (AGENTS.md, "The probe client"). Against 1.5b on mp_carentan dm the combat, motion, hit pair, weapon-select and item-drop runs (the last beside `client-probes/probe_itemdrop` under `COD_VERSION=1.5 tools/run_probe.sh`) recorded what they record on 1.1 (live, VERIFIED).

Server:
- `vcod-server` hosts protocol 6 when its install is 1.5 (section 13), with the match timeout (section 12), 1.5's chat and `g_deadChat`, votes and www redirects (sections 13, 14).

Not done: the 1.5 pmove changes `cod15-movement.md` leaves out and PunkBuster.
