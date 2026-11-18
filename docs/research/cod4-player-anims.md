# CoD4 1.7 player bodies: clips, skeletons, the anim index, heads

How `vcod_cod4::anim` turns a zone's xanims and xmodels into vcod-common's
`XAnim` and `Model`, and how the client poses a watched session's players
with them (`play::cod4`, `play::cod4_players`) through the shared pose
code CoD2 uses (`vcod_cod2::player`, `animtree`, `animclock`,
`playerpose`).

Sources, each claim naming its own:

- The 1.7 client install's zones (`zone/english/common_mp.ff`,
  `mp_crash.ff`) as walked by `vcod_cod4::walk`
  (`cod4-fastfile.md`). The gate `crates/cod4/tests/anim_census.rs` reads
  them (`COD4_DIR`).
- The committed spectator capture `play-1.7-spec.txt`
  (`cod4-netfields.md` 8), replayed by `play::cod4`'s tests.
VERIFIED means read out of a 1.7 zone or capture; INFERRED means read
off control flow. A claim labelled INFERRED that names no 1.7 address is
not yet confirmed on 1.7.

## 1. The xanim streams

- An `XAnimParts` keeps its bones' data in six streams (`dataByte`,
  `dataShort`, `dataInt`, `randomDataShort`, `randomDataByte`,
  `randomDataInt`) plus `indices`, read in step by `XAnimCalcParts`.
  The order: the rotation parts by `boneCount[0..5]` (no quat, half quat,
  full quat, half quat with no size, full quat with no size), each bone
  the next name; then the translation parts by `boneCount[5..9]` (small
  trans, trans, trans with no size, no trans), each opening with a
  `dataByte` naming its bone. INFERRED
  (`XAnimCalcParts`).
- A keyed track opens with a table size `n` off `dataShort`, then `n + 1`
  key frames: bytes off `dataByte` under 256 frames; at or past it shorts
  off `dataShort`, or, from 64 keys up, a coarse table there of
  `((n - 1) >> 8) + 2` shorts and the keys in `indices`. INFERRED (same).
- A half quat is `(0, 0, z, w)`, two shorts; a full quat four; both over
  32767. A no-quat bone's rotation is the identity. INFERRED (same).
- A translation key is `mins + size * v` (component-wise), `mins` and
  `size` six floats off `dataInt`, `v` three bytes off `randomDataByte`
  (small) or three unsigned shorts off `randomDataShort`; a no-size
  bone's three floats are its offset; a no-trans bone's offset is 0. The
  offset adds to the bone's bind translation before its parent turns it,
  and the quat is the whole local rotation (the skeleton build).
  INFERRED (`XAnimWeightedAccumLerpedTrans`).
- Read that way, all 1125 non-stub xanims of `common_mp` decode with every
  key list running from frame 0 to `numframes`, ascending, and every one
  of the five data streams read to its last byte. VERIFIED
  (`common_mp_clips_decode_to_their_last_byte`).
- Key frames run `0..=numframes`; a looping clip's `numframes` counts its
  closing frame, so the clip lasts `numframes / framerate` seconds.
  INFERRED (`XAnimGetLength`); the
  last key of every track equals `numframes`. VERIFIED (same gate).

## 2. The root motion

- The delta part's translation track is a `size` u16 and `smallTrans`
  u8, then a single vec3 at `size` 0, else `mins`, `size` (two vec3s),
  the frames slot and `size + 1` key frames, then `size + 1` frames of
  three bytes (small) or three u16s, each `mins + size * frame`. Its
  rotation track is a `size` u16, then one `(z, w)` short pair, else the
  frames slot, the key frames and `size + 1` pairs. INFERRED
  (`XAnim_CalcPosDeltaDuring`, `XAnim_CalcRotDeltaDuring`); the
  byte counts are the walk's (`cod4-fastfile.md`). vcod keeps it as the
  clip's root track; `pb_combatrun_forward_loop` travels over 100 units a
  second by it. VERIFIED (same gate).

## 3. The player anim index

- `legsAnim` and `torsoAnim` index the `multiplayer` animtree, flattened
  as CoD1's (`animtree::AnimIndex::build` over the names
  `mp/playeranim.script` references), with 512 the restart toggle:
  `BG_FinalizePlayerAnims` sizes the anim table off the tree. INFERRED.
  Both files are rawfiles of
  `common_mp` (`animtrees/multiplayer.atr` 17627 bytes,
  `mp/playeranim.script` 32688). VERIFIED.
- Under that flattening the walker of the spectator capture plays 185
  `pb_stand_alert` standing, 152 `pb_combatrun_forward_loop` running, 155
  `pb_standjump_takeoff` then 154 `pb_standjump_land` around its
  `EV_JUMP`, 178 `pb_crouch_alert` while `eFlags` has 4, and torso 316
  `pt_stand_shoot` with `eFlags` 0x40 at each shot. VERIFIED (the
  capture; `the_player_tree_resolves_its_clips`). The tree's 393 leaves
  that a zone holds a clip for come from `mp_crash` and `common_mp`.
  VERIFIED.
- `eFlags` keep CoD2's bits for the body (4 crouch, 8 prone, 0x40 firing,
  0x300 turret, 0x20000 dead, 0x40000 ADS, 0x80000 fresh corpse) but the
  mantle, 0x8000. INFERRED (`BG_PlayerAnimation`'s tests).
- `ClientSpawn` runs `ClientEndFrame` (the call at 0x80a7e66) before
  `ClientThink_real` (0x80a7e75), after its memset of the client
  (0x80a7cc9), so the spawn's own end frame reads `legsAnim` 0; the think
  picks the idle and the level's end frame at the same time carries it:
  the spawn snapshot reads `legsAnim` 0x2b9 (185, `pb_stand_alert`, with
  the toggle). VERIFIED (the calls; a live capture on mp_crash `war`, the
  spawn at 37850). That the end frame on 0 sets no clip is INFERRED: the
  update runs CoD2's (`BG_PlayerAnimation` 0x804c636, from 0x80a6f52),
  whose 0 sets none (cod2-player-anims.md 7), and no tag read can tell
  the two apart, the same level time's end frame setting 185 before any
  script runs. vcod's server poses a body carrying 0 in no clip, where
  it had posed the stance's idle; no gate moved. VERIFIED (the gates).
  The same capture shows the head tag posed at the spawn's next frame
  (37900, 56.6443 over the origin, then 56.5362, 56.428, 56.3196): a
  script reads the clips the last end frames set, at their frame's node
  time. VERIFIED (a probe gsc logging `getTagOrigin("j_head")` each frame).
  vcod-server had posed the bodies the scripts read ahead of the end
  frames, so a new clip reached them a frame late: the bind pose (63.521)
  at the spawn's next frame with or without the stance's idle, then the
  same phase. It now poses them after the end frames, and its heads read
  retail's 56.6443, 56.5362, 56.428, 56.3196 from the spawn's next frame
  (the horizontal offset follows each spawn's own yaw). VERIFIED (the
  probe on both servers).
- A new goal weight reads a step on the frame after the end frame that
  set it, its clip's node time still 0: the gdb trees read
  `pb_standjump_takeoff` at node time 0 and weight 0.5005 (from 0.001,
  one 50 ms step into its 100 ms blend), `pb_crouch_alert` at 0 and 0.12587,
  `pt_flinch_forward` at 0 and 0.14371 (`fixtures/skel/player-anim-gdb.txt`).
  VERIFIED (gdb). That `G_RunFrame`'s step over the DObjs (0x80920d0)
  moves the weights and leaves a node set since the last step at time 0
  is INFERRED. vcod's end frame poses the bodies the next frame's threads
  read with every weight `level.frametime` ahead of the node times
  (`PlayerBody::set_weight_lead`); a cmd's trace before that frame reads
  them unstepped. Before it, `cod4_tag_read_ab` read the walk's, the
  sight's and the reload's blends a frame late: 107 rows; after, 35, then
  17 with the controls eased (cod4-combat.md 6.1). Moving the lead onto
  the traces too put 879 rows into `cod4_corpse_drag_ab`. VERIFIED (the
  gates).
- `lerp.u.player.movementDir` is a signed byte of degrees, clamped to
  +-90. INFERRED (`PM_SetMovementDir`,
  `BG_PlayerStateToEntityState`).

## 4. Skeletons and skinning

- An xmodel's non-root bone `k` has parent `index - parentList[k]`, a
  local quat of four shorts over 32767 and a local translation of three
  floats at `12 k` (the zone's array holds `16 * nonroot` bytes; the last
  quarter is not read). Composed from the roots, every bone of 295 of
  `mp_crash`'s 297 xmodels lands on its zone `baseMat` (quat, then
  translation) within 0.01 unit; the two `viewhands_*` differ. VERIFIED
  (`mp_crash_models_compose_their_base_matrices`). A root stands at its
  `baseMat`. VERIFIED (a body's `tag_origin` is the identity).
- `viewmodel_base_viewhands` (`common_mp`) stores every non-root bone's
  local translation as zero and its `baseMat` whole (`tag_weapon` at
  (10.53, 0, -8.13)); its vertices sit at the `baseMat` pose. VERIFIED
  (the zone). vcod skins every model to its `baseMat` and keeps the
  composed locals for the anims, whose view clips key each hands bone's
  translation (cod4-client-view.md 4).
- A surface's vertices sit in model space at the bind pose. VERIFIED
  (`cod4-netfields.md` 12). A deformed surface's blend data gives each
  vertex its bones as byte offsets into 0x40-byte skeleton matrices, the
  vertices with one bone first, then two, three and four; each extra
  bone carries a weight over 65536 and the first takes the rest. A rigid
  surface's vertex runs each ride one bone. INFERRED
  (`R_SkinXSurfaceWeight0..3`, `R_SkinXSurfaceRigid`); every
  offset of every `mp_crash` model is a multiple of 64 and below its bone
  count, and the runs and blend data cover the vertices exactly.
  VERIFIED (the same gate; `common_mp`'s 597 models skin the same in a
  one-off run).

## 5. Heads, tags and the weapon

- A client's body is `CS_MODELS` (830) plus `modelindex`, each of its six
  attachments `CS_MODELS + attachModelIndex[i]` on tag `CS_TAGS` (2282)
  plus `attachTagIndex[i]`. INFERRED. In the spectator capture each
  player's `attachModelIndex[0]` names its head
  (`head_mp_usmc_tactical_mich_stripes_nomex` beside
  `body_mp_usmc_assault`) with tag index 0, whose configstring is empty.
  VERIFIED. An empty tag grafts the head by its root bone's namesake, as
  CoD2's untagged attachments. INFERRED (`DObjModel_s` with bone name 0).
- The held weapon's `worldModel` hangs off `cg_weaponrightbone`
  (`tag_weapon_right`), attached ahead of the client's attachments.
  INFERRED (`CG_AttachWeapon`).
- A gunner holds no weapon of his own: `BG_UpdatePlayerDObj` (iw3mp.exe
  0x4073a0) loads the entity's `weapon` (+0xc4) and zeroes it when
  `lerp.eFlags` (+0x8) has 0x300 (`test` at 0x4073c3, `xor edi,edi` at
  0x4073d3), before the weapon attach and the cached-weapon compare
  (0x4073f9). VERIFIED (the instructions). The server keeps sending the
  held weapon in `weapon`; vcod drops it in `body_view`
  (`cod4_live_a_gunner_holds_no_weapon_of_his_own`). Without that a
  gunner on mp_backlot's SAW is drawn with his own rifle beside the gun.
- `body_mp_usmc_assault`'s `tag_weapon_right` (parent `j_wrist_ri`),
  `tag_weapon_left` (parent `j_wrist_le`) and `tag_weapon_chest` (parent
  `j_spine4`) share one bind spot, (10.5, -28.5, 38.9) in model space,
  11.4 units off the right wrist. VERIFIED (the model's `baseMat`). The
  player clips key the tag: in the spectator capture's 5029 posed bodies
  the right wrist sits on average (-14.1, -1.0, -4.8) off the gun's
  `tag_weapon` in the gun's frame, behind it along the barrel, under the
  receiver. VERIFIED (`spectator_capture_poses_its_bodies`). A body
  whose `legsAnim` and `torsoAnim` are 0 keeps the tag at the bind spot,
  the weapon hanging off the hand: no clip moves it. INFERRED (vcod's
  pose with no clip; the live playtest's float was vcod-server sending
  none).
- An attachment's tag takes its configstring when the server builds the
  client info: `G_UpdateClientInfo` calls `G_TagIndex` on each
  attachment's tag inside the frame, so the `d` for a new tag
  (`tag_inhand` for the bomb, `tag_stowed_back` for a stowed RPG) goes
  out ahead of the first snapshot that names it. INFERRED.
- vcod-server flushes the roster's tag writes before the messages.
  Built after the frame's configstring flush, the client saw
  `attachTagIndex` one snapshot before its string and grafted
  `prop_suitcase_bomb` and `weapon_rpg*_stow` by their root
  (`tag_weapon`) for that frame (the map sweep's 15 pairs). VERIFIED
  (`cod4_live_a_new_attachment_tag_arrives_with_its_snapshot`).
- 1.7's `ClientEndFrame` (`cod4_lnxded` 0x80a68b8) runs that update
  inlined (the six-slot loop at 0x80a69db, the tag lookup at 0x80a6a4d)
  ahead of its session checks (0x80a6b0d, 0x80a6b20), so a spectating or
  intermission client's tags take their index too, where CoD1 indexes
  only a playing or dead one. INFERRED (disassembly control flow). vcod's
  `client_attachments` indexes for every CoD4 client
  (`cod4::gametype_tests::a_spectators_new_attachment_tag_takes_its_index`;
  without it the sweep showed a stowed-RPG graft on mp_shipment `dom`).
  The scripts attach the bomb on `tag_inhand` and the stowed weapons on
  `tag_stowed_back`, and the model's root bone rides that tag. INFERRED
  (the stock `.gsc`, the CoD2 graft rule above).

## 6. Drawing a watched body

- An entity lerps between its two snapshots when its older state is
  `TR_INTERPOLATE` (the newer not `TR_PHYSICS`) or a client's
  `TR_LINEAR_STOP`; any other runs `BG_EvaluateTrajectory` on the older
  state at the drawn time. CoD4's types match CoD2's up to `TR_GRAVITY`
  (5), then `TR_ACCELERATE` 6, `TR_DECELERATE` 7, `TR_PHYSICS` 8 and the
  ragdoll types 9-11. INFERRED (`CG_CalcEntityLerpPositions`). The
  capture's players send pos `trType` 3 and
  apos 1. VERIFIED.
- The replay gate (`spectator_capture_poses_its_bodies`) poses both
  players of the spectator capture on each of its 2701 frames: every
  body drawn is posed, every anim the bodies name has a clip, the
  walker's skeleton moves in most frames, and its `neck` stands at 53.7
  units standing and 35.0 crouched. VERIFIED (the gate). That the poses
  look right is pending a human's look.
- The server poses the same bodies for its shots (`game::cod4::combat`):
  each xmodel's `XBoneInfo` (0x28 a bone at `XModel`+164) gives a bone its
  hit box and `partClassification` its hit location
  (`vcod_cod4::anim::bones`; cod4-combat.md 3), under section 8's updater
  and controllers, eased once in a frame that poses it (cod4-combat.md
  3.3).
- Left out: the clip's float node time and the `duration` floor of 500 ms
  (`BG_FinalizePlayerAnims`); `hideWeapon` and the knife model; turret gunners' placement.
  Death ragdolls are section 10.

## 7. The anim script in pmove

The server's step (`pmove::player_step_animated`, `pmove::anim`) picks
`legsAnim` and `torsoAnim` as 1.7's pmove does, over `mp/playeranim.script`
(a `common_mp` rawfile, 32688 bytes, VERIFIED) and the player tree's index
(section 3).

- The movetypes are CoD2's with `sprint` at 20, 43 in all. INFERRED
  (`animMoveTypesStr`). The script's `combat` state names every
  one it uses. VERIFIED (the rawfile).
- The conditions `BG_AnimUpdatePlayerStateConditions` sets before each
  step: `PLAYERANIMTYPE` and `WEAPONCLASS` off the viewmodel weapon (the
  offhand while it is in hand; `WeaponDef` +296 and +304), `MOUNTED` on a
  turret's stance bits, `FIRING` on the attack key, `PERK` on the last
  stand; `MOVETYPE` and `STRAFING` are set by the calls and kept between
  cmds (vcod keeps them per client beside the playerstate). INFERRED.
  `WEAPON_POSITION` reads `PMF_SIGHT` (0x10), as
  CoD2's gate found for its `eFlags` 0x40000. INFERRED. The
  `playerAnimType` names are `mp/playeranimtypes.txt` in order (none,
  other, pistol, smg, autorifle, ...), VERIFIED; a script value list
  `all NOT m203` reads every name but those. INFERRED.
- `PM_Footsteps`' calls: a gunner's stance idle; standing still
  (`player_moveThreshhold` 10) the stance's idle or, inside
  `player_dmgtimer_flinchTime` of a hit, the flinch by `flinchYawAnim`;
  moving with a key held the strafe condition (a key within
  `player_strafeAnimCosAngle` 0.5 of the side) and the table at
  `cod4_lnxded` 0x820e0e0's clip by stance, back, walk and stumble (`player_dmgtimer_stumbleTime`
  500), a standing sprint `sprint`; no key held at 120 or under the
  prone or crouched idle; on a ladder the climb. Each continues and sets
  no timer, its command by `clientNum`. INFERRED; the dvar
  defaults VERIFIED (`cod4_dvars.py`).
- The events: the jump and its backward key (`Jump_Start`, and a walk
  off a ledge with nothing 64 under), the land over 12 units, the fire
  (`PM_Weapon_StartFiring` but a grenade's pin, and the offhand's throw),
  the reload (not a clip-only weapon), the drop and raise of a change,
  and the melee's `meleeattack` or `knife_melee`; each forced. INFERRED.
  Each event, and the death (`player_die`), plays command
  `rand() % n` of its clause, one draw whenever a clause with commands
  matched (`BG_AnimScriptEvent`, 0x8050280; cod4-combat.md 5, 10.9).
  VERIFIED (the disassembly, a traced run). Not run: the stance changes'
  events and the turn anims (`player_turnAnims` 0, VERIFIED).
- A mantle plays its clip's movetype each frame, forced, and the jump
  event at the end of one over a wall (cod4-pmove.md 17.5). VERIFIED
  (the disassembly, the mantle capture).
- A `both` line puts its anim on the legs and 0 on the torso, flipping
  each channel's toggle (0x200); a channel's timer at 50 or more keeps
  its anim unless forced, and a continuing call leaves the same anim
  playing. INFERRED (`BG_PlayAnim`). The walk capture's body
  matches on every snapshot (cod4-combat.md 11), where the walker's torso
  goes 0 to 0x200 at 51950 and back at 52450 on two `both` events.
  VERIFIED live. The kill capture's dying target keeps `torsoAnim` 0 on
  its death anim while its `torsoTimer` takes 0x7e0; the rule there is not
  settled. VERIFIED live (the values).

## 8. Aim and lean on a watched body

- `BG_PlayerStateToEntityState` is inlined at `cod4_lnxded` 0x8052940,
  and every caller passes `snap` 1 (the eight calls, 0x80a6214 ..
  0x80b863b). It copies the view angles (ps+0x108) into `apos.trBase`
  (es+0x3c) and, with `snap`, truncates them and the origin with
  `cvttss2si` (0x80529fc..0x8052a4e); it copies `leanf` (ps+0x5c) into
  `lerp.u.player.leanf` (es+0x54) at 0x8052aa7. VERIFIED. The kill
  capture's aiming shooter reads pitch 1.99951 and yaw -68.99414, the
  `-100` field's rounding of 2 and -69 (cod4-pmove.md 8), the score
  capture's 4.99878 (5).
  VERIFIED live.
- `fTorsoPitch` and `fWaistPitch` (es+0xdc, es+0xe0) are 0 unless
  `PM_GetEffectiveStance` (0x8055ffe: `viewHeightTarget` 11 is 1, 22
  and 40 are 2) is prone; then each is the playerstate's (ps+0x5cc,
  ps+0x5d0) times 1/360, less `floor(x + 0.5)` stored as a float, times
  360, times the eye lerp's fraction (`(commandTime -
  viewHeightLerpTime) / PM_GetViewHeightLerpTime`, 0x80560a2, clamped
  to 0..1, `1 - f` on a lerp up; 1 with no lerp) (0x8052cc1..0x8052d5f).
  VERIFIED. No capture lies prone or leans yet. vcod-server's
  `player_entity` sends all three (with the yaw alone, others see
  neither the aim's pitch nor the lean).
- The walk replay's body yaw matches retail on all 789 rows, and the idle
  teammate's on all 789 (`cod4::wire_ab`, which compares both). VERIFIED
  (the gate).
- The 31 turning rows where retail's yaw reads a short past the truncated
  integer (-122.99194 where `(int)` gives -123's -122.99744) are the
  writer's half-short rounding (cod4-pmove.md 8): -123 goes out as
  `(int)(-22391.47 + 0.5)`, -22390. The teammate's -134.99451 is -135 the
  same way. VERIFIED (the gate).
- 1.7's `SetClientViewAngle` holds `delta_angles` as the float view less
  the cmd's angles, wrapped to -180..180, on the x87 stack (0x80a77f6,
  cod4-pmove.md 15). VERIFIED. Trap: CoD2's rule
  (`delta_angles` in shorts) puts the spawn view off the spot's yaw, so
  the view reads -134.99449 and goes out -134.
- The client's controllers are `BG_Player_DoControllersInternal`
  (`cod4_lnxded` 0x804cf90; it skips a gunner, `eFlags & 0x300` at
  0x804cfa2, and the torso pitch on a climb, movetype bits 0xc0000 at
  0x804cff8). VERIFIED. Its split of the torso and head angles over
  `back_low`, `back_mid`, `back_up`, `neck` and `head`, the prone twist
  of `tag_origin` and the pelvis's waist bend are CoD2's (its rodata
  holds the same 0.925, 50, 0.075, 0.2, 0.3, 0.4, 0.5, -0.6, 0.7, -0.3,
  -1.2, -24, -12 and 16 at 0x8209670..0x82096a8, VERIFIED); the lean's
  `tag_origin` shift and the spine and head roll scale come from the
  `player_lean_shift_*` and `player_lean_rotate_*` dvars by side and
  stance, prone halving the head's roll only. INFERRED
  (`BG_Player_DoControllersInternal`). 1.7's defaults ride the
  dvar mirror: shift 5 left, 9 right, 4 and 15 crouched; rotate 0.8,
  0.4, 0.8 and 0.3. VERIFIED live (`handshake-1.7.txt`). The controls
  ease at 0.36 degrees a ms (`BG_LerpAngles`). INFERRED.
- 1.7's body xmodels carry `pelvis`, `back_low`, `back_mid`, `neck` and
  `head` but no `back_up` (`body_mp_usmc_assault`'s 51 bones), so the
  half of the torso pitch `back_up` takes goes nowhere, on retail as on
  vcod (`DObjGetBoneIndex` failing leaves the tag unset,
  INFERRED). VERIFIED (the zone).
- 1.7's `BG_PlayerAngles` (`cod4_lnxded` 0x804c636) swings the torso
  pitch toward twice the view's: the view pitch less 360 past 180 (the
  180.0 at 0x8209588), then `fadd %st(0),%st` (0x804cefe, 0x804cf76),
  into `BG_SwingAngles` with clamp 45 and speed 0.15 (0x804c7ad).
  VERIFIED. It holds it level for a dead body (`eFlags` 0x20000), a
  gunner (0x300), a climb (movetype 0xc0000) or a mantle as the only
  flag (`eFlags == 0x8000`, 0x804ced7). VERIFIED. At twice the view the
  spine's `back_low` and `back_mid` take the whole view pitch and the
  neck and head turn back by it. Trap: CoD2's 0.6 factor here, with no
  `back_up`, shows only 0.3 of the aim on the spine (looking straight up
  reads as a slight upward aim).
- 1.7's `BG_PlayerAngles` aims the torso at the view plus `movementDir`
  times `player_move_factor_on_torso` (dvar at 0x827afec, 0x804cbd8..
  0x804cbe9), registered with default 0 in 0..1 (0x80544a3..0x80544c7);
  the legs start their swing past `bg_legYawTolerance` (0x827afa4,
  0x804cc49), default 20 in 0..180 (0x80540dd..0x8054109); both swing at
  `bg_swingSpeed` 0.2 (0x827afa0). VERIFIED (the instructions, the dvar
  names at 0x820af0f and 0x820ad8c). `iw3mp.exe` registers the same two
  with 0 (0x40a6d2) and 20 (0x40a05b, the float at 0x6df498). VERIFIED
  (the instructions). A prone body's legs take the view's yaw, the move
  left out (0x804c72b..0x804c738). VERIFIED (the instructions). CoD2's
  0.3 and 40 had swung a strafing body's torso 27 degrees off the view
  and its head 2.8 units off retail's (`cod4_tag_read_ab`). VERIFIED (the
  gate).
- `ClientSpawn`'s own `ClientEndFrame` (0x80a7e66) runs
  `BG_PlayerAngles` at `legsAnim` 0, and the level's end frame runs it
  again at the same level time, each a `level.frametime` step: a gdb run
  read the shooter's legs and torso at 45 then 67.5 degrees off a fresh
  client's 0 toward the view's 90 in its spawn frame (breaks at 0x804c636
  and 0x80a6f57). VERIFIED (gdb). vcod's spawn swings the body once more
  (`Cod4Level::client_spawn`); without it the spawned head read up to
  0.4 units off retail's for four frames. VERIFIED (`cod4_tag_read_ab`).
- 1.7's `BG_SwingAngles` (0x804b3ba) and `AngleNormalize360` (0x8190540)
  fold in floats (`x - floor(x)` of the turns, times 360), not CoD1's
  shorts, and the controllers' `AnglesSubtract` (0x819397a) the same way,
  on the 64-bit x87 stack between stores. VERIFIED (the instructions).
  Ours matches all 374 distinct swings and all 255 controller runs of a
  gdb dump word for word (cod4-combat.md 15.1). VERIFIED (the tests).
  Trap: the shorts leave the torso pitch up to a short off twice the view
  and the head that much off level, which the throw back's bounce off a
  head box reads (cod4-combat.md 15.1).
- `iw3mp.exe` folds in floats as well, each step stored, and its
  `AngleNormalize360` (0x561610) never returns 360.0; it eases the
  controls by a float step (0x407288). VERIFIED (the instructions). The
  watched bodies take the client's arithmetic and the server's hit poses
  the server's (`playerpose::Cod4Arith`, cod4-combat.md 15.1). That the
  client's swing matches `iw3mp.exe` word for word is INFERRED (no
  client dump).
- 1.7 turns a player's six control bones in `tag_origin`'s frame,
  `r * c * r^-1 * p`, the anim's own turn of each dropped
  (`DObjCalcSkel`'s second arm, cod4-gsc.md 3.2, "The skeleton").
  VERIFIED (gdb). CoD1 turned them in the model's frame
  (`apply_controllers`), which reads the same while the legs face the view
  and moves the head by the legs' yaw off it: 5 units on a body strafing 45
  degrees and aiming 30 down. VERIFIED (the test below).
- `iw3mp.exe`'s cgame controller (0x432b50) eases the controls through
  `BG_Player_DoControllers` (0x407250: the targets off 0x406a60, then the
  six spine and head angles at `clientInfo_t` +0x3f8 and `tag_origin`'s at
  +0x440 through `BG_LerpAngles` 0x4070a0, its translation at +0x44c
  through 0x407150) and keeps the six bones' indices (names at 0x729c10)
  and a pointer to the angles on the centity (+0x44, +0x40). VERIFIED (the
  instructions). 0x446280 sets them on the skeleton as `cod4_lnxded`'s
  0x81b01e8 does: each bone's bits at skeleton +0x24 and +0x14, its local
  `AnglesToQuat` (0x580a40) with no translation and weight 0; then bone 0
  off `tag_origin`'s angles and translation, the +0x14 bit alone.
  VERIFIED (the instructions). `BG_LerpAngles` steps each component by at
  most the float step, `d` the stored difference: `step < d` adds it, `d <
  -step` takes it off, else the target (0x4070af..0x4070d5); ours is
  `Controllers::ease_toward`. VERIFIED (the instructions).
- `iw3mp.exe`'s `DObjCalcSkel` composes the roots at 0x57f550, an attached
  model's roots at 0x57f660 and every other bone at 0x57fbd0, which adds
  the model's `trans` to the local's (0x57ff70..0x57ff8e) and tests the
  control bit at 0x57fcb0. Its control arm (0x57fd4e..0x57ff15) is
  `r^-1 * p`, then `l * m`, then `r * t`, each component summed in
  `cod4_lnxded`'s order; the weight `2 / |q|^2` and the translation follow
  the first arm's sums, as on the server. VERIFIED (the instructions).
  `AnglesToQuat` takes its half angles off the float `M_PI / 360`
  (0x70b298) through `fsincos` and stores its four products. VERIFIED (the
  instructions). Its x87 runs at double precision (11) and it stores
  other values than the server: the first arm stores each component of
  `parent * local` once; the control arm keeps every partial sum of
  `r^-1 * p`, `l * m` and `r * t` and stores each component once; the
  weight is `2 / s` with `s = ((x^2 + y^2) + z^2) + w^2` off the stored
  quat, `s` stored (0x57ff18..0x57ff63; a root's at 0x57f5e0..0x57f607);
  the translation adds the model's `trans` with all three sums stored
  (0x57ff70..0x57ff8e), builds the parent's matrix with `t * px`, `t *
  py`, `t * pz`, the nine products and the nine entries stored
  (0x57ff91..0x5800bc), and sums each row as the local's `y` term, its
  `x`, its `z`, then the parent's translation, storing once
  (0x5800c3..0x58014c). `AnglesToQuat` takes `fsincos` of the stored half
  angle and stores each sine and cosine and the four products, the sums
  kept (0x580a40..0x580b5e). VERIFIED (the instructions). The drawn
  bodies compose so (`vcod_cod4::skel::player_mats` with
  `Cod4Arith::Client`); the
  server's hit poses take the same composition with `Cod4Arith::Server`
  over the locals 1.7's own sampler leaves (`player_mats_sampled`, 11).
  The drawn bodies' locals come off 1.7's sampler in the client's
  arithmetic (11).
  Off the gdb dump's own bone 0, parents and angles, the client's
  arithmetic parts from retail's server words on 141 words of the 804
  control bones, by at most 6e-8 (`skel_ab`; with every result a float,
  as vcod had it, 149 by 9e-8); a strafing body drawn by the client sits
  within 2e-5 units of the server's arithmetic on every bone
  (`a_drawn_bodys_control_bones_turn_as_the_servers`). VERIFIED (the
  tests). That the client's words match `iw3mp.exe`'s is INFERRED (no
  client dump).
- The cgame lerps a player's `leanf` and `movementDir` between the two
  snapshots with its angles, each difference wrapped through `floor`
  (`CG_InterpolateEntityPosition`). INFERRED. vcod lerps them too; the
  newest snapshot's alone makes the controls' targets jump at 20 Hz, a
  choppy lean with sliding feet. The shift moves the whole body, feet
  included, on retail too (`DObjSetLocalTag` on bone 0, INFERRED).
- `a_watched_body_aims_and_leans` poses the spectator capture's walker
  with 45 degrees down and up and half a lean either way: the head drops
  9 and moves 9 forward aiming down, moves 13 back aiming up, and moves
  13 to the side it leans to. VERIFIED (the test). That the bend looks
  right is pending a human's look.

## 9. A corpse's models and its death anim

- `CG_ResetEntity` runs for an entity new to the snapshot or whose
  `eFlags` 0x2 toggled; for an `ET_PLAYER_CORPSE` it fills the client's
  corpse slot (`corpseinfo[number - 64]`). With the fresh bit 0x80000 set
  it copies the client's live `clientInfo_t` (`CG_CopyCorpseInfo`, which
  drops any attachment on `J_Spine4`) and clones the client's anim tree
  onto the slot's, so the death anim plays on from the live body's pose;
  without it the slot copies only when it is empty or names another
  client, and keeps its own tree. Nothing else writes the slot, so a
  corpse keeps the models it was copied with. INFERRED
  (`CG_ResetEntity`, `CG_CopyCorpseInfo`).
- `play::cod4_players` copies a corpse's models at its reset and keeps
  them, and takes the live body over only for a fresh one; the server
  clears the fresh bit at `deathAnimDuration` (cod4-combat.md 5).
  `cod4_live_corpses_keep_their_models_and_go_stale` holds both over a
  45 s bots match and fails with either half reverted. VERIFIED (the
  test).
- Trap: a corpse that reads its client's live models every frame resets
  on each change. The stock class scripts attach and detach the stowed
  C4, RPG and rifle (`weapon_c4_mp` on `tag_stowed_hip_rear`,
  `weapon_rpg7_stow` and `weapon_m16gr_sp` on `tag_stowed_back`) as the
  client respawns and switches weapons, and with the fresh bit left set
  each change stands the corpse up and replays its fall. A bots `war` on
  mp_crash showed it on most corpses. VERIFIED (live, a spectator beside
  eight bots). That no corpse stands up again is pending a human's look.

## 10. The ragdoll

- The corpse turns ragdoll when its entity's `pos.trType` reads 9, 10 or
  11, which the server's `startRagdoll` writes (cod4-combat.md 5.1);
  `CG_UpdateRagdollPose` and `Ragdoll_CreateRagdollForDObj` take it from
  there on the client. INFERRED.
- The bodies come from `ragdoll.cfg` in `iw_00.iwd` (7925 bytes,
  2007-05-02), which iw3mp.exe runs as `exec ragdoll.cfg` at start up:
  `ragdoll_clear`, `ragdoll_bone`, `ragdoll_selfpair`, `ragdoll_joint`,
  `ragdoll_limit` and `ragdoll_baselerp_bone` (`ragdoll_pin_bone` exists,
  unused), each refusing a bad line with its own string (`Ragdoll:
  Unknown bone geom type %s` and kin). VERIFIED (the iwd, the strings).
  Def 0 has 10 capsules from `j_mainroot`, def 1 12 (`j_mainroot`,
  `j_spinelower`, `j_spineupper`, `j_neck` to `j_head`, both arms'
  shoulder and elbow, both legs' hip and knee), 11 joints (the spine's
  two and the elbows and knees hinges, neck, shoulders and hips
  swivels), 33 self collision pairs and 9 base lerp bones (head 2000 ms;
  wrists, ankles and the four twist bones 1000). VERIFIED (the file;
  `ragdoll::tests::the_stock_cfg_builds_the_mp_def`). vcod refuses a
  def's lines past 32 bones, 32 joints, 64 self pairs and 32 base lerp
  bones, its own caps: no `iw3mp.exe` string names a count refusal.
  VERIFIED (the strings). The corpse binds
  def 1 whatever it is asked. INFERRED
  (`Ragdoll_CreateRagdollForDObj`).
- The dvars, registered at iw3mp.exe 0x5ba330: `ragdoll_enable` bool 1
  (archived), `ragdoll_debug` 0, `ragdoll_fps` 20 (0..100),
  `ragdoll_max_life` 4500 ms, `ragdoll_max_simulating` 16 (0..32,
  archived), `ragdoll_explode_force` 18000 (0..60000),
  `ragdoll_explode_upbias` 0.8 (0..2), `ragdoll_bullet_force` 500,
  `ragdoll_bullet_upbias` 0.5, `ragdoll_baselerp_time` 1000 and
  `ragdoll_jointlerp_time` 3000 ms (100..6000), `ragdoll_rotvel_scale` 1,
  `ragdoll_jitter_scale` 1 (0..10), `ragdoll_self_collision_scale` 1.2
  (0.1..10). `phys_gravity` is -800 (0x5a3120), `phys_autoDisableLinear`
  20 (0x5a386f), `phys_autoDisableAngular` 1 (0x5a391c),
  `phys_autoDisableTime` 0.9 (0x5a39cd). VERIFIED (the registrations'
  immediates and float constants).
- `Ragdoll_BoneTrace` (0x5be2e0) traces a point from one end to the other
  under 0x2806c91 and, when that starts solid or hits, back; the tunnel
  test fails a bone blocked both ways. VERIFIED (the bytes: the three
  `push 0x2806c91` and the `vec3_origin` boxes). The tunnel test runs it
  along each bone and from the middle of a bone with several children
  to each child's root; a failure leaves the corpse on its death anim.
  INFERRED (`Ragdoll_TunnelTest`). A body runs until every
  rigid body sleeps or `ragdoll_max_life` passes, then holds its pose
  (`Ragdoll_CheckIdle`); no new one starts past `ragdoll_max_simulating`;
  the joints' friction falls from full to a tenth over
  `ragdoll_jointlerp_time` (`Ragdoll_UpdateFriction`). INFERRED.
- vcod's `vcod_cod4::ragdoll` keeps the def, the dvars, gravity, the
  mask, the tunnel test and the sleep rule, and stands a position-based
  solver in for ODE: each bone's two joints are points with half its
  mass and its radius, the lengths and the attachments (a child rooted
  off its parent's end held to both of the parent's points, mirrored
  siblings to each other) are links, a joint's y and z limits clamp the
  child's swing in the parent bone's bind frame (a hinge holds its other
  axis still; an axis whose start pose reads outside the cfg's range and
  inside the mirrored one is flipped, as ODE's axes need not match), the
  self pairs keep their capsules apart, each point sweeps the clip map as
  a sphere with Coulomb friction at its bone's friction (and static
  friction at the end of a step, 10.2), and a bone that would close over
  an edge is lifted off it. A bone with mirrored children takes its twist
  from the line between their roots; any other bone keeps the twist it
  started with against its parent (10.2). Every skeleton bone follows
  its def bone's frame from the pose it started in; the base lerp bones
  ease to their bind. The step is 1.7's (17 ms, at most two a frame,
  cod4-client-perf.md 36); the iteration counts and the damping are
  vcod's. A blast pushes it (10.1).
- `play::cod4_players` starts it on the first frame a corpse reads a
  ragdoll type with `ragdoll_enable` on, from the death anim's pose and
  the last frame's pose as its velocities. The Options page's Ragdoll row
  sets `ragdoll_enable` (cod4-front-end.md 9).
- Live: `cod4_live_corpses_ragdoll_onto_the_floor` (vcod-server `war`
  on mp_crash, a spectator posing through `Cod4Players`, 40 s on the
  live tests' shared clock): eight bots held still are each put on a
  spot of open, level floor near a `mp_tdm_spawn` and killed there by
  `kill`, 3 s apart, so where a corpse falls is the test's and not a
  fight's. All eight take the type and come to rest with every point
  clear of the world and every bone that started clear still clear;
  five to seven go to sleep on the floor, the rest freeze at max life,
  which `Ragdoll_CheckIdle` does wherever the body is (with eight
  shooting bots, about one run in 25 froze one mid-fall off a ledge, 9
  units over the floor, so the rest check skips those). VERIFIED (the
  test, ours). In the retail kill capture the corpse's hand is in the wall it
  fell against when its type changes at 49550, and the tunnel test
  refuses it (`the_kill_captures_corpse_fails_the_tunnel_test`); whether
  retail's refused it too is not measured, and no capture can measure
  it: the server sends only the corpse's trajectory types (pos 11, apos
  9 at 49550, cod4-combat.md 5.1) and the ragdoll and its tunnel test
  run in the client alone (`Ragdoll_BoneTrace` is iw3mp.exe's), so the
  answer needs retail's client on screen. INFERRED (the wire carries no
  ragdoll state). That a ragdoll looks right is
  pending a human's look.
- Left out: `physicsJolt` on the server, the killcam's
  own ragdoll (`killcamRagdollHandle`), a twist that moves inside its
  limit (vcod's bones keep their start twist, 10.2), entity collision
  (`phys_collUseEntities` read as 0, INFERRED).

### 10.1 Blasts on a ragdoll

- `Ragdoll_ExplosionEvent` is iw3mp.exe 0x5bdc10 (it reads the
  `ragdoll_explode_upbias` dvar at 0x5bdf7f and `ragdoll_explode_force`
  at 0x5bdf99, the globals the registrations at 0x5ba55e and 0x5ba505
  store). `CG_EntityEvent` calls it six times, 0x436b3e..0x437359.
  VERIFIED (the calls and the dvar reads). The arguments: a grenade's
  explode passes inner 0 and outer its weapon's `explosionRadius`
  (`fild [ebx+0x554]`, `WeaponDef` +1364) at scale 1 (0x436b3e); a
  rocket's and a custom explode, with or without marks, pass the radius
  as both (0x436c75, 0x436e24); the three script events pass the entity's
  `eventParm` as the outer and `lerp.u` word 0 as the inner, the sphere
  (0x437241) not level, the cylinder (0x4372ce) level, the jolt
  (0x437359) level with words 1..3 as its impulse. VERIFIED (the pushes
  before each call). The sphere's and the cylinder's scale is word 1
  (the magnitude), the jolt's 1. INFERRED.
  A flashbang's explode calls none. INFERRED.
- Inside, per running or sleeping body whose root bone is within the
  outer radius (level for a cylinder): it wakes and its clock restarts
  (`stateMsec` 0), and each bone whose centre of mass, moved by
  `flrand(-1, 1)` on each axis, is inside the outer radius takes
  `ragdoll_explode_force * scale / bone count`, the scale cut by
  `(outer^2 - d^2) / (outer^2 - inner^2)` past the inner radius, along the
  impulse or away from the blast (level for a cylinder) with
  `ragdoll_explode_upbias` added to its z and normalized, as a force at
  the centre (`Phys_ObjAddForce`, which scales it by `1000 / msecStep`).
  The three `flrand` draws are the LCG `* 0x343fd + 0x269ec3` at
  0x5bddf2..0x5bde7e, VERIFIED (the instructions); the rest INFERRED.
- `ragdoll_bullet_force` (global 0xcc789d4) and `ragdoll_bullet_upbias`
  (0xcc789f4) are read by nothing in iw3mp.exe but their registrations
  (0x5ba5c5, 0x5ba612): a bullet does not push a ragdoll on 1.7.
  VERIFIED (every reference to the two globals).
- vcod: `cod4_players::explosion_of` builds the push off a fired event
  (the weapon's `explosionRadius`, `cod4_audio::WeaponAv`, or the
  script words) and `Cod4Players::explode` hands it to every running or
  sleeping ragdoll (`Ragdoll::explode`): each body's push, divided by its
  `ragdoll_bone` mass, is one step's change of both its points'
  velocity, and its jitter draws run the same LCG. The force's size in
  the position solver is vcod's. vcod-server sends
  `physicsExplosionSphere` and `physicsExplosionCylinder` (event 57 and
  58, the outer radius as the parm, inner and magnitude in `lerp.u`
  words 0 and 1). Gates: `ragdoll::tests::a_blast_wakes_a_sleeping_body_and_throws_it_away`,
  `cod4::hardpoint_tests::physics_launch_and_the_explosion_sphere_go_out`
  and the live `cod4_live_blasts_push_the_ragdolls` (eight bots' war on
  mp_crash for 60 s: 4 explodes turned into 4 pushes that reached 10
  ragdolls in its run). VERIFIED (ours). How a thrown body looks is
  pending a hand check.

### 10.2 The spin at rest and the head's twist

- `ragdoll.cfg`'s def 1 limits each swivel on all three axes: limit 2
  (the neck, joint `1 3 swivel`) x -30..30 ("Head turn"), y -30..30, z
  -10..30; the shoulders x -104..100 and -100..104 ("Shoulder twist"),
  the hips x -45..45 ("Hip twist"). VERIFIED (the file). A swivel is a
  ball joint with an angular motor over the limit axes, each with its
  stops and its friction as the motor's force at speed 0; a hinge is a
  hinge with one stop pair. INFERRED
  (`Ragdoll_CreatePhysJoint`, `Ragdoll_UpdateFriction`).
- The ragdoll world's bodies lose a hundredth of their angular velocity
  each step and none of their linear: `Phys_DoBodyOncePerFrame`
  (iw3mp.exe 0x5a62f0, called from `Phys_RunFrame` at 0x5a6720) takes
  the world index at 0x5a6305 and, for world 2, adds `avel * -0.01`
  (the double at 0x70b488) to the angular velocity (0x5a63d0..0x5a6432);
  the other worlds scale both by `1 - phys_dragLinear * dt` and `1 -
  phys_dragAngular * dt` (dvars at 0xcc54414 and 0xcc54464,
  0x5a6310..0x5a63cf). VERIFIED (disassembly). That world 2 is the
  ragdolls' is INFERRED (`PHYS_WORLD_RAGDOLL`).
- ODE puts a body to sleep once its linear speed stays under
  `phys_autoDisableLinear` and its angular speed under
  `phys_autoDisableAngular` for `phys_autoDisableTime`: `Phys_RunFrame`
  stores the squares of the first two and the time into the world each
  frame (iw3mp.exe 0x5a6505..0x5a6533, `[esi+0x3c]`, `[esi+0x40]`,
  `[esi+0x44]`). VERIFIED (disassembly). The test is per body, on the
  squared speeds, over ten idle steps as well (0x5b9b70, cod4-gsc.md
  3.6.1). VERIFIED (disassembly).
- Two traps, measured live on the earlier solver
  (`cod4_live_ragdolls_hold_still_once_down`: six held bots on mp_crash
  killed on open floor, a spectator posing them through `Cod4Players`,
  the client at 4 ms and at 100 ms frames). VERIFIED (the test, ours):
  - A bone without mirrored children (the lower spine, the neck, the
    arms and legs) carried from step to step by the shortest turn of its
    axis picks up the solid angle its axis sweeps. A neck swinging while
    the body fell twisted 22 to 180 degrees against the upper spine at
    4 ms, 0 to 41 at 100 ms, where 1.7 stops it at 30. On the ground a
    resting bone's small swings kept turning it, up to 17 degrees with
    every point within a unit of where it lay. The head is drawn under
    the neck bone, so it spun.
  - With friction only in the collision pass, the constraint passes
    after it slide the points along the floor again: with the twist
    fixed, two of five bodies at 4 ms moved 7 units and turned 13
    degrees in the 900 ms before they slept.
- The rule now: a bone under a parent takes the parent's frame turned by the
  shortest turn from its axis at the start (in the parent's frame) to
  its axis now, times its start frame in the parent's, so its twist
  against the parent stays the death anim's whatever path its axis
  takes, inside 1.7's twist limits (`a_bone_swung_round_a_cone_comes_back_untwisted`).
  A point that met a surface in a step and slid along it less than its
  bone's friction times how far the surface pushed it out goes back
  along the surface at the end of the step (static friction). vcod's.
  After: neck twist 0 at 4 ms and at most 13 degrees at 100 ms (that
  number measured from the first step, whose 50 ms swing the measure
  itself turns); every corpse that slept moved under a unit and turned
  its pelvis and spine under 4 degrees in its last 900 ms (a run's
  measure, not a bound: see below); seven of
  eight slept in `cod4_live_corpses_ragdoll_onto_the_floor` and none
  froze at max life. VERIFIED (the tests, ours).
- Not taken: ODE's angular sleep test. This solver's resting points
  move a few hundredths of a unit a step, about a degree on the pelvis
  (whose four points, the root's two and the hips, lie in one plane),
  which at 17 ms steps reads over 1 rad/s: with the test, four of five
  corpses at 4 ms ran to `ragdoll_max_life`. The sleep test stays every
  point under `phys_autoDisableLinear`. Nor 1.7's 1% angular damping a
  step: vcod's damping takes 1.6 to 7% of each point's speed every
  sixtieth of a second, linear and angular together, already more.
  VERIFIED (the runs, ours).
- The linear rule does not bound how far a body moves in the 900 ms
  before it sleeps: the timer starts the step every point drops under 20
  units a second, and a body decaying from there covers more than a
  unit. With the drawn bodies on 1.7's sampler (11), `mp_crash`'s corpse
  67 at 4 ms frames fell the same way to its floor contact at 357 ms,
  then took a different bounce off a start pose 1e-5 units off the old
  one: under 20 units a second from 630 ms, 18 at 663, 14 at 714 to 816,
  asleep at 1547, 1.70 units from where its window began. The old
  sampler's run had slowed to 13 by 561 ms and moved 0.72. VERIFIED (the
  runs, ours). Measured per step from the start of the step, the
  resting bodies' turns average 0.2 to 0.5 rad/s and peak at 1.35: an
  angular test at `phys_autoDisableAngular`'s 1 rad/s keeps every corpse
  awake to `ragdoll_max_life`; averaged over six steps it lets corpse 67
  sleep at 1751 ms having moved 0.60, while corpse 64 at 100 ms frames
  then moves 1.35. VERIFIED (the runs, ours). Neither is taken.
- So `cod4_live_ragdolls_hold_still_once_down` no longer bounds that
  slide at a unit: the bound held only by how a bounce fell. It holds the
  corpses to what the rule promises, at 4 ms and 100 ms frames: asleep by
  `ragdoll_max_life` (at 4 ms every one before it; at 100 ms, with 50 ms
  steps, two of six freeze at it on either sampler), every step's
  fastest point under 20 units a second over the 900 ms before a body
  sleeps (`Ragdoll::step_speeds`), no point moving once asleep, and the
  neck under 30 degrees. VERIFIED (the test, ours).
- The skins a frame reuses between steps (cod4-client-perf.md 38, item
  4) are keyed on the ragdoll's version, which every step bumps, and on
  the corpse's transform: they are the step's pose, never one between.
  1.7 draws a pose lerped between its last two steps
  (`timeNowLerpFrac`, INFERRED); vcod draws the last step's.
  The spin is in the steps, at 100 ms frames as at 4 ms, so not the
  reuse. VERIFIED (the runs, ours).
- By eye (pending): on mp_crash or any map, kill a player standing in
  the open (`kill` with `sv_cheats 1`, or a bot) and watch the corpse
  fall and lie: the head should stay facing the way the neck points
  through the fall, never turning about itself; once down nothing should
  turn or slide, and no limb roll about its length. Check at a high frame
  rate (`com_maxfps 0`) and at a low one (`com_maxfps 20`).

## 11. The anim sampler

- `DObjCalcAnim` (`cod4_lnxded` 0x81b4c8c) masks out the bones not asked
  for and those already built or set by a controller (DObj +0x14), walks
  the tree from its root at weight 1 with the clear flag set (0x81b4f6f),
  then gives every asked bone no leaf reached its model's own local: a
  root the identity, a non-root its `XModelQuat` shorts times the float
  1/32767 (0x823ba10) stored, no shift, weight 0 (0x81b4d6c..0x81b4ecc).
  VERIFIED (the instructions). The player clips key none of the six
  control bones and not the head's `j_neck` or `j_head`. VERIFIED (gdb,
  the `remap` lines of `fixtures/skel/player-anim-gdb.txt`).
- The walk (0x81b2578): a node skips each child of weight 0 (+0x2c). With
  one live child it hands that child its own weight and flags (0x81b46bc).
  With two or more, the first child runs with the clear flag, the rest
  without, each at its own weight and with `p5` set, summing into the
  bones themselves when the node clears, else into scratch the first
  child clears (0x81b2957..0x81b2a07). VERIFIED (the instructions). A
  child node flagged 0x10 (+0x14) opens an additive tail, which no player
  tree reached in the dump. VERIFIED (gdb).
- The fold, by the node's flags: with `p5` clear (the root's), each bone
  of non-zero weight times `1 / weight` kept, each word stored, the
  weight left (0x81b4608..0x81b4678); with `p5` and the clear flag, the
  quat times `w * r` and the shift times `w / weight`, the weight then
  `w` (0x81b4a32..0x81b4abe); with `p5` alone, the scratch quat times
  `w * r` and its shift times `w / weight` added into the bones, `w` added
  to the weight (0x81b4b78..0x81b4c0d). `r` is the `0x5f3759df` step off
  the stored bits of `((x^2 + y^2) + z^2) + w^2`, one Newton step with 0.5
  and 1.5 (0x8209628, 0x820962c) on the kept sum; a zero quat is left.
  VERIFIED (the instructions).
- A leaf (`numframes` at parts +0xe): at node time 1 or with no frames
  every part reads its last key. Under 256 frames (0x81b1534) the frame
  `t * numframes` stays on the stack and the key search truncates its
  stored float (0x81b1557..0x81b1578); at or past 256 the frame is stored
  and 0x81b02b6 returns the key and the stored fraction past it, through
  a coarse table first from 64 keys up (0x81b38d7). The fraction under
  256 frames is kept. VERIFIED (the instructions).
- The sums per part: a no-quat bone adds the weight to `w`; a held quat
  adds `q * w`, each short times 1/32767 stored; a keyed one `(q0 * a +
  acc) + q1 * b` with `b = frac * w` and `a = w - b` kept, `q0` the key
  at or below the frame; a keyed shift `((v0 + (v1 - v0) * frac) * size +
  mins) * w + acc`, a held one `o * w + acc`; every translation part adds
  `w` to the weight. Each sum is stored once; nothing normalizes a leaf's
  quat. VERIFIED (the instructions). Rebuilt from the zone's clips and
  models and each tree's node times and weights, all 17392 words of the
  280 player trees in `fixtures/skel/player-anim-gdb.txt` match, the x87
  stack as an `f64` (`skel_ab`, `vcod_cod4::xanimcalc`). VERIFIED (the
  test).
- vcod-server's tree for a body is `root` over `main` over the legs and
  torso nodes at their weights, each over its clips at their node times
  and weights (`playerpose::player_leaves`), the shape every dumped tree
  has. VERIFIED (gdb). Its hit poses take these locals
  (`combat::Pose4`); CoD1's sampler had normalized each key and the
  blend, which left the thrown back frag's bounce off a head box ulps off
  retail's (cod4-combat.md 15.1). VERIFIED (the gate).
- `iw3mp.exe` runs the same sampler: `DObjCalcAnim` at 0x5875f0, the
  walk at 0x5872c0 (a leaf at 0x586b20; the clip sums under 256 frames at
  0x587c70, past it at 0x588e30, at node time 1 at 0x585500; the key
  search at 0x587ab0 and 0x587b90), the root's fold at 0x586c10, the
  clearing child's at 0x586ea0, a later child's at 0x586fe0. The sums
  under 256 frames, a keyed shift's (0x585450) and the three folds run in
  `cod4_lnxded`'s order; a quat short scales by the double at 0x70b2a0,
  which holds the float 1/32767's value. VERIFIED (the instructions).
  The sums past 256 frames follow the same order. INFERRED (only their
  frame and key search read). `DObjCalcAnim` has one call site
  (0x5801f6). VERIFIED (the call sites). That the view weapon's DObj is
  posed through it too is INFERRED.
- The client stores values the server keeps on the stack: the frame
  `t * numframes` under 256 frames (0x587cac), the key fraction
  (0x587b8a), the lerp's `b = frac * w` and `a = w - b` (0x587f07,
  0x587f17), a keyed shift's lerp and its `size * v + mins` (0x58546b,
  0x5854a2), the root fold's `1 / weight` (0x586c61), and in both nested
  folds the sum of squares, its half, the step, `w * r` and `w / weight`
  (0x586f07..0x586f98, 0x587049..0x5870e7). VERIFIED (the instructions).
- `iw3mp.exe` creates its device with behaviour flags 0x46 (0x5f47ab,
  through 0x5f44d0 to `CreateDevice` at 0x5f4528), which hold
  `D3DCREATE_FPU_PRESERVE`, and its CRT sets `_PC_53` at start up
  (`_controlfp_s(0, 0x10000, 0x30000)`, 0x682894, called at 0x67a6b3): its
  x87 runs at double precision, not single. VERIFIED (the instructions).
  Its other `fldcw`s load a word off the stack (295) or 0x70bf28 (5, in
  0x68b2bb..0x68bcd7); that none of them leaves the x87 at single
  precision is INFERRED (the ones read save and restore around a
  truncation).
- The drawn bodies sample so (`xanimcalc::player_locals` in the client's
  arithmetic: a double between stores, the client's stores), each clip
  off its seconds rather than the server's 50 ms node time; a gunner's
  legs, whose turret rows the tree does not carry, keep the shared
  sampler. Through the same function, the server's arithmetic matches every
  word of the 280 dumped trees but a head's copies of body bones,
  which vcod's skeleton merges into the body's; the client's arithmetic
  parts from them on 2120 words by at most 1.9e-6, a shift whose `mins`
  and `size` nearly cancel most (`skel_ab`). VERIFIED (the tests). The drawn body
  takes the sampler's locals word for word
  (`a_drawn_bodys_control_bones_turn_as_the_servers`). VERIFIED (the
  test). That the client's words match `iw3mp.exe`'s is INFERRED (no
  client dump).
- The view weapon samples through the same function over its own tree
  (12).

## 12. The view weapon's tree

- `CG_CreateWeaponViewModelXAnim` (`iw3mp.exe` 0x4541d0) makes 33 anims
  (0x581190, the count 0x21 at 0x4541d9): node 0 `root` over the 32 nodes
  1..=32 (`XAnimBlend` 0x5810c0 with first child 1, 32 children, flags
  0), node `n` the weapon's view anim at `WeaponDef` +0x50 + 4n, an empty
  name idle's (0x45420e..0x45424f). VERIFIED (the instructions). Nodes 31
  and 32 are ADS up and ADS down (+0xcc, +0xd0, read at 0x4542a1 and
  0x4542d7). VERIFIED.
- `CG_RegisterWeapon` (0x4543ce..0x454498) sets the root and idle to
  weight 1 at once, idle at rate 1, and, for a weapon naming ADS down, ADS
  down to weight 1 at rate 0 held at time 1 (0x584e90). VERIFIED (the
  instructions).
- Each frame 0x456f60 runs `WeaponRunXModelAnims` (0x453c70) for the view
  weapon, then the tree's step (0x583bf0) by the frame's ms times the
  float 0.001 (0x70af98), then the notetracks (0x454ae0). From `PM_DEAD`
  on it instead sends every weapon's tree to idle, or empty idle (0x454100),
  stepping none. VERIFIED (the instructions).
- `WeaponRunXModelAnims` sets the sight first: an `aimDownSight` weapon
  calls `PlayADSAnim` (0x453b30) for up while `pm_flags` 0x10 and not
  `weapFlags` 2, unless a reload's `weaponTime` is still past
  `adsReloadTransTime`, else for down, at `fWeaponPosFrac` as it stands;
  a weapon with no sight that names ADS down calls it for down at 0
  (0x453d11..0x453d24). Then, when `weapAnim` or the weapon (0x79e6b0)
  changed, its switch (the table at 0x454080) starts the node it names with
  no blend; a 0 or a 1 tests nodes 1..30 (0x583b20): one not finished sets
  the last anim to -1 and, for a 0, returns; a 1 goes on with a 0.5 s
  blend (0x6be648). With none left it starts idle, or empty idle with the
  weapon's clip at 0 (`ps` +0x35c by the clip index at `WeaponDef`
  +0x330). VERIFIED (the instructions).
- A node's start (0x453a40) sends it toward weight 1 over the blend at its
  rate, restarted, and every other node of 1..30 toward 0 over the blend at
  rate 1. The rate (0x4539d0): the clip's `numframes / framerate` stored,
  times the double 1000 truncated (0x68b360's `cvttsd2si`) to ms, over the
  node's `g_animRateOffsets` time; 0 for a time of 0, 1 for a node with
  none; a perk divides it (cod4-client-view.md 4.7). VERIFIED (the
  instructions).
- `PlayADSAnim` sends the wanted node toward 1 and the other toward 0, both
  over 0.1 s (0x6b72dc) at rate 0, then sets up's time to the fraction and
  down's to `1 - fraction` (0x453bc2..0x453c5b). VERIFIED (the
  instructions).
- The info's state (`XAnimInfo` +0x18..+0x34): time, the time before the
  step, the loops before and after it, the seconds left to the goal, the
  goal weight, the weight, the rate, an instant flag. VERIFIED (the loads
  and stores below).
- `XAnimSetGoalWeight` (0x584fd0): a goal under the float 0.001
  (0x70b278) is 0; a node with no info and a goal of 0 stays without one,
  else gets a zeroed info. With the restart flag (0x5827d0) a node at time
  0 and no loops, or one that looped in the last step, stays; with no blend
  or no weight it goes back to time 0 (0x582690); else the old info fades
  to 0 over the blend (0x584470) beside a new one (0x582700). Then
  0x584a80: with no blend the weight is the goal at once; else a weight of
  0 starts at the goal times the float 0.001, the seconds left are the blend
  when the old goal lay on the same side of the new one as the weight (the
  product at 0x584b32..0x584b44) or the shorter of the two (0x409400), and,
  for a goal not 0, at least the blend times the share the weight has to
  go, `(goal - w) / goal` rising, `(w - goal) / w` falling (0x584b8e..0x584bc5,
  the longer through 0x4093c0); the goal and rate are stored. VERIFIED (the
  instructions).
- The step: each weight moves `(goal - w) * dt / left` while more than `dt`
  is left, the seconds left less `dt`, a weight under 1e-6 (0x70b230)
  jumping to the goal times 0.001; else it is the goal (0x582899..0x5828ec).
  A leaf of weight 0, or whose clip has no frequency, takes the root's clock
  at 0; each copies its time and loops before the step (0x5829a7, 0x5829f6).
  A leaf at weight 0 with a goal of 0 is freed (0x581910). Each other leaf's
  time adds `rate * frequency * dt`, stored (0x5822c0): a looping clip
  wraps, counting its loops; one that does not holds at the float
  0.99999988 (0x70b610) for a step, then at 1 (0x582328..0x582366). A
  node is finished (0x583b20) with no info, a time that wrapped, a time of
  1 or a loop in the last step. VERIFIED (the instructions).
- vcod steps the tree so (`vcod_cod4::xanimtree::FlatTree`), one per
  weapon kept while another is out, and samples it through
  `xanimcalc::calc` and `skel::player_mats_sampled` in the client's
  arithmetic (`cod4_viewmodel`). VERIFIED (the tests:
  `the_m16s_tree_blends_its_main_clip_and_its_sight`, the `xanimtree` unit
  tests). INFERRED: the infos under the root in node order (the walk's sums
  follow it), and a blended restart's new info taking the old one's place
  with the old after it (0x582700 unread). A bone no leaf reaches takes the
  bind local of vcod's model, not its `XModelQuat` shorts times 1/32767 as
  `DObjCalcAnim` gives it; that the drawn words match `iw3mp.exe`'s is
  INFERRED (no client dump).
- The M16's ADS up and down clips key `tag_ads` alone, which none of its
  main clips keys, so the root's divide leaves each bone its own clip's:
  ADS down's end leaves the rig at rest, ADS up's carries it about 5
  units. A clip that does not loop reads finished on the switch after the
  step that brought it to 1, two steps past its end. VERIFIED (the zone,
  the tests).
