# CoD4 1.7: what vcod-server's CoD4 side still lacks

What a retail 1.7 dedicated server (`cod4_lnxded`) does that `vcod-server
--game cod4` lacks or fakes, ranked. The player-facing summary is
`docs/features/cod4.md`; the client's list is `cod4-gaps.md`. Sources:
the open sections of the `cod4-*.md` docs, the builtin handlers in
`game/builtins/cod4*.rs` against the stock scripts' calls, the console
commands 1.7 registers (their strings in `cod4_lnxded`) against
`cod4::oob`'s `exec`, and the dvars 1.7 reads against what `cod4/` and
`game/cod4/` read. First audit 2026-10-26 at b5311fc8; rechecked against
the code 2026-10-30. Sizes: S under an hour, M a few hours, L a day or
more.

## Fixed

- `setStat` sends a changed stat as 1.7's reliable `N <index> <value>`
  (`SV_SetClientStat`), with `GScr_SetStat`'s index and byte checks;
  `getStat` and `setStat` refuse as 1.7's do, receiver and
  `developer_script` included (cod4-gsc.md, "The stats, the loadout, the
  perks"). A `fast_restart` keeps what the scripts wrote; a map change
  takes the client's block again. The client keeps the profile's stats in
  a plain file of its own (cod4-netfields.md, "The stats file").
- `status` and `getstatus` read the script's `score`; `status` has the
  separator line, the `^7` after the name, a right-aligned GUID, pings
  past 999, and `lastmsg` as `svs.time` less `lastPacketTime`
  (cod4-handshake.md 14).
- The console: `say`, `tell`, `kick` with no name and `kick all`,
  `kick`'s brief ban, `banUser`, `banClient`, `unbanUser` with
  `ban.txt` (`--ban-file`) read at the `getchallenge`, `dumpuser`,
  `killserver`, `heartbeat`, the dvar commands, `vstr`, `exec`, `wait`,
  `cmdlist` (14, 14.1).
- `games_mp.log`: `g_log`, `g_logSync`, `logPrint`, the engine's own
  lines and `getGuid` ("games_mp.log" below).
- `g_inactivity` warns and drops ("g_inactivity" below).
- Rounds, throws and rockets leave from the eye with its walk bob and
  lean, along the angles the last cmd's aim block left: sway, movement
  tilt, idle drift, gun spring, and `P_DamageFeedback`'s damage kick, in
  the client's prediction too (cod4-combat.md 3.2).
- Rounds meet every solid script model's collision triangles and its
  attachments' (hidden parts skipped, the part named in the notify) and
  brush models, and go on past them at the damage left; `notSolid` and
  `solid` write a script model's contents; glass needs only
  `Bullet_FireExtended`'s contents bit (cod4-combat.md 13.1, 13.2). The
  stock destructible cars and barrels break under rounds and blasts
  (13, `game::cod4::entity_damage`).
- `setPlayerAngles` sets the view and `delta_angles`.
- Retail captures gate a rocket's flight, the live grenade's throw back,
  a prone death's torso and a `killcamentity` killcam (cod4-combat.md
  15). They brought `flinchYawAnim`, the dead yaw (`stats[1]`), a spawn
  that drops the last blow, `takeWeapon` and `dropItem` taking the alt
  chain and its ammo, the body's dead and sighted `eFlags`, a level
  vector's +0 pitch, and a killcam's entities shifted by the look back
  without the victim's own flesh event.
- Entity numbers follow retail's: `delete()` frees inside the call, so
  the free list keeps the scripts' delete order (cod4-combat.md 14).
- `sv_fps` is read every frame (cod4-server-frame.md 8).
- Off the LAN (cod4-handshake.md 15): bans answer the `getchallenge`;
  `sv_minPing` / `sv_maxPing` are read; a client out of the world waits
  a second between messages as on 1.7 (the gamestate's second fragment
  went 336 ms after the first, against retail's 957). `requeststats`
  after a map change matched retail packet for packet.
- A mantle plays its clips' movetypes and, over a wall, the jump event at
  its end, so others see the mantle; the mantle capture's `legsAnim`,
  `torsoAnim` and both timers replay exactly (cod4-pmove.md 17.5).

## Ranked

Open, by how much a player notices.

1. The authorize round trip stays out on purpose: with its authorize
   server silent, 1.7 answers no off-LAN challenge for its first 20
   minutes, then each after 7 s; vcod-server answers at once and an
   off-LAN client's address stands in for its ban guid
   (cod4-handshake.md 15).
2. The helicopter's flight runs no move trace and no touch, and its model
   collision traces at the bind pose (cod4-gsc.md 3.2, `game::cod4::vehicle`).
   M.
3. What the four capture gates (`cod4::capture_ab`, cod4-combat.md 15)
   still tolerate: damage applied at the victim's end frame rather than
   inside `finishPlayerDamage` (the push, `pm_time`, the timers, the
   corpse's launch), the dropped weapon's start and fall, a grenade's
   strike on a standing body, the second rocket's entity number, a
   `setviewpos`'s `delta_angles`, a killcam's trim and its archived view.
   The first is L, the rest S to M.
4. The touch pass after the move and a free spectator's flight have no
   A/B against retail yet (cod4-pmove.md 15.2). S.
5. The off-LAN snapshot rate above 20 (cod4-server-frame.md), and why
   retail counted its own public address as off the LAN
   (cod4-handshake.md 15). S to measure.
6. Closed: an empty `sv_master1` heartbeats `master.cod.pm:20810` at
   `dedicated 2`, as CoD1's and CoD2's servers do (`Cod4Server::heartbeat`).

## The retail run

`cod4_lnxded` 1.7 in `unshare -rnm` (cod4-handshake.md 7), stock `war`
on mp_crash, `+set g_log games_mp.log +set g_logSync 1 +set
g_inactivity 30 +set sv_pure 0 +set rcon_password ...`. Two
`cod4_probe` clients joined 40 s after the start: `alpha` (axis, idle,
`--cmd "5 say hello log" --cmd "6 say_team team log" --cmd "8 kill"`)
and `bravo` (a spectator); the console commands went over rcon, then
`kick bravo`, `killserver` and queries.

## games_mp.log

- `G_LogPrintf` (0x80b43c4) does nothing without the open file
  (0x836f6d8); else it formats the text and writes `%3i:%i%i %s`
  (0x8219b90) with the minutes and the two digits of the seconds of
  `level.time` (0x836f8ac). VERIFIED. It prints nothing to the console.
  INFERRED (no print call in it).
- `G_InitGame` (0x80b563a) sets `level.time` to its `levelTime`
  argument, the server's clock (0x80b56cb), so stamps run on across
  levels: the run's first `InitGame` read `0:00`, `J;` `0:43`, and a
  logged rotation's eighth level started at `74:21`. VERIFIED live.
- `g_log` (default `games_mp.log`, archived, 0x80b4830) empty prints
  `Not logging to disk.` (0x821a330); else the file opens in the home
  path's game directory, `FS_APPEND` (mode 2) or `FS_APPEND_SYNC` (mode 3)
  as `g_logSync` reads (0x80b5793..0x80b57b5); a failure prints `WARNING:
  Couldn't open logfile: %s` (0x821ae24). VERIFIED. The run's file was
  `<home>/main/games_mp.log`, a mod run's `<home>/mods/<mod>/games_mp.log`.
  VERIFIED live.
- On opening it writes the 60-dash separator (0x821ae48) and `InitGame:
  %s` (0x821a322) with `Dvar_InfoString(0x400)` (0x80b5aab). VERIFIED.
  The run's line was configstring 0's pairs without the empty `fs_game`:
  `\g_compassShowEnemies\0\g_gametype\war\gamename\Call of Duty 4\...\ui_maxclients\32`.
  VERIFIED live.
- `G_ShutdownGame` (0x80b5ce8) writes `ShutdownGame:` (0x821a36a) and the
  separator, then closes the file. VERIFIED. `killserver` wrote both.
  VERIFIED live. A `fast_restart` runs both functions again. INFERRED.
- The engine's other lines: `ExitLevel: executed` (0x80b4585), `say;%s;%d;%s;%s`
  and `sayteam;%s;%d;%s;%s` from `G_Say` (0x80ae9d5, 0x80aeb51; GUID,
  client number, the name `I_CleanStr`'d, the text), and
  `Weapon;%s;%d;%s;%s` (0x8092183, the weapon's internal name last).
  VERIFIED. A team line from a client on no team goes out and is logged
  as `say` (0x80aea99); a tell writes none (0x80ae9ab). INFERRED. The
  `Weapon` line sits past the held-weapon bit test of the pickup
  (0x80920b9), so it is a weapon not held being taken. INFERRED.
- The run's `say` lines read `say;<guid>;1;alpha;hello log` and
  `sayteam;<guid>;1;alpha;team log`. VERIFIED live.
- `logPrint` (`GScr_LogPrint`, 0x80bd1ac) joins its arguments up to 1023
  bytes (0x80bd216) and hands them to `G_LogPrintf` as the format.
  VERIFIED. The stock `_globallogic.gsc` writes `J;`, `Q;`, `D;` and
  `K;` with `self getGuid()` (VERIFIED, the rawfile); the run's `J;`
  and `Q;` lines carried the client's GUID. VERIFIED live. `kill` was
  refused with `GAME_CHEATSNOTENABLED`, so no `K;` line came. VERIFIED
  live.
- vcod (`engine::gamelog`, shared with CoD1 and CoD2, cod11-map-cycle.md
  9): the game queues each line with `level.time`
  (`GameHost::log_printf`), the engine writes them after each frame,
  packet and command; `vcod-server` fills `fs_homepath` with the game
  directory, and a server with none (a test's) writes no file.

## g_inactivity

- `g_inactivity` is an int, 0..INT_MAX, default 0 (0x80b4a3c).
  VERIFIED. 1.7 has no `g_inactivitySpectator` (no such string in
  `cod4_lnxded`). VERIFIED.
- `ClientSpawn` sets the deadline `level.time + 1000 * g_inactivity`
  (0x80a7e05). VERIFIED.
- `ClientThink_real` (0x80a5c1a) calls `ClientInactivityTimer`
  (0x80a4d56) only for a session state other than 2 and 3, spectating
  and the intermission (0x80a5cb4..0x80a5ccf), and thinks no further when
  it returns 0. INFERRED.
- The timer: `g_inactivity` 0 puts the deadline 60 s ahead and clears
  the warning; a cmd with a move (the `forwardmove`/`rightmove` word at
  +0x2fa6) or buttons 0x401 puts it `g_inactivity` seconds ahead; a local
  client (`pers.localClient`, +0x2fd0, a bot or loopback address per
  0x813b1b0) is never dropped; past the deadline
  `SV_GameDropClient(.., "GAME_DROPPEDFORINACTIVITY")` (0x8217d59);
  within 10 s of it, once, `c "GAME_INACTIVEDROPWARNING"` (0x8217d73) to
  the client. INFERRED (the branches; the strings VERIFIED).
- The run: `alpha` spawned about 5.5 s after its first snapshot, was
  sent `c "GAME_INACTIVEDROPWARNING"` 20 s later and `w
  "GAME_DROPPEDFORINACTIVITY"` 30 s after the spawn, its `Q;` line
  logged; `bravo`, a spectator as long, was never warned. VERIFIED live.
- vcod (`Cod4Level::inactivity_timer`) runs the timer for the network
  clients' cmds only, so its bots, like retail's, are never timed.

## Not gaps

- PunkBuster: `sv_punkbuster` reads 0 and no `pb_sv_*` command runs.
  1.7's PunkBuster is a separate download (`pbsetup.run`) a stock install
  of the dedicated server does not run either.
- Every builtin the stock scripts of the 21 stock maps call has a
  handler (cod4-gsc.md 3.1); the ones that return a constant
  (`getAssignedTeam`, `buttonPressed`, `sendRanks` and kin) do so on 1.7
  too, VERIFIED there.
- A `physicsLaunch`ed part stays where it stood on the server: 1.7's
  `BG_EvaluateTrajectory` (0x80536ba) reads `TR_PHYSICS` at `trBase`
  too. VERIFIED (cod4-gsc.md 3.2). Each client flies it off the
  launch's contact and force (cod4-gsc.md 3.6).
