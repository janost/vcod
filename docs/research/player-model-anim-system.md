# CoD 1.1 player model & animation system

How a CoD1 player body is built, which anim indices the server picks and how both sides play them. Sources: `game.mp.i386.so` disassembly (symbols intact), CoDExtended and RTCW-MP source, and the pk3 assets. Claims with no label are VERIFIED; everything else carries its own. `clientstate-wire-format.md` covers how the model indices reach the client.

## Model assembly: body + attachments

A player is one body xmodel plus up to 6 attached xmodels. `character/mp_american_airborne01.gsc` (pak5.pk3):

- Body: `setModel("xmodel/playerbody_american_airborne")` -> `clientState.modelindex`.
- Head: `attach` of a random pick from `xmodelalias\head_allied::main()` (20 `basehead*` entries; 25 for airborne). `head_axis.gsc` is the Axis list.
- Helmet: `attach("xmodel/USAirborneHelmet")`, kept on the entity as `hatModel`.
- Gear, only when `character\_utility::useOptionalModels()` allows it (`g_useGear`): `xmodel/gear_US_load1`, `xmodel/gear_US_bandolier`, `xmodel/gear_US_ammobelt`.
- No call names a tag. At most 6 attachments, matching `attachModelIndex[6]`.

`attach(model)` with no tag grafts by the attached model's root bone name. `xmodelparts/USAirborneHelmet0` is version 14 with 0 non-root bones and one root, `bip01 head`. `basehead*` roots are shared spine and head bones (`bip01 spine2 / bip01 neck / bip01 head / skull / tag_eye / jaw / ...`). `attach(model, tag)` overrides the graft; the tag goes through `G_TagIndex` -> `attachTagIndex` (5 bits, 32 slots).

An attachment bone named like a body-rig bone must alias that bone, not append a duplicate. `basehead*` carries its own `bip01 spine2 / bip01 neck / bip01 head` chain, every stock `pb_*`/`pt_*` clip keys those names, and helmets root at the body's `bip01 head`/`tag_helmet`. Appended duplicates hold the bind pose while the body's copies animate, which splits the face from the helmet by 1.4 u standing and 9.4 u prone (measured on `USAirborne3` + `basehead2` at frame 0 of the stock clips). INFERRED from the asset shapes, not from a decompile of the engine's DObj merge. vcod merges by name against the base rig (`Skeleton::build_grafted`, `crates/common/src/skeleton.rs`).

Model choice is server-side; the client sees only the indices. The map `.gsc` sets `game["allies"]` -> `maps\mp\gametypes\_teams::modeltype()` -> `mptype/american_airborne.gsc` runs `switch(randomint(9))` over `character/mp_american_airborne01..09`, each of which picks a random head.

Tags on `xmodelparts/USAirborne3` (79 non-root bones + root): `tag_helmet`, `tag_helmetside`, `tag_belt_*`, `tag_thigh_*`, `tag_calf_*`, `tag_shin_*`, `tag_breastpocket_*`, `tag_weapon_left/right`, and the control bones `pelvis`, `back_low`, `back_mid`, `back_up`.

## Grounding: origin at the feet

Bind pose of `xmodelparts/USAirborne3` (parsed by `crates/common/src/xmodel.rs`):

```
tag_origin           0.00    0.00     0.00
bip01 l toe0        -0.99    4.44     0.33
bip01 pelvis        -3.44   -0.00    37.23
bip01 neck          -3.12   -0.00    59.82
bip01 head          -2.18   -0.00    62.68
tag_helmet          -1.17   -0.00    70.88
```

Root `tag_origin` sits at the feet. Draw the model with its root at `entityState.pos.trBase`, no Z offset; Q3's -24 waist offset does not apply. `standViewHeight = 60` (CoDExtended `bg_pmove.c:135`) agrees with `bip01 neck` at z 59.82; crouch is 40, prone 11. vcod's pmove uses mins z = 0.

## Animation indices: the animtree

`legsAnim`/`torsoAnim` are 10 bits in entityState (offsets 204/208) and playerState (112/120). As in RTCW (`ANIM_BITS 10`, `ANIM_TOGGLEBIT = 512`), index = `value & 511` and the restart flag = `value & 512`; a toggle change restarts the clip.

The index is a node in the global MP animtree, not a per-model config. `BG_FinalizePlayerAnims` (`game.mp.i386.so` 0x27d70):

- `numAnimations = trap_XAnimGetAnimTreeSize(playerAnimTree)`; `animations[0].name = "root"`.
- Per node: `trap_XAnimIsPrimitive / GetAnimName / GetLength / GetRelDelta (moveSpeed) / IsLooped`.
- `animation_t` is 92 bytes; the array cap `0xb800 / 0x5c` is 512, the 9 usable bits.
- The anim script is `mp/playeranim.script` (a string in the module); the tree is `animtrees/multiplayer.atr` (pak5.pk3, 11270 bytes).

`.atr` shape: `main { torso { pt_* } legs { pb_*, pl_* } }`. The file's header gives the prefixes: `pt_` torso only, `pl_` legs only, `pb_` full body. Non-leaf nodes (`proneMG42_fire`, `standMG42_aim`, with `: complete loopsync` and aim-angle children) take indices too; the client descends them by aim angle.

**Index order (VERIFIED against a running server and live traffic; `crates/common/src/animtree.rs`).** File order does not give the index. The array was read out of a running 1.1d dedicated server (gdb on `animations[]`, base `*(game.mp+0x7b3a8)`, stride 92, count at `+0xb800`) and checked against live wire traffic. The walk:

- Index 0 is a synthetic `root` whose children are the file's top-level blocks, `main` and `turning`, so `turning` shares the index space.
- Expanding a node appends its kept children as one contiguous block in reverse file order, then expands each child in turn, depth first. A group's subtree lands after everything already queued: legs' children 7..122, the MG42 sub-groups and their leaves 123..206, torso's children 207..253.
- A child is kept if an uncommented `both|legs|torso|turret <name>` line in `mp/playeranim.script` names it, an ancestor of it, or a node under it. Every block in 1.1's tree has a named node under it, so 1.1's array is the same under either of the last two rules.
- The engine keeps only anims whose names are already interned, so 18 never-scripted leaves (`pb_crouchwalk_loop`, `pb_sprint_B`, `pb_stand_death_spin`, the `_pistol`/`_unarmed` combatrun variants, ...) are missing and shift everything after them.

The tree flattens to 254 entries. The game module names the containers `unused` (`main`, `legs`, `torso`, `turning`, the MG42 aim sub-groups). Anchors: 85 `pb_combatrun_forward_loop_rifles`, 91 `pb_combatrun_right_loop`, 94 `pb_combatrun_forward_loop`, 122 `pb_stand_alert`, 236 `pt_rifle_fire`, 253 `pt_stand_shoot`.

**United Offensive's array.** The same walk with two additions:

- UO's script puts a legs and a torso anim on one line (`legs c_mp_jeep_passenger_idle torso c_mp_torso_stand_ads`) and spells one channel in capitals (`BOTH C_MP_BINOCULAR_ADS`). Every name on such a line takes an index (INFERRED, the anchors below).
- Its tree has a block no line names anything under, `vehicleJeepGunnerDriving`, and that block takes none (INFERRED, the anchors below).

With both rules the anims in the retail UO 1.51b captures land where retail sends them: 85 `c_mp_binocular_ads`, 86 `c_mp_binocular_idle`, 108 `pb_prone_aim`, 174 `pb_crouch_alert`, 185 `pb_stand_alert`, 311 `pt_reload_stand_rifle`, 318 `pt_rifle_fire`, 320 `pt_rifle_fire_prone`, 341 `c_mg_lmg_idle`, 356 `c_mp_binocular_ads_torso` (VERIFIED, the snapshots-uo pairs and `mp_cassino-dm-combat-uo.txt` against `animtree::tests::uo_indices_match_the_retail_captures`). The five jeep passenger torso anims the one-line form adds sit at 282..286; no capture holds one (INFERRED).

`mp/playeranim.script` is RTCW's format verbatim (`state {movetype {condition {both|legs|torso <anim>}}}`, conditions `weapons / weaponclass / position / movetype / leaning / weapon_position / mounted / strafing`). Only the server reads it. Body-part enum, exported `animBodyPartsStr` at 0x7b4f0: `{ "** UNUSED **", "LEGS", "TORSO", "BOTH" }`.

The pk3s hold 195 `xanim/pb_*` files:

- Movement: `pb_combatrun_{forward,back,left,right}_loop` (+ `_pistol`/`_unarmed`/`_heavy`/`_light`/`_rifles`/`_stickgrenades`), `pb_crouch_run_*`, `pb_crouchwalk_loop`, `pb_sprint`, `pb_prone_crawl{,_left,_right,_back}`, `pb_runjump_takeoff/_land`, `pb_standjump_takeoff/_land`, `pb_climbup/down`.
- Idles: `pb_stand_alert`, `pb_stand_ads`, `pb_crouch_alert`, `pb_prone_aim`.
- Deaths, legs-section nodes: `pb_stand_death_*`, `pb_crouch_death_*`, `pb_death_run_*`.

There are no stance-transition clips: `animtrees/multiplayer.atr` has no `2crouch|2prone|to_crouch|to_prone|transition|getup` entry, only per-stance idles and loops. The engine cross-fades the outgoing and incoming nodes over a blend time the anim setter picks. The server's and the cgame's copies, with rate and phase, are in `cod11-movers.md` section 15 ("A moving body's clip", "The drawn body's clip"). vcod runs both through `vcod_common::animclock::ChannelClock`.

## xanim v14 header flags (decoded)

The flag byte at header offset 6 takes four values in the shipped pk3s. The format has no length prefixes, so the layout was decoded by exact-EOF probing: a correct layout consumes the file down to the last notetrack byte. With the rules below all 2957 `xanim/*` entries decode byte-exact (862 flag 0, 48 flag 1, 1490 flag 2, 557 flag 3).

**Flag 0x1, looping.** A looping clip stores `frame_count + 1` key positions; the extra one repeats frame 0 to close the loop, and every count and index bound widens to match. vcod folds it into `XAnim::frame_count`, so key indices stay in `0..frame_count` and `duration()` equals the loop period.

**Flag 0x2, delta (root motion).** One extra nameless track sits between the header and the two bone bitsets, before everything else. It has a bone track's shape with yaw-only rotations:

```
u16 rot_count; [frame indices]; rot_count * i16      // z-only quat, dequantized like a "simple" bone
u16 trans_count; [frame indices]; trans_count * 3*f32
```

Its frame-index lists follow the bone tracks' sparse rule (present only when `1 < count < frame_count`). vcod keeps it as `XAnim::root`. Its one reader is the mounted gunner's placement, which hangs the `standMG42_*` leaves' root translation off the gun (`cod11-turrets.md` section 7).

Flag 0x3 is both rules composed, with no third layout.

**Frame index width.** Whatever the flags, the sparse frame-index list is `u8` while the highest index fits a byte and `u16` above that. Pinned by `pegnight_facial_price_04_sgtgoback` (256 frames, u8) against a 257-frame clip (u16). 46 of the 862 flag-0 files need `u16`, all long facial or cinematic clips.

Player-anim coverage: 254 `pb_`/`pl_`/`pt_` files. `pb_*` splits 1/78/116 across flags 1/2/3, `pl_*` is 2 files at flag 1, `pt_*` is 43 at flag 0 and 14 at flag 1.

Notetracks: vcod parses them into `XAnim::notes`, but nothing on the CoD1 path reads them. CoD4's view weapon reads its own clips' notes (`cod4_viewmodel.rs`).

## Legs/torso split and aim layer

One skeleton, two anims, one tree. `pt_*` clips key only the upper body (spine up, arms, weapon tags); `pb_*` clips key every bone. The tree walk blends `main`'s `torso` and `legs` nodes per bone. A torso anim weights them 1 and 0.01, so the bones a torso clip keys take it with 1% of the legs clip and the rest take the legs clip alone (`cod11-movers.md` 15, "Torso over legs").

`BG_Player_DoControllers` (`game.mp.i386.so` 0x2b7f8) then sets a local tag on `tag_origin` (the legs' yaw off the view, the lean's roll and shift, the prone slope) and control angles on `pelvis`, `back_low`, `back_mid`, `back_up`, `neck` and `head`. Its inputs are the three swings `BG_PlayerAnimation`'s angle updater (0x2af78) keeps in the client's record: legs yaw, torso yaw, torso pitch. `cod11-combat.md` 16.3 decodes the pitch and the control axis, 16.4 the yaw, the lean, `tag_origin` and the anim record's movetype and strafe flags. vcod: `vcod_common::playerpose::{BodyAngles, Controllers, apply_controllers}`, shared by the server's locational pose and the client's draw.

A dead body's hook runs too, with `tag_origin` turned by the legs' world yaw instead of their yaw off the view, so a tag lookup on it reads the body turned twice. The client draws no player entity with `EF_DEAD`; the corpse is what it draws (`cod11-combat.md` 16.4, "The dying body").

VERIFIED, live (1.1d, 1.5b and UO 1.51b, 2026-10-17, `probe_leanhit` with `probe_bones 1`): eleven bones of a standing, crouched and prone body, level and leaning both ways, sit within 0.0007 units of vcod's on all three. 1.5b's standing body matches once his origin's 0.04 is taken out (`leanhit_ab`, `cod11-combat.md` 3.6).

The inputs ride the entityState: `fTorsoPitch` (232), `fWaistPitch` (236), `fTorsoHeight` (228), `leanf` (212), `animMovetype` (224, 4 bits).

### 1.5's and UO's stance window, drawn

The cgame's lerp-frame setter carries the server's 400 ms window (`coduo-vehicles.md` 13). VERIFIED decompile: 1.5's `cgame_mp_x86.dll` 0x30003870 stores `cg.time + 400` at `ci+0x4a4`, UO's `uo_cgame_mp_x86.dll` 0x30003a90 at `ci+0x4c0`, when the legs frame (`ci+0x380` on UO) changes crouch or prone class, and floors every later blend of either frame to what is left of it. 1.1's cgame has no such store.

vcod's drawn bodies run the window under 1.5 and UO rules only (`update_channels` in `crates/client/src/entities.rs`, through `animclock::update_body`), one window per drawn entity. INFERRED: the cgame keeps it per client info, so a corpse sharing the dead client's info could read his last window; vcod does not model that. Pending: no check against a retail client's draw (pixels).

### The prone body on a grade

VERIFIED, live (retail 1.1d, 2026-10-16, `client-probes/probe_pronebones`, fixtures in `crates/server/tests/fixtures/pronebones/`): a prone client facing downhill on mp_carentan's ~19 degree mound (-224, 60) has his pelvis at (5.92, 24.36, -0.03) off his origin and his feet about 10 under it. On flat ground the pelvis sits 6.27 above the origin. vcod's server pose puts all eight logged bones (pelvis, spine2, neck, head, both hands, both feet) within 0.03 of retail's offsets on the mound, on mp_carentan's flat ground and on mp_brecourt's field at (1659, 781). With the slope terms zeroed the mound's pelvis lands 5.8 too high. The probe reads bones through a `script_origin` linked to each tag, which lags a frame, so it measures a still body.

VERIFIED, same runs: where the body settles comes from the fall off `setorigin`'s raised unit (cod11-player-clip.md 8.9), not from the pose. The drop lands anywhere inside the ground trace's quarter unit (retail read -23.77 and -23.87 on one flat spot in two runs). On the brecourt grade it slides up to a unit sideways, differently in two retail runs.

VERIFIED, `probe_scriptmodel`'s prone part on mp_brecourt: a hold test of `distance(origin, tpos) > 0.5` with z included is a trap. On the field the body settles 1.33 under `tpos`, so every frame he was put back a unit above it and dropped again: pm_flags prone throughout, origin cycling between -101.76 and -104.09, on retail and vcod alike. The frag then met him at a height the cycle and cmd timing chose, and rested 13 units off retail's spot. With the hold on the horizontal only, retail's first frag rests at (1657.12, 817.81, -80.92) and vcod's within 0.12 of it (`scriptmodel_ab::a_frag_rests_on_a_prone_players_back_on_a_grade`).

VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-16, the same probe, placing the body twice a second apart so the second drop starts at rest; fixtures `*-1.5.txt` and `*-uo.txt` beside the 1.1 ones, retaken the same way): on all three spots vcod's eight bones sit within 0.05 of retail's off the origin once the playerstate's prone body (`fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`) is retail's. `pronepose_ab` holds that body to the fixture's within 0.01. On 1.5 and UO the body keeps what the drop's last airborne cmd sampled (cod15-movement.md 4, "The airborne prone body"): -0.134 on mp_carentan's flat spot, lowering every bone 0.134 against 1.1. The pose needs nothing 1.5-specific beyond the controllers' later split (coduo-vehicles.md 14). Even 16 ms cmds leave the body 0.1 to 0.6 off retail's and read as a pose error, so `pronepose_ab` replays the capture's cmd times (`ProbeHold::times`, framed).

VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-19, `probe_pronebones` with `probe_rays 1`, fixtures `mp_brecourt-brecourt-rays-1.5.txt` and `-rays-uo.txt`): the probe prints the settled origin and each bone mark to the float (integer part and exact remainder) and a 2-unit grid of downward `bulletTrace`s over the body. With the client's cmds replayed (`ProbeHold::times`) and the engine's pose arithmetic ported (`cod11-combat.md` 3.1, "The pose on 1.5 and UO"), vcod's origin, marks and every ray match retail's bits on both servers; 1.5b's rays need the clips on the engine's float node time (`cod11-combat.md` 3.1, "The clip's float time"). VERIFIED, the same runs and four more: the drop onto the grade leaves `fTorsoHeight` between -0.425 and -0.516 by the cmd split (-0.491 in the 2026-10-16 runs), and the whole body rides it. The 1.5b run without `probe_spawn_yaw` lay at `proneDirection` -170 off its spawn's yaw, so the 1.5b fixture spawns on the spot.

## The animscript: which index the server picks

The server picks `legsAnim`/`torsoAnim` by running `mp/playeranim.script` against the player's state. `crates/common/src/animscript.rs` parses and selects; `ClientSim::update_anims` (`crates/server/src/spectate.rs`) builds the conditions.

### The file

VERIFIED, `mp/playeranim.script` in `pak4.pk3` (1177 lines): three sections, `DEFINES` at line 18, `ANIMATIONS` at 98, `EVENTS` at 576. VERIFIED: `ANIMATIONS` holds three states; `STATE RELAXED` (100-103) and `STATE ALERT` (105-108) are empty, and `STATE COMBAT` (110-548) has every movetype block. INFERRED: `COMBAT` is the only state with content in MP, so the server never decides a state.

VERIFIED: `STATE COMBAT`'s movetype blocks are `idle`, `idlecr`, `idleprone`, `walk`, `walkbk`, `walkcr`, `walkcrbk`, `walkprone`, `walkpronebk`, `run`, `runbk`, `runcr`, `runcrbk`, `climbup`, `climbdown`, `turnright`, `turnleft`. VERIFIED: `EVENTS` holds `fireweapon`, `meleeattack`, `dropweapon`, `raiseweapon`, `reload`, `jump`, `jumpbk`, `land`, `DEATH`, `revive`, `pain`. VERIFIED: the header legend at lines 73-76 is RTCW's and lists `straferight`/`strafeleft` (no such block) while omitting `climbup`/`climbdown` (both exist).

VERIFIED: live clauses use the conditions `weapons`, `weaponclass`, `weapon_position`, `strafing`, `movetype`, `mounted` and `firing`. VERIFIED: `enemy_weapon` and `impact_point` appear only in commented-out clauses of `DEATH` and `pain` (lines 1032-1094 and 1116-1167). INFERRED: an unknown condition parses to a kind that never holds, so it makes its clause unselectable rather than unconditional, and no live clause reaches one (`animscript.rs`). VERIFIED: `leaning`, `position`, `underwater` and `underhand` occur only in the commented legend at lines 78-91. INFERRED: a lean changes no index, and the parser has no kind for those four.

### What retail sent, pose by pose

VERIFIED, both captures: `crates/server/tests/fixtures/playerstate/mp_carentan-dm-motion.txt` and `mp_pavlov-dm-motion.txt`, 22 poses each, `--net-probe --save-motion` against retail 1.1d, one client, `dm`, weapon `m1carbine_mp` (`weaponclass rifle`). Each pose holds one usercmd until the playerstate settles. Indices are `legsAnim & 511`; the speed is the pose's own `velocity`.

| pose | usercmd | carentan | pavlov | clause that selects it |
|---|---|---|---|---|
| `stand`, `stand_between`, `stand_up`, `stand_after_crouch` | none | 122 | 122 | `idle` default, `pb_stand_alert` |
| `lean_left`, `center`, `lean_right` | `wbuttons` lean | 122 | 122 | same; leaning is not a condition |
| `crouch` | `up=-127` crouch | 111 | 111 | `idlecr` default, `pb_crouch_alert` |
| `prone`, `prone_yaw_60`, `prone_yaw_150` | prone, view yawed | 45 | 45 | `idleprone` default, `pb_prone_aim` |
| `prone_crawl_150` | prone + `forward=127`, 13.0 u/s both maps | 50 | 50 | `walkprone` default, `pb_prone_crawl` |
| `run_forward` | `forward=127` | 122 at 8.0 u/s | 94 at 148 u/s | `idle` default / `run` default, `pb_combatrun_forward_loop` |
| `run_back` | `forward=-127` | 93 at 62.8 u/s | 93 at 158.5 u/s | `runbk` default, `pb_combatrun_back_loop` |
| `strafe_left` | `right=-127` | 122 at 8.0 u/s | 92 at 175 u/s | `idle` default / `run` `strafing left`, `pb_combatrun_left_loop` |
| `strafe_right` | `right=127` | 122 at 3.2 u/s | 91 at 143 u/s | `idle` default / `run` `strafing right`, `pb_combatrun_right_loop` |
| `ads_stand` | `buttons=16` held 2 s | 121 | 121 | `idle`, `weapon_position ads`, `pb_stand_ads` |
| `crouch_run` | crouch + `forward=127` | 84 at 58.8 u/s | 111 at 0 u/s | `runcr` default, `pb_crouch_run_forward` / `idlecr` default |
| `turn_left`, `turn_right` | yaw only | 122 | 122 | `idle` default, not `turnleft`/`turnright` |
| `jump_takeoff` | `up=127`, first airborne frame | 100 | 100 | `jump` default, `legs pb_standjump_takeoff duration 5 blendtime 100` |
| `land` | first frame back on the ground | 99 | 99 | `land` default, `legs pb_standjump_land duration 100 blendtime 50` |

VERIFIED: `torsoAnim` is 0 in every pose of both captures, `jump_takeoff` included, and `playerstate_motion_ab` compares it. No movement pose raises a weapon event, so a torso index in one is a defect. VERIFIED: every clause reached in the table is written `both`. INFERRED: retail does not write the torso half of the continuous selection, so vcod clears `Selection::torso` before applying it and leaves the torso to events. Captures from before 2026-09-02 read 720 (index 208, `pt_stand_pullout_pose`) at `jump_takeoff`: a holstered weapon, not a clause ("The `jump_takeoff` torso index is gone").

### Unarmed

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, 2026-10-16, `client-probes/probe_pronebones` with `probe_spawn_yaw 315`, fixtures `crates/server/tests/fixtures/pronebones/mp_brecourt-spawn315*.txt`): a player the script `spawn`s with no weapon (`ps.weapon` 0) crouches in `pb_crouch_alert_unarmed` (1.1 and 1.5 index 107) and lies in `pb_prone_aim_pistol` (44). Those are the clips of `idlecr`'s and `idleprone`'s `weapons none` clauses (script lines 176 and 221). INFERRED: the `weapons` condition names weapon 0 `none`, as `getCurrentWeapon` does. `pb_prone_aim_pistol` is a 3 s loop where `pb_prone_aim` is a 0.3 s pose, so an unarmed prone body keeps moving (the pelvis about 0.1 a second). vcod names weapon 0 `none` for the conditions (`AnimInputs`).

### How retail picks the movetype

VERIFIED, `PM_Footsteps` at `game.mp.i386.so` 0x322c8 (the static after `PM_ShouldMakeFootsteps`; `pmove.rs` and `cod11-sound-system.md` use the same name), with the movetype and condition enum orders from the rodata name blocks at 0x6e260 and 0x6e424. VERIFIED: both blocks are stored in reverse, `** UNUSED **` first, so the movetypes are:

| 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `idle` | `idlecr` | `idleprone` | `walk` | `walkbk` | `walkcr` | `walkcrbk` | `walkprone` | `walkpronebk` | `run` | `runbk` | `runcr` | `runcrbk` |

The 44 motion poses agree, and the anim gate in `crates/server/tests/playerstate_motion_ab.rs` holds the result.

**The idle threshold.** VERIFIED: at 0x3249f the function compares `xyspeed` with the float at 0x70c1c, 10.0, and neither arm reads the usercmd. INFERRED: the arm below it selects the stance's idle and the arm at or above it is the movetype dispatch. VERIFIED, the captures, both sides of the number: carentan `run_forward` pushes `forward=127` into geometry at 8.0 u/s and reads 122, the standing idle; both maps' `prone_crawl_150` crawls at 13.0 u/s and reads 50, `pb_prone_crawl`; carentan `strafe_left` at 8.0 u/s and `strafe_right` at 3.2 u/s read 122; pavlov `crouch_run` at 0 u/s reads 111. vcod: `ANIM_IDLE_SPEED` (`crates/server/src/spectate.rs`).

**The speed it compares.** VERIFIED: 1.5b's `PmoveSingle` main arm calls `PM_Footsteps` (Ghidra 0x34ae8, file 0x24ae8) ahead of the move clamp block and `trap_SnapVector(ps->velocity)`, its last call, so `xyspeed` is the unsnapped velocity. INFERRED: 1.1d is the same; its `PmoveSingle` calls `PM_Footsteps` at 0x34336, ahead of the calls at 0x34364 and 0x34390, which `pmove.rs` ports as the clamp and the snap. VERIFIED, live (retail 1.5b, 2026-10-20, `mp_carentan-dm-legs-client-stopgo-1.5.txt` at serverTime 36750): a strafe reversal's last cmd leaves a velocity that snaps to (0, 10, 0), and retail reads the idle (122). The snapped 10.0 is not under the threshold, so the compare read the unsnapped speed. vcod keeps it in `PlayerState::footsteps_xyspeed` and selects on it.

**The direction.** VERIFIED: above the threshold the movetype is the stance crossed with `pm_flags` 0x40 (backwards) and 0x80 (walk). The walk bit is masked out of `pm_flags` once at 0x32499. The stance dispatches at 0x326c4, 0x32710 and 0x32766; each arm tests 0x40 and passes a movetype to the selection call at 0x327d3. VERIFIED: the indices are 8 and 9 prone; 6, 7, 12 and 13 crouched; 4, 5, 10 and 11 standing: the `walkprone`, `walkcr`/`runcr` and `walk`/`run` families. VERIFIED: the prone arm at 0x326f1 tests 0x40 alone, never the walk bit, so prone has two movetypes and reaches `walkprone` whatever that bit says. VERIFIED: no arm between 0x326c4 and 0x327d1 reads the usercmd.

VERIFIED: `PmoveSingle` latches 0x40 before the move. The block loads the cmd's `forwardmove` (usercmd +0x18) at 0x3413e, sets 0x40 in `pm_flags` (playerState +0xc) at 0x34147, clears it at 0x3415e, and reads `rightmove` (+0x19) at 0x34156. INFERRED: it sets the bit when `forwardmove < 0` and clears it when `forwardmove > 0`, or when `forwardmove` is zero with a non-zero `rightmove`. There is no third arm, so a cmd asking for neither axis leaves the bit alone. VERIFIED: RTCW has the same three arms in `PmoveSingle` (RTCW-MP's `src/game/bg_pmove.c:3915`), the lineage `crates/common/src/pmove.rs` ports. VERIFIED: both captures carry 0x40 at `run_back` and at none of the other 42 poses. VERIFIED: 0x80 is the ADS walk, set by the sight; the `walk*` blocks are reached through it ("Walk against run").

**The stop.** VERIFIED: past the movetype's selection call (0x327d8, result in `esi`) and the bob update, 0x3282a tests the cmd's `forwardmove` and `rightmove` bytes as one word (`pm+0x18`), and 0x32831 compares the float at 0x70c48, 120.0, with `xyspeed`. With the word 0 and `xyspeed` not over 120 it compares `ps+0xd8` with `ps+0x33c` and `ps+0x340` as the under-10 arm at 0x324c1 does, and calls the selection again with movetype 2 (`idlecr`, 0x3285b) or 3 (`idleprone`, 0x3286c). Otherwise it calls it with 1 (`idle`, 0x3287f) only when the first call returned negative. INFERRED, off those branches: a crouched or prone body with no move key takes its idle once it slows to 120, and a standing one keeps its move clip down to 10 u/s unless the clip found no clause. VERIFIED, the `legs_ab` capture (`movers/mp_carentan-dm-legs-client.txt`): the first crouched snapshot after the back key is released reads index 111, `pb_crouch_alert`, at 84 u/s. INFERRED: two selection calls in one pmove leave `legsAnim` as one would; the second either repeats the clip already playing or flips the toggle back. vcod: `ANIM_STOP_SPEED` in `crates/server/src/spectate.rs`.

**The stance.** VERIFIED: the move dispatch's stance (`ebp-0x20`) is computed at 0x32340..0x3239a:

- prone when `pm_flags` 0x1 is set, when `ps+0xd8` equals `ps+0x33c`, or when `ps+0xd4` is non-zero, `ps+0xd8` equals `ps+0x340` and `ps+0xdc` is 0;
- crouched when `pm_flags` 0x2 is set, or `ps+0xd4` is non-zero with `ps+0xd8` equal to `ps+0x340`;
- standing otherwise.

VERIFIED: both idle arms (0x324c1..0x324f4 under 10 u/s, 0x32847..0x32871 at the stop) compare `ps+0xd8` with `ps+0x33c` and `ps+0x340` and read no `pm_flags` bit: `idleprone` on the first match, `idlecr` on the second, `idle` otherwise. VERIFIED: the mounted arm (0x3230a) reads `pm_flags` 0x1 and 0x2. INFERRED: `ps+0xd8` is `viewHeightLerpTarget`, `ps+0xd4` `viewHeightLerpTime`, `ps+0xdc` `viewHeightLerpDown`, and `ps+0x33c`/`0x340` the prone and crouch eye heights, the same test `PM_GetEffectiveStance` (0x34554) makes. So a lie-down from standing idles crouched through the eye's first leg (whose target is the crouch) and prone only once the second leg starts. VERIFIED, live (retail 1.1d, 2026-10-16, `crates/server/tests/fixtures/pronebones/mp_brecourt-spawn315.txt`): the first grounded snapshot of a lie-down, `pm_flags` 0x40001, reads `legsAnim` 107 (`pb_crouch_alert_unarmed`) and the next 556 (44, `pb_prone_aim_pistol`). vcod: `ClientSim::update_anims` picks 1.1's idles off `view_lerp_target` and its move clips off `effective_stance`; `pronepose_ab`'s `a_spawn_lies_down_on_its_own_yaw_like_retail` holds the bones. 1.5 and UO take every arm off the stepped eye target (cod15-movement.md 4, "The spawn's lie-down").

**The strafe condition.** VERIFIED: condition 8 is written between 0x32504 and 0x325b5. The block loads `forwardmove` at 0x32504 and `rightmove` at 0x32568 and reaches the update call at 0x325b0 from three places, pushing condition 8 with value 0 at 0x3255d, 1 at 0x325a2 and 2 at 0x3258d; the path leaving 0x3256d lands past the call. INFERRED:

- value 0 is `NOT`, reached by every arm with a non-zero `forwardmove`, diagonals included;
- 1 and 2 are the two sides, reached only with `forwardmove` zero and `rightmove` non-zero;
- a cmd asking for neither axis skips the call, so the condition keeps its last value.

INFERRED: it is state carried between frames, not a reading of the current cmd, and `ClientSim` holds it that way. VERIFIED: the file's legend at line 91 says strafing "will never be left or right while moving backwards".

**Nothing is selected in the air.** VERIFIED: 0x323a2 compares `groundEntityNum` (playerState +0x54) with 1023 and 0x323af tests `pm_flags` 0x10, the ladder flag; the arm leaving 0x323b3 jumps to the function's tail at 0x328c1, past every selection call. INFERRED: an airborne player without the ladder flag takes that arm, so a jump keeps the takeoff anim until the landing. VERIFIED: retail's restart toggle flips once between the `jump_takeoff` and `land` samples on both maps, one selection between the two. INFERRED: the two ground-edge events must be raised before the continuous selection, or the landing frame selects an idle first and flips the toggle twice. VERIFIED: the `land` default clause carries `duration 100`, which is why the fixture samples `land` on the first frame back on the ground (`sample=grounded`). VERIFIED: the mp_pavlov capture's `run_back` pose reads `groundEntityNum` 1023 and `legsAnim` 605, index 93, the `runbk` loop. INFERRED: retail's probe backed off a ledge and kept its loop, so leaving the ground does not raise the takeoff (this rests on the airborne early-out, itself a branch condition). INFERRED: the takeoff comes from the jump impulse, so vcod raises `jump`, or `jumpbk` while the backwards latch is set, only on the frame pmove takes one. UNVERIFIED: whether a fall nobody jumped into raises `land` on touchdown; no pose in either capture lands from one, and vcod raises it.

VERIFIED: the ladder flag is `pm_flags` 0x10 and 0x323af tests it. INFERRED: a set flag skips the airborne early-out, so a climber reaches the selection. VERIFIED: the script has `climbup` and `climbdown` blocks, each one default clause selecting `pb_climbup` and `pb_climbdown`. INFERRED: those are the movetypes a climber selects, by the sign of the vertical velocity. vcod derives them that way (`Movetype::ClimbUp` at `velocity.z >= 0`) and raises no takeoff on the mount, which takes no impulse. Neither capture climbs a ladder, so only the flag and the blocks are measured.

**The lie-down in the air.** VERIFIED, 1.5b `game.mp.i386.so` `PM_Footsteps` (0x24ae8): an airborne body whose eye target is not its flags' stance goes on to the ground code (0x24e51; cod15-movement.md 4, the footstep cadence). INFERRED, off the same branch: the movetype selection after it runs for that body too, selecting the stance window's idle. VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-20, `probe_scriptmodel`'s prone part, the `-cmds` fixtures and a gdb run): a target holding prone as `setorigin` drops him a unit shows the lie-down's legs (1.5b 111, UO 174) in the snapshot where `pm_flags` first reads prone, still in the air. vcod: `ClientSim::footsteps_select` takes that arm under 1.5 and UO rules and `update_anims` selects through it, so the prone idle starts on retail's frame (`scriptmodel_ab`'s `-cmds` gates, combat doc 3.1, "The prone strike on mp_carentan").

**What this overturned.** Two readings that agree with every settled pose and are still wrong; neither is in the code.

- The direction from the usercmd or from velocity. VERIFIED: pavlov `run_forward` carries velocity `(0, -148)` at `viewangles[1]` -31.94, 58 degrees off the facing, and pavlov `strafe_right` carries `(-143, 0)`. Projecting either onto the facing selects 91 where retail sent 94, and 93 where retail sent 91. INFERRED: reading the cmd matches every capture pose but differs on a diagonal, where the latch rule clears the strafe condition and the cmd would set it.
- A coasting player keeping his anim. VERIFIED: RTCW does hold one: RTCW-MP's `src/game/bg_pmove.c:1607-1622` returns without selecting when neither axis is asked for and `xyspeed > 120` ("continue what they were doing last frame, until we stop"). VERIFIED: retail dropped it; the only float compare in `PM_Footsteps` is the one against 10.0 at 0x3249f and the function holds no 120. INFERRED: retail re-selects every frame above the threshold, so a tap and release slides in the run loop, not the idle, and a backwards crouched coast is `runcrbk`, not a standing back-run.

### Walk against run

VERIFIED: CoD 1.1 MP has one move speed and no walk bit in the usercmd (`docs/protocol-1.1.md`, "Usercmd input bits"). VERIFIED: retail chose the `run` family in every moving standing or crouched pose of both movement captures (94, 93, 92, 91, 84); the only walk-family block it selected was `walkprone` (50), which prone reaches because there is no `runprone`. VERIFIED: holding the ads bit for two seconds selected the standing ads clause on both maps, `ads_stand` reading 121 `pb_stand_ads`. Captures from before 2026-09-02 read 122 there because their usercmd `weapon` byte was 0: the probe was holstered, so its ads bit reached no weapon.

VERIFIED, `game.mp.i386.so` 1.1d: `PM_Footsteps` keeps `pm_flags & 0x80` in `edi` (0x32490, 0x32499), and each standing and crouched arm passes the walk index for it:

| stance | walk bit set | clear |
|---|---|---|
| standing | 5 `walkbk` (0x32783) / 4 `walk` (0x327b5) | 11 / 10 |
| crouched | 7 (0x32745) / 6 (0x3275c) | 13 / 12 |

`pm_flags` 0x80 is `PM_UpdatePlayerWalkingFlag`'s (0x33694): the sight held, not prone, not reloading (vcod `pmove::weapon::ads_pm_flags`). VERIFIED, live (retail 1.1d, 2026-10-17, `movers/mp_carentan-dm-legs-client-torso.txt`, step `ads_walk_fire`): walking forward with the sight up reads `pm_flags` 0x400a0 and `legsAnim` 573, index 61 `pb_stand_shoot_walk_forward`, the `walk` block's default clause, at 90 u/s. So the `walk*` blocks are the ADS locomotion the file's NOTES at line 94 describe ("The player walks when they are ADS, so they can not ADS while running"), reached through the walk bit, not the usercmd. vcod selects them the same way; a selector that ignores 0x80 plays the run (94) for the ADS walk.

VERIFIED, `game.mp.i386.so` 1.5b: `PM_Footsteps` (0x24ae8) sets its walk local (`ebp-0x28`) when `pm_flags & 0x80` (0x24e6f) or the float at `ps+0x40`, `leanf`, is non-zero (0x24e82); the movetype dispatch tests that local (0x25307, 0x253c9). VERIFIED, `game.mp.uo.i386.so`: UO's (0x297e4) takes `pm_flags & 0x80` alone (0x29ba4, `ebp-0x28`, tested at 0x29eae and on); its lean test (0x29a2a, `ps+0x44`) sits in the bob block above. INFERRED: a 1.5 runner who leans plays the walk clips; no capture shows it. vcod: `ClientSim::footsteps_movetype_of` (`crates/server/src/spectate.rs`).

### The weapon event's movetype

VERIFIED, `game.mp.i386.so` 1.1d, `PmoveSingle` (0x33dfc, relocations resolved with `tools/re/annotate_func.py`): the main arm calls `PM_Weapon` (0x34331), then `PM_Footsteps` (0x34336). The `fireweapon` and `reload` events `PM_Weapon` raises select by the `movetype` condition, which only `PM_Footsteps` writes, so they read the movetype the previous pmove left. VERIFIED, 1.5b: the main arm calls `PM_Footsteps` (0x28889), then `PM_Weapon` (0x2888e); UO likewise (0x2e04e, 0x2e053). INFERRED: there the event reads the movetype its own pmove just wrote.

VERIFIED, live (retail 1.1d, 2026-10-17, the torso capture above): the first shot of each moving step, sent in the first cmd that moves, writes the idle's fire anim, each held for ten snapshots: `run_fire` 253 `pt_stand_shoot`, `strafe_fire` 253, `crouch_run_fire` 249 `pt_crouch_shoot`, `ads_walk_fire` 251 `pt_stand_shoot_ads`. Later shots while moving write nothing (the `movetype moving` clauses are empty). Selecting by the end-of-frame movetype instead plays none of the four. vcod: `ClientSim::note_footsteps`, called after each pmove of a cmd's chop; the events carry the movetype it returns (`spectate::AnimEvent`).

### The turn movetypes, and what selects them

VERIFIED: `turn_left` and `turn_right` turn the view 60 degrees with no move input, and retail sent 122, the standing idle, on both maps. VERIFIED: the `turnright` and `turnleft` blocks select `legs pl_chicken_dance` and `legs pl_chicken_dance_crouch`, gated on `movetype idle` and `movetype idlecr`. VERIFIED: `animtrees/multiplayer.atr` puts exactly those two clips in a top-level `turning` group commented "temp turning animations"; they are the only `pl_*` names in the tree and resolve to indices 4 and 3, which appear in neither capture.

VERIFIED, by cross-reference: `angles2[1]` is `ps.movementDir`, not a body yaw (`docs/protocol-1.1.md`, "What a player entity looks like", has the addresses and the held-input measurements). That settles the field, not what enters `turnright` or `turnleft`.

INFERRED: `movementDir` cannot select those blocks. `set_movement_dir` (`crates/common/src/pmove.rs`) zeroes it whenever neither move key is held, and the turn poses hold none, so it reads 0 through the whole turn.

INFERRED, off the retail control flow ported into `walk_move` (`pmove.rs`, from 0x2f6db): `PM_WalkMove` skips the position step at zero velocity but still calls `PM_SetMovementDir`, and that function's prone branch reads `proneDirection` against the view yaw, not displacement. That is why a still prone player's legs turn with his body. INFERRED: the mechanism is prone-only; `PM_SetMovementDir` has no stand or crouch equivalent, so if the turn blocks are reachable at all, something else selects them.

Open: what, if anything, drives a player into `turnright`/`turnleft`. vcod never selects them. A capture that samples every frame through a sustained still turn, reading `legsAnim` alone, would settle it: 3 or 4 means something drives them, 122 throughout means they are dead in MP. INFERRED: the group's "temp" comment and the absence of 3 and 4 from the held-input captures and the two-probe entity list both point to dead.

### The restart toggle

VERIFIED: the toggle's absolute value is not a function of the pose. `stand_up` is index 122 in both captures, with `legsAnim` 634 (toggle set) on mp_carentan and 122 (clear) on mp_pavlov; `run_back` is 93 in both and reads 93 against 605. INFERRED: only the change carries meaning, so a gate compares the index and checks per frame that the bit flips exactly when the index does, never the bit's value.

### Open questions

- Closed. The `torsoAnim` 720 and `weaponstate` 2 both old captures read at `jump_takeoff` were a holstered weapon from a `cmd.weapon` of 0; the retaken captures read 0 and 0 ("The `jump_takeoff` torso index is gone").
- Closed. `pm_flags` 0x8 read set at `jump_takeoff` on mp_pavlov and clear on mp_carentan in the superseded captures. VERIFIED: the retaken pair reads it clear on both, as vcod does, and `playerstate_motion_ab` no longer excepts it.
- Open. VERIFIED, in a run not committed: the ads pose latches. A capture that pressed the ads bit mid-script kept `pm_flags` 0x20 and 0x80 and `legsAnim` 121 `pb_stand_ads` for every later pose (a crouch run, a stand, two turns) with the bit long released, and read them clear at the jump that followed. UNVERIFIED: what clears them, and whether the jump did. The probe's motion script holds `ads_stand` last for this reason, so the committed captures cannot show it.

## The weapon channel: what writes `torsoAnim`

VERIFIED: `crates/server/tests/fixtures/playerstate/mp_carentan-dm-combat.txt` and `mp_pavlov-dm-combat.txt`, nine steps each, `--net-probe --save-combat` against retail 1.1d, one client, `dm`. Each step holds one input and taps the attack bit. The fixture logs one `!trace` line per snapshot, since the event ring holds four slots and a shot is shorter than a step.

Trap: captures taken with `cmd.weapon` 0 in every usercmd read as a holster request (`cod11-combat.md` 1.8). The pair committed before 2026-09-02 did that: it carries `weaponstate` 2 and `EV_PUTAWAY_WEAPON` at the reload key, both stance changes and the jump, and its reload key never reloaded. Both were retaken with the probe sending the weapon it holds, and every such putaway is gone. Claims below that rest on the superseded pair say so; it is in git at commit `0243e35`. VERIFIED: snapshots arrive 50 ms apart, so N consecutive samples in one state bound its length to `((N-1)*50, (N+1)*50)` ms, and nothing below is tighter.

VERIFIED: carentan's join gave `m1carbine_mp` and pavlov's `mosin_nagant_mp` (`weapons/mp/*` in `pak0.pk3`). Two weapons with different numbers let a state be matched to a weapon-file field instead of one rifle's timing.

| weapon | `fireTime` | `rechamberTime` | `reloadTime` | `dropTime` | `clipSize` |
|---|---|---|---|---|---|
| `m1carbine_mp` | 0.135 | 0.1 | 2.65 | 0.67 | 15 |
| `mosin_nagant_mp` | 0.33 | 1.0 | 2.4 | 0.4 | 5 |

### Firing

VERIFIED: a shot reads `weaponstate` 3 and `weapAnim` 514 and bumps `eventSequence` by one with event 159, `EV_FIRE_WEAPON` (`cod11-events-and-fx.md`). VERIFIED: the event goes to `events[eventSequence & 3]` and the counter increments after; every bump in both captures moves exactly that slot. VERIFIED: `weapAnim` carries the same 512 toggle as the anim channels; pavlov's traces hold 2 and 514, 4 and 516.

VERIFIED: `weaponstate` 3 runs 3 samples on carentan (100-200 ms) and 7 on pavlov (300-400 ms; 6, 250-350 ms, on the crouched and prone shots of the 2026-10-17 recapture and the single and crouched ones of the 2026-10-18 one). INFERRED: that is `fireTime`, the only weapon-file time inside both windows at 0.135 s and 0.33 s.

VERIFIED: the torso index of a shot is stance-specific: 253 `pt_stand_shoot` on every standing fire step of both maps, 249 `pt_crouch_shoot` crouched. INFERRED: the torso channel belongs to the weapon, not the continuous selection. No clause the continuous selection reaches produces those `pt_*` leaves, retail sent `torsoAnim` 0 in all 20 settled movement poses, and the index moves with `weaponstate` and stance, not movetype.

VERIFIED, resolved from the animtree (`crates/common/src/animtree.rs`, `PlayerAnims`), the shoot family:

| index | anim | index | anim |
|---|---|---|---|
| 245 | `pt_stand_shoot_pistol` | 250 | `pt_stand_shoot_auto_ads` |
| 246 | `pt_crouch_shoot_auto_ads` | 251 | `pt_stand_shoot_ads` |
| 247 | `pt_crouch_shoot_ads` | 252 | `pt_stand_shoot_auto` |
| 248 | `pt_crouch_shoot_auto` | 253 | `pt_stand_shoot` |
| 249 | `pt_crouch_shoot` | 222 / 223 / 224 | `pt_prone_shoot_auto` / `_pistol` / `pt_prone_shoot` |

UNVERIFIED on the wire: every member but 224, 249 and 253 (and 251, "The weapon event's movetype"). The combat probe never held the ads bit through a shot, and neither rifle is automatic.

### The restart toggle, doing its job

VERIFIED, carentan `sustained_fire`: six shots read 765, 253, 765, 253, 765, 253: index 253 throughout, bit 512 flipping on every `eventSequence` bump. VERIFIED, carentan `crouch_fire`: 249, 761, 249 across three shots. INFERRED: the toggle alone signals a repeated shot, so a client watching only the index plays the first shot's clip and nothing after.

VERIFIED: when a shot ends the torso reads 512, index 0 with the toggle flipped: the channel clears to no anim and flips the bit doing it.

### `weaponstate`, and what each value carried

Paired values are carentan then pavlov.

| `weaponstate` | seen on | event at the bump | torso index | run | weapon-file match (INFERRED) |
|---|---|---|---|---|---|
| 0 | both | - | - | - | ready |
| 3 | both | 159 `EV_FIRE_WEAPON` | 253 standing, 249 crouched, 224 prone | 100-200 ms, 300-400 ms | `fireTime` 0.135, 0.33 |
| 4 | pavlov | 162 `EV_RECHAMBER_WEAPON` | unchanged | 950-1100 ms | `rechamberTime` 1.0 |
| 5 | both | 151 `EV_RELOAD` | 229 `pt_reload_stand_auto`, 231 `pt_reload_stand_rifle` | 2600-2700 ms, 2350-2450 ms | `reloadTime` 2.65, 2.4 |

VERIFIED: the event ids, torso indices and run lengths, read off the traces. INFERRED: the right-hand column (a duration inside a bound is a match, not a proof) and the state numbers themselves, read off behaviour, not an enum.

VERIFIED: `weaponstate` 2 and event 156 appear in neither capture. In the superseded pair they sat at the reload key, both stance changes and the jump, and all went away once the probe sent its weapon.

VERIFIED: 4 appears on pavlov only. INFERRED: the bolt action; the carbine's `rechamberTime` is 0.1 s, under two snapshot intervals, and its 15-round clip never emptied.

VERIFIED: neither combat capture empties a clip, and no trace in either holds 161 `EV_FIRE_WEAPON_LASTSHOT`. VERIFIED, `mp_carentan-tdm-lean.txt`: the `kar98k_sniper_mp`'s fifth shot (`scope_right_2`) raised 161 where its four before raised 159, wrote the torso index those four wrote, 237 (749 with the toggle), and went on to `weaponstate` 5 and 231 `pt_reload_stand_rifle`. VERIFIED, `mp_carentan-tdm-grenade.txt`: the last frag's `throw_down` raised 161 too (`cod11-combat.md` 1.14; the rule is 1.5 step 7).

The next paragraph rests on the superseded pair. Its putaways came from a `cmd.weapon` of 0, but the putaway it measured is real and nothing since has measured another.

VERIFIED, superseded pair: the pullout pose is stance-matched like the shoot anim, 208 `pt_stand_pullout_pose` standing and 209 `pt_crouch_pullout_pose` crouched. Both maps' `stand_between` step reads 209 while `legsAnim` already reads the standing idle. INFERRED: the index latches when the state opens; that step's state opened while the player was still crouched. VERIFIED, same pair: the torso index clears before the state does; carentan's reload step holds 208 for about 500 ms of a `weaponstate` 2 that runs 600-700 ms.

**The reload key reloads a partly-full clip.** VERIFIED: the step that taps `wbuttons` 0x08 reads `weaponstate` 5 with event 151 `EV_RELOAD` on both maps, for 53 samples on carentan and 48 on pavlov (`reloadTime` 2.65 and 2.4). VERIFIED: the torso index is the weapon's, 229 `pt_reload_stand_auto` for the carbine and 231 `pt_reload_stand_rifle` for the mosin, and `weapAnim` takes index 11 `WEAP_RELOAD`. This confirms what `cod11-combat.md` 1.7 read off the instructions. VERIFIED: the superseded pair read `weaponstate` 2 with event 156 here instead, with rounds in the clip and no state near `reloadTime`; the usercmd's `weapon` byte is the only difference between the runs.

### What `weapAnim` is not written by

VERIFIED, superseded pair: no `weaponstate` 2 writes `weapAnim`. Carentan's reload step holds index 0 for all 13 samples of the state and takes 512 (index 0, toggle flipped) on the sample the state ends; pavlov's `stand_between` holds the rechamber index 4 for all 8 and lands on 512 the same way.

VERIFIED: a real putaway does store `WEAP_DROP`. The grenade capture's `to_frag` step reads `weapAnim` 521 (index 9 with the toggle) for all of `weaponstate` 2, then 10 and 512 across the raise, one write per transition (`cod11-combat.md` 1.14). INFERRED: the two runs above sent `cmd.weapon` 0 and 1.2's setter refuses to write on that input, so their unchanged channel is the same artifact as their phantom putaways. The old reading here, that a putaway stores no anim and section 1.8's `WEAP_DROP` is only the event's parm, is superseded.

VERIFIED: a rechamber ending writes no `weapAnim` either. Pavlov's `single_shot` reads 516 across the sample where `weaponstate` goes 4 to 0, and its `sustained_fire` never shows index 0 between two shots.

VERIFIED: pavlov's `weaponstate` goes 3 to 4 on consecutive samples 50 ms of server time apart, and `weapAnim` 2 to 4 across the same pair: 341 to 390 ms on `sustained_fire`, 359 to 408 on `prone_fire` (2026-10-18 recapture; the 2026-10-17 one read 344 to 408 and 310 to 374 on `crouch_fire`, an earlier capture 328 to 378). INFERRED: the shot's state ends and the rechamber opens within one server frame. That frame holds several cmds, so it cannot show whether the rechamber waits a cmd for the state 3 exit; it does (`cod11-combat.md` 1.4).

### The `jump_takeoff` torso index is gone

VERIFIED: the movement captures committed before 2026-09-02 read `weaponstate` 2 and `torsoAnim` 720 at `jump_takeoff`, and the retaken pair reads 0 and 0. VERIFIED: the only difference is the usercmd's `weapon` byte. The compact usercmd branch carries no weapon byte, so a probe sending 0 reaches the weapon-change check only when `upmove`, `wbuttons` or the byte itself changes, and the jump's `upmove` change is what put it on the wire (`docs/protocol-1.1.md`, "Usercmd delta"). INFERRED: the 720 was a holstered weapon, not an animscript clause.

### Prone fire, taken on both maps

VERIFIED: carentan's `prone_fire` step reads `legsAnim` 557 (index 45, `pb_prone_aim`) and `torsoAnim` 224 `pt_prone_shoot` across its three shots. VERIFIED: pavlov's 2026-10-17 recapture reads the same, `legsAnim` 557 and `torsoAnim` 736 (224 with the toggle), and so does the 2026-10-18 recapture from another spawn. VERIFIED: 230 `pt_reload_prone_rifle` appears in neither capture; neither reload ran prone.

VERIFIED: the recapture holds the spawn point's yaw. The `advance` walk ends turned on pavlov (90 on the 2026-10-18 recapture), and `idle` brings the view back to the spawn's facing (carentan -180, pavlov 45), held by every later step, at (995.1, 1038.3, -13.0) and (-7095.1, 10360.9, -103.9); carentan's walk never stalled. The 2026-10-17 take ended its walks at 135 and 45, faced 45 and 0, and stood at (-522.6, 1997.2, 144.1) and (-8829.7, 10319.9, -23.6).

VERIFIED, first capture (git `c52e9606`): pavlov's `prone_fire` read `legsAnim` 634 (index 122, the standing idle) and the standing shoot pose 253 throughout, with the crouch taken at the same spot one step earlier (`legsAnim` 111). INFERRED: the prone was refused and the step recorded a standing shot under a prone label. VERIFIED: both of that capture's settled field lines turn the view during the still `idle` step after the walk, while every `!input` line records yaw 0: pavlov's `viewangles[1]` reads -135 after `advance` and 90 from `idle` on at (-7440.875, 10399.125, 309.23); carentan's reads 0 then -90 at (-233.41, 1684.30, -7.87), 40 units from where its walk ended. INFERRED: that is the probe's stall turn, which rode past the walk until 5233cc3, a week after both captures. INFERRED: facing 90 on pavlov the body has a wall 15 units behind it. vcod's `BG_CheckProneValid` port (`pmove::check_prone`, 0x2d428) meets it at fraction 1/6 of the sweep 24 up behind the facing, a reach of 15 against the 17 (`HALF_WIDTH + 2`) it needs, and refuses at every yaw from 80 to 100; facing -135 it takes the prone. The refusal came from the spot and facing, not the map.

vcod: `playerstate_combat_ab` starts the step after such a turn at retail's settled origin and yaw (`Step::place`) and compares `prone_fire` on both maps like every other step. Against the first capture vcod lay down on carentan and stayed standing on pavlov, as retail did.

### Neither counter in `!observed` counts shots

VERIFIED: carentan's `sustained_fire` reads `shots_seen` 5 against six taps and six 159 events (3 on the 2026-10-17 recapture, 4 before it); pavlov's `single_shot` reads `seq_delta` 3 against one tap. INFERRED for the first: `shots_seen` counts `weaponstate` 0 -> 3 edges, and at carentan's 183 ms tap period against a 135 ms `fireTime` the weapon is ready for under one snapshot interval between shots, so some edges fall between samples. VERIFIED for the second: one mosin shot raises three events, 159, 162 `EV_RECHAMBER_WEAPON`, 163 `EV_EJECT_BRASS`, so `seq_delta` is three per shot on pavlov and one on carentan, whose carbine raises only 159.

INFERRED: a gate counts `events[]` entries equal to 159, or 161 for a clip's last round, across the per-snapshot traces, never `seq_delta` or `shots_seen`. VERIFIED: the ring holds four slots, so only a per-snapshot trace can count a burst longer than four.

### As implemented

The torso channel has no table of its own. The weapon machine raises an animscript event and the script's `EVENTS` clauses pick the index. `ClientSim::update_anims` (`crates/server/src/spectate.rs`) maps each `PmEvent` the frame's moves raised onto a block (`weapon_anim_event`) and runs it through `AnimScript::select_event`:

| event | block |
|---|---|
| 159 / 161 | `fireweapon` |
| 151 / 152 / 153 | `reload` |
| 156 | `dropweapon` |
| 155 | `raiseweapon` |
| `EV_MELEE_SWIPE` (the swing, not `EV_FIRE_MELEE`'s damage frame; combat doc 1.10) | `meleeattack`, drawn at random (below) |
| UO `EV_UO_DEPLOY_WEAPON` / `EV_UO_BREAKDOWN_WEAPON` | `lmg_deploy` / `lmg_breakdown` |

VERIFIED: the shipped clauses reproduce every index the two captures read, with no engine table. `fireweapon` gates on the `movetype` categories the file's `DEFINES` build (`set movetype crouching = idlecr AND runcr AND ...`), so 253 `pt_stand_shoot`, 249 `pt_crouch_shoot` and 224 `pt_prone_shoot` follow from the stance. `reload` gates on the weapon name first: the carbine is in the block's `bar_mp AND ... AND m1carbine_mp` list and takes 229 `pt_reload_stand_auto`; the mosin falls through to the `default` clause's 231 `pt_reload_stand_rifle`. Test: `the_event_blocks_pick_the_indices_the_combat_captures_read` in `crates/common/src/animscript.rs`.

VERIFIED decompile, `BG_PlayAnim` (1.1d `game.mp.i386.so` 0x3c338, UO `game.mp.uo.i386.so` 0x2bfb9): the event sets the channel's timer (`torsoTimer`, `legsTimer`) to its duration plus 50 (0x32), or the clip's own length plus 50 for a line with none. A continuous anim takes the channel only while the timer reads under 50, or with the force argument. So an event holds for its duration alone, on the timer, which pmove counts down by each cmd's msec. VERIFIED, the jeep seats capture (`coduo-vehicles.md` 8): the passenger's `pt_rifle_fire` (318) starts with the cmd at 45700, `torsoTimer` reads 1342 at commandTime 45741 and 42 at 47041, and that snapshot already carries the continuous 794.

vcod-server (`AnimState`, `animscript.rs`) keeps the hold on the cmd clock: each event is stamped with its cmd's `serverTime`, the release is read at the round's last cmd, and it releases under 50. Trap: holding on the frame clock releases 50 ms late, and `>= duration + 50` another 50, two snapshots in all (`vehicle_ab::the_jeep_seats_replay_on_ours`). The length comes from the xanim header (`PlayerAnims::length_ms`), truncated as the engine's `XAnimGetLength` stores it (VERIFIED disassembly, `cod_lnxded` 0x08088b3d..0x08088b5b; `cod11-combat.md` 16.4, "The dying body"). Both timers ride the playerstate: vcod-server writes each as the event's expiry less `commandTime`, floored at 0 (`AnimState::timers`). The death frames of all three servers match, and so does every snapshot of the per-cmd captures `animtimers_ab` replays ("The takeoff and the landing").

The hold is not the weapon state's length. VERIFIED: carentan's reload holds the torso for 950-1050 ms of a `weaponstate` 5 that runs 2650, and pavlov's for 2050-2150 ms of one that runs 2400: the 31-frame `pt_reload_stand_auto` and 64-frame `pt_reload_stand_rifle` at 30 fps. The four measured clears bracket the clip length to (900, 1050], (2000, 2150], (400, 550] and (450, 600] ms against clip lengths of 1000, 2100, 400 and 500. A 50 ms sample grid cannot tell `duration` from `duration + 50`, and the two shot clears lean one way while the two reload clears lean the other; the `BG_PlayAnim` decompile above settles it on `duration`.

INFERRED: the derived hold has a 500 ms floor (`animscript::MIN_EVENT_HOLD_MS`). The superseded pair's `dropweapon` held 208 `pt_stand_pullout_pose` for about 500 ms, and that clip is one frame, 0 ms long. A floor fits; `max(clip, the weapon state's length)` does not, since the carbine's reload clip is 1000 ms inside a 2650 ms `weaponstate` 5 and the capture clears at the clip. 500 keeps all four fire and reload holds where they were measured: the reload clips are longer, and `pt_stand_shoot`'s 400 and `pt_crouch_shoot`'s 500 hold 500, inside both maps' brackets. UNVERIFIED: no current capture puts a weapon away, so only the superseded measurement carries the floor. A clause's own `duration` never takes the floor, so the `duration 150` autofire shots keep 150.

The channel clears as the capture reads it. When the hold runs out and the continuous selection has nothing for the torso (always, since retail sends 0 in every settled pose), `AnimState::clear_torso` writes index 0 and flips the toggle, giving the 253 -> 512 a shot ends with. A repeated shot restarts the same index, so only the toggle moves.

`crates/server/tests/playerstate_combat_ab.rs` compares, per fire and reload step, the set of `torsoAnim & 511` values and the number of toggle flips. Both maps match:

| map | step | indices | flips |
|---|---|---|---|
| carentan | one shot | {0, 253} | 2 |
| carentan | six shots | {0, 253} | 7 |
| carentan | reload | {0, 229} | 2 |
| carentan | three crouched shots | {0, 249} | 4 |
| pavlov | shots | {0, 253} | 2 and 4 |
| pavlov | reload | {0, 231} | 2 |
| pavlov | crouched | {0, 249} | 2 |

### The takeoff and the landing

VERIFIED disassembly, 1.1d `game.mp.i386.so` `PmoveSingle` 0x33dfc, walking arm, in order:

1. `PM_CheckDuck` (0x316f4, call at 0x342c9)
2. `PM_GroundTrace` (0x30474, call at 0x342ce)
3. `PM_DropTimers` (0x32a44, call at 0x342f9): runs `legsTimer` (ps+0x6c) and `torsoTimer` (ps+0x74) down by the cmd's msec, floored at 0 (0x32a7d..0x32abc)
4. `PM_WalkMove` (0x2f258, call at 0x3431b), which opens with `PM_CheckJump`
5. a second `PM_GroundTrace` (call at 0x34327)

Three places raise `jump` (`jumpbk` where `cmd.forwardmove < 0`), each through `BG_AnimScriptEvent` (VERIFIED decompile; the same three on 1.5b `game.mp.i386.so` 0x31a17 / 0x31ccd and UO, the `%i:lift` and `%i:kickoff` strings):

- `PM_CheckJump` (0x2eb98), forced, in its ground arm only; a ladder push-off raises none.
- `PM_GroundTrace`'s kickoff: a hit with the velocity carrying the body off the plane (`velocity.z > 0`, `velocity . normal > 10`) and `pm_flags` 0x10 clear. Not forced, so it takes the legs only while `legsTimer` reads under 50.
- `PM_GroundTraceMissed` (0x3037c), forced, for a body that had ground when a trace 64 straight down from the origin meets nothing.

So the timer an event sets depends on where it is raised. VERIFIED, `animtimers_ab` over `mp_carentan-dm-slope-8ms-moves-1.1.txt` and `-springfield-1.1.txt`:

- A jump reads 55 (`duration 5` + 50) less the cmds after the jump cmd, not counting the jump cmd's own msec.
- 1.1's prone dive gets its impulse from `PM_CheckDuck` ahead of the first ground trace, so it is a kickoff in that trace and reads 55 less its own cmd's msec as well (20 at ct 48335 for a dive pressed on the cmd at 48308, 19 at 53344 for one at 53317).
- A landing (`PM_CrashLand` inside the second trace) reads 150 less the cmds after it.

Each picks its clause by the `movetype` the last `PM_Footsteps` left: the backward dive reads `pb_standjump_takeoff` off `runbk`, not the prone `pb_chicken_dance_crouch` the body has reached by the frame's end (VERIFIED, same capture).

vcod: pmove reports the event (`pmove::Takeoff`: forced or not, `back`, and whether the first ground trace raised it) and the landing per cmd. `ClientSim` keeps the cmd's serverTime (less its msec for an event ahead of the drop) and the `movetype` before the move, and `update_anims` plays it on that clock. The motion fixtures cannot gate the two transient samples (`jump_takeoff`, `land`): they log no cmd times. Retail's read 39 and 150 on carentan, 5 and 102 on pavlov under one script, which is 55 and 150 less the cmds between event and sample (INFERRED from the model above, which the per-cmd gates VERIFY).

UO's deploy and breakdown raise their own blocks, `LMG_DEPLOY` and `LMG_BREAKDOWN` (events 0xe and 0xf, forced; VERIFIED decompile, UO `game.mp.uo.i386.so` 0x347aa and 0x3484c, beside events 160 and 161). VERIFIED disassembly, 0x35bed..0x35c23: `PM_Weapon`'s deploy driver (0x35b66) raises `LMG_BREAKDOWN` again as a continue (`BG_AnimScriptEvent(ps, 0xf, 1, 1)`) on every cmd in `weaponstate` 13 while ps+0x100 (`viewHeightLerpTarget`) is non-zero. INFERRED from the `BG_PlayAnim` decompile above (its continue argument): a continue leaves an anim already on the channel alone, so it acts only when the clause changes, which the stance does. VERIFIED, `mp_carentan-dm-slope-8ms-sprint-uo.txt`: the deploy reads `torsoAnim` `c_mp_lmg_deploy_prone` with `torsoTimer` 1825 at ct 48091 (1850 less 25 ms); the prone breakdown `c_mp_lmg_breakdown_prone_torso` 1926 at 58582; the stand-up at the cmd of 60066 restarts the torso on `c_mp_mg34_breakdown_torso`, 1892 at 60090, while `weaponstate` and `weapAnim` stay put. vcod: `weapon_anim_event` maps events 160 and 161 to the two blocks, and `ClientSim::note_footsteps` logs each continue with its cmd's `movetype`.

The damage path (`ClientSim::take_damage`) raises `death` and `pain`, whose clauses list several anims: the standing `death` clause eight, the crouched one five, the prone `pain` clause two. INFERRED: retail draws one at random, `commands[rand() % numCommands]` in RTCW's `bg_animation.c`; vcod's `AnimScript::select_event_random` does that off the server's generator (`vcod_common::rng::xorshift`). No capture measures the draw, only that a death plays one of the clause's anims. One draw per clause, so a `both` line lands on both channels. INFERRED: the conditions are read before the hit's knockback, since retail's are the ones the last pmove left (`BG_UpdateConditionValue`), not the frame's impulse; otherwise a standing player shot off his feet dies a running death.

VERIFIED, a probe run against vcod's server on 2026-09-03: a bullet death read `legsAnim` 22 with `torsoAnim` 512, where the two retail deaths read 17 and 18. All three are in the standing `death` clause; the draw landed where retail's captures did not (`cod11-combat.md` 9.1). The same run's death frame raised 189 alone where retail raises 189 then 155 `raiseweapon`; that is fixed, the tick now runs the victim's cmds in retail's order (`cod11-combat.md` 9.2).

`fireweapon`, `reload`, `dropweapon`, `raiseweapon`, `jump`, `jumpbk` and `land` list one anim per channel per clause in the shipped file, so first-match `select_event` is their whole answer (`only_the_random_events_list_more_than_one_anim_per_clause`, `animscript.rs`). `meleeattack` is the third multi-anim block; vcod raises it off `EV_MELEE_SWIPE` and draws it through `select_event_random`.
