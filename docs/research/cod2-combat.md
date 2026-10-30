# CoD2 1.3 combat: the weapon state machine, shots, damage, deaths, missiles

How CoD2 1.3 fires a weapon, damages and kills a player and flies a
grenade or a rocket, read off the Linux dedicated server (`cod2_lnxded`,
md5 in `tools/re/dump_cod2_gsc.py`) and the Mac 1.3 client
(`cod2mp_mac13_i386`, whose game module keeps its symbols). Addresses are
lnxded's where its symbol table names the function, else the Mac's x86
slice (`Mac 0x...`); Mac rodata was read straight out of the Mach-O
(`__DATA` segment of the i386 slice). CoD1's structure for the same paths
is `cod11-combat.md`.

vcod: `vcod_cod2::pmove::weapon` (section 1), the server's
`game::cod2_combat` and `game::cod2` (sections 2 to 7), CoD2's
`finishPlayerDamage`, `cloneplayer`, `obituary`, `dropItem` and
`radiusDamage` in `game::builtins::cod2`; `game::cod2_blast`,
`cod2_corpse` and `cod2_items`.

## 1. `PM_Weapon`

`PM_Weapon` (Mac 0x7cbb2; lnxded splits it across helpers around
0x80ef1ac) runs once per `PmoveSingle` step. It reads the cmd's buttons
and its weapon and offhand bytes, and keeps its state in the playerstate:
`weapon` ps+0xd4, `offHandIndex` +0xd0, `weaponstate` +0xd8,
`weaponTime` +0x34, `weaponDelay` +0x38, `grenadeTimeLeft` +0x3c,
`weaponRestrictKickTime` +0x40, `fWeaponPosFrac` +0xdc, `adsDelayTime`
+0xe0, `weapons` bits +0x544, `weaponrechamber` bits +0x55c, `weapAnim`
+0x5d0. VERIFIED (the loads and stores, the STABS layout in
`cod2-movement.md` 3).

### 1.1 The states and the keys

`weaponstate` numbers, by what each arm does with it (names vcod's,
INFERRED from the arms): 0 ready, 1 raising, 2 dropping, 3 firing, 4
rechambering, 5 reloading, 6 reloading interrupted, 7 reload start, 8
reload start interrupted, 9 reload end, 10 melee init, 11 melee relax,
12 offhand quick drop, 13 offhand prepare, 14 offhand hold, 15 offhand
throw, 16 offhand done, 17..22 the binoculars. VERIFIED, the values the
arms store.

The cmd's `buttons` it tests: 0x1 attack, 0x4 melee, 0x10 reload, 0x1000
the sight, 0x10000 the frag key and 0x20000 the smoke key (the
`offhandClass` 1 and 2 the key looks for), 0x4000 the binoculars, and
0x30015 as "any weapon key". VERIFIED masks; the key names are INFERRED.
`pm_flags` it owns: 0x10 an offhand in hand, 0x8 a reload the script
asked for, 0x2000 the melee key's latch, 0x4000000 weapons disabled.
VERIFIED masks.

### 1.2 The order of one step

INFERRED, the code's order (Mac 0x7cbb2):

1. `pm_flags` 0x1000 returns at once; `pm_type` above 5 zeroes `weapon`
   and returns; a gunner (`eFlags & 0x300`) returns.
2. `PM_UpdateAimDownSightLerp` (Mac 0x7c762), the breath hold.
3. A cooked offhand grenade with `cookOffHold` counts `grenadeTimeLeft`
   down; at 0 its clip loses one and `EV_GRENADE_SUICIDE` (197) goes out.
4. `weaponRestrictKickTime` and `weaponTime` count down by the step's ms.
   A `weaponTime` run out holds at 1 for a `semiAuto` weapon while attack
   stays down on the same weapon with rounds in the clip, which is what
   makes a semi-automatic need a release; states 3, 4, 10 and 11 go ready
   there. `weaponDelay` counting out marks the step "delay fired".
5. The offhand keys: the cmd's offhand byte is taken when held; a frag or
   smoke key picks the first held weapon of that `offhandClass` with
   rounds, raises `EV_SWITCH_OFFHAND` (167) with its index, and from a
   weapon in hand starts the quick drop (state 12, `quickDropTime`,
   `EV_RESET_ADS` 149), from empty hands the prepare (state 13,
   `holdFireTime`, `EV_PREP_OFFHAND` 165). None found raises
   `EV_EMPTY_OFFHAND` (148). VERIFIED, the stores and events.
6. The switch to the cmd's weapon byte, when the weapon is idle enough
   (`weaponTime` 0, or a reload or rechamber, or no delay outside 3, 4,
   10, 11), not under `pm_flags` 0x8000: `PM_BeginWeaponChange` (Mac
   0x7c110) drops with `EV_PUTAWAY_WEAPON` (155), or `EV_WEAPON_ALT` (156)
   onto the alt weapon, for `dropTime` or `altDropTime`. A weapon no longer
   held is dropped to 0. VERIFIED, the stores. On a ladder the change is
   to 0 whatever the byte says (1.7). VERIFIED, the decompiles.
7. The reload: the reload key with `PM_Weapon_AllowReload` (Mac 0x7ac60),
   or an empty clip with reserve outside the firing state and not prone
   and moving, runs `PM_BeginWeaponReload` (Mac 0x7c5a2). Attack pressed
   during a segmented reload marks it interrupted (5 to 6, 7 to 8).
8. The melee key (`meleeDamage` non-zero): `EV_MELEE_SWIPE` (163), and
   `EV_FIRE_MELEE` (164) at once when `meleeDelay` is 0, else after it.
9. A bolt-action weapon owing a rechamber: from ready, state 4 for
   `rechamberTime` with `EV_RECHAMBER_WEAPON` (161) and the bolt at
   `rechamberBoltTime`, where `EV_EJECT_BRASS` (162) goes out.
10. Prone and moving, or meleeing, sets `aimSpreadScale` to 255.
11. With either timer running and no delay fired, return; else the
    closing switch on the state.

### 1.3 The shot

The ready arm (lnxded 0x80ef58a with 0x80ef1ac, 0x80ef380, 0x80ef496,
0x80ef50a, 0x80ee95e), VERIFIED off the stores and calls: attack up and
no delay fired leaves the state ready. Else a bullet or projectile weapon
sets `weaponDelay` to `fireDelay` and `weaponTime` to `fireTime` (an
`adsFire` weapon's delay is `(1 - fWeaponPosFrac) / adsTransInRate`),
marks the rechamber for a bolt action, and on the first shot of a burst
sets `weaponRestrictKickTime` to `fireDelay + fireTime * reducedKickBullets`
(the ADS or hip count by `fWeaponPosFrac >= 1`). A grenade weapon pulls
the pin instead: `grenadeTimeLeft` `fuseTime`, `EV_PULLBACK_WEAPON` (157),
delay `holdFireTime`. State 3.

An empty clip clicks (`EV_NOAMMO` 146, `weaponTime` + 500) or reloads.
With rounds and no delay the clip loses one (not on a turret), `weapAnim`
plays the hip or sighted fire anim (2/3 or 5/6, last shot), and
`EV_FIRE_WEAPON` (158) or `EV_FIRE_WEAPON_LASTSHOT` (160) goes out;
off the sight `aimSpreadScale` grows by `255 * hipSpreadFireAdd`; a
`clipOnly` weapon left with nothing is taken (`EV_NOAMMO`).

Since the ready arm sets `weaponDelay` from `fireDelay` on every pass,
a weapon with a non-zero `fireDelay` would never get past it. No stock
MP weapon sets one. INFERRED, the stock files.

### 1.4 Reloads

`PM_SetReloadingState` (Mac 0x7c3d6), VERIFIED stores: an empty bullet
weapon plays `reloadEmptyTime` with `EV_RELOAD_FROM_EMPTY` (151), else
`reloadTime` with `EV_RELOAD` (150), state 5 (6 after an interrupted
start); `weaponDelay` is the add time, `reloadAddTime` when shorter, or a
bolt action's `rechamberBoltTime` first when it owes one. When the delay
fires `PM_Weapon_ReloadDelayedAction` (Mac 0x7b114) ejects the owed
brass or runs `PM_ReloadClip` (Mac 0x7aba4): `min(clipSize - clip,
reserve)` rounds, `reloadAmmoAdd` a segment, `reloadStartAdd` at a
segmented start. A segmented reload loops a segment while
`PM_Weapon_AllowReload` holds and ends with `reloadEndTime` in state 9
and `EV_RELOAD_END` (153).

`PM_Weapon_AllowReload` (Mac 0x7ac60) refuses the reload key with no
reserve or a full clip, and for a weapon with `noPartialReload` it also
refuses while fewer rounds are missing than one `reloadAmmoAdd` segment,
or, when that segment is 0 or a whole clip, until the clip is empty.
VERIFIED (the compares). The stock Enfield (`noPartialReload 1`,
`reloadAmmoAdd 5`, `clipSize 10`) takes the key only with 5 or more
rounds spent, and the M1 Garand (`noPartialReload 1`, `reloadAmmoAdd 0`)
never: it reloads only on the empty clip, which reloads on its own.
VERIFIED (the stock files). A retail 1.3 capture on mp_toujane under
`sv_pure 1`: an Enfield at 8 of 10 holding `+reload` (button 0x10) for
100 ms in `weaponstate` 0 stays in state 0 with its clip, and a Thompson
(`noPartialReload 0`) at 12 of 20 goes to state 5 (`weapAnim` 0xb,
toggled) on the first cmd with the bit and lands 8 rounds about 1.35 s
later. VERIFIED. vcod-server matches both runs; the net probe's
`VCOD_COD2_TRACE_WEAPON=1` prints the weapon state, clip and reserve per
snapshot. A key held in state 4 (rechambering) reloads on neither.

### 1.5 The offhand throw

VERIFIED stores: state 12 ends in 13 (`holdFireTime`, `pm_flags` 0x10,
`EV_PREP_OFFHAND` with the offhand), 13 in 14 with `grenadeTimeLeft`
`fuseTime`; 14 holds while either offhand key stayed down this cmd and
the last (`weaponDelay` 1, asked again next step), and on release goes
to 15 for the offhand's `fireTime` with `EV_USE_OFFHAND` (166), which
`ClientEvents` turns into the throw. 15 raises the weapon again
(`quickRaiseTime`), 16 goes ready with `pm_flags` 0x810 cleared.

### 1.6 The sight and the spread

- `PM_UpdateAimDownSightFlag` (Mac 0x7bc60): `pm_flags` 0x40 follows the
  sight key alive (`pm_type` 0 or 1) on an `aimDownSight` weapon not
  raising, dropping, meleeing or on an offhand; prone it is 0x840, and
  only on the key's press or a still body. VERIFIED, the tests.
- `PM_UpdateAimDownSightLerp` (Mac 0x7c762): `fWeaponPosFrac` moves by
  `msec * adsTransInRate` toward 1 while sighted and back by
  `adsTransOutRate`; a reload past `adsReloadTransTime` and an unsighted
  rechamber hold it at the hip, an `adsFire` delay pulls it in.
  `player_adsExitDelay` (0) can hold it at 1. VERIFIED, the tests and
  stores.
- `PM_AdjustAimSpreadScale` (lnxded 0x80ee9ee): `aimSpreadScale` +=
  255 * (inc - dec), clamped to 0..255; dec is `hipSpreadDecayRate`
  (halved airborne, times `hipSpreadDuckedDecay` crouched,
  `hipSpreadProneDecay` prone) times the frame; inc, off the sight, is
  0.01 * |turn| * `hipSpreadTurnAdd` / frametime per axis, plus
  `hipSpreadMoveAdd` moving faster than `bg_aimSpreadMoveSpeedThreshold`
  (11), plus 2 * 1.28 airborne, times the frame. VERIFIED, rodata
  0x8155764..0x8155774 and the dvar's registration (0x41300000).
- `BG_GetSpreadForWeapon` (lnxded 0x80ec8cc): the hip min and max lerped
  stand to crouch by `(viewHeight - 40) / 20` above 40, crouch to prone
  by `(viewHeight - 11) / 29` below. VERIFIED, rodata 0x8155714..0x8155720.

### 1.7 The ladder

- `PM_Weapon_CheckForChangeWeapon` (lnxded 0x80eef06, Mac inline in
  `PM_Weapon` 0x7cbb2) asks for weapon 0 through `PM_BeginWeaponChange`
  while `Mantle_IsWeaponInactive` (lnxded 0x80de5d4, Mac 0x9e814) holds,
  or `pm_flags` 0x20 (on the ladder) or 0x4000000 (weapons disabled) is
  set, before it reads the cmd's weapon byte. VERIFIED, the tests and
  calls in both decompiles.
- `PM_FinishWeaponChange` (lnxded 0x80ede6c, `PM_Weapon`'s state 2 arm on
  Mac) brings up weapon 0 under the same three tests. VERIFIED, same.
- So taking a ladder drops the gun (`EV_PUTAWAY_WEAPON` 155, `weapAnim`
  9, state 2, `dropTime`), and the drop's end raises the empty hands:
  `weapon` 0, state 1 and `EV_RAISE_WEAPON` (154), since the raise event
  goes out whenever the old weapon was not 0 and the new one is not its
  `altWeapon`. Nothing fires: the ready arm needs a weapon. Off the rungs
  the cmd's weapon byte (the client's selection, never cleared) asks
  for the gun again; from weapon 0 `PM_BeginWeaponChange` only sets state
  2 with no time, and the finish raises it with no event (`weapAnim` 10,
  state 1, `raiseTime`, `aimSpreadScale` 255). VERIFIED, the `ladder`
  capture (`cod2-movement.md` 17.1): 0x9b and state 2 as 0x20 comes on,
  0x9a with `weapon=0` 211 ms later (kar98k `dropTime`), `weapon=3`,
  state 1 and `weapAnim` 0x20a with no event as 0x20 goes.
- A semi-automatic's held trigger keeps any running `weaponTime` at 1
  (step 4 of 1.2, which tests only the offhand states), so an attack held
  on the rungs stops the drop one ms short and the hands never empty;
  still nothing fires. INFERRED from the countdown's tests (Mac
  0x7cbb2); not captured.
- vcod: `pm_weapon`'s switch check and its state 2 arm test
  `mantle::weapon_inactive` and `pmf::LADDER` beside the disabled flag.
  `moves_ab` runs the weapon half beside the move and matches the
  `ladder` capture's weapon fields and events in every row
  (`cod2-movement.md` 17.2), and the mantle captures' (9): the putaway
  and the empty-handed raise on `mantle` and `mantle_crouch`, the gun
  kept up on `mantle_turn`'s low row. VERIFIED (the gate).
  `game::cod2_ladder`'s tests replay the climb on vcod-server: the
  putaway and the empty-handed raise twice, the rifle back at the end,
  no shot with the attack held on the rungs. VERIFIED (the tests).
- The client predicts no weapon (`cod2-movement.md` 18): the view weapon
  plays the snapshot's `weapAnim` (9 the drop, 10 the raise) on its
  `weapon`, and draws nothing for weapon 0, and fire effects come from
  the snapshot's events, so the server's state is all it needs.
  INFERRED (`cod2_viewmodel.rs`, `play/cod2_events.rs`).

### 1.8 The binoculars

`PM_Weapon` (Mac 0x7cbb2) runs a binocular block after the two timers'
countdown and before the offhand keys. VERIFIED, the stores and tests in
the Mac decompile unless labelled.

- The binocular weapon is the first `bg_weaponDefs` row whose
  `weaponType` is 3 (`binoculars`); the block does not test that the
  player holds it. VERIFIED, the loop. Stock `binoculars_mp`:
  `weaponClass` `item`, `weaponSlot` `none`, `aimDownSight` 1, no ammo.
  VERIFIED, the asset.
- The block runs in states 17..22, or in state 0 when the weapon is not
  down a scoped sight (`fWeaponPosFrac` above 0 with an
  `adsOverlayReticle`) and no key of 0x30015 (fire, melee, reload, frag,
  smoke) is held. VERIFIED.
- Inside, a weapon or offhand byte that differs from the last cmd's, a
  key of 0x30015, or a non-zero `ps+0x128` drops the binoculars at once:
  `pm_flags` 0x810 off, `fWeaponPosFrac` 0, `EV_RESET_ADS` (149),
  `pm_flags` 0x20040 off, `EV_BINOCULAR_EXIT` (169), both timers 0,
  state 0, `weapAnim` 0, and a held fire key leaves `weaponTime` 1.
  VERIFIED; what `ps+0x128` holds is not read.
- Else, without the fire key: in state 20 the use keys (0x28) raise
  `EV_BINOCULAR_FIRE` (170) on their press and `EV_BINOCULAR_RELEASE`
  (171) on their release, against the last cmd. VERIFIED.
- `player_toggleBinoculars` (default 1, `Dvar_RegisterBool`, Mac decompile
  line 72207) makes `+binoculars` (0x4000) a toggle: its press goes in
  from outside 17..22 and out from 18..20. With 0 the held key goes in
  and its release goes out. VERIFIED, the tests.
- In: with a cmd weapon byte of 0, state 19 at once, `weaponTime` the
  binoculars' `adsTransInTime` (+0x2c0), `pm_flags` 0x20000 and
  `EV_BINOCULAR_ENTER` (168); with the sight key held, `EV_RESET_ADS`,
  `pm_flags` 0x40 off, `weaponDelay` the weapon's `adsTransOutTime`
  (+0x2c4) plus `player_adsExitDelay`, state 17; otherwise state 18,
  `weaponTime` the weapon's `quickDropTime` (+0x240), `weapAnim` 0x11.
  VERIFIED.
- Out: from 18 (the weapon still going down) state 22, `pm_flags`
  0x20000 off, `EV_BINOCULAR_EXIT`, `weaponTime` the weapon's
  `quickRaiseTime` (+0x244) and `weapAnim` 0x12, or with a cmd weapon
  byte of 0 `weaponDelay` 1; from 19 or 20 state 21, `weaponTime` the
  binoculars' `adsTransOutTime` plus `player_adsExitDelay`,
  `EV_BINOCULAR_DROP` (172). VERIFIED.
- The closing switch on the state once the timers run out: 17 goes to
  18 as above; 18 to 19 with `pm_flags` 0x20000 and `EV_BINOCULAR_ENTER`;
  19 to 20; 20 stays; 21 to 22 with `EV_BINOCULAR_EXIT` and the weapon's
  raise; 22 (as 16) runs the drop's tail above, so the way out raises
  `EV_BINOCULAR_EXIT` twice. VERIFIED, the switch's arms 0x10..0x16.
- The other arms (offhand keys, weapon switch, reload, melee, rechamber)
  skip states 17..22 (`5 < weaponstate - 0x11`). VERIFIED.
- `BG_GetViewmodelWeaponIndex` (Mac 0x7b226) draws the binoculars while
  `pm_flags` 0x20000 is set, and `PM_IsBinocularsADS` (Mac 0x7a04a) is
  states 19 and 20, where the client sends the sight bit by itself
  (`cod2-protocol.md`, the button bits). VERIFIED.

vcod: `pmove::weapon::binoculars` and `binocular_state`, the sight and
the lerp on the binoculars' def while 0x20000 is set (`view_weapon`).
Not done: the hold mode, the `ps+0x128` test, the breath hold that reads
the binoculars' def under 0x20000. `ClientEvents` turns 168..172 into the
`binocular_*` notifies (`game::cod2::binocular_notify`). Tests:
`the_binoculars_come_up_look_and_go_down`, `the_fire_key_drops_the_binoculars`
(`pmove::weapon`), `the_binoculars_come_up_on_the_server`
(`game::cod2_gaps`, `COD2_DIR`).

## 2. Firing

### 2.1 `ClientEvents`

`ClientEvents` (Mac 0x1a30ec) walks the cmd's new playerstate events,
VERIFIED off its switch: 158, 159, 160 and 175 run `FireWeaponAntiLag`
with the client's lag time (`gclient+0x289c`) under `g_antilag`, else
`level.time`; 164 runs `FireWeaponMelee`; 166 `G_UseOffHand`; 168..172
the binocular notifies; 197 `player_die` with `MOD_SUICIDE` and 100000;
116..138 (the landing events) `G_Damage` with `MOD_FALLING` (0xb) and
`maxHealth * parm / 100`, all of it from a parm of 100 up.

### 2.2 The muzzle and the aim

`G_CalcMuzzlePoints` (lnxded 0x81207ec): `AngleVectors` of
(`gclient+0x27d8`, `gclient+0x27dc`, `ps.viewangles[2]`) and
`G_GetPlayerViewOrigin`. VERIFIED. `ClientThink_real` (Mac 0x1a3d00)
stores the two from `ps.viewangles` plus the view bob's angle offsets.
VERIFIED, the stores. It does so before `Pmove`, so a shot leaves along
the view before the cmd's own turn. INFERRED, the call order. 9.6 has the
whole block, which vcod runs (`vcod_cod2::pmove::aim`).

`G_GetPlayerViewOrigin` (Mac 0x193462, lnxded 0x80f8916): the origin,
raised `viewHeightCurrent`, plus the vertical bob and the horizontal bob
along the view's right, both capped at `bg_bobMax`, leaned
(`AddLeanToPosition` with 16 and 20), held at least 8 (rodata 0x8157e60)
over the origin. VERIFIED, the calls and the float. vcod:
`Cod2Level::player_view_origin`, for the shot, the offhand throw and the
look-at trace.

`FireWeaponAntiLag` (lnxded 0x8120870), VERIFIED: a gunner on a turret
does not fire through it. The spread is the hip's min, or `adsSpread` at
`fWeaponPosFrac` 1, lerped toward the hip's max by `gclient+0x2810`,
which `ClientEndFrame` (Mac 0x1a47a8) sets to `aimSpreadScale / 255`.
`weaponType` 0 goes to `Bullet_Fire`, 1 to `weapon_grenadelauncher_fire`,
2 to `Weapon_RocketLauncher_Fire`.

### 2.3 `Bullet_Fire`

lnxded 0x8120484, VERIFIED: `G_AntiLagRewindClientPos` (0x811fc76) moves
every connected, playing client to `SV_GetClientPositionAtTime` at the lag
time, when `g_antilag` is on and the lag is more than one frame
(`1000 / sv_fps`); `G_AntiLag_RestoreClientPos` (0x811fdec) puts them
back after the last round (2.3.2). A spread weapon (`weaponClass` 3) fires `shotCount` pellets
out to `minDamageRange`; everything else one round out to 8192.

`Bullet_Endpos` (0x811fac8): `range` along forward, plus right and up
offsets of `tan(spread) * range * r` at a random angle, `r` a second
random draw (0x811fa70: the angle `360 * G_random()` stored, the
fraction `G_random()` stored, the right offset `r * cos`, the up `r *
sin`, each product stored, each `VectorMA` component stored). VERIFIED,
the disassembly.

### 2.3.1 The rewind needs the archive

`G_AntiLagRewindClientPos` (lnxded 0x811fc76) does nothing unless
`g_antilag` reads true (`DAT_087a24cc+8`), VERIFIED.
`g_antilag` is registered as a bool defaulting to 1 with flags 0x1005
(lnxded 0x8111842), VERIFIED.
It then walks every slot with `pers.connected` 2 (gclient+0x26c4) and
`sessionState` 0 (gclient+0x26a8), the shooter included, VERIFIED.
For each one the lookup at 0x8099dac (`SV_GetClientPositionAtTime`)
answers, it saves `r.currentOrigin` (gentity+0x138), unlinks, writes the
answer and relinks, VERIFIED.

The lookup, VERIFIED off 0x8099dac and the ints at 0x817db80 (2) and
0x817db84 (1):

- `frame` is `1000 / sv_fps`, `now` is `svs.time` cut down to a
  multiple of `frame`, and `back` is `(now - time) / frame`.
- The older bracket asks `SV_GetArchivedClientInfo` (0x809a0b6) at an age
  of `(back + 2) * frame`, adding a frame after each failure, ten tries.
- The newer bracket asks at `(back + 1) * frame`, taking a frame off
  after each failure, ten tries.
- Each ask passes the age by pointer, so the archive's clamps
  (`SV_GetCachedSnapshot`, 0x8099c04) rewrite it before the next step
  and before the lerp reads it.
- With both found, the answer is `newer * f + (1 - f) * older` (0x809bf50,
  on the x87) with `f = (time % frame) / (older_age - newer_age)`. On a
  time that is a whole frame `f` is 0, so it answers with the frame
  before that time's own.
- With one found, the other is cleared (0x809bf66) and `f` is 1 or 0,
  so the one found answers as it is.
- With neither found it answers nothing and that client stays put.

`SV_GetCachedSnapshot` finds a frame only while `DAT_084230a8` is set,
VERIFIED.
`SV_EnableArchivedSnapshot` (0x8093312) is the only function that sets
that flag, VERIFIED.
`SV_EnableArchivedSnapshot` is reached only from `GScr_SetArchive`
(0x81178ea), VERIFIED, its one call in the lnxded decompile.
`SV_InitArchivedSnapshot` (0x809338e) clears the flag, and
`SV_SpawnServer` (0x8093520) and `SV_MapRestart` (0x808be3e) call it,
VERIFIED.
The archive write (lnxded 0x809b016, `SV_ArchiveSnapshot`, Mac 0x194da8)
keeps no frame while the flag is clear (the test
of `DAT_084230a8` before its write), VERIFIED.

Without the flag `SV_GetArchivedClientInfo` answers only for an age of
0 or less, from the live client: `CS_ACTIVE` and `pm_flags` 0x800000
(`GetFollowPlayerState`, lnxded 0x80f6dea), VERIFIED.
Only the newer bracket's walk reaches an age of 0, and only when `back`
is 8 or less, INFERRED from the arithmetic above.
So with the archive off the rewind puts a client back where his
playerstate already is, and from nine frames back it moves no one,
INFERRED.

The stock gametypes turn the archive on only through `_killcam.gsc`:
`init()` calls `setarchive(true)` when `scr_killcam` is above 0, and
`updateKillcamSettings()` calls `setarchive(true)` or `setarchive(false)`
on a change of `scr_killcam`, keyed on `scr_killcam > 0 || g_antilag > 0`,
VERIFIED, `maps/mp/gametypes/_killcam.gsc` in the stock iwds.
`dm`, `tdm`, `sd`, `ctf` and `hq` call `_killcam::init`, VERIFIED.
So a stock server started with `scr_killcam 0` has no rewind until that
cvar changes, INFERRED.
No `probe_*.gsc` under `crates/cod2/tests/fixtures/` calls `setarchive`,
VERIFIED, so no committed retail capture shot through a rewind.

vcod: `Cod2Level::rewound_origin` runs `cod2_archive::position_at_time`
over the archive's `client_info`
(`the_rewind_reads_the_archive_and_without_it_the_live_origin`), and
`bullet_fire` moves the bodies there for the shot (2.3.2).

### 2.3.2 What the rewind moves, and for how long

- The move writes `r.currentOrigin` (gentity+0x138) alone, between
  `SV_UnlinkEntity` (0x809c596) and `SV_LinkEntity` (0x809c63a), so the
  link box follows it, VERIFIED, 0x811fc76.
- Neither `ps.origin` nor `s.pos` is written there, VERIFIED, the same
  function's stores.
- The saved origins sit in a 0x340-byte buffer on `Bullet_Fire`'s stack,
  twelve bytes a slot from 0 and a flag byte a slot from 0x300, VERIFIED.
- `Bullet_Fire` (0x8120484) rewinds before its pellets or its one round
  and restores after the last, so every `G_Damage` of the shot, with the
  damage and killed callbacks it runs, sees the rewound origins, INFERRED,
  the call order.
- `G_AntiLag_RestoreClientPos` (0x811fdec) writes each flagged slot's
  saved origin back between an unlink and a link, over whatever the shot's
  callbacks left there, VERIFIED.
- The restore tests `g_antilag` again first, VERIFIED.
  A callback that clears it leaves the clients rewound, INFERRED.
- `weapon_grenadelauncher_fire` (0x8120552) and
  `Weapon_RocketLauncher_Fire` (0x812061a) never call the rewind,
  VERIFIED, their callees.
- The script's `origin` is `r.currentOrigin`: the entity field row at
  lnxded 0x815e830 names `origin` at offset 0x138, type 4, setter
  0x8117e62 (`Scr_SetOrigin`), VERIFIED.
- `Drop_Item` (Mac 0x1b1128) launches from `r.currentOrigin`, VERIFIED.
  A weapon a callback drops inside the shot leaves from the rewound spot,
  INFERRED.
- `CanDamage`'s client arm (Mac 0x1c3f18) takes the eye from
  `G_GetPlayerViewOrigin` (the playerstate) and the feet from
  `r.currentOrigin`, VERIFIED.
  A `radiusDamage` inside the shot measures the rewound feet under the
  live eye, INFERRED.
- `PlayerCmd_ClonePlayer` (Mac 0x1c3574) sets the clone at `ps.origin`
  (gclient+0x14), VERIFIED.
  A corpse cloned inside the shot falls from the live spot, INFERRED.

The retail capture (`crates/cod2/tests/fixtures/antilag/`, recipe in its
header) runs `probe_antilag.gsc` with the archive on: a shooter whose cmd
clock runs 100 ms behind holds a ppsh's trigger on a strafing target. Of
its 34 rounds, 26 have the damage callback's `self.origin` more than a
unit off the target's own snapshot origin of that frame, up to 19 units,
VERIFIED, the capture.
With the target standing still the two agree, VERIFIED, the capture.

vcod: `Cod2Level::antilag_move` writes the answer into the blast body's
origin and the script's `origin` and keeps `BlastBody::ps_origin`, which
`cloneplayer` reads; `antilag_restore` writes the saved pair back under
`g_antilag`. `cod2_antilag_ab` replays the capture: all 34 rewound
origins within 0.011 of retail's, 29 hit locations alike, the other five
the hit pose's (9.5, its `GAPS`). `a_shot_moves_its_victim_back_until_the_rounds_are_done`
(`game::cod2_gaps`) pins the move and the restore.

### 2.3.3 A rewind past the archive's start

- The lerp's weight is `(float)(time % frame) / (float)(older_age -
  newer_age)`: two `fildl` and an `fdivrp` at lnxded 0x8099ffc..0x809a013,
  with no test of the divisor on the path that finds both brackets,
  VERIFIED.
- 0x809bf50 then returns `(1 - f) * older + f * newer`, VERIFIED, its
  `fld1`, `fsubs`, `fmuls`, `flds`, `fmuls`, `faddp`.
- `SV_GetCachedSnapshot` (0x8099c04) clamps an age past the archive's
  first frame to that frame's age, and with no frame kept from there on
  sets the age to 0, VERIFIED.
- `SV_GetArchivedClientInfo` (0x809a0b6) answers an age of 0 from the live
  client, VERIFIED.
- So both brackets land on one age when the archive holds `back + 1`
  frames or fewer, `back` being 2.3.1's: both clamp to the first frame,
  or both fall to the live client at 0, INFERRED, the arithmetic.
- The archive's frame count (`DAT_084230ac`) counts only frames written
  while `setarchive` is on, VERIFIED, the write's test at 0x809b016.
- `SV_InitArchivedSnapshot` zeroes the count and the flag, VERIFIED.
- The window opens after a map load or a `map_restart` (the stock
  `_killcam::init` turns the archive back on in the new level), and after a
  late `setarchive(true)` in a level that ran without it, INFERRED.
- A shot in it whose lag reaches that far back divides by 0: 0 / 0 when
  `time` is on a frame, x / 0 otherwise, and the lerp of `inf` turns
  that into NaN as well, INFERRED, x87 arithmetic with the invalid and
  divide exceptions masked (an unmasked one would stop the server with
  SIGFPE; no control word the engine loads was read).
- The ages hang on the archive's count and `back`, not on the client, so
  every rewound client of that shot is written NaN together, the shooter
  included, INFERRED.
- A body linked at NaN fails every bounds comparison and meets no round,
  INFERRED, not read past `SV_LinkEntity`: the shot strikes the world
  only, no damage callback runs, and the restore puts every origin back.

vcod: `cod2_archive::position_at_time` answers `Rewind::Nowhere` for two
brackets at one age; `bullet_fire` leaves such a client where he stands
and its trace passes him (`Cod2Level::locational_trace`), so no NaN
reaches a link, a trace or the wire, INFERRED as the closest match.
Tests: `a_rewind_past_the_archive_start_places_no_one`
(`game::cod2_archive`) and
`a_shot_past_the_archive_start_strikes_no_rewound_body`
(`game::cod2_gaps`).

### 2.4 `Bullet_Fire_Extended`

lnxded 0x811fe90 (unnamed there; Mac 0x1c867e), VERIFIED:

- At most 13 recursions, then "Too many resursions" and the round stops.
- `rifleBullet` gives `MOD_RIFLE_BULLET` (2), `dflags` 0x20 and
  `riflePriorityMap` (0x8187c1f); else `MOD_PISTOL_BULLET` (1) and
  `bulletPriorityMap` (0x8187c0c). `armorPiercing` adds `dflags` 2.
- `G_LocationalTrace` with mask 0x2802831 from the start to the end,
  passing the shooter (on a pass-through, the body just struck).
- `G_CheckHitTriggerDamage` over the leg. Not done in vcod.
- A non-client contact short of the end, not `SURF_NOIMPACT`, raises a
  temp entity 182 (183 for `rifleBullet`) at the contact: `eventParm`
  `DirToByte(normal)`, `scale` the reflected shot's `DirToByte`,
  `surfType` `(surfaceFlags & 0x1f00000) >> 20` (7 on a corpse),
  `otherEntityNum` the shooter. The 181 the code computes for a spread
  weapon is never written. cod2-efx.md 6.2 has what the client draws.
- Contents 0x10 (glass): the round goes on from a quarter unit past the
  contact (`0.25 / -dot`, 0 when the dot is under 0.125).
- A contact with `takedamage`, not the shooter: `G_Damage` with
  `Bullet_CalcDamageRange` (0x811fb7c: `damage` inside
  `maxDamageRange`, `minDamage` past `minDamageRange`, rounded lerp
  between) times the leg's scale, the forward as the direction, the
  contact as the point, `timeOffset` `level.time` less the lag time.
- A client struck by a `dflags` 0x20 round, under `scr_friendlyfire` or
  not on the shooter's team, passes the round on through at half the
  scale (double 0.5 at 0x815edb8).

### 2.4.1 Entities and damage triggers on a leg

- Each leg calls `G_CheckHitTriggerDamage(attacker, start, hit, damage,
  mod)` (Mac 0x1b811c) right after its trace, with the weapon's `damage`
  (+0x1ec) and the means of death 1 (pistol) or 2 (rifle): the entities
  in the leg's box under contents 0x400000 whose classname is
  `trigger_damage` and whose clip the segment meets
  (`SV_SightTraceToEntity`) are notified `damage` (damage, attacker) and
  fired through `Activate_trigger_damage`; one with no think gets health
  32000. VERIFIED, the calls.
- A contact on an entity with no client leaves the impact as a world
  contact does (`surfType` 7 on a corpse, `eType` 2) and stops the round.
  `G_Damage` (Mac 0x1c4272) on it needs `takedamage` (+0x161): the damage,
  at least 1, comes off `health`, `damage` is notified (damage,
  attacker), and at 0 or below (clamped at -999) `death` (attacker) and
  the die handler, else the pain handler. VERIFIED.
- Only `ClientEndFrame`, `player_die` and `SP_trigger_damage` store a 1 in
  `takedamage` (+0x161) in the Mac game module; `G_SpawnTurret` stores a
  0, and no script builtin sets it (CoD2 has no `setTakeDamage`).
  VERIFIED, every store to +0x161. So a script or brush model stops a
  round and takes nothing.
- On mp_toujane the four linked `script_brushmodel`s carry contents 0x40
  and 0x8000000 and no bit of `MASK_SHOT` (0x2802831): rounds pass them.
  VERIFIED, the traces in `a_script_model_stops_a_round`'s search.

vcod: `Cod2Level::entity_contact` traces the linked brush models under
CoD2's `MASK_SHOT` and the script models' xmodel collision
(`builtins::combat::bullet_hit`); the leg's sweep is the shared
`damage_trigger::sweep`. A rocket's impact on an entity takes the same
gate (`Cod2Level::impact_entity`). Tests `a_round_fires_a_damage_trigger`
and `a_script_model_stops_a_round` (`game::cod2_gaps`).

### 2.5 Hit locations

`G_LocationalTrace` scores a client on its DObj's per-bone boxes
(`cod2-xmodel-v20-format.md`), ranked by the priority map, the way
1.1's does (`cod11-combat.md` 3.1). INFERRED, the CoD1 structure; CoD2's
`SV_LocationalTrace` was not read. vcod's server carries no `legsAnim`
or `torsoAnim` in its playerstate, so it poses a target in its stance's
idle clip (`pb_stand_alert`, `pb_crouch_alert`, `pb_prone_aim`) at its
origin and view, and traces vcod-common's `bonetrace::bone_trace` on the
rig the client's model and attachments assemble. That stand-in is
vcod's, not retail's.

### 2.6 Grenades and rockets fired

- `weapon_grenadelauncher_fire` (lnxded 0x8120552): forward times
  `projectileSpeed`, plus `projectileSpeedUp` up, `fire_grenade` with
  the weapon's `fuseTime`, then the thrower's velocity along the throw
  added. VERIFIED. `G_UseOffHand` (Mac 0x1c8192) fires the offhand the
  same way.
- `fire_grenade` (Mac 0x1c9796): a cooked grenade keeps its
  `grenadeTimeLeft` as the fuse; `eType` 4, `eFlags` 0x1000000 (bounce),
  `pos` `TR_GRAVITY` with the velocity truncated to whole units, `apos`
  `TR_LINEAR` spinning 720 +- 45 and 360 +- 45 a second, the launch pitch
  less 120. VERIFIED, rodata 0x3265c0, 0x3261f0, 0x326340.
- `Weapon_RocketLauncher_Fire` (lnxded 0x812061a): the forward pushed off
  by the spread at 16 units, `fire_rocket` along it, and the shooter
  pushed back 64. VERIFIED. `fire_rocket` (0x810e938) was not read; vcod
  flies the rocket `TR_LINEAR` at `projectileSpeed` and blows it up on
  contact. Its destabilization is not done.

## 3. `G_Damage` on a client

Mac 0x1c4272, VERIFIED: a client takes it only with `takedamage`,
connected and not in the two intermission states. The multiplier is the
inflictor's (else the attacker's) weapon's `loc*` field for its hit
location when that weapon is a bullet weapon, else
`g_fHitLocDamageMult`, which `G_ParseHitLocDmgTable` (Mac 0x1c3c7e) reads
from `info/mp_lochit_dmgtable`, an info string behind `LOCDMGTABLE`
(stock: 1 for none, 1.5 helmet, head and neck, 0.9 torso upper, 0.8
torso lower, 0.6/0.5/0.4 down the limbs, 0 for the gun; VERIFIED, the
asset). `(int)(mult * damage)` above 0 goes to `Scr_PlayerDamage`
(Mac 0x19d312) with ten arguments, `timeOffset` the tenth.

## 4. `finishPlayerDamage`

Mac 0x1c2e32 (lnxded 0x80fb170), VERIFIED:

- Ten arguments; a damage under 1 does nothing. The inflictor argument,
  when an entity, is read as the attacker (`Scr_GetEntity(1)`), as 1.1's.
- Knockback, unless the entity's flag 8 or `dflags` 4: 0.02 of the damage
  prone, 0.15 crouched, 0.3 standing, truncated, at most 60, times
  `g_knockback` (1000) over 250 along the direction, and a `pm_time` of
  twice it clamped to 50..200 with `pm_flags` 0x400 when none runs.
  Rodata 0x326568, 0x32631c, 0x3262ac, 0x32656c.
- A bullet weapon raises 182/183 at the point for every client but the
  victim (`r.clientMask` with the victim's bit) and 185/186 for the
  victim alone (`clientNum` the victim, every other bit set), both
  `surfType` 7, `otherEntityNum` the attacker.
- `damageTimer` (ps+0x90) grows `damage * player_dmgtimer_timePerPoint`
  (100), at most `player_dmgtimer_maxTime` (750); `damageDuration`
  (+0x94) takes it; `flinchYaw` (+0x98) is the direction's yaw less the
  view's.
- The health drops; `damage` is notified with the damage and the
  attacker. At 0 or below (clamped at -999) the die handler runs with
  `psOffsetTime`; `stats[0]` takes the health.

## 5. `player_die`

Mac 0x1c4a7c (lnxded 0x810175a), VERIFIED stores and calls:

- Returns for a client already dead (`pm_type` above 1).
- `death` is notified with the attacker (an MG's gunner for a turret).
- A grenade still in hand drops from 40 up at `160 * (crandom,
  crandom, random)` on the fuse left.
- `pm_type` becomes 6 (7 linked), then `BG_AnimScriptEvent(ps,
  ANIM_ET_DEATH, 0, 1)` (Mac 0x18729a) draws `rand() % count` among the
  first matching `DEATH` clause's lines and returns the anim's duration,
  the line's own or the clip's length, plus 50 (`BG_ExecuteCommand`,
  INFERRED from 1.1's `BG_PlayAnim`); that is `deathAnimDuration`.
- `Scr_PlayerKilled` (lnxded 0x8118492) with `timeOffset` and
  `deathAnimDuration` after the seven 1.1 arguments.
- `takedamage` stays on, contents 0x4000000, `stats[1]` the yaw toward
  the attacker (or the inflictor; the client's own when neither), the
  box's top 30, `health` 0, the die handler 11.

`ClientEndFrame` (Mac 0x1a47a8) gives a client the script made `dead`
(`sessionState` 1) `pm_type` 6 or 7, and a playing one 0 or 1, every
frame, and marks the dead one `SVF_NOCLIENT`. VERIFIED stores.

vcod draws the death anim off the `DEATH` clause the playerstate's anim
conditions select (its `MOVETYPE` and `STRAFING` included) and runs the
event on the playerstate (cod2-player-anims.md 9, 10).

## 6. `cloneplayer` and `obituary`

- `PlayerCmd_ClonePlayer` (Mac 0x1c3574), VERIFIED stores: a player clone
  entity (`G_SpawnPlayerClone`) with `eType` 2, `clientNum` the player,
  `eFlags` the playerstate's with bit 2 cleared and 0xa0000 (`EF_DEAD`
  and the fresh-corpse bit), the player's origin and angles, `pos`
  `TR_GRAVITY` at the velocity clamped by `g_clonePlayerMaxVelocity`,
  `legsAnim` and `torsoAnim` the player's, the DObj's anim tree cloned,
  contents 0x4002000, a think at `deathAnimDuration`. vcod lets the
  corpse fall in the body queue (6.2).
- `GScr_Obituary` (Mac 0x1980cc), VERIFIED stores: temp entity 198 at the
  origin, broadcast (`svFlags` 8), `otherEntityNum` the victim,
  `attackerEntityNum` an entity attacker or 1022, `eventParm` the
  weapon's index, or `0x80 | mod` for `MOD_MELEE`, `MOD_HEAD_SHOT`,
  `MOD_CRUSH`, `MOD_FALLING` and `MOD_SUICIDE` (7, 8, 9, 11, 12).

### 6.1 The obituary on the client

`CG_EntityEvent` (Mac 0x1e4650) case 198, VERIFIED reads: the victim is
`otherEntityNum`, the attacker `attackerEntityNum`, the parm
`eventParm`. A parm with bit 7 picks the icon by `parm & 0x7f`: 7
`killiconmelee`, 8 `killiconheadshot`, 9 `killiconcrush`, 11
`killiconfalling`, 12 `killiconsuicide`, anything else `killicondied`;
without it the parm is a weapon index and the icon is the weapon's
`killIcon` (`killicondied` when empty), wide by `wideKillIcon` and
flipped by `flipKillIcon`. A victim past 63 is an error; a victim or an
attacker under 64 whose client info is not valid draws nothing. An
attacker of 64 or more is none. Unless attacker and victim are the same,
a kill by the local client centre prints `CGAME_YOUKILLED` ("You killed
&&1") with the victim's name and a death of the local client
`CGAME_YOUWEREKILLED` ("Killed by &&1") with the attacker's, each with
`^1&&2^7 ` and `CGAME_TEAMMATE` ahead of the name on a teammate
(`CG_PriorityCenterPrint`); then `CL_DeathMessagePrint` writes the kill
line: attacker name, icon, victim name, each name in its team's colour.
Both are skipped while `cg+0x2cd14` is set. INFERRED, what that flag
is.

Live (2026-10-22, retail `cod2_lnxded` in `unshare -rn`, a probe sending
`kill`): the obituary's parm was `0x8c` and its attacker the victim,
which draws `killiconsuicide`, the victim's name and no centre print.
VERIFIED. vcod-server sends the same on its `kill` (3.4.1 of
cod2-protocol.md). vcod's CoD2 HUD (`cod2_hud::obituary`) draws the kill
line in CoD2's game message window, the icon mirrored by `flipKillIcon`,
and the centre print where CoD2 puts it (cod2-hud.md 14, 15).

### 6.2 The corpse's own fall

- `PlayerCmd_ClonePlayer` (Mac 0x1c3574) gives the clone `pos` `TR_GRAVITY`
  (5) at `level.time` from the player's origin at its velocity, whose x
  and y are capped above (not below) at `g_clonePlayerMaxVelocity`
  (default 80, `Dvar_RegisterFloat`), `clipmask` 0x10001, contents
  0x4002000, and a think at `level.time + deathAnimDuration` (its one
  argument), handler 12. VERIFIED, the stores.
- `G_RunCorpse` (Mac 0x1b0882, lnxded 0x8107dbc) runs `G_RunCorpseMove`
  (Mac 0x1b00ae, lnxded 0x8107846), then the DObj and lerp-frame update
  (lnxded 0x8107d30: `BG_UpdatePlayerDObj` and 0x80dbdbc on the corpse's
  own `clientInfo_t`), then the think. VERIFIED, the calls.
- `PlayerCmd_ClonePlayer` (lnxded 0x80fcc76) gives the corpse a slot of
  `0x4c8` bytes at 0x884e7b8: the anim tree cloned from the player's
  (`XAnimCloneAnimTree` 0x80c2c18), the player's `clientInfo_t` copied
  whole (0x4b8 bytes from 0x87035bc), and a falling flag at +0x4c4 set to
  1. VERIFIED, the stores.
- The move first takes `XAnimCalcDelta(tree, 0, rot, trans, 1)` (0x80c1504
  through 0x80c07cc): each node's goal weights (tree byte 7 set), a leaf's
  `XAnimCalcRelDeltaParts` (0x80bde02) from its old node time (+0x10) to
  its time (+0xc). VERIFIED, the calls and the reads. The leaf samples its
  delta part at each time (0x80bdc5a: the last key at time 1, the keys
  around `time * numframes` lerped otherwise, the rotation as two shorts),
  takes the translation's difference, adds a looping clip's whole travel
  when it wrapped, and turns it by the inverse of the yaw at the new time
  (0x80bdd54, `2 / (z^2 + w^2)` off rodata 0x815001c). VERIFIED, the
  decompile.
- `G_RunFrame` (0x810a13a) eases every DObj's weights and sets each old
  node time to its time (0x80bf112 from 0x80920d0) before the script
  threads, then steps each time from its old time by `0.001 *
  level.frametime * rate * frequency`, each product stored (0x80beb06 from
  0x8109f14), before the entity pass. VERIFIED, the calls and the stores.
  So the corpse's delta is the frame's own step. The death anim starts at
  0 in the corpse's first anim update, after its first move: a resting
  corpse moves from its second frame on. INFERRED from the order and the
  kill runs' first slide 50 ms after the corpse's first snapshot.
- While the falling flag is clear and the delta's squared length is above
  1, the move sweeps the capsule from `r.currentOrigin` to `pos.trBase`
  plus `delta.x` along `AngleVectors(r.currentAngles)`'s forward and
  `delta.y` along its right negated, both normalized (`Vec3Normalize`
  0x80a4424 stores the length); `r.currentOrigin` takes `Vec3Lerp` to the
  stop. A clear sweep rests it there (`TR_INTERPOLATE`, `trBase` the end,
  no delta), and, when a sweep a unit lower meets nothing (0x810a57e), sets
  the falling flag and drops it under `TR_GRAVITY` at the delta's move as
  its velocity. A blocked sweep leaves `pos` alone. VERIFIED, the
  decompile. Live, the tdm corpse slides 2.3 units a frame for three frames
  50 ms after it rests and 3.8 a frame back for three more 550 ms later, its angles
  fixed. VERIFIED, the fixture.
- A falling corpse sweeps the box from `r.currentOrigin` to the
  trajectory's point under the clipmask; a stop whose origin is in
  `CONTENTS_NODROP` frees the corpse; else the velocity is zeroed and a
  stop on a face with a positive z normal, or a start in solid, rests it
  and clears the flag (0x810758e): `TR_INTERPOLATE` at the stop,
  `groundEntityNum` the struck entity, the angles turned onto the face
  (forward projected onto it, `AxisToAngles`). A stop on a wall moves it
  out a unit along the normal and drops it again from there. VERIFIED, the
  stores; INFERRED, which `trace_t` byte is startsolid.
- `BodyEnd` (Mac 0x1c14d4): `eFlags` 0x80000 off, contents 0x4000000.
  VERIFIED.

vcod: `game::cod2_corpse` (`launch`, `run`, `Cod2Level::run_corpses`,
after the missiles) moves the body through `game::corpse::step`, the
frame CoD2 and CoD4 share, `Cod2Corpse::anim` keeps the death
anim's node time, and `vcod_cod2::rootdelta` is `XAnimCalcDelta` over the
one leaf with a goal weight (`rel_delta`, `step_time`). The root track's
yaw keys keep the loader's shorts (`xanim::Track::rot_raw`, each key's
sign turned to its predecessor's as `XAnimLoadFile` 0x80c3f8a turns it).
`BodyEnd`'s fresh bit is the body queue's own think at
`deathAnimDuration` (`BodyQueue::push_raw`'s `fresh`,
cod2-player-anims.md 10). `kill` draws the death anim as `player_die`
does, so its `cloneplayer` gets the duration. The kill runs' corpses match
retail's bit for bit (cod2-protocol.md 15.7). VERIFIED, the gate. Not done:
the startsolid re-trace 64 units up, a tree blending two weighted leaves
(only one has a goal weight on a corpse). Test `a_corpse_falls_and_rests`
(`game::cod2_gaps`).

### 6.3 The dropped weapon

- The stock killed callbacks call `_weapons::dropWeapon` and
  `dropOffhand`: `dropItem` of the weapon in hand when its slot has rounds,
  and of the offhand when it has any. VERIFIED, `dm.gsc` 318-319,
  `_weapons.gsc` 396-433.
- `PlayerCmd_dropItem` (Mac 0x1c053c): a weapon name goes to `Drop_Weapon`
  with the second argument as a tag (default a script constant), anything
  else to `G_FindItem` and `Drop_Item`. `Drop_Weapon` (Mac 0x1b165e): a
  weapon not held is taken and nothing drops; a `clipOnly` one with an
  empty clip likewise; else the item takes the reserve and the clip (0
  read as -1, both 0 clears contents 0x200000), the weapon is taken, and
  from a tag the item starts at the tag swept from the box's centre.
  VERIFIED, the stores.
- `Drop_Weapon`'s tag arm (lnxded 0x81064de): `G_DObjGetWorldTagMatrix`
  (0x811e542) poses the player (`G_DObjCalcPose` under the skeleton
  stamp, with `G_PlayerController` 0x80f729a easing the controls off the
  `clientInfo_t`) and turns the tag by `AnglesToAxis(r.currentAngles)`
  onto `r.currentOrigin` (`MatrixTransformVector43`); a capsule of the
  item's box goes from `0.5 * (mins + maxs) + r.currentOrigin` to the tag
  under the item's clipmask, 0x491 for none (0x810847a), passing the
  player; `pos.trBase` and `r.currentOrigin` take `Vec3Lerp` to the stop
  and `pos.trTime` the level's. VERIFIED, the calls and the stores.
- Its angles are the player's `r.currentAngles`: `AxisToAngles` of the
  tag and the 90 of roll (rodata 0x815a3e4) land in a local that
  `G_SetAngle(item, player+0x144)` never reads. VERIFIED, the decompile;
  VERIFIED live, the tdm kill run's two items fly at (0, -144.30542, 0),
  the dying view's yaw.
- The spin is `(50 c3, 40 c2, 60 c1)` degrees a second, `TR_LINEAR` from
  the level's time, `c1`..`c3` three `crandom()` draws in that order
  (rodata 0x815a3e8 60, 0x815a3ec 40, 0x815a3f0 50). VERIFIED, the calls
  and the stores.
- `G_RunItem` (lnxded 0x8107de6) reads the arc 50 ms ahead (a unit lower
  when that is under 0.1 squared from `r.currentOrigin`, rodata
  0x815a538), sweeps to it, and on a stop: `Vec3Lerp` to it, a second
  sweep along a face met under 0.01 of the way whose normal's z is under
  0.5 (rodata 0x815a53c, 0x815a540), then `TR_LINEAR_STOP` from
  `r.currentOrigin` at 20 times the move over 50 ms; a stop under 0.01 of
  the way rests the item (`G_SetOrigin`, the angles turned onto the face
  with the weapon's 90 of roll, 0x810773a, the ground entity) on a face
  whose normal climbs and frees it otherwise or in `CONTENTS_NODROP`.
  VERIFIED, the decompile. Live, the tdm items fly from 21200, stop at
  21500 (`trType` 3, `trDuration` 50) and rest at 21550. VERIFIED, the
  fixture.
- `Drop_Item` (Mac 0x1b1128): `eType` 3, the item index in `index`, a
  weapon's box (-1, -1, -1)..(1, 1, 1), contents 0x407c0108, owner the
  dropper, launched from the player's origin raised half its box's
  height along its yaw at `g_dropForwardSpeed` (10) and up at
  `g_dropUpSpeedBase + crandom() * g_dropUpSpeedRand` (10, 5), `TR_GRAVITY`,
  a think at +1000, kept in a ring of `g_maxDroppedWeapons` (16, 1..32)
  that evicts the slot farthest from every playing client. VERIFIED.
  `crandom` is `2 * rand() / 2^31 - 1` (Mac 0x2cf1a). VERIFIED.
- `Touch_Item` (Mac 0x1b1bea) is 1.1's (`cod11-items.md` 7) on CoD2's item
  rows (a weapon's row is its index): a weapon not held that needs a slot
  is taken only by the use key (`touched` 0), which drops the weapon in
  hand where the item lay (`Drop_Weapon` with no tag) and selects the new
  one (`a <index>`, `G_SelectWeaponIndex`); held, it gives rounds. The
  event on the player is 0x90 (`EV_ITEM_PICKUP`) with a weapon given,
  0x91 (`EV_AMMO_PICKUP`) with only rounds. VERIFIED.
  `BG_CanItemBeGrabbed` (Mac 0x6a3ca) lets a weapon that needs no slot be
  touched up when not held; vcod's 1.1 arithmetic does not. VERIFIED, the
  test; not done.
- A weapon walked over with a slot free (`touched` 1) is given with no
  `a`: only the use key's swap selects the new weapon (`Touch_Item`'s
  `param_3 == 0` arm before `G_SelectWeaponIndex`). VERIFIED, the branch.
- `BG_TakePlayerWeapon` (Mac 0x7bec4) clears `ps.weapon` when it takes
  the weapon in hand, unlike 1.1's, which leaves it for pmove. VERIFIED,
  the store at its end. So the swap's snapshot carries `weapon` 0.
- The cgame's `a <index>` (`CG_ServerCommand` case 0x61, Mac 0x1e2ea4)
  is `CG_SelectWeaponIndex` (Mac 0x1d7ef6): `cg_weaponSelect` becomes the
  index and the sight toggle drops unless it is the old weapon's
  `altWeapon`. VERIFIED. The next cmd's weapon byte names the new weapon,
  and `PM_Weapon` on an empty hand starts the change (`weaponstate` 2 for
  no time) and raises it. A client that ignores the `a` keeps asking for
  the dropped weapon, which no longer valid leaves the hand empty until a
  manual switch: the bug vcod's client had.
- Live, `crates/cod2/tests/fixtures/pickup/carentan-swap.txt` (retail
  1.3, `probe_pickup.gsc`: a kar98k (4) in hand and a thompson (5)
  given by two `giveWeapon`s, an mp40 (6) spawned 30 ahead at eye
  height, `+activate` tapped). VERIFIED, the capture: before the press
  `weapons[0]` 0x30, `weaponslots[0]` 0x50400 (slot 1 the kar98k, slot 2
  the thompson), the kar98k 5 + 55 rounds; the press's snapshot carries
  the server command `a 6`, `EV_ITEM_PICKUP` (0x90) with parm 6, `weapon`
  0, `weapons[0]` 0x60, `weaponslots[0]` 0x50600 (the mp40 in slot 1),
  the kar98k's rounds 0 and the mp40's 32 in the clip, and a new item
  entity of `index` 4 at the mp40's spot; the next snapshot, on cmds that
  name 6, `weapon` 6, `weaponstate` 1 (raising) and `weapAnim` 10 (raise,
  under the toggle), ready 300 ms on. The mp40's reserve (72) is a draw
  between its `dropAmmoMin` 96 and `dropAmmoMax` 128 less the clip.
- `PlayerCmd_giveWeapon` (Mac 0x1c2438): a weapon that needs a slot
  (`BG_DoesWeaponNeedSlot`: not type 9, no `offhandClass`) takes the first
  empty of the two (`BG_GetEmptySlotForWeapon`, ps+0x555 then +0x556; a
  script error with both full), then `Add_Ammo` of `startAmmo` less the
  reserve held, filling the clip first when the weapon was not held.
  VERIFIED, the calls; the capture's 5 + 55 on a kar98k of `startAmmo` 60.
  Every stock rifle, smg and pistol reads `weaponSlot primary`, so 1.1's
  slot-by-class put both of two given primaries in slot 1.

vcod: `builtins::cod2::drop_item` runs 1.1's `dropItem`; `item::launch`
names CoD2's row off the level's weapon list (`weapon_<name>`, INFERRED)
and `cod2_items::launch_off` launches it, its climb off the game's
`rand()` (`GameHost::cod2_crandom`). A thrown drop waits in
`Cod2HostState::tag_drops` until `Cod2Level::place_drops` (each frame's
start and after its threads) poses the dropper, with the controls eased
toward the frame its last end frame read, and moves it to the tag
(`world_tag`); `cod2_items::spin` draws its spin and
`cod2_items::run_flight` is CoD2's `G_RunItem`. The wire gate replays
retail's draws, read off the items in flight
(`Cod2HostState::drop_pins`): the kill runs' items fly, spin and rest as
retail's but for their start, whose pose is 0.01 off on the tdm run and
two ulps of x off on the sd run (cod2-protocol.md 15.7). VERIFIED, the
gate. The touch is the shared touch
pass; the use key and the item's cursor hint (`weapon + 4`, no string) are
the one use list of cod2-gsc-diff.md 5.4 (`game::cod2_use`).
`item::write_back` empties a CoD2 hand whose weapon was taken,
`builtins::cod2::give_weapon` takes CoD2's slots and `Add_Ammo`, and the
client's `Cod2Session` selects on `a`. Gate
`retail_use_key_swap_replays_on_the_cod2_server` (`game::cod2_pickup_ab`)
replays the capture: `weapon`, `weaponstate`, `weapAnim`, `weapons`,
`weaponslots`, the events, the `a` and the kar98k's spot match each
snapshot. Not done:
`g_maxDroppedWeapons`' ring (1.1's 32 is kept), the no-slot rule above,
the clear of contents 0x200000. Test
`a_dead_players_weapon_drops_and_is_swapped_up` (`game::cod2_gaps`).

## 7. Missiles and blasts

- `G_RunMissile` (Mac 0x1ca0e4), VERIFIED calls: a `G_LocationalTrace`
  from where the missile is to where its trajectory puts it, passing its
  owner; a `SURF_NOIMPACT` contact frees it; a world contact with the
  bounce flag runs `G_BounceMissile` and raises `EV_GRENADE_BOUNCE`
  (187) with the surface when that reports a real bounce; an entity
  that takes damage takes the missile's impact damage (`ent+0x19c`, the
  weapon's `damage`) and the missile explodes with `EV_ROCKET_EXPLODE`
  (189) or its no-marks twin. The think runs after.
- `G_BounceMissile` (Mac 0x1c9db0), VERIFIED: the velocity at the
  contact reflected; with the bounce flag scaled by the surface's
  `parallel` and `perpendicular` bounce lerped by how square it struck
  (`dot / -speed`); on a floor (normal z over 0.7) a speed under 20
  stops it there. The origin moves 0.1 along the normal (z only
  downward). It reports a bounce when not in water and the velocity
  changed by over 100. Rodata 0x340d50 (0.7), 0x32624c (20), 0x326388
  (0.1), 0x326350 (100).
- `G_ExplodeMissile` (Mac 0x1c94f4), VERIFIED: at the trajectory's point
  truncated to whole units, `eType` 0, `eFlags` 0x20, a 16-unit trace
  down for the normal, `EV_CUSTOM_EXPLODE` (191) for `projExplosionType`
  none else `EV_GRENADE_EXPLODE` (188), the parm the normal's byte,
  `surfType` 20 in water, and `G_RadiusDamage` with
  `explosionInnerDamage`, `explosionOuterDamage` and `explosionRadius`
  when the inner is non-zero.
- `G_RadiusDamage` (lnxded 0x81022a0), VERIFIED: no attacker, no walk;
  every entity in the box of `radius * sqrt 2` but the one to skip, with
  `takedamage`, within `radius` (a brush model by its bounds, else its
  origin), clients not under `setPlayerIgnoreRadiusDamage`, takes
  `outer + (1 - d / r) * (inner - outer)` times `CanDamage`'s share,
  rounded; one `CanDamage` misses takes a tenth when a capsule trace
  (mask 0x811) to its box's centre is short and that centre within a
  fifth of the radius. `dflags` 1, hit location 0, the direction the
  victim's origin less the point, raised 24. Rodata 0x815982c..0x8159838.
- `CanDamage` (Mac 0x1c3f18, lnxded 0x8101ee6), VERIFIED: a client's
  five probes from the blast (mask 0x802891, passing the victim) to the
  middle of eye and feet and four points 15 either side across the line
  to the blast, half the eye height up and down; none 0, one to three
  `n / 3`, four or five 1. A non-client's centre and four corners pass on
  the first clear one.

- `GScr_RadiusDamage` (Mac 0x19947a): `G_RadiusDamage(origin, NULL,
  &g_entities[1022], maxDamage, minDamage, range, NULL, 14)`, the level's
  `setPlayerIgnoreRadiusDamage` flag copied into the walk's word
  (`level+0x35f4`) for the call only. VERIFIED. `G_Damage` (Mac
  0x1c4272) makes a missing inflictor the world; the client arm's weapon
  is then the world's (0), so the multiplier is the hit-location table's
  `none` row. VERIFIED. `G_RadiusDamage` measures a brush model from the
  nearest point of its box (`absmin`/`absmax`), anything else from its
  origin, and `CanDamage`'s arm for an entity with no client traces its
  centre and four corners 15 out on x and y to the blast, passing on the
  first clear one. VERIFIED (Mac 0x1c45b8, 0x1c3f18; rodata 0x326570
  sqrt 2, 0x3262dc 0.2, 0x326388 0.1, 0x326470 24, 0x3264c4 15).

vcod: `game::cod2_blast` holds the measure both walks share; the builtin
(`builtins::cod2::radius_damage`) walks the playing clients (their body as
the last think or end frame left it) in slot order, then the entities
with `takedamage` in the area tree's order, and starts each client's
damage callback after all are measured, where retail measures each after
the last one's callback ran. The missiles' blast takes the entities too.
Tests `radius_damage_reaches_a_cod2_body` (`game::cod2_gaps`) and
`cod2_blast`'s.

vcod's missiles are its own (`game::cod2_combat::Missile`), not 1.1's
`Missiles`: a grenade's clip mask is the blast mask with bodies
(INFERRED; `fire_grenade`'s word is a relocated pointer in the Mac
decompile), and the missile's entity goes out `SVF_BROADCAST`.

## 8. What vcod does and does not

vcod-server's CoD2 level (`Cod2Level`) runs, per cmd: the sight flag and
the aim spread, pmove, `PM_Weapon`, the weapon tables mirrored in and
out of the script's, then the events: shots, the melee (a 64-unit trace,
INFERRED reach; `Weapon_Melee` was not read), the offhand throw, a
cooked grenade's suicide (as a blast where its holder stands) and the
falls. Damage goes through the stock callbacks; a kill, the corpse, the
obituary and the respawn are the script's.

The temp entities (impacts, the obituary, and every script one) are
raised in 1.1's numbering, as the shared builtins raise theirs, and the
snapshot builds them in CoD2's by name (`cod2_wire::temp_entity`, `event_to_cod2`);
one CoD2 has no event for (`EV_PLAY_FX_DIR`) stays off the wire. The
players' and the missiles' own rings carry CoD2's numbers from the start.

Done since: the binoculars (1.8), `G_CheckHitTriggerDamage` and rounds on
entities (2.4.1), the corpse's fall and `BodyEnd` (6.2), the dropped weapon
and its pickup (6.3), the `radiusDamage` builtin (7), the shot's angles
and the bobbed muzzle, and glibc's `rand()` once a test pins its seed
(9.6).

Not done: the breath hold, the primary slots' alt bookkeeping,
`fire_rocket`'s destabilization, the corpse's root motion,
`Weapon_Melee`'s hit triggers. Section 9 is what the retail captures
checked.

Tests: `moves_ab`'s weapon captures (9.1), `game::cod2_combat_ab`'s
two-client ones (9.2 to 9.4, 9.6) and `game::cod2_hitpose_ab` (9.5) with
`COD2_DIR`; `vcod_cod2::pmove::weapon`'s unit tests (a bolt rifle's shot and
rechamber, an smg's rate, an empty clip's reload, the offhand
sequence, the damage falloff); `game::cod2_fight` with `COD2_DIR` (two
clients on mp_toujane `dm`: a sighted rifle round kills, scores and the
victim respawns; an mp40 burst lands several pistol-bullet hits; the
frag key throws and a grenade at the victim's feet kills by its blast).

## 9. Against retail

The captures are retail `cod2_lnxded` 1.3 under
`crates/cod2/tests/fixtures/combat/probe_combat.gsc` on mp_toujane, made
by `tools/run_probe_cod2.sh` with `GT=combat` (the recipe heads each
file).

### 9.1 The weapon replay

`moves_ab` replays `rifle` (kar98k, colt in primaryb), `smg` (thompson),
`pistol` (colt), `switch` (kar98k and colt by the cmd's weapon byte) and
`frag` (the frag key held, tapped and sighted) through pmove and
`PM_Weapon`, each snapshot's weapon words, `ammo[]` and `ammoclip[]`
compared. All five match in every resynced row and every free-run row
up to the first server-side change (below). VERIFIED, the captures.

- The level's weapon table is `CS_WEAPONS` in order: with the probe's
  three weapons, `defaultweapon_mp` 1, the map's turret
  (`mg42_bipod_stand_mp`) 2, then the precaches: `G_SetupWeaponDef`
  (Mac 0x1c8c0c) names `defaultweapon_mp` before the spawn, and
  `SP_turret` its `weaponinfo` during it. `BG_SetupAmmoIndexes` and
  `BG_SetupClipIndexes` (Mac 0x7a4e4, 0x7a22a) match a name against the
  types so far from type 0, the `none` weapon's empty name: a new name
  takes the next index from 1, and the turret's empty `ammoName` shares
  0. defaultweapon's `enfield` is 1, kar98k's ammo and clip 2, colt's 3,
  the frag's `usgrenade` clip 4. VERIFIED, the decompile and the
  captures' `ammo[2]=60`, `ammo[3]=21`, `ammoclip[2..4]`. vcod numbered
  both from the first weapon in CS order and gave the turret's empty
  name its own index, which put every rifle's rounds a slot high.
- `eFlags` 0x40 (`EF_FIRING`) is `PmoveSingle`'s, inlined in Mac `Pmove`
  (0x70c7e): cleared every step, set outside the intermission while the
  attack key is down, `pm_flags` 0x1000 clear, the weapon ready (0) or
  firing (3) and `PM_WeaponAmmoAvailable` (Mac 0x7a8d2, the weapon's
  clip at `ps+0x344`) non-zero. VERIFIED, the decompile and
  the captures (0x42 while held on a loaded gun, 0x2 once the clip is
  empty). vcod: `weapon::update_firing_flag`, before the step.
- The throw (state 14 to 15 in `PM_Weapon`, Mac 0x7cbb2) takes the
  grenade out of the offhand's clip after `EV_USE_OFFHAND`, then raises
  `EV_EMPTY_OFFHAND` (148) when clip and reserve are both 0. VERIFIED,
  the decompile and `frag`'s `ammoclip[4]` 3, 2, 1, 0 at each throw.
  `G_UseOffHand` (Mac 0x1c8192) takes nothing. vcod took nothing
  anywhere before this.
- `fire_grenade` (Mac 0x1c9796) zeroes the thrower's `grenadeTimeLeft`
  once it has read it as the fuse, the same server frame; the snapshot
  after a throw carries 0. VERIFIED, the decompile and `frag`.
- `PM_UpdateAimDownSightLerp` (Mac 0x7c762) adds `msec * rate` to
  `fWeaponPosFrac` on the x87 stack and rounds once at the store; two
  float roundings left vcod a bit off in 9 rows. VERIFIED, `rifle`,
  `smg`, `pistol`, `frag`.
- The only rows `moves_ab` cannot replay are `frag`'s three after each
  blast: the thrower's own grenade lands beside him and `G_Damage`'s
  knockback (`pm_flags` 0x400, `pm_time`, the push) is server-side.

### 9.2 Two clients: damage and the end frame

`game::cod2_combat_ab` replays eight two-client captures (a `still`
target and a shooter, `SECOND=still`): kar98k rounds sighted at the
head, torso and legs from 220 units, a thompson burst there and from
1340, kar98k rounds from 1340 that meet a wall, a killing head shot
(`probe_mortal 1`), and a frag at the target's feet. Each body starts
from retail's playerstate at its own first snapshot as a player (9.3);
each frame runs the cmds retail had run by
its snapshot's `commandTime`, packet by packet in the order of each
packet's newest cmd (the `p` lines), then `G_RunFrame`. Compared: the
callback's `PROBE` lines (damage, flags, means, weapon, hit location,
health, point, direction), the target's health, damage words,
`pm_flags`, `pm_time` and velocity, the shooter's weapon words and
rounds, the event entities and the missiles. VERIFIED, the gate, where
`GAPS` does not name a row.

- `P_DamageFeedback` (Mac 0x1a3366), from `ClientEndFrame` (0x1a47a8)
  after `Player_UpdateLookAtEntity` and once `gclient+0x2810` has taken
  `aimSpreadScale / 255`: nothing past `pm_type` 5; `damageCount` 0;
  with damage gathered since the last end frame (`gclient+0x27e0`,
  which each `finishPlayerDamage` adds to) and a max health above 0,
  the count is `damage * 100 / maxhealth`, 127 at most; it is added to
  `aimSpreadScale` (255 at most); `damageYaw` and `damagePitch` are the
  gathered direction's `vectoangles` as `angle / 360 * 256` truncated
  (rodata 0x3261f0 360, 0x326468 256), both 255 when the damage came
  with no direction (`gclient+0x27f0`); `damageEvent` counts up and the
  gathered damage clears. VERIFIED, the decompile and the captures
  (`damageYaw` 189 for a round along yaw 266.4, `damageCount` 99, 127
  for the killing 270). vcod set none of the four before this.
- `ClientSpawn` (Mac 0x1930a6) memsets the gclient's first 0x28a4
  bytes, keeping only +0x26a8..+0x27a8 (the session and `pers`) and
  playerstate word +0x140 (`stats[5]`, stored plus one). So the four
  damage fields (ps +0x11c..+0x128) and the gathered damage, direction
  and no-direction flag (+0x27e0..+0x27f0) read 0 after a spawn.
  VERIFIED, the decompile. A blow that lands on a dead body is gathered
  but never fed back (`pm_type` 6 is past 5), so without the memset it
  would reach the next life's first end frame as a new `damageEvent`;
  vcod-server did exactly that, and the respawned client drew the
  killing blow's direction icon (`Cod2Player::clear_damage`). The same
  memset clears the event ring: a script respawn's first frame read
  `eventSequence` 0, VERIFIED live (cod2-killcam.md 5, `killerdies`).
- `flinchYaw` on lnxded (`PlayerCmd_finishPlayerDamage` 0x80fb170) is
  `(int)vectoyaw(vDir) - (int)viewYaw` (the yaw wrapped into 0..360),
  two truncations and an int subtraction; the Mac build subtracts in
  float. VERIFIED, the lnxded decompile and the captures (181 where the
  float subtraction gives 180).
- `Player_UpdateLookAtEntity` (Mac 0x1cd1d4) clears `pm_flags`
  0x300000, then traces 15000 units (rodata 0x326400) along the view
  under mask 0x22802801; a playing body met there sets 0x100000 for a
  teammate inside `g_friendlyfireDist` (default 256) or 0x200000 for
  anyone else inside the held weapon's `enemyCrosshairRange` (def
  +0x260). VERIFIED, the decompile and the target's 0xa00000 while the
  shooter stands in its view. The smoke test (`SV_FX_GetVisibility`
  0.2) is not done in vcod. That trace was most of a full server's tick
  until long sweeps skipped the BVH boxes off their line
  (`cod4-server-frame.md` 10).
- The knockback's `pm_time` (`knockback * 2` in 50..200) and the
  velocity after it match once the two clients' packets run in send
  order, to the bit. VERIFIED, the gate. The push runs along the shot's
  forward (9.6) after `PlayerCmd_finishPlayerDamage` (lnxded 0x80fb170)
  renormalizes the script's direction through `Vec3NormalizeTo`, its
  length and reciprocal stored, then `VectorScale` and `VectorAdd`, each
  component stored. VERIFIED, the calls. vcod normalized in `f32` and
  sent the gunner's (`hit-turret`) velocity a last bit off (-35.84677
  against -35.846775).

### 9.3 Rounds

- `SV_Trace` under `G_LocationalTrace` clips the static models
  (`misc_model` xmodel collision) after the world, before the bodies.
  vcod clipped none, so a round on the far line went through a static
  model and on into the target. VERIFIED, `hit-rifle-far`: retail's
  two rounds raise temp entity 183 on wood (`surfType` 21) at (211, 471,
  46), and vcod's now land there. vcod: the placements' world triangles
  (`vcod_common::props::collision_tris`) in a world of their own that
  only the locational trace reads.
- The struck body: see "The hit pose" below. The rifle captures' and the
  smg burst's first rounds strike retail's point to the log's digits.
  They used to land 0.07 to 0.46 units nearer the shooter (`hit-head`
  0.12, `hit-torso` 0.07, `hit-legs` 0.24, `hit-smg`'s arm 0.46): the
  gate ran neither client's cmds before both played, so the target's
  legs clip went from the spawn's 0x8a to the idle 0x99 only at the
  shared start, two seconds after retail's (its snapshots carry 0x99 from
  the frame after its spawn on), and the idle's node time sat two seconds
  behind. VERIFIED, the gate: each client now begins the frame before its
  first snapshot as a player, takes that snapshot's playerstate and runs
  its own cmds from there. VERIFIED, the gate.
- The burst's later rounds. After two blows the target backs off under
  the knockback with legs 0x1d, `pb_stumble_forward`, a 0.6 s looping clip
  whose root travels 161.08 units. VERIFIED, the paks and the wire's 541.
  vcod blended it in over the idle 0x99 in 120 ms, retail in 400, so
  later rounds passed the arm retail's strike. VERIFIED,
  `combat/hit-tags` ("The knocked body" below). The 400 is the setter's
  stance window (`ci+0x4ac`, cod2-player-anims.md 7): a legs change whose
  new and old anims differ in the crouched class opens it, and
  `pb_stumble_forward` reads crouched because `stumble_crouch_forward`
  plays it too (`+0x58 & 0x180`, lnxded 0x80d9c02). VERIFIED, the
  decompile and `mp/playeranim.script`. vcod's records held no movetype
  past 31, so the stumble read standing. With the window `hit-smg` strikes
  all six rounds where retail does: its three `GAPS` rows ("9.5 the struck
  body under the burst") are gone. VERIFIED, the gate.
- The stumble's start phase is the setter's `0.36 * clientNum + (time %
  period) / period` less its whole part, both truncations under the
  control word or-ed 0xc00 (`BG_RunLerpFrameRate` 0x80d9d00, 0x80c0d8c).
  VERIFIED, the disassembly. `period` is the old anim's `XAnimGetLength`
  plus 200 when the old node is a clip, else 1000 (0x80d9d00's
  `XAnimIsPrimitive` arm). VERIFIED, the decompile. The idle's 3300 ms
  give 3500, and the stumble set at 8800 starts at 0.5143. INFERRED, the
  arithmetic. The fit on `hit-tags` below starts there. VERIFIED, the
  fit.
- A clip's node time steps as vcod's `Play::step` does. `G_RunFrame`
  (0x810a13a) steps every DObj's weights first (0x80920d0 to 0x80bf112:
  each weight moves `dt / goalTime` of the way to its goal), then, after
  the trigger queue, each entity's times (0x8109f14 to 0x80c114e to
  0x80beb06: a leaf's `time` is `oldTime + dt * rate * frequency`, `dt`
  the frametime times 0.001 at rodata 0x815eca0). VERIFIED, the
  decompile. A notetrack splits the step but each call starts from the
  same `oldTime`, so the frame ends where an unsplit step does.
  INFERRED, the control flow.
- `XAnimCalc` (Mac 0x3bf76) samples a clip at its node time times the
  parts' `numframes`, the key below and the fraction past it, the last key
  at time 1. VERIFIED, the decompile. `XAnimLoadFile` (Mac 0xba204)
  stores `numframes` as the header's frame count plus one for a looping
  clip, less one, and the frequency as the frame rate over it. VERIFIED,
  the decompile. So a loop runs over the header's frames, as vcod's
  `XAnim::frame_pos` wraps over `frame_count - 1`. INFERRED, the two
  together.
- The goal weight (0x80c20de): with a blend time, a weight of 0 starts
  at `0.001 * goal` (rodata 0x8150274); a rise takes `(goal - weight) *
  time / goal`, a fall `(weight - goal) * time / weight`, a clear to 0
  the whole time; each ancestor with no goal goes to 1 at rate 1.
  VERIFIED, the decompile. vcod's `Ramp::raised` gives a rise from 0
  the whole blend where retail's takes 0.999 of it, a weight 0.0004 off
  at 50 ms into a 120 ms blend. INFERRED, the arithmetic.
- The rate: 0x80da42a measures it off `pos.trBase` between end frames
  (`bgs+0xb3be0`, which `G_RunFrame` sets to `level.time` beside
  `bgs+0xb3bdc`, 0x810a13a), as vcod does. VERIFIED, the decompile.
  `moveSpeed` (+0x44) is the root's travel over `XAnimGetLength` in
  float seconds (0x80d72e6, `BG_LoadAnim`'s array fill), 268.47 for the
  stumble where vcod truncated to 268. VERIFIED, the decompile and the
  paks. vcod now divides by the float on CoD2 (`PlayerAnims::
  float_move_speed`).
- The body's place. The rounds' lag times run ahead of `level.time`,
  so neither side rewinds. VERIFIED, the gate. Retail's rewind
  (`G_AntiLagRewindClientPos` 0x811fc76, through 0x8099dac) reads
  archived frames, and `SV_GetCachedSnapshot` (0x8099c04) finds one
  only after script `setarchive(1)` (`SV_EnableArchivedSnapshot`
  0x8093312). VERIFIED, the decompile. `probe_combat.gsc` never calls
  it, so retail would not rewind here at any lag. INFERRED.
  vcod rewinds from its own history whatever the archive holds.
  VERIFIED, `Combat::bullet_fire`.
- The spread's draws: 9.6. Retail's one thompson round on the far target
  dealt 16 (35 to 10 over 800..1500 at about 1330). VERIFIED,
  `hit-smg-far`.

#### The knocked body

`combat/hit-tags` is `hit-smg`'s burst with `+set probe_tags 1` (recipe
in its header): `probe_combat.gsc`'s `tags()` links eight script_origins
to the target's bones and logs where they stand every frame from 8500 to
10000. Addresses lnxded unless named Mac.

- CoD2 1.3's game module has no `getTagOrigin`. VERIFIED, `strings`
  on `cod2_lnxded`. A `linkTo` on a player's tag works once the spawn's
  end frame has built the DObj: in the spawn's own frame it fails with
  "parent model 'xmodel/playerbody_german_africa01' is invalid".
  VERIFIED, the retail log.
- Script `bulletTrace` with `hitCharacters` stops on the body's capsule,
  not on its bones: `probe_fan 1`'s depths draw one upright cylinder
  about the origin with a rounded top, the rows from 32 to 54 above the
  origin equal to six digits. VERIFIED, a retail run. `Scr_BulletTrace` (Mac
  0x19bf2a) hands `G_LocationalTrace` no priority map. VERIFIED, the
  decompile.
- `G_RunFrame` (Mac 0x1af284) runs the script threads, then
  `G_RunFrameForEntity` over every entity, then `ClientEndFrame`.
  VERIFIED, the call order. So a link moves in the entity pass after the
  frame's cmds and before its end frame, and a line logged at a frame's
  time holds what the frame before posed. INFERRED, the order. The
  pinned replay below matches retail at that offset. VERIFIED, the gate.
- Pinned to retail's snapshots (the target's playerstate set from each
  one before its frame runs, `cod2_hitpose_ab::
  retail_bones_follow_a_knocked_body`), vcod's bones stand within 0.011
  units of retail's in all 31 frames, through the knockback's stumble,
  its blend and the torso anim after it. VERIFIED, the gate. Without the
  crouched stumbles' window 11 frames go off, the left wrist by up to 6.2
  units at 9000 and again from 9850, where the change back to the idle
  opens the window too. VERIFIED, the gate. Dividing by the float
  `moveSpeed` takes the frames past 0.002 from 23 to 16. VERIFIED, the
  gate. The 0.011 left is not explained.
- Retail's stumble share over the idle rises 0.125 a frame from the frame
  after the set, 1 at 400 ms, and its node time is the start phase plus
  each frame's `0.05 * rate / 0.6`, the rate off retail's own `trBase`.
  VERIFIED, a fit of vcod's pose on the capture (shares on a 0.01 grid).
- The capture does not replay through `cod2_combat_ab`: its first round's
  packet ran before the target's cmds 8741..8765 on retail (the target's
  snapshot at 8800 has moved 1.548 units) and after them in the replay,
  which orders packets by their newest cmd. VERIFIED, the capture and the
  replay.

### 9.5 The hit pose

The capture is `crates/cod2/tests/fixtures/hitpose/` (recipe in
`probe_hitpose.gsc`): a ppsh sighted and held, the gametype setting the
shooter's view on a new spot of a target that stands, crouches, lies
prone and strafes; every round logged and dropped, so nothing knocks or
flinches the body. Addresses are lnxded unless named Mac.

- The server keeps each client's body in a `clientInfo_t` at
  `0x87035bc + client * 0x4b8`, which `ClientConnect` (0x80f8e7a) zeroes,
  keeping +0x4a4 (the anim tree). VERIFIED, the memset. Nothing in
  `ClientSpawn` clears it, so the legs and torso yaw swing from 0 at the
  connect and keep their history across lives. INFERRED.
- `ClientEndFrame` (0x80f7516), after `BG_PlayerStateToEntityState(ps,
  ent, 1, 1)`, runs `BG_UpdatePlayerDObj` (0x80dbeb0) and 0x80dbdbc,
  which is the angle updater 0x80da9c8, the conditions 0x80dafa6, then
  the legs and torso lerp frames (0x80da42a on the entity's `legsAnim`
  and `torsoAnim`), with no controllers. VERIFIED, the calls.
- The angle updater reads the view at ci+0x3e8, which `ClientEndFrame`
  copies from ps+0xe8, the unsnapped `viewangles`. VERIFIED, the copy
  and the read. The lerp frames measure the rate off es+0x18,
  `pos.trBase`. VERIFIED, the read. Both of
  `BG_PlayerStateToEntityState`'s snaps (`pos.trBase`, `apos.trBase`)
  truncate toward zero: each `fistp` runs under the control word or-ed
  with 0xc00 (0x80dfd2b, 0x80dfdc3). VERIFIED, the disassembly. The wire
  carries the same: the strafing target's x 1929.648 goes out 1929.
  VERIFIED, the capture.
- `G_RunFrame` (0x810a13a) first steps every entity's anim tree by
  `level.frametime` times 0.001 (rodata 0x815ab4c) through 0x80920d0.
  VERIFIED, the loop and the constant. So a clip is sampled at its node
  time, as on 1.5's and UO's server (cod11-combat.md 3.1, "The clip's
  float time"). INFERRED.
- A trace builds the body's skeleton through `G_DObjCalcPose` (0x811e3c0)
  only when none exists under the current `SV_ResetSkeletonCache`
  (0x8091edc) stamp, which `SV_PacketEvent` (each client packet),
  `SV_RunFrame` and 0x8096952 (once before the bots' cmds) bump.
  VERIFIED, the test and the calls. The build runs the entity type's
  controller hook, for a player `G_PlayerController` (0x80f729a):
  `BG_Player_DoControllers` with `level.frametime`. VERIFIED, the call.
  So the controls ease one frame per build, and a body nobody traces
  keeps them; the shooter's own look trace (`Player_UpdateLookAtEntity`,
  9.2) builds his target every frame. INFERRED.
- `DObjTraceline` (Mac 0x76600) skips every model whose bit is set in the
  DObj's `+0x1a` mask, and `BG_UpdatePlayerDObj` passes each
  attachment's `ignoreCollision` bit. VERIFIED, the test and the loop.
  The stock characters and `probe_combat.gsc` attach the head and helmet
  with it, so their boxes take no round and a head shot reads the body's
  own `j_head` box. VERIFIED, the capture: retail never logs `helmet`,
  and vcod scored the helmet 2.5 to 4 units early until it skipped them.

vcod: `Combat`'s `ServerBody` per client (reset in `Cod2Level::connect`),
`PlayerBody::animate` in `end_frame`, `PlayerBody::ease_controls` and
`PlayerBody::pose_in(.., node_time)` once per stamp in `body_pose`,
`Cod2Level::packet` before each packet's cmds, `Character::ignore_collision`
stripping the boxes in `PlayerRig::assemble`. Before this vcod posed a
fresh body per trace (its legs 18 degrees off the view, its head and
neck eased one step from 0) and scored the helmet: every point 0.5 to 2.7
units nearer the shooter and the second round of `hit-head` and
`hit-legs` on another bone. `game::cod2_hitpose_ab` replays the capture's
67 rounds on the posed body: all strike retail's bone, 62 within 0.02
along their line. VERIFIED, the gate. Its `GAPS` names the rest: four in
the strafe, 0.06 to 0.26 off (where the round fell among the target's own
cmds is not logged, and the gate runs the target's cmds up to the
shooter's cmd time), and one crouched round 0.0204 off; the cause of
neither was found. Neither moves with the phase rule (9.3's variants), and a legs node
that faded to 0 restarts: one that kept its node time instead put 18 of
the 67 rounds off. VERIFIED, the gate.

### 9.4 The frag

- The throw matches to the bit: `pos` from the muzzle at
  `projectileSpeed` and `projectileSpeedUp`, `trTime` the frame.
  VERIFIED, `hit-throw`.
- `G_RunMissile` (Mac 0x1ca0e4): a bouncing missile (`eFlags`
  0x1000000) whose trace ran clear or met a floor (normal z over 0.7,
  rodata 0x326264) traces 1.5 down (0x3261b8); a world contact there
  replaces the trace, and the missile rides 1.5 above it, `trBase` z
  raised by the same. So `G_BounceMissile` reads the velocity at that
  trace's fraction of the frame. A trace that ran clear sets
  `groundEntityNum` 1023. VERIFIED, the decompile and `hit-throw`'s
  first bounce (z 65.625 over a floor at 64.125, the velocity to the
  bit). vcod rested the grenade on the floor and kept 1022 in the air.
- `G_BounceMissile` (Mac 0x1c9db0) reflects with -2 (0x3261e0), lifts
  0.1 along the normal (0x326388) with z only downward, and rests a
  bounce slower than 20 on a floor. VERIFIED, as vcod had it.
- The second bounce strikes the target's body (9.5) and leaves along
  the normal retail's does: the blast, its damage and the target's words
  after it match. With the target's idle clip on retail's clock (9.3)
  the flight after the body is off only in the last bits of `trBase[2]`
  (81.896675 against 81.896774 at 10050) and `trDelta` (6e-5 at most;
  `GAPS`), where it was 0.01 to 0.9 off with the strike 0.19 along the
  throw's path from retail's. VERIFIED, `hit-throw`'s frames
  (`COMBAT_MISSILES=hit-throw`). The first bounce's `trBase[1]` is a last
  bit off, INFERRED to be the trace's fraction.

### 9.6 The aim block and the game's `rand()`

Addresses are lnxded unless named Mac. `cod2_lnxded` is an unoptimized
gcc build: each assignment to a float is a store, each expression runs on
the x87 stack. VERIFIED, the disassembly of every function below.

- `ClientThink_real` (0x80f5d2e, Mac 0x1a3d00), before `Pmove`, on the
  cmd's ms (`level.time - commandTime` clamped to 200; under 1 the whole
  think returns): `BG_CalculateViewAngles` (0x80f24c8) into offsets added
  to `ps.viewangles`; `BG_CalculateWeaponPosition_Sway` (0x80f2524) at
  scale 1.0 on the gclient's +0x284c (the last view), +0x2858 (offset)
  and +0x2864 (angles); `BG_CalculateWeaponAngles` (0x80f1c60) on a copy
  of +0x2870..+0x2898 written back after. When `ps.weapon` is
  `aimDownSight` (+0x32c), `fWeaponPosFrac` is not 0 and
  `adsOverlayReticle` (+0x278) is `none`, the shot's angles are
  `AxisToAngles(AnglesToAxis(weapon) * AnglesToAxis(view + offsets))`
  (`MatrixMultiply` 0x80a58f2), else the view plus offsets; pitch and yaw
  go to +0x27d8, +0x27dc. VERIFIED, the calls and the stores.
- The state lives at gclient +0x2840..+0x289c: the damage kick's time,
  roll and pitch (+0x2840, +0x2844, +0x2848), the sway's last view,
  offset and angles, the movement tilt (+0x2870), the idle factor
  (+0x287c), the gun spring's angles (+0x2880) and speeds (+0x288c), the
  idle clock (+0x2898, an int the view's and the weapon's idle both
  step). `ClientSpawn`'s memset zeroes it (9.2). VERIFIED, the offsets
  and the memset's span.
- `ClientSpawn`'s own `ClientThink_real` (9.2, 100 ms) runs the block too,
  on the empty hands: the idle clock starts at 100 and the stance factor
  at 0.05, and the sway takes the spawn's view. VERIFIED, by replay: the
  rifle rounds' forwards match retail's to the printed digit only with
  it, the forward's z 1.6e-5 off without.
- `BG_CalculateViewAngles`: the damage kick (0x80f1d52: up over 100 ms,
  down over 400, scaled `1 - 0.5 frac`), an overlay's idle drift and walk
  bob (0x80f1ede, 0x80f21ba), the sighted view bob (0x80f237e: `frac *
  adsViewBobMult` (+0x288) of the vertical and horizontal bob at the flat
  speed, capped at 45, off a turret). VERIFIED.
- `BG_CalculateWeaponAngles`: the lean roll, `frac * adsAimPitch`
  (+0x46c), the movement tilt (0x80f0d74: the stance's `*Rot` past its
  `*RotMinSpeed`, eased at `posRotRate`, at least 0.1 a second), the idle
  drift (0x80f11b0: `hipIdleAmount` or 80, blended to `adsIdleAmount` by
  the frac, times the stance factor eased at 0.5 a second; the clock
  steps by `(int)(1000 * ms * speed)` under a truncating control word;
  sines of the clock at 0.0005, 0.0007 and 0.001 on roll, yaw, pitch
  times a hundredth), the walk bob (0x80f14aa: the cycle a quarter turn
  past `BG_GetBobCycle`, at 0.16 of the speed, capped at 10), the damage
  kick (0x80f166c), on an `aimDownSight` weapon the gun spring (0x80f1a6a:
  `BG_CalculateWeaponPosition_GunRecoil_SingleAngle` 0x80f1862 on pitch
  then yaw in 0.005 s slices, cod2-weapons.md's recoil keys), less the
  sway's angles. VERIFIED. The 0.01 to 0.02 degrees the rifle rounds
  left off the bare view are the idle drift. VERIFIED, by replay.
- The spring is kicked by `BG_WeaponFireRecoil` (0x80f095a) from
  `G_PlayerEvent` (0x80f7b38), which `g_smoothClients`' entity copy
  (0x80f56f8) runs after `Pmove` for each new event; events 158, 159, 160
  and 175 kick. Four `G_random()` draws: view pitch, view yaw (both
  dropped), gun pitch, gun yaw, each `min + (max - min) * G_random()`,
  stored. The view kick takes the `ads` ranges at a frac of 1.0, the gun
  kick at any frac above 0; the scale is 1, or 0.01 times
  `*GunKickReducedKickPercent` while `weaponRestrictKickTime` is above 0.
  VERIFIED, the decompile; the Mac build's branches differ.
- `G_random` (0x80a3774) is `rand()` over 2147483648.0 (rodata
  0x814cfe4), on the stack. VERIFIED. `G_InitGame` (0x8109096) seeds
  `srand` with its second argument, which the caller (0x80925e6) takes
  from `Sys_MillisecondsRaw` (0x80d5e20): `tv_sec * 1000 + tv_usec /
  1000` of `gettimeofday`, the wall clock in ms, cut to 32 bits.
  VERIFIED, the decompile.
- `BG_AnimScriptEvent` (0x80d96ee) draws `rand() % n` for every matching
  clause with a command, one command or more. VERIFIED. So a fire event's
  torso anim draws once inside `Pmove`, before the spring's four.
  INFERRED, the call order; VERIFIED, by replay below.
- A held burst draws seven a round: the torso event, the spring's four,
  then `Bullet_Endpos`' two in `ClientEvents`. VERIFIED: glibc `rand()`
  seeded 1679727541 (wall clock 1792681089973 ms) gives `hit-smg`'s six
  rounds' spread draws, read back off their points, at draws 10, 17, ...
  45, and the replay pinned there (`Combat::pin_rand`, the gate's
  `SEEDS`, five draws before its start) sends every round along retail's
  forward to the printed digit and through retail's line.
  `recover_spread_seed` (ignored, `COMBAT_SEED_FROM`/`_TO` in Unix
  seconds) is the search.

vcod: `vcod_cod2::pmove::aim` (`AimState::fire_angles` in
`Cod2Level::client_think` and the spawn's think, `AimState::fire_recoil`
from `Combat::fire_recoil` after the step), the damage kick in
`damage_feedback`, `Combat::rand` the one `rand()` (the torso events'
picks, the spring, the spread, the grenade's tumble). Not done: the
overlay's idle drift and walk bob on the view (no stock shot reads them:
an overlay's sight skips the multiply, and its view's offsets are the
client's), the shellshock scale on the sway, the draws pmove's own anim
events (jump, land) make, the challenges' and the other `rand()` users'.
`hit-smg-far`'s and `hit-turret`'s seeds are not recovered: their rounds
mostly strike the world, whose impact events carry whole units, too coarse
at 1340 units for the search (`GAPS`, "9.6 a seed not recovered").
