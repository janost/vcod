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

**Index order (VERIFIED against a running server and live traffic; implemented in `crates/common/src/animtree.rs`).** File order does *not* give the index. I read the array out of a running 1.1d dedicated server (gdb on `animations[]`, base `*(game.mp+0x7b3a8)`, stride 92, count at `+0xb800`) and confirmed it against live wire traffic. Index 0 is a synthetic `root` whose children are the file's top-level blocks (`main` and `turning`, so `turning` shares the index space). Expanding a node appends its kept children as one contiguous block in *reverse* file order, then expands each of those children in turn (depth first), so a group's subtree lands after everything already queued: legs' children 7..122, the MG42 sub-groups and their leaves 123..206, torso's children 207..253. A child is kept only if it is a block, its name is referenced by an uncommented `both|legs|torso|turret <name>` line in `mp/playeranim.script`, or one of its ancestors is referenced. The engine only keeps anims whose names are already interned, so 18 never-scripted leaves (`pb_crouchwalk_loop`, `pb_sprint_B`, `pb_stand_death_spin`, the `_pistol`/`_unarmed` combatrun variants, ...) are missing from the array and shift everything after them. The multiplayer tree flattens to exactly 254 entries; the ones the game module leaves named `unused` are the containers (`main`, `legs`, `torso`, `turning`, MG42 aim sub-groups). Anchors: 85 `pb_combatrun_forward_loop_rifles`, 91 `pb_combatrun_right_loop`, 94 `pb_combatrun_forward_loop`, 122 `pb_stand_alert`, 236 `pt_rifle_fire`, 253 `pt_stand_shoot`.

`mp/playeranim.script` is RTCW's format verbatim (`state {movetype {condition {both|legs|torso <anim>}}}`, conditions `weapons / weaponclass / position / movetype / leaning / weapon_position / mounted / strafing`); only the server needs it to pick indices. Body-part enum from exported `animBodyPartsStr` at `0x7b4f0`: `{ "** UNUSED **", "LEGS", "TORSO", "BOTH" }`.

195 `xanim/pb_*` files in the pk3s. Movement: `pb_combatrun_{forward,back,left,right}_loop` (+`_pistol`/`_unarmed`/`_heavy`/`_light`/`_rifles`/`_stickgrenades` variants), `pb_crouch_run_*`, `pb_crouchwalk_loop`, `pb_sprint`, `pb_prone_crawl{,_left,_right,_back}`, `pb_runjump_takeoff/_land`, `pb_standjump_takeoff/_land`, `pb_climbup/down`. Idles: `pb_stand_alert`, `pb_stand_ads`, `pb_crouch_alert`, `pb_prone_aim`. Deaths are legs-section nodes (`pb_stand_death_*`, `pb_crouch_death_*`, `pb_death_run_*`).

There are no stance-transition clips: `animtrees/multiplayer.atr` has zero `2crouch|2prone|to_crouch|to_prone|transition|getup` entries, only per-stance idles/loops. Retail's smooth stance change is the engine cross-fading the outgoing and incoming animtree nodes over a blend time (inferred; the per-node times live in the XAnim tree, not the pk3 scripts). vcod approximates with a flat 200 ms cross-fade on clip switch (`entities.rs`, `ANIM_BLEND_MS`).

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
`weaponclass`, `weapon_position`, `strafing`, `movetype`, `mounted`, `firing`,
`enemy_weapon` and `impact_point`, the last two only inside `DEATH` and `pain`.
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
| `ads_stand` | `buttons=16` held 2 s | 122 | 122 | `idle` default, not the `weapon_position ads` clause |
| `crouch_run` | crouch + `forward=127` | 84 at 58.8 u/s | 111 at 0 u/s | `runcr` default, `pb_crouch_run_forward` / `idlecr` default |
| `turn_left`, `turn_right` | yaw only | 122 | 122 | `idle` default, not `turnleft`/`turnright` |
| `jump_takeoff` | `up=127`, first airborne frame | 100 | 100 | `jump` default, `legs pb_standjump_takeoff duration 5 blendtime 100` |
| `land` | first frame back on the ground | 99 | 99 | `land` default, `legs pb_standjump_land duration 100 blendtime 50` |

VERIFIED: `torsoAnim` is 0 in all 20 settled poses of both captures and 720
(index 208, `pt_stand_pullout_pose`) at `jump_takeoff` on both. VERIFIED: every
clause reached in the table is written `both`. INFERRED: retail therefore does
not write the torso half of the continuous selection, so vcod clears
`Selection::torso` before applying it and leaves the torso to events.

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
(`private/reference/RTCW-MP/src/game/bg_pmove.c:3915`), which is the lineage
`crates/common/src/pmove.rs` ports. VERIFIED: both captures carry 0x40 at
`run_back` and at none of the other 42 poses. INFERRED: 0x80 is the walk bit and
nothing in a CoD 1.1 usercmd can set it (see "Walk against run"), so the `walk*`
blocks are unreachable.

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
reason. INFERRED: leaving the ground raises `jump`, or `jumpbk` while the
backwards latch is set, and touching it raises `land`.

**What this overturned.** VERIFIED: pavlov `run_forward` carries velocity
`(0, -148)` at `viewangles[1]` -31.94, 58 degrees off the facing, and pavlov
`strafe_right` carries `(-143, 0)`; projecting either onto the facing selects
the wrong clause, 91 where retail sent 94 and 93 where retail sent 91. INFERRED,
and wrong: an earlier round read that as "the direction comes from the usercmd",
which agrees with every pose in the capture and still differs from retail on a
diagonal, where the latch rule leaves the strafe condition cleared and reading
the cmd sets it. INFERRED, and wrong: the same round read a coasting player as
keeping its anim. VERIFIED: RTCW does hold one --
`private/reference/RTCW-MP/src/game/bg_pmove.c:1607-1622` returns without
selecting when neither axis is asked for and `xyspeed > 120`, commented
"continue what they were doing last frame, until we stop" -- and VERIFIED: retail
dropped it, since the only float compare in `PM_Footsteps` is the one against
10.0 at 0x3249f and the function holds no 120 at all. INFERRED: retail therefore
re-selects every frame above the threshold, so a tap and release slides in the
run loop rather than the idle, and a backwards crouched coast is `runcrbk`
rather than a standing back-run. Neither reading is in the code.

### Walk against run

VERIFIED: CoD 1.1 MP has one move speed and no walk bit in the usercmd
(`docs/protocol-1.1.md`, "Usercmd input bits"), so nothing in the input picks
between the `walk*` and `run*` blocks. VERIFIED: retail chose the `run` family
in every moving standing or crouched pose of both captures (94, 93, 92, 91, 84),
and the only walk-family block it ever selected was `walkprone` (50), which
prone reaches because there is no `runprone`. VERIFIED: holding the ads bit for
two seconds selected no ads clause on either map, `ads_stand` reading the plain
standing idle 122. VERIFIED: the file's own NOTES at line 94 read "The player
walks when they are ADS, so they can not ADS while running", and the `walkbk`
block carries the comment "Always ADS when walking". INFERRED: `walk`, `walkbk`,
`walkcr` and `walkcrbk` are the ADS locomotion set and are unreachable from a
usercmd alone, so vcod parses them, holds `ads` false, and never enters them.

### The turn movetypes, and a body yaw that lags the view

VERIFIED: `turn_left` and `turn_right` turn the view 60 degrees with no movement
input and retail sent 122, the standing idle, on both maps. VERIFIED: the
`turnright` and `turnleft` blocks select `legs pl_chicken_dance` and
`legs pl_chicken_dance_crouch`, gated on `movetype idle` and `movetype idlecr`.
VERIFIED: `animtrees/multiplayer.atr` puts exactly those two clips in a
top-level `turning` group, commented "temp turning animations", and they are the
only `pl_*` names in the tree; they resolve to indices 4 and 3, neither of which
appears in either capture.

INFERRED: the turn movetypes are entered from a body yaw that lags the view
rather than from the view yaw itself, so a 60-degree snap between two settled
samples never enters them; the `pl_*` prefix (legs only) and the placement in
`turning` rather than under `main` fit a legs-only overlay that turns the body
towards the view while the torso already faces it. VERIFIED: a player entity
carries an `angles2` at entityState offsets 104/108/112 that the protocol doc
leaves unidentified (`docs/protocol-1.1.md`, "What a player entity looks like").
INFERRED: `angles2[1]` is a candidate for that body yaw. What would settle it is
a capture that samples every frame through a
sustained turn rather than at settled poses, reading `angles2[1]` against
`viewangles[1]` and `legsAnim` together. If `angles2[1]` trails the view during
the turn and `legsAnim` reads 3 or 4 while it trails, the hypothesis holds; if
`legsAnim` stays 122 through the whole turn, the turn blocks are dead in
multiplayer and the clip names say why.

### The restart toggle

VERIFIED: the toggle's absolute value is not a function of the pose. `stand_up`
is index 122 in both captures and `legsAnim` reads 634 (toggle set) on
mp_carentan against 122 (toggle clear) on mp_pavlov; `run_back` is index 93 in
both and reads 93 against 605. INFERRED: only the change carries meaning, so a
gate compares the index, and checks frame by frame that the bit flips exactly
when the index does, never the bit's value.

### Open questions

- VERIFIED: retail sends `torsoAnim` 720 (index 208, `pt_stand_pullout_pose`) at
  `jump_takeoff` on both maps. INFERRED: no clause the jump path reaches
  produces it, and nothing in vcod writes it. It stays unexplained.
- VERIFIED: `pm_flags` bit 0x8 reads set at `jump_takeoff` on mp_pavlov and
  clear at the same pose on mp_carentan, while both samples are the first
  airborne frame and both carry a `velocity[2]` within ten units of the standing
  jump impulse (237 and 222 against `sqrt(2 * 34 * 800)` = 233 u/s). INFERRED:
  the bit is not a function of the pose, so something neither capture records
  decides it. INFERRED: half of it resolves, since retail sets the bit inside
  `PM_Jump` (`game.mp.i386.so 0x2ec34`) and runs that for a ground jump too,
  while vcod sets `jump_latched` only in `ladder_push_off`
  (`crates/common/src/pmove.rs`). Why retail's carentan sample reads it clear
  does not resolve.
