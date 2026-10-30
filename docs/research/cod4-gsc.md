# CoD4 1.7 gsc: dialect, builtin tables, callbacks, the boot's configstrings

What vcod's compiler, VM and host need to run CoD4 1.7's stock MP scripts,
and how far `war` on mp_crash gets. Code: `crates/gsc` (`Dialect::Cod4`,
`VmProfile::COD4`), `crates/server/src/game/cod4/` (`Cod4Zones`,
`Cod4Scripts`, `Cod4HostState`, `load`), `game/builtins/cod4.rs` and the
generated `cod4_names.rs`, `game/builtins/cod4_client.rs` (the player
methods of section 7), `game/cod4/level.rs` (`Cod4Level`), `weapons.rs`,
`wire.rs`, `game/profile.rs` (`COD4`). Gates:
`game::cod4::boot::cod4_boot` and `cod4_stock_scripts_call_only_cod4_builtins`
(both need `COD4_DIR`), `builtins::set::tests::cod4_lists_its_own_tables`,
`cod4::wire_ab::a_join_on_war_matches_retail` (section 7, `COD4_DIR`).

## Sources

- `cod4_lnxded` 1.7 (md5 `bf8448740b3f4cb74c2254f01af14cdd`, `CoD4 MP 1.7
  build linux-i386 Jun 28 2008`), stripped, the game linked in, ET_EXEC
  with real pointers in `.data`. Addresses are its own.
- The stock scripts are the rawfiles of `zone/english/common_mp.ff` (124
  `.gsc`) and `mp_crash.ff` (6), walked by `vcod_cod4::walk`. VERIFIED
  (`cod4_stock_scripts_call_only_cod4_builtins` counts 130).
- The retail gamestate is `crates/cod4/tests/fixtures/net/handshake-1.7.txt`
  (`cod4-handshake.md` 5).
- A claim labelled INFERRED that names no 1.7 address is not yet
  confirmed on 1.7.

## 1. The dialect

`Dialect::Cod4` is `Dialect::Cod2` plus what the corpus fails on under
CoD2's front end. Run with `DIALECT=cod4 BUILTINS=<names>` through
`crates/gsc/examples/gsc_census.rs` over the 130 rawfiles, CoD2's dialect
fails one file and leaves three names to the builtin table that are in
no table. VERIFIED (census run). The three additions:

- `<<` and `>>`, binding between the relational operators and `+`/`-`, as
  C's do: `_rank.gsc` 667 `(addonMask<<16) | (1<<16)` is the corpus's use.
  VERIFIED (the parse failure). The precedence is INFERRED from the VM's
  opcode order (`OP_shift_left` between `OP_greater_equal` and
  `OP_plus`). Integer only, as `&`, `|`, `^` are. Lexed only
  under `Dialect::Cod4`.
- `waittillmatch(event, value...)`: five calls (`common_scripts/utility.gsc`
  26, `_helicopter.gsc` 538, 543, 795, 879). Neither `waittillmatch` nor
  `prof_begin`/`prof_end` is in any of the six builtin tables of section 2
  and neither string is in the binary. VERIFIED. So all three are compiler
  statements, which retail compiles the stock scripts with. INFERRED.
  Semantics: a `waittill` that wakes only on a notify whose first
  arguments equal the values, in order; one that differs leaves it
  waiting (`cod4_waittillmatch_wakes_on_matching_arguments_only`). INFERRED from `OP_waittillmatch`.
- `prof_begin(name)` / `prof_end(name)` compile to nothing (13 `prof_begin` each in
  `_globallogic.gsc` and `_spawnlogic.gsc`). INFERRED: they drive a
  profiler only.

- `*` and `/` take a vector against a number either way round and a pair
  of vectors per component (`Op::MulVector`, `Op::DivVector`), where
  CoD2's `*` refuses a vector. The stock scripts need it:
  `_helicopter.gsc` 393 `144*heli_forward_norm`, `_hardpoints.gsc` 279
  and 287 (`level.artilleryDangerForwardPush * ... * forward`,
  `forwardPart / level.artilleryDangerOvalScale`), the first of which
  killed `getSingleAirstrikeDanger` on ours. VERIFIED (the corpus and the
  error). The cast of the number up to a vector and a zero divisor
  component's `divide by 0` are `Scr_CastWeakerPair`,
  `Scr_EvalMultiply` and `Scr_EvalDivide`. INFERRED. Two ints
  still divide as floats.

The VM takes CoD2's rules (`VmProfile::COD4` is `COD2` with the dialect
swapped): newest waiter first, kill mid-step, thread returns. INFERRED,
unmeasured on 1.7.

## 2. The builtin tables

`tools/re/cod4_builtins.py <cod4_lnxded> [--rows|--rust]` finds every run
of at least four 12-byte rows `{char *name; void (*fn)(); int developer}`
(name an identifier, `fn` inside `.text`, developer 0 or 1). Seven runs
exist; each starts on a row a single `.text` instruction points at and ends
on a null row or on text. VERIFIED.

| Table | Address | Rows | developer 1 | Walked at |
|---|---|---|---|---|
| functions | 0x826d2c0 | 205 | 16 | 0x80bd26a |
| player methods | 0x8218de0 | 83 | 0 | 0x80a867f |
| script entity methods | 0x8216c40 | 18 | 3 | 0x8099f90 |
| vehicle methods | 0x821be40 | 25 | 0 | 0x80ba85d |
| entity methods | 0x821f580 | 82 | 0 | 0x80bff7e |
| hudelem methods | 0x8215900 | 22 | 0 | 0x808d99b |

VERIFIED (the script's summary). Which receiver each method table serves is
read off its first name (`giveweapon`, `moveto`, `freehelicopter`,
`attach`, `settext`). INFERRED. The seventh run, `userinfo` ..
`unmuteplayer` at 0x826e380 (12 rows), is the client command table, not
gsc. INFERRED from its names.

- The functions hold 204 distinct names (`weaponfiretime` twice); the five
  method tables 230. VERIFIED (`cod4_lists_its_own_tables`). The developer
  rows are `createprintchannel`, `setprintchannel`, `print`, `println`,
  `print3d`, `line`, `assert`, `assertex`, `assertmsg`, `getentbynum`, the
  six file builtins, and the three `devadd*` script entity methods.
  VERIFIED.
- `crates/server/src/game/builtins/cod4_names.rs` is the `--rust` output,
  committed. `BuiltinSet::cod4()` compiles against it, so a call to a name
  outside it fails the load, as the CoD1 and CoD2 sets do. Every script of
  the 130 compiles against it and 1.1's set refuses `tablelookup`.
  VERIFIED (`cod4_stock_scripts_call_only_cod4_builtins`).

## 3. Handlers

The shared handlers are bound by name and `builtins/cod4*.rs` add 1.7's
own. The `war` bootstrap on mp_crash and 30 s of an empty server reach no
name without one and kill no thread. VERIFIED (`cod4_boot`). Section 3.1
covers every name the stock scripts call.

- The dvar builtins (`getdvar`, `getdvarint`, `getdvarfloat`, `setdvar`)
  take the cvar handlers. INFERRED: the names change, the stock calls
  read and write the same strings. `makeDvarServerInfo(name[, default])`
  takes its default as optional: `_globallogic.gsc` 107 passes only the
  name. VERIFIED (the corpus).
- `tableLookup(table, searchColumn, value, resultColumn)` and
  `tableLookupIString` search `code_post_gfx_mp.ff`'s string tables (30,
  `mp/statstable.csv` and kin) for the first row whose search column
  equals the value, case-blind, `""` when none does. The tables' location
  is VERIFIED (the walk); the search rule is INFERRED.
- `getStat` / `setStat` read and write the slot's block: a fresh
  profile's (`vcod_cod4::net::stats::DEFAULT_STATS`) until the client's
  seven `stats` packets are in, then the client's (section "The stats,
  the loadout, the perks").
- `visionSetNaked(name, time)` and `visionSetNight` write 824 and 825 at
  `"<name>" <ms>`, through the `"%s" %i` string (0x821c3a8). VERIFIED (the
  string; the capture's `"mp_crash" 2000` for `visionSetNaked(getDvar("mapname"), 2.0)`). A missing time is a second: `_load.gsc` 59's
  one-argument `VisionSetNight("default_night")` reads `"default_night"
  1000` in the capture. VERIFIED.
- `setExpFog(startDist, halfwayDist, r, g, b, time)` writes slot 9 through
  `%g %g %g %g %g %.0f` (0x821c072), the second value `ln 2 /
  halfwayDist`: mp_crash's `setExpFog(500, 3500, ...)` reads `500
  0.000198042 ...` in the capture. VERIFIED (string and capture). The
  density formula is INFERRED from that one sample. The two distance
  messages are the binary's (0x821da24, 0x821da58). VERIFIED.
- `setMapCenter` writes 12 at `%f %f %f`; `setMiniMap(material, x1, y1,
  x2, y2)` writes 823 at `"%s" %f %f %f %f` (0x821c3e6);
  `precacheLocationSelector` takes the next of 827..829 (a repeated name
  is a developer print, a fourth an error: section 8); `getNorthYaw`
  reads 822 back;
  `setGameEndTime(ms)` writes 11. VERIFIED (each value against the
  capture: `429.000000 126.500000 137.000000`, the minimap line,
  `map_artillery_selector`, `619800`).
- `getArrayKeys` returns the newest key first: `_globallogic.gsc` 3802
  precaches `game["strings"]` over its keys, and the capture's localized
  strings 348..381 hold them in the reverse of their first assignments
  (`MP_OPFOR_NAME` .. `PLATFORM_PRESS_TO_SPAWN`). VERIFIED (capture). The
  heap keeps each array's insertion order for it (`Heap::array_keys_newest_first`).
- `precacheItem` and the spawn pass's `weapon_<name>` pickups register a
  weapon (`register_weapon`): appended to 2258's list, its 16
  `worldModel`s then its `projectileModel` indexed as models, its
  `altWeapon` registered right after it, and 2314's bits rewritten.
  The capture's list opens with `defaultweapon_mp` (registered at the
  load) and mp_crash's eight pickups in lump order, then the precaches,
  `g36c_gl_mp gl_g36c_mp` pairs included; its models open with
  `defaultweapon` at 831. VERIFIED (capture). The model order follows
  `G_RegisterWeapon`, INFERRED. 2314 is four bits a lowercase
  hex digit, weapon 0 and each registered weapon set: 113 weapons read
  `ffffffffffffffffffffffffffff3000`. VERIFIED (capture).
- `hidePart` keeps the entity's `partBits` (`Cod4HostState::part_bits`),
  and a hidden bone's collision surfaces no longer clip a round
  (`entity_damage::ModelPart`); `setCanDamage` is UO's `setTakeDamage`;
  `isSplitScreen` is 0; `abs`, `min`, `max`, `floor` and `ceil` return
  floats. INFERRED.

### 3.1 The census

`game::cod4::census::cod4_builtin_census` compiles every `.gsc` of the
base zones with each of the 21 stock `mp_*` zones in turn (up to 131
scripts) and lists each builtin the closure calls with no handler. Its
`LEFT_OUT` table is empty, and it fails on any name without a handler.
VERIFIED (the test).

`builtins/cod4_script.rs` answers the rest:

- `sendRanks` (0x80bb058), `endParty` (0x80bb05e), `endLobby`
  (0x80bb088), `fprintln` (0x80bb022) and the three `devAdd*` (0x8099f60,
  0x8099f66, 0x8099f6c) are `push ebp; mov ebp,esp; pop ebp; ret`, empty
  on the PC build. VERIFIED (the bytes). A ranked match (`sv_pure 1`,
  `level.rankedMatch`) runs `setXenonRanks` from `endGame`, whose
  `sendRanks()` must return or `endGame` dies before the intermission;
  `cod4_ranked_match_ends_with_the_scoreboard_and_rotates` runs a
  one-minute ranked match through the intermission (`pm_type` 5 on both
  clients) to the next map with no script error. VERIFIED (the test).
- `getAssignedTeam` (0x80bb48e) and `buttonPressed` (0x80a9838) each
  hand 0 to the call at 0x815effa and return. VERIFIED (the bytes). That
  call is `Scr_AddInt`. INFERRED (`GScr_ReturnZero`).
  `isRagdoll` (0x80c4eb4) reads whether `pos.trType` is a ragdoll type,
  which `startRagdoll` writes (cod4-combat.md 5.1). VERIFIED (the bytes).
- `getCorpseAnim` (0x80c4dfc) refuses anything but an `eType` 2 entity
  (`Only valid on player corpses`, 0x821c549) and returns the anim whose
  index is the corpse's `legsAnim` less the toggle (`& 0xfdff`,
  0x80c4e6d) in the corpse slot's tree (0x80c4e81). VERIFIED (the
  instructions). `getAnimLength` is `1 / frequency` of the xanim, and
  `animHasNotetrack` and `getNotetrackTimes` read its `XAnimNotifyInfo`
  (the times as fractions of the clip). INFERRED
  (`GScr_GetCorpseAnim`, `XAnimGetLength`).
- `spawnFx(fx, origin[, forward[, up]])` spawns an `ET_FX` (8) entity,
  `un1` the effect, `lerp.apos.trBase` the angles `Scr_SetFxAngles` makes
  (0x80c6852: no direction stores (270, 0, 0) at 0x80c6869, one
  direction goes through `vectoangles` at 0x80c6886, two build the axis;
  `playFx` with two arguments calls it with none at 0x80c6cbe. VERIFIED,
  the instructions);
  `triggerFx(ent[, time])` writes `time2`; `playLoopedFx` spawns an
  `ET_LOOP_FX` (9) with `lerp.u.loopFx.cullDist` and `.period`. INFERRED
 (`Scr_SpawnFX`, `Scr_TriggerFX`, `Scr_PlayLoopedFX`). The
  shared `playLoopedFx` wrote CoD1's `ET_FX` fields, which 1.7's
  `ET_FX` reads as a one-shot effect.
- `playSoundToTeam(alias, team[, ignore])` (0x80c3edc) raises a temp
  entity of event 3 (`EV_SOUND_ALIAS`) at the entity's `r.currentOrigin`,
  broadcast (`svFlags |= 8`, 0x80c3f3c), the alias in `eventParm`; the
  team `allies` is 2 and `axis` 1, anything else `Illegal team string
  '%s'. Must be allies, or axis.` (0x821d56c); a third argument must be a
  player (`entity %i is not a player`, 0x8217f5e). `r.clientMask` starts
  all set and loses the bit of every client in use on the team but
  `ignore` (0x80c3fbc..0x80c403b), so only they hear it. VERIFIED (the
  instructions). vcod's handler (`builtins::cod4_script`) sends it to
  the same clients; a stale no-op of the same name in
  `builtins::cod4_combat` is gone. `playSoundAsMaster` raises
  `EV_SOUND_ALIAS_AS_MASTER` (4) for everyone. INFERRED. The battle chatter (`_battlechatter_mp.gsc`) rides
  the first.
- `setWaypoint(constantSize[, offscreenMaterial])` (0x808f116) writes
  `type` 13, `value` the int argument as a float and
  `offscreenMaterialIdx` (+0x44) the material's index with two
  arguments, else 0 (`not a hud element`, 0x82155a4, on anything else).
  VERIFIED (the instructions).
- `setTargetEnt`, `clearTargetEnt`, `setPlayerNameString` (type 3, the
  client in `value`) and `clearAllTextAfterHudElem` write the element's
  `type`, `value` and `targetEntNum`. INFERRED
  (`HECmd_*`).
- `beginLocationSelection(material[, radius])` writes
  `ps.locationSelectionInfo` as the selector's 1-based slot among 827..829
  plus six radius bits over the minimap's height; while it is set a fire
  press notifies `confirm_location` with the minimap point under the
  cmd's `selectedLocation`, a 0x20000 press `cancel_location`. VERIFIED:
  `ClientThink_real` tests the gathered presses (client +0x3094) for
  0x10000 then 0x20000 while +0x5b4 is set and masks the buttons to
  0x1300 after (0x80a5d47..0x80a5d81); the point is `(c + 128) / 255` of
  the minimap's world size a side, turned off its upper left corner
  (0x80a662f..0x80a669d).
  `setTeamRadar` sets the team's `ps.radarEnabled`; `setDepthOfField` the
  six `dof*` fields; `fragButtonPressed` (0x4000), `meleeButtonPressed`
  (4) and `secondaryOffhandButtonPressed` (0x8000) read the held and
  newly pressed buttons; `updateDMScores` sends `I <client> <score>` for
  the player and the best other one. INFERRED.
- `kick(n)` queues `tempBanClient n` on the console (`GScr_KickPlayer`);
  `detonate([attacker])` explodes a grenade on the next missile pass;
  `damageConeTrace` and `sightConeTrace` count `CanDamage`'s five body
  points in clear sight of the map, a non-client's origin alone;
  `physicsTrace` and `playerPhysicsTrace` stop a point or the player's
  box at the map. INFERRED. The masks are the immediates the handlers
  load: 0x802011 (`damageConeTrace`, 0x80c5d3a) and 0x801
  (`sightConeTrace`, 0x80c5d28) into the shared body at 0x80c5c80,
  0x820011 stored by `physicsTrace` (0x80be7e0) and `playerPhysicsTrace`
  (0x80c1e98), the latter's box (-15, -15, 0)..(15, 15, 70) at 0x820e024
  and 0x820e030. VERIFIED (the instructions). `getEye` (0x80c2eea)
  returns `r.currentOrigin` with 40 (0x820ded4) added to z, for any
  entity and any stance. VERIFIED (the instructions). `getTagOrigin` and
  `getTagAngles` place the model's bind pose (`baseMat`) by the
  entity's origin and angles. INFERRED. The traces go through
  section 3.3's world. A tag on a player is not posed: it reads the
  model's bind pose. INFERRED (the code).

The hudelem fields are their own 19-row table at 0x8215a20, 28-byte rows
`{name, offset, type, mask, shift, setter, getter}`: CoD2's 15 names and
`glowcolor`, `glowalpha`, `hidewhendead`, `hidewheninmenu`. VERIFIED. The
`font` names at 0x826ce18 are `default`, `bigfixed`, `smallfixed`,
`objective`, `big`, `small`; the four align tables at 0x826cdc0 .. 0x826ce14
hold CoD2's names. VERIFIED (`fields::COD4_HUD_FIELDS`).

`radiant/keys.txt` is a rawfile of `common_mp.ff`, and the load reads it
through the zones: `_destructible.gsc` reads `destructible_type`, a key
only that file lists. VERIFIED (the rawfile). That 1.7's engine reads it is
INFERRED (the string `radiant` at 0x821fc48, and the scripts need the key).

### 3.2 The hardpoint vehicles

`builtins/cod4_vehicle.rs` and `game/cod4/vehicle.rs`. The stock
airstrike (`_hardpoints.gsc` `useAirstrike` .. `callStrike_bombEffect`)
and helicopter (`triggerHardpoint("helicopter_mp")`, `_helicopter.gsc`
`heli_think` .. `heli_leave`) run on ours with no script error on
mp_crash `war`: `cod4::hardpoint_tests` (needs `COD4_DIR`). VERIFIED
(the tests). The engine side is read off `cod4_lnxded` 1.7 below, each
address its own; the flight is compared with a retail capture in "The
flight against retail".

- `spawnPlane(owner, classname, origin[, spawnflags])` (0x80c0fde)
  refuses an owner with no client ("Owner entity is not a player"),
  spawns the classname at the origin through the spawn table
  (0x80c8af8), the fourth argument its `spawnflags` (+0x17c), then sets
  `eType` 13 (`ET_PLANE`) and `lerp.u.vehicle.teamAndOwnerIndex` (+0x6c)
  to the owner's `sess.cs.team` (client +0x3010) | slot << 2 (the
  multiply by 0x22e5e84 is the exact division by `sizeof(gclient_t)`
  0x3184, times 4). VERIFIED (the disassembly). The airstrike's planes
  reach a client as type 13 with `5` for the axis shooter in slot 1 and
  their model's `index`. VERIFIED (the test, ours). A plane's `moveTo`
  is the shared mover's: retail's planes and ours agree word for word,
  `trType` 6, 3 and 7 for a move with both eases (1.7 has no
  `TR_GRAVITY_PAUSED`, so CoD2's 7 and 8 move down one,
  `game::cod4::wire::tr_type4`) and the angles script set in
  `apos.trBase` under a move alone. VERIFIED (`hardpoint_ab`).
- `spawnHelicopter(owner, origin, angles, vehicleInfo, model)` (0x80c0e54)
  refuses an owner with no client, spawns a `script_vehicle`, places it
  and hands it to 0x80ba8d6: the model (0x80cae3a), the vehicle file
  (0x80cd966), `eType` 12, the vehicle type forced to 5 (helicopter),
  `teamAndOwnerIndex` as the plane's, `health` 99999 there and then (not
  in a first think), and a ±50 box. VERIFIED (the disassembly).
  `VEH_InitEntity` (0x809c650): `r.contents` 0x2080, `svFlags` 4, both
  `trType`s 1 (`TR_INTERPOLATE`), `s.weapon` the vehicle file's
  `turretWeapon` (+0x50 of the file record), `clipmask` 0x810211, and
  `nextthink` a frame on, so the first think is the next frame's.
  VERIFIED (the disassembly).
- A client reads the model's index in `un2` with `index` 0, `weapon`
  113 (`cobra_20mm_mp`), `pos.trDelta` 0 throughout (no velocity rides
  it), and `time2` 7000 from the second snapshot on. VERIFIED
  (`play-1.7-hardpoint.txt`). The think stores `(int)(1000 *` the
  vehicle record's +0x8c`)` in `s.time2` (0x80a03b8). VERIFIED (the
  disassembly). That +0x8c is `suspensionTravel` (7 on `cobra_mp`, the
  only key of the file at 7) is INFERRED. vcod sends all four.
  VERIFIED (`hardpoint_ab`).
- `VEH_InitPhysics` (0x809d13a) starts the rates: `maxPitch` and
  `maxRoll` 25, `maxAngleVel` (45, 90, 45), yaw acceleration 25 and
  deceleration 15, overshoot 0.1, drag speed 1056 (60 mph), turning
  ability 0.5, hover radius 30, hover speed 14.08 (0.8 mph) and rate 8.8
  (0.5 mph). VERIFIED (the disassembly).
- The methods: `setSpeed` (0x809be2a) takes
  miles an hour times 17.6 (0x8215108), the deceleration half the
  stored acceleration unless given (0.5 at 0x8209628); a helicopter
  whose speed is under the new speed and the new speed under the
  acceleration gets the acceleration capped at the speed; a speed below
  0 is an error and 0; an acceleration or deceleration not above 0 an
  error, both then 17.6. `resumeSpeed` (0x809cf4a) the acceleration
  times 17.6. `setYawSpeed` (0x809ce22) degrees as given: speed,
  acceleration, deceleration (the acceleration unless given), overshoot
  in 0..1. `setMaxPitchRoll` (0x809cd6c) only on a helicopter.
  `setTurningAbility` (0x809d0ae) and `setNearGoalNotifyDist`
  (0x809cb42) as given; `setAirResistance` (0x809cd36) times 17.6;
  `setHoverParams` (0x809cc24) radius, speed and rate as given, units a
  second, not miles an hour. `setVehGoalPos(pos, stop)` (0x809ca7c)
  stops when the second argument is non-zero, refuses a zero speed,
  acceleration or deceleration, and leaves the hover for the move.
  `setTargetYaw` (0x809c9da) as given. `setTurretTargetVec` (0x809d48a)
  refuses a player's vehicle and one with no health.
  `setDamageStage` (0x80ba890) writes `un1` (+0xd4). VERIFIED (the
  disassembly).
- The think (0x809e38a, `VEH_UpdateMoveToGoal` with the orientation
  inlined) flies toward the goal, or in the hover
  the goal plus the wander, at the set speed (the hover's speed and
  rate, its deceleration half the rate, while hovering); a stop goal or
  the hover brakes once the new speed's stopping distance reaches the
  goal, flying the frame's first part at the old speed; under the set
  speed a turn never brakes the motion; the change is capped at the
  rate a frame; then, outside the hover, a stop eases the horizontal
  speed into the goal, a turn tighter than the speed allows brakes by
  `setTurningAbility` (by 1 toward a stop), and a climb or descent eases
  to the goal's height (at most three frames of acceleration a frame).
  `phys.accel` goes half way to the frame's change, capped like it
  unless stopping. The position takes the mean of the old and the new
  velocity. The yaw turns to the goal yaw once the stop is close enough
  to turn into it, else the target yaw, else (hovering) holds, else
  along the move, at `setYawSpeed`'s rates, a fifth of them once it
  swings past the goal yaw, `goal_yaw` on reaching it; pitch and roll
  toward `maxPitch` and `maxRoll` times the smoothed acceleration plus
  the fake drag (the horizontal speed's share of the drag speed, squared,
  times 5) along and across the body, times `f + (1 - f) 0.1` where `f`
  is the acceleration over the file's `accel`, scaled down while
  stopping by the stopping time (`3.5 (1 - f) + 2.5 f`); the tilt rate
  `45 g + (1 - g)`, its deceleration 0.4 of it, `g` the set
  acceleration's share (the hover's once a hovering body is within 5
  degrees and 3 a second of level). Outside the hover it notifies
  `near_goal` on every frame it starts nearer than
  `setNearGoalNotifyDist`, and `goal` when a non-stop goal is within a
  frame's flight, or a stop goal inside the hover radius under 35.2
  (2 mph; at rest with no radius), which starts the hover. VERIFIED
  (the disassembly). The hover's wander is `G_flrand` off the level's
  `rand()` (cod4-combat.md 10.9): `hardpoint_ab`'s hover, on the run's
  recovered seed, stays within the flight's tolerance of retail's.
  VERIFIED (the gate). vcod's `Heli4::think` is this; left out: the
  move trace and the touch. INFERRED (the code against the
  disassembly).
- The turret tracks its target from `tag_flash` at `turretRotRate` a
  second inside the spans and notifies `turret_on_target` once both
  angles were within a degree, else `turret_not_on_target`.
  INFERRED (`VEH_UpdateAim`).
- `fireWeapon([tag[, target]])` raises `EV_FIRE_WEAPON` (26,
  `vcod_cod4::pmove::weapon::ev`, VERIFIED) on the helicopter. A bullet
  weapon (`cobra_20mm_mp`) traces from the tag along the gun, the hip
  spread's disc around it, and hurts the body it meets as the owner's
  round with the helicopter the inflictor; a projectile (`cobra_FFAR_mp`)
  bursts as a grenade does, its hit `MOD_PROJECTILE` and its splash
  `MOD_PROJECTILE_SPLASH`. INFERRED. The projectile is
  `Weapon_RocketLauncher_Fire` (0x80a2d80, called at 0x809de60) along the
  tag's axes (`G_DObjGetWorldTagMatrix`, 0x80cbe9a) at the weapon's
  `adsSpread` (+0x71c), the helicopter's velocity (+0x138) as the gun's,
  the second argument the target and the third its offset (0x809dde2,
  0x809ddfc); its flight is "Missiles under 1.7". VERIFIED (the
  disassembly). vcod turns the bind-pose tag by the body and takes
  `Heli4::vel` for the velocity. INFERRED (the code).
- A helicopter is linked with `r.contents` 0x2080 and `svFlags` 4
  (`VEH_InitEntity`, 0x809c650, above), so a locational trace meets its
  model's collision:
  `DObjGeomTraceline` over the xmodel's `collSurfs` (model space at the
  bind pose; a surface on a moved bone traces in that bone's frame).
  INFERRED (`SV_ClipMoveToEntity`, `XModelTraceLineAnimated`).
  On ours a player's round, a rocket's move and a blast meet it so,
  every surface at the bind pose (the turret's and rotors' bones are not
  posed); a blast measures to the surfaces' box at its origin, unturned,
  and reaches it with one clear trace to the box's middle. INFERRED (the
  code).
- `G_Damage` on it, INFERRED: `G_VehImmuneToDamage` passes a hit
  by unless the vehicle file allows its means of death (bullets
  `bulletDamage`, or `armorPiercingDamage` with damage flag 2; grenades
  `grenadeDamage`, or `heavyExplosiveDamage` for a heavy explosion;
  `projectileDamage`, `projectileSplashDamage`; `MOD_EXPLOSIVE` always;
  anything else never; `cobra_mp` sets all six to 1, VERIFIED the
  rawfile), then at least 1 comes off `health` (99999 from
  `Heli_InitFirstThink`), and `damage` is notified with (damage,
  attacker, direction, point, means of death, model, tag, part, damage
  flags), the three names empty. No location multiplier applies.
  `Helicopter_Pain`'s jolt scales by the file's `maxBodyPitch` and
  `maxBodyRoll`, both 0 on `cobra_mp`, so it moves nothing.
- The stock `_helicopter.gsc` does the rest: `heli_damage_monitor` adds
  the damage up (bullets at `scr_heli_armor_bulletdamage` 0.3 under
  `scr_heli_armor`), `heli_health` sets the damage stage (3, then 2 and 1
  as it smokes), and past `scr_heli_maxhealth` `heli_crash` flies the
  crash path, spins it (`setTargetYaw`), sets stage 0 and blows it up.
  `cod4::hardpoint_tests::a_helicopter_shot_down_crashes` shoots it down
  with an allied rifle on ours: stages 3, 1, 0 on the wire, the entity
  gone long before its loop would end. VERIFIED (the test, ours).
- A player's rocket launcher fires `Weapon_RocketLauncher_Fire`'s rocket
  (0x80a2d80, from `FireWeapon` at 0x80a31b0): `tan(spread) 16` of disc
  around 16 ahead (the angle `G_random() 360`, the radius `G_random()`,
  0x80a2db7), no gun velocity and no target (0x80a3193), the firer
  pushed back 64 along the view (0x80a2ef8). VERIFIED (the disassembly).
  It starts at the eye on vcod. INFERRED (the code). The rocket bursts on
  impact (`MOD_PROJECTILE`, the splash `MOD_PROJECTILE_SPLASH`) or at
  `projLifetime`. INFERRED.
- `playFxOnTag(fx, ent, tag)` indexes `"%02d%s"` into the effect tags
  from 1698 and puts `EV_PLAY_FX_ON_TAG` (56) on the entity, the tag
  checked against the model's bones (`model_tags`). The event number is
  VERIFIED (`vcod_cod4::net::events`); the rest is `Scr_PlayFXOnTag`, INFERRED. The shared handler checked the tag against a CoD1
  model and failed every plane's and bomb's call.
- `physicsLaunch([contact, force])` (0x809afb4) takes a
  `script_brushmodel`, `script_model`, `script_origin` or `light`, both
  vectors or neither (zeros), and refuses a second launch ("physicslaunch
  called more than once for the same entity."); it sets `pos.trTime` to
  the level time and `pos.trDuration` 0x7fffffff, both `trBase`s where
  the trajectories stand, the contact in `pos.trDelta` and the force in
  `apos.trDelta`, both `trType`s 8 (`TR_PHYSICS`), `r.contents` 0 and
  `takedamage` 0. VERIFIED (the disassembly). The server never moves a
  launched part: `BG_EvaluateTrajectory` (0x80536ba) reads type 8 as it
  reads 0, 1 and 11, at `trBase`. VERIFIED (the disassembly). vcod
  sends the same words, keeps the part where it stood and makes it
  `notSolid` and undamageable (`hardpoint_tests::physics_launch_and_the_explosion_sphere_go_out`).
  VERIFIED (the test, ours).
- `physicsExplosionSphere(origin, outer, inner, magnitude)` (0x80bd00e)
  wants four arguments, raises `EV_PHYS_EXPLOSION_SPHERE` (57) as a temp
  entity at the origin (0x80cbd96) with the outer radius as its parm,
  the inner radius in `lerp.u` word 0 (+0x54) and the magnitude in word
  1 (+0x58), and errors on a negative inner radius or one past the
  outer. VERIFIED (the disassembly). The event table sends both words
  (`lerp.u.anonymous.data[0]` and `[1]`, `vcod_cod4::net::fields`), and
  vcod's event entity carries both (the same test). VERIFIED (the test,
  ours). Nothing else happens on the server. VERIFIED (the disassembly).
- On a CoD4 entity with no client, `pers` and `maxhealth` are its own
  fields: the helicopter sets `chopper.pers["team"]` and
  `chopper.maxhealth`, which failed on ours as client fields. VERIFIED
  (the error). 1.7 reaches the client fields only through
  `ent->client`. INFERRED.

#### The flight against retail

`cod4::hardpoint_ab` (needs `COD4_DIR`) replays
`crates/cod4/tests/fixtures/net/play-1.7-hardpoint.txt`: retail
`cod4_lnxded` 1.7 under `+set fs_game mods/vcodhp +devmap mp_crash`,
`war`, sv_pure 0, with the probe gsc
`crates/cod4/tests/fixtures/gsc/hardpoint_flight/maps/mp/mp_crash_fx.gsc`
packed into the mod's iwd (retail reads it over the zone's), and one
`cod4_probe` (`--team axis --script idle --name owner --qport 4021
--secs 75`) under `unshare -rnm`. A second after the probe spawns the
gsc flies two planes (a steady `moveTo` and one with both eases) and a
`cobra_mp` helicopter along mp_crash's first loop nodes (no stop, no
stop, stop), hovers it, turns it 120 with `setTargetYaw` while the gun
takes a point below, then flies it home under `setHoverParams(60, 20,
10)`, `setAirResistance(40)` and a slower `setSpeed`, and deletes all
three. No script random draw. The run's seed is 1936999990 (the probe's
challenge at draw 17, `wire_ab::recover_game_seed`). VERIFIED (the
capture). Ours loads the same file (`Cod4Server::set_script_overrides`)
and compares every plane and helicopter word at the same time past its
spawn.

- The planes match bit for bit. VERIFIED (the gate).
- The helicopter matches bit for bit for its first 1.5 s; the turn
  through the first node then parts by 0.005 degrees of pitch and 0.08
  of yaw, and the rest of the flight stays within 8 units, 1.9 degrees
  of body and 1.6 of gun. VERIFIED (the gate). That the parting is the
  float model (retail's x87 intermediates against ours in `f32`, steered
  by a goal vector shrinking to nothing at the node) and not a rule is
  INFERRED: the think's rules read the same as the disassembly's.
  `hardpoint_ab::GAPS` tolerates it.

#### Missiles under 1.7: attractors, the wobble, guidance

Read off `cod4_lnxded` 1.7 and the stock weapon files (`main/iw_11.iwd`
`weapons/mp/rpg_mp`, `cobra_ffar_mp`); vcod's `game::cod4::missile`
(`g_fire_rocket`, `missile_trajectory`, `steer`, `wobble_and_attract`,
`attract`) and `builtins::cod4_vehicle` follow it. The weapon def's
fields (`vcod_cod4::pmove::weapon::WeaponDef`): `projectileSpeed` +0x568
(an int), `projLifetime` +0x578, `timeToAccelerate` +0x57c,
`projectileCurvature` +0x580, `guidedMissileType` +0x6b8,
`maxSteeringAccel` +0x6bc, `destabilizationRateTime` +0x810,
`destabilizationCurvatureMax` +0x814, `destabilizeDistance` +0x818 (an
int). The entity's: `missile.time` +0x1b4, the curvature (or the
wobble) +0x1d0, `targetOffset` +0x1dc, the target's handle +0x214
(number plus one), its own flags +0x180 (not `s.eFlags`). VERIFIED (the
disassembly).

- The attractors are 32 slots of 0x1c bytes at 0x8361a80: in use,
  attractor (1) or repulsor (0), the entity (0x3ff for a point), the
  point, the strength, the reach. `missile_createAttractorEnt(ent,
  strength, maxDist)` (0x8093486), `missile_createAttractorOrigin`
  (0x8096c28), `missile_createRepulsorEnt` (0x8096b5c) and
  `missile_createRepulsorOrigin` (0x8096a80) take the first free slot
  ("Ran out of attractor/repulsors.  Max allowed: %i"), refuse a reach
  not above 0 ("maxDist must be greater than zero") and return the
  slot; the entity forms set the entity's flag 0x800000.
  `missile_deleteAttractor(n)` (0x80933b0) clears slot `n` ("Invalid
  attractor or repulsor" past 31); freeing an entity clears every slot
  naming it (0x8093402); a level start clears them all (0x8093478).
  VERIFIED (the disassembly). vcod clears a slot whose entity is gone
  when a slot is next taken or a missile next reads it. INFERRED (the
  code). `missile_setTarget([ent[, offset]])` (0x80c5f34) refuses any
  entity but a `rocket` ("Entity %i is not a rocket"), sets the target
  (none without one) and the offset (0 without one). VERIFIED (the
  disassembly).
- `G_FireRocket` (0x8096d04): the `rocket` entity, `eType` 4, `eFlags`
  0x400; the launch time is the level time, plus the no-draw time
  (`clamp((int)(85 - 35 speed / 600), 20, 50)`) only when the parent is
  a client, so a helicopter's rocket is drawn at once; `nextthink` the
  level time plus `(int)(projLifetime 1000)`, at most 60 s on; the
  target and its offset; `pos` `TR_LINEAR` at `trBase` the start,
  `trDelta` the direction times the speed, or `TR_INTERPOLATE` from rest
  under a `timeToAccelerate`, the gun's velocity added and each
  component truncated (0x8096f69); the angles `vectoangles` of the
  direction. A curving weapon goes `TR_INTERPOLATE` and draws its
  curvature: `r1 = G_random()`, `r2 = G_random() projectileCurvature`,
  the vector `right cos(r1 360) r2 + up sin(r1 360) r2` off the angles
  (0x8096fe7). A guided one puts both trajectories `TR_INTERPOLATE`
  (0x80970af). `missile.time` is `destabilizeDistance / speed 1000`
  (0x80970d1), the only use of `destabilizeDistance`; the flags take
  0x20000 when `destabilizationRateTime` is 0, else the parent's 0x20000
  (0x80970fb). VERIFIED (the disassembly). Javelin's start (guidance 3:
  the climb, 10 to the parent's right) is not on vcod; no stock MP
  weapon is one. INFERRED (the weapon files).
- The missile trajectory step (0x8095928), `G_RunMissile`'s first step (called
  at 0x8097322, before the move trace), moves a rocket (handler 9) whose
  `pos.trType` is not 2 once the level time is past its launch time;
  anything else is `BG_EvaluateTrajectory`. Under `timeToAccelerate` the
  speed along the facing grows by `speed / timeToAccelerate` a frame's
  worth (0.05 s) until the forward speed is the speed, then exactly to
  it; a curving weapon adds a frame of its curvature; `trBase` steps a
  frame of `trDelta`; past `pos.trTime` plus `timeToAccelerate` the
  angles follow `vectoangles(trDelta)` (both trajectories stay
  `TR_INTERPOLATE`). VERIFIED (the disassembly).
- The steering (inlined at 0x8095a95), for guidance 1 (Sidewinder) and
  2 (Hellfire) with a target, past `pos.trTime` plus `timeToAccelerate`
  (`tta + (now - trTime) -0.001 <= 0`, 0x8095c4f): the flat heading `h`
  and speed `hs` of `trDelta`, the target's `r.currentOrigin` plus the
  offset less `trBase` in that frame (`rel0` ahead, `rel1` to the left,
  `rel2` up), the turn radius that meets it `(rel0^2 + rel1^2) / 2
  rel1`, the tightest turn `speed^2 / maxSteeringAccel`. Ahead: no side
  push when the turn is tighter than that, else `2 hs^2 / radius`
  within `maxSteeringAccel`; behind: no push when the turn is within 60
  of the tightest, else the full push to its side. The push is along
  `(h1, -h0)`, a frame of it. The vertical push (guidance 1, or a
  Hellfire past its climb) heads for the height at which the flat run
  reaches it, `(|hs d0| rel2 / |rel| - trDelta.z) 20` within
  `maxSteeringAccel`, none when the target is behind; a Hellfire first
  climbs under `missileHellfireMaxSlope` (0.5) and
  `missileHellfireUpAccel` (1000) until it would climb no more
  (`pos.trDuration` 1). Both pushes come off the velocity before either
  is added (0x8095e6b). VERIFIED (the disassembly). No check of the
  target's health: a dead body is still steered at; `G_FreeEntity`
  clears the handle (0x80cc1d7), and the rocket flies on unguided.
  INFERRED (control flow). The Hellfire's dvars are left at those values on vcod.
  INFERRED (the code). `hind_FFAR_mp` (and `cobra_FFAR_mp`) is
  `Sidewinder`, `maxSteeringAccel` 2000, `timeToAccelerate` 1,
  `projectileCurvature` 10, speed 6000. VERIFIED (the weapon file).
  `hardpoint_tests::the_helicopter_guides_its_rocket` fires one with
  `_helicopter.gsc`'s `fire_missile("ffar", 1, target)` at the allied
  target 4000 ahead: it leaves at the body's speed and gains over 200 a
  frame to past 5000 in a second, flies the same as one with no target
  for that second and then steers, and passes nearer the target.
  VERIFIED (the test, ours).
- The wobble and the attractors (0x8097984) follow the move only when it
  met nothing and the velocity is not nil, for a `projectile` weapon
  (+300 = 2) with no guidance and without the flag 0x20000; so a guided
  rocket never wobbles or bends. Once `pos.trTime + (int)missile.time`
  is past, each of the three angles draws a rate: `G_flrand(-1, 1)`
  (0x80cb548) times `destabilizationCurvatureMax` the first time, else
  `G_flrand(0, 1)` times it, pitch negative when the last yaw rate was
  and yaw negative when the last pitch rate was positive; `pos.trTime`
  becomes the level time, `missile.time` `destabilizationRateTime 1000`,
  and the flag 0x10000 goes on. While the flag is on every frame steps
  `apos.trBase` by a frame of the rates (`G_SetAngle`), the velocity is
  their forward times the speed, `trBase` the missile's place, and both
  trajectories `TR_INTERPOLATE`; before the first draw nothing of this
  runs, the attractors included (0x809872f). Then each slot in use whose
  point is ahead along the velocity: the side offset's direction
  (straight down for a repulsor whose offset points up or is nil; an
  attractor dead ahead is skipped; the upward repulsor's `|side|` is
  then 0, 0x8097ce4), `s = (1 - d / reach) strength` within
  the reach `d`, the share of the angle off the line `a =
  atan(|side| / ahead) 0.63662`; a repulsor adds `(a - 1) s` along the
  side's direction, an attractor `min(s a, 20 |side| speed / ahead)`.
  The sum, a frame of it, bends the velocity, which is then set back to
  the speed, and `vectoangles` of it goes to `apos.trBase` alone
  (0x8098782..0x809882d). VERIFIED (the disassembly). `rpg_mp`'s speed
  is 1500 and its first wobble 200 ms out (`destabilizeDistance` 300),
  then every 0.4 s; `_helicopter.gsc` puts an attractor on each
  helicopter (`scr_heli_attract_strength` 1000, `scr_heli_attract_range`
  4096). VERIFIED (the weapon file, the script, the test).
  `hardpoint_tests::the_helicopter_pulls_a_wide_rpg_in` fires an RPG 20
  degrees wide of the helicopter from 950 off with the stock strength
  and with 0: the same flight up to the first wobble (the same draws off
  the level's `rand()`), then the attracted one passes the body over 20
  nearer. VERIFIED (the test, ours). No retail capture of a rocket's
  flight backs this yet.

### 3.3 The script traces

`builtins/cod4_trace.rs`: `bulletTrace`, `bulletTracePassed` and
`sightTracePassed` trace the clip map, its static models (a point
trace), the linked brush entities and, with `hitCharacters`, the bodies
the last frame linked, passing the ignored entity. INFERRED (the code).
What 1.7's handlers in `cod4_lnxded` do:

- All three read `start` and `end` as vectors, `hitCharacters` as an
  int and `ignoreEnt` as an entity or else `ENTITYNUM_NONE` (1023,
  0x80c665b). A 0 `hitCharacters` takes 0x2000000 (`CONTENTS_BODY`) off
  the mask (`cmp`/`sbb`/`and 0xfe000000`, 0x80c6635). VERIFIED (the
  instructions).
- `bulletTrace` (0x80c65fa) and `bulletTracePassed` (0x80bee92) load
  mask 0x2806831 (0x80c6640, 0x80beed8) and call the locational trace
  (0x80b4020) and its passed form (0x80b3f06), each with a null priority
  map argument (0x80c6660, 0x80beef4). `sightTracePassed` (0x80c0198)
  loads 0x2801803 (0x80c01de), calls `G_SightTrace` (0x80b3ebe) and
  returns whether its hit number is 0. VERIFIED (the instructions).
- `bulletTrace`'s array is built in this order: `fraction`, `position`
  (`start + (end - start) * fraction`, 0x80c66a2), `entity` (undefined
  when the hit id is 1022 or 1023, 0x80c66f6), then `normal` and
  `surfacetype`. With `fraction < 1` (0x80c6743) the normal is the
  trace's plane and the surface the name of `(surfaceFlags & 0x1f00000)
  >> 20` (0x80c6806); else the normal is `end - start` normalized (0
  for a point) and the surface the constant string `none`. VERIFIED
  (the instructions).

vcod's surface names are `vcod_cod4::sound::SURFACES`, and
`cod4_trace::tests::bullet_trace_fills_its_array_for_a_miss_and_a_hit`
checks the array's keys and both arms. VERIFIED (the test). vcod's bullet
traces still score bodies under `bulletPriorityMap` where 1.7 passes a
null map; what the null map does inside the hit-box walk is not read
yet. INFERRED. The
two bullet traces are `G_LocationalTrace`s under `bulletPriorityMap`: a
playing body scores its hit boxes, posed as the last frame left it
(`combat::PosedBody4`, the shots' own bone trace), and reads the flesh
surface; a helicopter its model collision (3.2). INFERRED. The
sight trace, the cone traces and `physicsTrace` meet a body as its link
box. INFERRED (the code). `getTagOrigin` and `getTagAngles` on a playing
body read its posed bone, attachments included (`G_DObjGetWorldTagPos`); anything else reads its model's bind pose. INFERRED. The
level writes the bodies, poses and brushes after its entity pass
(`trace_bodies`, `posed_bodies`, `trace_brushes`), so a thread reads the
last frame's. INFERRED (the code).
`cod4::hardpoint_tests::bullet_traces_meet_posed_bodies_and_the_helicopter`,
`cod4_trace::tests::a_body_cuts_a_trace_that_hits_characters`.

### 3.4 `linkTo` on a model's tag

A non-client parent's tag is the same bind-pose bone `getTagOrigin` reads
(the zones' `model_tags`): CoD4's xmodels live in the zones, and the
`.iwd` search path the CoD1 and CoD2 `linkTo` loaded models through holds
none, so every tag link to a script model failed with "failed to link
entity since tag ... does not exist". On mp_backlot that was each
destructible sedan's part sounds (`_destructible.gsc` through
`_utility::play_sound_on_tag`, 20 errors a `sab` level). VERIFIED (the
map sweep, `cod4-map-sweep.md`; `cod4::gametype_tests::a_sound_links_to_a_destructible_cars_tag`).
Retail links them: the same `getTagOrigin` one line above in the stock
script finds the tag. INFERRED (the script).

### 3.5 The world's classname

`SP_worldspawn` (`cod4_lnxded` 0x80c81d2) ends by writing the world's
entity: `s.number` 0x3fe (0x80c83e0), `classname` `scr_const.worldspawn`
(the string at 0x821792f, interned into 0x836f16c at 0x80a409a) through
`Scr_SetString` (0x80c83ea..0x80c83fc, gentity offset 0x170), then
`r.inuse` 1 (0x80c8401). VERIFIED (disassembly). 1.1d's `SP_worldspawn`
writes `scr_const+0x9c` into the world's classname the same way
(`game.mp.i386.so` 0x61e84..0x61e91). VERIFIED (disassembly). CoD2's
(`cod2_lnxded` 0x811b56a) ends with a `Scr_SetString` of a `scr_const`
slot into the world too. INFERRED (the code names neither string). A blast with no attacker (`sd.gsc` 807,
`_destructible.gsc` 1064) hands the callbacks the world, and
`Callback_PlayerKilled:4787` compares `attacker.classname` with
`"script_vehicle"`: with the classname undefined that comparison is a
script error on every such kill (the map sweep's `sd` and `sab` rows).
vcod's world (`ObjectTable::world`) carries `worldspawn` on every game.
VERIFIED (`cod4::gametype_tests::a_blast_with_no_attacker_names_the_world`).

## 4. The load

- Zones: `code_post_gfx_mp`, `common_mp`, then the map's, the three names
  the capture's `sv_referencedFFNames` lists. VERIFIED. The iwds are
  `main/*.iwd` under them (`Cod4Source`).
- `Cod4Scripts` reads a loose `.gsc` from an override directory first, then
  the zones' rawfile. A loose file wins only under `fs_game` on retail, INFERRED, not yet
  confirmed on 1.7; the directory stands for that.
- Bootstrap: the gametype's `main`, the map's `main`, then
  `CodeCallback_StartGameType`, each run to its first wait, as CoD2's.
  INFERRED.
- `InitStructs` runs at every load, so `level.struct` is always an array:
  mp_crash has no `script_struct` and `common_scripts/utility.gsc`'s
  `struct_class_init` reads `level.struct.size`. INFERRED from `G_LoadStructs` and the corpus (`init_structs_always`).
- The engine's start-up `exec default_mp.cfg` (string in the binary,
  VERIFIED) runs `default_mp_gamesettings.cfg` from `iw_00.iwd`, which sets
  `scr_war_scorelimit 750`: the capture's `ui_scorelimit` is 750 where
  `war.gsc`'s own default is 500. VERIFIED. `cod4_boot` runs that file
  ahead of the load in its place; vcod-server's start-up runs it as
  retail's does.

### 4.1 `wait` counts frames

- A `wait` resumes on a whole 50 ms frame: the seconds times 20 rounded
  half up, a wait under half a frame (not `wait 0`) one frame. On retail
  `cod4_lnxded` 1.7 (map mp_crash, `war`, one idle probe client, the
  `logPrint` probe in `tmp/`'s scratch over `fs_game`), threads started on
  one frame read `getTime()` 600 after `wait 0.6`, `0.6001`, `0.62` and
  `0.624`, 650 after `0.626`, `0.643`, `0.649`, `0.65` and `0.651`, 50
  after `0.01`, `0.024`, `0.026` and `0.0001`, 0 after `wait 0`, 1000
  after `1.0249` and after `1.025` (a float a hair under it). VERIFIED
  (the log). CoD1's rounds to the millisecond and runs the thread on the
  first frame past it, which put 0.643 a frame early.
  `crates/gsc/tests/cod4_waits.rs` runs the probe's waits through
  `VmProfile::COD4` against these lines and fails with `frame_waits`
  off. VERIFIED (the gate, ours).
- A script mover's `origin` read after such a wait is the trajectory at
  that frame's time: a `moveTo` 10000 units in 1 s read 5500 at 600 and
  6000 at 650; a `moveGravity` (1000, 0, 0) from z 5000 read x 800 z 4744
  at 850, x 950 z 4639 at 1000. VERIFIED (the log). vcod-server read the
  same lines once its waits counted frames (`VmProfile::COD4.frame_waits`,
  gate `vm::sched::tests::cod4_waits_count_whole_frames`).
- The airstrike times its bomb by these waits (`callStrike_bombEffect`'s
  `wait (bombTime - 1.0)`, 2.643 s: 53 frames, not 2643 ms). A frame
  late, the bomb opened 100 to 250 units past its mark and the bomblets'
  `losRadiusDamage` found nobody where the stock aim put them; on frames
  they kill the player standing at the target
  (`cod4_live_three_kills_earn_the_airstrike_and_call_it_in`). That gate
  no longer tells the two waits apart: with the scripts' draws reseeded
  at the hardpoint key, it kills or misses alike on each of 12 seeds with
  `frame_waits` on and off, so whether the stock path and spray reach him
  is the draws'. VERIFIED (the gate, ours).

### 4.2 An `endon` belongs to its call

- On retail `cod4_lnxded` 1.7 (mp_crash, `war`, no client, a `logPrint`
  probe over `fs_game` in `tmp/`'s scratch, `probe_endon_return`):
  - a function that runs `level endon("x")` and returns leaves nothing
    behind: `x` notified afterwards does not end its caller, which runs
    on (`after_return_survived 1`);
  - `x` notified while that function still waits in a `waittill` ends
    the function alone: its caller resumes in the same frame (the
    notify's `getTime()` 1900, the caller's line 1900), the function's
    lines after the `waittill` never run (`inside_outer_ran h2=0
    t=1900`); that the caller reads its value as `undefined` is INFERRED;
  - the stock airstrike's chain (a called function's `endon`, a thread
    it started notifying it 0.05 s after the function returned) leaves
    the caller running (`chain_survived 1`). VERIFIED (the log).
- That fits a child thread per call (`AllocChildThread` on every script
  call, `Scr_KillThread` on its return dropping the endons registered on
  it). INFERRED.
- vcod: `VmProfile::COD4.endons_end_with_call` (`Vm::set_endons_end_with_call`):
  an endon records the depth of the call that ran it, a return drops its
  call's, and a notify of one at depth `d > 1` truncates the parked
  thread to its caller, pushes `undefined` and makes it runnable; one in
  the thread's own body still ends the thread. CoD1 and CoD2 keep
  thread-wide endons. A thread mid-step under a nested spawn's notify is
  still ended whole. Gate: `vm::sched::tests::cod4_endons_end_with_their_call`.
- Before, `hardpointItemWaiter` died once an airstrike was called:
  `selectAirstrikeLocation`'s `self endon("stop_location_selection")`
  stayed on the waiter thread and `finishAirstrikeUsage`'s
  `stopAirstrikeLocationSelection` notified it 50 ms later. The next
  hardpoint of that life (the helicopter) raised its radio and nothing
  waited on its `weapon_change`: the radio stayed in hand until the next
  spawn started a new waiter. An Esc out of the selector ended the
  waiter the same way. Gate:
  `play::cod4::tests::cod4_live_airstrike_then_helicopter_in_one_life`.
  VERIFIED (ours).

## 5. Configstrings

- `CS_LAYOUT_COD4` (`game::cod4`) is the `CS_*` enum:
  localized 309, models 830, sound aliases 1342, effects 1598, effect tags
  1698, shellshocks 1954, menus 1970, materials 2002, weapon files 2258,
  status icons 2259, head icons 2267, tags 2282, items 2314; fog 9, motd 10,
  game end time 11, map centre 12, vote 13, winner 19, the dvar mirror's
  names 20..147 and values 148..275, hint strings 277, ambient 821, north
  yaw 822, minimap 823, vision sets 824 and 825. Every slot the capture
  fills sits where it says. VERIFIED (capture). The worldspawn's message is
  slot 3 and the ambient track 821 (`SpawnRules::message_slot`,
  `ambient_slot`); the `n\ambient_crash\t\0` format is CoD1's. VERIFIED
  (capture).
- The server's table starts empty and the gamestate leaves out a slot equal
  to its `constantConfigStrings` default (`cod4-handshake.md` 5). A new name
  whose constant slot lies inside its range takes that slot; any other
  takes the first empty slot no constant names, the first empty one when
  those run out; a name is found anywhere in its range. Menus and icons
  keep their own scan, the first empty slot from the base. The rule is
  `G_FindConfigstringIndex`'s, INFERRED for 1.7. It puts
  every name where the capture has it: shellshocks `default` and
  `concussion_grenade_mp` on their constants 1968 and 1969 but `flashbang`
  on 1955 (its constant 1970 is a menu slot), localized strings around
  350 and 351, weapon models on their constants, menus sequential from
  1970 with `scoreboard` on 1977 rather than its constant 1986. VERIFIED
  (`cod4_boot`). Names from 821 on are stored lowercased
  (`smoke/battlefield_smokebank_S` reads `..._s`). VERIFIED (capture).
  vcod's `Allocators::with_constants` is this rule.
- `cod4_boot` compares slots 2, 9, 11, 12, 19 and 309..2314 one for one
  against the capture after 30 s of an empty server (the prematch's 15 s
  `scr_game_playerwaittime` and 5 s `scr_game_matchstarttime` end at 20 s, which sets 11 to `619800` and 824 to
  `"mp_crash" 2000`): all equal, its `GAPS` empty. VERIFIED. The dvar
  mirror is compared by name for the scripts' `scr_*` and `ui_*` dvars,
  all equal. VERIFIED. The engine's own mirrored dvars (`friction`,
  `g_TeamColor_*`, `jump_height`, `player_*`, ...) are not registered on
  `cod4_boot`'s bare host, so its block's sorted slots differ from
  retail's. vcod-server registers them (`cod4::oob`), and `cod4::oob_ab`
  compares its mirror with retail's.

## 6. Callbacks

`_callbacksetup.gsc`'s signatures (VERIFIED, the rawfile):

```text
CodeCallback_StartGameType()
CodeCallback_PlayerConnect()
CodeCallback_PlayerDisconnect()
CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset)
CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
CodeCallback_PlayerLastStand(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc, timeOffset, deathAnimDuration)
```

All six names are strings at 0x821c247 .. 0x821c2cf. VERIFIED. The damage
and killed callbacks take CoD2's extra arguments; `HostProfile::COD4`
sets `callback_extra_args`.

`modNames` registers `MOD_UNKNOWN` .. `MOD_EXPLOSIVE` and `MOD_IMPACT` (16)
from 0x80a4109, in string-pool order. INFERRED (`game::cod4::COD4_DEFS`).

## 7. The connect, the menus and the spawn

`vcod-server --game cod4` runs the level (`game::cod4::level::Cod4Level`)
and hands it each client: `ClientConnect` when the connect is accepted,
`ClientBegin` on the first move, the `mr` answers, the spawns. The gate is
`cod4::wire_ab` (needs `COD4_DIR`): the three probes of
`play-1.7-walk.txt` and `play-1.7-spec.txt` (cod4-netfields.md 8) join
ours in process on retail's level clock, and the gamestate, both clients'
server commands up to 52 s, their chat lines and the walker's spawn frame
are diffed. Every command the two retail clients read up to the cut
matches, in order. What still differs is the gate's `GAPS`.

### Server commands

- The connect callback's two `v` lines go out before the gamestate (the
  walker's commands 1 and 2, `v ui_3dwaypointtext "1"`, `v
  ui_deathicontext "1"`), the rest after `ClientBegin` in the first
  snapshot. VERIFIED live. The callback runs inside `ClientConnect`, at
  the connect. INFERRED.
- `iPrintLn(&"MP_CONNECTED", self)` reaches the clients as ` "MP_CONNECTED\x15walker^7"`:
  the command letter is a space. VERIFIED live. vcod's shared message
  packer already writes that form.
- The commands the connect, menus and spawn send, as the capture reads
  them: `v name "value"`, one `v` with several pairs, `H 0` then `G 0`,
  `t <menu>`, `u`, `L`, `J "1"`, `a 27`, `C 40`, `s 1` (the alias at
  configstring 1343) and `d <slot> <string>`. VERIFIED live. The builtin
  behind each, as the stock scripts' call order gives them, is INFERRED: `setClientDvar`,
  `setClientDvars`, `updateScores` (allies then axis), `openMenu`,
  `closeMenu`, `closeInGameMenu`, a `giveWeapon` that gives,
  `setSpawnWeapon`, the equipped offhand, `playLocalSound`, a configstring
  write. `setTeamScore` sends `H` or `G` to everyone and writes no
  configstring. INFERRED.
- A command's type is read off its letter: `J`, `G`, `H`, `I`, the
  messages `c`, `e`, `f`, `g`, the chat `h`, `i` and the letterless print
  go `SV_CMD_CAN_IGNORE`, the rest `SV_CMD_RELIABLE`. INFERRED.
  A client not yet in the world loses its pending ignorable commands and is
  sent no new one. INFERRED. The four `J "1"` in a row that the
  spawn's loadout sends are never squashed. VERIFIED live.
- `SV_AddServerCommand` squashes a reliable command into the first pending
  unsent reliable one with its letter that has the same text, or is one of
  `C D a b o p q r t`, or a `d`/`v` with the same first word: the old one
  is taken out and the new one goes last. INFERRED. The capture bears
  it out twice: `_menus.gsc` and `menuClass` each send `u` `L` on the class
  answer and the walker reads one `u` `L`, and the frag's give sends `C 40`
  before its `J "1"` and `switchToOffhand` sends it again after, and the
  walker reads `J "1"`, `C 40`. VERIFIED live.
- The rings hold 128 commands (`& 0x7f`). INFERRED. The walker is
  sent 74 in 26 s, past CoD1's 64. VERIFIED live. A 64-command ring keys a
  client message past the 64th command off the wrong text, which garbles
  it.
- Sound aliases keep their case in their configstrings (`d 1344
  US_1mc_team_deathmtch`), unlike the other names from 821 on. VERIFIED
  live.
- The colour dvars are every `g_ScoresColor_*` and `g_TeamColor_*`:
  `_scoreboard.gsc`'s three-component `setdvar` reads back four byte
  components, `0.6 0.639216 0.690196 0` for `0.6 0.64 0.69`, in
  `handshake-1.7.txt`'s mirror. VERIFIED.
- A chat line colours the speaker's name by the recipient's side: the
  walker's `say` reads `h "\x15\x15^8walker^7: ^7vcod probe here"` to
  itself and `^9walker` to the spectator. VERIFIED live. The rule is
  `G_SayTo`'s `^8` on the speaker's team, `^9` off it. INFERRED.
- `waittill` order: the mate connects a frame before the spectator. With
  both in one frame the spectator reads `v ui_xpText` (`_persistence`)
  before `_rank`'s unlock dvars; retail's reads it after them, the
  walker's (the third connect) before them. VERIFIED live. The waiters of a
  looping `level waittill("connected")` alternate with each notify, as
  vcod's newest-first resume gives. INFERRED.

### The start-up configs

- The dedicated server execs `default_mp.cfg` and its `exec`s from its
  iwds, before any zone loads (its console: `execing default_mp.cfg from
  disk`, then `execing default_mp_gamesettings.cfg from disk`, then
  `Loading fastfile code_post_gfx_mp`). VERIFIED live.
- `iw_00.iwd` holds two `default_mp_gamesettings.cfg`: 4128 bytes dated
  2007-09-13 at offset 11981, with `scr_game_playerwaittime 20` and
  `scr_game_matchstarttime 5`, and 4090 bytes dated 2007-08-23 at offset
  56739, with `scr_game_graceperiod 0` in their place. VERIFIED. Retail
  reads the later one: rcon on a fresh `war` server reads
  `scr_game_playerwaittime` "15" default "15" (the script's tweakable) and
  `scr_war_scorelimit` "750", and the start-up dvar dump has
  `scr_game_graceperiod` and no `scr_game_playerwaittime`. VERIFIED live.
  The prematch then ends at 19800 and configstring 11 reads 619800, as on
  every 1.7 capture. VERIFIED live. vcod reads the cfgs through `Pk3Fs`,
  which keeps the later entry too.

### The stats, the loadout, the perks

- `getStat` and `setStat` read and write the slot's own 0x2000-byte block,
  the one its seven `stats` packets carried, a fresh profile's until they
  are in. INFERRED (`_class.gsc` reads the class stats off it).
- `setStat` is `GScr_SetStat` (0x80c5490). It refuses a call without two
  arguments (`setstat: takes 2 arguments, got %i.`, 0x821f068), an index
  past 3499 unsigned (`setstat: invalid index %i`, 0x821c577), and a byte
  stat (index up to 1999) set past 255 unsigned (`setstat: index %i is a
  byte value, and you're trying to set it to %i`, 0x821f020), each a
  script error. It then calls `SV_SetClientStat` (0x816e712): a byte stat
  is compared and stored as a byte, an int stat (2000..3497) as a word,
  3498 and 3499 are dropped, and a changed stat goes to the client as the
  reliable `N <index> <value>` (`SV_SendServerCommand(cl, 1, "%c %i %i",
  0x4e, ...)`, the format at 0x8217f78). VERIFIED (the disassembly).
- Ahead of the argument count `GScr_SetStat` refuses a receiver with no
  client (`setstat: entity must be a player entity`, 0x821ee1c), then a
  set flag (0x8a3ce48, read by 0x814ddda) with `setstat: impure scripts
  loaded.` (0x821efdc), then `developer_script` on (`Dvar_GetBool`
  0x819e090 at 0x80c54e4) with `setstat: developer_script is false.`
  (0x821effc): the message names the opposite of what fails. Each is
  `Scr_Error` (0x815e9d4). VERIFIED (the disassembly).
- The impure flag. `Scr_BeginLoadScripts` clears it (0x814c337 calls the
  setter 0x814ddcc with 0). `Scr_ReadFile` (0x814fc52 on) reads a script
  through `Scr_ReadFile_LoadObj` (0x814e13e) when `fs_game` (0x13f9cc18)
  is empty and `useFastFile` (0x88a53f0, default 1) is off, else from the
  fastfile; with `fs_game` set it tries the loose file first. Each loose
  read ORs in `fs_game` being empty (0x814e17e, 0x814fcfb), so only a
  loose script under no mod with `useFastFile 0` sets it. VERIFIED (the
  disassembly). `developer_script` is registered off with flags 0x10
  (0x8121e47). VERIFIED. vcod reads every script from the zones, so the
  flag never sets and vcod has no check for it; the dvar's refusal it
  has (`stat_tests`).
- `getStat` is `GScr_GetStat` (0x80c55f6): `Scr_GetInt(0)` first, then
  `getstat: entity must be a player entity` (0x821f090) for a receiver
  with no client, `getstat: invalid index %i` (0x821c591) past 3499
  unsigned, and `getstat: called on a disconnected player` (0x821f0b8)
  when `sess.connected` (client +0x2f8c) is at most 0, each `Scr_Error`.
  `SV_GetClientStat` (0x816de58) reads a byte under 2000, a word to 3497
  and 0 for 3498 and 3499. VERIFIED (the disassembly). vcod reads a slot
  whose roster entry `ClientDisconnect` cleared as disconnected. The client
  stores `N` into its profile (`LiveStorage_SetStat`, INFERRED).
  vcod's `cod4_live_set_stat_reaches_the_clients_profile` joins a ranked
  (`sv_pure 1`) mp_crash: the rank scripts move stats 2317 and 2352
  (`maxxp`, 30) off a fresh profile's, and the client's block ends equal
  to the server's.
- The weapon def fields the loadout reads come out of the zone's
  `WeaponDef` at the offsets 1.7's `weaponDefFields` table gives them
  (`cod4_lnxded` 0x826a160, 502 rows; `tools/re/cod4_weapon_fields.py`):
  `weaponClass` +304, `inventoryType` +316, `offhandClass` +324,
  `startAmmo` +800, `maxAmmo` +820, `clipSize` +824, `sharedAmmoCap`
  +840, `moveSpeedScale` +1048, `clipOnly` +1280, `dropAmmoMin` +1348,
  `dropAmmoMax` +1352. VERIFIED (the table). The `weaponClass` names sit
  in enum order in the ten-pointer table at 0x826b900. VERIFIED (the
  table).
- The ammo and clip slots: weapon 0's empty names hold slot 0, and a
  weapon takes the slot of the first earlier one with its name
  (cod4-hud.md 9). The spawn frame reads `ammo[4]` 60 and `ammo[5]` 1
  (`m16_gl_mp` and its `gl_m16_mp`), `ammo[9]` 30 (`beretta_mp`), clips 11
  (15), 27 (30), 28 (1), 40 (1, the frag) and 43 (1, the stun grenade),
  which `giveWeapon`'s start ammo (`G_InitializeAmmo`, `Add_Ammo`,
  `Fill_Clip`, INFERRED) and `setWeaponAmmoClip` give. VERIFIED live.
- `ps.weapons` holds a bit per weapon (the spawn's 11, 27, 28, 40, 43),
  `ps.weaponold` the spawn weapon's (`setSpawnWeapon`). VERIFIED live.
- `setActionSlot`: `nightvision` 3, `altMode` 2 in `actionSlotType`.
  VERIFIED live. `weapon` is 1, `""` 0. INFERRED.
- `bg_perkNames`, the `perks` bits' order, is the 20-row pointer table at
  cod4_lnxded 0x820de40 (`specialty_gpsjammer` .. `specialty_specialgrenade`).
  VERIFIED. The assault class's `perks` read 0x4400 (`specialty_longersprint`,
  `specialty_bulletdamage`), and the roster carries the same word. VERIFIED
  live.
- `setMoveSpeedScale(0.95)` (a rifle class) is `moveSpeedScaleMultiplier`.
  VERIFIED live.
- `disableWeapons` sets `weapFlags` 0x80. INFERRED.

### The perks

Each perk's bit in `perks` (the `bg_perkNames` row), where 1.7 acts on
it and ours. The engine's sites are the ten tests of the playerstate's
`perks` word (gclient+0x5fc, the playerstate's own offset) in
`cod4_lnxded`'s disassembly, the addresses in the table. VERIFIED. What
each gates is read off the dvar loaded beside it. INFERRED. The
script's are `hasPerk` in the stock `common_mp` gsc (`_class.gsc`'s
`cac_modified_damage` and loadout, `_globallogic.gsc`'s Last Stand).
VERIFIED. Tests are `cod4::perk_tests::` in `vcod-server`.

| perk | bit | 1.7 | ours | test |
|---|---|---|---|---|
| UAV Jammer (`gpsjammer`) | 0x1 | client compass; the script's radar | works (radar) | none here |
| Steady Aim (`bulletaccuracy`) | 0x2 | hip spread times `perk_weapSpreadMultiplier` 0.65 (0x805f64e) | works | `steady_aim_narrows_the_hip_spread` |
| Sleight of Hand (`fastreload`) | 0x4 | reload time over `perk_weapReloadMultiplier` 0.5 (0x80636cb) | works (pmove) | `sleight_of_hand_halves_the_reload` |
| Double Tap (`rof`) | 0x8 | fire and rechamber time over `perk_weapRateMultiplier` 0.75 (0x8063926) | works (pmove; retail's rate, cod4-combat.md 2.1) | `double_tap_fires_faster`, `cod4_fire_rate_ab` |
| Iron Lungs (`holdbreath`) | 0x10 | `perk_extraBreath` 5 s more breath (0x8062e7a) | works (pmove) | `iron_lungs_holds_the_breath_longer` |
| Deep Impact (`bulletpenetration`) | 0x20 | depth times `perk_bulletPenetrationMultiplier` 2 (0x808aa10, 0x808abe9) | works | `a_round_goes_through_a_wall_and_deep_impact_goes_further` |
| Martyrdom (`grenadepulldeath`) | 0x40 | `player_die` drops `perk_grenadeDeath` (0x80b1bb2) | works | `martyrdom_drops_a_live_grenade` |
| Last Stand (`pistoldeath`) | 0x80 | `finishPlayerDamage` goes down (0x80acc2a); the script's pistol and bleed out | works | `last_stand_goes_down_then_bleeds_out` |
| Dead Silence (`quieter`) | 0x100 | client footsteps only | nothing server-side | client |
| Eavesdrop (`parabolic`) | 0x200 | `SV_QueueVoicePacket` to an enemy inside `perk_parabolicRadius` 400 and `..Angle` 180 (0x80a72aa) | works under `sv_voice 1` (cod4-voice.md 3) | `cod4_voice_eavesdrop_hears_the_enemy_in_its_radius` |
| Extreme Conditioning (`longersprint`) | 0x400 | sprint times `perk_sprintMultiplier` 2 (0x80526d9) | works (pmove) | `extreme_conditioning_doubles_the_sprint` |
| Bomb Squad (`detectexplosive`) | 0x800 | client icons only | nothing server-side | client |
| Sonic Boom (`explosivedamage`) | 0x1000 | script: blast 125 per cent | works | `sonic_boom_scales_a_blast` |
| (`exposeenemy`) | 0x2000 | no stock class gives it | nothing | none |
| Stopping Power (`bulletdamage`) | 0x4000 | script: bullet 140 per cent | works | `stopping_power_and_juggernaut_scale_a_round` |
| Bandolier (`extraammo`) | 0x8000 | script: `giveMaxAmmo` | works | `bandolier_fills_the_reserves` |
| Overkill (`twoprimaries`) | 0x10000 | script: a primary as the second weapon | works | `overkill_carries_two_primaries` |
| Juggernaut (`armorvest`) | 0x20000 | script: damage 75 per cent, cancels the two above | works | `stopping_power_and_juggernaut_scale_a_round`, `sonic_boom_scales_a_blast` |
| Frag x 3 (`fraggrenade`) | 0x40000 | script: three frags | works | `bandolier_fills_the_reserves` |
| Special Grenades x 3 (`specialgrenade`) | 0x80000 | script: three of the special grenade | works | `special_grenades_come_three` |
| C4, Claymore, RPG x 2 (`specialty_weapon_*`) | none | script: the weapon in the inventory slot, two rounds | works | `the_inventory_perks_give_two_of_their_weapon` |

The custom class tests build `custom1` off the stats `cac_getdata` reads
(201 the primary, 203 the secondary, 205..207 the perks, 208 the special
grenade, each a `mp/statsTable.csv` number). VERIFIED (the stock gsc).

### The spawn and the end frame

- `ClientSpawn` ends in its own think, before the script's loadout
  lands: the spawn frame's ground, `holdBreathScale` and `pm_flags` are
  that think's (cod4-pmove.md 15). The spot is `placeSpawnPoint`'s drop,
  a bit under retail's on mp_crash's terrain (same section), which
  `cod4::wire_ab` lists among its gaps.
- The walker's spawn frame (serverTime 47900, VERIFIED live): `commandTime`
  the frame's time, the spot (pinned off the capture: the script draws it
  from a clock-seeded `randomInt`), the view exactly the spot's 45, its
  `delta_angles` 110.00061, `eFlags` 2, `viewHeightCurrent` and
  `viewHeightTarget` 60, `dofNearBlur` 6, `dofFarBlur` 1.8,
  `throwBackGrenadeOwner` and `viewlocked_entNum` 1023, `speed` 190,
  `gravity` 800, `otherFlags` 4, `viewmodelIndex` 364, `stats` 100 0 100
  63 3. ours matches all of them.
- `ClientSpawn` (`cod4_lnxded` around 0x80a7c5d) keeps `eFlags` 0x100002
  (0x80a7c63) and flips 0x2 (0x80a7d68), clears the rest of the
  playerstate (the memset at 0x80a7cc9), steps the spawn count in
  `stats[4]` (0x80a7cfe; the walker's third spawn: the connect's
  spectator, the team join's, the class's), and writes the box
  (-15,-15,0)..(15,15,70) (rodata 0x820e024, stored at 0x80a7c24..0x80a7c5a),
  the view height 60 and 60.0, the blurs 6 and 1.8 and the two 1023s
  (0x80a7d2b..0x80a7dbc). VERIFIED (the instructions). The values it leaves
  are the spawn frame's above.
- A follower's cmds are not kept: the spawn's `delta_angles` 110.00061 is
  the spot's 45 less the -65 the walker sent before it followed its
  teammate. VERIFIED live. `cod4_probe`'s cmds while following are its held
  view less the copied `delta_angles` (-69.99), +70, which the spawn did not
  take. INFERRED (the probe's own code).
- `SpectatorClientEndFrame`: a client on a team with no class may not fly
  free (`allowSpectateTeam("freelook", false)`), so it follows the first
  playing teammate (`Cmd_FollowCycle_f`, INFERRED): the walker's
  playerstate is the mate's from the frame the mate spawns (46800) to its
  own spawn, `clientNum` 0, `otherFlags` 0xa, its own `eFlags` 0x100000
  kept. VERIFIED live. Before the mate spawns it floats with `otherFlags` 8.
  VERIFIED live. The copy is the mate's playerstate as the last frame left
  it, the newest archived frame. INFERRED.
- `CheckTeamStatus` writes `stats[3]` -1, 63 on the wire, for every client
  not following each frame: the spawn frame reads 63. VERIFIED live.
- A playing body's entity (`BG_PlayerStateToEntityState`): the origin
  rounded (the mate's z 236.125 reads 236), the view yaw, `pos.trType` 3,
  `apos.trType` 1, the weapon, and `solid` 0x66010f, `SV_LinkEntity`'s
  `x | (1 - mins.z) << 8 | (maxs.z + 32) << 16` over the body's box.
  VERIFIED live (one standing teammate); the packing INFERRED.

- `hidePart(tag)` sets the bit `0x80000000 >> (i % 32)` of `partBits[i /
  32]` for the tag's index `i` in the model's bone list: the
  `_destructible.gsc` cars' ten hidden parts read the walk capture's 0x242a,
  0x92480000 in their baselines and snapshots that way. VERIFIED live.
- A trigger's `eFlags` read 0x21 in the baselines (mp_crash's entities 88,
  344 .. 353). VERIFIED live. The bits are `InitTrigger`'s 1 and the nodraw
  0x20 of a trigger with no model. INFERRED.

- The destructible cars' baselines read the map's places as the writers
  round them: `58 2112.5 309.1` goes out as 2113 (the origin writer's
  `floor(f + 0.5f)`, cod4-netfields.md 2) and the yaw `101.765` as
  101.7664 (the angle writer's half short). VERIFIED (`cod4::wire_ab`).

### Open

Nothing left here. 1.7 sends no PVS cull to match (cod4-pmove.md 15);
pmove writes `holdBreathScale` and `groundEntityNum` (`vcod_cod4::pmove`)
and the anim script `legsAnim` (cod4-player-anims.md 7).

## 8. The stock gametypes on a running level

What `dom`, `sd`, `sab` and `koth` read off the engine past the spawn.
Gates: `cod4::gametype_tests` (`COD4_DIR`) and the bot soak in
`docs/bots.md`.

- A brush entity's box is its inline model's (`cmodel_t`'s `mins`,
  `maxs`, the clip map's `Submodel`), and `isTouching` and the touch
  pass test a box against that model's brushes. `SV_SetBrushModel`
  taking `CM_ModelBounds`: INFERRED. vcod's level carries both tables
  from the clip map (`game::cod4::brush_models`); without them no brush
  trigger touches anything, so `koth::SetupRadios` finds every radio
  outside its `radiotrigger` and `AbortLevel` loads `dm`.
  mp_crash's radios are `script_model`s whose point origins lie inside
  the `trigger_multiple` brushes `*24` .. `*27`. VERIFIED
  (`koth_on_mp_crash_finds_its_radios`, our clip map read off the zone).
- `objective_state` is 0x80c210a: an index above 15 is an error, and the
  state string is compared with four script-string constants, which store
  0, 2, 4 and 1. VERIFIED (the disassembly). The two first (0 and 2) also
  detach the record from its entity (`0x3ff`). INFERRED (the branch to
  0x80c21c7). The names, in the order of the error string at 0x821e6f4
  (`"empty", "invisible", "current", "active"`), give `active` 1. INFERRED
 (`ObjectiveStateIndexFromString`). `_gameobjects::
  updateCompassIcon` sets `active` on every flag, bomb and radio, so the
  1.1 reading (no `active`) raised a script error on each.
- `precacheLocationSelector` is 0x80bc4a6: it compares the name with
  configstrings 827 .. 829 (0x33b .. 0x33d), and a match goes to the
  print at 0x80bc5f2 with the string at 0x821d6d4 and returns. VERIFIED
  (the disassembly). That print (0x8122d74) writes only when `developer`
  (its dvar, named at 0x82232e9) is set. INFERRED (its early return on
  the dvar's value). A fourth name is the script error at 0x821d718.
  VERIFIED. A `fast_restart` keeps the configstrings, so every `sd` and
  `sab` round's `_hardpoints::init` meets its own selector there.
- `koth` has no start spawns: a player spawning in a level's first
  frames reads `numPlayersAtLastUpdate` and `distSum` off spawn points
  `_spawnlogic::spawnPerFrameUpdate` has not reached yet (it takes one a
  frame from `startGame`), so `getSpawnpoint_NearTeam:793` errors and
  leaves the weight undefined, and `spawnWeightDebug`, which runs while
  `scr_spawnpointdebug` is not `"0"` (only a `/#` block sets it), errors
  on that weight every 50 ms. INFERRED (the rawfiles). Retail does the
  same: `cod4_lnxded` 1.7, `koth` on mp_crash under an `fs_game` whose
  `_spawnlogic.gsc` (the stock one plus a `logPrint` at the top of
  `getSpawnpoint_NearTeam`) logs the unset spawn points and whose `mp_crash_fx.gsc`
  adds a test client at 200 ms that answers its menus, spawns it at 350 ms
  with 16 of the 19 `mp_tdm_spawn`s unset, and the comparison is a
  "script runtime error" ("pair 'undefined' and '0' has unmatching
  types", line 793 of the stock file) that ends the level and shuts the
  server down, at `developer 0` as at 1 (`logfile 2`). VERIFIED (the
  capture's console, rerun once). It ends it because it comes while the level is still loading,
  not because of what it is: see 8.1. A
  client loading the level takes longer than the 19 frames: retail's two
  probes joining 44 s past the load spawned with none unset. VERIFIED (the
  same run without the test client). vcod-server's bots enter a fresh
  level 3 s past its first frame (`bots::cod4::BOT_LOAD_MS`), so their
  first koth spawns run clean (`cod4::bots::tests::cod4_bots_spawn_clean_on_a_fresh_koth_level`).
  VERIFIED (the test, ours).

### 8.1 Which runtime errors end the level

`cod4_lnxded` 1.7. `Com_SetScriptSettings` (0x8122106) hands the VM
three flags (0x815cf70): `developer || logfile` at 0x8be4106,
`developer_script` at 0x8be4107 and `developer` at 0x8c055b5. VERIFIED
(the disassembly; the dvar names at 0x82232e9, 0x82232f3, 0x821c566).
`Scr_SetLoading` (0x815cf9a) stores its argument at 0x8c098f4; `G_InitGame`
sets it (0x80b59cc) and the server's frame loop clears it after each
frame it runs (0x8177366). VERIFIED (the disassembly). The error entry
at 0x815e906 (it copies the message into a 1024-byte buffer at
0x8c0b920, `Scr_SetErrorMessage`) marks the error terminal
(0x8c055b6) when the first flag is set and the VM is loading
(0x815e9b6). VERIFIED (the disassembly). `RuntimeError` (0x814f108)
returns without a word when neither the first flag nor a terminal
error is set; otherwise it prints `******* script runtime error *******`
(0x822b75c) and, when `developer` is set or the error is terminal,
`Com_Error`s with `script runtime error (see console for details)`
(0x822b788, 0x814f24d), which shuts the server down; else the thread
runs on. VERIFIED (the disassembly).

So a level ends on a runtime error only at `developer 1`, or with
`developer` or `logfile` set while the level loads: its spawn's frames
and the server's first frame after them. Retail 1.7, `war` on mp_crash
under an `fs_game` whose `mp_crash_fx.gsc` makes `if (undefined > 0)`
at a set level time (`run_err.sh`, scratch, server
alone, 15 s):

| when | developer | logfile | the level |
|---|---|---|---|
| 0, 200, ..., each frame | 0 | 2 | shut down at the first (time 0) |
| each frame from 0 | 0 | 0 | ran on, no error printed, the `if` taken |
| first frame at or past 350 (350) | 0 | 2 | shut down |
| first frame at or past 400 (400) | 0 | 2 | ran on |
| 2200 | 0 | 2 | ran on, error printed, the `if` taken |
| 2200 | 1 | 2 | shut down |

VERIFIED (the consoles and `games_mp.log`). The level's times run 0,
200, 300, 350, 400: the spawn's frames, then the loop's first, which a
2.6 s hitch warning precedes, still loading. INFERRED (the times and
0x8177366's place after the frame). The koth error above fell at 350.
VERIFIED (its `PROBE 350` line).

vcod-server never ends a CoD4 level on a runtime error: it logs it
(rate-limited) and runs on, which is retail at `developer 0` past the
load. Ending the level for an error in the load's frames under
`logfile` is not done. INFERRED (the code).

## 9. The use key, the touch pass and a linked player

What the stock game objects (`_gameobjects.gsc`) need of the engine to be
planted, defused, picked up and captured. vcod's port is
`game::cod4::use_key`; the gates are the bot objective tests
(`cod4::bots::tests::cod4_bots_*`, bot-objectives.md 12).

- `G_RunFrame` opens with `G_TouchTriggers` for every client in use and
  not in noclip, before the frame's threads; the pass returns at once
  for a `pm_type` past 1 (linked). INFERRED (the call order). Before this
  vcod's CoD4 level ran no touch pass, so no `trigger_radius` (the `sd`
  and `sab` bomb pickups, `dom`'s flags) or `trigger_multiple` (`koth`'s
  radios) ever sent `"trigger"`, and no carry or proximity object was
  ever taken. INFERRED (no caller of `ScriptRuntime::touch_triggers` in
  `game::cod4` at e6c7bf06).
- `Player_UpdateActivate` runs last in `ClientThink_real` for a player in
  the world: a press of 0x28 (`latched_buttons`) clears the held target
  and, off a mantle, a sprint and weapon states 15..20, takes the cursor
  hint's entity with `useHoldTime`; with 0x28 held and `g_useholdtime`
  run since, `Player_UseEntity` notifies `"trigger"` with the player and
  clears it. `Player_GetUseList` scores a `trigger_use_touch` whose box
  meets the player's at -256, ahead of everything in reach, with no sight
  trace. INFERRED (control flow).
  vcod takes the hint at the press off the use list of 9.1, a trigger
  open to the player's team (`setTeamForTrigger`) and not claimed by
  another client (`clientClaimTrigger`). INFERRED.
- The bomb zones of `sd` and `sab` and `sd`'s `bombDefuseTrig` are
  `trigger_use_touch` (`createUseObject` reads the class name's `use` for
  `triggerType "use"`). VERIFIED (`_gameobjects.gsc` `createUseObject`,
  `sd.gsc` `bombs`, `sab.gsc` `createBombZone`).
- `useHoldThink` `linkTo`s the player to the trigger, gives and switches
  to the `useWeapon` (`briefcase_bomb_mp`, `briefcase_bomb_defuse_mp`),
  and counts the use time only while that weapon is in hand, waiting at
  most 1.5 s for it; the hold ends on a release, a step off the trigger,
  a melee press or a death. VERIFIED (`_gameobjects.gsc` `useHoldThink`,
  `useHoldThinkLoop`).
- `G_EntLinkTo` on a client sets `pm_type` 1 (8 dead) and `G_EntUnlink`
  0 (7); `ClientEndFrame` writes 1 or 0 by `tagInfo`, 8 or 7 for the
  dead. INFERRED.
  vcod's CoD4 end frame wrote 0 every frame, so a link never showed.
- `PmoveSingle`'s linked arm ends in `PM_Weapon`, so the briefcase comes
  up under the link. INFERRED.
  `vcod_cod4::pmove` left it out, and the planter's weapon never changed.
- A linked player is a body for the others' pmove, is shot and goes out
  in the snapshot as an unlinked one does: vcod's CoD4 level had read
  those three off `pm_type` 0 alone. INFERRED (`pm_type` 1 is
  `PM_NORMAL_LINKED`, a living player).

### 9.1 The use list

`Player_GetUseList` is inlined in `Player_UpdateCursorHints` at
`cod4_lnxded` 0x80d2c30 (called from 0x80a6cea), with `compare_use` at
0x80d2c10 passed to `qsort` four times (0x80d3271, 0x80d33ad, 0x80d3700,
0x80d3720). vcod's port is `game::cod4::use_key` (`use_list`,
`use_hint`, `use_entity`).

- The constants the function reads: 0.76 (0x822094c, the
  `requireLookAt` dot), 512 (0x8211e70) off a missile, 128 (0x820e278)
  off a turret, 256 (0x8214ac8) off a `trigger_use`, 10000 (0x8220950)
  onto an item that cannot be grabbed or an entry the sight trace does
  not reach. VERIFIED (the rodata words, each read by an `fsubs` or
  `fadds` at 0x80d3655..0x80d36c4).
- The scores follow `Player_GetUseList`: a `trigger_use_touch` the player's box meets at -256; anything
  else within its reach of the eye (128, a missile
  `player_throwbackOuterRadius`, a turret `player_MGUseRadius`) at
  `(1 - (dot + 1) / 2) * 256` plus the distance to its bounds' middle,
  the offsets above added; sorted, the ungrabbable items cut, each entry
  but a touch trigger and a vehicle traced to from the eye
  (`G_TraceCapsuleComplete`, mask 17) and a blocked one cut, sorted
  again. INFERRED (control flow; the constants VERIFIED
  above).
- The item arm of the hint walk tests the player's `weapons` bit for the
  item's weapon (`ps+0x55c`, 0x80d3523) and the two weapons'
  `inventoryType` (+0x13c) against 0 and 3 (0x80d353c..0x80d355c): the
  item hints `weapon + 4` unless the player holds it, or the weapon in
  hand is neither primary nor alt mode while the item is one of the two
  and two primaries are held. VERIFIED (the instructions);
  that the call there counts the held primaries is INFERRED.
- An item taken with the use key is notified `trigger` with the player
  and the weapon dropped in its place: `_weapons.gsc`'s `watchPickup`
  waits for `"trigger", player, droppedItem` and reads a missing
  `droppedItem` as an ammo pickup. VERIFIED (the stock script). vcod's
  swap (`swap_weapon`): with two primaries held and the item a primary,
  the weapon in hand goes into the world with its clip and reserve as
  `dropItem` launches one, the item's weapon is given with the item's
  clip and reserve, switched to (`a <weapon>`), and the item freed the
  next entity pass. INFERRED (vcod's CoD2 swap rule; 1.7's `Touch_Item` is not read).
- Missiles and turrets are on the list (9.2, 9.3); a `trigger_use`'s
  `requireLookAt` is left out (`useTriggerRequireLookAt` has no
  handler). A dropped
  weapon's middle is its rest point, not its bounds' middle. INFERRED.
- What the stock maps reach: six maps place a
  `misc_turret`, each a `weapon_saw_MG_setup` on `saw_bipod_stand_mp` or
  `saw_bipod_crouch_mp`: mp_backlot two, mp_carentan three, mp_convoy,
  mp_farm and mp_overgrown two each, mp_pipeline one. VERIFIED (the
  entity strings of every stock `mp_*.ff`). No stock script calls
  `useTriggerRequireLookAt` (`common_mp.ff` and every map zone's
  rawfiles). VERIFIED (the rawfiles).
- The cursor hint is set in `ClientEndFrame` (`Player_UpdateCursorHints`) and a use press takes the entity the last end frame left
  under it, so a press in the cmd right after a teleport finds no hint.
  vcod's port: `use_key::update_cursor_hints` in the playing end frame,
  `update_activate` on `cursorHintEntIndex`. INFERRED; the retail SAW
  capture's hint (12, 0, 393) a frame before the mount is VERIFIED live.
- `cod4::bots::tests::cod4_use_key_swaps_a_held_gun_for_a_dropped_one`
  runs it on a live mp_crash level: a bot's gun dropped through the stock
  `dropWeaponForDeath`, taken by another player who holds use over it.

### 9.2 Mounted SAWs

Captured on retail with `cod4_probe --script saw` behind mp_backlot's
crouch SAW 393 (`devmap mp_backlot`, war, `setviewpos -770 68 320 0`,
`--face 0`): `play-1.7-saw.txt`, an uncommitted capture. The live gate
is the client's
`cod4_live_saw_mounts_fires_clamps_and_lets_go`; vcod's port is
`game::cod4::turret` over the shared `TurretRecord`. Names and order
are INFERRED where no label says otherwise.

- The gun goes out as `eType` 11 (`ET_MG42`, the general table), `index`
  its model (22), `weapon` its turret weapon's number in the level's list
  (8, `saw_bipod_crouch_mp`, which `G_SpawnTurret` registers in spawn
  order), `apos.trType` 3, and its barrel in `lerp.u.turret.gunAngles`
  (`u.anonymous.data[0..3]`): unmanned it rests at the weapon's drop
  pitch, -51 on this gun. VERIFIED live.
- Registering the turret weapon takes the hint-string slots for its
  `useHintString` and `dropHintString` (`PLATFORM_HOLD_TO_USE` at 277,
  `PLATFORM_HOLD_TO_DROP` at 278). VERIFIED live (the gamestate).
- Behind the gun and inside its yaw arcs the hint is the weapon plus 4
  (12), the use string's slot (0) and the gun (393). A use press mans it
  at once: `eFlags` 0x200 (the crouch gun's stance), `viewlocked` 1 on
  393, the hint the drop string (12, 1) with no entity (1023), and the
  view clamp centred on the arcs: yaw base 360 range 45, pitch base
  7.4999995 (0x40efffff) range 22.5 for arcs 45 either side and -15..30.
  The next frame stands the body crouched (`pm_flags` 0x2, the eye
  lerping to 40). VERIFIED live. The base's 360 is `AngleNormalize360`
  taking the turns' fraction once; vcod's `normalize360_once` keeps it.
  INFERRED.
- The first manned frame flips the gun's teleport bit (`eFlags` 2) and
  its yaw follows the view clamped to the arcs (45 when looked 70 past);
  held fire rings `EV_FIRE_WEAPON_MG42` (38) on the gun every frame
  (`fireTime` 50), its `eFlags` 0x40 on, the gunner's `eFlags` 0x40 and
  `viewlocked` 2. VERIFIED live. The round leaves from the gun's
  `tag_player` along the view as far as `tag_flash` is from it, with the
  file's `playerSpread`, at `level.time`, no antilag. INFERRED.
- A second press lets go: `EV_STANCE_FORCE_STAND` (the stance mounted
  from), the mount spot a unit up, view yaw 0, `eFlags` 0x206 to 0x4,
  `viewlocked` 0 and 1023, the next frame's hint the gun again. The gun
  then walks its pitch home 10 a frame to -51 and its yaw to 0. VERIFIED
  live.
- `G_ClientStopUsingTurret` is 0x80b7866: it puts the gunner a unit over
  the spot he mounted from (0x80b778a, the `fld1; fadds` at 0x80b77cd),
  flips `eFlags` 2, clears 0x300 (0x80b791d), `viewlocked` and its entity
  (0x80b792d, 0x80b793d) and both `active` bytes (0x80b7947, 0x80b7955).
  VERIFIED (the instructions). Its callers are `turret_think_client`
  (0x80b7a2a), `ClientSpawn` (0x80a7ecf) and the turret's free
  (0x80b8bce). VERIFIED (the calls).
- `ClientEndFrame` runs `turret_think_client` (0x80a715f) for any session
  that reaches its tail, dead or playing, when `otherFlags` 4 and `eFlags`
  0x300 are set (0x80a6f5a, 0x80a6f60); the dead session sets `pm_type`
  7 or 8 first (0x80a6f96, 0x80a710f). The think lets go when the gunner's
  `active` byte is not 1 (0x80b7a1e), his `sessionState` (gclient+0x2f64)
  is not 0 (0x80b7a40) or his `pm_type` is 6 (0x80b7a4a). VERIFIED (the
  instructions). `player_die` (0x80b1aa0..0x80b1dca) calls none of it. So
  a gunner killed on the gun is let go on his death frame, his body set
  over the mount spot, once the killed callback has made his session
  dead. INFERRED (the control flow). `ClientSpawn` tests the same two
  flags first (0x80a7b8c, 0x80a7b92) and lets go before it places the
  body at the spawn point, so a respawn never lands on the mount spot.
  INFERRED (the control flow). vcod ran the think for playing sessions
  only and let a dead gunner go at his respawn, after the placement,
  which put him back on the spot; `turret_tests` pins both releases.
- The barrel has no turn rate: each frame's yaw is the view's yaw of
  the frame's last cmd clamped to the arcs, on the 65536ths grid (view
  -1.033 gives -1.038, -3.378 gives -3.384; a positive view already on
  it is kept). The per-frame step is the client's: the probe turns 70
  degrees in 1.5 s of its own wall clock, so a frame's step is 2.34 or a
  missed frame's double on both servers, and where the turn starts
  against the frames is the probe's timing. VERIFIED (both captures).
- `G_PlayerTurretPositionAndBlend` places the gunner every manned frame,
  after the aim and before the fire: the legs' `turretanim` group
  (`crouchSAWgunner_aim`, legs 0x35) is walked from the file's first row
  (`_15down`) on, each row's two columns at `n / 2 + yaw /
  animHorRotateInc` over the engine's order (the file's reversed), the
  first row whose root height at time 0 reaches the gunner's height over
  `tag_weapon` blended with the row before it; the blend's root offset,
  turned by the barrel's yaw, hangs off `tag_weapon` at the gunner's
  height, and a point trace (mask 0x810011) from the eye's height to 60
  under it sets z. The barrel's yaw is the gun's pose from the frame
  before (the body trails the barrel a frame: a turn's first frame
  leaves the origin, the frame after a stop still moves it). Retail's
  gunner stands at -754.049 75.372 258.125 the first frame (the mount's
  z 258.194 picks the rows) and -754.039 75.398 after; at yaw -45
  -738.634 99.883, at 45 -752.460 51.005. VERIFIED live. The walk's
  order and column rule are INFERRED, and differ from CoD1's and CoD2's
  (`turretpose::GunnerRule`); vcod's run of the recipe puts the gunner
  on retail's origin to the printed thousandth at every barrel yaw both
  captures share (0, +-45, -42.012, 40.518, 40.523). VERIFIED (the two
  captures). The trace meets the world only, not entities.
- 1.7's `BG_PlayerStateToEntityState` copies `viewlocked_entNum`
  (ps+0x59c) into the body's `otherEntityNum` (es+0x74) when `eFlags &
  0x300` (`cod4_lnxded` 0x8052ab9). VERIFIED (the instructions). The
  vcod client places a watched gunner off it as the server does
  (`play::cod4_players::GunInfo`, the gun's `animHorRotateInc` at
  `WeaponDef` +2004), keeping the wire's height. The first-person eye
  on a gun is `tag_player` (`cod4_client::turret_eye`).

### 9.3 The live grenade's throw back

Captured on retail with two probes on `devmap mp_crash` war: a
`--script frag` thrower at 123 2110 300 and a `--script throwback` target
at 207 2291 296 (an uncommitted capture). The live gate is
the client's `cod4_live_frag_at_the_feet_is_thrown_back`.

- With a live frag at rest near it the target's hint reads 44 (the
  frag's weapon 40 plus 4), string -1, the grenade's entity, and
  `throwBackGrenadeTimeLeft` its fuse's time left (2550). VERIFIED live.
- The frag key's `EV_SWITCH_OFFHAND` with that hint takes it in the same
  frame: `EV_RESET_ADS`, `EV_AMMO_PICKUP` (parm 40), the grenade gone
  from the snapshot, `throwBackGrenadeOwner` the thrower (1), the hint
  cleared, `grenadeTimeLeft` the time left; the hold counts it down and
  the throw (`EV_USE_OFFHAND`) puts a new grenade in the world;
  `PM_Weapon_OffHandEnd` clears the owner (1023) and the time left.
  VERIFIED live. That the throw spends no frag is `PM_Weapon_OffHand`'s
  throw back arm, INFERRED (vcod's live gate checks its own). The client event handler's
  The live grenade pickup is inlined at `cod4_lnxded`
  0x80a59c6..0x80a5aa5: the hint entity not 0, the event's weapon and
  the missile's (`index % 128`) of `offhandClass` 1 (def+0x144), `eType`
  4, the time left set, then the owner from `parent` (1022 for none) and
  the grenade's touch. VERIFIED (the instructions).
- The list's reaches are dvars registered at 0x80b5343, 0x80b537c and
  0x80b53b5: `player_throwbackInnerRadius` 90, `..OuterRadius` 160,
  `player_MGUseRadius` 128. VERIFIED (the immediates). A missile moving
  faster than `bg_maxGrenadeIndicatorSpeed` is not listed,
  INFERRED.
- A throw back held past its fuse kills its holder as the thrower's
  `MOD_GRENADE` (3); with no owner it is `MOD_SUICIDE` (12): the compare
  of `throwBackGrenadeOwner` with 1023 at 0x80a593c picks one of the two
  `player_die` calls at 0x80a5996 and 0x80a5b93. VERIFIED (the
  instructions).
- vcod's run of the same recipe matches the hint (time left 2500, its
  frame later), the events and their parms, the owner and the throw.
  VERIFIED (the two captures).

### 9.4 `linkTo` on a tag of a script model

`_utility.gsc`'s `play_sound_on_tag`, which `_destructible.gsc` runs for
every part a sedan loses, spawns a `script_origin` at `getTagOrigin(tag)`
and `linkTo`s it to the car on that tag, with zero offsets.

- The linkTo builtin (`cod4_lnxded` 0x80c3160..0x80c32a7) looks the tag up
  through 0x817c2f8 and names `failed to link entity since tag '%s' does
  not exist in parent model '%s'` (0x821e8ac, pushed at 0x80c3293) only
  when it returns a negative index; `getTagOrigin` words its miss
  `tag '%s' does not exist in model '%s' (or any attached submodels)`
  (0x821e148, pushed at 0x80c008f). VERIFIED (the instructions). Both
  search the entity's DObj, its model and every attached one. INFERRED
  (the second string).
- mp_backlot's `vehicle_80s_sedan1_{silv,yel,red,brn}_destructible_mp`
  carry every tag the parts name (`tag_glass_back_d`,
  `tag_glass_{left,right}_{front,back}_d`, `tag_light_{left,right}_{front,back}`
  and their `_d`) in their own 48 bones; the doors
  (`vehicle_80s_sedan1_silv_door_lf` and kin) carry their window's
  `tag_glass_left_front`. VERIFIED (the zone's xmodels, `vcod-cod4`'s
  `bone_dump`). So retail finds every tag the script asks for.
  INFERRED (not shot on retail).
- vcod reads a CoD4 parent's tag off the zones' bind-pose bone, the
  body's first, then each attached model's placed on the body tag it
  hangs on (`link::cod4_model_tag`). VERIFIED (`cod4::destructible_tests`,
  which breaks mp_backlot's sedans with the gsc `radiusDamage`). Trap:
  the pk3 rigs hold no CoD4 model, so a tag looked up there fails every
  tagged `linkTo` on a CoD4 script model (each sedan part broken in `sab`
  on mp_backlot and mp_strike logged it).
- vcod's CoD4 rounds and blasts also damage the entities a script opened
  to damage (`setCanDamage`, `GameHost::script_takedamage`), so a
  player's fire breaks the cars and barrels as a script's `radiusDamage`
  does (`game::cod4::entity_damage`, cod4-combat.md 13).

## 10. Arrays are values

`b = a; b[k] = v;` leaves `a` as it was, and so does a write through `a`
after the copy: two places never share an array. The same holds for an
array passed as an argument, read out of a field or out of another
array's element, and for an array an array holds (the copy is deep),
while a struct inside one is shared. VERIFIED live on `cod4_lnxded` 1.7
(`war` on mp_crash under a mod whose `war.gsc` runs
`crates/gsc/tests/fixtures/semantics/probe_array_copy.gsc`'s body plus a
struct case, `logPrint` lines identical to the 1.1d capture in
`retail-captures.txt`, `struct_shared 2`) and on 1.1d (that capture).
The mechanism is a reference count with a copy at the write:
`Scr_EvalArrayRef` and `Scr_EvalArrayRefInternal` replace an array whose
`refCount` is non-zero with a fresh one filled by `CopyArray`, which
recurses into nested arrays and adds a reference to anything else.
INFERRED.

The stock scripts rely on it. `_teams::CountPlayers` (`_teams.gsc` 581)
copies `level.players` into a local and writes its `"allies"` and
`"axis"` counts there; `_globallogic::menuAutoAssign` (2052) calls it on
every team-menu auto-assign. VERIFIED (the stock scripts). On retail an
auto-assigned client leaves `level.players.size` at 1 and the timeout
takes it to 0 (a `PROBE players` line every 2 s from the same mod, one
`cod4_probe --answers "1 team autoassign|..."` that goes silent, VERIFIED
live). vcod's VM shared the array, so `level.players` grew two string
keys, its `size` counted them, and `_spawnlogic::spawnPerFrameUpdate`
(1555, `if ( player.sessionstate != "playing" )`) read an undefined
player every frame from then on: the `field access needs a struct or
entity`, `cannot compare undefined against a value` and `cannot cast
undefined to bool` flood. VERIFIED (the regression tests below fail with
exactly those three without the fix).

vcod copies at the write as retail does, with a flag for the count
(`Heap::share`, `Heap::own`): an array stored into a variable, a field, an
element, an argument or a `waittill`'s bound locals is flagged, and the
`EnsureArray*` and `OwnLocal` ops that resolve an index write's target
copy a flagged array and store the copy back. The flag is never cleared,
which costs a copy retail would skip and no script can tell apart. A
write through `game` itself (`game["k"] = v` after `g = game`) still
reaches `g`. INFERRED (unmeasured on retail). Gates:
`semantics_ab`'s `probe_array_copy`,
`cod4::disconnect_tests::an_autoassigned_player_beside_bots_times_out_without_script_errors`
and its `disconnects_` twin (`COD4_DIR`).

## 11. A freed player in a script array

`_battlechatter_mp::doSound` adds its speaker to `level.speakers[team]`
and removes it after `waittill_any( soundAlias, "death", "disconnect" )`,
whose `endon`s on the other two names end the whole thread, `doSound`
with it, so a player who dies or leaves inside his line's 2 s stays in
the list for the rest of the level; `isSpeakerInRange` (207) then reads
`teammate.origin` off him. VERIFIED (the stock scripts). Once he leaves,
his entity is freed and the reference reads as a removed entity.

Retail 1.7 under an `fs_game` whose `mp_crash_fx.gsc` keeps a test
client in a local array past its kick and runs `isSpeakerInRange`'s loop
on it: `isDefined` reads 0, and at `developer 1` the comparison is a
"script runtime error" (`pair 'removed entity' and 'undefined' has
unmatching types`) that shuts the server down; at `developer 0` the
thread runs on, the loop's `if` not taken past the errors, so the
function returns its first `return` (here "self", `isSpeakerInRange`'s
`true`), and the level keeps running. VERIFIED live
(`run_dead.sh`, scratch; `cod4_lnxded` 1.7, `war`,
mp_crash). So on retail the freed speaker silences the team's battle
chatter quietly. vcod-server does the same and logs each error
(`dead entity is not an object`, `distanceSquared takes two vectors`,
`< > <= >= need numbers`, `cannot cast undefined to bool`), rate-limited.
VERIFIED (a server log). The same `developer 0` run ran on past
a pair-type error as well, unlike section 8's `koth` comparison, which
ended its level: that one fell inside the load, this one 7 s past it
(8.1). VERIFIED (both consoles, the table in 8.1).

## 12. A name change

- The client: `CL_CheckUserinfo` (`iw3mp.exe` 0x46c680) runs each frame
  from `cls.state` 4 (`CA_CHALLENGING`) on; when a userinfo dvar changed
  (`dvar_modifiedFlags & 2`, 0xcba73f4) it gathers the userinfo dvars
  into an info string and queues `userinfo "%s"` (0x6d0e28) as a reliable
  command. VERIFIED. vcod's `Conn::set_name` sends the connect's info
  with the new `name` and without its `protocol`, `challenge` and
  `qport`, which are no dvars (INFERRED: the gather is the dvars').
- The server: the `userinfo` row of the client command table (0x8225380)
  is `SV_UpdateUserinfo_f` (0x8170536): `I_strncpyz` of the argument
  into the client's userinfo (+0x650, 0x400 bytes), `SV_UserinfoChanged`
  (0x8170320), then a tail call of the game's `ClientUserinfoChanged`
  (0x80a8068). VERIFIED.
- `ClientUserinfoChanged` reads `cg_predictItems`, then, unless the
  client's `sess.connected` (+0x2f8c) is 2 and the level's name mode
  (0x836f8d8) is set, cleans the userinfo's `name` into the client
  state's name (+0x3048: leading spaces dropped, at most 15 characters,
  `UnnamedPlayer` when empty) and copies it into `newnetname` (+0x2fd8);
  with both set it cleans the name into `newnetname` alone. VERIFIED
  (0x80a811e..0x80a8213, 0x80a829f..). That `updateClientNames` copies
  `newnetname` over the name is INFERRED.
- `setClientNameMode`: the stock `war`, `dm`, `dom`, `koth` and `sab`
  pass `auto_change`, `sd` `manual_change`, and no stock script in
  `common_mp.ff` calls `updateClientNames`. VERIFIED (the zone's
  scripts). So on `sd` a rename shows from the next map on.
- vcod: `Cod4Level::userinfo_changed` writes the name the scripts, the
  chat and the snapshots' client states read (`GameHost::client_names`),
  under `auto_change`, or under `manual_change` for a client not yet in
  the game; it does not clean the name. The engine's copy (`status`,
  the log) follows the userinfo at once, as retail's. The client sends
  its userinfo on every change, so a `name` takes effect in the game,
  not only at the next connect.
