# CoD4 1.7 gsc: dialect, builtin tables, callbacks, the boot's configstrings

What vcod's compiler, VM and host need to run CoD4 1.7's stock MP scripts.

- Code: `crates/gsc` (`Dialect::Cod4`, `VmProfile::COD4`);
  `crates/server/src/game/cod4/` (`Cod4Zones`, `Cod4Scripts`,
  `Cod4HostState`, `load`, `level.rs` `Cod4Level`, `weapons.rs`,
  `wire.rs`); `game/builtins/cod4.rs`, generated `cod4_names.rs`,
  `cod4_client.rs` (section 7's player methods); `game/profile.rs` (`COD4`).
- Gates: `game::cod4::boot::cod4_boot`,
  `cod4_stock_scripts_call_only_cod4_builtins` (both `COD4_DIR`),
  `builtins::set::tests::cod4_lists_its_own_tables`,
  `cod4::wire_ab::a_join_on_war_matches_retail` (section 7, `COD4_DIR`).

## Sources

- `cod4_lnxded` 1.7 (md5 `bf8448740b3f4cb74c2254f01af14cdd`, `CoD4 MP 1.7
  build linux-i386 Jun 28 2008`): stripped, game linked in, ET_EXEC, real
  pointers in `.data`. Addresses are its own.
- Stock scripts: rawfiles of `zone/english/common_mp.ff` (124 `.gsc`) and
  `mp_crash.ff` (6), walked by `vcod_cod4::walk`. VERIFIED
  (`cod4_stock_scripts_call_only_cod4_builtins` counts 130).
- Retail gamestate: `crates/cod4/tests/fixtures/net/handshake-1.7.txt`
  (`cod4-handshake.md` 5).
- An INFERRED claim naming no 1.7 address is unconfirmed on 1.7.

## 1. The dialect

`Dialect::Cod4` is `Dialect::Cod2` plus what the corpus needs. Under CoD2's
dialect, `crates/gsc/examples/gsc_census.rs` (`DIALECT=cod4
BUILTINS=<names>`) fails one of the 130 files and leaves three names in no
builtin table. VERIFIED (census run). Additions:

- `<<`, `>>`: integer only, lexed only under `Dialect::Cod4`. Corpus use:
  `_rank.gsc` 667 `(addonMask<<16) | (1<<16)`. VERIFIED (the parse
  failure). They bind between the relational operators and `+`/`-`, as in
  C. INFERRED from the opcode order (`OP_shift_left` between
  `OP_greater_equal` and `OP_plus`).
- `waittillmatch(event, value...)` (five calls: `common_scripts/utility.gsc`
  26, `_helicopter.gsc` 538, 543, 795, 879) and `prof_begin`/`prof_end`
  are in none of section 2's tables and not in the binary's strings.
  VERIFIED. So all three are compiler statements. INFERRED.
  `waittillmatch` wakes only on a notify whose first arguments equal the
  values in order (`cod4_waittillmatch_wakes_on_matching_arguments_only`).
  INFERRED from `OP_waittillmatch`.
- `prof_begin(name)` / `prof_end(name)` (13 `prof_begin` each in
  `_globallogic.gsc`, `_spawnlogic.gsc`) compile to nothing. INFERRED:
  profiler only.
- `*`, `/` take vector by number either way and vector by vector per
  component (`Op::MulVector`, `Op::DivVector`); CoD2's `*` refuses a
  vector. Uses: `_helicopter.gsc` 393 `144*heli_forward_norm`;
  `_hardpoints.gsc` 279, 287 (`level.artilleryDangerForwardPush * ... *
  forward`, `forwardPart / level.artilleryDangerOvalScale`), whose failure
  killed `getSingleAirstrikeDanger`. VERIFIED (corpus and error). The
  number's cast up to a vector and a zero component's `divide by 0` follow
  `Scr_CastWeakerPair`, `Scr_EvalMultiply`, `Scr_EvalDivide`. INFERRED.
  Two ints still divide as floats.

`VmProfile::COD4` is `COD2` with the dialect swapped: newest waiter first,
kill mid-step, thread returns. INFERRED, unmeasured on 1.7.

## 2. The builtin tables

`tools/re/cod4_builtins.py <cod4_lnxded> [--rows|--rust]` finds every run
of four or more 12-byte rows `{char *name; void (*fn)(); int developer}`
(identifier, `fn` in `.text`, developer 0/1). Seven runs; each starts on a
row one `.text` instruction points at, ends on a null row or text.
VERIFIED.

| Table | Address | Rows | developer 1 | Walked at |
|---|---|---|---|---|
| functions | 0x826d2c0 | 205 | 16 | 0x80bd26a |
| player methods | 0x8218de0 | 83 | 0 | 0x80a867f |
| script entity methods | 0x8216c40 | 18 | 3 | 0x8099f90 |
| vehicle methods | 0x821be40 | 25 | 0 | 0x80ba85d |
| entity methods | 0x821f580 | 82 | 0 | 0x80bff7e |
| hudelem methods | 0x8215900 | 22 | 0 | 0x808d99b |

VERIFIED (the script's summary). Receivers come from each table's first
name (`giveweapon`, `moveto`, `freehelicopter`, `attach`, `settext`).
INFERRED. The seventh run (`userinfo` .. `unmuteplayer`, 0x826e380, 12
rows) is the client command table. INFERRED from its names.

- 204 distinct function names (`weaponfiretime` twice); 230 method names.
  VERIFIED (`cod4_lists_its_own_tables`). Developer rows:
  `createprintchannel`, `setprintchannel`, `print`, `println`, `print3d`,
  `line`, `assert`, `assertex`, `assertmsg`, `getentbynum`, the six file
  builtins, the three `devadd*` script entity methods. VERIFIED.
- `game/builtins/cod4_names.rs` is the committed `--rust` output;
  `BuiltinSet::cod4()` compiles against it, so an unknown call fails the
  load. All 130 scripts compile; 1.1's set refuses `tablelookup`. VERIFIED
  (`cod4_stock_scripts_call_only_cod4_builtins`).

## 3. Handlers

Shared handlers bind by name; `builtins/cod4*.rs` add 1.7's. The `war` boot
on mp_crash plus 30 s of an empty server reach no unhandled name and kill
no thread. VERIFIED (`cod4_boot`). Section 3.1 covers every name the stock
scripts call.

- `getdvar`, `getdvarint`, `getdvarfloat`, `setdvar`: the cvar handlers.
  INFERRED. `makeDvarServerInfo(name[, default])`: default optional
  (`_globallogic.gsc` 107). VERIFIED (corpus).
- `tableLookup(table, searchColumn, value, resultColumn)`,
  `tableLookupIString`: first row of `code_post_gfx_mp.ff`'s 30 string
  tables (`mp/statstable.csv` and kin) whose search column equals the
  value, case-blind, else `""`. Location VERIFIED (the walk); rule
  INFERRED.
- `getStat` / `setStat`: a fresh profile's block
  (`vcod_cod4::net::stats::DEFAULT_STATS`) until the client's seven
  `stats` packets are in, then the client's (section 7).
- `visionSetNaked(name, time)` / `visionSetNight` write 824 / 825 through
  `"%s" %i` (0x821c3a8): capture `"mp_crash" 2000` for
  `visionSetNaked(getDvar("mapname"), 2.0)`. VERIFIED. No time is 1 s:
  `_load.gsc` 59's `VisionSetNight("default_night")` reads
  `"default_night" 1000`. VERIFIED.
- `setExpFog(startDist, halfwayDist, r, g, b, time)` writes slot 9 through
  `%g %g %g %g %g %.0f` (0x821c072): mp_crash's `setExpFog(500, 3500,
  ...)` reads `500 0.000198042 ...`. VERIFIED. The second value is `ln 2 /
  halfwayDist`. INFERRED from that one sample. Distance messages:
  0x821da24, 0x821da58. VERIFIED.
- `setMapCenter` writes 12 (`%f %f %f`); `setMiniMap(material, x1, y1, x2,
  y2)` 823 (`"%s" %f %f %f %f`, 0x821c3e6); `precacheLocationSelector`
  the next of 827..829 (repeat: developer print; fourth: error, section
  8); `getNorthYaw` reads 822; `setGameEndTime(ms)` writes 11. VERIFIED
  against the capture (`429.000000 126.500000 137.000000`, the minimap
  line, `map_artillery_selector`, `619800`).
- `getArrayKeys` returns newest first: `_globallogic.gsc` 3802 precaches
  `game["strings"]` by its keys, and localized strings 348..381 run in
  reverse of first assignment (`MP_OPFOR_NAME` ..
  `PLATFORM_PRESS_TO_SPAWN`). VERIFIED (capture). The heap keeps each
  array's insertion order (`Heap::array_keys_newest_first`).
- `precacheItem` and the spawn pass's `weapon_<name>` pickups register a
  weapon (`register_weapon`): appended to 2258's list, its 16
  `worldModel`s then `projectileModel` indexed as models, `altWeapon`
  registered next, 2314 rewritten. The capture's list: `defaultweapon_mp`
  (load), mp_crash's eight pickups in lump order, then the precaches
  (`g36c_gl_mp gl_g36c_mp` pairs); models open with `defaultweapon` at
  831. VERIFIED (capture). Model order follows `G_RegisterWeapon`.
  INFERRED. 2314: four bits per lowercase hex digit, weapon 0 and each
  registered weapon set; 113 weapons read
  `ffffffffffffffffffffffffffff3000`. VERIFIED (capture).
- INFERRED: `hidePart` keeps `partBits` (`Cod4HostState::part_bits`) and a
  hidden bone's collision stops clipping rounds
  (`entity_damage::ModelPart`); `setCanDamage` is UO's `setTakeDamage`;
  `isSplitScreen` is 0; `abs`, `min`, `max`, `floor`, `ceil` return floats.

### 3.1 The census

`game::cod4::census::cod4_builtin_census` compiles every base-zone `.gsc`
with each of the 21 stock `mp_*` zones (up to 131 scripts) and fails on any
called builtin without a handler; `LEFT_OUT` is empty. VERIFIED (the test).
`builtins/cod4_script.rs` answers the rest:

- `push ebp; mov ebp,esp; pop ebp; ret` on PC: `sendRanks` (0x80bb058),
  `endParty` (0x80bb05e), `endLobby` (0x80bb088), `fprintln` (0x80bb022),
  the three `devAdd*` (0x8099f60, 0x8099f66, 0x8099f6c). VERIFIED (the
  bytes). A ranked match (`sv_pure 1`, `level.rankedMatch`) runs
  `setXenonRanks` from `endGame`, which dies before the intermission
  unless `sendRanks()` returns.
  `cod4_ranked_match_ends_with_the_scoreboard_and_rotates` plays one
  minute through the intermission (`pm_type` 5 on both clients) to the
  next map, no script error. VERIFIED (the test).
- `getAssignedTeam` (0x80bb48e), `buttonPressed` (0x80a9838): pass 0 to
  the call at 0x815effa and return. VERIFIED (the bytes). That call is
  `Scr_AddInt`. INFERRED (`GScr_ReturnZero`). `isRagdoll` (0x80c4eb4)
  tests `pos.trType` for a ragdoll type, which `startRagdoll` writes
  (cod4-combat.md 5.1). VERIFIED (the bytes).
- `getCorpseAnim` (0x80c4dfc): `eType` 2 only (`Only valid on player
  corpses`, 0x821c549); returns anim `legsAnim & 0xfdff` (0x80c4e6d) in
  the corpse slot's tree (0x80c4e81). VERIFIED (the instructions).
  `getAnimLength` is `1 / frequency`; `animHasNotetrack`,
  `getNotetrackTimes` read `XAnimNotifyInfo` (clip fractions). INFERRED
  (`GScr_GetCorpseAnim`, `XAnimGetLength`).
- `spawnFx(fx, origin[, forward[, up]])`: an `ET_FX` (8), `un1` the
  effect, `lerp.apos.trBase` from `Scr_SetFxAngles` (0x80c6852): no
  direction stores (270, 0, 0) (0x80c6869), one goes through
  `vectoangles` (0x80c6886), two build the axis; two-argument `playFx`
  passes none (0x80c6cbe). VERIFIED (the instructions). `triggerFx(ent[,
  time])` writes `time2`; `playLoopedFx` spawns `ET_LOOP_FX` (9) with
  `lerp.u.loopFx.cullDist`, `.period`. INFERRED (`Scr_SpawnFX`,
  `Scr_TriggerFX`, `Scr_PlayLoopedFX`). Trap: CoD1's `ET_FX` fields read as
  a one-shot on 1.7.
- `playSoundToTeam(alias, team[, ignore])` (0x80c3edc): broadcast temp
  entity (`svFlags |= 8`, 0x80c3f3c), event 3 (`EV_SOUND_ALIAS`) at
  `r.currentOrigin`, alias in `eventParm`. `allies` 2, `axis` 1, else
  `Illegal team string '%s'. Must be allies, or axis.` (0x821d56c);
  `ignore` must be a player (`entity %i is not a player`, 0x8217f5e).
  `r.clientMask` starts all set and clears every in-use client on the team
  but `ignore` (0x80c3fbc..0x80c403b). VERIFIED (the instructions).
  `builtins::cod4_script` sends to the same clients. `playSoundAsMaster`
  raises `EV_SOUND_ALIAS_AS_MASTER` (4) for all. INFERRED. Battle chatter
  (`_battlechatter_mp.gsc`) uses the first.
- `setWaypoint(constantSize[, offscreenMaterial])` (0x808f116): `type` 13,
  `value` the int as float, `offscreenMaterialIdx` (+0x44) the material
  index or 0; `not a hud element` (0x82155a4) otherwise. VERIFIED (the
  instructions).
- `setTargetEnt`, `clearTargetEnt`, `setPlayerNameString` (type 3, client
  in `value`), `clearAllTextAfterHudElem` write `type`, `value`,
  `targetEntNum`. INFERRED (`HECmd_*`).
- `beginLocationSelection(material[, radius])` writes
  `ps.locationSelectionInfo`: the selector's 1-based slot among 827..829
  plus six radius bits over the minimap height. While set, fire notifies
  `confirm_location` with the minimap point under the cmd's
  `selectedLocation`, 0x20000 notifies `cancel_location`. VERIFIED:
  `ClientThink_real` tests the gathered presses (client +0x3094) for
  0x10000 then 0x20000 while +0x5b4 is set, then masks buttons to 0x1300
  (0x80a5d47..0x80a5d81); the point is `(c + 128) / 255` of the minimap's
  world size per side from its upper left (0x80a662f..0x80a669d).
- INFERRED: `setTeamRadar` sets the team's `ps.radarEnabled`;
  `setDepthOfField` the six `dof*`; `fragButtonPressed` (0x4000),
  `meleeButtonPressed` (4), `secondaryOffhandButtonPressed` (0x8000) read
  held and new presses; `updateDMScores` sends `I <client> <score>` for
  the player and the best other.
- INFERRED: `kick(n)` queues `tempBanClient n` (`GScr_KickPlayer`);
  `detonate([attacker])` explodes a grenade on the next missile pass;
  `damageConeTrace` / `sightConeTrace` count `CanDamage`'s five body
  points in clear sight (non-client: origin only); `physicsTrace` /
  `playerPhysicsTrace` stop a point or the player box at the map.
- Trace masks (loaded immediates): 0x802011 (`damageConeTrace`, 0x80c5d3a)
  and 0x801 (`sightConeTrace`, 0x80c5d28) into the body at 0x80c5c80;
  0x820011 (`physicsTrace` 0x80be7e0, `playerPhysicsTrace` 0x80c1e98, box
  (-15, -15, 0)..(15, 15, 70) at 0x820e024, 0x820e030). VERIFIED (the
  instructions).
- `getEye` (0x80c2eea): `r.currentOrigin` + 40 (0x820ded4) in z, any
  entity or stance. VERIFIED (the instructions). `getTagOrigin` /
  `getTagAngles` place the bind pose (`baseMat`) by origin and angles; a
  playing body reads its posed bone (3.3). INFERRED (the code). Traces
  use 3.3's world.

Hudelem fields: 19 rows at 0x8215a20, 28 bytes `{name, offset, type, mask,
shift, setter, getter}`: CoD2's 15 plus `glowcolor`, `glowalpha`,
`hidewhendead`, `hidewheninmenu`. VERIFIED. `font` names at 0x826ce18:
`default`, `bigfixed`, `smallfixed`, `objective`, `big`, `small`; the four
align tables at 0x826cdc0 .. 0x826ce14 hold CoD2's names. VERIFIED
(`fields::COD4_HUD_FIELDS`).

`radiant/keys.txt` is a `common_mp.ff` rawfile, read through the zones:
`_destructible.gsc` reads `destructible_type`, which only it lists.
VERIFIED (the rawfile). That 1.7's engine reads it is INFERRED (string
`radiant` at 0x821fc48; the scripts need the key).

### 3.2 The hardpoint vehicles

`builtins/cod4_vehicle.rs`, `game/cod4/vehicle.rs`. The stock airstrike
(`_hardpoints.gsc` `useAirstrike` .. `callStrike_bombEffect`) and
helicopter (`triggerHardpoint("helicopter_mp")`, `_helicopter.gsc`
`heli_think` .. `heli_leave`) run on mp_crash `war` with no script error:
`cod4::hardpoint_tests` (`COD4_DIR`). VERIFIED (the tests). Engine facts
below are `cod4_lnxded` 1.7's; "The flight against retail" compares a
capture.

- `spawnPlane(owner, classname, origin[, spawnflags])` (0x80c0fde):
  refuses an owner with no client ("Owner entity is not a player"),
  spawns through the spawn table (0x80c8af8), fourth argument to
  `spawnflags` (+0x17c), sets `eType` 13 (`ET_PLANE`) and
  `lerp.u.vehicle.teamAndOwnerIndex` (+0x6c) = `sess.cs.team` (client
  +0x3010) | slot << 2 (the multiply by 0x22e5e84 is the exact division by
  `sizeof(gclient_t)` 0x3184, times 4). VERIFIED (the disassembly). The
  airstrike's planes reach a client as type 13, `5` for an axis shooter in
  slot 1, plus the model `index`. VERIFIED (the test, ours). `moveTo` is
  the shared mover's and agrees with retail word for word: `trType` 6, 3,
  7 for a move with both eases (1.7 has no `TR_GRAVITY_PAUSED`, so CoD2's
  7 and 8 move down one, `game::cod4::wire::tr_type4`); the script's
  angles sit in `apos.trBase` under a move alone. VERIFIED
  (`hardpoint_ab`).
- `spawnHelicopter(owner, origin, angles, vehicleInfo, model)` (0x80c0e54):
  refuses an owner with no client, spawns and places a `script_vehicle`,
  then 0x80ba8d6: model (0x80cae3a), vehicle file (0x80cd966), `eType` 12,
  type forced to 5 (helicopter), `teamAndOwnerIndex` as the plane's,
  `health` 99999 at once, ±50 box. VERIFIED (the disassembly).
  `VEH_InitEntity` (0x809c650): `r.contents` 0x2080, `svFlags` 4, both
  `trType`s 1 (`TR_INTERPOLATE`), `s.weapon` the file's `turretWeapon`
  (record +0x50), `clipmask` 0x810211, first think next frame. VERIFIED
  (the disassembly).
- Wire: model in `un2`, `index` 0, `weapon` 113 (`cobra_20mm_mp`),
  `pos.trDelta` always 0, `time2` 7000 from the second snapshot. VERIFIED
  (`play-1.7-hardpoint.txt`). The think stores `(int)(1000 *` record
  +0x8c`)` in `s.time2` (0x80a03b8). VERIFIED (the disassembly). +0x8c is
  `suspensionTravel` (7 on `cobra_mp`, its only key at 7). INFERRED. vcod
  sends all four. VERIFIED (`hardpoint_ab`).
- `VEH_InitPhysics` (0x809d13a): `maxPitch`, `maxRoll` 25; `maxAngleVel`
  (45, 90, 45); yaw accel 25, decel 15; overshoot 0.1; drag speed 1056 (60
  mph); turning ability 0.5; hover radius 30, speed 14.08 (0.8 mph), rate
  8.8 (0.5 mph). VERIFIED (the disassembly).

Methods. VERIFIED (the disassembly):

| Method | Address | Rule |
|---|---|---|
| `setSpeed` | 0x809be2a | mph times 17.6 (0x8215108); decel half the stored accel unless given (0.5 at 0x8209628); if the helicopter is under the new speed and the new speed under the accel, accel is capped at the speed; speed < 0: error, 0; accel or decel <= 0: error, both 17.6 |
| `resumeSpeed` | 0x809cf4a | accel times 17.6 |
| `setYawSpeed` | 0x809ce22 | degrees: speed, accel, decel (accel unless given), overshoot 0..1 |
| `setMaxPitchRoll` | 0x809cd6c | helicopter only |
| `setTurningAbility`, `setNearGoalNotifyDist` | 0x809d0ae, 0x809cb42 | as given |
| `setAirResistance` | 0x809cd36 | times 17.6 |
| `setHoverParams` | 0x809cc24 | radius, speed, rate in units/s |
| `setVehGoalPos(pos, stop)` | 0x809ca7c | stop if second non-zero; refuses zero speed, accel or decel; leaves the hover |
| `setTargetYaw` | 0x809c9da | as given |
| `setTurretTargetVec` | 0x809d48a | refuses a player's vehicle and one with no health |
| `setDamageStage` | 0x80ba890 | writes `un1` (+0xd4) |

The think (0x809e38a, `VEH_UpdateMoveToGoal`, orientation inlined).
VERIFIED (the disassembly):

- Speed: set speed toward the goal; hovering, toward goal plus wander at
  the hover speed and rate (decel half the rate). A stop goal or the hover
  brakes once the new speed's stopping distance reaches the goal, flying
  the frame's first part at the old speed. Under the set speed a turn
  never brakes. Change capped at the rate per frame.
- Outside the hover: a stop eases horizontal speed into the goal; a turn
  tighter than the speed allows brakes by `setTurningAbility` (1 toward a
  stop); climbs and descents ease to the goal height (at most three
  frames of accel per frame).
- `phys.accel` moves half way to the frame's change, capped like it unless
  stopping. Position uses the mean of old and new velocity.
- Yaw: the goal yaw once the stop is close enough to turn into it, else
  the target yaw, else (hovering) hold, else along the move; at
  `setYawSpeed`'s rates, a fifth of them past the goal yaw; `goal_yaw`
  on reaching it.
- Pitch, roll: toward `maxPitch` / `maxRoll` times smoothed accel plus fake
  drag (horizontal speed's share of drag speed, squared, times 5), along
  and across the body, times `f + (1 - f) 0.1` (`f` = accel / file
  `accel`), scaled down while stopping by `3.5 (1 - f) + 2.5 f`. Tilt rate
  `45 g + (1 - g)`, decel 0.4 of it; `g` the set accel's share (the
  hover's once hovering within 5 degrees and 3/s of level).
- Outside the hover: `near_goal` each frame it starts nearer than
  `setNearGoalNotifyDist`; `goal` when a non-stop goal is within a frame's
  flight, or a stop goal inside the hover radius under 35.2 (2 mph; at
  rest with no radius), which starts the hover.

Around it:

- Wander: `G_flrand` off the level's `rand()` (cod4-combat.md 10.9);
  `hardpoint_ab`'s hover on the recovered seed stays within tolerance.
  VERIFIED (the gate). A hover radius of 0 draws nothing (the test at
  0x80a073c). VERIFIED (the disassembly). `Heli4::think` is this.
  INFERRED (the code against the disassembly).

The vehicle think around it (0x809fbd6, called by handler 23's think
0x80ba1f8, which sets `nextthink` again at 0x80ba212). VERIFIED (the
disassembly).

- It copies `r.currentOrigin` and `r.currentAngles` into the physics'
  last place and turn (0x809fc2e..0x809fc7b). VERIFIED (the disassembly).
  Under flag 2 (a goal) it runs `VEH_UpdateMoveToGoal` toward the goal
  (vehicle +0x2a0) for `moveState` 1, toward goal plus wander (+0x230)
  for 2 (0x809fce2..0x809fcf4, 0x80a06b0, 0x80a0841). VERIFIED (the
  disassembly). With no goal a helicopter (type 5) runs 0x808ba22 instead
  (0x80a048f..0x80a04a0). VERIFIED (the disassembly). What it does: "The
  no-goal think" below. Then 0x809c1dc places the
  entity (`G_SetOrigin` at 0x809c2d2, 0x80ca276 at 0x809c2de). VERIFIED
  (the calls); 0x80ca276 being `G_SetAngle` is INFERRED.
- The move trace (0x80a04aa..0x80a05e3): under flag 2, when 0x815cfe0
  answers non-zero and the body moved (a zero squared move skips at
  0x80a04fb), `G_TraceCapsule` (0x80b408a) from the last place to the new
  under 0x2081, passing itself, with `r.mins` and `r.maxs`; a box wider
  than tall gets its top raised to its width first (0x80a0549..0x80a055c).
  A contact (fraction under 1) with the world (hit id 0x3fe, 0x80a05ad)
  pushes the trace's normal (+4, 0x80a05b7) and notifies
  `script_vehicle_collision` (the `scr_const` stored at 0x80a4244 from the
  string at 0x80a4238). The move is not undone. VERIFIED (the
  disassembly). 0x815cfe0 reads two globals; that it asks whether the
  script system runs is INFERRED.
- The box: `VEH_InitEntity` sets `r.mins` and `r.maxs` through
  `DObjGetBounds` (0x809c725 to 0x81ac44c): minus and plus the DObj's
  radius on every axis, that radius summed over its models
  (0x81ac954..0x81ac978). VERIFIED (the disassembly). The cobra's model
  radius is 418.7. VERIFIED (the zone). The ±50 of `spawnHelicopter` is
  the vehicle's own box (vehicle +0x120 and +0x12c, 0x80ba97f..
  0x80ba99f). VERIFIED (the disassembly).
- The touch (0x809fd20..0x809ff85): the entities in the box at the last
  place grown by the bounds' radius and the move (0x81947ac, 0x81906bc,
  then 0x811f386 with 1024 and contents 0x2806081). Only missiles
  (`eType` 4) are read. One whose `groundEntityNum` (+0x7c) is the
  vehicle, with +0x218 clear, on a weapon with `stickiness` 1
  (`WeaponDef` +0x5a4) rides along once the vehicle moved or turned by
  0.001 squared (0x80a0400..0x80a0485): 0x809c462 keeps its place and turn
  in the vehicle's frame (the offset through the old axis transposed
  times the new; the turn into `r.currentAngles` and `apos.trBase`).
  Any other missile is touched (both touch handlers, `touch` notified
  both ways) only when the vehicle has `r.bmodel` (+0xf5, 0x809fea4),
  which `SV_SetBrushModel` alone sets (0x817c834). Handler 23's touch slot
  is empty (`entityHandlers` 0x826cee0, row 23). VERIFIED (the
  disassembly). A helicopter is an xmodel, so no missile touches it here.
  INFERRED. That +0x218 is the entity's link is INFERRED.
- Then the turret (`VEH_UpdateAim`, "The turret" below), `time2` and
  `nextthink`. VERIFIED (the disassembly).
- The controller (handler 23's slot +0x1c, 0x80bab1e) poses the DObj:
  `tag_body` to (bodyPitch, 0, bodyRoll), the `lerp.u` words at +0x54 and
  +0x58; `tag_turret` to (0, gunYaw, 0); `tag_barrel` to (gunPitch, 0, 0)
  (`gunPitch` +0x64, `gunYaw` +0x68; the bone numbers at vehicle +0x318,
  +0x31c and +0x324 from 0x80cdeb5..0x80cdf1e, off the names whose
  strings load at 0x80a45f2..0x80a4628). Each goes through 0x81abb2a to
  0x81b00de, which stores the angles' quat and a zero shift in the
  bone's slot (0x81b010a..0x81b0127). VERIFIED (the disassembly).

On ours (`game::cod4::vehicle`):

- `heli_collision` sweeps the radius box through the clip map and the
  brush entities and notifies `script_vehicle_collision` with the normal,
  as retail does on the collision capture below. VERIFIED (the gate).
  Script models' and other helicopters' boxes are left out of the sweep.
  INFERRED.
- `carry_riders` moves a sticky missile resting on the helicopter;
  `lost_ground` counts a helicopter's collision as ground. INFERRED (no
  capture holds a grenade on a helicopter).
- `heli_pose` sets the three tags' turns in place of their bind turns,
  places kept; everything under them follows. That the slot's turn stands
  in for the bone's own is INFERRED off the capture below: the cobra's
  `tag_barrel` sits 7.3 degrees nose up at the bind pose, and retail's
  `tag_flash` points along the gun's pitch. The stock MP scripts never
  wait on `script_vehicle_collision`. VERIFIED (`common_mp`'s rawfiles).
- A stock helicopter runs 0x808ba22 for its first 2 s: `heli_fly` waits
  2 s before its first goal. VERIFIED (`common_mp`'s `_helicopter.gsc`).
  `Heli4::hold` is its at-rest case ("The no-goal think"). INFERRED (the
  code against the disassembly).
- Turret: `Heli4::aim` and `heli_aim_from` are `VEH_UpdateAim` ("The
  turret"). INFERRED (the code against the disassembly).
- `fireWeapon([tag[, target]])` raises `EV_FIRE_WEAPON` (26,
  `vcod_cod4::pmove::weapon::ev`, VERIFIED). A bullet weapon
  (`cobra_20mm_mp`) traces from the tag along the gun inside the hip
  spread disc, hurting what it meets as the owner's round, helicopter the
  inflictor; a projectile (`cobra_FFAR_mp`) bursts as a grenade (hit
  `MOD_PROJECTILE`, splash `MOD_PROJECTILE_SPLASH`). INFERRED. The
  projectile: `Weapon_RocketLauncher_Fire` (0x80a2d80, called 0x809de60)
  along the tag's axes (`G_DObjGetWorldTagMatrix`, 0x80cbe9a) at
  `adsSpread` (+0x71c), the helicopter's velocity (+0x138) as the gun's,
  second argument the target, third its offset (0x809dde2, 0x809ddfc).
  VERIFIED (the disassembly). vcod reads the posed tag (`heli_tag`) and
  uses `Heli4::vel`; `getTagOrigin` on a helicopter reads it too.
  INFERRED (the code).
- Collision: `r.contents` 0x2080 and `svFlags` 4, so a locational trace
  meets the model's collision (`DObjGeomTraceline` over `collSurfs`).
  INFERRED (`SV_ClipMoveToEntity`). `XModelTraceLineAnimated` (0x81c61bc,
  called at 0x81abe6a) skips a surface off the mask or on a hidden bone,
  compares the bone's posed matrix with its base pose's word by word
  (0x81c6298..0x81c6378), and traces an unmoved bone's surface with the
  segment as it is; a moved one's takes the segment through the base
  matrix times the posed one's inverse (0x81c63e8..0x81c671b) into the
  base pose's space. A hit stores the triangle's plane normal as it is
  (0x81c6940..0x81c6954), so a moved surface's normal stays in the base
  pose's frame. VERIFIED (the disassembly). Ours traces each surface of
  the posed model so (`trace_posed`); the server runs no rotor anim.
  VERIFIED (the collision capture below). A blast measures to the
  surfaces' box at the origin, unturned, reached by one clear trace to its
  middle. INFERRED (the code).
- `G_Damage`, INFERRED: `G_VehImmuneToDamage` lets a hit through only if
  the file allows its means of death (bullets: `bulletDamage`, or
  `armorPiercingDamage` with flag 2; grenades: `grenadeDamage`, or
  `heavyExplosiveDamage` if heavy; `projectileDamage`,
  `projectileSplashDamage`; `MOD_EXPLOSIVE` always; else never). At least
  1 comes off `health` (99999, `Heli_InitFirstThink`), then `damage` is
  notified with (damage, attacker, direction, point, means of death,
  model, tag, part, damage flags), names empty. No location multiplier.
  `Helicopter_Pain`'s jolt scales by `maxBodyPitch` / `maxBodyRoll`, 0 on
  `cobra_mp`. `cobra_mp` sets all six damage keys to 1. VERIFIED (the
  rawfile).
- Stock `_helicopter.gsc` does the rest: `heli_damage_monitor` sums damage
  (bullets at `scr_heli_armor_bulletdamage` 0.3 under `scr_heli_armor`);
  `heli_health` sets stage 3, then 2 and 1 as it smokes; past
  `scr_heli_maxhealth`, `heli_crash` flies the crash path, spins
  (`setTargetYaw`), sets stage 0, explodes.
  `cod4::hardpoint_tests::a_helicopter_shot_down_crashes` (allied rifle, ours):
  stages 3, 1, 0 on the wire, entity gone well before its loop ends.
  VERIFIED (the test, ours).
- A player's launcher: `Weapon_RocketLauncher_Fire` (0x80a2d80, from
  `FireWeapon` 0x80a31b0), a `tan(spread) 16` disc around 16 ahead (angle
  `G_random() 360`, radius `G_random()`, 0x80a2db7), no gun velocity or
  target (0x80a3193), firer pushed back 64 along the view (0x80a2ef8).
  VERIFIED (the disassembly). vcod starts it at the eye. INFERRED (the
  code). It bursts on impact (`MOD_PROJECTILE`, splash
  `MOD_PROJECTILE_SPLASH`) or at `projLifetime`. INFERRED.
- `playFxOnTag(fx, ent, tag)`: `"%02d%s"` into the effect tags from 1698,
  `EV_PLAY_FX_ON_TAG` (56, VERIFIED, `vcod_cod4::net::events`) on the
  entity, tag checked against the model's bones (`model_tags`). INFERRED
  (`Scr_PlayFXOnTag`). Trap: a CoD1 model check failed every plane's and
  bomb's call.
- `physicsLaunch([contact, force])` (0x809afb4): `script_brushmodel`,
  `script_model`, `script_origin` or `light`; both vectors or neither
  (zeros); a second launch errors ("physicslaunch called more than once
  for the same entity."). Sets `pos.trTime` to level time,
  `pos.trDuration` 0x7fffffff, both `trBase`s in place, contact in
  `pos.trDelta`, force in `apos.trDelta`, both `trType`s 8 (`TR_PHYSICS`),
  `r.contents` 0, `takedamage` 0. The server never moves it:
  `BG_EvaluateTrajectory` (0x80536ba) reads 8 like 0, 1, 11, at `trBase`.
  VERIFIED (the disassembly). vcod sends the same words, keeps the part in
  place, `notSolid`, undamageable
  (`hardpoint_tests::physics_launch_and_the_explosion_sphere_go_out`).
  VERIFIED (the test, ours). Each client flies the part itself (3.6).
- `physicsExplosionSphere(origin, outer, inner, magnitude)` (0x80bd00e):
  four arguments; a temp entity `EV_PHYS_EXPLOSION_SPHERE` (57) at the
  origin (0x80cbd96), outer radius the parm, inner in `lerp.u` word 0
  (+0x54), magnitude word 1 (+0x58); errors on a negative inner or one
  past the outer; nothing else server-side. VERIFIED (the disassembly).
  The event table sends both words (`lerp.u.anonymous.data[0]`, `[1]`,
  `vcod_cod4::net::fields`), and vcod's event carries both (same test).
  VERIFIED (the test, ours).
- A client-less CoD4 entity owns `pers` and `maxhealth`
  (`chopper.pers["team"]`, `chopper.maxhealth` failed on ours as client
  fields). VERIFIED (the error). 1.7 reaches client fields only through
  `ent->client`. INFERRED.

#### The flight against retail

`cod4::hardpoint_ab` (`COD4_DIR`) replays
`crates/cod4/tests/fixtures/net/play-1.7-hardpoint.txt`. Recipe: retail
1.7, `+set fs_game mods/vcodhp +devmap mp_crash`, `war`, sv_pure 0; probe
gsc `crates/cod4/tests/fixtures/gsc/hardpoint_flight/maps/mp/mp_crash_fx.gsc`
in the mod's iwd (read over the zone's); one `cod4_probe` (`--team axis
--script idle --name owner --qport 4021 --secs 75`) under `unshare -rnm`.
A second after the spawn the gsc flies two planes (steady `moveTo`; both
eases) and a `cobra_mp` along mp_crash's first loop nodes (no stop, no
stop, stop), hovers, turns 120 with `setTargetYaw` while the gun takes a
point below, flies home under `setHoverParams(60, 20, 10)`,
`setAirResistance(40)` and a slower `setSpeed`, deletes all three. No
script random draw. Seed 1936999990 (the probe's challenge at draw 17,
`wire_ab::recover_game_seed`). VERIFIED (the capture). Ours loads the same
file (`Cod4Server::set_script_overrides`) and compares every plane and
helicopter word at the same time past spawn.

- Planes: bit for bit. VERIFIED (the gate).
- Helicopter: bit for bit for 1.5 s; the turn through the first node
  parts by 0.005 degrees pitch, 0.08 yaw; the rest stays within 8 units,
  1.9 degrees of body, 0.124 of gun. VERIFIED (the gate). The parting is
  the float model (retail's x87 intermediates against our `f32`, steered
  by a goal vector shrinking to nothing at the node). INFERRED: the
  think's rules match the disassembly. `hardpoint_ab::GAPS` tolerates it.
- The gun aims in the flown body's frame ("The turret"), so it parts as
  the body does: 0.45 degrees at worst when ours aimed from `tag_flash` in
  the body's own angles, 0.124 from `tag_barrel` in bone 0's. VERIFIED
  (the gate, both ways). On a body that holds still it matches to a few
  ulps (the idle capture below). VERIFIED (the gate).

#### The collision capture

`cod4::hardpoint_ab::helicopter_collision_and_pose_match_retail` replays
`crates/cod4/tests/fixtures/net/play-1.7-heli-collide.txt`. Recipe: the
flight's, under `+set fs_game mods/vcodcoll` with
`crates/cod4/tests/fixtures/gsc/heli_collide/maps/mp/mp_crash_fx.gsc` in
the mod's iwd and `--secs 45`. A cobra holds over the first loop node
(hover radius 0), turns its gun on a point off its left side, prints its
posed `tag_barrel` and `tag_flash` and a `bulletTrace` down through their
middle, then flies straight down into the ground and prints its
`script_vehicle_collision` notifies. Each line is an `iPrintLn`, its time
counted from the spawn. VERIFIED (the capture).

- Retail's `tag_flash` sits along the gun's pitch from `tag_barrel`
  (16.58 degrees down at `gunPitch` 16.66). VERIFIED (the capture).
- The trace meets the barrel at retail's point, within 0.01, with
  retail's normal word for word (-0.112754, 0, 0.993623): the base pose's
  normal of a gun yawed 99.8 degrees. VERIFIED (the gate).
- The first notify comes 5600 ms past the spawn with the body at z 750.08
  over ground near 332, normal (0, 0, 1); the next at 5650; the third,
  started in solid, reads (0, 0, 0); 209 by 16 s. Ours: the same times,
  normals, places and count, the last place 0.1 off. VERIFIED (the
  gate).

#### The no-goal think

`cod4_lnxded` 0x808ba22, run by the vehicle think for a helicopter with
no goal (above). Read off the disassembly; `Heli4::hold` is what it does
on the server.

- It is 1.7's player-helicopter step: it reads the 28 `vehHelicopter*`
  dvars registered at 0x80ba309..0x80ba82d (`vehHelicopterMaxSpeed` 150
  mph .. `vehHelicopterTiltMomentum` 0.4; `vehHelicopterJitterJerkyness`
  0.3, `vehHelicopterHoverSpeedThreshold` 400). VERIFIED (the
  disassembly, `tools/re/cod4_dvars.py`).
- Its four stick bytes are locals zeroed at 0x808ba6a..0x808ba76 and
  never written after, so on the server every stick input is 0. VERIFIED
  (the disassembly).
- Only `setVehGoalPos` sets flag 2 (vehicle +0x1c0, 0x809cb02); no store
  in the vehicle code clears it, and `VEH_InitPhysics` (0x809d13a) zeroes
  the velocity (vehicle +0x138), the tilt and its rate (+0x1a0..+0x1b0),
  the yaw rate (+0x154) and the jitter (+0x1e8..+0x224). VERIFIED (the
  disassembly). So it runs only before a helicopter's first goal, from
  rest. INFERRED (the two facts above).
- From rest every change it makes is 0: the velocity's (DiffTrack
  0x818ec02 toward 0 from 0, then `+= 0.05` of it), so the move (0x80a0b08)
  and the lookahead trace (0x808b920, `vehHelicopterLookaheadTime`, which
  returns at a zero velocity) never run; the tilt's spring (its target
  off the zero velocities, `vehHelicopterTiltSpeed` over the momentum);
  the yaw rate's. INFERRED (the arithmetic read at rest).
- The jitter only `setJitterParams` (0x809cc92) would start, and no row of
  the vehicle method table (0x821be40, 25 rows) names it. VERIFIED
  (`tools/re/cod4_builtins.py`).
- Pitch and roll: the tilt times `vehHelicopterMaxPitch` and `MaxRoll`,
  then (under the hover speed threshold, always at rest) the jitter's
  `share * jitter + angle` (0x808cfd4..0x808d006), so `+0 + angle`: both
  come out +0 whatever the spawn's angles. VERIFIED (the disassembly).
- Yaw: the last think's yaw (r.currentAngles, copied in at 0x809fc60)
  plus the rate's step, times the float 1/360, less `floor(x + 0.5)`,
  times 360, all on the stack with one store (0x808c719..0x808c798).
  VERIFIED (the disassembly). A spawn yawed 250 reads -110 from the first
  think. VERIFIED (the idle capture).
- It also tracks vehicle +0x2e4 toward 1 and +0x2e8 toward 0 at 4 a
  second; `VEH_UpdateMoveToGoal` stores them too (0x809e9ce), and nothing
  outside 0x808ba22 reads them. VERIFIED (the disassembly).

#### The turret

`VEH_UpdateAim`, inlined in the vehicle think (0x809ff9b..0x80a038c).

- It runs when the vehicle has a target (+0x268) and the entity health
  over 0 and a `tag_barrel` (bone slot +0x324, from the name at
  0x80a4628; none skips it, 0x80a004c). With no target the turning state
  (+0x1e4) runs down 2, 1, 0 and nothing is notified (0x80a03d4).
  VERIFIED (the disassembly).
- The target is its entity's `r.currentOrigin` plus the offset (+0x290),
  or the point (+0x284). VERIFIED (the disassembly).
- It aims from `tag_barrel`'s world place (`G_DObjGetWorldTagPos`
  0x80cbfc4: the posed bone's place through `AnglesToAxis` of
  `r.currentAngles`, plus the origin, 0x819016e). VERIFIED (the
  disassembly).
- In the frame of the angles (`AxisToAngles` 0x81951fe) of the world
  matrix (`G_DObjGetWorldTagMatrix` 0x80cbe9a) of the bone in slot
  +0x320; with -1 there, the body's angles (0x80a0684). VERIFIED (the
  disassembly). Nothing stores to vehicle +0x320, and the vehicle pool
  (0x8586de0, eight of 0x354 bytes, `.bss`) is reset only at +0x1b8
  (0x80cce3b..0x80cce81). VERIFIED (the disassembly). So the slot names
  bone 0, `tag_origin` on the cobra (`bone_dump`). INFERRED (the two
  facts). The gdb run below read the frame (0, 250, 0) on a body at (0,
  -110, 0). VERIFIED (gdb).
- The direction to the target is normalized with its length kept; its
  `vectoangles` (0x8191a72) taken back into the frame:
  `AxisToAngles(AnglesToAxis(dir) * transpose(AnglesToAxis(frame)))`
  (0x818e910, 0x818f6f6, 0x818effc): the target's pitch and yaw in the
  frame. VERIFIED (the disassembly).
- Each gun angle steps by 0x8194154: the target angle brought within 180
  of the gun's by whole turns, the gun moved `turretRotRate * 0.05`
  toward it unless the gap is within 0.001 or the step would pass it
  (then the target angle), normalised on the stack. Then held to
  `-turretVertSpanUp ..= turretVertSpanDown` and `-turretHorizSpanRight
  ..= turretHorizSpanLeft` (the file's +0xd8, +0xdc, +0xd4, +0xd0; the
  field table's rows at 0x826dddc..0x826de0c). VERIFIED (the
  disassembly).
- The turning state: 2 while either `AnglesSubtract` (0x819397a) of the
  target angle and the gun angle before the step is 2 or more on an angle
  the spans left alone; else 2 goes to 1, and 1 notifies
  `turret_rotate_stopped` and goes to 0. Then, either off by a degree or
  more: `turret_not_on_target`, `turret_no_vis`; else `turret_on_target`,
  then at an entity a sight trace from the barrel (0x817cf14, mask 0x801,
  passing the vehicle and the target) for `turret_on_vistarget` or
  `turret_no_vis`, and at a point `turret_no_vis` (the strings at
  0x8217aa0..0x8217ae8). VERIFIED (the disassembly).
- `Heli4::aim` and `heli_aim_from` do this, the x87 intermediates as
  `f64`. INFERRED (the code against the disassembly). Its sight trace
  passes only the helicopter, which is enough under 0x801, a mask with no
  body bit. INFERRED (the code).
- gdb on retail running the idle capture's gsc (0x80a011c, 0x80a012e,
  0x80a01a3: the barrel, the direction, the frame, the target angles, the
  target in the frame, the gun) against the same values on ours, 159
  thinks: wherever the barrel, the frame and the gun before the step match
  (51 thinks), the direction, the target angles and the target in the
  frame match word for word. Before the goal the barrel
  lands an ulp off in 26 of 80 thinks: ours poses the tags off the zone's
  bind-pose `baseMat`, 1.7's skeleton composes the model's local bones. In
  flight the frame's roll lands an ulp off (the `AxisToAngles` roll's
  extended intermediates against our `f64`). VERIFIED (gdb).

#### The idle capture

`cod4::hardpoint_ab::helicopter_idle_matches_retail` replays
`crates/cod4/tests/fixtures/net/play-1.7-heli-idle.txt`. Recipe: the
flight's, under `+set fs_game mods/vcodidle` with
`crates/cod4/tests/fixtures/gsc/heli_idle/maps/mp/mp_crash_fx.gsc` in the
mod's iwd and `--secs 30`. A cobra spawns over the first loop node at
angles (8, 250, -6) with no goal for 4 s, its gun on a point ahead and
below, then on one behind; then it flies a pass-through goal and is
deleted. No hover radius and no stop goal, so no random draw. VERIFIED
(the capture).

- Before the goal every place and body word matches retail's, the body
  (0, -110, 0) from the first think; the gun within 3.1e-5 degrees (the
  barrel's ulps above), `hardpoint_ab::STILL_GAPS`. VERIFIED (the gate).
  Without `Heli4::hold` the body read 8 degrees of pitch off from the
  first think and the gun 7.4. VERIFIED (the gate, both ways).
- Past the goal the flight's tolerances hold; the gun parts by 0.128 at
  worst, as its body does. VERIFIED (the gate).

#### Missiles under 1.7: attractors, the wobble, guidance

Read off `cod4_lnxded` 1.7 and the stock weapon files (`main/iw_11.iwd`
`weapons/mp/rpg_mp`, `cobra_ffar_mp`); `game::cod4::missile`
(`g_fire_rocket`, `missile_trajectory`, `steer`, `wobble_and_attract`,
`attract`) and `builtins::cod4_vehicle` follow it. VERIFIED (the
disassembly) offsets:

| Struct | Field | Offset |
|---|---|---|
| `WeaponDef` (`vcod_cod4::pmove::weapon`) | `projectileSpeed` (int) | +0x568 |
| | `projLifetime` | +0x578 |
| | `timeToAccelerate` | +0x57c |
| | `projectileCurvature` | +0x580 |
| | `guidedMissileType` | +0x6b8 |
| | `maxSteeringAccel` | +0x6bc |
| | `destabilizationRateTime` | +0x810 |
| | `destabilizationCurvatureMax` | +0x814 |
| | `destabilizeDistance` (int) | +0x818 |
| entity | `missile.time` | +0x1b4 |
| | curvature (or wobble) | +0x1d0 |
| | `targetOffset` | +0x1dc |
| | target handle (number + 1) | +0x214 |
| | own flags (not `s.eFlags`) | +0x180 |

- Attractors: 32 slots of 0x1c bytes at 0x8361a80 (in use, attractor 1 /
  repulsor 0, entity (0x3ff for a point), point, strength, reach).
  `missile_createAttractorEnt(ent, strength, maxDist)` (0x8093486),
  `missile_createAttractorOrigin` (0x8096c28), `missile_createRepulsorEnt`
  (0x8096b5c), `missile_createRepulsorOrigin` (0x8096a80) take the first
  free slot ("Ran out of attractor/repulsors.  Max allowed: %i"), refuse a
  reach <= 0 ("maxDist must be greater than zero") and return the slot;
  the entity forms set entity flag 0x800000. `missile_deleteAttractor(n)`
  (0x80933b0) clears slot `n` ("Invalid attractor or repulsor" past 31);
  freeing an entity clears every slot naming it (0x8093402); a level start
  clears all (0x8093478). VERIFIED (the disassembly). vcod clears a dead
  entity's slot when a slot is next taken or a missile next reads it.
  INFERRED (the code). `missile_setTarget([ent[, offset]])` (0x80c5f34)
  refuses all but a `rocket` ("Entity %i is not a rocket") and sets the
  target (none without one) and offset (0 without one). VERIFIED (the
  disassembly).
- `G_FireRocket` (0x8096d04). VERIFIED (the disassembly):
  - the `rocket` entity, `eType` 4, `eFlags` 0x400;
  - launch time: level time, plus the no-draw time (`clamp((int)(85 - 35
    speed / 600), 20, 50)`) only for a client parent, so a helicopter's
    rocket draws at once;
  - `nextthink`: level time plus `(int)(projLifetime 1000)`, at most 60 s;
  - target and offset;
  - `pos`: `TR_LINEAR` at the start, `trDelta` direction times speed, or
    `TR_INTERPOLATE` from rest under `timeToAccelerate`; gun velocity
    added, each component truncated (0x8096f69); angles `vectoangles` of
    the direction;
  - a curving weapon goes `TR_INTERPOLATE` and draws its curvature: `r1 =
    G_random()`, `r2 = G_random() projectileCurvature`, vector `right
    cos(r1 360) r2 + up sin(r1 360) r2` off the angles (0x8096fe7);
  - a guided one puts both trajectories `TR_INTERPOLATE` (0x80970af);
  - `missile.time` = `destabilizeDistance / speed 1000` (0x80970d1), the
    only use of `destabilizeDistance`; the flags take 0x20000 when
    `destabilizationRateTime` is 0, else the parent's 0x20000 (0x80970fb).
  - Javelin's start (guidance 3: the climb, 10 to the parent's right) is
    not on vcod; no stock MP weapon uses it. INFERRED (the weapon files).
- Trajectory step (0x8095928), `G_RunMissile`'s first step (called at
  0x8097322, before the move trace): moves a rocket (handler 9) whose
  `pos.trType` is not 2 once past its launch time; anything else is
  `BG_EvaluateTrajectory`. Under `timeToAccelerate` the forward speed
  grows by `speed / timeToAccelerate` per 0.05 s frame until it reaches
  the speed, then is set to it exactly; a curving weapon adds a frame of
  curvature; `trBase` steps a frame of `trDelta`; past `pos.trTime +
  timeToAccelerate` the angles follow `vectoangles(trDelta)` (both
  trajectories stay `TR_INTERPOLATE`). VERIFIED (the disassembly).
- Steering (inlined at 0x8095a95), guidance 1 (Sidewinder) and 2
  (Hellfire) with a target, past `pos.trTime + timeToAccelerate` (`tta +
  (now - trTime) -0.001 <= 0`, 0x8095c4f). VERIFIED (the disassembly):
  - Frame: flat heading `h` and speed `hs` of `trDelta`; the target's
    `r.currentOrigin` plus offset less `trBase` (`rel0` ahead, `rel1`
    left, `rel2` up). Turn radius to meet it `(rel0^2 + rel1^2) / 2 rel1`;
    tightest turn `speed^2 / maxSteeringAccel`.
  - Side push along `(h1, -h0)`, a frame of it. Ahead: none when the turn
    is tighter than the tightest, else `2 hs^2 / radius` within
    `maxSteeringAccel`. Behind: none within 60 of the tightest, else the
    full push to its side.
  - Vertical push (guidance 1, or a Hellfire past its climb) toward the
    height the flat run reaches it at: `(|hs d0| rel2 / |rel| -
    trDelta.z) 20` within `maxSteeringAccel`, none with the target behind.
    A Hellfire first climbs under `missileHellfireMaxSlope` (0.5) and
    `missileHellfireUpAccel` (1000) until it would climb no more
    (`pos.trDuration` 1).
  - Both pushes are computed off the velocity before either is added
    (0x8095e6b).
- No target health check: a dead body is still steered at; `G_FreeEntity`
  clears the handle (0x80cc1d7) and the rocket flies on unguided.
  INFERRED (control flow). vcod leaves the Hellfire dvars at those values.
  INFERRED (the code). `hind_FFAR_mp` and `cobra_FFAR_mp`: `Sidewinder`,
  `maxSteeringAccel` 2000, `timeToAccelerate` 1, `projectileCurvature` 10,
  speed 6000. VERIFIED (the weapon file).
  `hardpoint_tests::the_helicopter_guides_its_rocket` fires one through
  `_helicopter.gsc`'s `fire_missile("ffar", 1, target)` at an allied
  target 4000 ahead: it leaves at the body's speed, gains over 200 a frame
  to past 5000 in a second, flies as an untargeted one for that second,
  then steers and passes nearer. VERIFIED (the test, ours).
- Wobble and attractors (0x8097984). VERIFIED (the disassembly):
  - Run only when the move met nothing and the velocity is not nil, for a
    `projectile` weapon (+300 = 2) with no guidance and without flag
    0x20000; so a guided rocket never wobbles or bends.
  - Once `pos.trTime + (int)missile.time` is past, each of the three
    angles draws a rate: `G_flrand(-1, 1)` (0x80cb548) times
    `destabilizationCurvatureMax` the first time, else `G_flrand(0, 1)`
    times it; pitch negative when the last yaw rate was positive, yaw
    negative when the last pitch rate was. `pos.trTime` becomes the level
    time, `missile.time` `destabilizationRateTime 1000`, flag 0x10000 on.
  - While 0x10000 is on, every frame steps `apos.trBase` by a frame of the
    rates (`G_SetAngle`); velocity is their forward times the speed,
    `trBase` the missile's place, both trajectories `TR_INTERPOLATE`.
    Before the first draw none of this runs, attractors included
    (0x809872f).
  - Then each slot in use whose point is ahead along the velocity: the
    side offset's direction (straight down for a repulsor whose offset
    points up or is nil; an attractor dead ahead is skipped; the upward
    repulsor's `|side|` is then 0, 0x8097ce4); `s = (1 - d / reach)
    strength` within reach `d`; angle share `a = atan(|side| / ahead)
    0.63662`. A repulsor adds `(a - 1) s` along the side direction, an
    attractor `min(s a, 20 |side| speed / ahead)`. A frame of the sum
    bends the velocity, which is reset to the speed; its `vectoangles`
    goes to `apos.trBase` alone (0x8098782..0x809882d).
- `rpg_mp`: speed 1500, first wobble 200 ms out (`destabilizeDistance`
  300), then every 0.4 s. `_helicopter.gsc` puts an attractor on each
  helicopter (`scr_heli_attract_strength` 1000, `scr_heli_attract_range`
  4096). VERIFIED (the weapon file, the script, the test).
  `hardpoint_tests::the_helicopter_pulls_a_wide_rpg_in` fires an RPG 20
  degrees wide of the helicopter from 950 off, with the stock strength and
  with 0: the same flight to the first wobble (same draws off the level's
  `rand()`), then the attracted one passes over 20 nearer. VERIFIED (the
  test, ours). A retail capture of an RPG's two flights gates the rest:
  every point of the path matches, each wobble's `trDelta` a unit in the
  last place off (cod4-combat.md 15). VERIFIED (the gate).

### 3.3 The script traces

`builtins/cod4_trace.rs`: `bulletTrace`, `bulletTracePassed`,
`sightTracePassed` trace the clip map, its static models (point trace),
linked brush entities and, with `hitCharacters`, the bodies the last frame
linked, passing the ignored entity. INFERRED (the code). 1.7's handlers,
VERIFIED (the instructions):

- All three read `start`, `end` (vectors), `hitCharacters` (int),
  `ignoreEnt` (entity, else `ENTITYNUM_NONE` 1023, 0x80c665b). A 0
  `hitCharacters` drops 0x2000000 (`CONTENTS_BODY`) from the mask
  (`cmp`/`sbb`/`and 0xfe000000`, 0x80c6635).
- `bulletTrace` (0x80c65fa), `bulletTracePassed` (0x80bee92): mask
  0x2806831 (0x80c6640, 0x80beed8), the locational trace (0x80b4020) or
  its passed form (0x80b3f06), null priority map (0x80c6660, 0x80beef4).
  `sightTracePassed` (0x80c0198): mask 0x2801803 (0x80c01de),
  `G_SightTrace` (0x80b3ebe), true when its hit number is 0.
- `bulletTrace`'s array, in order: `fraction`; `position` (`start + (end -
  start) * fraction`, 0x80c66a2); `entity` (undefined for hit id 1022 or
  1023, 0x80c66f6); `normal`, `surfacetype`. With `fraction < 1`
  (0x80c6743) the trace plane and the name of `(surfaceFlags & 0x1f00000)
  >> 20` (0x80c6806); else `end - start` normalized (0 for a point) and
  `none`.

Surface names: `vcod_cod4::sound::SURFACES`;
`cod4_trace::tests::bullet_trace_fills_its_array_for_a_miss_and_a_hit`
checks keys and both arms. VERIFIED (the test). vcod, INFERRED (the code):

- Both bullet traces are `G_LocationalTrace`s under `bulletPriorityMap`
  (1.7 passes a null map; its effect in the hit-box walk is unread). A
  playing body scores its hit boxes as the last frame posed them
  (`combat::PosedBody4`, the shots' bone trace) and reads flesh; a
  helicopter its model collision (3.2).
- Sight, cone traces and `physicsTrace` meet a body as its link box.
- `getTagOrigin` / `getTagAngles` on a playing body read its posed bone,
  attachments included (`G_DObjGetWorldTagPos`); anything else the
  model's bind pose.
- The level writes bodies, poses and brushes after its entity pass
  (`trace_bodies`, `posed_bodies`, `trace_brushes`): threads read the last
  frame's.

Tests: `cod4::hardpoint_tests::bullet_traces_meet_posed_bodies_and_the_helicopter`,
`cod4_trace::tests::a_body_cuts_a_trace_that_hits_characters`.

### 3.4 `linkTo` on a model's tag

A non-client parent's tag is the bind-pose bone `getTagOrigin` reads
(`model_tags`, from the zones). Trap: CoD1's and CoD2's `linkTo` load
models through the `.iwd` path, which holds no CoD4 xmodel, so every tag
link to a script model failed ("failed to link entity since tag ... does
not exist"; on mp_backlot each destructible sedan's part sounds,
`_destructible.gsc` via `_utility::play_sound_on_tag`, 20 errors a `sab`
level). VERIFIED (the map sweep, `cod4-map-sweep.md`;
`cod4::gametype_tests::a_sound_links_to_a_destructible_cars_tag`). Retail
links them: the stock script's `getTagOrigin` a line above finds the tag.
INFERRED (the script).

### 3.5 The world's classname

`SP_worldspawn` (`cod4_lnxded` 0x80c81d2) ends writing the world entity:
`s.number` 0x3fe (0x80c83e0); `classname` `scr_const.worldspawn` (string
0x821792f, interned into 0x836f16c at 0x80a409a) via `Scr_SetString`
(0x80c83ea..0x80c83fc, gentity +0x170); `r.inuse` 1 (0x80c8401). VERIFIED
(disassembly). 1.1d writes `scr_const+0x9c` the same way
(`game.mp.i386.so` 0x61e84..0x61e91). VERIFIED (disassembly). CoD2's
(`cod2_lnxded` 0x811b56a) also ends with a `Scr_SetString` of a
`scr_const` slot into the world. INFERRED (the code names neither string).

A blast with no attacker (`sd.gsc` 807, `_destructible.gsc` 1064) passes
the world to the callbacks, and `Callback_PlayerKilled:4787` compares
`attacker.classname` with `"script_vehicle"`: an undefined classname
errors on every such kill (the sweep's `sd`, `sab` rows). vcod's world
(`ObjectTable::world`) carries `worldspawn` in every game. VERIFIED
(`cod4::gametype_tests::a_blast_with_no_attacker_names_the_world`).

### 3.6 Launched parts on the client

Addresses in this section are the 1.7 client's, `iw3mp.exe` (md5
`8c40cde102a9d8297a8674cb149f96ed`), read with `objdump`. The physics
world is ODE's; the names below are the strings' and ODE's.

Who flies a part:

- The server sends the launch once (3.2) and nothing after it. On the
  client, `CG_CalcEntityLerpPositions` (0x434fe0) hands an entity whose
  current `pos.trType` is 8 (`centity_t` +0x68) to 0x434e50, which returns
  for `eFlags` 0x20 (0x434e60), a brush model (`solid` 0xffffff,
  0x434e6d) or an entity with no DObj (0x434e9a). VERIFIED (disassembly).
- With no body yet it makes one (`CG_CreatePhysicsObject`, 0x434c70) when
  the time at 0x794460 is at most the launch's `pos.trTime` plus 1000
  (0x434ead..0x434ecf); later it marks the entity refused (-1 at
  `centity_t` +0x18). A refused entity, or one whose body failed, is drawn
  at its `trBase`s (0x434ede..0x434f08); a flying one at its body's
  interpolated state (0x434e00, world 1 through 0x5a6ec0). VERIFIED
  (disassembly). That 0x794460 is `cg.time` is INFERRED. So every client
  flies its own copy, and one that first sees a part more than a second
  after the launch draws it at the launch spot for good. INFERRED (the
  code above).
- A new model on the entity destroys the body (0x4351c7, 0x5a58c0).
  VERIFIED (disassembly).

The body:

- `CG_CreatePhysicsObject` takes the physics preset of the DObj's first
  model (`XModel` +0xd4); with none it prints "Failed to create physics
  object for '%s'.  No physics preset." (0x6c8344) and refuses. Else
  `Phys_ObjCreate` (0x5a4fb0) makes a body in world 1 at the current state's
  `pos.trBase` (`centity_t` +0x74), turned by `apos.trBase` (+0x98), at
  rest, with the preset's type (+4), mass (+8), bounce (+12) and friction
  (+16) (0x5a4fb0..0x5a5079); 0x5a5710 adds the model's collision; then
  0x5a5a20 pushes it with the contact `pos.trDelta` (+0x80), the force
  `apos.trDelta` (+0xa4) normalized (0x4076d0) as the direction, its length
  as the speed, and the preset's +20 as the scale. VERIFIED (disassembly).
- The collision (0x5a5710): with `physGeoms` (`XModel` +0xd8), each geom
  that has a brush (+0) a convex brush (0x5a5420), type 1 a box (0x5a52e0)
  of `halfLengths` (+0x38) at `offset` (+0x2c) along `orientation` (+8),
  any other type a cylinder off `halfLengths` x and y (0x5a55f0); the
  inertia is the list's `PhysMass` moments and products (list +0x14,
  +0x20) times the mass (0x5a49d0). With none, a box on the model's
  bounds (+0xac, +0xb8) at their midpoint (0x5a5240), a box flat on any
  axis taking -50..50 (0x5a587a, the floats at 0x70ba44 and 0x6be7e8).
  VERIFIED (disassembly). The body sits at the mass centre: 0x5a47c0
  moves it by the turned offset and the geoms back by it. INFERRED (the
  code). Stock car parts (`vehicle_80s_sedan1_*_door_lf` and kin) carry no
  `physGeoms`: their bounds box. VERIFIED (`phys_dump` on mp_backlot).
- The presets the stock cars launch: `car_door` mass 25, bounce 0.1,
  friction 0.5, scale 0.6; `car_hood` 15, 0.1, 0.5, 0.6; `tire` 18, 0.8,
  0.8, 0.6. VERIFIED (`phys_dump`, mp_backlot.ff).
- The push (0x5a5a20, its helper 0x5a5990), with `c` the body's position:
  the direction gets `phys_bulletUpBias` on z and is normalized; the
  contact moves out to `p + (p - c) * phys_bulletSpinScale`; with `r` that
  point less `c`, `L = |r x dir|` and `n` that cross normalized, the
  closing speed is `speed - (v . dir + w . n L)`, and nothing happens at or
  below 0. Else `J = closing * m * scale / (m + 0.5 + 0.5 m L^2 / (u . I u))`,
  `u` the body's rotation times `n` (0x5a5bc3..0x5a5c35: the rotation, not
  its transpose), `I` the body's inertia (its `dMass`, copied at
  0x5a5aab); `J dir` goes on at the moved point as a force times `1000 /
  msecStep` for one step (0x5a5900, `dBodyAddForceAtPos` 0x5b7000), which
  wakes the body. VERIFIED (disassembly).

The world (registrations 0x5a2d00..0x5a4420; a dvar's default the
immediate or float it is registered with):

| Dvar | Default | Read at |
|---|---|---|
| `phys_gravity` | -800 | 0x5a6565, as (0, 0, -800) |
| `phys_bulletUpBias` | 0.5 (0..2) | 0x5a599b |
| `phys_bulletSpinScale` | 3 (-1..100) | 0x5a59b0 |
| `phys_frictionScale` | 1 | 0x5a607f |
| `phys_dragLinear`, `phys_dragAngular` | 0.03, 0.5 | `cod4-player-anims.md` 10.2 |
| `phys_autoDisableLinear`, `Angular`, `Time` | 20, 1, 0.9 | 0x5a6505..0x5a6533 |
| `phys_contact_cfm`, `phys_contact_erp` | 1e-5, 0.8 | 0x5a6062 |
| `phys_mcv`, `phys_mcv_ragdoll` | 20, 1000 | 0x5a64c3..0x5a64d3 |
| `phys_csl` | 1 | 0x5a64d6 |
| `phys_collUseEntities`, `phys_interBodyCollision` | 0, 0 | |

VERIFIED (the registrations). Then:

- `Phys_RunFrame`'s clock (0x5a6e02..0x5a6e87): at most two steps a frame,
  each the larger of `msecStep` and half what is left, so the world runs
  up to a step ahead; the drawn state lerps from the frame's start to the
  last step. World 1's `msecStep` lies between 17 and 67 ms (the tables
  at 0x71e7a4 and 0x71e7b0), set each step off how many of its bodies are
  awake (0x5a6738..0x5a67bb). VERIFIED (disassembly). Its curve, the
  count less 32 over 18, is INFERRED: a handful of parts steps at 17.
- A contact (0x5a5f20): friction the two bodies' sum (the world adds 0)
  times `phys_frictionScale`, bounce the smaller of 1 and each body's
  (0x409400), `bounce_vel` 0.1 (0x6b72dc), the contact `erp` and `cfm`,
  mode 0x301c. VERIFIED (disassembly). That 0x301c is ODE's bounce, soft
  ERP and CFM and the friction pyramid is INFERRED (ODE's flags).
- The bodies meet the world under 0x2806c91, the ragdoll's mask: each
  step traces a body's last position to its new one (0x5a6acf). VERIFIED
  (the immediate). With `phys_collUseEntities` and
  `phys_interBodyCollision` 0, parts meet the world alone. INFERRED (the
  names).
- `physicsExplosionSphere`'s event calls `Ragdoll_ExplosionEvent` and
  0x48e670, among the dynamic entities' code (0x437241, 0x437284).
  Whether a blast pushes a launched part is not read. INFERRED.

vcod (`vcod_cod4::physics`, `play::cod4_physics`):

- Ported: the 1000 ms rule, the refusal without a preset, a brush model
  and `eFlags` 0x20 left out, the shape (boxes and cylinders, else the
  bounds box), the mass centre, the push with its up bias, spin scale and
  formula, gravity, the clock (two steps at most, the lerp), the drag,
  the per-body sleep on the squared speeds, the contact friction and
  bounce, the world-only collision under 0x2806c91. VERIFIED (the code
  against the above).
- In place of ODE: one impulse solver per body. Each box corner and 16
  points round each cylinder sweep the clip map as points along their
  velocity over the step; the kernels' 0.125 skin is the contact layer; a
  corner already in the world is traced out from the centre and pushed
  out at `phys_contact_erp` up to `phys_mcv`; an approach faster than 0.1
  bounces by the preset, from the corner where the step found it; ten
  passes of sequential impulses with Coulomb friction (the preset's times
  `phys_frictionScale`); the centre itself is never moved through a
  surface. A brush geom is left out (a model with only brushes takes its
  bounds box), `msecStep` stays 17 ms, a world edge between two corners
  can poke into a box, and a blast does not push a part. vcod's.
- Tests: `physics::tests` (free fall at `phys_gravity`, the push formula,
  a 0.8 bounce, a launched door that lands and sleeps, a fast part thrown
  into a corner staying out of all three planes, a tilted box settling
  flat, the clock, a model's geoms) and the live
  `cod4_live_a_blown_up_cars_parts_fly_and_come_to_rest`: vcod-server's
  `war` on mp_backlot, a held bot's frags bursting on the first
  destructible until `_destructible.gsc` blows the sedan up; its 9
  launched parts (both mirrors, both bumpers, both right doors, two wheels
  and the hood) flew 14 to 199 units off their spots and went to sleep
  within 1.6 to 2.7 s, every point clear of the world and resting on it.
  VERIFIED (the tests, ours). How a part looks in flight and at rest is
  pending a human's look.
- The ragdoll (`cod4-player-anims.md` 10) keeps its point solver. This
  body could carry it once the bones are rigid capsules held by joint
  constraints, which it lacks. INFERRED (vcod's design).

## 4. The load

- Zones: `code_post_gfx_mp`, `common_mp`, then the map's: the three names
  the capture's `sv_referencedFFNames` lists. VERIFIED. The iwds,
  `main/*.iwd`, sit under them (`Cod4Source`).
- `Cod4Scripts` reads a loose `.gsc` from an override directory first,
  then the zones' rawfile. On retail a loose file wins only under
  `fs_game`. INFERRED, unconfirmed on 1.7; the directory stands for that.
- Bootstrap: the gametype's `main`, the map's `main`, then
  `CodeCallback_StartGameType`, each to its first wait, as CoD2's.
  INFERRED.
- `InitStructs` runs at every load, so `level.struct` is always an array:
  mp_crash has no `script_struct` and `common_scripts/utility.gsc`'s
  `struct_class_init` reads `level.struct.size`. INFERRED from
  `G_LoadStructs` and the corpus (`init_structs_always`).
- The start-up `exec default_mp.cfg` (string in the binary, VERIFIED) runs
  `iw_00.iwd`'s `default_mp_gamesettings.cfg`, which sets
  `scr_war_scorelimit 750`: the capture's `ui_scorelimit` is 750 where
  `war.gsc` defaults to 500. VERIFIED. `cod4_boot` runs that file ahead of
  the load; vcod-server's start-up runs it as retail does.

### 4.1 `wait` counts frames

- A `wait` resumes on a whole 50 ms frame: seconds times 20 rounded half
  up; under half a frame (not `wait 0`) is one frame. Retail `cod4_lnxded`
  1.7 (mp_crash, `war`, one idle probe client, a scratch `logPrint` probe
  over `fs_game`), threads started on one frame reading `getTime()`. VERIFIED (the log):

  | `wait` | `getTime()` after |
  |---|---|
  | 0.6, 0.6001, 0.62, 0.624 | 600 |
  | 0.626, 0.643, 0.649, 0.65, 0.651 | 650 |
  | 0.01, 0.024, 0.026, 0.0001 | 50 |
  | 0 | 0 |
  | 1.0249, 1.025 (a float a hair under) | 1000 |

  CoD1 rounds to the millisecond and runs the thread on the first frame
  past it, which put 0.643 a frame early. `crates/gsc/tests/cod4_waits.rs`
  runs the probe's waits through `VmProfile::COD4` against these lines and
  fails with `frame_waits` off. VERIFIED (the gate, ours).
- A script mover's `origin` read after such a wait is the trajectory at
  that frame's time: a `moveTo` 10000 units in 1 s read 5500 at 600 and
  6000 at 650; a `moveGravity` (1000, 0, 0) from z 5000 read x 800 z 4744
  at 850, x 950 z 4639 at 1000. VERIFIED (the log). vcod-server reads the
  same (`VmProfile::COD4.frame_waits`, gate
  `vm::sched::tests::cod4_waits_count_whole_frames`).
- The airstrike times its bomb by these waits (`callStrike_bombEffect`'s
  `wait (bombTime - 1.0)`, 2.643 s: 53 frames, not 2643 ms). A frame late,
  the bomb opened 100 to 250 units past its mark and the bomblets'
  `losRadiusDamage` found nobody. On frames they kill the player at the
  target (`cod4_live_three_kills_earn_the_airstrike_and_call_it_in`). That
  gate no longer tells the two waits apart: with the script draws reseeded
  at the hardpoint key it kills or misses alike on each of 12 seeds with
  `frame_waits` on and off, so the hit is up to the draws. VERIFIED (the
  gate, ours).

### 4.2 An `endon` belongs to its call

- Retail `cod4_lnxded` 1.7 (mp_crash, `war`, no client, scratch
  `logPrint` probe `probe_endon_return` over `fs_game`). VERIFIED (the
  log):
  - A function that runs `level endon("x")` and returns leaves nothing:
    `x` notified afterwards does not end its caller
    (`after_return_survived 1`).
  - `x` notified while that function waits in a `waittill` ends the
    function alone: the caller resumes in the same frame (notify
    `getTime()` 1900, caller's line 1900) and the function's lines after
    the `waittill` never run (`inside_outer_ran h2=0 t=1900`).
  - The stock airstrike's chain (a called function's `endon`, notified by
    a thread it started 0.05 s after it returned) leaves the caller
    running (`chain_survived 1`).
- That the caller reads the value as `undefined` is INFERRED.
- That fits a child thread per call (`AllocChildThread` on every script
  call; `Scr_KillThread` on return drops the endons registered on it).
  INFERRED.
- vcod: `VmProfile::COD4.endons_end_with_call`
  (`Vm::set_endons_end_with_call`). An endon records the depth of the call
  that ran it; a return drops its call's; a notify of one at depth `d > 1`
  truncates the parked thread to its caller, pushes `undefined` and makes
  it runnable; one in the thread's own body still ends the thread. A
  thread mid-step under a nested spawn's notify is still ended whole. CoD1
  and CoD2 keep thread-wide endons. Gate:
  `vm::sched::tests::cod4_endons_end_with_their_call`.
- Trap if reverted: `hardpointItemWaiter` died once an airstrike was
  called (`selectAirstrikeLocation`'s `self endon("stop_location_selection")`
  stayed on the waiter, and `finishAirstrikeUsage` notified it 50 ms
  later), so the next hardpoint's radio stayed in hand until the next
  spawn; an Esc out of the selector did the same. Gate:
  `play::cod4::tests::cod4_live_airstrike_then_helicopter_in_one_life`.
  VERIFIED (ours).

## 5. Configstrings

- `CS_LAYOUT_COD4` (`game::cod4`) is the `CS_*` enum. Every slot the
  capture fills sits where it says. VERIFIED (capture).

  | Slot | Content | Slot | Content |
  |---|---|---|---|
  | 9 | fog | 309 | localized |
  | 10 | motd | 830 | models |
  | 11 | game end time | 1342 | sound aliases |
  | 12 | map centre | 1598 | effects |
  | 13 | vote | 1698 | effect tags |
  | 19 | winner | 1954 | shellshocks |
  | 20..147 | dvar mirror names | 1970 | menus |
  | 148..275 | dvar mirror values | 2002 | materials |
  | 277 | hint strings | 2258 | weapon files |
  | 821 | ambient | 2259 | status icons |
  | 822 | north yaw | 2267 | head icons |
  | 823 | minimap | 2282 | tags |
  | 824, 825 | vision sets | 2314 | items |

  The worldspawn's message is slot 3 and the ambient track 821
  (`SpawnRules::message_slot`, `ambient_slot`), in CoD1's
  `n\ambient_crash\t\0` format. VERIFIED (capture).
- The table starts empty and the gamestate omits a slot equal to its
  `constantConfigStrings` default (`cod4-handshake.md` 5). A new name
  whose constant slot lies in its range takes that slot; any other takes
  the first empty slot no constant names, then the first empty one; a name
  is found anywhere in its range. Menus and icons scan from the base for
  the first empty slot. The rule is `G_FindConfigstringIndex`'s, INFERRED
  for 1.7. It places every name where the capture has it: shellshocks
  `default` and `concussion_grenade_mp` on constants 1968 and 1969 but
  `flashbang` on 1955 (its constant 1970 is a menu slot), localized strings
  around 350 and 351, weapon models on their constants, menus sequential
  from 1970 with `scoreboard` on 1977, not its constant 1986. VERIFIED
  (`cod4_boot`). Names from 821 on are stored lowercased
  (`smoke/battlefield_smokebank_S` reads `..._s`). VERIFIED (capture).
  vcod's `Allocators::with_constants` is this rule.
- `cod4_boot` compares slots 2, 9, 11, 12, 19 and 309..2314 one for one
  with the capture after 30 s of an empty server (the prematch's 15 s
  `scr_game_playerwaittime` and 5 s `scr_game_matchstarttime` end at 20 s,
  setting 11 to `619800` and 824 to `"mp_crash" 2000`): all equal, `GAPS`
  empty. VERIFIED. The dvar mirror's `scr_*` and `ui_*` dvars compare
  equal by name. VERIFIED. The engine's own mirrored dvars (`friction`,
  `g_TeamColor_*`, `jump_height`, `player_*`, ...) are not registered on
  `cod4_boot`'s bare host, so its sorted slots differ from retail's;
  vcod-server registers them (`cod4::oob`) and `cod4::oob_ab` compares the
  mirror with retail's.

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
and killed callbacks take CoD2's extra arguments; `HostProfile::COD4` sets
`callback_extra_args`.

`modNames` registers `MOD_UNKNOWN` .. `MOD_EXPLOSIVE` and `MOD_IMPACT`
(16) from 0x80a4109, in string-pool order. INFERRED
(`game::cod4::COD4_DEFS`).

## 7. The connect, the menus and the spawn

`vcod-server --game cod4` runs the level (`game::cod4::level::Cod4Level`)
and hands it each client: `ClientConnect` when the connect is accepted,
`ClientBegin` on the first move, the `mr` answers, the spawns. Gate:
`cod4::wire_ab` (`COD4_DIR`). The three probes of `play-1.7-walk.txt` and
`play-1.7-spec.txt` (cod4-netfields.md 8) join ours in process on retail's
level clock; it diffs the gamestate, both clients' server commands up to
52 s, their chat lines and the walker's spawn frame. Every command the two
retail clients read up to the cut matches, in order; the rest is the
gate's `GAPS`.

### Server commands

- The connect callback's two `v` lines go out before the gamestate (the
  walker's commands 1 and 2, `v ui_3dwaypointtext "1"`, `v
  ui_deathicontext "1"`), the rest after `ClientBegin` in the first
  snapshot. VERIFIED live. The callback runs inside `ClientConnect`.
  INFERRED.
- `iPrintLn(&"MP_CONNECTED", self)` reaches the clients as `
  "MP_CONNECTED\x15walker^7"`: the command letter is a space. VERIFIED
  live. vcod's shared message packer writes that form.
- Commands the connect, menus and spawn send, as captured: `v name
  "value"`, one `v` with several pairs, `H 0` then `G 0`, `t <menu>`, `u`,
  `L`, `J "1"`, `a 27`, `C 40`, `s 1` (the alias at configstring 1343), `d
  <slot> <string>`. VERIFIED live. Builtins behind them, by the stock
  scripts' call order, INFERRED: `setClientDvar`, `setClientDvars`,
  `updateScores` (allies then axis), `openMenu`, `closeMenu`,
  `closeInGameMenu`, a `giveWeapon` that gives, `setSpawnWeapon`, the
  equipped offhand, `playLocalSound`, a configstring write. `setTeamScore`
  sends `H` or `G` to everyone and writes no configstring. INFERRED.
- Type by letter: `J`, `G`, `H`, `I`, the messages `c`, `e`, `f`, `g`, the
  chat `h`, `i` and the letterless print are `SV_CMD_CAN_IGNORE`; the rest
  `SV_CMD_RELIABLE`. A client not yet in the world loses its pending
  ignorable commands and gets no new one. INFERRED. The spawn loadout's
  four `J "1"` in a row are never squashed. VERIFIED live.
- `SV_AddServerCommand` squashes a reliable command into the first pending
  unsent reliable one with its letter that has the same text, or is one of
  `C D a b o p q r t`, or is a `d`/`v` with the same first word: the old
  one is removed, the new one goes last. INFERRED. The capture shows it
  twice: `_menus.gsc` and `menuClass` each send `u` `L` on the class answer
  and the walker reads one `u` `L`; the frag's give sends `C 40` before
  its `J "1"`, `switchToOffhand` sends it again after, and the walker reads
  `J "1"`, `C 40`. VERIFIED live.
- The rings hold 128 commands (`& 0x7f`). INFERRED. The walker gets 74 in
  26 s, past CoD1's 64. VERIFIED live. A 64-command ring keys a message
  past the 64th command off the wrong text and garbles it.
- Sound aliases keep their case in configstrings (`d 1344
  US_1mc_team_deathmtch`), unlike other names from 821 on. VERIFIED live.
- The colour dvars are every `g_ScoresColor_*` and `g_TeamColor_*`:
  `_scoreboard.gsc`'s three-component `setdvar` reads back four byte
  components (`0.6 0.639216 0.690196 0` for `0.6 0.64 0.69`) in
  `handshake-1.7.txt`'s mirror. VERIFIED.
- Chat colours the speaker's name by the recipient's side: the walker's
  `say` reads `h "\x15\x15^8walker^7: ^7vcod probe here"` to itself and
  `^9walker` to the spectator. VERIFIED live. The rule is `G_SayTo`'s `^8`
  on the speaker's team, `^9` off it. INFERRED.
- `waittill` order: the mate connects a frame before the spectator. With
  both in one frame the spectator reads `v ui_xpText` (`_persistence`)
  before `_rank`'s unlock dvars; retail's reads it after them, the
  walker's (third connect) before. VERIFIED live. The waiters of a looping
  `level waittill("connected")` alternate with each notify, as vcod's
  newest-first resume gives. INFERRED.

### The start-up configs

- The dedicated server execs `default_mp.cfg` and its `exec`s from its
  iwds before any zone loads (console: `execing default_mp.cfg from disk`,
  `execing default_mp_gamesettings.cfg from disk`, `Loading fastfile
  code_post_gfx_mp`). VERIFIED live.
- `iw_00.iwd` holds two `default_mp_gamesettings.cfg`: 4128 bytes dated
  2007-09-13 at offset 11981 (`scr_game_playerwaittime 20`,
  `scr_game_matchstarttime 5`) and 4090 bytes dated 2007-08-23 at offset
  56739 (`scr_game_graceperiod 0` in their place). VERIFIED. Retail reads
  the later one: rcon on a fresh `war` server reads
  `scr_game_playerwaittime` "15" default "15" (the script's tweakable) and
  `scr_war_scorelimit` "750", and the start-up dvar dump has
  `scr_game_graceperiod` and no `scr_game_playerwaittime`. VERIFIED live.
  The prematch then ends at 19800 and configstring 11 reads 619800, as on
  every 1.7 capture. VERIFIED live. vcod reads the cfgs through `Pk3Fs`,
  which also keeps the later entry.

### The stats, the loadout, the perks

- `getStat` / `setStat` use the slot's own 0x2000-byte block, the one its
  seven `stats` packets carried, a fresh profile's until they are in.
  INFERRED (`_class.gsc` reads the class stats off it).
- `setStat` is `GScr_SetStat` (0x80c5490). VERIFIED (the disassembly):
  - Checks, in order, each `Scr_Error` (0x815e9d4): a receiver with no
    client (`setstat: entity must be a player entity`, 0x821ee1c); the
    impure flag (0x8a3ce48, read by 0x814ddda: `setstat: impure scripts
    loaded.`, 0x821efdc); `developer_script` on (`Dvar_GetBool` 0x819e090
    at 0x80c54e4: `setstat: developer_script is false.`, 0x821effc, the
    message naming the opposite of what fails); two arguments (`setstat:
    takes 2 arguments, got %i.`, 0x821f068); index <= 3499 unsigned
    (`setstat: invalid index %i`, 0x821c577); a byte stat (index up to
    1999) <= 255 unsigned (`setstat: index %i is a byte value, and you're
    trying to set it to %i`, 0x821f020).
  - Then `SV_SetClientStat` (0x816e712): a byte stat is compared and
    stored as a byte, an int stat (2000..3497) as a word, 3498 and 3499
    dropped; a changed stat goes to the client as the reliable `N <index>
    <value>` (`SV_SendServerCommand(cl, 1, "%c %i %i", 0x4e, ...)`, format
    at 0x8217f78).
- The impure flag: `Scr_BeginLoadScripts` clears it (0x814c337 calls the
  setter 0x814ddcc with 0). `Scr_ReadFile` (0x814fc52 on) reads a script
  through `Scr_ReadFile_LoadObj` (0x814e13e) when `fs_game` (0x13f9cc18)
  is empty and `useFastFile` (0x88a53f0, default 1) is off, else from the
  fastfile; with `fs_game` set it tries the loose file first. Each loose
  read ORs in `fs_game` being empty (0x814e17e, 0x814fcfb), so only a
  loose script under no mod with `useFastFile 0` sets it. VERIFIED (the
  disassembly). `developer_script` registers off with flags 0x10
  (0x8121e47). VERIFIED. vcod reads every script from the zones, so it has
  no impure check; it has the dvar's refusal (`stat_tests`).
- `getStat` is `GScr_GetStat` (0x80c55f6): `Scr_GetInt(0)` first, then
  `Scr_Error`s `getstat: entity must be a player entity` (0x821f090) for
  no client, `getstat: invalid index %i` (0x821c591) past 3499 unsigned,
  `getstat: called on a disconnected player` (0x821f0b8) when
  `sess.connected` (client +0x2f8c) <= 0. `SV_GetClientStat` (0x816de58)
  reads a byte under 2000, a word to 3497, 0 for 3498 and 3499. VERIFIED
  (the disassembly). vcod treats a slot whose roster entry
  `ClientDisconnect` cleared as disconnected. The client stores `N` into
  its stats buffer (`iw3mp.exe` 0x44c42d calling 0x579c70;
  cod4-netfields.md, "The stats file"). VERIFIED (the disassembly).
  `cod4_live_set_stat_reaches_the_clients_profile` joins a ranked (`sv_pure
  1`) mp_crash: the rank scripts move stats 2317 and 2352 (`maxxp`, 30)
  off a fresh profile's, and the client's block ends equal to the
  server's.
- Ranks. `_globallogic.gsc` sets `level.rankedMatch` to `level.onlineGame
  && getDvarInt("sv_pure")` (`onlineGame` is `true`), or 1 under
  `scr_forcerankedmatch 1`; `_persistence.gsc`'s `statSet` and `_rank.gsc`'s
  `incRankXP` do nothing outside a ranked match. `giveRankXP` also wants a
  player on each team (two players in a free-for-all). `updateRank` walks
  each rank passed: `statSet`s `rank` (2350), `minxp` (2351), `maxxp`
  (2352), `setStat(252, rank)`, then the rank's unlocks off
  `mp/ranktable.csv`. VERIFIED (the rawfiles, `mp/playerstatstable.csv`).
  `stat_tests::cod4_ranked_xp_unlocks_create_a_class`: 300 XP off a fresh
  profile reach rank 3, whose `feature_cac` sets stat 260 to 2, each
  write sent as `N`.
- `SV_SetClientStat` writes the client's own block, which a
  `fast_restart` keeps, so the restarted level's `_rank.gsc` reads the
  earned XP back. INFERRED (one block per client, `SV_GetClientStat` and
  `SV_SetClientStat` both off the client). vcod's level holds the scripts'
  copy, carried across the restart (`Cod4Server::map_restart`;
  `stat_tests::cod4_fast_restart_keeps_the_scripts_stats`). A map change
  asks the client for its block again (`requeststats`), which holds every
  `N` (cod4-netfields.md, "The stats file").
- Loadout `WeaponDef` fields come from the zone at the offsets of 1.7's
  `weaponDefFields` (`cod4_lnxded` 0x826a160, 502 rows;
  `tools/re/cod4_weapon_fields.py`): `weaponClass` +304, `inventoryType`
  +316, `offhandClass` +324, `startAmmo` +800, `maxAmmo` +820, `clipSize`
  +824, `sharedAmmoCap` +840, `moveSpeedScale` +1048, `clipOnly` +1280,
  `dropAmmoMin` +1348, `dropAmmoMax` +1352. VERIFIED (the table). The
  `weaponClass` names sit in enum order in the ten-pointer table at
  0x826b900. VERIFIED (the table).
- Ammo and clip slots: weapon 0's empty names hold slot 0; a weapon takes
  the slot of the first earlier one with its name (cod4-hud.md 9). The
  spawn frame reads `ammo[4]` 60 and `ammo[5]` 1 (`m16_gl_mp` and its
  `gl_m16_mp`), `ammo[9]` 30 (`beretta_mp`), clips 11 (15), 27 (30), 28
  (1), 40 (1, frag), 43 (1, stun grenade), from `giveWeapon`'s start ammo
  and `setWeaponAmmoClip`. VERIFIED live. The start ammo path is
  `G_InitializeAmmo`, `Add_Ammo`, `Fill_Clip`. INFERRED.
- `ps.weapons` holds a bit per weapon (the spawn's 11, 27, 28, 40, 43),
  `ps.weaponold` the spawn weapon's (`setSpawnWeapon`). VERIFIED live.
- `setActionSlot`: `nightvision` 3, `altMode` 2 in `actionSlotType`.
  VERIFIED live. `weapon` is 1, `""` 0. INFERRED.
- `bg_perkNames`, the `perks` bit order, is the 20-row pointer table at
  0x820de40 (`specialty_gpsjammer` .. `specialty_specialgrenade`).
  VERIFIED. The assault class's `perks` read 0x4400
  (`specialty_longersprint`, `specialty_bulletdamage`), and the roster
  carries the same word. VERIFIED live.
- `setMoveSpeedScale(0.95)` (a rifle class) is
  `moveSpeedScaleMultiplier`. VERIFIED live.
- `disableWeapons` sets `weapFlags` 0x80. INFERRED.

### The perks

Each perk's bit in `perks` (its `bg_perkNames` row), where 1.7 acts on it,
and ours. The engine sites are the ten tests of the playerstate's `perks`
word (gclient+0x5fc, the playerstate's own offset) in `cod4_lnxded`, at
the table's addresses. VERIFIED. What each gates is read off the dvar
loaded beside it. INFERRED. The script sites are `hasPerk` in the stock
`common_mp` gsc (`_class.gsc`'s `cac_modified_damage` and loadout,
`_globallogic.gsc`'s Last Stand). VERIFIED. Tests: `cod4::perk_tests::` in
`vcod-server`.

| perk | bit | 1.7 | ours | test |
|---|---|---|---|---|
| UAV Jammer (`gpsjammer`) | 0x1 | client compass; the script's radar | works (radar) | none here |
| Steady Aim (`bulletaccuracy`) | 0x2 | hip spread times `perk_weapSpreadMultiplier` 0.65 (0x805f64e) | works | `steady_aim_narrows_the_hip_spread` |
| Sleight of Hand (`fastreload`) | 0x4 | reload time over `perk_weapReloadMultiplier` 0.5 (0x80636cb) | works (pmove; the view clips, cod4-client-view.md 4.7) | `sleight_of_hand_halves_the_reload` |
| Double Tap (`rof`) | 0x8 | fire and rechamber time over `perk_weapRateMultiplier` 0.75 (0x8063926) | works (pmove, retail's rate, cod4-combat.md 2.1; the rechamber clip, cod4-client-view.md 4.7) | `double_tap_fires_faster`, `cod4_fire_rate_ab` |
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
(201 primary, 203 secondary, 205..207 perks, 208 special grenade, each a
`mp/statsTable.csv` number). VERIFIED (the stock gsc).

### The spawn and the end frame

- `ClientSpawn` ends in its own think, before the script's loadout lands:
  the spawn frame's ground, `holdBreathScale` and `pm_flags` are that
  think's (cod4-pmove.md 15). The spot is `placeSpawnPoint`'s drop, a bit
  under retail's on mp_crash's terrain (same section), one of
  `cod4::wire_ab`'s `GAPS`.
- The walker's spawn frame (serverTime 47900). VERIFIED live: `commandTime`
  the frame's time; the spot (pinned off the capture, since the script
  draws it from a clock-seeded `randomInt`); the view exactly the spot's
  45; `delta_angles` 110.00061; `eFlags` 2; `viewHeightCurrent` and
  `viewHeightTarget` 60; `dofNearBlur` 6, `dofFarBlur` 1.8;
  `throwBackGrenadeOwner` and `viewlocked_entNum` 1023; `speed` 190;
  `gravity` 800; `otherFlags` 4; `viewmodelIndex` 364; `stats` 100 0 100
  63 3. Ours matches all of them.
- `ClientSpawn` (around 0x80a7c5d) keeps `eFlags` 0x100002 (0x80a7c63) and
  flips 0x2 (0x80a7d68), clears the rest of the playerstate (memset at
  0x80a7cc9), steps the spawn count in `stats[4]` (0x80a7cfe; the walker's
  third spawn: the connect's spectator, the team join's, the class's), and
  writes the box (-15,-15,0)..(15,15,70) (rodata 0x820e024, stored at
  0x80a7c24..0x80a7c5a), view height 60 and 60.0, blurs 6 and 1.8, the two
  1023s (0x80a7d2b..0x80a7dbc). VERIFIED (the instructions). What it leaves
  is the spawn frame above.
- A follower's cmds are not kept: the spawn's `delta_angles` 110.00061 is
  the spot's 45 less the -65 the walker sent before it followed its
  teammate. VERIFIED live. `cod4_probe`'s cmds while following are its
  held view less the copied `delta_angles` (-69.99), +70, which the spawn
  did not take. INFERRED (the probe's code).
- `SpectatorClientEndFrame`: a client on a team with no class may not fly
  free (`allowSpectateTeam("freelook", false)`), so it follows the first
  playing teammate (`Cmd_FollowCycle_f`, INFERRED). The walker's
  playerstate is the mate's from the mate's spawn (46800) to its own:
  `clientNum` 0, `otherFlags` 0xa, its own `eFlags` 0x100000 kept.
  VERIFIED live. Before the mate spawns it floats with `otherFlags` 8.
  VERIFIED live. The copy is the mate's playerstate as the last frame
  left it, the newest archived frame. INFERRED.
- `CheckTeamStatus` writes `stats[3]` -1 (63 on the wire) for every client
  not following, each frame: the spawn frame reads 63. VERIFIED live.
- A playing body's entity (`BG_PlayerStateToEntityState`): origin rounded
  (the mate's z 236.125 reads 236), the view yaw, `pos.trType` 3,
  `apos.trType` 1, the weapon, `solid` 0x66010f. VERIFIED live (one
  standing teammate). The `solid` packing is `SV_LinkEntity`'s `x | (1 -
  mins.z) << 8 | (maxs.z + 32) << 16` over the body's box. INFERRED.
- `hidePart(tag)` sets bit `0x80000000 >> (i % 32)` of `partBits[i / 32]`
  for the tag's index `i` in the model's bone list: the
  `_destructible.gsc` cars' ten hidden parts read the walk capture's
  0x242a, 0x92480000 in baselines and snapshots. VERIFIED live.
- A trigger's `eFlags` read 0x21 in the baselines (mp_crash's entities 88,
  344 .. 353). VERIFIED live. The bits are `InitTrigger`'s 1 and the
  nodraw 0x20 of a model-less trigger. INFERRED.
- The destructible cars' baselines round the map's places as the writers
  do: `58 2112.5 309.1` goes out as 2113 (the origin writer's `floor(f +
  0.5f)`, cod4-netfields.md 2), yaw `101.765` as 101.7664 (the angle
  writer's half short). VERIFIED (`cod4::wire_ab`).

### Open

Nothing left. 1.7 sends no PVS cull (cod4-pmove.md 15); pmove writes
`holdBreathScale` and `groundEntityNum` (`vcod_cod4::pmove`), the anim
script `legsAnim` (cod4-player-anims.md 7).

## 8. The stock gametypes on a running level

What `dom`, `sd`, `sab` and `koth` read off the engine past the spawn.
Gates: `cod4::gametype_tests` (`COD4_DIR`) and the bot soak in
`docs/bots.md`.

- A brush entity's box is its inline model's (`cmodel_t`'s `mins`,
  `maxs`, the clip map's `Submodel`); `isTouching` and the touch pass test
  a box against that model's brushes. `SV_SetBrushModel` taking
  `CM_ModelBounds`: INFERRED. vcod's level carries both tables from the
  clip map (`game::cod4::brush_models`). Trap: without them no brush
  trigger touches anything, `koth::SetupRadios` finds every radio outside
  its `radiotrigger` and `AbortLevel` loads `dm`. mp_crash's radios are
  `script_model`s whose point origins lie inside the `trigger_multiple`
  brushes `*24` .. `*27`. VERIFIED (`koth_on_mp_crash_finds_its_radios`,
  our clip map read off the zone).
- `objective_state` is 0x80c210a: an index above 15 is an error; the state
  string is compared with four script-string constants, which store 0, 2,
  4 and 1. VERIFIED (the disassembly). The first two (0 and 2) also detach
  the record from its entity (`0x3ff`). INFERRED (the branch to
  0x80c21c7). The names, in the order of the error string at 0x821e6f4
  (`"empty", "invisible", "current", "active"`), give `active` 1. INFERRED
  (`ObjectiveStateIndexFromString`). `_gameobjects::updateCompassIcon`
  sets `active` on every flag, bomb and radio, so 1.1's reading (no
  `active`) raised a script error on each.
- `precacheLocationSelector` is 0x80bc4a6: it compares the name with
  configstrings 827 .. 829 (0x33b .. 0x33d); a match goes to the print at
  0x80bc5f2 with the string at 0x821d6d4 and returns. VERIFIED (the
  disassembly). That print (0x8122d74) writes only with `developer` (named
  at 0x82232e9) set. INFERRED (its early return on the dvar). A fourth
  name is the script error at 0x821d718. VERIFIED. A `fast_restart` keeps
  the configstrings, so every `sd` and `sab` round's `_hardpoints::init`
  meets its own selector there.
- `koth` has no start spawns. A player spawning in a level's first frames
  reads `numPlayersAtLastUpdate` and `distSum` off spawn points
  `_spawnlogic::spawnPerFrameUpdate` has not reached (one a frame from
  `startGame`), so `getSpawnpoint_NearTeam:793` errors and leaves the
  weight undefined; `spawnWeightDebug`, which runs while
  `scr_spawnpointdebug` is not `"0"` (only a `/#` block sets it), errors
  on that weight every 50 ms. INFERRED (the rawfiles).
- Retail does the same. Recipe: `cod4_lnxded` 1.7, `koth` on mp_crash
  under an `fs_game` whose `_spawnlogic.gsc` (stock plus a `logPrint`
  atop `getSpawnpoint_NearTeam`) logs the unset spawn points and whose
  `mp_crash_fx.gsc` adds a menu-answering test client at 200 ms. It
  spawns at 350 ms with 16 of the 19 `mp_tdm_spawn`s unset, and the
  comparison is a "script runtime error" ("pair 'undefined' and '0' has
  unmatching types", stock line 793) that ends the level and shuts the
  server down, at `developer 0` as at 1 (`logfile 2`). VERIFIED (the
  capture's console, rerun once). It ends the level because it falls
  while the level still loads (8.1). A loading client takes longer than
  the 19 frames: retail's two probes joining 44 s past the load spawned
  with none unset. VERIFIED (the same run without the test client).
  vcod-server's bots enter a fresh level 3 s past its first frame
  (`bots::cod4::BOT_LOAD_MS`), so their first koth spawns run clean
  (`cod4::bots::tests::cod4_bots_spawn_clean_on_a_fresh_koth_level`).
  VERIFIED (the test, ours).

### 8.1 Which runtime errors end the level

`cod4_lnxded` 1.7. VERIFIED (the disassembly; dvar names at 0x82232e9,
0x82232f3, 0x821c566):

- `Com_SetScriptSettings` (0x8122106) hands the VM three flags
  (0x815cf70): `developer || logfile` at 0x8be4106, `developer_script` at
  0x8be4107, `developer` at 0x8c055b5.
- `Scr_SetLoading` (0x815cf9a) stores its argument at 0x8c098f4;
  `G_InitGame` sets it (0x80b59cc) and the server's frame loop clears it
  after each frame it runs (0x8177366).
- The error entry at 0x815e906 (copies the message into a 1024-byte
  buffer at 0x8c0b920, `Scr_SetErrorMessage`) marks the error terminal
  (0x8c055b6) when the first flag is set and the VM is loading
  (0x815e9b6).
- `RuntimeError` (0x814f108) returns silently with neither the first flag
  nor a terminal error. Otherwise it prints `******* script runtime error
  *******` (0x822b75c) and, with `developer` set or the error terminal,
  `Com_Error`s with `script runtime error (see console for details)`
  (0x822b788, 0x814f24d), which shuts the server down; else the thread
  runs on.

So a level ends on a runtime error only at `developer 1`, or with
`developer` or `logfile` set while the level loads (its spawn's frames and
the server's first frame after them). Retail 1.7, `war` on mp_crash under
an `fs_game` whose `mp_crash_fx.gsc` runs `if (undefined > 0)` at a set
level time (scratch `run_err.sh`, server alone, 15 s):

| when | developer | logfile | the level |
|---|---|---|---|
| 0, 200, ..., each frame | 0 | 2 | shut down at the first (time 0) |
| each frame from 0 | 0 | 0 | ran on, no error printed, the `if` taken |
| first frame at or past 350 (350) | 0 | 2 | shut down |
| first frame at or past 400 (400) | 0 | 2 | ran on |
| 2200 | 0 | 2 | ran on, error printed, the `if` taken |
| 2200 | 1 | 2 | shut down |

VERIFIED (the consoles and `games_mp.log`). The level's times run 0, 200,
300, 350, 400: the spawn's frames, then the loop's first (preceded by a
2.6 s hitch warning), still loading. INFERRED (the times and 0x8177366's
place after the frame). The koth error above fell at 350. VERIFIED (its
`PROBE 350` line).

vcod-server never ends a CoD4 level on a runtime error: it logs it
(rate-limited) and runs on, which is retail at `developer 0` past the
load. It does not end the level for an error in the load's frames under
`logfile`. INFERRED (the code).

## 9. The use key, the touch pass and a linked player

What the stock game objects (`_gameobjects.gsc`) need of the engine to be
planted, defused, picked up and captured. Port: `game::cod4::use_key`;
gates: the bot objective tests (`cod4::bots::tests::cod4_bots_*`,
bot-objectives.md 12).

- `G_RunFrame` opens with `G_TouchTriggers` for every client in use and
  not in noclip, before the frame's threads; the pass returns at once for
  a `pm_type` past 1 (linked). INFERRED (the call order). vcod runs it
  (`Cod4Level::touch_triggers`). Without it no `trigger_radius` (`sd` and
  `sab` bomb pickups, `dom`'s flags) or `trigger_multiple` (`koth`'s
  radios) sends `"trigger"`, and no carry or proximity object is taken.
- `Player_UpdateActivate` runs last in `ClientThink_real` for a player in
  the world. A press of 0x28 (`latched_buttons`) clears the held target
  and, off a mantle, a sprint and weapon states 15..20, takes the cursor
  hint's entity with `useHoldTime`. With 0x28 held and `g_useholdtime` run
  since, `Player_UseEntity` notifies `"trigger"` with the player and
  clears it. `Player_GetUseList` scores a `trigger_use_touch` whose box
  meets the player's at -256, ahead of everything in reach, with no sight
  trace. INFERRED (control flow). vcod takes the hint at the press off
  9.1's use list: a trigger open to the player's team
  (`setTeamForTrigger`) and not claimed by another client
  (`clientClaimTrigger`). INFERRED.
- The bomb zones of `sd` and `sab` and `sd`'s `bombDefuseTrig` are
  `trigger_use_touch` (`createUseObject` reads the class name's `use` for
  `triggerType "use"`). VERIFIED (`_gameobjects.gsc` `createUseObject`,
  `sd.gsc` `bombs`, `sab.gsc` `createBombZone`).
- `useHoldThink` `linkTo`s the player to the trigger, gives and switches
  to the `useWeapon` (`briefcase_bomb_mp`, `briefcase_bomb_defuse_mp`),
  and counts the use time only while that weapon is in hand, waiting at
  most 1.5 s for it; the hold ends on a release, a step off the trigger, a
  melee press or a death. VERIFIED (`_gameobjects.gsc` `useHoldThink`,
  `useHoldThinkLoop`).
- `G_EntLinkTo` on a client sets `pm_type` 1 (8 dead), `G_EntUnlink` 0
  (7); `ClientEndFrame` writes 1 or 0 by `tagInfo`, 8 or 7 for the dead.
  INFERRED. Trap: an end frame writing 0 every frame hides the link.
- `PmoveSingle`'s linked arm ends in `PM_Weapon`, so the briefcase comes
  up under the link. INFERRED. `vcod_cod4::pmove`'s `LINKED_STEPS` end in
  `Weapon`; without it the planter's weapon never changes.
- A linked player (`pm_type` 1, `PM_NORMAL_LINKED`, living) is a body for
  the others' pmove, is shot, and goes out in the snapshot as an unlinked
  one. INFERRED. Trap: reading those three off `pm_type` 0 alone.

### 9.1 The use list

`Player_GetUseList` is inlined in `Player_UpdateCursorHints` at
`cod4_lnxded` 0x80d2c30 (called from 0x80a6cea), with `compare_use` at
0x80d2c10 passed to `qsort` four times (0x80d3271, 0x80d33ad, 0x80d3700,
0x80d3720). Port: `game::cod4::use_key` (`use_list`, `use_hint`,
`use_entity`).

- Constants, each read by an `fsubs` or `fadds` at 0x80d3655..0x80d36c4:
  0.76 (0x822094c, the `requireLookAt` dot); 512 (0x8211e70) off a
  missile; 128 (0x820e278) off a turret; 256 (0x8214ac8) off a
  `trigger_use`; 10000 (0x8220950) onto an ungrabbable item or an entry
  the sight trace does not reach. VERIFIED (the rodata words).
- Scores: a `trigger_use_touch` the player's box meets at -256; anything
  else within reach of the eye (128; a missile
  `player_throwbackOuterRadius`; a turret `player_MGUseRadius`) at `(1 -
  (dot + 1) / 2) * 256` plus the distance to its bounds' middle, plus the
  offsets above. Sorted; ungrabbable items cut; each entry but a touch
  trigger and a vehicle traced to from the eye (`G_TraceCapsuleComplete`,
  mask 17), a blocked one cut; sorted again. INFERRED (control flow).
- The item arm of the hint walk tests the player's `weapons` bit for the
  item's weapon (`ps+0x55c`, 0x80d3523) and both weapons'
  `inventoryType` (+0x13c) against 0 and 3 (0x80d353c..0x80d355c): the
  item hints `weapon + 4` unless the player holds it, or the weapon in
  hand is neither primary nor alt mode while the item is one of the two
  and two primaries are held. VERIFIED (the instructions). That the call
  there counts held primaries is INFERRED.
- An item taken with the use key is notified `trigger` with the player and
  the weapon dropped in its place: `_weapons.gsc`'s `watchPickup` waits
  for `"trigger", player, droppedItem` and reads a missing `droppedItem`
  as an ammo pickup. VERIFIED (the stock script).
- vcod's swap (`swap_weapon`): with two primaries held and the item a
  primary, the weapon in hand goes into the world with its clip and
  reserve as `dropItem` launches one; the item's weapon is given with the
  item's clip and reserve and switched to (`a <weapon>`); the item is
  freed the next entity pass. INFERRED (vcod's CoD2 swap rule; 1.7's
  `Touch_Item` is unread).
- Missiles and turrets are on the list (9.2, 9.3). A `trigger_use`'s
  `requireLookAt` is left out (`useTriggerRequireLookAt` has no handler).
  A dropped weapon's middle is its rest point, not its bounds' middle.
  INFERRED.
- Stock maps: six place a `misc_turret`, each a `weapon_saw_MG_setup` on
  `saw_bipod_stand_mp` or `saw_bipod_crouch_mp` (mp_backlot two,
  mp_carentan three, mp_convoy, mp_farm and mp_overgrown two each,
  mp_pipeline one). VERIFIED (the entity strings of every stock `mp_*.ff`).
  No stock script calls `useTriggerRequireLookAt` (`common_mp.ff` and
  every map zone's rawfiles). VERIFIED (the rawfiles).
- The cursor hint is set in `ClientEndFrame` (`Player_UpdateCursorHints`)
  and a press takes the entity the last end frame left under it, so a
  press in the cmd right after a teleport finds no hint. Port:
  `use_key::update_cursor_hints` in the playing end frame,
  `update_activate` on `cursorHintEntIndex`. INFERRED. The retail SAW
  capture's hint (12, 0, 393) a frame before the mount is VERIFIED live.
- `cod4::bots::tests::cod4_use_key_swaps_a_held_gun_for_a_dropped_one`
  runs it on a live mp_crash level: a bot's gun dropped through the stock
  `dropWeaponForDeath`, taken by another player holding use over it.

### 9.2 Mounted SAWs

Captured on retail with `cod4_probe --script saw` behind mp_backlot's
crouch SAW 393 (`devmap mp_backlot`, war, `setviewpos -770 68 320 0`,
`--face 0`): `play-1.7-saw.txt`, uncommitted. Live gate: the client's
`cod4_live_saw_mounts_fires_clamps_and_lets_go`. Port: `game::cod4::turret`
over the shared `TurretRecord`. Names and order are INFERRED where no
label says otherwise.

- The gun goes out as `eType` 11 (`ET_MG42`, general table), `index` its
  model (22), `weapon` its turret weapon's number in the level's list (8,
  `saw_bipod_crouch_mp`, registered by `G_SpawnTurret` in spawn order),
  `apos.trType` 3, its barrel in `lerp.u.turret.gunAngles`
  (`u.anonymous.data[0..3]`). Unmanned it rests at the weapon's drop
  pitch, -51 on this gun. VERIFIED live.
- Registering the turret weapon takes hint-string slots for its
  `useHintString` and `dropHintString` (`PLATFORM_HOLD_TO_USE` at 277,
  `PLATFORM_HOLD_TO_DROP` at 278). VERIFIED live (the gamestate).
- Behind the gun and inside its yaw arcs the hint is weapon plus 4 (12),
  the use string's slot (0), the gun (393). A use press mans it at once:
  `eFlags` 0x200 (the crouch gun's stance), `viewlocked` 1 on 393, hint
  the drop string (12, 1) with no entity (1023), view clamp centred on the
  arcs (yaw base 360 range 45; pitch base 7.4999995 (0x40efffff) range
  22.5, for arcs 45 either side and -15..30). The next frame stands the
  body crouched (`pm_flags` 0x2, eye lerping to 40). VERIFIED live. The
  base's 360 is `AngleNormalize360` taking the turns' fraction once;
  vcod's `normalize360_once` keeps it. INFERRED.
- The first manned frame flips the gun's teleport bit (`eFlags` 2); its
  yaw follows the view clamped to the arcs (45 when looked 70 past). Held
  fire rings `EV_FIRE_WEAPON_MG42` (38) on the gun every frame (`fireTime`
  50), gun `eFlags` 0x40 on, gunner `eFlags` 0x40 and `viewlocked` 2.
  VERIFIED live. The round leaves from the gun's `tag_player` along the
  view, as far as `tag_flash` is from it, with the file's `playerSpread`,
  at `level.time`, no antilag. INFERRED.
- A second press lets go: `EV_STANCE_FORCE_STAND` (the stance mounted
  from), the mount spot a unit up, view yaw 0, `eFlags` 0x206 to 0x4,
  `viewlocked` 0 and 1023, next frame's hint the gun again. The gun then
  walks its pitch home 10 a frame to -51 and its yaw to 0. VERIFIED live.
- `G_ClientStopUsingTurret` is 0x80b7866: it puts the gunner a unit over
  his mount spot (0x80b778a, `fld1; fadds` at 0x80b77cd), flips `eFlags`
  2, clears 0x300 (0x80b791d), `viewlocked` and its entity (0x80b792d,
  0x80b793d) and both `active` bytes (0x80b7947, 0x80b7955). VERIFIED (the
  instructions). Callers: `turret_think_client` (0x80b7a2a), `ClientSpawn`
  (0x80a7ecf), the turret's free (0x80b8bce). VERIFIED (the calls).
- `ClientEndFrame` runs `turret_think_client` (0x80a715f) for any session
  reaching its tail, dead or playing, with `otherFlags` 4 and `eFlags`
  0x300 set (0x80a6f5a, 0x80a6f60); the dead session sets `pm_type` 7 or 8
  first (0x80a6f96, 0x80a710f). The think lets go when the gunner's
  `active` byte is not 1 (0x80b7a1e), his `sessionState` (gclient+0x2f64)
  is not 0 (0x80b7a40) or his `pm_type` is 6 (0x80b7a4a). VERIFIED (the
  instructions). `player_die` (0x80b1aa0..0x80b1dca) calls none of it.
- So a gunner killed on the gun is let go on his death frame, his body
  over the mount spot, once the killed callback has made his session dead.
  INFERRED (control flow). `ClientSpawn` tests the same two flags first
  (0x80a7b8c, 0x80a7b92) and lets go before placing the body, so a respawn
  never lands on the mount spot. INFERRED (control flow). `turret_tests`
  pins both releases (trap: releasing a dead gunner at respawn, after the
  placement, puts him back on the spot).
- The barrel has no turn rate: each frame's yaw is the view yaw of the
  frame's last cmd, clamped to the arcs, on the 65536ths grid (view -1.033
  gives -1.038, -3.378 gives -3.384; a positive view already on it is
  kept). The step per frame is the client's: the probe turns 70 degrees in
  1.5 s of its wall clock, so a frame's step is 2.34 (or double for a
  missed frame) on both servers, and where the turn starts against the
  frames is the probe's timing. VERIFIED (both captures).
- `G_PlayerTurretPositionAndBlend` places the gunner every manned frame,
  after the aim and before the fire. The legs' `turretanim` group
  (`crouchSAWgunner_aim`, legs 0x35) is walked from the file's first row
  (`_15down`) on, each row's two columns at `n / 2 + yaw /
  animHorRotateInc` over the engine's order (the file's reversed); the
  first row whose root height at time 0 reaches the gunner's height over
  `tag_weapon` is blended with the row before. The blend's root offset,
  turned by the barrel's yaw, hangs off `tag_weapon` at the gunner's
  height, and a point trace (mask 0x810011, world only) from eye height to
  60 under it sets z. The barrel yaw is the gun's pose from the frame
  before, so the body trails the barrel a frame (a turn's first frame
  leaves the origin; the frame after a stop still moves it).
- Retail's gunner stands at -754.049 75.372 258.125 the first frame (the
  mount's z 258.194 picks the rows), -754.039 75.398 after; at yaw -45
  -738.634 99.883, at 45 -752.460 51.005. VERIFIED live. The walk order
  and column rule are INFERRED and differ from CoD1's and CoD2's
  (`turretpose::GunnerRule`). vcod's run of the recipe puts the gunner on
  retail's origin to the printed thousandth at every barrel yaw both
  captures share (0, +-45, -42.012, 40.518, 40.523). VERIFIED (the two
  captures).
- 1.7's `BG_PlayerStateToEntityState` copies `viewlocked_entNum`
  (ps+0x59c) into the body's `otherEntityNum` (es+0x74) when `eFlags &
  0x300` (`cod4_lnxded` 0x8052ab9). VERIFIED (the instructions). The vcod
  client places a watched gunner off it as the server does
  (`play::cod4_players::GunInfo`, the gun's `animHorRotateInc` at
  `WeaponDef` +2004), keeping the wire's height. The first-person eye on a
  gun is `tag_player` (`cod4_client::turret_eye`).

### 9.3 The live grenade's throw back

Captured on retail with two probes on `devmap mp_crash` war: a `--script
frag` thrower at 123 2110 300 and a `--script throwback` target at 207
2291 296. Live gate: the client's
`cod4_live_frag_at_the_feet_is_thrown_back`. A second run, the thrower
aiming 60 under the target's feet (`play-1.7-throwback-*.txt`), is
`cod4::capture_ab::cod4_throwback_ab`: hint, pickup, hold and throw match
retail word for word up to the thrown grenade's strike on the thrower
(cod4-combat.md 15). VERIFIED (the gate).

- With a live frag at rest near it the target's hint reads 44 (the frag's
  weapon 40 plus 4), string -1, the grenade's entity, and
  `throwBackGrenadeTimeLeft` its fuse's time left (2550). VERIFIED live.
- The frag key's `EV_SWITCH_OFFHAND` with that hint takes it in the same
  frame: `EV_RESET_ADS`, `EV_AMMO_PICKUP` (parm 40), the grenade gone from
  the snapshot, `throwBackGrenadeOwner` the thrower (1), the hint cleared,
  `grenadeTimeLeft` the time left. The hold counts it down; the throw
  (`EV_USE_OFFHAND`) puts a new grenade in the world;
  `PM_Weapon_OffHandEnd` clears the owner (1023) and the time left.
  VERIFIED live. That the throw spends no frag is `PM_Weapon_OffHand`'s
  throw back arm. INFERRED (vcod's live gate checks its own).
- The pickup is inlined at `cod4_lnxded` 0x80a59c6..0x80a5aa5: hint entity
  not 0, the event's weapon and the missile's (`index % 128`) of
  `offhandClass` 1 (def+0x144), `eType` 4, time left set; then the owner
  from `parent` (1022 for none) and the grenade's touch. VERIFIED (the
  instructions).
- The list's reaches are dvars registered at 0x80b5343, 0x80b537c,
  0x80b53b5: `player_throwbackInnerRadius` 90, `..OuterRadius` 160,
  `player_MGUseRadius` 128. VERIFIED (the immediates). A missile faster
  than `bg_maxGrenadeIndicatorSpeed` is not listed. INFERRED.
- A throw back held past its fuse kills its holder as the thrower's
  `MOD_GRENADE` (3), with no owner `MOD_SUICIDE` (12): the compare of
  `throwBackGrenadeOwner` with 1023 at 0x80a593c picks one of the
  `player_die` calls at 0x80a5996 and 0x80a5b93. VERIFIED (the
  instructions).
- vcod's run of the recipe matches the hint (time left 2500, its frame
  later), the events and parms, the owner and the throw. VERIFIED (the two
  captures).

### 9.4 `linkTo` on a tag of a script model

`_utility.gsc`'s `play_sound_on_tag`, run by `_destructible.gsc` for every
part a sedan loses, spawns a `script_origin` at `getTagOrigin(tag)` and
`linkTo`s it to the car on that tag, offsets zero.

- The linkTo builtin (`cod4_lnxded` 0x80c3160..0x80c32a7) looks the tag up
  through 0x817c2f8 and names `failed to link entity since tag '%s' does
  not exist in parent model '%s'` (0x821e8ac, pushed at 0x80c3293) only on
  a negative index; `getTagOrigin`'s miss reads `tag '%s' does not exist
  in model '%s' (or any attached submodels)` (0x821e148, pushed at
  0x80c008f). VERIFIED (the instructions). Both search the entity's DObj,
  its model and every attached one. INFERRED (the second string).
- mp_backlot's `vehicle_80s_sedan1_{silv,yel,red,brn}_destructible_mp`
  carry every tag the parts name (`tag_glass_back_d`,
  `tag_glass_{left,right}_{front,back}_d`,
  `tag_light_{left,right}_{front,back}` and their `_d`) in their own 48
  bones; the doors (`vehicle_80s_sedan1_silv_door_lf` and kin) carry their
  window's `tag_glass_left_front`. VERIFIED (the zone's xmodels,
  `vcod-cod4`'s `bone_dump`). So retail finds every tag the script asks
  for. INFERRED (not shot on retail).
- vcod reads a CoD4 parent's tag off the zones' bind-pose bone, the
  body's first, then each attached model's placed on the body tag it hangs
  on (`link::cod4_model_tag`). VERIFIED (`cod4::destructible_tests`, which
  breaks mp_backlot's sedans with the gsc `radiusDamage`). Trap: the pk3
  rigs hold no CoD4 model, so a lookup there fails every tagged `linkTo`
  on a CoD4 script model.
- vcod's CoD4 rounds and blasts also damage entities a script opened to
  damage (`setCanDamage`, `GameHost::script_takedamage`), so a player's
  fire breaks cars and barrels as a script's `radiusDamage` does
  (`game::cod4::entity_damage`, cod4-combat.md 13).

## 10. Arrays are values

`b = a; b[k] = v;` leaves `a` alone, and a write through `a` after the
copy leaves `b` alone. The same holds for an array passed as an argument,
read out of a field or another array's element, and for a nested array
(the copy is deep); a struct inside one is shared. VERIFIED live on
`cod4_lnxded` 1.7 (`war` on mp_crash under a mod whose `war.gsc` runs
`crates/gsc/tests/fixtures/semantics/probe_array_copy.gsc`'s body plus a
struct case: `logPrint` lines identical to the 1.1d capture in
`retail-captures.txt`, `struct_shared 2`) and on 1.1d (that capture). The
mechanism is a reference count with a copy at the write:
`Scr_EvalArrayRef` and `Scr_EvalArrayRefInternal` replace an array whose
`refCount` is non-zero with a fresh one filled by `CopyArray`, which
recurses into nested arrays and adds a reference to anything else.
INFERRED.

The stock scripts rely on it. `_teams::CountPlayers` (`_teams.gsc` 581)
copies `level.players` into a local and writes its `"allies"` and `"axis"`
counts there; `_globallogic::menuAutoAssign` (2052) calls it on every
team-menu auto-assign. VERIFIED (the stock scripts). On retail an
auto-assigned client leaves `level.players.size` at 1 and the timeout
takes it to 0 (a `PROBE players` line every 2 s from the same mod, one
`cod4_probe --answers "1 team autoassign|..."` that goes silent). VERIFIED
live. Trap: a shared array grows `level.players` two string keys, its
`size` counts them, and `_spawnlogic::spawnPerFrameUpdate` (1555, `if (
player.sessionstate != "playing" )`) reads an undefined player every frame:
the `field access needs a struct or entity`, `cannot compare undefined
against a value` and `cannot cast undefined to bool` flood. VERIFIED (the
regression tests below fail with exactly those three without the copy).

vcod copies at the write, with a flag for the count (`Heap::share`,
`Heap::own`): an array stored into a variable, field, element, argument or
a `waittill`'s bound locals is flagged, and the `EnsureArray*` and
`OwnLocal` ops that resolve an index write's target copy a flagged array
and store the copy back. The flag is never cleared, which costs a copy
retail would skip and no script can see. A write through `game` itself
(`game["k"] = v` after `g = game`) still reaches `g`. INFERRED
(unmeasured on retail). Gates: `semantics_ab`'s `probe_array_copy`,
`cod4::disconnect_tests::an_autoassigned_player_beside_bots_times_out_without_script_errors`
and its `disconnects_` twin (`COD4_DIR`).

## 11. A freed player in a script array

`_battlechatter_mp::doSound` adds its speaker to `level.speakers[team]`
and removes it after `waittill_any( soundAlias, "death", "disconnect" )`,
whose `endon`s on the other two names end the whole thread, `doSound`
with it. So a player who dies or leaves inside his line's 2 s stays in the
list for the rest of the level, and `isSpeakerInRange` (207) reads
`teammate.origin` off him. VERIFIED (the stock scripts). Once he leaves,
his entity is freed and the reference reads as a removed entity.

Retail 1.7 (`war`, mp_crash, scratch `run_dead.sh`) under an `fs_game`
whose `mp_crash_fx.gsc` keeps a test client in a local array past its kick
and runs `isSpeakerInRange`'s loop on it: `isDefined` reads 0. At
`developer 1` the comparison is a "script runtime error" (`pair 'removed
entity' and 'undefined' has unmatching types`) that shuts the server down.
At `developer 0` the thread runs on, the loop's `if` not taken past the
errors, so the function returns its first `return` (here "self",
`isSpeakerInRange`'s `true`) and the level keeps running. VERIFIED live.
So on retail the freed speaker quietly silences the team's battle chatter.
vcod-server does the same and logs each error, rate-limited (`dead entity
is not an object`, `distanceSquared takes two vectors`, `< > <= >= need
numbers`, `cannot cast undefined to bool`). VERIFIED (a server log). This
`developer 0` pair-type error ran on where section 8's `koth` one ended
its level: that one fell inside the load, this one 7 s past it (8.1).
VERIFIED (both consoles, 8.1's table).

## 12. A name change

- Client: `CL_CheckUserinfo` (`iw3mp.exe` 0x46c680) runs each frame from
  `cls.state` 4 (`CA_CHALLENGING`) on; when a userinfo dvar changed
  (`dvar_modifiedFlags & 2`, 0xcba73f4) it gathers the userinfo dvars into
  an info string and queues `userinfo "%s"` (0x6d0e28) as a reliable
  command. VERIFIED. vcod's `Conn::set_name` sends the connect's info with
  the new `name`, minus `protocol`, `challenge` and `qport`, which are no
  dvars. INFERRED (the gather is the dvars').
- Server: the `userinfo` row of the client command table (0x8225380) is
  `SV_UpdateUserinfo_f` (0x8170536): `I_strncpyz` of the argument into the
  client's userinfo (+0x650, 0x400 bytes), `SV_UserinfoChanged`
  (0x8170320), then a tail call of the game's `ClientUserinfoChanged`
  (0x80a8068). VERIFIED.
- `ClientUserinfoChanged` reads `cg_predictItems`. Unless the client's
  `sess.connected` (+0x2f8c) is 2 and the level's name mode (0x836f8d8) is
  set, it cleans the userinfo's `name` into the client state's name
  (+0x3048: leading spaces dropped, at most 15 characters, `UnnamedPlayer`
  when empty) and copies it into `newnetname` (+0x2fd8); with both set it
  cleans the name into `newnetname` alone. VERIFIED (0x80a811e..0x80a8213,
  0x80a829f..). That `updateClientNames` copies `newnetname` over the name
  is INFERRED.
- `setClientNameMode`: stock `war`, `dm`, `dom`, `koth` and `sab` pass
  `auto_change`, `sd` `manual_change`; no `common_mp.ff` script calls
  `updateClientNames`. VERIFIED (the zone's scripts). So on `sd` a rename
  shows from the next map on.
- vcod: `Cod4Level::userinfo_changed` writes the name the scripts, chat
  and snapshots' client states read (`GameHost::client_names`), under
  `auto_change`, or under `manual_change` for a client not yet in the
  game. It does not clean the name. The engine's copy (`status`, the log)
  follows the userinfo at once, as retail's. The client sends its userinfo
  on every change, so a `name` takes effect in the game, not only at the
  next connect.
