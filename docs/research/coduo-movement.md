# United Offensive 1.51 movement against 1.5

What United Offensive changed in the movement a client predicts, read out
of its game module and measured on the retail 1.51b server. The wire is in
`coduo-diff.md`; the 1.5 movement UO keeps is in `cod15-movement.md`. In
short, UO is 1.5 plus a sprint with a fatigue bar, an LMG that sights only
by deploying a bipod, and a grenade that cooks.

Contents: 1 call order, 2 sprint flag, 3 speed, 4 fatigue, 5 jump,
6 weapon while sprinting, 7 LMG bipod, 8 buttons and binds, 9 the
capture, 10 what vcod does, 11 mounts and cursor hint, 12 grenade cook,
13 weapon times and the sighted prone hold, 14 client sounds and sprint
pose, 15 automatic fire and the view kick, 16 lean footstep cadence,
17 prone check, 18 stance box.

## Sources and conventions

- Binary: `uo/game.mp.uo.i386.so` 1.51b (md5 in `coduo-diff.md`, dynamic
  symbols). Addresses are `nm` addresses. Ghidra's import sits 0x10000
  higher, and its `FUN_xxxx` labels for the module's statics carry the
  higher address.
- Rodata constants are read from the file bytes at the `nm` address
  (`.rodata` file offset equals the vaddr).
- `ps` field offsets come from the playerState netfield table
  (`coduo-diff.md` 4): `fatigueScale` 0x5b0, `lastSprintTime` 0x5b4,
  `sprintSpeedScale` 0x58c, `proneDirection` 0x5a4, `weaponstate` 0xdc,
  `fWeaponPosFrac` 0xe0.
- Method: the module's decompile paired with 1.5b's function by function
  through `PmoveSingle`'s call order, plus the capture of section 9. Every
  claim carries its own label.

## 1. Call order

`PmoveSingle` (0x2d530) against 1.5's (0x27eb7), VERIFIED from both
decompiles:

- Prologue additions: the held weapon's def is looked up first. When the
  previous cmd sprinted (`pm_flags` 0x10000),
  `PM_InteruptWeaponWithSprintMove` (0x33a44) runs. With the sight flag
  (0x20) on an LMG (`weaponClass` 3) the cmd's forward and right moves are
  zeroed, as with the sight flag on an effective prone stance (1.1 and 1.5
  zero the prone case too, section 13).
- The normal arm runs `PM_UpdatePlayerSprintingFlag` (0x2c74a) after
  `PM_UpdatePlayerWalkingFlag` and before `PM_UpdatePronePitch`, and
  `PM_UpdateFatigue` (0x2aa22) after `PM_DropTimers`, before the move.
- The mounted arm (`eFlags & 0x106000`, a turret or vehicle seat) and the
  linked arm also run the sprint flag and the fatigue. Their weapon step
  runs only for the one seat `BG_AllowPlayerWeaponAtVehiclePos` allows.

The sprint flag a cmd moves with is decided on that cmd, after its stance
and ground trace (INFERRED from the order).

## 2. The sprint flag

`PM_UpdatePlayerSprintingFlag` (0x2c74a) clears `pm_flags` 0x10000 and
sets it again only when all of these hold (VERIFIED decompile; the two
fatigue thresholds VERIFIED bytes at 0x9b968 and 0x9b96c):

- `pm_type < 4` and the cmd's `buttons` 0x8.
- `fatigueScale > 0`, and `> 0.25` unless the previous cmd sprinted.
- `pm_flags & 3` clear (not prone or ducked) and 0x20 clear (no sight).
- On the ground, unless the previous cmd sprinted.
- A forward or right move, either sign.
- Not mounted, `weaponstate` not 13, not on a ladder (`pm_flags` 0x10).
- With a grenade in hand (`weaponType` 1): not cooking, attack button up,
  `weaponstate` not 3.

The capture's backpedal and strafe sprints set the flag (VERIFIED). A
sprint held against a wall keeps it with no speed (VERIFIED, ct 29092).

## 3. Speed

The walk's cmd scale (0x23af5) picks `walkSpeedScale` for the ADS walk, a
lean, or an LMG in `weaponstate` 13; else `sprintSpeedScale` while 0x10000
is set; else `runSpeedScale`. While sprinting it leaves out the weapon's
`moveSpeedScale` (VERIFIED decompile). The air and ladder scale (0x23905)
makes the same three-way pick (VERIFIED decompile). `sprintSpeedScale`
reads 1.6 on the wire (VERIFIED, `coduo-diff.md` 5). In the capture the
mg30cal (`moveSpeedScale` 1.1) runs at 209 and sprints at 304, 190 x 1.6
with no weapon factor (VERIFIED).

`PM_Footsteps` (0x297e4) divides the bob rate by the sprint scale instead
of the run scale while sprinting (VERIFIED decompile).

Footstep ids, from the picker (0x25b9b) (VERIFIED decompile, those
immediates):

| Body | Id |
|---|---|
| prone | surface + 47 |
| sprinting | surface + 70 |
| walking | surface + 24 |
| else | surface + 1 |

The jump's own event (0x24328) is also `surface + 70`, 83 on a ladder
(VERIFIED decompile). The UO cgame plays 70..92 as
`step_sprint_<surface>` plus `gear_rattle_sprint` (`CG_EntityEvent`
`uo_cgame_mp_x86.dll` 0x30022810, footstep ranges 0x3002289c-0x30022933,
VERIFIED disassembly), so a UO jump sounds like a sprint step. The
capture's sprint steps read 76, dirt (70 + 6) (VERIFIED, section 9).

Acceleration and friction are 1.5's: rodata 0x9b4a4..0x9b4e4 equals 1.5's
0x75624..0x75664 (VERIFIED bytes).

## 4. Fatigue

`PM_UpdateFatigue` (0x2aa22). Constants, VERIFIED bytes:

| Constant | Value | Address |
|---|---|---|
| ms to bar scale | 0.001 | 0x9b8fc |
| winded threshold | 0.5 | 0x9b900 |
| `pm_sprintFatigue` | 1/3 | 0x9b4e8 |
| `pm_fatigueRegen` | 1/7 | 0x9b4ec |
| `pm_fatigueRegenDelay` | 1000 | 0x9b4f0 |

Logic, VERIFIED decompile:

- A sprinting cmd (not noclip, `bg_nofatigue` 0) stamps `lastSprintTime`
  with its `serverTime` and takes `0.001 * msec / 3` off the bar. Under
  0.5 it raises event 140 (`EV_FATIGUE_LAST_SOUND`) and sets `pm_flags`
  0x20000 (winded), on every such cmd, unthrottled. Under 0 the bar is 0.
- A cmd that neither sprints nor holds the button adds
  `0.001 * msec / 7` once `serverTime >= lastSprintTime + 1000`. At 1 the
  bar is 1 and 0x20000 clears. Then it calls `PM_PlayFatigueSound`
  (0x2a968).
- A held button that does not sprint refills nothing and calls
  `PM_PlayFatigueSound`. A cmd inside the refill delay without the button
  calls nothing.

`PM_PlayFatigueSound` (0x2a968, VERIFIED decompile; immediate 0x6a4 =
1700): when winded and `iFatigueSoundTime` (ps+0x3c, the netfield of that
name) is more than 1700 ms behind the cmd's `serverTime`, it raises event
140 and stamps the field with `serverTime`. When not winded it zeroes a
positive stamp that old. Event 141 (`EV_FATIGUE_SOUND`) is raised nowhere
in the module (VERIFIED by absence of the immediate). The UO cgame plays
140 as `fatigue_breath` on the event's entity, for every player, and 141
as `sprint_breath_last`, an alias no stock csv defines (VERIFIED,
`CG_EntityEvent` cases 0x30022b3c and 0x30022b23 and the csvs). The
capture's event ring and `iFatigueSoundTime` match vcod's prediction on
every snapshot (VERIFIED, section 9).

Measured in the capture (VERIFIED):

- A sprint from full ran 2967 ms (ct 21099 to 24066) and went winded at
  0.492, 1483 ms in.
- The bar refilled from 0.09 to 1 at ct 37240: 1000 ms after the last
  sprint plus 6.4 s.
- A sprint pressed at 0.345 started.

Nothing else in the module reads the bar (VERIFIED by absence): it slows
nothing and does not drain on a jump or stance change.

## 5. The jump

`PM_CheckJump` (0x24483) is 1.5's: takeoff curve, landing arms and the
500 ms gate are equal (VERIFIED bytes and decompile). One refusal is new:
no jump with the sight flag on an LMG (VERIFIED decompile). The capture's
sprinting jump rose 40.8, 1.5's unpenalised height at 125 fps (VERIFIED,
peak at ct 38890).

## 6. The weapon while sprinting

There are no sprint weaponstates (state names at 0x9d6a0, VERIFIED).
`PM_InteruptWeaponWithSprintMove` (0x33a44) drops a deploy or breakdown to
`weaponstate` 0 with both timers 0 and the raise anim, and leaves every
other state (VERIFIED decompile). Inside `PM_Weapon` (0x3726c, VERIFIED
decompile):

- The shot (0x36c0e) never fires while sprinting; a spotter fires only down
  an `adsFire` sight; an LMG only with the sight flag and `fWeaponPosFrac`
  >= 0.99 (0x9d58c).
- A reload (0x358b2) never starts while sprinting, nor on an LMG short of
  the same 0.99 (0x9d4e8).
- The melee swing (0x36fb7) does not latch while sprinting, and never on a
  deployed LMG or one in `weaponstate` 13.
- `aimSpreadScale` is held at 255 while sprinting with a move key, as for
  a moving prone body.

The sight through a rechamber is 1.5's (`cod15-movement.md` 6). VERIFIED:
the weapon file field table entry at `.data` 0xabd90 names
`rechamberWhileAds` at weaponDef +0x32c (type 5).
`PM_UpdateAimDownSightLerp` tests that field against 0 at 0x33682 and
`weaponstate` against 4 at 0x33695, both branching to 0x336a0. INFERRED
from the branch targets: 0x336a0 lowers the sight, so a rechamber on a gun
whose file says 0 drops the scope whatever the button says, and the held
button raises it again once the state leaves 4. UO's `kar98k_sniper_mp`,
`mosin_nagant_sniper_mp` and `springfield_mp` all say 0 (VERIFIED,
`pakuo` weapon files). 1.1 has no such test: its rechamber picks
`WEAP_ADS_RECHAMBER` on a raised sight and keeps it (`cod11-combat.md`
1.9). So on UO a scoped shot comes every `fireTime` + `rechamberTime` +
`adsTransInTime` (0.33 + 0.95 + 0.45 s on the springfield), for players
and bots alike (INFERRED, the sum of the three).

## 7. The LMG bipod

`PM_UpdateAimDownSightFlag` (0x32c0b, VERIFIED decompile; 15.0 at 0x9d4cc
and 1.0 at 0x9d4d0, VERIFIED bytes). The sight's usual conditions are
1.5's plus one: `weaponstate` 13 keeps the flag only while the cmd's
weapon byte is the held weapon. For an LMG without the flag, in order:

1. `pointcontents` 15 units ahead along the view's yaw and 1 up, mask
   0x404000. A hit deploys there: standing, or crouched where the contents
   carry 0x400000, with `proneDirection` the view's yaw.
2. Else `serverCursorHint` 7 deploys the same way; a non-zero
   `serverCursorHintVal` means crouched (section 11).
3. Else a body not prone that is airborne or fails `BG_CheckProneValid`
   (0x2162c) with its 16th argument set is refused: `pm_flags` 0x8000 and
   `EV_STANCE_FORCE_STAND` or `_CROUCH`, unless `wbuttons` 0x2.
4. Else the flag goes on with `pm_flags` 1: the body is put prone.

An LMG with the flag keeps it while the conditions hold (VERIFIED
decompile). The 16th argument adds a +/-6 box from 8 up to 38 ahead,
which must be clear (VERIFIED decompile; 8, -38 and 180 at 0x9a9b4,
0x9a9b8 and 0x9a9ac, VERIFIED bytes). The mount contents are the
`hmgstanding` and `hmgcrouching` brushes (`coduo-diff.md` 11). In the
capture a sighted press standing against a wall was refused on every cmd
(`pm_flags` 0x8000 and `EV_STANCE_FORCE_STAND` from ct 42499, VERIFIED).

### Deploy and breakdown

The deploy driver (0x35b66) runs in `PM_Weapon` for `weaponClass` 3 and 8
outside a match timeout, after the timers (VERIFIED decompile). It leaves
`weaponstate` 1, 2, 10 and 11 alone, and 5 to 9 while sighted (INFERRED,
its jump table). Otherwise (VERIFIED decompile):

| Trigger | Routine | `weaponstate` | `weaponTime` | `weapAnim` | Event |
|---|---|---|---|---|---|
| sight flag, fraction < 1 | deploy 0x347aa | 12 | `adsTransInTime` | 19 | 160 |
| flag lost, fraction > 0 | breakdown 0x3484c | 13 | `adsTransOutTime` | 20 | 161 |

Each ends at `weaponstate` 0 and `weapAnim` 0 once both timers run out
(0x350c2, 0x35120, VERIFIED decompile). The stock mg30cal reads
`adsTransInTime 1.8`, `adsTransOutTime 1.9`, `moveSpeedScale 1.1`
(VERIFIED, the weapon file). The capture's first deploy snapshot reads
`weaponTime` 1774, 25 ms in, and the breakdown's 1875, 24 ms in
(VERIFIED, ct 48091 and 58582): both start at 1799 and 1899, a
millisecond short. INFERRED from those two: the def stores the float's
thousandfold truncated (confirmed in section 13). Both raise a torso anim
event too, and the breakdown raises it again across a stance change
(`player-model-anim-system.md`, "The takeoff and the landing").

### While deployed

VERIFIED decompile:

- Moves are zeroed (section 1).
- `PM_CheckDuck` (0x27e16) takes no stance key.
- `PM_UpdateLean` (0x2ac45) eases any lean back.
- Jump, sprint start and melee swing are refused.
- The putaway (0x348ee) refuses a switch off an LMG or spotter with the
  flag or a fraction above 0.0001.
- The sight stays up through an LMG's reload
  (`PM_UpdateAimDownSightLerp` 0x3349e).
- The breakdown walks (section 3).

### View cap

`PM_UpdateViewAngles` (0x2b15c, VERIFIED decompile; VERIFIED bytes: 5 at
0x9b948, 55 deg/s at 0x9b94c, 0.98 at 0x9b950, +/-45 at 0x9b954/0x9b958)
runs its prone block for a prone body and for a deployed LMG.

- Yaw cap: `bg_lmg_yawcap` (55, a mirrored cvar) whenever the held weapon
  is an LMG, prone and undeployed included; `bg_prone_yawcap` otherwise.
- The body swings toward the view only for a prone body not deployed.
- The view is pulled toward the body only for a prone body without an LMG.
- The cap and pitch clamp then run as in 1.5.

In the capture the deployed view swung 70 degrees off `proneDirection` was
held at 55 by `delta_angles[1]` pushes (VERIFIED, ct 53450 on). The pitch
clamp of a standing or crouched deploy measures from `proneTorsoPitch`,
which only prone writes (INFERRED).

A client reads a deploy off the wire as: `pm_flags` 0x20 with an LMG in
hand, the stance in `pm_flags` 3, `weaponstate` 12 or 13 through the
transitions, and `proneDirection` as the cap's centre (INFERRED from the
writes above).

A deployed LMG's rounds start from the eye pushed up to 19 forward along
`proneDirection` by an 8-unit box trace, `CalcMuzzlePoint` 0x8b8ea
(VERIFIED; addresses and the rest of UO's muzzle in `cod11-combat.md` 2.1,
"1.5 and United Offensive").

## 8. Buttons and binds

`CL_CmdButtons` (`CoDUOMP.exe` 0x40c700, VERIFIED decompile) puts
`+sprint` in `buttons` 0x8; every other bit is 1.1's. UO's
`default_mp.cfg` (`localized_english_pakuo01.pk3`, line 9) binds ALT to
`+sprint` (VERIFIED, the file). There is no hold-breath or binoculars
button; binoculars are a weapon (VERIFIED by absence of the strings).

Controls screen: UO's `ui/options_move.menu`
(`localized_english_pakuo00.pk3`) is 1.1's file plus a last bind row,
`@GMI_MENU_SPRINT` on cvar `+sprint` at rect `5 325` after `+strafe`
(VERIFIED, both files). The UI module's `g_bindings` table
(`uo_ui_mp_x86.dll` 0x400406b0, 55 records of 24 bytes; 1.1's has 50 at
`ui_mp_x86.dll` 0x40036130) adds (VERIFIED, the table):

- `mp_quickmap` and `mp_purchase` after `mp_QuickMessage`;
- `weaponslot satchel` and `weaponslot binocular` after
  `weaponslot smokegrenade`;
- `+sprint` between `toggle cl_run` and `screenshot`.

Config: UO's `Com_Init` queues `exec default_mp.cfg`, `language.cfg`,
`uoconfig_mp.cfg` and `autoexec_mp.cfg` (`CoDUOMP.exe` 0x43bd3c-0x43bd5f,
VERIFIED strings and pushes). A written `uoconfig_mp.cfg` opens with
`unbindall` (VERIFIED, the file). A config without a `+sprint` line
therefore leaves ALT unbound (INFERRED from that order). UO's file name is its own,
so a first UO run never reads 1.1's `config_mp.cfg` (VERIFIED, the two
names).

## 9. The capture

`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-sprint-uo.txt`
(2026-10-14). Recipe: `sprint_script` (`--probe-sprint` in
`crates/client/src/probe.rs`) from `client-probes/probe_prone`'s street
placement on `COD_VERSION=uo tools/run_probe.sh`, `--probe-weapon
mg30cal_mp`, cmds every 8 ms, every snapshot's whole playerstate.

The script runs; sprints past exhaustion with the view swinging round;
rests; sprints too early; sprints with a jump and each other gait
(backpedal, strafe, crouched, sighted, firing); lies down; deploys; swings
the view past the cap both ways; moves and fires deployed; breaks down and
stands. mp_carentan is pak4's map, the same file in 1.1 and 1.5 (VERIFIED,
both installs list it at 10971520 bytes, 2003-10-03).

`crates/server/tests/moves_ab.rs`'s
`the_predictor_replays_uo_sprint_and_bipod` replays it through the
client's predictor. Under UO's rules: 900 snapshots, every compared field
right except the first pair's `aimSpreadScale` (no cmd at the first
snapshot). Under 1.5's rules the sprint is missed on 156 snapshots. Needs
`COD_DIR` at a UO install.

## 10. What vcod does

`MoveRules::Uo` (`crates/common/src/pmove.rs`), which the predictor takes
for protocol 22, implements sections 1 to 7, 11 and 12, the fatigue
breaths and sprint footstep ids included. The vehicle seat arm is
`seat_move` (`coduo-vehicles.md` 3). Left out:

- `bg_nofatigue` and the mirrored `bg_lmg_yawcap`: pmove uses the
  defaults (`LMG_YAWCAP` 55). vcod-server registers both cvars but pmove
  does not read them.

The client sends `buttons` 0x8 to a UO server only, from `+sprint`, which
vcod's default binds put on ALT. `BIND_COMMANDS`
(`crates/client/src/frontend/options.rs`) carries UO's five extra
`g_bindings` rows, so UO's Sprint row rebinds it. vcod keeps one
`vcod_mp.cfg` per mod directory, as UO keeps its own file. A
`--mod-dir uo` config written before vcod had the sprint gets ALT back
when ALT and `+sprint` are both unbound (`Shell::bind_if_free`), which is
what a UO-written config would hold. Client sounds and pose: section 14.

## 11. The mounts and the cursor hint

`G_CheckForCursorHints` (0x575bb, VERIFIED decompile; 15.0 and 1.0 at
0xa1098 and 0xa109c, VERIFIED bytes) clears `serverCursorHint`, its value
and its string each frame. For a live client holding a `weaponClass` 3
weapon, not sighted and not in a vehicle seat, it probes
`trap_PointContents` 15 ahead along the view's yaw and 1 up under mask
0x404000. A hit, or `G_CheckPointInsideTriggerMount` (0x7780f) finding a
`trigger_mount` within 0.1 of the point, sets the hint to 7. Only the
second sets the value, to the trigger's spawnflag 1 (INFERRED, the branch
order). So a crouched mount the server's own contents see sends value 0.

`trigger_mount` (`SP_trigger_mount` 0x776d5) is a trigger with contents
0x4000, or 0x400000 with spawnflag 1. Script makes one with
`spawnTriggerMount(mins, maxs, crouch)` (0x67974,
`SP_trigger_mount_no_brush` 0x77754), a box with the same contents
(VERIFIED decompile). No stock UO map places one and no stock script
spawns one (VERIFIED, every UO BSP's entity lump and every stock `.gsc`).
Every stock UO map has both mount brushes (VERIFIED, the material lumps).

Captures (VERIFIED, 2026-10-14):
`crates/server/tests/fixtures/playerstate/mp_foy-dm-slope-8ms-uofoot-{stand,crouch,mount}.txt`.
Recipe: `--save-slope --probe-uofoot <yaw>` (`uofoot_script` in
`crates/client/src/probe.rs`) on `COD_VERSION=uo tools/run_probe.sh
client-probes/probe_uofoot mp_foy +set probe_teleport 1 +set probe_spot
<spot>`, `--probe-weapon mg30cal_mp`, cmds every 8 ms.

| Capture | Setup | Result |
|---|---|---|
| `stand` | facing the `hmgstanding` brush at (-85 2700 -5) | deployed standing (`pm_flags` 0xa0 over the base) in 1799 ms; the sight pressed crouched stood the body up deployed (0xa2 to 0xa0); the breakdown left it crouched under the held key |
| `crouch` | facing the `hmgcrouching` brush at (-1080 -3313 0) | deployed crouched; hint 7 with no value on the wire beforehand |
| `mount` | in front of a crouched `spawnTriggerMount` box over open ground | hint 7 value 0; the server deployed crouched |

The `mount` result is the trigger's own contents seen by the server's
pmove (INFERRED: the server's point contents include linked entities, the
client's do not). The predictor, reading hint 7 value 0, deploys standing
on those two snapshots, as the retail cgame's identical code would
(INFERRED); the next snapshot corrects it. `moves_ab`'s
`the_predictor_replays_uo_mounts_and_cook` replays all three under UO's
rules: every compared field right except those two snapshots
(`UOFOOT_GAPS`). The stand and mount captures were retaken 2026-10-17
(VERIFIED, same recipe) with the cmds sent before the first logged
snapshot in the file: those stamped past its `commandTime` run in the
first pair, and without them that pair's `aimSpreadScale` read one turn
short.

### The server's half

`G_CheckForCursorHints` (game.mp.uo 0x67aa0, VERIFIED decompile):

- Zeroes the hint and its value; for a live client with no in-use flag
  sets the string to -1 (0xff on the wire).
- A client holding a `weaponClass` 3 weapon with `pm_flags` 0x20, or with
  `eFlags` 0x100000 (vehicle seat), skips the rest: a deployed bipod reads
  hint 0 and no activate entity is looked for.
- Otherwise the LMG arm probes `r.currentOrigin + 15 * forward(0,
  viewangles[1], 0) + (0, 0, 1)` with `trap_PointContents(point, -1,
  0x404000)`. A hit, or `G_CheckPointInsideTriggerMount`, sets hint 7 and
  the value that function wrote (0 where only the contents hit). When the
  weapon's `useHintString` (weapon def +0x494) is not empty, its index
  (+0x498) is the string.
- The activate-entity loop runs after; the first usable entity's hint
  replaces the 7 and the value stays (stores at 0x67c37).

No stock LMG file sets `useHintString` (VERIFIED, the `pakuo` weapon
files), so the string stays -1.

`G_CheckPointInsideTriggerMount` (0x8780f, VERIFIED decompile) asks
`trap_EntitiesInBox` for the point grown 0.1 under mask 0x405c0008 and
takes the first entity whose classname is `trigger_mount`; the value is
its spawnflag 1. The query walks linked boxes, which `SV_LinkEntity` grows
a unit, so the value is reached only from a point within that margin
outside the trigger (INFERRED).

Whether `trap_PointContents` masks the world's contents with 0x404000 as
it does the entities' is not read out of the server binary. vcod masks
both (INFERRED: otherwise every wall 15 ahead would hint 7, and the
captures show no hint away from the mounts).

`spawnTriggerMount(mins, maxs, crouch)` (0x77974, VERIFIED decompile)
reads two vectors and `Scr_GetBool`, sorts each axis, `G_Spawn`s, sets
classname `trigger_mount` and `r.mins`/`r.maxs` to the sorted corners at
origin 0, and calls `SP_trigger_mount_no_brush`:

- touch function `Touch_trigger_mount` (`G_Trigger`, the `"trigger"`
  notify);
- contents 0x401c0008 with 0x4000, or crouched 0x400000 and spawnflag 1;
- `r.svFlags` 1 (no client sees it), `eFlags` |= 2, linked;
- returns the entity.

`SP_trigger_mount` (0x876d5), the map's, ORs the same bit into
`InitTrigger`'s contents.

vcod-server:

- `spawnTriggerMount` and a UO map's `trigger_mount` register
  `TriggerKind::Mount` triggers (`game::trigger`,
  `builtins::uo::spawn_trigger_mount`).
- The end frame runs the LMG arm (`ScriptRuntime::cursor_hints`), writes
  `serverCursorHintVal` and puts hint 7 on the server's playerstate
  (`ClientSim::set_cursor_hint`), which the shared pmove reads as the
  client's does.
- The server's pmove point contents add every linked trigger box holding a
  mount bit (`MoveWorld::with_boxes`, `trigger::mount_boxes`), as retail's
  `SV_PointContents` meets entities. That is why the server deploys
  crouched on the script trigger where the predictor, with hint value 0,
  deploys standing.
- A brush `trigger_mount` is tested by its box, not its brushes; no stock
  map places one.

A mount is in the touch pass only while its contents meet
`G_TouchTriggers`' area query, `trap_EntitiesInBox` under 0x405c0008
(`G_TouchTriggers` 0x3fb15, VERIFIED decompile); nothing else in the pass
reads a trigger's kind. Measured on retail UO 1.51b, mp_foy
(`client-probes/probe_mounttouch`, one spawned `--probe-team allies` probe
inside a standing script mount around his spawn), `"trigger"` notifies
per one-second phase (VERIFIED):

| Contents | Fires |
|---|---|
| spawned, 0x401c4008 | 57 |
| `setContents(0x4000)` | 2 |
| `setContents(0x400000)` | 55 |
| `setContents(0)` | 2 |
| `setContents(0x40000000)` | 55 |
| `setContents(0x401c4008)` | 59 |

`setContents` returned the previous contents each phase (0x401c4008,
0x4000, 0x400000, 0, 0x40000000; VERIFIED, same run). The mount fires
once per cmd, not per frame: the probe sends about 60 cmds a second
against 20 server frames (INFERRED, from the counts). The two fires under
a contents the mask misses are notifies queued by cmds before the
`setContents`, delivered after the counting thread started (INFERRED).
vcod-server's touch pass (`game::trigger::touched`) queries the area tree
under the same mask, so `setContents` moves a mount out of the pass and
back (`set_contents_moves_a_mount_in_and_out_of_the_touch_pass`).

`crates/server/tests/mount_uo_ab.rs` replays the three captures' cmds
through vcod-server on `probe_uofoot` (VERIFIED, 2026-10-16): every
snapshot's `pm_flags`, hint and value pass through the same states as
retail's, each within three snapshots of retail's length. One state one
snapshot long is dropped on both sides: the first cmd off the bipod still
reads ducked, since `PmoveSingle` checks the stance before the sight flag,
and retail's snapshot there held five release cmds where ours may hold
one.

## 12. The grenade cook

In `PM_Weapon` (0x3726c), before its timers, for `weaponType` 1
(VERIFIED decompile; immediates 10, 0x33, 0x32, 0xa3, 0xd2, 0x640, 0xb6):

- With `cookOffHold` set, an edge of `buttons` 0x10 (the sight, pressed or
  released) against the previous cmd while `grenadeTimeLeft >= fuseTime`
  takes 10 ms off it and raises `EV_GRENADE_SPOON` (182).
- With `grenadeTimeLeft` positive: with `cookOffHold` and the fuse below
  `fuseTime` it loses the frame's msec. Under 51 the grenade goes by
  itself: `grenadeTimeLeft` 50; event 163 (`EV_FIRE_WEAPON`), plus 210
  (`EV_GRENADE_SUICIDE`) when `explosionInnerDamage` (def+0x37c) is
  non-zero; a round off the clip; `weaponDelay` 0; `weaponTime` 1600; the
  clip-only weapon taken with event 151 when it was the last (0x35ce8);
  and `PM_Weapon` returns.
- Otherwise the held trigger pins `weaponDelay` at the frame's msec plus
  one, as 1.1's pin reads.

So the pullback alone leaves the fuse whole, as on 1.1, and only the
sight's edge starts it (INFERRED from the conditions).

- The server's `ClientEvents` (0x40860) answers 210 with `player_die`,
  means of death 0x15, unless the thrower has the god flag (VERIFIED
  decompile).
- `fire_grenade` (0x5f7ad) fuses the missile with what is left of
  `grenadeTimeLeft` (2500 ms when that is 0) and zeroes it (VERIFIED
  decompile).
- `PM_UpdatePlayerSprintingFlag` refuses the sprint while a `cookOffHold`
  grenade's fuse runs (section 2, VERIFIED decompile).
- Every stock UO grenade has `cookOffHold 1`; the frag's `fuseTime` is 4
  and `explosionInnerDamage` 120 (VERIFIED, `pakuo07.pk3`).

Measured in all three captures of section 11 (VERIFIED):

- A plain throw reads `grenadeTimeLeft` 4000 from pullback to throw.
- The sight pressed 400 ms into a hold raises 182, and the fuse runs down
  50 ms a snapshot to the release.
- The sight held into the pullback and released 600 ms later spoons on the
  release.
- A cook held past the fuse kills the thrower (a corpse, no further
  spawned snapshot); the last spawned snapshot reads 63 to 93 ms left.

vcod's replay matches the fuse, `weaponDelay` and every event by id on
every snapshot.

## 13. Two questions from the first pass

**Weapon times truncate.** The weapon parser's field type 7 (the 22
millisecond keys from `fireDelay` to `adsTransBlendTime`) loads the float,
stores it to f32, multiplies by 1000.0f on the x87 stack and stores it
with `fistp` under a control word OR'd with 0xc00. VERIFIED instructions:

| Server | `ParseConfigStringToStruct` | case | `fistpl` | 1000.0f |
|---|---|---|---|---|
| 1.1d | 0x6adf8 | 0x6af30 | 0x6af62 | 0x7a31c |
| 1.5b | 0x717de | 0x7194e | 0x71984 | 0x7f7dc |
| UO | 0x943ea | 0x9455a | 0x94590 | 0xa80fc |

So 1.8 is 1799 and 0.35 is 349 on every Linux server. 83 of 1.1's 522
stock time values, 88 of 1.5's and 125 of UO's 812 land a millisecond
under rounding (VERIFIED, a scan of the stock files). The Windows modules
call `__ftol2` on the unstored double instead (VERIFIED instructions,
`cgame_mp_x86.dll` 0x3003f3f8, `uo_cgame_mp_x86.dll` 0x3004f288). Under
the default 53-bit precision that equals rounding for every stock value
but two (INFERRED). vcod's `weapon::stored_ms` truncates for pmove and the
turrets; every 1.1, 1.5 and UO gate passes under it.

**1.1 and 1.5 hold a sighted prone body still too.** 1.1's `PmoveSingle`
(0x33dfc) zeroes the cmd's forward and right moves when
`pm_flags & 0x21 == 0x21` (0x33f48-0x33f61). 1.5's (0x27eb7) does it when
`pm_flags` 0x20 is set and `PM_GetEffectiveStance` returns prone
(0x281ae-0x281c8). Both run early, before the move (VERIFIED
instructions). vcod applies both (`hold_still_sighted`).

## 14. The client: sounds and the sprint pose

Sounds, all VERIFIED from the cgame and the csvs:

- 140 plays `fatigue_breath` (section 4); vcod's cue table plays it on the
  event's entity.
- 182 plays the fixed `us_grenade_lever` at the event's origin from the
  world entity (case 0x30023298), not the weapon's `pullbackSound`.
- 70..92 play the sprint steps (section 3).

The sprint pose. The cgame's weapon field table (0x300843a8) loads
`sprintMoveF/R/U` (def+0x11c..0x124), `sprintRotP/Y/R` (0x128..0x130) and
`sprintMoveMinSpeed` (0x19c). `sprintRotMinSpeed`, present in every file,
has no row (VERIFIED, the table). While `pm_flags` 0x10000 is set
(VERIFIED code):

- the gun's movement term (0x30044ce0, UO's 0x300362c0) takes
  `sprintMove*` and `sprintMoveMinSpeed` in place of the stance's;
- the tilt (0x30014ea0, the game's twin 0x37cfb) takes `sprintRot*` with
  the unparsed minimum;
- the stance's rates and the sight's fade are unchanged.

Every stock UO player weapon has `sprintMove` (-14, -10, -14) except
`svt40_mp` at -35 forward, `sprintRot` 0 and `sprintMoveMinSpeed` 110
(VERIFIED, the files). There is no sprint viewmodel anim: no `*Anim` key
or weaponstate names one (VERIFIED). vcod's `GunMove::step` and
`movement_rot` (`crates/common/src/pmove/aim.rs`) take the sprint keys on
the predicted sprint flag.

## 15. Automatic fire and the view kick

Capture: `mp_carentan-dm-slope-8ms-autofire-uo.txt`, `--probe-autofire`
(`autofire_script` in `crates/client/src/probe.rs`) with `mp44_mp` on
retail 1.51b, from `probe_prone`'s street placement. `moves_ab.rs`
(`the_predictor_replays_uo_automatic_fire`) replays it under UO's rules.

- Cadence is 1.1's. The trigger held 1.5 s from the hip fires 13 rounds,
  one `EV_FIRE_WEAPON` and one round off the clip each, 100 or 150 ms
  apart on the 50 ms snapshots: 120 ms a shot, the file's `fireTime` 0.12.
  The same 1.5 s down the sight fires 12. Four taps 400 ms apart fire four
  rounds (VERIFIED, the capture). UO's `mp44_mp` (`pakuo07.pk3`) has the
  same `fireTime`, `fireDelay`, `rechamberTime`, kick and spread keys as
  1.1's `pak0.pk3` copy, except `adsSpread`, `hipSpreadMoveAdd` and
  `hipSpreadTurnAdd` (VERIFIED, the two files diffed).
- vcod-server fires the same script at the same rate: 12 hip rounds, 13
  sighted (one off retail's either way, by where the hold starts against
  the 120 ms), four taps, and the same `weaponTime` pattern after each
  shot (VERIFIED, a `--capture-tag` run against ours, not committed). The
  predictor misses none of `weaponTime`, `weaponstate`, `weaponDelay` or
  `eventSequence` across the 240 snapshot pairs (VERIFIED, the gate); the
  one miss is the first pair's `aimSpreadScale`, as in the sprint gate.
- Recoil is 1.1's. `CG_WeaponFireRecoil` (`uo_cgame_mp_x86.dll`
  0x30016120, called from `CG_FireWeapon` 0x30047d20 under `pm_flags`
  0xc0000 for the view's own client) sets the view kick's speed from the
  sight's ranges (def+0x3f4..0x400) only at `fWeaponPosFrac` 1.0, else
  the hip's (def+0x43c..0x448), roll -0.5 times the yaw. It adds the
  gun's kick from the sight's (def+0x3d4..0x3e0) at any `frac` above 0.0,
  else the hip's (def+0x41c..0x428) (VERIFIED, the decompile; constants
  1.0, 0.0, -0.5 and 1/32768 at 0x3007bce0, 0x3007bcec, 0x3007bf50,
  0x3007bec0).
- `CG_KickAngles` (0x3003f9f0) steps 5 ms slices, centres at the sight's
  speed (def+0x404) above `frac` 0.5, else the hip's (def+0x44c), 2400
  with no weapon; it moves back at 0.06 of the speed and clamps at 10
  degrees (VERIFIED, the decompile; constants at 0x3007bdac, 0x3007bda8,
  0x3007bd30, 0x3007bce8).
- The sight's offsets sit 0x84 past 1.1's (0x370, 0x350, 0x380), the
  hip's 0x8c past (0x3b0, 0x390, 0x3c0), each block in 1.1's order
  (VERIFIED, both decompiles). The code reads the same keys (INFERRED, the
  def layout is not dumped).

### Traps fixed in vcod

Each of these doubled a shot on the client. Keep the fix.

**The ring in UO's numbering.** The prediction replays from every new
snapshot's playerstate. `predict::from_wire` once kept that ring in UO's
event numbering while the prediction pushes and the dedup
(`play::events::PredictedEvents`) record 1.1's. A slot the snapshot
confirmed read as changed and played as the 1.1 event of UO's number
(UO's `EV_FIRE_WEAPON` 163 is 1.1's `EV_EJECT_BRASS`); its record then
made the snapshot drain's own copy play too: a second fire sound, flash
and kick per round (VERIFIED,
`a_uo_snapshot_confirming_a_shot_plays_nothing_more` fails on the old
ring). It hit every UO predicted event at 140 and up (INFERRED, the
mapping is by id). 1.1 and 1.5 number the ring alike, so neither was
affected (VERIFIED, `Cod1Wire::event_to_v1` is the identity on both).
`from_wire` maps the ring through `event_to_v1`.

`crates/client/src/play/replay.rs` replays the capture through the
client's frame, one frame per cmd: prediction, event collection and dedup
(`play::events::frame_events`), kick, effects and sound cues. It plays 29
fire events, flashes, fire sounds and kicks for the 29 rounds the
snapshots brought, and every event id as often as the snapshots brought
it (VERIFIED, `a_uo_autofire_capture_plays_each_shot_once`). With the ring
left in UO's numbering it plays 58 fire events and 58 `EV_EJECT_BRASS`
(VERIFIED, the gate on a revert).

**`iFatigueSoundTime` missing from the server's playerstate.** Retail's
carries it (VERIFIED, the sprint capture's `wire=` holds ten stamps, 24074
to 44484, each 1700 ms or more apart). The predictor rebuilds the field
from every snapshot (`predict::from_wire`), so a winded body read a stamp
of 0, called the last breath stale and raised event 140 on the first cmd
it replayed, every snapshot. The server raised it once every 1700 ms
(VERIFIED, the loopback trace: snapshots read `iFatigueSoundTime` 0 and a
breath at one sequence while the replay put a breath on each new one).
Held fire after a sprint then found its round at another sequence on the
next rebuild, the slot read as changed, and the round played again inside
50 ms (VERIFIED, the trace: ring `[159, 4236, 159, 4236]` after one
snapshot, `[159, 159, 4236, 4236]` after the next). The breath also played
every snapshot (INFERRED, the same events reach `audio::cues`).
`ClientSim::to_wire` writes the field.

`crates/client/src/play/loopback.rs` runs vcod-server in the test process
and plays it through the client's frame (`play::clock`, cmd ring and
packets, prediction, `frame_events`, kick) beside a server frame every
50 ms, with the mp44 held from the hip, sighted, moving, after a sprint,
jumping, crouched, prone and leaning. Each round the server took off the
ammo plays one fire event and one kick at a steady 125 fps, at 4-25 ms
frames with 1% hitches of 60-250 ms, and over a 60 ms link with 40 ms of
jitter (VERIFIED, `each_round_of_held_fire_plays_and_kicks_once` on UO and
1.1). Without the field the 125 fps run plays 97 fires and 97 kicks for 76
rounds (VERIFIED, the gate on the old `to_wire`).

**Late snapshots past the 16-slot window.** With 3% loss each way the
jittery run played one fire more than the server fired, at a UO install
only, and still with `cl_packetdup` 5, so not lost cmds. A winded sprint
writes up to five fatigue events between snapshots, so a late snapshot's
events sat 17 sequences behind the prediction. The drain recorded one in
the 16-slot played table over a newer shot's slot, and that shot played
again when the next snapshot brought it (VERIFIED, a trace of seed 14).
Retail's `CG_CheckChangedPredictableEvents` (cgame 0x30028c70) compares
only sequences inside `(cg.eventSequence - 16, cg.eventSequence)`
(`add eax,-0x10` at 0x30028c8e) and leaves the rest alone (VERIFIED,
disassembly). `PredictedEvents::filter_snapshot` drops a sequence 16 or
more behind without recording it. The gate runs a lossy case (seed 14):
77 fires for 76 rounds before the fix, and 13 seeds exact after it
(VERIFIED, 2026-10-18).

### The server-step round trip

`predict_ab`'s `predictor_matches_the_uo_server_step` checks the round
trip field by field. Each UO on-foot slope capture's cmds (sprint and
bipod, automatic fire, the three mp_foy mount and cook runs) go to a
protocol 22 `ClientSim` and to a predictor rebuilt from that sim's
playerstate after `ps_to_protocol` and the wire codec, at every captured
snapshot and every fifth cmd. Every field a rebuild restores agrees on
every cmd, the event ring read back through the wire's 8 bits included
(VERIFIED, 22 392 cmds, 0 mismatches). Dropping `iFatigueSoundTime` from
`to_wire` puts 3098 fatigue clock misreads and 2632 `eventSequence`
misses on the sprint run (VERIFIED, the gate on that revert).
`predictor_matches_the_1_5_server_step` runs the 1.5 moves and turns
captures the same way on protocol 6 (VERIFIED, 12 821 cmds, 0
mismatches).

| Capture set | Harness | Result (VERIFIED) | Mutation check (VERIFIED) |
|---|---|---|---|
| six seated captures (tank and jeep seats and drive, jeep gun and rider) | `follow_seat` puts retail's seat, vehicle angles (`vang`) and origin on the sim at each snapshot, since boarding, exit and tag placement are the server's | 33 648 cmds, 3 200 seated snapshots, 0 mismatches | dropping `vehPos` from `to_wire`: 367 and 381 `delta_angles` misreads on the jeep seats and rider runs (the passenger's arcs), six `weapon_pos_frac` on the tank seats run |
| `mp_carentan-dm-slope-8ms-uostates-uo.txt` | `follow_state` puts retail's `pm_type` on the sim: the death; the respawn or spectator spawn, reseeded from retail's playerstate; the link, held at retail's origin | 6855 cmds, 220 dead, spectating or linked snapshots, 0 cmds unpredicted, 0 mismatches (2026-10-19) | a `to_wire` that sends the linked `pm_type` 1 as 0: 1285 misreads |
| `mp_carentan-dm-slope-8ms-uoturret-uo.txt` | `follow_turret` mounts and releases the sim where retail's `eFlags` do and holds it at retail's origin on the gun | 2890 cmds, 450 mounted snapshots, 0 mismatches | a `from_wire` ignoring the mounted bits: 8076 misreads, 2416 of them `mounted` |

The uostates capture (2026-10-18, `client-probes/probe_uostates` beside a
`--save-slope --slope-every-state --probe-moves` client) adds a death, a
respawn, a spell as a spectator, a second respawn, and a `linkTo` to a
script_origin that moves 96 units. The client predicts the corpse and the
free-flying spectator, as retail's cgame does (`cod11-player-clip.md`
1.1). The dead body's `view_target` is excluded from the comparison; its
step never reads it. Before the corpse and spectator were predicted, 328
and then 770 cmds went unpredicted (VERIFIED).

- `predictor_drops_retail_corpse_uo` runs the corpse's 59 snapshots the
  same way, 59 of 59 bit for bit, with the dead move taking 1.5's and
  UO's ladder-check timer test (VERIFIED).
- `predictor_flies_retail_spectator` rebuilds the predictor from each
  spectating snapshot and runs the capture's cmds to the next: 79 of 79
  land on retail's origin and velocity bit for bit, 65 of them moving
  (VERIFIED, 2026-10-19). This needs the fly move's 1.5 and UO stored
  friction and accelerate (`cod15-movement.md` 5, "The fly move"); with
  1.1's, 41 of 79 were off in the velocity's last bits and one stop's z.

The uoturret capture (2026-10-18, `client-probes/probe_turret` under
`probe_teleport 1` beside a `--save-slope --slope-every-state
--probe-uoturret 229` client) mounts the misc_mg42 at (1712 1830 8)
standing, sweeps the view past both arcs, fires a burst, holds the move,
jump and stance keys on the gun and lets go. Retail's playerstate on the
gun, each VERIFIED off the capture:

- `eFlags` carries `EF_MOUNTED_STAND` (0xc000) from the snapshot after the
  use tap; `viewlocked_entNum` is the gun (298); `groundEntityNum` 1023.
- The origin follows the barrel's yaw, the turret pass's gunner placement:
  (1746.4 1861.9) at the gun's rest, (1733.3 1873.2) swung 20 degrees.
- The back key sets `pm_flags` 0x40 (`PMF_BACKWARDS_RUN`), which outlives
  the mount.
- The release brings `EV_STANCE_FORCE_STAND` and puts the body at the
  mount origin raised one unit, from where it falls.

## 16. The lean's footstep cadence

`PM_Footsteps` (0x297e4; 1.5's 0x24ae8) on the ground picks the walk's
cadence (the walk's speed scale in the full-speed divisor and the walk's
K) from `pm_flags` 0x80 only. 1.5's also takes it on a non-zero `leanf`
(`ps+0x40`, 0x24bf7). UO's ladder branch and `PM_FootstepEvent` (0x2937c,
its walk step ids) still test the lean (`ps+0x44`). VERIFIED, both
decompiles. So a leaning body on UO bobs at the run's rate for its speed:
a lean strafe at 71 ticks `bobCycle` 6 per 50 ms snapshot on retail
(VERIFIED, `mp_carentan-dm-slope-8ms-moveshot-walk-uo.txt`,
`cod11-combat.md` 2.1, "Measured moving"). vcod under 1.5's rule ticked 14
on the same cmds (VERIFIED, the gate before the fix). vcod's
`pmove::footsteps` takes the lean out of the cadence under
`MoveRules::Uo` only (`a_uo_lean_bobs_at_the_run_s_rate`).

Every snapshot of the UO and 1.5 walk captures carries retail's
`bobCycle`, and `moveshot_ab` asserts it on each (VERIFIED, the gate). A
one-step start error (ours 3, retail 0 on a lean strafe's first snapshot)
went away with the replay's cmd timing (`moveshot_ab` waits on the probe's
8 ms cmds), not with a cadence change (INFERRED: no footstep code changed
in between).

## 17. The prone check

`BG_CheckProneValid` (0x2162c) against 1.5b's (0x1cf58), read side by
side in the decompile (VERIFIED). UO adds two arguments:

- The 16th turns on a sweep of a +/-6 box from the origin 8 up, 38 units
  along the facing (rodata 0x9a9b4 8.0, 0x9a9b8 -38.0 along the back
  vector), refusing on any hit. Only the bipod's own call passes it
  (`BG_CheckProne`, 0x22ff3, passes 0).
- The 17th, a function of the hit entity, refuses a body whose ground 10
  units down (0x9a9a4) is entity type 12, a vehicle. It runs only on the
  lie-down call (9th argument 0).

VERIFIED: with the 14th argument 0 the trace mask is 0x810031, 1.5b's
0x810011 plus water. A feet or chest sample that lands in water skips the
steep test, and a chest sample more than 24 (0x9a9e0, a double) off the
origin plus 6 refuses. `PM_VerifyPronePosition` (0x2e3b0) passes the 14th
as 0, so the prone move's check sees water.

VERIFIED, rodata: the bend test after the chest sample (the legs' pitch,
feet less hips, `AngleSubtract`ed from the torso's, hips less chest):

| Build | Top | Bottom |
|---|---|---|
| 1.1d | 70.0 | -50.0 (0x701a8) |
| 1.5b | 70.0 | -50.0 (0x74fb8) |
| UO | 70.0 (0x9a9ec) | -45.0 (0x9a9e8) |

A UO body refuses legs bent up more than 45 degrees past the hips; 1.1 and
1.5 allow 50.

VERIFIED, retail capture (`mp_carentan-dm-slope-8ms-patchwalk-pillar-uo.txt`,
2026-10-17): crawling forward and left along the brick wall at x 971.9 on
mp_carentan from (956.9, 588), UO refuses every cmd past y 599.77 from ct
34625 (the snapshot keeps 599.76776, velocity 0). 1.1d and 1.5b crawl on
to 600.36 and 600.34 on the same script. VERIFIED, ours under the same
samples: between y 599.77 and 599.9 the feet sample, 51 behind the origin,
lands on top of brush 640 (z 46.125, 32 over the floor) instead of the
floor, and the bend reads -49.7 at y 599.9 to -49.9 at 600.34: inside
1.5's -50, outside UO's -45. With UO's bound ours refuses the same cmds
and the pillar run matches all 372 snapshots (`moves_ab.rs`,
`the_predictor_replays_walks_against_patches`, no gap rows;
`pmove::tests::a_uo_body_refuses_legs_bent_up_past_45_degrees`).

## 18. The stance box

VERIFIED, disassembly, `PM_CheckDuck` (0x27e16): after the eye's target
steps, UO calls `PM_GetEffectiveStance` (0x28d66; 1 for the prone target
`ps+0x574`, 2 for the crouch target `ps+0x578`) and writes the box's top
(`pm->maxs[2]`, `pm+0xec`) from both it and `pm_flags` 0x1 (prone) and
0x2 (crouch):

| Target | Flags 0x3 clear | 0x1 alone | 0x2 |
|---|---|---|---|
| prone | `ps+0x570`, 70 (0x28d9e) | 30 (0x28ddc) | 50 (0x28dc8) |
| crouch | `ps+0x570`, 70 (0x28e61) | 50 (0x28e77) | 50 (0x28e77) |
| stand | `ps+0x570`, 70 (0x28edb) | 70 | 70 |

VERIFIED, decompile: 1.5b writes 30, 50 or `ps+0x338` from the target
alone (game15.so, the same tail; `cod15-movement.md` 3). `eFlags` 0x40 and
0x20 follow the target on both.

INFERRED: a UO box grows on the cmd the flags rise, and the flags rise
only through the stance traces' fit test (`cod15-movement.md` 3), so it
never grows into a ceiling. A 1.5 box grows when the target next steps, up
to 200 ms past the test, so a body that moved under a low ceiling in
between stands up into it, all solid, held in the air with no ground.

vcod: `PlayerState::maxs` under `MoveRules::Uo` (pinned by
`pmove::tests::a_uo_rise_sizes_the_box_off_the_flags`). Sizing UO's box
the 1.5 way is the trap: VERIFIED (measured, ours, `bot_match` UO `tdm`,
10 bots, mp_cassino seeds 1..=8, master 589516ea), three bots were held
all solid in a caulk or clip brush for 100 ticks and typed `kill`
(`Bot::wedged`). Two traced cmd by cmd had boxes growing from 50 to 70 and
from 30 to 50 on a cmd that moved neither, as the target stepped. With the
UO box none was held (`bot-combat.md` 12.3).
