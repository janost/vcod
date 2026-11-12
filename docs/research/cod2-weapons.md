# CoD2 1.3 weapon files and the view weapon

How CoD2 1.3 reads `weapons/mp/<name>` and how its cgame drives the
first-person weapon, against 1.1. Addresses are `cod2_lnxded` (the 1.3
Linux dedicated server, md5 in `tools/re/dump_cod2_gsc.py`) first and the
Mac 1.3 client (`cod2mp_mac13_i386`, symbols intact) beside them. The two
share `bg_*`; the view weapon is the Mac cgame's alone.

Sources: the two decompiles, `.data` read straight out of `cod2_lnxded`
(`tools/re/dump_cod2_weapon_fields.py`), the 42 stock `weapons/mp/*` files
of a 1.3 install, and the 1.1d `game.mp.i386.so` for the comparison.

vcod reads the files with `vcod_cod2::weapon` (`parse`, `load`,
`WeaponDef`) and draws the view weapon with `crates/client/src/cod2_viewmodel.rs`.

## 1. The loader

- `BG_LoadWeaponDefInternal` (0x80f3382; Mac 0xbd248) opens
  `weapons/%s/%s` (`mp`, the name), compares the first ten bytes with
  `WEAPONFILE`, refuses a body of 0x2000 bytes or more and one
  `Info_Validate` rejects (a `"` or `;`), allocates a 0x604-byte
  `WeaponDef`, and hands the body to `ParseConfigStringToStruct`
  (0x80b8a36; Mac 0x2975c) with the field table, 0x16e records and 16 field
  types. VERIFIED (the calls, the immediates and the strings).
- `ParseConfigStringToStruct` walks the table, not the file: for each record
  it asks `Info_ValueForKey` for the record's name and skips an empty value.
  A key the table does not name is never read, and the lookup is Q3's
  case-insensitive first match. INFERRED (the loop; Q3's `Info_ValueForKey`).
- Before the parse every string field is set to `""` (0x80f3330, the loop
  over the table's type-0 records); everything else is the allocator's zero.
  INFERRED (the loop; the allocator's zeroing is assumed).
- After it: `adsTransInTime` and `adsTransOutTime` become rates at +0x588 and
  +0x58c (`1 / ms`, or 0.0033333334 and 0.002 at 0) (0x80f310a);
  `maxDamageRange` and `minDamageRange` at or below 0 become 999999.0 and
  999999.125 (0x80f3186, the words 0x497423f0 and 0x497423f2);
  `enemyCrosshairRange` over 15000 (0x8157670) is a `Com_Error`; a
  `projectile` weapon needs `projectileSpeed` above 0, a
  `destabilizationAngleMax` in [0, 45) (0x8157508) and positive
  `destabilizationBaseTime` and `destabilizationTimeReductionRatio`
  (0x80f31da); then `G_ParseWeaponAccurayGraphs` reads the two AI accuracy
  files and `ammoName` and `clipName` are lowercased. VERIFIED (the
  constants and strings). The order is INFERRED (the calls' order).

## 2. The field types

`ParseConfigStringToStruct`'s switch, VERIFIED (both decompiles):

| Type | Reads |
|---|---|
| 0 | a pooled string (the setter `0x80f3316`) |
| 1, 2, 3 | `strncpyz` into 0x400, 0x40, 0x100 bytes; no weapon field uses them |
| 4 | `atoi` |
| 5 | `atoi(v) != 0` |
| 6 | `(float)atof` |
| 7 | `(int)((float)atof * 1000.0f)` |
| 8..15 | `BG_ParseWeaponDefSpecificFieldType` (0x80f2de8; Mac 0xbce12) |

- Type 7 stores the float, multiplies it on the x87 stack and truncates
  under a `| 0xc00` control word (0x80b8bad..0x80b8bcd), as 1.1d does: 1.8
  is 1799 (`vcod_common::weapon::stored_ms`). VERIFIED (the instructions).
- The specific types look the value up with `stricmp` in a name table; a
  miss is a `Com_Error`. VERIFIED (the tables, the error strings):

| Type | Field | Offset | Names (Linux table) |
|---|---|---|---|
| 8 | `weaponType` | 0x78 | `bullet`, `grenade`, `projectile`, `binoculars` (0x81869e0) |
| 9 | `weaponClass` | 0x7c | `rifle`, `mg`, `smg`, `spread`, `pistol`, `grenade`, `rocketlauncher`, `turret`, `non-player`, `item` (0x8186a20) |
| 10 | `adsOverlayReticle` | 0x278 | `none`, `crosshair`, `FG42`, `Springfield`, `binoculars` (0x8186a08) |
| 11 | `weaponSlot` | 0x80 | `none`, `primary`, `primaryb` (0x81869f0) |
| 12 | `stance` | 0x8c | `stand`, `duck`, `prone` (0x8186a48) |
| 13 | `projExplosionType` | 0x390 | `grenade`, `rocket`, `none` (0x81869fc) |
| 14 | `offhandClass` | 0x84 | `None`, `Frag Grenade`, `Smoke Grenade` (0x8186a54) |
| 15 | `playerAnimType` | 0x74 | the tokens of `mp/playeranimtypes.txt` (0x8627f60, its count 0x8627f44) |

- The stock `mp/playeranimtypes.txt` (`iw_07.iwd`) lists `none`, `other`,
  `panzerfaust`, `stickgrenade`, `springfield`, `bar`, `mp40`, `thompson`,
  `sten`, `bren`, `kar98k`, `m1carbine`, `enfield`. VERIFIED (the file).

## 3. The field table

`weaponDefFields` sits at 0x8186a60 (Mac 0x348c60): 366 records of 12
bytes `{char *name; int offset; int type}`. VERIFIED (`.data`; the count
0x16e is the parse call's immediate). `vcod_cod2::weapon::FIELDS` holds
every record as `tools/re/dump_cod2_weapon_fields.py cod2_lnxded rust`
prints it; the plain dump lists them with their indices. Its types: 83
strings, 26 ints, 16 bools, 210 floats, 23 millisecond times and one of
each specific type. VERIFIED (the dump).

The view weapon's 22 anim keys sit in one run, +0x18 `idleAnim` to +0x6c
`adsDownAnim`, in the order section 6 numbers them. VERIFIED (the table).

## 4. What changed from 1.1

1.1d's table is 248 contiguous records at `.data` 0x7c994 of
`game.mp.i386.so`. VERIFIED (the dump). That 248 is the whole table is
INFERRED (the loop at 0x35b7a loads the table and starts its counter at
0xf7). Against 1.3's, VERIFIED (both dumps, names compared
case-insensitively):

- Dropped (10): `adsTransBlendTime`, `aiEffectiveRange`, `aiMissRange`,
  `ammoIcon`, `convergenceTime`, `fireEchoSound`, `horTurnSpeed`,
  `radiantName`, `twoHanded`, `vertTurnSpeed`.
- New (128), by family:
  - damage by hit location: `locNone`, `locHelmet`, `locHead`, `locNeck`,
    `locTorsoUpper`, `locTorsoLower`, the arm, hand, leg and foot pairs,
    `locGun` (19 floats, +0x5b0..+0x5f8);
  - range falloff: `minDamage`, `minPlayerDamage`, `playerDamage`,
    `maxDamageRange`, `minDamageRange`, `shotCount`, `armorPiercing`;
  - projectile destabilization: `destabilizationBaseTime`,
    `destabilizationTimeReductionRatio`, `destabilizationAngleMax`,
    `destabilizeDistance`;
  - grenade bounce per surface: `parallel<Surface>Bounce` and
    `perpendicular<Surface>Bounce` for 23 surface types (46 floats);
  - the quick switch: `quickRaiseAnim`, `quickDropAnim`, `quickRaiseTime`,
    `quickDropTime`;
  - first-person sound variants: `fireSoundPlayer`, `lastShotSoundPlayer`,
    `loopFireSoundPlayer`, `stopFireSoundPlayer`, `rechamberSoundPlayer`,
    `reloadSoundPlayer`, `reloadEmptySoundPlayer`, `reloadStartSoundPlayer`,
    `reloadEndSoundPlayer`, and `meleeSwipeSound`;
  - aim assist: `autoAimRange`, `slowdownAimRange`, `slowdownAimRangeAds`,
    `lockonAimRange`, `lockonAimRangeAds`, `enemyCrosshairRange`;
  - kick and sway: `hipGunKickReducedKickBullets`/`Percent`,
    `adsGunKickReducedKickBullets`/`Percent`, `hipIdleSpeed`,
    `adsIdleSpeed`, `adsViewErrorMin`/`Max`, `hipSpreadDuckedMax`,
    `hipSpreadProneMax`, `horizViewJitter`, `vertViewJitter`,
    `rechamberWhileAds`;
  - the turret and AI: `maxHorTurnSpeed`, `maxVertTurnSpeed`,
    `minHorTurnSpeed`, `minVertTurnSpeed`, `pitchConvergenceTime`,
    `yawConvergenceTime`, `suppressionTime`, `aiSpread`, `playerSpread`,
    `fightDist`, `maxDist`, `aiVsAiAccuracyGraph`,
    `aiVsPlayerAccuracyGraph`;
  - the rest: `offhandClass`, `playerAnimType`, `dropHintString`,
    `fireRumble`, `meleeImpactRumble`, `flipKillIcon`.

## 5. The stock files

- 42 files under `weapons/mp/`. Every key they carry is in the table but
  for eleven: `adsTransBlendTime`, `adsZoomGunFov`, `grabarc`,
  `horTurnSpeed`, `isHandModelOverridable`, `projectileTrail`,
  `projectileTrailRadius`, `projectileTrailTime`, `shellEject`,
  `twoHanded`, `vertTurnSpeed`. None of the eleven is a string in either
  1.3 binary, so the loader never reads them. VERIFIED (the files,
  `strings` over both binaries). Four are 1.1 keys the table dropped
  (`adsTransBlendTime`, `horTurnSpeed`, `twoHanded`, `vertTurnSpeed`).
  vcod keeps the eleven as `weapon::STALE_KEYS`; `tests/weapons.rs` parses
  all 42 with no key outside the table and that list.
- Values the stock files use: `weaponType` all four, `weaponClass` all but
  `mg` and `non-player`, `weaponSlot` `none` and `primary`,
  `adsOverlayReticle` all but `crosshair`. VERIFIED (the files).

## 6. The view weapon

- `CG_RegisterWeapon` (Mac 0x1d85cc) builds the view DObj from two models,
  `xmodel/` plus `handModel` (+0x10) and `xmodel/` plus `gunModel` (+0xc)
  with the literal `tag_weapon`, as 1.1 does. An empty `handModel` or
  `idleAnim` is a `Com_Error`. VERIFIED (the strings and the stores).
- Its anim tree is `VIEWMODEL` with 23 nodes: node 0 the root blend, node
  `n` (1..22) the clip of the def's string at `0x18 + 4 * (n - 1)`, an
  empty key taking `idleAnim`'s clip. VERIFIED (the `XAnimCreate` loop).
  The nodes: 1 idle, 2 empty idle, 3 fire, 4 hold fire, 5 last shot, 6
  rechamber, 7 melee, 8 reload, 9 reload empty, 10 reload start, 11 reload
  end, 12 raise, 13 drop, 14 alt raise, 15 alt drop, 16 quick raise, 17
  quick drop, 18 ADS fire, 19 ADS last shot, 20 ADS rechamber, 21 ADS up, 22
  ADS down (`vcod_cod2::weapon::node`).
- Each node plays at a rate the same function fixes: 1, except the twelve
  nodes fitted to a weapon time, `clip ms / time ms`, or 0 when the time is
  0. The fitted nodes and their times: hold fire `holdFireTime`, melee
  `meleeTime`, reload `reloadTime`, reload empty `reloadEmptyTime`, reload
  start `reloadStartTime`, reload end `reloadTime` (+0x218 again, not
  `reloadEndTime`'s +0x22c), raise `raiseTime`, drop `dropTime`, alt raise
  `altRaiseTime`, alt drop `altDropTime`, quick raise `quickRaiseTime`,
  quick drop `quickDropTime`. VERIFIED (the stores and their divisors;
  `weapon::NODE_TIMES`). The ADS clips must not loop (a `Com_Error`).
  VERIFIED (the strings).
- `CG_UpdateHandViewmodels` (Mac 0x1d96e8) swaps every weapon's hands for
  the model at configstring `0x14e + ps.viewmodelIndex` (the model block,
  334) whenever a snapshot carries a non-zero index (`CG_SetNextSnap`,
  Mac 0x1dee08). VERIFIED (the call and its operand). vcod loads the rig
  with those hands (`cod2_viewmodel::Cod2View`).
- `BG_GetViewmodelWeaponIndex` (Mac 0x7b226) picks the drawn weapon: under
  `pm_flags` 0x20000 the first weapon whose `weaponType` is binoculars,
  under 0x10 `offHandIndex` (ps+0xd0), else `weapon` (ps+0xd4). VERIFIED
  (the tests and loads).
- `CG_AddViewWeapon` (Mac 0x1da2f8) draws nothing at `pm_type` 4 or 5, on a
  turret (`eFlags & 0x300`), or for weapon 0. VERIFIED (the early returns).

### Driving the nodes

`CG_WeaponRunXModelAnims` (Mac 0x1dc3cc), each frame:

- The sight layer, for a weapon with `aimDownSight` (+0x32c): ADS up is
  wanted under `pm_flags` 0x40, but not under 0x10, and not while
  `weaponstate` is 5 with `weaponTime` past `adsReloadTransTime` (+0x524).
  `CG_PlayADSAnim` (Mac 0x1d7bc0) crossfades nodes 21 and 22 over 0.5 s and
  sets node 21's time to the cgame's sight fraction and node 22's to one
  less it. VERIFIED (the tests and the immediates). That fraction is
  `cg.predictedPlayerState.fWeaponPosFrac` (cg+0x25ca0, ps at cg+0x25bc4),
  which moves every frame: predicted for the own body, lerped for a
  followed one (cod2-hud.md 18.1). VERIFIED (the read's offset).
- The crossfade is `XAnimSetGoalWeight`'s (Mac 0x3f84c) through
  `XAnimSetGoalWeightInternal` (Mac 0x3b59e): a node with no anim info
  and a goal of 0 is left alone; a weight of 0 with a non-zero time
  starts at `goal * 0.001` (Mac rodata 0x326210); a goal of 0 takes the
  whole time unless the goal was already 0 and its time left is shorter;
  any other goal `time * (w - goal) / w` going down, `time * (goal - w) /
  goal` going up. `XAnimUpdateOldTime` (Mac 0x3b1b0) then moves the
  weight `(goal - w) * dt / left` and the time left down by `dt`, the
  goal outright once `left <= dt`, a weight under 1e-6 (0x326224) put back
  at `goal * 0.001`. VERIFIED (the stores and the branches). Set every
  frame, the wanted node climbs at 2 a second and the other falls over 0.5
  s from wherever it stood. INFERRED (the arithmetic). vcod:
  `cod2_viewmodel::AdsBlend`, the two clips blended by their weights
  under the main node (vcod's layering, not the tree's normalization
  over all 22 nodes).
- When `weapAnim` (ps+0x5d0) differs from the number last acted on, its
  value with the toggle bit 0x200 masked picks one node, weighted 1 and
  restarted, every other main node (1..20) weighted 0. VERIFIED (the
  switch). Every one of those `XAnimSetGoalWeight` calls passes a goal
  time of 0, so the change is a cut, where `CG_PlayADSAnim` passes 0.5.
  VERIFIED (the immediates in both):

| `weapAnim` | node | `weapAnim` | node |
|---|---|---|---|
| 2 | fire | 11 | reload |
| 3 | last shot | 12 | reload empty |
| 4 | rechamber | 13 | reload start |
| 5 | ADS fire | 14 | reload end |
| 6 | ADS last shot | 15 | alt drop |
| 7 | ADS rechamber | 16 | alt raise |
| 8 | melee | 17 | quick drop |
| 9 | drop | 18 | quick raise |
| 10 | raise | 19 | hold fire |

- 0 waits: while any of nodes 1..20 has not finished, the last number is
  reset to -1 so the next frame asks again; once all have, empty idle
  (node 2) when `ammoclip` of the weapon's clip reads 0, else idle (node 1).
  Any other number plays idle and prints `Unknown weapon animation`.
  VERIFIED (the loop and the branches).
- 1.1 numbers 0..16 the same way; its 17 is hold fire, which CoD2 moved to
  19 to fit the quick switch at 17 and 18. INFERRED (the two switches side
  by side; 1.1's in `vcod_common::weapon::view_anim`).

### Clip indexes

`BG_SetupClipIndexes` (Mac 0x7a22a) gives each new `clipName` the next
slot, compared with `strcmp`, in weapon registration order; the default
weapon holds slot 0 (`BG_ClearWeaponDef`, Mac 0x79f38, starts the count at
1), so the first real clip is 1, as 1.1's. VERIFIED (the loop and the
store). That default is not a file: `BG_LoadDefaultWeaponDef` (Mac
0xbcdac) fills every string field of a built-in def named `none` with
`""`. VERIFIED. So a weapon file without `clipName` (or `ammoName`) matches
slot 0: stock `mg42_bipod_stand_mp` and `binoculars_mp` hold slot 0.
INFERRED from the `strcmp` against the default def's `""`; VERIFIED
against a retail 1.3 capture on mp_toujane, where CS 7 opens
`defaultweapon_mp mg42_bipod_stand_mp ...` and the Thompson (weapon 11)
carries its 20 rounds in clip slot 9, which counting the two empty names
as one new slot would put at 10. `BG_SetupAmmoIndexes` (Mac 0x7a4e4)
assigns `ammoName` slots by the same rule. VERIFIED (the loop). Two weapons sharing a clip name with different `clipSize` are a
`Com_Error`. VERIFIED (the string). `weapon::clip_indices` assigns them in
configstring 7's order.

### The recoil

A shot kicks the view and the gun, all of it client-side; CoD2 runs 1.1's
code (cod11-combat.md 15.7) with a reduced kick added.

- `CG_FireWeapon` (Mac 0x1d823c) runs `BG_WeaponFireRecoil` (Mac 0x7b298)
  on `cg.predictedPlayerState` when `snap->ps.pm_flags & 0xc00000` and the
  event's entity is `snap->ps.clientNum`. VERIFIED, the test and the call.
  Events 158, 159, 160 and 175 call `CG_FireWeapon` once, 176 and 177
  twice (`CG_EntityEvent`, Mac 0x1e4650). VERIFIED, the switch.
- `BG_WeaponFireRecoil` reads the `BG_GetViewmodelWeaponIndex` weapon's
  def. VERIFIED. The view kick takes the `ads` ranges only at
  `fWeaponPosFrac` 1.0 (0x326188), else `hip`; the gun kick takes `ads` at
  any frac above 0.0 (0x3261a0). VERIFIED, the compares. Each range is
  `min + randomf() * (max - min)`, drawn view pitch, view yaw, gun pitch,
  gun yaw. VERIFIED, the call order. The scale is 1.0, or
  `0.01` (0x3262f0) times `hipGunKickReducedKickPercent` (+0x4c4, frac
  other than 1.0) or `adsGunKickReducedKickPercent` (+0x47c) while
  `weaponRestrictKickTime` (ps+0x40) is 1 or more. VERIFIED, the floats
  and the branch. The view kick's speed is set, not added: (-pitch, yaw,
  yaw * -0.5 (0x3261f4)) times the scale, into `cg + 0x2c030`; the gun's
  pitch and yaw times the scale are added to the spring's speed
  (`cg + 0x2c0b0`). VERIFIED, the stores.
- `CG_DrawActiveFrame` (Mac 0x1d4a50) steps the view kick
  (`cg + 0x2c03c`) after the snapshots' events, in slices of at most 5 ms
  of `cg.frametime` (0x326210 turns ms to seconds), with 1.1's rules: the
  centring speed `hipViewKickCenterSpeed` (+0x4f8) at a frac of 0.5
  (0x326190) or less, else `adsViewKickCenterSpeed` (+0x4b0), 2400
  (0x326628) for weapon 0; a step back toward 0 at 0.06 (0x326624) of the
  speed; a crossing stops at 0; a clamp at 10 (0x32626c) and -10
  (0x326548) that zeroes the speed. VERIFIED, the loop and the floats.
  Without `snap->ps.pm_flags & 0x800000` both vectors are zeroed instead.
  VERIFIED. `CG_Respawn` (Mac 0x1e3c56) zeroes both kicks. VERIFIED.
- `CG_DrawActive` (Mac 0x1cefca) hands the view kick, plus the scope's
  `adsViewErrorMin`/`Max` offset at `cg + 0x2c048`, to
  `CL_SetUserCmdAimValues` (Mac 0x17b85e), and `CL_CreateCmd` (Mac
  0x189f0e) sends `ANGLE2SHORT(cl.viewangles[i] + aim[i])` (0x3261fc,
  182.044) for all three angles. VERIFIED. So the kick moves the aim: the
  server's view and the shots after it follow the cmd. INFERRED, the data
  flow. The server adds no view kick of its own: in a retail BAR capture
  (`crates/cod2/tests/fixtures/combat/bar.txt`, nine fire events at the
  hip and down the sight) `viewangles` and `delta_angles` never change
  from the spawn's, and `weaponRestrictKickTime` is never sent: the BAR
  and the Thompson have `*GunKickReducedKickBullets` 0 and `fireDelay` 0,
  so `PM_Weapon` sets it to 0. VERIFIED, the capture and the files.
- The gun spring is stepped in `BG_CalculateWeaponAngles` (Mac 0x7f226)
  from `CG_AddViewWeapon` (Mac 0x1da2f8) for an `aimDownSight` (+0x32c)
  weapon: `BG_CalculateWeaponPosition_GunRecoil_SingleAngle` (Mac
  0x7af8c) on pitch then yaw, in 0.005 (0x32639c) slices of the frame,
  each key blended hip to ads by the frac (`gunKickAccel` +0x4d8/+0x490,
  `SpeedMax` +0x4dc/+0x494, `SpeedDecay` +0x4e0/+0x498, `StaticDecay`
  +0x4e4/+0x49c) and clamped by `gunMaxPitch`/`gunMaxYaw` (+0x2e0/+0x2e4).
  VERIFIED, the disassembly's arguments. An axis under 0.25 (0x3261bc)
  with a speed under 1 is zeroed, and the slices stop once both are.
  VERIFIED. 1.1's spring to the instruction (cod11-combat.md 15.7).
  INFERRED, the comparison. The angles are added to the weapon angles,
  whose `AnglesToAxis` multiplies onto the view's to place the gun.
  VERIFIED, the `MatrixMultiply`. A stock gun kick pitch is positive (the
  BAR's 55 to 60), so the gun dips against the view, which kicks up.
  VERIFIED, the files.
- With the hip BAR's keys (accel 1000, speed decay 16, static decay 20)
  the spring never meets its zeroing rule: each 5 ms slice turns the speed
  by 5, so it hovers a few hundredths of a degree off centre. INFERRED,
  the arithmetic (`cod2_recoil`'s test).

vcod: `play::cod2_recoil` keeps both kicks with 1.1's `play::recoil`, fed
the CoD2 file's kick keys and the reduced scale; the session adds the view
kick to every cmd's angles and hands the spring to `play::cod2_gun`'s weapon angles. The
weapon is predicted (cod2-hud.md 18.1), so the kick starts at the
predicted shot, as retail's does off `cg.predictedPlayerState`'s events,
and the snapshot's copy of the fire event in the playerstate ring is not
raised again (`EventTracker::predicting`, the predicted move and weapon
events of `cod2_predict::predicted_event`), so a shot kicks once. The
drawn view takes all three kick angles off the predicted playerstate:
`PM_UpdateViewAngles` (Mac 0x6c082, lnxded 0x80e6eb8) writes the
cmd's roll over `delta_angles[2]` as it does pitch and yaw. VERIFIED (the
loop over three angles; `pmove::tests::the_cmd_roll_rides_into_the_view`).

### Flash, brass and notetracks

- The view weapon's DObj is client DObj `0x400 + weapon`
  (`Com_ClientDObjCreate` in `CG_RegisterWeapon`, Mac 0x1d85cc). VERIFIED
  (the call's operand). Its `tag_flash` takes the view flash and its
  `tag_brass` the own brass (cod2-efx.md 6.8). `viewmodel_kar98k` holds
  `j_gun j_bolt j_clip j_round j_stripper tag_brass tag_flash`
  (cod2-xmodel-v20-format.md). VERIFIED (asset).
- `CG_UpdateViewWeaponAnim` (Mac 0x1dd3fe) steps the tree, builds the
  skeleton, then walks the DObj's notify list for this frame and plays
  the weapon's `noteTrackSoundA`..`D` on a note of that name
  (cod2-sound.md 7.8). INFERRED (the order of the calls). It runs at
  `pm_type` under 6 for a view weapon above 0, whether or not the gun
  draws. INFERRED (the tests).

vcod: `cod2_viewmodel::Cod2View` keeps the drawn rig's `tag_flash` and
`tag_brass` as posed each frame (`ViewTags`) and the main clip's
notetracks crossed in the frame's step (`note_sounds`), which the client
plays local; the sight layer's clips are set to their times, so vcod
walks no notes off them. INFERRED that retail's notify list carries none
of theirs either.

## 7. What vcod carries over

- The weapon switch: `weapnext` / `weapprev` on 1, 2 and the wheel cycle
  `weaponslots` 1 and 2 (`CG_CycleWeap`, Mac 0x1dbac4); the cmd's weapon
  byte, the server's drop and raise and the view weapon's change were
  measured on retail and on vcod-server in cod2-protocol.md 4.2.
  The weapon choice goes through the stock script menus (cod2-protocol.md
  4.2, "Script menus on a CoD2 client"): `--weapon` answers the first
  weapon menu, X (`openscriptmenu ingame changeweapon`) brings it back,
  and the pick applies at the next spawn.

- `vcod_cod2::weapon` parses every field as section 2 types it, applies
  section 1's defaults and errors (not the AI accuracy graphs), and reports
  keys outside the table. `vcod_cod2::model::recover_bind` rebuilds the
  view rigs' bind (cod2-xmodel-v20-format.md, "Viewmodels and hands").
- `cod2_viewmodel` loads the rig, steps one main node by section 6's
  rules with its rate, poses the sight clip at the lerped (or predicted)
  `fWeaponPosFrac` under it, and draws hands and gun at the eye with the
  view's angles, for the own predicted body and for a followed one. A
  follower's camera takes the followed view angles.
- `play::cod2_gun` places the gun off the view: offsets, bob, sway, lean
  roll, the idle drift, the damage kick, the turn lag
  (`BG_CalculateWeaponPosition_Sway`, `BG_CalculateWeaponAngles`;
  constants in cod2-hud.md 18). The scope overlay hides the gun through
  `play::cod2_view::ViewOut::overlay`.
- The recoil, as "The recoil" above.
- A main node change cuts, as retail's does; the sight switch
  crossfades (section 6).
- The view weapon's flash and brass on its tags and its notetrack sounds
  (section 6, "Flash, brass and notetracks").
- Not modelled: `cg_drawGun` on the CoD2 path.
