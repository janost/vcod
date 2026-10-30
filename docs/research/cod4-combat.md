# CoD4 1.7 combat on vcod-server: the weapon half, shots, damage, death

What `vcod-server --game cod4` does from a fire key to a respawn, and how
close it replays retail's kill capture. The weapon half of pmove is
`vcod_cod4::pmove::weapon`; the server's shots, hit poses, damage
feedback, items, melee and temp entities are `game::cod4::combat`, its
grenades `game::cod4::missile`; the builtins the death path reaches are
`builtins::cod4_combat`.

Sources, each claim naming its own:

- The kill capture (`play-1.7-kill-target.txt`, `-shooter.txt`,
  cod4-netfields.md 10): two `cod4_probe` clients on `cod4_lnxded` (md5
  `bf8448740b3f4cb74c2254f01af14cdd`) under `+devmap mp_crash`, `war`,
  `scr_player_maxhealth 30`. VERIFIED live.
- The 1.7 zones `common_mp.ff` and `mp_crash.ff` and the stock gsc in
  `common_mp` (`_globallogic.gsc`, `_killcam.gsc`, `_weapons.gsc`,
  `_callbacksetup.gsc`). VERIFIED.
- `tools/re/cod4_dvars.py` over `cod4_lnxded`: dvar defaults. VERIFIED.
- The walk capture's frag throw (`play-1.7-walk.txt`, cod4-netfields.md
  8) and the mantle capture (`play-1.7-mantle.txt`, cod4-pmove.md 17).
  VERIFIED live.
- The fire rate captures (`play-1.7-firerate.txt`, `-firerate-rof.txt`,
  recipe in their headers): one `cod4_probe --script auto` on
  `cod4_lnxded` 1.7 under the `fire_rate` probe gsc
  (`crates/cod4/tests/fixtures/gsc/fire_rate`), an AK-47 on the trigger
  plain and under Double Tap. VERIFIED live.
- The corpse run (section 5): 24 kills of a `cod4_probe --script poses`
  target on `cod4_lnxded` under gdb, its `torsoAnim` watched. VERIFIED
  live.

A claim labelled INFERRED that names no 1.7 address is not yet confirmed
on 1.7.

## 1. The `WeaponDef` offsets

The weapon half reads 0x878-byte `WeaponDef`s out of the zones at the
offsets the struct's field order gives, the order already
anchored by the string slots (cod4-gsc.md 7). INFERRED, each offset; the
values the M16 (`m16_mp`) reads there are the stock weapon file's
(VERIFIED, `cargo run -p vcod-cod4 --example weapon_dump -- common_mp.ff
m16_mp`): fire type 3 (a three-round burst), clip 30, `fireTime` 64,
reload 2000, empty reload 2365, `reloadAddTime` 1500, raise 349,
`adsTransInTime` 250 (the inverse sight times 0.004), damage 40, min damage
30 past 2000 units, head and neck multipliers 1.4. The table:

| field | offset | field | offset |
|---|---|---|---|
| `weapType` | 300 | `dropTime`..`emptyDropTime` | 932..964 |
| `weapClass` | 304 | `sprintInTime`, `sprintOutTime` | 968, 976 |
| `penetrateType` | 308 | | |
| `fireType` | 320 | `fuseTime` | 1004 |
| `clipSize`, `shotCount` | 824, 828 | `overlayReticle` | 1080 |
| `damage`, `playerDamage` | 844, 848 | the hip spreads | 1104..1148 |
| `fireDelay`, `fireTime` | 860, 876 | `rifleBullet`..`rechamberWhileAds` | 1248..1264 |
| `rechamberTime`, `..BoltTime` | 880, 884 | `clipOnly`, `adsFireOnly` | 1280, 1284 |
| `holdFireTime` | 888 | `noPartialReload`, `segmentedReload` | 1324, 1328 |
| `reloadTime`..`reloadEndTime` | 904..928 | `adsSpread` | 1820 |
| `adsReloadTransTime` | 1940 | the inverse sight times | 2040 |
| `minDamage` | 2048 | `maxDamageRange`, `minDamageRange` | 2056, 2060 |
| `locationDamageMultipliers[19]` | 2076 | | |

## 2. `PM_Weapon` and the sight

`vcod_cod4::pmove::weapon` runs, with the level's defs handed to the step
(`player_step_armed`): `PM_AdjustAimSpreadScale` after the clock,
`PM_UpdateAimDownSightFlag` before the sprint on every arm, and
`PM_Weapon` (the sight's lerp, the breath, the timers, the sprint, change
and reload checks, the rechamber, the fire) after the foliage on the
default arm. INFERRED, the order and each function. The capture's shooter reads 5 firing and 7
reloading, `weapAnim` 2 and 0x202 alternating its toggle on each shot,
`weaponShotCount` 1..3 over a burst. VERIFIED live.

- A burst's end holds the gun ready for `player_burstFireCooldown`'s 0.2
  s (registered 0.2, VERIFIED): the shooter's `weaponTime` after its third
  round reads 0xb8 and 0xc8 counting down to 0, and the gate fails at 0.25.
  VERIFIED (the gate). Its `weapFlags` 0x100 is the fresh trigger pull.
  VERIFIED live.
- An empty clip with rounds in reserve reloads: 58900 reads `weaponstate`
  7, `weaponTime` 2365 and `weaponDelay` 1500, the empty reload and its add
  time, with `EV_RELOAD_START_NOTIFY` (19), `EV_RESET_ADS` (14) and
  `EV_RELOAD_FROM_EMPTY` (16) on the ring. VERIFIED live.
- A segmented reload's start gives way to the trigger's edge only once
  more than 0.4 of `reloadStartTime` has run: `PM_Weapon_CheckForReload`
  (`cod4_lnxded` 0x806148e) compares `(start - weaponTime) / start` with
  the float at 0x82095bc (0.4) at 0x8061682, strictly greater, and stores
  state 10. VERIFIED (the disassembly). No capture fires a segmented
  reload; `pmove::tests::weapon_pins` pins vcod's
  (`pmove::weapon::START_CUT_FRACTION`), a start on exactly 0.4 included.
- `PM_BeginWeaponChange` is 0x805fd22 (it raises events 23 and 24)
  and `PM_SetReloadingState` 0x805fb7a (events 15 and 16, called from
  `PM_Weapon` at 0x8063bbe). INFERRED (the events each raises).
- `EF_FIRING` (0x40) follows the attack key in a ready or firing state with
  a round in the clip. VERIFIED (the gate's `eFlags` rows).
- The breath holds on the 0x2000 key on a sighted scope weapon for
  `player_breath_hold_time`, plus `perk_extraBreath` (5, VERIFIED) under
  perk 0x10, with `player_breath_hold_lerp` (1, VERIFIED). INFERRED; no capture holds it.
  `pmove::tests::weapon_pins::hands` pins vcod's hold, gasp and recovery,
  with the sight's raise and its blocks, the hip spread, the offhand
  throws and the melee swings no capture covers.
- The offhand (15..20) and the melee (12..14) with its charge are
  section 10's. Not run: detonation, night vision.
- A mantle on a row whose over clip is not the low one puts the weapon
  away as the ladder does (`Mantle_IsWeaponInactive` at both of
  `PM_Weapon`'s calls), and the first change after its end is quick
  (`mantleState.flags` 0x10). INFERRED. The empty hand's aim
  spread drops a whole 255 a step: the mantle capture's `aimSpreadScale`
  reads 0 at 48850, the cmd after its weapon went to 0. VERIFIED live.
  `cod4_moves_ab`'s `mantle` rows replay armed with every weapon word
  exact. VERIFIED (the gate).

`cod4_moves_ab`'s `shooter` rows replay the shooter's own cmds from the
first snapshot after its `setviewpos` (47550) with the weapon half: 275
rows, 792 cmds, every pmove word of section 13 of cod4-pmove.md and the
weapon words (`weapon`, `weaponstate`, `weaponTime`, `weaponDelay`,
`weaponShotCount`, `weapAnim`, `weapFlags`, `fWeaponPosFrac`,
`aimSpreadScale`, `weaponRestrictKickTime`, the event ring, `ammo`,
`ammoclip`, `eFlags` 0x40) exact on all 275, free and resynced. VERIFIED
(the gate).

### 2.1 The fire clock

Read off `PM_Weapon` (`cod4_lnxded` 0x8062df0), where
`PM_Weapon_WeaponTimeAdjust` is inlined, and the shot's start
(`PM_Weapon_StartFiring`, inlined at 0x80640af).

- One step comes off both `weaponTime` (ps+0x3c) and `weaponDelay`
  (ps+0x40): `pml.msec` (0x8063054), or a perk's quotient. VERIFIED (the
  disassembly). The quotient is `msec / perk_weapReloadMultiplier` (the
  dvar at 0x828d068, read at 0x80636d8) in a reload state (7..11) under
  perk 0x4, and `msec / perk_weapRateMultiplier` (0x828d06c, read at
  0x8063933) in state 5 or 6 (firing, rechambering) under perk 0x8.
  INFERRED (the branches at 0x8063009..0x806304e and 0x8063926). Either
  quotient is rounded half up: `fadd` of 0.5 (0x8209628 holds 0x3f000000),
  `frndint` under round-down, then a truncating `fistp` (0x80645c8..
  0x80645ed, 0x80636f0..0x806372b). VERIFIED (the disassembly). A
  multiplier of 0 steps by the larger of the two timers (0x806394f).
  INFERRED (the branch). vcod: `pmove::weapon::time_adjust`, `perk_msec`.
- A timer the step takes to 0 or below is set to 0 (`weaponTime` at
  0x8064592, `weaponDelay` at 0x80645b5): the overshoot is dropped, never
  carried into the next round. INFERRED (the branches into both stores).
- A shot stores the def's `fireTime` (+0x36c) into `weaponTime` at
  0x80640be and `fireDelay` (+0x35c) into `weaponDelay` at 0x80640b5,
  plain stores, not adds. VERIFIED (the disassembly).
- So a held trigger fires every `ceil(fireTime / step)` cmds. The
  AK-47's `fireTime` is 85 (`ak47_mp` in `common_mp.ff`, VERIFIED,
  `--example weapon_dump`): at the probe's 16 ms cmds a round takes six
  cmds plain (about 100 ms) and four under Double Tap (steps of 21 to
  24, about 67 ms), 1.5 times the rounds where the multiplier alone says
  1.33. Retail fired 20 rounds over the first 2 s held plain and 30
  under Double Tap; the second 2 s fired the 10 left in the clip plain,
  and 12 under the perk once the empty reload ended. VERIFIED live (the
  fire events of each snapshot's ring).
- `cod4_moves_ab`'s `autofire` and `doubletap` rows replay both captures
  armed: 306 rows each (913 and 911 cmds), every pmove and weapon word
  exact, free and resynced. `cod4_fire_rate_ab` replays their cmds free
  and fires retail's rounds in each burst. VERIFIED (the gates).

## 3. The shot

`ClientEvents` after each cmd reads the events the cmd raised; a fire
event runs `FireWeapon` at the cmd's time under `g_antilag` (1,
VERIFIED), else at `level.time`. INFERRED. `FireWeapon` takes
the spread between `BG_GetSpreadForWeapon`'s ends by
`currentAimSpreadScale` (the end frame's `aimSpreadScale / 255`), the ADS
spread when fully sighted; `Bullet_Fire` rewinds the bodies (only for a
shot 50..400 ms old, through CoD2's `SV_GetClientPositionAtTime` walk
over the level's position archive), draws the end off
`Bullet_RandomDir(time)` (`G_GoodRandomFloat`'s shuffle), 8192 units out
(a spread weapon `shotCount` rounds to `minDamageRange`), and traces the
clip map under 0x2806831, then each other playing body's hit boxes.
INFERRED. The eye and the angles the round leaves along are
3.2's; script models open to damage and brush models are 13.1's. Past
its first contact a round penetrates (3.1).

The hit boxes are each xmodel's `XBoneInfo` bounds (0x28 a bone at
`XModel`+164: `bounds[2][3]`, `offset`, `radiusSquared`) and the hit
location its `partClassification` byte. INFERRED; `body_mp_usmc_assault`'s boxes sit on the bones a body is hit
on (head class 2 on `head`, torso on `j_clavicle_*` 4, `pelvis` 5, the
limbs 6..17) and none on a tag (VERIFIED, `cargo run -p vcod-cod4
--example bone_dump -- mp_crash.ff body_mp_usmc_assault`). The trace walks
them as CoD2's under `bulletPriorityMap` or `riflePriorityMap` by
`rifleBullet` (the two maps are `cod4_lnxded` `.data` 0x826ce60 and
0x826ce73, loaded at 0x8089f4c and 0x8089f3f; VERIFIED, the bytes and
the loads), each body posed off the player
animtree by `vcod_cod2::player`'s body (cod4-player-anims.md), the
stance's idle where the playerstate carries no anim (the anim script is
not ported).

### 3.1 Penetration

`game::cod4::bullet`. Sources: `cod4_lnxded` 1.7 for the call shape and
the constants. The algebra between them is INFERRED.

- The constants of 1.7's round, as immediates in `cod4_lnxded`: 12 legs
  (0x808a893) and 5 (0x808acfc), the advance 0.135 (0x3e0a3d71) and
  `perk_bulletPenetrationMultiplier`'s dvar pointer 0x828ce74 read at
  0x808aa1d and 0x808aeb8. VERIFIED.
- 1.7 inlines `Bullet_FirePenetrate` and `Bullet_FireExtended` into one
  function calling `Bullet_Trace` (0x8089f14), `Bullet_Process`
  (0x808a136), `Bullet_ImpactEffect` (0x808a05c),
  `BG_GetSurfacePenetrationDepth` (0x805f2c0) and `BG_AdvanceTrace`
  (0x8060e44) in that order, the multiplier under `perks` 0x20
  (0x808aa10, 0x808abe9) for the entry's and the exit's depth. INFERRED
  (the call shape; the algebra between the calls).
- Dvars: `bullet_penetrationEnabled` 1, `perk_bulletPenetrationMultiplier`
  2, `bullet_penetrationMinFxDist` 30, `sv_clientSideBullets` 1
  (`tools/re/cod4_dvars.py`). VERIFIED. With client-side bullets the
  server raises no impact effect (`Bullet_ImpactEffect`'s temp entity is
  skipped), so ours sends none; the client draws its own. INFERRED.
- The depths are `info/bullet_penetration_mp` in `common_mp`: a
  `BULLET_PEN_TABLE` info string of `<type>_<surface>` keys for `small`,
  `medium` and `large` over the 28 named surface types (`small_bark` 20
  .. `large_paintedmetal` 0; concrete 6/12/12, metal 12/20/20, flesh
  24/34/40). VERIFIED (the rawfile). Type 0 (`none`) and surface 0 read
  0. `penetrateType` is `WeaponDef`+308: every bullet weapon has one
  (pistols and SMGs and shotguns small, rifles and the Desert Eagle
  medium, snipers large), the grenades and launchers 0. VERIFIED
  (`weapon_dump` over `common_mp`).
- A body hit reads surface flags 0x700000 (flesh, 7); a surface with
  0x100 has depth type 0 (stops the round); type 0 takes the last
  surface's. Each round starts at multiplier 1; past a surface the next
  leg starts 0.135 along the round over the cosine to the face (a graze
  under 0.125 ends it), the next contact is traced, then a trace back
  from it (or the round's end) to 0.01 short of the entry finds the exit.
  The thickness (at least 1) over the lesser of the entry's and the
  exit's depths comes off the multiplier; at 0 the round ends. A body
  hit past the first surface takes `Bullet_GetDamage` (the range
  falloff off the round's start) times the multiplier, truncated, with
  `iDFLAGS_PENETRATION` (8, `_callbacksetup.gsc`, VERIFIED). INFERRED.
- Retail, the wall capture (`play-1.7-wall-target.txt`): `cod4_probe`
  under `+devmap mp_crash` `war`, a target (`--script idle --goto "207
  2380 296"`, allies) outside the room's north wall and a shooter
  (`--script shoot --goto "207 2250 296"`, axis) inside, both on the
  assault class (the M16, Stopping Power). The wall there is a metal
  plate (surface 13) from y 2331.875 to 2336.125. Each of the shooter's
  first two rounds took 43 off the target (100, 57, 14). VERIFIED live.
  Ours: `1 - 4.27 / 20` of the M16's 40 is 31, Stopping Power's 140 per
  cent makes 43, Deep Impact's doubled depth 35 without it
  (`perk_tests::a_round_goes_through_a_wall_and_deep_impact_goes_further`).
- The first leg eases each body's hit pose a frame as before; the later
  legs trace the same poses without stepping them, so a round that
  penetrates moves no pose a plain one would not. INFERRED (ours: a
  `PlayerBody`'s controls are eased per trace, not per frame).

### 3.2 The eye and the gun's angles

`vcod_cod4::pmove::aim`; the server keeps its state per slot
(`Combat4::aim`, `gun_angles`) and runs it in `Cod4Level::think`.

- `CalcMuzzlePoints` is inlined twice, at 0x80a2b81 and 0x80a30a7: the
  view angles with pitch and yaw replaced by gclient +0x30a4 and +0x30a8
  (the gun pitch and yaw), through `AngleVectors` (0x818ea3a), then
  `G_GetPlayerViewOrigin` (0x80a7778). VERIFIED (the instructions). The
  two are `FireWeapon` and `G_UseOffHand`. INFERRED (the
  two callers' shape). So bullets, throws, launched grenades and
  rockets all leave along the gun's angles; ours does the same
  (`Cod4Level::muzzle_axis`).
- `G_GetPlayerViewOrigin` off a turret calls `BG_GetPlayerViewOrigin`
  (0x805399c) at `level.time` (0x836f8ac). VERIFIED (the instructions).
  That function: the origin plus `viewHeightCurrent` (ps+0x118); plus
  `BG_GetVerticalBobFactor` (0x805fac4) on z; plus
  `BG_GetHorizontalBobFactor` (0x805fa4c) along `AngleVectors`' right of
  the view; both at `BG_GetBobCycle` (0x805f8ca), `BG_GetSpeed`
  (0x8056c38) and `bg_bobMax` (8); then `AddLeanToPosition` (0x81a9f8e)
  with 16 (0x41800000) and 20 (0x41a00000). The z is held at 8 (rodata
  0x820aac4) over the feet at least. VERIFIED (the calls and the
  immediates). `AddLeanToPosition`: `GetLeanFraction` (0x81a91e8, `(2 -
  |f|) f`) of `leanf`, the right of (0, yaw, 16 times it), 20 times it
  along that. INFERRED, not yet confirmed on 1.7 instruction by
  instruction. The bob factors read the
  `bg_bobAmplitude*` vector dvars by `viewHeightTarget` 11, 40 and the
  sprint bit (ps+0xc as a negative short): the vertical `(sinf(2c) +
  0.2 sin(4c + pi/2)) * 0.75` of the second component, the horizontal
  `sinf(c)` of the first, each amplitude capped. INFERRED, not yet
  confirmed on 1.7 instruction by instruction. The constants 0.2, 0.75,
  4 and pi/2 are VERIFIED (the rodata). The amplitudes' values are the
  client's (cod4-client-view.md 4). INFERRED (the dvars are constant
  configstrings, so the gamestate carries no value to compare).
- `ClientThink_real` (0x80a5c1a), before `Pmove` and only past the
  inactivity check, on the cmd's ms (`serverTime - commandTime`, at most
  200): `BG_CalculateViewAngles` (0x806523e) into offsets added to
  `ps.viewangles`; `BG_CalculateWeaponPosition_Sway` (0x8060f20) on +0x3118
  (last view), +0x3124 (offset), +0x3130 (angles) at a scale of 1.0
  (0x3f800000 pushed at 0x80a5f49: 1.7 has no shellshock sway scale);
  `BG_CalculateWeaponAngles` (0x80659d0) on a copy of +0x313c..+0x3164
  written back after; when `ps.weapon` is `aimDownSight` (`WeaponDef`
  +0x4ec, the test at 0x805f4d2), `fWeaponPosFrac` (ps+0xf4) is not 0 and
  its `overlayReticle` (+0x438) is none, the angles are
  `AxisToAngles(AnglesToAxis(weapon) * AnglesToAxis(view + offsets))`
  (0x818e910, 0x818effc, 0x81951fe), else view plus offsets; pitch and yaw
  are stored at 0x80a6159. VERIFIED (the calls and the stores). The view
  is the one the last cmd left: `Pmove` runs after (0x80a6171). VERIFIED
  (the call order).
- `BG_CalculateViewAngles`: the damage kick, an overlay weapon's idle
  drift and walk bob scaled by the sight, the sighted view bob
  (`fWeaponPosFrac * adsViewBobMult` (+0x44c) of the bob factors at the
  flat speed, capped at 45, off a turret). `BG_CalculateWeaponAngles`: the
  lean's roll (-2 times the lean fraction), `frac * adsAimPitch` (+0x6cc),
  the movement tilt (the stance's `*Rot` (+0x240, +0x264, +0x288) past
  its `*RotMinSpeed` (+0x2b0..), eased at `posRotRate` (+0x2a8, prone
  +0x2ac), at least 0.1 a second), the idle drift (`hipIdleAmount` or 80,
  blended to `adsIdleAmount` by the sight (+0x48c..+0x498), the stance
  factor (+0x49c, +0x4a0) eased 0.5 a second; sines of the clock at
  0.0005, 0.0007 and 0.001 times a hundredth), the walk bob (a quarter
  turn on, 0.16 of the speed, capped at 10, `adsBobFactor` +0x448 up the
  sight), the damage kick, on an `aimDownSight` weapon the gun spring
  (0.005 s slices, `gunMaxPitch`/`Yaw` +0x4a4, the springs +0x6f0 and
  +0x738), less the sway's angles. The sway: the turn since the last cmd
  per 60th of a second, clamped to `swayMaxAngle`, `DiffTrack`ed at
  `swayLerpSpeed` (hip +0x4ac.., ads +0x4c8..; an overlay weapon sighted
  has none). INFERRED, not yet confirmed on 1.7 instruction by
  instruction. The capture below holds it to retail's rounds. VERIFIED (the gate).
- `BG_WeaponFireRecoil` (0x80609fc) draws four `G_random`s (0x8191e86,
  `rand()` times 2^-31): the view kick's pitch and yaw, then the gun's,
  added to the spring's speeds (+0x3158). The ranges are the sight's
  (+0x700.. view, +0x6e0.. gun) at a full sight, the hip's (+0x748..,
  +0x728..) otherwise, the gun's the sight's at any sight above 0; under
  `weaponRestrictKickTime` (ps+0x50) the scale is a hundredth of
  `*GunKickReducedKickPercent` (+0x6dc, +0x724). INFERRED, not yet
  confirmed on 1.7 instruction by instruction.
- The damage kick. `P_DamageFeedback` (0x80a5156, from the end frame)
  on a living player (`pm_type` 6 or under) with blood (gclient +0x30ac)
  and a max health (+0x2fe8): `count = 100 * blood / maxHealth` at most
  127 into `aimSpreadScale` (ps+0x628, 255 at most); `kick = count *
  bg_viewKickScale` (dvar 0x8277498) raised to `bg_viewKickMin`
  (0x82774a0) and cut to `bg_viewKickMax` (0x827749c), 0.2, 5 and 90;
  with `damage_fromWorld` (+0x30bc) roll (+0x3110) 0 and pitch (+0x3114)
  `-kick` and both direction bytes 255, else `vectoangles` of
  `damage_from` (+0x30b0) into the bytes and, against `AnglesToAxis` of
  `ps.viewangles`, roll `-kick` times its dot with the left and pitch
  `kick` times its dot with the forward; `damageEvent` up one,
  `damageTime` (+0x310c) `level.time - 20` (0x80a5256), `damageCount`
  the count, the blood cleared. A `damageTime` over 500 ms back clears
  `damageCount` first (0x80a517c). VERIFIED (the instructions; the dvars'
  defaults by `tools/re/cod4_dvars.py`). `ClientThink_real` hands
  `damageTime`, `level.time`, +0x3114 and +0x3110 to both blocks
  (0x80a5e6a..0x80a5e85, 0x80a5fef..0x80a6007). VERIFIED.
  `BG_CalculateViewAngles` opens with the view's kick: with a
  `damageTime`, factor `-0.5 frac + 1` (0x820aab4), times `frac * 0.5 +
  1` (0x8209628) under the view weapon's overlay; under 100 ms (0x820df10)
  `GetLeanFraction(t / 100)`, then `1 - GetLeanFraction(1 - r)` for `r =
  (t - 100) / -400 + 1` (0x820aab0) while `r` is over 0; pitch and roll
  each plus it times the factor and their kick. `BG_CalculateWeaponAngles`
  adds the gun's after the walk bob, before the spring: `k = 0.5 frac +
  0.5`, in over `100 k` ms, out over `400 k` (0x820dea8), scaled by `k`,
  times `-0.75 frac + 1` (0x820aad4, 0x820a318) under an overlay; pitch
  plus half the kick's pitch, yaw less the roll, roll plus half the roll.
  INFERRED, not yet confirmed on 1.7 instruction by instruction; the
  constants VERIFIED (the rodata at the addresses given). The memset of `ClientSpawn`
  zeroes the kick with the rest of the aim state. INFERRED.
  vcod keeps it in `AimState::kick` (`vcod_cod4::pmove::aim::DamageKick`).
- The kick capture (`play-1.7-kick-target.txt`, `-shooter.txt`,
  `-hits.txt`; recipe in the last's header): the sway capture's two
  probes under the `shot_kick` gsc, which from 10 s past the shooter's
  spawn hurts him every 1.3 s with `finishPlayerDamage` (flags 4, no
  knockback), in turn 40 along his view's forward, 25 along its right
  and 40 with no direction, and heals him at once. 23 rounds hit, the
  ones a hurt's kick turned among them (62050, 50 ms past a forward
  hurt, struck the right lower leg). VERIFIED live. Ours puts all 23
  along retail's direction within a ten-thousandth of a degree and
  through its line within 0.001 units
  (`cod4::shot_aim_ab::hurt_rounds_leave_along_retails_damage_kick`);
  without the kick 7.2 degrees off, two of retail's hits missed and four
  others struck. VERIFIED (the gate).
- The capture (`play-1.7-sway-target.txt`, `-shooter.txt`, `-hits.txt`;
  recipe in the last's header): retail under the `shot_aim` probe gsc,
  whose damage callback logs each round's point and direction and does no
  damage; `cod4_probe --script sway` fires an AK-47 at an idle target 200
  units off, sighted while strafing, leaning left and right and swinging
  the view, from the hip while strafing, and crouched. 38 rounds hit.
  VERIFIED live. Along the bare view from the plain eye ours put them up
  to 2.16 degrees and 7.98 units off retail's lines (the hip strafe worst,
  the view a cmd late), and three missed; with the block, all 38 leave
  along retail's direction within a ten-thousandth of a degree (the log's
  millionths) and through its line within 0.001 units
  (`cod4::shot_aim_ab`). VERIFIED (the gate).
- The client's own rounds (client-side bullets) leave from the same eye
  along the same angles: the predictor runs the aim block on each kept
  cmd's playerstate before its step, on the view the last kept cmd's
  step left (a snapshot's `viewangles` is not the server's to the bit),
  and hands each fire event its cmd's eye and angles
  (`play::cod4_predict::ShotAim`). 1.7's cgame fires along
  `cg.gunPitch`/`gunYaw`, which `CalculateWeaponPosition_SaveOffsetAngles`
  set in the last frame's view weapon: the view a frame back, the gun's
  axis up a sight. INFERRED. The
  predictor's cmd for frame stands in for that frame, and its gun
  spring takes no kick: the server's kick draws four `G_random`s off the
  level's own `rand()` stream (10.9), which no client can draw alike, and
  1.7's cgame draws its own (`BG_WeaponFireRecoil` in `CG_FireWeapon`),
  so a sighted burst's spring parts the two sides on retail as well.
  INFERRED (the two streams). On vcod-server an M16's sighted bursts
  stay within 0.05 degrees of the server's rounds without it, the view
  kick in the cmds' angles carrying the rest.
  `cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds` and its
  sighted twin hold each own round within 0.05 degrees of the server's.
  VERIFIED (the tests).
- The predictor's aim state counts from the first cmd it predicts, the
  server's from `ClientSpawn`: the idle clock (+0x3164) and the idle
  factor (+0x3148) agree only when the client predicts every frame from
  the spawn on, as its frame does. A session drawn first at the burst
  read an idle clock of 14 and a factor of 0.009 against the server's
  4687 and 1, and its sighted rounds left 0.02 to 0.04 degrees off the
  server's by the drift alone, 0.051 with the gun spring's kick on a
  burst's second round, by how long the join took. VERIFIED (both
  sides' aim state logged per cmd in the live test).
- The predictor's damage kick is the server's, worked out off the
  snapshot that first carries a step of `damageEvent`: its
  `damageCount`, its view and the direction bytes read back in 256ths,
  as the server packed them (a 1.4 degree step), timed at the snapshot's
  time less 20. The server runs every cmd of a packet at the frame the
  packet's newest cmd falls in (a packet of 13382, 13400 and 13416 at
  13400, one of 13432, 13450 and 13466 at 13450, VERIFIED on ours); the
  predictor runs a kept cmd's block at the newest snapshot's time or the
  later frame the newest cmd built has reached (`Cod4Predictor::
  note_damage`, `level_at`). INFERRED (the server's order above). A cmd
  first predicted as the frame's own cannot know its packet, which goes
  out on the client's 33 ms clock, so its kick may be a frame off: up to
  1.3 degrees early in the kick's return. INFERRED (the test below).
  1.7's cgame eases its own kick (`CG_DamageFeedback`), off
  `cg.snap->serverTime` and the bytes read as 255ths. VERIFIED
  (cod4-client-view.md 14.2). Rounds fired before the snapshot reaches
  the client leave without it. `cod4_live_hurt_rounds_take_the_servers_damage_kick` hurts
  us 40 (ahead, right, no direction) 280 ms before each burst and holds
  every own round within 1.5 degrees of the server's, most within 0.05;
  without the predictor's kick one read 3.9 off. VERIFIED (the test).
- `cod4_probe`'s sight key was 0x80000 (the detonator's) until this
  capture: `shoot` never sighted, so the wall capture (3.1) fired from the
  hip. The key is 0x800 (`net::clmsg::buttons::ADS`; the sway capture's
  `fWeaponPosFrac` rises to 1). VERIFIED live.

## 4. Damage

- `Bullet_GetDamage`: full inside `maxDamageRange`, `minDamage` past
  `minDamageRange`, lerped between; `G_DamageClient` times the weapon's
  location multiplier, at least 1, into `CodeCallback_PlayerDamage` with a
  tenth argument, `level.time` less the shot's time. INFERRED;
  the stock `_callbacksetup.gsc` takes ten and the killed callback nine
  (`timeOffset`, `deathAnimDuration`). VERIFIED.
- The stock weapon files' numbers (`weapon_dump` over `common_mp.ff`), damage / `minDamage` / `maxDamageRange`..`minDamageRange`:
  the M16, AK-47, G3, MP44 40/30 1500..2000; the M4 and G36C 30/20
  1500..2000; the M14 50/40; the MP5 and AK-74u 40/20 750..1000; the P90
  and Uzi 30/20 750..1000; the Skorpion 50/20 200..400; the SAW 30/30, the
  RPD 40/40, the M60E4 50/40 1000..1500; the bolt and semi-auto snipers
  70/70 (the M40A3 ACOG 75); the Winchester 40/10 and M1014 30/10 a
  pellet, 300..500 and 300..600; the pistols 40/20, the Desert Eagle
  50/30. A silencer shortens both ranges (the M16's to 500..1000). The
  rifles' and SMGs' location multipliers are 1.4 on the helmet and head
  and 1 elsewhere; the snipers' 1.5 on the helmet and head, 1.5 (the
  M40A3, M21) or 1.1 (the R700, Dragunov, Barrett) on the neck, 1.1 on the
  torso; the gun 0 on every weapon. VERIFIED (the weapon files).
- What a kill takes on ours, against those: `war`'s 100 health
  (`scr_player_maxhealth`) is three rounds of a 40 inside its full range,
  two with Stopping Power (`_class.gsc`'s `perk_bulletDamage` 40, so 56 a
  round: `bot_match`'s kill lines read 56 off the assault preset's M16
  and 107 off the sniper's M40A3 on the lower torso, and no bonus off the
  spec ops MP5 or the heavy gunner's SAW), four of a 30. `_healthoverlay.gsc` regenerates health 5 s
  after the last hit (`regenTime`), so the hits that do not finish a
  victim are spent: the 3.7 hits a kill of bot-objectives.md 12.4 counts
  every scored round on any body, not the rounds of one life. VERIFIED
  (the scripts and `bot_match`'s `K;` lines). That a round deals this on
  retail too is the kill gate's (section 8): the target's health after
  each of the shooter's rounds matches retail's every snapshot. VERIFIED
  (`cod4_combat_ab`).
- `finishPlayerDamage` (0x80ac58e, `builtins::cod4_combat`) returns at
  once while `client->lastStand` (gclient+0x317c) is set and
  `lastStandTime` (+0x3180) is past `level.time` (0x80ac5d7..0x80ac5ed),
  and for a damage under 1 (0x80ac604); an undefined inflictor or
  attacker reads as entity 1022 (0x80ac61e); a victim at `pm_type` 7 is
  the script error `Trying to do damage to a client that is already
  dead` (0x8218d4c, raised at 0x80acc09). VERIFIED (the instructions).
- Its flesh events are raised only for a weapon other than 0 whose
  `weapType` (+0x12c) is 0 and a means of death 1, 2 or 8
  (0x80acadf..0x80acde4): `EV_BULLET_HIT` (0x29) at the point, its parm
  the direction's byte, `surfType` 7, the attacker in
  `otherEntityNum`, the weapon, the scale 1 for hit location 2, and the
  victim's bit set in its excluded-client mask (0x80acb07..0x80acb80);
  then 0x2a, or 0x2b when `rifleBullet` (+0x4e0) is set, with the
  victim's client number and every client bit but his excluded
  (0x80acb87..0x80acbfd). VERIFIED (the instructions).
- Entity flag 1 ends the hit after the knockback (0x80ac714), and flag
  2 holds a killing damage to health less 1 (0x80aca05). VERIFIED (the
  instructions). vcod sets neither flag and carries neither arm.
- The hit takes the health (0x80ac891), notifies `damage` with the
  damage and the attacker (0x80ac8c3), and at health 0 or under
  (0x80ac8e9) goes to Last Stand when the flag is clear and the perks
  byte at gclient+0x5fc has bit 7 (0x80acc2a), else adds 2 to the first
  event's scale (0x80acca4) and calls the die handler. VERIFIED (the
  instructions). Before the die handler a health that `health + 999 >
  998` (unsigned, 0x80accab) catches is set to -999; ours sets 0, and
  what the dead playerstate's health reads on retail is not checked
  here. INFERRED.
- The knockback's scales are 1.7's floats: 0.3 a point standing
  (0x8209684), 0.15 crouched (0x820deac), 0.02 prone (0x8212488), over
  250 (0x8218dbc), at most 60 (0x80aca47), and `pm_time` twice it
  clamped to 50..200 (0x80acaaf, 0x80ace2a). VERIFIED (the instructions).
- The playerstate half on ours (`Damage4`): the knockback (0.3 a point standing, 0.15
  crouched, 0.02 prone, at most 60, times `g_knockback` 1000 over 250;
  `pm_time` twice it, 50..200, `PMF_TIME_KNOCKBACK`), `damageTimer` 100 ms
  a point to 750, and the blood for the end frame. INFERRED; the
  target's first hit reads `pm_flags` 0x100, `pm_time` 0x32 and
  `damageTimer` 750 (VERIFIED live, the gate).
- `P_DamageFeedback` at the end frame: `damageCount` the blood as a share
  of max health (at most 127), `damageEvent` stepped, the direction's
  pitch and yaw in 256ths, the count cleared 500 ms on. INFERRED; the capture's 56 of a 30-health target, yaw 47, pitch 3 match
  (VERIFIED, the gate).
- A kill: `death` with the attacker, `pm_type` 7, health 0, the weapon 0,
  then the killed callback. INFERRED (`player_die`); the death
  frame reads `pm_type` 7 and `weapon` 0. VERIFIED live.
- `player_die`'s constants (the death grenade drop, inlined from
  0x80b1af7): the launch 40 over the body (0x820ddf4), the velocity's
  scale 160 (0x82199f0), `perk_grenadeDeath`'s pointer 0x828ce78 (default
  `frag_grenade_short_mp`), `G_crandom` at 0x80cb57a as `rand() * 2^-31`
  (0x8212118) doubled less 1 (0x820a238). VERIFIED.
- `player_die` drops live grenades ahead of the `death` notify: a
  grenade held (`grenadeTimeLeft`, ps+0x44, not 0; the offhand under
  weapFlags 2, else the weapon) with its time left, then, unless the
  means of death is 12 (`MOD_SUICIDE`), Martyrdom (`perks` 0x40,
  0x80b1bb2) drops `perk_grenadeDeath` on its `fuseTime` (+0x3ec) and
  sets anim condition 9 to 2, each at three `G_crandom`s, spinning,
  through `G_FireGrenade` (0x80944a8). INFERRED (the branches). Ours draws the six values ahead
  of the death anim's draw, as retail's order is, and fires the grenades
  once the hit's playerstate half is applied; the anim condition is not
  set. So a grenade cooked to its end (`EV_GRENADE_SUICIDE`, time left
  -1) bursts in the hand.
- Last Stand (`perks` 0x80): `finishPlayerDamage` on a killing hit sets
  `client->lastStand` (gclient+0x317c) and `lastStandTime` at
  `level.time` + 500 (0x80acc2a..0x80acc49) instead of dying. INFERRED
  (the branch at 0x80acc2a). It then runs `CodeCallback_PlayerLastStand`
  through 0x80c293a, which pushes nine arguments as the killed
  callback's pusher at 0x80c2a64 does, but a `deathAnimDuration` of 0
  where that one passes its own (0x80c2970 against 0x80c2a9a). VERIFIED
  (the instructions). It takes no damage until
  `lastStandTime`, and `ClientEndFrame` puts the client in
  `PM_LASTSTAND` (6) while the flag holds; `ClientSpawn` clears it.
  INFERRED. The stock `Callback_PlayerLastStand` gives the
  pistol, and `lastStandTimer`'s 10 s later `suicide` bleeds it out;
  ours does both (`perk_tests::last_stand_goes_down_then_bleeds_out`). A
  body in last stand is in play: it is shot, collides and goes out as an
  entity as a standing one does.

## 5. The corpse and the obituary

`obituary` raises `EV_OBITUARY` (66) broadcast, the victim in
`otherEntityNum`, the attacker in `attackerEntityNum`, the parm the
weapon's number (27, the M16) or `0x80 |` the means of death for 7, 8, 9,
11, 12 and 15. INFERRED (`GScr_Obituary`); the capture's
obituary matches. VERIFIED (the gate). `clonePlayer` puts the corpse in
the body queue at 64, `eType` 2, its client number the victim's. VERIFIED
(the gate). `player_die` draws its death anim: `BG_AnimScriptEvent(ps,
ANIM_ET_DEATH, 0, 1)` over the body's last conditions, one of the
clause's commands by `rand()`, its duration the killed callback's
`deathAnimDuration` (`pmove::anim::event_choices`, kept at each end
frame). INFERRED. The kill capture's standing target dies on
`legsAnim` 36, one of the standing default clause's eight
(`pb_stand_death_neckdeath` .. `_neckdeath_thrash`), VERIFIED (the
capture; `the_kill_captures_death_anim_is_a_standing_death`); the draw is
the stream's 31st (10.9), VERIFIED (the gate). The corpse keeps the body's `eFlags` with
0xa0000 (0xa0002 off the target's 2) and `pos.trType` 1. VERIFIED live.
Its fall and the death anim's drag are 5.2. The clone's handler row (`.data` 0x826cf48,
`think` 0x80a8630) is `BodyEnd`, whose whole body clears `eFlags` 0x80000,
sets `r.contents` 0x4000000 and `r.svFlags` 0 (`cod4_lnxded` 0x80a8630..
0x80a864f). VERIFIED (the bytes). `clonePlayer` arms it at `level.time +
deathAnimDuration`. INFERRED (`PlayerCmd_ClonePlayer`).
vcod-server clears the bit then (`BodyQueue::run_thinks` in the level's
frame, ahead of the items); before, every corpse kept it for life and
a client reset replayed its death anim (cod4-player-anims.md 9).
`self suicide()` (`PlayerCmd_Suicide`) zeroes the health and runs
`player_die` with the client as inflictor and attacker, 100000 damage and
`MOD_SUICIDE` (12), so it draws a death anim as any death does. INFERRED.
vcod-server's CoD4 `suicide` does the same; `damage_soak`'s transitions'
corpses all play a `*death*` clip. VERIFIED. Trap: CoD1's `suicide` draws
none, and the cloned corpse keeps the live body's idle (`pb_stand_alert`),
a standing body with empty hands that no player collides with, on every
team or spectator switch of a live player (the stock menus kill him that
way).

The death's torso. `player_die` runs `BG_AnimScriptEvent(ps, 1, 0, 1)`
(0x80b1c00..0x80b1c21: the death event, not continuing, forced), and
`BG_ExecuteCommand` (0x804f9ca) plays a `both` line's torso with anim 0
(0x804fb1e) through the not-continuing arm that flips the toggle
(0x804feaf: `torsoAnim & 0x200 ^ 0x200`) and sets `torsoTimer` to the
duration. VERIFIED (the instructions). The command layout is
`bodyPart[2]`, `animIndex[2]`, `animDuration[2]` (the parser's stores at
0x804e3e9, 0x804e41f, 0x804e436), so a `both` line has no second part.
VERIFIED. `clonePlayer` copies the playerstate's word into the corpse
(0x80ad1c2). VERIFIED (the bytes). The flip always happens, on the
playerstate as the dying client's last cmd left it: a hardware watch on
the target's `torsoAnim` (client offset 0x94) under gdb saw 24 kills,
every one written once, by 0x804febf inside `player_die`'s call, to the
flip of the word it found. VERIFIED live. The words at the call and
after it, by the target's state:

| state | torso at the death | after (the corpse's) |
|---|---|---|
| standing, hit once a frame before (`pt_flinch_*`) | 0x308 or 0x305 | 0 |
| crouched | 0x200 | 0 |
| firing (`pt_*_shoot*`) | 0x13c, or 0x33c the next tap | 0x200, 0 |
| reloading | 0x321 | 0 |
| a jump's torso | 0x105 | 0x200 |
| switching weapons | 0x30b | 0 |

The corpses on the wire carried the same words, kill for kill. VERIFIED
(the shooter's capture). The prone life never lay down before the shot
(its `legsAnim` read the standing idle, 0x2b9); no prone death is in the
run. VERIFIED live. The kill capture's target took its first
round in one frame and died the next: the cmd between ran the flinch
(the `flinch_*` movetypes' torso line, 0x308) after the 47950 snapshot
went out, so the wire never showed it, and the death flipped it to 0.
INFERRED from the table's first row. vcod-server draws the death's
choices after every cmd (`Cod4Level::think`), and `cod4::combat_ab`
matches with no GAPS. VERIFIED (the gate). Drawn once a frame at
`ClientEndFrame`, before that cmd, they flip the stale 0 to 0x200. `anim_census::a_death_flips_the_torso_toggle_it_finds`
holds the table's pairs.

The corpse run's recipe:
`cod4_lnxded` under gdb in the namespace of `cod4-handshake.md` 7, `+set
scr_player_maxhealth 30 +devmap mp_crash`, `war`; `target` joins allies
with `cod4_probe --script poses --respawn --goto "207 2291 296"` (one
state a life), `shooter` axis with `--script shoot --fire-after 2.5
--goto "123 2110 300"`, a spectator beside them, 240 s. gdb breaks at
0x80b1c21 and 0x80b1c26 (the call and its return) and watches each dying
client's 0x94.

### 5.1 The ragdoll

- The stock `_globallogic.gsc` threads `delayStartRagdoll(body, ...)` off
  the killed callback's `clonePlayer`: unless the death anim has an
  `ignore_ragdoll` notetrack it waits 0.2 s, then the anim's length times
  its `start_ragdoll` notetrack's time (0.35 without one), and calls
  `body startRagdoll(1)` (`level.oldschool` calls it at once and adds a
  `physicsExplosionSphere`). VERIFIED (the zone's rawfile).
- `startRagdoll` (`cod4_lnxded` 0x80c4f08) reads its optional int and drops
  it, then turns `s.lerp.pos.trType` (entity +0xc) and
  `s.lerp.apos.trType` (+0x30) into ragdoll types: `TR_INTERPOLATE` (1)
  to 11, `TR_GRAVITY` (5) to 10, anything else to 9. It evaluates no
  trajectory. `isRagdoll` (0x80c4eb4) answers whether `pos.trType` is 9,
  10 or 11. VERIFIED (the bytes).
- In the kill capture the corpse (64) goes from `pos` 1 / `apos` 0 to
  11 / 9 at 49550, 1550 ms after the clone at 48000, on both clients, and
  again at 52650 in the killcam's replay (clone at 51100). VERIFIED (the
  capture). vcod-server's `startRagdoll` and `isRagdoll` do the same on a
  corpse and nothing on another entity; the gate
  `cod4::combat_ab::the_corpse_turns_ragdoll_when_retails_did` holds both
  clients' type changes to retail's. VERIFIED (the gate, ours). The
  server runs no ragdoll: the client sees the type and simulates the
  bones (cod4-player-anims.md 10).

### 5.2 The corpse's fall and drag

- `clonePlayer` (0x80acf50) gives the clone `pos.trType` 5
  (`TR_GRAVITY`, 0x80ad004) from `level.time` (0x80ad010) at the
  player's origin (`G_SetOrigin`, 0x80acfeb) and velocity, x and y each
  capped above at `g_clonePlayerMaxVelocity` (80, 0x80ad058..0x80ad082),
  marks the corpse slot falling (+0x4d8 = 1, 0x80ad0b9), `clipmask`
  0x10001 and contents 0x4004000 (0x80ad1ce, 0x80ad1d8). VERIFIED (the
  stores).
- `G_RunCorpseMove` (0x80b97e0) runs in the entity pass: the root motion
  since the last frame (`XAnimCalcDelta`, 0x80b98a1); falling, the
  gravity point at `level.time`; resting, only when that motion's
  `Vec3LengthSq` is above 1 (0x80b9c3c), `pos.trBase` plus the motion
  along the corpse's forward and left (`AngleVectors` of
  `r.currentAngles`, 0x80b9c96). A capsule sweep from `r.currentOrigin`
  to there, the origin lerped by the fraction (0x80b9983..0x80b99fd). A
  clear sweep on the drag rests it at the end (`trType` 11 after
  `startRagdoll`, else 1, 0x80b9a74..0x80b9a96) and, when nothing is a
  unit under it (0x80b9b4c), drops it at the motion's pace (10 or 5,
  0x80b9b66). VERIFIED (the instructions). A fall that stops on ground
  rests it there with the hit as its ground and its angles on the
  surface, one stopped on a wall is nudged off along the normal and
  dropped again, one stopped in contents 0x80000000 is freed. INFERRED
 (`G_BounceCorpse`; CoD2's measured shape,
  cod2-combat.md 6.2).
- The kill capture's corpse (64) at 48000 reads (208, 2294, 236) and
  `trType` 1, landed in its first frame, then drags to (214, 2310, 236)
  by 48650 and holds: the death anim's later frames move its root under a
  unit a frame, which the length test drops. The killcam's replay
  repeats the path from 51100. VERIFIED (the capture, the wire's rounded
  origins).
- vcod: `game::cod4::corpse` (`launch` in `clonePlayer`, `run_corpses`,
  the move `game::corpse::step`, CoD2's frame with 1.7's trace and types;
  ahead of the body queue's thinks): the node time stepped by `0.001 *
  level.frametime` at rate 1, the motion off the clip's root tracks
  (`vcod_cod4::rootmotion::RootClip`, the mantle's translation sample and
  CoD2's yaw turn, INFERRED on 1.7), then the anim update that starts a new
  legs anim at 0. `cod4::combat_ab::
  the_corpse_drifts_on_its_death_anim_as_on_retail` holds the corpse's
  rounded `pos.trBase` path to retail's, live and in the killcam (a
  clone set down where a box trace under it meets the floor never moves).

## 6. The dropped weapon

`dropWeaponForDeath` drops the last droppable weapon through `dropItem`:
an `ET_ITEM` (3) whose `index` is the weapon number (27), from the middle
of the body's box along its yaw at `g_dropForwardSpeed` (10) and up at
`g_dropUpSpeedBase` (10) (VERIFIED, the defaults), falling under gravity
until a trace meets the world. INFERRED (`Drop_Item`,
`LaunchItem`). The three `crandom` spreads are drawn and added (10.9);
the start at `tag_weapon_right` and `G_RunItem`'s bounce are left out. `itemWeaponSetAmmo(clip, stock)`
keeps the counts; a living player whose box meets an item at rest and who
holds the same weapon takes clip and stock into the reserve up to
`maxAmmo` (+820), gets `EV_AMMO_PICKUP` (9) and the item goes
(`Touch_Item`, `WeaponPickup_Touch`, `Add_Ammo`). INFERRED;
`a_dropped_rifle_hands_its_ammo_over` runs it on the kill capture's
rifle. VERIFIED (the gate). Not run: the use key's swap for a weapon not
held, the compatible-weapon rule, the pickup prints.

- The dropped item's `clientNum` is its dropper for 1000 ms, then 64:
  in `play-1.7-planefx.txt` the probe's `dropItem` at 47500 makes item
  317 with `clientNum` 0, which reads 64 from the 48500 snapshot on.
  VERIFIED (the capture). The store is `cod4_lnxded` 0x8090050 (64 into
  entity + 0x8c), the drop's think. VERIFIED (the bytes); that it runs
  off a `nextthink` 1000 ms past the drop is INFERRED (`LaunchItem`, `DroppedItemClearOwner`, matching the
  capture). Until then the use list cuts it for its dropper
  (`BG_CanItemBeGrabbed`, cod4-gsc.md 9.1): the dropper's cursor hint
  for it (31, the M16's 27 plus 4) appears in the same 48500 snapshot.
  VERIFIED (the capture). vcod: `Item4::clear_owner_at`, held to
  retail's by `cod4::planefx_ab`. VERIFIED (the gate, ours).

## 7. The killcam

`_killcam.gsc` sets `spectatorclient` to the attacker and `archivetime`
to `camtime + predelay` (2.5 + 2 s). The engine sends the follower the
followed client's archived playerstate that long ago, `otherFlags` 2,
`deltaTime` the look back, `killCamEntity` 1023, trimmed to the oldest
frame the followed client was a player in, and writes the trim back into
`archivetime`. `killCamEntity` is the script's `self.killcamentity`
(gclient +0x2f6c), 1023 for -1 (the `cmovne` at 0x80a4aa9); its setter
(0x808b288) refuses anything but -1..1023 (`killcamentity can only be
set to -1, or a valid entity number`, 0x8214d3c), and `ClientSpawn` and
the killcam's end write -1 (0x80af26b, 0x80a4afb). VERIFIED (the
instructions). The stock `_globallogic.gsc` passes the inflictor's number
for a `cobra` kill (the helicopter) and -1 otherwise. VERIFIED (the
script). vcod reads the field into the killcam frame
(`cod4::combat_ab::a_killcam_entity_rides_the_killcam_playerstate`). The kill capture's target at 50000 reads the shooter's
spawn frame of 46900 with `deltaTime` 3100 where 4500 was asked.
VERIFIED live; the trim's rule is INFERRED from that. Ours matches. The
follower is sent the entities of the snapshot round at or before that
frame (each round's entities kept 200 frames, the playerstates'
look-back): retail's killcam frames carry the victim's living body
(entity 0, `eType` 1) where the live world holds only its corpse.
VERIFIED live; without the archive 22 of the gate's killcam rows fail,
with it none. VERIFIED (the gate).

The archived playerstate's clocks move up by the look back: the kill
capture's first killcam frame at 50000 reads `commandTime` 50000, the
shooter's 46900 plus the `deltaTime` 3100. VERIFIED live
(`play-1.7-kill-target.txt` message 132). The same shift covers
`pm_time`, `foliageSoundTime`, `jumpTime`, `viewHeightLerpTime`,
`shellshockTime` and the archival hudelems' `time`, `scaleStartTime`,
`moveStartTime` and `fadeStartTime` (no later than now), each only when
set, up to the first archival hudelem of type 0 (31 at most), and adds
to `deltaTime`: `SV_GetArchivedClientInfo` (`cod4_lnxded` 0x81795e2),
the playerstate's +0, +0x18, +0x54, +0x80, +0x11c and +0x630, the
hudelems' +0x70, +0x2c (clamped to `svs.time`), +0x50 and +0x68 (the
array at +0x1c04, 0xa0 apart), then +0x89c (0x81797bc..0x81798aa).
VERIFIED (the instructions). Without it the follower's `commandTime` sits
seconds behind its own cmds and the client's `CG_DrawDisconnect`
(`iw3mp.exe` 0x42f930, cod4-hud.md 14) draws Connection Interrupted for
the whole killcam: 260 of 265 frames in vcod's live gate without the
shift, none with it. vcod: `level::shift_archived`; `cod4_combat_ab` compares the
killcam's first `commandTime`; live gate
`cod4_live_killcam_draws_no_interrupt_and_the_use_key_skips_it`.

The skip: `_killcam::waitSkipKillcamButton` waits for `useButtonPressed`
to go 0 then 1. 1.7's `useButtonPressed` (`cod4_lnxded` 0x80a9b46) tests
`(buttons | buttonsSinceLastFrame) & 0x28` off the client (the OR at
client+0x3094); `attackButtonPressed` (0x80a9aae) tests 0x1 and
`adsButtonPressed` (0x80a9a14) 0x800. VERIFIED (the code). A follower's
`SpectatorThink` stores its cmd's buttons there. INFERRED, not yet
confirmed on 1.7.
vcod reads the CoD4 words for the three (`builtins::cod4_script`), and
the live gate's press ends the killcam a frame later. CoD1's last-cmd
word, which a follower's cmds never reach, never skips.

## 8. `cod4_combat_ab`

`cargo test -p vcod-server --lib -- cod4::combat_ab --nocapture`
(`COD4_DIR`, `COMBAT_REPORT=1` for every row): the kill capture's two
probes join ours in process on retail's clock, each sends its `setviewpos`
when its twin did, and from their spawns (45950, 46900) the captures' own
cmds. Compared against retail by server time: the shooter's weapon words
every snapshot from 47550 to the capture's end (63350), the target's damage words,
health, and the flesh event, corpse, item and obituary from 47500 to 48400,
the shooter's `EV_BULLET_HIT` parms, and the killcam's first frame and look
back. Every compared value matches retail, the entity numbers included
(section 14).

The gate has no `GAPS`: the corpse's `torsoAnim`, ours 0x200 against
retail's 0 until the death drew off the live playerstate (section 5),
was its last.

The corpse's `legsAnim`, `torsoAnim`, `eFlags`, `pos.trType` and
`groundEntityNum`, and the killcam frames' players and corpses, are
compared too. VERIFIED (the gate).

VERIFIED (the gate). `getCorpseAnim` and `setWaypoint`, which ended the
ragdoll's and the death icon's threads here once, are ported
(cod4-gsc.md 4).

The live client's inputs: `the_sight_the_trigger_and_a_switch_drive_the_weapon`
sends the shooter, over the wire, the sight key 0x800, then 0x801, then a
weapon byte of 11 (the Beretta `weapnext` selects): `fWeaponPosFrac` reaches
1 with `PMF_SIGHT`, `EV_FIRE_WEAPON` rides the ring and the M16's
clip drops 30 to 27 (one burst), and by 52900 `weapon` is 11. VERIFIED
(the gate).

## 9. Hit detection against the aim

`a_shot_at_the_head_scores_the_head`: on the kill pair, from 47450 the
shooter's cmd views the centre of the target's posed head box (the box of
hit location 2, through the same pose the trace uses) from its eye over its
`delta_angles`, sights up and taps once; the first round scores location 2.
VERIFIED (the gate). The shooter's view, eye and the bodies' hit boxes agree
on ours. The hip spread is wide: the capture's hip rounds leave 3 to 6.3
degrees off the view (21 units at 200), so a hip shot at a head misses most
of the time, as on retail. VERIFIED (the gate's spreads). Still left out:
the walk bob and the lean in the eye and the gun's sway angles (section 3),
which a client's own aim includes.

## 10. Grenades, the launcher and the melee

### 10.1 The offhand in pmove

`PM_Weapon_CheckForOffHand`: off a turret, with weapons on, not sprinting
and not holding a main-hand grenade, outside the offhand and night
vision, the cmd's offhand byte is taken where held, and the frag key
(0x4000) or the smoke key (0x8000, the flash class under
`offhandSecondary`) switches to the first weapon of its class with a
round (`EV_SWITCH_OFFHAND`, the weapon in the parm); the main weapon goes
down at its quick drop (`OFFHAND_LOWER`, 15), or with empty hands straight
to the pin (`GRENADE_PIN`, 16, `holdFireTime`, `EV_PREP_OFFHAND`),
the fuse at `GRENADE_HELD` (18), the throw anim at its `fireTime` and the
release at `fireDelay` (`GRENADE_THROW`, 17), the release
(`EV_USE_OFFHAND`, a round, `GRENADE_GONE`, 19) and the quick raise
(`OFFHAND_RAISE`, 20). Held, the key keeps the grenade at 18; under
`bCookOffHold` the fuse runs down there and at 0 reads -1 with
`EV_GRENADE_SUICIDE`. INFERRED, each step.
`frag_grenade_mp` reads `holdFireTime` 600, `fireTime` 360,
`fuseTime` 3500, cook on, quick raise and drop 500, offhand class 1.
VERIFIED (`weapon_dump`, `common_mp.ff`). The walk capture's throw reads
states 15, 16 (`weaponTime` 0x238), 17 (0x158, delay 0x71, the fuse
0xd8c) and 19 at 66050 then 20. VERIFIED live. `cod4_moves_ab`'s `frag`
rows replay it armed from 64500: every weapon word exact but
`grenadeTimeLeft` on the throw's row, which the server's
The grenade timer init zeroes inside `ClientEvents`. VERIFIED (the gate).
A main-hand grenade's pin raises `EV_PULLBACK_WEAPON` (25). INFERRED.

### 10.2 The missile

`G_UseOffHand` and `Weapon_Throw_Grenade` throw from the eye along the
view at `projectileSpeed` (940) plus `projectileSpeedUp` (120) up and
the thrower's speed along the throw, `G_FireGrenade` truncating the speed
whole; `G_InitGrenadeEntity` makes an `ET_MISSILE` (4) with `eFlags`
0x1000000, `clientNum` 64 and `index` the weapon (a frag is a pickup
item), `G_InitGrenadeMovement` the spin (pitch less 120, two random
rates), the no-draw time hides it 20..50 ms by the speed, and
`grenade_fire` goes to the thrower with the grenade and the name.
INFERRED. The walk capture's frag reads `trDelta` (-664, -664,
120), `launchTime` 29 past `trTime`, `trType` 5, `apos.trType` 2,
`eFlags` 0x1000000, `clientNum` 64, `index` 40. VERIFIED live; ours
matches each of those words on every row. VERIFIED
(`the_frag_flies_on_ours_as_on_retail`).

`G_RunMissile`: the arc's next point traced through the clip map, the
static models (`CM_PointTraceStaticModels`) and the bodies' hit boxes
under 0x2806891 passing the thrower; a bouncing grenade near a floor snaps
onto it (0.135 up, 1.5 down); on a contact `MissileImpact`: off the world
or an undamageable entity `BounceMissile` reflects the move at the moment
of contact and scales it by the weapon's `parallelBounce` and
`perpendicularBounce` for the surface type by the angle, and on a floor
(normal z over 0.7) under 20 units a second the grenade comes to rest
lying along it; a body takes the weapon's `damage` as `MOD_IMPACT` above
`g_minGrenadeDamageSpeed` (400) and bounces it as flesh;
`EV_GRENADE_BOUNCE` (44) carries the surface type where the move changed
by over 100. INFERRED, each step. The walk capture's frag
bounces at 66550 and 67100 (`events` 0x2c, parms 0x16 and 5), off a wall
at 67150 and comes to rest at 67300. VERIFIED live.

The wall at 67150 is static model 1901 of mp_crash's clip map, an
`fx_me_brick_01` rubble piece (origin (-529.1, 1447.7, 276), scale
1.264) whose collision surface is concrete (surface type 5, the bounce's
parm). The map zone names that xmodel only (`,fx_me_brick_01`, a
reference to an asset another zone holds); common_mp holds it, with one
collision surface of 12 triangles. VERIFIED (the zones). mp_crash places
219 of them. `ClipMap::from_zones` takes a comma model from the zones
loaded before the map's, and every walk keeps an xmodel's collision
(`XModel::coll`). From the map zone alone the bricks clip nothing and
the frag flies past the wall.

The arithmetic, VERIFIED (the disassembly):

- `BG_EvaluateTrajectory` (0x80536ba) and `BG_EvaluateTrajectoryDelta`
  (0x8053124): the time is the float 0.001 (0x8209640) times the ms, every
  axis `delta t + base` kept on the stack, gravity's `-400 t t`
  (0x820aab0) added to the kept z; the delta's `-800 t` (0x820aaac).
- `BounceMissile` (0x80946ba): `hitTime` is `(int)` of the stored float
  `(time - previousTime) * fraction`. The dot's first two terms are
  stored as floats, the last kept, and the reflection's scale is
  `-2.0` times the kept sum; the stored dot over the kept length
  `-|v|` is the cosine, and the factor `(perp - par) cos + par` stays on
  the stack for the three products. The push off the surface is the
  normal times the float 0.1 (0x8209648), z clamped to 0, added on the
  stack.
- `G_InitGrenadeMovement`'s spin (0x8093012): `G_flrand(320, 800)` and
  `G_flrand(180, 540)`, each times `2 G_irand(0, 2) - 1`. `G_flrand`
  (0x80cb548) is `(max - min) rand() * -1/2^31 + min` (the float at
  0x8214ac4 is negative), so a draw falls in `(2 min - max, min]`;
  `G_irand` (0x80cb522) divides the wrapped 32-bit product by
  `(int)0x80000000`, so `G_irand(0, 2)` is 0 but for a draw of 2^30 and
  the spin turns backwards on both axes. `MissileLandAngles` (0x80930d4)
  draws `rand() & 0x7f` less 63 and `G_random` (0x80ca2ee, `rand() /
  2^31`). The walk capture's spin, -296.36 and -102.30 degrees a second,
  is in those ranges and backwards. VERIFIED live.
- `rand()` is glibc's (the PLT slot 0x826a768); `G_InitGame` seeds it
  with `srand` (0x80b56ee). Every caller before the throw is modelled
  (10.9).

`the_frag_flies_on_ours_as_on_retail` meets every bounce on the same
frame as retail, and the bounce points, times and events, the spin and
its turns (10.9), the rest, the burst and the script's quake match on
every row, word for word; `FRAG_GAPS` is empty. VERIFIED (the gate).
Three float models closed the last bits: the second bounce, off a
terrain triangle, takes 1.7's point sweep (the cross product kept on the
stack and stored, `Vec3NormalizeTo`, cod4-collision.md 3); the third,
off the rubble, the static model's normal rounded the same way
(cod4-collision.md 4); and the burst's x, which read a unit off, is
`G_TempEntity`'s truncation of the resting point (-523.58 to -523), which
the wire's rounding of the grenade's own -524 had hidden (10.3).

### 10.3 The burst

At its fuse `G_ExplodeMissile`: `explode` with the point, an event entity
(`ET_GENERAL`, `eFlags` 0x20, the weapon) with `EV_GRENADE_EXPLODE` (45)
and the normal and surface type a trace 16 down meets, freed 300 ms on (a
smoke cloud's lives 61 s with `eFlags` 0x90000), then `G_RadiusDamage`:
every player in `explosionRadius` but the ignored one, through
`CanDamage`'s five traces (the body's middle half way to the eye and four
points 15 to each side and half that height up and down) under 0x802011
inside the cone, the inner damage falling to the outer at the edge times
a third a trace through (all at four or five), radius flags 5, the
direction toward the body 24 up, `MOD_GRENADE_SPLASH`; then `death` and
the grenade freed. INFERRED. The capture's burst entity 317 is `eType` 0, `eFlags`
0x20, weapon 40, event 0x2d with parm 5, and the script's earthquake
(`_shellshock.gsc`'s `grenade_earthQuake`, 0.3, 0.5 s, 400) rides 17 + 61
at the same point. VERIFIED live. The burst's point is
the grenade's truncated (`G_TempEntity`'s `SnapVector`, as every temp
entity's), the quake's too: the resting grenade at x -523.58 goes out as
-524 (the field's rounding) and its burst as -523. VERIFIED (the
capture). Ours raises both with those words at retail's point. VERIFIED
(the gate).

A claymore or C4 kill needs the `owner` the script gives the grenade on
its `grenade_fire` (`_weapons.gsc` `watchClaymores`, `watchC4`):
`Callback_PlayerDamage` (`_globallogic.gsc`) sets
`explosiveInfo["bulletPenetrationKill"]` only for a `c4_` or `claymore_`
weapon whose inflictor has one, and `_missions.gsc` `ch_kills` reads it
on every such enemy kill (line 581 for the claymore), so a burst without
an owner aborts there with "cannot cast undefined to bool". The frag,
RPG and launcher arms read only fields the damage callback always sets,
and a kill whose attacker is no player (a damage trigger) never reaches
`ch_kills` (`_globallogic.gsc` `Callback_PlayerKilled` calls
`_missions::playerKilled` only for an enemy player). VERIFIED (the
1.7 rawfiles in `common_mp.ff`). A planted claymore, C4, frag and RPG
kill at 100 health run clean on ours; only a test that bursts a grenade
in the call that threw it, ahead of the frame's notifies, met the abort.
VERIFIED (`a_claymore_kill_runs_the_kill_scripts_clean`).

An impact grenade (`projImpactExplode`, the launcher's `gl_m16_mp`:
speed 2400, up 10, arming distance 375, inner 155 and outer 25 over 300,
VERIFIED `weapon_dump`) bursts on its own entity where it struck past its
arming distance (the event on the missile, `projectile_impact` to the
thrower) and bounces as a dud short of it; `Weapon_GrenadeLauncher_Fire`
fires it with no spin and no fuse. INFERRED; no capture fires
the launcher. `the_grenade_launcher_fires_its_grenade` switches the kill
pair's shooter to the M16's alt and fires: the grenade (weapon 28)
strikes inside its arming distance and bounces on as a dud. VERIFIED (the
gate).

`Weapon_GrenadeLauncher_Fire` (`cod4_lnxded` 0x80a2f30) marks its
grenade stable right after `G_FireGrenade` returns it: `orl $0x20000,
0x180(%eax)` at 0x80a2f9b, the entity's `flags` (`FL_STABLE_MISSILES`).
VERIFIED (the disassembly). `G_RunMissile` wobbles every unguided
`weapType` 2 missile without that flag (0x80979dd), the launcher's grenade
included. INFERRED (the branch). vcod-server left the flag off, so the
grenade's first free step took the wobble with no wait, went
`TR_INTERPOLATE` and, run as a grenade (its arc evaluated, not stepped),
hung where it stood 120 units ahead of the eye and never burst. VERIFIED
live (vcod-server and our client). `cod4_live_grenade_launcher_fires_along_the_view`
(client `play::cod4`) fires the assault class's launcher through the
action slot and the trigger on vcod-server: the grenade goes out
`TR_GRAVITY` at 2400 along the view and bursts (`EV_GRENADE_EXPLODE` on
its own entity) 668 units on. VERIFIED (the test, ours).

Damage from a grenade, the launcher or a turret takes
`info/mp_lochit_dmgtable`'s multipliers, 1 everywhere but the gun's 0, not
the weapon's (`G_GetWeaponHitLocationMultiplier`). INFERRED;
the table is VERIFIED (the `common_mp` rawfile). Before that the frag's
splash did 1 point on a live run, its own table's 0 for no location.
VERIFIED live.

Left out of this section: water and glass, the live grenade's pickup and
throw back (working, ungated, 12), damage to entities other than players
(13), rockets (12). The flashbang's blast and
the concussion's shock are 10.8.

### 10.4 The melee

`PM_Weapon_CheckForMelee`: the melee key's edge (0x4) on a weapon with
`meleeDamage`, outside the melee, the offhand and night vision, not
through a scope, no delayed action and none pending but a reload's, from
ready or past the raise and drop states: `MELEE_SWING` (12) for
`meleeTime` with `EV_MELEE_SWIPE` (30) and the `meleeattack` or
`knife_melee` anim event, at `meleeDelay` `MELEE_HIT` (13) with
`EV_FIRE_MELEE` (31), then a knife weapon's quick raise (`MELEE_RECOVER`, 14,
anim 20) or ready. INFERRED. On `EV_FIRE_MELEE` the server's
`FireWeaponMelee` traces from the eye along the view (the bodies rewound)
`Melee_Trace`'s centre and four corners `player_meleeWidth` and `Height`
(10, 10) apart at `player_meleeRange` (64; VERIFIED, `cod4_dvars.py`),
then the two cross traces; the first contact raises `EV_MELEE_HIT` (35; a
body, with `EV_MELEE_BLOOD` on the attacker) or `EV_MELEE_MISS` (36) with
the knife bit in the parm, and a body takes `meleeDamage` plus `G_rand()
% 5` as `MOD_MELEE` (no location multiplier). INFERRED. The ray
ends' signs are `cod4_lnxded` rodata 0x82173e0 (the centre, then upper
right, upper left, lower right, lower left), read at 0x80a24c0 by the
rays and at 0x80a26ab by the cross traces; every trace is under mask
0x2806891 (0x80a2417, 0x80a25a0) and `bulletPriorityMap` (0x80a240f,
0x80a2598), and tests surface flag 0x10 (0x80a247b, 0x80a25fc). VERIFIED
(the floats and the loads). The cross traces join corner 1 to 2 and 3
to 4 and refuse a start in solid (0x80a2602). INFERRED (the index
arithmetic). The
M16's `meleeDamage` 135, delay 100, time 660 (VERIFIED `weapon_dump`).
`the_melee_key_swings_and_hits` runs it in process. VERIFIED (the gate).

The lunge: a press whose cmd carries `meleeChargeDist` (the client's pick
of a target 64 to 128 units off near the crosshair) sets `PMF_MELEE_CHARGE`
(`PM_MeleeChargeStart`, `cod4_lnxded` 0x80561e6) and, on a weapon with a
charge anim and `meleeChargeTime`, swings the charge swing: weapon anim 9,
`meleeChargeDelay` to the hit (159 on the stock rifles against the plain
129) and `meleeChargeTime` (1159 against 800), `KNIFE_MELEE_CHARGE` (0x13)
in the anim script. VERIFIED (0x806346a..0x806350c; the zone). Pmove
throws the body along the cmd's yaw at `sqrt(2 * dist * 1200)` and
`player_meleeChargeFriction` stops it in `dist` units, so the hit's trace
at `player_meleeRange` starts beside the target. VERIFIED (cod4-pmove.md
19 has the addresses and the client's pick).
`cod4_live_the_melee_key_lunges_at_a_bot_in_reach` (vcod, client and
server) lunges at a bot 100 units off. VERIFIED (the gate).

### 10.5 `kill`

`Cmd_Kill_f`: a playing client dies by its own hand under `CheatsOk`
(`sv_cheats` and health), as `player_die(self, self, self, 100000,
MOD_SUICIDE)` with the death anim and `CodeCallback_PlayerKilled`;
without cheats `CheatsOk` prints `e "GAME_CHEATSNOTENABLED"`, a dead
client `"GAME_MUSTBEALIVECOMMAND"`. INFERRED; both strings are
in `cod4_lnxded` (VERIFIED, `strings`). Whether 1.7's `kill` asks
`CheatsOk` is not measured. `the_kill_command_kills_its_sender`. VERIFIED
(the gate).

### 10.6 Live

A scratch script (`live.sh frag|melee`, not committed): vcod-server
`--game cod4 mp_crash` with `sv_cheats`, `cod4_probe` `target` (allies,
`idle`, `--goto 207 2291 296`) and `attacker` (axis, `--script frag` from
123 2110 300, or `--script melee` from 180 2260 300). The frag hit the
target as it flew (15, `MOD_IMPACT`, `torso_lower`) and burst 3.3 s after
the throw, killing the target (221) and the thrower (199); the melee's
first swing killed the target (137, `MOD_MELEE`). VERIFIED live (the
server's `D;`/`K;` log lines). The scripts start 5 s past the spawn, past
the prematch freeze (`pm_flags` 0x800 masks the keys).

### 10.7 The damage soak

`cod4::damage_soak` hunts a state in which no shot hurts. Two in-process
clients (allies `target` in slot 0, axis `shooter` in slot 1) on mp_crash
`war` under `sv_cheats` run 33 steps one after another: bullet kills
each way, `kill`, all five stock classes, a team swap, autoassign, the
back swap, spectator and back, a cooked frag held past its fuse, melee
kills each way, a firefight both fire in, a `kill` while the other watches its killcam,
and a `disconnect` followed by a new join (in the next free slot, the old
one still a zombie). After each step both are put 150 units apart and
each sights and fires at the other's head box. On ours every round hurts
its victim on every step, with the cmds on the server's clock and with
them 120 ms behind it (each shot rewound, section 9). VERIFIED (the gates
`cod4_damage_survives_every_transition` and
`..._on_a_lagged_link`). Every stock map poses both teams' heads with
each class's body (urban, desert and woodland sets).
VERIFIED (`cod4_every_map_poses_both_teams`, ignored: 21 map loads,
about 50 s).

Two steps of the soak are the stock script's rules, not ours. The team
menu's answer and the class's in one packet lose the class, on ours and
on retail (cod4-map-cycle.md 5), so the soak sends the class a second
later, as a click does.
A side away past `default_onForfeit`'s 20 s ends nothing:
`updateGameEvents` forfeits only under `level.rankedMatch`. INFERRED (the
script, `_globallogic.gsc`); VERIFIED that ours plays on
(`cod4_damage_survives_a_long_spectate`).

The one state the soak reaches in which nobody hurts anybody is the
match's end. At the time limit `endGame` sets `game["state"]` to
`postgame`, the first thing `Callback_PlayerDamage` returns on, freezes
everyone (`pm_flags` 0x800) for `postRoundTime`, then puts everyone in
the intermission (`pm_type` 5) and, 30 s on (`scr_intermission_time`),
calls `exitLevel(false)`. INFERRED (the script). That queues
`map_rotate`, which spawns the rotation's next map or restarts the level
(cod4-map-cycle.md 3). On ours a 1-minute `scr_war_timelimit` match
rotates and both players hurt each other on the next level. VERIFIED
(`cod4_damage_survives_the_time_limit` and `..._restarted`). `war`'s
default limit is 10 minutes (`scr_war_timelimit`), so any session of
that length reaches it.

### 10.8 The flashbang and the concussion grenade

`flash_grenade_mp` is an offhand of class 3 with a 1.5 s fuse,
`projExplosion` 2 (flashbang), no explosion damage, `explosionRadius`
800 and `explosionRadiusMin` 200; `concussion_grenade_mp` is class 2
(the smoke slot: it waits to land before it bursts, as smoke does), a
1 s fuse, `projExplosion` 3 (`EV_CUSTOM_EXPLODE`), inner and outer
damage 1 over 512. VERIFIED (`weapon_dump`, `common_mp.ff`).

The flashbang's `G_ExplodeMissile` raises `EV_FLASHBANG_EXPLODE` (48)
and then `G_FlashbangBlast`: every client with `takedamage` within the
outer radius whom one of `CanDamage`'s traces (mask 0x801, no cone)
reaches is notified `flashbang` with the share of the way in from the
outer radius to the inner one (1 inside it), `(dot + 1) / 2` of the view
forward and the eye-to-burst direction, the thrower (or undefined) and
the thrower's team at the throw. INFERRED. `_flashgrenades.gsc`'s `monitorFlash`, started for
each player by `_globallogic.gsc`, clamps the angle share to 0.5..1
(over 0.8 reads 1), takes `distance * angle * 6` seconds, drops a shock
under 0.25 s, applies `level.friendlyfire`'s team rules, and a frame
later (`applyFlash`'s `wait .05`) calls `shellShock("flashbang",
duration)`. VERIFIED (the script, the `common_mp` rawfile).

A concussion's one point of splash goes through
`Callback_PlayerDamage`'s `finishPlayerDamageWrapper` into
`_weapons.gsc`'s `onWeaponDamage`, which takes `2 + 4 * (1 - d / 512)`
seconds off the distance `d` from the victim's origin to the grenade's
and calls `shellShock("concussion_grenade_mp", time)` a frame on.
VERIFIED (the script). The grenade's script entity carries the
missile's `r.currentOrigin` in `origin` for that read (ours writes it
each time the missile moves).

`GScr_ShellShock` (`cod4_lnxded` 0x80c4a36) takes exactly two
arguments (else `USAGE: <player> shellshock(<shellshockname>,
<duration>)`, 0x821ed44), finds the name among configstrings 1955..1969
(`strcasecmp`, else `shellshock '%s' was not precached`, 0x821eda8),
takes `floor(duration * 1000 + 0.5)` and refuses one above 60000 as
unsigned (`duration %g should be >= 0 and <= 60`, 0x821ed80), writes
`ps.shellshockIndex`, `shellshockTime` (`level.time`) and
`shellshockDuration` (gclient +0x62c..+0x634) and, on an entity with
health above 0, sets `pm_flags` 0x10000 (`PMF_SHELLSHOCKED`) and runs
anim event 0x14 (`ANIM_ET_SHELLSHOCK`). `GScr_StopShellShock`
(0x80c49cc) zeroes the three and clears the flag. VERIFIED (the
disassembly). The event's stock clauses (`playeranim.script`'s
`shellshock` block) play `torso pb_stand_shellshock duration 5000
blendtime 50` moving, idle and crouching and nothing prone; the event's
`BG_AnimScriptEvent` draws `rand()` only for a clause with commands
(0x8050280). VERIFIED (the script, the instructions). vcod runs it at the
end frame that writes the flag (`Cod4Level::shellshock_anim`), its draw
off the level's stream at that point rather than inside the call
(INFERRED: no capture orders it against another draw);
`cod4::shock_tests::a_shellshock_plays_the_torso_shock_anim`.
`ClientThink_real` clears the flag once `shellshockTime +
shellshockDuration` is past `level.time`, and `PM_CmdScaleForWeapon`
takes 0.4 of the walk under the flag when the shock's record sets
`bg_shock_movement`. INFERRED. The records are `shock/<name>.shock` rawfiles of
`bg_shock_*` lines, read over the dvars' defaults
(`BG_LoadShellShockDvars`, `shock/default.shock` for a missing file). `flashbang.shock` sets `bg_shock_movement` 0,
`concussion_grenade_mp.shock` 1. VERIFIED (the rawfiles). The float and
bool defaults are `cod4_lnxded`'s (`tools/re/cod4_dvars.py bg_shock`:
blur 0.4 s over 1 s, flash fades 1 s, kick 0.75 s at 0.05, sound fades
0.25 and 2.5 s, loop fade 1.5 s, loop end delay -3 s, dry 1, wet 0.5,
end delay 2 s, look control on at 90 a second and 0.5 over 2 s,
movement on). VERIFIED. The screen type `blurred`, the four alias
names `shellshock_loop`, `_loop_silent`, `_end`, `_end_abort`, the room
`generic` and the channel volumes' 0.5 are INFERRED, not yet confirmed
on 1.7.

Ours: `G_FlashbangBlast` over the playing clients in slot order (not the
area tree's), the shellshock builtins as above with the flag written at
the end frame, and the 0.4 walk (`vcod_cod4::shock`,
`Rules::shock_slows`). A flashbang 40 units in front of the target's eye
gives it the `flashbang` shock for 6000 ms with the flag, which clears
once the shock runs out; a concussion there gives it
`concussion_grenade_mp` for the distance's time. VERIFIED (the gates
`a_flashbang_in_the_face_shocks_for_six_seconds`,
`a_concussion_grenade_shocks_by_its_distance`,
`a_slowing_shellshock_walks_at_four_tenths`). Live, a scratch script (`live.sh
flash`, not committed: vcod-server `--game cod4 mp_crash`, a
`cod4_probe` `target` on allies at 207 2291 296 and an axis `attacker`
with `--script flash`, the secondary offhand's key 0x8000, from 123 2110
300, both `assault_mp,0`) shocked the target for 5286 ms (configstring
slot 15) from 21150, `pm_flags` 0x10000 until the end and clear after.
VERIFIED live (the probe's per-second lines). No retail capture of a
flashbang exists yet. What the client draws and plays is
cod4-client-view.md 8.

`setNormalHealth` (`ScrCmd_SetNormalHealth`, 0x80c3aa2) on a player
caps its argument at 1, sets the health to `floor(maxHealth * frac +
0.5)` when that is above 0 (else the console's `ERROR: Cannot
setnormalhealth to 0 or below.`, 0x821ea58) and sends the player `J
"0"` (`%c "%i"` with 0x4a). VERIFIED (the disassembly). Without it
`_healthoverlay.gsc`'s regeneration thread ended at its first wound on
ours (a script error, `MissingBuiltin`).

### 10.9 The `rand()` stream

`cod4_lnxded` has one random stream, glibc's `rand()` (PLT 0x804aa5c),
and three `srand`s: `SV_SpawnServer` seeds it with the wall clock's
milliseconds and draws the checksum feed (0x8174afd), then `G_InitGame`
seeds it again with `Sys_MillisecondsRaw()` (0x81d67ac,
`tv_sec * 1000 + tv_usec / 1000` in 32 bits) passed by
`SV_InitGameProgs` (0x817bfad, 0x80b56ee). VERIFIED (the disassembly).
Every draw of the level comes after that second seed.

The callers, VERIFIED by a traced rerun of each capture's recipe (gdb
logging every `rand@plt` with its return address, its caller's and
`svs.time`, under `unshare -rnm`):

| caller | draws | where |
|---|---|---|
| `G_SpawnItem`'s ammo (0x8090072) for each `weapon_*` the map places | `rand() % (max - min + 1)` over `dropAmmoMin..Max` (+1348, +1352), then `rand() % (clip + 1)` when there is a reserve and the clip is not 1; the alt weapon the same when its the shared cap index (+836) is negative | the spawn pass; mp_crash's eight `oldschool_pickup`s make 17 (the `g36c_gl` its alt's one) |
| `SV_GetChallenge` (0x816f394) | `(rand() << 16) ^ rand() ^ svs.time`, a new challenge only | each `getchallenge` |
| `BG_AnimScriptEvent` (0x8050280) | `rand() % numCommands` whenever a clause with commands matched | pmove's events (fire 2, jump 3, land 5, ...), `player_die`'s death |
| `BG_WeaponFireRecoil` (0x80609fc) off the fire event's callback (0x80a47b4, events 26, 27 and 38) | four `rand() / 2^31`: the gun's two kicks and the view's two | after the fire's anim draw |
| `Drop_Item`'s launch (0x80915b3) | three `crandom`s: z by `g_dropUpSpeedRand`, then x and y by `g_dropHorzSpeedRand` | after the death anim's draw |
| `G_InitGrenadeMovement`, `MissileLandAngles` (10.2) | the spin's four, one a bounce | the throw, each bounce |

The order and the counts are the table's: the walk run made 17 at the
spawn, the mate's, the spectator's and the walker's challenges at 17, 19
and 21, the walker's jump and land, then each of its 18 rounds one anim
draw and four recoil draws, the frag's throw anim at 115 and its spin at
116; the kill run 17, the two challenges at 17 and 19, each round five,
the death's draw and the drop's three. The scripts' `randomInt` and
`randomFloat` draw nothing from it (no `rand@plt` call in either run).
VERIFIED (the traces). The recoil draws four on each of its branches,
VERIFIED (the disassembly); that event 38 (a turret's fire) reaches it
rests on the callback's test alone, INFERRED. The name of +836 is the
order's, INFERRED; the draw rule on it is VERIFIED (the
disassembly, the `g36c_gl`'s one alt draw).

The seed is recovered off a capture: the run's `games_mp.log` is created
by `G_InitGame` the same millisecond, and the challenges pin it within a
search (`cod4::wire_ab::recover_game_seed`): the walk run's is
1803421502 (its challenges at draws 19 and 21, `svs.time` 41200 and
42200), the kill run's 1807258603 (17 and 19, 40300 and 41300). The
walk's draw 116 is the frag's spin, `G_flrand(320, 800)` of 0.0492 and
`G_flrand(180, 540)` of 0.2158 against the capture's 296.36 and 102.30
to the bit, and 120..123 its four bounces' turns; the kill's draw 31 is
`player_die`'s, 852869521 % 8 = 1, the clause's `pb_stand_death_headchest_topple`
(36) the capture's corpse plays. VERIFIED (the captures).

vcod keeps the stream on the level (`Cod4HostState::rand`, a
`GlibcRand`), seeded at each load ahead of the spawn pass, and draws it
at every caller above: the pickups' ammo (only the draws: the stock
script deletes the pickups before the baselines), the challenges once a
level is up, pmove's anim events (`Pmove::rand`), the four recoil draws
(the kick itself is the client's), the drop's spreads, the death anim,
the grenades and the melee's `G_rand() % 5`. A server seeds each level
off its own draw; `Cod4Server::pin_game_seed` gives a gate retail's.
On the pinned seeds `the_frag_flies_on_ours_as_on_retail` meets the spin
and every bounce's turn, `cod4_combat_ab` the corpse's `legsAnim`, and
`the_challenges_come_off_the_levels_rand` the spectator's and the
walker's challenge above `svs.time`'s bits. VERIFIED (the gates).

`AngleNormalize360` (0x8190540) is `x = a * 0.0027777778f` on the stack,
its floor stored, `(x - floor) * 360` kept and less 360 at 360 or more;
the bounce off the wall's spin reads it to the bit. VERIFIED (the
disassembly, the gate).

### 10.10 `radiusDamage` and `weaponClass`

- `radiusDamage` (function 0x80bdc12, method 0x80c5e32) both run
  0x80bdab0 with the inflictor in `%eax`: NULL for the function, the
  entity for the method. VERIFIED (the disassembly). It reads origin,
  range, max and min (`Scr_GetVector(0)`, `Scr_GetFloat(1..3)`), then the
  attacker from argument 4 when `Scr_GetNumParam() > 4` and its type is
  not undefined, else the world entity (0x84bbd78); the means of death
  from argument 5, else 14 (`MOD_EXPLOSIVE`); the weapon from argument 6,
  else -1. VERIFIED (the disassembly). vcod's CoD4 level takes the
  attacker and the method's inflictor; the means of death and weapon
  arguments are not read (no stock call passes them). VERIFIED
  (`cod4::gametype_tests::a_bomb_blast_credits_its_attacker`). Trap:
  CoD1's builtin drops every argument past the fourth, so the `sd` and
  `sab` bombs (`visuals[0] radiusDamage(..., player)`) and a destructible
  car's blast (`self.damageOwner`) kill as the world, and the stock
  `Callback_PlayerKilled` (`attacker.classname`, line 4787) errors on
  each such death. VERIFIED (the map sweep, `cod4-map-sweep.md`).
- `weaponClass(name)` (0x80bd81a) pushes `"none"` (0x8209e27) when the
  name's index (0x80a2182, a lookup that returns 0 for a name it lacks)
  is 0, and the class name of the weapon's `weapClass` (offset 0x130)
  otherwise. VERIFIED (the disassembly). So `checkHit("none")`, which a
  blast's `Callback_PlayerDamage` runs, reads `"none"` where vcod raised
  "unknown weapon". VERIFIED (the same test).

## 11. The anim script

The server's step runs 1.7's anim script in pmove
(`pmove::player_step_animated`, cod4-player-anims.md 7): the walker's
body to the spectator matches retail's `legsAnim` and `torsoAnim` on all
789 rows of `cod4::wire_ab`'s walk. VERIFIED (the gate). The hit boxes
pose off those anims.

## 12. Open

- Rockets fly and explode; their flight has no retail capture to gate on.
- The live grenade's pickup and throw back and the use key's weapon swap
  work in play (a hand check); no retail capture gates them.
- The terrain triangle normal's last bit.
- A prone death's torso (5): the corpse run's prone life stood when shot.
- The corpse's drag rides the yaw turn's shape off CoD2; a death anim
  whose root turns has no 1.7 capture yet. No retail capture holds a
  shellshock's torso anim or a killcam with a `killcamentity` (a
  helicopter kill).

## 13. Rounds and blasts on script entities

The stock maps' destructible cars and barrels (`_destructible.gsc`) are
`script_model`s the script opens with `setCanDamage(true)`; they break
on the `damage` notify alone, counting their own health per part.

- `setCanDamage` writes its argument to the entity's `takedamage`
  (`illegal call to setcandamage()` at 0x8216904 names its miss).
  VERIFIED (the string); the write INFERRED (`ScriptEntCmd_SetCanDamage`).
- `G_Damage` on an entity that is no client and takes damage: at least
  1 off `health` (ent+0x1a0), `g_debugDamage`'s `target:%i health:%i
  damage:%i` (0x82199a4, pushed at 0x80b19c2), the `damage` notify with
  nine arguments (`Scr_Notify` at 0x80b18be: damage, attacker,
  direction, point, means of death, model, tag, part, damage flags);
  while health stays above 0 the handler's pain (table 0x826cd54), else
  health held at -999 at worst (0x80b19e2), `death` notified with the
  attacker (0x80b1a0d) and the handler's die (0x826cd58). VERIFIED (the
  instructions). The model, tag and part are the struck attachment's and
  bone's names, empty for the body. INFERRED (the damage notify).
- `G_RadiusDamage` takes every entity with `takedamage` in its box:
  distance to the origin (a brush model's to its bounds), `CanDamage`'s
  five traces to a script model's physics bounds at its origin,
  unturned (middle and four corners across the line), the falloff and
  the flags 5 as on a player. INFERRED (`G_RadiusDamage`, `CanDamage`).
- `CanDamage` (0x80b1f60, called from `G_RadiusDamage` 0x80b2f8c at
  0x80b31ab) scores the two kinds apart. A client (+0x15c set) takes a
  third a trace that gets through, all from three on (0x80b23a4..0x80b23c4);
  any other entity takes the whole damage on the first trace inside the
  cone that gets through and stops tracing there, nothing when none does
  (the five tries from 0x80b2bfa, each clear one jumping to 0x80b2f46,
  which returns 1). VERIFIED (the disassembly).
  `missile::LineScore` holds both (scored as a client, a sedan or barrel
  next to a frag takes only a third a line).
- vcod: a frag's or rocket's blast meets such an entity by its model's
  collision surfaces' bounds (`Cod4HostState::model_coll`), unturned,
  and takes the arm above with empty names
  (`game::cod4::entity_damage`); a round meets its collision triangles
  (13.1). `rounds_shoot_a_backlot_sedan_until_it_blows` holds a player's
  fire on an mp_backlot sedan until it burns and blows. VERIFIED (the
  test).
- The car's explosion is the script's own `self radiusDamage(origin,
  range, max, min, self.damageOwner)` (`_destructible.gsc`
  `destructible_explode`), whose attacker and inflictor 10.10 has; its
  walk damages the cars about it through the same entity arm. VERIFIED
  (the script).
- With rounds stopping at the cars, six-bot `bot_match` (`dm` and
  `war`, mp_backlot and mp_carentan, 6 000 ticks, seeds 1-12 summed)
  killed 1 982 against 2 266 and hit 7 687 rounds against
  8 596, mp_backlot `dm` most (612 to 470), with no script error and
  suicides alike (16 against 17): the bots' sight took no car into
  account and they fired into cars between them and their enemy. With
  the cars in the bots' sight test, 2 255 kills (`bot-measurements-cod2-cod4.md` 6).
  VERIFIED (the counts).

### 13.1 A round through a script entity

- `BG_AdvanceTrace` (0x8060e44) sets the entity the next leg passes to
  `Trace_GetEntityHitId` (0x811a870) of the hit, and only off the world
  (0x3fe) with a distance above 0 moves the start past the surface;
  off any entity the next leg starts at the hit. VERIFIED (the
  instructions). So past a body, a script model or a brush model the
  penetration loop (3.1) traces on with that entity passed, and the
  trace back to the entry passes it too: the entity's thickness takes
  nothing off the round, which goes on as long as the struck surface's
  depth is above 0. INFERRED (`Bullet_FirePenetrate`, the call shape of 3.1).
- A script model's round is `XModelTraceLineAnimated`: each collision
  surface of the model whose bone `hidePart` has not hidden, its
  triangles crossed front to back, the struck surface's flags and its
  bone's name as the hit's `partName`, which `G_Damage`'s notify passes
  as its eighth argument (the model and tag empty for the base model).
  INFERRED (the damage notify).
- The capture (`play-1.7-pen-target.txt`, `-shooter.txt`, `-hits.txt`;
  recipe in the last's header): retail on mp_backlot under the
  `pen_ent` probe gsc, which sets a target 80 units off the right side of
  the first destructible sedan (`vehicle_80s_sedan1_silv_destructible_mp`
  at 368.2 -406.3 68, yaw 259) and the shooter 260 off its left, logs the
  car's `damage` notifies and the target's hits and deals the target no
  damage; the shooter fires its six AK-47 rounds sighted. The first three
  strike the near window (29 each, about 393.5 -412 113.6) and stop; the
  next three strike the far door from inside (29 at about 341.3 -403
  109.4) and hit the target for 28 with `iDFlags` 8. VERIFIED live. The
  window's part breaks on the third (`_destructible.gsc` by the part
  name), so the later rounds pass where it was.
- vcod: `Bullet_Trace` meets the entities open to damage by their
  collision triangles at the bind pose (`DamageBox::trace_surfs`, hidden
  bones skipped) and every solid brush entity through
  `CM_TransformedBoxTrace` (`ClipMap::trace_model`), each with its
  surface flags; a hit passes its entity (`Hit::number`), and a
  script model's notify names the struck bone. On the capture ours gives
  retail's nine lines: the times, damage, flags, locations and means of
  death, the points within 0.02 units and the directions within 1e-4
  (`cod4::shot_aim_ab::rounds_go_through_a_sedan_as_on_retail`; the
  residue is the gsc's placement off `anglesToRight` below the log's
  thousandth). VERIFIED (the gate). Trap: a round stopped at the car's
  bounds names no part in the notify, so the window never breaks on its
  own and no round reaches the target.
- `SP_script_model` (0x809b8e2, the spawn table's `script_model` row at
  rodata 0x821fa18) sets 4 in the byte at +0xf6 (0x809b95d,
  `svFlags |= 4`) and contents 0x2080
  (0x809b964), then links. `notSolid` (0x8099fd2) writes contents 0 for
  a script model or brush model (0x809a053); `solid` (0x809a0ac) writes
  0x2080 for a script model (0x809a186), 1 for a brush model (0x809a136).
  VERIFIED (the instructions). 0x2080 meets the shot mask 0x2806831 at
  0x2000, so `SV_PointTraceToEntity` traces every solid script model by
  its DObj's collision surfaces (`DObjGeomTraceline`, svFlags 4), open
  to damage or not, each model of the DObj with its attachments, and
  The damage notify names the struck attachment's model (its configstring)
  and tag, empty for the base model. INFERRED (`SV_PointTraceToEntity`, the damage notify).
- The script model capture (`play-1.7-models-target.txt`,
  `-shooter.txt`, `-hits.txt`; recipe in the last's header): the
  entity penetration capture's two probes under the `model_shot` gsc,
  which stands a script model of the sedan's own model 150 units off
  the car's left, between them, in five 2.5 s phases. Never opened to
  damage, at the car's height and 30 units up, it stopped every round:
  no line in either phase, where a round past it would have struck the
  car's window. A `tag_origin` model opened to damage with the sedan
  attached at `tag_origin` took the rounds as `damage` notifies naming
  `vehicle_80s_sedan1_silv_destructible_mp`, `tag_origin` and the
  struck bone (`tag_body`, then `tag_door_left_front`), and so did the
  same attached with ignoreCollision: the flag keeps nothing out of a
  round's trace. With no stand-in the rounds went on as the penetration
  capture's. VERIFIED live.
- vcod: `Bullet_Trace` meets every linked script model whose contents
  meet the mask, open to damage or not, and the models `attach`ed to it
  at their tag's bind pose, whatever their ignoreCollision
  (`Cod4Level::shot_boxes`, `ModelPart`); only one open to damage takes
  the damage, its notify naming the attachment. A hidden part counts on
  the base model only. `notSolid` and `solid` write a script model's
  contents. On the capture ours gives retail's lines up to 69400, where
  the target's capture ends (`cod4::shot_aim_ab::
  rounds_stop_on_script_models_as_on_retail`); before, the rounds went
  through the plain stand-ins and struck the car. VERIFIED (the gate).
  The bots' sight test reads the same list (13), and the client's own
  rounds meet the script models too (cod4-effects.md 12). A brush
  model's capture is not taken (the same `Bullet_Trace`, INFERRED).

### 13.2 Glass

`Bullet_FireExtended`'s only glass rule in 1.7 is the contents bit: a
leg that struck contents 0x10 (0x808a8f4) advances 0.135 (0x3e0a3d71,
0x808a901) and the loop goes on, at most 12 legs. VERIFIED (the
instructions). `cod4_lnxded` holds no shatter or breakable glass code
(its strings name glass only as a surface type and a bounce key), and
the loop runs only with `bullet_penetrationEnabled` 0 or a weapon of no
`penetrateType`. VERIFIED (the strings). vcod's `fire_extended` keeps
the same rule; nothing more is open.

## 14. Entity numbers

Read off the `cod4_lnxded` 1.7 disassembly, then checked against the kill
capture.

- `G_Spawn` (0x80cbbe8) takes the free list's head (`level.firstFreeEnt`,
  0x836f6d0) when it was freed more than 499 ms ago (`level.time`,
  0x836f8ac, less the entity's `freetime` at +0x184) or
  `level.num_entities` (0x836f6cc) is past 1021; it unhooks it (`nextFree`
  at +0x270, `lastFreeEnt` 0x836f6d4 cleared with an empty list). Else it
  hands out `level.num_entities` and counts it up, `G_Error` at 1022.
  VERIFIED. That is CoD2's rule (cod2-protocol.md 15.1).
- `G_FreeEntity` (0x80cc040) clears the entity (`memset` 0x274 bytes),
  stamps `freetime` with `level.time` and appends it to the list's tail
  only past byte offset 0xb09f of `g_entities`, so numbers 0..71 (the
  clients and the body queue) never go on it. VERIFIED.
- `delete()` (`Scr_Delete`, 0x80c3cd6) refuses a client (`Cannot delete a
  client entity`, 0x821c478) and the entity whose think is running
  (`Cannot delete entity during its think`, 0x821ea88, against
  0x83724b0), notifies `death` (0x80c7604) and tail-calls `G_FreeEntity`:
  the free is inside the call, not the 100 ms think CoD1's `delete` arms
  (cod11-gsc-language.md, "`delete()` defers the free"). VERIFIED.
- So the free list holds the level's deletes in the order the scripts ran
  them: on mp_crash `war`, `_oldschool::deletePickups` (314..323, 368,
  369) ahead of `_gameobjects::main`'s other gametypes' objects (80,
  99..365). INFERRED (the script order; the numbers below agree).
- vcod deferred the free, and its think pass freed them all at 100 ms by
  number, 80 first: the kill's first temp entities took 99..104 where
  retail's took 315..320. vcod still frees the entity 100 ms on, so the
  `death` waiters run on a live `self` (`_hardpoints::delete_on_death`
  stops a jet's loop that way), but CoD4's `delete()` puts the number on
  the list's tail inside the call, stamped then
  (`HostProfile::delete_queues_number`, `ObjectTable::queue_number`); a
  number whose entity has not gone yet is not handed out. With that every
  entity number `cod4_combat_ab` compares matches retail's: the flesh
  events 316 and 318, the obituary 319, the dropped rifle 320 and the
  eType 58 events 315 and 317. VERIFIED (the gate).
- What a script sees of a deleted entity inside the frame is not
  measured on 1.7. `Scr_FreeEntityNum` likely only marks the object
  dead and leaves its threads for later, INFERRED; vcod keeps
  CoD1's 100 ms instead.
- The two refusals are not ported: no stock script deletes a client or an
  entity from its own think.
