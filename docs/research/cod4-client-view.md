# CoD4 1.7: the watched view, its clock and its lerp

How the client draws a CoD4 session (`play::cod4`, `cod4_client`): the
time it draws at, the two snapshots it lerps between, when it cuts
instead, and how it predicts its own body. The gates are
`play::cod4::tests::walk_capture_view_runs_smooth` and
`walk_capture_predicts_its_own_body`.

Sources, each claim naming its own:

- `iw3mp.exe` 1.7 (md5 in `cod4-netfields.md`), disassembled with
  `objdump`; addresses are its own.
- The committed play captures (`cod4-netfields.md` 8, 10), replayed by the
  gate.

A claim labelled INFERRED that names no 1.7 address is not yet confirmed
on 1.7.

A followed view, and any view before the level's clip map is loaded, is
the snapshots' playerstate, lerped, as retail draws one with prediction
off (sections 1-3). Our own body is predicted through
`vcod_cod4::pmove` (section 5).

## 1. The client clock

- `CL_SetCGameTime` is `iw3mp.exe` 0x45c440 and `CL_AdjustTimeDelta`
  0x45c0e0. VERIFIED: the adjust's `<RESET> ` (0x6cef44) and `<FAST> `
  (0x6cef50) are pushed at 0x45c190 and 0x45c1be, the set's
  `Extrapolating snapshot!` (0x6cf030) at 0x45c597.
- Each frame `cl.serverTime` (0xc628d0) is `cls.realtime` (0x956e98) plus
  `cl.serverTimeDelta` (0xc628dc), never below last frame's
  (`oldServerTime`, 0xc628d4). There is no `cl_timeNudge` term. A frame
  whose unfloored time reaches the newest snapshot's (0xc5f940) less 5
  sets the extrapolated flag (0xc628e4). The delta adjusts only when
  `newSnapshots` (0xc628e8) is set. VERIFIED (0x45c52b-0x45c5ac).
- The adjust aims the delta at `ideal = snap - realtime - interval - 5`,
  `interval` the newest snapshot's time less the one before it
  (`oldSnapServerTime`, 0xc628e0): the drawn time runs one snapshot
  interval and 5 ms behind the newest. VERIFIED (0x45c107-0x45c124). Off
  by more than 500 (a delta above the ideal measures all of it, one below
  only past `interval`, all of it once `interval` passes 500) it resets
  to `snap - realtime - 5` and puts the time at the snapshot's; more than
  100 averages the two; anything closer slews: 2 ms back after an
  extrapolated frame (clearing the flag), else 1 ms toward the ideal,
  only at `com_timescale` 1. VERIFIED (0x45c126-0x45c1f2). This is CoD2's
  adjust (`cod2-protocol.md` 18.1) but for the reset's target, which
  vcod's CoD2 clock puts at the ideal. `ServerClock::cod4` runs it.
- 1.7 has no `Cl_AdjustTimeDelta RESET` warning. VERIFIED: the string is not in the 1.7 image.

## 2. The snapshots drawn and the lerp

- `cg.snap` is the newest snapshot at or before the drawn time and
  `cg.nextSnap` the first after it (the same snapshot when none has
  come); a time before `cg.snap`'s is raised to it. The lerp fraction is
  `(time - snap) / (next - snap)`, floored at 0 and 0 for one snapshot.
  INFERRED (`CG_ProcessSnapshots`, `CG_SetFrameInterpolation`).
- Unpredicted, the view's origin, velocity and `viewHeightCurrent`
  lerp between the two playerstates, so a stance change eases the eye.
  The client's own view is its newest cmd's angles over `nextSnap`'s
  `delta_angles`, unlerped; one it follows (otherFlags 2, the killcam)
  lerps `viewangles` the short way round. INFERRED (`CG_PredictPlayerState`'s `cg_nopredict` and demo arms,
  `CG_InterpolatePlayerState`).
- The mouse turns `cl.viewangles`, the cmd's angles, with no clamp but
  a 90 degree cap on one frame's pitch change (`CL_CreateCmd`), and the
  camera draws the predicted view, which `PM_UpdateViewAngles` clamps
  to `player_view_pitch_up` / `_down` with the excess pushed into
  `delta_angles`. INFERRED. vcod
  keeps the cmd's view as `Cod4Session::view` (`cl.viewangles` plus the
  newest `delta_angles`, moved by each change of it) and adds the
  mouse's turn to it each frame; the camera takes the scene's view.
  Trap: a camera that is the mouse's own climbs past the limit to its 89
  between snapshots and each push throws it back to 85, a jump at the
  edge. `cod4_live_pitch_held_past_the_limit_stays_at_the_limit` holds the
  drawn view at 85 and -85 frame for frame. VERIFIED (the test).
- No distance cuts a lerp. The playerstate starts over (the older one
  replaced by the newer) when the teleport bit 2 of `eFlags` toggles, the
  view's `clientNum` changes or `stats[4]` does; an entity starts over
  when its own bit 2 toggles or it was not in the older snapshot
  (`CG_ResetEntity`). INFERRED (`CG_SetNextSnap`). vcod cuts the same
  way and has no distance cut.
- Every cut in the three play captures is one of those. VERIFIED (the
  gate):
  - `walk`: snapshot 29, the team pick, the view turns to the idle
    teammate's (`clientNum` 2 to 0, otherFlags 0x2) while the class menu
    is up; snapshot 51, its own spawn 552 units away (`clientNum` back,
    the bit toggled). This pair is the "camera teleported" a human
    watching the capture saw.
  - `kill-shooter` and `kill-target`: snapshot 48, the spawn (`pm_type`
    4 to 0, the bit toggled); 59, the respawn after the class pick. The
    target also: 129, the killcam (`clientNum` 0 to 1, `deltaTime`
    3100); 140, the killer's respawn inside it; 219, the killcam's end and
    its own respawn.
  - At the walker's own spawn one frame draws the spawn's yaw (45) before
    the probe's next cmd, built on the new `delta_angles`, turns it back
    to the script's -135. That is the probe holding its scripted view.
    VERIFIED.

## 3. What the gate measures

The walk capture drawn at 60 and 144 frames a second, before and after
the clock above (the old one restarted a lerp at each snapshot's arrival,
so arrival jitter stepped the drawn time). VERIFIED (the gate):

| | drawn time step, settled | largest eye step outside a cut |
|---|---|---|
| before, 60 Hz | 0 to 100 ms | 10.7 |
| after, 60 Hz | 14 to 18 ms | 6.6 |
| before, 144 Hz | 0 to 50 ms | 4.0 |
| after, 144 Hz | 4 to 8 ms | 3.1 |

The eye still moves in 100 ms beats while the walker runs: the probe
sent a move message every 90 to 100 ms, so the server ran its cmds on
every other snapshot and `ps.commandTime` and the origin advance in
pairs (snapshots 53-104 of `walk`). VERIFIED. Retail would draw that
the same way with prediction off; with it, its own pmove fills the gap,
as vcod's does (section 5).

## 4. The view weapon

`cod4_viewmodel` draws the drawn playerstate's view weapon (our own body
or the one we follow; never a free spectator's view), as `CG_AddViewWeapon`
and `CG_UpdateViewWeaponAnim` do. The gates are
`play::cod4::tests::shooter_draws_its_view_weapon_and_flashes_on_its_tag`
and `walker_draws_its_view_weapon`; the zones' census is
`crates/cod4/tests/view_census.rs`.

### 4.1 Fields and defaults

- `cg_fov` defaults to 65, `cg_fovScale` to 1 and `cg_fovMin` to 10.
  VERIFIED (iw3mp.exe: the floats 65.0 at 0x70bb8c pushed at 0x43ad70
  before the register of `cg_fov` (0x6c9010) at 0x43adc0; `fld1` at
  0x43adca before `cg_fovScale` (0x6c903c); 10.0 at 0x6bdf70 at 0x43ae2c
  before `cg_fovMin`).
- `cg_gun_x`, `cg_gun_y` and `cg_gun_z` default to 0. VERIFIED (iw3mp.exe
  0x43bec1, 0x43bf23, 0x43bf85: `fldz` after each register, names at
  0x6c9fd8, 0x6ca000, 0x6ca028).
- `bg_bobAmplitudeSprinting` (0.02, 0.014), `bg_bobAmplitudeStanding`
  (0.007, 0.007), `bg_bobAmplitudeDucked` (0.0075, 0.0075),
  `bg_bobAmplitudeProne` (0.02, 0.005). VERIFIED (iw3mp.exe
  0x40a0bd-0x40a24c, the floats at 0x70bc5c, 0x70b880, 0x70bc58,
  0x70bc54 and 0x70b7a8 loaded before each register).
- `g_animRateOffsets`, each view node's time field in `WeaponDef`, is the
  33 ints at iw3mp.exe 0x727580 (-1, -1, -1, -1, 888, ...). VERIFIED (read
  out of the image).
- The `WeaponDef` fields read (offsets into the zone's 0x878 bytes, laid
  out so the zone walk's `OPS` match its pointer slots): the move
  offsets and turns 564-648, the rates and min speeds
  660-696, `adsZoomFov` 1060, `overlayReticle` 1080, `adsBobFactor`
  1096, the idle amounts 1164-1184, the sway 1196-1244, `aimDownSight`
  1260, `adsAimPitch` 1740, `adsReloadTransTime` 1940. INFERRED.

### 4.2 The rig and its clips

- The rig is `handModel` (or the hands `CS_MODELS + viewmodelIndex`
  names, `CG_UpdateHandViewmodels`) with `gunModel[0]` grafted at
  `tag_weapon`; node `n` plays its `n`th view anim (`idleAnim` 1, `fireAnim` 3), an empty one idle's.
  INFERRED (`CG_RegisterWeapon`, `CG_CreateWeaponViewModelXAnim`). Every `common_mp` weapon with a gun
  but the placeholder `defaultweapon_mp` (111) has both models whole in
  `common_mp` and each named clip in a zone, case-folded
  (`viewmodel_M4m203_knife_melee_1`). VERIFIED (`view_census`).
- `WeaponRunXModelAnims` starts the node its switch names off `weapAnim`
  (toggle bit 0x200 off; 2 fire node 3, 3 last shot 5, 5 ADS fire 28, 13
  reload 9, 11 raise 13 and so on); 0 and 1 wait for the running main
  clip, a 1 cutting it, then play idle (empty idle with the clip at 0).
  A node's clip is stretched over its `g_animRateOffsets` time. The
  sight layer, for an `aimDownSight` weapon, sets ADS up (31) at
  `fWeaponPosFrac` and ADS down (32) at its complement, the wanted one's
  goal 1 over 0.1 s; it is up while `pm_flags` 0x10 and not `weapFlags`
  2, unless a reload's `weaponTime` is still past
  `adsReloadTransTime`. INFERRED (`WeaponRunXModelAnims`,
  `PlayADSAnim`, the anim rate). Left out: the perks'
  rate changes, the crossfade a 1 asks for, notetracks, camo variants
  and the rocket and goggles models.
- `hideTags[8]` sits at `WeaponDef`+216 as zone script string indices:
  read there, `mp5_mp` names `tag_acog`, `tag_ak47_mount` and
  `tag_reflex_sight`, `mp5_acog_mp` `tag_reflex_sight` alone,
  `winchester1200_mp` `tag_ak47_mount`, `tag_reflex_sight` and
  `tag_foregrip`, `winchester1200_reflex_mp` `tag_foregrip`; all 131 tags
  of `common_mp`'s weapons name a bone of their `gunModel[0]`. VERIFIED
  (`common_mp.ff`, `weapon_view`). The variants of a gun share one view
  model carrying every attachment (`mp5_mp` and `mp5_acog_mp` both draw
  `viewmodel_mp5_mp`), each attachment's surfaces skinned to its tag
  alone (the ACOG's four to `tag_acog`, the reflex sight's lens, dot and
  housing to `tag_reflex_sight`). VERIFIED (`XSurface.partBits`).
  `CG_RegisterWeapon` sets the tags' bits in the view DObj's
  `hidePartBits`, and the renderer skips a surface whose `partBits` meet
  them. INFERRED (`CG_RegisterWeapon`, `R_SkinXModel`). vcod builds each weapon's gun without those surfaces
  (`vcod_cod4::anim::model_hiding`); without that, a plain MP5 draws the
  ACOG over its iron sight and the plain W1200 the reflex sight. VERIFIED
  (screenshots of a live session).
- During a melee (`weapAnim`, toggle off, 8 or 9: nodes 7 and 8, the
  `viewmodel_M4m203_knife_melee_*` clips) the weapon's `knifeModel` is
  grafted at the hands' `tag_knife_attach`, and dropped once `weapAnim`
  leaves them. INFERRED (the knife test,
  The view model rebuild). 102 of the 111 view weapons carry a
  knife (`viewmodel_knife`), each whole in `common_mp`, and every hands
  model has the tag. VERIFIED (`view_census`). The clip runs on after
  the knife goes until it ends, then idle plays (the 0 rule above).
- 1.7's stock binds put `+melee` on V and `mp_QuickMessage` on B
  (`default_mp_controls.cfg`, a rawfile of
  `localized_code_post_gfx_mp.ff`). VERIFIED. vcod takes them over its
  CoD1 and CoD2 defaults (`Shell::use_cod4_binds`).

### 4.3 The gun's place

- The gun's offset off the eye is the lean's (`(1 - frac) * f * 1.6`
  along the roll's right), the stance's move offset eased toward its
  target past the stance's min speed plus the ducked or prone offset,
  less the sway offset; its angles are the lean's roll, `adsAimPitch`
  times the sight fraction, the eased move turn, the idle drift
  (`weapIdleTime` on the hip or ADS idle speed), the bob, less the sway
  angles; the view weapon is drawn in view space at those, at the
  world's fov. INFERRED (the weapon position step,
  `BG_CalculateWeaponAngles`, `BG_CalculateWeaponPosition_Sway`,
  `DiffTrack`).
- The bob: the cycle `bobCycle / 255 * 2 pi + 2 pi`, plus pi / 4, the
  speed `xyspeed * 0.16`, the vertical and horizontal factors capped at
  10, the roll's at the cycle less 0.4712 and 1.5 times the speed, then
  each times -1 (the roll kept only when negative), scaled down by
  `adsBobFactor` with the sight. VERIFIED (iw3mp.exe 0x41b0d0: the
  doubles 255 at 0x70aee0, pi 0x70b080, 2 pi 0x70b078, pi / 4 0x70b1c0,
  0.16 0x70b688, 1.5 0x70ada0, 0.4712 0x70b680, -1 0x70b2d8; the cap
  10.0 at 0x6bdf70; `adsBobFactor` at `[edi+0x448]`, 1096).
- The fov is `CG_GetViewFov`'s: `cg_fov`, the sight's `adsZoomFov`
  eased in over the raise's last `adsZoomInFrac` (out over
  `adsZoomOutFrac`), 55 on a turret, 90 at the intermission. INFERRED.
- The damage kick, the gun recoil (`BG_WeaponFireRecoil`) and the
  landing dip come in through `play::cod4_kick` (section 14). Left out:
  the shellshock's sway scale. Predicted, `xyspeed`, `bobCycle`, the eye's height and lerp,
  `leanf`, `pm_flags` and `eFlags` are the predicted state's (section 5);
  unpredicted, `xyspeed` is the lerped playerstate velocity's.

### 4.4 What the captures draw

| capture | frames | view weapon drawn | own shots | fire clips started |
|---|---|---|---|---|
| `kill-shooter` | 1020 | 863 (`m16_gl_mp`) | 30 | 30 |
| `walk` | 2525 | 2428 (2362 `m16_gl_mp`, 66 `frag_grenade_mp`) | 18 | 19 |

VERIFIED (the gates at 60 frames a second). Every own shot flashes on the
view model's `tag_flash`, 33.8 to 34.9 units ahead of the eye, 2.6 to 3.3
right of it and 2.0 to 3.2 below, along the view, and ejects once
(cod4-effects.md 10). That the hands and gun look right on screen is
pending a human's look.

### 4.5 The reflex sight's dot

- The dot (`mc/mtl_weapon_reflex_red_dot`, and `mtl_weapon_red_dot` on
  the G36C) uses the set `mc_reflexsight`, whose techniques 4, 5, 6, 24
  and 25 run `reflexsight.hlsl`; its lit state is (One, InvSrcAlpha), no
  depth write, `sortKey` 59, its constant `detailScale` (25, 25). The
  colour map `weapon_red_dot_reflexsight` is sampled with state 0xEB and
  the detail map `weapon_red_cross_background` with 0x0B. VERIFIED
  (`common_mp.ff`). 0xEB clamps u, v and w. INFERRED.
- The vertex shader takes the unit direction from the eye to the vertex
  in view space and dots it with the binormal (`binormalSign` times
  normal cross tangent) and the tangent, each times `detailScale`, plus
  0.5: the colour map's coordinates, so the dot sits at infinity along
  the lens's normal. The detail map reads the mesh's own coordinates.
  The pixel shader takes m, the two maps' red channels multiplied, and
  writes (2m, 1.2(m - 0.2), 1.2(m - 0.2), 1.2m). VERIFIED (the
  technique's bytecode, disassembled with `tools/re/d3d9dis.py`).
  Drawn with the model's coordinates instead, the opaque DXT1 dot map
  tiled three white dots over a black lens. VERIFIED (a live session).
- vcod's `viewmodel.wgsl` draws the dot as the shaders do but without
  the detail map (one texture a stage), and blends (SrcAlpha,
  InvSrcAlpha) with the colour divided by its alpha, so the dot's faint
  rim reads dimmer red than retail's premultiplied blend. The glass
  (`mc/mtl_weapon_reflex_lens`, `mc_unlit`, (SrcAlpha, InvSrcAlpha) with
  alpha `> 0`) draws its colour map's alpha. Every material of the 529
  weapon models (guns, world models, hands, knives) compiles to its lit
  state's blend, alpha test, cull and depth write. VERIFIED
  (`weapon_view`). The ACOG reticle's (One, InvSrcAlpha) draws as is:
  its shader writes its colour as is and its colour map is
  premultiplied (cod4-effects.md 13).

### 4.6 The scope hides the gun

- `CG_AddViewWeapon` (`iw3mp.exe` 0x457010) zeroes the refdef's view
  model depth of field (0x457026, 0x45702d, into 0x797660 and 0x797664),
  then clears its draw-gun flag (`[esp+0x10]`, set to 1 at 0x457035)
  when `cg.cubemapShot` (0x74e344) is set, `cg_drawGun` (0x74a8dc) is
  off or `CG_GetWeapReticleZoom` (0x430120, called at 0x45707b) returns
  true. VERIFIED (the compares and the call); the globals' names
  INFERRED. The flag only skips the draw and the view
  model's depth of field; the pose, tags and notetracks still run
  (`CG_AddPlayerWeapon`'s draw-gun flag, INFERRED).
- `CG_GetWeapReticleZoom` is true for a weapon with an `adsOverlayShader`
  (+1072) or an `adsOverlayReticle` (+1080), its sight's lerp past
  `1 - adsZoomInFrac` (+1064; `adsZoomOutFrac`, +1068, going down,
  read off the sight-up flag) by more than 0.01 of that
  fraction. The test happens before `CalculateWeaponPostion_PositionToADS`
  updates the direction that frame. INFERRED; the
  call order is VERIFIED at 0x45707b. The M40A3, R700, Dragunov and Barrett
  carry both keys with both fractions 0.05, so the gun goes once the
  lerp passes 0.9505. The M16 carries neither. VERIFIED (`common_mp.ff`,
  `cod4_viewmodel`'s `a_scoped_rifle_hides_its_gun_under_the_overlay`).
- vcod hides the gun the same way. Drawn under the overlay, its scope
  model (lens and tube) fills the overlay's clear centre a few units from
  the eye, under the view model's depth of field: on mp_crash 0 of 4096
  centre pixels matched the bare view, mean brightness 46 against 152.
  With the gun hidden, 3899 of 4096 match, 150 against 152 (the rest is
  the reticle's lines). VERIFIED
  (`renderer::cod4_scope_live::cod4_live_sniper_scope_shows_the_world_through_its_centre`,
  lavapipe, the stock Sniper class's M40A3 on vcod-server).
- The ADS depth of field stays on under the scope (section 12):
  `CG_UpdateAdsDof` has no overlay test. INFERRED.

## 5. Prediction

`play::cod4_predict`, wired by `Cod4Session::scene` once
`Cod4Session::set_world` hands it the map's `ClipMap` and the zones'
weapon scales (`cod4_fly::load` builds them for a watched session).

### 5.1 What retail does

- `CG_PredictPlayerState` predicts unless the view is followed
  (`otherFlags` 2) or a demo plays: it copies `cg.nextSnap`'s playerstate,
  keeps its own `oldVelocity`, and runs `Pmove` for every kept cmd past
  `commandTime` up to the newest, `oldcmd` the cmd before each; a linked
  body (`pm_type` 1 or 8) interpolates instead. The trace is
  `CG_TraceCapsule`: the world, then `CG_ClipMoveToEntities` over the
  snapshot's solid entities but our own, a packed `solid` as the box a
  unit inside it on every side (`CONTENTS_BODY` for a player, solid for
  the rest), a brush model through its cmodel at the entity's drawn
  angles. INFERRED, not yet confirmed on 1.7
  (`CG_ClipMoveToEntity`, `CG_GetEntityBModelBounds`).
- Before it runs the cmd last frame's prediction ended at, the replay's
  origin is compared with last frame's: past 0.1 the difference is
  added to `cg.predictedError` (what is left of the last one first
  scaled by `(cg_errordecay - (cg.time - predictedErrorTime)) /
  cg_errordecay`), `predictedErrorTime` set to `cg.oldTime`; the view
  adds that fraction of the error while it is in (0, 1). A respawn, a
  teleport (`eFlags` 2 toggled), another view client or a new `stats[4]`
  clears the error. INFERRED, not yet confirmed on 1.7
  (`CG_PredictPlayerState_Internal`, `CG_CalcViewValues`,
  `CG_TransitionSnapshot`'s playerstate arm).
- `cg_errordecay` defaults to 100, at least 0, at most `FLT_MAX`.
  VERIFIED (iw3mp.exe 0x43c37d loads the float 100.0 at 0x6f9e58 as the
  default, 0x43c393 `fldz` as the minimum, 0x43c399 `FLT_MAX` at 0x70af58
  as the maximum, ahead of the register call at 0x43c3d6 with the name
  at 0x6ca2cc and "Decay for predicted error" at 0x6ca2b0).
- A step in `PM_StepSlideMove` adds its rise to `pmove_t.viewChange`
  once per cmd (`viewChangeTime`), starting each frame from
  `cg.stepViewStart`; a change of at least `cg_viewZSmoothingMin` (1)
  restarts the ease with what is left of the last, capped at
  `cg_viewZSmoothingMax` (16), and `CG_SmoothCameraZ` lowers the eye by
  `(1 - t / cg_viewZSmoothingTime) * change` over 0.1 s. INFERRED, not
  yet confirmed on 1.7 (`CG_SmoothCameraZ`). `vcod_cod4::pmove` reports each step
  as `Pmove::view_steps`.

### 5.2 What vcod does

- The cmds. Retail stamps a cmd with `cl.serverTime` every frame. vcod's
  live cmds, and the probe's in every capture, go out every 33 to 100 ms
  stamped with the newest snapshot's time plus its age, which leads the
  drawn time by about a snapshot interval. The prediction keeps that
  clock (its offset off the session's ms slewed a ms a snapshot, as
  `CL_AdjustTimeDelta` slews, so arrival jitter does not step it): it
  replays the cmds sent by then (a capture's: its client's, by their
  stamps) and puts the frame's own cmd on top, stamped at the clock with
  the keys and view of the cmd that covers the moment (live, the last
  sent). The frame's cmd is not kept: the next frame builds its own off
  the same kept cmds, so a step it takes is counted once, its change from
  last frame's. INFERRED (vcod's design; retail keeps every frame's cmd).
- The rules are the dvar mirror's (`CS_CODINFO` names, the values 128 on;
  cod4-pmove.md 12), each name the mirror leaves out at its default; the
  held weapon's scales come from the zones' `WeaponDef`s by the level's
  weapon list.
- The eye is the predicted origin plus the predicted `viewHeightCurrent`
  (pmove's lerp), plus the correction left, less the step's ease; the
  view's angles are pmove's `viewangles`; the view weapon and its bob read
  the predicted state (section 4.3).
- The weapon half runs in the replay with the level's weapon defs
  (`Cod4World::level_defs`: the zones' `WeaponDef`s by the level's
  weapon list, each ammo and clip slot the first earlier weapon's of its
  name and the alt linked, as the server registers them), not for a
  spectator, as vcod-server's think leaves it out; the view weapon reads
  the predicted `weapon`, `weaponstate`, `weapAnim`, `weaponTime`,
  `weapFlags`, `fWeaponPosFrac` and clip (section 5.4 for its events).
- Left out: item pickup prediction, movers carrying the body
  (`CG_AdjustPositionForMover`) and the ADS view error.

### 5.3 What the gate measures

`walk_capture_predicts_its_own_body` plays `walk` at 60 and 144 frames a
second, unpredicted and predicted; the cuts' gate plays the kill pair
predicted. VERIFIED (the gates):

| | largest level eye step, unpredicted / predicted | largest eye step |
|---|---|---|
| `walk`, 60 Hz | 6.57 / 3.40 | 6.61 / 4.36 |
| `walk`, 144 Hz | 2.98 / 1.48 | 3.11 / 1.96 |

A run at 190 units a second covers 3.17 a 60 Hz frame and 1.32 a 144 Hz
one; the frame's cmd is stamped in whole ms, 6 to 8 at 144 Hz. The
largest steps left are a jump's takeoff (about 250 units a second up).
The two 100 ms beats of section 3 are gone; the eye still cuts only at
the two spawns.

| capture | snapshots corrected | largest correction | past 0.1 |
|---|---|---|---|
| `walk` | 21 | 0.0129 | 0 |
| `kill-shooter` | 0 | 0 | 0 |
| `kill-target` | 2 | 3.169 | 2 |
| `fly` | 113 | 0.0004 | 0 |

VERIFIED (the gates' counts). The target's two misses are its first
hit's snapshots (`damageEvent` 1): the server's knockback, which the
client cannot predict. VERIFIED (the misses' `damageEvent`). The rest
are the last bits of `cod4-pmove.md` 13.2's float model.

That the predicted view looks right on screen is pending a human's look.

### 5.4 The predicted events

- Predicting, `CG_PredictPlayerState` ends with `CG_TransitionPlayerState`
  on the predicted state against last frame's (`transPlayerState_t`:
  `eventSequence` and `events[4]`), and `CG_TransitionSnapshot` runs the
  snapshot's playerstate through it only when it does not predict (a
  demo, a followed view, `cg_nopredict`, `cg_synchronousClients`).
  `CG_TransitionPlayerState` keeps `eventSequence` and `events[4]` of
  the state it fired (the copy at `iw3mp.exe` 0x446250: +4 the
  sequence, +8 the four events). VERIFIED (the disassembly).
  `CG_CheckPlayerstateEvents` (0x446170) takes the old sequence 256
  lower when it is more than 64 past the new one (0x446183..0x446191),
  then walks the new sequence's last four: one at or past the old fires,
  one under it fires when it is above the old less 4 and its event
  differs from the old ring's (0x44619a..0x4461b9), each through the
  event handler at 0x435ce0 with its parm byte (+0xc8). VERIFIED (the
  disassembly).
- vcod: `cod4_predict::new_events` is that test, on the wire's 8-bit
  sequence (pinned by `new_events_picks_new_and_corrected_slots`);
  `Cod4Session::scene` fires the predicted ring's new events
  with the predicted origin and weapon, and `walk_snapshot` leaves a
  playerstate the scene predicts alone. One ring state carries over
  between the two, so an own view that stops or starts predicting (a
  spawn, the killcam) fires nothing twice. A predicted view with no ring yet (no
  scene since the gamestate) starts from the snapshot's (cod4-effects.md
  12.2). The window runs the scene
  before it plays the frame's events (`Cod4Client::pump`), so they play
  off this frame's eye and view weapon tags. INFERRED (vcod's design).
- Replayed at 60 frames a second, the predicted own events against the
  snapshots' ring read straight: equal for the kill shooter (29
  `EV_FIRE_WEAPON`, the last shot, the reload's four) and the walk (18
  shots, the frag's three offhand events, footsteps, the jump, the
  landing, 102 stance refusals) and the mantle, whose putaway the
  weapon half predicts as retail sends it (`Mantle_IsWeaponInactive`,
  cod4-pmove.md 17.4). VERIFIED (the gate
  `play::cod4::tests::predicted_own_events_fire_once`).


## 6. A capture's view between its cmds

- Live, retail builds a cmd every frame off the mouse, so the own view
  turns every frame. Playing a demo (`cg.demoType` set) it predicts
  nothing: `CG_InterpolatePlayerState` lerps the two snapshots'
  `viewangles` and `delta_angles` the short way round. INFERRED (`CG_PredictPlayerState`, `CG_InterpolatePlayerState`).
- vcod replays a capture as live (section 5), and a capture's cmds come
  16 to 100 ms apart; a frame's own cmd taking the next cmd's view turns
  the view in those steps. So the frame's cmd eases its
  angles from the cmd before it to the next by the frame's time between
  them, each the short way round in shorts (`play::cod4::eased_cmd`). A
  gap over 250 ms, or a turn faster than 0.36 degrees a ms (a scripted
  view, or the probe making up a `delta_angles` change: the shooter's
  65 to 155 degree snaps one 19 ms cmd apart), is taken at once. This is
  replay-only smoothing; retail has no counterpart. INFERRED (vcod's
  design).
- The walk capture at 60 Hz turns on 183 frames instead of 92, its
  largest yaw step a frame outside the cuts 0.27 degrees instead of 1.29.
  VERIFIED (the gate `capture_view_turns_every_frame`). A followed view
  (the killcam) still lerps the snapshots' `viewangles`, as retail's
  demo arm does.

## 7. The lean

`CG_OffsetFirstPersonView` moves the eye last by `AddLeanToPosition(vieworg,
refdefViewAngles[1], leanf, 16, 20)`: the eased lean `(2 - |leanf|) *
leanf` times 20 along the right of (0, yaw, 16 times it), then holds the
eye 8 over the feet. Nothing rolls the view: `refdefViewAngles[2]` is the
playerstate's roll. INFERRED. vcod's scene does the same to the
predicted eye and to a followed player's, whose `leanf` is the
snapshots' lerp (`play::cod4`, `vcod_common::pmove::aim::add_lean` in
the Windows build's arithmetic). The lean itself is pmove's
(cod4-pmove.md 18): before this the eye stayed upright over the body
while the view weapon leaned. A by-eye check of the drawn lean is pending.

## 8. The shellshock

The drawn snapshot's `shellshockIndex` (1..15, configstring 1954 + it
names the shock), `shellshockTime` and `shellshockDuration` start the
view's shock each frame (`CG_StartShellShock`, then
`CG_UpdateShellShock` and the shellshock blend); an index of 0 ends it.
INFERRED. The
record is the zone's `shock/<name>.shock` (cod4-combat.md 10.8). Per frame
at `time` ms into a shock of `duration`:

- The screen. A `flashed` record saves the frame on the shock's first
  frame and from the next on draws the material `shellshock_flashed`
  over the 3D view: the still times `grab` plus `white`, blend One One,
  alpha 1 (`shell_shock_flashed.hlsl`: `texld r0; mad oC0.xyz, r0, v0.w,
  v0`; state bits 0x19288922, op 1 add, both factors 2 one). `white` and
  `grab` are `BlendSmooth` (`(sin((p - 0.5) * pi) + 1) / 2`) of the time
  left over `screenFlashWhiteFadeTime` and `..ShotFadeTime` (1 while more
  is left), each a byte of the vertex colour. A `blurred` record draws
  the material `shellshock`: the last capture over the frame at
  `0.01 ^ (age / fade)` (capped at 0.99), `screenBlurBlendTime` scaled
  down over the last `..FadeTime`, a quarter of the way to its luminance
  (`shell_shock.hlsl`, the same constants as CoD2's), then this frame
  captured. VERIFIED (both techsets' pixel shaders and state bits,
  `code_post_gfx_mp.ff`, `techset_shaders` and `tools/re/d3d9dis.py`);
  the order and factors are INFERRED. Both
  stock grenade records are `flashed`: the flashbang's white holds until
  3.5 s before the end and its still until the last second; the
  concussion's fades over 7.5 and 15 s, so its white never reaches full.
  VERIFIED (the rawfiles).
- The camera: 1.1's kick (`cod11-shellshock.md`), the same 128 points,
  `time * 0.001 / period` along them, `radius` eased out over the last
  3 s, turning the drawn view only (`CG_PerturbCamera`).
- The mouse (`lookControl`): the sensitivity scale and the turn caps (in
  degrees a second), eased back over `lookControl_fadeTime`. The
  concussion's record caps the turn at 35 and scales the mouse by 0.1;
  the flashbang's leaves the mouse alone.
- The sound: the channel volumes and the room (`SND_SetChannelVolumes`
  priority 3, `SND_SetEnvironmentEffects` priority 2) fade in over
  `soundFadeInTime` and out over `soundFadeOutTime`, ending
  `soundModEndDelay` past the shock; the record's loop blends toward its
  silent twin over `soundLoopFadeTime` and stops `soundLoopEndDelay`
  past the shock plus that fade, its end plays once at the delay, and a
  shock cut short plays its abort. The flashbang's are
  `flashbang_tinnitus_loop`, `_loop_silent`, `_end`, `_abort`.
  INFERRED; the aliases are in the zones (VERIFIED,
  `cod4_shock::the_zones_hold_the_flashbang_and_concussion_records`).

In `iw3mp.exe` the update is 0x44d020: with a nonzero start and `cg.time`
at or past it, it runs the sound (0x44cbc0), the mouse (0x44ce30) and the
camera (0x44cf10); otherwise it calls 0x44cb30, which plays
`shellshock_end_abort` (0x6c16bc, pushed at 0x44cb77), and resets the
mouse scale to 1 and the caps and view deltas to 0. The flashed blend is
0x44ca00 (two calls to `BlendSmooth`, 0x44c9a0, then the draw command at
0x5f77e0), the blurred one 0x44c8d0 (0x5f7770). The mouse arm reads the
fade share as `(left / lookControl_fadeTime)` below the fade time, scales
the sensitivity toward 1 by it and divides both caps by it. INFERRED (the
call graph, the string reference and the stores at 0x79e818 and
0xc84f90/94).

vcod (`play::cod4_shock`, the renderer's `set_cod4_shock`,
`AudioSystem::set_cod4_shellshock`) runs those formulas on the newest
snapshot at the render time. Its own engine fades are a straight lerp
of the mix by the fade's share, its 33 channel volumes land on vcod's
eleven channels by name (`auto`, `menu`, `weapon`, `voice`, `item`,
`body`, `local`, `music`, `announcer`, `shellshock`, `auto2d`), the
saved still and the blur's age are on the wall clock, and the
prediction slows the walk under a record that sets `bg_shock_movement`
as the server does (cod4-combat.md 10.8). The unit gates
`cod4_shock::tests` hold the screen, sound and mouse curves. A
by-eye check of the white-out and a by-ear one of the tinnitus are
pending.

## 9. The action slots

`default_mp_controls.cfg` (`localized_code_post_gfx_mp.ff`) binds `n` to
`+actionslot 1` (night vision), `5` to `+actionslot 3` (the grenade
launcher), `6` to `+actionslot 4` (C4, claymore and the hardpoints) and
`7` to `+actionslot 2`. VERIFIED (the rawfile). `_class.gsc` makes slot
3 `altMode` on a primary with an alt (`setActionSlot(3, "altMode")`)
and slot 1 `nightvision`; the server sends `actionSlotType[0..3]` and
`actionSlotParam` (cod4-netfields.md). VERIFIED (the script).
`+actionslot` (`iw3mp.exe` 0x457960, the handler its registration
stores at 0x42af49) acts on the press. It does nothing without a next
snapshot or when that snapshot's `otherFlags` lacks 4 (`POF_PLAYER`,
0x4579a3), while the weapon drops (`weaponstate` 3 or 4, 0x457974), on
a `PMF_RESPAWNED` or `PMF_FROZEN` body (`pm_flags` 0xc00, 0x457986),
with weapons off (`weapFlags` 0x80, 0x457996) and inside 250 ms of the
last press that acted (0x4579b2). It tests no vehicle bit. A `weapon`
slot (type 1) selects its weapon when held and not in hand; pressed on
the selection it goes back to the last primary selected (0x79e6ac,
written by the select at 0x458f7d) when that is held and another
weapon, else to the next weapon (0x4576d0). An `altMode` slot (type 2,
0x4578b0) does nothing in states 2..4, else selects the selection's
alt (+0x540 of its def), else the alt of the next held weapon after the
selection with no `inventoryType` and an alt, looked for with ammo
first and then without (0x457630). `nightvision` (type 3) puts 0x40000
in `cg.extraButtons` (0x84ca8c), which the next cmd carries once. A
press that acted stamps the clock, and a `weapon` slot that acted adds
0x20 while `eFlags` 0x300 is set (0x457a9a..0x457ab5). VERIFIED (the
disassembly). vcod takes the first next primary with an alt without
the ammo pass, and keeps its last primary from the selection at each
press, not from every select. INFERRED (vcod's design). vcod
(`play::cod4_input::Cod4Input::action_slot`, `take_extra`) does the
rest; the selected weapon goes in the cmd's weapon byte, which the
server's pmove switches to (cod4-combat.md 10.3,
`the_grenade_launcher_fires_its_grenade`). VERIFIED (the gate
`cod4_actionslot_3_toggles_the_grenade_launcher`).

## 10. Out of ammo

- `CG_EntityEvent`'s EV_NOAMMO (0x43633b) plays the weapon's
  `emptyFireSound`, `emptyFireSoundPlayer` in the player's view (+0x190,
  +0x194), unless the weapon is `clipOnly` (+0x500) or
  `cancelAutoHolsterWhenEmpty` (+0x508); in the player's view it then
  calls `CG_OutOfAmmoChange` (0x457560, at 0x436376). VERIFIED (the
  code).
- `CG_OutOfAmmoChange` does nothing from `pm_type` 7 on or for a weapon
  with `cancelAutoHolsterWhenEmpty`; an empty hand with
  `weaponLatestPrimaryIdx` held selects that; an alt mode
  (`inventoryType` +0x13c = 3) whose base weapon (`altWeaponIndex`
  +0x540) is not held selects nothing (the alt mode check,
  0x457510, which prints `Player is holding alt-mode weapon "%s", but
  does not posses it's original, "%s".`), one whose base weapon has
  ammo selects it, and otherwise the primary cycle (0x4576d0, both arguments 1)
  runs, which on an alt-mode selection selects its base weapon and
  else steps to the next held weapon by index that has rounds (clip
  0x7947d0 plus reserve 0x7945d0) and is no alt mode. VERIFIED (the
  code).
- So the M16's grenade launcher (`gl_m16_mp`, `inventoryType` 3, not
  `clipOnly`) clicks `weap_dryfire_pistol_plr` on its last round, as
  pmove raises EV_NOAMMO when clip and reserve read 0 after the shot,
  and goes back to the rifle (`m16_gl_mp`). VERIFIED (the zone's def;
  the code). vcod runs it on the predicted ring's EV_NOAMMO
  (`Cod4Session::out_of_ammo_change`, the selection going out in the
  next cmd); the empty-hand arm is left out. Gate:
  `cod4_out_of_ammo_picks_as_cg_out_of_ammo_change`.


## 11. The hardpoint vehicles

What 1.7's cgame does with an airstrike's jets (`ET_PLANE`, 13) and the
helicopter (`ET_HELICOPTER`, 12), and what vcod does
(`cod4_vehicles`, `play::cod4::visual`). The script's own effects and
sounds (`_hardpoints.gsc`, `_helicopter.gsc`) ride the events of
cod4-effects.md and cod4-sound.md.

### 11.1 Retail

- A plane is drawn as a script mover: `CG_ProcessEntity` updates its
  compass entry and falls into `CG_ScriptMover`, its `index` model in
  its bind pose. INFERRED. Its jump-table slot is the script
  mover's in `CG_ResetEntity` too (iw3mp.exe 0x44d2eb: `eType` 6 and 13
  both index case 3 of the byte table at 0x44d550, 0x44d4d6). VERIFIED
  (the table).
- A helicopter is a vehicle: `CG_VehProcessEntity` poses and draws it.
  INFERRED. Its model is `un2`'s, `index` 0. VERIFIED
  (`play-1.7-hardpoint.txt`, cod4-gsc.md 3.2).
- Its anim tree is the cgame's own, `"helicopter"` (0x6c82fc) with one
  clip, `bh_rotors` (0x6c8308), built in `CG_GetHelicopterAnims`
  (0x432ae0). VERIFIED (the strings and their pushes). On the DObj's
  creation an `eType` of 12 (`cmp ecx,0xc` at 0x435288) sets that clip's
  goal weight 1.0 over a 0.2 s blend at rate 1.5 (the floats at 0x70b198
  and 0x70b4e0). VERIFIED (the constants). That the clip then runs on
  the client's clock from the DObj's creation is INFERRED.
- The pose controllers lerps `lerp.u.vehicle`'s `bodyPitch`,
  `bodyRoll`, `gunPitch` and `gunYaw` between the two snapshots, and
  `CG_VehPoseControllers` turns `tag_body` by the body's pitch and roll,
  `tag_turret` by the gun's yaw and `tag_barrel` by the gun's pitch and
  a spin that firing winds up. INFERRED.
- The vehicle effects step's helicopter arm (from 0x44f568, `eType` 12) plays
  the rotor wash and the damage stage's smoke. INFERRED (the code's
  flow).
  - The wash's trace: a 0.5 box (0x70b42c, 0x6be648) 1200 down
    (0x70bb50) from the entity's `trBase`, mask 0x831 (0x44f5e1).
    VERIFIED (the constants).
  - Its effect: `treadfx/heli_water` (0x6cc040) where the surface type
    is 20 (`0x1400000` at 0x44f6d8), else `treadfx/heli_dust_default`
    (0x6cc024), on the ground point, its forward up. VERIFIED (the
    strings, the compare); the forward is INFERRED.
  - Its interval: `(int)(((dist - 350) / 850 * 0.1 + 0.05) * 1000)` ms
    after a hit (350, 850, 0.1, 0.05 and 1000 at 0x70b7c0, 0x70bb40,
    0x70bb38, 0x70ae08, 0x70af28), 1000 ms after a miss (0x44f62e).
    VERIFIED (the constants).
  - The smoke: none at stage 3 (`cmp [esi+0x1a0],3` at 0x44f712); stage
    0 `fire/fire_smoke_trail_L` and 1 `smoke/smoke_trail_black_heli` on
    `tag_engine_left`, 2 `smoke/smoke_trail_white_heli` on
    `tag_engine_right` (the media at 0x84f2a4, 0x84f2a0, 0x84f29c,
    registered at 0x43f0f8..0x43f0ae); the next 50 ms on (0x44f7ba).
    VERIFIED (the code's loads and constants).
  - Each smoke puff is bolted to its tag (`FX_PlayBoltedEffect`).
    INFERRED.
- `CG_FireWeapon`'s helicopter arm (`eType` 12) plays the weapon's
  world flash bolted to the helicopter's own `tag_flash`, ejects its
  brass, and winds the barrel's spin up by `heli_barrelRotation`; a
  player's shot flashes on his held weapon instead. INFERRED
  (`EV_FIRE_WEAPON` passes `tag_flash`).
- The two timers read `Sys_Milliseconds`, not the server's clock.
  INFERRED (the call at 0x44f701).
- An entering plane or helicopter takes its `eventSequence` as already
  played (0x44d4e6 and 0x44d4ff store it). VERIFIED (the code). The
  airstrike's `playPlaneFx` puts its four `playFxOnTag`s (the
  afterburners on `tag_engine_left` and `_right`, the contrails on the
  wingtips) on the plane in the frame that spawns it. INFERRED (the
  stock script). On retail those four reach the client in the plane's
  first snapshot: `play-1.7-planefx.txt` (`cod4_lnxded` 1.7, `war` on
  mp_crash, the probe gsc `fixtures/gsc/plane_fx` beside one idle
  `cod4_probe`) has the jet whose four `playFxOnTag`s ran in its spawn
  frame enter at 42450 with `eventSequence` 4 and the ring four
  `EV_PLAY_FX_ON_TAG` (56) with parms 1..4 (the `jet_afterburner` and
  `jet_contrail` tag pairs), and the jet whose four came 0.5 s later
  enter with an empty ring and `eventSequence` 4 at 43000. VERIFIED
  (the capture). So a client first seeing a stock jet in that snapshot
  takes its trails as played and draws none; a jet already in view when
  its effects come plays them. INFERRED (the capture over 0x44d4e6).
  vcod's `walk_snapshot` replays no entering plane's ring, and
  vcod-server sends the same rings on the same snapshots:
  `cod4::planefx_ab` holds both jets' entering rings and the late one's
  change time to retail's. VERIFIED (the gate, ours).
- There is no doppler: no alias flag carries one (cod4-sound.md 2) and
  `iw3mp.exe` holds no `doppler` string. VERIFIED (`strings`). That the
  engine has none is INFERRED.
- The helicopter's `playLoopSound` (`mp_cobra_helicopter`,
  `mp_hind_helicopter`) is its `loopSound`; the jets' loops ride
  `script_origin`s linked to them (`play_loop_sound_on_entity`); the
  sonic boom, the impacts and the crash's sounds are `playSound`s, each
  an `EV_SOUND_ALIAS` temp entity at the entity (the script sound start, INFERRED). The explosion that ends the crash is `playFx`'s
  `EV_PLAY_FX` temp entity. INFERRED (`Scr_PlayFX`).

### 11.2 vcod

- Both types draw their model (`play::cod4::visual`); a helicopter's is
  `un2`'s, or `index` from a vcod-server that sends it there.
- A helicopter is posed: `bh_rotors` (`vcod_cod4::anim::ENTITY_CLIPS`,
  loaded with the player clips) at rate 1.5 from its first drawn frame
  on the render clock, its weight eased in over 0.2 s, and the four
  controllers as local turns on the clip's pose
  (`Cod4Vehicles::place`). The barrel's spin is left out.
- The wash and the stage smoke run on the fx clock at the intervals
  above (`Cod4Vehicles::heli_fx`). The wash traces the clip map
  from the drawn origin, not the older snapshot's `trBase`, and meets
  no entity.
- `EV_PLAY_FX_ON_TAG` reads `CS_EFFECT_TAGS` (1698) + parm, `"%02d%s"`,
  and bolts the effect to the named tag of the entity's drawn model
  (`cod4_fx::effect_on_tag`, `Cod4Vehicles::bolt`); a model without the
  tag plays nothing. Any drawn model's tags pose: the bind pose, or the
  helicopter's posed one.
- A helicopter's shots flash on its posed `tag_flash` and eject on its
  `tag_brass` (`Cod4Client::draw` adds both to the frame's weapon tags,
  as it does a mounted gun's). With no tag kept the flash falls back to
  56 units over the origin, by the rotor, pointing up. On the retail
  capture the
  posed `vehicle_cobra_helicopter_fly`'s `tag_flash` sits 194 units ahead
  of its origin and 185 under it, its forward 0.86 along the body's.
  VERIFIED (`retail_hardpoint_capture_draws_and_poses_the_vehicles`,
  ours). The capture's helicopter fires no round, so the flash itself
  is pending a human's look.
- The sounds riding an entity follow it into the mixer's step
  (`Cod4Client::entity_positions`): a drawn model's drawn origin,
  anything else's newest `trBase`.
- Gates: `cod4_vehicles::tests::retail_hardpoint_capture_draws_and_poses_the_vehicles`
  (the retail capture: models, rotors, tags) and
  `cod4_live_hardpoints_draw_smoke_and_sound` (vcod-server's stock
  helicopter and airstrike through the live client: drawn models, an
  offscreen frame with the helicopter in it, stages 2, 1 and 0 smoking,
  the crash's and the airstrike's effects and sounds). VERIFIED (the
  tests, ours).
- The helicopter's compass entry is `cod4_radar`'s (cod4-hud.md 15.7).
  Left out: the wash's and smoke's lighting and the rotor blur's look,
  which a human checks.

## 12. The depth of field

What 1.7 blurs by depth, and what vcod does (`cod4_dof`,
`Cod4Client::depth_of_field`, the renderer's `DofPass`).

- `r_dof_enable` is registered at 0x62cc46 (a bool, default 1, flags 1),
  `r_dof_tweak` at 0x62cc8d (a bool, default 0, flags 0x80), and the
  tweak's floats after it: `r_dof_nearBlur` (default 6, 4..10),
  `r_dof_farBlur` (1.8, 0..10), `r_dof_viewModelStart` (2, 0..128),
  `r_dof_viewModelEnd` (8, 0..128), `r_dof_nearStart` (10, 0..1000),
  `r_dof_nearEnd` (60, 0..1000), `r_dof_farStart` (1000, 0..20000),
  `r_dof_farEnd` (7000, 0..20000), `r_dof_bias` (0.5, 0.1..3), each
  value and domain read off the floats the code loads (0x6bdf6c,
  0x70bbd8, 0x700c0c, 0x6c6300, 0x70bae8, 0x70b9b0, 0x70bc64, 0x6be648
  and their neighbours). VERIFIED (the disassembly).
  `r_znear_depthhack` defaults to 0.1 (0x62a46b, the float at
  0x6b72dc). VERIFIED (the disassembly).
- 1.7's `.vision` files carry no depth of field; it comes from the
  playerstate's `dofNearStart` .. `dofViewmodelEnd` (the netfields,
  `setDepthOfField` writing six of them) and the sight. VERIFIED (the
  netfield table, cod4-netfields.md). The spawn writes `dofNearBlur` 6
  and `dofFarBlur` 1.8 (cod4-gsc.md 9). VERIFIED (live). That it leaves
  the view model pair 0 is INFERRED (the capture lists no other `dof*`
  field).
- `CG_UpdateSceneDepthOfField`: with any of the four
  scripted distances set the frame takes the six fields; else
  `CG_UpdateAdsDof`. Off the sight (`fWeaponPosFrac` 0, alive) near
  0..0, far 5000..5000, near blur 6, far 0, which draws nothing. Up the
  sight the targets are near 1 to `nearEnd`, far from `farStart` to four
  times it, blurs 6 and 0: `nearEnd` 256 and `farStart` 2500 with no aim
  assist target on screen, `nearEnd` cut to 30 short of what an 8192
  capsule trace along the view meets (mask 0x806c31, at least 1), and
  `farStart` raised to it. Between the hip and the sight the values
  lerp by `fWeaponPosFrac`; fully up (or dead) each eases half the way
  per frame, at least 1, at most 50 (near) or 400 (far) or 0.1 (blurs)
  per 50 ms (`CG_UpdateAdsDofValue`). `CG_AddViewWeapon` sets the view
  model pair to the playerstate's lerped toward the weapon's
  `adsDofStart` and `adsDofEnd` (WeaponDef +2160 and +2164, the weapon
  file's keys at those offsets; VERIFIED) by `fWeaponPosFrac`. INFERRED. vcod runs these with no aim assist targets and the trace
  through the clip map's world only.
- `R_UsingDepthOfField`: a view model range over a unit, a
  near range over a unit, or a far range over a unit with a far blur.
  INFERRED. `R_SetDepthOfField` takes the tweak's dvars under
  `r_dof_tweak`, the cgame's under `r_dof_enable`, else zeroes. INFERRED.
- The chain is `code_post_gfx_mp`'s techniques, their shaders read off
  the bytecode (`--example techset_shaders code_post_gfx_mp <set>`,
  `tools/re/d3d9dis.py`). VERIFIED (bytecode):
  - `dof_downsample.hlsl` (resolved scene and `floatZ` in): a quarter
    target each way (its taps 0.25 of a target pixel off, two scene rows
    apart), colour the mean of 4x4 scene pixels, alpha the most of
    `min(saturate(eqScene.x z + eqScene.z), saturate(eqViewModel.x (-z)
    + eqViewModel.z))` over their 16 depths: `floatZ` is negative on the
    view model, so it reads its own equation there.
  - A gaussian of that into `postEffect0`, sigma `nearBlur / 4` at
    640x480 scaled by the scene's height (`RB_GetDepthOfFieldInputImages`,
    `RB_VirtualToSceneRadius`, `RB_GaussianFilterPoints1D`).
    INFERRED.
  - `dof_near_coc.hlsl`: colour the unblurred quarter's, alpha
    `2 max(blurred, own) - own`; `small_blur.hlsl`: four bilinear taps
    half a pixel out.
  - `postfx_dof.hlsl`: the scene sharp, its small blur (four bilinear
    taps at (0.5, -1.5) and its turns, 4/17 each, the centre 1/17), the
    quarter's medium and the gaussian's large colours, mixed by
    `saturate(coc * dofLerpScale + dofLerpBias)`; `coc` the quarter's
    near blur, widened by `saturate(eqScene.y z + eqScene.w) *
    eqViewModel.w` where `z` is under 1500000.
  - The constants (`RB_ApplyMergedPostEffects`, INFERRED):
    the scene equation off the near and far ranges and `viewParms.zNear`,
    the view model's off its range and `r_znear_depthhack`, `w` the far
    over the near blur to the bias; the small and medium fractions
    `(1.4 or 3.6 * 480 / height / nearBlur) ^ r_dof_bias`, the lerp
    scale `(-1/s, -1/(m-s), -1/(1-m), 1/(1-m))` and bias `(1, m/(m-s),
    1/(1-m), -m/(1-m))`.
- vcod draws the chain over the resolved world before the film and the
  glow, as `RB_ProcessPostEffects` orders them, at the
  bias's default. Its `floatZ` is the frame's depth: under the view
  model's slice of the depth range (0..0.2) it reads the gun's depth off
  its own planes, negated, so a world surface nearer than 5 units reads
  as the gun. `cod4_live_sight_blurs_and_smoke_feathers` joins
  vcod-server's war on mp_crash, turns to an open view, sights and
  checks the pump hands the renderer no depth of field at the hip and a
  used one up the sight (near 1 .. 246.7 there), and that the frame
  differs from the one drawn with `r_dof_enable` 0. VERIFIED (the test,
  ours).
- Left out: the aim assist targets' ranges, the killcam's hold
  (`CG_KillCamEntityEnabled`), `r_dof_tweak` and its dvars, the
  traced entities. The look is pending a hand check.

## 13. The gunner's view

What the cgame does for our own view while we man a gun (`eFlags`
0x300, `viewlocked_entNum` the gun; the server's side is cod4-gsc.md
9.2).

- `CG_CalcTurretViewValues` is iw3mp.exe 0x4507b0: under the predicted
  playerstate's `eFlags & 0x300` (0x794524) it puts the eye on the gun's
  `tag_player` (the error at 0x45081d is "Turret has no bone:
  tag_player"), and under `viewlocked` 2 (0x794a0c) with third person
  off adds `crandom()` (0x55efb0) times the gun weapon's
  `vertViewJitter` (`WeaponDef` +0x7f0) to the drawn pitch
  (`refdefViewAngles`, 0x79b698), then `crandom()` times
  `horizViewJitter` (+0x7ec) to the yaw. VERIFIED (the instructions).
- `CG_mg42_PreControllers` is iw3mp.exe 0x432be0: the gun we man
  (`eFlags & 0x300` and `viewlocked_entNum` == its number at 0x794a10)
  takes `refdefViewAngles` (0x79b698) as its view angles; any other gun
  lerps its `gunAngles` pitch and yaw. Both lerp the third angle, the
  barrel's spin. Then the anim: our own gun's goal is anim 1 always, any
  other gun's anim 2 while its `lerp.eFlags` 0x40 is set, else 1
  (0x432d8b..0x432dba). VERIFIED (the instructions). Anims 1 and 2 are
  the weapon's `idleAnim` and `fireAnim` (`CG_GetMG42Anims`), INFERRED: our own gun does not play its fire anim on retail either.
  `CG_mg42_DoControllers` turns `tag_aim` and `tag_aim_animated` by the
  view less the gun's angles (`AngleDelta`) for our gun, by the lerped
  angles for the rest, and `tag_flash` by the spin. INFERRED.
- `EV_FIRE_WEAPON_MG42` (`CG_EntityEvent`, INFERRED) shakes
  the camera (`CG_StartShakeCamera(0.05, 100, origin, 100)`) and fires
  through `CG_FireWeapon` on the gun's `tag_flash`: for the gun our
  playerstate mans, the flash step plays the weapon's `viewFlashEffect`
  and the fire sound is the player's; the client-side round
  (`DrawBulletImpacts`' `ET_MG42` arm) leaves `tag_flash` along the tag
  with the file's `playerSpread`.
- vcod (`cod4_client`): the drawn pitch is normalized to -180..180 before
  the camera takes it. The predicted gunner's view comes out of
  `PM_UpdateViewAngles`' turret arc in 0..360 (`AngleNormalize360`,
  cod4-pmove.md), so 15 up read 345; the camera's own pitch clamp held
  that at 89 down, and the next mouse turn, measured off the camera,
  pushed the cmd's view by the difference: pushing the mouse up past
  the arc made the view jump up and down. VERIFIED (ours,
  `cod4_live_saw_gunner_view`).
- vcod poses our own gun off the drawn view, puts the eye on its
  `tag_player` posed so, jitters the view as above, and hands the gun's
  `tag_flash` to the effects as the view's flash tag, so the flash and
  the tracer leave the barrel in front of the eye (with no tag to ride
  the flash plays 56 over the gun's origin, the smoke over the head), and
  an `EV_FIRE_WEAPON_MG42` draws a client-side round. VERIFIED (ours, the
  test).
- The map zone's `weapon_saw_mg_setup` (mp_backlot, mp_carentan) names
  `,mc/mtl_weapon_saw`, a stub; the material whole, its
  `mc_l_sm_r0c0s0` techset and `weapon_saw_col`, is in `common_mp`.
  An entity model's stub material comes from the zone that holds it
  whole (`cod4_fly::add_whole_material`); the stub alone draws the gun
  black. VERIFIED (the zones).
- vcod poses every gun with its weapon's `idleAnim` and
  `fireAnim`, decoded for a `weapClass` 7 weapon
  (`cod4_fly::WeaponModels::gun_anims`), before the controllers turn
  it: the goal is the idle for the gun our view mans and the fire for
  another while its `lerp.eFlags` 0x40 is on, set at full weight over
  0.1 s (the float at 0x6b72dc pushed at 0x432dc8, VERIFIED), the clip
  restarting from its first frame when it becomes the goal (vcod's).
  `cod4_live_saw_gunner_view`
  runs a spectator beside the gunner who poses gun 393 so and sees the
  fire anim take over while it fires, move its bones against the
  controllers-only pose, and give way to the idle after. VERIFIED (ours).
  That it looks right is pending a hand check.
- Left out: the jitter riding into our own gun's pose.

## 14. The kicks: recoil, the damage kick, the landing dip

Code: `crates/client/src/play/cod4_kick.rs` (state and steps), the
session's scene (`play::cod4`), `cod4_damage` (the hit), `cod4_client`
(the drawn view) and `cod4_viewmodel` (the gun). Addresses are
`iw3mp.exe` 1.7. Gates: `play::cod4`'s
`cod4_live_recoil_rides_the_cmd_and_the_server_takes_it`,
`cod4_live_landing_dips_the_eye_and_the_gun` and
`cod4_live_blast_pulses_the_overlay_and_points_an_icon_at_it` (its
damage kick), each a client joined to vcod-server's `war` on mp_crash.

### 14.1 The recoil

- `BG_WeaponFireRecoil` is 0x41a7b0, called from `CG_FireWeapon` at
  0x457d2e only when `cg.nextSnap->ps.otherFlags & 6` and the event's
  entity is the playerstate's client (0x457cc7..0x457cdc), with
  `vGunSpeed` 0x79e78c and `kickAVel` 0x79e748. VERIFIED (the calls and
  pushes). It reads the `WeaponDef` at the field table's offsets:
  `adsGunKickReducedKickPercent` +1756 (0x6dc) and the hip one +1828
  (0x724) while `weaponRestrictKickTime` (ps+0x50) is over 0, the ads
  view kick +1792..+1804 at a `fWeaponPosFrac` of exactly 1, the hip one
  +1864..+1876 otherwise; the gun kick's ads (+1760..) or hip (+1832..)
  ranges; each draw `rand() * (1/32768) * (max - min) + min` off the
  CRT's `rand` (0x670622); `kickAVel` = (-pitch, yaw, -yaw / 2) times the
  reduced share, the gun's speeds added to. VERIFIED (0x41a7cd..
  0x41a9f8). Which of the two gun ranges a frac between 0 and 1 takes is
  INFERRED (the sight fraction at or under 0); vcod's shared
  `Recoil` takes the sight's above 0.
- `CG_KickAngles` is 0x44fe20: 5 ms slices of `cg.frametime`, per axis a
  centring speed of the held weapon's `adsViewKickCenterSpeed` (+0x710)
  above a frac of 0.5, else `hipViewKickCenterSpeed` (+0x758), 2400 with
  no weapon (0x70b670), a move against the kick cut to 0.06 (0x70aef0),
  the angle clamped to 10 (0x6bdf70) and stopped at the centre.
  VERIFIED. These are CoD1's numbers, so vcod steps 1.7's kick with
  `play::recoil::Recoil`.
- `CG_DrawActiveFrame` runs it when `cg.nextSnap->ps.otherFlags & 4`
  (0x4529fb) and otherwise zeroes `kickAVel` and `kickAngles` (0x79e748..
  0x79e75c, 0x452a10). VERIFIED. `CG_DrawActive` hands `kickAngles` plus
  `offsetAngles` (0x79e760) to the client (0xc84f98, 0x42f831..0x42f89f),
  and the cmd's angles are `ANGLE2SHORT(cl.viewangles + those)`: the
  float times 182.0444 plus 0.5 through the truncating store, masked to
  16 bits (0x463aa9..0x463acf). VERIFIED. So the kick turns the aim: the
  server reads it off the cmd like a mouse turn, and the prediction draws
  it for the same reason. vcod adds `Cod4Kick::view_kick` to the cmd's
  view in `live_frame`; its `angle2short` truncates without the 0.5, a
  rounding difference of at most one short that predates this.
- Order: vcod steps the kick in the scene by the cg time's change, then
  takes the frame's own fire events (the predicted ring's, else the
  snapshot's), so the cmd the next step builds carries the stepped kick,
  as retail's `CL_Input` reads the last `CG_DrawActive`'s. INFERRED
  (`CG_DrawActiveFrame` order).
- The gate fires the M16's burst with the view at 0, 0: the kick moves
  the view over 0.1 degrees (the M16's hip pitch range is -30..60 a
  second, so it may go down as well as up), the server's `viewangles` of
  each probed cmd equal the client's own view of that cmd (cmd angles
  over `delta_angles`) to 0.01, the server's view moves as far, and the
  kick and the drawn view are back at 0, 0 within 2.5 s. VERIFIED (the
  gate, ours).
- Left out: the CRT `rand` is shared with every other caller, so the
  draws are vcod's own sequence (INFERRED); `offsetAngles` (the
  vehicle's, 0x79e760) is 0.

### 14.2 The damage kick

- `CG_DamageFeedback` (0x445d00): `kick = damageCount * bg_viewKickScale`
  clamped to `bg_viewKickMin`..`bg_viewKickMax`, 0.2, 5 and 90 (the
  dvars at 0x734ae4, 0x734a8c and 0x734b74, registered at 0x4095c9..
  0x4096e5). VERIFIED. With yaw and pitch both 255 the pitch is `-kick`
  and the roll 0 (0x445d5c); else the hit's direction
  (`AngleVectors(pitch / 255 * 360, yaw / 255 * 360)`) dotted with the
  view axis' forward (0x797624) times `kick` is `v_dmg_pitch`
  (0x79e738), and minus its dot with the left (0x797630) times `kick`
  `v_dmg_roll` (0x79e73c); `damageTime` (0x79e718) takes
  `cg.snap->serverTime`. VERIFIED (0x445d75..0x445f59). vcod measures
  the hit against the session's view (the mouse's, without the kick),
  not last frame's drawn axis. INFERRED.
- `BG_CalculateView_DamageKick` (0x41b8b0): with a `damageTime`, factor
  `1 - frac / 2`, times `1 + frac / 2` under a sight's overlay
  (`overlayReticle`, +0x438); under 100 ms `GetLeanFraction(t / 100)`,
  then `1 - GetLeanFraction(1 - r)` for `r = 1 - (t - 100) / 400` while
  `r` is over 0; pitch and roll each times it. VERIFIED (the constants
  0.5, 100, 400 and 2 at 0x70ace8, 0x70b0e0, 0x70b398, 0x70ae50). It
  turns `refdefViewAngles` only (0x4500f8 from
  `CG_OffsetFirstPersonView`), never the cmd. VERIFIED. That function
  returns at once on a gun (`eFlags & 0x300`, 0x450063), so a gunner's
  view takes no damage kick and no landing dip; vcod skips both there.
  VERIFIED (the branch). A gunner's rounds ring `EV_FIRE_WEAPON_MG42` on
  the gun's entity, not his playerstate (cod4-gsc.md 9.2), so
  `CG_FireWeapon`'s `isPlayer` test fails and no recoil kicks his view;
  vcod takes only the playerstate's own fire events. INFERRED. vcod adds the
  pitch to the camera and the roll to the draw.
- `BG_CalculateWeaponPosition_DamageKick` (0x41b2b0): factor
  `k = frac / 2 + 0.5`, deflect over `100 k` ms, return over `400 k`,
  under a sight's overlay `k (1 - frac * 0.75)` (0.75 at 0x70b0c8);
  pitch `+ f * v_dmg_pitch / 2`, yaw `- f * v_dmg_roll`, roll
  `+ f * v_dmg_roll / 2`. VERIFIED (0x41b2b0..0x41b3c3).
- The blast gate's frag lies at yaw 90 from a view at yaw 0: the view
  rolls right by 4 degrees or more, pitches under 1.5, and is level
  again within the frame loop's 3.5 s. VERIFIED (the gate, ours).

### 14.3 The landing dip

- The own `EV_LANDING_*` sets `landChange` (0x7975f4) to minus its parm
  and `landTime` (0x7975f8) to `cg.time` (0x435e43); a hurt landing's
  dip is `(h - 12) / 26 * 4 + 4` truncated, at most 24 and only when
  over 0, for the height `h = (bg_fallDamageMaxHeight - MinHeight) *
  parm * 0.01 + MinHeight` and only past 12 (0x435eb7..0x435f3e, the
  constants 12, 26 and 4 at 0x70b238, 0x70b6d0, 0x70b340). VERIFIED.
- `CG_OffsetFirstPersonView` (0x4502b2): `t = cg.time - landTime`; over
  0 and under 150 the eye goes down `landChange * t / 150`, from 150 to
  450 `landChange * (1 - (t - 150) / 300)`. VERIFIED (0x70b8a8,
  0x70b8a0, 0x70b010). The gun (0x455500): a quarter of it,
  `landChange / 4 * t / 150` under 150, `* (450 - t) / 300` under 450,
  on the world's up. VERIFIED. vcod turns that up into the view's frame
  for its view-space offset.
- The gate jumps in place: the landing's change is a unit or more, the
  eye drops at least half of it under its rest and is back to 0.01.
  VERIFIED (the gate, ours).

### 14.4 Pending hand check

The look is pending a hand check: a burst at the hip and up the sight (the view
climbs and settles, the gun spring jumps), a frag going off beside you
(the view rolls away from it for half a second, the gun swings), and a
jump and a drop off a roof (the eye and the gun dip).

## 15. An entity's hidden parts

- A script model's `hidePart(tag)` sets the tag's bone bit in its
  `s.partBits` (cod4-gsc.md, the walk capture), and the client draws the
  model without each surface whose `XSurface.partBits` meet them, as a
  view weapon's `hideTags` (4.2). INFERRED (`R_SkinXModel`, the DObj's hide bits off the entity's `partBits`).
- `_destructible.gsc` (`common_mp`) builds each destructible's parts at
  the setup and hides every later state's tag (`hideapart`); a part's
  state change hides its tag and shows the next state's. Of the sedan's
  windows the states are the intact glass on `tag_glass_<w>`, the
  cracked on `tag_glass_<w>_d`, then nothing; of its lights the lamp and
  its `_d`. VERIFIED (the rawfile).
- mp_crash's `vehicle_80s_sedan1_red_destructible_mp` LOD 0 carries all
  four glass layers (`mtl_80s_econ_glass_outside`, `_inside`,
  `com_glass_shattered_outside`, `_inside`); the setup's ten `_d` tags
  give `partBits` 0x242a, 0x92480000, 0, 0, the bits retail's snapshots
  carry for every sedan of the spectator capture, and under them only
  the intact pair is left; a window broken to its second state (its tag
  hidden, its `_d` shown) keeps only the shattered pair. VERIFIED
  (`weapon_view::a_destructible_sedans_parts_keep_one_glass_pair`, the
  zone; `play::cod4` `retail_sedans_draw_with_their_hidden_parts`, the
  capture).
- vcod: `Drawn::part_bits` is the newer snapshot's `partBits`;
  `Cod4Client::handle` uploads a model once for each set of hidden parts
  that takes a surface off it (`cod4_fly::Parts`, each entity model's
  surfaces' bits off `vcod_cod4::anim::model_parts`). Without it every
  sedan draws both glass pairs over each other, inside and out
  (cod4-client-perf.md 15).
