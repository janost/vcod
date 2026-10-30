# CoD2 1.3 MP: mounted guns

How a `misc_turret` works on the CoD2 1.3 server and client: the spawn, the
use key's mount, the gunner's pmove, the frame that aims, places, fires and
sounds the gun, the release, and the gunner's first-person eye. CoD1's
`cod11-turrets.md` is the model; this document records where CoD2 differs
and what vcod does.

No document-level label: every claim carries its own. Addresses are `cod2_lnxded` (1.3 Linux
dedicated server, the game module linked in) unless marked Mac, which is
the 1.3 Mac client (`cod2mp_mac13_i386`, cgame inside). Unnamed statics are
named here for what their bytes do. `ent` is a gentity (stride 0x230,
`g_entities` 0x8716400), `cl = ent+0x158` its client (the playerstate at
`cl+0`), `rec = ent+0x15c` a turret's record.

| offset | meaning | evidence |
|---|---|---|
| `ent+0x68..0x70` | `s.angles2`: barrel pitch, yaw, a slew carry | VERIFIED, `turret_use` 0x810c3de stores 0x68/0x6c/0x70; meanings INFERRED from `turret_controller` 0x810c10e |
| `ent+0x74` | `s.otherEntityNum` | VERIFIED, entity netfield; `turret_use` writes the gun's into the player and the player's into the gun |
| `ent+0x84` | `s.loopSound` | VERIFIED, 0x810b926 |
| `ent+0x138` / `+0x144` | `r.currentOrigin` / `r.currentAngles` | VERIFIED, `G_SpawnTurret` `G_SetOrigin`/`G_SetAngle` arguments |
| `ent+0x150` | `r.ownerNum`, 0x3ff for none | VERIFIED, `turret_use` and `G_ClientStopUsingTurret` 0x810b9a4 |
| `ent+0x162` | busy byte: 1 on a gun, 2 a release asked for | VERIFIED, `turret_use`, `Player_ActivateCmd` 0x8121174 |
| `cl+0x10c` / `+0x114` | `viewAngleClampBase[2]` / `viewAngleClampRange[2]` | VERIFIED, player netfields 268..280 |
| `cl+0x590` / `+0x594` | `viewlocked` / `viewlocked_entNum` | VERIFIED, player netfields 1424, 1428 |
| `cl+0x598` / `+0x59c` / `+0x5a0` | `cursorHint` / `cursorHintString` / the hint's entity | VERIFIED, netfields 1432 and 1436; `+0x5a0` is not sent |
| `cl+0x27bc` / `+0x27c0` / `+0x27c4` | the cmd's buttons, the last cmd's, the pressed edge | INFERRED from `FUN_08121314`'s tests |
| `cl+0x2830` | the use target | VERIFIED, `Player_ActivateCmd` |

The record: 32 of 0x44 bytes at 0x8844e00. VERIFIED, `G_SpawnTurret`
0x810c6b2's scan, `memset(rec, 0, 0x44)` and `Com_Error` on the 33rd.

| off | meaning | evidence |
|---|---|---|
| `+0x04` | flags: 3 at spawn, 0x800 a mount not yet aimed | VERIFIED, the stores |
| `+0x08` | fire cooldown, ms | VERIFIED, 0x810b850 |
| `+0x0c` / `+0x14` | pitch min (`-toparc`, at most 0) / max (`bottomarc`, at least 0) | VERIFIED, `G_SpawnTurret` |
| `+0x10` / `+0x18` | yaw min (`-rightarc`) / max (`leftarc`) | VERIFIED, same |
| `+0x1c` | rest pitch, -90 at spawn, then `turret_think_init`'s sweep | VERIFIED, the store of `0xc2b40000` and 0x810bf00 |
| `+0x20` | gun stance (def+0x8c): 0 stand, 1 duck, 2 prone | VERIFIED, the load; the order INFERRED from `turret_use`'s `eFlags` arms |
| `+0x24` | the player's stance at mount, -1 none | VERIFIED, `turret_use`, `G_ClientStopUsingTurret` |
| `+0x28` | loop timer, ms | VERIFIED, 0x810b7e0, 0x810b926 |
| `+0x2c..0x34` | the player's origin at mount | VERIFIED, `turret_use` |
| `+0x38` | `playerSpread` (def+0x540, or the key) | VERIFIED, `G_SpawnTurret` |
| `+0x40..0x43` | sound alias indices: `loopFireSound`, `loopFireSoundPlayer`, `stopFireSound`, `stopFireSoundPlayer` (def+0xb0..0xbc) | VERIFIED, `G_SpawnTurret`; the keys from `vcod_cod2::weapon`'s field table |

## 1. Placements

The stock maps place `misc_turret` with `weaponinfo` `mg42_bipod_stand_mp`
or `30cal_stand_mp` (mp_carentan's mg42 at (1718 1824 5), yaw 233.1;
mp_brecourt, mp_burgundy, mp_dawnville, mp_decoy, mp_downtown,
mp_farmhouse and others). VERIFIED, the entity lumps. The stand files hold
`leftArc`/`rightArc` 45, `topArc`/`bottomArc` 15, `fireTime` 0.05,
`playerSpread` 0.5, `animHorRotateInc` 15, `useHintString`
`PLATFORM_HOLD_TO_USE`, `dropHintString` `PLATFORM_HOLD_TO_DROP`, and no
overheat key. VERIFIED, `weapons/mp/mg42_bipod_stand_mp`.

## 2. Spawn

VERIFIED, `G_SpawnTurret` 0x810c6b2, in order: the record; `s.weapon =
G_GetWeaponIndexForName(weaponinfo)`, which registers the weapon and, inside
`G_RegisterWeapon` 0x8107216, its use then its drop hint string
(`G_GetHintStringIndex` 0x8112432: the first of 32 configstrings from
1278 that holds it or is empty); a `Scr_Error` unless the
def's class (def+0x7c) is 7, turret; the four sound aliases; the arcs and
`damage` and `playerSpread` keys; flags 3, `takedamage` 1, contents
0x200004, `eType` 9; bounds (-32,-32,0)..(32,32,56); `apos.trType` 3 and
the think 50 ms on. INFERRED, that the weapon index comes before anything
else the level registers off the entity: the capture's gun carries weapon
2, `defaultweapon_mp` (`G_SetupWeaponDef` 0x8120d74) taking 1.

VERIFIED, `GScr_SpawnTurret` 0x811138c: `spawnTurret(classname, origin,
weapon)` is `G_Spawn`, the classname, the origin, `G_SpawnTurret` and the
entity returned; `GScr_PrecacheTurret` 0x811141a is
`G_GetWeaponIndexForName` behind the before-any-wait check. VERIFIED, Mac
`G_SpawnTurret` 0x1bc84a reads each arc, `damage` and `playerSpread` key
only under a level byte (`level+0x1348`) and takes the def's otherwise;
INFERRED, that the byte is `spawnVarsValid`, clear outside the map load,
so a script gun takes its arcs off the weapon file. A gun spawned with no model has no DObj until
`setModel` (`ScrCmd_SetModel`, Mac 0x19e778: `G_SetModel`, `G_DObjUpdate`,
the link); `turret_think_init` runs 50 ms on and sweeps the rest pitch off
that DObj. VERIFIED (capture, `toujane-prone.txt`): a gun spawned and
modelled in the start callback rests at -15 on mp_toujane's floor.
vcod's `spawnTurret` and `precacheTurret` are those (`builtins::cod2`,
`spawn::spawn_turret`); a `setModel` on a gun whose think has not run yet
takes its tags and sweeps its rest pitch (`spawn::turret_model_changed`).

vcod-server registers the weapon and the four aliases at the spawn on CoD2
(`game::spawn`, under `HostProfile::turret_weapon_and_player_sounds`); CoD1
keeps its two. `defaultweapon_mp` is registered first at the load
(`game::script`, `WeaponNumbering::Registered`), so the gun reads weapon 2
and its hint `weapon + 4` as retail's.

## 3. Use key

VERIFIED, `FUN_08121314` (called last in `ClientThink_real`): off weapon
states 17..22; with a target set, the `+usereload` bit 0x20 released
since the last cmd asks for a reload (`pm_flags` 0x8) and skips the rest; a press of 0x28 calls `Player_ActivateCmd`; then,
with a target and 0x28 held, `Player_ActivateHoldCmd`. The whole
function, items and use triggers included, is cod2-gsc-diff.md 5.4.

VERIFIED, `Player_ActivateCmd` 0x8121174: the target cleared; on a player
whose busy byte is set, the byte becomes 2 if `eFlags & 0x300`, else 0; off a
gun and not mantling, the target is the hint's entity (`cl+0x5a0`) and the
press time is kept. `Player_ActivateHoldCmd` 0x8121264 uses the target
once `g_useholdtime` has passed since the press. VERIFIED, its default 0:
`Dvar_RegisterInt("g_useholdtime", 0, ..)`. INFERRED, a press uses the
target inside its own cmd, so CoD2's "hold to use" is a tap.

VERIFIED, `Player_UseEntity` 0x8121054: an `eType` 9 target is used only if
`G_IsTurretUsable`; the gun is notified `trigger` with the player, then its
use callback, `turret_use`, runs; the target is cleared.

VERIFIED, `G_IsTurretUsable` 0x810c358: the gun's busy byte 0, a record, the
arc test 0x810c1be, no grenade in hand (`ps+0x3c`) and on the ground. The
arc test is CoD1's (cod11-turrets.md 4.2): the yaw arc's centre, `acos` of
the flat direction to the gun times 180/pi (double 0x815ac60) against half
the arc. vcod uses `game::turret::usable` for both.

## 4. Mount

VERIFIED, `turret_use` 0x810c3de, the stores in order: both busy bytes 1;
`r.ownerNum`; `viewlocked` 1 and `viewlocked_entNum`; `rec+4 |= 0x800`; the
mount origin; the two `otherEntityNum`s; the stance (`pm_flags` 1 prone 2,
2 crouch 1, else 0); `eFlags` 0x100 for a prone gun, 0x200 duck, 0x300
stand; `angles2` = the view less the gun's angles, each clamped by
0x810ccf2 (two `fsel`s, the high bound first), `angles2[2]` 0; then
`viewAngleClampRange[i] = 0.5 * AngleSubtract(max, min)` (rodata 0x815ac68)
and `viewAngleClampBase[i] = AngleNormalize360Accurate(gun angle + max -
range)`, pitch then yaw.

Where CoD1 snaps the view to the barrel with `SetClientViewAngle`, CoD2
calls nothing on the view: pmove's clamp holds it (section 5). VERIFIED,
no call in the function. VERIFIED (capture): the mount snapshot carries
`eFlags` 0x302, `viewlocked` 1, `viewlocked_entNum` 295, the clamp (0,
233.1) and (15, 45), the gun's `angles2` (0, -0.003) and its teleport bit.

## 5. Mounted pmove

VERIFIED, `PM_UpdateViewAngles` 0x80e6eb8: after the view is built off
`delta_angles` and the cmd (pitch capped as ever), a player with `eFlags &
0x300` takes its own arm: for pitch then yaw, `AngleDelta(base, view)`
past `range` either way has the excess pushed into `delta_angles` (`(int)(
182.04445 * excess) & 0xffff` added, the sum unmasked) and the view set to
`AngleNormalize360Accurate(base -+ range)`. The prone arm, the ladder cap
and the lean are not run. VERIFIED, the branch at the function's tail.
`vcod_cod2::pmove`'s `turret_clamp` is that arm; the client predicts
through it off the snapshot's clamp fields.

`PmoveSingle`'s gunner arm (`linked`), `PM_Weapon`'s early return on
`eFlags & 0x300`, the forced stance and the null lean were already in
`vcod_cod2::pmove` (cod2-movement.md).

VERIFIED, `PmoveSingle` 0x80e8b56..: `eFlags & 0x40` (`EF_FIRING`) is
cleared, then set when `pm_type` is not 5, `pm_flags & 0x1000` is clear,
`weaponstate` is 0 or 3, `PM_WeaponAmmoAvailable` (the held weapon's clip,
0x80ed5fc) and the cmd holds attack. A gunner's carried rifle counts, and
the anim script's `mounted mg42, firing` reads the bit. vcod-server calls
`pmove::weapon::update_firing_flag` before each cmd's move. VERIFIED
(capture): the gunner's `eFlags` reads 0x342 on every frame the trigger is
held.

## 6. The cursor hint

VERIFIED, `FUN_08121bc6` (from `ClientEndFrame`): the hint, its string and
its entity reset to 0, -1 and 0x3ff; alive and off weapon states 17..22, a
player off a gun walks `Player_GetUseList` 0x8121494 and a usable turret
first in it hints `weapon + 4` with the use hint string (def+0x574); a
gunner takes `Player_SetTurretDropHint` 0x8121b36: no entity, `weapon +
4`, the drop hint string (def+0x578).

VERIFIED, `Player_GetUseList`: entities with contents 0x200000 in a box of
(192, 192, 96) about the eye; one whose bounds centre is within 128
(rodata 0x815ee60) scores `256 * (1 - 0.5 * (dot + 1))` plus its distance
(0x815ee58, 0x815ee5c); a turret is traced to from the eye at its `tag_aim`
and one the trace does not reach takes 10000 (0x815ee64) and drops out; the
list is sorted by score. vcod walks the one use list of
cod2-gsc-diff.md 5.4, guns, items and use triggers together.

VERIFIED (capture): `cursorHint` 6 and `cursorHintString` 0 before the
mount, 1 (the drop hint) while mounted, 0 after.

## 7. The gunner's frame

VERIFIED, `ClientEndFrame`: with `pm_flags & 0x800000` and `eFlags &
0x300`, `turret_think_client` (0x810bb0a) on `g_entities[viewlocked_entNum]`
after the hint pass. With the gunner's busy byte 1 and `sessionstate` 0 it
runs the mounted frame and the loop sound; anything else releases.

VERIFIED, the mounted frame 0x810b850 in order:

1. The aim 0x810b6ce: `viewlocked` 1, `viewlocked_entNum`, `angles2` the
   view less the gun's angles clamped as at the mount, `angles2[2]` 0, and
   on 0x800 the flag cleared and the gun's `eFlags ^= 2`. No 15-degree rate
   limit and no view write, unlike CoD1's.
2. The body placement 0x810aa50: the gunner's anim group (`clientinfo`'s
   legs anim less 0x200) blended by the barrel yaw over
   `animHorRotateInc` (def+0x564) and by the body's height above
   `tag_weapon`, hung off `tag_weapon` turned by the gun; the origin set,
   then a capsule trace from the eye height (`ps+0xf8`) down to 60 under it
   (rodata 0x815ac4c) under mask 0x2810011 sets its height; `r.currentAngles`
   the body's. The same walk as CoD1's 0x515a8; vcod uses
   `turretpose::place_gunner` for both. vcod hands it the legs anim pmove's anim
   script picked (`mounted mg42` and its `firing` clause: `standMG42_aim`,
   `standMG42_fire`, `proneMG42_*`), which goes on the wire as `legsAnim`.
3. `viewlocked` 1, the gun's `eFlags &= ~0x40`, the cooldown down 50; at
   or under 0 it is 0 and, with the frame's last cmd holding attack, the
   cooldown takes `fireTime` (def+0x204), the gun's `eFlags |= 0x40` and
   0x810b7e0 fires.

INFERRED, from the capture: the placement reads the barrel a frame behind
the aim. The gun's `tag_weapon` comes off its DObj pose, which the frame
built before the aim moved `angles2`; every placed origin replays to
1e-5 only when placed off the previous frame's `angles2`.

VERIFIED, 0x810b7e0: the loop timer at three `fireTime`s; with a client,
`Turret_FillWeaponParms` 0x810a880 (the eye `G_GetPlayerViewOrigin`, whose
gunner arm is the gun's `tag_player`, 0x80f8916; the view's axes; the
muzzle along the view as far as `tag_flash` is from the eye), then
`Bullet_Fire` 0x8120484 with `rec+0x38` as spread and `level.time` (no
antilag), or `Weapon_RocketLauncher_Fire` for a non-bullet def, and
`G_AddEvent(gun, 175, attacker)` (`EV_FIRE_WEAPON_MG42`); `viewlocked` 2.

VERIFIED, the loop sound 0x810b926: `loopSound` 0; while the timer is
positive the loop alias and the timer down 50; at or under 0 with a stop
alias, `loopSound` 0 and `G_PlaySoundAlias` (event 179 on the gun with the
alias). VERIFIED (capture): the held trigger fires 20 rounds in a second,
one event a frame, `loopSound` on from the first and the stop alias two
frames past the last.

VERIFIED, absence: no `overheat` string in `cod2_lnxded`, and no record
field past 0x43. INFERRED, CoD2's guns never overheat or run dry.

`turret_think` 0x810be6e runs for an unmanned gun only (its owner has no
client): the loop sound and the walk home at 200 degrees a second
(0x810bb76, the slew CoD1 has), which `GameHost::run_turret_think` already
does. VERIFIED (capture): from the frame after the release the barrel steps
10 degrees a frame on each axis back to (-63, 0).

## 8. Release

VERIFIED, `G_ClientStopUsingTurret` 0x810b9a4, in order: the loop timer and
`loopSound` 0; for a kept stance `EV_STANCE_FORCE_PRONE`/`_CROUCH`/`_STAND`
(142, 141, 140) on the player, the stance -1; `TeleportPlayer` 0x810a708 to
the mount origin raised 1 with the teleport bit flipped and the view at the
player's `r.currentAngles`, which each think leaves (0, view yaw, 0);
`eFlags &= ~0x300`, `viewlocked` 0, `viewlocked_entNum` 0x3ff, the player's
busy byte and `otherEntityNum` 0, the gun's busy byte 0, its owner 0x3ff,
`rec+4 &= ~0x800`. VERIFIED (capture): the release snapshot's origin is the
mount spot raised 1, its pitch 0, event 140.

The player's free (0x80f910e) releases too. vcod also releases a gun whose
gunner is gone or respawned off it. INFERRED.

## 9. The gunner's eye

VERIFIED, Mac `CG_CalcViewValues` 0x1d3b28: with `eFlags & 0x300` the view
origin is the ridden gun's `tag_player` (`CG_DObjGetWorldTagPos`,
`Com_Error` without it); on `viewlocked` 2 out of third person the view's
pitch and yaw take `crandom()` times `vertViewJitter` (def+0x580) and
`horizViewJitter` (def+0x57c); the bob, lean and kick offsets are skipped.
vcod's client takes the eye from the gun's bind `tag_aim` and `tag_player`,
the barrel turned to the predicted view
(`play::cod2_predict::gunner_eye`). The jitter: VERIFIED, Mac 0x1d3b28
reads `viewlocked` (ps+0x590) for the 2 and the gun's `s.weapon`
(`cg_entities[viewlocked_entNum]`, centity+0x1b8) for the def, and adds
`crandom()` to `cg.refdefViewAngles` only, never to the cmd's view.
vcod draws the same each frame (`Cod2Session::gun_jitter`); the stock
MG42 files carry 0.15 degrees each way, the 30cal ones 0.5. The server
sets `viewlocked` 2 only on a frame its gun fires (section 7), so a held
trigger jitters the view and a cooldown frame does not. The view weapon is already
hidden on a gun (`cod2_viewmodel`).

## 10. The capture and the gate

`crates/cod2/tests/fixtures/turret/carentan-mg42.txt`: `move_probe --script
turret` beside retail `cod2_lnxded` under `probe_turret.gsc`, which spawns
the player 40 units behind the gun nearest the given spot (recipe in the
fixture's header). The script taps use, looks inside the arcs and past
them each way, holds the trigger a second, taps it, fires while turning,
taps use again and looks about. Each snapshot logs the playerstate and
the gun's entity (`e` lines).

`game::cod2_turret_ab` replays it through `Cod2Level` (each snapshot's
cmds, then its frame) and compares every snapshot: the gunner flags,
`viewlocked` and its entity, the clamp, the cursor hint's presence and
string, the view (worst 0.004 degrees), the origin (worst 1e-5), the
barrel (exact), the gun's firing bit, loop sound and event ids, the
gunner's `legsAnim` (toggle aside), the gun's `weapon` and `loopSound`,
and the release's stance event. Left out: `delta_angles`, which
`SetClientViewAngle` leaves unmasked on retail.

Sections 13, 14 and 16 add six captures under the same gate: a prone
and a duck mg42 and a prone 30cal, a gunner killed on the gun, one shot
off it while he fires, and two players at one gun. A two-client capture
(`SECOND=`, the second client joined first as slot 0) replays both
clients' packets in the order they were sent and compares both
playerstates. A round's hit on a target is section 11's.

## 11. A round on a body

VERIFIED, 0x810a982 (the round 0x810b7e0 fires): `Bullet_Fire` 0x8120484
gets the gunner as the attacker and the gun as the entity it skips, and
`Bullet_Fire_Extended` 0x811fe90 then calls `G_Damage(target, attacker,
attacker, ...)`: the gunner is the inflictor too, never the gun. The round's
weapon parms are the gun's def (its `damage`, `rifleBullet`,
`armorPiercing`): 0x810a982 stores `BG_GetWeaponDef(gun.s.weapon)` in the
local that sits at the parms' +0x3c. A world impact's `otherEntityNum`
(te+0x74) is the gun's number.

VERIFIED, `Scr_PlayerDamage` 0x8101b40: the weapon the damage callback is
handed, and whose `loc*` multipliers (def+0x5b0) scale the damage when it is
a bullet def (def+0x78 0), is the inflictor's `s.weapon` (ent+200), which
`BG_PlayerStateToEntityState` 0x80dfcc2 copies from `ps.weapon`: the weapon
in the gunner's hands. VERIFIED, the stock `weapons/mp/mg42_bipod_*_mp`
and `30cal_*_mp` files carry no `loc*` keys, and the weapon file loader
0x80f3382 starts from a zeroed def, so the gun's own multipliers are all 0.
Trap: reading the gun's multipliers makes every turret round on a body do
0 and the callback never run.

VERIFIED, `player_die` 0x810175a: when the callback's weapon is not 0, the
attacker is a client with `eFlags & 0x300` and the entity his
`otherEntityNum` (`viewlocked_entNum`) names has `eType` 9, the killed
callback gets that gun's `s.weapon` instead: the obituary shows the gun.

VERIFIED (capture `crates/cod2/tests/fixtures/combat/hit-turret.txt`,
`move_probe --script mgfire` beside a `still` target under
`probe_combat.gsc` with `probe_turret 250`: the gunner behind mp_carentan's
gun 295 with the kar98k, the target 250 units out along its yaw, mortal):
the first two rounds strike `torso_lower` for 52 (35 times the kar98k's
1.5), `MOD_RIFLE_BULLET`, flags 32 (penetration), weapon `kar98k_mp`; the
second kills, and `PROBE killed` reads `mg42_bipod_stand_mp`. Each body hit
also raises a world impact behind the body (the round goes on at half).
`game::cod2_combat_ab` replays it; vcod matches the damage, the weapons, the
kill and the events up to the death.

Left as gaps in that gate (`GAPS`, "cod2-turrets.md 11"):

- a dead body left playing: `probe_combat.gsc`'s killed callback leaves
  the target's `sessionstate` playing, so `ClientEndFrame` puts
  `CONTENTS_BODY` back on him (`G_SetClientContents` 0x80f5682) and retail's
  rounds strike the corpse from a second after the death, knocking it
  along. vcod's trace passes a dead body. A stock gametype sets the dead
  `sessionstate` first, so play never meets this. INFERRED.
- `weapAnim` on the gun, fixed: `PmoveSingle`'s mounted arm (lnxded
  0x80e8934, `eFlags & 0x300` under `pm_type` 0) runs 0x80f0946 where the
  other arms run `PM_Weapon`, and that is 0x80ef946: `pm_flags` 0x10 and
  0x800 off, `PM_ExitBinocularsQuick` (0x80effda) from weapon states
  17..22, `weaponTime`, `weaponDelay` and `weaponstate` 0 and
  `PM_StartWeaponAnim(ps, 0)`. So the toggle flips every cmd on a gun, and
  a snapshot shows its parity. VERIFIED, the calls and the stores, and the
  capture's 0x200 now matching in every row. vcod:
  `vcod_cod2::pmove::weapon::pm_weapon`'s mounted return.

## 12. The gunner's crosshair and the gun's tracers

VERIFIED, Mac `CG_DrawCrosshair` 0x1d0784: with `eFlags & 0x300`, the HUD
menus shown and `viewlocked_entNum` (cg+0x26158) not 0x3ff, it draws
`CG_DrawTurretCrossHair` 0x1cf59c and nothing else: under
`cg_drawTurretCrosshair` (registered 1) and not paused, the gun entity's
`s.weapon` def's `reticleCenter` material (cg_weapons+0x178), a square of
`reticleCenterSize` (def+0x120) centred on the screen (both aligns 2),
coloured (0.25, 1, 0.25) on `pm_flags & 0x100000` (a friend under the
aim) and white otherwise, at `cg_crosshairAlpha` (registered 1.0). The stock MG42 and 30cal
files name `gfx/reticle/mg42_cross.tga` at 32. vcod:
`cod2_player_hud`'s `turret_crosshair`.

VERIFIED, Mac `CG_BulletHitEvent` / `CG_BulletHitClientEvent` 0x1d9b78:
the tracer starts at `CG_CalcMuzzlePoint` 0x1d7f72 of the impact's
`otherEntityNum`: the local first-person client's own number reads the
view origin and draws no tracer, any other entity its DObj's
`tag_flash`. A turret round's world impact names the gun (section 11),
so its tracer leaves the gun's `tag_flash` and the gunner sees his own;
the flesh impact `finishPlayerDamage` raises names the gunner, whose own
first-person skip holds. vcod's tracers already ran off those events
(`play::cod2_fx`); the gun's drawn `tag_flash` joins the bodies' in
`Cod2Draw::muzzles`. A world impact that named the gunner instead would
leave the gunner seeing no tracer.

## 13. Prone and duck guns

No stock MP map places one: every `misc_turret`/`misc_mg42` of the
stock maps carries `mg42_bipod_stand_mp` or `30cal_stand_mp`. VERIFIED, the
entity lumps of `iw_*.iwd`. The paks ship `mg42_bipod_prone_mp`,
`mg42_bipod_duck_mp` and `30cal_prone_mp` (`iw_13.iwd`) with `stance`
`prone`, `duck` and `prone`: the mg42 pair keeps the stand file's 45/45/15/15
arcs, `playerSpread` 0.5 and 0.15 jitters; `30cal_prone_mp` has 30/30/10/10,
spread 1 and 0.5 jitters. VERIFIED, the weapon files.

The captures: `toujane-prone.txt` and `toujane-duck.txt`, `move_probe
--script turret` beside retail under `probe_turret.gsc` with `probe_gun`
naming the file, which `spawnTurret`s it on mp_toujane's floor at (-299,
1080) facing 90 and spawns the client 56 behind it (recipe in each
header). VERIFIED (captures):

- The use hint reads `cursorHint` 8 (the gun's weapon 4, plus 4).
- The mount snapshot: `pm_flags` 2 (crouched) and `eFlags` 0x106 for the
  prone gun, 0x206 for the duck one (the gun's stance, the crouch bit and
  the teleport bit); the clamp ranges (15, 45) about base yaw 90. A
  snapshot on, the prone gunner is down: `pm_flags` 0x880001, `eFlags`
  0x10a, `viewHeightTarget` 11.
- The release raises `EV_STANCE_FORCE_STAND` (140), the stance the client
  mounted from, and the prone body stands through a crouch snapshot.

`game::cod2_turret_ab`'s `PRONE` and `DUCK` replay both; vcod matches
every compared field (worst view 0.004 degrees, origin 1e-4) with no
change to the mount or the frame: `turret_use`'s stance arms and the
forced stance in `vcod_cod2::pmove` were already right.

`toujane-30cal.txt` is the same recipe with `probe_gun 30cal_prone_mp`
and `probe_gunmodel xmodel/weapon_30cal`. VERIFIED (capture): the
script's trigger fires 17 `EV_FIRE_WEAPON_MG42` where the prone mg42's
fires 33, a round each `fireTime` (the 30cal's twice the mg42's 0.05);
the mount, the prone drop and the release read as the mg42's. `CAL30`
replays it with no difference.

## 14. A gunner killed on the gun

`carentan-die.txt`: `move_probe --script turretdie` on mp_carentan's
mg42 under `probe_turret.gsc` with `probe_die 1500`: 1.5 s after the
mount the gunner `suicide()`s; the killed callback sets the stock dead
session and `cloneplayer`s the body, and two seconds on he spawns again
behind the gun. VERIFIED, `player_die` (lnxded 0x810175a) returns at once
for a client with no server DObj: a body with no model never dies, so the
probe gives the gunner the stock German model first.

VERIFIED (capture), the death frame (7550):

- The killed callback ran inside it, and the corpse it cloned (entity 64)
  carries `eFlags` 0xa0302: the gun's stance bits as the gunner's entity
  last had them, at the placed origin on the gun.
- The gunner's playerstate is released in the same frame's end frame
  (`turret_think_client` sees `sessionstate` not playing): `pm_type` 6,
  origin the mount spot raised 1, `EV_STANCE_FORCE_STAND`, `eFlags` 0,
  `viewlocked` 0 and 1023, the hint 0 and its string -1.
- The gun walks home from the next frame, 10 degrees a frame (section 7).
- A dead body's hint pass still resets `cursorHintString` to -1; only a
  spectator's and the intermission's keep `ClientSpawn`'s 0. vcod's
  `cod2_wire` zeroes it for `pm_type` 4 and 5 only.
- The respawn snapshot's `pm_flags` is 0x801000: `ClientSpawn` (Mac
  0x1930a6) sets 0x1000, which holds `EF_FIRING` off while the trigger
  stays down; it goes once attack is let go. vcod's CoD2 spawn sets it
  too (without it the first cmd after the respawn fires).

The probe's `wait ms / 1000` waits 1.5 s on retail: CoD2's
`Scr_EvalDivide` (Mac 0x9012c) divides two ints as floats, where CoD1's
truncates. VERIFIED, the decompile and the capture's timing. vcod's gsc
compiler emits `Op::DivFloat` for `/` under `Dialect::Cod2`.

`game::cod2_turret_ab`'s `DIE` replays it and matches every compared
field through the death, the dead frames and the respawn.

### 14.1 Shot off the gun while firing

`carentan-shot.txt` and `-b.txt`: `SECOND=mgkill` `turretfire` under
`probe_turret.gsc` with `probe_shooter 200`, `probe_weapon kar98k_mp`,
`probe_aimz 40`: client 0 stands 200 in front of the gun and its sighted
kar98k rounds go through `finishPlayerDamage`; the gunner holds the
trigger from a second after the mount. VERIFIED (capture), the death
frame (10550; the round lands in the shooter's cmd at 10512, `PROBE
damage` 135 `torso_upper`, and the gunner's three cmds of that frame run
after it):

- The dead gunner's `eFlags` reads 0x40: the release clears 0x300 and
  flips the teleport bit, and `EF_FIRING` is the one his dead cmds set.
  VERIFIED, lnxded `PmoveSingle` (0x80e8b56..): its default arm, which
  `pm_type` 6 takes as 0 does, runs the mounted reset (0x80f0946) in place
  of `PM_Weapon` while `eFlags & 0x300`, so the weapon stays in hand (it
  goes to 0 the next frame) and the firing test still finds rounds.
  vcod's `pm_weapon` takes the mounted reset first on `pm_type` 0 and 6.
- The gun keeps `eFlags` 0x40 that frame and `loopSound` goes to 0: no
  store in `G_ClientStopUsingTurret` (lnxded 0x810b9a4) touches the gun's
  `eFlags`, VERIFIED, and its next `turret_think` clears it (10600). vcod
  leaves the bit to the think too.
- No stop alias plays: no event rides the gun from the death on.
  INFERRED, the release zeroes the loop timer, so the unmanned think's
  loop pass never runs out with a positive timer.

`SHOT` replays it with no difference. vcod's shooter kills with the same
round only because the aim at the torso leaves room for the shot-angle
gap of `cod2-combat.md` 9.3; an aim at the head hit on a different round.

### 14.2 Two players at one gun

`carentan-share.txt` and `-b.txt`: `SECOND=turretwait` `turretshare`
under `probe_turret.gsc` with `probe_side 36` (client 0 spawns 36 to the
gun's left of the spot behind it, 42 degrees off its yaw). Client 1
mounts, fires and lets go, then backs off; client 0 taps use while the
gun is manned and again once it is free. VERIFIED (capture):

- While the gun is manned the other player's `cursorHint` is 0 and his
  string -1: `G_IsTurretUsable` (0x810c358) fails on the gun's busy byte,
  so the hint walk passes the gun. His use tap does nothing: no `trigger`
  notify, no mount.
- A snapshot after the release both players' hints read 6 again, and the
  second's next tap mounts him; the first's hint goes to 0 then.
- Who gets a free gun: `Player_UseEntity` (0x8121054) asks
  `G_IsTurretUsable` at the use and `turret_use` sets the busy byte, so the
  first `ClientThink_real` to press takes it and a later press that frame
  is refused. INFERRED, from the order of the calls; no capture puts two
  presses in one frame.

`SHARE` replays it with no difference. With the second player 32 to the
side the gunner releases onto his spot, and `ClientEndFrame` runs `StuckInClient`'s CoD2 form (lnxded 0x80f6e9e,
after the hint pass and the entity copy, for a player with health) on
two players whose boxes and capsule radii overlap: both get the
separation plus a `G_crandom()` per axis, normalized, times
`g_playerCollisionEjectSpeed` (25, `Dvar_RegisterInt` at 111819 of the
export) for each one whose 2D velocity is not 0, `ps.speed` for both when
both are under 1e-4 (rodata 0x8157aac), as their velocity, `pm_time` 300,
`pm_flags |= 0x200`, and the caller's contents 0x4000000. VERIFIED, the
disassembly. The capture shows a placed gunner shove a standing player
25 a second and a mounting one shove the released gunner at 190.
INFERRED, that the first is the speed path and not understood: both read
still on the wire.

The boxes are `r.absmin`/`r.absmax`, the link's box about the origin
grown a unit each way (`SV_LinkEntity` 0x809c63a), compared inclusively,
and the radius test sums the two `r.maxs[0]` against the x/y separation.
VERIFIED, the export. The partner counts with contents 0x2000000 or
0x4000000, so on the partner's own turn the same pair is found again and
pushed the same way, and both end the frame at 0x4000000. INFERRED, the
conditions. `G_SetClientContents` (0x80f5682) gives a client 0x2000000
unless it is dead, noclipping or flying; `ClientEndFrame` calls it for a
playing client before the push test. VERIFIED. The living pmove mask
0x2810011 meets 0x2000000, so a body that is not mid-push stops other
players' moves. VERIFIED, the mask.

vcod: `game::stuck::stuck_in_client_cod2`, run from the CoD2 server's end
frame per slot for a client with health; `Cod2Player::contents` holds
`r.contents`, and every cmd's move clips the other clients whose
contents are set (`Cod2Level::move_bodies`) as 1.1's movetrace capsules,
where retail clips its own capsule model. INFERRED, that the two clip
alike; a run into a standing player stops where retail's does, to the
net probe's resolution (cod2-movement.md 20). VERIFIED. Tested in `players_block_each_other_and_overlapping_ones_part` and
`cod2_pushes_overlapping_players_apart_at_the_eject_speed`. Six-bot
`bot_match` runs on mp_toujane (seeds 1 to 4) stall no bot.

## 15. Other gunners on the client

VERIFIED, Mac `CG_Player` 0x1e6834: a player entity with `eFlags & 0x300`
whose `otherEntityNum` is past the clients and not 1023, with a client info
and a gun entity whose DObj has `tag_weapon`, is placed as the server's
0x810aa50 places it: the legs anim's group (less 0x200) blended by the
gun's `tag_weapon` yaw over `animHorRotateInc` and the body's height over
the rows, hung off `tag_weapon`, then a capsule trace under 0x2810011
between the placed spot and the gun's height sets the z. INFERRED, the
trace's direction. VERIFIED, `BG_Player_DoControllers` (Mac 0x182b82) zeroes every
control of a body with `eFlags & 0x300`. vcod's `vcod_cod2::player::gunner_placement` runs the shared
`turretpose::place_gunner` on the lerped gun's barrel and poses the legs
by the placement's rows (`PlayerBody::pose`'s `turret`); the z stays the
snapshot's, which the server's trace already set. INFERRED, that the two
traces agree. `cod2_players`' `a_remote_gunner_stands_where_retail_placed_him`
puts each mounted snapshot of the section 10 capture within 0.05 of
retail's origin in the plane.

A spectator following a gunner sees from the gun's `tag_player` too:
`CG_CalcViewValues`' arm reads the playerstate the client draws, the
followed one included. INFERRED, from the arm's test of
`cg.predictedPlayerState.eFlags`. vcod's follower view takes
`Cod2Session::gunner_eye` with the followed view. Pixel check pending.


## 16. The use list's touch test

VERIFIED (capture `carentan-share.txt`, 12200): the released gunner
backing off the gun meets mp_carentan's `trigger_use_touch` (inline model
5, an angled side 12 degrees off the y axis) a frame after vcod's box
said so. `Player_GetUseList` hands the player's box to `SV_EntityContact`
(lnxded 0x8091ad6), which runs `CM_TransformedBoxTrace` (0x805b8f6) with
no capsule argument; the frame retail reaches the side is where the
box's capsule (radius 15) reaches it, 2.8 units short of the box's
corner. INFERRED, that CoD2's box trace is the capsule one for every
box. vcod's use list tests a touch trigger with
`trigger::contacts(.., capsule: true)`; CoD1's touch pass keeps its box.

## 17. Bots on the guns

Retail's server has no bots. vcod's CoD2 bots man a gun in `dm` and
`tdm` by a rule of vcod's own (`bots::cod2::gun`, bot-combat.md 14):
the walk to the spot 40 behind a free gun with an enemy in its arc, the
use tap once the hint names the gun, the barrel turned onto the nearest
foe in the arc and the trigger held, a use tap after 4 s with none. The
mount, the gunner's frame and the release are the player's
(`Cod2Level`'s use key and `turret_think_client`); the bot only sends
the cmds.

