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
  `ffffffffffffffffffffffffffff3000`. VERIFIED (capture). 2314's 128
  bits end at weapon 127; vcod refuses a 128th registration with its own
  script error, as the held bits and the 128-slot ammo arrays hold no
  more. Retail's limit is not recorded.
- `isMantling` (0x80c4fc8) and `isOnLadder` (0x80c5040) answer
  `ps.pm_flags` bit 0x4 and bit 0x8 of the receiver's `gclient_t`
  (`gentity+0x15c`, `pm_flags` at +0xc). VERIFIED (objdump).
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
  `getTagAngles` place the model's bones composed with no anim (3.2, "The
  skeleton") by origin and angles; a playing body reads its posed bone
  (3.3). INFERRED (the code). Traces
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
| `setGoalYaw`, `clearGoalYaw` | 0x809ca42, 0x809ca14 | the yaw at +0x2ac, its flag at +0x270 |
| `setLookAtEnt` | 0x809c900 | an entity handle at +0x280; refuses a player's vehicle ("Can't set target on player's vehicle") and one with no health ("Vehicle must have health to control") |
| `clearLookAtEnt` | 0x809c8d2 | the handle cleared |
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
- Yaw: toward a look-at entity, else the goal yaw once the stop is close
  enough to turn into it, else the target yaw, else (hovering) hold, else
  along the move; at
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
- `heli_mats` builds the DObj's skeleton as 1.7 does ("The skeleton"
  below): the three tags' locals take the angles' quats in place of their
  base turns, places kept; everything under them follows. VERIFIED (the
  skeleton gate). The cobra's `tag_barrel` sits 7.3 degrees nose up at
  the base pose, and retail's `tag_flash` points along the gun's pitch.
  VERIFIED (the collision capture). `bone_moves` tells the trace's moved
  bones from the rest against the zone's `baseMat` word for word, as
  `XModelTraceLineAnimated` does (its base is the model's +0x1c,
  0x81cb06a). VERIFIED (the disassembly). The stock MP scripts never wait
  on `script_vehicle_collision`. VERIFIED (`common_mp`'s rawfiles).
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
- `physicsJitter(origin, outer, inner, min, max)` (0x80bce34): an
  `EV_PHYS_JITTER` (60) temp entity, outer radius the parm, the inner and
  the two pushes in `lerp.u` words 0 to 2, and its three errors
  (cod4-world.md 13.8). VERIFIED (the disassembly). vcod sends the same
  words (same test). VERIFIED (the test, ours). `physicsJolt` has no
  handler on vcod-server.
- A client-less CoD4 entity owns `pers` and `maxhealth`
  (`chopper.pers["team"]`, `chopper.maxhealth` failed on ours as client
  fields). VERIFIED (the error). 1.7 reaches client fields only through
  `ent->client`. INFERRED.

#### 3.2.1 The flight against retail

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
- Helicopter: bit for bit, body, gun and all, through the nodes, the
  hover and the flight home. VERIFIED (the gate, no tolerance). Before
  "The flight's float model" the turn through the first node parted by
  0.08 degrees of yaw and the gun by up to 0.13. VERIFIED (the gate,
  both ways).

#### 3.2.2 The collision capture

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
- The posed `tag_barrel` and `tag_flash` and the trace's point print
  retail's digits. VERIFIED (the gate); off the zone's `baseMat` they
  printed a thousandth off.
- The trace meets the barrel at retail's point with
  retail's normal word for word (-0.112754, 0, 0.993623): the base pose's
  normal of a gun yawed 99.8 degrees. VERIFIED (the gate).
- The first notify comes 5600 ms past the spawn with the body at z 750.08
  over ground near 332, normal (0, 0, 1); the next at 5650; the third,
  started in solid, reads (0, 0, 0); 209 by 16 s. Ours: the same times,
  normals, places and count, the last place 0.1 off. VERIFIED (the
  gate).

#### 3.2.3 The no-goal think

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
  VERIFIED (the disassembly). A spawn yawed 250 comes out -109.99999
  (0xc2dbffff), which the think's link snaps to -110 ("The link's
  snap" below); the next think reads -110 and keeps it. VERIFIED (gdb at
  0x808c719 and 0x808c79b).
- It also tracks vehicle +0x2e4 toward 1 and +0x2e8 toward 0 at 4 a
  second; `VEH_UpdateMoveToGoal` stores them too (0x809e9ce), and nothing
  outside 0x808ba22 reads them. VERIFIED (the disassembly).

#### 3.2.4 The turret

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
  frame match word for word. VERIFIED (gdb). Before the goal the barrel
  landed an ulp off in 26 of 80 thinks. Its cause was the body's yaw, not
  the pose: retail's `r.currentAngles` reads -110 (0xc2dc0000) where ours
  kept -109.99999, as ours ran no link snap. VERIFIED (gdb, 0x80a0068).
  With the snap and "The skeleton", the barrel's place matched every word
  of the first 81 thinks; think 82, in flight, read the body's yaw an ulp
  off (0xc2db4001 against 0xc2db4000), the flight's yaw step. VERIFIED
  (gdb against ours). With "The flight's float model" all 159 match.
  VERIFIED (the idle capture's gate).

#### 3.2.5 The skeleton

How 1.7's server builds a DObj's bones, read off `cod4_lnxded`.
`vcod_cod4::skel` is its port; `heli_mats` runs it for a helicopter and
`game::cod4::combat::skel_worlds` composes a player body's pose with it.

- `G_DObjGetWorldTagPos` (0x80cbfc4) and `G_DObjGetWorldTagMatrix`
  (0x80cbe9a) call `G_DObjCalcBone` (0x80ca732), then read the bone's
  0x20-byte `DObjAnimMat` (quat, translation, weight) out of the DObj's
  skeleton (DObj +0x48; 0x817c2aa returns it). VERIFIED (the disassembly).
- `G_DObjCalcBone`: the server's DObj (`Com_GetServerDObj` 0x8125e12: the
  slot index at 0x88e7780 + 2 * entnum, the DObj at 0x88e7fa0 + 100 *
  index); if 0x817cd6e says the bone is built under the current skeleton
  stamp (0x13ed7ac4), nothing more. Else its hierarchy bits (0x81abf36),
  the entity type's controller (the handler table 0x826cefc, indexed by
  the byte at entity +0x16e), then `DObjCalcSkel` (0x81af4f8). VERIFIED
  (the disassembly). A stale stamp gets a fresh skeleton with no bone
  built (0x817cda4..). VERIFIED (the disassembly). 0x817bed2 bumps the
  stamp ahead of every `G_RunFrame` (cod4-server-frame.md, step 6). So a
  DObj's bones are built once a frame, by the first tag read, with the
  controller's angles of that moment. INFERRED (the three facts).
- A controller's tag (0x81abb2a, 0x81af996): only a bone in the call's
  hierarchy bits and not yet set this stamp; its local takes the angles'
  quat (`AnglesToQuat` 0x81b0008), a zero translation (`vec3_origin`
  0x821be20) and weight 0 (0x81b00de). VERIFIED (the disassembly).
  0x81afa6c and 0x81b01e8 set a bone the same way and also its bit at
  skeleton +0x10 (DObj +0x24; 0x81b0228), which sends it down
  `DObjCalcSkel`'s second arm (0x81ae2d8 to 0x81ae5da). VERIFIED (the
  disassembly). A player's controller (`BG_Player_DoControllers`,
  0x8056b48, from the player entity's handler 0x80a480e) sets `back_low`,
  `back_mid`, `back_up`, `neck`, `head` and `pelvis` through 0x81b01e8,
  each off `clientInfo_t` +0x3fc in that order, 12 bytes apart, a bone
  the model lacks (0xfe from the name lookup) skipped; then bone 0
  (`tag_origin`) through 0x81b0144, which takes its angles (+0x444) and
  its translation (+0x450) and sets only the +0x14 bit. VERIFIED (the
  disassembly). `DObjCalcAnim` masks every bone so set out of its tree
  walk (0x81b4cca..0x81b4ce5, the walk at 0x81b4f6f). VERIFIED (the
  disassembly). A control bone so keeps no anim of its own: the second
  arm rebuilds each from its angles' quat alone (below). VERIFIED (gdb).
- The second arm, a control bone `c` under parent `p` with bone 0's `r`:
  `m = r^-1 * p`, each component's partial sums stored
  (0x81ae5da..0x81ae67f); `t = c * m`, `w` and `y` stored, `x` and `z`
  kept (0x81ae682..0x81ae763); the bone's quat `r * t`, kept for the
  weight (0x81ae766..0x81ae819), which and the translation then run as
  the first arm's (0x81ae3a7). The control turns in `tag_origin`'s frame,
  where CoD1's turned in the model's. VERIFIED (the disassembly).
  `vcod_cod4::skel::compose_controlled` is its port: rebuilt off retail's
  own bone 0, parent and angles, all 804 control bones of 200 player
  skeletons gdb dumped match word for word, `tag_origin` yawed up to 42
  degrees. VERIFIED (`skel_ab`, `fixtures/skel/player-mp_crash-gdb.txt`).
- `AnglesToQuat`: each angle times the double pi / 360 (0x8238538) stored
  as a float through `sinf` and `cosf`, the roll's cosine kept on the
  stack; `x = sr cy cp - cr sp sy`, `y = cr cy sp + sr sy cp`, `z = cr sy
  cp - sr cy sp`, `w = cr cy cp + sr sp sy`, each stored. VERIFIED (the
  disassembly).
- `DObjCalcSkel`: the bones the call needs less those built
  (0x81af50a..0x81af558). `DObjCalcAnim` (0x81b4c8c) runs first; with no
  anim tree a root's local is the identity quat with no translation and
  weight 0 (0x81b4dc0..0x81b4df3), a non-root's quat each `XModelQuat`
  short times the float 0x38000100 (1 / 32767) stored, no translation,
  weight 0 (0x81b4e6b..0x81b4ecc). VERIFIED (the disassembly). A bone a
  controller set is skipped. VERIFIED (the bit test at 0x81b4e65).
- Then per model: the bones a later model shares by name copied from the
  first (0x81af655..0x81af67f); a root keeps its local and takes the
  weight `2 / (x^2 + y^2 + z^2 + w^2)` summed on the stack, a zero quat
  becoming `w` 1, weight 2 (0x81af7ab..0x81af850); every other bone in
  order through 0x81ae164. VERIFIED (the disassembly).
- 0x81ae164, a bone `c` under parent `p` (bone less `parentList`'s
  offset): the quat `p * c` (Hamilton), each component summed on the
  stack in the order `w = ((pw cw - cx px) - py cy) - pz cz` and its
  kin, stored, and its weight `2 / |q|^2` off the unstored sums. The
  translation: the local's plus the model's `trans` (12 bytes a bone),
  x and y stored, z kept; turned by the parent's matrix built off its
  quat times its weight (`a = w px`, `b = w pz` stored, `w py` kept; the
  products `xx`, `xy`, `xw`, `yy`, `yz`, `yw`, `zw` stored, `xz` and
  `zz` kept) and added to the parent's translation, x's partial sums
  stored twice, y's and z's kept to one store (0x81ae3de..0x81ae54d).
  VERIFIED (the disassembly).
- `G_DObjGetWorldTagMatrix`'s rows off a bone (0x80cbef3..0x80cbf8a): the
  same weighted products, `1 - (yy + zz)` with the sum stored, then times
  the entity's `AnglesToAxis` (0x818effc); the place through
  `MatrixTransformVector43` (0x819016e). VERIFIED (the disassembly).
- gdb at 0x80a0068 dumped the cobra's skeleton on all 159 thinks of the
  idle capture's gsc: the nine bones the barrel's chain builds (0..3, 41,
  42, 43, 47, 48; `tag_body` 2, `tag_turret` 41, `tag_barrel` 48, the
  flash-tag slot 0). Rebuilt from the zone and each think's gun angles,
  every word matches, the x87 sums as `f64`. VERIFIED
  (`crates/cod4/tests/skel_ab.rs`,
  `fixtures/skel/cobra-idle-gdb.txt`).
- Off the zone's `baseMat` (ours before), the cobra's barrel landed an
  ulp off now and then; with the composed skeleton alone the idle gate
  still read 8 gun words off before the goal, against 6 before. VERIFIED
  (the gate). The rest was the link's snap below.
- A player body's locals come from vcod's own sampler and controllers,
  the control bones' through the second arm (`skel::player_mats`, which
  the client's drawn bodies take too, cod4-player-anims.md 8);
  composed through 0x81ae164 instead of as quaternions, the dying body's
  `tag_weapon_right` lands where 1.7's does word for word on both
  dropped-weapon captures (cod4-combat.md 6.1). VERIFIED (`capture_ab`).
  That the sampler's locals are 1.7's word for word is INFERRED off it.
- `getTagOrigin` on any other entity reads the model's bones so composed
  with no anim (`Cod4Zones::model_tags`); they sit within 0.01 of the
  zone's `baseMat` (cod4-player-anims.md 4). VERIFIED (the census).

#### 3.2.6 The link's snap

`SV_LinkEntity` (0x817e3f0) snaps each component of `r.currentAngles`
within a thousandth of a degree of a whole one to it: `n = floor(a +
0.5)` (`frndint` under round-down, then `fistp`), and `a = n` when `(n -
a)^2` is under the float 0x358637be (1e-6, at 0x8212470), on every link
(0x817e426..0x817e5ba). VERIFIED (the disassembly). gdb watching the
idle cobra's yaw caught the store at 0x817e53a, from the think's set and
link (0x809c1dc, which runs `G_SetOrigin`, `G_SetAngle` and the link when
the place or the angles changed). VERIFIED (gdb). `G_SetAngle`
(0x80ca276) writes the angles to `s.lerp.apos.trBase` (+0x3c) and
`r.currentAngles` (+0x148), and the link snaps +0x148 alone, so the wire
carries the flight's unsnapped angles and the vehicle's own angles
(+0xd8) never see the snap. VERIFIED (the disassembly; the gdb replay in
"The flight's float model" shows no change to +0xd8 between thinks).
CoD1's snap truncates `a + 0.5` (`mover::snap_angles`), which leaves a
negative angle alone. VERIFIED (cod11-movers.md 12). vcod-server's CoD4
links take 1.7's (`mover::snap_angles4`, `HostProfile::link_snap`):
movers, script models and the host's other links, and `Heli4` keeps
`link_angles` (`r.currentAngles`) beside its flight's `angles`. The
turret, the tags, the riders and the no-goal think read `link_angles`.
INFERRED (the code). The mover, combat and capture gates read the same
with either snap; no capture yet holds a CoD4 mover at a negative angle
a thousandth off a whole one. VERIFIED (the gates).

#### 3.2.7 The idle capture

`cod4::hardpoint_ab::helicopter_idle_matches_retail` replays
`crates/cod4/tests/fixtures/net/play-1.7-heli-idle.txt`. Recipe: the
flight's, under `+set fs_game mods/vcodidle` with
`crates/cod4/tests/fixtures/gsc/heli_idle/maps/mp/mp_crash_fx.gsc` in the
mod's iwd and `--secs 30`. A cobra spawns over the first loop node at
angles (8, 250, -6) with no goal for 4 s, its gun on a point ahead and
below, then on one behind; then it flies a pass-through goal and is
deleted. No hover radius and no stop goal, so no random draw. VERIFIED
(the capture).

- Before the goal every place, body and gun word matches retail's, the
  body (0, -110, 0) from the first think. VERIFIED (the gate). It took
  the link's snap and the skeleton ("The turret"); without them the gun
  parted by 3.1e-5 degrees. VERIFIED (the gate, both ways).
  Without `Heli4::hold` the body read 8 degrees of pitch off from the
  first think and the gun 7.4. VERIFIED (the gate, both ways).
- Past the goal the flight and the gun match word for word too. VERIFIED
  (the gate). Before "The flight's float model" the gun parted by 0.128
  at worst, after the body's yaw parted by an ulp at think 82. VERIFIED
  (the gate, gdb).

#### 3.2.8 The flight's float model

`VEH_UpdateMoveToGoal` (0x809e38a) as `game::cod4::vehicle::flight` ports
it, x87 values kept on the stack as `f64`, stores as `f32`. A Ghidra
decompile of the function served as the map of its arms; each item below
is the disassembly's:

- The aim: x's difference stays on the stack, y's and z's are stored; the
  flat length squared is stored, the length is stored
  (0x809e3b6..0x809e426). The direction's three components are stored; z's
  product also stays on the stack for the change (0x809e51d..0x809e56a).
  VERIFIED (the disassembly).
- The brake (0x809ea42..0x809eb1a): the next speed, its stopping distance
  `0.5 next (next / decel)` and `dist - 0.05 next` all on the stack; the
  frame's tail `0.05 - (dist - stop) / speed` is stored as a float only
  when it is under 0.05, and taken only when above 0. VERIFIED (the
  disassembly).
- The new speed stays on the stack; the change's limit `rate dt` is stored
  (0x809e4c3..0x809e51d, 0x809f6e9, 0x809fa6f). x and y of the change are
  stored, z stays on the stack. Under the set speed the turn-never-brakes
  arm calls `Vec2Normalize` (0x81915d8), which spills the aim's x and the
  change's z to floats around the call (0x809f73f..0x809f7c2); its x drops
  a stored product, its y one kept. VERIFIED (the disassembly).
- The stop's shed and the turn's brake (0x809ecc1..0x809eddc,
  0x809fb07..0x809fb8f), the climb's ease (0x809f97f..0x809fa5d): every
  intermediate on the stack, the compares against stored `|x|` floats. The
  brake writes the velocity in passing; the velocity's store at
  0x809e5ef..0x809e60f overwrites it from the think's first copy. VERIFIED
  (the disassembly).
- The smoothed change: each half-way sum stored and kept, its length off
  the kept values; a scale by the limit takes x kept, y and z stored
  (0x809e63c..0x809e6b1, 0x809f7c7). VERIFIED (the disassembly).
- The move: `((v' + v) 0.5) dt` plus the place, the head's x kept when the
  brake started in the frame (0x809e6bd..0x809e74f, 0x809ee2a). VERIFIED
  (the disassembly).
- The angles (yaw 0x809e8ff..0x809ef32, pitch 0x809f38a..0x809f4e9, roll
  0x809f50c..0x809f6ae): the gap normalised on the stack and kept; the
  rate's step and its limits on the stack, the rate stored; the angle
  takes the kept rate, `angle + rate 0.05` normalised on the stack, then
  stored. Pitch and roll steer to an unstored target and read the tilt's
  up and down rates as stored floats; the yaw's rates are the stored ones,
  a fifth of them kept. VERIFIED (the disassembly).
- The look-at (0x809e769..0x809e772, 0x809f19a..0x809f1f2): a non-zero
  handle's low half at vehicle +0x280 sends the yaw to the entity
  (`g_entities` 0x841f260 plus 628 times the handle less one, the
  handle's base at 0x808af9e): its `r.currentOrigin` (+0x13c) less the
  moved place (vehicle +0xc0), each difference stored, through
  `vectoyaw` (0x8191c22) into the wanted yaw. VERIFIED (the
  disassembly). It skips the goal and target yaws' choice (back at
  0x809e849), not what follows: the goal yaw's slow-down, its overshoot
  and `goal_yaw` still run when one is set. VERIFIED (the yaw run's
  replay, whose look-at flight holds a goal yaw and notifies it).
- The change's z through the velocity: the whole chain is kept at the
  x87's 64 bits, and near rest `v0.z` and the change all but cancel, so
  an `f64` there parts from retail by an ulp of the velocity's z: three
  words over two thinks of the yaw run's hover, at 4e-10 and 2e-11. `vcod_common::x87::F80`
  carries it (the direction's reciprocal, the limit's root and quotient,
  the add back). VERIFIED (the replay, both ways).
- The tilt (0x809ef90..0x809f36a): the fake drag's share stored around
  `Vec2Normalize`, the push's x kept, y stored; the push rate stored; the
  normalised push stored; the heading's sine and cosine of the new yaw
  stored. VERIFIED (the disassembly).

`flight::tests::every_think_matches_retail_word_for_word` replays
`crates/cod4/tests/fixtures/heli/think-gdb.txt` and `yaw-gdb.txt`: retail
1.7 under gdb (`unshare -rnm`, the probe as in "The flight against
retail") with breakpoints at 0x809e38a and its two returns (0x80a06e8,
0x80a084e) dumping the vehicle record, once with the idle capture's gsc
(80 thinks), once with the flight's (524: hover, stop goals, target yaw,
near-goal notifies), and once with the yaw capture's (802, below), which
also dumps the look-at entity's origin. From each think's record before,
the record after matches word for word, and the notifies. VERIFIED (the
test). The yaw run reaches every arm the flight has: the stop with no
hover radius, the goal yaw on a stop and in the hover, the target yaw
with overshoot, the climb's ease, the turn's brake, the shed into a stop
entered at speed, the turn that never brakes, and the look-at. VERIFIED
(the replay, counted).

#### 3.2.9 The yaw capture

`cod4::hardpoint_ab::helicopter_yaw_matches_retail` replays
`crates/cod4/tests/fixtures/net/play-1.7-heli-yaw.txt`, the probe's
capture of the gdb run above (`+set fs_game mods/vcodyaw`, the gsc
`crates/cod4/tests/fixtures/gsc/heli_yaw/maps/mp/mp_crash_fx.gsc`), cut 4 s
after the helicopter's delete. A cobra with hover radius 0 (no random
draw) and `setYawSpeed(60, 30, 20, 0.3)` descends to a stop under goal yaw
150, takes goal yaw -100 in the hover, climbs through a pass-through goal
under target yaw -60, turns sharply through another, stops from speed
under goal yaw 45, then looks at a script_origin (`setLookAtEnt`) while
hovering, the point jumping once, and while flying to a stop with the goal
yaw still set; `clearLookAtEnt` hands the yaw back to the goal yaw.
VERIFIED (the capture). Every helicopter word matches retail's; with the
look-at dropped on ours the yaw parts by 179 degrees. VERIFIED (the gate,
both ways). The notifies' calls name them: 0x809ebe1 `goal`, 0x809eeb8
`near_goal`, 0x809f235 `goal_yaw` (its `scr_const` read at 0x809f21c). VERIFIED
(gdb, the disassembly).

On the wire, a -100 angle field's zero bit tests the word: a non-zero
angle under half a unit, or -0, goes out as a set bit and a short 0, which
the client reads as the smallest float above zero. The flight capture's
hovering roll sits there for 49 snapshots. VERIFIED (the gate;
`delta::angle_field_sent`).

#### 3.2.10 Missiles under 1.7: attractors, the wobble, guidance

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
  test, ours). A retail capture of an RPG's two flights gates the rest,
  word for word: the wobble's `AngleVectors` takes glibc's `sinf` and
  `cosf` and `vectoangles` keeps its flat length a double (cod4-combat.md
  15). VERIFIED (the gate). The `vectorToAngles` builtin (0x80be762)
  calls the same `vectoangles` (0x80be79d), so vcod's CoD4 builtin takes
  that path too (`math::vectoangles_1_7`). VERIFIED (the instructions).

### 3.3 The script traces

`builtins/cod4_trace.rs`: `bulletTrace`, `bulletTracePassed`,
`sightTracePassed` trace the clip map, its static models (point trace),
linked brush entities and, with `hitCharacters`, the bodies where their
last cmds linked them, passing the ignored entity. INFERRED (the code). 1.7's handlers,
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
checks keys and both arms. VERIFIED (the test).

- `SV_ClipMoveToEntity`'s locational arm (0x817de77) takes the DObj
  path for an entity with `svFlags` 2 (a player's, set by
  `ClientEndFrame` 0x80a6c41) only under a priority map (`clip+0x30`,
  0x817de8a..0x817de8f); else the plain entity clip. VERIFIED (the
  instructions). So the script's bullet traces meet a player as its
  capsule and pose no body: in a gdb run of the `trace_read` probe below,
  1677 entity poses (0x80ca99e) fired, all on entity 93 (handler 5), none
  on a player, and no controller ran. VERIFIED (gdb). An entity with
  `svFlags` 4 (a helicopter) still takes its model collision (3.2).
- The capsule is `CM_Trace`'s temp box arm (0x811d29c,
  cod4-pmove.md 15) where the trace's start z picks the primitives as
  1.5b's dispatcher does: a point trace from above the top sphere's
  centre meets that sphere alone, though it crosses the cylinder's band
  first. VERIFIED (the instructions 0x811d484..0x811d49c,
  0x811db38..0x811dce4; the gate below).
- A player's capsule stands where its last cmd linked it
  (`SV_LinkEntity` in the think), not where the last frame did: the
  gate's normals read a frame late without it. VERIFIED (the gate).
- `cod4::capture_ab::cod4_trace_read_ab` gates them on a retail run of
  the `trace_read` gsc (`play-1.7-traceread-*`): the target sways
  (`--script sway --aim-z 150`), and every frame three `bulletTrace`s
  from the idle shooter's eye run past the target's origin at 64, 56 and
  44 up, printing the entity, the point in tenths and the normal in
  thousandths. Ours matches every print and every playerstate word,
  the target's `aimSpreadScale` with 1.7's spread step on the x87 stack
  (cod4-combat.md 2.3); its `GAPS` are empty. VERIFIED (the gate).

vcod, INFERRED (the code):

- Both bullet traces run `trace4` (the clip map, static models, brush
  entities and the players' capsules) and the helicopters' model
  collision (`cod4_trace::locational4`). A player's capsule reads surface
  flags 0, `default`; the surface 1.7's capsule arm copies from its trace
  work (0x811bbda, 0x811deea) is unread, and the probe prints none.
- Sight, cone traces and `physicsTrace` meet a body as its link box.
- `getTagOrigin` / `getTagAngles` on a playing body read its posed bone,
  attachments included (`G_DObjGetWorldTagPos`), placed at the body's
  origin and yaw at the call and its controls eased once a frame
  (cod4-combat.md 6.1); anything else the model's bind pose.
- The level writes bodies, poses and brushes after its entity pass
  (`trace_bodies`, `posed_bodies`, `trace_brushes`), and each cmd's
  relink moves its client's body in `trace_bodies`.

Tests: `cod4::hardpoint_tests::bullet_traces_meet_bodies_and_the_helicopter`,
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
  code). Of a stock car's parts, the hood and the two right doors
  (`vehicle_80s_sedan1_*_hood`, `_door_rf`, `_door_rb` and the wagon's
  and hatchback's kin) carry one brush geom each, the rest
  (`_door_lf`, bumpers, mirrors, lights) no `physGeoms`: their bounds
  box. VERIFIED (`phys_dump` on mp_backlot). A brush geom is a
  `BrushWrapper` in the model's frame: the sedan hood's bounds (-3.5,
  -24, -8) .. (41.5, 24, 0), its `PhysMass` centre (19.04, 0, -3.56) and
  moments (195.1, 130.4, 318.9) per unit of mass. VERIFIED (`phys_dump`).
  All 37 brush geoms of `common_mp` and the `mp_` zones are such boxes,
  with no side past their bounds. VERIFIED (`phys_dump`, every zone).
  The brush becomes geom class 3 (0x5a5420 copies the geom and calls
  0x5a4ab0 with 3), which ODE meets through the game's own collider.
  INFERRED (the class number, not its collider, read).
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
- The force (`Phys_ObjAddForce`, 0x5a5900): `1000 / msecStep` of the
  world in `esi` (`fild [esi*4+0x71e798]`, `fdivr` the double 1000.0 at
  0x70af28, stored as a float) times the push, then
  `dBodyAddForceAtPos`, which adds it to the body's force (+0x98..+0xa0)
  and `(p - x) x F` to its torque (+0xa8..+0xb0), `x` the body's position
  then (+0xe0) (0x5b7000..0x5b70b7); then the disabled bit (+0x18 & 4)
  cleared and the idle time and steps (+0xd8, +0xdc) reset from +0xd0 and
  +0xd4 (0x5a5968..0x5a5981). VERIFIED (disassembly). `Phys_ObjBulletImpact`
  takes its world as its first argument and passes it in `esi`
  (0x5a5d73); the worlds named: 1 for the launch (`push 0x1` at
  0x434dd8), 0 for a round on a launched part (0x48e043) and on a dynamic
  entity (0x48e349), 0 for a blast on a dynamic entity (`xor esi,esi` at
  0x48ea84). The jitter (0x5a23e0) names the world its contact pass was
  handed, for both factors (0x5a2424, 0x5a2496, 0x5a2550). VERIFIED (the
  arguments).
- The world step adds `force * h * invMass` (body +0x94) to the linear
  velocity (+0x130) and the body's world inverse inertia times
  `torque * h` to the angular (+0x140) (0x5b9187..0x5b9290, `h` the step
  at `[ebp+0x1c]`); 0x5a6640, after each step, zeroes both on each body
  not disabled (0x5a666b..0x5a6689). VERIFIED (disassembly). That ODE's
  quickstep solves the contacts with that force on, as with gravity, is
  INFERRED (ODE's `dxQuickStepper`). So a push lands as the impulse
  `J` times the length of its body's next step over the named world's
  `msecStep`: whole when the two are one, 25 / 17 of it on a 50 ms frame's
  two 25 ms steps, 28 / 17 for a round naming world 0 on a part stepped at
  world 1's 28. INFERRED (the arithmetic).

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
  last step (the fraction at the world block's +8, 0x5a6ea4). VERIFIED
  (disassembly).
- `msecStep` per world (0x71e798, 17 for each at start) is picked after
  each step: 0x5a6640, which 0x5a6d50 calls after each step's 0x5a6440
  (0x5a6e4e, 0x5a6e5e), counts the world's bodies 0x5a62f0 passes, those
  whose `+0x18 & 4` is clear, ODE's disabled flag (0x5a62f0..0x5a62fd;
  it applies the drag to them), and sets `msecStep = min + ftol(clamp((count - 32) /
  18, 0, 1) * (max - min))` (0x5a6738..0x5a67bb: the double 18 at
  0x70b330, the clamp against 1.0 at 0x70ad08 and 0, `_ftol2` 0x68b360,
  which truncates). `min` and `max` are the tables at 0x71e7a4 (17, 17,
  17) and 0x71e7b0 (67, 67, 34), by world. VERIFIED (disassembly, the
  tables). So up to 32 bodies awake a world steps at 17 ms, at 41 at 42
  ms, from 50 at 67 (34 for the ragdolls' world 2). INFERRED (the
  formula).
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
- Each explode case of
  `CG_EntityEvent` calls `Ragdoll_ExplosionEvent` (0x5bdc10) and then
  `DynEntCl_ExplosionEvent` (0x48e670); the arms, their call sites and
  their arguments are in cod4-world.md 13.3. 0x48e670 walks the map's
  dynamic entities (the lists at 0x14099b8..0x14099c8, picked by
  0x48e560) and pushes each through `Phys_ObjAddForce` (0x5a5900, call at
  0x48eaa1); the ragdoll's pushes go the same way at 0x5bdfe0.
  VERIFIED (disassembly). The worlds: `Phys_ObjCreate` takes its world
  in `edi`, 0 for a dynamic entity (0x48dcc1, 0x48ed98), 1 for a
  launched part (0x434d44), for a broken dynamic entity's pieces
  (0x490fd0, beside "Failed to spawn pieces model") and at 0x4a21b4, 2
  for a ragdoll's bones (0x5bc9d3). VERIFIED (the immediates). What
  0x4a21b4's caller makes is not read. The only callers of
  `Phys_ObjAddForce` are 0x48eaa1, 0x5bdfe0, `Phys_ObjBulletImpact`'s
  0x5a5daf and 0x5a25a1, a jitter that reads a per-world list at
  0xcc54498 (stride 0xcc) which only 0x5a7730 fills, at world 0's
  address (called from 0x48ee1b, `EV_PHYS_JITTER`'s path), and
  0x5a66ff zeroes; `Phys_ObjBulletImpact` (0x5a5a20) is
  called only by the launch (0x434dda), a round on a launched part
  (0x48e049), a round on a dynamic entity (0x48e34f) and a dynamic
  entity's pieces' break (0x491271). VERIFIED (every `call` to the two
  in the binary). 0x48e049 is in 0x48df10, which takes a round's hit of
  type 1 on an entity whose `centity_t` (0x84f2d8, stride 0x1dc) holds a
  body at +0x18, and passes world 0 (0x48e043), not the part's world 1;
  it runs from the client-side round's walk (0x455b92) and, under
  `sv_clientSideBullets` 0, from the dynent round (0x48e0f0 through
  0x48e060). VERIFIED (the code). So a blast does not push a launched
  part, and a round does: after its launch's push it meets gravity, the
  world and the rounds this client draws, and the server sends nothing
  more (3.2). INFERRED (the callers above). The dynamic entities
  themselves: cod4-world.md 13. The jitter's event, `EV_PHYS_JITTER`,
  and its push: cod4-world.md 13.8.
- 0x48df10's push (from 0x455b92, the first leg only, after
  `BulletTrace` met something): a hit of type 1 under `dynEnt_active`
  on an entity whose `centity_t` +0x18 holds a body (neither 0 nor -1);
  the round's impact once more at the contact (0x48dd00, or the melee
  sound with the melee flag); then `Phys_ObjBulletImpact` at the contact
  along the round, normalized (0x48dfb7), at `dynEnt_bulletForce`
  (0x11015fc) and the preset of the DObj's first model's
  `bulletForceScale` (`XModel` +0xd4, +0x14). VERIFIED (the code). The
  world it passes, 0, picks only the `msecStep` `Phys_ObjAddForce`
  spreads the impulse over (0x5a5900) while world 1 steps the body.
  INFERRED (the two reads of 0x71e798).

vcod (`vcod_cod4::physics`, `play::cod4_physics`):

- Ported: the 1000 ms rule, the refusal without a preset, a brush model
  and `eFlags` 0x20 left out, the shape (brushes, boxes and cylinders,
  else the bounds box), the mass centre, the push with its up bias, spin scale and
  formula, gravity, the clock (two steps at most, the lerp), the drag,
  the per-body sleep on the squared speeds, the contact friction and
  bounce, the world-only collision under 0x2806c91, no push from a
  blast, and a round's push (`Cod4Physics::shoot`, off the first
  contact's entity, `cod4_fx::BulletHit::entity`) at `dynEnt_bulletForce`
  and the part's `bulletForceScale`, with the impact played again there.
  VERIFIED (the code against the above). Each world steps its bodies
  together on one clock (`WorldClock`: world 1's in
  `play::cod4_physics::Cod4Physics::advance`, world 0's in
  `DynEnts::advance_in`) at the `msecStep` the count awake after each step
  picks (`physics::msec_step`). A push (`Body::push`) is a force at the
  named world's `msecStep`, spent with gravity in the body's next step
  and zeroed: the launch at world 1's, a round on a part
  (`Cod4Physics::shoot`) and a round or a blast on a dynamic entity at
  world 0's (`DynEnts::clock`), a jitter over the step it is collided in.
  VERIFIED (the code against the above).
- In place of ODE: one impulse solver per body. Each box's (and an
  axial brush's) 8 corners, 12 edge midpoints and 6 face centres, a
  brush with sides' corners (where three of its planes meet), and 16
  points round each cylinder sweep the clip map as points along their
  velocity over the step; the kernels' 0.125 skin is the contact layer; a
  corner already in the world is traced out from the centre and pushed
  out at `phys_contact_erp` up to `phys_mcv`; an approach faster than 0.1
  bounces by the preset, from the corner where the step found it; ten
  passes of sequential impulses with Coulomb friction (the preset's times
  `phys_frictionScale`); after a frame's steps the centre takes 1.7's
  post-step sweep (3.6.1). Each box geom also meets the brushes and triangles round
  it as a box, by face clipping, and each cylinder geom as a cylinder, by
  1.7's separating axes (3.6.1), so a world edge between its points meets
  its face. vcod's.
- Why the 26 points and the cut speed: with the hood's brush in place
  of its bounds box, the hood in the live gate below came down with its
  low corners on two floors 18 units apart and a 39-degree clip map
  surface (normal (-0.12, -0.61, 0.78)) between them through its
  centre. Each step the centre's move was cut to nothing
  while its speed (about 55 units/s along the ramp) stayed, so it never
  slept (1739 steps in 30 s). With the edge and face points the box
  meets the ramp. vcod then cut the centre's speed with its move; that
  guard is gone for 1.7's post-step sweep (3.6.1), which leaves the speed
  but zeroes both speeds on a stop under a fraction of 0.001, and a
  centre held that way sleeps under it
  (`a_box_held_on_its_centre_by_a_ridge_comes_to_rest`, on the box's 8
  corners alone). VERIFIED (the runs, ours).
- Tests: `physics::tests` (free fall at `phys_gravity`, the push formula,
  a 0.8 bounce, a launched door that lands and sleeps, a fast part thrown
  into a corner staying out of all three planes, a tilted box settling
  flat, the clock, a model's geoms and brushes, a box held on its
  centre by a ridge, a box across a kerb's edge on its corners alone, a
  cylinder on a floor on its side and upright and against a kerb's edge,
  the post-step sweep and its once a frame, a pinched body's facing
  contacts and its drop of those it leaves, 0x5a2320's cut, the contact
  pass per geom and its walk's order, `msecStep` and the world clock, a
  push as a force over its body's next step),
  `cod4::part_edges` (3.6.1) and the live
  `cod4_live_a_blown_up_cars_parts_fly_and_come_to_rest`: vcod-server's
  `war` on mp_backlot, a held bot's frags bursting on the first
  destructible until `_destructible.gsc` blows the sedan up; its 9
  launched parts (both mirrors, both bumpers, both right doors, two wheels
  and the hood) flew 14 to 154 units off their spots and went to sleep
  within 1.6 to 2.1 s (92 to 120 steps), every point clear of the world
  and resting on it, the same on three runs; at 22 s a round slanting
  down onto the first resting part the client's round trace meets
  (`cod4_fx::first_contact` through `shot_solids`, the left mirror)
  pushes it 4.6 units on (one run), and it sleeps again. With the wheels
  as cylinders and the post-step sweep, the parts flew 13 to 167 units
  and slept within 1.3 to 2.2 s (79 to 127 steps), and the round pushed
  the left mirror 7.4 units on (one run). With the pinched body's
  contact pass (3.6.1) they flew 17 to 167 units and slept within 85 to
  126 steps, and the round pushed the left mirror 7.4 units on (one
  run). With the pass per geom, 0x5a2320's cut, the world's `msecStep`
  and the sweep once a frame they flew 14 to 164 units and slept within
  84 to 170 steps (the shot left mirror's 173 count those after the
  round), and the round pushed it 7.4 units on (one run). With each push
  a force over the body's next step at its named world's `msecStep` they
  flew 14 to 175 units and slept within 84 to 122 steps (the shot left
  mirror's 178 count those after the round), and the round pushed it 4.0
  units on (one run). VERIFIED (the tests, ours). How a part looks in flight and at rest is pending a
  human's look.
- The ragdoll (`cod4-player-anims.md` 10) keeps its point solver. This
  body could carry it once the bones are rigid capsules held by joint
  constraints, which it lacks. INFERRED (vcod's design).

#### 3.6.1 The world collider

Addresses in this section are `iw3mp.exe`'s (3.6), read with `objdump`.

What 1.7 collides a part against:

- The world is one geom of a class the game registers: 0x5a89d0 hands
  0x5a9bc0 a class whose collider getter is 0x5a89c0 and whose bounds
  function is 0x5b2930, and keeps the geom at 0xcc5ff28 (0x5a8a18).
  0x5a89c0 returns 0x5a8110 for any class it meets. Each world step
  collides that geom against the world's space (0x5a6609..0x5a6615 into
  0x5aa100), whose near callback 0x5a5f20 calls 0x5a95a0 with room for
  0x80 contacts. VERIFIED (disassembly). That 0x5a9bc0 is ODE's
  `dCreateGeomClass` and 0x5a95a0 its `dCollide` is INFERRED (their
  arguments).
- 0x5a8110 switches on the body geom's class (`geom` +4, the byte table
  at 0x5a89b0, the jumps at 0x5a8998): class 1 (a box, its sides at
  +0x4c..+0x54) is kind 1, class 11 kind 2, class 12 (a brush, its
  wrapper at +0x4c) kind 3, class 13 (radius +0x50, half height +0x54)
  kind 4, class 14 kind 5; classes 2 to 10 take the default arm
  (0x5a8370), which sets no kind. VERIFIED (the tables, the reads). That 13 is the cylinder
  is INFERRED (3.6's geom types, and its collider's depth below, a
  cylinder's reach).
- The cylinder's arm (0x5a86ad..0x5a871c): its radius (+0x50) twice
  over and its half height (+0x54) go to the collide context's half sizes
  (context +0x8c, +0x90, +0x94), its +0x4c to context +4, and its world
  box is a cube of `max(radius, half height)` (0x4093c0) times the double
  at 0x70ba88 (1.414214); it is narrow when twice its radius is under
  `phys_narrowObjMaxLength`. The colliders read context +4 less 1 as the
  column of the geom's rotation (context +0x44, stride 12) that is the
  axis (0x5a11f0). VERIFIED (disassembly). That a `physGeoms` cylinder
  gets axis 1, its rotation's first column, is INFERRED (0x5a55f0 sets a
  1 into the block it hands 0x5a4ab0, at 0x5a5613; the field not traced
  through).
- A box with a side under `phys_narrowObjMaxLength` (0xcc54428,
  registered at 0x5a4408: 4, 0 to FLT_MAX, "If a geom has a dimension
  less than this, then extra work will be done to prevent it from falling
  into cracks (like between the wall and the floor)") is marked narrow
  (0x5a8447). VERIFIED (the registration, the compares).
- The geom's world box grown by 1 (0x5a8370..0x5a8425) goes to 0x4f2c60
  for up to 0x400 leaves. Of each leaf (stride 0x2c at 0x14098fc) whose
  brush contents meet 0x2806c91, 0x5a7bc0 walks the brush tree (0x5a7a60,
  each brush of `cm.brushes` (stride 0x50) whose bounds meet the box and
  whose contents the mask) into the kind's brush collider through
  0x5a79b0; of each whose terrain contents meet it, 0x5a7ea0 walks its
  AABB trees (0x5a7d20, nodes culled against the box at 0x5a7c40, each
  partition once by a check count at 0x5a7d8e) into the kind's triangle
  collider (0x5a7dd7). Under `phys_collUseEntities` (0xcc5444c) 0x5a7ef0
  adds the entities. VERIFIED (disassembly). That 0x4f2c60 is
  `CM_BoxLeafnums` is INFERRED (its arguments and use).
- The colliders by kind 1 to 5: against a brush 0x59a610, 0x59bc90,
  0x59b370, 0x5a1330, 0x59ea10 (the jumps at 0x5a7a40); against a
  partition's triangles 0x59c500, 0x59c460, 0x59c300, 0x5a17c0, 0x59ed10
  (at 0x5a7e88). VERIFIED (the tables).
- A box against a brush (0x59a610): 0x599da0 builds the windings of the
  brush's 6 + `numsides` sides and returns the least deep of its sides
  against the box; 0x5998e0 hands each side's winding points, which are
  taken into the box's frame (its centre at +0x38, rotation +0x68..+0x88,
  half sizes +0x8c) and give the least deep of the box's six faces
  (0x59a700..0x59a8ac). 0x59a98f..0x59a9a9 compares the two: one way a
  brush side goes to 0x59a2b0 with the box, the other a box face takes the
  brush's windings through 0x598d60 and 0x597fd0. A narrow box then also
  takes each side's winding clipped by its six faces (0x59a500, through
  0x597800, from 0x59aabe). VERIFIED (the calls and loops). That the less
  deep is the reference, that these are polygon clips and that no axis
  between two edges is tried is INFERRED (the compare's sense not traced
  to its signs; 0x597800's plane arguments; no cross product among the
  tests).
- A box against triangles (0x59c500) hands the partition's triangles to
  0x5b1620 with the box's centre (+0x38), rotation (+0x44) and half sizes
  (+0x8c). VERIFIED (the call). That 0x5b1620, among ODE's own code, is
  ODE's box against a triangle list is INFERRED.
- A cylinder against a brush (0x5a1330): each of the brush's six axial
  planes (built by 0x596ae0 off its bounds) and then each side's plane
  gets a depth from 0x5a11f0: the float at 0x70af5c (-FLT_MAX) when both
  the geom's centre (context +0x38) and the body's position (context
  +0x98, body +0xe0) lie behind it, else the centre's distance off it less
  `h |n . a| + r sqrt(1 - (n . a)^2)`, `h` the half height, `r` the
  radius, `a` the axis. A depth at or over 0 ends the collide with no
  contact. Of the faces with edges (the brush's `edgeCount` bytes at
  +0x40 for the axial ones, a side's byte +10), the largest depth is the
  reference; its winding (0x5998e0) goes to 0x5a0f70. With the narrow
  flag (context +0xa4), each other axial face whose side of the bounds
  the centre lies past, and each other side whose plane the centre lies
  in front of, goes to 0x5a0f70 too (0x5a152e..0x5a17a6). VERIFIED (the
  decompiled control flow and the compares).
- A cylinder against triangles (0x5a17c0): each triangle of the
  partition that 0x596c70 does not cull against the centre and the
  context's +0x30 goes to 0x5a0f70. VERIFIED (the calls). That +0x30 is
  the length 0x5616f0 and 0x401050 give at 0x5a88cf, the geom's half
  diagonal, is INFERRED.
- 0x5a0f70 takes one polygon, its plane `(n, d)`: nothing unless the
  centre or the body's position lies in front of it; then 0x59fc90 runs
  a separating axis test, each axis through 0x59f5d0: the axis turned so
  its dot with a reference vector is not positive, the cylinder's reach
  along it `r sqrt(1 - c^2) + h c` (`c` the axis's dot with it, absolute),
  the polygon's corners projected off the centre; apart (no contact) when
  the reach is under the nearest or the farthest is under minus the
  reach, else the overlap `reach - nearest` replaces the best when, plus a
  bias, it is under it (the best starting at 0x70af58, FLT_MAX). The axes
  in order and their codes: the plane's normal negated (1), the cylinder's
  axis (2), each edge crossed with the axis (3 on), skipped when the
  axis's dot with the normal is at most the float at 0x70b880 (0.02), and
  some only tested, never recorded, by the sign of the edge's in-plane
  normal against the axis; the line from the axis to each corner; then
  for each corner and each cap centre `c +- h a`, 0x59f9f0's axis from the
  cap's rim to it with a bias of 1.0 (the float 0x3f800000 it passes).
  An axis shorter than the float at 0x70b234 (1e-5) is skipped. VERIFIED
  (the decompiled code and the constants). Which vector the axes are
  turned against is INFERRED (`eax` into 0x59f5d0 not traced; the
  overlap's sense says the plane's normal).
- 0x5a0f70's arms by the best code: the normal with the axis's dot under
  the double at 0x70af08 (0.9), 0x5a02b0, the cylinder's side line toward
  the plane (its axis's ends at `c + r u +- h a`, `u` the best axis less
  its part along `a`, normalized, at least the double 1e-5 at 0x70b720)
  clipped to the polygon (0x597f30), each end behind the plane a contact
  (0x59ee60, `d - n . p` at least 0); the normal otherwise, 0x5a09a0, the
  cap facing the plane against the polygon (0x5989f0); the axis,
  0x59f460, the polygon cut to the cylinder's circle (0x59f170, up to
  256 points), each point within the facing cap a contact of that depth;
  an edge, 0x5a0ef0, the nearest points of the edge and the axis
  (0x560050); a corner, 0x5973e0, one contact; a cap's rim against a
  corner, 0x5a0740, the cap's circle cut by a plane (0x5a0440's chord).
  Every contact carries the best axis and its overlap. VERIFIED (the
  calls); what 0x5989f0 and 0x560050 compute inside is INFERRED (their
  arguments).
- `Phys_RunToTime` (0x5a6d50) does nothing unless the time is past the
  world's (block +4, 0x5a6d7d). Then it stores the world's time as the
  frame's start (block +0, 0x5a6d90), copies each body's position (body
  +0xe0) to its userdata (body +0xc) +0x10 and its rotation to +0x1c
  (0x5a6da0..0x5a6deb), once, before the frame's steps; after the last
  step 0x5a6c10 runs on each body of the world (0x5a6e6d..0x5a6e85), which
  runs 0x5a6930 on one not disabled (body +0x18 bit 4) and then sets that
  bit on one whose position falls outside a box it reads
  (0x5a6c43..0x5a6cb9). VERIFIED (disassembly). So the sweep below runs
  once a frame, from where the frame began to where its last step ended.
  INFERRED (the order).
- 0x5a6930 reads a size `m` off the body's first geom (body +0x1c, the
  last ODE gave the body) through 0x5a9a20: a box (class 1) half its
  least side; a brush (class 12, 0x5a684f) its bounds times the double at
  0x70afb8 (0.01), x and y cut to z's size about their middles where
  longer (0x561910), then the top x; a cylinder (class 13, 0x5a688b) its
  radius times 0.7071 (0x70ae70), or its half height when that is less.
  It sweeps when the position moved since the copy, or the state
  (userdata +0x4c) is 0 or 1 (0x5a6a79..0x5a6ab2), from the copy to the
  new position under 0x2806c91 (0x4f4d30, called at 0x5a6aea): with `m`
  under twice `phys_csl` (0xcc54474) a point (corners at -0.001 and
  0.001, 0x70b460, 0x70b278), else a box of `0.2 m` (the double at
  0x70b3e8), or of `max(m - 2 phys_csl, 0.001)` when `0.8 m` is under
  twice `phys_csl`. The state becomes 1 when the trace started solid,
  else 2 (0x5a6af1..0x5a6b02). A hit not started solid puts the body
  where the trace stopped (0x432700, `dBodySetPosition` 0x5b6e40), a
  point's fraction first less `0.5 m` times the move's inverse length
  (0x4032e0, 0x403a90, the fast inverse root, floored at 0 by 0x4093c0);
  a fraction then under 0.001 zeroes the linear and angular velocity
  (body +0x130, +0x140, 0x5a6bd0..0x5a6bf0). No other speed changes.
  VERIFIED (disassembly).
- The state's other writers: `Phys_ObjCreate`'s helper stores its
  block's +0x60 (0x5a4f8d), and `Phys_ObjCreate` writes 0 there
  (0x5a4fbe), INFERRED (the block's passing not traced); the near
  callback 0x5a5f20 stores 2
  when the collide gives no contact (0x5a612d); 0x5a2280, run first in
  the contact pass 0x5a2760, stores 1 on both bodies when two of the
  pair's contacts' normals dot under the double at 0x70b268 (-0.99,
  0x5a22f6, 0x5a2308). VERIFIED (disassembly). Of the other stores at
  +0x4c in 0x5a0000..0x5b0000, 0x5a2d75's goes through the worlds' table
  at 0xcc54478 and the rest (0x5a4d38, 0x5a8db9, 0x5a8df2, 0x5a8e32,
  0x5a9d46, 0x5aeb76) are not traced to their base. INFERRED (the base
  registers). So the sweep runs
  every step a body moves, and on an unmoved one only while it is new,
  started solid or pinched between facing contacts. INFERRED (the gate
  above).
- Which body is which: `dSpaceCollide2`'s stand-in 0x5aa100 walks the
  space's geoms and calls the near callback 0x5a5f20 with a body's geom
  as `o1` and the world geom as `o2` (pushes at 0x5aa129..0x5aa12b),
  skipping a geom whose body is disabled (body +0x18 bit 4) or whose
  enabled flag (geom +8 & 8) is clear. 0x5a5f20
  reads each geom's body (+0x10), so `b1` is the part's and `b2` none,
  and the contact joint is attached `(b1, b2)` (0x5a2add, 0x5b72e0).
  VERIFIED (disassembly). That each normal points out of the world into
  the part is INFERRED (ODE's convention for `dCollide(o1, o2)` and the
  joint pushing `b1` along it).
- The collide gives up to 0x80 contacts into a list whose count follows
  them (+0x1800). With none 0x5a5f20 stores the state 2 (below); with 1
  to 4 the contact pass takes them as they are; with 5 or more 0x5a2320
  (0x5a6078, `cmp ebp,5`, call at 0x5a60da) copies a cut of them into a
  second list (`esp+0x1854`) that the pass takes instead (0x5a60f3).
  VERIFIED (the branch and the calls).
- 0x5a2320 runs four helpers on the contacts and a tag per contact
  (0x5a2375..0x5a23d0; between them, under the byte 0xcc5ff20, 0x5a1bf0
  and 0x5a1c40 print, and under the dvar at 0xcc54404 0x5a21d0 draws
  each contact first). VERIFIED (the calls).
  - 0x5a18e0 seeds three clusters by normal: every tag -1, then contact
    `k0 = 104729 mod count` (the immediate 0x19919, `idiv`) tag 0; the
    first contact whose normal's dot with `k0`'s is least (each taken
    when the best so far is over it, from FLT_MAX at 0x70af58) tag 1;
    the first whose larger dot with the two is least tag 2. VERIFIED
    (disassembly).
  - 0x5a1aa0 sums the normals of each tag's contacts into three means,
    normalizes them (0x4076d0, which leaves a zero vector zero), and
    five times over gives each contact the tag of the mean it dots most
    with (from -FLT_MAX at 0x70af5c, a later mean only when strictly
    larger) and takes the means again (the counter against 5 at
    0x5a1b2f). VERIFIED (disassembly).
  - 0x5a1cc0: for each pair `i < j` of the three, a mean dot over the
    double 0.999 (0x70b270) gives every contact tagged `j` the tag `i`,
    the means as they were. VERIFIED (disassembly).
  - 0x5a1e40 empties the output, sums each tag's normals again, and per
    tag 0 to 2: the sum normalized (0x55f6e0) is `N`; two tangents,
    `T1 = N x u` normalized with `u` (0, 0, 1) when `|N.z|` is under 0.5
    (0x6be648) and (0, 1, 0) otherwise, and `T2 = N x T1` normalized
    (0x5a1d80). Walking the tag's contacts in order, each later one of
    the tag within a squared distance under 0.01 (0x70b87c) of it loses
    its tag; each one walked counts, and keeps the least and the most
    of `T1 . x` and `T2 . x` (a later one only when strictly past) and
    the deepest (`+0x20`, strictly). A tag with any is copied out as its
    least along `T1`, its least along `T2`, its most along `T1`, its
    most along `T2` and its deepest, each skipped when it is one already
    copied for the tag (0x5a206d..0x5a218b). VERIFIED (disassembly).
  So the pass sees at most 15 contacts, at most five per cluster of
  normals: the cluster's extremes along two tangents and its deepest.
  INFERRED (the code above).
- The contact pass 0x5a2760 (contacts, `b1`, `b2`, the surface, the
  world), each contact ODE's 0x30-byte `dContactGeom` (position +0,
  normal +0x10, depth +0x20): 0x5a2280 first; then for each body a flag,
  its state (userdata +0x4c) equal to 1 (0x5a27ae), and a point, the
  body's position (body +0xe0), or the userdata's +0x50 when the world's
  byte at 0xcc54498 is set (0x5a2785, 0x5a27b5). VERIFIED (disassembly).
  Nothing in `.text` stores that byte: the world block (stride 0xcc from
  0xcc54484) is reached only at 0x5a2435, 0x5a2785, 0x5a5009, 0x5a65e0,
  0x5a66f7, 0x5a6cd3, 0x5a6d61, 0x5a6ec0 and the start-up loop at
  0x5a2d2b, and none of them writes +0x14. INFERRED (every reference to
  the block read; a store through some other pointer not ruled out). So
  the point is the body's position.
- With `b1`'s flag set, or else `b2`'s, 0x5a25d0 takes that body's point
  `p` and the contacts: for each `i` and each `j` after it whose normals
  dot under -0.99 (the double at 0x70b268), contact `j` is copied over
  `i` when `n_i . p - n_i . x_i` is under 0 (the double at 0x70adf0, 0.0;
  `x_i` the contact's position), then the last contact over `j` and the
  count dropped by one, and `j` goes on to the next slot, so the contact
  moved in is never set against `i` (0x5a25fe..0x5a2748). VERIFIED
  (disassembly). So of two facing contacts the first stays when the
  centre lies on its plane or in front of it, else the second takes its
  place. INFERRED (the arithmetic, the normals' sense above).
- Then each contact in turn: with `b1`'s flag, its point's velocity on
  `b1` (`dBodyGetPointVel`, 0x5b70d0: the linear velocity +0x130 plus the
  angular +0x140 crossed with the position less the body's) normalized
  (0x5b7460, ODE's `dSafeNormalize3`, a zero vector giving (1, 0, 0) at
  0x5b762c..0x5b7648) dotted with the normal; over the float at 0x6df494
  (0.01) the contact is skipped (0x5a2894, `test ah,0x41`). The same with
  `b2`'s flag for `b2`. A contact kept takes the jitter (0x5a2918) and
  its joint (0x5a2a23..0x5a2ae2, up to 0x1000 in the world's group).
  VERIFIED (disassembly). So a pinched body loses each contact its point
  is leaving, and a still one each whose normal leans over 0.01 toward
  +x. INFERRED (the arithmetic).
- A body sleeps by ODE's own rule (0x5b9b70, run first in the world step
  0x5b9d40, called at 0x5a6625 after the collide): for each body with
  auto-disable and not disabled (flags & 0x14 == 0x10), idle when its
  squared linear speed is at most +0xc8 and its squared angular speed at
  most +0xcc; an idle step takes one off the steps left (+0xdc) and the
  step size off the time left (+0xd8), any other resets them to +0xd4
  and +0xd0; both under 0 sets the disabled bit (0x5b9c49). VERIFIED
  (disassembly). 0x5b7350 sets a world's idle steps to 10 (0x5b7376),
  but the start-up loop that calls it for each of the three worlds (at
  0x5a2d34, the loop 0x5a2d32..0x5a2da1) then writes 0 over them
  (0x5a2d83) and 1 to the auto-disable flag (+0x4c, 0x5a2d75). VERIFIED
  (the instructions). A body copies the world's +0x3c..+0x48 into
  +0xc8..+0xd4 when it is made, takes its auto-disable bit from +0x4c
  and starts its time and steps left at +0xd0 and +0xd4
  (0x5b6d07..0x5b6d48). VERIFIED (the instructions). So the steps left
  go under 0 on the first idle step and the time alone decides: a body
  sleeps on the idle step that takes its time left under 0. VERIFIED
  (the instructions, 0x5b9c0a..0x5b9c49).

vcod (`ClipMap::world_solids`, `physics::box_contacts`,
`physics::cylinder_contacts`, `Body::sweep`):

- The gathering is 0x5a8110's: the leaves the box grown by the step's
  reach touches (`box_leafs`), their brushes and AABB tree triangles
  under the mask, each once. vcod's reach: the most any of the box's
  corners travels in the step, plus the 0.125 skin.
- Each box geom of a part or a dynamic entity (`Shape::boxes`: the
  bounds box, a `physGeoms` box, a brush no side cuts) meets each brush
  as 0x59a610 does: the least deep of the brush's faces and the box's
  three axes is the reference, and the other's face most against it is
  clipped to it; each clipped point within the reach is a contact. The
  box's face clipped to a brush face is the one whose normal is most
  against it. vcod's.
- The narrow pass: a box with a side under `phys_narrowObjMaxLength`
  (`Dvars::narrow_max_length`, 4) also takes as references the brush's
  other faces its centre lies in front of whose windings its six faces
  leave something of, as 0x59aabe..0x59ac8a's loop picks them (the
  axial sides by the centre against the brush's bounds, the others by
  the plane). VERIFIED (the compares); the reference's clip is vcod's
  (above). `a_narrow_box_over_a_brushs_edge_takes_its_other_faces`.
- Past 1.7's colliders: each brush edge that runs more than the skin
  into the box pushes it out along the box face nearest the point, at
  the clipped edge's ends and middle, but for the edges of the face the
  reference pair took, which its clip already meets (with them a resting
  sedan bumper in the live gate below slept with a point in the floor). Neither face reference holds a
  ledge's edge across a box's face: mp_crash's red toolbox slept on a
  ledge's edge half a unit into it without this. A triangle's three
  edges take the same pass: with the post-step sweep, mp_crash's
  `me_picture4` and `me_picture7` dropped turned 90 degrees onto a mesh
  stair at (135, 2383, 236) took its nosing through the frame in the step
  they met it, and the sweep's stop under 0.001 froze them 4 units in.
  vcod's.
- A triangle is always the reference and is met from its front. It is
  skipped once the box's centre is behind it, unless it is a floor's
  (normal z at least 0.7) and the centre lies off its edges, as over a
  step's corner: a picture frame tipped across a mesh stair's tread
  slept 4 units into the step without it. A box face as reference would
  catch the seams between a mesh's triangles, and a back triangle that
  is not a floor's would push a box onto a handrail's tube (a sedan door
  on mp_backlot did). vcod's. Not ported: 0x5b1620's own tests.
- Each cylinder geom of a part or a dynamic entity (`Shape::cylinders`,
  a `physGeoms` type 4, its axis the orientation's first row) meets each
  brush as 0x5a1330 and each triangle as 0x5a17c0 do
  (`physics::cylinder_contacts`): the faces' depths, the reference, the
  narrow pass, and on each polygon 0x5a0f70's gate and 0x59fc90's axes in
  order with their 0.02, 1e-5 and 1.0 and the 0.9 between its two face
  arms, the overlap and the separation within the step's reach. vcod's:
  the axes turned against the plane's normal; an edge's axis recorded
  only with the centre outside that edge; the upright cap as 8 points
  round its rim clipped to the polygon; a rim-against-corner axis's
  contact at the corner, as a corner's; each contact's point on the
  cylinder. Past 1.7's colliders, a brush's edges but the reference
  face's that run more than the skin into the cylinder push it out along
  its cap or side nearest the point, as the box's edge pass does: a stock
  junk tire tipped onto one of mp_crash's steps slept with the step's
  edge a unit into its cap without it. The pass runs only on a brush
  whose reference face 0x5a0f70's gate passes: the live gate's wheels
  start with their centres on their car clip's floor face, inside the
  clip, and took its edges as contacts that ate their launch. The 16
  points round each cylinder still sweep as the box's do.
- The sweep after a frame's steps is 0x5a6930's (`Body::sweep`, from
  `Body::end_frame`), in place of the centre guard vcod had (3.6): from
  where the frame began, its size off the body's last geom
  (`Shape::sweep_half`), its gate on a move or the state, its point or
  box, the point's back-off, the stop and the zeroing under 0.001, the
  state it writes. vcod's. `the_post_step_sweep_runs_once_a_frame`: a
  centre at 100 units/s whose first of two 17 ms steps ends where the
  sweep stops it keeps its speed when swept once over the frame, and
  loses it all when swept each step, as vcod had it.
- The contact pass (`Body::contact_pass`, `physics::pinch`,
  `physics::unwedge`, `physics::reduce`) runs per geom, as the near
  callback does: a geom with no contact stores 2; five or more go
  through 0x5a2320's cut as above; then 0x5a2280's 1, and on a body in
  state 1 0x5a25d0's pick between facing contacts (its point the body's
  position), the walk's skip of the moved-in contact, and 0x5a2760's
  drop of each contact whose point leaves it, the velocity the one
  before the step's gravity, a still point's taken as (1, 0, 0); then
  the jitter on each contact kept. The geoms go in 0x5aa100's order,
  below. vcod's: a geom's
  contacts here are those its box or cylinder collider gives in the
  0.125 skin or inside, ODE's overlaps, or, for a brush with sides,
  which has no collider here, its points'. A box's or cylinder's points
  and the contacts met further on within the step's travel stay out of
  the pass and are kept: with a box's points in the cut, 78 of the edge
  drops in `cod4::part_edges` below failed, most with a point left in
  the world. VERIFIED (the run, ours). The points found in the solver's
  second pass take the drop alone.
- The walk's order. 0x5aa100 walks the space's list from its head (space
  +0x50) along each geom's +0x20 (0x5aa10f..0x5aa139). A geom is built
  by 0x5a9790 (each `physGeoms` entry in its order, 0x5a5710's loop to
  0x5a57bb, through 0x5a4ab0's creators, e.g. 0x5a8d50, into 0x5a9970),
  which adds it to the world's space (0xcc546e8 by world, 0x5a4bbc) at
  the head (0x5a9f00, the space's add) and to the body's list at its
  head (body +0x1c, the next at geom +0x14, 0x5a982e..0x5a9839). The
  space's `dirty` (0x5a9f80, the space vtable 0x6f8200's seventh slot)
  moves a geom to the head; `dGeomMoved` (0x5a9cc0, and inlined) calls it
  on a geom whose dirty flag (geom +8 & 1) is clear and sets it. Each
  step's space collide (0x5aa070, called at 0x5a65d8) first runs
  `cleanGeoms` (0x5aa010), which clears the flag from the head on. After
  each step 0x5a6640 walks each body not disabled and, through the
  body's list, moves each of its geoms (0x5a668f..0x5a66d7). ODE's own
  body step (0x5b8380) moves none. VERIFIED (disassembly). So on a body's
  first step the walk meets its geoms last made first, as they went in,
  and on every step after first made first, the body's list walked last
  made first and each put at the head in turn; a body asleep is not
  moved and keeps that order. INFERRED (the code above; nothing found
  cleaning the flags between a body's making and its first step).
  `Body::contact_pass` walks them so; a walk by the steps taken. vcod's.
  `the_contact_pass_pinches_on_one_geoms_facing_contacts_only`: of a
  geom pinched between two walls and a geom on a floor the body rises
  off, the floor's contact stays on the first step and is dropped on a
  later one. VERIFIED (the test, ours).
- The push as a force: `a_push_is_a_force_over_the_next_step_of_its_body_world`
  (a push at 17 spent 25 / 17 over a 25 ms step and not again; one at 28
  spun 17 / 28 over a 17 ms step), and in `cod4::part_edges`
  `a_round_on_a_flying_part_is_a_force_at_world_zeros_msec_step`: the 36
  parts below in flight, world 1 at 28 ms, a round on the hood naming
  world 0's 17 moves it 1.647 times what the same round naming world 1
  does (28 / 17). As one whole impulse each moved it the same. VERIFIED
  (the runs, ours).
- The sleep is ODE's rule above on the step's end speeds, `idle` the
  time under both, which is 1.7's rule too: 1.7 zeroes the idle steps
  (0x5a2d83, above). vcod's: idle on a speed strictly under the
  threshold where ODE takes at most it.
- The contacts go into the impulse solver with the points'. vcod's.
- Measured: `cod4::part_edges` drops mp_backlot's sedan hood (its brush
  geom, a box) and left front door (its bounds box) from 24 units over
  six steps' edges (two level floors 8 apart, 6 to 18 high, with room
  over them, near the `mp_tdm_spawn`s), across, along and at 45 degrees,
  tipped 30 degrees: on the points alone 30 of the 36 drops slept with
  world solid at least a quarter unit inside the box (most of them 4
  units, through the hood's middle); with the boxes none, each asleep
  within 3.5 s (4.6 s with the post-step sweep). Its second row drops one
  def of each of mp_crash's 16 dynamic entity models with a box on six of
  mp_crash's steps the same way: on the points alone 167 of the 288 drops
  slept with world solid inside the box, the picture frames and crates
  mostly 4 units; with the boxes none (each asleep within 4.7 s with the
  sweep). Its third row drops one def of each of the 30 dynamic entity
  models of every `mp_` zone whose shape has a cylinder (bottles, cans,
  pails, pots, pans, plates, tires, hub caps, fruit, a propane tank, a
  hookah) on mp_crash's six steps, out to 30 s: on the 16 points alone
  480 of the 540 drops slept with world solid inside the cylinder, 361 of
  them 4 units; with the cylinders none, 537 asleep (533 within 10 s) and
  3 still rolling down a slope at 30 s, their speed their spin times
  their radius. VERIFIED (the runs, ours).
- Those 3 against the sleep rule (`com_bottle4` and both mangoes, turned
  45 and rolled -30): at 30 s the bottle's angular speed squared was 11.2
  and the mangoes' 1.49, over `phys_autoDisableAngular` squared (1), their
  linear speed 9 and 2.8, so ODE's rule keeps them awake too. Each was
  slowing under `phys_dragAngular`, and all three slept by 34.2 s (33.4,
  34.2, 34.2), 256 and 190 units off. The row now runs every drop to
  60 s and passes none on its roll: all 540 sleep, 515 within 10 s,
  with the pinched body's filter below and without it. The box rows
  sleep within 4.6 s (the parts) and 4.7 s (the props) with it.
  VERIFIED (the runs, ours).
- A wedged body: `pictures_wedged_between_two_walls_lose_one_walls_contacts`
  stands two 16-thick slabs on mp_crash's floor at its first open
  `mp_tdm_spawn` (207, 2291, 236), 0.9 of a picture frame's length apart,
  and lays each of `me_picture4` to `10` flat across them 40 over the
  floor, tipped until it fits with a unit to spare and turning at 2, 4 or
  8 rad/s to lie flatter. Each comes down to lean between the walls with
  one end on the floor. Pinched on 45 to 640 of its 72 to 663 steps, it
  keeps the first wall's contacts and loses the other's, and all 18 slept
  (within 1.2 to 11.3 s) with that end 0.02 to 0.20 units into the far
  wall and none into the near one, the floor at most 0.17 in. Without the
  filter all 18 slept within 1.3 s with no point in either wall. With the
  pass per geom and 0x5a2320's cut (below) they are pinched on 45 to 51
  of 71 to 75 steps and slept within 1.2 to 1.3 s, 0.15 to 0.18 into the
  far wall, none into the near one, the floor at most 0.05 in.
  VERIFIED (the runs, ours). That a wedged body on 1.7 slips into the far
  wall the same way is INFERRED (the rule above; no retail client runs
  here). How it looks is pending a human's look.
- The contact pass per geom: `two_geom_pictures_wedged_between_two_walls_are_never_pinched`
  cuts each of those frames across its length into two box geoms, one
  at each wall, its mass as it was. Neither geom's contacts face each
  other, so none of the 18 is pinched, and each slept within 1.3 s with
  no point in either wall and the floor at most 0.07 in. Run as one
  body's pass, as vcod had it, the frames were pinched on 20 to 51 of
  their steps and slid 0.15 to 0.18 into the far wall (the first eight
  runs read). VERIFIED (the runs, ours).
- The cut and the jitter on its contacts:
  `a_crate_flat_on_the_floor_takes_a_jitter_at_three_corners` lays
  mp_crash's `me_plastic_crate1` to rest on that floor and wakes it
  under one region whose least and most push are equal, so each kept
  contact with a normal z of 0.7 or more lifts it by the same speed. The
  box meets the four floor brushes under it in 16 contacts; 0x5a2320's
  cut keeps three, the face's corners but the one at its most x and
  least y, and the lift, read against the same step with no region,
  comes to 3.1 pushes. On the box's points, as vcod had it, it read 8.9;
  on all 16, uncut, 12.3. VERIFIED (the runs, ours). That retail's
  colliders give that floor's four brushes the same 16 is INFERRED
  (0x59a610's clips above).
- `msecStep`: `parts_dropped_together_step_at_world_ones_msec_step`
  drops the 36 parts of the first row at once in one world 1 at 16 ms
  frames: its first step is 17 ms, then 28 (36 awake) until they sleep,
  and all 36 slept within 2.4 s (115 steps) clear of the steps. With
  the pass per geom, the cut and the sweep once a frame, the three edge
  rows still pass: the parts slept within 2.3 s, and all 828 prop drops
  within the 60 s. VERIFIED (the runs, ours).

#### 3.6.2 Worlds 0 and 1 on 1.7, captured

Captured from the Windows client under Wine and gdb on 2026-11-26
(docs/dev/retail.md, "Worlds 0 and 1 from the first push to rest"), each
the first physics event after its map load, at `fixedtime` 17. The four
takes are `crates/cod4/tests/fixtures/phys/` (its README: the recipes, the
line format and each take's summary). Addresses are `iw3mp.exe`'s (3.6).

Where the dump reads, and the layout it rests on:

- The body list. `Phys_RunToTime` (0x5a6d50) walks its world from
  `dxWorld` +0 through each body's +4, copying the body's position (+0xe0)
  and rotation into its userdata (body +0xc, from +0x10)
  (0x5a6d92..0x5a6df2). VERIFIED (the instructions). In all four takes
  every body a `Phys_ObjCreate` call returned (its `eax` at the return
  addresses 0x434d4e, 0x48dcc8, 0x4a21be) was in that list at the next
  `Phys_RunToTime` entry, the list's length equalled the world's +8, and
  each body's +0 held its world. VERIFIED (the captures). So the scope's
  ODE 0.5 layout holds for these: `dxWorld` +0 the first body, +8 the body
  count; a body's +0 its world, +4 the next body.
- A body's geoms. Body +0x1c holds its first geom and the geom's +0x10
  the body. VERIFIED (the captures, every geom). No captured body had a
  second geom and every geom's +0x14 read 0, so the next-geom link at
  +0x14 is INFERRED (ODE 0.5's `body_next`).
- A geom: +4 its class, +0x18 and +0x1c pointers to its body's position
  (+0xe0) and rotation (+0x100), +0x44 and +0x48 all ones. A box (class
  1, vtable 0x6f822c) holds its three side lengths at +0x4c: the
  cinderblock's bounds box reads 16.005, 8, 7.97. A class 12 geom (vtable
  0x6f81c4) holds a `BrushWrapper` pointer at +0x4c. VERIFIED (the
  captures).
- Cylinders come wrapped. Every `physGeoms` cylinder in the takes (a
  bottle, a can, four tyres) is a class 6 geom (vtable 0x6f823c) on the
  body, whose +0x4c points to a class 13 geom with no body (+0x10 0) whose
  position and rotation pointers are the body's own (+0xe0, +0x100). The
  class 13 geom holds 1 at +0x4c, the radius at +0x50 and the half height
  at +0x54 (the tyre: 11.95 and 3.5, `phys_dump`'s 11.947635 and 3.5).
  The class 6 geom holds 1 at +0x50, 0 at +0x54, then a 3x4 rotation
  from +0x58 (the tyre's a quarter turn about z) and a position near
  zero at +0x88. VERIFIED (the captures). That +0x58 is the inner geom's
  frame in the body's is INFERRED (the values).
- The userdata (body +0xc, 0x50 bytes): +0xc points back to the body,
  +0x40 is the impact sound row, +0x44 the friction and +0x48 the bounce:
  the tyres read 0.8 and 0.8, the doors 0.5 and 0.1, `tire`'s and
  `car_door`'s (3.6). The contact pass's surface (0x5a2760's fourth
  argument) carries them: mode 0x301c, mu 0.3 and bounce 0.01 off the
  cinderblock's +0x44 and +0x48, bounce velocity 0.1, soft ERP 0.8 and
  CFM 1e-5. VERIFIED (the captures).
- The collide. Each `dCollide` in the near callback (0x5a5f96) pairs a
  body's geom with the world geom, class 15, which has no body. VERIFIED
  (every take). With 5 or more raw contacts the pass gets 0x5a2320's cut
  (0x5a6078..0x5a60f3). VERIFIED (the instructions). The cinderblock's 8
  raw contacts on each of its first 40 steps came out as 6. VERIFIED (the
  round take).
- World 0's `dxWorld` is at 0xd5ec6e0 and world 1's at 0xd5fc7f0, 0x10110
  on (section 1's stride). VERIFIED (the captures).

What the takes show:

- Each world's seed (+0x30) read 0x89abcdef at its first body in all
  four takes. VERIFIED (the captures). Both worlds start from it, as
  world 2 does (`cod4-player-anims.md` 10.5).
- The seed held on the car's second step, with all nine parts awake and
  seven in contact with the ground (1 to 6 contacts each into the pass), and on
  every step with every body asleep; it changed on every other step.
  VERIFIED (the car take). So a step with contacts does not always draw.
  The round's fourth seed (0x29ad4fbf) is the frag's first: one sequence,
  a varying number of draws a step. VERIFIED (the values). How many draws
  a step takes is P4's to read.
- `msecStep` stayed 17 on every step of all four, with at most nine
  bodies awake. VERIFIED (the captures). That matches 3.6's rule, 17 at
  32 awake or fewer.
- A push lands before the first step: the bullet push (0x5a5db4, the
  return from `Phys_ObjBulletImpact`'s 0x5a5900 call) and the blast's
  (0x48eaa6) are logged at step 0, and each pushed body's first line has
  zero velocity and the push in its force accumulator (+0x98). VERIFIED
  (the round, frag and car takes). The FX chunks get no push: they start
  with their velocities set, 109 to 380 u/s and 1.7 to 9.2 rad/s.
  VERIFIED (the watermelon take).
- Every body starts with flags 0x10 (auto-disable) and 0.9 s of idle
  time. VERIFIED (the captures).
- World 0 drops a body once it sleeps: the frag's bodies leave the list on
  the step after the one that sets their disabled bit. VERIFIED (the frag
  take; 13.2 read the destroy). World 1's sleeping parts stay in it: the
  car's nine were still listed 600 steps after the last slept. VERIFIED
  (the car take).
- The round: one MP5 round into a cinderblock of mass 20, 60 units off;
  it moved 1.9 units and slept on step 61 (1.02 s). VERIFIED (the round
  take).
- The frag: eight dynents woke (two plastic crates, a bottle, a can, two
  barrel shards, two tyres) and slept between steps 74 and 140 (2.4 s).
  An earlier take of the same throw woke the same eight and its bottle
  slept only on step 455 (7.7 s). VERIFIED (the two takes). The push
  points come off the shared random state (13.3), which other client code
  also draws on, so takes of one throw differ. INFERRED.
- The car: `_destructible.gsc` launched nine parts in one frame, 450
  frames after the first round. Four bounds boxes of mass 6, three brush
  parts (two of 25, one of 6) and the two tyres of 18. They slept between
  steps 89 and 132 (2.2 s), the farthest 99.5 units from its launch.
  VERIFIED (the car take).
- The watermelon: one round broke it into six FX chunks of mass 1, each a
  box. Four slept between steps 95 and 165. One was still rolling at 4
  u/s and 2.2 rad/s on step 600 (10.2 s), past `phys_autoDisableAngular`.
  Two fell through the ground near z -8 and came to rest at z -767.7.
  VERIFIED (the watermelon take). Our clip trace finds no ground under
  the first one's crossing, (3631.8, -124.8), down to -767.9, and ground
  at -5.9 under the second's, (3673.3, -43.5). VERIFIED
  (`ClipMap::trace`, ours). Why the second fell through is open: it
  started at z 29.6, 14 units under the watermelon's centre, held within
  2 units of that for 30 steps, then fell. VERIFIED (the take).
- Impact sounds over each take: 23 in the frag, 37 in the car, 6 in the
  watermelon, none in the round. VERIFIED (the captures).

#### 3.6.3 Worlds 0 and 1's masses and geoms, as ported

I ported how 1.7 builds a world 0 or 1 body off its model and preset
into `ode::obj` (the creation, the parts, the sound row) and
`ode::geom` (the geoms), P2 of the world 0/1 port. Addresses are
`iw3mp.exe`'s (3.6).

The creation:

- `Phys_ObjCreate` (0x5a4fb0; the world in `edi`, the position in `esi`,
  the velocity in `edx`, the preset in `ecx`, the axes on the stack)
  fills a block for 0x5a4d90: position, the axes' nine floats, velocity,
  zero spin and zero centre (+0x3c..+0x50), the preset's mass (+8 into
  +0x54), friction (+0x10 into +0x58), bounce (+0xc into +0x5c), state 0
  (+0x60), the world clock's time (+0x64, which 0x5a4d90 does not read),
  the preset's +4 (+0x68) and an enable byte 1 (+0x6c). VERIFIED
  (0x5a4fb0..0x5a507b).
- 0x5a4d90 stores the world's sleep thresholds first: the dvars at
  0xcc54450 and 0xcc54400 squared into `dxWorld` +0x3c and +0x40, the one
  at 0xcc54430 into +0x44, the same three `Phys_RunFrame` stores
  (0x5a64f0..0x5a6533). Then `dBodyCreate` (0x5b6b10; at the cap the
  "more than %i" message of 0x200 at 0x5a4e10), in world 2 only the flags
  less 3 and finite rotation (0x5a4e2e..0x5a4e39), a 0x5c-byte phys object
  off the free list at 0xcc5ff00, cleared, into body +0xc; the position
  (`dBodySetPosition` 0x5b6e40), velocity (+0x130), spin (+0x140), and the
  rotation from the axes through 0x596b80 and `dBodySetRotation`
  (0x5b6ec0). The object's offset (+0) takes the block's +0x48, then
  0x5a4ab0 runs a type 0 block at the block's mass about the negated
  +0x48; last the object gets the body (+0xc), the block's position
  (+0x10) and axes (+0x1c), friction (+0x44), bounce (+0x48), state
  (+0x4c) and sound row (+0x40), and an enable byte of 0 sets the
  disabled bit. VERIFIED (0x5a4d90..0x5a4fab).
- 0x5a4ab0 (world, body, mass, block; the centre in `eax`) clears a
  `dMass`, moves the body (0x5a47c0), and switches on the block's +0
  through the table at 0x5a4d70: 0 a sphere's mass of radius 1
  (0x5b5c70) and no geom; 1 `dMassSetBox` (0x5b5e50) and `dCreateBox`
  (0x5aa500) of the sides at +0x2c; 2 `PhysMass` (0x5a49d0) and class 11
  (0x5a8d50) of a u16 at +0x2c; 3 `PhysMass` of the moments at +0x30 and
  products at +0x3c and class 12 (0x5a8d90) of the `BrushWrapper` at
  +0x2c; 4 0x5b5dc0 and class 13 (0x5a8dd0) of the axis (+0x2c), radius
  (+0x30) and length (+0x34); 5 the capped cylinder of density 1
  adjusted to the mass and class 14 (0x5a8e10). A geom whose block has
  +4 set is wrapped in a class 6 transform (0x5aeb50): the inner geom
  leaves its space (the space's vtable +0x14) and its body's list
  (0x5a9850) but keeps the body's position and rotation pointers, the
  transform takes the block's axes (+8) at +0x58 through 0x596b80 and
  zeros at +0x88..+0x94. Last `dBodySetMass` (0x5b6f70). VERIFIED (the
  table and each arm's instructions).
- 0x5a47c0 (the body; the centre in `edi`): the model's origin is `pos +
  R offset` (0x5a4740), the offset becomes the negated centre, the body
  goes to `origin + R centre`, every class 6 geom on the body moves its
  frame by `-offset - centre` (two stores a part), and the object's +0x10
  takes the new position. No other class is touched: a geom outside a
  transform sits at the body's point. VERIFIED (0x5a47c0..0x5a49c8).
- `Phys_ObjSetCollisionFromXModel` (0x5a5710; the model in `eax`): with
  `physGeoms` (+0xd8: count +0, geoms +4, `PhysMass` +8) each 0x44-byte
  geom in order at the body's mass: one with a brush through 0x5a5420
  (type 3, no transform, about the list's centre); type 1 through
  0x5a52e0 (sides twice `halfLengths`, each stored, turned by
  `orientation`, about `offset`); any other type through 0x5a55f0 (type
  4, axis 1, radius `halfLengths` y at +0x3c, length `halfLengths` x at
  +0x38, turned and about `offset` likewise). Then the list's `PhysMass`
  at the body's mass, `dBodySetMass`, and the body moved to the list's
  centre (0x5a57bd..0x5a57fc). VERIFIED (0x5a5710..0x5a580a). So a
  `PhysGeomInfo` of type 0, 2, 3 or 5 would also become a cylinder.
  VERIFIED (the compare at 0x5a5764). With no `physGeoms`, the bounds
  box below on the model's bounds (+0xac, +0xb8), any axis whose bounds
  are equal (`fucompp`, 0x5a5847..0x5a5878) making them -50..50.
  VERIFIED.
- 0x5a5240, a box on bounds: the midpoint `(mins + maxs) 0.5` and the
  sides `maxs - mins`, each stored, a type 1 block with no transform at
  the body's mass about the midpoint. 0x5a54a0, a dynamic entity's
  cylinder: the same midpoint, a type 4 block on axis 3, radius half the
  x size unless `x - y` stored is under 0 (then half the y), length half
  the z size, in world 0. VERIFIED (0x5a5240..0x5a52da,
  0x5a54a0..0x5a557d).
- A dynamic entity (0x490240): with `physGeoms`, 0x5a5710 in world 0; a
  brush model (def +0x26) through 0x5a53a0 (type 2, its `PhysMass` at
  def +0x38); else its bounds (0x48fd00, a brush model's through
  0x4ed9c0), the cylinder when the preset's +0x28 is set, the box
  otherwise, no axis widened. VERIFIED (0x490240..0x4902ba). The launch
  (0x434d49, 0x434d8d) and the FX models (0x4a21b9, 0x4a21cc) take
  0x5a5710 in world 1. VERIFIED (the calls).

The masses, each IW's taking the total mass where ODE 0.5's takes a
density (INFERRED, the upstream source):

- The sphere (0x5b5c70): `I = ((m 0.4) r) r` stored on the diagonal,
  0.4 the float widened at 0x70b250. VERIFIED (the instructions).
- The box (0x5b5e50): of the full sides, `(lz^2 + ly^2) (m / 12)`,
  `(lz^2 + lx^2) (m / 12)` and `(ly^2 + lx^2) (m / 12)` on the stack,
  12 the double at 0x70b238, each stored once. VERIFIED (the
  instructions).
- The cylinder (0x5b5dc0, the axis 1..3 in `edx`): `r^2` stored, the two
  across `((1/12) L L + 0.25 r^2) m`, the axis's `(m 0.5) r^2`, each
  stored (1/12 the float widened at 0x70b240, 0.25 at 0x70b058, 0.5 at
  0x70ace8). The class 13 geom keeps the same `L` as its +0x54, which
  0x5a688b's sweep size reads as a half height. VERIFIED (the
  instructions).
- `PhysMass` (0x5a49d0): each moment over 0 times the mass, else 100
  (the float at 0x6f9e58, a NaN too); each product times the mass, not
  negated, at `I01`/`I10`, `I02`/`I20` and `I12`/`I21`; the centre 0.
  VERIFIED (the instructions).

The geoms:

- 0x5a9790 (space, placeable, body), the constructor every class takes:
  vtable 0x6f81b4, class -1, flags 0xb or 0xf when placeable, the links
  and the bounds 0, the category and collide bits -1, the geom put in
  the space; a placeable one points at its body's position (+0xe0) and
  rotation (+0x100) and goes first on the body's list (+0x1c, the geom's
  +0x14 the old first). VERIFIED (0x5a9790..0x5a984d).
- The box: class 1, vtable 0x6f822c, the sides at +0x4c..+0x54. Classes
  11..14 (0x5a9970): IW's table at 0xcc78168, 0x14 bytes a class from
  11, its +0x10 byte the placeable flag and +0 the bytes cleared from
  +0x4c; vtable 0x6f81c4. Class 12 holds its `BrushWrapper` at +0x4c and
  the centre the body moved to at +0x50, class 13 its axis, radius and
  length at +0x4c..+0x54. VERIFIED (0x5aa500..0x5aa544,
  0x5a8d90..0x5a8e01, 0x5a9970..0x5a99b6).
- The transform (0x5aeb50): placeable on the body, vtable 0x6f823c, class
  6, the inner geom (+0x4c) 0, `cleanup` (+0x50) 1, `infomode` (+0x54) 0,
  the frame's rotation (+0x58) the identity and position (+0x88) 0, a
  final rotation at +0x94 the identity and final position at +0xc4 0.
  VERIFIED (0x5aeb50..0x5aec1d). 0x5aed10 points the inner geom at +0xc4
  and +0x94 before sizing it (VERIFIED, 0x5aed1e..0x5aed37), so those are
  the final pose, and the frame's position written as four words
  (0x5a4d54, 0x5a4994) clears the final rotation's first word. VERIFIED
  (the offsets; every captured transform reads 0 there). That the
  recompute on the dirty bit (0x5aed13) always runs before a read is
  INFERRED. Against ODE 0.5's `dxGeomTransform` (`obj`, `cleanup`,
  `infomode`, `final_pos`, `final_R`) IW put the frame before the final
  pose. INFERRED (the upstream layout).

The impact sound row (the preset's +4, the object's +0x40):

- 0x43e030 takes the loaded `PhysPreset`s (asset type 1, at most 0x32)
  through 0x489120, which walks the asset hash's 0x8000 buckets
  (0xe62a80) up, each chain from its head; a new asset goes to its
  bucket's head (0x489442..0x489459). The bucket is 0x488fd0's: from the
  type, `h = 31 h + c` for each character through 0x670945, a backslash
  as `/`, the low 15 bits. For each preset 0x43dfb0 looks its
  `sndAliasPrefix` (+0x1c) up among the rows so far (0x571a90, length
  0x7fffffff) and stores the match's index at +4, or makes a new row
  (0x43de60, 0x74 bytes from 0x84db98) and stores its index. VERIFIED
  (the instructions). That 0x670945 lowers and 0x571a90 compares without
  case is INFERRED (the call shapes).

What the port matches:

- `phys_create::fixture_bodies_build_word_for_word` builds every body
  of the four takes of 3.6.2 at its captured pose through
  `PhysWorld::obj_create` and `set_collision_from_xmodel` or
  `dynent_collision`. All 24 equal the capture word for word: the 17
  `dMass` words, the 13 of the inverse, the object's offset, sound row,
  friction, bounce and state, the geom's words from +4 to its class's
  last field, the inner geom's and the brush's bounds. VERIFIED (the
  run, ours). The model: a dynamic entity's is the def at the origin
  the capture's pose and offset give; a car part's the one among the
  sedans' parts that builds it, the silver sedan's named (the other
  colours' build alike, and both tyres are `_wheel_lf`); an FX chunk's
  the one watermelon visual that builds it (the FX system picks the
  visual at random). VERIFIED (the run).
- The sound rows come out of the hash order over the zones loaded
  (`code_post_gfx_mp`, `localized_code_post_gfx_mp`, `common_mp`,
  `localized_common_mp`, the map) on all three maps: mp_crash's
  cinderblock 0, shards 4, crates, bottle and can 5, tyres 12;
  mp_backlot's parts 2, doors 3, tyres 11; mp_bog's chunks 7. VERIFIED
  (the run against the captures).
- No take holds a type 1 `physGeoms` box in a transform, a flat model's
  -50..50 box or a dynamic entity's cylinder; the unit tests in
  `ode::tests` pin the last two as read. Class 11 is not ported: vcod
  leaves brush dynamic entities out.
- `run_frame` refuses a body whose first geom is not a capsule, a
  brush (3.6.4), a cylinder or a transform round one (3.6.6), so no box
  on worlds 0 or 1 steps until its collider (P3a) comes.

The census (`phys_create::the_stock_zones_phys_geoms_are_brushes_boxes_and_cylinders`,
every zone of the client's `zone/english/`, one copy of a model for
every zone): 72 zones, 200 models with `physGeoms`; 95 brush geoms (each
of type 0), 72 of type 1, 82 of type 4, no other type. The lists: 175
of one geom, 10 of two, 10 of three, 2 of four, 2 of five and one of six
(`vehicle_mi17_woodland_d_piece02_low`, six brushes). VERIFIED (the
run). So no stock model reaches 0x5a5710's cylinder arm with a type
other than 4.

For the colliders: a body carries up to six geoms, newest first. A
brush geom sits at the body's point while its brush is in the model's
frame: the collider takes the body's rotation times its +0x50 off the
body's position and turns the brush about that point, the model's
origin (3.6.4). VERIFIED (0x5a8513..0x5a856b).

#### 3.6.4 The brush geom collider, as ported

I ported IW's collider for a class 12 geom, a `physGeoms` brush, into
`ode::collide::brushgeom`: P3b of the world 0/1 port. Addresses are
`iw3mp.exe`'s (3.6), read with `objdump`; every function below stores
where the binary writes a DWORD and keeps the x87's 53 bits between, as
the capsule's do (`cod4-player-anims.md` 10.4, 10.6).

The geom, 0x5a8110's class 12 arm (0x5a8513..0x5a86a8):

- The context's position (+0x38) is the body's position less the
  rotation times the geom's +0x50 (0x431440, each component a stored
  three-term dot, each difference stored); its axes (+0x44) are the
  rotation's columns (0x5a8170..0x5a819a); the brush goes to +0x8c and
  the kind is 3. VERIFIED (the instructions).
- The walk's box: the largest magnitude among the brush's `maxs` x, y,
  z and `mins` x, y, z (0x401000's `fabs`, a later one only when
  strictly larger) times the double at 0x70ba90 (1.7320508, `sqrt 3`
  as a float), stored as each half size, grown by 1 round the position
  (0x5a8370). VERIFIED (the instructions).
- The geom is narrow when `phys_narrowObjMaxLength` is over the brush's
  `maxs` z (0x5a869e..0x5a87f9), its top and not a size. VERIFIED (the
  compare). The car take's hood (`maxs` z 0) is narrow and its doors
  (28.5) are not. VERIFIED (the take's brush lines).

Against a world brush (0x59b370, from 0x5a79b0 while the contacts have
room):

- The geom's faces are 0x596ae0's six axial planes of its brush, then
  its sides' planes, each normal turned by the rotation (0x59b120 for the
  axial ones, the same arithmetic inlined for the sides) and each
  distance `n' . pos + d`, both stored. VERIFIED (0x59b380..0x59b696).
- 0x599cb0 winds each geom face off those planes through 0x599560 into
  one buffer of 0x100 corners, a face's neighbours the brush's own;
  0x599560 is 0x5998e0 on a plane array (10.6), the same sequence of x87
  operations. 0x599d30 winds the world brush's faces through 0x5998e0
  into another 0x100. Either buffer empty ends the collide. VERIFIED (the
  calls, the two functions' operations compared).
- Each world face's least height over every geom corner (0x59acb0:
  `n . p - d` stored, a later point taken when its height less the least
  so far, stored, is under 0, from `FLT_MAX` at 0x70af58) of 0 or more
  ends the collide; of the faces with edges the least deep is kept, a
  later one only when strictly less deep, from `-FLT_MAX`. After the
  world's windings 0x59b010 does the same for each geom face over the
  world's corners. VERIFIED (0x59b3f0..0x59b8a5, 0x59b010..0x59b117).
- The world's face is the reference when its winding has corners and
  the geom's face is deeper or has none; else the geom's, when it has
  corners; else there is no contact. VERIFIED (0x59b951..0x59ba7a).
- A world face as the reference (0x59b1a0): the incident face is the
  geom face with corners whose normal dots least with the reference's
  (0x5992a0, from the float 2.0 at 0x700c0c, a later one only when
  strictly less); each contact's normal is the reference's negated and
  its surface flags the world face's material's (+0x40 of the 0x48-byte
  materials at 0x14098dc, the axial material at +0x24 `[i & 1][i >> 1]`,
  a side's at +4). VERIFIED (the instructions).
- A geom face as the reference (0x59b240): the incident face is the
  world face with corners whose normal dots least with the geom face's,
  and under the double at 0x70b490 (-0.1) too (0x598d60), none ending
  the collide; each contact's normal is the geom face's and its surface
  flags the incident face's, which the narrow pass then skips in place
  of the world's reference. VERIFIED (the instructions).
- 0x5989f0 clips the incident winding to the reference (0x597e70) and
  makes each point strictly behind the reference's plane a contact of
  depth `d - n . p`, stored, written straight into the `dContactGeom`s
  (the surface flags at +0x2c) while they have room: no local contact
  list, no 0x59c710 merge. VERIFIED (0x5989f0..0x598d56).
- 0x597e70 reads the reference winding's sense (0x5974b0: the first edge
  crossed with the first later edge whose cross squared is over the
  float 0.01 at 0x6df494, 1 when that cross dots under 0 with the
  reference's normal, else 0; 2, no contact, when no edge is long
  enough), then cuts the incident polygon (0x597800) by each of the
  reference's edges from its last corner round: the plane through the
  edge's first corner whose normal is `n x e` when the sense is 1, else
  `e x n`, not normalized, skipped when the edge's square or the
  normal's is under the double at 0x70b040 (0.1, 0x5975e0). Fewer than
  three corners left after a cut gives no contact. VERIFIED (the
  instructions).
- 0x597800 cuts a polygon to `n . p <= d` in place, a point out when
  `d < n . p` (NaN out); none in leaves it as it was and returns 0. The
  last vertex in before the run out and the first in after it bound the
  run: one vertex out opens a slot after it and becomes the meets with
  both neighbours' edges; a longer run's ends become their meets and the
  vertices between drop (0x597440). A meet is `b + (a - b) t` with
  `t = (d - n . b) / n . (a - b)` stored, the rest on the stack; an edge
  whose `n . (a - b)` reads 0 leaves its vertex. VERIFIED (0x597800..
  0x597e6a, 0x597440..0x5974a6).
- A narrow geom (context +0xa4) then takes as references (through
  0x59b1a0) each world axial face with corners whose side of the bounds
  the geom's position lies past (`mins > pos` for a lower face, `maxs <
  pos` for an upper one; a lower face past which it lies ends its axis),
  then each side with corners whose plane it lies in front of
  (`d < n . pos`), but the face skipped above, and only when every geom
  plane leaves something of the face's winding (0x59b2f0, 0x597800 per
  plane, 0 left refusing it). VERIFIED (0x59b9bb..0x59bc43).

Against a partition's triangles (0x59c300):

- Each triangle, while the contacts have room, is culled against a
  sphere round the geom (0x596c70, the context's +0x30), then taken into
  the model's frame (0x59c050: `R^T (v - pos)`, each stored) and
  collided there (0x59bd00); each contact it made turns back
  (`R n`, `pos + R p`, stored; the stride hard 0x30). VERIFIED (the
  calls and the instructions). The cull is not ported; on the car take
  the gate below met 360 partitions and matches without it. VERIFIED
  (the run).
- 0x59bd00: each brush face (the brush's own planes) against the
  triangle through 0x59ae80, which gives `-FLT_MAX` (0x70af5c) when the
  triangle's edges off `t0` and their cross `e2 x e1` are each over 0.01
  squared and that cross dots over 0 with the face's normal, and
  otherwise the face's least height over the corners (0x59acb0); 0 or
  more ends it, and the least deep face with edges is kept. Then the
  brush's windings (0x599d30, 0x400 corners), the triangle's plane
  (0x596bc0: `(t0 - t1) x (t2 - t0)` normalized by 0x4076d0, `d` at
  `t0`) and its least height over the brush's corners, 0 or more ending
  it. The brush face is the reference when it is strictly less deep
  than the triangle and has corners, the triangle incident, each
  contact along the face's normal; else the triangle is, the brush face
  facing it most under -0.1 incident (0x598d60), each contact along the
  triangle's normal negated; both through 0x5989f0 with the
  partition's surface flags. VERIFIED (0x59bd00..0x59c048,
  0x59ae80..0x59b002, 0x596bc0..0x596c66).
- `dCollide` turns every normal round, so each points out of the world
  into the geom, as for the capsule (10.6). INFERRED (the reversed
  class pair).

The sweep size (0x5a684f): the brush's bounds times the double at
0x70afb8 (0.01f), each stored; 0x561910 takes the stored sizes and, when
z is under x, moves both x bounds in by half the stored `x - z`, each
stored, then, when y is over z, both y bounds by half of `y - z`
(0x561910..0x561998). VERIFIED (the instructions). `Geom::bounds`. The
hood's comes to x 0.15..0.23, so its sweep is a point (3.6.1). VERIFIED
(`a_brush_geoms_sweep_size_is_its_bounds_cut_to_its_height`, ours).

The cut (0x5a2320, `collide::reduce`) now stores where the binary does:
each cluster's normals summed one add stored at a time, the means and
the clusters' normals normalized as 0x4076d0 and 0x55f6e0 do (the same
instructions), 0x5a1d80's tangents `N x u` and `N x T1` with `u`'s zero
parts multiplied in, each normalized, and every dot and squared
distance a stored three-term dot. VERIFIED (0x5a18e0..0x5a21c1,
0x5a1d80..0x5a1e3a, 0x55f6e0..0x55f755). In `f32` it kept 6 of the
car door's 9 contacts on step 1 where retail kept 7: two corners
2.4e-5 apart along the tangent, under an `f32` step at 360 units.
VERIFIED (the run).

The brush's loader keeps each face's neighbours (the axial runs at
+0x34 and +0x40, a side's at +8 and +0xa, into the edges at +0x30) and
each side's own inline plane (`PhysBrush::side_planes`); the plane
array at +0x4c read empty for the sedan's brushes, so `PhysBrush::planes`,
which the impulse solver reads, stays as it was. VERIFIED (the run).

`phys_brush_ab::the_car_takes_brush_geoms_collide_as_retail` feeds the
port the car take's pose of each brush body (the hood, 9, and the two
doors, 5 and 6) on every one of its 120 brush geom collides (steps 1
to 40): all 582 contacts equal the capture's `dCollide` output word for
word in count, order, position, normal, depth and surface flags, 19 of
them off triangles, and the cut of each equals the contact pass's
input word for word. VERIFIED (the run, ours).

#### 3.6.5 The box collider, as ported

I ported 1.7's box (class 1) collider against the world into
`ode::collide::boxes`, P3a of the world 0/1 port. Addresses are
`iw3mp.exe`'s (3.6), read with `objdump` and decompiled. The triangle
test is ODE 0.5's box against a triangle list (`collision_trimesh_box.cpp`)
as IW rebuilt it; that 0x5b1620 and its callees are that file's is
INFERRED (their shape against the upstream source).

The arm (0x5a8237), `BoxGeom`:

- The sides (+0x4c..+0x54) times the double 0.5 at 0x70ace8 are the half
  sides (context +0x8c). The geom's rotation goes in twice, its columns
  (the axes) at +0x44 and its rows at +0x68 (0x5a8173..0x5a8215); the
  body's position (body +0xe0) is at +0x98. The walk's box is the
  position less and plus `(|a1| h1 + |a0| h0) + |a2| h2 + 1` per world
  axis, `a_k` the axes. The box is narrow when `phys_narrowObjMaxLength`
  is over a side (0x5a832c..0x5a8362). VERIFIED (the instructions).
- The walk, the room test before a brush and the normals turned round
  after are the capsule's (`cod4-player-anims.md` 10.6). A box geom
  outside a transform sits at its body's point (3.6.3), so the centre and
  +0x98 are one point for every box the takes hold. VERIFIED (the
  captures).

Against a brush (0x59a610):

- 0x599da0 runs first. Of the faces whose plane the centre lies in front
  of, the axial ones in the order `+x -x +y -y +z -z` and then the sides,
  a separation (the centre's distance less the box's reach) of 0 or more
  ends the collide. The reach is the world extent for an axial face and
  `|n . a2| h2 + (|n . a0| h0 + |n . a1| h1)` for a side, each dot
  stored for 0x401000's `fabs`. The largest separation of a face with
  edges is the brush's candidate, its plane kept; `-1` when no face has
  one. VERIFIED (the decompiled code, the compares).
- Every face's winding (0x5998e0, room for 0x400 corners over all the
  faces) is taken into the box's frame, `q = R^T (p - c)`. Each box face
  keeps its least `q_k - h_k` (the plus face) or `-h_k - q_k` (the minus
  face), from 0; one left at 0, the brush clear of that face, ends the
  collide. The largest is the box's candidate, a later one only when
  strictly larger, the axes outer and the sides inner
  (0x59a6a2..0x59a960). VERIFIED (the decompiled code, the compares).
- The box's candidate is the reference when the brush's separation is
  under it, or the brush has no candidate or its winding is empty; else
  the brush's is (0x59a98f..0x59a9a9). VERIFIED (the compare). So the
  less deep is the reference, which 3.6.1 left INFERRED. No axis between
  two edges is tried: 0x59a610 calls nothing but 0x599da0, 0x596ae0,
  0x5998e0, the debug draw 0x597200, 0x598d60, 0x597fd0, 0x59a2b0 and
  0x59a500. VERIFIED (the calls).
- A box face as reference: 0x598d60 picks the face with a winding whose
  normal dots least with the box face's, under -0.1 (the double at
  0x70b490, from 2.0 at 0x700c0c). 0x597fd0 walks the box face's corners:
  `R hs` (the halves, `hs_k` negated for the minus face), then `-2 h`
  along the next axis, `-2 h` along the one after and `2 h` along the
  next, each stored, plus the centre, each put onto the brush face's
  plane. The face's winding is cut by the four edges' planes in order
  (0x597800). An edge's plane is `e x n`, or `n x e` when the corners
  wind against the brush normal (0x5974b0: the first corner pair whose
  cross product with the first edge is over 0.01 squared, the float at
  0x6df494), unnormalized, through the edge's start; an edge or plane
  normal under 0.1 squared is skipped (0x5975e0, the double at
  0x70b040). Fewer than three corners left give no contact. Each corner
  left with `(n . c + h) - n . p` over 0 is a contact of that depth along
  the box face's normal, with the brush face's surface flags, while the
  list has room. VERIFIED (the decompiled code, the instructions).
- A brush face as reference (0x59a2b0): its normal negated is the
  contacts'. The box face is the least of `a . n` and `-a . n` over the
  axes from 1.0 (0x5994c0). Its corners, walked as above from the
  centre, are cut by the brush face's winding's edge planes, starting
  with the edge from the last corner (0x5989f0 into 0x597e70). Each
  corner left with `d - n . p` over 0 is a contact of that depth.
  VERIFIED (the decompiled code, the instructions).
- The narrow pass is 3.6.1's loop (0x59aabe..0x59ac8a), its excluded
  face the one the reference clip took: 0x598d60's pick when a box face
  was the reference, or `-1` when it found none. A face goes through
  0x59a2b0 when its winding, cut by the box's six face planes in turn
  (`a_k . x <= h_k + a_k . c`, then the minus side, 0x59a500), keeps a
  corner after each cut. VERIFIED (the decompiled code).
- 0x597800, IW's clip in place to `n . x <= d` (the dot stored): no
  corner inside gives 0; all inside, the polygon. Else the corner after
  the last edge leaving and the corner before the last edge entering
  move onto the plane along those edges, and the run between them goes
  (0x597440). With one corner outside, a corner goes in after it,
  taking the meet on its edge to the next corner, and it takes the meet
  on its edge from the one before. An edge along the plane (its dot 0)
  moves nothing. VERIFIED (the decompiled code, the instructions).
- The depths: 0x597fd0 and 0x5989f0 store each dot before the
  subtraction (0x5986a7, 0x598a52), so a contact a few thousand units
  out has its depth rounded to the dot's float. VERIFIED (the
  instructions, and the watermelon take's depths).

Against triangles (0x59c500 into 0x5b1620):

- 0x59c500 hands 0x5b1620 the half sides, the position, the rotation as
  ODE's 3x4 (0x596b80), the body's position (context +0x98), the room
  left and the list, and gives each new contact the partition's surface
  flags (0x59c57f..0x59c593). 0x5b1620 takes each triangle while its
  count is under the room (`and 0xffff`, 0x5b1749), as ODE's
  `(v0, v1, v2) = (t2, t1, t0)` (the stores at 0x5b1753..0x5b17cb),
  through 0x5af410 and, when that passes, 0x5b0590. VERIFIED (the
  instructions).
- 0x5af410 against upstream's `_cldTestSeparatingAxes`: a triangle with
  an edge's squared length under 0.01 (`e0`, `e1`, then `e1 - e0`) or a
  normal `e0 x e1` under 0.01 long is skipped. The normal is normalized
  in place (0x4076d0, its length returned and used for upstream's
  unnormalized dots). One with `n . (v0 - c)` at least 0 and the body's
  `n . b - n . v0` under 0 is skipped too (0x5af620..0x5af678). VERIFIED
  (the compares). Then axis 1 with its depth `fR + n . (v0 - c)`, under 0
  separating. VERIFIED (the instructions). Every value handed to a test
  is a pushed float. VERIFIED (the pushes).
- The tests: the normal's (0x5aefa0) scales the depth by the inverse of
  `sqrt` of the normal's squared length (0x401050) and keeps it strictly
  under the best. A box axis (0x5af030) turns the axis against the
  triangle's normal when their dot is over 0 and takes `r + max` then,
  else `r - min`, where upstream flips by the lesser depth. An edge axis
  (0x5af170) is upstream's, kept when 1.5 (the double at 0x70ada0) times
  its scaled depth is under the best. VERIFIED (the instructions).
- 0x5b0590 against upstream's `_cldClipping`: the edge arm
  (0x5b05b0..0x5b094a) makes its one contact with no room test, the box
  corner built an axis at a time. The box face arm
  (0x5b095c..0x5b0e56) is upstream's. Then every path goes on to the
  triangle arm at 0x5b0e62 (the jumps at 0x5b094a and 0x5b0956, the fall
  through at 0x5b0e5c). It picks the box face by the triangle's normal
  `n`, not the best normal, and makes contacts along `-n`, `-min(0, n .
  p)` deep. VERIFIED (the jumps, the instructions). So a triangle met on
  a box axis or an edge gives that arm's contacts and the face's too.
  INFERRED (the flow above).
- No contact goes through 0x59c710's merge; the box writes the list
  directly. VERIFIED (the calls).

The gate, `cod4::phys_box_ab::box_contacts_match_retails_collides`:

- Every class 1 collide of the four takes (3.6.2), fed the captured pose
  and sides, against the capture's `dCollide` output and its contact
  pass input: count, order, position, normal, depth within 1e-4,
  surface flags. Raw contacts: the round's 40 collides, the frag's 160
  and the car's 160 equal to the bit; the watermelon's 238 of 240 equal
  to the bit, one within 5e-7, one 2.4e-4 off. The cut through
  `collide::cut`: every pass input but the one off's, 239 of 240.
  VERIFIED (the run, ours).
- A take's `body` line is dumped at 0x5a6e69, before the frame's sweep
  (0x5a6c10 into 0x5a6930, 3.6.1), so the gate moves each body as that
  sweep does from the line before: three collides (the car's step 39
  body 2, the watermelon's steps 28 and 34) match only so. VERIFIED (the
  run). The one left 2.4e-4 off (step 28) is an ulp of position at x
  3530, the sweep's stop: vcod's trace and retail's put it an ulp apart.
  INFERRED.
- The watermelon's step 13 body 5 cut to its five in another order
  while `cut` reduced in `f32`; with the stores 3.6.4 put in it, it
  matches. VERIFIED (the runs, ours).
- What the takes reach: a box face reference with contacts 114 times,
  a brush face reference 97 times, the triangle face arm 692 contacts,
  the box face arm 12 and the edge arm 12; the narrow pass never added
  one. VERIFIED (the run, counted with a probe not kept).
  `a_narrow_box_over_a_brushs_edge_takes_its_top_too` pins the narrow
  pass; `a_clip_past_one_corner_puts_a_corner_in` and
  `a_clip_past_two_corners_moves_them_onto_the_plane` pin 0x597800's
  two arms. VERIFIED (the tests, ours).

#### 3.6.6 The cylinder geom collider and the transform, as ported

I ported IW's collider for a class 13 geom, a `physGeoms` cylinder, into
`ode::collide::cylinder` and ODE 0.5's geom transform collide (class 6)
into `ode::collide::transform`: P3c of the world 0/1 port. Addresses are
`iw3mp.exe`'s (3.6), read with `objdump`; every function stores where
the binary writes a DWORD and keeps the x87's 53 bits between
(`cod4-player-anims.md` 10.4). The context and the walk's box are
3.6.1's.

The transform:

- 0x5aee10, class 6's collider: nothing without an inner geom (+0x4c);
  the final pose recomputed when the geom's flag +8 & 2 is set
  (0x5aed50); the inner geom's position and rotation pointers set to
  +0xc4 and +0x94 and its body to the transform's (+0x10); 0x5a95a0 on
  the inner geom and `o2`; with `infomode` (+0x54) set each contact's
  `g1` (+0x24) the transform; the inner geom's pointers and body put
  back. VERIFIED (0x5aee10..0x5aeeba). Every captured cylinder contact's
  `g1` is its inner geom, as `infomode` 0 gives. VERIFIED (the frag and
  car takes). That this is ODE 0.5's `dCollideTransform` with IW's frame
  fields is INFERRED (the upstream source).
- 0x5aed50: the final rotation the body's times the frame's (0x5aeec0,
  each element a stored three-term dot), the final position the body's
  rotation times the frame's position, stored, plus the body's
  position, stored. VERIFIED (0x5aed50..0x5aee05, 0x5aeec0..0x5aef84).
  `transform::final_pose`.
- The sweep: 0x5a9a20 sends class 6 to 0x5aed10, which sizes the inner
  geom at the final pose, and class 13 to 0x5a688b: `r` times the
  double sqrt(1/2) at 0x70ae70, stored, or the +0x54 when it is less.
  VERIFIED (0x5a9a20..0x5a9aef, 0x5a688b..0x5a68b5). `Geom::bounds` had
  it (3.6.3); a body whose first geom is a cylinder, or a transform round
  one, now steps (`PhysWorld::run_frame`, `transform::collide_geom`).

Against a brush (0x5a1330, from 0x5a79b0 while the contacts have room):

- Each face's depth (0x5a11f0), the six axial planes (0x596ae0) then the
  sides: `-FLT_MAX` (0x70af5c) when the centre and the body's position
  both lie behind it (each `n . p` stored, then less `d`, stored); else
  `(dc - c h) - r sqrt(1 - c^2)`, `c` the axis's `|a . n|` (0x401000)
  made 1 when `1 - c` stored is under 0, `dc - c h` stored and
  `1 - c^2` stored before its root (0x401050). VERIFIED
  (0x5a11f0..0x5a1322).
- A depth of 0 or more ends the collide. The deepest face with edges (a
  later one only when strictly deeper) is the reference; its winding
  (0x5998e0, room 0x100) goes to 0x5a0f70 with its material's surface
  flags (0x5a7970), even when the winding is empty. A narrow cylinder
  (2 `r` under `phys_narrowObjMaxLength`) then takes axial face `2k`
  when the centre lies under the brush's `mins` on axis `k`, `2k + 1`
  when over its `maxs`, and each side whose plane it lies strictly in
  front of, each but the reference and each with a winding. VERIFIED
  (0x5a1330..0x5a17a6).

Against a partition's triangles (0x5a17c0): each triangle, its corners
in index order, that 0x596c70 passes, with 0x596c70's plane `(t0 - t1)
x (t2 - t0)` normalized by 0x4076d0 and its distance at `t0` (0x5a18c0).
VERIFIED (0x5a17c0..0x5a18bc, 0x596c70..0x5971fa). The operands' order
sets the sign of a zero part: the port's first `(t2 - t0) x (t1 - t0)`
gave four of the frag's collides a -0.0 in a normal where retail has
0.0. VERIFIED (the run, ours). The cull, a sphere of the context's +0x30
round the centre against the triangle's bounds, plane and edges, is not
ported; no captured collide needs it. VERIFIED (the run).

The polygon test (0x5a0f70; the face's plane `(n, d)`, its winding):

- Nothing unless the centre or the body's position lies in front of the
  plane (`n . p` stored, less `d`, stored, over 0); then the separating
  axes (0x59fc90), a separation ending it. VERIFIED (0x5a0f70..0x5a1012).
- 0x59f5d0 on each axis: turned unless its dot with the plane's normal,
  negated, is at least 0; the reach `sqrt(1 - c^2) r + h c` with `c` the
  axis's `|a . u|` (made 1 as above), `c^2`, `1 - c^2`, the root and
  `h c` each stored; each corner's `u . p - u . c` stored, the nearest
  and farthest taken by a stored difference under 0; apart when the
  reach is under the nearest or minus the reach over the farthest; the
  overlap `reach - nearest` stored replaces the best when the best is
  over it plus the bias, the best's axis the turned one. VERIFIED
  (0x59f5d0..0x59f9ee).
- 0x59fc90's axes and codes: the normal negated (1), the axis (2), then
  for each edge `e_k = p_{k+1} - p_k` code `3 + k`: `a x e_k` turned when
  it dots over 0 with `n`, normalized, skipped when under 1e-5 long, and
  only tested when `(e_k x n)` dots under 0 with it; all skipped when
  `|a . n|` is at most 0.02. Then for each corner `3 + n + k`: `d - a
  (a . d)`, `d = p_k - c`, normalized, skipped under 1e-5. Then for each
  corner two rim axes (0x59f9f0), the cap at `c + h a` then at `c - h a`,
  each `((cap - q) x a) x u` normalized with a bias of 1, `u` the edge
  `e_k` normalized and `q` its line's point at the cap's height; none when
  `u` or `|a . u|` is under 1e-5, or when the corner lies behind the
  centre along `u x n` while that dots over 0 with `a`. VERIFIED
  (0x59fc90..0x5a02a8, 0x59f9f0..0x59fc85).
- The arms by the best's code (0x5a1018..0x5a11e1): 1 with `|a . axis|`
  under 0.9 the side line (0x5a02b0), else the cap (0x5a09a0); 2 the
  axis (0x59f460); `3 + k` the edge `k` (0x5a0ef0); `3 + n + k` corner
  `k` (0x5973e0); the rest the rim against edge `(code - 3 - 2n) >> 1`
  (0x5a0740). Every contact carries the best axis as its normal and its
  overlap as its depth but where said. VERIFIED (the instructions).
- The side line: the best axis less its part along `a`, normalized (none
  under the double 1e-5 at 0x70b720), times `r` off the centre; the
  segment `+- h a` about it cut by the winding's edge planes (0x597f30:
  0x5974b0's sense and 0x5975e0's planes as 3.6.4, each cut by 0x597700,
  the side `n . x <= d` kept, both ends past a plane ending it); each end
  on or behind the face's plane (0x59ee60, `d - n . p` stored at least 0)
  a contact that deep. VERIFIED (0x5a02b0..0x5a0432, 0x597f30..0x597fc5,
  0x597700..0x5977f9, 0x59ee60..0x59eeaf).
- The cap: the axis turned so it dots at most 0 with `n`, `w`; `v` the
  normal negated less its part along `w`, normalized (`1 / sqrt`
  stored), or the rotation's next column when its square is under the
  double 1e-10 at 0x70b5c0; eight rim points `cc + cs (v x w) + sn v`
  round the cap's centre `cc`, `(cs, sn)` `r` times (1, 0), (sqrt 1/2,
  sqrt 1/2), (0, 1), (-sqrt 1/2, sqrt 1/2), (-1, 0), (-sqrt 1/2,
  -sqrt 1/2), (0, -1), (sqrt 1/2, -sqrt 1/2) (the doubles at 0x70adf0,
  0x70ae70, 0x70b718, 0x70b2d8, each product stored); the octagon cut
  to the winding (0x5989f0, 0x597e70 and 0x597800 as 3.6.4), each point
  strictly behind the plane a contact of depth `d - n . p` along `-n`.
  VERIFIED (0x5a09a0..0x5a0ee9).
- The axis: the winding cut to the cylinder's circle (0x59f170: a corner
  inside when `|a x (p - c)|^2` is at most `r^2` stored, each crossing
  edge's meets off 0x59eeb0's quadratic, up to 0x100 points, a crossing
  whose meets miss leaving the last ones on the stack); each point with
  `s = a . p - a . c` within the cap facing the face a contact along `a`
  of depth `h - s`, or along `-a` of `s + h` when `a . n` is at least 0.
  VERIFIED (0x59f460..0x59f5cc, 0x59f170..0x59f45b, 0x59eeb0..0x59f165).
- The edge: 0x560050's nearest points of the axis's line and the edge's
  (parallel when 1e-4 `|b a|` at 0x70b000, stored, is at least the
  determinant squared, then the edge's parameter 0); the contact at the
  edge's point. VERIFIED (0x5a0ef0..0x5a0f65, 0x560050..0x5601c6).
- The rim: the cap on the best axis's side (`-h` when `a . axis` is under
  0); the chord where its circle crosses the plane through the edge's
  first corner with normal `u x axis` normalized (0x5a0440: the rim's
  point toward the plane past it, `x = dc r / (dc - dr)`, a half chord
  `sqrt(r^2 - x^2)` along `v x a`, each stored, none at 0 or less), cut
  to the edge's span by planes along `u` (0x597700); each end whose
  `axis . x` less the corner's, stored, is not under 0 a contact that
  deep. VERIFIED (0x5a0740..0x5a099f, 0x5a0440..0x5a0734).
- `dCollide` turns every normal round, so each points out of the world
  into the cylinder, as for the capsule (10.6). INFERRED (the reversed
  class pair).

The cut (0x5a2320), on top of 3.6.4's rounding: the third seed's larger
dot is 0x4093c0's rule (`d1` when `d0 - d1` stored is under 0), and the
squared distance that drops a near contact is under the float's own
bits at 0x70b87c, 0x3c23d70b, a hair over `0.01f`. VERIFIED
(0x5a1a1a..0x5a1a49, the bytes at 0x70b87c). In `f32` the cut kept a car
tyre's step 7 contacts in another order: two read the same along `T1`
at x87 precision, and `f32` put the later one first. VERIFIED (the run,
ours).

What the port matches:

- `phys_cylinder_ab::cylinder_contacts_match_the_captures` feeds the
  port each captured pose of the frag's bottle, can and two tyres and of
  the car's two tyres on every one of their 240 collides (steps 1 to
  40): all 666 contacts equal the capture's `dCollide` output word for
  word in count, order, position, normal, depth and surface flags, and
  each of the 146 cuts equals the contact pass's input word for word.
  VERIFIED (the run, ours). The pose is a `body` line's position and its
  quaternion through `dQtoR`; the sweep after a frame does not move a
  body in these takes, or a pose would miss. INFERRED (all match).
- Over those collides the faces that passed the axes took the side line
  103 times, the cap 107, the axis 44 and the rim once; no edge or
  corner arm ran, and the narrow pass ran on 8 brushes (the bottle and
  the can). VERIFIED (a counting run, ours). The edge and corner arms
  and 0x560050 rest on the instructions alone.
- `a_cylinder_on_a_floor_meets_it_with_its_cap_or_its_side`: a flat
  cylinder half a unit into a floor brush gives its eight rim points
  0.5 deep, one on its side the side line's two ends.
  `a_tyre_dropped_on_a_floor_rests_on_its_cap`: a tyre in world 0 lands
  and sleeps 2.50 over the floor, a unit under its +0x54 (`phys_csl`).
  VERIFIED (the tests, ours).

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
  `play::cod4::tests::live_streaks::cod4_live_airstrike_then_helicopter_in_one_life`.
  VERIFIED (ours).

### 4.3 `==` on two numbers

- `cod4_lnxded` 0x8158844 tests two numbers as CoD2 does: `|a - b| <
  1e-6`, the difference stored as a float (`fsubs`, `fabs`, `fstps`,
  `fucomip` against the float at 0x8220c38, 0x81588bf..0x81588de); two
  ints compare exactly (0x81588a4). 0x8158a30 is `!(a == b)`. VERIFIED
  (the instructions).
- Retail `cod4_lnxded` 1.7 (mp_crash, a scratch `war.gsc` under
  `fs_game` holding `probe_eq_tol`'s 24 lines, 2026-11-26) logged CoD2's
  lines. VERIFIED (the log). The table: cod11-gsc-object-model.md 9.3.1.
- vcod: `VmProfile::COD4.number_eq` is `NumberEq::Stored`. Gate:
  `semantics_ab::probe_eq_tol_matches_retail_stored`.

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

### 7.1 Server commands

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

### 7.2 The start-up configs

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

### 7.3 The stats, the loadout, the perks

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
  cod4-netfields.md 10.2). VERIFIED (the disassembly).
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
  `N` (cod4-netfields.md 10.2).
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

### 7.4 The perks

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

### 7.5 The spawn and the end frame

- `ClientSpawn` ends in its own think, before the script's loadout lands:
  the spawn frame's ground, `holdBreathScale` and `pm_flags` are that
  think's (cod4-pmove.md 15). The spot is `placeSpawnPoint`'s drop,
  retail's to the bit on mp_crash's terrain (same section).
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
- A follower's cmds are kept: `ClientThink` (0x80a6810) copies the last
  cmd to the old (0x80a6857) and takes each new one into `sess.cmd`
  (client+0x2f90, `SV_GetUsercmd` 0x80a686e) before `ClientThink_real`,
  following or not, and `SetClientViewAngle` puts the spawn's view off
  its angles (+0x2f98, 0x80a7841). VERIFIED (the instructions). The walk
  capture's spawn `delta_angles` 110.00061 is the spot's 45 less the -65
  the walker sent before and all through its follow (53703 from 45210 to
  47886). VERIFIED live. The plate capture's target spawned off its
  killcam's last cmd (yaw 4973: `delta_angles` 96.68518). VERIFIED live.
  That capture's cmd of yaw 64319, sent after its client read the
  50100 snapshot, reached the server after the 50100 frame's spawn.
  VERIFIED live (the line order). The gates (`wire_ab`, `capture_ab`)
  send a twin's own cmd angles before its spawn and put a packet after
  the frame its client had already read.
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

#### 7.5.1 A spectator's `pm_type`

What 1.7 writes into `ps.pm_type` (ps+4) for a client off the field.
VERIFIED (the instructions) where no label says otherwise:

- `ClientBegin` (0x80a7700) sets `pers.connected` 2 (+0x2f8c) and
  `pm_type` 4 (0x80a7731) before its notify (0x80a776c). The name is
  INFERRED (the `begin` it notifies, vcod's `begin`).
- `ClientSpawn`'s memset (0x80a7cc9) leaves 0 for every session; a
  spectator's 4 comes from the spawn's own think.
- `SpectatorThink` (the block at 0x80a4ef6) sets 4 for a spectator not
  following (`otherFlags` bit 2 clear, 0x80a4eea), `speed` 400 or 0 by the
  free-flight bit of +0x3004 (0x80a4efd..0x80a4f10).
- `SpectatorClientEndFrame` (0x80a49f0) never writes it. A killcam
  (`archivetime` set) and a follow both take the archived playerstate
  whole (`SV_GetArchivedClientInfo` 0x81795e2, the 0x2f64-byte copy at
  0x80a4bd7), so they send the watched client's: 0 while it plays, 7 once
  it is dead. The float leaves what the last think left.
- 0x80a4c76 writes 5 (0x80a4cab), INFERRED the intermission's end frame.
  The dead read 7, 8 linked (9). INFERRED.
- The captures agree. Every killcam reads `pm_type` 0 with `otherFlags` 2
  and the shooter's `clientNum` (`play-1.7-kill-target.txt` from 50000;
  `kcent`, `csb0corpse`, `csb0drag`, `prone` and `score` targets); the
  walker's follow of its mate reads 0 with `otherFlags` 0xa
  (`play-1.7-walk.txt` 46800, above); a spectator floating or in the team
  menu reads 4 with `otherFlags` 0 or 8 (`play-1.7-spec.txt` and every
  probe before its team). VERIFIED live.
- vcod-server writes the same: 4 at `connect`, `begin`, a spectator spawn
  and each free spectator's think; the copy in `watch` and the killcam;
  5 at the intermission's end frame. The damage soak's spectators at
  `pm_type` 0 are in their killcams, as retail's are. VERIFIED (a
  `SOAK_REPORT` run, ours). The client predicts and draws the native HUD
  off `otherFlags` 2 and `clientNum`, not `pm_type` (`play::cod4::cmd`).
  INFERRED (the code).

### 7.6 Open

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
level time (a scratch run, not kept: the server alone for 15 s, each row
its own boot with the `+set`s the table names):

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
0x80d3720). Port: `game::cod4::use_key` (`use_list`, `entry_hint`,
`update_cursor_hints`, `use_entity`).

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
- vcod's swap (`swap_weapon`) is 1.7's `Touch_Item` arm, 9.1.2.
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

#### 9.1.1 The swap capture

`play-1.7-swap-target.txt` and `-shooter.txt` (mp_crash, war): the
shooter (`cod4_probe --script swap --weapon 27`) kills the target
(`--class specops_mp,0`), walks to the MP5 he dropped and holds use while
the hint is up. VERIFIED live, in the frame of the pickup (44400):

- the taker's playerstate rings `EV_ITEM_PICKUP` (9) with the new weapon
  (16) as its parm, `weapon` reads 0 that frame and the `a 16` server
  command selects the new one;
- the M16 given up is item 321 lying at rest where the taken MP5 lay
  (203 2310 237, `groundEntityNum` 1022, yaw 124 and roll 90), not
  launched from the taker;
- the hint then names it: weapon 27 plus 4 (31) on entity 321.

`cod4::capture_ab::cod4_use_key_swap_ab` replays the pair and matches
every playerstate word and entity but its `GAPS`. VERIFIED (the gate).
Before 9.1.2's port it failed on about 6000 rows. What else it took:

- The two probes' sends of the frame ending at 43700 tie to the ms on
  the target's clock (both 7819), and retail ran the shooter's first: the
  target's 43700 snapshot is dead with a dead pmove run on it
  (`viewHeightTarget` 8, the death anim's timers 50 down, no knockback
  `pm_time`). VERIFIED (the capture). The harness's `Pair::ties` names
  that order; with the target first ours kills after its cmds and the
  corpse, the dropped MP5 and the M16 left in its place all lie a unit
  off.
- `BG_PlayerStateToEntityState` snaps the body's origin and angles with
  `cvttss2si` and `fild` (0x80529fc..0x8052a4e), so a view a fraction under
  0 goes out +0, a clear zero bit. VERIFIED (the instructions). Rust's
  `trunc` kept -0, which the -100 field sent as the smallest float: the
  shooter's pitch after the pickup, and the SAW gate's let go (9.2.1).
- The shooter's spawn point (-850 2204 254.125) has angles 0 270 0
  (VERIFIED, its fields on ours' loaded level); the capture's view reads
  -90. With the spawn pinned at -90 the body's first two frames
  went out a degree off retail's; the pin takes the spot's own angles on
  CoD4 (`pinned_spot`).
- Left in `GAPS`: the target's killcam from 46600 (the shooter's 43400
  on, `deltaTime` 3200). Retail's frames there carry the shooter's
  `delta_angles` and, from 47650, its yaw as its own snapshots coded them
  once (`delta_angles[1]` 0xc1ed8640, yaw 0xc2b3fd30); ours codes the
  archive twice, a short nearer zero (0xc1ed7b00, 0xc2b3fa60), as retail's
  own killcam frames 45700..46550 read. VERIFIED (the capture). What makes
  retail's archive read once there is unread (cod4-server-gaps.md 1).

#### 9.1.2 `Touch_Item`'s use-key arm

`Touch_Item` is `cod4_lnxded` 0x8091ffc (`ent`, `other`, `touched`); the
touch callback at 0x8092a44 sets the item's byte +0x16c and jumps in.
`Player_UseEntity` reaches it with `touched` 0. Port:
`game::cod4::use_key` (`swap_weapon`, `drop_for_swap`, `place_swapped`).
VERIFIED (the instructions) where no label says otherwise:

- It returns at once unless the byte +0x16c is set (cleared on entry), the
  toucher has a client (+0x15c) and health (+0x1a0) above 0, and the word
  at 0x836f6e0 is 0 (0x8092008..0x8092043).
- The grab test 0x805301e failing takes nothing; with `touched` 0 a
  dropper's own item stays silent, else the toucher is told
  `GAME_CANT_GET_PRIMARY_WEAP_MESSAGE` (weapon not held) or 0x8215db4's
  line (0x8092079..0x8092110).
- A missile (`eType` 4, a live frag) rings 10 and frees (0x8092666, 9.3).
  Otherwise the item's flag 0x1000000 (+0x180) is set for the take
  (0x809227c) and cleared after it (0x8092350, 0x8092427); `LaunchItem`
  never frees an item holding it.
- A weapon whose `inventoryType` (+0x13c) is not 0 is given on the item's
  camo (`index >> 7`, 0x80a327c at 0x8092415).
- A primary: with `ps.weapon` (+0xe8, read at 0x80922c2) set but not
  held, nothing is taken. With fewer than two primaries held (0x806120e
  counts held weapons of `inventoryType` 0; called at 0x80922f3) it is
  given. Otherwise the weapon in hand, or an alt mode's (`inventoryType`
  3) `altWeapon` (+0x540), must be a held primary, else the toucher is
  told `GAME_CANT_GET_PRIMARY_WEAP_MESSAGE` and nothing is taken
  (0x8092314).
- That gun goes through `Drop_Weapon(ent, gun, weaponmodels[gun], 0)`
  (0x8091642, called at 0x8092768): `LaunchItem` (0x80910ec) spawns it
  with no tag, its three `crandom`s drawn; its `ammoCount` (+0x1b4) is the
  reserve less what the ammo slot keeps without it (0x805f538), floored at
  0, `clipAmmoCount` (+0x1b8) the clip, the alt's into the second slot
  (+0x1c0..); then `BG_TakePlayerWeapon(ps, gun, 1)` (0x806206c), which
  zeroes `ps.weapon` when it was the gun: the pickup frame's `weapon` 0.
- The drop takes the item's +0x17c less bit 1 and its `groundEntityNum`
  (+0x7c) (0x809277b..0x8092796). With the item unlinked (+0x218 empty)
  the drop's box (±1) is traced from the item's `r.currentOrigin`
  (+0x13c) to 2 (0x820964c) above it, passing the drop's owner less one
  (1023 for none) with the item's `clipmask` (+0x190, 0x491 for 0); unless
  that trace is all solid (+0x28, INFERRED `allsolid`) a second goes back
  down to the origin, each end `(end - start) * fraction + start` on the
  x87 stack and stored; `G_SetOrigin` (0x80ca218) at the last end,
  `G_SetAngle` (0x80ca276) to the item's `r.currentAngles` (+0x148), and
  linked (0x809285b..0x8092a2a). A linked item's drop is linked to its tag
  instead (0x80cba78 at 0x809280b); vcod's items are never linked.
- The item's weapon is given on its camo (0x8092836). Each item slot
  with a weapon writes its clip into `ammoclip` when not negative and
  `Add_Ammo`s (0x8091af0) its count, filling the clip for a clip of -1
  (0x8092438..0x80925be). The drop's count then goes through 0x8091d44
  with no event (0x80925eb): only a held weapon on its ammo slot takes any
  back, out of the drop.
- `trigger` on the item with the player and the drop, or undefined for
  none (0x80925f0..0x8092622, 0x80926d8); `a <weapon>` (0x80a2148) unless
  the weapon's `weapType` (+0x12c) is 1 and it has an `offhandClass`
  (0x8092627..0x809265c).
- `EV_ITEM_PICKUP` (9) with the item's weapon (`index & 0x7f`) on the
  toucher, `G_AddPredictableEvent` (0x80ca0d2) or 0x80ca454 by the
  client's +0x2fd4 (0x809236c..0x80923d9); the item freed (0x80cc040 at
  0x80923a3). The taker's `cursorHintEntIndex` reads 1023 from the
  pickup frame on, where ours had kept the freed item's number. VERIFIED
  live.

vcod leaves out the drop's alt slot (an item carries one clip and
stock), the tag link, the `offhandClass` 2 and 3 test on `ps+0xe4`
(0x8052526) and the select's offhand exception: no use-key item reaches
them.

### 9.2 Mounted SAWs

VERIFIED live: every VERIFIED in this section without a gate or an
address rests on a retail run of `cod4_probe --script saw` behind
mp_backlot's crouch SAW 393 (`devmap mp_backlot`, war, `setviewpos -770
68 320 0`, `--face 0`). The first run's capture was not kept; the second,
under `sv_clientSideBullets` 0 with a watcher behind the gunner, is
`play-1.7-saw-gunner.txt` and `-watcher.txt`, which
`cod4::capture_ab::cod4_mounted_saw_ab` replays (9.2.1). Live gate: the
client's `cod4_live_saw_mounts_fires_clamps_and_lets_go`. Port: `game::cod4::turret`
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

#### 9.2.1 The A/B

`cod4_mounted_saw_ab` replays both probes of the second run on ours and
matches every playerstate word and every entity but its `GAPS`. VERIFIED
(the gate). What it took:

- `PmoveSingle`'s gunner arm ends in 0x8060d8e (called at 0x805c2df):
  `weapFlags` 2 and `pm_flags` 0x200 cleared, both weapon clocks 0,
  state ready and, under `pm_type` 7, the idle anim restarted. The capture's
  gunner `weapAnim` reads 0x200 and 0 by turns every manned frame, one
  flip a cmd. VERIFIED (the instructions, the gate). `PM_Weapon`
  (0x8062e1c) returns at once on `eFlags` 0x300. VERIFIED (the
  instructions). The arm's 0x8061a3c, between the duck and the
  footsteps, zeroes `fWeaponPosFrac` and `adsDelayTime` on a gun; vcod
  leaves it out (the capture never sights). INFERRED.
- `G_PlayerTurretPositionAndBlend` rebuilds the gunner's entity off the
  placed playerstate (`BG_PlayerStateToEntityState` at 0x80b863b, after
  the point trace on either branch), so a placed gunner goes out with
  `pos.trType` 1, not the end frame's 3 (0x80a6db3). VERIFIED (the
  instructions, the watcher's rows).
- The release rebuilds the entity inside 0x80b778a (0x80b7815), after the
  stance, the spot a unit up, the teleport flip and `SetClientViewAngle`
  but before 0x80b791d clears 0x300, then zeroes its `otherEntityNum`
  (0x80b794e): the watcher reads `eFlags` 0x204, `trType` 1 and the
  crouched box (`solid` 0x52010f, the last link's) on the release frame.
  VERIFIED (the instructions, the capture).
- `turret_clientaim`'s `AngleDelta` (0x80b7aed..0x80b7b3c) subtracts the
  gun's angle from the view on the x87 stack, stores only the floor and
  clamps the unstored fraction. VERIFIED (the instructions). vcod's
  doubles cut the barrel's rows from 104 to 76. VERIFIED (the gate).
- Still differing, the gate's `GAPS`: the barrel's angles 1 to 5 ulps off
  where the view is still near 0 or walking home, and the unmanned gun's
  roll residuals (4.8e-8) where ours reads 0, with the gunner's origin an
  ulp off where the body hangs off them; and the watcher's spawn. The
  gunner's yaw after the let go went out as the smallest float (ours -0,
  retail +0) until the body's snap read as 1.7's (9.1.1). The last: the watcher's spectator view read 94
  and its spot's yaw is 0 (mp_backlot's `mp_tdm_spawn` at -670 -2176 88,
  angles 0 2.5e-6 0, VERIFIED, the zone's entity string). Retail's spawn
  frame shows `viewangles` 94, `delta_angles` 94 and the body's yaw 0 for
  two frames (VERIFIED live), so the snapshot's view is not the spot's,
  and the harness's spawn pin (off the snapshot's view) puts ours at 94.
  How the spawn frame keeps 94 is unread. INFERRED.

### 9.3 The live grenade's throw back

Captured on retail with two probes on `devmap mp_crash` war: a `--script
frag` thrower at 123 2110 300 and a `--script throwback` target at 207
2291 296. Live gate: the client's
`cod4_live_frag_at_the_feet_is_thrown_back`. A second run, the thrower
aiming 60 under the target's feet (`play-1.7-throwback-*.txt`), is
`cod4::capture_ab::cod4_throwback_ab`: hint, pickup, hold, throw and the
thrown grenade's strike on the thrower's head match retail word for word
but the bounce's `trDelta` (cod4-combat.md 15). VERIFIED (the gate).

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
and store the copy back. The flag stays until a sweep between frames
counts each array's holders (`Heap::collect`, cod2-gsc-diff.md 3.7) and
clears it on an array one place holds; until then it costs a copy retail
would skip and no script can see. A write through `game` itself
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

Retail 1.7 (`war`, mp_crash, a scratch run, not kept) under an `fs_game`
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
  and snapshots' client states read (`HostClient::name`), under
  `auto_change`, or under `manual_change` for a client not yet in the
  game, cleaned as above (`cod4::game_name`). The engine's copy (`status`,
  the log, the drop print) follows the userinfo at once, as retail's, cut
  to 15 characters: `SV_UserinfoChanged` copies the `name` into 0x10
  bytes at client +0x21290 (0x8170341..0x8170356). VERIFIED (the code).
  vcod keeps at most 1023 bytes of a `userinfo` command's argument, as
  `SV_UpdateUserinfo_f`'s 0x400-byte copy (0x817055e), and of any client
  command. The client sends its userinfo
  on every change, so a `name` takes effect in the game, not only at the
  next connect.
