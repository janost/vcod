# CoD2 1.3 player bodies and their animation

How CoD2 builds and poses another player's body, against 1.1
(`player-model-anim-system.md`, `cod11-combat.md` 16.3-16.4) and 1.5/UO
(`coduo-vehicles.md` 14). Addresses are `cod2_lnxded` (1.3 Linux server)
first, the Mac 1.3 client (`cod2mp_mac13_i386`, symbols intact) beside
them. The two share `bg_*`: the server poses bodies for its traces with
the same code the cgame draws them with.

Sources: the two decompiles, `.rodata`/`.data` read straight from both
binaries, the stock `iw_07.iwd`, `iw_08.iwd` and `iw_13.iwd`, and the
retail capture `crates/cod2/tests/fixtures/mp_toujane-spectate.bin` (a
second client running on `cod2_lnxded`).

## 1. The models

- The gametype picks a character through `mptype/<team type>.gsc`, which
  calls one of the `character/mp_*.gsc` scripts (37 in `iw_13.iwd`); each
  runs `setModel` or `codescripts\character::setModelFromArray` over an
  `xmodelalias/mp_body_*` list, then `attach`es a head (a literal or
  `attachFromArray` over `xmodelalias/mp_head_*`) and a `hatModel`, every
  attach with an empty tag. VERIFIED (the scripts).
- `ClientEndFrame` (0x80f7516) calls 0x80f72f8, which writes the entity's
  model index into the client state's `modelindex` and its six attached
  models and tag names into `attachModelIndex`/`attachTagIndex` (the tag
  through 0x811d5cc, `G_TagIndex`). VERIFIED (the stores). It also copies the
  names into the client's info at `0x87035bc + client * 0x4b8` (+0x40 body,
  +0x80 attachments, +0x200 tags), what `BG_UpdatePlayerDObj` reads. INFERRED.
- `ClientEndFrame` makes that call first, before its session arms, so
  a client's attachment tags take their configstring in the game frame
  whatever its session, and `SV_SetConfigstring`'s `d` is queued ahead of the
  frame's snapshots. VERIFIED (the call at the function's head). vcod's
  `Cod2Level::end_frame` indexes them there
  (`ScriptRuntime::client_attachments`). Trap: indexing them in the
  snapshot build sends the first snapshot naming a new tag (ctf's flag on
  `J_Spine4`) to a client whose table lacks the string, and the client
  grafts `flag_carry0`'s one root bone, whose name is empty in
  `xmodelparts/flag_carry0` (iw_13), as a new root for that frame.
  VERIFIED (the parts file: 0 children, 1 root, an empty name).
- The capture's client 0 carries `modelindex` 72, `attachModelIndex[0]`
  73 and `[1]` 76, every tag index 0; configstrings 406, 407 and 410 (model
  base 334, cod2-protocol.md 6) hold `xmodel/playerbody_german_africa01`,
  `xmodel/head_german_afrca_christoph` and `xmodel/helmet_german_africa`,
  `mp_german_africa.gsc`'s body, a head off its alias and its helmet.
  VERIFIED (capture). So a client draws a body from the client state and
  the model configstrings alone; it never runs a character script.
- `BG_UpdatePlayerDObj` (0x80dbeb0, Mac 0x182312) builds the DObj from the
  body, then each non-empty attachment with its tag name and its
  ignore-collision bit. VERIFIED (the loop).
- `mp_allies_default.gsc` and `mp_axis_default.gsc` name
  `playerbody_american_ranger` and `playerbody_german_wehrmacht`, which no
  stock pak ships. VERIFIED (file listing). vcod's stand-in for a client
  state naming no body is the first character of the side's Normandy
  mptype (`vcod_cod2::player::Character::default_for_team`); that is
  vcod's, not retail's.

## 2. The skeleton

- A body keeps 1.1's control bones and puts them between its `j_*` joints:
  `playerbody_german_africa01` runs `tag_origin` > `j_mainroot` > `pelvis`
  > `torso_stabilizer` > `j_spine1` > `back_low` > `j_spine2` > `back_mid`
  > `j_spine3` > `back_up` > `j_spine4` > `j_neck` > `neck` > `j_head` >
  `head`, `tag_origin` at the feet. VERIFIED (`xmodelparts`, through
  `vcod_cod2::model`). A control's turn carries every joint above it.
- A head's root is `j_spine4` with its own `j_neck` and `j_head` under it;
  a helmet's root is `j_head`. VERIFIED (`head_german_afrca_christoph`,
  `helmet_german_africa`). An untagged attach grafts by the root's name,
  and the head's `j_neck`/`j_head` share the body's, as 1.1's heads share
  `bip01 neck`/`bip01 head`. VERIFIED (gdb: retail's DObj builds the
  head's three as copies of the body's, section 11). Every stock character
  grafts to one root this way (`tests/player_rigs.rs`).

## 3. The anim array

- `BG_LoadAnim` (Mac 0x186658) finds `root`, `torso`, `legs` and `turning`
  in `multiplayer`, parses `mp/playeranim.script` once to intern its
  names, precaches the tree and sizes the array off
  `XAnimGetAnimTreeSize`; an index no script line named reads `unused`.
  VERIFIED (the calls). That is 1.1's scheme, so vcod builds the array by
  1.1's walk (`animtree::AnimIndex::build`), which gives 301 entries over
  CoD2's tree and script. INFERRED. The capture's running client sends
  `legsAnim` 127, then 665 (153 with the restart toggle 512); the walk
  names them `pb_combatrun_forward_loop` and `pb_stand_alert`. VERIFIED
  (capture against the walk).
- An entry is 0x60 bytes (1.1: 0x5c): name, then at +0x40 the blend, +0x44
  `moveSpeed` as a float (root travel over the clip's length in seconds,
  not 1.1's truncated integer), +0x48 the ms duration floored at 500,
  +0x4c the name hash, +0x50 the flags (0x80 looping). VERIFIED (stores in
  `BG_LoadAnim`).
- The script is 1.1's grammar. Its condition names are, in table order,
  `PLAYERANIMTYPE`, `WEAPONCLASS`, `MOUNTED`, `MOVETYPE`, `UNDERHAND`,
  `CROUCHING`, `FIRING`, `WEAPON_POSITION`, `STRAFING` (0x8184c00); its
  movetypes run `IDLE` at 1 to `CLIMBDOWN` at 19, `TURNRIGHTCR` and
  `TURNLEFTCR` at 16 and 17, then the mantles, flinches and stumbles to 40
  (0x81848c0). VERIFIED (tables). vcod parses the file with 1.1's parser
  and keeps its movetype bits in 1.1's numbering, read only by its own
  updater.
- `xanim` is v14 and parses (cod2-xmodel-v20-format.md, "xanim").

## 4. The entity's flags the body reads

`eFlags` 0x4 crouch, 0x8 prone, 0x40 firing, 0x300 a gunner (0x100 prone,
0x200 crouched, both standing), 0x4000 mantling. INFERRED
(cod2-movement.md 10). 0x20000 dead (set off `pm_type` >= 6) and 0x40000
sighted (off `pm_flags` 0x40) by `BG_PlayerStateToEntityState` (0x80dfcc2).
VERIFIED. 0x80000 is read by `BG_RunLerpFrameRate`; clear, a death anim
jumps to its end. INFERRED (the branch). `angles2[1]` is `movementDir`.
VERIFIED (0x80dfcc2). vcod maps these onto 1.1's bits for the shared code
(`vcod_cod2::player::body_flags`).

## 5. The angle updater

0x80da9c8 (inlined in the Mac `BG_PlayerAnimation`, 0x183ef2) reads 1.1's
immediates: clamps 90, 45 and 150, the legs' tolerance 40, the move's 0.3
share of the torso, the pitch's 0.6, its swing speed 0.15, and
`bg_swingSpeed`. VERIFIED. Its branches are 1.1's with two changes.
INFERRED (control flow):

- A mantle (`eFlags & 0x4000`) swings all three angles and sends the torso
  (clamp 90) and the legs toward the view, not the move.
- The pitch holds level only when `eFlags` equals 0x4000: the test is
  `cmpl $0x4000` (0x80daf19), an equality, so a mantle with any other flag
  set pitches.
- `BG_SwingAngles` (0x80da7dc, Mac 0x182170) sets a swing past its clamp to
  `dest ∓ clamp`, where 1.1 leaves it a degree inside.

The updater's stance tests read the `MOVETYPE` condition (condition 3,
`ci+0x474`) through 0x80d98a4 (0x80daa7a, 0x80dab05): `& 0xc0000` swings
all three angles, `& 6` clear sets the legs and torso swinging.
VERIFIED (the instructions). Two writers fill it: pmove's anim script at
each cmd (`BG_AnimScriptAnimation`, 0x80d958f, the movetype's bit) and
the end frame's conditions after the updater (0x80db296, the legs
record's movetype mask when it differs). VERIFIED (the calls). So a
server's updater reads the movetype the frame's cmds picked, not the one
the last end frame wrote. VERIFIED (gdb, `tools/re/cod2_skel_gdb.py` on
the `antilag-slow` run: at 9700 the strafing target's legs go to
`pb_stand_alert` with `movementDir` still -14, the condition reads 2
going in, and the legs stay at the view's yaw, not swinging). vcod's
updater had read the strafe's 1024 the end frame before wrote and swung
the legs 7 degrees there, the round's 51 bones up to 2.6 units off.
VERIFIED, the gate. vcod's server
writes the condition off the end frame's legs record ahead of the step
(`PlayerBody::script_movetype`); the record's mask stands in for the
movetype's one bit, which the wire does not carry. INFERRED (the two
agree on every test the updater makes while no anim plays under both an
idle and a moving movetype). The cgame runs no pmove for other bodies,
so a drawn body keeps the end frame's order. INFERRED.

## 6. The controllers

`BG_Player_DoControllers` (0x80dbc70, Mac 0x182b82; its target half
0x80db362). The six control tags are `back_low`, `back_mid`, `back_up`,
`neck`, `head`, `pelvis` (`GScr_LoadConsts` 0x81224f8 into the table at
0x8184ca8), and `tag_origin` takes the local tag. VERIFIED (strings and
table).

The constants are 50 and 0.925 for the roll, -2.5, -5 and -12.5 for the
lean's shift, 1.25 for the roll's scale, 0.5 for the prone head, -24, -12
and 16 for the prone twist, 0.075 for the legs' roll, the spine shares
0.2/0.3/0.5, 0.4/0.4/0.2, 0.5/0.5/-0.6 and the prone ones, neck 0.3, head
0.7 and -0.3, and the steps 0.36 and 0.1. VERIFIED (Mac 0x326188..0x3264f4).
How they combine, read off the branches. INFERRED:

- A gunner (`eFlags & 0x300`) gets zero targets.
- Otherwise 1.5's split (`coduo-vehicles.md` 14) with CoD2's lean: the
  shift along y is `-2.5 f` leaning right, `-5 f` left standing, `-12.5 f`
  left crouched; the roll `f * 50 * 0.925` scaled 1.25 except crouched
  leaning right, the head's halved prone; the prone twist and every spine,
  neck and head share as 1.5's.
- Every control steps toward its target by `0.36 * frametime` degrees a
  component, `tag_origin`'s offset by `0.1 * frametime` units
  (`BG_LerpOffset`, Mac 0x1824ee).

## 7. The lerp frames

`BG_RunLerpFrameRate` (0x80d9d00, Mac 0x183492) holds the blends 120, 170,
200, 250 and the 400 ms window, and the rate clamps 0.1, 0.01, 2, 150, 20,
3, 130 and 4 (0x81531e8..0x8153208), 1.1's. VERIFIED. Read off the
branches, INFERRED: a clip's blend is the entry's, else 120 ms into a
moving clip, 250 into a still one out of a moving one, 170 otherwise, 200
into no anim; a legs change between stance classes holds a 400 ms floor
on the blend (`ci+0x4ac`), as 1.5's and UO's stance window does; the legs
node goes to 0.01 and the torso node to 1 under a torso anim.

The stance classes are two record tests on the new and the old legs anim:
crouched is `+0x54 & 0xc4` (`idlecr`, `walkcr`, `walkcrbk`) or `+0x58 &
0x180` (movetypes 39 and 40, `stumble_crouch_forward` and `_backward`;
lnxded 0x80d9c02), prone is `+0x54 & 0x308` (0x80d9c58). VERIFIED, the
decompile. The record's two words are a 64-bit movetype mask, one bit for
each movetype block whose clauses play the anim. INFERRED, the masks
against the movetype order. `pb_stumble_forward` is the default line of
both `stumble_forward` and `stumble_crouch_forward` (`mp/playeranim.script`;
VERIFIED, the paks), so it reads crouched, and a standing idle that
stumbles opens the 400 ms window: the idle fades out over 400 ms, not 120.
VERIFIED, `combat/hit-tags` (cod2-combat.md 9.3, "The knocked body").

The server's end frame sets a new legs clip at node time 0 with its goal
weight 1 and its weight `goal * 0.001` (0x3a83126f), the old one going to
goal 0, both over the blend's seconds; the next `G_RunFrame` steps it
`0.05 * frequency` (0.0151515 for `pb_stand_alert` at 16100 after a set
at 16050), and every frame after. VERIFIED (gdb, `tools/re/cod2_anim_gdb.py`
on a tdm kill run: the infos at `0x85bc900 + 0x28 * index`, +0xc the time,
+0x10 the time before the step, +0x18 the blend's seconds left, +0x1c the
goal, +0x20 the weight, +0x24 the rate). vcod's channel clock
(`animclock::Play`) runs the same rule: time 0 at the end frame that sets
the clip, a step a frame after. INFERRED (the code against the dump).
The step stores its increment as a float before the sum: `pb_stand_alert`'s
node time on a retake of `hit-throw`'s recipe (239 frames, two wraps)
is `float(t + float(0.05 * frequency))` at every frame, the wrap taken
after the store, where a sum with the increment kept on the stack leaves
it at the 17th step (0x3e83e0f9 against 0x3e83e0f8). VERIFIED (gdb,
`cod2_anim_gdb.py`). vcod's CoD2 bodies step so
(`animclock::Flavour::Cod2`); off it the struck body's bones sat a few
ulps off retail's (cod2-combat.md 9.4). A
spawn's own end frame (the spawn calls it, at the level time the begin
ran) carries `legsAnim` 0 and sets no clip; the idle is set at the next
end frame, after the first cmd's pmove picks it (the melee target: begun
and spawned at 5150, `legsAnim` 0, 153 set at node time 0 at 5200, 0.01515
at 5250). VERIFIED (gdb on the `hit-melee` recipe). vcod's server had posed
a body carrying no legs anim in the stance's idle, which set the idle at
the spawn's end frame, and the first cmd's 153 with the toggle then came
onto that clip rather than starting it; it now sets none, and the
`hit-melee` contacts land on retail's digits (cod2-combat.md 9.7).
VERIFIED, the gate.

`cod2_lnxded`'s setter (0x80c20de) differs from 1.1's and 1.5's in the
goal time: a node at weight 0 is raised to `0.001 * goal` first, and the
goal time is then the part of the given time that distance from the
raised weight takes, `(goal - w) * time / goal` going up and `(w - goal)
* time / w` going down, stored at info +0x18; going to 0 it is the whole
time when the old goal (+0x1c) was not 0, else the sooner of the two.
VERIFIED (the instructions). The step (0x80bf112) lands on the goal once
the frame's `dt` reaches the goal time, else stores `(goal - w) * dt / gt
+ w` (one under 1e-6 raised to `0.001 * goal`) and takes `dt` off the
goal time. VERIFIED (the instructions). So a fresh clip in a 120 ms
cross-fade reads 0.001, 0.417667 and 0.834333 before 1, where the whole
time off 0.001 gave 0.41725 and 0.8335. VERIFIED (gdb, the antilag recipe
under `tools/re/cod2_skel_gdb.py`: every leaf weight of the target's
builds from 9000 to 17150 matches vcod's word for word, the stand to run
and run to stand fades included). vcod's channel clock runs this setter
and step on CoD2 bodies (`Ramp::cod2`).

The rate (0x80da42a): the distance from the lerp frame's `pos` (+0x1c) to
the entity's `pos.trBase` (0x80a40ea: the difference stored, its squares
summed on the stack and stored, libm `sqrt` stored), divided on the stack
by `ms * 0.001` (the float at 0x81531e8, the product never stored),
stored, then by the float `moveSpeed` (anim +0x44), stored at +0x28; the
clamps compare the float `moveSpeed` with 150 and 20 and the cap is `3 -
(moveSpeed - 20) / 130` on the stack, stored. VERIFIED (the
instructions). A clock that took the seconds as a stored float (`0.05`
is `0x3d4ccccd`, `50 * 0.001f` on the stack is above it) left the rate a
word off now and then, and the node time from then on: on a fresh
antilag run the run clip's time parted by a word at 10700 after the rate
`0x3ea8907f` came out `0x3ea89080`. VERIFIED (gdb: `tools/re/cod2_gdb_all.py`
runs `cod2_trace_gdb.py`, `cod2_anim_gdb.py` and `cod2_skel_gdb.py` in one
session; the lerp frame's +0x1c, +0x28 and +0x2c at each build). vcod
runs it so on CoD2 bodies (`animclock::rate_cod2`); on three fresh runs
every leaf's node time and weight at every build of the target matches
vcod's word for word (209, 209 and 206 builds). VERIFIED (the dumps
against the replays). The drawn body takes the same rate; the cgame's
(`CoD2MP_s.exe`) is not read. INFERRED.

A frame the server runs with no packet before it measures nothing:
`SV_Frame` runs every frame due back to back and sends one snapshot after
the last, so a run slowed by gdb's breakpoints holds the snapshot's
`pos` at each frame in between, the first of them measuring the whole
move (rate 2, the cap) and the rest none (0.1). VERIFIED (gdb on the
`antilag-slow` run: from 10100 to 11250 its target's lerp frame holds
each next snapshot's snapped origin at the frames with no snapshot,
10150 and 10200 that of 10250). That is the 0.0083 step against vcod's
near 0.1 an earlier dump showed at 12100: the gates had skipped each
frame with no snapshot, so vcod's clock measured the move over the gap.
INFERRED (that dump is gone; the slowed run shows the same steps at each
such frame). The antilag and hit pose gates now run such a frame on the
next snapshot's playerstate (`Side::ran_at`).

## 8. What vcod carries over

- `vcod_cod2::player` runs 1.1's `animtree`, `animclock` (1.5's windowed
  setter), `playerpose` and `skeleton` over CoD2's files.
  `playerpose::BodyAngles::step_by(Updater::Cod2(..))` and
  `Controllers::cod2` hold sections 5 and 6.
- The server poses the same body for its traces, kept per client from
  the connect and animated in `ClientEndFrame`; its controls ease only
  when a trace builds the skeleton, and its clips play at node time
  (cod2-combat.md 9.5). `PlayerBody::animate`, `ease_controls` and
  `dobj_mats` are that split of `advance` and `pose`; `dobj_mats` builds
  the bones in retail's arithmetic (section 11).
- The record's second word is `animscript::AnimRecord::movetypes_hi`
  (movetypes 32 and up by CoD2's names, clear for a CoD1 script), which
  `animclock`'s windowed setter reads for the crouched stumbles; the float
  `moveSpeed` is `animtree::AnimMotion::move_speed_f`, which the rate
  divides by under `animclock::Flavour::Cod2` and
  `Flavour::Cod4`.
- Not modelled: the blend table's exact 1.5 differences in section 7.
- The drawn body's light origin (`BodyFrame::light_origin`) is the one
  `CG_Player` (Mac 0x1e6834) and `CG_Corpse` (0x1e78ea) build, the same
  as 1.1's (VERIFIED, the stores; cod2-materials.md 9.2).
- Open: what `animMovetype` (entity field, 4 bits) carries; the capture's
  running body sent 0.

## 9. The anims pmove picks

What puts `legsAnim`/`torsoAnim` (ps+0x7c, +0x84; timers +0x78, +0x80) on
a playerstate; `BG_PlayerStateToEntityState` (0x80dfcc2) copies both
words to the entity (es+0xcc, +0xd0). VERIFIED (the stores). vcod's port
is `vcod_cod2::pmove::anim`; `tests/anim_ab.rs` replays the movement
captures in `tests/fixtures/moves/` and the `fire` and `hurt` ones beside
them through pmove and `PM_Weapon` and matches every `legsAnim`,
`torsoAnim`, `legsTimer` and `torsoTimer`, its `GAPS` empty. VERIFIED.

- `Mantle_Move` (0x80de260) calls `BG_AnimScriptAnimation(ps, 3, slot +
  20, 1)` every mantle frame, the slot the up anim's until the timer
  passes the up anim's length and the over anim's after (0x80dd1b2):
  the `mantle_up_*` / `mantle_over_*` movetypes, 21..30. At the end, under
  the over flag, `BG_AnimScriptEvent(ps, 3, 0, 1)`: the forced `jump`
  event. VERIFIED (the calls). The mantle arm runs no `PM_DropTimers` and
  no `PM_Footsteps`, so the timers hold and no move anim is picked while
  mantling (cod2-movement.md 9). VERIFIED (the call order). `anim_ab`
  matches every row of `mantle`, `mantle_turn` and `mantle_crouch`.
  VERIFIED.

- The conditions live per client beside the playerstate (`bgs` + 0xb4058
  + clientNum * 0x4b8, eight bytes a condition). `animConditionsTable`
  (0x8184c60) gives each a type and a name table: `PLAYERANIMTYPE` (bit
  set over `mp/playeranimtypes.txt`), `WEAPONCLASS` (bits over
  0x8184b60, `rifle` .. `item`), `MOUNTED` (value, `mg42` 1), `MOVETYPE`
  (bits over 0x81848c0), `UNDERHAND`, `CROUCHING`, `FIRING` (values, no
  names), `WEAPON_POSITION` (value, `hip` 0, `ads` 1), `STRAFING` (value,
  `not` 0, `left` 1, `right` 2). VERIFIED (the tables).
- `Pmove` (Mac 0x70c7e) runs `BG_AnimUpdatePlayerStateConditions` (Mac
  0x182666) before each `PmoveSingle`: conditions 0 and 1 off the
  viewmodel weapon's `WeaponDef` +0x74 and +0x7c
  (`BG_GetViewmodelWeaponIndex`, Mac 0x7b226: `ps.weapon`, the offhand
  under `pm_flags` 0x10), `MOUNTED` off `eFlags & 0x300`, `UNDERHAND`
  off ps+0xe8, `FIRING` off the cmd's 0x1, `WEAPON_POSITION` off `eFlags`
  0x40000. VERIFIED. No store puts 0x40000 on a playerstate's `eFlags`
  (only `BG_PlayerStateToEntityState`, 0x80dfcc2, on the entity, off
  `pm_flags` 0x40) and none of the captures' playerstates carries it, yet
  retail's `weapon_position ads` clauses (`pb_stand_ads`,
  `pt_rifle_fire_ads`) play exactly while `pm_flags` 0x40 is up, from the
  snapshot the sight bit comes up in (`fire` and `hurt` captures,
  VERIFIED); vcod reads the condition off `pm_flags` 0x40. Why the code
  and the capture part is open. Weapon 0 reads `playerAnimType none`, so a body
  whose weapon is not up runs `pb_pistol_run_fast` and idles in
  `pb_stand_bombplant` (both on the retail walker below). VERIFIED
  (capture).
- `PM_Footsteps` (0x80e572c, Mac 0x6ed5c) calls `BG_AnimScriptAnimation`
  (Mac 0x1873d2): a gunner idles in its turret stance; airborne off a
  ladder it returns unless its stance bits lag the eye's target; on a
  ladder, 300 ms past the jump, `climbdown` when falling else `climbup`;
  under `player_moveThreshhold` (or linked) the stance's idle, else
  `idle`; moving, `STRAFING` from the keys (a forward key `not`, a side
  key alone `left`/`right`, neither unchanged), then `walkprone[bk]`,
  `walkcr[bk]`/`runcr[bk]` and `walk[bk]`/`run[bk]` by stance, the back
  flag (`pm_flags` 0x80) and the sighted walk or lean; with no key held
  past 120 (rodata 0x8155104) the move clip stays, under it a crouched or
  prone body takes its idle. INFERRED (control flow).
- `player_turnAnims` registers 0 (`BG_RegisterDvars`, 0x80de830, flags
  0x1180 as every `player_` pmove dvar), so the `turn*` movetypes
  (14..17, off the legs' yaw in `bgs` clientinfo +0x380/+0x3b0 and a
  turn end time at +0x4b0) never play on a stock server; vcod has no turn
  anim. VERIFIED (the registration).
- The hit anims (`PM_Footsteps`): with `stumble` = `damageTimer`
  (ps+0x90) above `damageDuration` (ps+0x94) less
  `player_dmgtimer_stumbleTime` (500, floored at 0), a standing idle
  plays the flinch `flinchYaw` (ps+0x98) picks (0x80e5672: the yaw, plus
  360 when negative, 315..45 `flinch_forward`, 45..135 `flinch_left`,
  135..225 `flinch_backward`, else `flinch_right`; rodata
  0x8155088..0x8155098) and returns; a failed prone idle falls back to
  `flinch_forward` under it; a standing run or walk takes
  `stumble_forward`/`_backward` or `stumble_walk_forward`/`_backward`,
  a crouched one `stumble_crouch_forward`/`_backward` by the back flag
  alone, prone none. INFERRED (control flow). The `hurt` capture
  (`probe_moves.gsc` under `probe_hurt`, 3 points every 0.7 s from a
  turning direction) replays row for row on it: torso 246..249
  `pt_flinch_*` standing still, legs 28/29 `pb_stumble_*` running.
  VERIFIED (capture). `PAIN` (event 0) is never raised: no
  `BG_AnimScriptEvent` call in `cod2_lnxded` passes 0. VERIFIED (the
  call list).
- `BG_AnimScriptAnimation` takes the movetype's first matching clause in
  `COMBAT`, its command `clientNum % count`, and sets `MOVETYPE` before
  running it continuing, with no timer. VERIFIED (the loop).
  `BG_ExecuteCommand` (Mac 0x186baa): a channel with its timer at 50 or
  more keeps its anim unless forced; a continuing call on the anim playing
  does nothing; else the anim goes in with the toggle 0x200 flipped and,
  for an event, the timer at the command's duration plus 50. A `both`
  line puts its anim on the legs and anim 0 on the torso, which is why
  `torsoAnim` reads 0 or 512 in every capture. VERIFIED (the stores).
- `BG_AnimScriptEvent` (Mac 0x18729a; event names 0x8184a28): forced
  `jump`/`jumpbk` (by the cmd's forward sign) in `Jump_Check`
  (0x80dcca8), unforced off a plane that throws the body up and forced off
  a ledge with nothing within 64 (`PM_GroundTrace`, 0x80e38a2, 0x80e36aa),
  forced `jump` when an airborne climber lets go (0x80e805a), forced
  `land` past a 12-unit fall (0x80e30c4), and unforced `prone_to_crouch`,
  `stand_to_crouch`, `crouch_to_stand`, `prone_to_stand` in
  `PM_CheckDuck` (0x80e4644; the stock script has blocks for
  `prone_to_crouch` and `crouch_to_prone` only). VERIFIED (the calls).
- `PM_Weapon`'s events, all `isContinue` 0 (VERIFIED, the calls): forced
  `fireweapon` (2) in the shot's timing helper (0x80ef1ac) for every
  non-grenade shot, before the empty-clip check, and in the offhand
  throw (0x80efb12); forced `reload` (10) in `PM_BeginWeaponReload`
  (0x80edb04) unless clip-only, and again in states 5..9 when pmove's
  `PM_CheckDuck` went into or out of prone that step (`pmove_t` +0xe0,
  0x80ee724); unforced `dropweapon` (6) in `PM_BeginWeaponChange`
  (0x80edc30) unless it swaps to the alt or `pm_flags` 0x4 (mantle) is
  up; unforced `raiseweapon` (7) when a weapon comes up out of another
  (0x80ede6c), after the conditions take the new weapon's, the block
  empty in the stock script; forced `meleeattack` (17) on the melee key
  (0x80ef74e). Event numbers off `animEventTypesStr` (0x8184a20, `PAIN`
  0 .. `SHELLSHOCK` 18). VERIFIED (the table). The `fire` capture (two
  hip shots, reload, melee, a crouched, a prone and a sighted shot, a
  prone reload stood out of, a shot on the run) replays on vcod's port,
  the melee's command among its clause's five off retail's own draw.
  VERIFIED (capture, `anim_ab`).
- `BG_AnimScriptEvent` (0x80d96ee) picks its command `rand() % n` for
  every event, `n` the clause's count (`call rand` at 0x80d977a, `idivl`
  by the clause's +0x70). VERIFIED (the call). The `fire` capture is made
  under `RAND_TRACE=1` (`tools/run_probe_cod2.sh`), so its `# rand` lines
  hold the server's draws: nine at 0x80d977a from the first shot on, one
  for each event with a matching clause, `n` 1 but for the melee's 5.
  VERIFIED (capture). `anim_ab` hands each of vcod's picks the next of
  those draws and requires as many picks as draws; nine and nine, and
  every row matches. VERIFIED (the gate). A pick one command on from the
  draw puts the melee's `torsoAnim` on `pt_melee_right2right_2` (267)
  against retail's `pt_melee_right2right_1` (268), so the row reads the
  draw. VERIFIED (the gate, by hand). This capture's melee draw
  (173704750) gives 0; the one before it (no trace) took another command,
  `torsoAnim` 264. VERIFIED (the old capture). vcod-server draws for
  every clause with a command, off glibc's `rand()` (`GameRand`,
  cod2-combat.md 9.6).
  `PM_DropTimers` (0x80e6936) runs both timers down to 0. VERIFIED.
- `ClientSpawn` (Mac 0x1930a6) ends in `ClientThink_real` on the
  client's last cmd restamped to `level.time`, `commandTime` 100 ms
  before it, then `BG_PlayerStateToEntityState`. VERIFIED (stores and
  calls). That think picks a fresh body's first anims before the script
  hands it a weapon.

The retail walker, `crates/server/tests/fixtures/cod2/walker-anims.txt`
(recipe in its header): its own snapshots carry `legsAnim` 127
(`pb_combatrun_forward_loop`) running, 153 (`pb_stand_alert`) blocked,
136 and 138 before the rifle is up, `torsoAnim` 0 and both timers 0;
another client sees the body's words unchanged. VERIFIED (capture).

## 10. The death and the corpse

On the same capture (the walker runs `suicide()` 6 s after each spawn),
VERIFIED:

- The dying player's own snapshot: `legsAnim` a `death` clause anim for
  `run` (33 `pb_death_run_stumble` or 34 `pb_death_run_onfront`) toggled,
  `torsoAnim` 512 (anim 0, toggled), both timers the duration counting
  down from 949. `player_die`'s `BG_AnimScriptEvent(ps, DEATH, 0, 1)`.
- Other clients stop receiving the body the same frame (`SVF_NOCLIENT`)
  and get the corpse, `eType` 2, its `legsAnim`/`torsoAnim` the
  playerstate's (545 and 512), `eFlags` 0xa0002. Its first frame is
  `pos.trType` 5 (`TR_GRAVITY`) with `groundEntityNum` 1023, then 1
  (`TR_INTERPOLATE`) on the ground; about the death anim's duration later
  (1650 ms) the fresh bit 0x80000 goes (0x20002) and the origin moves a
  few units.
- Stock `dm` respawns a dead player on `useButtonPressed` (cmd 0x28)
  unless `scr_forcerespawn` is set (`dm.gsc`'s `respawn` and
  `waitRespawnButton`), after the killed callback's `wait 2`. The walker
  tapping use came back each time.

vcod-server runs the event on the playerstate with the line
`finishPlayerDamage` drew, and `cloneplayer` copies the anims, keeps the
corpse's CoD2 `eFlags` on the wire and takes the fresh bit out at
`deathAnimDuration`. The corpse falls under `TR_GRAVITY` until it rests and moves by its
death anim's root motion each frame (`game::cod2_corpse::run`,
`vcod_cod2::rootdelta::rel_delta`; cod2-combat.md 6.2).

## 11. The server's DObj pose

How `cod2_lnxded` 1.3 builds a body's bones for its traces and tags.
`vcod_cod2::player::dobj` is its port; `PlayerBody::dobj_mats` runs it for
the server (`game::cod2_combat`). Addresses lnxded. Every function below is
an unoptimized gcc build: each float variable and each float argument is a
store, each expression runs on the x87 stack at the 64-bit precision Linux
leaves it (vcod_common's `F80`). VERIFIED (the disassembly).

- `G_DObjCalcPose` (0x811e3c0, all 128 part bits) and `G_DObjCalcBone`
  (0x811e454, the bone's hierarchy bits) return at once when the skeleton
  exists under the stamp (0x8092002, 0x8091f7a); else `DObjCalcAnim`
  (0x80c1260 through 0x8092134), the entity type's controller off the
  table at 0x8187d1c by the byte at entity +0x166, then `DObjCalcSkel`
  (0x80b9dfa through 0x809215e). VERIFIED (the calls).
- `DObjCalcAnim` walks the tree from node 0 at weight 1 with the clear flag
  (0x80bfc04), then gives every asked bone no leaf reached its model's
  local: a root the identity, any other bone its four `XModel` quat shorts
  times the float 1/32767 (0x38000100) stored (0x80c3b90), no translation,
  the weight word untouched. VERIFIED (the instructions; gdb reads 0 there
  on a fresh skeleton).
- The walk is 1.7's (cod4-player-anims.md 11): a node skips each child of
  weight 0 (the info's +0x20); one live child takes the node's weight and
  flags; with two or more, the first runs with the clear flag, the rest
  without, each at its own weight and nested, into the bones when the node
  clears, else into scratch (0x200 bones at most). Then the fold, by the
  node's flags: not nested, each bone of non-zero weight times `1 /
  weight` passed as a float; nested and clearing, the quat times `rsqrt *
  w` and the shift times `w / weight`, both passed as floats, the weight
  `w`; nested alone, those added out of scratch, `w` added to the weight.
  VERIFIED (the instructions). `rsqrt` (0x80c34fe) is the `0x5f3759df`
  step off the stored `((x^2 + y^2) + z^2) + w^2` (0x80c38e6) with one
  Newton step, 0.5 (0x815042c) and 1.5 (0x8150428), its half and its result
  stored; a zero quat is left. VERIFIED (the instructions).
- A leaf (`numframes` at parts +0): at node time 1 or with no frames every
  part reads its last key (0x80bd20e); under 256 frames 0x80c2ffa, else
  0x80c336e. Each takes the frame `t * numframes` stored and truncated
  (0x80bd0a6) and `k = w * 1/32767` stored. A part keyed on every frame
  takes the truncated frame as its key and `frame - key` stored as the
  fraction; one with a key list searches it off the truncated frame and
  stores `(frame - key) / (next - key)` (0x80bd638, 0x80bd4c4; 0x80bda1c,
  0x80bd876 past 256 frames). VERIFIED (the instructions).
- The sums per bone, each stored once: a full quat's four shorts or a half
  quat's `z` and `w` (parts +0x14 the bit per bone) as `s0 + (s1 - s0) *
  frac` on the stack (0x80c3cb0), times `k`, added (0x80c39fe, 0x80c3986);
  a held key `s * k` added (0x80c3b20, 0x80c3aea); a bone with the half bit
  and no data adds `w` to the quat's `w`. A keyed translation `((v1 - v0) *
  frac + v0) * w` (0x80c3ce2), a held one `v * w`, added (0x80c3be4,
  0x80c37a0); every bone the leaf reaches adds `w` to its weight. VERIFIED
  (the instructions).
- The loaders rebuild a quat's `w` short alike, the xmodel's (0x80c613c)
  and an xanim key's (0x80c3cf4, `z` only 0x80c3dd6): `32767^2` less the
  squares, converted to a float, its root through libm `sqrt` stored as a
  float (0x80c77cc, 0x80c522a), plus 0.5 (0x81505e0, 0x8150440), floored,
  truncated; 0 once the squares reach `32767^2`. VERIFIED (the
  instructions). An exact integer root, as CoD1's loaders take, put the
  `w` of two keyed bones (`j_thumb_ri_3`, `j_eye_lid_top_ri`) off retail's
  in 32 of the dumped calls. VERIFIED (`skel_ab`).
- The controller (`G_PlayerController` 0x80f729a, `BG_Player_DoControllers`
  0x80dbc70) sets `back_low`, `back_mid`, `back_up`, `neck`, `head` and
  `pelvis` (the table at 0x8184ca8) through `DObjSetControlTagAngles`
  (0x80bbea4), then `tag_origin` through `DObjSetLocalTag` (0x80bbf22), a
  bone outside the call's part bits skipped (0x80babee, 0x80bab64). Both
  land in 0x80bbd20: the local's quat off the angles, the translation given
  (`vec3_origin` 0x814cfc8 for a control), weight 0; a control also sets
  its bit at skeleton +0x10. VERIFIED (the calls and the stores). So a
  control bone keeps no anim of its own. VERIFIED (gdb).
- 0x80bbd20's quat: each angle times the double pi / 360 (0x814ff70)
  stored, its libm `sin` and `cos` stored (0x80bc046); `t0 = -ps * ys`,
  `t1 = ps * yc`, `t2 = pc * ys`, `t3 = pc * yc` stored; `x = rs t3 + rc
  t0`, `y = rc t1 + rs t2`, `z = -rs t1 + rc t2`, `w = rc t3 - rs t0`, each
  stored. VERIFIED (the instructions).
- `DObjCalcSkel`: a bone a later model shares by name with an earlier one
  is a copy of the earlier's built bone (`DObjCreateDuplicateParts`); every
  shared name, not only the root's: the head's `j_spine4`, `j_neck` and
  `j_head` copy the body's, the helmet's `j_head`, `j_head_end` and
  `j_helmet` the head's. VERIFIED (gdb: the copies' words equal).
  vcod's skeleton merges only an attachment's root (section 2), so its
  helmet's two other bones sit at their bind under `j_head`; nothing
  traces or tags them. A root of model 0 keeps its local and takes the
  weight (0x80bc34e): `2 / s` with `s` the stored square sum, a zero quat
  becoming `w` 1, weight 2. Any other bone: the quat its local's times its
  parent's, a control bone's its parent's times its local's (0x80bc452,
  0x80bc58a: `x = ((ax bw + aw bx) + az by) - ay bz` and its kin on the
  stack, each stored), then its weight; its translation the local's plus
  the model's bind offset, stored (0x80bc134), moved out through the
  parent (0x80bc39e): the parent's rows (0x80ab4a0: `quat.xyz * weight`
  stored, each product with the quat stored, `1 - (yy + zz)` and the rest
  rounded once), each component `((t.x r0 + t.y r1) + t.z r2) + trans`
  stored. VERIFIED (the instructions).
- gdb on retail under the hit pose probe (`tools/re/cod2_skel_gdb.py`,
  `crates/cod2/tests/fixtures/skel/hitpose-gdb.txt`) dumped 820 builds of
  both bodies: one, two and three leaves, standing, crouched, prone and
  reloading. Rebuilt off each dumped tree's clips, node times and weights,
  every local the walk left matches word for word (616640 words), and the
  build off retail's locals every bone it left (701920 words); an `f64` for
  the stack left one word of the build `1.4e-17` where retail's is 0.
  VERIFIED (`skel_ab`, all 820 calls; the fixture keeps 25 of them).
- Its cost on vcod-server, the 60-bot mp_toujane `dm` match (`bot_match`
  seed 1, 600 ticks, `taskset -c 0-7`, the dev profile), in the ticking
  thread's user instructions (`tick_profile::instructions`) around each
  build: the shared sampler before (b781bfb93) 87.8 M a tick, the pose
  4.8 M of it (5.4%, 172 k a build); the port on `F80`s alone 172.7 M,
  the pose 87.3 M (50.6%, 3.14 M a build); now 93.6 M, the pose 9.8 M
  (10.5%, 351 k a build), 16676 builds on each and the match's `DIGEST`
  the same on the last two. VERIFIED (the runs). What moved it: the
  stack runs as an `f64` with a running bound on how far it and the
  64-bit stack can sit from the exact value (0 where Knuth's two-sum, a
  product of two floats or a fused multiply-add's error shows it exact),
  and a store whose bound straddles a float's rounding reruns the pass on
  `F80`s, so every word is the 64-bit stack's (2 reruns in the 820
  dumped calls); each parent's rows are built once for its children; a
  trace builds only the bones with a hit box and their parents
  (`PlayerRig::traced`), a tag the whole body, the controls eased once a
  stamp either way. INFERRED, the code; the dump, the masked build's
  words (`a_masked_pose_keeps_the_words_of_the_bones_it_builds`) and the
  digest VERIFIED.
- The server's poses off this (`body_pose`, `world_tag`): the sd kill run's
  two drops start where retail's do and match to the bit; on a fresh tdm
  kill capture under gdb, `G_DObjGetWorldTagMatrix`'s place (0x811e542;
  0x80a7436's sums on the stack) of `tag_weapon_right` matches retail's
  word for word once the legs clip's node time is retail's (cod2-protocol.md
  15.7). VERIFIED (gdb, the gate).

## 12. The trace through the boxes

How `cod2_lnxded` 1.3 traces a line through a posed body's hit boxes.
`vcod_cod2::player::dobj::traceline` is its port; `body_trace` in
`game::cod2_combat` runs it for the server. Addresses lnxded; the build is
the unoptimized one of section 11 (each float variable a store, the stack
at 64-bit precision).

- `SV_PointTraceToEntity` (0x809d0b1) builds the body (`G_DObjCalcPose`,
  0x809d11b), takes `AnglesToAxis(r.currentAngles)` (0x80a94aa: forward,
  `vec3_origin - right`, up) and moves the clip's start and end into the
  body's frame through 0x80a74f8: each `p - origin` stored, each dot with a
  row `((d.x r.x + d.y r.y) + d.z r.z)` on the stack, stored. It then calls
  `DObjTraceline` (0x80bb27e) with the priority map and the clip's fraction
  as the limit; a nearer result takes the fraction and turns the normal
  out through 0x80a71b6 (each component's three products summed on the
  stack, stored). An entity with 0x4 in its +0xf2 byte takes 0x80bbb32
  instead.
  VERIFIED (the instructions).
- `DObjTraceline`: `dir = end - start` stored, its squared length stored
  (0 returns), `1 / len2` stored. The walk runs every model's bones in
  order with the global bone index, skips a model whose bit is set in the
  DObj's +0x1a, resolves each bone's location (one whose priority is 1
  takes its original's on a duplicate, else its parent's, a root's the
  bone it attaches to) and skips a bone whose `radiusSquared` (bone info +0x24) is
  0 or whose priority is under the best so far (2 at the start). VERIFIED
  (the instructions).
- The sphere: the box centre (bone info +0x18) placed by the bone's rows
  as 0x80bc39e places a translation (0x80ab3f8); `v = start - c` stored;
  `t = -(v . dir) * (1 / len2)` on the stack, stored; the squared distance
  of the nearest point (`v + t dir` stored when `0 < t < 1`, else `v` or
  `end - c`) stored; `r2 - dist2` stored, a bone with `0 >= gap` skipped.
  At the best priority, `t - sqrtf(gap / len2)` (libm `sqrtf`, the
  difference on the stack) at or past the result's fraction skips it too.
  VERIFIED (the instructions). Both tests only cull boxes the slab test
  would not take nearer. INFERRED. The loader stores the centre `(mins +
  maxs) * 0.5` and `|maxs - centre|^2` (0x80c7322, 0x80c7344, 0x80c736c).
  VERIFIED (the instructions).
- The slab, in the bone's frame (0x80bb1f0: `p - trans` stored, each dot
  with a row of 0x80ab4a0's matrix on the stack, stored): the mins with
  sign -1, then the maxs with +1, each axis `d1 = (ls - plane) * sign` and
  `d2` alike stored. `d1 > 0` and `d2 > 0` skip the bone; `d1 > 0` clears
  the starts-inside flag and, when `d1 > enter * den` (`den = d1 - d2`
  stored), stores `enter = d1 / den`, skips the bone once `enter >= exit`,
  else records the plane's axis and sign; `d2 > 0` alone clears the
  ends-inside flag and moves `exit` down alike. `enter` starts at 0; `exit`
  at the result's fraction when the bone's priority is the best so far,
  else at the call's limit, so a higher priority takes a box behind a
  nearer hit. VERIFIED (the instructions).
- A segment inside a box end to end returns at once with fraction 0 unless
  `dir.xy . start.xy > 0` (on the stack), which skips the bone; its normal
  is `start.xy` normalized (0x80a44c2) or, with no `dir.xy`, z `-1` for a
  `dir.z` at or over 0 and `+1` under it. One that starts inside and leaves
  is skipped. VERIFIED (the instructions).
- Otherwise a bone at the best priority whose `enter` is at or past the
  result's fraction is skipped; one that is not takes the fraction, the
  location and the best priority. The normal is built once at the end:
  the struck bone's row of the recorded axis times the recorded sign,
  stored. The sign (ebp-0x118) is whichever bone's entry last recorded
  one, a later bone the walk skipped included; the axis (ebp-0x11c) is
  copied at the hit. VERIFIED (the instructions).
- `tools/re/cod2_trace_gdb.py` dumped every call on the antilag recipe
  (`crates/cod2/tests/fixtures/trace/antilag-gdb.txt`, 84 of 406 kept):
  the port off each dumped line, map, limit and bones gives retail's
  fraction, hit location and normal word for word, and the caller's
  move of the line into the body word for word. VERIFIED
  (`dobj_trace_ab`, 84 traces, 406 on the whole dump). The port costs
  about 8 us a trace in the dev profile. VERIFIED (a timing run).
