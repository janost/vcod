# CoD 1.1 player model & animation system

Research notes for client-side player rendering. Sources: `game.mp.i386.so` disassembly (symbol table intact), CoDExtended source, RTCW-MP source, and pk3 assets. Confirmed unless marked otherwise. See `clientstate-wire-format.md` for how the model indices reach the client.

## Model assembly: body + attachments

A player is one body xmodel plus up to 6 attached xmodels (head, helmet, gear). `character/mp_american_airborne01.gsc` (pak5.pk3) builds one this way: `setModel` with `xmodel/playerbody_american_airborne` as the body, then five `attach` calls. The first attaches a head picked at random from `xmodelalias\head_allied::main()`; the second the helmet `xmodel/USAirborneHelmet`, stored on the entity as `hatModel`; and when `character\_utility::useOptionalModels()` allows it (that is `g_useGear`), three gear pieces `xmodel/gear_US_load1`, `xmodel/gear_US_bandolier` and `xmodel/gear_US_ammobelt`. None of the calls names a tag.

- Body -> `setModel` -> `clientState.modelindex`.
- Head -> random pick from `xmodelalias\head_allied::main()` (20 `basehead*` entries; 25 for airborne). `head_axis.gsc` is the Axis list.
- Helmet + up to 3 gear pieces -> `attach` calls. Total attachments <= 6, matching `attachModelIndex[6]`.

`attach(model)` with no tag grafts by the attached model's own root bone name. `xmodelparts/USAirborneHelmet0` is version 14, 0 non-root bones, 1 root named `bip01 head`. `basehead*` roots are shared spine/head bones (`bip01 spine2 / bip01 neck / bip01 head / skull / tag_eye / jaw / ...`). `attach(model, tag)` with an explicit tag overrides this; the tag name goes through `G_TagIndex` -> `attachTagIndex` (5 bits, 32 slots).

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

**Index order (VERIFIED against a running server and live traffic; implemented in `crates/common/src/animtree.rs`).** File order does *not* give the index. I read the array out of a running 1.1d dedicated server (gdb on `animations[]`, base `*(game.mp+0x7b3a8)`, stride 92, count at `+0xb800`) and confirmed it against live wire traffic. Index 0 is a synthetic `root` whose children are the file's top-level blocks (`main` and `turning`, so `turning` shares the index space). Expanding a node appends its kept children as one contiguous block in *reverse* file order, then expands each of those children in turn (depth first), so a group's subtree lands after everything already queued: legs' children 7..122, the MG42 sub-groups and their leaves 123..206, torso's children 207..253. A child is kept only if it is a block, its name is referenced by an uncommented `both|legs|torso|turret <name>` line in `mp/playeranim.script`, or one of its ancestors is referenced. The engine only keeps anims whose names are already interned, so 18 never-scripted leaves (`pb_crouchwalk_loop`, `pb_sprint_B`, `pb_stand_death_spin`, the `_pistol`/`_unarmed` combatrun variants, ...) are missing from the array and shift everything after them. The multiplayer tree flattens to exactly 254 entries; the ones the game module leaves named `unused` are the containers (`main`, `legs`, `torso`, `turning`, MG42 aim sub-groups). Anchors: 85 `pb_combatrun_forward_loop_rifles`, 91 `pb_combatrun_right_loop`, 94 `pb_combatrun_forward_loop`, 122 `pb_stand_alert`, 236 `pt_rifle_fire`, 253 `pt_stand_shoot`.

`mp/playeranim.script` is RTCW's format verbatim (`state {movetype {condition {both|legs|torso <anim>}}}`, conditions `weapons / weaponclass / position / movetype / leaning / weapon_position / mounted / strafing`); only the server needs it to pick indices. Body-part enum from exported `animBodyPartsStr` at `0x7b4f0`: `{ "** UNUSED **", "LEGS", "TORSO", "BOTH" }`.

195 `xanim/pb_*` files in the pk3s. Movement: `pb_combatrun_{forward,back,left,right}_loop` (+`_pistol`/`_unarmed`/`_heavy`/`_light`/`_rifles`/`_stickgrenades` variants), `pb_crouch_run_*`, `pb_crouchwalk_loop`, `pb_sprint`, `pb_prone_crawl{,_left,_right,_back}`, `pb_runjump_takeoff/_land`, `pb_standjump_takeoff/_land`, `pb_climbup/down`. Idles: `pb_stand_alert`, `pb_stand_ads`, `pb_crouch_alert`, `pb_prone_aim`. Deaths are legs-section nodes (`pb_stand_death_*`, `pb_crouch_death_*`, `pb_death_run_*`).

## xanim v14 header flags (decoded)

The header flag byte at offset 6 takes exactly four values in the shipped pk3s. Decoded by exact-EOF probing: the format has no length prefixes, so a correct layout consumes the file down to the last notetrack byte and nothing more. With the rules below all 2957 `xanim/*` entries decode byte-exact (862 flag 0, 48 flag 1, 1490 flag 2, 557 flag 3).

**Flag 0x1, looping.** Not metadata-only: a looping clip stores `frame_count + 1` key positions, the extra one being the loop-closing repeat of frame 0. Every count and index bound widens accordingly. vcod folds the extra position into `XAnim::frame_count`, so key indices stay in `0..frame_count` and `duration()` stays equal to the loop period.

**Flag 0x2, delta (root motion).** One extra nameless track sits between the header and the two bone bitsets, i.e. before everything else, not appended. Same shape as a bone track but with yaw-only rotations:

```
u16 rot_count; [frame indices]; rot_count * i16      // z-only quat, dequantized like a "simple" bone
u16 trans_count; [frame indices]; trans_count * 3*f32
```

Frame-index lists follow the same sparse rule as bone tracks (present only when `1 < count < frame_count`). The server owns entity movement, so the client parses this and drops it.

Flag 0x3 is simply both rules composed; no third layout.

**Frame index width.** Independently of the flags, the sparse frame-index list is `u8` only while the highest index fits a byte, and widens to `u16` above that. Pinned by `pegnight_facial_price_04_sgtgoback` (256 frames, u8) against a 257-frame clip (u16). 46 of the 862 flag-0 files need the `u16` width, all of them long facial/cinematic clips.

Player-anim coverage: 254 `pb_`/`pl_`/`pt_` files. `pb_*` splits 1/78/116 across flags 1/2/3, `pl_*` is 2 files at flag 1, `pt_*` is 43 at flag 0 and 14 at flag 1.

## Legs/torso split and aim layer

One skeleton, two anims. `pl_*`/`pt_*` clips only key their own bones, so applying legs then torso on one pose buffer reproduces the split. Aim pitch/lean is then distributed over named spine control bones by `BG_Player_DoControllers` (`game.mp.i386.so 0x2b7f8`):

```
G_DObjSetLocalTag(obj, ?, "tag_origin", angles)
G_DObjSetControlTagAngles(obj, ?, "back_low", &a)
G_DObjSetControlTagAngles(obj, ?, "back_mid", &a)
G_DObjSetControlTagAngles(obj, ?, "back_up",  &a)
G_DObjSetControlTagAngles(obj, ?, "pelvis",   &a)
```

using `AngleSubtract` / `AnglesSubtract` / `AngleNormalize180` / `GetLeanFraction`. Weight constants in `.rodata` at `0x6efa8..`: `0.5, 0.25, 50.0, 0.925, 1.5, 1.8, 2.5, 0.075, -1.2, 0.3, 0.1, 0.2, 0.8, -0.2, 0.4, -0.6`. The per-bone mapping of these constants is UNVERIFIED (not decoded).

Inputs are all transmitted in entityState: `fTorsoPitch` (232), `fWaistPitch` (236), `fTorsoHeight` (228), `leanf` (212), `animMovetype` (224, 4 bits).
