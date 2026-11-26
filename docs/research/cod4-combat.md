# CoD4 1.7 combat on vcod-server: the weapon half, shots, damage, death

What `vcod-server --game cod4` does from a fire key to a respawn, and how
close it replays retail's kill capture. Code: pmove's weapon half
`vcod_cod4::pmove::weapon`; shots, hit poses, damage feedback, items,
melee and temp entities `game::cod4::combat`; grenades
`game::cod4::missile`; the death path's builtins `builtins::cod4_combat`.

Sources (each claim names its own):

- Kill capture (`play-1.7-kill-target.txt`, `-shooter.txt`,
  cod4-netfields.md 10): two `cod4_probe` clients on `cod4_lnxded` (md5
  `bf8448740b3f4cb74c2254f01af14cdd`), `+devmap mp_crash`, `war`,
  `scr_player_maxhealth 30`. VERIFIED live.
- The 1.7 zones `common_mp.ff`, `mp_crash.ff` and `common_mp`'s stock gsc
  (`_globallogic.gsc`, `_killcam.gsc`, `_weapons.gsc`,
  `_callbacksetup.gsc`). VERIFIED.
- `tools/re/cod4_dvars.py` over `cod4_lnxded`: dvar defaults. VERIFIED.
- The walk capture's frag throw (`play-1.7-walk.txt`, cod4-netfields.md 8)
  and the mantle capture (`play-1.7-mantle.txt`, cod4-pmove.md 17).
  VERIFIED live.
- Fire rate captures (`play-1.7-firerate.txt`, `-firerate-rof.txt`, recipe
  in their headers): one `cod4_probe --script auto` under the `fire_rate`
  gsc (`crates/cod4/tests/fixtures/gsc/fire_rate`), an AK-47 held plain
  and under Double Tap. VERIFIED live.
- Corpse run (section 5): 24 kills of a `cod4_probe --script poses` target
  on `cod4_lnxded` under gdb, its `torsoAnim` watched. VERIFIED live.

An INFERRED claim naming no 1.7 address is unconfirmed on 1.7.

## 1. The `WeaponDef` offsets

The weapon half reads 0x878-byte `WeaponDef`s from the zones at the
offsets the struct's field order gives, anchored by the string slots
(cod4-gsc.md 7). INFERRED, each offset. `m16_mp` reads the stock file's
values there (VERIFIED, `cargo run -p vcod-cod4 --example weapon_dump --
common_mp.ff m16_mp`): fire type 3 (three-round burst), clip 30,
`fireTime` 64, reload 2000, empty reload 2365, `reloadAddTime` 1500, raise
349, `adsTransInTime` 250 (inverse sight time 0.004), damage 40, min
damage 30 past 2000 units, head and neck multipliers 1.4.

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

`vcod_cod4::pmove::weapon` runs with the level's defs
(`player_step_armed`): `PM_AdjustAimSpreadScale` after the clock,
`PM_UpdateAimDownSightFlag` before the sprint on every arm, `PM_Weapon`
(sight lerp, breath, timers, sprint, change and reload checks, rechamber,
fire) after the foliage on the default arm. INFERRED (order and each
function). The shooter reads states 5 firing and 7 reloading, `weapAnim` 2
and 0x202 toggling per shot, `weaponShotCount` 1..3 over a burst. VERIFIED
live.

- A burst's end holds the gun ready for `player_burstFireCooldown`'s 0.2 s
  (registered 0.2, VERIFIED): `weaponTime` after the third round reads
  0xb8, 0xc8, counting to 0; the gate fails at 0.25. VERIFIED (the gate).
  `weapFlags` 0x100 is the fresh trigger pull. VERIFIED live.
- A burst whose clip runs dry with no reserve stays mid-burst, on 1.7 as in
  vcod: the gun clicks every 500 ms and takes no sprint, offhand, change,
  reload or melee request until ammo for it, a use-key swap or death.
  INFERRED (the branches): `PM_Weapon` tests `weaponShotCount` (ps+0xf0)
  against `fireType` 2..4 at 0x80630a4..0x80630c0 and, mid-burst, jumps
  past the requests to 0x8063574; the trigger test at
  0x8063802..0x8063827 fires mid-burst with no key; the shot's empty clip
  (0x806409c) with no reserve raises `EV_NOAMMO` (0x80649c1) and adds 500
  to `weaponTime` (0x8064a1e) without touching the count; the time-out
  skips its reset mid-burst (0x806454b..0x806460f).
- The only stores to ps+0xf0 in 0x8050000..0x8070000 are 0x80612d4 (the
  reload start, zero), 0x80641a3 (the shot's increment), 0x806460f (the
  time-out, zero) and 0x8065106 (the held-trigger reload arm, zero).
  VERIFIED (the disassembly).
- An empty clip with reserve reloads: 58900 reads `weaponstate` 7,
  `weaponTime` 2365, `weaponDelay` 1500 (empty reload, add time), with
  `EV_RELOAD_START_NOTIFY` (19), `EV_RESET_ADS` (14),
  `EV_RELOAD_FROM_EMPTY` (16) on the ring. VERIFIED live.
- A segmented reload's start yields to the trigger edge only once more
  than 0.4 of `reloadStartTime` has run: `PM_Weapon_CheckForReload`
  (0x806148e) compares `(start - weaponTime) / start` with 0.4 (float at
  0x82095bc) at 0x8061682, strictly greater, and stores state 10. VERIFIED
  (the disassembly). No capture has one; `pmove::tests::weapon_pins` pins
  vcod's (`pmove::weapon::START_CUT_FRACTION`), exactly 0.4 included.
- `PM_BeginWeaponChange` is 0x805fd22 (events 23, 24),
  `PM_SetReloadingState` 0x805fb7a (events 15, 16; called from `PM_Weapon`
  at 0x8063bbe). INFERRED (the events each raises).
- `EF_FIRING` (0x40) follows the attack key in a ready or firing state
  with a round in the clip. VERIFIED (the gate's `eFlags` rows).
- The breath holds on key 0x2000 on a sighted scope for
  `player_breath_hold_time`, plus `perk_extraBreath` (5, VERIFIED) under
  perk 0x10, with `player_breath_hold_lerp` (1, VERIFIED). INFERRED; no
  capture holds it. `pmove::tests::weapon_pins::hands` pins vcod's hold,
  gasp and recovery, plus the sight's raise and blocks, hip spread,
  offhand throws and melee swings no capture covers.
- Offhand (15..20) and melee (12..14, with its charge): section 10. Not
  run: detonation, night vision.
- A mantle on a row whose over clip is not the low one puts the weapon
  away as the ladder does (`Mantle_IsWeaponInactive` at both of
  `PM_Weapon`'s calls); the first change after it is quick
  (`mantleState.flags` 0x10). INFERRED. The empty hand's aim spread drops
  a whole 255 a step: the mantle capture's `aimSpreadScale` reads 0 at
  48850, the cmd after its weapon went to 0. VERIFIED live.
  `cod4_moves_ab`'s `mantle` rows replay armed, every weapon word exact.
  VERIFIED (the gate).

`cod4_moves_ab`'s `shooter` rows replay the shooter's cmds from the first
snapshot after its `setviewpos` (47550) with the weapon half: 275 rows,
792 cmds, every pmove word (cod4-pmove.md 13) and weapon word (`weapon`,
`weaponstate`, `weaponTime`, `weaponDelay`, `weaponShotCount`, `weapAnim`,
`weapFlags`, `fWeaponPosFrac`, `aimSpreadScale`, `weaponRestrictKickTime`,
event ring, `ammo`, `ammoclip`, `eFlags` 0x40) exact on all 275, free and
resynced. VERIFIED (the gate).

### 2.1 The fire clock

From `PM_Weapon` (0x8062df0, `PM_Weapon_WeaponTimeAdjust` inlined) and
the shot's start (`PM_Weapon_StartFiring`, inlined at 0x80640af).

- One step comes off `weaponTime` (ps+0x3c) and `weaponDelay` (ps+0x40):
  `pml.msec` (0x8063054) or a perk's quotient. VERIFIED (the disassembly).
  The quotient is `msec / perk_weapReloadMultiplier` (dvar 0x828d068, read
  at 0x80636d8) in a reload state (7..11) under perk 0x4, `msec /
  perk_weapRateMultiplier` (0x828d06c, read at 0x8063933) in state 5 or 6
  under perk 0x8. INFERRED (branches at 0x8063009..0x806304e, 0x8063926).
  Either rounds half up: `fadd` 0.5 (0x8209628 = 0x3f000000), `frndint`
  under round-down, truncating `fistp` (0x80645c8..0x80645ed,
  0x80636f0..0x806372b). VERIFIED (the disassembly). A multiplier of 0
  steps by the larger of `weaponTime` and `weaponDelay`
  (0x806394f..0x8063963; the reload dvar's test at 0x80636dd..0x80636ea,
  the rate dvar's at 0x8063938..0x8063949, a NaN taking the quotient).
  VERIFIED (the instructions). vcod: `pmove::weapon::run_clocks`,
  `clock_step`, `perk_msec`, the multipliers, `player_burstFireCooldown`
  and `bg_aimSpreadMoveSpeedThreshold` off `Rules` (the client's off the
  server's dvar mirror when it carries them).
- A timer stepped to 0 or below is set to 0 (`weaponTime` 0x8064592,
  `weaponDelay` 0x80645b5): the overshoot is dropped, never carried.
  INFERRED (the branches into both stores).
- A shot stores (not adds) `fireTime` (+0x36c) into `weaponTime` at
  0x80640be and `fireDelay` (+0x35c) into `weaponDelay` at 0x80640b5.
  VERIFIED (the disassembly).
- So a held trigger fires every `ceil(fireTime / step)` cmds. AK-47
  `fireTime` 85 (`ak47_mp`, VERIFIED, `--example weapon_dump`): at 16 ms
  cmds, six cmds a round plain (about 100 ms), four under Double Tap
  (steps 21..24, about 67 ms): 1.5 times the rounds, where the multiplier
  says 1.33. Retail fired 20 rounds in the first 2 s held plain, 30 under
  Double Tap; the next 2 s fired the clip's last 10 plain, 12 under the
  perk after the empty reload. VERIFIED live (each snapshot ring's fire
  events).
- `cod4_moves_ab`'s `autofire` and `doubletap` rows replay both captures
  armed: 306 rows each (913 and 911 cmds), every word exact, free and
  resynced. `cod4_fire_rate_ab` replays their cmds free and fires
  retail's rounds in each burst. VERIFIED (the gates).

### 2.2 The reload's delayed action

`cod4_lnxded` 0x80624a6 (`PM_Weapon_ReloadDelayedAction` by its shape,
INFERRED) runs when a reload's `weaponDelay` runs out.

- A bolt-action weapon (`WeaponDef` +0x4e8) with its `weaponrechamber`
  bit set (ps+0x57c) clears the bit and adds `EV_EJECT_BRASS` (0x1d) at
  0x8062572..0x8062586. In a reload start (state 9 or 10) whose
  `reloadStartAddTime` (+0x39c) is 0 it returns there (0x806258b,
  0x806270c..0x8062714): no rounds. VERIFIED (the instructions). CoD2's
  copy does the same.
- Else, with `weaponTime` running, the add moment (a start's
  `min(reloadStartAddTime, reloadStartTime)`, a segment's
  `reloadTime` or, from an empty bullet clip, `reloadEmptyTime`, capped
  by `reloadAddTime`) less `rechamberBoltTime` (+0x374, 1 when the moment
  is no longer) goes into `weaponDelay` when positive
  (0x80625e8..0x8062602). VERIFIED (the instructions).
- Otherwise the rounds go in: a start's `reloadStartAdd` (+0x538),
  nothing when 0, or a segment's `reloadAmmoAdd`, capped by the clip's
  room and the reserve, with `EV_RELOAD_ADDAMMO` (0x14) when any move
  (0x80624e4..0x806255d, 0x806260a..0x806265d). VERIFIED (the
  instructions). vcod: `pmove::weapon::reload_delay_out`.

### 2.3 The hip spread's step

`PM_AdjustAimSpreadScale` is `cod4_lnxded` 0x8060688 (`pm`, `pml`;
`ps.aimSpreadScale` at ps+0x628). INFERRED (the name). Its arithmetic,
VERIFIED (the instructions 0x8060688..0x80609dc):

- The rate is the def's `hipSpreadDecayRate` (+0x468); 0 drops the scale
  by 255 (-255 at 0x820e268). On the ground (`groundEntityNum` not 0x3ff,
  or `pm_type` 1) a prone body (`eFlags` 8) multiplies it by
  `hipSpreadProneDecay` (+0x47c), a crouched one (4) by
  `hipSpreadDuckedDecay` (+0x478); in the air it halves (0.5 at
  0x8209628). The product stays on the stack.
- Not under the override: `rate * frametime` is stored as a float
  (0x8060715..0x8060718) before it is taken 255 times off the scale.
- The growth, unless `fWeaponPosFrac` is 1: for pitch then yaw, the cmd's
  and the old cmd's angle ints (pm+0xc, +0x2c; +0x10, +0x30) times
  360/65536 (0x820deb0), their difference times the float 1/360
  (0x8209630), less `floor` of it plus 0.5 under round-down (stored as a
  double, then a float), times 360, `fabs`, stored as a float. The yaw's
  turns are stored as a float (0x80608c4) before the subtraction, the
  pitch's stay on the stack. Each turn times 0.01 (0x8209644), times
  `hipSpreadTurnAdd` (+0x470), over `frametime`. A move key down
  (pm+0x1a, both bytes) with `hipSpreadMoveAdd` (+0x474) set adds it times
  `fsqrt` of `vx^2 + vy^2` on the stack over `ps.speed` (`fidivl`), when
  that sum passes the dvar at 0x827afbc squared. The air adds 1.28
  (0x820e26c) twice. Times `frametime`, when above 0 it opens the scale
  by 255 times it; else the decay above runs.
- The override (`spreadOverrideState` 1, ps+0x100) divides the decay by
  the override's place in the stance's min..max, both kept, and clears
  the state once the scale times that place falls under 255, storing
  that product. The clamp to 0..255 comes last.

vcod: `pmove::weapon::adjust_aim_spread` on `x87::F80`. It had kept each
sum in a double, the stance's rate stored as a float, the flat speed
squared stored and its root a float's, which put the swaying target's
`aimSpreadScale` one or two ulps off retail's on eleven snapshots of
`cod4_trace_read_ab`; ported, it matches every snapshot. VERIFIED (the
gate). Every weapon the capture gates fire has `hipSpreadTurnAdd` 0,
so the turn's arm rests on the instructions alone, and that the cmd's
angle ints run 0..65535, not signed shorts, is INFERRED (the wire's 16
bits read unsigned). The override arm runs on doubles; no capture holds
it.

## 3. The shot

INFERRED: `ClientEvents` after each cmd reads its events; a fire event
runs `FireWeapon` at the cmd's time under `g_antilag` (1, VERIFIED), else
at `level.time`. `FireWeapon` takes the spread between
`BG_GetSpreadForWeapon`'s ends by `currentAimSpreadScale` (end frame's
`aimSpreadScale / 255`), the ADS spread when fully sighted. `Bullet_Fire`
rewinds bodies (only for a shot 50..400 ms old, through CoD2's
`SV_GetClientPositionAtTime` walk over the position archive), draws the
end off `Bullet_RandomDir(time)` (`G_GoodRandomFloat`'s shuffle) 8192 out
(a spread weapon: `shotCount` rounds to `minDamageRange`), traces the clip
map under 0x2806831, then each other playing body's hit boxes. Eye and
angles: 3.2. Script models and brush models: 13.1. Penetration: 3.1.

Hit boxes are each xmodel's `XBoneInfo` bounds (0x28 per bone at
`XModel`+164: `bounds[2][3]`, `offset`, `radiusSquared`), the hit location
its `partClassification` byte. INFERRED. `body_mp_usmc_assault`'s boxes
sit on the hit bones (head class 2 on `head`, torso 4 on `j_clavicle_*`,
`pelvis` 5, limbs 6..17), none on a tag. VERIFIED (`cargo run -p
vcod-cod4 --example bone_dump -- mp_crash.ff body_mp_usmc_assault`). The
trace walks them under `bulletPriorityMap` or
`riflePriorityMap` by `rifleBullet`: `.data` 0x826ce60 and 0x826ce73,
loaded at 0x8089f4c and 0x8089f3f. VERIFIED (the bytes and the loads).
Unlike CoD1's walk, `DObjTraceline` (0x81acb18) clips a box of higher
priority than the best so far to the trace's own fraction on entry
(`ebp-0x224`, 0x81acc57, read at 0x81ad293), not the best's (taken
only for an equal priority, 0x81ad7f6), and takes it on any contact
(0x81ad8a1..0x81ad8f1); an equal priority takes it only nearer
(0x81ad8fc..0x81ad907). VERIFIED (the instructions). So the highest
priority crossed wins in any bone order: a corpse round that crossed a
`back_mid` (torso lower, 7 under the rifle map) before the nearer
`j_clavicle_le` (torso upper, 8) scored the clavicle's face, word for
word. VERIFIED (`cod4_corpse_rounds_ab`, 5.3). Ours:
`vcod_common::bonetrace::bone_trace_worlds_cod4`.
Each body is posed off the player animtree by `vcod_cod2::player`'s body
(cod4-player-anims.md) at the playerstate's `legsAnim` / `torsoAnim`
(the anim script's), the stance's idle when `legsAnim` is 0. INFERRED
(the code).

### 3.1 Penetration

`game::cod4::bullet`. `cod4_lnxded` 1.7 gives the call shape and
constants; the algebra between them is INFERRED.

- Constants (immediates): 12 legs (0x808a893) and 5 (0x808acfc), advance
  0.135 (0x3e0a3d71), `perk_bulletPenetrationMultiplier`'s dvar pointer
  0x828ce74 read at 0x808aa1d and 0x808aeb8. VERIFIED.
- 1.7 inlines `Bullet_FirePenetrate` and `Bullet_FireExtended` into one
  function calling, in order, `Bullet_Trace` (0x8089f14), `Bullet_Process`
  (0x808a136), `Bullet_ImpactEffect` (0x808a05c),
  `BG_GetSurfacePenetrationDepth` (0x805f2c0), `BG_AdvanceTrace`
  (0x8060e44), with the multiplier under `perks` 0x20 (0x808aa10,
  0x808abe9) on the entry's and exit's depth. INFERRED (call shape and
  algebra).
- Dvars: `bullet_penetrationEnabled` 1, `perk_bulletPenetrationMultiplier`
  2, `bullet_penetrationMinFxDist` 30, `sv_clientSideBullets` 1
  (`tools/re/cod4_dvars.py`). VERIFIED. With client-side bullets the
  server skips `Bullet_ImpactEffect`'s temp entity, so ours sends none and
  the client draws its own. INFERRED.
- Depths: `info/bullet_penetration_mp` in `common_mp`, a
  `BULLET_PEN_TABLE` info string of `<type>_<surface>` keys for `small`,
  `medium`, `large` over 28 named surface types (`small_bark` 20 ..
  `large_paintedmetal` 0; concrete 6/12/12, metal 12/20/20, flesh
  24/34/40). VERIFIED (the rawfile). Type 0 (`none`) and surface 0 read 0.
  `penetrateType` (`WeaponDef`+308): pistols, SMGs, shotguns small; rifles
  and the Desert Eagle medium; snipers large; grenades and launchers 0.
  VERIFIED (`weapon_dump` over `common_mp`).
- INFERRED: a body hit reads surface flags 0x700000 (flesh, 7); a surface
  with 0x100 has depth type 0 (stops the round); type 0 takes the last
  surface's. A round starts at multiplier 1. Past a surface the next leg
  starts 0.135 along the round over the cosine to the face (a graze under
  0.125 ends it); the next contact is traced, then a trace back from it
  (or the round's end) to 0.01 short of the entry finds the exit. The
  thickness (at least 1) over the lesser of entry and exit depth comes
  off the multiplier; at 0 the round ends. A body hit past the first
  surface takes `Bullet_GetDamage` (range falloff off the round's start)
  times the multiplier, truncated, with `iDFLAGS_PENETRATION` (8,
  `_callbacksetup.gsc`, VERIFIED).
- Wall capture (`play-1.7-wall-target.txt`): `cod4_probe`, `+devmap
  mp_crash` `war`, a target (`--script idle --goto "207 2380 296"`,
  allies) outside the room's north wall, a shooter (`--script shoot --goto
  "207 2250 296"`, axis) inside, both assault class (M16, Stopping Power).
  The wall is a metal plate (surface 13) from y 2331.875 to 2336.125. Each
  of the first two rounds took 43 (100, 57, 14). VERIFIED live. Ours: `1 -
  4.27 / 20` of 40 is 31, Stopping Power's 140 per cent makes 43; Deep
  Impact's doubled depth gives 35 without it
  (`perk_tests::a_round_goes_through_a_wall_and_deep_impact_goes_further`).
- The first leg eases each body's hit pose; later legs trace the same
  poses without stepping them, so penetration moves no pose a plain round
  would not. INFERRED (ours: a body's controls ease at most once a frame,
  3.3).

### 3.2 The eye and the gun's angles

`vcod_cod4::pmove::aim`; the server keeps it per slot (`Combat4::aim`,
`gun_angles`) and runs it in `Cod4Level::think`.

- `CalcMuzzlePoints` is inlined twice (0x80a2b81, 0x80a30a7): the view
  angles with pitch and yaw replaced by gclient +0x30a4, +0x30a8 (gun
  pitch, yaw), through `AngleVectors` (0x818ea3a), then
  `G_GetPlayerViewOrigin` (0x80a7778). VERIFIED (the instructions). The two
  sites are `FireWeapon` and `G_UseOffHand`. INFERRED (the callers'
  shape). So bullets, throws, launched grenades and rockets all leave
  along the gun's angles, as ours does (`Cod4Level::muzzle_axis`).
- `G_GetPlayerViewOrigin` off a turret calls `BG_GetPlayerViewOrigin`
  (0x805399c) at `level.time` (0x836f8ac): origin plus
  `viewHeightCurrent` (ps+0x118); plus `BG_GetVerticalBobFactor`
  (0x805fac4) on z; plus `BG_GetHorizontalBobFactor` (0x805fa4c) along the
  view's `AngleVectors` right; both at `BG_GetBobCycle` (0x805f8ca),
  `BG_GetSpeed` (0x8056c38), `bg_bobMax` (8); then `AddLeanToPosition`
  (0x81a9f8e) with 16 (0x41800000) and 20 (0x41a00000); z held at least 8
  (rodata 0x820aac4) over the feet. VERIFIED (calls and immediates).
- INFERRED, unconfirmed instruction by instruction: `AddLeanToPosition`
  takes `GetLeanFraction` (0x81a91e8, `(2 - |f|) f`) of `leanf`, the right
  of (0, yaw, 16 times it), 20 times it along that. The bob factors read
  the `bg_bobAmplitude*` vector dvars by `viewHeightTarget` 11, 40 and the
  sprint bit (ps+0xc as a negative short): vertical `(sinf(2c) + 0.2
  sin(4c + pi/2)) * 0.75` of the second component, horizontal `sinf(c)`
  of the first, each amplitude capped. The constants 0.2, 0.75, 4, pi/2
  are VERIFIED (the rodata). The amplitude values are the client's
  (cod4-client-view.md 4). INFERRED (constant configstrings: the gamestate
  carries no value to compare).
- `ClientThink_real` (0x80a5c1a), before `Pmove` and past the inactivity
  check, on the cmd's ms (`serverTime - commandTime`, at most 200).
  VERIFIED (the calls and stores):
  - `BG_CalculateViewAngles` (0x806523e) into offsets added to
    `ps.viewangles`;
  - `BG_CalculateWeaponPosition_Sway` (0x8060f20) on +0x3118 (last view),
    +0x3124 (offset), +0x3130 (angles) at scale 1.0 (0x3f800000 pushed at
    0x80a5f49: 1.7 has no shellshock sway scale);
  - `BG_CalculateWeaponAngles` (0x80659d0) on a copy of +0x313c..+0x3164,
    written back after;
  - with `ps.weapon` `aimDownSight` (`WeaponDef` +0x4ec, test at
    0x805f4d2), `fWeaponPosFrac` (ps+0xf4) non-zero and no
    `overlayReticle` (+0x438), the angles are
    `AxisToAngles(AnglesToAxis(weapon) * AnglesToAxis(view + offsets))`
    (0x818e910, 0x818effc, 0x81951fe), else view plus offsets; pitch and
    yaw stored at 0x80a6159.
  - The view is the last cmd's: `Pmove` runs after (0x80a6171). VERIFIED
    (the call order).
- INFERRED, unconfirmed instruction by instruction:
  - `BG_CalculateViewAngles`: damage kick; an overlay weapon's idle drift
    and walk bob scaled by the sight; the sighted view bob
    (`fWeaponPosFrac * adsViewBobMult` (+0x44c) of the bob factors at the
    flat speed, capped at 45, off a turret).
  - `BG_CalculateWeaponAngles`: lean roll (-2 times the lean fraction);
    `frac * adsAimPitch` (+0x6cc); movement tilt (the stance's `*Rot`
    (+0x240, +0x264, +0x288) past its `*RotMinSpeed` (+0x2b0..), eased at
    `posRotRate` (+0x2a8, prone +0x2ac), at least 0.1/s); idle drift
    (`hipIdleAmount` or 80, blended to `adsIdleAmount` by the sight
    (+0x48c..+0x498), stance factor (+0x49c, +0x4a0) eased 0.5/s; sines
    of the clock at 0.0005, 0.0007, 0.001 times a hundredth); walk bob (a
    quarter turn on, 0.16 of the speed, capped at 10, `adsBobFactor`
    +0x448 up the sight); damage kick; on an `aimDownSight` weapon the gun
    spring (0.005 s slices, `gunMaxPitch`/`Yaw` +0x4a4, springs +0x6f0,
    +0x738); less the sway's angles.
  - Sway: the turn since the last cmd per 60th of a second, clamped to
    `swayMaxAngle`, `DiffTrack`ed at `swayLerpSpeed` (hip +0x4ac.., ads
    +0x4c8..; none for a sighted overlay weapon).
  - The sway capture below holds this to retail's rounds. VERIFIED (the
    gate).
- `BG_WeaponFireRecoil` (0x80609fc) draws four `G_random`s (0x8191e86,
  `rand()` times 2^-31): view kick pitch and yaw, then the gun's, added to
  the spring speeds (+0x3158). Ranges: the sight's (+0x700.. view, +0x6e0..
  gun) at full sight, the hip's (+0x748.., +0x728..) otherwise, the gun's
  the sight's at any sight above 0; under `weaponRestrictKickTime`
  (ps+0x50) scaled by a hundredth of `*GunKickReducedKickPercent`
  (+0x6dc, +0x724). INFERRED, unconfirmed instruction by instruction.
- Damage kick, `P_DamageFeedback` (0x80a5156, from the end frame), on a
  living player (`pm_type` <= 6) with blood (gclient +0x30ac) and max
  health (+0x2fe8). VERIFIED (the instructions; dvar defaults by
  `tools/re/cod4_dvars.py`):
  - `count = 100 * blood / maxHealth`, at most 127, into `aimSpreadScale`
    (ps+0x628, at most 255);
  - `kick = count * bg_viewKickScale` (0x8277498, 0.2) raised to
    `bg_viewKickMin` (0x82774a0, 5), cut to `bg_viewKickMax` (0x827749c,
    90);
  - with `damage_fromWorld` (+0x30bc): roll (+0x3110) 0, pitch (+0x3114)
    `-kick`, both direction bytes 255; else `vectoangles` of
    `damage_from` (+0x30b0) into the bytes and, against `AnglesToAxis` of
    `ps.viewangles`, roll `-kick` times its dot with the left, pitch
    `kick` times its dot with the forward;
  - `damageEvent` +1, `damageTime` (+0x310c) `level.time - 20`
    (0x80a5256), `damageCount` the count, blood cleared; a `damageTime`
    over 500 ms back clears `damageCount` first (0x80a517c).
  - `ClientThink_real` hands `damageTime`, `level.time`, +0x3114, +0x3110
    to both blocks (0x80a5e6a..0x80a5e85, 0x80a5fef..0x80a6007).
  - Trap: the bytes are `vectoangles` in 256ths of a turn, truncated, so a
    real direction whose yaw and pitch both lie in 358.6..360 (a blow along
    yaw -1.4..0 from up to 1.4 degrees below) packs as 255 both, the
    no-direction value. INFERRED, the truncation above. vcod's CoD4 bots
    read the gathered direction (`Cod4Level::hit_dir`) instead of the
    bytes, as its CoD2 bots do (cod2-combat.md 9.2).
- Easing the kick, INFERRED, unconfirmed instruction by instruction; the
  constants VERIFIED (rodata at the addresses given).
  `BG_CalculateViewAngles` opens with the view kick: with a `damageTime`,
  factor `-0.5 frac + 1` (0x820aab4), times `frac * 0.5 + 1` (0x8209628)
  under an overlay; under 100 ms (0x820df10) `GetLeanFraction(t / 100)`,
  then `1 - GetLeanFraction(1 - r)` for `r = (t - 100) / -400 + 1`
  (0x820aab0) while `r > 0`; pitch and roll each add it times the factor
  and their kick. `BG_CalculateWeaponAngles` adds the gun's after the walk
  bob, before the spring: `k = 0.5 frac + 0.5`, in over `100 k` ms, out
  over `400 k` (0x820dea8), scaled by `k`, times `-0.75 frac + 1`
  (0x820aad4, 0x820a318) under an overlay; pitch plus half the kick's
  pitch, yaw less the roll, roll plus half the roll. `ClientSpawn`'s
  memset zeroes the kick with the rest of the aim state. INFERRED. vcod:
  `AimState::kick` (`vcod_cod4::pmove::aim::DamageKick`).
- Kick capture (`play-1.7-kick-target.txt`, `-shooter.txt`, `-hits.txt`;
  recipe in the last's header): the sway capture's two probes under the
  `shot_kick` gsc, which from 10 s past the shooter's spawn hurts him every
  1.3 s with `finishPlayerDamage` (flags 4, no knockback), in turn 40
  along his view's forward, 25 along its right, 40 with no direction, and
  heals him at once. 23 rounds hit, kicked ones among them (62050, 50 ms
  past a forward hurt, struck the right lower leg). VERIFIED live. Ours
  puts all 23 along retail's direction within a ten-thousandth of a
  degree and through its line within 0.001 units
  (`cod4::shot_aim_ab::hurt_rounds_leave_along_retails_damage_kick`);
  without the kick, 7.2 degrees off, two hits missed and four others
  struck. VERIFIED (the gate).
- Sway capture (`play-1.7-sway-target.txt`, `-shooter.txt`, `-hits.txt`;
  recipe in the last's header): the `shot_aim` gsc's damage callback logs
  each round's point and direction and does no damage; `cod4_probe
  --script sway` fires an AK-47 at an idle target 200 units off: sighted
  while strafing, leaning both ways and swinging the view, from the hip
  strafing, and crouched. 38 rounds hit. VERIFIED live. Along the bare
  view from the plain eye ours were up to 2.16 degrees and 7.98 units off
  (hip strafe worst, view a cmd late), three missed; with the block all 38
  leave within a ten-thousandth of a degree (the log's millionths) and
  0.001 units (`cod4::shot_aim_ab`). VERIFIED (the gate).
- The client's own rounds (client-side bullets) leave from the same eye
  and angles: the predictor runs the aim block on each kept cmd's
  playerstate before its step, on the view the last kept cmd left (a
  snapshot's `viewangles` is not the server's to the bit), and hands each
  fire event its cmd's eye and angles (`play::cod4_predict::ShotAim`).
  1.7's cgame fires along `cg.gunPitch`/`gunYaw`, set by
  `CalculateWeaponPosition_SaveOffsetAngles` in the last frame's view
  weapon (the view a frame back, the gun's axis up a sight). INFERRED. The
  predictor's cmd stands in for that frame, and its spring takes no kick:
  the server's draws come off the level's `rand()` (10.9), which no client
  can draw alike, and 1.7's cgame draws its own (`BG_WeaponFireRecoil` in
  `CG_FireWeapon`), so a sighted burst's spring parts the sides on retail
  too. INFERRED (the two streams). On vcod-server an M16's sighted bursts
  stay within 0.05 degrees of the server's rounds without it, the view
  kick in the cmds' angles carrying the rest.
  `cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds` and its
  sighted twin hold each own round within 0.05 degrees. VERIFIED (the
  tests).
- The predictor's aim state counts from the first cmd it predicts, the
  server's from `ClientSpawn`: the idle clock (+0x3164) and factor
  (+0x3148) agree only when the client predicts every frame from the
  spawn, as its frame does. A session first drawn at the burst read idle
  clock 14 and factor 0.009 against the server's 4687 and 1; its sighted
  rounds left 0.02 to 0.04 degrees off by the drift alone, 0.051 with the
  spring kick on a burst's second round. VERIFIED (both sides' aim state
  logged per cmd in the live test).
- The predictor's damage kick is the server's, rebuilt off the snapshot
  that first steps `damageEvent`: its `damageCount`, view and direction
  bytes read back in 256ths as the server packed them (a 1.4 degree
  step), timed at the snapshot's time less 20. The server runs every cmd
  of a packet at the frame its newest cmd falls in (13382, 13400, 13416 at
  13400; 13432, 13450, 13466 at 13450; VERIFIED on ours); the predictor
  runs a kept cmd's block at the newest snapshot's time or the later frame
  the newest cmd built has reached (`Cod4Predictor:: note_damage`,
  `level_at`). INFERRED (the server's order above). A cmd first predicted
  as the frame's own cannot know its packet (sent on the client's 33 ms
  clock), so its kick may be a frame off: up to 1.3 degrees early in the
  return. INFERRED (the test below). 1.7's cgame eases its own kick
  (`CG_DamageFeedback`) off `cg.snap->serverTime`, bytes read as 255ths.
  VERIFIED (cod4-client-view.md 14.2). Rounds fired before the snapshot
  arrives leave without it.
  `cod4_live_hurt_rounds_take_the_servers_damage_kick` hurts us 40
  (ahead, right, no direction) 280 ms before each burst: every own round
  within 1.5 degrees of the server's, most within 0.05; without the
  predictor's kick one read 3.9 off. VERIFIED (the test).
- `cod4_probe`'s sight key is 0x800 (`net::clmsg::buttons::ADS`; the sway
  capture's `fWeaponPosFrac` rises to 1). VERIFIED live. Trap: 0x80000 is
  the detonator's; the wall capture (3.1) was taken with it, so it fired
  from the hip.

### 3.3 The hit pose on the server

vcod: `game::cod4::combat` (`body_trace_eased`, `HIT_BROAD_MINS`), held to
retail's by `cod4::capture_ab`'s throw back and prone runs. VERIFIED (the
gates).

- Both entity clips of `SV_Trace` (0x817d6ac, 0x817dcca) pose a body and
  walk its hit boxes only when the trace meets the box `r.currentOrigin +
  (-64, -64, -32)..(64, 64, 72)` (`.data` 0x826e41c..0x826e430, read at
  0x817d82b and 0x817ded1; no store to it). VERIFIED (the instructions,
  the floats). Ours took (-31, -31, -16)..(31, 31, 86), so a grenade's
  step past the shooter's chest a frame before its strike built no pose.
- The pose (`G_DObjCalcBone` 0x80ca732, and 0x80ca99e) builds a body's
  bones once a frame, running the entity handler's `controller` first:
  cod4-gsc.md 3.2.5, has the path. The player's handler's
  controller is 0x80a480e (handler +0x1c, called at 0x80ca785 and
  0x80ca9d3). VERIFIED (the instructions). The player's controller is
  `BG_Player_DoControllersInternal` (0x804cf90) at `level.frametime`
  (0x80a4838), easing each control 0.36 degrees a ms (0x820963c,
  0x804d5de..0x804d5e7). VERIFIED (the instructions, the float). So a
  body's controls ease once in each frame something poses it, however
  many traces meet it, and a tag read later in that frame (the dying
  body's `tag_weapon_right`, 6.1) reads that frame's skeleton. INFERRED
  (that the skeleton's "built" lasts a server frame). Nothing poses every
  player each frame: no path from `ClientEndFrame` (0x80a68b8) or the
  frame's entity pass reaches the pose but a gunner's turret
  (0x80b79e4), a linked entity's tag (0x80cbab2, entity +0x218) and the
  use list's turret tag (0x80d2c30). INFERRED (the call graph).
- The server's bodies take 1.7's angle updater and controllers
  (cod4-player-anims.md 8: the torso pitch toward twice the view's, no
  `back_up`); ours ran CoD2's. With the three rules the throw back's frag
  meets the shooter's `head` box at 50100, entering its +y face, whose
  normal reads (0.3478, 0.8072, -0.4769) on ours against the reflection's
  (0.348, 0.808, -0.476) on retail. VERIFIED (the gate, a trace on ours).
- The torso pitch's swing and the controllers' differences fold in
  floats, the controllers read the last end frame's view, and the struck
  face's normal comes off the bone's weighted matrix (15.1). VERIFIED
  (the instructions).
- A script's `getTagOrigin` on a player poses it too (0x80cc7e6), so a
  probe reading every player's tag each frame eases every body's controls
  each frame. vcod eases a body at its first tag read of a frame as at its
  first trace (6.1). VERIFIED (`cod4_tag_read_ab`).
- A script's `bulletTrace` does not: it passes no priority map, and the
  locational arm poses a player only under one, so the trace meets the
  player's capsule (cod4-gsc.md 3.3). VERIFIED (the instructions, gdb,
  `cod4_trace_read_ab`).
- The skeleton is built once a skeleton time stamp (`0x13ed7ac4`, checked
  at 0x817cc3a, the DObj's at +0x44) for the bones asked: a later read
  asking for bones the first did not build runs the controller again
  (0x81afaaa). VERIFIED (the instructions). In the gdb run of 6.1 the
  shooter's controller ran twice at 47200, the frame its fire read
  `tag_weapon_right` after `j_head`. VERIFIED (gdb). With the controls at
  their targets the second ease moves nothing. INFERRED.

## 4. Damage

- `Bullet_GetDamage`: full inside `maxDamageRange`, `minDamage` past
  `minDamageRange`, lerped between; `G_DamageClient` multiplies by the
  weapon's location multiplier, at least 1, into
  `CodeCallback_PlayerDamage` with a tenth argument, `level.time` less the
  shot's time. INFERRED. The stock `_callbacksetup.gsc` takes ten, the
  killed callback nine (`timeOffset`, `deathAnimDuration`). VERIFIED.
- Stock weapon numbers (`weapon_dump` over `common_mp.ff`). VERIFIED (the
  weapon files):

  | Weapons | damage / `minDamage` | range |
  |---|---|---|
  | M16, AK-47, G3, MP44 | 40/30 | 1500..2000 |
  | M4, G36C | 30/20 | 1500..2000 |
  | M14 | 50/40 | |
  | MP5, AK-74u | 40/20 | 750..1000 |
  | P90, Uzi | 30/20 | 750..1000 |
  | Skorpion | 50/20 | 200..400 |
  | SAW; RPD; M60E4 | 30/30; 40/40; 50/40 | 1000..1500 |
  | bolt and semi-auto snipers | 70/70 (M40A3 ACOG 75) | |
  | Winchester; M1014 (a pellet) | 40/10; 30/10 | 300..500; 300..600 |
  | pistols; Desert Eagle | 40/20; 50/30 | |

  A silencer shortens both ranges (M16: 500..1000). Location multipliers:
  rifles and SMGs 1.4 on helmet and head, 1 elsewhere; snipers 1.5 on
  helmet and head, 1.5 (M40A3, M21) or 1.1 (R700, Dragunov, Barrett) on
  the neck, 1.1 on the torso; the gun 0 on every weapon.
- What a kill takes on ours: `war`'s 100 health (`scr_player_maxhealth`)
  is three rounds of a 40 in full range, two with Stopping Power
  (`_class.gsc`'s `perk_bulletDamage` 40, so 56 a round: `bot_match`'s
  kill lines read 56 off the assault M16 and 107 off the sniper's M40A3 on
  the lower torso, no bonus off the spec ops MP5 or heavy gunner's SAW),
  four of a 30. `_healthoverlay.gsc` regenerates 5 s after the last hit
  (`regenTime`), so hits that do not finish a victim are spent:
  bot-objectives.md 12.4's 3.7 hits a kill counts every scored round on
  any body, not one life's. VERIFIED (the scripts and `bot_match`'s `K;`
  lines). Retail deals the same: the target's health after each round
  matches retail's every snapshot (section 8). VERIFIED (`cod4_combat_ab`).
- `finishPlayerDamage` (0x80ac58e, `builtins::cod4_combat`) returns at
  once while `client->lastStand` (gclient+0x317c) is set and
  `lastStandTime` (+0x3180) is past `level.time` (0x80ac5d7..0x80ac5ed),
  and for damage under 1 (0x80ac604); an undefined inflictor or attacker
  reads as entity 1022 (0x80ac61e); a victim at `pm_type` 7 is the script
  error `Trying to do damage to a client that is already dead`
  (0x8218d4c, raised at 0x80acc09). VERIFIED (the instructions).
- Flesh events, only for a non-0 weapon with `weapType` (+0x12c) 0 and
  means of death 1, 2 or 8 (0x80acadf..0x80acde4): `EV_BULLET_HIT` (0x29)
  at the point, parm the direction byte, `surfType` 7, attacker in
  `otherEntityNum`, the weapon, scale 1 for hit location 2, the victim's
  bit set in its excluded-client mask (0x80acb07..0x80acb80); then 0x2a,
  or 0x2b with `rifleBullet` (+0x4e0), with the victim's client number and
  every client bit but his excluded (0x80acb87..0x80acbfd). VERIFIED (the
  instructions).
- Entity flag 1 ends the hit after the knockback (0x80ac714); flag 2 holds
  a killing damage to health less 1 (0x80aca05). VERIFIED (the
  instructions). vcod sets neither flag and has neither arm.
- The hit takes the health (0x80ac891), notifies `damage` with damage and
  attacker (0x80ac8c3), and at health <= 0 (0x80ac8e9) goes to Last Stand
  when the flag is clear and the perks byte at gclient+0x5fc has bit 7
  (0x80acc2a), else adds 2 to the first event's scale (0x80acca4) and
  calls the die handler. VERIFIED (the instructions). Before the die
  handler a health caught by `health + 999 > 998` (unsigned, 0x80accab) is
  set to -999; ours sets 0, and retail's dead playerstate health is
  unchecked. INFERRED.
- Knockback scales (1.7's floats): 0.3 a point standing (0x8209684), 0.15
  crouched (0x820deac), 0.02 prone (0x8212488), over 250 (0x8218dbc), at
  most 60 (0x80aca47); `pm_time` twice it clamped to 50..200 (0x80acaaf,
  0x80ace2a). VERIFIED (the instructions).
- The order inside `finishPlayerDamage`, every write to the client's
  playerstate (gclient+0) made in the call: the knockback onto
  `ps.velocity` (+0x28) and, with no `pm_time` (+0x18) running, `pm_time`
  and `PMF_TIME_KNOCKBACK` (0x80aca21..0x80acacf); the flesh events; the
  blood (`damage_blood` +0x30ac, `damage_from` +0x30b0); `damageTimer`
  (+0xa0) up by the damage times `player_dmgtimer_timePerPoint`, capped
  at `player_dmgtimer_maxTime` (dvar pointers 0x827b030, 0x827b034,
  registered at 0x8054825 and 0x805485a), into `damageDuration` (+0xa4)
  (0x80ac787..0x80ac888); `flinchYawAnim`; the health; the `damage`
  notify; then the die or pain handler, and `stats[0]` (+0x148) the
  health after (0x80ac93a). VERIFIED (the instructions and the dvar
  names). `player_die` (0x80b1aa0) fires the death grenades, notifies
  `death` (0x80b1be3), sets `pm_type` 7 (8 from 1, 0x80b1bfd), draws the
  death anim (0x80b1c21), zeroes `stats[0]` (0x80b1c2f) and only then runs
  `CodeCallback_PlayerKilled` through 0x80c2a64 (0x80b1c75); after it the
  followers, contents 0x4000000, the dead yaw, `maxs[2]` 30 and health 0.
  VERIFIED (the instructions). So the killed callback's `clonePlayer` reads
  a velocity the push already holds. INFERRED. `P_DamageFeedback` stays at
  the end frame.
- Ours: `finishPlayerDamage` queues the playerstate half (`Damage4`) and
  the level applies it when the gsc call returns: a round's inside the
  attacker's cmd, a script thread's before the frame's spawns
  (`Cod4Level::run_frame`). The push lands in the call itself on what the
  scripts read (`getVelocity`, `clonePlayer`'s source), each component
  summed as the x87 stack does. INFERRED. The knockback is `g_knockback`
  1000 over 250. `damageTimer` reads the two float dvars (`Dvar_RegisterFloat`
  defaults 100 and 750, 0x8054811 and 0x8054846): the damage times
  `player_dmgtimer_timePerPoint` stored as a float and truncated, added
  on; a sum over `player_dmgtimer_maxTime`, compared as floats, becomes
  the maximum truncated (0x80ac787..0x80ac888). VERIFIED (the
  instructions). `player_die` sets `pm_type` 7, or 8 off a linked body's
  1 (0x80b1bf3: `sete` plus 7). VERIFIED (the instructions). Ours does
  both (`pen_pin_tests::damage_reads_the_dmgtimer_dvars_and_links_the_dead`).
  The kill capture's target, hit after its cmds of that frame,
  reads `pm_flags` 0x100, `pm_time` 0x32, `damageTimer` 750; the prone
  capture's, whose cmds arrived after the shooter's, reads them run down
  by the next cmds. VERIFIED live (the gates, 15).
- `P_DamageFeedback` at the end frame: `damageCount` the blood as a share
  of max health (at most 127), `damageEvent` stepped, direction pitch and
  yaw in 256ths, the count cleared 500 ms on. INFERRED. The capture's 56
  of a 30-health target, yaw 47, pitch 3 match. VERIFIED (the gate).
- A kill: `death` with the attacker, `pm_type` 7, health 0, weapon 0, then
  the killed callback. INFERRED (`player_die`). The death frame reads
  `pm_type` 7, `weapon` 0. VERIFIED live.
- `player_die`'s death grenade constants (inlined from 0x80b1af7): launch
  40 over the body (0x820ddf4), velocity scale 160 (0x82199f0),
  `perk_grenadeDeath`'s pointer 0x828ce78 (default
  `frag_grenade_short_mp`), `G_crandom` at 0x80cb57a as `rand() * 2^-31`
  (0x8212118) doubled less 1 (0x820a238). VERIFIED.
- `player_die` drops live grenades before the `death` notify: a held one
  (`grenadeTimeLeft`, ps+0x44, non-0; the offhand under weapFlags 2, else
  the weapon) with its time left; then, unless means of death 12
  (`MOD_SUICIDE`), Martyrdom (`perks` 0x40, 0x80b1bb2) drops
  `perk_grenadeDeath` on its `fuseTime` (+0x3ec) and sets anim condition 9
  to 2; each at three `G_crandom`s, spinning, through `G_FireGrenade`
  (0x80944a8). INFERRED (the branches). Ours draws the six values before
  the death anim's draw, as retail orders them, and fires the grenades
  once the hit's playerstate half is applied; the anim condition is not
  set. A grenade cooked to its end (`EV_GRENADE_SUICIDE`, time left -1)
  bursts in the hand.
- Last Stand (`perks` 0x80): a killing hit sets `client->lastStand`
  (gclient+0x317c) and `lastStandTime` = `level.time` + 500
  (0x80acc2a..0x80acc49) instead of dying. INFERRED (the branch at
  0x80acc2a). It runs `CodeCallback_PlayerLastStand` through 0x80c293a,
  which pushes nine arguments like the killed callback's pusher at
  0x80c2a64, but `deathAnimDuration` 0 where that one passes its own
  (0x80c2970 against 0x80c2a9a). VERIFIED (the instructions). No damage
  until `lastStandTime`; `ClientEndFrame` holds `PM_LASTSTAND` (6) while
  the flag holds; `ClientSpawn` clears it. INFERRED. The stock
  `Callback_PlayerLastStand` gives the pistol, and `lastStandTimer`'s
  `suicide` 10 s later bleeds out; ours does both
  (`perk_tests::last_stand_goes_down_then_bleeds_out`). A last-stand body
  is shot, collides and goes out as an entity like a standing one.

## 5. The corpse and the obituary

- `obituary` raises `EV_OBITUARY` (66) broadcast: victim in
  `otherEntityNum`, attacker in `attackerEntityNum`, parm the weapon
  number (27, the M16) or `0x80 |` the means of death for 7, 8, 9, 11, 12,
  15. INFERRED (`GScr_Obituary`). The capture's obituary matches.
  VERIFIED (the gate).
- `clonePlayer` puts the corpse in the body queue at 64, `eType` 2, client
  number the victim's. VERIFIED (the gate).
- `player_die` draws its death anim: `BG_AnimScriptEvent(ps,
  ANIM_ET_DEATH, 0, 1)` over the body's last conditions, a clause command
  by `rand()`, its duration the killed callback's `deathAnimDuration`
  (`pmove::anim::event_choices`, kept each end frame). INFERRED. The kill
  capture's standing target dies on `legsAnim` 36, one of the standing
  default clause's eight (`pb_stand_death_neckdeath` ..
  `_neckdeath_thrash`). VERIFIED (the capture;
  `the_kill_captures_death_anim_is_a_standing_death`). The draw is the
  stream's 31st (10.9). VERIFIED (the gate).
- The corpse keeps the body's `eFlags` plus 0xa0000 (0xa0002 off the
  target's 2) and `pos.trType` 1. VERIFIED live. Fall and drag: 5.2.
- The clone's handler row (`.data` 0x826cf48, `think` 0x80a8630) is
  `BodyEnd`, which only clears `eFlags` 0x80000, sets `r.contents`
  0x4000000 and `r.svFlags` 0 (0x80a8630..0x80a864f). VERIFIED (the
  bytes). `clonePlayer` arms it at `level.time + deathAnimDuration`.
  INFERRED (`PlayerCmd_ClonePlayer`). vcod-server clears the bit then
  (`BodyQueue::run_thinks` in the level's frame, ahead of the items).
  Trap: a corpse keeping the bit for life replays its death anim on a
  client reset (cod4-player-anims.md 9).
- `self suicide()` (`PlayerCmd_Suicide`) zeroes health and runs
  `player_die` with the client as inflictor and attacker, 100000 damage,
  `MOD_SUICIDE` (12), so it draws a death anim. INFERRED. vcod-server's
  CoD4 `suicide` does the same; `damage_soak`'s transitions' corpses all
  play a `*death*` clip. VERIFIED. Trap: CoD1's `suicide` draws none, so
  the corpse keeps the live idle (`pb_stand_alert`), a standing body with
  empty hands no player collides with, on every team or spectator switch
  of a live player (the stock menus kill him that way).

The death's torso. `player_die` runs `BG_AnimScriptEvent(ps, 1, 0, 1)`
(0x80b1c00..0x80b1c21: death event, not continuing, forced);
`BG_ExecuteCommand` (0x804f9ca) plays a `both` line's torso with anim 0
(0x804fb1e) through the not-continuing arm, which flips the toggle
(0x804feaf: `torsoAnim & 0x200 ^ 0x200`) and sets `torsoTimer` to the
duration. VERIFIED (the instructions). The command layout is
`bodyPart[2]`, `animIndex[2]`, `animDuration[2]` (parser stores at
0x804e3e9, 0x804e41f, 0x804e436), so a `both` line has no second part.
VERIFIED. `clonePlayer` copies the playerstate's word into the corpse
(0x80ad1c2). VERIFIED (the bytes). The flip always happens, on the
playerstate as the dying client's last cmd left it: a gdb hardware watch
on the target's `torsoAnim` (client +0x94) saw 24 kills, each written
once, by 0x804febf inside `player_die`'s call, to the flip of the word it
found. VERIFIED live. Words at the call and after:

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
(`legsAnim` the standing idle, 0x2b9), so no prone death is in the run.
VERIFIED live. The prone capture of 15 has one: torso 0 at the death,
0x200 on the corpse, the table's flip. VERIFIED live. The kill capture's target took its first round in one frame
and died the next: the cmd between ran the flinch (the `flinch_*`
movetypes' torso line, 0x308) after the 47950 snapshot went out, so the
wire never showed it, and the death flipped it to 0. INFERRED from the
table's first row. vcod-server draws the death's choices after every cmd
(`Cod4Level::think`); `cod4::combat_ab` matches with no GAPS. VERIFIED (the
gate). Trap: drawn once a frame at `ClientEndFrame`, before that cmd, they
flip the stale 0 to 0x200.
`anim_census::a_death_flips_the_torso_toggle_it_finds` holds the table's
pairs.

Recipe: `cod4_lnxded` under gdb in `cod4-handshake.md` 7's namespace,
`+set scr_player_maxhealth 30 +devmap mp_crash`, `war`; `target` joins
allies with `cod4_probe --script poses --respawn --goto "207 2291 296"`
(one state a life), `shooter` axis with `--script shoot --fire-after 2.5
--goto "123 2110 300"`, a spectator beside them, 240 s. gdb breaks at
0x80b1c21 and 0x80b1c26 (the call, its return) and watches each dying
client's 0x94.

### 5.1 The ragdoll

- The stock `_globallogic.gsc` threads `delayStartRagdoll(body, ...)` off
  the killed callback's `clonePlayer`: unless the death anim has an
  `ignore_ragdoll` notetrack, it waits 0.2 s, then the anim's length times
  its `start_ragdoll` notetrack's time (0.35 without one), and calls `body
  startRagdoll(1)` (`level.oldschool` calls it at once and adds a
  `physicsExplosionSphere`). VERIFIED (the zone's rawfile).
- `startRagdoll` (0x80c4f08) reads its optional int and drops it, then
  turns `s.lerp.pos.trType` (entity +0xc) and `s.lerp.apos.trType` (+0x30)
  into ragdoll types: `TR_INTERPOLATE` (1) to 11, `TR_GRAVITY` (5) to 10,
  anything else to 9; no trajectory evaluated. `isRagdoll` (0x80c4eb4)
  tests `pos.trType` for 9, 10 or 11. VERIFIED (the bytes).
- Kill capture: the corpse (64) goes from `pos` 1 / `apos` 0 to 11 / 9 at
  49550, 1550 ms after the clone at 48000, on both clients, and again at
  52650 in the killcam replay (clone at 51100). VERIFIED (the capture).
  vcod-server's `startRagdoll` and `isRagdoll` do the same on a corpse and
  nothing elsewhere; `cod4::combat_ab::the_corpse_turns_ragdoll_when_retails_did`
  holds both clients' type changes to retail's. VERIFIED (the gate, ours).
  The server runs no ragdoll: the client sees the type and simulates the
  bones (cod4-player-anims.md 10).

### 5.2 The corpse's fall and drag

- `clonePlayer` (0x80acf50): `pos.trType` 5 (`TR_GRAVITY`, 0x80ad004) from
  `level.time` (0x80ad010) at the player's origin (`G_SetOrigin`,
  0x80acfeb) and velocity, x and y each capped at
  `g_clonePlayerMaxVelocity` (80, 0x80ad058..0x80ad082); corpse slot
  falling (+0x4d8 = 1, 0x80ad0b9); `clipmask` 0x10001, contents 0x4004000
  (0x80ad1ce, 0x80ad1d8). VERIFIED (the stores).
- `G_RunCorpseMove` (0x80b97e0), in the entity pass: root motion since the
  last frame (`XAnimCalcDelta`, 0x80b98a1); falling, the gravity point at
  `level.time`; resting, only when the motion's `Vec3LengthSq` > 1
  (0x80b9c3c), `pos.trBase` plus the motion along the corpse's forward and
  left (`AngleVectors` of `r.currentAngles`, 0x80b9c96). A capsule sweep
  from `r.currentOrigin` to there; origin lerped by the fraction
  (0x80b9983..0x80b99fd). A clear drag sweep rests it at the end (`trType`
  11 after `startRagdoll`, else 1, 0x80b9a74..0x80b9a96) and, with nothing
  a unit under it (0x80b9b4c), drops it at the motion's pace (10 or 5,
  0x80b9b66). VERIFIED (the instructions).
- The clone's box is the live body's: `clonePlayer` copies `r.mins`,
  `r.maxs` and the abs box (0x80ad121..0x80ad1b0) from inside the killed
  callback (`Scr_PlayerKilled`, 0x80b1c75), before `player_die` sets
  `r.maxs[2]` to 30 (0x80b1da0). VERIFIED (the instructions). A standing
  clone swept `(-15, -15, 0)..(15, 15, 70)`. VERIFIED (gdb at 0x80b9988).
  The sweep is `G_TraceCapsule` (0x80b408a, entities included) passing
  `r.ownerNum - 1`, none for a clone (0x80b990e). INFERRED (the call).
- A sweep that stopped short: the origin lerped by the raw fraction
  (0x80b9983..0x80b99fd), the fraction then read 0 for `startsolid`
  (0x80b9a05); a stop in contents 0x80000000 frees the clone
  (0x80b9dd8..0x80b9df8, 0x80ba0fd); a drag stops there (0x80b9e04). A
  fall `allsolid` all along sweeps again from 32 over the origin
  (0x821b094, 0x80b9e31) to the same point with playerclip (0x10000) out
  of the mask (0x80b9e55) and, unless that one is all solid too, sets the
  origin from its start toward the old origin by its fraction
  (0x80b9ea3..0x80b9efe). Then, on whichever sweep is left: all solid or
  a normal with z above 0, the clone rests there (`trType` 1, or 11 after
  `startRagdoll`, no time, no velocity), its ground the sweep's entity,
  that entity's flags 0x100000 (0x80ba008), its angles on the surface
  unless all solid (0x80b9f83..0x80ba0f6); else the origin moves by the
  normal and the clone falls again from there with `trTime` now
  (0x80b9f2e..0x80b9f7b). VERIFIED (the instructions).
- The plate capture (5.3): the target dies at (207, 2380, 237) beside a
  playerclip brush its 70-high box meets all along (contents 0x8030200);
  the second sweep is clear (fraction 1, normal 0), so the clone stays
  where it is and falls again each frame: `pos.trType` 5 (10 after
  `startRagdoll`), no ground, `trBase` the death's origin, `trTime` the
  frame's. VERIFIED (gdb at 0x80b9988 and 0x80b9e9d, the capture).
  `cod4_corpse_rounds_ab` holds the corpse's every word.
- Kill capture: the corpse (64) at 48000 reads (208, 2294, 236), `trType`
  1, landed in its first frame; drags to (214, 2310, 236) by 48650 and
  holds (the death anim's later frames move its root under a unit a frame,
  which the length test drops). The killcam replays the path from 51100.
  VERIFIED (the capture, the wire's rounded origins).
- vcod: `game::cod4::corpse` (`launch` in `clonePlayer` with the live
  box's height, `run_corpses`; the move is `game::corpse::step`, CoD2's
  frame with 1.7's trace, second sweep (`CorpseWorld::unstick`) and
  types; ahead of the body queue's thinks): node time stepped by `0.001 *
  level.frametime` at rate 1, motion off the clip's root tracks
  (`vcod_cod4::rootmotion::RootClip`, the mantle's translation sample and
  CoD2's yaw turn, INFERRED on 1.7), then the anim update that starts a
  new legs anim at 0.
  `cod4::combat_ab::the_corpse_drifts_on_its_death_anim_as_on_retail`
  holds the rounded `pos.trBase` path to retail's, live and in the killcam
  (a clone set down where a box trace under it meets the floor never
  moves).

### 5.3 Rounds into a corpse

- A clone's contents are 0x4004000 (0x80ad1d8) until `BodyEnd` leaves
  0x4000000 (0x80a8630), and the shot mask 0x2806831 carries 0x4000, so
  a round meets a corpse until its fresh bit goes. VERIFIED (the stores
  and the mask). `SV_ClipMoveToEntity`'s locational arm (0x817de77)
  takes the clone (`svFlags` 2, 0x80ad11a) by its DObj; with none yet it
  sweeps the box's `CM_TempBoxModel` (0x817e0dc), a capsule. VERIFIED
  (gdb at 0x817de9f: no DObj in the kill's frame, one from the next).
  `G_RunCorpseAnimate` builds it: `BG_UpdatePlayerDObj` (0x80ba1c3) off
  the clone's own `clientInfo_t` copy (0x80ad0c0..0x80ad0f6), with an
  ignore-collision mask of 0 (0x80ba1a8, read at 0x804b168), so the head
  the live body's `attach` leaves out of the traces takes rounds on the
  corpse; then `BG_PlayerAnimation` (0x80ba1e5). VERIFIED (the
  instructions). The clone's handler row names no controller (`.data`
  0x826d104), so its skeleton is the anims alone (`G_DObjCalcPose`
  0x80ca9c9). VERIFIED (the bytes). Its handler row has no pain or die
  and `G_Damage` (0x80b16c0) leaves an entity without `takedamage`
  (+0x16b), which `clonePlayer` never sets, so a corpse takes no damage
  and no knockback. VERIFIED (the code).
- `Bullet_Trace` (0x8089f14) reads a struck player or corpse (`eType` 1
  or 2) with no surface as flesh, 0x700000 (0x808a04c). VERIFIED (the
  code). Not a client, a corpse gets 0x808a05c's `EV_BULLET_HIT`, flesh,
  at its entry and at its exit (`un1` 4) under the penetration walk's
  30-unit spacing (cod4-effects.md 12.4). VERIFIED (the captures).
- The kill's own round: past the dying body the next leg meets the
  fresh clone's capsule from inside (fraction 0, the normal off the
  axis), finds its far side 15 out, and the two bodies' flesh spends the
  round, so nothing past the kill raises an impact. VERIFIED (gdb at
  0x8089f83, the capture).
- Captures (cod4-effects.md 12.4's recipe, `--corpse-z 10` on the
  shooter's `cod4_probe`): `play-1.7-csb0corpse-*` (target at 207 2380
  296 behind the metal plate, the gdb run of the two breakpoints above)
  and `play-1.7-csb0drag-*` (207 2291 296 on the open floor, shooter at
  123 2110 300, its corpse dragged by its death anim). vcod's server:
  `Cod4Level::corpse_trace` (`game::cod4::combat`) over
  `Cod4Level::shot_corpses`, the clone's `PlayerBody` copied from the
  client's at its first trace or frame and stepped in `run_corpses`.
  `cod4::capture_ab::cod4_corpse_rounds_ab` and `cod4_corpse_drag_ab`
  match every corpse word, every impact and the dropped rifle with no
  `GAPS`. VERIFIED (the gates).
- The knockback pushes along `Bullet_Endpos`'s direction (0x808a654..
  0x808a6c1): the end less the start, stored, times one over the length
  the stack keeps. glam's float normalize put all three of the floor
  target's `velocity` words an ulp off; `bullet::round_dir` matches them.
  VERIFIED (`cod4_corpse_drag_ab`).

## 6. The dropped weapon

`dropWeaponForDeath` drops the last droppable weapon through `dropItem`:
an `ET_ITEM` (3) whose `index` is the weapon number (27), from the middle
of the body's box along its yaw at `g_dropForwardSpeed` (10) and up at
`g_dropUpSpeedBase` (10) (VERIFIED, the defaults), falling under gravity. INFERRED (`Drop_Item`, `LaunchItem`). The three
`crandom` spreads are drawn and added (10.9). The start at the hand's tag
and `G_RunItem`'s fall and rest are 6.1.
`itemWeaponSetAmmo(clip, stock)` keeps the counts. A living player the
item touches (6.2), falling or at rest, who holds the same weapon takes
clip and stock into the reserve up to `maxAmmo` (+820) and the item goes
(`Touch_Item`, `WeaponPickup_Touch`, `Add_Ammo`). INFERRED. The touch's
event is `EV_ITEM_PICKUP` (9, not `EV_AMMO_PICKUP`'s 10): the lunge
capture (`play-1.7-lunge.txt`, t=52450) has event 9 parm 27 on an attacker
already holding weapon 27. VERIFIED. `a_dropped_rifle_hands_its_ammo_over` runs it on the kill
capture's rifle. VERIFIED (the gate). The use key's swap for a weapon not
held is cod4-gsc.md 9.1. Not run: the compatible-weapon rule, the pickup
prints.

- The item's `clientNum` is its dropper for 1000 ms, then 64: in
  `play-1.7-planefx.txt` the probe's `dropItem` at 47500 makes item 317
  with `clientNum` 0, which reads 64 from the 48500 snapshot on. VERIFIED
  (the capture). The store is 0x8090050 (64 into entity + 0x8c), the
  drop's think. VERIFIED (the bytes). It runs off a `nextthink` 1000 ms
  past the drop. INFERRED (`LaunchItem`, `DroppedItemClearOwner`, matching
  the capture). Until then the use list cuts it for its dropper
  (`BG_CanItemBeGrabbed`, cod4-gsc.md 9.1): the dropper's hint for it (31,
  the M16's 27 plus 4) appears in the same 48500 snapshot. VERIFIED (the
  capture). vcod: `Item4::clear_owner_at`, held to retail's by
  `cod4::planefx_ab`. VERIFIED (the gate, ours).

### 6.1 The start and the fall on 1.7

vcod: `builtins::cod4_combat::launch_weapon` (the start) and
`game::cod4::item` (`G_RunItem`), held to retail's by `cod4::capture_ab`'s
prone and killcam runs. VERIFIED (the gates).

- `PlayerCmd_dropItem` (0x80aaf2a) passes `Drop_Weapon` (0x8091642) a
  tag, the scr string at 0x836ef4c when the script gives none (stock
  `dropWeaponForDeath` gives none). VERIFIED (the instructions, the
  script). That tag is the hand's: ours's posed `tag_weapon_right` of the
  dying body is (210.124, 2311.023, 285.961) in the killcam run and
  (185.978, 2278.777, 246.538) in the prone run, where retail's item starts
  at (210, 2311, 286) and (186, 2279, 246). VERIFIED (the gates' captures,
  ours's pose). INFERRED that the tag is `tag_weapon_right`.
- `Drop_Weapon`'s sweep (0x80918ed..0x80919f7): from the box's middle,
  `(mins + maxs) / 2` plus `r.currentOrigin`, to the tag's origin with
  the item's box, passing the dropper, under the item's mask; the start
  is the sweep's lerp. The killed callback drops before `player_die`
  lowers the box, so `maxs[2]` reads 70 for a standing body. VERIFIED
  (gdb at 0x80918ed and 0x80919d2: the floor run's sweep clear, the
  start the tag). Ours sweeps from the live box (`CorpseSource::height`).
- The tag's read, `G_DObjGetWorldTagMatrix` (0x80cc7ba): the tag's index
  (0x817c2f8), the pose (0x80ca732), the skeleton (0x817c2aa), the
  tag's `DObjAnimMat` at 32 bytes a bone, then `AnglesToAxis` of
  `r.currentAngles` (entity +0x148, 0x80cc810) and `r.currentOrigin`
  (+0x13c, 0x80cc815..0x80cc82d) read at the call. VERIFIED (the
  instructions). In a gdb run on the floor (target facing -90) the dying
  body's `tag_weapon_right` in the body's frame read (20.40226,
  -3.4929945, 50.28386), ours's to the digits, and the placement its
  origin (207.97768, 2293.0752, 236.125) and yaw -89.99451. VERIFIED
  (gdb at 0x80cc7f6, ours's pose).
- So the hand sits where `r.currentOrigin` stands when the killed
  callback drops: after the dying body's own cmds of that level time, and
  inside a round's antilag where the round rewound it. INFERRED (the read
  above, the two runs below). `G_AntiLagRewindClientPos` (0x80a2a0c,
  called at 0x808a71c) runs before the round's traces and the restore
  (0x80a297a, called at 0x808a99a) after them. VERIFIED (the call order).
  The rewind moves only `r.currentOrigin` (+0x13c), saved and relinked,
  for each client whose `sess.connected` (+0x2f8c) is 2 and
  `sessionState` (+0x2f64) 0, when the shot's time is 51..400 ms behind
  `level.time` (0x80a2a43..0x80a2b35); the angles stay. VERIFIED (the
  instructions). The damage and the killed callback run between the two:
  a gdb run (breaks at 0x808a71c, 0x80b16c0, 0x80b1aa0, 0x80cc7ba,
  0x80918ed and 0x808a99a) logged, at the kill of 43550, the rewind, then
  `G_Damage`, `player_die`, `dropItem`'s tag read (called from 0x80918ca),
  the drop's sweep, then the restore, all inside one `FireWeapon`; the
  round before it the same order without the death. VERIFIED (gdb).
  In a gdb run whose round was thought ahead of the target's cmds the tag
  read the origin (208.5629, 2294.3694), 0.36 of the way from the
  origin the 43150 snapshot carried to the 43200 one's. VERIFIED (gdb at
  0x80cc7f6, the capture). That this is the antilag's rewound origin is
  INFERRED. In the floor capture the target's cmd at 43800 ran ahead of
  the killing round and carried the knockback's 1.8 units on, from
  (208.287, 2293.768) to (209.053, 2295.434); ours had placed the posed
  body at its last end frame's origin, so the rifle started at (202, 2314,
  286) where retail's starts at (203, 2315, 286). VERIFIED (the
  captures, ours). vcod places the drop's tag at the dropper's
  `CorpseSource` origin, which each cmd updates, or at the round's
  rewound origin (`Cod4Host::antilag`), and sweeps from the same origin;
  `cod4_corpse_drag_ab` matches the rifle's every word. VERIFIED (the
  gate).
- A script's tag read on a player places the pose the same way. In the
  gdb run above every `getTagOrigin` / `getTagAngles` on a player (615
  calls of `G_DObjGetWorldTagMatrix` returning to 0x80c0053) read
  `r.currentOrigin` equal to `ps.origin` and `r.currentAngles` equal to
  (0, the view's yaw, 0) as the client's last cmd left them; 37 of them
  read an origin and 52 a yaw some cmd had moved since that client's
  `ClientEndFrame`. VERIFIED (gdb). vcod: `cod4_script::posed_body_tag`
  places at `cod4_combat::current_place` (the cmd's origin and view yaw,
  or the antilag's origin). The read also builds the frame's skeleton, so
  it eases the controls once a frame as a trace does (3.3): ours takes the
  controls the end frame eased a copy of the body to
  (`PosedBody4::eased_controls`) and hands them to the level
  (`Cod4HostState::script_poses`). INFERRED (3.3). Without the ease the
  sighted shooter's `tag_weapon_right` read 4.3 degrees of pitch off
  retail's in the gate below. VERIFIED (the gate).
- `FireWeapon` (0x80a3030) notifies `weapon_fired` on the player first,
  every weapon type, unless `eFlags` has 0x300 and the entity's byte at
  +0x16c is set (0x80a3045..0x80a3074, `weapon_fired` at 0x8217922 into
  the const at 0x836f16a). VERIFIED (the instructions). That the byte is
  the one the turret code sets (0x80b8c11) is INFERRED. vcod raises it off each fire
  event (`Cod4Level::fire_weapon`); its waiters run in the next frame's
  threads.
- `cod4::capture_ab::cod4_tag_read_ab` gates both on a retail run of the
  `tag_read` gsc (`play-1.7-tagread-*`): the target walks (`--script push
  --face 90`) and the sighted shooter fires over his head; every frame
  prints each living player's `j_head` and, the frame after each
  `weapon_fired`, the shooter's `tag_weapon_right` and its angles, in
  tenths. The reads also find each goal weight the last end frame set a
  step on (cod4-player-anims.md 3). Ours matches every frame. VERIFIED
  (the gate). The strafe's eight rows were the torso's share of
  the move, 0 on 1.7 (`player_move_factor_on_torso`), which closed three
  of the five tenth rows too, and the spawn's four the spawn's own end
  frame swinging the angles once more (cod4-player-anims.md 8). VERIFIED
  (gdb, the gate). `G_DObjGetWorldTagMatrix` turns the bone's `trans` and
  weighted rows through `AnglesToAxis` of the body's angles (glibc's
  `sinf` and `cosf`) on the x87 stack, and adds the origin last
  (0x80cc810..0x80cc8f4). VERIFIED (the instructions). Ours had placed
  them through a quaternion; it now follows (`cod4_script::world_tag`).
  A gdb run of the recipe (breaks at 0x804c636, 0x80a6f57, 0x804cf90,
  0x80a484d and the tag read's return 0x80c0053) replayed on ours then
  matched 932 of 940 `j_head` origins and 906 axes word for word, where
  the quaternion had matched 918 origins and no axis; the rest sit a few
  ulps off where the skeleton is. VERIFIED (gdb, ours). The gate's last
  two tenth rows (48350, 49850) were not ulps: a second gdb run of the
  recipe (breaks at 0x804cf90, 0x80a484d, `BG_SwingAngles` 0x804b3ba and
  its five returns, 0x80cc7f6 and 0x80c0053;
  `tools/re/cod4_tagread_gdb.py`), replayed on ours, read the
  target's `j_head` 0.049 and 0.034 units off on the frame the strafe
  started and the frame the back-off started, and every other frame
  within 4e-6. On each, retail's legs had swung a frame before ours: the
  updater read the `MOVETYPE` the frame's cmds wrote, ours the one the
  last end frame wrote (cod4-player-anims.md 8.1). VERIFIED (gdb, ours).
  With that, ours matches the run's swings on 706 of 716 frames and the
  target's eased controls on every frame; the 10 are the two spawns'
  frames, whose joins under gdb ours does not replay exactly (the
  shooter's spawn rows differ too). VERIFIED (gdb, ours).
- `LaunchItem` (0x80910ec): angles (0, the body's yaw plus the offset,
  0); `G_Spawn`; `eType` 3, `index` the item, mins -1 and maxs 1 on every
  axis (0x809141b..0x8091453), +0x120 0x407c0108, `clientNum` the dropper;
  handler 16 (0x80914cd: think 0x8090050, touch 0x8092a44); origin at the
  body's x, y and `z + (maxs.z - mins.z) / 2`; `trType` 5 from the level
  time at the launch velocity; flags 0x10; `nextthink` 1000 on.
  VERIFIED (the instructions).
- `LaunchItem` leaves `r.ownerNum` 0 and `clipmask` 0, so the item's own
  traces pass no entity and take mask 0x491; its `groundEntityNum` stays
  `G_Spawn`'s 0 until the rest. VERIFIED (the instructions, the stores
  0x8091392..0x809153e). Retail's flying item reads ground 0. VERIFIED
  live.
- Right after its `G_Spawn` (0x8091169) `LaunchItem` holds the new item
  in a list of entity handles (0x8370c04), its length the integer of the
  dvar at 0x84bc29c (0x8091174..0x8091184). VERIFIED (the instructions).
  That dvar is `g_maxDroppedWeapons` (0x8219d45, `Dvar_RegisterInt` at
  0x80b4a03 storing it at 0x80b4a08), default 16 in 2..32, flags 0.
  VERIFIED (the instructions, the string). The item takes the first empty slot (0x8091199,
  0x80911dd). VERIFIED (the instructions). With none, it frees one slot's
  item (`G_FreeEntity`, 0x80cc040) and takes that slot (0x8091317..
  0x8091387): of the items not being picked up (entity flag 0x1000000,
  0x8091201) whose weapon's `avoidDropCleanup` (+0x3fc, `weaponDefFields`
  offset 1020) is 0 (0x809121f..0x809122c), the one whose nearest playing
  client (`sess.connected` 2 and `sessionState` 0, 0x8091288, 0x8091291)
  stands farthest by squared distance from its `r.currentOrigin`, 1e12
  with no one playing (0x8215e58), the first on a tie (a strict `>` from
  -1, 0x80912e9). VERIFIED (the instructions, the floats). With none to
  free it prints `Could not find a suitable weapon entity to free out of
  %i possible.  Using index zero.` (0x8215cd4) and frees slot 0's
  (0x8091614..0x809163c). VERIFIED (the instructions, the string). The
  flag is set and cleared inside the pickup (0x809227c, 0x8092350,
  0x8092427), so a swap's dropped gun never frees the one it takes.
  INFERRED. A slot whose item was picked up or freed reads empty.
  INFERRED (the handle). vcod: `builtins::cod4_combat::hold_dropped`,
  checked by `a_full_dropped_weapons_list_frees_one_for_the_new_item`.
  VERIFIED (the test, ours).
- With the tag found (`G_DObjGetWorldTagPos`, 0x80cc7ba), `Drop_Weapon`
  traces the item's box from the body's box middle (`currentOrigin +
  (mins + maxs) / 2`) to the tag's origin, mask the item's clipmask or
  0x491, the body passed, and puts `trBase` and the origin at the trace's
  end, `trTime` the level time; either way the item takes the body's
  `currentAngles` (0x80918b1..0x80919f7). VERIFIED (the instructions).
- `G_RunItem` (0x80908bc), each frame: with no ground, or a ground entity
  whose `pos.trType` is set, an item not already falling (`trType` 5 or
  10) falls from where it is (`trType` 5 from now, no velocity), unless
  flag 1 at +0x17c holds it (0x80908cb..0x809096b). VERIFIED (the
  instructions). A client's entity always has a `trType`, so an item met
  mid-air, its ground still 0, falls on from rest the next frame.
  INFERRED. A stationary or
  linked item only thinks. A moving one evaluates its `pos` at the level
  time plus 50 (0x8090974..0x8090986); when that is under 0.1 units off
  (squared, 0x8209568) it aims one unit lower instead. It traces its box
  there from its origin, mask as above, its owner passed. A clean trace
  moves it. A hit takes the end; a hit under 0.01 of the way
  (0x8209564) on ground steeper than 0.5 slides once along the plane and
  traces again (0x8090c4d..0x8090d20); then `pos` becomes `trType` 3 from
  now for 50 ms, from its origin, `trDelta` the move times 20
  (0x8209574), and the origin the end (0x8090d5d..0x8090dd1). After the
  link and the think, an item with +0x100 set whose trace went under 0.01
  of the way rests: freed when the ground's normal points down or its
  origin's contents hold 0x80000000 (0x817da9c); else its axis is rebuilt
  on the normal (left the normal across its old forward, forward the left
  across the normal), turned back into angles (0x81951fe), roll plus 90
  (0x8215d6c) when `index` is set, `G_SetAngle`, `G_SetOrigin` at the end,
  `groundEntityNum` the trace's entity, bit 0x100000 on that entity
  (0x8090ab0..0x8090bed). VERIFIED (the instructions).
- The arithmetic. The move's end is `start + (goal - start) fraction` on
  the stack; the slide adds `normal (1 - (goal - start) . normal)` to the
  goal, kept, then stored, and traces from the stored end
  (0x8090c92..0x8090d20). The 50 ms move's `trDelta` takes x off the
  stack, y and z off their stored differences, each times 20 (0x8090d91..
  0x8090dc7). At rest `left = normal x forward` stays on the stack and is
  stored, `forward = left x normal` is stored (0x8090b18..0x8090b7a),
  `forward` the old angles' `AngleVectors` (0x818ea3a). VERIFIED (the
  instructions).
- `AxisToAngles` (0x81951fe): pitch and yaw are the forward's
  `vectoangles` (0x8191a72: yaw `180 / pi * atan2(y, x)` stored, then 360
  or 0 added; pitch `-180 / pi * atan2(z, |xy|)` the same way, the flat
  length unrounded). The roll turns the left back by the negated yaw and
  pitch (`sinf`, `cosf` of the radians stored as floats; `a = x cos - y
  sin` and `b = x sin + y cos` stored, the pitch's turn kept) and is
  `-180 / pi * atan2(d, sqrt(b^2 + c^2))` (the float -180 at 0x820decc),
  negated for `b` at least 0, else less 180 when positive and plus 180
  when negative. VERIFIED (the instructions).
- The prone run's item: (186, 2279, 246) at 46100 along
  (-104.9452, -103.4213, 8.3989); `trType` 3 at 46250 from
  (170, 2263, 239) along (-24.8587, -24.4981, -31.1728) for 50 ms; at rest
  from 46300 on (169, 2262, 237), yaw -134.9945 where it fell at
  -134.989, roll 90, ground 1022. VERIFIED live (`play-1.7-prone-*`).
  Ours matches every word of both runs' items, on the live frames and the
  killcam's, the hit's `trDelta` included. VERIFIED (the gates). Before
  the bodies' tags took 1.7's skeleton (cod4-gsc.md 3.2.5)
  the `trDelta` read (-24.8591, -24.4986, -31.1734) against (-24.8587,
  -24.4981, -31.1728) prone, (4.3898, -0.9870, -39.2181) against (4.3897,
  -0.9870, -39.2175) standing, the tag some 3e-5 units off retail's,
  composed off quaternions. VERIFIED (the gates, both ways).
  The prone start's z read 247 against 246, the tag 0.11 high,
  until the server's bodies took 1.7's controllers and the tag read the
  killing frame's skeleton (3.3): the round's trace eases the controls
  at 46150 and the drop in the same frame reads that pose, where ours
  read the frame before's, one ease short. VERIFIED (the gate, which
  read 247 under 1.7's controllers until the drop read that pose).

### 6.2 The touch

VERIFIED (the instructions) where no label says otherwise:

- When. `G_RunFrame` (`cod4_lnxded` 0x80b61fc) opens with a pass over
  the clients (0x80b624b..0x80b6298): each in use (+0x100) whose client
  word +0x3078 is 0 goes through `G_TouchTriggers` (0x80a5352), ahead of
  the scripts and the entities. So an item dropped in a cmd's
  `ClientEvents` (a kill's `dropWeaponForDeath`) is touched at the next
  frame's start, after that frame's cmds and before its snapshot.
- Who. `G_TouchTriggers` returns for a `pm_type` (ps+4) past 1
  (0x80a5367). `BG_CanItemBeGrabbed` (0x805301e) refuses an item whose
  `clientNum` (+0x8c) is the toucher's (ps+0xdc, 0x8053043..0x805304f),
  the dropper's 1000 ms (6), and a toucher with `weapFlags` (ps+0x10) bit
  7 or `pm_flags` 0x100000 (0x805302d, 0x8053051).
- Where. The candidates are `CM_AreaEntities` (0x811f386) on the
  player's `absmin`/`absmax` (+0x124, +0x130) less and plus 20 (0x8209654)
  under contents 0x405c0008 (0x80a5378..0x80a53e3). An `ET_ITEM` (`eType`
  3, 0x80a550c) goes through `BG_PlayerTouchesItem` (0x8053910) at
  `level.time`, anything else through `SV_EntityContact` (0x817c862). The
  first evaluates the item's `pos` (`BG_EvaluateTrajectory` 0x80536ba,
  stored) and takes the origin less it on the x87 stack: within 36 either
  way in x and y (0x820aab8, 0x820aabc) and from 18 below the origin to
  88 above it in z (0x820a314, 0x820aac0). No test of rest.
- Then `touch` both ways with the script up, and the entity's handler
  touch (row +0xc of 0x826cee0 by the handler byte +0x16e), items' being
  0x8092a44 into `Touch_Item` (cod4-gsc.md 9.1.2).

`play-1.7-lunge.txt` shows it: the target's M16 (27) drops from the kill
at 52450 and is taken in that frame, `EV_ITEM_PICKUP` parm 27 ringing on
the attacker after `EV_MELEE_BLOOD`, reserve 60 to 150, with no `ET_ITEM`
in any snapshot. VERIFIED (the capture). 150 is the attacker's 60 plus a
full clip of 30 and a stock of 60 (`maxAmmo` 180), the spawn rounds of
the class both probes got, which the target never fired. INFERRED.

`cod4_moves_ab`'s `lunge` replay writes the pickup at the frame's start
from its `PICKUPS` row (the item's rounds as above), through
`vcod_cod4::pmove::weapon::take_ammo`, the server's own take. VERIFIED
(the gate).

Open in vcod: `Cod4Level::touch_items` still takes only resting items,
after the frame's item moves, inside a 15-unit box over the item's base,
with no dropper's cut. 1.7's bounds, timing and cut stall
`cod4_damage_survives_every_transition` at its `KillInKillcam(1)` step
(the victim never leaves spectator; the bounds alone do it), not yet
understood. The item's link box, which the area query reads, is unread.

## 7. The killcam

`_killcam.gsc` sets `spectatorclient` to the attacker and `archivetime`
to `camtime + predelay` (2.5 + 2 s). The engine sends the follower the
followed client's archived playerstate from that long ago, `otherFlags`
2, `deltaTime` the look back, `killCamEntity` 1023, trimmed to the oldest
frame the followed client was a player in, and writes the trim back into
`archivetime` (the rule is 7.1). `killCamEntity` is the script's `self.killcamentity`
(gclient +0x2f6c), 1023 for -1 (`cmovne` at 0x80a4aa9); its setter
(0x808b288) refuses all but -1..1023 (`killcamentity can only be set to
-1, or a valid entity number`, 0x8214d3c); `ClientSpawn` and the
killcam's end write -1 (0x80af26b, 0x80a4afb). VERIFIED (the
instructions). Stock `_globallogic.gsc` passes the inflictor's number for
a `cobra` kill (the helicopter), else -1. VERIFIED (the script). vcod
reads the field into the killcam frame
(`cod4::combat_ab::a_killcam_entity_rides_the_killcam_playerstate`).

- The kill capture's target at 50000 reads the shooter's spawn frame of
  46900 with `deltaTime` 3100 where 4500 was asked. VERIFIED live. Ours
  matches.
- The archived playerstate has been through the delta codec once: a gdb
  run (a break at `ClientEndFrame` 0x80a68b8 on each client's view word,
  client+0x108) read the shooter's live pitch -1.0382 (cmd -189 shorts)
  and the killcam follower's copy -1.03271, -188 shorts. VERIFIED live.
  So a view of -354 shorts archives as -353 and goes out as -352, which
  the plate capture's killcam reads. VERIFIED live. That the archive is
  `SV_ArchiveSnapshot`'s `MSG_WriteDeltaPlayerstate` read back is
  INFERRED (the quantization's shape). Ours sends the archive's angles
  once (`PlayerState::sent_angles`) as it records it.
- The follower gets the entities of the snapshot round `archivetime`
  back (each round's kept 200 frames, the playerstates' look-back; 7.1):
  retail's killcam frames carry the victim's living body (entity 0,
  `eType` 1) where the live world holds only the corpse. VERIFIED live.
  Without the archive 22 of the gate's killcam rows fail, with it none.
  VERIFIED (the gate).
- `SV_BuildClientSnapshot`'s archived pass skips an entity whose archived
  `clientMask` (+0xf8) holds the bit of the followed client's number
  (the archived playerstate's +0xdc, read at 0x817a3f5; the test at
  0x817a5ad..0x817a5c5) and the followed client's own entity
  (0x817a5cd), and sends one with `svFlags` 0x18 (+0xf4, 0x817a5d5)
  unculled; nothing in it reads a live entity.
  VERIFIED (the instructions). Ours had also left out every archived
  entity whose number a live single-client temp entity held: the plate
  run's archived impact 348 of 45100 went missing from the killcam's
  48350..48500 while a live sound for the target held 348. Ours now
  filters the live temps from the live world only;
  `cod4_corpse_rounds_ab` matches every killcam entity. VERIFIED (the
  gate).

The archived playerstate's clocks move up by the look back: the first
killcam frame at 50000 reads `commandTime` 50000, the shooter's 46900 plus
`deltaTime` 3100. VERIFIED live (`play-1.7-kill-target.txt` message 132).
`SV_GetArchivedClientInfo` (0x81795e2) shifts, each only when set:
playerstate +0, +0x18, +0x54, +0x80, +0x11c, +0x630 (`commandTime`,
`pm_time`, `foliageSoundTime`, `jumpTime`, `viewHeightLerpTime`,
`shellshockTime`); the archival hudelems' +0x70, +0x2c (clamped to
`svs.time`), +0x50, +0x68 (`time`, `scaleStartTime` no later than now,
`moveStartTime`, `fadeStartTime`; array at +0x1c04, 0xa0 apart) up to the
first of type 0 (31 at most); then adds to +0x89c (`deltaTime`)
(0x81797bc..0x81798aa). VERIFIED (the instructions). Without it the
follower's `commandTime` sits seconds behind its own cmds and
`CG_DrawDisconnect` (`iw3mp.exe` 0x42f930, cod4-hud.md 14) draws
Connection Interrupted all killcam: 260 of 265 frames in the live gate
without the shift, none with it. vcod: `level::shift_archived`;
`cod4_combat_ab` compares the first `commandTime`; live gate
`cod4_live_killcam_draws_no_interrupt_and_the_use_key_skips_it`.

### 7.1 The look back, frame by frame

`SpectatorClientEndFrame` (0x80a49f0) asks `SV_GetArchivedClientInfo`
(0x81795e2) for the followed client `archivetime - psoffsettime` back
(gclient +0x2f74 less +0x3070, 0x80a4a68..0x80a4a74), into a local copy.
That call turns the age into whole frames, `age * sv_fps / 1000`
(0x8179604..0x8179618), takes the archived frame that many back from the
next one (at most 1200 back, 0x817961e), walks forward to the first frame
still held (0x8178d0e), and finds the client in it; a client absent or
without a playerstate fails (0x81796f1..0x817977c). On a failure the
caller lowers `archivetime` itself by 50 and asks again, and at 0 ends
the killcam: `spectatorclient`, `killcamentity` and +0x3074 go -1
(0x80a4ac5..0x80a4b05). So the trim written back is `archivetime` stepped
down 50 at a time, never the frame's age. VERIFIED (the instructions).
`deltaTime` grows by `svs.time` less the found frame's time
(0x8179782..0x8179785, 0x81798aa). VERIFIED (the instructions).

`psoffsettime` is a plain int (field table row at 0x8214fec: offset 0x3070,
setter 0x808b1aa stores `Scr_GetInt(0)`; its getter 0x808b194 reads
+0x2f74, `archivetime`'s cell). VERIFIED (the bytes). `_killcam::killcam`
sets it to the `offsetTime` `Callback_PlayerKilled` passes on, the kill's
`psOffsetTime`; `ClientSpawn`'s script and the killcam's end set it 0.
VERIFIED (the script, `common_mp.ff`).

`SV_BuildClientSnapshot` takes the killcam's entities on its own clock:
`archivetime` alone (`G_GetClientArchiveTime`, 0x80b38a2, called at
0x817a2f4), whole frames back the same way (0x817a31d..0x817a335), and
writes a clamp back through 0x80b38cc. Each archived entity's clocks move
up by that round's age, `svs.time` less its time (0x817a3b0..0x817a3c1):
`pos.trTime` (+0x10), `apos.trTime` (+0x34) and `time2` (+0x70) where
set, and +0x54 (a missile's `launchTime`) of `eType` 0, 4 and 0x42 always
(0x817a849..0x817a8b6). VERIFIED (the instructions).

So with a kill's `psOffsetTime` in 1..50 the playerstate comes a frame
younger than the entities. The prone capture's killcam reads `deltaTime`
4200 where its entities sit 4250 back. VERIFIED live. vcod:
`Cod4Level::killcam_end_frame`, `killcam_entities`; `cod4_prone_death_ab`
matches every killcam row, `deltaTime` and the end included. VERIFIED (the
gate).

The skip: `_killcam::waitSkipKillcamButton` waits for `useButtonPressed`
to go 0 then 1. 1.7's `useButtonPressed` (0x80a9b46) tests `(buttons |
buttonsSinceLastFrame) & 0x28` (the OR at client+0x3094);
`attackButtonPressed` (0x80a9aae) tests 0x1, `adsButtonPressed`
(0x80a9a14) 0x800. VERIFIED (the code). A follower's `SpectatorThink`
stores its cmd's buttons there. INFERRED, unconfirmed on 1.7. vcod reads
the CoD4 words for all three (`builtins::cod4_script`); the live gate's
press ends the killcam a frame later. Trap: CoD1's last-cmd word, which a
follower's cmds never reach, never skips.

## 8. `cod4_combat_ab`

`cargo test -p vcod-server --lib -- cod4::combat_ab --nocapture`
(`COD4_DIR`; `COMBAT_REPORT=1` for every row). The kill capture's two
probes join ours in process on retail's clock; each sends its
`setviewpos` when its twin did, then from their spawns (45950, 46900) the
captures' cmds. Compared by server time: the shooter's weapon words every
snapshot 47550..63350 (the capture's end); the target's damage words,
health, flesh event, corpse, item and obituary 47500..48400; the
shooter's `EV_BULLET_HIT` parms; the killcam's first frame and look back;
the corpse's `legsAnim`, `torsoAnim`, `eFlags`, `pos.trType`,
`groundEntityNum`; the killcam frames' players and corpses. Every value
matches retail, entity numbers included (section 14), with no `GAPS`.
VERIFIED (the gate). The corpse's `torsoAnim` was the last gap (section
5).

`the_sight_the_trigger_and_a_switch_drive_the_weapon` sends the live
shooter, over the wire, sight key 0x800, then 0x801, then weapon byte 11
(the Beretta `weapnext` selects): `fWeaponPosFrac` reaches 1 with
`PMF_SIGHT`, `EV_FIRE_WEAPON` rides the ring, the M16's clip drops 30 to
27 (one burst), and by 52900 `weapon` is 11. VERIFIED (the gate).

## 9. Hit detection against the aim

`a_shot_at_the_head_scores_the_head`: on the kill pair, from 47450 the
shooter's cmd views the centre of the target's posed head box (hit
location 2, the trace's own pose) from its eye over its `delta_angles`,
sights up and taps once; the first round scores location 2. VERIFIED (the
gate). View, eye and hit boxes agree on ours. The hip spread is wide: the
capture's hip rounds leave 3 to 6.3 degrees off the view (21 units at
200), so a hip shot at a head mostly misses, as on retail. VERIFIED (the
gate's spreads). The eye carries the walk bob and lean and the gun its
sway (3.2), as a client's own aim does.

## 10. Grenades, the launcher and the melee

### 10.1 The offhand in pmove

`PM_Weapon_CheckForOffHand`, INFERRED, each step: off a turret, weapons
on, not sprinting, not holding a main-hand grenade, outside the offhand
and night vision, the cmd's offhand byte is taken where held. The frag
key (0x4000) or smoke key (0x8000, the flash class under
`offhandSecondary`) switches to the first weapon of its class with a
round (`EV_SWITCH_OFFHAND`, weapon in the parm). The states:

| State | When |
|---|---|
| `OFFHAND_LOWER` (15) | main weapon down at its quick drop |
| `GRENADE_PIN` (16) | straight here with empty hands; `holdFireTime`, `EV_PREP_OFFHAND` |
| `GRENADE_HELD` (18) | the fuse; held key stays here |
| `GRENADE_THROW` (17) | throw anim at `fireTime`, release at `fireDelay` |
| `GRENADE_GONE` (19) | the release: `EV_USE_OFFHAND`, a round |
| `OFFHAND_RAISE` (20) | the quick raise |

Under `bCookOffHold` the fuse runs down at 18 and at 0 reads -1 with
`EV_GRENADE_SUICIDE`. `frag_grenade_mp`: `holdFireTime` 600, `fireTime`
360, `fuseTime` 3500, cook on, quick raise and drop 500, offhand class 1.
VERIFIED (`weapon_dump`, `common_mp.ff`). The walk capture's throw reads
states 15, 16 (`weaponTime` 0x238), 17 (0x158, delay 0x71, fuse 0xd8c),
19 at 66050, then 20. VERIFIED live. `cod4_moves_ab`'s `frag` rows replay
it armed from 64500, every word exact on all 456 rows, free and resynced,
the fuse's zero on the throw's row written as the server writes it
(10.1.1). VERIFIED (the gate). A main-hand grenade's pin raises
`EV_PULLBACK_WEAPON` (25). INFERRED.

#### 10.1.1 The fuse's zero

VERIFIED (the instructions):

- `ClientThink_real` hands each event a cmd raised to `ClientEvents`'
  body (0x80a581e, the loop 0x80a62f4..0x80a6327) before the link and
  `ClientImpacts`. `EV_USE_OFFHAND` (33) is the jump table's (0x8217d98)
  entry 14, 0x80a5aa7, a tail jump into `G_UseOffHand` (0x80a2b42): the
  offhand's def by `offHandIndex` (ps+0xe0), the view, the eye, then the
  throw (0x80a21fa) with the offhand's `weaponmodels` byte and the def's
  `fuseTime` (+0x3ec) into `G_FireGrenade` (0x80944a8, called at
  0x80a22de).
- `G_FireGrenade`'s fuse arm (0x8094595..0x80946b5): a def with
  `projectileActivateDist` (+0x574) and no fuse above 0 skips it.
  Otherwise, for a thrower with a client, a `timedDetonation` def
  (+0x5ac) bursts at `level.time` plus the thrower's `grenadeTimeLeft`
  (ps+0x44) when that is set (0x809467c..0x80946af), else at `level.time`
  plus the fuse (0x8094683), and both arms store 0 into `grenadeTimeLeft`
  (0x80945c8).

So the snapshot after the throw reads `grenadeTimeLeft` 0 where pmove
left the cook's remainder (3275 on `frag`'s row 414). vcod:
`WeaponDef::takes_fuse`, read by `Cod4Level::new_grenade` and by the
replay's `Replay::think` after each cmd's `EV_USE_OFFHAND`.

### 10.2 The missile

INFERRED, each step: `G_UseOffHand` and `Weapon_Throw_Grenade` throw from
the eye along the view at `projectileSpeed` (940) plus
`projectileSpeedUp` (120) up and the thrower's speed along the throw,
`G_FireGrenade` truncating the speed whole. `G_InitGrenadeEntity` makes an
`ET_MISSILE` (4), `eFlags` 0x1000000, `clientNum` 64, `index` the weapon
(a frag is a pickup item); `G_InitGrenadeMovement` the spin (pitch less
120, two random rates); the no-draw time hides it 20..50 ms by speed;
`grenade_fire` goes to the thrower with the grenade and name. The walk
capture's frag reads `trDelta` (-664, -664, 120), `launchTime` 29 past
`trTime`, `trType` 5, `apos.trType` 2, `eFlags` 0x1000000, `clientNum` 64,
`index` 40. VERIFIED live. Ours matches each word on every row. VERIFIED
(`the_frag_flies_on_ours_as_on_retail`).

`G_RunMissile`, INFERRED, each step: the arc's next point traced through
the clip map, static models (`CM_PointTraceStaticModels`) and bodies' hit
boxes under 0x2806891, passing the thrower; a bouncing grenade near a
floor snaps onto it (0.135 up, 1.5 down). On contact, `MissileImpact`:
off the world or an undamageable entity, `BounceMissile` reflects the
move at the contact and scales it by the weapon's `parallelBounce` and
`perpendicularBounce` for the surface type by the angle; on a floor
(normal z > 0.7) under 20 units/s it rests lying along it. A body takes
the weapon's `damage` as `MOD_IMPACT` above `g_minGrenadeDamageSpeed`
(400) and bounces it as flesh. `EV_GRENADE_BOUNCE` (44) carries the
surface type where the move changed by over 100. The walk capture's frag
bounces at 66550 and 67100 (`events` 0x2c, parms 0x16 and 5), off a wall
at 67150, rests at 67300. VERIFIED live.

The wall at 67150 is static model 1901 of mp_crash's clip map, an
`fx_me_brick_01` rubble piece (origin (-529.1, 1447.7, 276), scale 1.264)
with a concrete collision surface (type 5, the bounce's parm). The map
zone only names that xmodel (`,fx_me_brick_01`, a reference into another
zone); common_mp holds it, one collision surface of 12 triangles.
mp_crash places 219. VERIFIED (the zones). `ClipMap::from_zones` takes a
comma model from the zones loaded before the map's, and every walk keeps
an xmodel's collision (`XModel::coll`). Trap: from the map zone alone the
bricks clip nothing and the frag flies past the wall.

Arithmetic, VERIFIED (the disassembly):

- `BG_EvaluateTrajectory` (0x80536ba), `BG_EvaluateTrajectoryDelta`
  (0x8053124): time is the float 0.001 (0x8209640) times the ms; every
  axis `delta t + base` kept on the stack; gravity's `-400 t t`
  (0x820aab0) added to the kept z; the delta's `-800 t` (0x820aaac).
- `BounceMissile` (0x80946ba): `hitTime` is `(int)` of the stored float
  `(time - previousTime) * fraction`. The dot's first two terms are stored
  as floats, the last kept; the reflection scale is `-2.0` times the kept
  sum; the stored dot over the kept `-|v|` is the cosine; the factor
  `(perp - par) cos + par` stays on the stack for the three products. The
  push off the surface is the normal times the float 0.1 (0x8209648), z
  clamped to 0, added on the stack.
- `G_InitGrenadeMovement`'s spin (0x8093012): `G_flrand(320, 800)` and
  `G_flrand(180, 540)`, each times `2 G_irand(0, 2) - 1`. `G_flrand`
  (0x80cb548) is `(max - min) rand() * -1/2^31 + min` (the float at
  0x8214ac4 is negative), so a draw falls in `(2 min - max, min]`.
  `G_irand` (0x80cb522) divides the wrapped 32-bit product by
  `(int)0x80000000`, so `G_irand(0, 2)` is 0 except for a draw of 2^30,
  and the spin turns backwards on both axes. `MissileLandAngles`
  (0x80930d4) draws `rand() & 0x7f` less 63 and `G_random` (0x80ca2ee,
  `rand() / 2^31`). The walk capture's spin, -296.36 and -102.30 degrees/s,
  is in range and backwards. VERIFIED live.
- `rand()` is glibc's (PLT slot 0x826a768); `G_InitGame` seeds it with
  `srand` (0x80b56ee). Every caller before the throw is modelled (10.9).

`the_frag_flies_on_ours_as_on_retail` meets every bounce on retail's
frame; bounce points, times and events, the spin and its turns (10.9),
the rest, the burst and the script's quake match on every row, word for
word; `FRAG_GAPS` is empty. VERIFIED (the gate). The last bits came from
three float models: the second bounce, off a terrain triangle, takes
1.7's point sweep (cross product kept on the stack and stored,
`Vec3NormalizeTo`, cod4-collision.md 3); the third, off the rubble, the
static model's normal rounded the same way (cod4-collision.md 4); the
burst's x, a unit off, is `G_TempEntity`'s truncation of the resting point
(-523.58 to -523), hidden by the wire's rounding of the grenade's own -524
(10.3).

### 10.3 The burst

At its fuse `G_ExplodeMissile`, INFERRED: `explode` with the point; an
event entity (`ET_GENERAL`, `eFlags` 0x20, the weapon) with
`EV_GRENADE_EXPLODE` (45) and the normal and surface type a trace 16 down
meets, freed 300 ms on (a smoke cloud's lives 61 s with `eFlags`
0x90000); then `G_RadiusDamage`: every player in `explosionRadius` but the
ignored one, through `CanDamage`'s five traces (the body's middle half way
to the eye, four points 15 to each side and half that height up and down)
under 0x802011 inside the cone, inner damage falling to outer at the edge,
times a third a trace through (all at four or five), radius flags 5,
direction toward the body 24 up, `MOD_GRENADE_SPLASH`; then `death` and
the grenade freed. The capture's burst entity 317 is `eType` 0, `eFlags`
0x20, weapon 40, event 0x2d parm 5; the script's earthquake
(`_shellshock.gsc`'s `grenade_earthQuake`, 0.3, 0.5 s, 400) rides 17 + 61
at the same point. VERIFIED live. Both points are the grenade's
truncated (`G_TempEntity`'s `SnapVector`, as every temp entity's): the
resting grenade at x -523.58 goes out as -524 (the field's rounding), its
burst as -523. VERIFIED (the capture). Ours raises both with those words
at retail's point. VERIFIED (the gate).

A claymore or C4 kill needs the `owner` the script gives the grenade on
`grenade_fire` (`_weapons.gsc` `watchClaymores`, `watchC4`):
`Callback_PlayerDamage` (`_globallogic.gsc`) sets
`explosiveInfo["bulletPenetrationKill"]` only for a `c4_` or `claymore_`
weapon whose inflictor has one, and `_missions.gsc` `ch_kills` reads it on
every such enemy kill (line 581 for the claymore), so a burst with no
owner aborts there ("cannot cast undefined to bool"). The frag, RPG and
launcher arms read only fields the damage callback always sets, and a kill
by a non-player (a damage trigger) never reaches `ch_kills`
(`Callback_PlayerKilled` calls `_missions::playerKilled` only for an enemy
player). VERIFIED (the 1.7 rawfiles in `common_mp.ff`). Planted claymore,
C4, frag and RPG kills at 100 health run clean on ours; only a test that
bursts a grenade in the call that threw it, ahead of the frame's notifies,
met the abort. VERIFIED (`a_claymore_kill_runs_the_kill_scripts_clean`).

An impact grenade (`projImpactExplode`; the launcher's `gl_m16_mp`: speed
2400, up 10, arming distance 375, inner 155 and outer 25 over 300,
VERIFIED `weapon_dump`) bursts on its own entity where it struck past its
arming distance (event on the missile, `projectile_impact` to the thrower)
and bounces as a dud short of it; `Weapon_GrenadeLauncher_Fire` fires it
with no spin and no fuse. INFERRED; no capture fires the launcher.
`the_grenade_launcher_fires_its_grenade` switches the kill pair's shooter
to the M16's alt and fires: the grenade (weapon 28) strikes inside its
arming distance and bounces on as a dud. VERIFIED (the gate).

`Weapon_GrenadeLauncher_Fire` (0x80a2f30) marks its grenade stable right
after `G_FireGrenade` returns it: `orl $0x20000, 0x180(%eax)` at
0x80a2f9b, the entity's `flags` (`FL_STABLE_MISSILES`). VERIFIED (the
disassembly). `G_RunMissile` wobbles every unguided `weapType` 2 missile
without that flag (0x80979dd), the launcher's grenade included. INFERRED
(the branch). Trap: without the flag the grenade's first free step takes
the wobble at once, goes `TR_INTERPOLATE` and, run as a grenade (arc
evaluated, not stepped), hangs 120 units ahead of the eye and never
bursts. VERIFIED live (vcod-server and our client).
`cod4_live_grenade_launcher_fires_along_the_view` (client `play::cod4`)
fires the assault class's launcher through the action slot and trigger on
vcod-server: the grenade goes out `TR_GRAVITY` at 2400 along the view and
bursts (`EV_GRENADE_EXPLODE` on its own entity) 668 units on. VERIFIED
(the test, ours).

Damage from a grenade, the launcher or a turret takes
`info/mp_lochit_dmgtable`'s multipliers (1 everywhere but the gun's 0),
not the weapon's (`G_GetWeaponHitLocationMultiplier`). INFERRED. The table
is VERIFIED (the `common_mp` rawfile). Trap: the frag's own table gives 0
for no location, so its splash did 1 point. VERIFIED live.

Elsewhere: the live grenade's pickup and throw back (cod4-gsc.md 9.3),
damage to entities other than players (13), rockets (12), the flashbang
and concussion (10.8). Not run: water and glass.

### 10.4 The melee

`PM_Weapon_CheckForMelee`, INFERRED: the melee key's edge (0x4) on a
weapon with `meleeDamage`, outside the melee, the offhand and night
vision, not through a scope, no delayed action and none pending but a
reload's, from ready or past the raise and drop states: `MELEE_SWING`
(12) for `meleeTime` with `EV_MELEE_SWIPE` (30) and the `meleeattack` or
`knife_melee` anim event; at `meleeDelay` `MELEE_HIT` (13) with
`EV_FIRE_MELEE` (31); then a knife weapon's quick raise (`MELEE_RECOVER`,
14, anim 20) or ready.

On `EV_FIRE_MELEE` the server's `FireWeaponMelee` traces from the eye
along the view (bodies rewound) `Melee_Trace`'s centre and four corners
`player_meleeWidth` and `Height` (10, 10) apart at `player_meleeRange`
(64; VERIFIED, `cod4_dvars.py`), then the two cross traces; the first
contact raises `EV_MELEE_HIT` (35; on a body with `EV_MELEE_BLOOD` on the
attacker) or `EV_MELEE_MISS` (36), the knife bit in the parm; a body takes
`meleeDamage` plus `G_rand() % 5` as `MOD_MELEE`, no location multiplier.
INFERRED. The ray ends' signs are rodata 0x82173e0 (centre, upper right,
upper left, lower right, lower left), read at 0x80a24c0 by the rays and at
0x80a26ab by the cross traces; every trace uses mask 0x2806891
(0x80a2417, 0x80a25a0) and `bulletPriorityMap` (0x80a240f, 0x80a2598) and
tests surface flag 0x10 (0x80a247b, 0x80a25fc). VERIFIED (the floats and
the loads). The cross traces join corner 1 to 2 and 3 to 4 and refuse a
start in solid (0x80a2602). INFERRED (the index arithmetic). M16
`meleeDamage` 135, delay 100, time 660. VERIFIED (`weapon_dump`).
`the_melee_key_swings_and_hits` runs it in process. VERIFIED (the gate).

The lunge: a press whose cmd carries `meleeChargeDist` (the client's pick
of a target 64 to 128 off near the crosshair) sets `PMF_MELEE_CHARGE`
(`PM_MeleeChargeStart`, 0x80561e6) and, on a weapon with a charge anim and
`meleeChargeTime`, swings the charge: weapon anim 9, `meleeChargeDelay` to
the hit (159 on the stock rifles against 129 plain), `meleeChargeTime`
(1159 against 800), `KNIFE_MELEE_CHARGE` (0x13) in the anim script.
VERIFIED (0x806346a..0x806350c; the zone). Pmove throws the body along the
cmd's yaw at `sqrt(2 * dist * 1200)` and `player_meleeChargeFriction`
stops it in `dist` units, so the hit's trace at `player_meleeRange` starts
beside the target. VERIFIED (cod4-pmove.md 19: addresses, the client's
pick). `cod4_live_the_melee_key_lunges_at_a_bot_in_reach` (client and
server) lunges at a bot 100 units off. VERIFIED (the gate).

#### 10.4.1 The blood on the attacker

VERIFIED (the instructions):

- `EV_FIRE_MELEE` (31) is the jump table's entry 12, 0x80a5ada: with the
  bool of the dvar behind 0x84bc368 set (its byte +0xc; 0x80a5b4a when
  not, unread), a tail jump into `FireWeaponMelee` (0x80a3492), the word
  at +0x3168 of the client its second argument. After the traces
  (0x80a2a0c, called at 0x80a3540) the contact goes to 0x80a2794 (called
  at 0x80a355c).
- There, with the attacker a client (+0x15c) and the entity met a client
  too, `G_AddEvent(attacker, EV_MELEE_BLOOD (37), 0)` (0x80a2820..0x80a284b)
  comes first; then `EV_MELEE_HIT` (35) from `G_TempEntity` (0x80cbd96) with
  `otherEntityNum` (+0x74) the victim's number and +0x9c 1 for a def
  whose +0x304 is set and an attacker with a client (0x80a285e..0x80a2896);
  then, for a victim not the world (1022) that
  takes damage (+0x16b), `G_Damage` (0x80b16c0) with the damage plus
  `G_rand() % 5` (0x80ca308, the `0x66666667` division at 0x80a2919) and
  means 7 (0x80a28cd..0x80a2956).
- `G_AddEvent` (0x80ca0d2) on an entity with a client writes the
  playerstate's ring: `events[seq & 3]` (ps+0xb8), `eventParms` (ps+0xc8),
  `eventSequence` (ps+0xb4) plus 1 masked to a byte, and the entity's
  `eventTime` (+0x184) and +0x158 to `level.time`.

`play-1.7-lunge.txt` rings 31, 37 and 9 on the attacker at 52450 with
`EV_MELEE_HIT` (parm 1, `otherEntityNum` 0, the target) in the same
snapshot. VERIFIED (the capture). `cod4_moves_ab` writes the blood after
the cmd whose `EV_FIRE_MELEE` its row's snapshot shows meeting a client
(an `EV_MELEE_HIT` with `otherEntityNum` under 64), and the pickup at the
frame's start (6.2): all 147 `lunge` rows exact. VERIFIED (the gate).

### 10.5 `kill`

`Cmd_Kill_f`: a playing client dies by its own hand under `CheatsOk`
(`sv_cheats` and health), as `player_die(self, self, self, 100000,
MOD_SUICIDE)` with the death anim and `CodeCallback_PlayerKilled`; without
cheats `CheatsOk` prints `e "GAME_CHEATSNOTENABLED"`, a dead client
`"GAME_MUSTBEALIVECOMMAND"`. INFERRED. Both strings are in `cod4_lnxded`.
VERIFIED (`strings`). Whether 1.7's `kill` asks `CheatsOk` is unmeasured.
`the_kill_command_kills_its_sender`. VERIFIED (the gate).

### 10.6 Live

A scratch run, not kept, whose parts were: vcod-server `--game cod4
mp_crash` with `sv_cheats`; `cod4_probe` `target` (allies, `idle`,
`--goto 207 2291 296`) and `attacker` (axis, `--script frag` from 123 2110
300, or `--script melee` from 180 2260 300). The frag hit the target in
flight (15, `MOD_IMPACT`, `torso_lower`) and burst 3.3 s after the throw,
killing the target (221) and the thrower (199); the melee's first swing
killed the target (137, `MOD_MELEE`). VERIFIED live (the server's `D;`/`K;`
log lines). The scripts start 5 s past the spawn, after the prematch
freeze (`pm_flags` 0x800 masks the keys).

### 10.7 The damage soak

`cod4::damage_soak` hunts a state in which no shot hurts. Two in-process
clients (allies `target` slot 0, axis `shooter` slot 1) on mp_crash `war`
under `sv_cheats` run 33 steps in turn: bullet kills each way, `kill`, all
five stock classes, a team swap, autoassign, the swap back, spectator and
back, a cooked frag held past its fuse, melee kills each way, a firefight
both fire in, a `kill` while the other watches its killcam, a `disconnect`
then a new join (next free slot, the old one still a zombie). After each
step both stand 150 units apart and fire at each other's head box. Every
round hurts on every step, with the cmds on the server's clock and 120 ms
behind it (each shot rewound, section 9). VERIFIED (the gates
`cod4_damage_survives_every_transition` and `..._on_a_lagged_link`).
Every stock map poses both teams' heads with each class's body (urban,
desert, woodland). VERIFIED (`cod4_every_map_poses_both_teams`, ignored:
21 map loads, 29 s of test in release on 2026-11-18).

Two steps follow the stock script's rules. The team menu's answer and the
class's in one packet lose the class, on ours and on retail
(cod4-map-cycle.md 5), so the soak sends the class a second later, as a
click does. A side away past `default_onForfeit`'s 20 s ends nothing:
`updateGameEvents` forfeits only under `level.rankedMatch`. INFERRED (the
script, `_globallogic.gsc`). Ours plays on. VERIFIED
(`cod4_damage_survives_a_long_spectate`).

The one state where nobody hurts anybody is the match's end. At the time
limit `endGame` sets `game["state"]` to `postgame` (the first thing
`Callback_PlayerDamage` returns on), freezes everyone (`pm_flags` 0x800)
for `postRoundTime`, puts everyone in the intermission (`pm_type` 5) and
30 s on (`scr_intermission_time`) calls `exitLevel(false)`. INFERRED (the
script). That queues `map_rotate`, which spawns the next map or restarts
the level (cod4-map-cycle.md 3). On ours a 1-minute `scr_war_timelimit`
match rotates and both players hurt each other on the next level.
VERIFIED (`cod4_damage_survives_the_time_limit` and `..._restarted`).
`war`'s default limit is 10 minutes, so any session that long reaches it.

### 10.8 The flashbang and the concussion grenade

`flash_grenade_mp`: offhand class 3, 1.5 s fuse, `projExplosion` 2
(flashbang), no explosion damage, `explosionRadius` 800,
`explosionRadiusMin` 200. `concussion_grenade_mp`: class 2 (the smoke
slot: it lands before it bursts, as smoke does), 1 s fuse,
`projExplosion` 3 (`EV_CUSTOM_EXPLODE`), inner and outer damage 1 over
512. VERIFIED (`weapon_dump`, `common_mp.ff`).

The flashbang's `G_ExplodeMissile` raises `EV_FLASHBANG_EXPLODE` (48),
then `G_FlashbangBlast`: every client with `takedamage` within the outer
radius whom one of `CanDamage`'s traces (mask 0x801, no cone) reaches is
notified `flashbang` with the share of the way in from outer to inner
radius (1 inside), `(dot + 1) / 2` of the view forward and the
eye-to-burst direction, the thrower (or undefined) and the thrower's team
at the throw. INFERRED. `_flashgrenades.gsc`'s `monitorFlash` (started per
player by `_globallogic.gsc`) clamps the angle share to 0.5..1 (over 0.8
reads 1), takes `distance * angle * 6` seconds, drops a shock under 0.25
s, applies `level.friendlyfire`'s team rules, and a frame later
(`applyFlash`'s `wait .05`) calls `shellShock("flashbang", duration)`.
VERIFIED (the `common_mp` rawfile).

A concussion's one point of splash goes through `Callback_PlayerDamage`'s
`finishPlayerDamageWrapper` into `_weapons.gsc`'s `onWeaponDamage`, which
takes `2 + 4 * (1 - d / 512)` seconds off the distance `d` from the
victim's origin to the grenade's and calls
`shellShock("concussion_grenade_mp", time)` a frame on. VERIFIED (the
script). The grenade's script entity carries the missile's
`r.currentOrigin` in `origin` for that read (ours writes it on each move).

`GScr_ShellShock` (0x80c4a36): exactly two arguments (else `USAGE:
<player> shellshock(<shellshockname>, <duration>)`, 0x821ed44); the name
among configstrings 1955..1969 (`strcasecmp`, else `shellshock '%s' was
not precached`, 0x821eda8); `floor(duration * 1000 + 0.5)`, refused above
60000 unsigned (`duration %g should be >= 0 and <= 60`, 0x821ed80);
writes `ps.shellshockIndex`, `shellshockTime` (`level.time`),
`shellshockDuration` (gclient +0x62c..+0x634); on an entity with health >
0 sets `pm_flags` 0x10000 (`PMF_SHELLSHOCKED`) and runs anim event 0x14
(`ANIM_ET_SHELLSHOCK`). `GScr_StopShellShock` (0x80c49cc) zeroes the three
and clears the flag. VERIFIED (the disassembly).

- The event's stock clauses (`playeranim.script`'s `shellshock` block)
  play `torso pb_stand_shellshock duration 5000 blendtime 50` moving, idle
  and crouching, nothing prone; `BG_AnimScriptEvent` draws `rand()` only
  for a clause with commands (0x8050280). VERIFIED (the script, the
  instructions). vcod runs it at the end frame that writes the flag
  (`Cod4Level::shellshock_anim`), its draw off the level's stream there
  rather than inside the call. INFERRED: no capture orders it against
  another draw. `cod4::shock_tests::a_shellshock_plays_the_torso_shock_anim`.
- `ClientThink_real` clears the flag once `shellshockTime +
  shellshockDuration` is past `level.time`; `PM_CmdScaleForWeapon` takes
  0.4 of the walk under the flag when the record sets
  `bg_shock_movement`. INFERRED.
- Records: `shock/<name>.shock` rawfiles of `bg_shock_*` lines over the
  dvars' defaults (`BG_LoadShellShockDvars`; `shock/default.shock` for a
  missing file). `flashbang.shock` sets `bg_shock_movement` 0,
  `concussion_grenade_mp.shock` 1. VERIFIED (the rawfiles). Float and bool
  defaults (`tools/re/cod4_dvars.py bg_shock`): blur 0.4 s over 1 s, flash
  fades 1 s, kick 0.75 s at 0.05, sound fades 0.25 and 2.5 s, loop fade
  1.5 s, loop end delay -3 s, dry 1, wet 0.5, end delay 2 s, look control
  on at 90/s and 0.5 over 2 s, movement on. VERIFIED. Screen type
  `blurred`, the four aliases `shellshock_loop`, `_loop_silent`, `_end`,
  `_end_abort`, room `generic` and the channel volumes' 0.5 are INFERRED,
  unconfirmed on 1.7.

Ours: `G_FlashbangBlast` over the playing clients in slot order (not the
area tree's), the shellshock builtins as above with the flag written at
the end frame, the 0.4 walk (`vcod_cod4::shock`, `Rules::shock_slows`). A
flashbang 40 units before the target's eye gives the `flashbang` shock for
6000 ms with the flag, which clears when it runs out; a concussion there
gives `concussion_grenade_mp` for the distance's time. VERIFIED (the gates
`a_flashbang_in_the_face_shocks_for_six_seconds`,
`a_concussion_grenade_shocks_by_its_distance`,
`a_slowing_shellshock_walks_at_four_tenths`). Live, a scratch run, not
kept (vcod-server `--game cod4 mp_crash`, a `cod4_probe`
`target` on allies at 207 2291 296, an axis `attacker` with `--script
flash` (secondary offhand key 0x8000) from 123 2110 300, both
`assault_mp,0`): the target shocked for 5286 ms (configstring slot 15)
from 21150, `pm_flags` 0x10000 to the end and clear after. VERIFIED live
(the probe's per-second lines). No retail flashbang capture exists yet.
The client side is cod4-client-view.md 8.

`setNormalHealth` (`ScrCmd_SetNormalHealth`, 0x80c3aa2) on a player caps
its argument at 1, sets health to `floor(maxHealth * frac + 0.5)` when
above 0 (else the console's `ERROR: Cannot setnormalhealth to 0 or
below.`, 0x821ea58), and sends `J "0"` (`%c "%i"` with 0x4a). VERIFIED
(the disassembly). Trap: without it `_healthoverlay.gsc`'s regeneration
thread dies at the first wound (`MissingBuiltin`).

### 10.9 The `rand()` stream

`cod4_lnxded` has one random stream, glibc's `rand()` (PLT 0x804aa5c), and
three `srand`s: `SV_SpawnServer` seeds it with the wall clock's
milliseconds and draws the checksum feed (0x8174afd), then `G_InitGame`
seeds it again with `Sys_MillisecondsRaw()` (0x81d67ac, `tv_sec * 1000 +
tv_usec / 1000` in 32 bits) passed by `SV_InitGameProgs` (0x817bfad,
0x80b56ee). VERIFIED (the disassembly). Every level draw comes after that
second seed.

Callers, VERIFIED by a traced rerun of each capture's recipe (gdb logging
every `rand@plt` with its return address, its caller's and `svs.time`,
under `unshare -rnm`):

| caller | draws | where |
|---|---|---|
| `G_SpawnItem`'s ammo (0x8090072) for each `weapon_*` the map places | `rand() % (max - min + 1)` over `dropAmmoMin..Max` (+1348, +1352), then `rand() % (clip + 1)` when there is a reserve and the clip is not 1; the alt weapon the same when its shared cap index (+836) is negative | the spawn pass; mp_crash's eight `oldschool_pickup`s make 17 (the `g36c_gl` its alt's one) |
| `SV_GetChallenge` (0x816f394) | `(rand() << 16) ^ rand() ^ svs.time`, a new challenge only | each `getchallenge` |
| `BG_AnimScriptEvent` (0x8050280) | `rand() % numCommands` whenever a clause with commands matched | pmove's events (fire 2, jump 3, land 5, ...), `player_die`'s death |
| `BG_WeaponFireRecoil` (0x80609fc) off the fire event's callback (0x80a47b4, events 26, 27 and 38) | four `rand() / 2^31`: the gun's two kicks and the view's two | after the fire's anim draw |
| `Drop_Item`'s launch (0x80915b3) | three `crandom`s: z by `g_dropUpSpeedRand`, then x and y by `g_dropHorzSpeedRand` | after the death anim's draw |
| `G_InitGrenadeMovement`, `MissileLandAngles` (10.2) | the spin's four, one a bounce | the throw, each bounce |

The traces follow the table: the walk run made 17 at the spawn, the
mate's, spectator's and walker's challenges at 17, 19, 21, the walker's
jump and land, then per round (18) one anim draw and four recoil, the
frag's throw anim at 115 and spin at 116; the kill run 17, the two
challenges at 17 and 19, five a round, the death's draw, the drop's three.
The scripts' `randomInt` and `randomFloat` draw nothing from it (no
`rand@plt` call in either run). VERIFIED (the traces). The recoil draws
four on each branch. VERIFIED (the disassembly). That event 38 (a turret's
fire) reaches it rests on the callback's test alone. INFERRED. The name of
+836 is INFERRED from the order. The draw rule on it is VERIFIED (the
disassembly, the `g36c_gl`'s one alt draw).

Seed recovery: `G_InitGame` creates the run's `games_mp.log` in the same
millisecond, and the challenges pin the seed within a search
(`cod4::wire_ab::recover_game_seed`): walk run 1803421502 (challenges at
draws 19 and 21, `svs.time` 41200 and 42200), kill run 1807258603 (17 and
19, 40300 and 41300). The walk's draw 116 is the frag's spin,
`G_flrand(320, 800)` of 0.0492 and `G_flrand(180, 540)` of 0.2158 against
the capture's 296.36 and 102.30 to the bit, 120..123 its four bounces'
turns; the kill's draw 31 is `player_die`'s, 852869521 % 8 = 1, the
clause's `pb_stand_death_headchest_topple` (36) the corpse plays.
VERIFIED (the captures).

vcod keeps the stream on the level (`Cod4HostState::rand`, a
`GlibcRand`), seeded at each load ahead of the spawn pass, and draws it at
every caller above: pickup ammo (draws only; the stock script deletes the
pickups before the baselines), challenges once a level is up, pmove's
anim events (`Pmove::rand`), the four recoil draws (the kick itself is
the client's), drop spreads, the death anim, grenades and the melee's
`G_rand() % 5`. A server seeds each level off its own draw;
`Cod4Server::pin_game_seed` gives a gate retail's. On the pinned seeds
`the_frag_flies_on_ours_as_on_retail` meets the spin and every bounce's
turn, `cod4_combat_ab` the corpse's `legsAnim`, and
`the_challenges_come_off_the_levels_rand` the spectator's and walker's
challenges above `svs.time`'s bits. VERIFIED (the gates).

`AngleNormalize360` (0x8190540): `x = a * 0.0027777778f` on the stack, its
floor stored, `(x - floor) * 360` kept, less 360 at 360 or more; the wall
bounce's spin reads it to the bit. VERIFIED (the disassembly, the gate).

### 10.10 `radiusDamage` and `weaponClass`

- `radiusDamage` (function 0x80bdc12, method 0x80c5e32) both run
  0x80bdab0 with the inflictor in `%eax` (NULL for the function, the
  entity for the method). It reads origin, range, max, min
  (`Scr_GetVector(0)`, `Scr_GetFloat(1..3)`); the attacker from argument 4
  when `Scr_GetNumParam() > 4` and it is defined, else the world entity
  (0x84bbd78); means of death from argument 5, else 14 (`MOD_EXPLOSIVE`);
  weapon from argument 6, else -1. VERIFIED (the disassembly). vcod's CoD4
  level takes the attacker and the method's inflictor and ignores the
  means of death and weapon (no stock call passes them). VERIFIED
  (`cod4::gametype_tests::a_bomb_blast_credits_its_attacker`). Trap:
  CoD1's builtin drops every argument past the fourth, so the `sd` and
  `sab` bombs (`visuals[0] radiusDamage(..., player)`) and a destructible
  car's blast (`self.damageOwner`) kill as the world, and the stock
  `Callback_PlayerKilled` (`attacker.classname`, line 4787) errors on each
  such death. VERIFIED (the map sweep, `cod4-map-sweep.md`).
- `weaponClass(name)` (0x80bd81a) pushes `"none"` (0x8209e27) when the
  name's index (0x80a2182, 0 for an unknown name) is 0, else the class
  name of `weapClass` (offset 0x130). VERIFIED (the disassembly). So
  `checkHit("none")`, which a blast's `Callback_PlayerDamage` runs, reads
  `"none"` (trap: not "unknown weapon"). VERIFIED (the same test).

## 11. The anim script

The server's step runs 1.7's anim script in pmove
(`pmove::player_step_animated`, cod4-player-anims.md 7): the walker's body
to the spectator matches retail's `legsAnim` and `torsoAnim` on all 789
rows of `cod4::wire_ab`'s walk. VERIFIED (the gate). Hit boxes pose off
those anims.

## 12. Open

- A rocket's flight, the live grenade's throw back, a prone death's torso
  and a `killcamentity` killcam are gated against retail captures (15),
  which tolerate nothing: their `GAPS` are empty. The tag and trace read
  gates' few rows are cod4-server-gaps.md's Open list.
- The use key's weapon swap is gated against its retail capture
  (cod4-gsc.md 9.1.1, 9.1.2), the killcam's archived angles in its
  `GAPS`; the mounted SAW's pair is gated, a few ulps in its `GAPS`
  (cod4-gsc.md 9.2.1).
- The terrain triangle normal's last bit: closed. The frag's bounce off a
  terrain triangle takes 1.7's point sweep (cod4-collision.md 3) and
  `FRAG_GAPS` is empty (10.2).
- The corpse's drag uses CoD2's yaw turn shape; no 1.7 capture of a death
  anim whose root turns. No retail capture holds a shellshock's torso
  anim.

## 13. Rounds and blasts on script entities

The stock maps' destructible cars and barrels (`_destructible.gsc`) are
`script_model`s the script opens with `setCanDamage(true)`; they break on
the `damage` notify alone, counting their own health per part.

- `setCanDamage` writes its argument to `takedamage` (`illegal call to
  setcandamage()` at 0x8216904 names its miss). VERIFIED (the string). The
  write is INFERRED (`ScriptEntCmd_SetCanDamage`).
- `G_Damage` on a non-client that takes damage: at least 1 off `health`
  (ent+0x1a0); `g_debugDamage`'s `target:%i health:%i damage:%i`
  (0x82199a4, pushed at 0x80b19c2); the `damage` notify with nine
  arguments (`Scr_Notify` at 0x80b18be: damage, attacker, direction,
  point, means of death, model, tag, part, damage flags); while health
  stays above 0 the handler's pain (table 0x826cd54), else health held at
  -999 at worst (0x80b19e2), `death` notified with the attacker
  (0x80b1a0d), the handler's die (0x826cd58). VERIFIED (the instructions).
  Model, tag and part are the struck attachment's and bone's names, empty
  for the body. INFERRED (the damage notify).
- `G_RadiusDamage` takes every entity with `takedamage` in its box:
  distance to the origin (a brush model's to its bounds), `CanDamage`'s
  five traces to a script model's physics bounds at its origin, unturned
  (middle and four corners across the line), the falloff and flags 5 as on
  a player. INFERRED (`G_RadiusDamage`, `CanDamage`).
- `CanDamage` (0x80b1f60, called from `G_RadiusDamage` 0x80b2f8c at
  0x80b31ab) scores the two kinds apart. A client (+0x15c set) takes a
  third per trace that gets through, all from three on
  (0x80b23a4..0x80b23c4); any other entity takes the whole damage on the
  first clear trace inside the cone and stops there, nothing when none is
  clear (the five tries from 0x80b2bfa, each clear one jumping to
  0x80b2f46, which returns 1). VERIFIED (the disassembly).
  `missile::LineScore` holds both. Trap: scored as a client, a sedan or
  barrel next to a frag takes only a third a line.
- vcod: a frag's or rocket's blast meets such an entity by its model's
  collision surfaces' bounds (`Cod4HostState::model_coll`), unturned, and
  takes the arm above with empty names (`game::cod4::entity_damage`); a
  round meets its collision triangles (13.1).
  `rounds_shoot_a_backlot_sedan_until_it_blows` holds a player's fire on
  an mp_backlot sedan until it burns and blows. VERIFIED (the test).
- The car's explosion is the script's `self radiusDamage(origin, range,
  max, min, self.damageOwner)` (`_destructible.gsc`
  `destructible_explode`), attacker and inflictor as in 10.10; its walk
  damages nearby cars through the same entity arm. VERIFIED (the script).
- With rounds stopping at the cars, six-bot `bot_match` (`dm` and `war`,
  mp_backlot and mp_carentan, 6 000 ticks, seeds 1-12 summed) killed 1 982
  against 2 266 and hit 7 687 rounds against 8 596, mp_backlot `dm` most
  (612 to 470), no script error, suicides alike (16 against 17): the bots'
  sight ignored the cars and fired into those between them and their
  enemy. With the cars in the sight test, 2 255 kills
  (`bot-measurements-cod2-cod4.md` 6). VERIFIED (the counts).

### 13.1 A round through a script entity

- `BG_AdvanceTrace` (0x8060e44) sets the entity the next leg passes to
  `Trace_GetEntityHitId` (0x811a870) of the hit, and moves the start past
  the surface only off the world (0x3fe) with a distance above 0; off any
  entity the next leg starts at the hit. VERIFIED (the instructions). So
  past a body, script model or brush model the penetration loop (3.1)
  traces on with that entity passed, and the trace back to the entry
  passes it too: the entity's thickness takes nothing off the round, which
  goes on while the struck surface's depth is above 0. INFERRED
  (`Bullet_FirePenetrate`, 3.1's call shape).
- A script model's round is `XModelTraceLineAnimated`: each collision
  surface whose bone `hidePart` has not hidden, triangles crossed front to
  back; the struck surface's flags and its bone's name as the hit's
  `partName`, the notify's eighth argument (model and tag empty for the
  base model). INFERRED (the damage notify).
- Capture (`play-1.7-pen-target.txt`, `-shooter.txt`, `-hits.txt`; recipe
  in the last's header): retail on mp_backlot under the `pen_ent` gsc,
  which puts a target 80 units off the right side of the first
  destructible sedan (`vehicle_80s_sedan1_silv_destructible_mp` at 368.2
  -406.3 68, yaw 259) and the shooter 260 off its left, logs the car's
  `damage` notifies and the target's hits, and deals the target no damage;
  the shooter fires six AK-47 rounds sighted. The first three strike the
  near window (29 each, about 393.5 -412 113.6) and stop; the next three
  strike the far door from inside (29 at about 341.3 -403 109.4) and hit
  the target for 28 with `iDFlags` 8. VERIFIED live. The window's part
  breaks on the third (`_destructible.gsc` by part name), so later rounds
  pass where it was.
- vcod: `Bullet_Trace` meets entities open to damage by their collision
  triangles at the bind pose (`DamageBox::trace_surfs`, hidden bones
  skipped) and every solid brush entity through `CM_TransformedBoxTrace`
  (`ClipMap::trace_model`), each with its surface flags; a hit passes its
  entity (`Hit::number`); a script model's notify names the struck bone.
  On the capture ours gives retail's nine lines: times, damage, flags,
  locations, means of death, points within 0.02 units, directions within
  1e-4 (`cod4::shot_aim_ab::rounds_go_through_a_sedan_as_on_retail`; the
  residue is the gsc's `anglesToRight` placement below the log's
  thousandth). VERIFIED (the gate). Trap: a round stopped at the car's
  bounds names no part, so the window never breaks and no round reaches
  the target.
- `SP_script_model` (0x809b8e2, spawn table row at rodata 0x821fa18) sets
  4 in byte +0xf6 (0x809b95d, `svFlags |= 4`) and contents 0x2080
  (0x809b964), then links. `notSolid` (0x8099fd2) writes contents 0 for a
  script or brush model (0x809a053); `solid` (0x809a0ac) writes 0x2080 for
  a script model (0x809a186), 1 for a brush model (0x809a136). VERIFIED
  (the instructions). 0x2080 meets the shot mask 0x2806831 at 0x2000, so
  `SV_PointTraceToEntity` traces every solid script model by its DObj's
  collision surfaces (`DObjGeomTraceline`, svFlags 4), open to damage or
  not, each model of the DObj with its attachments; the notify names the
  struck attachment's model (its configstring) and tag, empty for the base
  model. INFERRED (`SV_PointTraceToEntity`, the damage notify).
- Script model capture (`play-1.7-models-target.txt`, `-shooter.txt`,
  `-hits.txt`; recipe in the last's header): the penetration capture's two
  probes under the `model_shot` gsc, which stands a script model of the
  sedan's own model 150 units off the car's left, between them, in five
  2.5 s phases. Never opened to damage, at the car's height and 30 up, it
  stopped every round (no line in either phase, where a round past it
  would have struck the window). A `tag_origin` model opened to damage
  with the sedan attached at `tag_origin` took the rounds as `damage`
  notifies naming `vehicle_80s_sedan1_silv_destructible_mp`, `tag_origin`
  and the struck bone (`tag_body`, then `tag_door_left_front`); so did the
  same attached with ignoreCollision (the flag keeps nothing out of a
  round's trace). With no stand-in the rounds went on as in the
  penetration capture. VERIFIED live.
- vcod: `Bullet_Trace` meets every linked script model whose contents
  meet the mask, open to damage or not, plus the models `attach`ed to it
  at their tag's bind pose whatever their ignoreCollision
  (`Cod4Level::shot_boxes`, `ModelPart`); only one open to damage takes
  damage, its notify naming the attachment. A hidden part counts on the
  base model only. `notSolid` and `solid` write a script model's contents.
  On the capture ours gives retail's lines up to 69400, where the target's
  capture ends
  (`cod4::shot_aim_ab::rounds_stop_on_script_models_as_on_retail`).
  VERIFIED (the gate). The bots' sight test reads the same list (13), and
  the client's own rounds meet script models too (cod4-effects.md 12). No
  brush model capture (the same `Bullet_Trace`, INFERRED).

### 13.2 Glass

`Bullet_FireExtended`'s only glass rule in 1.7 is the contents bit: a leg
that struck contents 0x10 (0x808a8f4) advances 0.135 (0x3e0a3d71,
0x808a901) and the loop goes on, at most 12 legs. VERIFIED (the
instructions). `cod4_lnxded` has no shatter or breakable glass code (its
strings name glass only as a surface type and a bounce key), and the loop
runs only with `bullet_penetrationEnabled` 0 or a weapon with no
`penetrateType`. VERIFIED (the strings). vcod's `fire_extended` keeps the
same rule; nothing more is open.

## 14. Entity numbers

From the `cod4_lnxded` 1.7 disassembly, checked against the kill capture.

- `G_Spawn` (0x80cbbe8) takes the free list's head (`level.firstFreeEnt`,
  0x836f6d0) when it was freed over 499 ms ago (`level.time` 0x836f8ac less
  `freetime` at +0x184) or `level.num_entities` (0x836f6cc) is past 1021,
  and unhooks it (`nextFree` at +0x270; `lastFreeEnt` 0x836f6d4 cleared
  with an empty list). Else it hands out `level.num_entities` and counts
  it up, `G_Error` at 1022. VERIFIED. That is CoD2's rule
  (cod2-protocol.md 15.1).
- `G_FreeEntity` (0x80cc040) clears the entity (`memset` 0x274 bytes),
  stamps `freetime` with `level.time`, and appends it to the list's tail
  only past byte offset 0xb09f of `g_entities`, so 0..71 (clients, body
  queue) never go on it. VERIFIED.
- `delete()` (`Scr_Delete`, 0x80c3cd6) refuses a client (`Cannot delete a
  client entity`, 0x821c478) and the entity whose think is running
  (`Cannot delete entity during its think`, 0x821ea88, against 0x83724b0),
  notifies `death` (0x80c7604) and tail-calls `G_FreeEntity`: the free is
  inside the call, not CoD1's 100 ms think (cod11-gsc-language.md,
  "`delete()` defers the free"). VERIFIED.
- So the free list holds the level's deletes in script order: on mp_crash
  `war`, `_oldschool::deletePickups` (314..323, 368, 369) ahead of
  `_gameobjects::main`'s other gametypes' objects (80, 99..365). INFERRED
  (the script order; the numbers below agree).
- vcod still frees the entity 100 ms on, so `death` waiters run on a live
  `self` (`_hardpoints::delete_on_death` stops a jet's loop that way), but
  CoD4's `delete()` puts the number on the list's tail inside the call,
  stamped then (`HostProfile::delete_queues_number`,
  `ObjectTable::queue_number`); a number whose entity has not gone yet is
  not handed out. Every entity number `cod4_combat_ab` compares matches
  retail's: flesh events 316 and 318, obituary 319, dropped rifle 320,
  eType 58 events 315 and 317. VERIFIED (the gate). Trap: freeing by
  number at 100 ms gave the kill's first temp entities 99..104 where
  retail's took 315..320.
- What a script sees of a deleted entity inside the frame is unmeasured on
  1.7. `Scr_FreeEntityNum` likely only marks the object dead and leaves
  its threads for later. INFERRED. vcod keeps CoD1's 100 ms.
- A deleted grenade's missile row goes with `delete()`'s unlink, at the
  next missile pass and from the next snapshot, so it neither bursts nor
  is sent in the 100 ms (`Cod4Level::run_missiles`, `missile_entities`).
  The module's own frees go through `GameHost::free_entity`, which drops
  the row and the entity's `setCanDamage` entry.
- The client refusal is ported as a script error
  (`HostProfile::delete_refuses_client`); the think refusal is not, as no
  stock script deletes an entity from its own think. On CoD1 and CoD2 vcod
  notifies `death` on a deleted client and keeps his entity: retail's
  client arm there is unread.

## 15. The capture gates

Four retail runs gate a rocket, a throw back, a prone death and a
`killcamentity` killcam (`cod4::capture_ab`, fixtures
`play-1.7-{rocket,throwback,prone,kcent}-{target,shooter}.txt`). Each is
`cod4_lnxded` 1.7 in `cod4-handshake.md` 7's namespace, `+set g_gametype
war +set sv_pure 0 +devmap mp_crash`, and two `cod4_probe`s 35 and 36 s
after the start: `target` (allies, `--goto "207 2291 296"`) and `shooter`
(axis, `--goto "123 2110 300"`), the scripts and extra `+set`s on each
`Pair`. The seeds came off the two challenges
(`wire_ab::recover_game_seed`, 10.9). Ours replays both probes in process
on retail's clock, seed and spawn spots, a frame's two packets in the
order retail received them, and compares every playerstate
word, health, ammo and every entity by number, both clients, snapshot by
snapshot from the later spawn. Each gate's `GAPS` is empty: ours matches
every row of the five runs (the touch run's is cod4-pmove.md 15.3).
VERIFIED (the gates).

- Rocket: `demolitions_mp,0`'s RPG (weapon 29) from 697 -2038 200, one
  round at yaw 105 pitch -10 that flies 3.5 s, one at the target that
  flies 1.15 s. VERIFIED live. Ours matches every row, the second
  rocket's number and each wobble's `trDelta` included (below). VERIFIED
  (the gate).
- Throw back: the frag aimed 60 under the target's feet (`--aim-z -60`)
  lands short; thrown back, it strikes the shooter's head at 50100 and
  falls to the ground. VERIFIED live. The hint, the pickup, the hold, the
  throw, the strike's event and the fall match word for word; the bounce's
  `trDelta` differs in its third decimal. VERIFIED (the gate).
- Prone death: under `scr_player_maxhealth 30` the target lies down (`--script
  prone`) and the shooter's second round kills it at 46150: the corpse
  reads `legsAnim` 0x34, `torsoAnim` 0x200 (the death flips the prone
  torso's 0, 5) and `eFlags` 0xa000a. VERIFIED live. Ours matches the
  corpse word for word. VERIFIED (the gate).
- `killcamentity`: the `killcam_entity` gsc (in a mod's iwd under `+set
  fs_game`) spawns a script model at 123 2110 500 moving 600 along x over
  12 s, then kills the target through the damage callback with it as the
  inflictor and `cobra_20mm_mp` as the weapon. The killcam frames read
  `killCamEntity` 315, the model's number, and carry it. VERIFIED live.
- The corpse runs (`play-1.7-csb0corpse-*`, `-csb0drag-*`, 5.3) under
  `sv_clientSideBullets 0`: `cod4_corpse_rounds_ab` and
  `cod4_corpse_drag_ab` match every corpse word and every impact, with
  empty `GAPS`. VERIFIED (the gates). What closed the killcam's pitch,
  the target's second spawn and the floor run's dropped rifle:
  cod4-server-gaps.md, Closed.

What the captures fixed on ours:

- `finishPlayerDamage` (0x80ac58e) sets `flinchYawAnim` (ps+0xa8) from the
  blow's `vectoyaw` less the view yaw's whole degrees, both in 0..360: 0
  from 315 or under 45, 2 to 135, 1 to 225, 3 to 315, 0 with no direction
  (0x80ac7b5..0x80ac85a, 0x80acdbd..0x80ace25, rodata 0x8218dc0..0x8218dc8
  and 0x820dec0). VERIFIED (the instructions). The killcam run's kill
  reads 3, the prone hit 1. VERIFIED live.
- `player_die` writes `stats[1]` (ps+0x14c, the dead yaw): `(int)vectoyaw`
  toward the attacker, for a death by one's own hand toward an inflictor
  of another, else the body's own yaw (0x80b1cf5..0x80b1d52,
  0x80b1ddc..0x80b1e12, 0x80b1f44). VERIFIED (the instructions). The
  prone and killcam runs' targets read 245, toward the shooter at 123 2110.
  VERIFIED live.
- `ClientSpawn`'s memset of the whole client (0x80a7cc9, 0x3184 bytes)
  drops `damage_blood` (client+0x30ac), so a killing blow steps no
  `damageEvent` on the next life. VERIFIED (the instructions). Ours
  stepped it at every respawn. VERIFIED (the gates).
- `BG_TakePlayerWeapon` (0x806206c), called with its third argument 1 by
  `takeWeapon` (0x80abbfe), `takeAllWeapons` (0x80abb5c) and
  `Drop_Weapon` (0x809182d, 0x8091885): the bit; the clip; the reserve
  down to what the weapons left keep (its shared cap, a clip-only
  weapon's clip, else the other held users' `maxAmmo` summed, 0 for none);
  then each held alt down the chain with its ammo and clip; the current
  weapon 0 when it was the one. VERIFIED (the instructions). The dead
  target's `weapons[0]` reads 0x800 after `dropWeaponForDeath`, the M16's
  `gl_m16_mp` gone with it. VERIFIED live.
- `BG_PlayerStateToEntityState` (0x8052940) sets the body's `eFlags`
  0x20000 past `pm_type` 6 and 0x40000 under `PMF_SIGHT` (0x10), clearing
  each otherwise (0x8052a54..0x8052a82, 0x8052ca2, 0x8052cb1). VERIFIED
  (the instructions). The sighted shooter's body reads 0x40000. VERIFIED
  live.
- `vectopitch` (0x8192510) and `vectoyaw` (0x8191c22) add 360 or 0 to the
  stored angle (`cmovb` off a stored 0), so a level vector's -0 comes back
  +0. VERIFIED (the instructions). The prone body on level ground reads
  `fWaistPitch` 0 where ours sent -0. VERIFIED live.
- The killcam's archived entities: each `pos.trTime` moves up by the
  round's age: the mover started at 41550 reads 45800 in a killcam 4250
  back, the corpse cloned at 46550 reads 50800. VERIFIED live. Which words
  move, and that the age is the entities' own, not the playerstate's
  `deltaTime`: 7.1.
- The killcam's frames leave out the temp entities the followed client
  was not sent: the victim's own `EV_BULLET_HIT_CLIENT_*` (17 + 42, 43)
  never appears in its killcam. VERIFIED live.
- `EV_BULLET_HIT`'s kill bit (5, `scale` 2) rides `un1`. VERIFIED live.

- The order of a frame's packets. Each cmd is thought on arrival
  (cod4-server-frame.md 2), so a hit lands between the victim's cmds as the
  two clients' packets arrived. In the prone run the shooter's packet came
  first in the frames of both hits: the target hit at 46050 reads velocity
  0, `pm_flags` 0x1, `damageTimer` 700, its next cmds run on the push.
  VERIFIED live. The gates take each packet's capture time, put the
  shooter's clock on the target's by the median gap between the two
  probes' arrivals of the same snapshot (one frame's messages leave
  together, cod4-server-frame.md 4), and send each frame's packets in that
  order. With a fixed target-first order ours read the fresh push, 0x101
  and 750, and the dead body's anim timers 50 high. VERIFIED (the gate).
  The prone kill then lands before the target's last cmds of its frame:
  those cmds kept the rifle `dropWeaponForDeath` took, as a dead client's
  cmd took its weapons from the playerstate, not the script's loadout.
  Fixed: a dead client's cmd takes the loadout as a living one's does.
  VERIFIED (the gate).
- The killed callback's `clonePlayer` reads the pushed velocity (4): the
  killcam run's corpse leaves at 80 along x (`g_clonePlayerMaxVelocity`'s
  cap of the 20mm's 240), on ours too since the push lands inside the
  call. VERIFIED (the gate).

- The dead body's slide. The killcam run's target, pushed to 240 along x
  at its death, read the same velocity as retail's every snapshot, but
  its x ran 43 ulps ahead from 46600 on (217.04066 against 217.04001).
  VERIFIED (the gate). The body stood 0.11 over mp_crash's terrain; its
  first dead cmd (18 ms, `pm_time` running, so `PM_WalkMove` adds the
  slick gravity) slid down onto the floor. CoD2's trace set-up met it at
  fraction 0.4190510, 1.7's at 0.4190263: 1.7 keeps the shifted end on
  the x87 stack, so its sweep is not rounded through `z + 35`
  (cod4-collision.md 2). At the first the plain slide stopped one ulp of
  x short of the step's, the step won and kept vz -14.4, and the next
  cmd's `PM_DeadMove` cut 20 off a length that held it (x 200.04); at the
  second the slide ties, the fallback snaps the body to the ground and
  clips vz to 0. INFERRED (the bumps traced on ours under both set-ups).
  Under 1.7's set-up the body's path matches retail's bit for bit.
  VERIFIED (the gate).

- The spawn's `delta_angles`. The rocket shooter's spawn reads 0.40649,
  -118.30078 and the touch run's walker's 0, 9.00879, each the spawn
  spot's view less the angles of the last cmd before the spawn: (0, -90)
  less (-0.40649, 28.30627), (0, 124) less (0, 114.99390). VERIFIED live
  (the captures' cmds and snapshots). The gap the gates once filed under
  `setviewpos` ran from the spawn to the `setviewpos` half a second later,
  and was the gates' own: before the spawn the replayed probe sent view 0
  over its own `delta_angles` where `cod4_probe` aims its script's view,
  and the spot's yaw came off the spawn snapshot's view, which the wire's
  half short reads -89.99451 for -90. Each probe now sends its twin's cmd
  angles before its spawn and takes the spot's yaw as a whole degree; the
  views, `delta_angles` and bodies then match retail's, and ours's
  `SetClientViewAngle` (cod4-pmove.md 15) stands as it was. VERIFIED (the
  gates).

- The prone killcam's trim, 4200 on retail against ours 4250 and its end
  a frame apart, was `psoffsettime`: the playerstate is asked
  `archivetime - psoffsettime` back, the entities `archivetime` back
  (7.1). Ours asked both at `archivetime`. With 7.1's rule the gate's 14
  rows on the followed shooter's words are gone. VERIFIED (the gate).

The dropped weapon (6.1) matches word for word on both runs, the hit's
`trDelta` included, since the bodies' tags compose through 1.7's skeleton
(cod4-gsc.md 3.2.5); before, the posed tag sat some 3e-5
units off retail's. VERIFIED (the gates, both ways).

What the gates once listed, each closed:

- A grenade's strike on a standing body. The thrown back frag meets the
  shooter at 50100 (`EV_GRENADE_BOUNCE` parm 7, flesh, and the hurt).
  VERIFIED live. `G_RunMissile` traces through 0x80b4020 (`SV_Trace` with
  a point box, the locational flag and `bulletPriorityMap` 0x826ce60,
  0x80973df and 0x809765b), the map ours's `missile_trace` passes too.
  VERIFIED (the instructions). Retail's frag stops near the end of its
  50100 step, at (128, 2122, 285), 13 units ahead of the shooter's origin
  (123, 2110, 236); its bounce, `trDelta` (26.759, 74.923, -171.269) at
  0.2 of the speed, reflects off a normal near (0.348, 0.808, -0.476).
  INFERRED (the reflection). Ours flew on, met the chest a frame later
  and came to rest on the body (ground 1). VERIFIED (the gate). Ours's
  server bodies ran CoD2's angle updater and controllers (0.6 of the view
  on the spine, of which `back_up`'s half goes nowhere), and the step a
  frame before the strike met no broad box of ours, so the controls eased
  only at 50100 and stood short of their goal (3.3). VERIFIED (the code,
  a trace on ours). Under 1.7's updater, broad box and once-a-frame ease
  the shooter, looking 31 down, bends the spine by the whole of it and
  turns the head back up, and the frag enters the `head` box's +y face
  at 50100. VERIFIED (a trace on ours, the gate). The bounce read
  (26.752, 74.908, -171.276), the head composed off quaternions (26.753,
  74.910, -171.278). VERIFIED (the gate, both ways). What parted it, and
  what is left, is 15.1.
- The second rocket's number. A retail run of the rocket recipe under
  gdb, breaking at `G_Spawn`'s return (0x80cbcbf) and `G_TempEntity`
  (0x80cbd96), shows 316 taken 450 ms after the first shot by a temp
  entity, event 3 (`EV_SOUND_ALIAS`) at the shooter, made by
  `playSoundToTeam` (0x80c3edc, its `G_TempEntity` at 0x80c3f35) and freed
  350 ms on (0x80b6118). VERIFIED (gdb). `_battlechatter_mp.gsc`'s
  `reloadTracking` waits on `reload_start` and plays the reload line to
  the shooter's team but the shooter, so neither probe saw it. VERIFIED
  (the script, `common_mp.ff`). `ClientEvents` (0x80a581e, its jump table
  0x8217d98 from event 19) notifies `reload_start` on
  `EV_RELOAD_START_NOTIFY` (19, 0x80a5b0f), `reload` on
  `EV_RELOAD_ADDAMMO` (20, 0x80a5afc), `grenade_pullback` with the
  weapon's name on `EV_PULLBACK_WEAPON` (25) as on `EV_PREP_OFFHAND` (32,
  both 0x80a5ab5) and `detonate` on `EV_DETONATE` (63, 0x80a58e5), each
  through `Scr_Notify` (0x80c7604). VERIFIED (the instructions). vcod
  raised none of the first three; with them the second rocket takes 317.
  VERIFIED (the gate). vcod's pmove raises no `EV_DETONATE`, so that arm
  is not ported. INFERRED (the code).
- The wobble's `trDelta`. 1.7's `AngleVectors` (0x818ea3a) turns each
  angle into radians through the double `M_PI / 180` (0x820a288), stores
  them, and calls `sinf` and `cosf` (plt 0x804a9fc, 0x804a6cc), the
  host's glibc, whose results since 2.28 are not correctly rounded: at the
  yaw 100.63505 (radians 0x3fe0d224) `cosf` gives 0xbe3cfb5e where the
  correctly rounded is 0xbe3cfb5d. VERIFIED (gdb at 0x8097b4e: 78 wobble
  frames' angles and forward from a fresh run, all 78 matching a model on
  the host's `sinf` and `cosf`, 77 the correctly rounded `sin`).
  `vcod_common::libm` ports glibc's two (bit for bit with the host's on 4
  million inputs, its tests), and `x87::angle_vectors_libm` is 1.7's
  `AngleVectors`. 1.7's `vectoangles` (0x8191a72) keeps the pitch's flat
  length a double into `atan2` (`fstpl` at 0x8191afc) where the shared
  one stored a float; the trajectory step (0x8095928) puts
  `vectoangles(trDelta)` into the angles every frame (0x8095bad), so from
  50350 the run's `trDelta` drifted up to 11 ulps. VERIFIED (the gate, both
  ways). `game::cod4::missile` takes both.
- The killcam run's archived view. Retail's killcam frames carry no view
  row, so the follower keeps the yaw 0 of its own `setviewpos`, and the
  gate read ours's 90 against it. VERIFIED live. The rows are considered
  (`otherFlags` 2) but the field writer writes one only changed, against
  the server's own base (cod4-netfields.md 7). VERIFIED (the
  instructions). That base held the target's real view: after its
  `setviewpos` it turned back to its spawn's 90 (`cod4_probe` re-aims
  there), and the archived shooter's view was 90 too, so nothing went
  out. INFERRED. A fresh run under gdb reads the archive's copy of the
  shooter (0x816b8af) holding its live view, and there, the target having
  turned back to -90, the killcam sends the shooter's 124. VERIFIED
  (gdb, the run's capture). With the compare ours's 90 rows are gone.
  VERIFIED (the gate).

### 15.1 The throw back's bounce off the head

- `BounceMissile` (0x80946ba) mirrors the velocity at the contact
  (`BG_EvaluateTrajectoryDelta` at `previousTime + (int)(float)(frame *
  fraction)`, 0x8094702..0x8094737) in the trace's plane: the dot's first
  two products stored as floats, the third sum kept, `-2` times it
  (0x820a24c) along the normal, each component stored (0x8094742..
  0x80947ad). The bounce factor is `parallelBounce + (perpendicularBounce
  - parallelBounce) * (dot / -|v|)` for the surface type (`surfaceFlags`
  bits 20..24 into `WeaponDef` +0x5c0 and +0x634), the stored dot and the
  kept length, times each component (0x8094c16..0x8094c59). VERIFIED (the
  instructions). Ours does the same; replayed on our inputs, a model of
  that arithmetic gives ours's words. VERIFIED (a Python model of the
  instructions against the gate's words).
- So the normal parted it. Solved from retail's words (the reflection keeps
  the length; the frag's flesh factor is 0.2 both ways), retail's normal
  was (0.3478266, 0.8072591, -0.4768118) against ours's (0.3478169,
  0.8072385, -0.4768535): 0.0027 degrees about the shooter's right.
  INFERRED (the reflection). That is half of the 0.0055 the torso
  pitch's swing stood short of twice the view's (62.08923 for 62.09471,
  11303 shorts): ours folded the swings in shorts, CoD1's
  `AngleNormalize360`. VERIFIED (a trace on ours). 1.7's `BG_SwingAngles` (0x804b3ba) folds each
  swing as `x = d * 0.0027777778f` less `floor(x + 0.5)` stored, times
  360, and the angle through `AngleNormalize360` (0x8190540: `x -
  floor(x)` times 360, less 360 at 360) of the float sum; the view yaw
  `BG_PlayerAngles` swings toward goes through the same (0x804c67c).
  VERIFIED (the instructions). With the swing on floats the bounce came
  within ten ulps of retail's. VERIFIED (the gate).
- The server's fold can store 360.0 for a yaw at rest: a swing to 0 off
  an angle a hair over 0 sums to a hair under 0 on the stack, and
  `AngleNormalize360` (0x8190540) tests for 360 before it stores
  (0x8190580..0x819058a), so it returns a hair under 360, stored 360.0.
  VERIFIED (gdb: `BG_SwingAngles` from 0x32ffff13 to 0 returned
  0x43b40000). A dead body's `tag_origin` yaw takes the legs' yaw raw
  (0x804cfe6, the dead test at 0x804d075) and the controls ease linearly
  with no wrap (0x804d5fb..0x804d652). VERIFIED (the instructions). Such
  a yaw swings the control toward 0 and 360 in turn, up to 18 degrees a
  frame: on ours a corpse placed at yaw 360 kicked its ragdoll's neck to
  33 degrees (`cod4_live_ragdolls_hold_still_once_down`). VERIFIED (the
  test). `iw3mp.exe` folds in floats too, every step stored
  (`AngleSubtract` 0x561690, `AnglesSubtract` 0x561530,
  `BG_SwingAngles` 0x4062b0), and its `AngleNormalize360` (0x561610)
  stores `(x - floor(x)) * 360` before it takes 360 off one that reaches
  it, so a hair under 0 comes back 0 and a yaw at rest never reads 360.0.
  VERIFIED (the instructions). Its controls ease by a step stored as a
  float, `(float)frametime * 0.36` (0x407288, `BG_LerpAngles` 0x4070a0),
  as ours did. VERIFIED (the instructions). The client's bodies take
  that arithmetic (`Cod4Arith::Client`), the server's hit poses
  `cod4_lnxded`'s (`Cod4Arith::Server`); the ragdoll test holds still
  under the client's. VERIFIED (the test).
- The controllers read the view and `leanf` off the body's `clientInfo_t`
  (+0x3e8, +0x3e4; 0x804cfcb, 0x804d031), which `ClientEndFrame` copies
  from the playerstate and entity (0x80a6ec7..0x80a6f00), so a trace
  before the frame's end poses the last end frame's aim. VERIFIED (the
  instructions). Their differences go through `AnglesSubtract`
  (0x819397a, the fold above per component; 0x804d019, 0x804d02c).
  VERIFIED (the instructions). Neither moved this run's words. VERIFIED
  (the gate).
- The hit box's normal: `SV_Trace`'s entity clip (0x817dcca) takes the
  body's frame off `AnglesToAxis` of `r.currentAngles` (0x818e910, the
  angles times the double `M_PI / 180` stored, `sinf` and `cosf`),
  moves the line into it (0x819211a) and poses the body; `DObjTraceline`
  (0x81acb18) returns the struck face's sign times that row of the
  bone's weighted matrix (0x81ad4e6..0x81ad615: `xx`, `xy` and `yy`
  stored, `1 - (yy + zz)` kept), and the clip turns it back summed over
  the frame's rows, one store a component (0x817e01b..0x817e075).
  VERIFIED (the instructions). Ours takes both forms (`struck_normal`,
  `entity_normal`); the bounce's z, which the yaw leaves alone, then sits
  one to three ulps from retail's and x and y ten and six. VERIFIED (the
  gate).
- The head bone's matrix: a player's six control bones go down
  `DObjCalcSkel`'s second arm (cod4-gsc.md 3.2.5), turned
  in `tag_origin`'s frame with no anim of their own, where the server
  had posed them as CoD1's `G_DObjSetControlTagAngles` did, a glam quat
  turned into the parent's frame. VERIFIED (gdb; `skel_ab`). Ported
  (`skel::compose_controlled`), the bounce's words moved from (10, 6, 0)
  ulps off retail's to (26, 9, 2). VERIFIED (the gate).
- The controller's arithmetic: 1.7 eases each control by `frametime *
  0.36f` kept on the x87 stack (18.0000007 a 50 ms frame, 0x804d5de),
  not a float's 18.0; the legs' yaw less the view's folds in floats
  (0x804d355); every swing and fold keeps the 64-bit stack
  (`BG_SwingAngles` 0x804b3ba, `AnglesSubtract` 0x819397a). VERIFIED (the
  instructions). Fed retail's `clientInfo_t` inputs, ours's controls
  match retail's on all 255 controller runs gdb dumped, and ours's swing
  all 374 distinct `BG_SwingAngles` calls, word for word. VERIFIED
  (`skel_ab`, `playerpose` tests). Neither moved the bounce's words.
  VERIFIED (the gate).
- The parent chain: the server's bodies sampled their clips through
  CoD1's sampler, each key's quat normalized as a float and the blend
  normalized after. 1.7's walk (`DObjCalcAnim` 0x81b4c8c, the tree at
  0x81b2578) sums each leaf's raw quat shorts times 1/32767 and its
  quantized shifts, weighted, unnormalized, on the stack, and folds a
  node of two or more live children by its weight or by the `0x5f3759df`
  step (cod4-player-anims.md 11). VERIFIED (the instructions). Ported
  (`vcod_cod4::xanimcalc`), it matches all 17392 words of 280 player
  trees gdb dumped (`skel_ab`), and the bounce matches retail's word for
  word: `THROWBACK_GAPS` is empty. VERIFIED (the gates). The locals a bone
  no leaf reaches (the head's `j_neck` and `j_head` among them) did not
  move it: the bounce matches with those left to CoD1's sampler too.
  VERIFIED (the gate, both ways).

## 16. Fall damage

- The arm. `ClientEvents` (`cod4_lnxded` 0x80a5bf9..0x80a5c11 calls its
  per-event body 0x80a581e with the client, the entity, the event and its
  parm) sends events 19 to 63 through the jump table at 0x8217d98; past
  it, an event from 106 to 134 (`EV_LANDING_PAIN_DEFAULT` to
  `_CERAMIC`, 0x80a5841..0x80a5847) on an entity whose `s.eType` (+4) is
  1 (`ET_PLAYER`, 0x80a5849) deals damage; anything else returns.
  VERIFIED (the instructions).
- The amount. A parm over 99 (0x80a5859) takes the float at 0x8217d94,
  0x3f8ccccd (1.1); any other the float at 0x8209644, 0x3c23d70a (0.01),
  times the parm (`fimul`, 0x80a5b22), returning when that is 0
  (0x80a5b33..0x80a5b45). Either is multiplied by the playerstate's
  `stats[2]` (`fimul` of +0x150, 0x80a58a0), the max health
  (cod4-hud.md 19), stored as a float and truncated (`cvttss2si`,
  0x80a58ae). The two products stay on the x87 stack, so only the last
  store rounds: parm 25 of 100 deals 25 where 1.1d's unrounded product
  deals 24. VERIFIED (the instructions).
- The call. `G_Damage` (0x80b16c0, called at 0x80a58d9) with the entity
  as target, no inflictor, attacker, direction or point, `dflags` 0,
  means 11 (`MOD_FALLING`, `modNames`), weapon -1, hit location 0 and
  time offset 0. A client target goes on to `G_DamageClient` (0x80b14f8,
  called at 0x80b1722), which with weapon -1 and neither party takes
  weapon 0 (0x80b1625..0x80b167d), multiplies by `mp_lochit_dmgtable`'s
  hit location 0 (0x836f620, 0x80b158b) and keeps at least 1, then hands
  `Scr_PlayerDamage` (0x80c27d6) the lot: the callback reads undefined
  inflictor, attacker, point and direction, weapon `"none"` and hit
  location `"none"`. VERIFIED (the instructions). That table's row 0 is
  1 (section 10.3, `info/mp_lochit_dmgtable`). INFERRED (the rawfile, not
  read again here).
- vcod: `Cod4Level::client_events` takes the same events to
  `fall_damage` (`game::cod4::combat`), which deals through
  `ScriptRuntime::deliver_fall` as CoD2's does. Before this the arm was
  missing and a CoD4 fall took no health, only the landing stun.
  VERIFIED (the code, `fall_damage_is_1_7s_share_of_max_health`).
- The bots judge a drop by 1.7's `bg_fallDamageMinHeight` 128 and
  `MaxHeight` 300 (cod4-pmove.md's dvar table), not CoD1's 256 and 480
  (bot-navigation.md 3.14). VERIFIED (the code).
