# CoD2 1.3 MP: the killcam

How the CoD2 1.3 server replays a kill: the stock script's fields, the
engine's frame archive, the follow copy taken out of it, the snapshot that
carries the archived frame's entities, and the spawn that ends it. CoD1's
`cod11-spectator-follow.md` section 12 is the model; this document records
CoD2's shape and what vcod does.

Evidence rules as everywhere in this directory, no document-level default:
every claim carries its own label. Addresses are the 1.3 Mac client
(`cod2mp_mac13_i386`, which links the server and game in, with symbols)
unless marked lnxded (`cod2_lnxded`). `cl` is a gclient, the playerstate at
`cl+0`.

| offset | meaning | evidence |
|---|---|---|
| `cl+0x26a8` | `sessionState`: 2 spectator, 3 intermission | VERIFIED, `ClientEndFrame` 0x1a47a8 compares 3 and 2 before its arms |
| `cl+0x26ac` | `spectatorclient` (script) | VERIFIED, `ClientScr_SetSpectatorClient` 0x1b9f94's store |
| `cl+0x26b4` | `archivetime`, ms | VERIFIED, `ClientScr_SetArchiveTime` 0x1ba2c4 stores `Scr_GetFloat * 1000.0` there; `G_GetClientArchiveTime` 0x1ada16 reads it |
| `cl+0x27a4` | `psoffsettime` | INFERRED: `SpectatorClientEndFrame` subtracts it from the age; the field name is the script's (`cod2-gsc-diff.md` 2.2) |
| `cl+0x27a8` | the client followed now | VERIFIED, `SpectatorClientEndFrame`'s stores |
| `cl+0x2740` | the spectate mask | VERIFIED, the shifts by team and the 0x10 free-flight test in `SpectatorClientEndFrame` |

## 1. The stock script

- VERIFIED, `maps/mp/gametypes/_killcam.gsc` in `iw_07.iwd`: `init` turns
  the archive on with `setarchive(true)` when `scr_killcam` is above 0 (1
  by default). `killcam(attackerNum, delay, offsetTime, respawn)` writes
  `sessionstate` "spectator", `spectatorclient` the attacker,
  `archivetime` `delay + 7` and `psoffsettime` `offsetTime`, allows every
  spectate team, waits 0.05 and gives up (back to "dead") when
  `archivetime <= delay`; else it draws five non-archived HUD elements
  (two bars, the title, the skip text, a tenths timer of `archivetime -
  delay`) and waits for `waitKillcamTime`'s `wait (archivetime - 0.05)` or
  a use press after a release. The end writes "dead", -1, 0 and 0.
- VERIFIED, `dm.gsc` 298..386: `Callback_PlayerKilled` writes "dead",
  clones the body, waits `delay` (2) and calls `killcam` with its
  `psOffsetTime` argument, then threads `respawn()`.

### 1.1 The skip key

- VERIFIED, `_killcam.gsc`: `waitSkipKillcamButton` polls `self
  useButtonPressed()` every 0.05 s, first while it reads 1, then until it
  reads 1, and notifies `end_killcam`. The skip text is
  `&"PLATFORM_PRESS_TO_RESPAWN"` when `killcam`'s `respawn` argument is
  defined (`dm` passes `true`), else `&"PLATFORM_PRESS_TO_SKIP"`.
- VERIFIED, `PlayerCmd_useButtonPressed` (0x1bff00): 1 when `(cl+0x27c8 |
  cl+0x27bc) & 0x28`, so `+activate` (0x8) or `+usereload` (0x20).
  `PlayerCmd_attackButtonPressed` (0x1bff98) tests the same OR against 0x1.
- VERIFIED, the stores: each of the three arms of `ClientThink_real`
  (0x1a3d00) moves `cl+0x27bc` to `cl+0x27c0`, writes the cmd's buttons
  (`cl+0x26cc`) to `cl+0x27bc` and ORs the new presses into `cl+0x27c8`:
  the intermission arm (`sessionState` 3), `SpectatorThink` (0x1a2a08,
  `sessionState` 2) and the playing arm.
- INFERRED from the instruction order: the intermission arm stores before
  its return and `SpectatorThink` before its follow keys, so a follower in
  the killcam reads his own keys.
- VERIFIED: `ClientEndFrame` (0x1a47a8) zeroes `cl+0x27c8` on every arm's
  way out.
- INFERRED: the script threads run in `G_RunFrame` before the end frames,
  so a press let go between two frames still reads 1 to that frame's
  threads.
- INFERRED, vcod's port: `Cod2Level::client_think` stores both words
  before any arm and mirrors their OR onto the host, and `end_frame`
  clears the latch. Until 2026-10-23 the mirror sat after pmove, past the
  follower's early return, so a killcam's skip never fired on vcod-server
  (`cod2_fight::a_use_press_skips_the_killcam`). Not run live against
  `cod2_lnxded`; the decompile is the evidence.

## 2. The archive

- VERIFIED: `SV_Frame` (0x15bf3c) calls `SV_ArchiveSnapshot` (0x194da8;
  lnxded 0x809b016, named by its `SV_ArchiveSnapshot: ignoring snapshot
  because it overflowed.` string) after each `G_RunFrame` of a tick but
  the last, and again after `SV_SendClientMessages`.
- INFERRED from 0x194da8's writes: each frame keeps its time, every
  connected client's `clientState` (`G_GetClientState`), the playerstate
  `GetFollowPlayerState` answers for each, and the linked entities a
  snapshot could carry, as CoD1's does.
- VERIFIED: `GetFollowPlayerState` (0x1a28d2) answers only for a
  playerstate with `pm_flags` 0x800000, copies 0x26a8 bytes and zeroes the
  0xf80 bytes at `ps+0x7a8`, the current HUD half (31 elements of 0x80).

## 3. The follow copy

`SpectatorClientEndFrame` (0x1a2bc6), which `ClientEndFrame` runs for a
`sessionState` of 2:

- VERIFIED, the stores: with `spectatorclient` 0 or above it is copied into
  `cl+0x27a8`, then a loop clamps a negative `archivetime` to 0, asks
  `SV_GetArchivedClientInfo` for that client at `archivetime -
  psoffsettime`, and lowers `archivetime` by 50 after each failure. At
  `archivetime` 0 it stops and writes -1 to `spectatorclient` and to
  `cl+0x27a8`.
- INFERRED: an answer counts only when the answered `clientState`'s team
  bit is clear in the spectate mask (the shift of `cl+0x2740` by the
  record's byte at `+4`).
- VERIFIED: a follow not forced asks at `archivetime + psoffsettime`.
- VERIFIED, the stores on a copy: 0x26a8 bytes over the playerstate,
  `HudElem_UpdateClient(ps, self, 2)`, `eFlags` 0x100000 kept from the
  spectator's own, `pm_flags` 0x800000 off and 0x400000 on; a forced follow
  also clears 0x3000000 (the killcam's copy reads `pm_flags` 0x600000 where
  the shooter's own read 0xa00000, section 6).

`SV_GetArchivedClientInfo` (0x195a06, lnxded 0x809a0b6):

- VERIFIED, the arithmetic: with the archive on and an age above 0 the
  frame is the frame count less `age * sv_fps / 1000`, clamped to the last
  0x4b0 frames and to frame 0, each clamp rewriting the age (a local: the
  caller's `archivetime` is not written here); the first kept frame from
  there answers. A frame with no record for the client fails, with no live
  fallback. No frame sets the age to 0.
- VERIFIED: an age of 0 or less with no frame falls back to the live
  client, which must be in the world (connection state 4) and answered by
  `GetFollowPlayerState`.
- VERIFIED, the shift: the record's age, `svs.time` less its time, is
  added to each non-zero `commandTime`, `pm_time`, `foliageSoundTime`,
  `jumpTime`, `viewHeightLerpTime` and `shellshockTime` (`ps+0x0`, `+0x10`,
  `+0x44`, `+0x70`, `+0xfc`, `+0x5dc`), to each non-zero `fadeStartTime`,
  `scaleStartTime`, `moveStartTime` and `time` of the archived HUD half at
  `ps+0x1728`, and to `deltaTime` (`ps+0x7a4`) whatever it held. A shifted
  `fadeStartTime` past `svs.time` is held at `svs.time`.
- VERIFIED: `ClientEndFrame` zeroes `ps+0x7a4` first, so a live frame's
  `deltaTime` is 0.

## 4. The snapshot

`SV_BuildClientSnapshot` (lnxded 0x809a408; inlined in
`SV_SendClientSnapshot`, 0x195f84):

- VERIFIED: every snapshot reads the client's `archivetime`
  (`G_GetClientArchiveTime`), runs section 3's lookup on it alone (no
  `psoffsettime`) and writes the trimmed age back
  (`G_SetClientArchiveTime`), so a killcam asked for on a young level reads
  back what the archive could serve a frame later, which the stock
  `wait 0.05; if(self.archivetime <= delay)` tests.
- VERIFIED: with a frame, the entities are that frame's, skipping the
  number in `ps.clientNum` (the followed client) and the single-client
  bits against it, culled from the playerstate's eye by the records' boxes;
  each non-zero `pos.trTime`, `apos.trTime`, `time` and `time2` (`+0x10`,
  `+0x34`, `+0x54`, `+0x58`) takes the age, and the roster is the frame's
  client records.
- VERIFIED, lnxded 0x809a408: the archived arm's roster loop runs over the
  frame's record count (`+0x10`) from its first record (`+0x14`), 0x2708
  bytes a record, copying the 0x5c-byte `clientState` at the record's
  `+4`; the live arm's over every client past connection state 1 through
  `G_GetClientState`. Live, section 6's `killerdies` pair.

## 5. The end

- VERIFIED: `ClientEndFrame` (0x1a47a8; lnxded 0x80f7516) runs its playing
  and dead arm only while `ps.clientNum` is the client's own; otherwise it
  calls `ClientSpawn` at the playerstate's origin with the angles
  `(0, ps.viewangles[1], 0)` (`ps+0x14..0x1c`, `ps+0xec`).
- VERIFIED live (section 6): that spawn's frame reads the victim's own
  `clientNum`, `pm_type` 6, `pm_flags` 0x801000, `eFlags` 0 where the copy
  read 2, weapon 0, `deltaTime` 0, `commandTime` the frame's time,
  `viewHeightCurrent` 42 and `viewHeightTarget` 8, the replay's last origin
  and yaw with pitch 0, and `stats[5]` (the spawn count) one up.
- VERIFIED, `ClientSpawn` (Mac 0x1930a6): the memset of the gclient's
  first 0x28a4 bytes clears the whole playerstate, the event ring with it
  (`eventSequence` at `ps+0xa4`, `events`, `eventParms`); `eFlags`
  (`ps+0xa0`) keeps only 0x100002 (the vote and teleport bits) and then
  flips 0x2. `ClientConnect` (Mac 0x193a5e) memsets the gclient too, so a
  client's first spawn reads `eFlags` 2, and a spectator reads 2 only
  because the stock `spawnSpectator` spawned it.
- VERIFIED live (`killerdies`, section 6): every spawn's first frame reads
  `eventSequence` 0 and its four events 0, the script's `spawn` of a
  client whose ring stood at 3 as much as the killcam's dead spawn; the
  next events count from 0 (1 and 2 within 300 ms of the respawn). Both
  clients' first spawn read `eFlags` 2. The cgame's ring test
  (`CG_TransitionPlayerState`, cod2-sound.md) raises nothing for a
  sequence that went down.
- VERIFIED: `ClientEndFrame`'s playing arm is also the dead one
  (`sessionState` not 2 or 3, `ps.clientNum` the client's own): it sets
  `pm_flags` 0x800000 with 0x3000000 cleared, `speed`, `gravity`, and runs
  `Player_UpdateLookAtEntity` (which first clears 0x300000) and
  `P_DamageFeedback` for a dead client as for a living one. Live the dead
  target read `pm_flags` 0x800400, then 0x800000 (section 6).
- VERIFIED, `player_die` (Mac 0x1c4a7c) writes the entity's angles into
  `ps.viewangles`; `ClientThink_real` (0x1a3d00) leaves them at (0,
  `viewangles[1]`, 0) after each cmd (`SetClientViewAngle` sets them
  whole). Live the killer's `viewangles[0]` went from 1.02 to 0 on the
  frame of his suicide.

## 6. The retail run

2026-10-22, `GT=killcam SECOND=killcam
PROBE_GSC=crates/cod2/tests/fixtures/killcam/probe_killcam.gsc
tools/run_probe_cod2.sh shootlate mp_toujane 1943 1991 65 0` with the
combat captures' `probe_v*` cvars, inside `unshare -rn`. The gametype is
`probe_combat.gsc` with the target mortal and dm's death tail. The
committed pair is `crates/cod2/tests/fixtures/killcam/killcam.txt` (the
shooter) and `killcam-b.txt` (the target).

- VERIFIED: the head shot killed at 18050, `killcam` started at 20100,
  `archivetime` read 9 after the trim (no trim: the level was older than
  9 s) and `psOffsetTime` -8; the killcam ended at 29100.
- VERIFIED: from 20100 to 29050 the target's playerstate read `clientNum`
  1, `pm_flags` 0x600000, `pm_type` 0, `deltaTime` 9000 and the shooter's
  `commandTime`, origin and view 9000 ms back. The target's own body,
  alive, came back as entity 0 with its `pos.trTime` shifted; the shooter's
  entity 1 and the target's corpse (64) left on the same snapshot, the
  corpse back at 27100, one age after the clone.
- VERIFIED: the dead spawn of section 5 at 29100; `viewHeightCurrent` read
  42, 36.24, 29.58, 16.26, 9.42 and 8 on the frames from it, the dead
  pmove's ease, as on the death at 18150..18400.

The second pair, `killerdies.txt` and `killerdies-b.txt`, is the same run
with `+set probe_killerdies 1 +set probe_respawn 3`, the target's
`killcam` client logging every snapshot's roster (`r` lines): the killer
detaches his helmet 0.5 s after the kill, suicides 0.5 s later and
respawns 3 s after that.

- VERIFIED: the kill at 17750, the detach at 18300, the suicide at 18800,
  the killcam 19800..28800 at `archivetime` 9, the respawn at 21800.
- VERIFIED: the killcam kept following the dead killer: at 27800 the
  copy read `pm_type` 6 and then `pm_flags` 0x400000 (the archived
  0x800000 swapped), on to the dead spawn at 28800.
- VERIFIED, the roster: the killer's `attachModelIndex[1]` (the helmet)
  read 0 live from 18300; the target's snapshots from 19800 carried it
  back (the archived frame's), dropped it at 27300 (18300 replayed), and
  read the live one again at 28800.

## 7. vcod

`crates/server/src/game/cod2_archive.rs` is the archive: while script has
`setarchive` on, `engine::tick` archives each frame in section
2's order through `Cod2Level::archive_frame` (the frame's world entities,
roster, and each playing client's own wire playerstate with its current
HUD cleared; unchanged entities and HUDs share storage), keeping 0x4b0
frames in CoD1's ring (`engine::archive::FrameRing`). `Cod2Archive::lookup` and `client_info` are section 3's lookup and
`SV_GetArchivedClientInfo`, `shift_ps` and `shift_entity` the shifts.
`Cod2Level::spectator_end_frame` runs section 3's loop and keeps the
archived copy on the player (`follow_copy`), which `wire_playerstate`
sends; `snapshot_entities` runs section 4 (the culled entity is the one
`ps.clientNum` names, live or archived, and a temp entity's scope is
tested against it too). `end_frame` takes its arm off `sessionstate`: a
session made "spectator" runs the follow whatever `pm_type` the client
had (the dead one kept the corpse view before), and a client whose last
frame was a copy is spawned at it (`spawn_at_follow_copy`, section 5),
its spawn think run on the last cmd.

A killcam's snapshot carries the archived frame's roster
(`snapshot_entities` hands it to `Cod2Server::client_message`). The dead
keep `pm_flags` 0x800000 and run the look-at pass (`end_frame`), so the
archive and a live follow answer for them. Both spawns go through
`spawn_e_flags` (0x100002 kept, 0x2 flipped) from a connect's 0, and a
script spawn starts a fresh event ring. A death levels the view
(`cod2_combat::damaged`), though a `setPlayerAngles` since the last cmd is
not kept apart.

`cod2_killcam_ab` replays both pairs of section 6 on `Cod2Level` and
matches retail on every snapshot with no gap: both clients' `eFlags` at
the first spawn; the target's `clientNum`, `pm_type`, `pm_flags`,
`eFlags` and `eventSequence` outside the killcam; the copy's `pm_type`,
`pm_flags`, `deltaTime`, `commandTime`, weapon, view, origin, `eFlags` and
`eventSequence`; the dead spawn's words on its frame and the seven after
(the eye's ease included); which clients and bodies each killcam
snapshot carries; every roster word where the capture logged one; the
killer's own words through his death and respawn; and the gametype's
PROBE lines.
