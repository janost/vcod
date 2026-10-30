# CoD 1.1 player model & animation system

Research notes for client-side player rendering. Sources: `game.mp.i386.so` disassembly (symbol table intact), CoDExtended source, RTCW-MP source, and pk3 assets. Confirmed unless marked otherwise. See `clientstate-wire-format.md` for how the model indices reach the client.

## Model assembly: body + attachments

A player is one body xmodel plus up to 6 attached xmodels (head, helmet, gear). `character/mp_american_airborne01.gsc` (pak5.pk3) builds one this way: `setModel` with `xmodel/playerbody_american_airborne` as the body, then five `attach` calls. The first attaches a head picked at random from `xmodelalias\head_allied::main()`; the second the helmet `xmodel/USAirborneHelmet`, stored on the entity as `hatModel`; and when `character\_utility::useOptionalModels()` allows it (that is `g_useGear`), three gear pieces `xmodel/gear_US_load1`, `xmodel/gear_US_bandolier` and `xmodel/gear_US_ammobelt`. None of the calls names a tag.

- Body -> `setModel` -> `clientState.modelindex`.
- Head -> random pick from `xmodelalias\head_allied::main()` (20 `basehead*` entries; 25 for airborne). `head_axis.gsc` is the Axis list.
- Helmet + up to 3 gear pieces -> `attach` calls. Total attachments <= 6, matching `attachModelIndex[6]`.

`attach(model)` with no tag grafts by the attached model's own root bone name. `xmodelparts/USAirborneHelmet0` is version 14, 0 non-root bones, 1 root named `bip01 head`. `basehead*` roots are shared spine/head bones (`bip01 spine2 / bip01 neck / bip01 head / skull / tag_eye / jaw / ...`). `attach(model, tag)` with an explicit tag overrides this; the tag name goes through `G_TagIndex` -> `attachTagIndex` (5 bits, 32 slots).

An attachment's non-root bones that share a name with a body-rig bone must alias that bone, not append duplicates: `basehead*` carries its own `bip01 spine2 / bip01 neck / bip01 head` chain, every stock `pb_*`/`pt_*` clip keys those names, and helmets root at the body's `bip01 head`/`tag_helmet`. Appended duplicates would hold bind pose while the body's copies animate, splitting the face from the helmet by 1.4 u standing up to 9.4 u prone (measured on `USAirborne3` + `basehead2` at frame 0 of the stock clips). Inferred from the asset shapes, not from decompilation of the engine's DObj merge; vcod merges by name against the base rig (`skeleton.rs`, `build_grafted`).

Model choice pipeline (all server-side; client only sees resulting indices): map `.gsc` sets `game["allies"]` / soldiertype -> `maps\mp\gametypes\_teams::modeltype()` -> `mptype/american_airborne.gsc` does `switch(randomint(9))` over `character/mp_american_airborne01..09`, each of which picks a random head.

Useful tags on `xmodelparts/USAirborne3` (79 non-root bones + root): `tag_helmet`, `tag_helmetside`, `tag_belt_*`, `tag_thigh_*`, `tag_calf_*`, `tag_shin_*`, `tag_breastpocket_*`, `tag_weapon_left/right`, control bones `pelvis`, `back_low`, `back_mid`, `back_up`.

## Grounding: origin at the feet

Baked bind pose of `xmodelparts/USAirborne3` (via the `crates/common/src/xmodel.rs` parser):

```
tag_origin           0.00    0.00     0.00
bip01 l toe0        -0.99    4.44     0.33
bip01 pelvis        -3.44   -0.00    37.23
bip01 neck          -3.12   -0.00    59.82
bip01 head          -2.18   -0.00    62.68
tag_helmet          -1.17   -0.00    70.88
```

Root `tag_origin` is at the feet: draw the model with root at `entityState.pos.trBase`, zero Z offset. Q3's -24 waist offset does not apply. Corroboration: `standViewHeight = 60` (CoDExtended `bg_pmove.c:135`, and `bip01 neck` at z=59.82); crouch 40, prone 11; vcod pmove already uses mins z=0.

## Animation indices: the animtree

`legsAnim`/`torsoAnim` are 10 bits in entityState (offsets 204/208) and playerState (112/120). RTCW heritage: `ANIM_BITS 10`, `ANIM_TOGGLEBIT = 512`. So index = `value & 511`, restart flag = `value & 512` (toggle change -> restart clip).

The index maps to a node index in the global MP animtree, not a per-model config. `BG_FinalizePlayerAnims` (`game.mp.i386.so 0x27d70`):

- `numAnimations = trap_XAnimGetAnimTreeSize(playerAnimTree)`; `animations[0].name = "root"`.
- Per node: `trap_XAnimIsPrimitive / GetAnimName / GetLength / GetRelDelta (moveSpeed) / IsLooped`.
- `animation_t` is 92 bytes, array cap `0xb800 / 0x5c` = exactly 512 (matches 9 usable bits).
- Anim script is `mp/playeranim.script` (hardcoded string in the module); the tree is `animtrees/multiplayer.atr` (pak5.pk3, 11270 bytes).

`.atr` structure: `main { torso { pt_* } legs { pb_*, pl_* } }`. Prefix convention (documented in the file's own header): `pt_` torso-only, `pl_` legs-only, `pb_` full body. Non-leaf nodes exist (`proneMG42_fire`, `standMG42_aim`, with `: complete loopsync` and aim-angle children) and consume indices; the client descends them using aim angles.

**Index order (VERIFIED against a running server and live traffic; implemented in `crates/common/src/animtree.rs`).** File order does *not* give the index. I read the array out of a running 1.1d dedicated server (gdb on `animations[]`, base `*(game.mp+0x7b3a8)`, stride 92, count at `+0xb800`) and confirmed it against live wire traffic. Index 0 is a synthetic `root` whose children are the file's top-level blocks (`main` and `turning`, so `turning` shares the index space). Expanding a node appends its kept children as one contiguous block in *reverse* file order, then expands each of those children in turn (depth first), so a group's subtree lands after everything already queued: legs' children 7..122, the MG42 sub-groups and their leaves 123..206, torso's children 207..253. A child is kept only if its name is referenced by an uncommented `both|legs|torso|turret <name>` line in `mp/playeranim.script`, one of its ancestors is referenced, or a node under it is (every block in 1.1's tree has a referenced node under it, so 1.1's array is the same whichever of the last two rules runs). The engine only keeps anims whose names are already interned, so 18 never-scripted leaves (`pb_crouchwalk_loop`, `pb_sprint_B`, `pb_stand_death_spin`, the `_pistol`/`_unarmed` combatrun variants, ...) are missing from the array and shift everything after them. The multiplayer tree flattens to exactly 254 entries; the ones the game module leaves named `unused` are the containers (`main`, `legs`, `torso`, `turning`, MG42 aim sub-groups). Anchors: 85 `pb_combatrun_forward_loop_rifles`, 91 `pb_combatrun_right_loop`, 94 `pb_combatrun_forward_loop`, 122 `pb_stand_alert`, 236 `pt_rifle_fire`, 253 `pt_stand_shoot`.

**United Offensive's array.** The same walk, with two things UO's script and tree add. Its script puts a legs and a torso anim on one line (`legs c_mp_jeep_passenger_idle torso c_mp_torso_stand_ads`) and spells one channel in capitals (`BOTH C_MP_BINOCULAR_ADS`), and every one of those names takes an index (INFERRED, the anchors below). Its tree has a block no line names anything under, `vehicleJeepGunnerDriving`, and that block takes none (INFERRED, the anchors below). With both rules the anims the retail UO 1.51b captures carry land where retail sends them: 85 `c_mp_binocular_ads`, 86 `c_mp_binocular_idle`, 108 `pb_prone_aim`, 174 `pb_crouch_alert`, 185 `pb_stand_alert`, 311 `pt_reload_stand_rifle`, 318 `pt_rifle_fire`, 320 `pt_rifle_fire_prone`, 341 `c_mg_lmg_idle`, 356 `c_mp_binocular_ads_torso` (VERIFIED, the snapshots-uo pairs and `mp_cassino-dm-combat-uo.txt` against `animtree::tests::uo_indices_match_the_retail_captures`). The five jeep passenger torso anims the one-line form adds sit at 282..286; no capture holds one of them (INFERRED).

`mp/playeranim.script` is RTCW's format verbatim (`state {movetype {condition {both|legs|torso <anim>}}}`, conditions `weapons / weaponclass / position / movetype / leaning / weapon_position / mounted / strafing`); only the server needs it to pick indices. Body-part enum from exported `animBodyPartsStr` at `0x7b4f0`: `{ "** UNUSED **", "LEGS", "TORSO", "BOTH" }`.

195 `xanim/pb_*` files in the pk3s. Movement: `pb_combatrun_{forward,back,left,right}_loop` (+`_pistol`/`_unarmed`/`_heavy`/`_light`/`_rifles`/`_stickgrenades` variants), `pb_crouch_run_*`, `pb_crouchwalk_loop`, `pb_sprint`, `pb_prone_crawl{,_left,_right,_back}`, `pb_runjump_takeoff/_land`, `pb_standjump_takeoff/_land`, `pb_climbup/down`. Idles: `pb_stand_alert`, `pb_stand_ads`, `pb_crouch_alert`, `pb_prone_aim`. Deaths are legs-section nodes (`pb_stand_death_*`, `pb_crouch_death_*`, `pb_death_run_*`).

There are no stance-transition clips: `animtrees/multiplayer.atr` has zero `2crouch|2prone|to_crouch|to_prone|transition|getup` entries, only per-stance idles/loops. Retail's smooth stance change is the engine cross-fading the outgoing and incoming animtree nodes over a blend time the anim setter picks; the server's and the cgame's copies of it, with the rate and phase, are in `cod11-movers.md` section 15 ("A moving body's clip", "The drawn body's clip"). vcod runs both through `vcod_common::animclock::ChannelClock`.

## xanim v14 header flags (decoded)

The header flag byte at offset 6 takes exactly four values in the shipped pk3s. Decoded by exact-EOF probing: the format has no length prefixes, so a correct layout consumes the file down to the last notetrack byte and nothing more. With the rules below all 2957 `xanim/*` entries decode byte-exact (862 flag 0, 48 flag 1, 1490 flag 2, 557 flag 3).

**Flag 0x1, looping.** Not metadata-only: a looping clip stores `frame_count + 1` key positions, the extra one being the loop-closing repeat of frame 0. Every count and index bound widens accordingly. vcod folds the extra position into `XAnim::frame_count`, so key indices stay in `0..frame_count` and `duration()` stays equal to the loop period.

**Flag 0x2, delta (root motion).** One extra nameless track sits between the header and the two bone bitsets, i.e. before everything else, not appended. Same shape as a bone track but with yaw-only rotations:

```
u16 rot_count; [frame indices]; rot_count * i16      // z-only quat, dequantized like a "simple" bone
u16 trans_count; [frame indices]; trans_count * 3*f32
```

Frame-index lists follow the same sparse rule as bone tracks (present only when `1 < count < frame_count`). vcod keeps it as `XAnim::root`. Its one reader is the mounted gunner's body placement, which hangs the `standMG42_*` leaves' root translation off the gun (`cod11-turrets.md` section 7).

Flag 0x3 is simply both rules composed; no third layout.

**Frame index width.** Independently of the flags, the sparse frame-index list is `u8` only while the highest index fits a byte, and widens to `u16` above that. Pinned by `pegnight_facial_price_04_sgtgoback` (256 frames, u8) against a 257-frame clip (u16). 46 of the 862 flag-0 files need the `u16` width, all of them long facial/cinematic clips.

Player-anim coverage: 254 `pb_`/`pl_`/`pt_` files. `pb_*` splits 1/78/116 across flags 1/2/3, `pl_*` is 2 files at flag 1, `pt_*` is 43 at flag 0 and 14 at flag 1.

## Legs/torso split and aim layer

One skeleton, two anims, one tree. `pt_*` clips key only the upper body (spine up, arms, weapon tags); `pb_*` clips key every bone. The tree walk blends `main`'s `torso` and `legs` nodes per bone, and a torso anim weights them 1 and 0.01, so the bones a torso clip keys take it with 1% of the legs clip and the rest take the legs clip alone (`cod11-movers.md` 15, "Torso over legs"). `BG_Player_DoControllers` (`game.mp.i386.so 0x2b7f8`) then sets a local tag on `tag_origin` (the legs' yaw off the view, the lean's roll and shift, the prone slope) and control angles on `pelvis`, `back_low`, `back_mid`, `back_up`, `neck` and `head`. Its inputs are the three swings `BG_PlayerAnimation`'s angle updater (0x2af78) keeps in the client's record: legs yaw, torso yaw and torso pitch. All of it is decoded in `cod11-combat.md` 16.3 (pitch, the control axis) and 16.4 (yaw, lean, `tag_origin`, the anim record's movetype and strafe flags). vcod: `vcod_common::playerpose::{BodyAngles, Controllers, apply_controllers}`, shared by the server's locational pose and the client's draw. A dead body's hook runs too, its `tag_origin` turned by the legs' world yaw rather than their yaw off the view, so a tag lookup on it reads the body turned twice; the client draws no dead player (`cod11-combat.md` 16.4, "The dying body"). VERIFIED, live (1.1d, 1.5b and UO 1.51b, 2026-10-17, `probe_leanhit` with `probe_bones 1`): eleven bones of a standing, crouched and prone body, level and leaning both ways, sit within 0.0007 units of ours on all three, 1.5b's standing body once his origin's 0.04 is taken out (`leanhit_ab`, `cod11-combat.md` 3.6).

Inputs are all transmitted in entityState: `fTorsoPitch` (232), `fWaistPitch` (236), `fTorsoHeight` (228), `leanf` (212), `animMovetype` (224, 4 bits).

### 1.5's and UO's stance window, drawn

The cgame's lerp-frame setter carries the same 400 ms window as the server's (`coduo-vehicles.md` 13): 1.5's `cgame_mp_x86.dll` 0x30003870 stores `cg.time + 400` at `ci+0x4a4`, UO's `uo_cgame_mp_x86.dll` 0x30003a90 at `ci+0x4c0`, when the legs frame (`ci+0x380` on UO) changes crouch or prone class, and floors every later blend of either frame to what is left of it (VERIFIED decompile). 1.1's cgame has no such store. vcod's drawn bodies (`entities.rs`, `update_channels`) run `ChannelClock::update_windowed` off 1.1, one window per drawn entity; the cgame keeps it per client info, so a corpse sharing the dead client's info could read his last window, which vcod does not model (INFERRED). Not measured against a retail client's draw (pixels).

### The prone body on a grade

VERIFIED, live (retail 1.1d, 2026-10-16, `client-probes/probe_pronebones`, fixtures in `crates/server/tests/fixtures/pronebones/`): a prone client facing downhill on mp_carentan's ~19 degree mound (-224, 60) has his pelvis at (5.92, 24.36, -0.03) off his origin and his feet about 10 under it, where on flat ground the pelvis sits 6.27 above the origin. vcod's server pose puts all eight logged bones (pelvis, spine2, neck, head, both hands, both feet) within 0.03 of retail's offsets on the mound, on flat ground on mp_carentan and on mp_brecourt's field at (1659, 781). With the slope terms zeroed, the mound's pelvis lands 5.8 too high. The bones are read through a `script_origin` linked to each tag, which shows the pose a frame late, so a still body is what the probe measures.

VERIFIED, same runs: where the body settles is set by the fall from `setorigin`'s raised unit (cod11-player-clip.md 8.9), not by the pose. The drop lands anywhere inside the ground trace's quarter unit (retail read -23.77 and -23.87 on the same flat spot in two runs). On the brecourt grade it slides up to a unit sideways, and two retail runs slid it differently.

VERIFIED, `probe_scriptmodel`'s prone part on mp_brecourt: before 2026-10-16 the part held its target with `distance(origin, tpos) > 0.5`, z included. On the field he settles 1.33 under `tpos`, so every frame he was put back a unit above it and dropped onto the grade again: pm_flags prone throughout, origin cycling between -101.76 and -104.09, on retail and on vcod alike. The frag met him at whatever height that cycle and the cmd timing gave, which is how a frag came to rest 13 units off retail's spot. Held across the ground only, retail's first frag rests at (1657.12, 817.81, -80.92) and vcod's within 0.12 of it (`scriptmodel_ab::a_frag_rests_on_a_prone_players_back_on_a_grade`).

VERIFIED, live (retail 1.5b and United Offensive 1.51b, 2026-10-16, the same probe, now placing the body twice a second apart so the second drop starts at rest; fixtures `*-1.5.txt` and `*-uo.txt` beside the 1.1 ones, which were retaken the same way): on all three spots vcod's eight bones sit within 0.05 of retail's off the origin once the playerstate's prone body (`fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`) is retail's, and `pronepose_ab` holds that body to the fixture's within 0.01. On 1.5 and UO the body keeps whatever the drop's last airborne cmd sampled (cod15-movement.md 4, "The airborne prone body"): -0.134 on mp_carentan's flat spot, which lowers every bone 0.134 against 1.1's. The pose itself needs nothing 1.5-specific beyond the controllers' later split (coduo-vehicles.md 14). Before 2026-10-16 the gate's even 16 ms cmds left ours' body 0.1 to 0.6 off retail's, which read as a pose error.

VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-19, `probe_pronebones` with `probe_rays 1`, fixtures `mp_brecourt-brecourt-rays-1.5.txt` and `-rays-uo.txt`): the probe prints the settled origin and each bone mark to the float (integer part and exact remainder) and a 2-unit grid of downward `bulletTrace`s over the body. With the client's own cmds replayed (`ProbeHold::times`) and the engine's pose arithmetic ported (`cod11-combat.md` 3.1, "The pose on 1.5 and UO"), ours' origin, marks and every ray are retail's bits on both servers, 1.5b's rays once its clips run on the engine's float node time (`cod11-combat.md` 3.1, "The clip's float time"). VERIFIED, the same runs and four more: the drop onto the grade leaves `fTorsoHeight` anywhere from -0.425 to -0.516 by the cmds' split (it read -0.491 in the 2026-10-16 runs), and the whole body rides it; the 1.5b run without `probe_spawn_yaw` lay at `proneDirection` -170 off its spawn's yaw, so the 1.5b fixture spawns on the spot.

## The animscript: which index the server picks

The server picks `legsAnim`/`torsoAnim` by running `mp/playeranim.script`
against the player's state. `crates/common/src/animscript.rs` is the parser and
the selector; `ClientSim::update_anims` (`crates/server/src/spectate.rs`) builds
the conditions. What follows is what the file says and what retail actually
sent.

### The file

VERIFIED, read from `mp/playeranim.script` in `pak4.pk3` (1177 lines): three
sections, `DEFINES` at line 18, `ANIMATIONS` at 98, `EVENTS` at 576. VERIFIED:
`ANIMATIONS` holds three states, `STATE RELAXED` (100-103) and `STATE ALERT`
(105-108) are both empty braces, and `STATE COMBAT` (110-548) carries every
movetype block. INFERRED: `COMBAT` is therefore the only state with content in
multiplayer, so the server never has to decide which state a player is in.

VERIFIED: `STATE COMBAT`'s movetype blocks are `idle`, `idlecr`, `idleprone`,
`walk`, `walkbk`, `walkcr`, `walkcrbk`, `walkprone`, `walkpronebk`, `run`,
`runbk`, `runcr`, `runcrbk`, `climbup`, `climbdown`, `turnright`, `turnleft`.
VERIFIED: `EVENTS` holds `fireweapon`, `meleeattack`, `dropweapon`,
`raiseweapon`, `reload`, `jump`, `jumpbk`, `land`, `DEATH`, `revive`, `pain`.
VERIFIED: the header legend at lines 73-76 is RTCW's and does not match what
ships, listing `straferight`/`strafeleft` (no such block exists) and omitting
`climbup`/`climbdown` (both do).

VERIFIED: the condition keywords the file uses in live clauses are `weapons`,
`weaponclass`, `weapon_position`, `strafing`, `movetype`, `mounted` and
`firing`. VERIFIED: `enemy_weapon` and `impact_point` appear inside `DEATH` and
`pain` only in commented-out clauses (script lines 1032-1094 and 1116-1167), so
no live clause tests either. INFERRED: a condition the parser has no kind for
therefore cannot be reached in the shipped file; it parses to a kind that never
holds, so an unmodelled condition makes its clause unselectable rather than
unconditional (`crates/common/src/animscript.rs`).
VERIFIED: `leaning`, `position`, `underwater` and `underhand` occur only in the
commented condition legend at lines 78-91 and in no clause. INFERRED: a lean
therefore changes no index, and the parser carries no kind for those four.

### What retail sent, pose by pose

VERIFIED, both captures: `crates/server/tests/fixtures/playerstate/mp_carentan-dm-motion.txt`
and `mp_pavlov-dm-motion.txt`, 22 poses each, taken with `--net-probe
--save-motion` against the retail 1.1d dedicated server, one client, `dm`,
weapon `m1carbine_mp` (`weaponclass rifle`). Each pose holds one usercmd until
the playerstate settles. Indices below are `legsAnim & 511`, the toggle
stripped; the horizontal speed is the pose's own `velocity`.

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

VERIFIED: `torsoAnim` is 0 in every pose of both captures, `jump_takeoff`
included, and `playerstate_motion_ab` compares it: no movement pose raises a
weapon event, so a torso index in one is a defect. VERIFIED: every clause
reached in the table is written `both`.
INFERRED: retail therefore does not write the torso half of the continuous
selection, so vcod clears `Selection::torso` before applying it and leaves the
torso to events. The captures taken before 2026-09-02 read 720 (index 208,
`pt_stand_pullout_pose`) at `jump_takeoff`, which was a holstered weapon and
not a clause; "The `jump_takeoff` torso index is gone" below has the
measurement.

### Unarmed

VERIFIED, live (retail 1.1d, 1.5b and UO 1.51b, 2026-10-16,
`client-probes/probe_pronebones` with `probe_spawn_yaw 315`, fixtures
`crates/server/tests/fixtures/pronebones/mp_brecourt-spawn315*.txt`): a
player the script `spawn`s and gives no weapon (`ps.weapon` 0) crouches in
`pb_crouch_alert_unarmed` (1.1 and 1.5 index 107) and lies in
`pb_prone_aim_pistol` (44), the clips `idlecr`'s and `idleprone`'s
`weapons none` clauses (script lines 176 and 221) select. INFERRED: the
`weapons` condition names weapon 0 `none`, as `getCurrentWeapon` does.
`pb_prone_aim_pistol` is a 3 s loop where `pb_prone_aim` is a 0.3 s pose,
so an unarmed prone body keeps moving (the pelvis about 0.1 a second).
vcod names weapon 0 `none` for the conditions (`AnimInputs`).

### How retail picks the movetype

VERIFIED, read out of `PM_Footsteps` at `game.mp.i386.so` 0x322c8 -- the static
after `PM_ShouldMakeFootsteps`, the same function `crates/common/src/pmove.rs`
and `cod11-sound-system.md` already call by that name -- with the movetype and
condition enum orders taken from the rodata name blocks at 0x6e260 and 0x6e424.
VERIFIED: both blocks are stored in reverse, `** UNUSED **` first, so the
movetype order is `idle` 1, `idlecr` 2, `idleprone` 3, `walk` 4, `walkbk` 5,
`walkcr` 6, `walkcrbk` 7, `walkprone` 8, `walkpronebk` 9, `run` 10, `runbk` 11,
`runcr` 12, `runcrbk` 13. This replaces two readings of the 44 poses that
earlier rounds of this document carried and that the binary contradicts; the
poses still confirm the result, and the anim gate in
`crates/server/tests/playerstate_motion_ab.rs` holds it.

**The idle threshold.** VERIFIED: at 0x3249f the function compares `xyspeed`
against the float at 0x70c1c, which holds 10.0, and neither arm of that compare
reads the usercmd. INFERRED: the arm taken below it selects the stance's idle
and the arm at or above it is the movetype dispatch below. VERIFIED: the
capture agrees on both sides of the number -- carentan `run_forward` pushes
`forward=127` into
geometry at 8.0 u/s and reads 122, the standing idle, while both maps'
`prone_crawl_150` crawls at 13.0 u/s and reads 50, `pb_prone_crawl`. VERIFIED:
carentan `strafe_left` at 8.0 u/s and `strafe_right` at 3.2 u/s read 122 as
well, and pavlov `crouch_run` at 0 u/s reads 111. `ANIM_IDLE_SPEED`
(`crates/server/src/spectate.rs`) is that literal.

**The speed it compares.** VERIFIED: 1.5b's `PmoveSingle` main arm
calls `PM_Footsteps` (Ghidra 0x34ae8, file 0x24ae8) ahead of the move
clamp block and `trap_SnapVector(ps->velocity)`, its last call, so
`xyspeed` is the unsnapped velocity. INFERRED: 1.1d is the same; its
`PmoveSingle` calls `PM_Footsteps` at 0x34336, ahead of the calls at
0x34364 and 0x34390, which `pmove.rs` ports as the clamp and the snap. VERIFIED, live (retail 1.5b, 2026-10-20,
`mp_carentan-dm-legs-client-stopgo-1.5.txt` at serverTime 36750): a
strafe reversal's last cmd leaves a velocity that snaps to (0, 10, 0)
and retail reads the idle (122); the snapped 10.0 is not under the
threshold, so the speed the compare read was. vcod keeps the footsteps'
speed in `PlayerState::footsteps_xyspeed` and selects on it.

**The direction.** VERIFIED: above the threshold the movetype is the stance
crossed with `pm_flags` 0x40, backwards, and `pm_flags` 0x80, walk. The walk bit
is masked out of `pm_flags` once at 0x32499 and the stance dispatches at
0x326c4, 0x32710 and 0x32766; each arm tests 0x40 and passes a movetype index to
the selection call at 0x327d3. VERIFIED: those indices are 8 and 9 prone, 6, 7,
12 and 13 crouched, 4, 5, 10 and 11 standing, which against the name block above
are the `walkprone`, `walkcr`/`runcr` and `walk`/`run` families. VERIFIED: the
prone arm at 0x326f1 tests 0x40 alone and never the walk bit, so prone has
exactly two movetypes and reaches `walkprone` whatever that bit says. VERIFIED:
no arm between 0x326c4 and 0x327d1 reads the usercmd.

VERIFIED: 0x40 is latched in `PmoveSingle` before the move, in the block that
loads the cmd's `forwardmove` (usercmd +0x18) at 0x3413e, sets 0x40 in
`pm_flags` (playerState +0xc) at 0x34147, clears it at 0x3415e, and reads
`rightmove` (+0x19) at 0x34156. INFERRED: the set arm is the one taken when
`forwardmove < 0` and the clear arm when `forwardmove > 0` or `forwardmove` is
zero with a non-zero `rightmove`; there is no third arm, so a cmd asking for
neither axis leaves the bit as it was. VERIFIED: RTCW carries the same three
arms in `PmoveSingle`
(RTCW-MP's `src/game/bg_pmove.c:3915`), which is the lineage
`crates/common/src/pmove.rs` ports. VERIFIED: both captures carry 0x40 at
`run_back` and at none of the other 42 poses. VERIFIED: 0x80 is the ADS
walk, which the sight sets, and the `walk*` blocks are reached through it
(see "Walk against run").

**The stop.** VERIFIED: past the movetype's selection call (0x327d8, its
result kept in `esi`) and the bob update, 0x3282a tests the cmd's
`forwardmove` and `rightmove` bytes as one word (`pm+0x18`) and 0x32831
compares the float at 0x70c48, 120.0, with `xyspeed`; with the word 0 and
`xyspeed` not over 120 it compares `ps+0xd8` with `ps+0x33c` and `ps+0x340`
as the under-10 arm at 0x324c1 does and calls the selection again with
movetype 2 (`idlecr`, 0x3285b) or 3 (`idleprone`, 0x3286c); otherwise it
calls it with 1 (`idle`, 0x3287f) only when the first call returned
negative. INFERRED, off those branches: a crouched or prone body with no
move key held takes its idle once it slows to 120, and a standing one keeps
its move clip down to the 10 u/s threshold unless the clip found no clause.
VERIFIED, the `legs_ab` capture (`movers/mp_carentan-dm-legs-client.txt`):
the first crouched snapshot after the back key is released reads index 111,
`pb_crouch_alert`, at 84 u/s. INFERRED: two selection calls in one pmove leave
`legsAnim` as one would: the move clip it replaces is either the one already
playing (no change) or flips the toggle a second time back. vcod:
`ANIM_STOP_SPEED` in `crates/server/src/spectate.rs`.

**The stance.** VERIFIED: the move dispatch's stance (`ebp-0x20`) is
computed at 0x32340..0x3239a: prone when `pm_flags` 0x1 is set, when
`ps+0xd8` equals `ps+0x33c`, or when `ps+0xd4` is non-zero, `ps+0xd8`
equals `ps+0x340` and `ps+0xdc` is 0; crouched when `pm_flags` 0x2 is set
or `ps+0xd4` is non-zero with `ps+0xd8` equal to `ps+0x340`; standing
otherwise. VERIFIED: both idle arms (0x324c1..0x324f4 under 10 u/s,
0x32847..0x32871 at the stop) compare `ps+0xd8` with `ps+0x33c` and
`ps+0x340` and read no `pm_flags` bit: `idleprone` on the first, `idlecr`
on the second, `idle` otherwise. VERIFIED: the mounted arm (0x3230a) reads
`pm_flags` 0x1 and 0x2. INFERRED: `ps+0xd8` is `viewHeightLerpTarget`,
`ps+0xd4` `viewHeightLerpTime`, `ps+0xdc` `viewHeightLerpDown` and
`ps+0x33c`/`0x340` the prone and crouch eye heights, the same test
`PM_GetEffectiveStance` (0x34554) makes, so a lie-down from standing idles
crouched through the eye's first leg (the leg's target is the crouch) and
prone only once the second leg starts. VERIFIED, live (retail 1.1d,
2026-10-16, `crates/server/tests/fixtures/pronebones/mp_brecourt-spawn315.txt`):
the first grounded snapshot of a lie-down, `pm_flags` 0x40001, reads
`legsAnim` 107 (`pb_crouch_alert_unarmed`) and the next one 556 (44,
`pb_prone_aim_pistol`). vcod: `ClientSim::update_anims` picks 1.1's idles
off `view_lerp_target` and its move clips off `effective_stance`;
`pronepose_ab`'s `a_spawn_lies_down_on_its_own_yaw_like_retail` holds the
bones. 1.5 and UO take every arm off the stepped eye target
(cod15-movement.md 4, "The spawn's lie-down").

**The strafe condition.** VERIFIED: condition 8 is written between 0x32504 and
0x325b5. The block loads `forwardmove` at 0x32504 and `rightmove` at 0x32568,
and reaches the update call at 0x325b0 from three places, pushing condition 8
with value 0 at 0x3255d, value 1 at 0x325a2 and value 2 at 0x3258d; the path
leaving 0x3256d lands past the call. INFERRED: value 0 is `NOT` and every arm a
non-zero `forwardmove` takes reaches it, diagonals included; 1 and 2 are the two
sides and are reached only with `forwardmove` zero and `rightmove` non-zero; and
the path that skips the call is the one a cmd asking for neither axis takes, so
the condition keeps its previous value. INFERRED: it is therefore state carried
between frames rather than a reading of the current cmd, and `ClientSim` holds
it that way. VERIFIED: the file's legend at line 91 says strafing "will never be
left or right while moving backwards".

**Nothing is selected in the air.** VERIFIED: 0x323a2 compares
`groundEntityNum` (playerState +0x54) against 1023 and 0x323af tests `pm_flags`
0x10, the ladder flag; the arm leaving 0x323b3 lands on the function's tail at
0x328c1, past every selection call. INFERRED: that is the arm an airborne player
without the ladder flag takes, so a jump keeps the takeoff anim until the
landing. VERIFIED: retail's restart toggle flips exactly once between the
`jump_takeoff` and `land` samples on both maps, which is what one selection
between the two looks like. INFERRED: the two
ground-edge events have to be raised before the continuous selection, or the
landing frame selects an idle first and flips the toggle twice. VERIFIED: the
`land` default clause carries `duration 100`, and the fixture's `land` pose is
sampled on the first frame back on the ground (`sample=grounded`) for that
reason. VERIFIED: the mp_pavlov capture's `run_back` pose reads
`groundEntityNum` 1023 and `legsAnim` 605, index 93, the `runbk` loop.
INFERRED: retail's probe backed off a ledge and kept the loop it was already
playing, so leaving the ground is not what raises the takeoff; that step rests
on the airborne early-out, which is itself a branch condition. INFERRED: the takeoff comes from the jump
impulse itself, so vcod raises `jump`, or `jumpbk` while the backwards latch is
set, only on the frame pmove takes one. UNVERIFIED: whether a fall that nobody
jumped into raises `land` on touchdown; no pose in either capture lands from
one, and vcod raises it.

VERIFIED: the ladder flag is `pm_flags` 0x10 and 0x323af tests it.
INFERRED: a set flag skips the airborne early-out, so a climber reaches the
selection. VERIFIED: the
shipped script carries `climbup` and `climbdown` blocks, each a single default
clause selecting `pb_climbup` and `pb_climbdown`. INFERRED: those two are the
movetypes a climber selects, and the climb direction is the sign of the vertical
velocity; vcod derives them that way and raises no takeoff on the mount, since
mounting takes no impulse. Neither capture climbs a ladder, so nothing here is
measured against retail beyond the flag and the blocks.

**The lie-down in the air.** VERIFIED, `game.mp.i386.so` 1.5b
`PM_Footsteps` (0x24ae8): an airborne body whose eye target is not its
flags' stance goes on to the ground code (0x24e51; cod15-movement.md 4,
the footstep cadence). INFERRED, off the same branch: the movetype
selection after it runs for that body too, which is the stance window's
idle. VERIFIED, live (retail 1.5b and UO 1.51b, 2026-10-20,
`probe_scriptmodel`'s prone part, the `-cmds` fixtures and a gdb run):
a target holding prone as `setorigin` drops him a unit shows the
lie-down's legs (1.5b 111, UO 174) in the snapshot where `pm_flags` first
reads prone, still in the air; vcod showed them a snapshot later, on the
landing. vcod: `ClientSim::footsteps_select` takes that arm under 1.5 and
UO rules, and `update_anims` selects through it, so the prone idle's
clip starts on retail's frame (`scriptmodel_ab`'s `-cmds` gates, combat
doc 3.1, "The prone strike on mp_carentan").

**What this overturned.** VERIFIED: pavlov `run_forward` carries velocity
`(0, -148)` at `viewangles[1]` -31.94, 58 degrees off the facing, and pavlov
`strafe_right` carries `(-143, 0)`; projecting either onto the facing selects
the wrong clause, 91 where retail sent 94 and 93 where retail sent 91. INFERRED,
and wrong: an earlier round read that as "the direction comes from the usercmd",
which agrees with every pose in the capture and still differs from retail on a
diagonal, where the latch rule leaves the strafe condition cleared and reading
the cmd sets it. INFERRED, and wrong: the same round read a coasting player as
keeping its anim. VERIFIED: RTCW does hold one --
RTCW-MP's `src/game/bg_pmove.c:1607-1622` returns without
selecting when neither axis is asked for and `xyspeed > 120`, commented
"continue what they were doing last frame, until we stop" -- and VERIFIED: retail
dropped it, since the only float compare in `PM_Footsteps` is the one against
10.0 at 0x3249f and the function holds no 120 at all. INFERRED: retail therefore
re-selects every frame above the threshold, so a tap and release slides in the
run loop rather than the idle, and a backwards crouched coast is `runcrbk`
rather than a standing back-run. Neither reading is in the code.

### Walk against run

VERIFIED: CoD 1.1 MP has one move speed and no walk bit in the usercmd
(`docs/protocol-1.1.md`, "Usercmd input bits"). VERIFIED: retail chose the
`run` family in every moving standing or crouched pose of both movement
captures (94, 93, 92, 91, 84), and in those the only walk-family block it
selected was `walkprone` (50), which prone reaches because there is no
`runprone`. VERIFIED: holding the ads bit for two seconds selected the
standing ads clause on both maps, `ads_stand` reading 121 `pb_stand_ads`.
The captures taken before 2026-09-02 read the plain standing idle 122 there,
and the only difference is the usercmd's `weapon` byte: a probe sending 0 was
holstered, so its ads bit reached no weapon.

VERIFIED, `game.mp.i386.so` 1.1d: `PM_Footsteps` keeps `pm_flags & 0x80` in
`edi` (0x32490, 0x32499), and each standing and crouched arm passes the walk
index for it: standing 5 (`walkbk`, 0x32783) or 4 (`walk`, 0x327b5) with it,
11 or 10 without; crouched 7 (0x32745) or 6 (0x3275c) with it, 13 or 12
without. `pm_flags` 0x80 is `PM_UpdatePlayerWalkingFlag`'s (0x33694): the
sight held, not prone and not reloading (vcod
`pmove::weapon::ads_pm_flags`). VERIFIED, live (retail 1.1d, 2026-10-17,
`movers/mp_carentan-dm-legs-client-torso.txt`, step `ads_walk_fire`): walking
forward with the sight up reads `pm_flags` 0x400a0 and `legsAnim` 573, index
61 `pb_stand_shoot_walk_forward`, the `walk` block's default clause, at 90
u/s. So the `walk*` blocks are the ADS locomotion the file's NOTES at line 94
describe ("The player walks when they are ADS, so they can not ADS while
running"), reached through the walk bit, not the usercmd. Before 2026-10-17
vcod never entered them and ran the ADS walk on 94, the run.

VERIFIED, `game.mp.i386.so` 1.5b: `PM_Footsteps` (0x24ae8) sets its walk
local (`ebp-0x28`) when `pm_flags & 0x80` (0x24e6f) or the float at
`ps+0x40`, `leanf`, is non-zero (0x24e82); the movetype dispatch tests that
local (0x25307, 0x253c9). VERIFIED, `game.mp.uo.i386.so`: UO's (0x297e4)
takes `pm_flags & 0x80` alone (0x29ba4, `ebp-0x28`, tested at 0x29eae and
on); its lean test (0x29a2a, `ps+0x44`) sits in the bob block above.
INFERRED: a 1.5 runner that leans plays the walk clips; no capture shows it.
vcod: `ClientSim::footsteps_movetype_of` (`crates/server/src/spectate.rs`).

### The weapon event's movetype

VERIFIED, `game.mp.i386.so` 1.1d, `PmoveSingle` (0x33dfc, relocations
resolved with `tools/re/annotate_func.py`): the main arm calls `PM_Weapon`
(0x34331) and then `PM_Footsteps` (0x34336). The `fireweapon` and `reload`
events `PM_Weapon` raises select their clause by the `movetype` condition,
which only `PM_Footsteps`' selection writes, so they read the movetype the
previous pmove left. VERIFIED, 1.5b: the main arm calls `PM_Footsteps`
(0x28889) and then `PM_Weapon` (0x2888e); UO likewise (0x2e04e, 0x2e053).
INFERRED: there the event reads the movetype its own pmove just wrote.

VERIFIED, live (retail 1.1d, 2026-10-17, the torso capture above): the first
shot of each moving step, sent in the first cmd that moves, writes the idle's
fire anim: `run_fire` 253 `pt_stand_shoot`, `strafe_fire` 253,
`crouch_run_fire` 249 `pt_crouch_shoot` and `ads_walk_fire` 251
`pt_stand_shoot_ads`, each held for ten snapshots; the shots after it,
while moving, write nothing (the `movetype moving` clauses are empty).
Before 2026-10-17 vcod picked every event's clause by the movetype at the end
of the frame, and none of those four anims played. vcod:
`ClientSim::note_footsteps`, called after each pmove of a cmd's chop; the
events carry the movetype it returns (`spectate::AnimEvent`).

### The turn movetypes, and what selects them

VERIFIED: `turn_left` and `turn_right` turn the view 60 degrees with no movement
input and retail sent 122, the standing idle, on both maps. VERIFIED: the
`turnright` and `turnleft` blocks select `legs pl_chicken_dance` and
`legs pl_chicken_dance_crouch`, gated on `movetype idle` and `movetype idlecr`.
VERIFIED: `animtrees/multiplayer.atr` puts exactly those two clips in a
top-level `turning` group, commented "temp turning animations", and they are the
only `pl_*` names in the tree; they resolve to indices 4 and 3, neither of which
appears in either capture.

VERIFIED, by cross-reference: `angles2[1]` is `ps.movementDir`, not the body
yaw this section used to guess at (`docs/protocol-1.1.md`, "What a player
entity looks like", carries the addresses and the held-input measurements).
That closes what the field is; it does not close what enters `turnright` or
`turnleft`.

INFERRED: `movementDir` cannot be what selects those blocks. `set_movement_dir`
(`crates/common/src/pmove.rs`) zeroes it whenever neither move key is held, and
`turn_left`/`turn_right` hold no movement input, so `movementDir` reads 0
through the whole turn regardless of whether the legs should follow.

The open question is narrower than it was: not what the field is, but what, if
anything, ever drives a player into `turnright`/`turnleft`. INFERRED, off the
retail control flow ported into `walk_move`
(`crates/common/src/pmove.rs`, from `0x2f6db`): `PM_WalkMove` skips the
position step when velocity is zero but still calls `PM_SetMovementDir`
afterward, and prone's branch of that function reads `proneDirection` against
the view yaw rather than displacement, which is why a stationary prone
player's legs keep turning with its body. INFERRED: that mechanism is specific
to prone; nothing in `PM_SetMovementDir` has an equivalent stand/crouch branch,
so if `turnright`/`turnleft` are reachable at all, whatever selects them is
neither `movementDir` nor this skip-the-move path.

What would settle it is a capture that samples every frame through a sustained
stationary turn rather than only settled poses, reading `legsAnim` on its own.
If it ever reads 3 or 4, something drives the turn blocks and is worth chasing;
if it reads 122 for the whole turn, the blocks are dead in multiplayer.
INFERRED, and worth stating as the live possibility rather than a settled one:
the `turning` group's own "temp" comment, and index 3 and 4 appearing in
neither the held-input captures nor the two-probe entity list, are both
consistent with that.

### The restart toggle

VERIFIED: the toggle's absolute value is not a function of the pose. `stand_up`
is index 122 in both captures and `legsAnim` reads 634 (toggle set) on
mp_carentan against 122 (toggle clear) on mp_pavlov; `run_back` is index 93 in
both and reads 93 against 605. INFERRED: only the change carries meaning, so a
gate compares the index, and checks frame by frame that the bit flips exactly
when the index does, never the bit's value.

### Open questions

- Closed. The `torsoAnim` 720 and `weaponstate` 2 both captures used to read at
  `jump_takeoff` were a holstered weapon: they came from a `cmd.weapon` of 0
  and the retaken captures read 0 and 0 ("The `jump_takeoff` torso index is
  gone").
- Closed. `pm_flags` bit 0x8 read set at `jump_takeoff` on mp_pavlov and clear
  on mp_carentan in the superseded captures; VERIFIED: the retaken pair reads
  it clear on both, which is what vcod does, so nothing is left to explain and
  `playerstate_motion_ab` no longer excepts it.
- VERIFIED, in a run not committed: the ads pose latches. A capture that
  pressed the ads bit in the middle of the script kept `pm_flags` 0x20 and 0x80
  and `legsAnim` index 121 `pb_stand_ads` for every pose after it, through a
  crouch run, a stand and two turns, with the bit long released; the same run
  read them clear at the jump that followed. UNVERIFIED: what clears them, and
  whether the jump is what did. The probe's motion script holds `ads_stand`
  last because of this, so the committed captures cannot show it.

## The weapon channel: what writes `torsoAnim`

VERIFIED: `crates/server/tests/fixtures/playerstate/mp_carentan-dm-combat.txt`
and `mp_pavlov-dm-combat.txt`, nine steps each, taken with `--net-probe
--save-combat` against the retail 1.1d dedicated server, one client, `dm`. Each
step holds one input and taps the attack bit; the fixture carries one `!trace`
line per snapshot rather than a settled sample, because the event ring holds
four slots and a shot is shorter than a step.

The captures committed before 2026-09-02 were taken with `cmd.weapon` 0 in
every usercmd, which retail's weapon-change check reads as a request to
holster (`cod11-combat.md` section 1.8): they carry a `weaponstate` 2 and an
`EV_PUTAWAY_WEAPON` at the reload key, at both stance changes and at the jump
that the input never asked for, and their reload key never reloaded. Both
were retaken with the probe sending the weapon it holds, and every one of
those putaways is gone. Where a claim below rests on the superseded pair it
says so; they are in git at commit `0243e35`. VERIFIED: snapshots arrive 50 ms
apart, so a run of N consecutive samples in one state bounds that state's
length to `((N-1)*50, (N+1)*50)` ms and no measurement below is tighter than
that.

VERIFIED: carentan's join gave `m1carbine_mp` and pavlov's `mosin_nagant_mp`,
read from `weapons/mp/*` in `pak0.pk3`:

| weapon | `fireTime` | `rechamberTime` | `reloadTime` | `dropTime` | `clipSize` |
|---|---|---|---|---|---|
| `m1carbine_mp` | 0.135 | 0.1 | 2.65 | 0.67 | 15 |
| `mosin_nagant_mp` | 0.33 | 1.0 | 2.4 | 0.4 | 5 |

Two weapons with different numbers is the point: it is what lets a state be
matched to a weapon-file field rather than to one rifle's timing.

### Firing

VERIFIED: a shot reads `weaponstate` 3 and `weapAnim` 514, and bumps
`eventSequence` by one carrying event 159, `EV_FIRE_WEAPON`
(`cod11-events-and-fx.md`). VERIFIED: the event is written to
`events[eventSequence & 3]` and the counter incremented after -- every bump in
both captures moves exactly that slot. VERIFIED: `weapAnim` carries the same
512 toggle the anim channels do, since pavlov's traces hold 2 and 514, and 4
and 516, the same index with and without the bit.

VERIFIED: `weaponstate` 3 runs 3 samples on carentan, 100-200 ms, and 7 on
pavlov, 300-400 ms (6, 250-350 ms, on some shots: the crouched and prone
ones of the 2026-10-17 recapture, the single and crouched ones of the
2026-10-18 one). INFERRED: that is the weapon's `fireTime`, the only
weapon-file time inside both windows at 0.135 s and 0.33 s.

VERIFIED: the torso index a shot writes is stance-specific. Every standing fire
step on both maps reads index 253, `pt_stand_shoot`; the crouched step reads
249, `pt_crouch_shoot`. INFERRED: the torso channel is the weapon's, not the
animscript's. Those two are `pt_*` leaves no clause the continuous selection
reaches produces, retail sent `torsoAnim` 0 in all 20 settled poses of the
movement captures, and the index here moves with `weaponstate` and the stance
rather than with the movetype.

VERIFIED, resolved out of the animtree (`crates/common/src/animtree.rs`,
`PlayerAnims`): the shoot family is 245 `pt_stand_shoot_pistol`, 246
`pt_crouch_shoot_auto_ads`, 247 `pt_crouch_shoot_ads`, 248
`pt_crouch_shoot_auto`, 249 `pt_crouch_shoot`, 250 `pt_stand_shoot_auto_ads`,
251 `pt_stand_shoot_ads`, 252 `pt_stand_shoot_auto`, 253 `pt_stand_shoot`, and
prone adds 222 `pt_prone_shoot_auto`, 223 `pt_prone_shoot_pistol` and 224
`pt_prone_shoot`. UNVERIFIED: every member of it but 224, 249 and 253. The
probe never held the ads bit through a shot, and neither rifle is
automatic.

### The restart toggle, doing its job

VERIFIED, carentan `sustained_fire`: six shots read 765, 253, 765, 253, 765,
253 -- index 253 throughout, with bit 512 flipping on every `eventSequence`
bump. VERIFIED, carentan `crouch_fire`: 249, 761, 249 across three shots.
INFERRED: a repeated shot is signalled by the toggle alone, so a client that
watches only the index plays the first shot's clip and nothing after it. This
is the first thing measured in this project that needs the toggle for what it
is for; "The restart toggle" above only ever saw it as noise.

VERIFIED: when a shot ends the torso reads 512, index 0 with the toggle set, so
the channel clears to no anim and flips the bit doing it.

### `weaponstate`, and what each value carried

| `weaponstate` | seen on | event at the bump | torso index | run | weapon-file match |
|---|---|---|---|---|---|
| 0 | both | -- | -- | -- | ready |
| 3 | both | 159 `EV_FIRE_WEAPON` | 253 standing, 249 crouched, 224 prone | 100-200 ms, 300-400 ms | `fireTime` 0.135, 0.33 |
| 4 | pavlov | 162 `EV_RECHAMBER_WEAPON` | unchanged | 950-1100 ms | `rechamberTime` 1.0 |
| 5 | both | 151 `EV_RELOAD` | 229 `pt_reload_stand_auto`, 231 `pt_reload_stand_rifle` | 2600-2700 ms, 2350-2450 ms | `reloadTime` 2.65, 2.4 |

VERIFIED: `weaponstate` 2 appears nowhere in either capture, and neither does
event 156. It is in the superseded pair at the reload key, at both stance
changes and at the jump, and every one of those went away when the probe
started sending the weapon it holds.

Paired columns are carentan then pavlov. VERIFIED: the event ids, the torso
indices and the run lengths, all read off the traces. INFERRED: the right-hand
column, because a duration falling inside a bound is a match and not a proof,
and INFERRED: the state numbers themselves, which are read off behaviour rather
than out of an enum.

VERIFIED: 4 appears on pavlov only. INFERRED: that is the bolt action --
the carbine's `rechamberTime` is 0.1 s, under two snapshot intervals, and its
15-round clip never emptied.

VERIFIED: pavlov's last round of a clip raised 161 `EV_FIRE_WEAPON_LASTSHOT`
rather than 159, and wrote the same torso index 253.

The paragraph that follows rests on the superseded pair, whose putaways came
from a `cmd.weapon` of 0 rather than from the input. What put the weapon away
there is not what these steps do, but the putaway it measured is a real one and
nothing since has measured another.

VERIFIED, superseded pair: the pullout pose is stance-matched the way the shoot
anim is, 208 `pt_stand_pullout_pose` standing and 209 `pt_crouch_pullout_pose`
crouched, and both maps' `stand_between` step reads 209 while `legsAnim`
already reads the standing idle. INFERRED: the index is latched when the state
opens, since that step's state opened while the player was still crouched.
VERIFIED, same pair: the torso index clears before the state does -- carentan's
reload step holds 208 for about 500 ms of a `weaponstate` 2 that runs 600-700
ms.

**The reload key reloads a partly-full clip.** VERIFIED: the step that taps
`wbuttons` 0x08 reads `weaponstate` 5 with event 151 `EV_RELOAD` on both maps,
for 53 samples on carentan and 48 on pavlov, which are `reloadTime` 2.65 and
2.4. VERIFIED: the torso index is the weapon's, 229 `pt_reload_stand_auto` for
the carbine and 231 `pt_reload_stand_rifle` for the mosin, and `weapAnim`
takes index 11 `WEAP_RELOAD`. That settles what `cod11-combat.md` 1.7 read off
the instructions and could not see happen.

VERIFIED: the superseded pair read `weaponstate` 2 with event 156 here instead,
with rounds still in the clip and no state anywhere near `reloadTime`. VERIFIED:
the only difference between the two runs is the usercmd's `weapon` byte.

### What `weapAnim` is not written by

VERIFIED, superseded pair: no `weaponstate` 2 in either of those captures
writes `weapAnim`. Carentan's reload step holds index 0 for all 13 samples of
the state and takes 512 -- index 0 with the toggle flipped -- on the sample the
state ends; pavlov's `stand_between` holds the rechamber index 4 for all 8 and
lands on 512 the same way.

VERIFIED: the putaway does store `WEAP_DROP`. The grenade capture's `to_frag`
step reads `weapAnim` 521 -- index 9 with the toggle -- for the whole of
`weaponstate` 2, and 10 then 512 across the raise that follows, one write per
transition (`cod11-combat.md`, 1.14). INFERRED: the two runs above sent
`cmd.weapon` 0 in every usercmd and 1.2's setter refuses to write on that
input, so their unchanged channel is the same artifact as their phantom
putaways and not a fact about the path. The reading this paragraph used to
carry, that the putaway stores no anim and the `WEAP_DROP` of section 1.8 is
only the event's parm, is superseded.

VERIFIED: a rechamber ending writes no `weapAnim` either. Pavlov's
`single_shot` reads 516 across the sample where `weaponstate` goes 4 to 0, and
its `sustained_fire` never shows index 0 at all between two shots.

VERIFIED: pavlov's `weaponstate` goes 3 to 4 on consecutive samples, 50 ms
of server time apart, and its `weapAnim` 2 to 4 across the same pair: 341 to
390 ms on `sustained_fire` and 359 to 408 on `prone_fire` (re-measured on
the 2026-10-18 recapture; the 2026-10-17 one read 344 to 408 and 310 to 374
on `crouch_fire`, an earlier capture 328 to 378).
INFERRED: the shot's state ends and the rechamber opens within one server
frame. That frame holds several cmds, so it does not show whether the
rechamber waits a cmd for the state 3 exit; it does (`cod11-combat.md` 1.4).

### The `jump_takeoff` torso index is gone

VERIFIED: the movement captures committed before 2026-09-02 read `weaponstate`
2 and `torsoAnim` 720 at the `jump_takeoff` pose, and the retaken pair reads 0
and 0. VERIFIED: the only difference is the usercmd's `weapon` byte, which the
jump's own `upmove` change is what puts on the wire (the compact usercmd
branch carries no weapon byte at all, so a probe sending 0 only reaches the
weapon-change check when `upmove`, `wbuttons` or the byte itself moves;
`docs/protocol-1.1.md`, "Usercmd delta"). INFERRED: so the 720 was a holstered
weapon and not an animscript clause, and there is nothing left here to
explain.

### Prone fire, taken on both maps

VERIFIED: carentan's `prone_fire` step reads `legsAnim` 557 (index 45,
`pb_prone_aim`) and `torsoAnim` 224 `pt_prone_shoot` across its three shots.
That is the prone shoot pose this document previously carried as unreachable.
VERIFIED: on the 2026-10-17 recapture pavlov's reads the same, `legsAnim` 557
and `torsoAnim` 736 (224 with the toggle), and so does the 2026-10-18
recapture's from another spawn. VERIFIED: 230
`pt_reload_prone_rifle` appears in neither capture, since neither reload ran
prone.

VERIFIED: the recapture holds the spawn point's yaw. The walk of `advance`
ends turned on pavlov (90 on the 2026-10-18 recapture) and `idle` brings
the view back to the spawn's facing (carentan -180, pavlov 45), where every
later step holds it, at (995.1, 1038.3, -13.0) and (-7095.1, 10360.9,
-103.9); carentan's walk never stalled. The 2026-10-17 take ended its walks
at 135 and 45, faced 45 and 0 and stood at (-522.6, 1997.2, 144.1) and
(-8829.7, 10319.9, -23.6).

VERIFIED, first capture (in git at `c52e9606`): pavlov's `prone_fire` read
`legsAnim` 634 (index 122, the standing idle) and the standing shoot pose 253
throughout, with the crouch taken at the same spot one step earlier
(`legsAnim` 111). INFERRED: the prone was refused there and the step recorded
a standing shot under a prone label. VERIFIED: both of that capture's settled
field lines turn the view during the still `idle` step after the walk while
every `!input` line records yaw 0: pavlov's `viewangles[1]` reads -135 after
`advance` and 90 from `idle` on at (-7440.875, 10399.125, 309.23), carentan's
0 and then -90 at (-233.41, 1684.30, -7.87), 40 units from where its walk
ended. INFERRED: that is the probe's stall turn, which rode past the walk
until 5233cc3, a week after both captures. INFERRED: facing 90 on pavlov the
body has a wall 15 units behind it. vcod's `BG_CheckProneValid` port
(`pmove::check_prone`, 0x2d428) meets it at fraction 1/6 of the sweep 24 up
behind the facing, a reach of 15 against the 17 (`HALF_WIDTH + 2`) it needs,
and refuses at every yaw from 80 to 100; facing -135 it takes the prone. So
the refusal was the spot and the facing, not the map.

As implemented: `playerstate_combat_ab` starts the step after such a turn at
retail's settled origin and yaw (`Step::place`), and compares `prone_fire` on
both maps like every other step. Against the first capture ours lay down on
carentan and stayed standing on pavlov, as retail did.

### Neither counter in `!observed` counts shots

VERIFIED: carentan's `sustained_fire` reads `shots_seen` 5 against six taps and
six 159 events (3 on the 2026-10-17 recapture, 4 before it), and pavlov's `single_shot` reads `seq_delta` 3 against one tap.
INFERRED for the first: `shots_seen` counts `weaponstate` 0 -> 3 edges, and at
carentan's 183 ms tap period against a 135 ms `fireTime` the weapon is ready
for under one snapshot interval between shots, so two or three of the six edges
fall between samples. VERIFIED for the second: one mosin shot raises three events,
159, then 162 `EV_RECHAMBER_WEAPON`, then 163 `EV_EJECT_BRASS`, so `seq_delta`
is three per shot on pavlov and one per shot on carentan, whose carbine raises
only 159.

INFERRED: a gate therefore counts `events[]` entries equal to 159, or 161 for
the last round of a clip, across the per-snapshot traces, and never
`seq_delta` or `shots_seen`. VERIFIED: the ring holds four slots, so nothing
but a per-snapshot trace can count a burst longer than four.

### As implemented

The torso channel is not a table of its own: the weapon machine raises an
animscript event and `mp/playeranim.script`'s own `EVENTS` clauses pick the
index. `ClientSim::update_anims` (`crates/server/src/spectate.rs`) maps each
`PmEvent` the frame's moves raised onto a block -- 159/161 `fireweapon`,
151/152/153 `reload`, 156 `dropweapon`, 155 `raiseweapon` -- and runs it
through `AnimScript::select_event`.

VERIFIED: the shipped script's clauses reproduce every index the two captures
read, with no engine table involved. The `fireweapon` block gates on the
`movetype` categories the file's own `DEFINES` build (`set movetype crouching =
idlecr AND runcr AND ...`), so `pt_stand_shoot` 253, `pt_crouch_shoot` 249 and
`pt_prone_shoot` 224 fall out of the stance; `reload` gates on the weapon name
first, which is why the carbine takes `pt_reload_stand_auto` 229 (it is in the
block's `bar_mp AND ... AND m1carbine_mp` list) and the mosin falls through to
the `default` clause's `pt_reload_stand_rifle` 231. That is
`the_event_blocks_pick_the_indices_the_combat_captures_read` in
`crates/common/src/animscript.rs`.

VERIFIED decompile, `BG_PlayAnim` (1.1d `game.mp.i386.so` 0x3c338, UO
`game.mp.uo.i386.so` 0x2bfb9): the event sets the channel's timer
(`torsoTimer`, `legsTimer`) to its duration plus 50 (0x32), the clip's own
length plus 50 for a line with none, and a continuous anim takes the
channel only while the timer reads under 50 (or with the force argument).
So an event holds for its duration alone, and the hold runs on the timer,
which pmove counts down by each cmd's msec. VERIFIED, the jeep seats
capture (`coduo-vehicles.md` 8): the passenger's `pt_rifle_fire` (318)
starts with the cmd at 45700, `torsoTimer` reads 1342 at commandTime
45741, 42 at 47041, and that snapshot already carries the continuous 794.
vcod-server (`AnimState`, `animscript.rs`) keeps the hold on the cmd clock,
each event stamped with its cmd's `serverTime` and the release read at the
round's last cmd, and releases under 50; it had released 50 ms late on the
frame clock and another 50 late on `>= duration + 50`, two snapshots in
all (`vehicle_ab::the_jeep_seats_replay_on_ours`). The length comes from
the xanim header (`PlayerAnims::length_ms`), truncated as the engine's
`XAnimGetLength` stores it (VERIFIED disassembly, `cod_lnxded`
0x08088b3d..0x08088b5b; `cod11-combat.md` 16.4, "The dying body"). Both
timers ride the playerstate: vcod-server writes each as the event's
expiry less `commandTime`, floored at 0 (`AnimState::timers`), which the
death frames of all three servers match, and so does every snapshot of
the per-cmd captures `animtimers_ab` replays (below, "The takeoff and the
landing"). The hold is not the weapon state's length: VERIFIED,
carentan's reload holds the torso for 950-1050 ms of a `weaponstate` 5 that
runs 2650, and pavlov's for 2050-2150 ms of one that runs 2400, which are the
31-frame `pt_reload_stand_auto` and the 64-frame `pt_reload_stand_rifle` at 30
fps. UNVERIFIED: the exact rounding. The four measured clears bracket the
clip's own length to (900, 1050], (2000, 2150], (400, 550] and (450, 600] ms
against clip lengths of 1000, 2100, 400 and 500, so a 50 ms sample grid cannot
tell `duration` from `duration + 50` and the two shot clears lean one way while
the two reload clears lean the other; the decompile above settles it on
`duration`.

INFERRED: the derived hold has a floor of 500 ms
(`animscript::MIN_EVENT_HOLD_MS`), which is the fifth measurement: the
superseded pair's `dropweapon` held 208 `pt_stand_pullout_pose` for about 500
ms, and that clip is a single frame, 0 ms long. A floor is what fits, and
`max(clip, the weapon state's own length)` is not: the carbine's reload clip is
1000 ms inside a `weaponstate` 5 that runs 2650, and the capture clears at the
clip. 500 leaves all four fire and reload holds where they were measured (the
two reload clips are longer than it, and `pt_stand_shoot`'s 400 and
`pt_crouch_shoot`'s 500 hold 500, inside both maps' brackets). UNVERIFIED: nothing
in the current pair puts a weapon away, so the floor is carried by the
superseded measurement alone and no committed capture exercises it. A clause's
own `duration` never takes the floor, so the `duration 150` autofire shots keep
their 150.

The channel clears the way the capture reads it: when the hold runs out and the
continuous selection has nothing for the torso -- which is always, since retail
sends index 0 in every settled pose -- `AnimState::clear_torso` writes index 0
and flips the toggle, giving the 253 -> 512 the captures end a shot with. A
repeated shot restarts the same index, so the toggle is the only thing that
moves.

`crates/server/tests/playerstate_combat_ab.rs` compares, per fire and reload
step, the set of `torsoAnim & 511` values and the number of toggle flips.
Matching on both maps: carentan reads {0, 253} over 2 flips for one shot, {0,
253} over 7 for six, {0, 229} over 2 for the reload and {0, 249} over 4 for
three crouched shots; pavlov reads {0, 253} over 2 and over 4, {0, 231} over 2
and {0, 249} over 2.

### The takeoff and the landing

VERIFIED disassembly, 1.1d `game.mp.i386.so` `PmoveSingle` 0x33dfc, its
walking arm: `PM_CheckDuck` (0x316f4, the call at 0x342c9), then
`PM_GroundTrace` (0x30474, call at 0x342ce), then `PM_DropTimers`
(0x32a44, call at 0x342f9; it runs `legsTimer` at ps+0x6c and `torsoTimer`
at ps+0x74 down by the cmd's msec, floored at 0, 0x32a7d..0x32abc), then
`PM_WalkMove` (0x2f258, call at 0x3431b, which opens with `PM_CheckJump`)
and a second `PM_GroundTrace` (call at 0x34327). Three places raise the
`jump` event, `jumpbk` where `cmd.forwardmove < 0`, each through
`BG_AnimScriptEvent` (VERIFIED decompile, the same three on 1.5b
`game.mp.i386.so` 0x31a17 / 0x31ccd and UO, the `%i:lift` and
`%i:kickoff` strings):

- `PM_CheckJump` (0x2eb98), forced, in its ground arm only; a ladder
  push-off raises none.
- `PM_GroundTrace`'s kickoff: a hit with the velocity carrying the body
  off the plane (`velocity.z > 0`, `velocity . normal > 10`) and
  `pm_flags` 0x10 clear, not forced, so it takes the legs only while
  `legsTimer` reads under 50.
- `PM_GroundTraceMissed` (0x3037c), forced, for a body that had ground
  when a trace 64 straight down from the origin meets nothing.

So the timer an event sets depends on where it is raised. VERIFIED,
`animtimers_ab` over `mp_carentan-dm-slope-8ms-moves-1.1.txt` and
`-springfield-1.1.txt`: a jump reads 55 (`duration 5` + 50) less the cmds
after the jump cmd, the jump cmd's own msec not among them; the 1.1 prone
dive, whose impulse `PM_CheckDuck` gives ahead of the first ground trace,
is a kickoff in that trace and reads 55 less its own cmd's msec as well
(20 at ct 48335 for a dive pressed on the cmd at 48308, 19 at 53344 for
one at 53317); a landing (`PM_CrashLand` inside the second trace) reads
150 less the cmds after it. Each picks its clause by the `movetype` the
last `PM_Footsteps` left: the backward dive reads `pb_standjump_takeoff`
off `runbk`, not the prone `pb_chicken_dance_crouch` the body has reached
by the frame's end (VERIFIED, same capture).

vcod: pmove reports the event (`pmove::Takeoff`: forced or not, `back`,
and whether the first ground trace raised it) and the landing per cmd;
`ClientSim` keeps the cmd's serverTime, less its msec for an event ahead
of the drop, and the `movetype` before the move, and `update_anims`
plays it on that clock. The motion fixtures cannot gate the two
transient samples (`jump_takeoff`, `land`): they log no cmd times, and
retail's reads 39 and 150 on carentan, 5 and 102 on pavlov, under one
script, which is 55 and 150 less the cmds between the event and the
sample (INFERRED from the model above, which the per-cmd gates VERIFY).

UO's deploy and breakdown raise their own blocks, `LMG_DEPLOY` and
`LMG_BREAKDOWN` (events 0xe and 0xf, forced; VERIFIED decompile, UO
`game.mp.uo.i386.so` 0x347aa and 0x3484c, beside events 160 and 161), and
`PM_Weapon`'s deploy driver (0x35b66) raises `LMG_BREAKDOWN` again on every
cmd in `weaponstate` 13 while ps+0x100 (`viewHeightLerpTarget`) is
non-zero, as a continue (VERIFIED disassembly, 0x35bed..0x35c23:
`BG_AnimScriptEvent(ps, 0xf, 1, 1)`). INFERRED from the `BG_PlayAnim` decompile cited
above (its continue argument): a continue leaves an anim already on the
channel alone, so it only does anything when the clause changes,
which the stance does. VERIFIED, `mp_carentan-dm-slope-8ms-sprint-uo.txt`:
the deploy reads `torsoAnim` `c_mp_lmg_deploy_prone` with `torsoTimer`
1825 at ct 48091, 1850 less 25 ms; the prone breakdown
`c_mp_lmg_breakdown_prone_torso` 1926 at 58582; and the stand-up at the
cmd of 60066 restarts the torso on `c_mp_mg34_breakdown_torso`, 1892 at
60090, while `weaponstate` and `weapAnim` stay as they were.
`weapon_anim_event` maps events 160 and 161 to the two blocks, and
`ClientSim::note_footsteps` logs each continue with its cmd's `movetype`.

The `death` and `pain` blocks are the two the damage path raises
(`ClientSim::take_damage`), and they are the ones whose clauses list several
anims: the standing `death` clause lists eight, the crouched one five, the
prone `pain` clause two. INFERRED: one of them is drawn at random,
`commands[rand() % numCommands]` in RTCW's `bg_animation.c`, which is what
`AnimScript::select_event_random` does off the server's own generator
(`vcod_common::rng::xorshift`); no capture we hold measures the draw, only
that a death plays one of the clause's anims. One draw per clause, so a
`both` line lands on both channels. INFERRED: the conditions are read before
the hit's knockback is applied, since retail's are the ones the last pmove
left (`BG_UpdateConditionValue`) rather than the impulse of the frame; without
that a standing player shot off his feet dies a running death.

VERIFIED: a probe run against vcod's own server on 2026-09-03 read `legsAnim`
22 with `torsoAnim` 512 on a bullet death, where the two retail deaths read 17
and 18; all three are indices of the standing `death` clause, which is the
first observation of the draw landing somewhere retail's captures did not
(`cod11-combat.md` section 9.1). VERIFIED from the same run: vcod's death
frame raises event 189 alone and reads `eventSequence` 1, where retail raises
189 then 155 and reads 2, so the weapon channel's `raiseweapon` event does not
fire on a death yet (section 9.2).

The events `select_event` still serves -- `fireweapon`, `reload`,
`dropweapon`, `raiseweapon`, `jump`, `jumpbk`, `land` -- list one anim per
channel per clause in the shipped file, so first-match is their whole answer
(`only_the_random_events_list_more_than_one_anim_per_clause`,
`crates/common/src/animscript.rs`). `meleeattack` is the third multi-line
block and nothing raises it yet.
