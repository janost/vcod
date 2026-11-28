# United Offensive 1.51 vehicles from the seat

How a UO player drives, rides and fires a vehicle. Sections 1 to 9 are the
occupant's side: the cmd a seat sends, what the server makes of it, what
the client predicts and where the camera sits. Sections 10 to 14 are the
server's spawn, seats, drive step, guns and damage as `vcod-server`
reproduces them; 15 the vehicle's dust and death effects. `coduo-diff.md` 9
covers the vehicle as an entity (files, spawn, physics outline, damage, the
cgame's pose and draw).

In short: the server owns the vehicle and the body in it, the client
predicts only the seated player's view angles and sight, and the camera
comes off the posed vehicle's tags.

Contents: 1 getting in and out; 2 the seat's cmd; 3 pmove in a seat; 4 the
vehicle's motion; 5 the view; 6 view weapon and HUD; 7 prediction against
the capture; 8 captures; 9 what vcod does (client); 10 server files and
spawn; 11 seats on the server; 12 the drive step; 13 guns, heat and
damage; 14 what vcod-server does; 15 wheel dust, death, seat view; 16
clearing and verifying a vehicle's spot; 17 the script surface no stock
MP script uses.

## Sources and conventions

- `uo/game.mp.uo.i386.so` 1.51b and `uo/uo_cgame_mp_x86.dll` 1.51, md5s in
  `coduo-diff.md`. Game addresses are `nm` addresses (Ghidra's import sits
  0x10000 higher); cgame addresses are virtual at 0x30000000;
  `CoDUOMP.exe` at 0x00400000.
- `ps` offsets through the netfield table: `pm_type` 0x4, `pm_flags` 0xc,
  `delta_angles` 0x4c, `eFlags` 0x84, `weapon` 0xd8, `fWeaponPosFrac`
  0xe0, `viewangles` 0xe8, `viewlocked_entNum` 0x5bc, `vehPos` 0x614,
  `vehType` 0x618. The cgame's `cg.predictedPlayerState` is at
  0x304831c4, `centity_t` at 0x3048c6e0 (stride 0x288); its +0x88 is the
  vehicle's `surfType` (file type: 1 four-wheel, 2 tank, 5 artillery).
- `eFlags` here are UO's raw bits: 0x100000 seated, 0x200000 look along
  the hull, 0x400000 popped out of the hatch. vcod's decoder shifts them up
  one (`EF_IN_VEHICLE_V22` 0x200000, `EF_HULL_VIEW_V22` 0x400000,
  `EF_POPPED_OUT_V22` 0x800000).
- Method: decompiles of both modules, `objdump` of `CoDUOMP.exe` and the
  cgame, constants read from file bytes, and the captures of section 8.
  Every claim carries its own label.

## 1. Getting in and out

- **Picking the vehicle.** `G_GetActivateEnt` (0x56e85) gathers contents
  0x200000 entities in a ±(192, 192, 96) box around the eye. A vehicle
  (`eType` 12) must be the first hit of a 128-unit view trace (mask
  0x2810011), within 128 units and view dot ≥ 0.76; the lowest
  `(1 - (dot - 0.76) / 0.24) * 256 + distance` wins (VERIFIED (the instructions)).
- **Cursor hint.** `G_CheckForCursorHints` (0x575bb) sets
  `serverCursorHint` 2 (`hintActivate`) and `serverCursorHintString` to the
  file's `useHintString`, and does nothing for a seated player (VERIFIED (the instructions)). The strings are `GMI_VEHICLE_*_HINT` ("Press [{+activate}]
  to use the Panzer", ...) in `gmi_vehicle.str` (VERIFIED, the paks). The
  capture reads hint 2 with string index 6 beside the Panzer IV and 5
  beside the Willys (VERIFIED, capture). The cgame has no vehicle icon:
  hint 2 is `hintActivate` in the table at 0x3044b6f8 (VERIFIED).
- **The use press and entering.** `Cmd_Activate_f` (0x4dffb),
  `G_IsVehicleUsable` (0x8b673) and `VEH_LinkPlayer` (0x8998d): the server's
  rules, in order, are 11.1. The seat is the first free one from 1 (seat
  2 needs `hasGunner`, seat n ≥ 3 needs `numPassengers > n - 3`), and the
  mount clears `pm_flags` 0x20 (INFERRED (decompile)). `ps.weapon` is
  untouched on entry: the
  infantry weapon stays. The capture's entries keep the view and move
  `delta_angles` (VERIFIED, capture).
- **pm_type.** `G_RunClient` (0x41dfc) puts a linked client in `pm_type` 1
  (VERIFIED (the instructions)); every seated snapshot of the captures reads 1
  (VERIFIED, capture).
- **Exiting** is the use button again. `VEH_UnlinkPlayer` (0x89fb2) refuses
  a tank's voluntary exit for 1500 ms after entry
  (`GMI_GAME_VEHICLE_EXIT_DELAY`), places the body at
  `VEH_FindValidDismountSpot` (0x88dee) or refuses
  (`GMI_GAME_VEHICLE_UNLINK_BLOCKED`), clears `eFlags` 0x700000, sets
  `viewlocked_entNum` 1023 and `vehPos` 0 (keeps `vehType`), clears
  `fWeaponPosFrac` and sends `v cl_run 1` and `v cl_stance 0`. It does not
  call `SetClientViewAngle` (INFERRED (decompile)). In the capture the body
  lands beside the vehicle and the next snapshot reads `pm_type` 0
  (VERIFIED, capture).
- **Changing seats.** While the predicted `eFlags` hold 0x100000 the
  cgame's `weapnext` (0x300474c0) and `weapprev` (0x30047550) send
  `nextvehslot` and `prevvehslot` instead of cycling weapons (VERIFIED (the instructions)). `Cmd_NextVehSlot_f` (0x4ed9b) and `Cmd_PrevVehSlot_f`
  (0x4ede1) run for a seated, living player; `G_VEH_CycleSlot` (0x8da1e)
  moves him to the next free seat. It refuses inside 250 ms of the last
  change (`GMI_GAME_VEHICLE_DELAY_IGNORE`) and stamps the change 1500 ms
  ahead, so changes are 1750 ms apart (INFERRED (decompile)). The new seat's
  entry runs `SetClientViewAngle` again: both capture cycles move
  `delta_angles` (VERIFIED, capture). A Panzer IV (`numPassengers 0`)
  cycles 1 → 2 → 1, a Willys 1 → 2 → 3 (VERIFIED, capture).
- **The hatch is disabled.** `G_VehiclePopOut` (0x8bbb0) opens with
  `Com_Error(1, "G_VehiclePopOut: Popout disabled\n")` (string 0xa74c0,
  call 0x8bbe6, VERIFIED (the instructions)), ahead of the code that would toggle
  0x400000 and move the rider to `tag_popout`. Nothing calls it and no
  data word holds its address; the only word equal to 0x8bbb0 is its own
  `.dynsym` entry (VERIFIED, `objdump` and a byte search). So retail never
  sets 0x400000, and the cgame's tests of it (`CL_KeyMove`'s `upmove`, the
  seat crosshair 0x30019cf0, the seat HUD owner draws from 0x30031cb0)
  never see it set (INFERRED).

## 2. The cmd a seat sends, and what the server reads

The client sends its on-foot usercmd; nothing in `CL_CreateCmd`
(CoDUOMP.exe 0x40c900) or `CL_MouseMove` (0x40c330) tests a seat (VERIFIED (the instructions)). One thing changes:

- **upmove.** `CL_KeyMove` (0x40bf40) adds the kbutton at 0x8cdf18 to
  `upmove` while `pm_type` is 2, 3 or 4, or while `eFlags` hold 0x100000
  without 0x400000 (VERIFIED (the instructions), 0x40c056..0x40c099). `+moveup`
  (0x40b740) and `+gostand` (0x40bdf0) both press it (VERIFIED (the instructions)), so stock SPACE (`+gostand`) puts 127 in a seated cmd's
  `upmove`.

The server reads each seat's stored cmd in the vehicle think, which
`G_VehicleClientThink` (0x8b0ce) runs for every driven vehicle from
`G_RunFrame`, 50 ms apart (VERIFIED (the instructions)):

| Seat | Function | What it reads |
|---|---|---|
| Driver, moves | 0x87cd3 | `forwardmove`, `rightmove`, `upmove` of the last cmd; skipped while popped out or within 150 ms of a collision. Only signs count: forward accelerates toward 0.8 `maxSpeed`, back toward half that, `rightmove` steers, a tank with `upmove > 0` turns its hull toward the view yaw at up to `rotRate`. A reversing tank's steer is negated; one with no turret span (SU-152, Elefant) treats a held sight as `upmove` 127. Clears `eFlags` 0x200000 on a tank or under `upmove > 0`, sets it otherwise (INFERRED (decompile)). |
| Driver, turret | 0x85b32 | View pitch and yaw in the hull's frame, stepped at `turretRotRate * 0.05` a think, held inside the file's spans; out on `angles2[0]`, `angles2[1]` (VERIFIED (the instructions)). Stock rates: Panzer IV and T-34 30, Sherman 32, Flak 88 20 (VERIFIED, the files). The spans bind the gun, never the view. |
| Driver, fire | 0x86c57 | `buttons` 0x1 fires the main gun (`turret_fire`), 0x20 the alternate (`turret_alt_fire`), each on its own timer; aimed along the view from `tag_chasecam`, or `tag_aimdownbarrel` sighted (VERIFIED (the instructions)). No turret: 0x1 sounds the horn (INFERRED). |
| Gunner | 0x86285, 0x871a6 | View pitch and yaw in the hull's frame at `turretGunnerRotRate` (stock 360) inside the gunner spans; pitch on `angles2[2]`, yaw on `origin2[1]` relative to the turret; 0x1 fires (`turret_gunner_fire`) (VERIFIED (the instructions)). |
| Passengers | | Nothing to the vehicle; the four-wheel's seat 3 fires its own weapon through `PM_Weapon` (section 3). |

In the capture (VERIFIED,
`the_tank_capture_drives_and_turns_on_the_seat_cmds`) `forwardmove` 127
carried the Panzer IV over 200 units with its yaw untouched, and `upmove`
127 turned the hull more than 100°, to within 3° of a view sweeping from
-90° to -30°.

## 3. Pmove in a seat

The client predicts a seated player; the prediction moves nothing.

- `CG_PredictPlayerState` (cgame 0x30035800) runs `Pmove` for every seated
  cmd as on foot; it bails only on demo playback, `pm_flags` 0x40000,
  `cg_nopredict` and `g_synchronousClients` (INFERRED (decompile)).
- Before the cmd loop, seated, it fills `pm` +0x3c..0x44 with the vehicle
  centity's lerped angles and `pm` +0x48/+0x4c with 60 and 70 for the
  four-wheel's seat 3, 0 elsewhere (INFERRED (decompile)). The server's
  `ClientThink_real` (0x40c91) fills them from the hull's angles (VERIFIED (the instructions)).
- `PmoveSingle` (0x2d530) takes a no-move arm on `eFlags & 0x106000`
  (VERIFIED (the instructions)). It clears `pm_flags` 0x10, sets `groundEntityNum`
  1023, zeroes the ground plane and walking flag, and runs the sight flag,
  walking flag, sprint flag, fatigue and two unnamed steps (INFERRED (decompile)). It ends on `PM_Weapon` where
  `BG_AllowPlayerWeaponAtVehiclePos` (0x23148: `vehType` 1, `vehPos` 3)
  allows, else on `PM_UpdateAimDownSightLerp` (INFERRED (decompile)).
  `pm_type` 1 takes the same arm (INFERRED, jump table at 0x9b9a8).
- **The body.** `ClientEndFrame` (0x42f59) calls
  `G_PlayerVehiclePositionAndBlend` (0x9048f), which puts `ps.origin` at
  the seat tag plus `BG_GetVehiclePosOffset` (INFERRED (decompile)). Nothing
  writes the velocity (INFERRED, absence).
- **The sight.** `PM_UpdateAimDownSightFlag` (0x32c0b) sets `pm_flags` 0x20
  off the button alone in a seat (VERIFIED (the instructions)).
  `PM_UpdateAimDownSightLerp` (0x3349e) wants the sight whenever the flag
  is set in a seat, snaps `fWeaponPosFrac` to 0 or 1 in seat 1 and ramps
  it at the held weapon's rates elsewhere (VERIFIED (the instructions)). The
  capture's driver reads 0 then 1 with no step between (VERIFIED,
  capture).
- **The view.** `PM_UpdateViewAngles` (0x2b15c) clamps pitch at 16000
  shorts on foot and 12000 seated by rewriting `delta_angles[0]` to
  `limit - cmd.angles[0]` (INFERRED (decompile)); the capture's view stops
  at 65.92° both ways (VERIFIED, capture). Then, seated, each axis whose
  `pm` +0x48 arc is ≥ 1.0 is held within the arc around the vehicle's
  angle: the excess is pushed onto `delta_angles` as `excess * 182.0444`
  and the view set on the arc's edge (INFERRED (decompile); VERIFIED 1.0 double at
  0x9b930, 182.0444 at 0x9b938, 0.0 at 0x9b93c, VERIFIED bytes). The push
  truncates, as vcod's other pushes do (INFERRED from the replay
  matching).
- **The cone.** `PM_AdjustAimSpreadScale` (0x35dcb) counts a body off the
  ground as airborne only when `pm_type` is not 1 (INFERRED (decompile)), so
  a seat opens no cone. A seated rider is `pm_type` 1 on the wire
  (VERIFIED, capture), so the cone decays at the weapon's standing rate:
  the tank driver's reads 0 by the first snapshot, 900 ms after the seat,
  and climbs only on a turn (VERIFIED, capture). vcod-server marks a seat
  linked for the move, as the predictor does off `pm_type`; run unlinked,
  the airborne add held the driver's cone at 255 (VERIFIED, `vehicle_ab`
  replay).

## 4. The vehicle's own motion

The client simulates no vehicle: neither `CoDUOMP.exe` nor the cgame
carries vehicle physics (`coduo-diff.md` 9, VERIFIED absence). The seated
body's origin is the snapshot's; the drawn vehicle is the interpolated
snapshot entity.

## 5. The view

`CG_CalcViewValues` (0x30041a30) runs 0x30040580, then, unless rendering
third person, `CG_VehicleView` (0x30040810) for a seat with a valid
`viewlocked_entNum`, else `CG_OffsetFirstPersonView` (0x3003fb60)
(INFERRED (decompile)). A seat takes no kick, bob, landing dip or lean
(INFERRED). `cg_thirdPerson` plays no part: the cgame only registers it
and resets it to 0 (VERIFIED (the instructions)).

`CG_VehicleView` poses the vehicle first (0x30021d30), so every tag comes
off the drawn, interpolated vehicle (VERIFIED (the instructions)). Its branches
(VERIFIED (the instructions); constants VERIFIED bytes at the cited addresses):

- **Artillery** (class 5), no sight: the eye is `tag_chasecam`.
- **Seat 2, the gunner**, any vehicle: gun frame G is `tag_secondary_gun`;
  the eye is `tag_secondary_view`, else G's origin -20 forward, 6.5 up
  (0x3007c2b4, 0x3007c2b0). The refdef takes G's pitch and roll and keeps
  the view's yaw (VERIFIED (the instructions) 0x30040c3d). When the view's pitch
  is more than 0.4° (double at 0x3007c2a8) off G's, trap 0xf7 sets the
  client's view to G's pitch and yaw less `delta_angles` (VERIFIED (the instructions)); the engine writes it into `cl.viewangles` (CoDUOMP.exe,
  INFERRED (decompile)). So the gunner's aim sits on the gun the server turns
  at 360°/s. A sway and a firing shake ride the eye (below).
- **First person** (four-wheel's seat 3, a four-wheel with no sight,
  anything else sighted): on a four-wheel the seat tag
  (`BG_GetVehiclePosTag`, 0x30008190) -12 forward, +32 up (0x3007bdc4,
  0x3007bdd0); elsewhere, or with no such tag, `tag_aimdownbarrel`, else
  `tag_barrel` +30 forward, -3 left, +10 up (0x3007becc, 0x3007be5c,
  0x3007bda4). Angles are the view's.
- **The chase camera** otherwise (a tank driver at the hip, a four-wheel's
  driver and passengers sighted). Pivot T is `tag_turret` for seat 1 and
  `tag_passenger`..`tag_passenger4` for seats 3..6 (0x300407c0). The
  offset is `tag_chasecam` less T (×2.5 on a four-wheel, 0x3007be68), else
  T's forward -200 and up 32 (0x3007c2a4). It is taken into T's frame and
  put back on the axis of the view's angles at roll 0, so the camera
  orbits T as the view turns. A tank scales the vertical part by 0.75
  (0x3007be38); a sighted four-wheel's camera drops 135 along the view's up
  (0x3007c2a0). The distance from T is held between half and twice the
  offset's length.
- **The gunner's sway.** The arm copies `cg.predictedPlayerState`, puts
  `thompson_MP` (string 0x3007a718, looked up by name through 0x300110f0)
  in the copy and runs 0x30015ca0 on it with scale 2.0 and `cg.frametime`
  (pushes at 0x30040964..0x3004096d, VERIFIED (the instructions)). 0x30015ca0 is
  the turn sway vcod ports as `BG_CalculateWeaponPosition_Sway`
  (`cod11-combat.md` 15): each view axis' turn since the last call,
  clamped to `swayMaxAngle`, times the `sway*Scale`s and the scale, eased
  at `swayLerpSpeed` by `frametime * 0.001` (VERIFIED (the instructions); 0.001 at
  0x3007bd94, VERIFIED bytes). Its state is at 0x300dc9dc..0x300dca00
  (VERIFIED (the instructions)); nothing else writes it, so no seat change resets
  it (INFERRED). The eye moves by the offset in the gun's frame,
  `fwd * o[0] + left * o[1] - up * o[2]`, and only `o[1]` and `o[2]` are
  ever written (INFERRED (decompile)).
- **The gunner's firing shake** (0x30040ae3..0x30040d26, VERIFIED (the instructions)) runs while `cg.snap`'s `eFlags` hold `EF_FIRING` (UO's
  0x200) and the vehicle centity's es +0xe4 (`animMovetype`) reads 0.
  Phase is `cg.time % 100` folded at 50, times 0.02 (0x3007c2b8). The eye
  backs along the gun by `phase * 0.5` (-0.5 at 0x3007bf50); after the
  snap the refdef pitch lifts by `phase * 0.5` (0x3007bce8) and the yaw
  gains `0.25 * sin(2.334 * 2π t) * cos(3.198 * 2π t)`, `t = cg.time /
  1000` (0x3004d200 with 2.334 and 3.198 pushed as floats, 2π at
  0x3007bd00, 1000 at 0x3007bd08, 0.25 at 0x3007be58; `fsin`/`fcos` CRT
  stubs at 0x3006ba30, 0x3006b980; VERIFIED bytes and disassembly). The
  gunner's think (game 0x871a6) writes +0xe4 to 1 at an overheat and 0 once
  the gun cools (INFERRED (decompile)). Over the seat bits 2 and 6 the
  captures show while crewed (VERIFIED, `snapshots-uo-overheat.bin`: 6,
  then 1 at the overheat, then 0 with both seats still taken). So a crewed
  vehicle's gunner shakes only after its gun has overheated and cooled
  (INFERRED from the two).
- **Clip.** Every branch ends on a ±8 box trace (mask 0x211, vehicle
  skipped) from `tag_body`, else the vehicle's origin, 20 up (0x3007be04),
  to the eye; retraced under 0x11 when it starts in a 0x200 brush. A hit
  stops the eye there, `(1 - fraction) * 32` higher, and a second trace
  under 0x11 brings it back (VERIFIED (the instructions) 0x30040d30..0x3004152e).

The stock MP Sherman, T-34, Panzer IV, Flak 88 and Willys carry
`tag_chasecam` and `tag_aimdownbarrel`; all but the Flak 88 carry
`tag_secondary_view` (VERIFIED, xmodelparts strings).

**Sight toggle.** MOUSE2 is `toggle cl_run` in UO's `default_mp.cfg`
(VERIFIED, the pak). With the mount's `cl_run` 1, the right button takes a
tank driver from chase camera to gunsight and a jeep driver from first
person to chase camera (INFERRED from the branches).

**The four-wheel's riders** (0x30040580). On a four-wheel, for every seat
but 2 and the passenger with his own weapon, with a weapon in hand, the
cgame keeps last frame's vehicle axis and turns the view by the vehicle's
turn since: new axis = view × last frame's vehicle axis transposed × this
frame's. It writes the pitch while `fWeaponPosFrac` is 0 and the yaw while
`eFlags` 0x200000 is set, and sets the client's view through trap 0xf7
(VERIFIED (the instructions)). That the product turns the view by the vehicle's
turn is a reading of the matrices (INFERRED). A jeep driver's view swings
with the jeep unless `upmove` is held, which steers toward the view
instead (section 2).

**The riders' roll** (0x30040770..0x3004078e, VERIFIED (the instructions)).
After trap 0xf7 the same arm multiplies the view's axis, built from the
predicted `viewangles` before the pitch and yaw writes (0x300406b3), by
this frame's vehicle axis transposed (0x30489ef8, which 0x3004ac80 has just
filled with the transpose), runs `AxisToAngles` (0x3004c2a0) and stores the
negated roll into the predicted `viewangles[2]` (0x304832b4). 0x3004a5b0 is
`MatrixMultiply(in1 = ecx, in2 = eax, out = edx)` and 0x3004ac80
`MatrixTranspose(in = ecx, out = eax)` (VERIFIED, the instructions). So the
roll is written whatever the sight, for the seats the turn covers.
`CG_CalcViewValues` copies `viewangles` into `refdefViewAngles`
(0x30041bd2..0x30041bff) after the arm; the chase camera zeroes the roll
(0x30040dc0) and the first-person branch keeps it (VERIFIED, the
instructions). Looking along the hull the drawn roll is the hull's roll,
looking back its opposite, looking aside its pitch (INFERRED from the
matrices; `seat::tests::a_four_wheel_rolls_its_riders_with_the_hull`). The
arm reads `cg.predictedPlayerState`, which a followed player's snapshot
fills, so a followed jeep driver rolls too (INFERRED).

**Which angles.** The four-wheel turn builds the vehicle's axis from the
centity's `lerpAngles` (+0x214, `AnglesToAxis` at 0x3004060f, VERIFIED (the instructions)) after a `tag_player` lookup that only gates it (string
0x30072d68 loaded at 0x300405f1, VERIFIED). The prediction's `pm`
+0x3c..0x44 come from the same field (section 3). The frame runs
`CG_PredictPlayerState` (inside 0x30036070) before `CG_CalcViewValues`
(0x30041a30) and the packet entities (0x3001f810) (VERIFIED (the instructions),
0x30036780..0x30036790). `CG_CalcEntityLerpPositions` (0x30021d30,
VERIFIED (the instructions)) writes `lerpAngles`, called from `CG_VehicleView`,
0x30040580 and the packet entities. So the prediction reads the angles
lerped the frame before (INFERRED from the order).

**Field of view** (0x3003ffc0, VERIFIED (the instructions)). In a seat other than
the four-wheel's seat 3, the driver with a sighted weapon closes from
`cg_fov` toward 70 (0x3007bf84) by `fWeaponPosFrac`; every other seat
keeps `cg_fov`.

## 6. View weapon and HUD

- `CG_AddViewWeapon` (0x30046570) draws no gun while `eFlags & 0x106000`,
  except in the four-wheel's seat 3 (INFERRED (decompile)).
- The seat crosshair (0x30019cf0) and vehicle status owner draws are
  `coduo-diff.md` 19's; vcod draws them.
- **The own body.** `CG_AddPacketEntities` adds `cg.predictedPlayerEntity`
  when `pm_flags` hold UO's 0xc0000 (0x3001f810, INFERRED (decompile)),
  built as the game module's `BG_PlayerStateToEntityState` builds one:
  seat in `surfType` bits 0-2, `vehType` in 3-5, `vehMotion` in 6-7,
  `viewlocked_entNum` in `otherEntityNum` while `eFlags & 0x106000`
  (INFERRED (decompile)). `CG_Player` (0x300343e0) sends a seated body
  (`eFlags` 0x100000) through 0x30032fe0, draws nothing when that returns
  0, and draws a seated body with `renderfx` 0 where a first-person own
  body gets 2 (INFERRED (decompile)). 0x30032fe0 (from 0x3003392b, VERIFIED (the instructions)) returns 0 for a tank's seat 1, and, for `cg.snap`'s own body
  outside third person, for a four-wheel's seats 2 and 3, any other
  four-wheel seat while the predicted `fWeaponPosFrac` is 0, and a tank's
  seat 2. So the jeep driver and passengers 4..6 see their own body with
  the sight up (when the view leaves for the chase camera), and an
  artillery crew always (INFERRED from the branches). INFERRED (decompile):
  `CG_Player` runs the anim routine (0x30005860) before 0x30032fe0, whose
  refusals return before it writes `lerpOrigin` (`cent+0x208`), so a
  hidden body's DObj is stepped where the entity pass left it; vcod poses
  it there undrawn for its tags (`cod11-events-and-fx.md` 2,
  "playFxOnTag").

## 7. Prediction against the capture

`moves_ab`'s `the_predictor_replays_uo_seats` rebuilds every snapshot of
section 8's two seat captures through `predict::from_wire` with its
vehicle's angles and predicts it to the next under UO's rules. Every
field matches retail but these, which the predictor alone misses
(VERIFIED, replay):

- `delta_angles` at each `nextvehslot`: the new seat's
  `SetClientViewAngle`, the server's.
- `aimSpreadScale` on the first pair, which has no cmd at its
  `commandTime`.
- Tank: `aimSpreadScale` and `eventSequence` on the main gun's snapshot,
  where the driver took damage feedback. That snapshot carries
  `damageEvent` 1, `damageCount` 44, `damageYaw` 105, `damagePitch` 10,
  `aimSpreadScale` 44.0 and one event, v1 187 (UO's 191, `EV_PAIN`) parm
  100 (VERIFIED, capture). `P_DamageFeedback` (game 0x3f65a) adds the
  damage count to `aimSpreadScale` and raises `EV_PAIN` (0xbf) with
  `100 * health / maxHealth` (VERIFIED (the instructions)); parm 100 is a driver
  whose health did not move, as a tank driver takes no damage
  (`coduo-diff.md` 9). That the hit is the tank's own shell is INFERRED
  (the snapshot is the shot's). Neither is pmove's: the hull's damage
  reaches the driver through `Scr_Vehicle_Pain` (13, "The driver's
  feedback"), which `vehicle_ab` compares on the server.
- Tank: the landing after the exit once read surface 0 against retail's
  17 (rock). Two brushes meet under (4369.4, -2165.1, -7.875) on
  mp_cassino: a `textures/common/caulk` brush whose top side is
  `textures/italy/floors/rock@cobblestone-01` (0x1100000, material 17)
  and a `textures/common/clip_nosight_rock` one (0x11040a0, material 17)
  (VERIFIED, lumps 0, 3 and 4). A trace reports the struck side's
  material (`bsp-ibsp59-format.md`, lump 3), 17 on either brush, and the
  landing matches (VERIFIED, `moves_ab`).

Two rows the gate once carried are closed:

- The tank's two `aimSpreadScale` rows (33.8 vs 32.8 inside the pitch
  clamp, 88.6 vs 78.4 before the exit) were the replay's. The capture
  holds two cmds stamped 20750 and two stamped 47500 (VERIFIED, capture).
  Retail ran the first of each pair and dropped the second: the snapshot
  at 20750 reads pitch 62.720947, the first cmd's 51244 plus
  `delta_angles[0]` 25710, not the second's 51526 (VERIFIED, capture).
  UO's `SV_UserMove` (`coduo_lnxded` 0x808d8af) runs `SV_ClientThink`
  (0x808d829, which copies the cmd into `lastUsercmd`) only for a cmd
  whose `serverTime` is past `lastUsercmd`'s (VERIFIED (the instructions)), as
  1.1's does (`protocol-1.1.md`, "How long a cmd is simulated for"). With
  the first kept, both rows match (VERIFIED, replay).
- The jeep passenger rechambered one cmd late through vcod's weapon timer.
  `weaponTime` reaches -2 on the cmd ending 46032 (jeep capture: 39 at
  45991, then cmds of 9, 8, 8, 8, 8 ms, VERIFIED, capture); the snapshot
  at 46041 reads `weaponTime` 1000, `weaponDelay` 400, `rechamberTime`
  and `rechamberBoltTime` whole (VERIFIED, capture). `PM_Weapon`'s timer
  step (game 0x360fa) stores 0 into `weaponTime` and leaves `weaponstate`
  alone unless the semi-automatic latch holds; only the latched arm
  writes state 0 (INFERRED (decompile), ghidra 0x460fa). The rechamber check
  (0x340ab) opens a rechamber only from state 0 (INFERRED (decompile)), and
  the trigger-release check (0x366b2) that ends a released shot's state 3
  runs after it (INFERRED (decompile), `PM_Weapon` 0x3726c). So the state
  goes 3 to 0 after the rechamber check on the cmd the time runs out, and
  the next cmd rechambers with its timers untouched by its own msec
  (INFERRED, call order). 1.1's timer (`.so` 0x387c0, ghidra 0x487c0) and
  1.5's (0x2f5ce, ghidra 0x3f5ce) have the same shape (VERIFIED (the instructions)). vcod's timer leaves the end of a released shot to the
  release check (`cod11-combat.md` 1.4); ending it in the timer was the
  bug. The seat was incidental: no on-foot capture had a bolt-action shot
  released before its `fireTime` ran out.
- The tank's `origin.z` and `velocity` on the exit's snapshot (the
  dismount, `VEH_UnlinkPlayer` setting the body down) match: the replay of
  2026-11-18 misses neither (VERIFIED, replay). Which change closed them is
  not traced. The gate now lists each gap by the commandTimes it misses on
  and fails on a listed row that matches.

Since 2026-11-26 the gate lists no gaps: the replay runs the two server
writes itself after the cmds (`moves_ab`'s `server_writes`). A seat
change takes the snapshot's `delta_angles` in place of
`SetClientViewAngle`'s, the new seat's tag view being the hull's; a new
`damageEvent` adds the snapshot's `damageCount` to the predicted
`aimSpreadScale` (capped at 255) and raises the snapshot's `EV_PAIN`, as
`P_DamageFeedback` does. With them the tank's main-gun snapshot matches on
`aimSpreadScale` and `eventSequence` (VERIFIED, replay). The server's own
`delta_angles` write is `vehicle_ab`'s: its absolute value rides each
side's own cmd angles, so the gate compares each snapshot's step in
`delta_angles[0]` and `[1]` from the one before. On both seat captures
ours steps as retail's at every snapshot, the four seat changes included
(the tank's (-305, -18) at 33536 and (-3812, 385) at 41042, the jeep's
(-6591, -27029) at 32241 and (-10188, -27097) at 39741 as shorts)
(VERIFIED, the gate).

With the seat stripped from the playerstate the replay misses
`delta_angles` on 44 and 230 snapshots, `fWeaponPosFrac` and the driver's
weapon state (VERIFIED, replay), so the gate does test the seat.

The server's half of the round trip (`ClientSim::to_wire`, protocol map,
codec, `from_wire`) is `predict_ab`'s UO gate over the same six captures,
with the seat put on the sim at each snapshot (`coduo-movement.md` 15).

### 7.1 The push-off

`CG_PredictPlayerState` (0x30035800) runs its own version of the game's
section 11.6 ahead of each predicted cmd's `Pmove` (VERIFIED (the
instructions), 0x30035dba..0x30035f87):

1. A `CG_Trace` (0x30035390) position test at `ps.origin` with the box of
   `ps` +0x55c..+0x570, `maxs.z` 30 under `pm_flags` 1 and 50 under 2,
   passing `ps` +0xd4, under `pm.tracemask` (0x2810011 below `pm_type` 6,
   0x810011 from it, less 0x2010000 when `cg.snap`'s `pm_type` is 4;
   0x300358b0..0x300358cf). Nothing tests a seat: the arm runs seated too.
2. Allsolid or startsolid (trace +0x2e, +0x2f) on an entity whose centity
   is valid (+0x1e8) with `eType` 12 (+0x4): `dir` is `ps.origin` less the
   centity's `lerpOrigin` (+0x208), `VectorNormalize`d whole (0x30049700),
   and the velocity's `VectorNormalize2` (0x30049920) gives `along` and the
   speed; `dir` then loses its z. Both normalizers store the squared length
   and the length as floats and multiply by `1 / length`.
3. Speed under 1.0 (0x3007bce0), or `along.y * dir.y + along.x * dir.x`
   past 0.8 (0x3007bdf0): the centity's +0x238 is set, which
   `CG_ClipMoveToEntities` (0x300350f8) skips, and cleared after the
   `Pmove` (0x30035f7d). Otherwise nothing.
4. Then, not prone (`pm_flags` 1) and speed under 1.0: the velocity becomes
   `(200 dir.x, 200 dir.y, 0)` (200.0 at 0x3007bf44), `pm_time` 200 and
   `pm_flags` takes 0x200.

So the client has neither the game's seat test nor its moving-vehicle arm
(`VEH_PlayerCollision`, the 200 floor on the struck velocity): off a moving
vehicle it predicts the resting push, and the snapshot corrects it
(INFERRED from the two). The prone damage is the game's alone.

Run over the binary seat captures (`gamestate-uo-*.bin`, `snapshots-uo-*.bin`)
on their maps, with every vehicle's collmap where its snapshot has it, the
seated box starts inside the jeep's collmap on all 736 seated snapshots of
`-uo-jeep` and 1041 of `-uo-overheat`, and inside the tank's on 52 of 734
(VERIFIED, a one-off replay through `vehicle_push_off` and vcod's trace,
2026-11-23). So retail's client predicts a jeep rider
pushed off his own jeep on every frame (INFERRED). The seat's pmove moves
nothing (section 3) and nothing vcod draws in a seat reads the predicted
velocity.

vcod: `play::predict::vehicle_push_off` before each replayed cmd, the
collmap copy unlinked for the cmd as the server's is
(`CollisionWorld::unlink_model_instance`), off `Predictor::vehicles`, the
snapshot's vehicles where they were drawn last frame.

## 8. Captures

All retail UO 1.51b, in `crates/server/tests/fixtures/playerstate/`.
Recipe: `client-probes/probe_vehicle` installed as the gametype
`probe_vehicle` with `+set probe_vehicletype panzeriv_mp` (or
`willyjeep_mp`), then `vcod --game-dir <UO install> --mod-dir uo
--net-probe <addr> --probe-team axis --probe-vehicle seats --save-slope
--probe-cmd-ms 8 --capture-tag tank-seats-uo --probe-secs 55`.

| Capture | Date | What |
|---|---|---|
| `mp_cassino-tdm-slope-8ms-tank-seats-uo.txt`, `mp_peaks-tdm-slope-8ms-jeep-seats-uo.txt` | 2026-10-14 | One probe through every seat of a Panzer IV (mp_cassino) and a Willys (mp_peaks, `+set probe_side 75 +set probe_pitch 15`). Script `vehicle_probe.rs` `SEATS`: enter, pitch past the clamp both ways, sight, drive, turn the hull with `upmove`, fire, `nextvehslot` twice, sweep the gunner's gun and the passenger's arcs, fire from the passenger's seat, get out. Each `!snap` line carries `vang`, the seat's vehicle angles as that snapshot sent them. |
| `mp_peaks-tdm-slope-8ms-jeep-gun-uo.txt` | 2026-10-16 | `--probe-vehicle gun --capture-tag jeep-gun-uo` (`GUN`): use, `nextvehslot` 4 s in (the change waits out 1750 ms; 1.5 s was refused), then the gun swept past its pitch spans both ways, round both sides and behind, and a 300 ms flick, each sweep held a second; then out. `veh=` lines carry the jeep's whole entity state, `angles2[2]` and `origin2[1]` included. |
| `mp_peaks-tdm-slope-8ms-jeep-gun-spawn-uo.txt` | 2026-10-20 | `GUN` with `--slope-from-spawn`: logs from the first playing snapshot, so the spawn's think, the placement's fall and the boarding are in it. Written as `--capture-tag jeep-gun-r5` and renamed: the only one of five runs whose spawn (tdm spawnpoint 277) gave seat 606. Its `PROBE` lines (`PROBE spawned` among them) are `fixtures/vehicles/mp_peaks-jeep-gun-spawn-uo.txt` (section 13, "The driver's idle"). |
| `mp_cassino-tdm-slope-8ms-tank-gun-uo.txt` | 2026-10-20 | `GUN` on the Panzer IV (`+set probe_vehicletype panzeriv_mp +set probe_bones 1`, `--capture-tag tank-gun-uo`), the gun on `tag_secondary_player`'s base with the tank's 50-up span. Its `PROBE bone` lines are `fixtures/vehicles/mp_cassino-tank-gun-bones-uo.txt`. `vehicle_ab`'s `the_tank_gun_replays_on_ours` matches every field of all 923 snapshots, the gun's included (VERIFIED, replay); `the_tank_rider_s_bones_follow_retail_s_in_both_seats` matches both stints' marks (section 13, "The tank driver's link offset"). |

`cargo run -p vcod-common --example rider_dump -- gamestate-uo-tank.bin
snapshots-uo-tank.bin` lists every seated player entity of a binary UO
capture in `crates/common/tests/fixtures/net/` with its trajectory fields.

## 9. What vcod does

- **Prediction.** `pmove::VehicleSeat` (`PlayerState::seat`) comes off a
  UO playerstate's `eFlags`, `vehPos` and `vehType`; the caller fills the
  vehicle's angles as drawn the frame before
  (`entities::BuiltScene::vehicle_angles`), the newest snapshot's until
  it has been drawn. The four-wheel turn takes this frame's. `pmove`'s
  seat arm and `cmd::clamp_view` follow section 3; the 16000 pitch clamp
  applies on foot under every patch's rules.
- **View.** `play::seat::camera` and `clip` follow section 5, off
  `EntityScene::tag_pose` on the drawn vehicle and the world's box trace,
  with the gunner's sway (`play::seat::GunnerSway`, `aim::step_turn_sway`
  off configstring 7's `thompson_mp`) and firing shake
  (`play::seat::gunner_shakes`, `shake_phase`). The four-wheel riders'
  roll is `play::seat::seat_roll`, drawn in first person, a followed
  rider's included.
- **The own body.** `play::seat::own_body_drawn` is 0x30032fe0's gate,
  read on the previous frame's predicted sight; `play::seat::own_body`
  builds the entity `BG_PlayerStateToEntityState` would, drawn by the seat
  path of `entities::build_instances`. The gunner's snap and the
  four-wheel's turn go through `PlayInput::set_view`, the trap 0xf7 write.
  A followed rider gets the seat camera on the snapshot's view, without
  the writes.
- **Other riders.** The cgame's seat placement 0x30032fe0 reruns the
  server's `G_PlayerVehiclePositionAndBlend` on the drawn vehicle:
  - It writes `AxisToAngles` of the seat tag's axis into the entity's
    `lerpAngles` (`cent+0x214`, call at 0x30033a68) and the client info's
    `+0x3f4` (0x30033a77) (VERIFIED (the instructions)). The controllers
    0x30004da0 read `+0x3f4..+0x3fc` for a rider (`eFlags` 0x100000), and
    turn the torso with the view only in a four-wheel's seat 3 with the
    entity's `weapon` (`+0xcc`) non-zero (VERIFIED (the instructions)); game.mp.uo
    0x1e761 makes the same `weapon` test (VERIFIED (the instructions)).
  - It sets the same goal weights as the server's four arms on a legs anim
    with `turretanim` (client info `+0x390`, record `+0x394`, `+0x50 & 4`)
    (VERIFIED (the instructions)). The steer is `LerpAngle` of the vehicle's
    current and next `leanf` (`cent+0xd8`, `cent+0x1cc`) by the frame's
    interpolation (0x304831a8) (VERIFIED (the instructions),
    0x30033083..0x30033097), scaled by 1/60, not the server's 1/45:
    `0x3007c2c8` holds `1/60` as a float for a positive steer and
    `0x3007c2c0` as a double for a negative one, capped at 1.0
    (`0x3007bce0`, `0x3007bcf8`) (VERIFIED, file bytes and disassembly
    0x30033186..0x300331e3). A positive steer takes the group's last
    child, a negative one child 0 (VERIFIED (the instructions), `dec eax` at
    0x300331a1). So the drawn jeep driver turns his wheel less than the
    server poses him.
  - Each goal weight is set over `1 / ((1000 / frametime) * |weight -
    goal|)` (`0x3007be88` 1000.0, VERIFIED file bytes; `0x304831ac` is
    `cg.frametime`, INFERRED from its other readers, which count timers
    down by it). The setter stores `|goal - weight|` times that as the
    goal time (`cod11-movers.md` 15, "The weight ramp"), so the weight
    reaches its goal in one frame (INFERRED). vcod draws the goal weights
    at once. The server's `1 / (10 |change|)` ramp is a fixed 100 ms
    (INFERRED), which vcod-server plays (section 14, "The seat blend's
    ramp").
  - `BG_UpdatePlayerDObj`'s cgame copy 0x300058f0 drops the held weapon
    from the DObj when `eFlags & 0x106000` and
    `BG_AllowPlayerWeaponAtVehiclePos` (0x30008210) refuses the rider's
    `surfType` (INFERRED (decompile)); game.mp.uo 0x1f556 does the same
    (INFERRED (decompile)).

  vcod's draw path (`entities::rider_frame`, `body_controllers`) builds
  `playerpose::BodySeat` off the drawn seat tag and lerped steer
  (`BodySeat::drawn`, `STEER_FULL_CGAME`) and poses the rider through the
  server's `Controllers::rider` and `seat_inputs`. `entities`'s
  `a_drawn_rider_is_posed_as_the_server_poses_him` replays every rider in
  `snapshots-uo-jeep.bin`, `-uo-tank.bin` and `-uo-overheat.bin` (667 jeep
  gunner and 1637 tank gunner samples): the seat tag off the drawn vehicle
  matches the server's hull-only pose to 0.001, and the controllers and
  seat leaves match the server's `BodyPose` given the cgame's steer scale
  (VERIFIED, replay). The drawn tag sits up to 4.9 units off the rider's
  sent origin, which retail took from the controls the frame started from
  (section 11.4) (VERIFIED, replay).
- **Input.** The stock keys make section 2's bytes
  (`a_seat_drives_with_the_stock_keys`); `weapnext`/`weapprev` send
  `nextvehslot`/`prevvehslot` in a seat; `v cl_run` sets the cvar and
  `v cl_stance` the stance.
- **View weapon and fov** as sections 5 and 6.
- **The steering wheel.** The vehicle callback (0x30020540) turns
  `tag_steeringwheel` by the lerped steer × 0.75 (`0x3007be38`), or × 4.0
  (`0x3007be40`) when `cg.snap`'s player sits in this vehicle's seat 1
  (`eFlags` 0x100000, `vehPos` 1, `viewlocked_entNum` the entity) with
  `fWeaponPosFrac` 0 (0x30020b8b..0x30020bf9) (VERIFIED (the instructions) and file
  bytes). `entities::steering_ratio` reads `cg.snap` as the older snapshot
  of the drawn pair.
- **The own gunner's gun.** When `cg.snap`'s player sits in this vehicle's
  seat 2 (`eFlags` 0x100000, `vehPos` 2 at ps +0x614, `viewlocked_entNum`
  the entity), the callback poses the gunner's base and gun off the view,
  not the wire's `angles2[2]`/`origin2[1]` (0x300207c0..0x30020a2b,
  VERIFIED (the instructions)):
  - The hull is the centity's `lerpAngles` (`cent+0x214`) plus the lerped
    body pitch and roll, the same `origin2[0]`/`[2]` unpack the remote case
    reads (0x300207f0..0x30020838).
  - The view is a copy of `cg.predictedPlayerState` (0x304831c4, 0x1141
    dwords) run through `PM_UpdateViewAngles` (0x3000c8e0) with the newest
    cmd (traps 0x53 and 0x54), skipped when `cg.snap`'s `pm_flags` hold
    0x40000 or a demo plays (0x30459148); its `viewangles` are the copy's
    +0xe8 (VERIFIED (the instructions) 0x3002083b..0x3002088b).
  - `AxisToAngles(viewAxis * hullAxis^T)` (0x3004c200, 0x3004a5b0,
    0x3004c2a0) gives the view in the hull's frame. Its pitch, through
    `AngleNormalize180` (0x3004be60), is held to `-es.fTorsoPitch ..
    es.fWaistPitch` (`cent+0xec`, `+0xf0` of `cg.snap`'s state; the
    entityState sits at `cent+0`, as `origin2[0]` at +0x5c and
    `angles2[2]` at +0x70 match UO's netfield offsets); the yaw is not
    clamped (VERIFIED (the instructions) 0x30020926..0x30020970). The server
    writes the gunner's `vert_up` and `vert_down` into those two fields
    (section 14).
  - The clamp's excess is added to the view's pitch and the transform run
    again; the gun's pitch is the new local pitch, the base's yaw the new
    local yaw less the lerped turret yaw (`angles2[1]`) (VERIFIED (the instructions) 0x30020970..0x30020a40).

  So with the gun on a span the drawn yaw comes off the clamped view in
  the tilted hull's frame, while the server clamps the pitch of the
  unclamped transform and keeps its yaw (section 13): they differ by a
  fraction of a degree on a tilted hull (INFERRED from the two, and
  measured below). vcod: `vehiclepose::own_gunner`, called from
  `entities::build_instances`' vehicle loop when `entities::own_gunner`
  reads the older snapshot of the drawn pair, off `EntityScene::view`
  (the cmd's view; a followed gunner's off the snapshots). `vehicle_ab`'s
  `the_own_gunner_s_gun_settles_where_retail_s_server_puts_it` runs it on
  every settled seat-2 snapshot of the jeep gun capture: off the spans
  pitch and yaw land within 0.0055° (one 16-bit angle) of retail's
  settled server gun on 81 snapshots; on the spans (94) the pitch matches
  and the yaw is up to 0.63 off, the difference above (VERIFIED, replay).
  `the_jeep_gun_replays_on_ours` replays the capture on vcod-server: every
  vehicle field matches on all 501 seated snapshots, view and cone
  included (VERIFIED, replay).
- **The push-off** of a body inside a vehicle is 7.1's.
- **Not done.** The hatch needs nothing: retail
  disabled it (section 1). Vehicle sounds and the vehicle MG's
  client-traced impacts are `coduo-diff.md` 19's; wheel dust and a
  vehicle's death are section 15's.

## 10. The server: definition files and spawn

Sections 10 to 14 are the server's half: what `vcod-server` reproduces of
`game.mp.uo.i386.so` (`nm` addresses as above; `coduo_lnxded` addresses
are the engine's, non-PIC). Gentity offsets are UO's, 4 above 1.1's (UO's
`entityState_t` grew `xmodel`): `r.svFlags` +0xf8, `r.contents` +0x120,
`r.ownerNum` +0x154, vehicle record pointer +0x164, `health` +0x240
(VERIFIED, the netfield table at `coduo_lnxded` 0x080e11a0 and
`dump_script_fields.py`).

### 10.1 The files

- `G_ParseScrVehicleInfo` (0x8a51f) runs at the top of `G_InitGame`
  (0x586cf), after `BG_SetupWeaponInfo` and before every other configstring
  registration of the level (INFERRED (decompile)).
- It lists `vehicles/` through `trap_FS_GetFileList`, at most 64 files
  (`Com_Error` past that), each `VEHICLEFILE` plus an info string under
  8 KB, parsed over the 58-key table at 0xae9c0 (INFERRED (decompile) and
  table dump).
- The list is in search-path order: newest pak first, each pak's own entry
  order, a name already listed (case-folded) skipped, no sort (INFERRED: a
  simulation of that order reproduces the gamestate's alias block 662..727
  exactly). A UO-over-1.5 install has 39 files: `pakuo07`'s nine `_mp`
  files, `pakuo03`'s fourteen others, `main/pak0`'s sixteen (VERIFIED,
  `Pk3Fs::list_dir_in_search_order` against the paks; `coduo-diff.md` 9's
  23 counts the UO paks alone).
- After the parse `maxSpeed`, `accel`, `collisionSpeed` and
  `engineSndSpeed` are scaled by 17.6 (f32 at 0xa72b4) and the box's half
  extents by 0.5 (0xa72b8) (VERIFIED bytes).
- Per file in list order, the seven alias keys (`lowIdleSnd` ..
  `impactSnd`, `info+0x174 + 0x40 i`) then `vehicle_hit_person` go through
  `G_SoundAliasIndex`, so the vehicles hold aliases 1..66 (CS 662..727) on
  every map (INFERRED (decompile); VERIFIED capture, `configstrings_uo_ab`).
- vcod skips, with a warning, a file retail would load but whose numbers
  panic or hang its drive step: a number past 100000 or not finite, a
  negative `rotRate`, `rotAccel`, turret or gunner `RotRate`, `maxsX`,
  `collisionFrontDist`, `collisionBackDist`, `stepsize` or
  `suspensionTravel`, or `mins` past `maxs` (`VehicleInfo::check`). Every
  stock file passes (largest number 1760 after the scaling). A
  `script_vehicle` `angles` key past 131072 is reduced mod 360 at the spawn
  (`fguard::turns`), where retail's 360 folds would never end.

### 10.2 Spawning one

In BSP order, as any entity (`G_CallSpawn` 0x719a6, VERIFIED (the instructions)):

1. `G_Spawn` takes the number; the field parse indexes the `model` key
   (CS 405 + i) (VERIFIED (the instructions)).
2. `SP_script_vehicle` (0x8dec6): `delay` defaults to `g_vehicleBurnTime`
   ("10"); the spawn vars are backed up for `spawnDuplicate` (VERIFIED (the instructions)).
3. `G_SpawnVehicle` (0x8abfd): the first free slot of 64 (0x3ec bytes
   each), its index in `s.attackerEntityNum`; the file found by
   `strcasecmp` on `vehicletype`, else `Com_Error` naming the targetname
   (INFERRED (decompile)).
4. `fn_7c935`: `eType` 12, `r.svFlags` 4, `r.contents` 0x2080 (| 0x200000
   under spawnflag 1), both trTypes 1, `s.weapon` the turret weapon,
   `surfType` the file's type, `origin2[0]` and `[2]` the ints 127, think
   `Scr_Vehicle_Init` 50 ms on, `takedamage` 1, clipmask 0x201; the DObj's
   bounds; `RegisterItem` on the turret, alt and gunner weapons, which
   registers their models only (VERIFIED (the instructions)).
5. `fn_7cebf`: `otherEntityNum` 1023, the slot's record, the
   `useHintString` through `G_GetHintStringIndex` (CS 1365 + i, first equal
   or empty slot). Then up to four sound-blend entities: B1 when both idle
   aliases are set, B2 when both engine aliases are, B3 and B4 always
   (their test is a null check that never fires, 0x7d1e4). Each is a
   `G_SpawnSoundBlend` (0x5a65d): `eType` 9, contents 0, `svFlags |= 8`,
   `otherEntityNum` the vehicle, no classname, linked silent at once
   (`trap_LinkEntity` 0x7d19b, 0x7d1df, 0x7d214, 0x7d249) (VERIFIED (the instructions)). The snapshot builder culls a blend as its vehicle
   (`coduo-diff.md` 9, "The cause was the broadcast"). Then the alt and
   gunner weapons' `loopFireSound` and `stopFireSound` aliases (INFERRED (decompile)).

So mp_cassino's eight tanks take 156, 161, .., 191, each followed by its
four blends, in pool slots 0..7; dm keeps 176 (slot 4) and 181 (slot 5),
the trigger_hurt lands on 435 and the collmaps on 436, 437 (VERIFIED, the
gamestate's baselines against a simulation of the order). No effect or
other model is registered on the way: wrecks and effects come from
`_tankeffects_gmi` (VERIFIED (the instructions)).

### 10.3 Collmaps

- `SP_script_vehicle_collmap` (0x8df52) sets `eType` 14 and contents 0 and
  never links (INFERRED (decompile)).
- After the BSP pass, `G_SetupScrVehicles` (0x8ab36) pairs each vehicle in
  slot order. `VEH_SetupCollmap` (0x7d3e2) takes the first in-use collmap
  by number whose `targetname` is the vehicle's model name (case folded),
  puts its `*N` in the **vehicle's** `s.index`, runs `SetBrushModel` on the
  vehicle and sets the vehicle's contents to 0x800000 | 0x200000 (VERIFIED (the instructions); `coduo-diff.md` 9 had it on the collmap). One collmap serves
  every vehicle of its model (INFERRED, the scan). A vehicle placed after
  load pairs at once (INFERRED (decompile)).
- The capture's tanks carry `index` 27 and 28 and `solid` 0xffffff
  (VERIFIED, baselines).
- mp_cassino's two collmaps' brushes carry contents 0x28031640: player
  clip, the vehicle clip 0x200, no SOLID, no shot bit (VERIFIED, the BSP).
- So a player bumps a tank and a bullet passes its collmap; retail tests a
  brush model's own contents against the mask inside the clip (INFERRED).
  A vehicle's clipmask 0x201 meets the collmap brushes through 0x200, a
  player's 0x2810011 through 0x10000 (VERIFIED, the masks and the brush
  word).
- Each vehicle clips its own copy of the brushes where it stands, under its
  own `r.contents`: mp_kharkov's Horch glances off a parked Panzer IV that
  is not the last of its model to have moved (VERIFIED, the ram capture,
  section 12). vcod keeps a per-entity model instance for each vehicle
  (`CollisionWorld::set_model_instance`), names the vehicle on its hits and
  passes it on the vehicle's own traces. One shared posed brush model per
  kind lets only one tank of each kind block anything.

### 10.4 The first think and the park

`Scr_Vehicle_Init` (0x8c5ab), 50 ms after the spawn, settles a tank or a
four-wheel on its wheel traces: the BSP's z -8 parks at -7.875 (VERIFIED,
baselines). `time2` becomes `trunc(1000 * suspensionTravel)`, 6 bits on
the wire (24 for the Panzer IV, 8 for the Sherman) (INFERRED (decompile);
capture): the files' `suspensionTravel` 7 and 5 give 7000 and 5000, whose
low 6 bits are 24 and 8 (VERIFIED, the paks). The cgame reads it as the
wheels' reach, `time2 / 1000` units (`coduo-diff.md` 9). The vehicle table's angles are 16 bits, so the BSP's yaw 270
reads -90 (VERIFIED, baselines).

It does nothing else of the step: it settles, places the vehicle through
0x7c762 (`G_SetOrigin`, `G_SetAngle`, a relink only when `r.linked`),
copies the settled pose into the record's previous pose, runs
`G_DoTouchTriggers` and hands the think to `Scr_Vehicle_Think` 50 ms on
(VERIFIED (the instructions), 0x8c5ab..0x8c7c9). It leaves the committed copy at
`rec+0x170`, which `VEH_InitPhysics` (0x7cd00) took from the unsettled
spawn pose. So a vehicle that settled fails 0x827ab's "nothing moved" test
on its first think and runs the collision probes once (INFERRED (decompile);
sections 15.2 and 17.6 for what that does to a wreck's duplicate).

### 10.5 Freeing

`G_FreeEntity` runs `G_FreeVehicle` (0x8ad7d) first: B1..B4 join the free
list, then the vehicle (INFERRED (decompile)). `freeVehicle` turns a vehicle
into `eType` 13 `script_vehicle_corpse`; no stock MP script calls it
(VERIFIED grep). Section 17.4 has the measurement.

## 11. Seats on the server

### 11.1 Boarding

- The use press (`ClientThink_real` 0x40c91, inside the cmd) reaches
  `Cmd_Activate_f` (0x4dffb). A rider in a usable vehicle gets the
  vehicle's `use`, the exit; anyone else takes the cursor hint's entity
  and, for a vehicle `G_IsVehicleUsable` (0x8b673) passes, its `use`, the
  entry (INFERRED (decompile)).
- `G_IsVehicleUsable`: not seated, a free seat, the vehicle at most 100
  units/s and alive, contents 0x200000, no grenade in hand, every rider on
  the player's team (INFERRED (decompile)).
- `VEH_LinkPlayer` (0x8998d), in order (INFERRED (decompile)):
  1. The owner lock (`vehicle_owner` a client, seat 1 free, not the
     player: `e "GMI_GAME_VEHICLE_NOT_OWNER"`).
  2. Silent unless on the ground or falling slower than 80, and 250 ms
     past the last seat stamp.
  3. The stamp (`client+0x468c`: now on a tank's first entry, now + 1500
     else).
  4. The seat; `SetClientOrigin` at the seat tag (+1 z, the teleport bit);
     `SetClientViewAngle` at the view tag's forward (`tag_player`, seat 2
     `tag_secondary_gun`, else `tag_body`), roll 0; the link.
  5. The record's occupant and `animMovetype |= 1 << seat`; on seat 1 the
     vehicle driven, its `eFlags` 0x100000, `r.ownerNum` and
     `otherEntityNum` the driver.
  6. `ps.eFlags |= 0x300000`, `viewlocked_entNum`, sight flag off;
     reliable `v cl_run 1` and `v cl_stance 0`; `vehPos`, `vehType`.
  7. `activated (vehpos, player)` on the vehicle, `vehicle_activated
     (vehpos, vehicle)` on the player; `fTorsoPitch` and `fWaistPitch` the
     gunner's vertical spans.

### 11.2 Getting out

- `VEH_UnlinkPlayer` (0x89fb2). A voluntary exit (the use key) is refused
  on a live tank inside 1500 ms of the stamp while the seat has changed at
  most once (`e "GMI_GAME_VEHICLE_EXIT_DELAY"`), or with no dismount spot
  (`e "GMI_GAME_VEHICLE_UNLINK_BLOCKED"`). Then: the seat and its mask bit
  cleared; on seat 1 the vehicle's driver fields, and a tank coasts 3 s on
  its last forward input; the player's seat bits, `viewlocked_entNum`
  1023, `vehPos` 0 (`vehType` and `vehMotion` kept), the sight; `v cl_run
  1`, `v cl_stance 0`; a voluntary exit lands on the spot (+1 z) and inside
  the coast window pops up at 200; `deactivated (player)`,
  `vehicle_deactivated (vehicle)` (INFERRED (decompile); the exits of both
  seat captures, VERIFIED).
- `VEH_FindValidDismountSpot` (0x88dee) (INFERRED (decompile)):
  - Base point off the side away from the vehicle's motion, or toward the
    player at rest: `60 + maxsX + maxs.y` sideways or `maxs.x +
    max(collisionFront, collisionBack) + 100` along, 4 up, along
    `AngleVectors` of the record's whole angles (`rec+0xd0`: on a slope the
    spot tilts with the hull).
  - A 20-unit grid of offsets -80..60 about it (0xa6e08 80, 0xa6e1c 20),
    each point at two heights 32 apart (inner loop bound 2, 0xa6e18), four
    passes flipping forward then side. The first point is the base itself:
    each offset is applied after its point's two heights.
  - A spot passes when a capsule position test under 0x2000000 (no body)
    is clear, a capsule from it 256 down under 0x810011 hits without
    starting solid, and a box (the player's, `maxs.z - 30`) from the
    vehicle's origin 30 up to the spot 30 up, passing the player's
    `r.ownerNum` (the vehicle) under 0x810011, arrives or stops on the
    vehicle.
  - Trap: the yaw alone with three heights put mp_peaks' jeep exit 10
    above retail's; vcod lands on retail's spot (VERIFIED,
    `vehicle_ab`). The six vectors at 0xa6660 are the collision probes'
    (section 12), not the dismount's (VERIFIED, every reference;
    `coduo-diff.md` 9 had them here).
- The exit leaves `pm_type` as the seat had it. `VEH_UnlinkPlayer` calls
  `G_EntUnlink` and `SetClientOrigin` and writes no `pm_type` (VERIFIED (the instructions), 0x8a1ac, 0x8a463); `G_RunClient`'s unlinked arm steps 1 to
  0 and 7 to 6 on the client's entity-pass turn (VERIFIED (the instructions),
  0x41f39..0x41f62). The cmds left in the exit's frame run under `pm_type`
  1 and move nothing; the fall or the pop starts on the next frame's first
  cmd (INFERRED, that control flow). All three captured exits show it: the
  use edge's frame ends on the spot with the velocity unspent though one
  to four cmds ran after the press (jeep drive msg 482, ct 37342, velocity
  0; tank drive msg 483 and tank seats msg 685, velocity 200). vcod's
  pmove from that snapshot's `commandTime` on the next frame's cmds lands
  on the next snapshots to 0.005 (jeep 93.11678 vs 93.11677 at ct 37394,
  tank 30.149006 vs 30.149002 at ct 36991) (VERIFIED, capture and replay).
- `ClientEndFrame` (0x42f59) writes `pm_type` again off the link alone: 1
  linked, 0 not, 7 and 6 for a dead client (VERIFIED (the instructions)). A rider
  the vehicle think puts out after `G_RunClient`'s turn is unlinked by his
  end frame and runs the next frame's cmds free (VERIFIED, 11.7's
  capture). vcod holds a left seat's cmds linked until
  `ClientSim::run_client` (after the entity pass, before the vehicle
  thinks) or the client's end frame, whichever comes first. Holding them
  to the next frame's `run_client` ran a corpse the think put out a frame
  linked.
- Forced exits (no refusal, no search, the body left at the seat): a seat
  change, the vehicle think's pass over dead riders, `ClientSpawn`,
  `ClientDisconnect`, a team change (VERIFIED (the instructions)).
- `Scr_Vehicle_Die` (0x8dccb) probes each rider with a 5-point
  `MOD_EXPLOSIVE` hit (health padded by 10), dismounts one it did not
  hurt and puts the rest out in place (VERIFIED (the instructions)). Details below.

#### The death's riders: `Scr_Vehicle_Die`

`G_Damage` (0x4fedb) runs the die callback after the `damage` and `death`
notifies, as `(self, inflictor, attacker, damage, mod, inflictor's
s.weapon, &dir, hitloc)`, a NULL inflictor or attacker already made the
world (INFERRED (decompile)). `Scr_Vehicle_Die` (0x8dccb):

- For seats 1 to 6 in order, each seat whose entity is a client: `health`
  (+0x240) goes up 10, then `G_Damage(rider, attacker, attacker,
  vec3_origin, ps.origin, 5, 0, MOD_EXPLOSIVE (0x17), 0)`. The rider is
  shielded when his health still reads the padded value. His health then
  goes back to what it was, whatever the hit did, and `VEH_UnlinkPlayer`
  (0x89fb2) runs with `voluntary` = shielded (INFERRED (decompile)).
- After the unlink `G_SetOrigin` puts `r.currentOrigin` 32 (f32 at
  0xa7624) above where the unlink left it (INFERRED (decompile)). The f32 is VERIFIED bytes.
  The playerstate keeps the old origin, so only reads of `r.currentOrigin`
  before the rider's end frame see the lift: the `origin` field and a
  `radiusDamage` measure do, a `doDamage`'s direction does not (VERIFIED
  capture below).
- `G_DamageClient` (0x4fd4b) decides the probe like any hit. A tank's
  driver is shielded from every attacker but his own tank (13, the
  invulnerability), so the stuck kill and a tank that is its own attacker
  put him out where he sits. A damage callback that never calls
  `finishPlayerDamage` (stock tdm's friendly fire 0) shields its rider too
  (VERIFIED capture below).
- A shielded rider's unlink searches a dismount spot as a use exit does
  (the exit delay is skipped: the vehicle's health is under 1); a blocked
  search refuses and leaves him seated (INFERRED (decompile)). A dismount
  sets the origin through `SetClientOrigin` and pops a tank's driver up at
  200 while the tank coasts (11.2).
- Last, an attacker whose `s.weapon` is a grenade (type 1) or projectile
  (2) jolts the hull along the hit's direction, scale 1 (0x8d492)
  (INFERRED (decompile)).
- `PlayerCmd_isInVehicle` (0x46ca0) reads the entity state's `eFlags`
  0x100000, not the playerstate's: a rider put out where he sat reads 1
  until his end frame; one a dismount moved reads 0 at once (VERIFIED (the instructions) and capture below).
- UO's `radiusDamage` (0x6c229) takes an attacker and an inflictor after
  its four arguments, by count: attacker the world when absent, inflictor
  NULL (INFERRED (decompile)). `_tankdrive_gmi`'s death blast passes both,
  so its victims' callbacks name the tank's last attacker and its weapon.
  The damage callback's weapon is the inflictor's `s.weapon`, else the
  attacker's (`G_DamageClient`, INFERRED (decompile)).
- Stock tdm's `Callback_PlayerKilled` reads `level.teamkill_penalty`,
  which nothing sets, for a death to something not a player. A tank that
  is its own attacker has `tank_death_player_damage` kill its driver that
  way, and on `run_probe.sh`'s retail server that is a fatal script error
  (VERIFIED capture, the probe's first run). Below `developer 1` the `if`
  takes its branch and the victim's team loses a point
  (cod11-gsc-language.md, "Runtime errors below developer 1").

Measured on retail UO 1.51b, 2026-10-17, `client-probes/probe_vehdie`
(fixtures `crates/server/tests/fixtures/vehicles/*-die-*-uo.txt`, recipes
in their headers). A `--probe-vehicle driver` and a `--probe-vehicle
gunner` client board, and a frame after the second seat the gsc kills the
vehicle with `doDamage(10000, ..., by, by, "MOD_EXPLOSIVE")` (VERIFIED,
capture):

| Case | Result |
|---|---|
| Panzer IV, its own attacker | Both riders' callbacks run with 5, flags 0, the tank as inflictor and attacker, `panzeriv_turret_mp`, their own origins as the point, zero direction, `none`, health 110. Both read health 100 after, `isInVehicle` 1, origin 32 up; the gunner's `vehicle_deactivated` waiter runs before the driver's. |
| Panzer IV killed by its gunner | The driver's callback never runs; he reads `isInVehicle` 0 at a spot 227 units ahead of the hull's origin, 32 up. The gunner's runs (his own `kar98k_mp`) and he stays put. |
| Willys, driver alone, killed by a teammate on foot, friendly fire 0 | The callback runs, tdm returns, health stays 110, the driver is dismounted. |

vcod walks the riders in `vehicle::die`. Inside the builtin that killed
the vehicle each rider's callback is a `spawn_then` (`DIE_TOKEN`); a death
outside a builtin (a round, a blast, a collision) is walked at
`ScriptRuntime::vehicle_deaths`, each callback run to its first `wait`. A
rider put out in place is held in `Vehicles::stale_seated`, and the lifted
origin's unlifted one in `Vehicles::lifted`, until his origin is next
mirrored. The seats the frame's threads left are applied ahead of the
script's sim ops, so a dismount's pop sets the velocity and a hit after it
adds its knockback. `vehicle_die_ab` replays the three captures: every
line from the kill on matches retail's word for word, the death blast's
victims and their damage, the gunner's mid-turn body, the dismounted
driver's first fall frame and the corpses leaving the hull included.
VERIFIED, the gate on the 2026-11-14 recaptures, whose fixtures carry each
probe client's `CMDS` lines: ours joins both clients on the capture's
clock (client 0 as late as its `PROBE place`, client 1 as much later as
its own) and each sends the cmds and use taps the lines logged. The 4, 2.2
and 6.5 units the gate allowed before were the cmd clock: the 2026-10-17
captures logged none, and ours sent 16, 16 and 18 ms a frame and tapped on
its own clock. INFERRED, from the gate going exact once the cmds are
retail's.

### 11.3 Changing seats

`Cmd_NextVehSlot_f` (0x4ed9b) and `Cmd_PrevVehSlot_f` (0x4ede1), for a
living rider, run `G_VEH_CycleSlot` (0x8da1e) (VERIFIED (the instructions)):

- nothing when no seat is free;
- a tank's first change after a use entry is never refused;
- inside 250 ms of the stamp, `e "GMI_GAME_VEHICLE_DELAY_IGNORE"` at most
  every 500 ms;
- else a forced exit and an entry at the next free seat (wrapping 6 to 1),
  stamped 1500 ms ahead: changes are 1750 ms apart and the script sees
  `deactivated` then `activated`.

The cycle is a client command, so it runs where `SV_ExecuteClientMessage`
runs its packet's commands: after every earlier packet's usercmds, before
its own packet's. `SetClientViewAngle` (0x44301) writes `delta_angles` as
`ANGLE2SHORT(view) - client+0x452c`, the angles of the last cmd run
(INFERRED (decompile)), so where the command lands among the frame's cmds
moves the seat's `delta_angles` and every view after it. Both seat
captures fit a change after the last cmd that still turns and before the
held run the probe's `say` step starts: the tank's `delta_angles` (23003,
63172) at the driver's seat are `tag_player`'s view (8.88, -27.60) less
cmd 41000's (44149, 62875); the jeep's (13195, 41447) at the passenger's
`tag_body` (-11.21, 76.53) less cmd 39700's, to a short (VERIFIED,
capture). vcod-server queues the cycle with its packet and runs it in the
move pass ahead of that packet's cmds, as it runs `kill`
(`client::PacketCommand`). Run on arrival, ahead of earlier packets' cmds,
the seat's view stood up to 0.84° off retail's on the change's snapshot.

### 11.4 Every frame

- `G_RunClient` (0x41dfc) puts a linked player in `pm_type` 1, 7 dead
  (INFERRED (decompile)).
- `ClientEndFrame` (0x42f59):
  - `vehMotion` 1 under 20 units/s of the hull's forward speed, else 2
    forward, 3 back; the entity's `surfType` is `vehPos | vehType << 3 |
    vehMotion << 6`, its `otherEntityNum` the vehicle (VERIFIED (the instructions)).
  - `G_PlayerVehiclePositionAndBlend` (0x9048f) puts `ps.origin` at the
    seat tag plus `BG_GetVehiclePosOffset` in the tag's axes (VERIFIED (the instructions)).
  - Then the plain `BG_PlayerStateToEntityState` (0x20a35) with the snap
    on: `pos.trType` and `apos.trType` 1, `pos.trBase` the origin
    truncated (`fldcw` with `| 0xc00` at 0x20ab0), `apos.trBase` the view,
    over the extrapolated write's `trTime`, `trDuration` and `trDelta`
    (VERIFIED (the instructions)). A seated player's entity in
    `snapshots-uo-jeep.bin`, `-uo-tank.bin` and `-uo-overheat.bin`
    carries `pos.trType` 1, whole-unit bases, a 50 ms duration and a stale
    delta (VERIFIED, capture); vcod-server sends the type and the
    truncated base.
- The tag comes off the vehicle's DObj as the frame's first tag query
  posed it, with the controls from before the frame's step. A rider placed
  with this frame's controls misses retail's origin by up to 3 units in a
  turn; with the last frame's, by none (VERIFIED, replay, section 14).
- The engine never sends a tank's driver to anyone (`coduo_lnxded`
  0x8096b97, test at 0x8096d7a: a player in a vehicle whose `surfType` is
  2 and whose own `surfType & 7` is 1) (INFERRED (decompile)).
- The skip's operands, read off the disassembly (2026-11-16): `eType` 1
  (`es+0x4`, 0x8096d6e), `eFlags & 0x100000` (`es+0x8`, 0x8096d7a), the
  entity `r.ownerNum` names (`+0x154`, 0x8096d86, the vehicle) with
  `surfType` (`es+0x88`) exactly 2 (0x8096da0), and his own `surfType & 7`
  1 (0x8096db3). VERIFIED (the compared offsets and constants). A match
  jumps to the loop's next entity (0x8096db8 to 0x8096f1f). INFERRED
  (control flow).
- The skip comes after the builder's other per-entity tests: `svFlags` 1,
  the single-client bits 0x800 and 0x2000, the viewer's own number, then
  `svFlags & 0x18` and a live `r.broadcastTime` (0x8096ce3..0x8096d4b),
  which add the entity with no cull. So a driver a `playSound` or
  `playLoopSound` stamped goes to everyone while the stamp lasts. INFERRED
  (control flow). vcod drops the driver from the frame's culled set after
  the broadcast split (`build_world_frame`);
  `a_tank_driver_is_sent_only_while_broadcast` pins the order. Before
  2026-11-16 it dropped him ahead of the broadcast test.
- A rider's legs come off the animscript.
  `BG_AnimUpdatePlayerStateConditions` (0x1c973) sets `mounted` to the
  seat (`vehDriver`, `vehGunner`, `vehPassenger1`..), `vehicle` to the kind
  (`tank`, `jeep`, `flak88`) and `vehicle_motion` (`idle`, `forward`,
  `reversing`) (INFERRED (decompile)), and `playeranim.script`'s clauses pick
  them; the selection runs for a rider though he is off the ground
  (INFERRED; the replay's legs match).
- Slot 4 takes `ps.vehMotion` (`ps+0x61c`) as it stands (VERIFIED (the instructions), 0x1c973); its value table opens on `** UNUSED **`: `.data`
  0xa92a0 holds `** UNUSED **`, `IDLE`, `FORWARD`, `REVERSING`, each with a
  -1 (VERIFIED, file bytes and `.rel.dyn`). `VEH_LinkPlayer` writes
  `vehPos` and `vehType` but not `vehMotion` (11.1), the exit keeps it
  (11.2) and the spawn's memset zeroes it (11.7). So a boarding after a
  spawn runs its cmds before the end frame under motion 0, which no
  `vehicle_motion` clause names (INFERRED, off the three). The seat's legs
  then wait for the end frame's `vehMotion` and the next frame's cmds: the
  jeep driver boarded by a use cmd at 29716, with a seated cmd at 29733 in
  the same frame, is sent with the on-foot idle (697) at 29750 and the
  seat's (94) at 29800, and his bones begin the glide down a frame later
  than vcod's once did (VERIFIED, capture 2026-10-21, the first cut of
  `mp_peaks-jeep-glide-uo.txt`, since recaptured, 13). vcod: the boarding
  keeps the sim's `veh_motion`, and `seat_conditions` names no motion for
  0.

### 11.5 A spawn out of a seat

`ClientSpawn` (0x44df4) tests `ps.pm_flags & 0x80000` and `ps.eFlags &
0x6000` and calls `G_ClientStopUsingTurret` (0x44e6a), then tests
`pm_flags & 0x80000` and `eFlags & 0x100000` and calls
`VEH_UnlinkPlayer(ent, 0)` (0x44e9e), both ahead of `G_EntUnlink`
(0x44ea9) (VERIFIED (the instructions)). The think on the client's last cmd
(0x45409, cod15-movement.md 4) comes after the playerstate is rebuilt, so
a rider spawned out of his seat moves from the spawn spot on that cmd,
with nothing of the seat in it (INFERRED, call order).

VERIFIED, retail capture
(`crates/server/tests/fixtures/seatspawn/mp_cassino-tank-uo.txt`,
`client-probes/probe_vehicle` with `probe_respawn 3`): a panzeriv driver
holding forward, spawned 3 s after his seat at (4676, -2179, 40.12) facing
0, reads `(4677.49, -2179.00, 36.12) (0.00, 0.00, 0.00)` as `self.origin`
and `self.angles` straight after the `spawn`: 1.49 along the spawn's yaw
and the 100 ms fall. vcod's `spawn` builtin runs the same think on a fresh
sim, which holds no seat, and reads the same text (`vehicle_ab`'s
`a_driver_spawned_out_of_the_seat_reads_retail_s_origin`).

A frame later the script's read (`PROBE seatnext`) is wherever the
client's next cmds took him: 33.15 in that run, 31.96 in a second with an
8 ms cmd clock (`fixtures/seatspawn/mp_cassino-tank-cmds-uo.txt`, its cmds
and snapshots in `playerstate/mp_cassino-tdm-slope-8ms-tank-respawn-uo.txt`),
and yaw -90, the view the probe kept across the spawn's new
`delta_angles` (VERIFIED, capture). The spawn frame's `commandTime` is
18550, the frame's own time, not a cmd's. The probe's cmd stamped 18550
sits on the spawn's `delta_angles` (13653 against the seat's 62805) and
its weapon echoes the spawn's, so it went out after that snapshot and ran
in the next frame, where `ClientThink_real` drops it (its clock is not
past `commandTime`) and it stays the next cmd's `pers.oldcmd`
(cod11-combat.md 2.1) (VERIFIED, capture). Replayed that way,
`a_driver_spawned_out_of_the_seat_moves_on_retail_s_cmds` reads retail's
`seatnext` text and matches every later snapshot on foot (VERIFIED,
replay). Trap: running the cmds up to a snapshot's `commandTime` before
its frame sends that cmd ahead of the spawn; its angles then base the
spawn's `delta_angles` and the yaw reads 0.

### 11.6 A body inside a vehicle

UO's `ClientThink_real` (game.mp.uo 0x40c91) gets a client on foot out of
a vehicle before his `Pmove` (VERIFIED (the instructions), 0x415e4..0x419c3, and
decompile):

1. Not seated (`ps.eFlags & 0x100000` clear): a `trap_TraceCapsule`
   position test at `ps.origin` with the client's box (`maxs.z` 30 under
   `pm_flags` 1 prone, 50 under 2 crouched), passing his own number, under
   the cmd's mask: 0x2810011 alive, 0x810011 at `pm_type` 6 and up.
2. When it starts or is all solid and names an `eType` 12 entity: `dir` is
   `ps.origin - r.currentOrigin` of the vehicle, `VectorNormalize`d whole,
   and `along` the velocity's `VectorNormalize2`, which returns the speed;
   both then lose their z without renormalizing.
3. Speed under 1.0 (rodata 0x9de18), or `dir . along` past 0.8 (0x9de24):
   the vehicle is `trap_UnlinkEntity`ed and `trap_LinkEntity`ed back after
   the `Pmove` (0x41a97). Otherwise nothing happens.
4. Then, not prone and speed under 1.0: a vehicle whose record velocity
   (`rec+0xe8`) is past 1.0 runs `VEH_PlayerCollision(vehicle, player)`,
   and a velocity under 200 (0x9de28) is scaled up to 200; a vehicle at
   rest sets the velocity to `200 dir`. Either way `pm_time` becomes 200
   and `pm_flags` takes 0x200 (the damage knockback's timer).
5. Prone, inside the vehicle (unlinked or not), and `client+0x472c` more
   than 500 ms old: `VEH_PlayerDamage(player, vehicle, 10)`, and a second
   `VEH_PlayerDamage(player, player, 10)` when the first left his health
   alone (INFERRED (decompile)). `client+0x472c` is read there alone
   (0x41964) and written nowhere in the module (VERIFIED (the instructions),
   every reference to the displacement). Unlike `VEH_PlayerCollision`'s
   `+0x4728` it holds nothing back, so past the level's first 500 ms the
   prone body takes the 10 on every cmd (VERIFIED, capture below).

`VEH_PlayerDamage` (0x81ffd) of a vehicle is `G_Damage` with the vehicle
as inflictor and its `r.ownerNum` as attacker (the vehicle itself at
1023), no direction, the body's `r.currentOrigin` as the point, dflags 0,
`MOD_CRUSH_JEEP` (18) where the file's type (`info+0x40`) is 1, else
`MOD_CRUSH_TANK` (17). `VEH_LinkPlayer` (0x8998d) sets a vehicle's
`r.ownerNum` to its driver, `VEH_UnlinkPlayer` back to 1023. Of anything
else (the player himself) it is `G_Damage` with him as inflictor and
attacker under `MOD_SUICIDE` (21) (INFERRED (decompile)).

A rider put out where he sat (11.2) is that body. His box starts in the
hull's collmap, so a cmd with velocity left meets the linked hull, is held
all solid and ends with its velocity zeroed by the pmove's tail; the next
starts under 1.0, unlinks the hull and pushes him off at 200 from its
origin. The corpse leaves a cmd at a time, as the three
`client-probes/probe_vehdie` captures show: Panzer IV killed by itself,
the driver's corpse 0.65 along x at the kill's next frame and 8.7 half a
second on, the gunner's 20 units out (VERIFIED, the 2026-10-17 capture);
the 2026-11-14 recapture now in `mp_cassino-tank-die-self-uo.txt` reads
0.37 and 8.33, the gunner 20.8 out (VERIFIED, capture).

vcod runs it as `ClientSim::vehicle_overlap` before each UO cmd's pmove,
the collmap instance taken out of the collision world for the cmd
(`CollisionWorld::unlink_model_instance`), and deals the run-over and
prone damage ahead of the cmd's pmove (`Server::vehicle_overlap_hits`), so
a kill there runs the pmove on the corpse. `vehicle_die_ab` sends the
probe clients' own cmds (11.2) and puts both corpses on retail's numbers at
both lines. VERIFIED, the gate.

**Run-over, standing and prone.** Retail UO 1.51b, 2026-10-17,
`client-probes/probe_vehicle_ram` with `probe_victim` (fixtures
`crates/server/tests/fixtures/vehicles/mp_kharkov-jeep-runover-*-uo.txt`,
recipes in their headers): mp_kharkov's second Horch, driven straight
ahead by `--probe-vehicle ram`, meets an allied victim standing or prone
150 units ahead.

- Standing, he is struck twice. First 52 points 350 ms past the jeep's
  first move: `MOD_CRUSH_JEEP`, the Horch the inflictor, its driver the
  attacker, weapon `none`, the victim's origin as the point, no direction;
  he is thrown 11 up and forward, and `vehicle_hit_person` (alias 8) plays
  at his origin 972 up. The jeep catches him in the air 650 ms later for
  138, which kills him, the sound 115 up (VERIFIED, capture). Both come
  from this path, not the drive step's probes: the amounts are `150
  min(speed / 400, 1)` of the hull's speed after the frame before (140.5
  gives 52, 369.2 gives 138; the heights `1500 (1 - f)`), and the
  callbacks read the last frame's time, as a cmd's do (INFERRED, the
  amounts and times; vcod deals them on this path and matches every one).
- Prone, the hull runs over him without a push or a sound: 10 on each of
  his cmds, three a frame (100 to 20), then a 72 at 192 units/s, which
  kills him (VERIFIED, capture). The 72 reads the frame's own time: the
  drive step's strike (INFERRED, the time).
- Prone under a teammate's jeep with `scr_friendlyfire 0`, every cmd's
  crush is refused and the second call follows: `MOD_SUICIDE`, the victim
  as inflictor and attacker, his own `mosin_nagant_mp`. Past his death the
  two keep coming until his end frame, health reading 0, -10, -20
  (VERIFIED, capture). They keep coming because `takedamage` stays set
  until his end frame (INFERRED, combat doc 14.7's rule).

`vehicle_runover_ab` replays the three on ours. Every damage callback and
hit sound matches retail's in time, amount, means, attacker, inflictor,
weapon and health. The victim's origin matches to the hundredth up to
150 ms past the first strike, and after it within a probe client cmd's
travel (INFERRED: the gsc reads it after however many of the probe's cmds
a frame ran; retail's flight steps 14, 3.8 and 17.6 units over three
frames, ours 10.6 each); the standing row has since moved to a 2026-11-14
recapture that replays both clients' cmds and matches exactly ("The first
drop" below). Where a prone victim lies down is the probe
client's timing too: retail's two prone runs lie 1.1 units apart. The same
captures carry the Horch's own ram sounds as it strikes the Panzer past
the victim, `EV_SOUND_ALIAS` parm 7 (`vehicle_impact`) at z 1825 and 2409
over the struck hull, 1300 and 2100 ms after the jeep's first move in the
standing run (VERIFIED, capture); the gate matches each one's time from
that move, alias and snapped origin (`compare_impacts`).

- **The first drop.** The probe sets the victim down 8 over the jeep's
  line on the first frame he plays; he falls about 14 units to rest before
  the second placement a second later (VERIFIED, `try_victim` in
  `probe_vehicle_ram.gsc` and the captures). The four retail runs fall
  differently from the same spot (VERIFIED, captures; z at the first four
  frames past the placement):

  | Run | z |
  |---|---|
  | stand | -76.13, -77.47, -82.77, -90.05 |
  | prone | -76.11, -78.40, -82.69, -88.98 |
  | prone under a teammate | -75.81, -79.64, -81.12, -89.06 |
  | bail | -76.29, -78.78, -83.23, -89.77 |

  The drops read as a fall from rest after a whole number of the probe
  client's ~16.7 ms cmds by the first three frames: two, four, eight
  standing; one, six, seven under the teammate; two, five, eight prone
  (INFERRED, `400 t^2` against each drop). In the bail run he also slides
  13.9 units/s across the ground from the first frame. Nothing in the gsc
  moves him sideways, so that is velocity his spawn left: a frame of his
  cmds ran between the spawn and the placement (INFERRED,
  `replace_victim`'s comment; the spawn point is not logged). Ours runs
  the probe's 16, 16 and 18 ms cmds with no velocity into the placement,
  so the first drop reads two, five and eight cmds from rest on every run
  (VERIFIED, `RUNOVER_DUMP` of all four tests): retail's prone run to the
  hundredth, rest spot included (VERIFIED, the gate). Joining him later
  moves none of it (VERIFIED, measured on ours): placed at retail's 7850,
  7900 and 7950 ms instead of 2900, the rows are the same lines at the
  shifted times. `vehicle_runover_ab` compares the prone run's first drop
  line for line (`compare_first_drop`) and names the other three in
  `FIRST_DROP_GAPS`. Closing them needs a recapture with the victim's
  `CMDS` lines replayed through `ProbeHold::times`, plus his spawn pinned.

  VERIFIED, the 2026-11-14 recaptures, whose fixtures append each probe
  client's `SPAWNED` and `CMDS` lines: the victim's first spawned snapshot
  already stands on the placement (`SPAWNED t` is `PROBE victim`'s time,
  `ct` the same), so the spawn and the placement share a frame, and its
  velocity is what the cmds run at the spawn point left (0 on some runs;
  (-10, 27, -12), (12, -10, -3) and (13, -1, -2) on three others). The
  spawn point itself stays unlogged. `vehicle_runover_ab` now runs ours
  alone until the victim's placement lands on the capture's time, sends
  his cmds on the `CMDS` times with the buttons, prone key and up move of
  their `in=` changes, and gives his placement the `SPAWNED` velocity
  (`Server::test_set_client_velocity`). The first drop then matches
  retail's to the hundredth on every standing and prone-teammate run
  tried, the spawn's velocity included. VERIFIED, the gate. The fixture
  `mp_kharkov-jeep-runover-prone-team-uo.txt` was such a run, its gap
  gone; a 2026-11-28 recapture under gdb has since replaced it (11.6.1).

  The driver's `CMDS` lines replay too (2026-11-15): his cmds on their
  times, his use taps on the cmds that carried them (`in=` 64), and the
  throttle where `vehicle_probe.rs` holds it, for 6 s (`RAM`) or to the
  exit tap's end (`BAIL`), on the cmds the probe's script clock puts there
  (11.6.1, since 2026-11-28; it was 2000 ms of `serverTime` past the first
  tap's end). The jeep then moves on retail's frame: its first move had
  landed 50 ms early on the gate's own clock. With it, and the corpse rule
  below, the 2026-11-14 recaptures that log the victim's spawn velocity
  match retail line for line to the hundredth, first drop, flight, both
  strikes, corpse and ram sounds included: standing (runs 6 and 7) and
  driverless standing (runs 6 and 7, and the first run, whose velocity
  was 0) (VERIFIED, the gate on each). Standing run 4, which logs no
  spawn velocity, matches to its second strike and takes that one a frame
  early on ours (125 at +900 against retail's 131 at +950); not run down.
  `mp_kharkov-jeep-runover-stand-uo.txt` and
  `mp_kharkov-jeep-runover-bail-uo.txt` are now runs 6, compared with no
  drift, and both `FIRST_DROP_GAPS` entries are gone. Still open:

  - prone: the fall matches, and a prone body landing on the grade slides
    0.1 to 0.3 units off retail's (two runs), so the 2026-10-17 capture,
    whose drop ours meets, stayed. VERIFIED, the gate on those runs. The
    2026-11-28 recapture that replaced it (11.6.1) lands on retail's
    numbers to the hundredth (VERIFIED, the gate); the two runs' slide
    stays unread.
  - a victim who lies down on his placement 300 ahead (the driverless
    prone runs below) lands 300 ms later on the grade, and retail slides
    him 0.35 units on along y over two frames where ours stops: every one
    of the six runs, (-1032.09, 7763.64) then (-1032.00, 7763.87) on run 6
    against ours' (-1032.24, 7763.31). Retail's second drop onto the same
    spot, already prone, lands on ours' point (VERIFIED, captures). So
    the slide belongs to the prone press's transition on landing, which
    ours lacks (INFERRED). `FIRST_DROP_GAPS` named `bail-prone`.

  Closed 2026-11-26: retail never lies that victim down in the air. His
  placement's lie-down test passes the stale ground, and its chest sample
  lands 27.4 under the origin plus 6, past UO's bound of 24
  (`BG_CheckProneValid` 0x22841..0x22866, `fabs` against the double at
  0x9a9e0, then the shared miss at 0x22f9b, which refuses an on-ground
  caller; coduo-movement.md 17) (VERIFIED (the instructions)). Every later
  cmd of the fall is airborne and drops the press, so he lands standing,
  his clipped velocity kept with no prone timer running, slides on down
  the grade and lies down on the cmd after the landing (INFERRED, off the
  replay). Ours, with the bound ported, lands and slides on retail's
  numbers to the hundredth, (-1032.09, 7763.64, -103.04) then (-1032.00,
  7763.87, -103.10), and the gate compares every first drop exactly (VERIFIED).
  The 150-ahead prone runs fall about 14 units, inside the bound, and lie
  down on the placement as before.

**The corpse's creep.** Two rows held the prone corpse's creep to a
drift until 2026-11-28: `prone` (0.1, the 2026-10-17 capture, no `CMDS`) and `prone-team`
(0.15; ours 0.13 at worst with both clients' cmds replayed on the
capture's `ct=` frames). In the prone-team capture the Horch's origin is
135 units on by the first frame that differs (`PROBE a 14800`), and the
corpse moves 0.14 along y on the frame at +900 (14800), which ran two of
his cmds (`ct=` 14750:14750 to 14800:14782), and not at all at +1150
(15050), which ran three (15000:14983 to 15050:15033) (VERIFIED,
capture). Ours moves 0.04 on the first and 0.06 on the second, and lands
retail's +900 point at +1050 (VERIFIED, `RUNOVER_REPORT`). So retail's
creep is not a fixed share per cmd run (INFERRED, those two frames).
What moves it and what holds it is read in 11.6.1; both rows now match
retail's to the hundredth.

**A strike that kills.** `VEH_PlayerCollision` runs inside the drive
step's probe loop (0x827ab), on each of the six `vehCheckSolidOffsets`
probes whose trace meets a body, each probe passing on through it under a
mask without `CONTENTS_BODY` (0x2000000) (VERIFIED (the instructions)). So one
think can push a body up to six times. `player_die` (0x4f972) sets
`r.contents` (+0x120) to `CONTENTS_CORPSE` 0x4000000 (VERIFIED (the instructions)). A strike that kills him therefore leaves a corpse outside
the later probes' mask, which takes that strike's push alone (INFERRED,
the mask against the contents). vcod's think queues its strikes and
`server::crush` applies them after the driven steps; it skips a body a
strike before it killed. Without that, the driverless jeep's three
strikes in its killing think left the prone corpse at 392 units/s
along, past the 0.8 dot that unlinks the hull, and it flew 26 units a
frame where retail's creeps 0.2; the driven standing run's second strike
threw its corpse past retail's (VERIFIED, the gate with and without).

**Run-over of a prone body by a driverless jeep.** Retail UO 1.51b,
2026-11-14, `probe_victim prone`, `probe_victim_ahead 300`, the driver
on `--probe-vehicle bail` (fixture
`crates/server/tests/fixtures/vehicles/mp_kharkov-jeep-runover-bail-prone-uo.txt`,
run 6 of six, recipe in its header): 5200 ms after the driver's
placement the rolling Horch's hull covers the prone victim, and each of
his cmds takes 10, `MOD_CRUSH_JEEP`, the Horch (182) both inflictor and
attacker, weapon `none`, his origin as the point, three, two and four a
frame by the cmds that frame ran. 150 ms on, the drive step's strike
deals 110 (`150 min(speed / 400, 1)` at 293 units/s) on the frame's time
and kills him; in run 7 his tenth 10 killed him first. No
`vehicle_hit_person` plays, as he lies prone. The corpse creeps 0.2
units a frame (VERIFIED, six captures). `vehicle_runover_ab`'s
`a_driverless_jeep_runs_over_a_prone_player` matches every line to the
hundredth (VERIFIED, the gate).

**Run-over by a driverless jeep.** Retail UO 1.51b, 2026-10-20, the same
probe with `--probe-vehicle bail` for the driver (out at full throttle a
second into the drive) and the victim 300 ahead (fixture
`crates/server/tests/fixtures/vehicles/mp_kharkov-jeep-runover-bail-uo.txt`,
recipe in its header): the driverless Horch rolls on at about 320 units/s
and strikes the standing victim once.

- 121 points, `MOD_CRUSH_JEEP`, the Horch (182) both inflictor and
  attacker, weapon `none`, no direction, the victim's unsnapped origin as
  the point, the callback's time the last frame's (VERIFIED, capture).
  The caller is `VEH_PlayerDamage` with `r.ownerNum` 1023 after the
  driver's exit (11.2), on the cmd's path (INFERRED, the time).
- The callback reads the victim's `origin` equal to the point, the origin
  his cmd started from: the kill lands before that cmd's `Pmove`
  (VERIFIED, capture: the probe's `dmgorigin` line). Trap: reading the
  origin after the pmove gave (-1025, 7768, -97) against retail's
  (-1032.02, 7763.81, -103.08).
- The corpse leaves on the knock, 418 units/s by the frames after. Where
  the death frame's gsc reads it depends on how many of the victim's cmds
  that frame ran past the kill: four retail runs put it 7.5, 14.2, 20.9
  and 27.7 units out (a fifth was lost to the crash below), vcod's three
  cmds a frame 20.9 (INFERRED, the steps are whole cmds of that speed).
  Where the victim lands from his placement is the probe client's timing
  too: four runs settle within 0.03 of (-1032.02, 7763.81, -103.08), where
  vcod settles, and one 0.7 away (VERIFIED, capture).
- Stock UO `tdm.gsc`'s `Callback_PlayerKilled` reads
  `level.teamkill_penalty`, which nothing sets, for a kill whose attacker
  is not a player (line 815): a script runtime error, fatal on a
  `developer 1` server (VERIFIED, the first capture run) and run past
  below it (cod11-gsc-language.md, "Runtime errors below developer 1").
  The probe sets it to 0.

`vehicle_runover_ab`'s `a_driverless_jeep_runs_over_a_standing_player`
replays it on the 2026-11-14 recapture run 6, which logs both clients'
cmds: every line matches retail's to the hundredth, the corpse's flight
included, once ours runs the victim's cmds on their times (the whole
cmds a frame above). VERIFIED, the gate.

### 11.6.1 What moves a dead body near a vehicle

Read under gdb on retail UO 1.51b, 2026-11-28 (`tools/re/gdb_corpse.py`, recipe
in the headers of `mp_kharkov-jeep-runover-prone{,-team}-uo.txt`): every cmd of
the prone victim's corpse, the overlap test's trace, what `ClientThink_real`
unlinked, and the ps at eight points of `PmoveSingle`'s default arm.

- The creep is the corpse's own dead move: every cmd that is not held moves
  it 0.009 along -x and 0.018 along +y on the grade under it, 0.012 and 0.023
  on an 18 ms cmd, and leaves the velocity snapped to (-1, 1, 0) (VERIFIED,
  the gdb log). That is the ladder check's drop and the air move's slide
  (cod11-player-clip.md 8.11) (INFERRED, the shape).
- A cmd is held when the hull is linked across his box: the origin stays,
  `pm_flags` takes 0x8000 and the velocity snaps to 0 (VERIFIED, the gdb
  log). In the held cmds the first ground trace of the default arm (the call
  at 0x2dfd4; it runs again at 0x2e044 after the move) leaves
  `groundEntityNum` (`ps+0x58`) at 1023, and `PM_UpdatePronePitch` (0x2c158)
  then refuses the airborne body and sets 0x8000 (0x2c31d); the move after
  it leaves the origin where it was (VERIFIED, a second team run's gdb log
  at 0x2dfe8, 0x2dfed and 0x2e044). INFERRED: the dead move's box starts
  all solid in the hull.
- So under the hull the creep alternates: a cmd with the snapped (-1, 1, 0)
  left is past 1.0 and, while the flat dot to the hull's origin is under
  0.8, keeps the hull linked (11.6 step 3) and is held; the next starts at 0,
  unlinks the hull and creeps. Once the hull has passed to his side the dot
  goes past 0.8 and every cmd creeps (VERIFIED, the gdb log: the frame at
  17200 of the team run unlinks 182 on all three cmds).
- The dead move's box is the standing one. `PM_CheckDuck`'s `pm_type > 5`
  arm takes `pm.maxs[2]` from `ps.maxs` (UO `ps+0x570` at 0x2807e-0x28084,
  1.1d `ps+0x338` at 0x317f6, 1.5b `ps+0x338` at 0x2352c;
  cod11-gsc-object-model.md 22.3) (VERIFIED (the instructions)), and UO's
  `pmove_t` box reads (-15, -15, 0)..(15, 15, 70) at `pm+0xd8` on every cmd
  of the prone corpse (VERIFIED, a third team run's gdb log). The overlap
  test ahead of it takes the stance's 30 instead (11.6 step 1). So the hull can
  hold the corpse after the test has stopped finding it: three cmds of the
  team run (17450 to 17482) miss the hull in the test, trace `entityNum`
  1023, unlink nothing, and are held all the same (VERIFIED, the gdb log).
  vcod sized the dead move by the stance, 30 for a prone corpse, and crept
  through those three (VERIFIED, ours' per-cmd log against the gdb log).

vcod's `dead_move` now sizes its box by `PlayerState::corpse`, the standing 70
(`PlayerState::maxs`), on all three games. With it the corpse's origin on ours
matches the gdb log on every cmd of the team run, and `vehicle_runover_ab`'s
prone and prone-team rows match retail's lines to the hundredth with no drift
on the 2026-11-28 recaptures, as do two more team runs taken the same way, one
of them with the probe client's 34, 1 and 0 ms cmds (VERIFIED, the gate). The
2026-11-14 prone-team capture this replaced missed by 0.19 until the driver's
throttle moved to the probe's script clock below.

**The driver's script clock.** `vehicle_probe.rs` times its steps on the probe
client's wall clock from the loop pass that first sees a `PM_NORMAL` snapshot,
not on `serverTime`, and the `CMDS` lines log the forward key nowhere (`in=`
holds buttons, `wbuttons` and up). The `w=` stamps carry that clock. Taking its
start as the `w=` of the first cmd sent on the `SPAWNED` snapshot (its `snap=`
entry), 23 of the 24 use-tap edges (`in=` 64 on and off) of the six
2026-11-14 and 2026-11-28 run-over captures fall on the first cmd at or past
their step's boundary, and one 0.03 ms short of it (VERIFIED, the captures).
The stamp trails the loop's clock by a few varying microseconds (INFERRED).

- On the 2026-11-14 prone-team run the drive step (4150 ms) opens on a cmd
  repeating 13450, 16 ms after the first 13450 (4142 and 4158 ms on that
  clock). `SV_UserMove` skips it, so the first throttle cmd the server runs
  is 13466: frame 13450 reads `ct=` 13450, frame 13500 13466, and the Horch
  first moves at 13550 (VERIFIED, capture). The gate had put the throttle on
  `serverTime` 13450 (the tap's release at 11450 plus 2000), the first 13450,
  which ran in frame 13450: ours' jeep moved at 13500 (VERIFIED, the gate).
- On all six captures the first throttle cmd the server runs lands in frame F
  and `PROBE a`'s first nonzero speed reads F + 50 (VERIFIED, `ct=` and
  `PROBE a` lines).

`vehicle_runover_ab` now puts the driver's throttle on the cmds whose script
time (`common::cmds_script_ms`) lies in the drive step. The 2026-11-14
prone-team run (`mp_kharkov-jeep-runover-prone-team-repeat-uo.txt`) then
matches retail's lines to the hundredth, first drop, creep, damage and ram
sounds; the other five rows still do, and on `bail-prone` and `prone-team`
the first throttle cmd moves one cmd on inside the same frame (14066 for 14050,
16266 for 16250) (VERIFIED, the gate). No other gate replays a driver's keys
off the step table: `vehicle_die_ab` replays only `in=`, `vehicle_ram_ab`'s
captures log no `CMDS`.

### 11.7 A rider killed in his seat

VERIFIED, retail capture
(`crates/server/tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-die-uo.txt`,
2026-10-19, `client-probes/probe_vehicle` on mp_peaks with `+set
probe_vehicletype willyjeep_mp +set probe_side 75 +set probe_pitch 15
+set probe_suicide 3.5` and a `--probe-vehicle drive --save-slope
--slope-every-state` client): the Willys' driver, 3.5 s after his seat
(1.35 s into the drive script's full-throttle step), is killed with
`suicide()` where he sits.

- The death's snapshot (t 18700) is already out of the seat: `pm_type` 6,
  `vehPos` 0, `viewlocked_entNum` 1023, seat bits off, the origin where
  the seat left it, velocity 0; `vehType` 1 and `vehMotion` 2 stay until
  the respawn. `player_die` (0x4f972) touches no seat. The driven jeep's
  think, `G_VehicleClientThink` (0x8b0ce) after the entity pass, puts its
  dead riders out (`Scr_Vehicle_Think` 0x8c7cc, health under 1,
  `VEH_UnlinkPlayer(rider, 0)`), and the end frame's write (11.2) reads no
  link (INFERRED (decompile)).
- The view's pitch drops (24.999 seated, 0 on the death's snapshot), the
  yaw kept. `player_die` copies the entity's angles into `ps.viewangles`
  (combat doc 5.1 step 12, UO the same stores), and every cmd leaves them
  (0, view yaw, 0) (`ClientThink_real` 0x40c91's tail) (VERIFIED
  decompile and capture).
- The corpse leaves the hull as 11.6's body does, a cmd held inside it
  then a push off its origin: (-234, -218, 274) at t 18750 and (-189,
  -355, 267) at 18800, then a fall onto the slope (VERIFIED, capture). At
  ct 18793 retail's held cmd also lands on the hull: `EV_LANDING` on
  surface 13 with parm 9, the height of a 274 rise from where it stands.
  Its ground trace there meets the collmap's brushes in lump order: the
  sixth of the Willys' nine (vcod's brush 6989 on mp_peaks) at fraction 0
  on the face that turns to (-0.548, 0.101, 0.830); the seventh, an axial
  box, holds the whole sweep. The submodel's leaf trace (UO `coduo_lnxded`
  0x8059875; the box trace 0x805a9d5 sends a moving submodel sweep there)
  stops its brush loop once the fraction reaches 0, so the box is never
  clipped and the trace stands the corpse on the face (INFERRED (decompile);
  the event and its parm VERIFIED, capture and replay).
  `CollisionWorld::clip_model` stops at 0 the same way; clipping every
  brush read the trace all solid.
- The corpse's cone reads 255 on the death's snapshot and 0 after, and the
  drive script's reverse step sets `pm_flags` 0x40 on it
  (cod11-player-clip.md 1.1).
- The respawn (tdm's, at t 37000) reads `vehType` and `vehMotion` 0:
  `ClientSpawn`'s rebuild clears them (VERIFIED, capture).

vcod: the end frame clears `ClientSim::unseated`, `ClientSim::die` drops
the pitch, `cmd::dead_step` runs the head, and the respawn clears the
seat's two fields. `vehicle_ab`'s
`the_jeep_driver_killed_in_his_seat_replays_on_ours` replays the capture
(server seed 2, retail's death clip, retail's respawn spot pinned) and
matches every compared field through the whole capture, the landing's
event and parm included. Past the gsc's placement beside the hull (37050)
retail's body slides 65 units down the slope and settles at 37900, and
ours slides with it to the bit (VERIFIED, replay). The placement lands a
hair inside the terrain, so `PM_GroundTrace`'s start in solid lets the
body off the ground (cod11-mantle.md, "A start in solid on the ground
trace").

### 11.7.1 The corpse's strikes on the predictor

`predict_ab`'s UO slope replay misses the jeep-die capture's two snapshots
after the death (ct 18743 and 18793; `REPLAY_GAPS`). Both are game writes
ahead of a cmd's `Pmove`. The snapshots read `pm_flags` 0x200 with
`pm_time` 166 and 157 (VERIFIED, capture). Counted back over the cmds'
msecs, that is 200 set ahead of cmd 18718 (9 + 8 + 8 + 9 = 34 ms run
since) and again ahead of cmd 18759 (9 + 8 + 8 + 8 + 10 = 43) (INFERRED,
`PM_DropTimers` taking each cmd's msec). The velocities carry z (274, then
267), which the resting arm's `200 dir` never does, so both are the moving
hull's `VEH_PlayerCollision` (11.6 step 4) (INFERRED). That strike reads
the hull's record velocity (`rec+0xe8`) and its `r.currentOrigin`. The
capture logs the hull's entity only while the probe is seated (`veh=`),
and a vehicle's `pos.trType` 1 sends no velocity at all, so no predictor
replay of this capture can run it. Closing the gap needs a recapture that
logs the hull's state per think beside the slope lines
(`tools/re/gdb_vehicle.py`'s `think` lines), and a replay-side port of the
overlap test with the hull's collmap posed there. `vehicle_ab` replays the
same capture on vcod-server, and the corpse's velocity matches retail's to
the bit on both snapshots (VERIFIED, the gate).

## 12. The drive step

- `Scr_Vehicle_Think` (0x8c7cc) is every vehicle's think, a fixed 50 ms
  step. A driven one (`ent+0x17e` 2) steps once a server frame from
  `G_VehicleClientThink` (0x8b0ce), after every client's cmds and before
  every `ClientEndFrame`; any other on its own think (INFERRED (decompile)).
- The input is the driver's `pers.cmd`, the last cmd that ran: a cmd
  repeating the last one's `serverTime` never runs and never drives
  (VERIFIED, the tank drive capture at 29750).
- The motion (0x87cd3 and callees: the slide 0x8036e, the suspension
  0x81331, the collision probes 0x827ab), the turret and gunner aim
  (0x85b32, 0x86285), the sway pack (0x86735), the steer and the track
  clock (0x86a29, 0x86aa1) live in `crates/server/src/game/vehicle/physics.rs`,
  each formula with its constants. Two facts the replay forced: the yaw
  passes `AngleNormalize180`'s 16-bit truncation every step (VERIFIED (the instructions) 0x3dc9e), and terrain meets the hull's half box as a capsule
  of its half width (VERIFIED, replay: the min-radius capsule lands the
  Panzer 0.63 low on the first slope).
- **The sway pack's last bit.** 0x86735 builds the jolt's angle as a
  double, `phase * pi / 180` with `pi` and 180 loaded as doubles (`fldl` at
  0x867a8 and 0x867b0, rodata 0xa6c60 and 0xa6c68), so a half turn's sine
  is 1.2e-16, not the -8.7e-8 an f32 `pi` gives; the sine is stored as an
  f32 (VERIFIED (the instructions)). The pack sums `127 + 127 * deg / 30` on the
  x87 stack and stores it as a double before `floor` (0x86864..0x86920, 127
  and 30 as floats at 0xa6c78 and 0xa6c7c), so a sway a hair under 0 packs
  126 where an f32 sum rounds to 127 (VERIFIED (the instructions)). Both sit on
  the body's `tag_body` pose, so the seat tag and every rider's origin ride
  them. Packed in f32 with an f32 `pi`, the tank seats replay's driver
  missed retail by up to 0.39 on 2 snapshots (msg 305, 352: roll byte 127
  vs retail's 126), and by 0.47 on 7 others with the pack alone fixed.
  With both, the tank seats, drive and gun replays hold the rider's origin
  to 0.01 on every snapshot (VERIFIED, `vehicle_ab`).
- The probes and the slide meet vehicle clip brushes (contents 0x200),
  which only the vehicle clipmask 0x201 holds: where retail's tank bounced
  at mp_cassino's slope the brush is one (VERIFIED, replay). vcod's
  collision world keeps such brushes, unmet by every other mask.
- **Another vehicle.** The probes' mask holds 0x800000, so another
  vehicle's collmap stops them; when what they struck is a
  `script_vehicle` (scr_const 0x82), 0x827ab pushes it (VERIFIED (the instructions), constants at 0xa6b78..0xa6c2c):
  - unless its sway kick still runs (`rec+0x2b0` not 0) it is kicked
    (0x7d492) toward its origin from ours, scale `clamp(2 impact, 0.2,
    0.5)`, decel 700 (0x442f0000 stored as the fifth argument at 0x8391f)
    (VERIFIED (the instructions); gdb, 17.6);
  - with `k = 100 * its maxSpeed / ours` and `share = clamp(|our velocity|
    / our maxSpeed, 0.5, 1)`, a struck vehicle slower than `k * share`
    gains `k * share` along the line from us to it, capped at its
    `maxSpeed`;
  - not moving away from it (`(its origin - ours) . our velocity >= 0`,
    0x84341): one no faster-topped than us takes `min(|its velocity -
    ours|, our maxSpeed / 2)` off ours along that line when our velocity
    points at it, and the soft bounce is skipped; a faster-topped one
    zeroes ours (0x84361 to 0x848d6). Moving away leaves ours alone
    (INFERRED (disassembly)). Trap: putting the zeroing on the moving-away
    side.

  The kick (0x839aa) and the velocity (0x83ddd, 0x83fb9, 0x84195, again
  0x8423f..0x84281) are written on the other record at once (VERIFIED (the instructions)), so a retry pass that strikes it again in the same think
  reads the first push: the kick still runs, and a velocity under the
  share gains it again. vcod's step hands the kick and velocity to the
  parent, which writes them on the other record (`physics::VehiclePush`);
  a later hit in the same step starts from the earlier push.
- The struck vehicle moves on its own think, in the entity pass ahead of
  `G_VehicleClientThink`'s driven steps, so it first moves the frame after
  the hit (VERIFIED, the ram capture below: the Horch's speed falls 424 to
  113 at 24250, the tank first moves at 24300). That first think moves it
  the whole pushed velocity: the push writes `rec+0xe8` but not the local
  velocity the step's target reads, which a parked tank last wrote as 0,
  so no brake runs. Each later think loses the tank's `accel` (12.55 mph,
  220.9 units/s², 11.04 a think) (INFERRED, the formulas against the
  capture: the two hits move it 1.115 then 0.555, and 0.934 then 0.382,
  22.3 and 18.7 units/s, `k * share` with `k` 37.5 and `share` 0.59 and
  0.5). Measured on retail UO 1.51b, `client-probes/probe_vehicle_ram`
  (`--probe-vehicle ram`, fixture `mp_kharkov-jeep-ram-uo.txt`):
  mp_kharkov's second Horch driven 6 s at the Panzer IV parked 680 ahead
  glances off it twice (yaw 35 to 74) and shoves it 2.95 units, two thinks
  a hit (VERIFIED, capture). vcod runs the undriven thinks on their
  entity-pass turns (`think::run_undriven`, "The think's turn" below),
  moves the tank on retail's thinks and lands the first shove on retail's
  pose to 0.01 (-722.47, 7985.67) (VERIFIED, `vehicle_ram_ab`, which holds
  the tank's pose to 0.02 and its speed to 0.01 on every think between the
  hits).
- **The second shove.** Closed. A recapture with the tank's `speed` on
  each line (2026-10-15, `mp_kharkov-jeep-ram-uo.txt`) repeats the first
  capture's poses and shows the tank's think after the second hit starting
  from the whole pushed velocity: speed 18.745, then 7.699, moving 0.93
  then 0.38 (VERIFIED, capture). Without the fix below, that think's
  back-left probe (3) on ours strikes the Horch's right side at fraction
  0.976 (the box's end 2.6 units inside brush 22245's -y face, the Horch
  at yaw 58.93, pitch 3.77), the restore puts back the velocity the last
  think committed (0, the push wrote only `s`), and ours ends on the first
  shove's pose: 1.67 in all against 2.95 (VERIFIED, `vehicle_ram_ab`'s
  former `SECOND_SHOVE_GAP`).

  Ruled out:
  - the probe's box (-24,-24,-1)..(24,24,10) and `vehCheckSolidOffsets`
    (INFERRED (decompile), .data 0xa6660);
  - the info fields the probe reads: `suspensionTravel` +0x88, `stepsize`
    +0x390, front and back distances +0x394 and +0x398 (VERIFIED, the field table at
    .data 0xaea68);
  - `CM_TransformedBoxTrace` (coduo_lnxded 0x805b19a): the box symmetric
    about its centre and axial in the model's frame, the rotation
    `AngleVectors` with the right axis negated, as vcod's (INFERRED (decompile));
  - the brush clip's box arm (0x8058f0a) and `trap_Trace` passing no
    capsule (syscall 0x25 calls `SV_Trace` 0x809afc9 with 0) (INFERRED (decompile));
  - the collmap pairing: mp_kharkov has one Horch collmap, `*40` (VERIFIED, the BSP);
  - the pass rules: `SV_ClipMoveToEntity` (0x809a594) skips only the pass
    entity, what it owns and what its owner owns (INFERRED (decompile));
  - the entity area tree: `SV_Trace` walks it (0x805e8e5) on the box's
    centre with its half size plus a unit, and a vehicle is filed under
    its xmodel's bounds (`svFlags` 4), for the Horch a ±113.6 cube that
    holds the probe's end, so the walk reaches it (INFERRED (decompile)).

  Two replays reproduce retail's second shove exactly, speeds included:
  the tank's probes ignoring the Horch, or the Horch already being the
  tank's last struck entity (`rec+0x3d0`), so the strike reads as the tank
  moving away from it (`dot <= 0`, 0x827ab's `bVar4`) (VERIFIED, replay).
  The second needs a probe run between the hits. With the early-out (seat
  1 empty and the pose equal to the committed one) disabled, ours strikes
  the Horch on the think before the second hit and the second shove lands,
  but its think in the first hit's frame also strikes the approaching
  Horch and the first shove ends 0.41 short (VERIFIED, replay).

  Retail's tank runs the probes because its pose never settles after the
  first shove. `probe_vehicle_ram` with `+set probe_fine 1` logs the
  struck tank's pose off its placement pose, scaled so a float's last bit
  shows (`PROBE c`). From the placement (20300) to the first hit (26250)
  the delta is 0. After the first shove x and y stop at 26400 (1.52686,
  0.67334), but z flips by one ulp (1.5e-5 at z -135.3): -0.0210876 and
  -0.0211029 on alternate thinks from 26450 to 26750, while the speed
  reads 0 from 26550 (VERIFIED, retail UO 1.51b, 2026-10-15; the speeds
  repeat both earlier captures). The suspension (0x81331) rebuilds an
  undriven vehicle's z from its wheel traces each think, starting from the
  last z, so a last-bit change in a trace's end feeds the next think; at
  the spawn's whole-unit origin it holds still. A pose that differs from
  the committed one fails 0x827ab's early-out, so the probes run on those
  thinks, strike the approaching Horch and leave it in `rec+0x3d0`
  (INFERRED, the decompile against the capture). An `f32` suspension lands
  on a fixed point (z -135.30846, 1 to 2 ulps off retail's pair) and skips
  them. Replaying with the early-out off once the tank has left its spawn
  pose reproduces retail exactly: total shove 2.9504983, speeds 18.745,
  7.69916, 3.03935e-07, 2.71178e-14, 1.74685e-21 after the second hit,
  first shove unchanged (VERIFIED, replay).

  The cause is terrain. The wheels stand on mp_kharkov triangles 6504 to
  6506 (one sloped), and UO's engine clips a point trace against terrain
  in code that stores every intermediate as a float. VERIFIED,
  `coduo_lnxded` disassembly: the point clip (0x8055595, reached from
  0x8056ff5 when the trace is a point, `tw+0xc0`) computes per triangle
  `d2 = end . n - dist` and `d1 = start . n - dist`, each stored (`fstp`
  at 0x8055625, 0x80556ab). It goes on when `d2 < 0` and `d1 > 0`, stores
  `f = (d1 - 0.125) / (d1 - d2)` (0x80556d1, 0.125 at .rodata 0x80dc918)
  and compares it with the trace's fraction (`tw+0xc4`), stores `t = d1 /
  (d1 - d2)` and the point `delta * t + start` per axis (`tw+0x18` the
  delta), then the two barycentric distances `p . u - ud` and `p . v -
  vd`, each stored, against -0.001 and 1.001 (0x80dc91c, 0x80dc920), their
  sum on the stack against 1.001; a pass writes `f` and the normal
  (0x8055867..0x80558c7). The records come from 0x805403e
  (`PlaneFromPoints` 0x806a964 at 0x80547b8: `(p2 - p0) x (p1 - p0)`
  through `CrossProduct` 0x80666b1 and `VectorNormalize` 0x8066755, whose
  squared length, root and reciprocal are each stored; both edges stored,
  normalized, their cosine and `1 / (1 - cos^2)` stored, each scaled
  component stored, `u = e1 - cos e2` and `v = e2 - cos e1` stored and
  scaled by the stack's reciprocal of the edge's length, every distance a
  stored dot product with `p0`). 1.1d's terrain point clip (`cod_lnxded`
  0x8052894) keeps the same values on the stack.

  vcod clips points against terrain with `Terrain::point_clip_stored` (the
  records built the same way at load, `PointPlanes::v22`) for every point
  trace on a UO server and on a 1.5 one, whose 0x8055763 is the same code
  (`CollisionWorld::set_arith`, `bsp-ibsp59-format.md`, "Patch collision
  on 1.5 and UO"). With it the parked tank's z keeps moving by a last bit
  after the first shove, its probes run between the hits, and the second
  shove lands: the tank ends 2.9504983 from its spawn as retail's does,
  every think's pose to 0.02 and speed to 0.01 through both hits
  (VERIFIED, `vehicle_ram_ab`, 2026-10-16). A plain `f32` clip with a
  recomputed normal put a wheel's end on the face where the start's last
  bit said nothing.
- **The think's turn.** `G_RunFrame`'s entity loop (game.mp.uo 0x59c21)
  calls `G_RunEntity` (0x59a35) once per in-use entity by number, its link
  parent first. A vehicle is not a missile (4), item (3), mover (5, 8) or
  client, so its arm is `G_RunThink` (0x59862): when `nextthink`
  (`ent+0x20c`) is positive and not past `level.time` it is zeroed and
  `Scr_Vehicle_Think` called (INFERRED (decompile)). The think returns at
  once for a driven vehicle (`ent+0x17e` 2) while the flag at .data
  0xae954 is 0, setting `nextthink` to `level.time`; otherwise it steps
  and sets `nextthink` to `level.time + 50` (INFERRED (decompile), 0x8c7cc).
  `G_VehicleClientThink` (0x8b0ce) sets that flag, walks the 64 pool
  records (.bss 0xd43c0, stride 0x3ec, entity number at +0x228) in order,
  `G_RunThink`s each driven one, then clears it (INFERRED (decompile)). So
  an undriven vehicle steps on its own turn in the entity pass, among the
  missiles, movers and links by number, and a driven one after the loop in
  pool order.

  A model linked to a parked tank follows the tank's shove in the same
  frame. `probe_vehicle_ram` with `+set probe_link 1` spawns a
  script_model 100 over the struck tank (entity 437, the tank 187), links
  it and logs its origin each frame (`PROBE l`). On every frame the tank
  moves (first at 19800, the same shoves as above, speeds 21.10, 10.47,
  then 18.745, 7.69916) the model's x and y move with it in that frame
  (VERIFIED, retail UO 1.51b, 2026-10-20, fixture
  `mp_kharkov-jeep-ram-link-uo.txt`). vcod's `run_entity` runs
  `think::run_undriven` on the vehicle's turn, and `vehicle_ram_ab`'s
  `a_model_linked_to_a_shoved_tank_moves_on_the_tanks_frame` holds the
  model to 0.02 on every frame from 200 ms before the first hit to a second
  past the second. Run after the pass, the model trailed by a frame
  (-724.0 vs retail's -722.98, 50 ms past the first hit).

  Open: the seat ops (a dead rider's exit) and the run-over knock
  (`VEH_PlayerCollision`) an undriven think queues still apply in vcod
  after `G_VehicleClientThink`, with the driven ones'. Retail's think calls
  `VEH_UnlinkPlayer` for a dead rider itself, first thing after the driven
  check (INFERRED (decompile), 0x8c7cc). No capture tells the two apart yet.
- **Self-damage.** 0x827ab's tail runs two `G_Damage`s on the vehicle
  itself: inflictor and attacker the vehicle, dir the hit's normal, point
  the hit, dflags 0, `MOD_COLLISION` (0x18), hit location 2 (VERIFIED (the instructions) 0x856b8..0x857e2):
  - Under `g_vehicleEnableCollisionDamage` (registered 0), when the hit
    drove into the surface past 0.5 (`into`, -0x160): `trunc(2 (impact -
    0.5) * collisionDamage)` (`fistp` under 0xc00, 0x8571b). `impact` is
    capped at 0.5 (0xa6ba4) a few lines up, so the amount is never above 0,
    and `G_Damage`'s generic arm lifts it to 1: a collision costs 1 health
    (VERIFIED (the instructions) and bytes; INFERRED that nothing between the cap
    and the damage raises `impact`, read off the decompile).
  - With the stuck clock (`rec+0x3d8`) more than 7000 ms old: 999999
    (0xf423f), and the clock set 99999 ms ahead (0x857dd). The think resets
    the clock to `level.time` whenever the last hit (`rec+0x3dc`) is over a
    second old (0x9c9ac in the decompile's numbering, `.so` 0x8c9ac),
    after 0x827ab. `G_SpawnVehicle`'s memset leaves both 0, so a hit on a
    vehicle's first think takes the kill; every later think starts from a
    clock its predecessor reset (VERIFIED (the instructions)).

  `G_IsVehicleImmune` (0x8b893) lets `MOD_COLLISION` through, and
  `G_Damage` (0x4fedb) needs `takedamage`, which the spawn (0x7c935) sets
  (VERIFIED (the instructions)). vcod-server runs both from `think::self_damage`
  inside the think, ahead of the readers and the sounds; a death runs
  `Scr_Vehicle_Die` there, as the die callback does.
- **Sounds**, 0x87480 with `G_SetSoundBlend` (0x5a6a9) (VERIFIED (the instructions); VERIFIED capture: the crew Panzer's 187 on 17/18 and 188 on
  19/20):
  - B1 the idle pair and B2 the engine pair, crossfaded by the step's
    blend while a driver runs the engine (B2 also quiet with B1 present and
    the blend 0, or driverless under 0.1 units/s, 0.1 double at 0xa6d30);
  - `loopSound` the turret spin alias while it turns, its stop alias once
    as it stops;
  - B3 and B4 each MG's loop, held 200 ms past a round, its cooldown alias
    as the hold runs out.

  A blend goes out with `eventParm` and `scale` the aliases and
  `angles2[1]` the crossfade, unlinked when both aliases are 0.
- **Ported and unwired.** One step input has no writer in vcod-server,
  so its ported branches never run (read off vcod's code, not a retail
  claim; the neighbour walk has had one since 17.6):
  - `HitEntity::script_model`, classname `script_model` (scr_const 0x7e)
    on an entity a trace struck: `BodiesWorld::entity` fills only bodies
    and vehicles. So the slide's `trigger` notify to a struck script_model,
    the collision probes' 700 jolt with its 150 ms lockout, and the
    `trigger` notify in `think` never run (INFERRED: the struck-entity
    arms are ported off the decompile of 0x8036e and 0x827ab; no capture
    drives a vehicle into a script_model).
- **Dot products and normalizes.** `VectorNormalize` (0x3a1c3) and
  `VectorNormalize2` (0x3a466) sum the squared length x, y, z on the x87
  stack (`flds`/`fmuls` of offsets 0, 4, 8, two `faddp`), store it as a
  float, store `sqrt`'s result as a float and the reciprocal `1 / len`
  as a float (1.0 at rodata 0x9da10), and store each component times it;
  `VectorNormalize` returns the stored length, and `VectorNormalize2`
  writes the 0.0 it compared the length against (rodata 0x9da0c) into
  each component of a zero vector (VERIFIED (the instructions),
  0x3a1d5..0x3a277, 0x3a479..0x3a528). The
  inline dot products in `VEH_SlideMove` sum x, y, z the same way
  (0x806f2..0x80731, 0x80826..0x8086b) (VERIFIED (the instructions)).
  `physics.rs` takes `vcod_common::x87`'s `dot`, `normalize_stored` and
  `normalize2_stored`, which sum in that order.

## 13. Guns, heat and damage

Contents, by subsection and the paragraph names code cites: 13.1 the
readers and fire calls, "Inside the call, and only with a driver", "The
bend's arithmetic", "The guns' spread", "The shell's bits", heat; 13.2
`G_Damage` on a vehicle, "The driver's feedback"; 13.3 "A blast on a
hull", "The firing vehicle's own splash", "A frag's splash on a hull",
"Measured, rockets and frags"; 13.4 "The hull's clip, surface by
surface", the riders under fire; 13.5 "The rider's pose", "The broad
phase"; 13.6 "The silhouette"; 13.7 "The tank driver", "Back from the
gunner's seat", "The driver's idle", "The tank driver's link offset",
"The glide back", "Two runs that part", "The glides on the jeep", "The
weapon-swap frame", "The angle updater on a rider", "The tank driver's
idle"; 13.8 "Retail's spawn point", "The shooter's trigger"; 13.9 "The
inflictor swap", "The barrel test", "A hull stops rounds", "A shell on a
hull".

### 13.1 The readers, the fire calls and heat

- **The readers.** The driver's reader (0x86c57), with a turret weapon
  and `capturing` 0 (INFERRED (decompile)):
  - `s.weapon` the turret weapon;
  - main timer `< 1` with `buttons & 1` notifies `turret_fire`, else 50
    off; the alt timer 50 off every think, `< 1` with `buttons & 0x20`
    notifies `turret_alt_fire`;
  - the aim point 10240 along the view from `tag_chasecam`
    (`tag_aimdownbarrel` sighted), stopped by a locational trace under
    0x2091 (no vehicle, no body) passing the driver, stored at
    `rec+0x28c`;
  - a `trap_Trace` between the flash and barrel tags that hits sets
    `rec+0x270`, the straight shot.

  The reader runs in the vehicle's think, after the frame's threads, so a
  gsc `setPlayerAngles` on the driver turns the aim, and a script's
  `fireTurret` bends toward the point the last think stored (VERIFIED, the
  shell capture below: the round bent 5 down and 2 across toward the
  Elefant where the cmd's own view pointed behind the tank). vcod's think
  stores the point (`fire::aim_point`) and the barrel flag
  (`barrel_blocked`, "The barrel test" below). The gunner's reader
  (0x871a6) notifies `turret_gunner_fire` the same way (VERIFIED (the instructions)). The scripts fire on the next frame's threads.
- **The fire calls.** `fireTurret` (0x8ee02): from `tag_flash`, the
  barrel's forward bent toward the aim point by at most 5° pitch, 2° yaw.
  A bullet weapon's spread is `minSpread + rand * maxSpread`; a projectile
  weapon goes through `Weapon_Artillery_Fire` (0x7b594) to `fire_rocket`
  (0x5fc74): `trType` 5 (z falls 400 t²), speed truncated per axis, the
  driver credited through the vehicle, `MOD_PROJECTILE` direct and
  `_SPLASH` around. Then event 163 on the vehicle, the hull jolted 0.75
  back along `tag_barrel`, the timer `fireTime` (INFERRED (decompile)).
  `fireAltTurret` (0x8f57e) fires from `tag_altfire` (164) and
  `fireGunner` (0x8fc7a) along `tag_secondary_flash` (165); both refuse
  while overheated, add `fireHeat` and hold their loop 200 ms (VERIFIED (the instructions)).
- **Inside the call, and only with a driver.** `fireTurret` compares the
  record's `+0x3a0` with 0x3ff (0x8ee8a) and on a match jumps to its
  epilogue (0x8f574), past every fire (VERIFIED (the instructions)). INFERRED:
  `+0x3a0` is the driver's number. VERIFIED, live capture
  (`client-probes/probe_shellfuse`, `cod11-combat.md` 13.7): an empty
  Panzer IV spawns no `rocket`; with a driver the shell is listed by
  `getEntArray("rocket", "classname")` on the calling thread's next
  instruction. vcod fires a shell gun in the builtin
  (`vehicle::fire::launch_shell`); a bullet gun's round still waits for the
  server's trace after the threads.
- **The bend's arithmetic.** `fireTurret` (0x8f0fe..0x8f2ae) (VERIFIED (the instructions)):
  1. the aim point less `tag_flash`'s origin, each component stored,
     through the stored `VectorNormalize` (0x3a1c3);
  2. `vectoangles` (0x3aded) of the tag's forward and of that;
  3. `AnglesSubtract` (0x3dc11) of the two, pitch clamped to [-5, 5] and
     yaw to [-2, 2] (f32 at rodata 0xa79cc, 0xa79d0, 0xa79d4, 0xa79d8),
     roll 0;
  4. `AnglesSubtract` of the tag's angles and the clamped pair, which wraps
     the yaw back into [-180, 180];
  5. `AngleVectors` (0x3b068) of the result for the forward.

  `fireAltTurret` runs the same chain off `tag_altfire` with its own copy
  of the four limits (0xa79e4..0xa79f0) (VERIFIED (the instructions) and bytes). The
  bend is skipped where the vehicle's `r.ownerNum` (+0x154) is 0x3ff, the
  barrel is blocked (`rec+0x270`), or the main gun has two barrels
  (VERIFIED (the instructions), 0x8f0ce..0x8f0f8).
- **The guns' spread.** All three fire calls hand the round the flash
  tag's frame: the bent (or straight) forward, the tag's `axis[1]` (its
  left) where the spread reads its right, its `axis[2]`, and the muzzle
  (VERIFIED (the instructions), 0x8f2d9..0x8f3da). A bullet weapon goes to
  `Bullet_Fire` with `minSpread + rand() / 2^31 * maxSpread` (weapon
  `+0x298`, `+0x2a4`; 2^31 at 0xa79dc), so its end point is
  `BG_Bullet_Endpos`'s stored chain 10000 out (VERIFIED (the instructions),
  0x8f3ec..0x8f44c; INFERRED that it lands on the bits the kar98k's did,
  cod11-combat.md 2.2: no vehicle MG round was read under gdb). A
  projectile weapon goes to `Weapon_Artillery_Fire(vehicle, 0.0, frame)`
  (0x8f459..0x8f471), the bazooka's function with spread 0 and `trType`
  rewritten to 5 (0x7b6c8): `tan(0)` makes `k` 0, `gunrandom` (0x399e0)
  still draws its two `rand()`, the point is `forward * 16` stored plus two
  zero offsets, then the stored `VectorNormalize`, and `fire_rocket`
  normalizes it again (VERIFIED (the instructions), 0x7b594..0x7b6c8).
- **The shell's bits.** VERIFIED, gdb on retail UO 1.51b (2026-10-21,
  `client-probes/probe_vehicle_shell` on mp_kharkov with
  `probe_vehicletype panzeriv_mp`, the driver `--probe-vehicle sit`;
  dprintfs at 0x8f0ce, 0x8f145, 0x8f17a, 0x8f19c, 0x8f288, 0x8f2b3,
  0x8f459, 0x7b5e6, 0x7b681, 0x7b6cf). All three shells read:

  | Value | Bits |
  |---|---|
  | tag forward | (-0.9721, -0.0983, -0.2129) |
  | aim point | (-1472.85, 6193.82, -62.38) |
  | the bend's angles | (12.29, 185.77) and (43.47, 182.98) |
  | difference, clamped | (-31.18, 2.80) to (-5, 2) |
  | bent angles | (17.29, -176.23) |
  | bent forward | (-0.95273, -0.06282, -0.29724) |
  | `k` | 0 |
  | `trDelta` | (-9527, -628, -2972), the forward × 10000 truncated |

  Each shell's own `gunrandom` pair leaves the point at 16 times the
  forward and the direction on the forward's bits. vcod-server runs the
  chain in `vehicle::fire::bend` and `prepare` (`combat::rocket_dir`,
  spread 0, `gunrandom` drawn) on those bits to the last one
  (`vehicle::fire::a_uo_tank_shell_leaves_on_retails_bits`). In
  `vehicle_shell_ab`'s replay ours poses the tag a few ulps off retail's
  (forward y 0xbdc941d5 vs 0xbdc941b0, muzzle z 0x40e2db70 vs 0x40e2db6a),
  so its bent forward's y sits 0x1f ulps off and its z one (VERIFIED,
  replay): the pose, not the chain. The capture cannot hold the shell's
  `trDelta`: it strikes the Elefant inside its first frame and is only
  ever sent exploded, stationary (VERIFIED, capture). Traps: an f32
  `atan2` with a `%` wrap, an unwrapped yaw, a single normalize, or an MG
  disc around the tag's right instead of `BG_Bullet_Endpos` all miss
  these bits.
- **A round's event parm** is its weapon with bit 7 set unless it struck a
  player: the round raises no impact of its own because its inflictor
  (the vehicle) is not its attacker (INFERRED (decompile), 0x7a920; VERIFIED
  capture: 164 parm 0x9b, 165 parm 0xa0).
- **Heat.** Overheat at 1.0, recovery at 0.5 or under (0xa6d14, 0xa7590),
  checked before the cooling by `cooldownRate` each think; event 208 on
  the vehicle (the alt gun's not on a four-wheel). The gunner's overheat
  writes `animMovetype` 1 and its recovery 0, over the seat mask, sets
  `pm_flags` 0x800000 on the gunner and notifies `overheating` every think
  (VERIFIED (the instructions); VERIFIED capture: 49 rounds, 208, the mask 6 → 1 →
  0, recovery 2550 ms on).

### 13.2 Damage on a vehicle

- **`G_Damage` on a vehicle** (0x4fedb) (VERIFIED (the instructions)):
  1. `G_IsVehicleImmune` (0x8b893): bullets need `bulletDamage`, grenades
     `grenadeDamage`, projectiles and artillery `projectileDamage`; water,
     explosive and collision always land; every other means never does.
  2. A blast under 100 is dropped.
  3. `Scr_Vehicle_DamageScale` (0x8ccad) by the hit's side, 0.7 of a blend
     for splash, `dmgScaleBullet` on bullets.
  4. `damage (amount, attacker, dir, point, mod, inflictor)`; at 0 `death`
     and `Scr_Vehicle_Die`; else `Scr_Vehicle_Pain` (0x8d1ca) jolts the
     hull when the attacker holds a grenade or a launcher, the means is
     `MOD_EXPLOSIVE` or the damage tops 200.
- **The driver's feedback.** `Scr_Vehicle_Pain` (0x8d1ca), after the jolt,
  hands the damage to the driver. When `otherEntityNum` (`es+0x74`) is
  under `level.maxclients` and names a client whose `connected`
  (`client+0x4520`) is 2, it adds the scaled damage (≥ 1) to
  `damage_blood` (`client+0x4658`) and sets `damage_from`
  (`client+0x465c`) to the vehicle's `r.currentOrigin` (+0x13c) less the
  point, unnormalised, with the no-direction flag (`client+0x4668`) 0. A
  vehicle that is its own attacker stores the point and sets the flag 1,
  which `P_DamageFeedback` turns into yaw and pitch 255 (VERIFIED (the instructions), `game.mp.uo` 0x8d1ca; `G_Damage` 0x4fedb calls pain as
  `(self, attacker, damage, point, mod, dir, hitloc)`). The driver's
  health does not move; his end frame's `P_DamageFeedback` raises
  `damageEvent`, `damageCount` (the damage, out of `maxHealth` 100), the
  cone and `EV_PAIN` 100. The tank captures' main gun shots read this on
  the driver: `damageCount` 44 on the seats capture, 30 on the drive
  (VERIFIED, capture). vcod-server queues the same feedback
  (`SimOp::VehiclePain`) and matches the event, its parm, `damageEvent`
  and both counts (VERIFIED, `vehicle_ab` replay), given the splash
  measure below.

### 13.3 Blasts on a hull

- **A blast on a hull.** `G_RadiusDamage` (game.mp.uo 0x5098b) measures an
  entity with `r.bmodel` (+0x104) to its `r.absmin`/`r.absmax` (+0x124,
  +0x130) on each axis, and one that also has a vehicle record (+0x164) to
  that box drawn in by three quarters of `r.mins`/`r.maxs` (+0x108,
  +0x114): `absmin - 0.75 mins` to `absmax - 0.75 maxs` (VERIFIED (the instructions)). `VEH_SetupCollmap`'s `SetBrushModel` makes a paired vehicle
  a brush model, so UO's `SV_LinkEntity` (`coduo_lnxded` 0x8099dee) files
  it, once any angle is set, under a cube of `RadiusFromBounds`
  (0x806a0c3) of the collmap's bounds about `r.currentOrigin`, grown a
  unit (INFERRED (decompile)). `CanDamage` probes about the box's middle,
  the origin. The direction `G_Damage` gets is `r.currentOrigin` less the
  blast lifted 24, normalized for the `damage` notify and the pain's jolt
  (VERIFIED (the instructions), 0x5098b and 0x4fedb). `Scr_Vehicle_DamageScale`
  (0x8ccad) measures the point against the record's physics pose
  (`rec+0xb8`, angles at `rec+0xd0`) through `AnglesToAxis` of all three
  angles, the hull's pitch and roll included (VERIFIED (the instructions)).

  vcod links a paired vehicle as a brush model (`LinkKind::Vehicle`),
  measures the blast as above (`EntityVictim::vehicle_link`) and scales on
  the record's pose. On the two tank captures the own shell's splash
  charges 189 × 0.233 = 44 and 134 × 0.231 = 30, retail's counts
  (VERIFIED, `vehicle_ab` replay); measured axially on the file's box from
  the origin it charged 43 and 38. The scale times the damage is truncated
  (`fistp` under a 0xc00 control word, 0x4ffbf..0x4ffd1) and a result
  under 1 is lifted to 1 (VERIFIED (the instructions); the jeep's rear 18.75
  reads 18 in the hits capture below). The generic tail takes the damage
  off `health` for every classname but `misc_mg42` and `misc_turret`
  (scr_const 0x4e, 0x50), so a vehicle's health falls; the stock scripts
  set 2900 and refund friendly fire (INFERRED (decompile) and script).
- **The firing vehicle's own splash.** `G_MissileImpact` (0x5db67) hands
  the struck entity to `G_RadiusDamage` as the one to skip (call at
  0x5e1ae, after the direct `G_Damage` at 0x5ded6), and `G_RadiusDamage`
  (0x5098b) skips only that entity and those without `takedamage`: no test
  of the attacker or the inflictor (INFERRED (decompile) and disassembly).
  VERIFIED, live capture (retail UO 1.51b, 2026-10-21,
  `client-probes/probe_vehicle_shell` on mp_kharkov, fixture
  `crates/server/tests/fixtures/vehicles/mp_kharkov-tank-shell-uo.txt`):
  each of the Panzer IV's three shells strikes the Elefant 276 units off,
  about 200 from the Panzer's origin, and the Panzer's own `damage` notify
  reads 70 `MOD_PROJECTILE_SPLASH`, attacker 0 (the driver), inflictor 162
  (itself), in the same frame and logged ahead of the Elefant's 279
  `MOD_PROJECTILE`. vcod charges the same 70 in the same order
  (`vehicle_shell_ab`). The notify's point, the blast 4 off the struck
  face, reads (-1417.37, 6198.68, 1.10) on retail and (-1417.64, 6197.88,
  0.40) on ours; the direct hit's point is retail's to the hundredth
  (VERIFIED, replay). Not measured: whether the struck face's normal or
  the trace's start puts it there; the gate holds the point to 4 units.
- **A frag's splash on a hull.** `Scr_Vehicle_DamageScale` (0x8ccad) takes
  `MOD_GRENADE_SPLASH` (4) through its own arm: the scale is the
  inflictor's weapon's `vehiclescale` (weapon info `+0x4ac`, off the
  grenade's `s.weapon` through `BG_GetInfoForWeapon`) when above 0, else
  1, times `dmgScaleUnder` / 100 when the point sits under half the hull's
  height plus 10 and within 0.8 `r.maxs[1]` of the origin (VERIFIED (the instructions)). `G_Damage` (0x4fedb) passes it the attacker and the
  inflictor in that order; `G_ExplodeMissile` (0x5e2b5) hands
  `G_RadiusDamage` the grenade as the inflictor (INFERRED (decompile)). The
  stock frags carry `vehiclescale 0.5` (`fraggrenade_mp`,
  `stielhandgranate_mp`, VERIFIED, the files). A grenade's fuse explode
  never sets 0x100000; only `G_MissileImpact` does (VERIFIED (the instructions)).
- **Measured, rockets and frags.** Retail UO 1.51b, 2026-10-20,
  `client-probes/probe_vehblast` (fixtures `*-blast-tank-rocket-uo.txt`,
  `*-blast-tank-frag-uo.txt`, `*-blast-jeep-rocket-uo.txt`,
  `*-blast-jeep-frag-uo.txt` and the `*-uo-blasttank`, `*-uo-fragtank`,
  `*-uo-blastjeep`, `*-uo-fragjeep` snapshot pairs; recipes in their
  headers):
  - A bazooka round on the Panzer IV: 500 on its flank, 329 on its front,
    1000 on its rear (1000 times the side, front and rear scales, no
    falloff at 200 to 300), `MOD_PROJECTILE`, attacker the shooter,
    inflictor the rocket; the explode is 186, `eFlags` 0x210000, type 2,
    weapon 5, surface 13 (metal). The third takes the tank under the
    script's 2000 health buffer and `_tankdrive_gmi` kills it inside the
    notify: the probe's waiter, woken by the same notify, reads -999
    (VERIFIED, live).
  - On the Willys: 899 on its flank (1000 times the f32 0.9, truncated),
    which kills it; its wreck's `radiusDamage(origin + (0, 0, 25), 300,
    80, 0)` reaches the shooter 245 units off for 11 `MOD_EXPLOSIVE` 10.6 s
    later, after `setContents(0)`. A round 150 off its flank charges
    nothing: the splash is under 100 (VERIFIED, live).
  - Frags: 60 by the tank's flank (120 × 0.5), 60 on its deck, 120 under
    the jeep's body (× the under scale 2), 51 and 60 by it; those further
    off charge nothing (under 100 before the scale). The explodes are 185
    with `eFlags` 0x100 and no vehicle bit. The deck frag rests on the hull
    at z 51.7 (its ground the tank) and its explode's down trace (mask
    0x11) meets nothing: surface 0, parm 0, the splash point 10 up
    (VERIFIED, live).
  - A frag that strikes a hull bounces with no event and charges nothing
    on the strike: the deck frag's hull contacts and the jeep's second
    frag, which strikes its flank and its body before it rolls back to the
    thrower, raise no 181 (`EV_GRENADE_BOUNCE`) there, and the hull's first
    notify is the splash's (VERIFIED, `*-uo-fragtank.bin`,
    `*-uo-fragjeep.bin`, `event_dump`). INFERRED: that is
    `G_MissileImpact`'s arm for a struck entity that takes damage while
    the frag's `damage` is 0 (cod11-combat.md 13.1), keyed on
    `tr.entityNum`.
  - The bazooka's hip spread (7°) draws on each server, so its contacts
    repeat neither between two retail runs nor against ours; the amounts
    do (VERIFIED, live).

  `vehicle_blast_ab` replays all four: notifies, the shooter's own blasts
  and the explodes match field for field, a rocket's aim within its spread,
  and each frag's bounces by the ms since its throw and their surface
  (VERIFIED, replay). Ours named the jeep's flank through its clip in the
  world rather than its posed model and so raised a metal bounce there,
  until the quiet rule read the struck entity. It rests on three rules: the frag's `vehiclescale`
  (without it ours charged 120 by the flank); a wreck with no contents
  meets no trace (the entity clip skips one whose `r.contents` meet none
  of the mask, `coduo_lnxded` 0x809a72a, INFERRED (decompile); else the
  posed hull stops the wreck's blast); and the hull's clip a surface at a
  time (below). At mp_cassino's spot 300 ahead of the tank, (4376, -1979),
  the placed shooter slides 65 to 90 units down the slope before the press
  (two runs, two slides; VERIFIED, live), off the ground by the ground
  trace's start in solid (cod11-mantle.md, "A start in solid on the ground
  trace"). Where he stops rides the split of his cmds: on the probe's even
  16 ms cmds ours stops 31 units from retail's spot; on retail's own cmd
  times (the capture's `CMDS` lines, clocks aligned) on it to the
  hundredth (VERIFIED, replay). The gate flies each bazooka round on
  retail's `trDelta`, rebuilt from its direct hit's direction and the
  `projectileSpeed`, so the contacts sit within a unit of retail's
  (INFERRED: the direction is logged to two decimals).

### 13.4 The hull's clip and rounds on riders

- **The hull's clip, surface by surface.** The DObj clip (`coduo_lnxded`
  0x80c7be8 over each model, 0x80c4892 over its surfaces) moves the
  segment into each surface's bone frame and tries the surface only when
  its contents meet the mask and the segment's part up to the best
  fraction so far enters the surface's stored `mins`/`maxs` (0x805e387, a
  slab test with no epsilon, minimum faces first) (INFERRED (decompile)).
  Its triangle test enters through the front (start strictly ahead, end
  behind), backs the fraction off 0.125 (`0x80ea90c`) and keeps it when
  under the best, even below 0 (INFERRED (decompile)). So a triangle on a
  surface walked later loses to an earlier surface's hit when its bounds
  start past it, whatever its backed-off fraction: the deck frag above
  slid down the tank's right side 0.1 off the wall, and the wall's -0.045
  lost to the deck's 0.152 (VERIFIED, replay). vcod's posed hull keeps
  each surface's run, bone frame and bounds
  (`vehiclepose::posed_collision_surfs`, `collision::clip_model_surfs`),
  and a script model takes the same clip at its bind (cod11-combat.md
  2.7.1).
- **A rider and a blast.** A rider takes a tenth of a blast, a fifth in
  artillery (`G_VehicleOccupantRadiusDamageScale` 0x8ba86), and a tank's
  driver nothing whose inflictor is not his own tank
  (`G_IsVehicleOccupantInvulnerable` 0x8b96b) (VERIFIED (the instructions)). The
  rider branch, in order, inside `G_DamageClient` (0x4fd4b), ahead of
  `Scr_PlayerDamage` and so of the damage callback:
  - It runs for a victim whose `ps.eFlags` has 0x100000 (VERIFIED (the instructions)).
  - A `DFLAG_RADIUS` hit is multiplied by the scale and truncated (`fldcw`
    with 0xc00 ORed in, 0x4fdf3..0x4fe05) (VERIFIED (the instructions)).
  - The scale is 0.2 (f32 at 0xa74b0) for file type 5 and 0.1 (0xa74b4)
    for every other (VERIFIED bytes and decompile).
  - Both helpers protect a rider only when `ps.eFlags` has 0x100000
    without 0x400000 and `r.ownerNum` (+0x154) names an entity with a
    vehicle record. `VEH_LinkPlayer` (0x8998d) writes the vehicle's number
    into the rider's `r.ownerNum` (INFERRED (decompile)).
  - The invulnerability needs file type 2 (`cmpw $0x2` at 0x8ba54) and
    `vehPos` 1 (`cmpl $1,0x614` at 0x8ba64): in a tank only the driver has
    it, and only while not popped out, which 1.51 never sets (section 1)
    (VERIFIED (the instructions)).
  - The early return needs an inflictor whose number differs from the
    rider's `r.ownerNum`. A NULL inflictor passes, so a `radiusDamage`
    blast and fall damage reach a tank's driver (INFERRED (decompile)).
  - A bullet names its shooter as the inflictor (`Bullet_Fire_Extended`
    0x7a920), unless the shooter rides in a seat other than 3, when it is
    his vehicle (INFERRED (decompile)).
- **Rounds on riders, held trigger.** Retail UO 1.51b, 2026-10-15,
  `client-probes/probe_vehicle_armor` (fixtures
  `crates/server/tests/fixtures/vehicles/*-armor-uo.txt`, recipe in their
  headers). The rider sits 12 s in each seat while a sighted `thompson_mp`
  140 units off the flank holds its trigger on him. The gsc aims at the
  height where a `bulletTrace` (mask 0x2802031 and `bulletPriorityMap`, as
  `Bullet_Fire_Extended`'s, builtin 0x6b145) from the shooter's eye meets
  the rider first; its damage callback only logs.
  - Panzer IV driver: 0 callbacks in two 12 s stints, though the trace met
    his body at 6 to 8 of 16 heights. Gunner: 1 callback in one run, 10 in
    another, the trace meeting him at 2 or 3 heights. On foot before and
    after: about 10 a second (VERIFIED capture).
  - Willys: driver 1, gunner 4, passenger 11 (VERIFIED capture).
  - Traces toward the seated rider that miss him stop on the tank, entity
    186, at fraction 0.69 to 0.83 (VERIFIED capture).
  - The retail probe client (`--probe-uo-weapon hold`) sends 60 Hz cmds
    (cod15-movement.md 6) holding the view it saw at its placement,
    rebased on each snapshot's `delta_angles`, while the gsc's
    `setPlayerAngles` writes the view and `delta_angles` against the last
    cmd every frame. Over a real link the rebase lags, so the rounds land
    between the two views by the client's timing (INFERRED from the two
    runs below). With the shooter's placement turned 25° off the target
    (`probe_shoot_yaw 25`) both servers took a round every 100 ms on foot
    (retail 240, ours 241 in 26 s), so neither fires along the held view
    alone; a second retail run of the jeep recipe put 4, 6 and 68 rounds
    into driver, gunner and passenger and 57 into the body after the exit,
    against the fixture's 1, 4, 11 and 6 (VERIFIED, live, 2026-10-15). The
    hold captures measure the stream, not the riders.
- **Rounds on riders, aim sweep.** Retail UO 1.51b, 2026-10-15 (fixtures
  `*-armor-sweep-uo.txt`, recipe in their headers): `--probe-uo-weapon
  aim` sends every cmd at the newest snapshot's view, and `probe_aim_sweep
  1` steps the aim through 0..120 above the target's origin by 8 every
  half second. Rounds that reached the rider, by height aimed at (a step
  fires about five) (VERIFIED, live):

  | Seat | Retail | vcod-server (loopback, same probes) |
  |---|---|---|
  | Willys driver | 0..40 (40 rounds) | 41 |
  | Willys gunner | 16..64 (53) | 58 |
  | Willys passenger | 0..40 (41) | 41 |
  | Panzer IV driver | none | none |
  | Panzer IV gunner | 48..64 (15) | 8 at 48..56 |

  Willys hits land mostly on `torso_lower`, then `torso_upper` and `head`,
  on both. A driver left in his bind pose took rounds up to 72, on the
  feet and legs. `vehicle_armor_ab`'s sweep tests replay the probe in
  process (`common::ProbeHold::aim`, the target holding the view of its
  use tap) and compare each seat's rounds a step, height by height, within
  2.5 a step; every seat is in the comparison (VERIFIED, replay).

### 13.5 The rider's pose and the broad phase

- **The rider's pose.** `G_PlayerVehiclePositionAndBlend` (0x9048f), at
  the end of `ClientEndFrame`, besides placing the body:
  - Seat 1 or 3 of a type 1 vehicle (four-wheel) whose legs anim carries
    `turretanim` (anim record `+0x50 & 4`): it clears the anim's goal
    weights, takes its child at `numChildren / 2` and, under that, the
    middle child at `1 - s` and an end child at `s` (the last for a
    positive steer, entity `+0xd8`, the first for a negative), `s =
    min(|steer| / 45, 1)` (0xa7b10 45.0), each goal weight set over `1 /
    (10 |change|)` s (0xa7b28 10.0); no end child at `s` 0 (VERIFIED (the instructions)). Constants: `.rodata` 0xa7b0c 0.0f, 0xa7b10 45.0f, 0xa7b14
    1.0f, 0xa7b18 45.0 and 0xa7b20 1.0 as doubles (the negative arm's),
    0xa7b28 10.0f (VERIFIED, file bytes). The steer is the vehicle's
    `s.leanf`: entity `+0xd8` is UO's netfield offset 216 for `leanf`
    (`fields_v22.rs`) (VERIFIED, table).
  - Type 1 seat 2, type 5 (any seat) and type 2 seat 2 clear the goal
    weights the same way and set the child at `numChildren / 2` and its
    middle child to 1, with no steer (INFERRED (decompile)). So the Panzer IV
    gunner's `standMG42_aim` always plays
    `pb_standMG42gunner_aim_forward_level`, wherever the MG points, and the
    jeep gunner his one `_level` leaf. vcod ports all four arms
    (`playerpose::seat_leaves`, played in place of either channel's layer
    on that node, the torso of a `both` anim sharing it) at the goal
    weights, with the ramp (section 14, "The seat blend's ramp"). The jeep
    driver's `vehicleJeepDriverIdle` holds `_aim` over `turnL`, `idle`,
    `turnR` (file order; the engine's `turnR`, `idle`, `turnL`), so a left
    steer (positive) blends toward `turnL`.
  - `r.currentAngles` and the client record's `+0x3f4` take `AxisToAngles`
    of the seat tag (VERIFIED (the instructions)). The controllers (0x1e761) read
    `+0x3f4` for a rider (`eFlags` 0x100000): the legs' angles are the
    tag's; the torso's the tag's too, or the view's when
    `BG_AllowPlayerWeaponAtVehiclePos` (0x23148: type 1, seat 3) allows a
    weapon; the head's the view's, a yaw more than 90 off the torso folded
    back (`180 - d`, `-180 - d`); then head less torso, torso less legs,
    and `tag_origin` gets no turn; the spine splits as on foot (VERIFIED (the instructions); vcod `Controllers::rider`).
  - `ClientThink_real` writes `r.currentOrigin` from `ps.origin` and
    `r.currentAngles` as `(0, view yaw, 0)` on every cmd (0x41c35..0x41ca7)
    (VERIFIED (the instructions)), so a round from another client's cmd usually
    meets the rider in his view's yaw, not the tag's. Posed in the tag's
    frame, the driver and passenger took rounds on the right arm and thigh
    where retail's took them on the torso; in the yaw they match (VERIFIED,
    the sweeps above).
- **The broad phase.** `SV_ClipMoveToEntity` (`coduo_lnxded` 0x809a72a)
  rejects a player (`svFlags` 2) when the segment up to the trace's
  fraction misses the box `r.currentOrigin` + (-64, -64, -32)..(64, 64, 72)
  (`.data 0x80f5110`; 1.1's `cod_lnxded` the same at 0x80e30d0), and a
  model (`svFlags` 4) when it misses the xmodel's own bounds (0x80c6b36:
  the model's `+0x4c`/`+0x58`) added to the origin unrotated (VERIFIED (the instructions); 0x805e387 is a slab test over `[0, fraction]`). 0x80c3ff0
  reads the model bounds off the DObj's first model's `+4` record,
  `+0x4c..+0x60` (INFERRED (decompile)): the descriptor's own `mins`/`maxs`
  after its version, a cube about the origin (INFERRED (decompile) of the
  parser 0x80c3310 and the paks; `cod11-combat.md` 2.7, "The whole-model
  bounds reject"). vcod's body test takes the fixed box
  (`combat::BODY_BROAD_MINS`), keeps the nearest bone any candidate
  scores, and passes the 1.1 combat gates unchanged; a seated passenger's
  feet under his origin meet the profile's rays at -4 as retail's do
  (VERIFIED, replay). vcod ports the model reject (`PlacedModel::bounds`,
  the same for a script model); no UO `mp_vehicle_*` or
  `mp_static_vehicle_*` model's collision leaves its cube at rest
  (VERIFIED, the paks: the Panzer IV's reaches 172.3 of 201.7). On a
  hull the reject therefore only saves work, a turned turret staying
  inside (INFERRED).

### 13.6 The silhouette

- **The silhouette.** Retail UO 1.51b, 2026-10-15, `probe_profile 1`
  (fixtures `*-silhouette-uo.txt`, recipe in their headers; the tank's and
  the seated jeep's recaptured 2026-10-21, see "Retail's spawn point").
  Every 2 s the probe traces a grid of level `bulletTrace`s
  (hitCharacters) from the vehicle's right across client 0, `z` -8..84 by
  4 above his origin and `f` -24..24 by 6 along the hull, and logs where
  each that meets him or the vehicle stops. The rider's origin and angles
  match retail's to the hundredth in every seat (VERIFIED, live). Three
  rules close the differences:
  - The jeep gunner: four rays at 4 and 8 met his feet on ours where
    retail's met the bed's side panel (surface 8, `tag_player`'s, 2 to 7
    units in front of the feet), with his body matching to 0.01. Retail
    clips each entity only up to the trace's fraction (0x809a72a,
    VERIFIED (the instructions)); `combat::trace_bodies` drops a bone past it
    (VERIFIED, replay).
  - Two rays in front of the jeep driver meet the steering wheel.
    `Scr_Vehicle_Controller` (0x8c2d2) sets only the body (`+0x300`), the
    turret (`+0x304`), the barrel (`+0x308`) and the gunner's base and gun
    (`+0x318` or `+0x310`, `+0x314`) (VERIFIED (the instructions)): the server never
    turns the steering wheel or the front wheels, which the cgame does. At
    its bind the wheel meets the ray at y 20.27 in the model's frame
    against retail's 20.35 (VERIFIED, `xmodel_ray` example). The server
    poses through `vehiclepose::apply_hull`.
  - The Panzer IV gunner: with the seat blend above every ray matches
    (VERIFIED, replay).

  The jeep passenger (seat 3). His clause is `legs
  c_mp_jeep_passenger_idle torso c_mp_torso_stand_alert` (uo
  `mp/playeranim.script`, the `mounted vehPassenger1, vehicle jeep`
  default), and retail's seat capture (section 8) sends `torsoAnim` 794,
  `c_mp_torso_stand_alert` with the toggle, through the whole seat, and
  `pt_rifle_fire` (318) after a shot (VERIFIED, capture). A `torso` line
  writes its anim and a `both` line's torso half writes 0 (`AnimRef`'s
  `both`); 1.1's and 1.5's continuous states carry no `torso` line, so only
  UO's is affected. Writing 0 to every continuous torso (the 1.1 rule for
  `both`) put his rays up to 30 units off. With the standing torso clip
  over the seated legs every passenger ray is within 1.02 of retail's but
  one cell at 32 above the origin: the torso clip loops over 9 s and its
  phase follows the seat change's timing (INFERRED, replay). Retail's
  target joined 18 s into the level and took the first seat at 29350
  (VERIFIED, fixture). Every retail fixture boards 10 050 ms after the
  target's placement and changes seat 12 000 or 12 050 apart (VERIFIED,
  the six `*-armor*-uo.txt` and `*-silhouette-uo.txt` fixtures); the
  replay's target boards 10 050 after its placement and, in the
  silhouette runs, joins late enough to be placed on retail's level time
  and changes seat at the fixture's own times (`schedule_of`).
  `vehicle_ab`'s `the_jeep_seats_replay_on_ours` compares `torsoAnim`
  through the seat capture; ours releases the fire anim 100 ms (two
  snapshots) after retail, the one torso gap.

  `vehicle_armor_ab`'s silhouette tests compare every seat ray for ray
  within a quarter unit, the stints after the gunner's seat included. The
  paragraphs below are the facts those tests rest on.

### 13.7 The silhouette through seat changes

- **The tank driver.** No clause of `playeranim.script` names a tank's
  driver, and none is needed: his legs play `pb_stand_alert` (185, wire
  697 with the toggle) on retail and ours, the on-foot idle carried into
  the seat (VERIFIED, the tank seats capture's `legsAnim`, section 8;
  `vehicle_ab`). What parts his cells is the turret: the profile 2.7 s
  after boarding catches it still swinging onto the driver's view at
  30°/s, and most rays that pass the driver meet it. Boarding one frame
  early (29300 vs retail's 29350) puts the turret one think (1.5°) further
  round and the cells up to 6.4 off; on retail's frame every one of the
  stint's 88 cells matches to 0.01 (VERIFIED, replay).
- **Back from the gunner's seat.** On retail the tank driver's second
  stint (phase 3, after the gunner's) does not repeat his first: the rider
  cells stand up to 1.1 units apart, growing with height above the origin,
  with the same seat origin, view and vehicle angles (VERIFIED, fixture).
  Measured 2026-10-17 with `probe_bones 1` added to `probe_vehicle_armor`
  (a `script_origin` linked to nine bones and `tag_origin` once the target
  sits, one more linked untagged, logged every frame; retail UO 1.51b,
  `tank`, `jeep` and `foot` runs):
  - The bones the link pass reads match ours to 0.01 in every settled
    frame of every stint, profile frames included, in both vehicles
    (VERIFIED, replay on the run's own seat times). The gunner's seat
    leaves the same body.
  - The rays do not. On the run's own times ours meets every cell of the
    tank's stints 1 and 2 to 0.01 and misses phase 3 by up to 1.18; with
    the rays posed 50 ms earlier (the trace's anim clock one frame back),
    phase 3 and the 12 s on foot after the exit match to 0.00 and phase 1
    goes off by up to 0.45 (VERIFIED, replay). The jeep: on foot before
    boarding the rays match at the frame's own time (0.01) and not a frame
    back (up to 0.26); phases 1 and 2 read the same either way; phase 3 and
    the exit match a frame back (VERIFIED, replay). With no vehicle at all,
    90 s on foot, the rays match at the frame's own time throughout
    (VERIFIED, `mp_peaks-jeep-silhouette-foot-uo.txt`), so it is not the
    client's clock drifting: from some point after the first seat change,
    retail's script trace poses that rider at the last frame's anim time,
    where the link pass (after `G_XAnimUpdate`, game.mp.uo 0x59983, called
    in `G_RunFrame` 0x59c21 after the first `Scr_RunCurrentThreads`) poses
    him at the frame's.
  - The switch is traced in `cod11-turrets.md` 13.7. UO's thread pass runs
    before `G_XAnimUpdate`, so every script trace poses at the last
    frame's anim time; a spawn's `setModel` rebuilds the DObj inside the
    thread, so the spawn's clips run a frame ahead. The two cancel until a
    seat change starts clips on an end frame (INFERRED, read of
    `G_RunFrame` and `G_DObjUpdate`). With both ported every stint and the
    on-foot run match at a quarter unit (VERIFIED, replay).
    `G_XAnimUpdateEnt` (0x5992c) skips only an entity with `flags` 0x4000
    (INFERRED (decompile)). The seat cycle (`Cmd_NextVehSlot_f` 0x4ed9b,
    `G_VEH_CycleSlot` 0x8da1e) calls no DObj time setter (INFERRED, read of
    the decompile).
- **The driver's idle.** A driver boarded by use keeps the legs clip he
  stood in, `pb_stand_alert` (a 6.27 s loop, no `moveSpeed`), at the phase
  it reached on foot: the seat changes no legs anim (`legsAnim` 697 from
  the tank gun run's first snapshot to the `nextvehslot`) (VERIFIED,
  capture). Its sways move his head, chest and hands 0.8 units at a time,
  so the bones in the seat depend on where that clip started.
  - Every retail vehicle fixture that logs both lines has `PROBE
    activated` 2050 ms past `PROBE place` (all seven under
    `fixtures/vehicles/` and `fixtures/seatspawn/`) (VERIFIED, captures).
    The net probe settles 2 s from the placed snapshot (`vehicle_probe.rs`'s
    `settle`) and its use rides the next frame (INFERRED).
  - 697 is the idle with the restart toggle (0x200) on, so an odd number
    of legs sets since the spawn's `gclient_t` memset zeroed it
    (cod15-movement.md 4, player-model-anim-system.md on the toggle)
    (INFERRED): retail's spawn went straight to the idle on its own frame,
    the placement's.
  - A replay that boards 2000 ms past the place, on a seeded spawn on a
    sloped point (-688, -1160) whose 100 ms think slides the driver 30
    units/s (a walk clip at the end frame, the idle a frame later), puts
    the idle 150 ms behind retail's (hands and head worst 0.83). With the
    use at 2050 and a flat `mp_teamdeathmatch_spawn` pinned (64, 152),
    `the_tank_rider_s_bones_follow_retail_s_in_both_seats` reads every
    driver mark to the print (0.00) and every gunner mark within 0.01
    (VERIFIED, replay, 2026-10-20). The lag was the replay's boarding, not
    the server.
  - Every mp_cassino `vehicle_ab` replay spawns on that flat point
    (`FLAT_SPAWNS`), and the gates compare `legsAnim` and `torsoAnim` with
    the toggle: all four tank playerstate captures read retail's toggle on
    every snapshot (VERIFIED, replay, 2026-10-20).
  - On mp_peaks the seeded draw reads it in every jeep capture the gates
    tally but one: `mp_peaks-tdm-slope-8ms-jeep-gun-uo.txt` opens seated
    with 606 where the seeded draw sends 94, one set more. Five retail
    `GUN` runs logged from the spawn (`--slope-from-spawn`, `PROBE
    spawned`, 2026-10-20) settle it (VERIFIED, captures). Spawn and
    placement fall on one frame in all five. Four spawns (tdm spawnpoints
    260, 263, 268, 272) open on foot with 697 and seat with 94. The fifth,
    spawnpoint 277, placed at (-3872, 3064, -25.528)
    (`placebits-retail.txt`): its think left him at (-3872.11, 3064.00,
    -28.18), the snapshot carrying velocity (-12, 0, -3) and `legsAnim`
    640 (128, `pb_crouch_walk_forward_unarmed` in vcod's index, with the
    toggle). The fall ends on the 277 slope and the clip leaves a slide, so
    the think's `PM_Footsteps` picks a walk with empty hands (INFERRED). He
    lands from the placement with 185 and no toggle and seats with 606, the
    old capture's value. A capsule rests 2.65 under the spawnpoint's box
    drop there; the same think on 260 and 272, also sloped, leaves no slide
    (VERIFIED, captures). Ours with 277 pinned leaves the same velocity and
    640 on the spawn frame. On that pin
    `the_jeep_gun_from_a_logged_spawn_replays_on_ours` matches every field
    of `mp_peaks-tdm-slope-8ms-jeep-gun-spawn-uo.txt` from 900 ms into the
    seat, and `the_jeep_gun_replays_on_ours` every field of the old one,
    with no gap (VERIFIED, replay, 2026-10-20). The old capture's own spawn
    is not in evidence: of the map's 32 tdm spawns pinned on it, 277 and
    (-1328, -160) match every field, the other 30 miss only the toggle
    (VERIFIED, replay).
- **The tank driver's link offset.** The seat link (0x8998d) links the
  player with `G_EntLinkToWithOffset` at `BG_GetVehiclePosOffset(type,
  requested slot)` (0x230e5). A use requests slot 0, so a tank driver
  boarded by use is linked with no offset where his seat has (0, 0, -20)
  (`.data` 0xa94f8; the gunner's (10, 5, -39) at 0xa94ec, artillery's (0,
  -24, 0) at 0xa94e0) (VERIFIED decompile, file bytes). `G_RunClient`
  (0x41dfc) writes the linked player's `r.currentOrigin` and `ps.origin`
  from that link every entity pass (`G_SetFixedLink` mode 2), and
  `G_PlayerVehiclePositionAndBlend` puts `ps.origin` back on the offset
  seat at the end frame (INFERRED (decompile)).
  - So through the first stint the driver's entity sits 20 units above his
    `ps.origin`: an untagged link and every bone link to him read 20 high
    (129.08 vs 109.08), and from the first `nextvehslot`, which requests
    the slot it takes, level (VERIFIED, `tank` run; again in the
    2026-10-20 tank gun run, whose driver's marks
    `the_tank_rider_s_bones_follow_retail_s_in_both_seats` gates, 20 high
    through the first stint on both).
  - The relink links at `r.currentOrigin` as written, unsnapped, where
    `ClientThink_real`'s own link takes the snapped origin: `G_RunClient`
    runs `G_SetFixedLink(ent, 2)`, `G_SetOrigin(ent, r.currentOrigin)`,
    `trap_LinkEntity`, then copies `r.currentOrigin` into `ps.origin`, no
    snap on the way (INFERRED (decompile)). Through the first stint `r.absmin`
    sits 20 above the seat's `ps.origin` less the box's unit, and level
    with it once a `nextvehslot` asked for the slot. Linking at the
    truncated origin sat 0.08 low in the first stint, 0.63 back in the
    driver's seat on mp_cassino; `vehicle_ab`'s
    `a_tank_driver_boarded_by_use_sits_in_the_area_tree_at_his_link` pins
    it.
  - Every entity-pass trace (a missile's move, a blast's walk) builds its
    bodies off the sims after his turn, so its broad box and bones sit at
    the link too (INFERRED, read of `server.rs`'s entity pass); a script
    trace or a turret round, before the pass or after the end frame, finds
    him on the seat. The jeep's offsets are all zero and its links read
    level in every stint (VERIFIED, `jeep` run).
  - vcod runs the refresh on the rider's turn of the entity pass, his
    vehicle first (`run_rider_link` in `server/sim_ops.rs`): `r.currentOrigin`
    and `ps.origin` at the seat tag of the vehicle as it stands, shifted by
    the offset of the slot `VEH_LinkPlayer` asked for (the seat's
    `link_offset`), and a link there. The end frame then puts `ps.origin` and
    `r.currentOrigin` on the seat, as retail's does:
    `G_PlayerVehiclePositionAndBlend` copies the offset `ps.origin` into
    `r.currentOrigin` (`ent+0x13c..0x144`, 0x91869..0x91893, after its
    `BG_PlayerStateToEntityState` at 0x91855) and the seat tag's angles into
    `r.currentAngles` (`+0x148`) (VERIFIED (the instructions)). So only what reads
    him inside the entity pass after his turn (a link to him, a blast or round
    on a later entity's turn) sees the link. `G_RunFrame` (0x59c21) runs its
    one `Scr_RunCurrentThreads` ahead of the pass and none between the pass
    and `ClientEndFrame` (INFERRED (decompile)). The probe's threads read
    the first-stint driver at 109.08, his seat
    (`mp_cassino-tank-silhouette-uo.txt`'s `PROBE pose`, VERIFIED).
- **The 400 ms window.** The 400 ms after a seat change: retail's bones
  glide to the new seat over eight frames (VERIFIED, `tank` run). The
  setter (UO 0x1ce5f, 1.5b 0x28ced), when the legs anim changes crouch or
  prone class (`BG_IsCrouchingAnim` 0x1cdac, movetype mask 0xc4;
  `BG_IsProneAnim` 0x1ce04, 0x308), stores `bg.time + 400` in the client
  info (`ci+0x4c0`, 1.5b `ci+0x4a4`), and every later blend of either
  channel is at least what is left of that. Its other floors are 1.1's,
  without 1.1's 200/250 stance rule (INFERRED (decompile)). 1.1d has no such
  window (no `bg.time + 400` store in the `game.mp.i386.so` decompile,
  INFERRED (decompile)). The gunner's `standMG42` legs are of another class
  than a seat's, so a change in or out of seat 2 blends over 400 ms. vcod
  ports it (`animclock::Flavour::Server15`, the server's 1.5
  and UO end frames); without it ours arrived in four frames, up to 6
  units apart on the way. Measured 2026-10-17 (retail UO 1.51b, `tank`
  run, `probe_bones 1`): the change into the gunner's seat matches
  retail's every frame within 0.21, but for one frame where retail repeats
  the last pose (41850).
- **The glide back.** The change back into the driver's seat also takes
  eight frames on both (VERIFIED, replay of the `tank` run, 2026-10-18).
  The fading clip is the gunner's `standMG42_aim` (wire 95), an aim group
  of three leaves. In the gunner's seat `G_PlayerVehiclePositionAndBlend`
  set its leaf `pb_standMG42gunner_aim_forward_level` (node 261) to weight
  1; the driver's seat sets goals under its own legs anim only, so nothing
  clears the old node's leaves and the clip fades out on them (INFERRED,
  read of 0x9048f against the replay). Dropping the seat blend at the
  change and descending the group by the view's pitch (10°) to its
  `_15down` leaf dipped `bip01 r hand` up to 6.55 below retail's path.
  `playerpose::SeatRamp` keeps the node it leaves and
  `PoseInputs::seat_leaves` lists it after the current seat's; the
  client's drawn rider keeps it the same way (`entities.rs` `SeatLeaves`).
  - With it every bone of the glide sits within 0.02 of retail's
    (VERIFIED, replay,
    `vehicle_armor_ab::the_tank_rider_glides_back_into_the_drivers_seat_as_on_retail`,
    fixture `mp_cassino-tank-glide-uo.txt`, cut from that run and since
    recut from the 2026-10-21 recapture).
  - 54100 and 54250, where retail repeats the last frame's pose: a
    `bulletTrace` in the first thread pass (the profile's at 54050, the
    aim sweep's at 54200) poses the body under the frame's DObj stamp and
    the link pass reads those bones (`cod11-movers.md` 15,
    cod11-combat.md 3.6, `probe_stamp`); vcod's script trace does it
    through `GameHost::dobj_posed`. The gate connects its shooter as late
    as the recipe's (placed at 23200 on both), since the sweep's
    half-second clock runs from his placement; joined with the target the
    sweep ran at 54150 and its repeat landed a frame early (VERIFIED,
    replay, 2026-10-18).
  - 53900, every bone 21 units high for one frame: the probe's marks link
    to the rider, and `G_RunClient` (0x41dfc) re-reads a linked player's
    `r.currentOrigin` off his link each entity pass ("The tank driver's
    link offset"). Without the refresh the gate misses every bone at 53900
    by 21.00 in z (VERIFIED, replay).
  - 54000, `bip01 head` 0.34 off: the mark reads the pose of 53900.
    VERIFIED, vcod measurement (the glide gate with the rider's eased neck
    control overwritten at 53900): neck (3, 27) puts the head on retail's
    (4690.53, -1979.62, 162.44); (3, 18) reads 0.34 off, (0, 27) 0.1, (6,
    27) 0.13, (3, 31) 0.16. So retail's neck was on its target (0.3 of the
    90° head turn) a frame after the change, two or more 18° ease steps
    where one step falls short.
  - The ease. VERIFIED, `game.mp.uo.i386.so`: `BG_Player_DoControllers`
    (0x1f2fe) has one caller, the static 0x42c26, which `ClientEndFrame`
    stores at `ent+0x230` (0x43510) before `G_UpdateClientInfo`. The hook's
    only readers are `G_DObjCalcPose` (0x78f4f) and `G_DObjCalcBone`
    (0x78fe2), each of which runs it between `trap_DObjCalcAnim` and
    `trap_DObjCalcSkel` unless `trap_DObjCreateSkelForBones` or
    `trap_DObjCreateSkelForBone` (syscall 0x6e) returns non-zero. INFERRED,
    off 1.1d's engine case for that syscall (`cod11-movers.md` 15, "A bone
    asked for before the anim update"; UO's engine not read): it returns
    zero when a bone asked for is not yet built under the frame's stamp.
    The ease step is `0.36 * bg.frametime`, and `G_RunFrame` sets
    `bg.frametime` to `level.frametime` (0x59c21), so it is 18° a run
    however often the hook runs (INFERRED (decompile)). INFERRED, off the
    same: the controls step once for every build that finds a bone
    missing, not once a frame, and a bone keeps the controls of the build
    that first posed it. The probe's ten marks each ask for their own tag
    in the entity pass (pelvis, spine, spine2, neck, head, ...), each a new
    chain, so a frame that changes the target eases it up to ten times;
    with every bone built once a frame (one script trace) it eases once,
    and a frame with no build eases nothing.
  - vcod: `combat::DobjStamps` keeps, for each client and frame, each
    bone's stamp and build and the controls each build eased to; each build
    eases a step on from the last build's controls toward the end frame's
    target, and the last build's controls go to the record at the slot's
    next end frame (`DobjStamps::record`, `ClientSim::keep_eased`). Every
    bone of the glide sits within 0.02 of retail's (VERIFIED, replay; no
    gap list). The engine's own traces (rounds in a cmd, turret rounds,
    missiles, blasts, the lookat trace) stamp their builds too, each client
    packet starts a stamp, the record persists across stamps and the end
    frame eases nothing (cod11-combat.md 3.7, cod11-movers.md 15).
- **Two runs that part.** A fresh retail jeep run (seat changes at 41550
  and 53550) seemed to meet the passenger's stint up to 1.07 off ours,
  where the committed fixture (changes at 41500 and 53500) matches at
  0.00. The fresh run's target was placed 50 ms later (19350 vs 19300),
  and a rider's clips run on level time; joined 50 ms later, every seat's
  rays meet retail's within a quarter unit (VERIFIED, replay, 2026-10-18;
  on the committed join one cell reads 0.26 off). `ARMOR_FIXTURE` shifts
  the join by the capture's placement. That run's bones then parted
  inside the glides by up to 12 units while boarding (ours a frame ahead),
  4.3 into the gunner's seat (ours a frame behind while the gun swings
  toward the view, 18° a frame), 6.5 into the passenger's (VERIFIED,
  replay). Placing the gunner off the vehicle's post-step controls closed
  the gunner's to 0.16 but broke `vehicle_ab`'s seated origins, so the lag
  was in what his marks read, not his placement (INFERRED). The boarding
  and gunner's glides are closed ("The glides on the jeep").
- **The glides on the jeep.** A fresh retail jeep run (2026-10-21,
  `probe_bones 1`, seat changes at 29750, 41900, 53900, out at 65900) cut
  to every glide and the exit's fall was the first
  `mp_peaks-jeep-glide-uo.txt`, the tank's exit from the same day the
  first `mp_cassino-tank-exit-uo.txt` (both recaptured since, "Retail's
  spawn point"). Both keep the two net probes' `CMDS` lines, which
  `vehicle_armor_ab` replays as each client's cmd times
  (`Capture::cmds`). Against them (VERIFIED, replay,
  `the_jeep_rider_glides_into_each_seat_and_falls_out_as_on_retail`,
  `the_tank_driver_idles_and_falls_off_the_hull_as_on_retail`):
  - Boarding: the 12 units, ours a frame ahead, was the seat's legs set a
    frame early (11.4, `vehMotion`); with the motion held every bone of the
    glide sits within the print's 0.01.
  - The gunner's seat glides within 0.02 on every frame since
    `G_RunClient`'s seat link refresh.
  - The exit's fall (once 0.3 to 1.5 off in z on the tank) was the
    replay's cmd split: 16 ms cmds up to the next frame's time, where
    retail's 60 Hz client had run through 33 ms past the snapshot (ct
    65933 at 65950). On the capture's own cmd times both bodies fall frame
    for frame to the print and land on retail's spot (jeep x -4048.06).
  - Not gated: the passenger's 55400 and 55450 read the body turned 23
    units from ours; retail's net probe stalled there (no cmd ran from
    55250 to 55416, its `ct=` pairs), and the harness sends cmds through a
    stall.
- **The weapon-swap frame.** The first frame after each change that hides
  or shows the rider's own weapon parted from ours in the spine and up,
  legs and pelvis matching: the boarding 29800 (2.36 on `bip01 l hand`),
  the passenger's seat 53950 (2.64 on the head), the jeep's exit 65950
  (25.9 on the left hand) and 66000 (0.86), the tank's exit 65750 and 65800
  (0.95, 0.87). A probe line at T reads the marks the entity pass of T - 50
  placed, so the parting frame is the change's own: its cmds have changed
  the seat, its end frame has not yet run. The cause is the hook's inputs,
  not the DObj.
  - INFERRED (decompile), `game.mp.uo.i386.so`: the hook (0x1e761) reads off
    the entity whether he rides (`s.eFlags` 0x100000), the seat and kind
    (`s+0x88`) and the weapon (`s+0xcc`, through
    `BG_AllowPlayerWeaponAtVehiclePos`), and off the record the view
    (`+0x3e8`), the swings (`+0x380`, `+0x3b0`, `+0x3b8`) and the seat tag
    angles (`+0x3f4`).
  - INFERRED (decompile): `ClientThink_real` (0x40c91) runs
    `BG_PlayerStateToEntityState` after each cmd's `Pmove` and before its
    `Cmd_Activate_f`, and `SetClientOrigin` (0x44210) runs it too.
    `VEH_LinkPlayer` (0x8998d) calls `SetClientOrigin` ahead of `ps.eFlags
    |= 0x300000` and `vehPos`; `VEH_UnlinkPlayer` (0x89fb2) clears
    `ps.eFlags` 0x700000 ahead of its voluntary exit's `SetClientOrigin`.
  - INFERRED (decompile): `ClientConnect` (0x44ab4) zeroes the record, whose
    `+0x3f4` the end frame's `G_PlayerVehiclePositionAndBlend` (0x9048f)
    writes. INFERRED, no other writer in the decompile: a first boarding
    reads seat angles of zero until its end frame, and an exit keeps the
    last seat's.
  - INFERRED (decompile): the angle updater (0x1dbac) on a rider (`s.eFlags &
    0x106000`) sets the torso and legs yaw to the view's and swings the
    torso pitch to 0. Keeping 1.1's on-foot pitch for a rider let the exit
    frame's on-foot hook split the spine.
  - INFERRED (decompile): `BG_UpdatePlayerDObj` (0x1f556) writes only the
    record's `+0x4c8`, `+0x404` and `+0x4cc`, so the rebuild keeps the
    eased controls (`+0x408`).
  - vcod: `ClientSim::write_entity` after each cmd's move, in `enter_seat`
    ahead of the seat, in a voluntary `leave_seat` past the clear, and at
    the end frame; `BodyPose::es` and `ci_seat_angles` feed
    `BodyPose::target_controllers`; `BodyAngles::step` treats a UO rider
    (`EF_IN_VEHICLE_V22`) as mounted. Every bone of both glide fixtures
    sits within the print's 0.01 of retail's, these frames included, with
    no gap list (VERIFIED, replay).
- **The angle updater on a rider.** The UO cgame's updater
  (`uo_cgame_mp_x86.dll` 0x30004550, 1.1's 0x300040e0) tests `s.eFlags &
  0x106000` at the same three places as the server's (0x1dbac): it sets the
  torso yaw, pitch and legs flags (`+0x3b4`, `+0x3bc`, `+0x384`) at
  0x30004596, sets torso and legs yaw (`+0x3b0`, `+0x380`) to the view's at
  0x300047c7 and swings the torso pitch to 0 at 0x300047f6 (VERIFIED (the instructions)). Both copies skip the flag setting for a rider whose seat
  allows his weapon: the cgame tests `s+0x88` `& 0x38 == 8` and `& 7 == 3`
  inline (0x300045b7), the server calls `BG_AllowPlayerWeaponAtVehiclePos`
  (0x23148) on the same bits (VERIFIED (the instructions) and decompile). For
  that seat with `s+0xcc` (the weapon) non-zero, both swing the legs toward
  the record's `+0x3f8`, the seat tag's yaw, instead of the view's (cgame
  0x300046c6..0x300046f2, VERIFIED (the instructions); server 0x1dbac, VERIFIED (the instructions)). The yaw overwrite that follows hides both from the angles;
  they show only in the legs' flag, which the exit frame's on-foot swing
  reads (INFERRED, control flow: a set flag swings the legs at tolerance 0,
  a clear one at 40). 1.1's cgame (0x30004126) and 1.5's (`cgame_mp15.dll`
  0x30004376) test `eFlags & 0xc000`, the turret bits only, with no seat
  test (VERIFIED (the instructions)), so a 1.1 or 1.5 gunner's updater is the
  server's. vcod: `BodyInput::seat_weapon` (`SeatWeapon`) carries the seat
  test on both sides; reading the flag branch off `EF_MOUNTED` alone kept
  the on-foot flags in every seat.
- **The tank driver's idle.** In the 2026-10-21 tank run the driver's
  first stint sat a steady 100 ms of clip off ours (up to 1.68 on the
  hands, 1.47 on one silhouette cell with that capture as
  `ARMOR_FIXTURE`). VERIFIED, vcod trace: the driver keeps his on-foot legs
  anim in the seat (no legs set from the spawn to the gunner's seat at
  41700), so its phase runs from the spawn; ours, alone at the spawn, took
  the script's pick, whose think started legs 128, then 157, and the idle
  at 19600. VERIFIED, replay: with his spawn pinned on the spot the probe
  then places him (idle from the spawn frame) every bone of the whole
  local capture (29550 to 76000) sits within 0.01 of retail's and the
  silhouette cell matches.

### 13.8 The silhouette run's spawn, trigger and times

- **Retail's spawn point.** The probe logs each spawn's pick: `log_spawns`
  wakes on `"spawned"` after tdm's `spawnPlayer` returns and prints the
  origin and angles of the `mp_teamdeathmatch_spawn` nearest the body
  (`PROBE spawn <time> <client> <origin> <angles>`). Recaptured on retail
  UO 1.51b, 2026-10-21, each fixture's recipe in its header: the target's
  picks were real tdm points, nowhere near the placement (VERIFIED, the
  four captures):

  | Map, run | Spawn | Yaw |
  |---|---|---|
  | mp_cassino, silhouette | (-688, -1160, 75.80) | 0 |
  | mp_cassino, bones | (2688, -160, 320.12) | 180 |
  | mp_peaks | (-696, 2456, 304.12) | 270 |
  | mp_peaks | (-1808, 1072, 298.12) | 90 |

  `vehicle_armor_ab` pins the logged point (`logged_spawn`) for every
  capture it replays on retail's clock. Pinned there, the tank driver's
  first stint (29850 to 30750) and his exit (65250 to 65850) match every
  bone within 0.021 (VERIFIED, replay,
  `the_tank_driver_idles_and_falls_off_the_hull_as_on_retail`). On the
  script's own pick the stint misses from its first sample (0.68 on the
  right hand at 29950; VERIFIED, replay). So retail's think on its pick
  started the idle at the spawn frame, as one on the placement spot does
  (VERIFIED, replay), which is why the earlier fit held. The jeep's gates
  pass pinned or not (VERIFIED, replay). The boarding sets the jeep
  rider's legs (11.4), so his spawn's clip never reaches a seat (INFERRED).
- **The shooter's trigger.** The net probe's `--probe-uo-weapon aim` first
  presses attack and sight on its first cmd 1000 ms or more past its
  placement, in all four runs and the two earlier bones runs (VERIFIED, the
  `CMDS` lines' `in=`: placed 23650, pressed 24650 on the mp_peaks bones
  run). Holding the trigger from the placement put ours' rounds off
  retail's frames: on the jeep exit hits at 65950 and 66150 vs retail's
  65900 and 66100 (VERIFIED, replay). Each round's trace builds the
  rider's DObj (combat doc 3.6), and the landing frame 66000 read up to
  0.51 off (`bip01 l hand`; VERIFIED, replay). With the shooter's buttons
  taken off the capture's `in=` cmd by cmd (`common::cmds_inputs`) every
  bone matches (VERIFIED, replay). The silhouette fixtures keep both
  probes' `CMDS` lines and their tests replay them with the shooter joined
  as late as retail's; joined beside the target and holding from his
  placement, one jeep cell, (12, 6) on the passenger at 56050, met him
  where retail's ray missed (VERIFIED, replay).
- **The committed recapture's times.** The frames named above for the
  glides and the stint are the earlier runs'. The committed
  `mp_cassino-tank-silhouette-uo.txt`, `-tank-glide-uo.txt`,
  `-tank-exit-uo.txt`, `mp_peaks-jeep-silhouette-uo.txt` and
  `-jeep-glide-uo.txt` are the recapture's: tank seats at
  29050/41200/53200, out 65200 (silhouette) and 29100/41250/53250, out
  65250 (bones); jeep 29650/41800/53800, out 65800 and 29700/41850/53850,
  out 65850. The gates join on each fixture's placement (`late_for`,
  `LATE_MS` set on a placement at 19300).

### 13.9 Rounds and shells on a hull

- **The inflictor swap.** `Bullet_Fire_Extended` (0x7b13c..0x7b1ae) uses
  the given inflictor, else the attacker; when that is the attacker, a
  client whose entity `eFlags` has 0x100000 and whose `vehPos`
  (`client+0x614`) is not 3, it takes `g_entities[r.ownerNum]`, his
  vehicle (VERIFIED (the instructions)). Only the vehicle guns fire a seat 1 or 2
  rider's rounds, and vcod names the vehicle there
  (`server::vehicle_fire`); a seat 3 passenger's own rounds name him.
- **The barrel test.** The driver's reader (0x86c57, its `trap_Trace` at
  0x870e9) traces from `tag_barrel` to `tag_flash` (rec `+0x308`,
  `+0x324`) with no box, passing the vehicle, under the vehicle's clipmask
  less `CONTENTS_BODY`. Clear, it runs the aim's locational trace; meeting
  anything, it sets `rec+0x270` and keeps the far point.
  `Scr_Vehicle_Think` clears the flag (0x8ca55). With it set `fireTurret`
  (0x8f33c), `fireAltTurret` (0x8fa79) and `fireGunner` (0x8ffaf) bend
  nothing and start the round at their flash less `rec+0x26c` along it,
  the `tag_flash` to `tag_barrel` distance `Scr_Vehicle_Init` measured
  (0x8c6e6) (VERIFIED (the instructions)). vcod ports it (`fire::aim_point`,
  `fire::prepare`), measuring the distance on the current pose.
- **A hull stops rounds.** `SV_ClipMoveToEntity` (`coduo_lnxded`
  0x809a72a) sends a locational trace on an entity with a DObj and
  `svFlags & 6` to the model arm: the model's collision contents against
  the mask (0x80c7d32), its bounds (0x80c6b36), vmMain 15 posing the
  entity, then 0x80c7be8, which walks every part's collision surfaces
  (0x80c4892), each in its bone's posed frame, a one-sided segment test
  per triangle (INFERRED (decompile)). A vehicle has `svFlags` 4 and
  contents 0xa00000, which every bullet (0x2802031), missile (0x2802091)
  and blast probe (0x802091) mask meets, so the Panzer's turret surfaces
  stop rounds where the turret points. The result carries the entity's own
  contents (no glass, so the round ends), the struck surface's flags (13,
  metal, on the stock hulls) and hit location 0 (INFERRED (decompile)). The
  pass rule skips the pass entity, what it owns and what its owner owns
  (0x809a72a): a vehicle's own round (`fireTurret` passes the vehicle)
  skips its hull and riders, owned by it; a rider's own round skips the
  other riders but not the hull.
- **Bullets on a hull.** `Bullet_Fire_Extended` draws the shooter's impact
  (178 with the surface, the weapon and the reflection) and runs
  `G_Damage` on the vehicle with the leg's falloff damage, the shot's
  forward and the hit point (`coduo-diff.md` 21) (INFERRED (decompile)).
  Measured, retail UO 1.51b, `client-probes/probe_vehicle_hits` (fixtures
  `*-jeep-hits-{side,front,rear}-uo.txt`,
  `mp_cassino-tank-hits-side-uo.txt`) (VERIFIED, live): a `thompson_mp` 250
  off an empty Willys's flank lands 22 a round (50 at `dmgScaleSide` 90 and
  `dmgScaleBullet` 50, truncated), 25 from the front, 18 from behind (18.75
  truncated), `MOD_PISTOL_BULLET`, the shooter as both attacker and
  inflictor; a Panzer IV stops the `bulletTrace` and the rounds on its side
  and takes none (`bulletDamage` 0). vcod poses the hull per bone
  (`vehiclepose::posed_collision`) for every shot, swing, missile step and
  blast probe, and `vehicle_hits_ab` matches all four within a fifth of
  retail's notify count with retail's amounts.
- **The shooter's phase.** VERIFIED, vcod measurement, 2026-11-13 (24
  `test_seed_rng` seeds at the UO install): with `vehicle_hits_ab` firing
  from the first frame after the placement on even cmds, ours landed
  front 82.3 notifies against retail's 70, rear 69.0 against 58, side 56.1
  against 65, and its hits sat 3 to 7 units higher on every side; the rear
  passed or failed on the RNG stream. Three harness differences from the
  captures' recipe, each VERIFIED off the capture or the probe:
  - the net probe's `hold` script settles 1000 ms from its spawn before it
    sights and fires (`vehicle_probe::HOLD`); retail's first notify is
    1050-1100 ms after the placement on all three captures, ours was 50;
  - its cmds run on the 60 Hz grid (`ProbeHold::sixty`, cod15-movement.md
    6), under which the stock kick settles between rounds;
  - the gsc's `setPlayerAngles` each frame moves `ps.viewangles` to its
    aim, and the aim block of the next cmd reads that view before the cmd
    rebases back to the probe's held one (`ClientSim::update_aim`, combat
    doc 15), so the first cmd after a frame aims along the gsc's angle and
    the other two along the held view. The thompson fires every sixth cmd,
    on the phase its press fell on. Ours pressed on the first cmd of a
    frame's batch and every round left 3.2 degrees above the held view
    (pitch 3.4 against 6.6 on the side), over the Willys's door cutout.
    Retail's side hits sit at z -79.6, the held view's line at the near
    panel -79.8, the gsc's -66. INFERRED: retail's press fell on a later
    cmd of its frame.
  With the three in the gate (the press on its batch's second cmd), ours
  reads front 70 (retail 70), rear 53-66 (58), side 69-70 (65) over the
  seeds, and the hits' mean z front -70.8 (-70.9), side -80.1 (-79.6),
  rear -63.9 (-66.9). INFERRED: the earlier offsets were the harness's,
  not the server's.
- **A shell on a hull.** `G_MissileImpact` (0x5db67) runs `G_Damage` on a
  struck entity that takes damage: `Damage_Falloff` of the shell's flight
  distance, truncated (0x5de9d), the flight velocity as the direction, the
  contact as the point, dflags 0, the shell's means of death, the driver
  as the attacker and the vehicle as the inflictor for a vehicle's shell.
  The splash then skips that entity. The explode is 186, not 185, when the
  trace named a player or a bone (`trace+42`, a hull's surface), with
  0x100000 on a vehicle (INFERRED (decompile)). Measured, retail UO 1.51b,
  `client-probes/probe_vehicle_shell` on mp_kharkov (fixtures
  `mp_kharkov-tank-shell-uo.txt`, `*-uo-shellveh.bin`): a Panzer IV's three
  shells on the Elefant 276 units off each give it one notify, 279
  `MOD_PROJECTILE` (700 through its side scale), attacker the driver,
  inflictor the tank, and an explode 186 with `eFlags` 0x100000 on metal
  (13) (VERIFIED, live). `vehicle_shell_ab` matches every field.
- **A strike on a body.** `VEH_PlayerCollision` (0x8213f): a body a probe
  strikes is knocked away and, at most every 500 ms, takes `150 min(speed
  / 400, 1)` under `MOD_CRUSH_JEEP` or `MOD_CRUSH_TANK`, the driver the
  attacker (VERIFIED (the instructions)). The 500 ms runs off `client+0x4728`,
  which every strike past it restarts: `VEH_PlayerDamage` (0x81ffd) runs
  on 0 damage too (a vehicle moving away from him), through the damage
  callback (VERIFIED (the instructions)). Then, unless `ps.pm_flags` has 1, the
  file's `vehicle_hit_person` alias (`info+0x33b`) plays at the body's
  `r.currentOrigin`, `1500 (1 - min(speed / 400, 1))` up, broadcast
  (`svFlags |= 8`) (VERIFIED (the instructions), constants 0xa6b68 0.0, 0xa6b70
  1500.0 and 0xa6b50 1.0). Bit 1 is the prone stance: a prone body's strike
  plays no sound (VERIFIED, the run-over captures of 11.6).

## 14. What vcod-server does, and what it measured

- **Code.** `crate::game::vehicle` (`info`, `physics`, `seat`, `think`,
  `fire`, `die`) and `game::builtins::vehicle` (the 31 methods,
  `precacheVehicle`, `getNumVehicles`, `doDamage`,
  `doDamageMod`, `spawnDuplicate`, `linkIntoWorld`, `unlinkFromWorld`,
  `verifyPosition`, `clearVehiclePosition`). A tank shell is a
  `crate::game::missile` shell with the vehicle as its inflictor.
- **`configstrings_uo_ab`.** The whole configstring table against
  `gamestate-uo.bin` (vehicles included: models, aliases, effects, hint
  strings, configstring 8), and the dm tanks' baselines 176 and 181 field
  by field (VERIFIED).
- **`vehicle_ab`.** Two retail captures, `--probe-vehicle drive
  --save-slope` (`mp_cassino-tdm-slope-8ms-tank-drive-uo.txt`, a Panzer
  IV; `mp_peaks-tdm-slope-8ms-jeep-drive-uo.txt`, a Willys; each snapshot
  line carries the vehicle's whole entity state), and the seat captures of
  section 8. The replay boards the same vehicle on ours through the probe
  gametype and the use key, sends the capture's cmds on our clock (rebased
  once a seat from retail's `delta_angles` onto ours, with `nextvehslot`
  where the capture's seat changes) and diffs every snapshot (VERIFIED,
  replay):
  - The vehicle matches on every field and snapshot of both drives: pose,
    sway bytes, turret, steer, track clock, seat mask, events (main gun,
    coaxial MG, the turret's stop alias), `loopSound`.
  - The rider matches in seat, view, pm flags, legs, origin (the exits'
    rise and fall included, since the cmds after an exit run linked,
    11.2), velocity, weapon state and cone, the pitch clamp's and the jeep
    passenger's arcs' per-cmd `delta_angles` pushes included. Trap:
    rebasing every cmd on the newest `delta_angles` puts each push a
    snapshot apart and opens the cone on the jump.
  - The replay sends `nextvehslot` in its own packet ahead of the held run
    of cmds that ends the change's frame (11.3); every seat change's view
    and cone match.
  - The driver's feedback from the tank's own shell matches (13, "A blast
    on a hull"). `delta_angles` itself is not compared: its absolute value
    rides the replaying client's own cmd angles.
  - The jeep driver's legs in the four snapshots after the exit read
    `pb_stand_alert` (185) on retail, airborne. UO's `PM_Footsteps`
    (0x297e4) returns early for an airborne body only when `pm_type` is not
    1, and takes the stance's idle arm for `pm_type` 1, so the exit's
    linked cmds stand the rider up (INFERRED (decompile); 1.5's 0x24ae8 has
    the same test, 1.1's 0x322c8 returns for any airborne body). vcod
    selects the idle for a linked player under 1.5's and UO's rules
    (VERIFIED, replay).
- **`vehicle_drive_ab`** replays the step alone on the collision world
  against the two drive captures. Run free from the first snapshot it
  matches retail on every compared field through both drives, the clip
  brush bounces included, hull yaw, turret yaw, sway bytes and track clock
  bit for bit (VERIFIED, replay). Anchored (each frame stepped from
  retail's wire pose, with the free run's state for what the wire does not
  carry: vertical velocity, the sway's velocities, a hit's lockout and
  bounce), it matches every row too, with no gaps (VERIFIED, replay).
  Rebuilding that state off the snapshots' displacement breaks on the
  crest, the clip brush bounces and the probe hits.
- **`vehicle_fire_ab`.** The Panzer IV's guns on ours against
  `*-uo-tank.bin` (VERIFIED, replay and capture):
  - Retail's two main-gun shells explode as 185 on `eType` 0 with type 6 in
    `es.scale`, weapon 42 (`panzeriv_turret_mp`), `eFlags` 0x8000 and no
    vehicle bit. The second, fired at 31100, is first sent already exploded
    (entity 164, `eventSequence` 1): it struck within its first step. So is
    ours, fired from the probe's seat into the wall 500 units ahead. The
    client plays that explode only because retail's `CG_ResetEntity`
    replays an entering missile's or general entity's ring from 0
    (`cod11-events-and-fx.md` 1, "An entity that enters the snapshot"); a
    tracker that skips it plays the fire sound with no explosion.
  - Along retail's first shell's line (its in-flight `trBase` to its
    explode) ours stops on the face retail's struck, (3363.5, -2709.9, 89)
    on mp_cassino, with retail's normal (byte 32) and surface (16,
    plaster). That face is a plaster side of a caulk brush; the trace takes
    the struck side's material (`bsp-ibsp59-format.md`, lump 3), so the
    client draws `tank_plaster.efx`, not `tank_generic.efx`.
  - The coaxial (164) and gunner (165) rounds that strike no player carry
    their weapon with bit 7, `0x9b` and `0xa0` on both sides, and neither
    side sends a bullet-hit temp entity for them: the client traces them
    (`fx::client_bullet`, coduo-diff.md 19).
  - A shell on a vehicle: `G_MissileImpact` (game.mp.uo 0x5db67) ors
    0x100000 into the explode's `eFlags` when the struck entity's `eType`
    is 12 (INFERRED (decompile)), which turns the client to the impact row's
    `vehicle` column. vcod's shell names the entity its trace stopped on (a
    vehicle's collmap names its vehicle) and the entity pass marks the
    explode when that is a live vehicle: `vehicle_shell_ab` (a shell) and
    `vehicle_blast_ab` (a bazooka round) match retail's 186 with 0x100000
    (section 13).
  - The landing surface off the tank (section 7) is the same per-side
    material.
- **Riders.** Every damage path into a player's callback goes through
  `Vehicles::occupant_damage`, the rider branch of section 13: shots,
  swings, blasts, the `radiusDamage` walk, crushes, shells, `doDamage`. A
  bot never picks a tank's driver as its enemy, since no client is sent
  his body. `vehicle_armor_ab` replays the probe on ours: under the hold
  stream the tank driver's two stints take no round and every other seat
  some; under the sweep each seat's rounds by height are within retail's,
  and the silhouettes match ray for ray (section 13, VERIFIED, replay).
- **Rockets and frags.** Section 13's measured blasts (`vehicle_blast_ab`).
- **Hulls, shells, pushes.** Section 13's hull (`vehicle_hits_ab`,
  `vehicle_armor_ab` with the probe client's cmd stream), shell
  (`vehicle_shell_ab`) and section 12's push (`vehicle_ram_ab`).
- **The rest of the script surface.** `closespeed`, `harc`,
  `getWheelSurface`, the path methods and `freeVehicle`: section 17.
- **The seat blend's ramp.** `G_PlayerVehiclePositionAndBlend` (0x9048f)
  clears the legs anim's tree with `trap_XAnimClearTreeGoalWeightsStrict`
  and blend time 0, then sets each goal weight at the rate `1 / (10
  |weight - goal|)` (0xa7b28 is 10.0, the weight read back with
  `trap_XAnimGetWeight` just before), and the group node's goal 1 the same
  way (INFERRED (decompile)). The weights are VERIFIED file bytes. The setter stores `|goal - weight|`
  times the rate as the goal time (`cod11-movers.md` 15, "The weight
  ramp"), so every weight runs to its goal in a straight line over 100 ms
  from where the frame found it, half way by the next frame's step, and a
  leaf the frame no longer sets lands on 0 at that step (INFERRED, off the
  setter and the step). A node just entered is no exception: its leaves
  were zero, so each rises from a thousandth of its goal over the same
  100 ms (0x080c05a2 raises a zero weight to `0.001 * goal` after storing
  the goal time), and a steer change on the next frame ramps from about
  half of each (INFERRED, off the setter).
  - The node a legs change leaves keeps its leaves' goals. UO's legs setter
    (game.mp.uo 0x1ce5f) clears the old anim with
    `trap_XAnimClearGoalWeight` (0x74c62, syscall 0x57), which
    `coduo_lnxded`'s dispatcher sends to 0x080bfc22, the node alone; the
    recursive clear 0x080bfd0c is reached only from syscalls 0x56 and 0x58
    (`trap_XAnimClearTreeGoalWeights`, `...Strict`) (INFERRED (decompile)).
  - The weight step 0x080bcc1e descends into a node's children only while
    its weight or goal is non-zero; once both read 0 it runs 0x080bca32 on
    each child instead, which sets the weight to the goal and the goal time
    to 0 and frees a leaf whose weight, goal and other half's weight and
    goal all read 0 (INFERRED (decompile)). The time step skips a node with
    no weight (`cod11-combat.md` 3.1). So once the old legs node fades out
    its seat leaves sit on their last goals with their times stopped where
    the last weighted step left them, and a return to that legs anim, any
    time later, ramps the leaves from those goals and steps the times on
    from there (INFERRED, off those).
  - The walk normalizes a node's weighted children (`cod11-movers.md` 15,
    "Torso over legs", read on 1.1's `cod_lnxded` 0x080bbea8), so only the
    leaves' shares count, however small the weights (INFERRED for UO).
  - vcod: `playerpose::SeatRamp` keeps the server's weights across end
    frames, the node last left's with them (`SeatRamp::fade_held` stills
    it), and `layer_clips` normalizes the leaves under the legs layer's
    weight. It matters only to a four-wheel's driver and passenger, whose
    middle and end children share the steer.

  Measured on retail UO 1.51b, 2026-10-15 (fixtures
  `vehicles/mp_peaks-jeep-bones-uo.txt` and
  `playerstate/mp_peaks-tdm-slope-8ms-jeep-bones-uo.txt`, recipe in the
  first's header): `client-probes/probe_vehicle` with `probe_bones 1` links
  a `script_origin` to five of client 0's bones (both hands, head, `bip01
  spine2`, left foot; UO has no `getTagOrigin`) and logs them every frame
  while a `--probe-vehicle drive` client drives the Willys. The legs anim
  toggles between `vehicleJeepDriverDriving` and `vehicleJeepDriverIdle`
  every 200 to 300 ms through the swings and sits on one for seconds
  between them (VERIFIED, replay). `vehicle_ab`'s
  `the_jeep_driver_s_bones_follow_retail_s` replays the drive on ours: past
  the links' first frame the 2170 samples sit within retail's print (0.01)
  but one at 0.02 (17550, a hand), mean 0.000 (VERIFIED, replay,
  2026-10-21). Each part was measured by taking it out (VERIFIED, replay):

  | Removed | Worst |
  |---|---|
  | the left node's kept leaves (entered at its goals) | 0.55 at 25150 (hands, after the legs return to the idle node left 5 s earlier) |
  | a faded node's leaves dropped | 0.54 |
  | leaves kept with their times running | 0.10 at 29300 (head, `spine2`) |
  | normalization (ramps from a thousandth) | 3.81 |
  | the 100 ms ramp | 0.9 in the frames after each steer change |
- **The controllers.** 1.5 and UO ease every body's controls toward their
  targets; 1.1 sets them. game.mp.uo's `BG_Player_DoControllers` (0x1f2fe)
  and 1.5b's (`game.mp.i386.so` 1.5b, 0x1ad71) compute the targets
  (0x1e761, 0x1a389), then step each of the six spine controls and
  `tag_origin`'s angles through 0x1f107 (1.5b 0x1ab82) by at most `0.36 *
  bg.frametime` degrees a component, toward the target with no wrap, and
  `tag_origin`'s translation through `BG_LerpOffset` (0x1f1e7, 1.5b
  0x1ac62) by at most `0.1 * bg.frametime` units along the line, into the
  client info's `+0x408..+0x468` (1.5b `+0x3fc..+0x45c`) (VERIFIED (the instructions); 0.36 and 0.1 at `.rodata` 0x9974c and 0x99750, 1.5b 0x73dcc
  and 0x73dd0, VERIFIED file bytes). `bg.frametime` is `level.frametime`,
  set in `G_RunFrame` (0x59c21) (VERIFIED (the instructions)). The cgames do the
  same per drawn frame on `cg.frametime`: UO 0x30005730 through 0x300055b0
  and 0x30005670 (0.36 at `0x3007c06c`, 0.1 at `0x3007bf6c`), 1.5
  0x300051c0 through 0x30005040 and 0x30005100 (0.36 at `0x3006b8c0`, 0.1
  at `0x3006b7a8`) (VERIFIED decompile and file bytes). 1.1's (0x2b7f8)
  sets them outright.

  The targets also differ from 1.1's (INFERRED (decompile); constants
  VERIFIED file bytes, the same in both modules):

  | Target | 1.5 and UO | 1.1 |
  |---|---|---|
  | lean roll scale | 0.8 standing; crouched 1.5 right, 1.25 left | 1 standing; 1.5 and 1.8 crouched |
  | shift | `-2.5 f` in every stance | 1, 1.5, 2.5 |
  | prone `tag_origin` | the torso's twist `T1` moves it `(-24 (1 - cos T1), -12 sin T1)`, and when `f sin T1 > 0` `16 (1 - cos T1) (-f)` more to the side | |
  | mounted body | 1.5 aims every control at 0 (`eFlags & 0xc000`, the `memset` arm of 0x1a389); UO computes them as on foot (0x1e761 tests only the rider bit) | the hook runs nothing |

  The hook runs inside `G_DObjCalcPose`, on a build of the body: a
  locational trace or a tag lookup (`cod11-combat.md` 3, "What poses the
  entity"). vcod-server eases the controls a step on each such build
  (`combat::DobjStamps`, `playerpose::Controllers::ease_toward`), and the
  last build's controls go to the record at the slot's next end frame
  (`ClientSim::keep_eased`), so a body nothing traces keeps its controls,
  as on retail (section 13, "The glide back"). The client steps them each
  drawn frame (`entities::EntityAnim`). The jeep bone capture above does
  not tell easing from setting: the driver's view moved under 18° a frame
  and both read the same (VERIFIED, replay); the tank glide's head
  (section 13) does.

## 15. Vehicle effects: wheel dust and a vehicle's death

### 15.1 Wheel and tread dust

The vehicle DObj callback (UO cgame 0x30020540, `coduo-diff.md` 9) plays
the only effects a vehicle makes on its own. Neither vehicle file carries
an effect key (`vehicles/panzerIV_mp`, `vehicles/willyjeep_mp`, VERIFIED,
the files); the cgame registers twelve at load into the table at
0x3044cac0 (0x3002c82c..0x3002c920, VERIFIED (the instructions)):

| Slot | Tank (`surfType` not 1) | Slot | Four-wheel |
|---|---|---|---|
| 1 | `fx/map_mp/mp_tanktread_grass.efx` | 7 | `fx/map_mp/mp_jeepwheel_grass.efx` |
| 2 | `mp_tanktread_sand` | 8 | `mp_jeepwheel_sand` |
| 3 | `mp_tanktread_dirt` | 9 | `mp_jeepwheel_dirt` |
| 4 | `mp_tanktread_rock` | 10 | `mp_jeepwheel_rock` |
| 5, 6 | `mp_tanktread_snow` | 11, 12 | `mp_jeepwheel_snow` |

For each of the six `tag_wheel_*` the model has (front left, front right,
back left, back right, middle left, middle right; pointers
0x300851ac..0x300851c0, VERIFIED bytes), the callback (VERIFIED (the instructions)
0x30020d42..0x30021323; constants VERIFIED bytes):

1. Traces the world (syscall 0x26, mask 0x211, then the entity clip
   0x300350d0 skipping the vehicle) from 40 (0x3007c284) above the wheel
   along the vehicle's up to `time2 / 1000 + 16` (0x3007bd94, 0x3007bf00)
   below it. It goes on only when `cg_vehicletrails` (registered `"1"`,
   flags 1, at 0x30085d60, its integer at 0x304582ac) is set and the
   fraction is under 1.0 (0x3007bce0).
2. Keeps the wheel's last play in cent+0x23c (12 bytes a wheel). Zero
   there: take this position, play nothing. More than 200 (0x3007bf44)
   away: the same. 32 (0x3007bdd0) or less: play nothing, keep the mark.
   Otherwise the mark moves here and the wheel plays.
3. Picks the slot by the trace's material (`surfaceFlags >> 20 & 0x1f`):
   dirt 6 → 3, grass 10 → 1, gravel 11 and rock 17 → 4, ice 12 → 5, sand
   18 → 2, snow 19 → 6, any other none; a four-wheel adds 6.
4. On a four-wheel, for a front wheel or travel against the vehicle's
   forward, lowers the spot 10 (0x3007bda4) in world z; otherwise raises
   it by `rand() / 32768 * 8` (0x3007bd10, 0x3007be08).
5. Plays the slot's effect (syscall 0xe8) at the spot along the normalized
   travel since the last mark, negated, each axis scaled by `rand() /
   32768 * 0.5 + 0.25` (0x3007bce8, 0x3007be58).

The marks last until a level load. `cg_entities` is 1024 records of 0x288
bytes at 0x3048c6e0, and the callback's `lea 0x23c(%ebx)` (0x30020d18) is
the only reference to the marks. The only clear that covers them is the
`rep stos` of all 0x28800 dwords at 0x3002df4d; the two other loops over
the array (0x3003c9f0, 0x3003d360) zero +0x1e8 alone (VERIFIED (the instructions)). That 0x3002df30 is the cgame's load-time clear is INFERRED:
it zeroes four other large blocks with the array and touches no single
entity. vcod keeps each mark until a level load. The spot is the wheel as
the suspension trace placed it, at most `time2 / 1000` (under 0.03 units
on the stock vehicles) from the tag (INFERRED, `coduo-diff.md` 9).

The entity clip (0x300350d0) walks the solid list 0x30035030 builds (every
snapshot entity with a `solid`, triggers apart) and clips a brush model
(`solid` 0xffffff) as `CM_InlineModel(s.index)` (syscall 0x21) at the
centity's lerped origin and angles, testing no contents; the pass entity
is the vehicle itself (VERIFIED (the instructions)). A vehicle's `index` is its
collmap (section 10.3), so a wheel over another vehicle reads that
vehicle's collmap, whose brushes carry the vehicle clip 0x200 the mask
holds.

vcod's port is `fx::vehicle_trails`. `entities::BuiltScene::vehicle_wheels`
carries each drawn vehicle's posed wheel tags and its collmap at its drawn
origin and angles; the live frame puts each vehicle's own copy of its
collmap there (`place_collmaps`), traces the client's world under mask
0x211 without the static models and past the vehicle's own copy
(`wheel_trace`), and spawns each play as a directed effect. The client's
`SnapshotMovers` gives every UO vehicle its own copy too
(`CollisionWorld::set_model_instance`), so prediction and the vehicle
round's second trace meet each tank of a model, not the last placed. The
direction's per-axis scale leaves it off unit length; vcod's spawn
normalizes it, and whether CoDUOMP.exe's 0xe8 does is not checked
(INFERRED, CoDUOMP.exe not read).
`the_uo_driver_captures_throw_dust_off_the_ground_they_cross` replays
`*-uo-tank.bin` over mp_cassino and `*-uo-jeep.bin` over mp_peaks: 191
tread plays, all `mp_tanktread_rock` (the square's cobblestone), and 179
wheel plays, all `mp_jeepwheel_snow`, from the driven vehicle only
(VERIFIED, replay; the parked tanks throw none). Every table effect is in
the UO paks and parses.

### 15.2 A vehicle's death

No engine code draws a death. `_tankdrive_gmi::init_tank` threads
`_tankeffects_gmi::tank_death`, which on the vehicle's `death` notify:

1. plays `fx/explosions/vehicles/t34_mp_n/1st_exp_v2.efx` on
   `tag_origin`, raises `earthquake(0.25, 3, origin, 1050)`, loops
   `fx/explosions/vehicles/t34_mp_n/burn_smoke.efx` (`playLoopedFx`, 1 s)
   and the `distantfire` alias;
2. on `allow_explode` (`tank_respawn`, `self.delay` = `g_vehicleBurnTime`,
   10 s on) deletes the loop and plays `2nd_exp.efx` on `tag_origin`;
3. waits 0.5 s, clears its contents, runs `radiusDamage(origin + (0, 0,
   25), 300, 150, 10)` and deletes itself.

The `deathmodel` swap is commented out (VERIFIED, the scripts in
`pakuo0*.pk3`). The four tank types share these effects.

`client-probes/probe_vehwreck` kills mp_cassino's first Panzer IV with the
stock self-destruct's `dodamage(10000, origin + (0, 0, 1), self, self,
"MOD_EXPLOSIVE")`. On retail UO 1.51b (VERIFIED, live, `*-uo-wreck.bin`,
entity 186, kill at 33300):

| Time | Wire |
|---|---|
| 33300 | `d 998 04tag_origin`, `d 735 distantfire`; `EV_EARTHQUAKE` temp entity, `angles2` (0.25, 1050, 3000) and `time` 0; `EV_PLAY_FX_ON_TAG` parm 1 on the tank; an `ET_FX` entity on the tank, effect 3, cull 0, period 1000 |
| 43300 | `d 999 05tag_origin`; the `ET_FX` entity gone; `EV_PLAY_FX_ON_TAG` parm 2 on the tank |
| 43400 | `EV_SOUND_ALIAS` parm 7 (`vehicle_impact`) on a temp entity 2507 above the tank |
| 43800 | the tank gone, never `eType` 13 and on its live model throughout |
| 53300 | its `spawnDuplicate` in as entity 160 |

**The 43400 sound** is the duplicate's first think. `spawnDuplicate` at
43300 places it where the burning tank still stands, and
`wait_for_safe_respawn` unlinks it at once. `Scr_Vehicle_Init` settles it
at 43350 and `Scr_Vehicle_Think` steps it at 43400 (section 10.4). 0x827ab
gates its probes on `r.bmodel` (`ent+0x104`), not `r.linked` (`ent+0xf4`):
`SV_SetBrushModel` (`coduo_lnxded` 0x0808e247) stores `bmodel` 1 at +0x104
between the bounds at +0x108..+0x11c and contents -1 at +0x120, and
`SV_UnlinkEntity` (0x08099d49) clears +0xf4 (VERIFIED (the instructions)). So an
unlinked vehicle with a collmap still probes. The first think's settle
fails the "nothing moved" test, and the broad box at origin + 24 starts
inside the old tank. 0x827ab then plays `impactSnd` through
`G_PlaySoundAliasAtPoint` at the hit plus `2500 * (1 - impact)` up,
`svFlags |= 8`, under its own 1 s clock and the stuck clock's 200 ms
(VERIFIED (the instructions), 0x827ab's tail). The duplicate has no speed, so the
impact is near 0 and the sound lands 2506 above the tank (VERIFIED,
capture). vcod-server sends it from the same temp entity number (165),
time and position (VERIFIED,
`vehicle_wreck_ab::the_wrecks_duplicate_plays_its_impact_sound`); gating
on `r.linked`, or committing the settled pose at init and stepping in the
same think, loses or advances it.
`the_uo_wreck_capture_blows_the_tank_up_twice_and_burns_between` drives
the capture through the client's resolver, `play::quake` and
`fx::looped`: both blasts on `tag_origin`, the 3 s shake and the burn from
33300 then every whole second to 43000, as `CG_FX`'s timer steps
(VERIFIED, replay). Recipe:

    COD_VERSION=uo COD_DIR=<1.5 main + UO uo> PORT=31621 PROBE_SECS=75 \
        tools/run_probe.sh client-probes/probe_vehwreck mp_cassino
    vcod --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:31621 \
        --probe-team axis --save-snapshots --capture-tag uo-wreck --probe-secs 50

**The duplicate's stuck kill.** The duplicate does not take that think
unhurt. Its stuck clock is 0 from the spawn's memset, so the strike also
runs section 12's stuck kill. `client-probes/probe_vehdupe` (no client;
the first Panzer IV dies at 2050 and its duplicate, entity 160, appears at
12100) logs at 12200 a `damage` of 329999 (999999 times the hit side's
scale), attacker 160, dir (0, 0, -1), point 0.88 under its origin,
`MOD_COLLISION`, then `death`, and `health` -999 on every frame after
(VERIFIED, retail UO 1.51b, fixture
`crates/server/tests/fixtures/vehicles/mp_cassino-tank-dupe-uo.txt`).
Nothing waits on it: `tank_respawn` threads `init_tank`, which sets the
health to 2900 and waits on `death`, only once `wait_for_safe_respawn`
links the duplicate (VERIFIED, the scripts). Retail logs the `death`
waiter ahead of the `damage` one, though `G_Damage` notifies `damage`
first (VERIFIED, the fixture). INFERRED, from that order and
`cod11-gsc-language.md` ("Engine notifies and the trigger queue",
`probe_touchorder`): both notifies only wake their waiters during the
vehicle's think, and `G_RunFrame`'s `Scr_RunCurrentThreads` runs the last
woken first. vcod raises the entity pass's notifies ahead of the trigger
queue's first round, which runs them under the frame pass's pick;
`vehicle_wreck_ab::the_wrecks_duplicate_takes_the_stuck_kill` compares
the lines in order (VERIFIED, replay, 2026-10-20).

**The earthquake.** UO's builtin (game.mp.uo `.so` 0x6d8dd, ghidra
0x7d8dd) stores `(int)(1000 * duration + 0.5)` as a float in `angles2[2]`
(VERIFIED (the instructions)), not 1.1's `time`; vcod-server does under protocol
22. The same probe on vcod-server (`--gametype-script`) sends the same
events, entities and times (VERIFIED, live, 2026-10-15, and the gates
above).

**The player's trBase.** A player's entity goes out with `pos.trBase`
truncated to whole units: `BG_PlayerStateToEntityState` (0x20a35) snaps
it under a `| 0xc00` control word (`fldcw` at 0x20ab0) (VERIFIED (the instructions), section 11.4), and vcod-server sends the
truncated base.

### 15.3 The seat view

`CG_VehicleView`'s motion on the eye, the gunner's sway and firing shake,
is section 5's and drawn. The only other view write in a seat is
0x30040580's: after its trap 0xf7 it stores `-(int at 0x30483218) * 360 /
65536` (0x3007bd5c) into 0x304832b4 (0x3004073c..0x3004078e, VERIFIED (the instructions)). Taking `cg.predictedPlayerState` to start at 0x304831b8, as
its `eFlags` at +0x90 and `fWeaponPosFrac` at +0xec do, those are
`delta_angles[2]` and `viewangles[2]`: the view's roll set against the
roll delta, which no seat change turns, so 0 (INFERRED). vcod writes no
roll. The turret view (0x30041550, `eFlags` 0x6000) has a sway and firing
kick of its own off the turret weapon's file; that is a turret's, not a
vehicle's: coduo-diff.md 19.5, "The turret view".

## 16. Clearing and verifying a vehicle's spot

VERIFIED, `game.mp.uo.i386.so` `VEH_GetMinsMaxs` (0x8d5a6): the box the
two methods search is the vehicle file's `mins` and `maxs` (`info+0x36c`,
`+0x378`) times 1.2 (the float at 0xa75f4), each component plus
`tv(-50, -50, 0)` for the mins (0x8d648..0x8d6af) and `tv(50, 50, 20)` for
the maxs (0x8d716..0x8d77d).

VERIFIED, `clearVehiclePosition` (0x6950e): on a vehicle (`eType` 12,
0x69534), that box about `r.currentOrigin` (`+0x13c`), `trap_EntitiesInBox`
for at most ten entities under mask 0x4000100 (0x695e8), and for each the
four steps of `delete` inline: `Scr_AddEntity` of itself and `Scr_Notify`
of `death` (0x69646), `trap_UnlinkEntity` (0x69654), the touch and use
handlers cleared (`+0x220`, `+0x21c`), `think = G_FreeEntity` and
`nextthink = level.time + 100` (0x6968b..0x696a0).

VERIFIED, `verifyPosition` (0x692e8): the same `VEH_GetMinsMaxs` box
(0x69336) searched under mask 0x2800000 (0x69420), then the optional reset
(`VEH_InitPhysics`, 0x6946a) and a `trap_Trace` (0x694da). vcod's
`position_clear` still tests the file's own box against the bodies and
the other vehicles and runs no trace; not measured.

VERIFIED live, `client-probes/probe_clearveh` on mp_cassino (2026-11-14,
`clearveh_uo_ab`): a test client stood over the first tank (186,
`panzeriv_mp`, origin 4676, -1979) drops an `mp40_mp` that lands 129
units ahead of the origin, past the file's 70 and inside the padded box,
and a second 512 off. The call deletes the near one alone: its
`waittill("death", e)` wakes after the calling thread's step at the same
`getTime()` with `e` the item, a thread's `endon("death")` on it fires,
and `isDefined` still reads 1 on it 50 ms on. UO's script `spawn` has no
item classnames (`weapon_mp40_mp doesn't have a spawn function`, then the
server aborts), so the probe drops. vcod: `clutter_in_box` walks the area
tree over `VEH_GetMinsMaxs`'s box with retail's mask and count; it tested
each item's origin against the file's box before, which missed the
capture's item.

## 17. The script surface no stock MP script uses

The vehicle methods and fields the stock MP scripts never touch:
`closespeed`, `harc`, `getWheelSurface`, the seven path methods and
`freeVehicle`. Addresses are `game.mp.uo.i386.so`'s. The captures are
`client-probes/probe_vehscript.gsc` on mp_kharkov (one `probe_case` per
run, fixtures `vehicles/mp_kharkov-vehscript-{read,free}-uo.txt`) and two
drives under the fields (`playerstate/mp_peaks-tdm-slope-8ms-jeep-closespeed-uo.txt`,
`playerstate/mp_cassino-tdm-slope-8ms-tank-closespeed-uo.txt`), all on
2026-11-17. Gates: `vehscript_uo_ab`, `vehicle_ab`, `vehicle_drive_ab`.

### 17.1 The fields

- `speed`, `closespeed` and `harc` are the entity's +0x1f0, +0x1f4 and
  +0x2bc, float fields (VERIFIED, `dump_script_fields.py`).
- `closespeed` scales the drive's top speed where it is not 0: the
  target speed `0.8 maxSpeed` (0x7d7db..0x7d803, VERIFIED (the instructions))
  and the velocity cap of 0x827ab's hit response (`param_1[0x7d]`,
  VERIFIED (the instructions)). `harc` replaces the turret's
  file arcs with `[-harc, harc]` in yaw where it is not 0 (0x85b32,
  `param_1[0xaf]`) (VERIFIED (the instructions)). Nothing else in the module reads
  either (INFERRED (decompile), every `+0x1f4`/`+0x2bc` and
  `[0x7d]`/`[0xaf]` use).
- A map's vehicles read 0 in both, and a script's write reads back
  (`PROBE set 152 0.5 10`). `speed` is the think's own: 176 written reads
  0 a frame on, and one T-34 on mp_kharkov reads 24.3591 (VERIFIED,
  capture).
- The step's pitch factor `1 ± angles[0] / 90` stays on the x87 stack into
  its product with the target speed, rounded to a float once
  (0x7da19..0x7da63) (VERIFIED (the instructions)). Under `closespeed` 0.5 the
  jeep reaches the target speed's clamp, and a factor rounded first put
  its z an ulp off retail's from 14800.
- A turret held on `harc` 10 sends 9.997559: the float limit, truncated
  to the wire's 16 bits (VERIFIED, capture).
- vcod: the think passes the entity's two fields to the step
  (`vehicle::think`). The jeep drive matches retail field for field to the
  driver's exit, all 418 seated snapshots (VERIFIED, replay, 17.5 for the
  hit at 18500 that used to part it). After the exit the fall off the
  dismount spot runs 3 u/s slow from its first frame (z velocity -34 where
  retail has -37 at `commandTime` 33432, the same six cmds) and lands 0.6
  higher with no slide; not read. The tank's drive matches through to its
  end, the track clock's part at 27200 included (17.5). `vehicle_ab`
  names the jeep's tail as a gap.

### 17.2 `getWheelSurface(name)`

- 0x8e7c1 reads the name with `Scr_GetConstString(0)` first (an int
  reads as its text), then refuses a vehicle whose type is neither
  four-wheel (1) nor tank (2) with `Scr_Error("Vehicle type [%s] has no
  wheels\n")`. The names are scr_const 0x88..0x92, `front_left`,
  `front_right`, `back_left`, `back_right`, `middle_left`,
  `middle_right`, the `tag_wheel_*` order (VERIFIED (the instructions),
  `GScr_LoadConsts` 0x652d2). Any other name is `Scr_ParamError(0, "Valid wheel
  names are: [front_left, ...]\n")`, a four-wheel's middle pair
  `Scr_ParamError(0, "Vehicle has no middle wheels\n")` (VERIFIED, gdb on
  `Scr_ParamError`).
- The answer is the wheel's `rec+0x158 + 4i`: the suspension trace's
  `(surfaceFlags & 0x1f00000) >> 20`, an integer store at 0x815e1..0x815ec,
  0 for a miss (VERIFIED (the instructions)). 0 answers `"none"` (0xa78c2);
  anything else goes to `trap_SurfaceTypeToName`. `coduo_lnxded`'s
  0x8072dd4 returns `"default"` (VERIFIED (the instructions)). INFERRED: the trap
  lands there, as 1.1's does (`cod11-combat.md` 2.7).
- Every wheel of mp_kharkov's 22 vehicles reads `default` (VERIFIED,
  capture). vcod: `builtins::vehicle::get_wheel_surface` off the step's
  `wheel_surface`.

### 17.3 The path methods

- `attachPath`, `startPath`, `setSwitchNode`, `setWaitNode`,
  `setWaitSpeed`, `setSpeed` and `resumeSpeed` (0x8dfe9..0x8e52e) drive a
  vehicle along `info_vehicle_node`s, which `G_CallSpawn` (0x719a6) takes
  from the map into a pool of its own, outside the entities (VERIFIED (the instructions)). A node reaches script only as a class-2 object, and only
  `GScr_GetVehicleNode`, `GScr_GetVehicleNodeArray` and
  `GScr_GetAllVehicleNodes` (0x984d2..0x98726) make one; none of the three
  is in UO MP's `functions` table, and nothing calls them (VERIFIED,
  `dump_builtins.py`, decompile). So an MP script holds no node:
  - `attachPath(n)`, `setSwitchNode(a, b)` and `setWaitNode(n)` fail in
    `GScr_GetVehicleNodeIndex` (0x9836c) with `Scr_ParamError(0, "Not a
    vehicle node")` for an entity (VERIFIED, gdb). A value that is no
    entity aborts with no module error (VERIFIED, capture); INFERRED: the
    engine's `Scr_GetEntityNum` raises it.
    `attachPath` and `setSwitchNode` read the node ahead of the use test
    below, `setWaitNode` after it.
  - `startPath()` raises `Can't start path on a vehicle that hasn't been
    attached` (VERIFIED, gdb).
  - `setWaitSpeed(mph)`, `setSpeed(mph, accel)` and `resumeSpeed(accel)`
    store `rec+0x23c`, `+0x274..0x27c` times 17.6, which only the path
    think (0x88561) reads, and raise `Cannot have a negative wait speed on
    a vehicle`, `Cannot set negative speed on vehicle` (param 0) and
    `Cannot set negative acceleration on vehicle` (param 1 for `setSpeed`,
    0 for `resumeSpeed`) on a negative value (VERIFIED, gdb).
    `setSpeed(5)` aborts with no module error (VERIFIED, capture);
    INFERRED: the engine's `Scr_GetFloat(1)` raises it.
  - Each but `attachPath` and `setSwitchNode` first raises `Vehicle is
    invalid on path after it's been used` while `ent+0x17e` is 2, a
    driven vehicle (INFERRED (decompile); not captured).
- A builtin's error on UO throws past the module's frames and aborts the
  process before the console prints it, at `developer` 1 as at 0
  (VERIFIED: every erroring case above ends `Aborted`, and
  `attachPath`'s backtrace shows `__cxa_throw` under `std::terminate`). The messages above were read at
  `Scr_Error` and `Scr_ParamError` under gdb.
- vcod: `builtins::vehicle` checks the arguments in retail's order and
  raises the same messages; the stores are dropped, since nothing reads
  them without a node.

### 17.4 `freeVehicle()`

- 0x8e72f runs `G_FreeVehicle` (0x8ad7d: health 0, the sound blends
  freed, the think and `speed` cleared, `eFlags`, `pos.trType` and
  `apos.trType` 0, the record pointer cleared), sets `eType` 13 and
  classname `script_vehicle_corpse` (scr_const 0x84), notifies `death`
  with no argument and runs `Scr_FreeEntityNum` on the entity (VERIFIED (the instructions)). It does not unlink the entity or let its riders go.
- Measured on a Gaz (`PROBE free 152 gaz67b_mp 2900 ...`): the handle
  reads undefined on the calling thread's next line; a `level` thread
  waiting on its `death` wakes after the call, and the vehicle's own
  threads (`self` the vehicle: a `death` waiter, an `endon("death")`
  waiter, a `wait 0.05`) never run again. `getEntArray("script_vehicle_corpse",
  "classname")` finds the same number at the same origin under a new
  handle, `speed` and `health` 0, a script field written before the call
  gone, `vehicletype` kept (an entity field); the vehicle count drops by
  one. A second `freeVehicle` on the old handle is `dead entity is not an
  entity` (VERIFIED, capture).
- vcod: `vehicle::free_to_corpse` frees the record and the blends, leaves
  the entity linked with its collmap, renumbers its handle
  (`Entities::reissue`) with a fresh script object, carries UO's own
  entity fields (`vehicletype`, `capturing`, `vehicle_owner`, script
  fields in vcod) across, and sends it as `eType` 13 standing still
  (`Vehicles::corpses`). Riders: 17.4.1.

#### 17.4.1 Freeing a vehicle with riders

Captured 2026-11-26 on retail UO, mp_cassino's Panzer IV with a driver and
a gunner seated (`probe_vehdie` with `probe_kill free` and `free_quiet`,
fixtures `vehicles/mp_cassino-tank-free{,-quiet}-uo.txt`, gate
`vehicle_die_ab`).

- Nothing touches the riders: on the calling thread's next line both read
  `isInVehicle` 1, health 100, the same origins (VERIFIED, capture).
- Stock MP scripts: a rider's `tank_death_player_damage`
  (`_tankdrive_gmi.gsc`; the jeep's `death_player_damage` has the same
  body), started 0.2 s after his seat, wakes on the `death` and reads
  `tank.last_attacker` off the freed handle: `dead entity is not an object`
  at line 461, which at `developer 1` ends the server (VERIFIED, capture).
  The gunner's had not started yet, 50 ms after his seat.
- With those threads ended first (`notify("deactivated_player")` on each
  rider), the level thread's `death` waiter runs and the server segfaults
  before the next frame's script (VERIFIED, capture). Under
  `tools/re/gdb_fault.py` the fault is `ClientThink_real` 0x40fea: with
  the client's `eFlags & 0x100000` (0x40fbf) it reads
  `g_entities[ent->r.ownerNum (+0x154)]`'s record pointer (+0x164), which
  `G_FreeVehicle` zeroed (0x8afcf), and loads its +0xd0 through it
  (VERIFIED, gdb and the instructions). INFERRED: every occupied
  `freeVehicle` ends a retail server this way, at any `developer`.
- vcod does not fault: the seats go with the record and each rider gets a
  forced exit where he sits (`SeatOp::Exit` with no spot), `isInVehicle`
  reading the stale bit until his end frame, so the same-frame lines match
  retail's; a frame on ours reads 0 where retail has crashed. A `delete()`
  of an occupied vehicle does the same.

### 17.5 The closespeed drives' two parts

The jeep part at 18500 is the probes' brush clip. Addresses are
`coduo_lnxded`'s and `game.mp.uo.i386.so`'s.

- `CM_TraceThroughBrush`'s UO build (0x8058f0a) branches on the trace's
  capsule flag (`tw+0xf4`): a capsule takes the arm at 0x8058f5f, a plain
  box (`trap_Trace`, every vehicle sweep) the arm at 0x80593dc (VERIFIED
  (the instructions)). The box arm walks the six axial sides first, mins
  then maxs, each against the box's half size (`tw+0x34`), then the other
  sides with their distance less `offsets[signbits] . normal`
  (`tw+0x40`), stored (0x80595f4..0x8059644) (VERIFIED (the
  instructions)). Both arms share one side test: the enter fraction backs
  off 0.125 (0x80dca40), the leave fraction is `d1 / (d1 - d2)` with no
  epsilon (0x8059542..0x805957a), leave starts at the trace's fraction
  (`tw+0xc4`, 0x8058f1e), and a side that brings enter to leave ends the
  brush (VERIFIED (the instructions)). Q3's box arm backs the leave off
  0.125 as well (INFERRED, Q3's `cm_trace.c`).
- At 18500 probe 0 sweeps from (-3698.30, 847.52, 29.74) to (-3673.48,
  792.57, 63.42) past brush 465, a `clip_nosight` post (x -3656.07 ..
  -3648.67, y 814.40 .. 821.07, z 10.90 .. 55.08, 66 sides). It enters
  the post's slanted side (normal (-0.9996, 0, 0.0287)) at 0.78013 and
  leaves the axial top at 0.78201 by retail's rule, at 0.77828 with Q3's
  epsilon. Retail's probe 0 strikes at (-3678.94, 804.65, 56.01) and
  kicks the yaw (VERIFIED, gdb at 0x8499f on a rerun of the drive, the
  same hit 5150 ms later), the 0.78013 point (VERIFIED, replay). The
  server's `Box collision on terrain currently being faked with capsule
  collision` print is the terrain clip's box arm (0x8056f57 sets the
  capsule flag and the radius for 0x80558dd), not the brush clip's
  (VERIFIED (the instructions)). vcod's box sweeps
  take that arm (`clip::clip_brush_box`, the capsule arm's side walk), and
  both jeep closespeed replays match to the exit (VERIFIED, replay).

The tank part at 27200 is a velocity that reaches 0. Read under
`tools/re/gdb_vehicle.py` on a rerun of the drive, the record's state at
the track clock each think, every float equal to vcod's through the
capture's 27150 (VERIFIED, gdb, 2026-11-21).

- The tank stops at 24150 and its velocity decays by about 0.316 a think
  through the turn in place that starts at 26750. At 27200 retail's is
  exactly 0, vcod's 1.17e-23. `VEH_SlideMove`'s walkable arm (0x81175)
  tests `clip(v) . v` against 0 on the x87 stack (0x8120e..0x8123a); the
  sum there is 1.75e-46, so it renormalizes. `VectorNormalize` stores the
  squared length as a float, 0, and leaves the vector; the scale by the
  old speed (5.07e-23) then stores each component as 0 (VERIFIED (the
  instructions); gdb). vcod rounded the dot to a float first and skipped
  both.
- With the local velocity's x exactly 0 the track clock reads the yaw rate
  instead (0x86b20..0x86b37): `trunc(1000 * 50.097656 / 176 * 0.05 *
  5.5)` is the 78 (VERIFIED (the instructions); capture).
- The stack test alone moves `vehicle_ram_ab` and `vehicle_runover_ab`:
  their struck tank then reaches 0 where retail's keeps stepping, which
  `VEH_PhysicsNotRequired`'s walk of the pool does (0x87c2d..0x87cc3:
  each record in use within 1024 units, 1048576 squared at 0xa6d38, that
  is not itself at rest keeps the caller stepping) (VERIFIED (the
  instructions)). vcod runs both (`walkable_move`'s `dot64`, the walk in
  `Physics::physics_not_required`), and the ram, runover and closespeed
  replays match. The walk also wakes a wreck beside its duplicate, which
  took more (17.6).
- The push a hit hands another vehicle (0x827ab's `script_vehicle` arm)
  keeps the speed share on the stack into each component, `share * k *
  d[i] + v[i]` stored once (0x83a7a..0x83b9f, 0x83c3c..0x83dcb) (VERIFIED
  (the instructions)). vcod rounded `k * share` and the product apiece and
  pushed each component an ulp high; it now matches the struck tank's
  pushed velocity to the bit (41a5c060 41121df8 bf356778) (VERIFIED, gdb
  on a rerun of `vehicle_runover_ab`'s capture).

### 17.6 A wreck beside its duplicate

With the walk of 17.5 the mp_peaks Willys's duplicate shoved its wreck off
retail's line: `vehicle_blast_ab`'s wreck blast landed 10 on the shooter
where retail's lands 11. Two reruns of retail under
`tools/re/gdb_vehicle.py` show why: the mp_peaks blast capture
(2026-11-21) and `client-probes/probe_vehdupe` on mp_cassino
(2026-11-23, no client; the same kill at 2050, the duplicate 160 at
12100). Addresses are `game.mp.uo.i386.so`'s.

- **The walk runs on every pass.** `Scr_Vehicle_Think`'s two passes
  (0x8c8f5..0x8c98c) each call the drive (0x87cd3, or 0x88561 for
  `ent+0x17e` 1) and then 0x827ab; the drive asks
  `VEH_PhysicsNotRequired(ent, 1)` at 0x8811c, so the walk reads the
  other records as they stand at that pass (VERIFIED (the instructions)).
  A push writes the struck record's velocity at once (12, "Another
  vehicle"). So a duplicate's first pass that strikes the wreck restores
  its committed copy (the BSP pose, no velocity: 10.4), pushes the wreck,
  and its second pass steps, because the wreck it just pushed no longer
  rests. On mp_cassino the duplicate enters the think with the committed
  z 4 (the BSP's) and leaves it settled at -7.875 with velocity (0, 0,
  -40); the wreck's next think enters with (0, 0, -50) and leaves with
  (0, 0, 0.5) and a kick whose decel is 700 (VERIFIED, gdb). vcod read
  the walk once before the think, so the second pass stood still and the
  duplicate kept its BSP pose (4 on mp_cassino, -87 on mp_peaks). The
  step now reads the other records through `VehicleWorld::pool` on each
  pass, a velocity pushed this think from `StepOutput::pushes`, and the
  push's kick decelerates at 700 (vcod had 175).
- **The duplicate never rests.** Each think its probes strike the wreck
  and the restore puts back the velocity the last pass committed: retail's
  duplicate holds (0, 0, 0.4) at -7.875 from 12200 to the end of the run
  at 30400, long after the wreck goes at 12600 (VERIFIED, gdb). vcod's
  holds the same. A body that walks into it takes `VEH_PlayerCollision`'s
  knock (0x8213f, 0.4 is over its 0.01 floor): `bot_vehicles`'
  `a_bot_keeps_out_of_a_burning_hull` reads the bot's health at the
  hull's blast, not after.
- **The wreck settles on its death model.** `_jeepeffects_gmi::death`
  sets a jeep's `deathmodel` at the death (`xmodel/v_us_lnd_willysjeep_d`
  for the Willys); the tanks' swap is commented out (15.2) (VERIFIED, the
  scripts in `pakuo0*.pk3`). The suspension (0x81331) reads each wheel's
  tag off the entity's DObj on every step: its wheel loop
  (0x813e0..0x8171d) calls 0x7c488, which is `G_DObjGetLocalTagMatrix(ent,
  tag)` on the tag name table at GOT - 0x4ea0, with `Com_Error`
  when one is missing (0x7c4ae..0x7c4f3) (VERIFIED (the instructions)).
  On the mp_peaks rerun the wreck, at rest since the kill, steps at the
  duplicate's first frame (the duplicate's settle left it an angular
  velocity) and rises 0.12 (z -110.5165 to -110.3952) and rolls -0.846
  to -0.912 with no velocity: the death model's wheels (VERIFIED, gdb).
  vcod kept the spawn model's wheel tags, so its wreck's step changed
  nothing and the duplicate's walk found every record at rest. `setModel`
  on a vehicle now re-reads them (`vehicle::model_changed`).
- With the three, vcod's mp_peaks duplicate and wreck repeat the rerun's
  pose, velocity and kick on every think from the duplicate's settle
  through the wreck's slide, and mp_cassino's duplicate and wreck repeat
  the probe_vehdupe rerun's to the bit (VERIFIED, replay against both gdb
  logs, 2026-11-23; `vehicle_wreck_ab::the_wreck_and_its_duplicate_step_as_on_uo`
  holds mp_cassino's, fixture `mp_cassino-tank-dupe-steps-uo.txt`). The
  wreck's blast lands 11 on the shooter, as in the fixture (VERIFIED,
  `vehicle_blast_ab::a_rocket_strikes_a_jeep_as_on_uo`).

### 17.7 The rest test's fifteen words

`VEH_PhysicsNotRequired` (0x87a13, by its symbol in
`game.mp.uo.i386.so`) tests the physics state `s` (`rec+0xb8`,
0x87a28..0x87a39) against `vec3_origin` (GOT + 0xf98, the
`R_386_GLOB_DAT` at 0xb47b0) one float at a time, `fucompp` with a jump
out on not equal or unordered (VERIFIED (the instructions)):

| Words | Order | Field | vcod |
|---|---|---|---|
| `s+0x48..0x50` | 0x87a45..0x87a9b | angular velocity | `State::angular_velocity` |
| `s+0x30..0x38` | 0x87aaa..0x87b00 | velocity | `State::velocity` |
| `s+0x64..0x6c` | 0x87b0f..0x87b65 | the body sway: pitch, a word nothing moves, roll | `sway_pitch`, `sway_roll` |
| `s+0x58..0x60` | 0x87b74..0x87bbe | the sway's rates, laid out the same | `sway_pitch_vel`, `sway_roll_vel` |
| `s+0x3c..0x44` | 0x87bc9..0x87c0b | local velocity | `State::local_velocity` |

- The sway's step (0x7d709's tail) writes the pitch rate at `s+0x58`
  (0x7f668, clamped at 0x7f6d4), adds it to the pitch at `s+0x64`
  (0x7f6e0..0x7f6f0), writes the roll rate at `s+0x60` (0x7f908) and adds
  it to the roll at `s+0x6c` (0x7f914..0x7f924) (VERIFIED (the
  instructions)). The base is `s`: 0x7d72f adds 0xb8 to the record and
  0x7d734 keeps it at `ebp-0x20`.
- `s+0x5c` and `s+0x68`, the two vec3s' middle words, are zeroed by
  `VEH_InitPhysics` (0x7ce38, 0x7ce50) and stored by nothing else in the
  vehicle code (0x78000..0x92000: every other store at `+0x5c`,
  `+0x68` or `rec+0x120` there is a temp entity's, a definition's or an
  entity's `r.contents`; none at `rec+0x114`) (VERIFIED (the
  instructions)). Every think of the 2026-11 tank, ram and blast reruns
  under `tools/re/gdb_vehicle.py`, 54114, holds `s+0x5c` at 0,
  3065 of them with the sway moving; the script's five sway words stop
  short of `s+0x68` (VERIFIED, gdb).
- So retail's fifteen words test what vcod's `Physics::still` (ten) and
  `physics_not_required`'s velocity (three) do: the two words more are
  always 0. Neither test tells -0 from 0; a NaN is never at rest in
  either. A vehicle at rest goes on to walk the pool when the second
  argument is 1 (0x87c1b, 17.5).
