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
  (`DObjCalcSkel`'s second arm, cod4-gsc.md 3.2.5).
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

### 8.1 The movetype the swing reads

- `BG_PlayerAngles` (0x804c636) reads the `MOVETYPE` condition at
  `ci+0x470` ahead of its swings for a body off a turret (`eFlags & 0x300`
  clear, 0x804c68d, into 0x804cb4a): `& 0xc0000` swings all three, `& 6`
  clear (a body not idle) sets the legs and torso swinging
  (0x804cb4d..0x804cb6b). VERIFIED (the instructions). Further on, the
  same function writes the legs record's 64-bit movetype mask (+0x54,
  +0x58) there when it differs (0x804ca1f..0x804ca65). VERIFIED (the
  instructions). That write comes after the swings. INFERRED (the
  layout).
- pmove's anim script writes the condition at each cmd, ahead of the
  frame's end, as on CoD2 (cod2-player-anims.md 5): in a gdb run of the
  tag read recipe the target's `ci+0x470` read 2 at the 40300 controller
  run and 0x800 at the 40350 one, a tag read before that frame's end
  frame, and the 40350 end frame's three swings entered with their flags
  set, so the legs' swing to the strafe's heading began there. VERIFIED
  (gdb). vcod's end frame had read the last end frame's write and swung
  the legs a frame later, the head 0.05 units off retail's on the frame
  the strafe started and the one the back-off started. VERIFIED (gdb,
  `cod4_tag_read_ab`). The server writes the condition off the end
  frame's legs record ahead of the step (`PlayerBody::script_movetype`,
  as CoD2's), which stands in for the cmd's bit. INFERRED (the
  stand-in's limits are CoD2's).

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
  `ragdoll::tests::the_stock_cfg_builds_the_mp_def`). The corpse binds
  def 1 whatever it is asked. INFERRED
  (`Ragdoll_CreateRagdollForDObj`).
- 1.7's caps on a def: 14 bones (`cmp eax,0xe` at 0x5bab01 in
  `ragdoll_bone`, 0x5baab0), 28 joints (0x5badfe in `ragdoll_joint`,
  0x5badb0), 3 limit axes a joint (0x5baf2f in `ragdoll_limit`,
  0x5bae90), 33 self pairs (0x5bb092 in `ragdoll_selfpair`, 0x5bb040)
  and 9 base lerp bones (0x5bb182 in `ragdoll_baselerp_bone`, 0x5bb130,
  a count `ragdoll_pin_bone` shares at 0x5bb2b2). Past one the handler
  prints its string through 0x4fcbc0 and returns before the count goes
  up, so the line is dropped: `Ragdoll: Too many ragdoll bones, max %d`
  (0x6f8924), `Ragdoll: Too many ragdoll joints, max %d` (0x6f89f0),
  `Ragdoll: Too many limit axes for joint %d, max %d` (0x6f8ab4),
  `Ragdoll: Too many ragdoll self collision pairs, max %d` (0x6f8ae8),
  `Ragdoll: Too many base pose lerping bones, max %d` (0x6f8b4c). A
  `ragdoll_limit` line on a def that holds 28 joints prints the joints'
  string too (0x5baed5). VERIFIED (the compares, the strings). A line
  short of its words, the command's own counted (11 for a bone, 4 for a
  joint or a self pair, 7 for a limit, 3 for a base lerp bone), or naming
  a def over 1 is dropped with no print (0x5baac3, 0x5baaeb, 0x5badc0, 0x5bb050, 0x5baea1,
  0x5bb140). VERIFIED (the instructions). Def 1's 33 self pairs and 9
  base lerp bones fill those two caps. VERIFIED (the file). vcod takes 1.7's
  caps and drops the same lines (`ragdoll::MAX_BONES` and kin,
  `ragdoll::tests::a_def_past_its_caps_stops_growing`). VERIFIED (ours).
- `ragdoll_joint` reads the def, the bone (not 0: `Ragdoll: Joint
  referenced bone with no parent (0)`) and the type, one of `none`,
  `hinge` and `swivel` (the table at 0x71e708, matched in 0x5bad20), and
  no word after them (0x5badb0..0x5bae7c), though the file's comment
  gives the format as `<def> <bone index> <type> <axis> <low limit> <hi
  limit>`. `ragdoll_limit` reads the def, the joint, the axis (`x`, `y`
  or `z`, a leading `-` flipping it, 0x5ba920), the friction, then the
  low and high angles in degrees: each times the double at 0x70b2d0
  (pi/180 as a float) and clamped to -pi..pi (0x401070, called with the
  floats at 0x70b710 and 0x70b4cc), 0x5baf5d..0x5bb026. VERIFIED (the
  instructions, the strings).
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
- `ragdoll_jitter_scale` is registered and never read: its global
  0xcc789dc is named only by the store at 0x5ba780, and its name string
  (0x6f8790) only by the registration at 0x5ba753. The same holds for
  `ragdoll_dump_anims` (global 0xcc789e0, stored at 0x5ba808). VERIFIED
  (every reference: the disassembly and a byte search of the file for
  each address).
- `Ragdoll_BoneTrace` (0x5be2e0) traces a point from one end to the other
  under 0x2806c91 and, when that starts solid or hits, back; the tunnel
  test fails a bone blocked both ways. VERIFIED (the bytes: the three
  `push 0x2806c91` and the `vec3_origin` boxes). The tunnel test runs it
  along each bone and from the middle of a bone with several children
  to each child's root; a failure leaves the corpse on its death anim.
  INFERRED (`Ragdoll_TunnelTest`). No new body starts past
  `ragdoll_max_simulating`. INFERRED.
- A body runs until every rigid body sleeps or `ragdoll_max_life`
  passes, then holds its pose: the simulating state's handler 0x5bebe0
  asks 0x5be890, which says yes when the body's clock (+0x28) is past
  `ragdoll_max_life` (global 0xcc8c480) or every rigid body has its
  disabled bit (flags +0x18, bit 2), and then calls 0x5bec30 with state
  5. VERIFIED (the instructions, 0x5bebe0..0x5bec2b, 0x5be890..0x5be8ce).
  0x5be890 is `Ragdoll_CheckIdle`. INFERRED.
- Otherwise the handler sets the joints' friction (0x5be780, called at
  0x5bec1d) every frame: each joint's motor force (`FMax`) is its limit
  friction times `0.9 * (1 - t)^2 + 0.1`, t the clock over
  `ragdoll_jointlerp_time` (`fild [eax+0x28]`, `fidiv` the dvar's
  integer, the doubles 0.9 at 0x70b478 and 0.1 at 0x70b040), at motor
  speed 0: a hinge's through 0x5a70c0, a swivel's three axes through
  0x5a72f0 (10.2). VERIFIED (the instructions, 0x5be783..0x5be880).
  Nothing clamps t, and nothing but the idle test above stops the calls:
  past `ragdoll_jointlerp_time` the factor rises again. With the stock
  3000 and 4500 ms it is 1 at the start, 0.1 at 3000 ms and back at
  0.325 at 4500; with `ragdoll_jointlerp_time` at its floor of 100 it
  passes 1700 by the stock max life. VERIFIED (the instructions, the
  dvars). The
  build sets each `FMax` to 15 times the friction (the double at
  0x70b588, 0x5bcc89..0x5bccac), which no step sees: state 4's update
  runs in the build's own frame and sets it again before
  `Phys_RunToTime` (10.7). VERIFIED (the instruction order, and the
  gate's first step).
- vcod's `vcod_cod4::ragdoll` runs the def on the ODE port as 1.7 runs
  it: the slots' states, the build, world 2's steps and the pose (10.7).
  Until 2026-11-26 a position-based solver stood in for ODE (each bone's
  two joints points, its links, limits and self pairs constraints, each
  point swept as a sphere); 10.2's measurements are that solver's.
  VERIFIED (ours, the commit history).
- `play::cod4_players` takes a slot the first frame a corpse reads a
  ragdoll type with `ragdoll_enable` on and feeds it the death anim's
  pose every frame (10.7, 10.7.2). The Options page's Ragdoll row sets
  `ragdoll_enable` (cod4-front-end.md 9).
- Live: `cod4_live_corpses_ragdoll_onto_the_floor` (vcod-server `war`
  on mp_crash, a spectator posing through `Cod4Players`, 40 s on the
  live tests' shared clock): eight bots held still are each put on a
  spot of open, level floor near a `mp_tdm_spawn` and killed there by
  `kill`, 3 s apart, so where a corpse falls is the test's and not a
  fight's. All eight take the type and go idle with every capsule clear
  of the world but for the layer its contacts sink into (10.7) and every
  bone's segment clear; each that slept rests on what is under it, and
  one of the eight ran to max life in the run. VERIFIED (the test,
  ours). In the retail kill capture the corpse's hand is in the wall it
  fell against when its type changes at 49550, and the tunnel test at
  the build two frames on refuses it
  (`the_kill_captures_corpse_fails_the_tunnel_test`); whether
  retail's refused it too is not measured, and no capture can measure
  it: the server sends only the corpse's trajectory types (pos 11, apos
  9 at 49550, cod4-combat.md 5.1) and the ragdoll and its tunnel test
  run in the client alone (`Ragdoll_BoneTrace` is iw3mp.exe's), so the
  answer needs retail's client on screen. INFERRED (the wire carries no
  ragdoll state). That a ragdoll looks right is
  pending a human's look.
- Left out: `physicsJolt` on the server, the killcam's
  own ragdoll (`killcamRagdollHandle`), entity collision
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
  that jittered centre (`Phys_ObjAddForce`, which scales it by `1000 /
  msecStep`; the point VERIFIED in 10.7.1).
  The three `flrand` draws are the LCG `* 0x343fd + 0x269ec3` at
  0x5bddf2..0x5bde7e, VERIFIED (the instructions); the rest INFERRED.
- `ragdoll_bullet_force` (global 0xcc789d4) and `ragdoll_bullet_upbias`
  (0xcc789f4) are read by nothing in iw3mp.exe but their registrations
  (0x5ba5c5, 0x5ba612): a bullet does not push a ragdoll on 1.7.
  VERIFIED (every reference to the two globals).
- vcod: `cod4_players::explosion_of` builds the push off a fired event
  (the weapon's `explosionRadius`, `cod4_audio::WeaponAv`, or the
  script words) and `Cod4Players::explode` hands it to the pool
  (`Ragdolls::explode`): a held ragdoll is rebuilt first (10.7), and
  each body's share goes in as `Phys_ObjAddForce` puts it, times `1000 /
  msecStep` of the last post step (0x5a5900) at the jittered centre, the
  body woken; its jitter
  draws run the same LCG. VERIFIED (ours against the instructions).
  vcod-server sends
  `physicsExplosionSphere` and `physicsExplosionCylinder` (event 57 and
  58, the outer radius as the parm, inner and magnitude in `lerp.u`
  words 0 and 1). Gates: `ragdoll::tests::pool::a_blast_wakes_a_held_figure_and_throws_it`,
  `cod4::hardpoint_tests::physics_launch_and_the_explosion_sphere_go_out`
  and the live `cod4_live_blasts_push_the_ragdolls` (eight bots' war on
  mp_crash for 60 s: 3 explodes turned into 3 pushes that reached 9
  ragdolls in its run on the ODE port). VERIFIED (ours). How a thrown body looks is
  pending a hand check.

### 10.2 The spin at rest and the head's twist

The vcod measurements and rules below are the position-based solver's,
which 10.7's port replaced on 2026-11-26; the 1.7 facts stand.

- `ragdoll.cfg`'s def 1 limits each swivel on all three axes: limit 2
  (the neck, joint `1 3 swivel`) x -30..30 ("Head turn"), y -30..30, z
  -10..30; the shoulders x -104..100 and -100..104 ("Shoulder twist"),
  the hips x -45..45 ("Hip twist"). VERIFIED (the file).
- The joint build 0x5bcb30 joins the body of the joint's bone (body 1)
  to the body of the bone that bone's entry names in its first word
  (body 2), and turns the def's limit axes by body 1's rotation into
  world axes (0x5bcb38..0x5bcb65, 0x5bcb75..0x5bcb9e,
  0x5bcc08..0x5bcc7b). VERIFIED (the instructions). Body 2 is the
  parent's. INFERRED (the entry's layout).
  A swivel is a ball joint (0x5a7250, called at 0x5bccca; type 1 at
  joint +0x14) and an angular motor (0x5a7510, called at 0x5bccf2; type
  8) between the same two bodies. VERIFIED (the instructions). The motor
  takes 3 axes (+0x40) in Euler mode (1 at +0x44, 0x5a756b..0x5a7589):
  axis 0 is the first limit axis, anchored to body 1, and axis 2 the
  third, anchored to body 2 (0x5b5860 called with axis 2 and rel 2 at
  0x5a75c9, axis 0 and rel 1 at 0x5a75f8). VERIFIED (the instructions).
  ODE's Euler mode makes axis 1 from those two. INFERRED (ODE 0.5's
  `dxJointAMotor`).
- Each motor axis i takes limit i's low and high angles as its stops,
  speed 0 (the zeros at 0x69c69c), its friction as the force, fudge
  factor 0.9 (the float at 0x70b264), and `phys_joint_cfm`,
  `phys_joint_stop_erp` and `phys_joint_stop_cfm` (globals 0xcc54418,
  0xcc5440c, 0xcc5445c): parameters 0 to 4 and 6 to 8 plus 0x100 times
  i, 0x5a72f0..0x5a7503. VERIFIED (the instructions). Those numbers are
  ODE's `dParamLoStop` to `dParamFudgeFactor` and `dParamCFM` to
  `dParamStopCFM`. INFERRED (ODE 0.5's `common.h`). A hinge is a hinge
  (0x5a7150, type 2) on the first limit axis with that limit's stop
  pair, speed 0, fudge 0.9 and the same three dvars (0x5a70c0). VERIFIED
  (the instructions). A joint with no `ragdoll_limit` line gets one z
  axis, -pi/2..pi/2 and friction 0, written into the def
  (0x5bcbd4..0x5bcbff); a `none` joint gets no ODE joint (0x5bcc8c..0x5bccb7).
  VERIFIED (the instructions).
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
  squared speeds (0x5b9b70, cod4-gsc.md 3.6.1). VERIFIED (disassembly).
  It is time alone: ODE's idle steps count, 10 from 0x5b7350
  (0x5b7376), is zeroed for each of the three worlds by the start-up
  loop at 0x5a2d83, so a body's steps left go under 0 on its first idle
  step and it sleeps on the idle step that takes its time left under 0.
  VERIFIED (the instructions, 0x5a2d32..0x5a2da1, 0x5b9c0a..0x5b9c49).
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

### 10.3 ODE's version and IW's changes

- The physics is ODE built into iw3mp.exe: 0x5b775c passes the file
  `.\physics\ode\src\odemath.cpp` (0x6f8388) and line 126 to the
  message `vector has zero size (%s:%d)` (0x6f83a8). VERIFIED (the
  pushes, the strings).
- The tree is ODE 0.5 (the 2004-05-29 release) or a CVS tree from
  before `dxPosR` came in. INFERRED (the marks below against the 0.5,
  0.6, 0.7, 0.8 and 2006-02-10 snapshot sources).
  - A geom points at its body's position and rotation: 0x5a9803..0x5a9814
    stores body +0xe0 and +0x100 into geom +0x18 and +0x1c. VERIFIED
    (the instructions). Those are 0.5's `pos` and `R` pointers; the 2006
    snapshot and 0.6 on hold `dxPosR` pointers there. INFERRED (the
    sources).
  - The first user geom class is 11 (`lea edi,[edx+0xb]` at 0x5a9be8).
    VERIFIED (the instructions). 11 is 0.5's and the snapshot's; 0.6
    adds the convex class (12), 0.7 the heightfield (13). INFERRED (the
    sources).
  - 0x5b7350 sets gravity 0, ERP 0.2, CFM 1e-5, both sleep thresholds
    1e-6, idle time 0, 10 idle steps, auto-disable off, 20 iterations,
    SOR w 1.3, contact speed FLT_MAX and depth 0 (0x5b735f..0x5b73c1).
    VERIFIED (the instructions, the floats). Those are 0.5's
    `dWorldCreate` defaults (the thresholds 0.001 squared). INFERRED
    (the source).
  - `dBodySetMass` (0x5b6f70) copies the mass and inverts the inertia
    with no mass check; its message is `inertia must be positive
    definite (%s:%d)` (0x6f8328). VERIFIED (the instructions, the
    string). 0.6 adds a `!` to the message, 0.8 the mass check.
    INFERRED (the sources).
  - `initColliders` (0x5a92e0) fills 13 pairs of a 16 by 16 table (0x800
    bytes at 0xcc781d0, rows 0x80 apart): classes 0 and 0, 0 and 1, 0
    and 4, 1 and 1, 1 and 4, 2 and 0, 2 and 1, 2 and 2, 2 and 4, 5 and
    0, 5 and 1, 5 and 2, 5 and 4, then every pair of class 6
    (0x5a9358..0x5a958b). VERIFIED (the instructions). Those are 0.5's
    sphere, box, capped cylinder, plane and ray pairs and its geom
    transform, less its trimesh pairs; 0.5's table is 15 by 15.
    INFERRED (the source).
- IW's changes. The field names below are ODE's. INFERRED.
  - A body is 0x150 bytes, 512 of them in a pool at 0xd820b50
    (0x5b6aa0..0x5b6ac6): flags +0x18, mass +0x20, inverse mass +0x94,
    force +0x98, torque +0xa8, sleep thresholds, time and steps
    +0xc8..+0xd4, time left +0xd8, steps left +0xdc, position +0xe0,
    quaternion +0xf0 (1 at 0x5b6c29), rotation +0x100 (its diagonal 1s
    at +0x100, +0x114, +0x128), linear velocity +0x130, angular velocity
    +0x140 (0x5b6b50..0x5b6d48, 0x5b9b70, 0x5b9158..0x5b9284). VERIFIED
    (the instructions). 0.5 has the position at +0x98 and the
    accumulators after the velocities. INFERRED (the source).
  - A joint keeps its type at +0x14 (0x5b71fa; 1 ball, 2 hinge, 8
    angular motor at 0x5a72a0, 0x5a7191, 0x5a7549); its two body nodes
    sit 0x10 apart from +0x1c (bodies at +0x20 and +0x30, 0x5b58ba,
    0x5b586d), and the type's own fields start at +0x40 (the motor's
    count, mode, `rel` +0x48 at 0x5b5881, axes +0x54 at 0x5b58ab, limit
    motors +0x84, 0x2c apart, at 0x5a734e). Pools: 192 hinges, 160 balls
    and 160 angular motors (`Physics: Out of hinge joints (%d max)` and
    kin at 0x5a722a, 0x5a7288, 0x5a75a3). VERIFIED (the instructions, the
    strings). 0.5 keeps a vtable pointer at +0x14, and its base, with
    `lambda[6]`, ends at 0x50. INFERRED (the source).
  - Three worlds of 0x10110 bytes from 0xd5ec6e0 (0x5b7352..0x5b7358),
    made by the loop at 0x5a2d32..0x5a2da1 in 0x5a2cd0, which then turns
    auto-disable on (+0x4c, 0x5a2d75) and zeroes the idle steps (+0x48,
    0x5a2d83, 10.2). The world holds the iteration count at +0x28 (20
    from 0x5b7389, then `phys_qsi` each frame at 0x5a6502), w at +0x2c
    and a reorder seed at +0x30, set to 0x89abcdef (0x5b7396). 0x5a2cd0
    returns at once while the byte at 0xd5ec4fb is set (0x5a2cd9, set at
    0x5a4648), and 0x5a6ff0 clears it (0x5a70b0): the seed restarts at
    each init after a shutdown and carries across every step of its
    world between. VERIFIED (the instructions). That an init is a map
    load is INFERRED.
  - The world step 0x5b9d40 (called only at 0x5a6625) runs the sleep
    test (0x5b9d6d), cuts islands (0x5b9cc0, at 0x5b9e51) and steps each
    with the quick stepper 0x5b86e0 (0x5b9eef, its one call). 0x5b9cc0
    trims an island to 14 bodies, then drops more from its end while it
    holds 74 joints or more, clearing each dropped body's tag (+0x10,
    0x5b9cc3..0x5b9d30). No instruction holds `dRand`'s constants
    (0x19660d, 0x3c6ef35f). VERIFIED (the instructions).
  - The quick step points the seed pointer 0xcc781cc at world +0x30
    (0x5b86fa..0x5b870b). After SOR (0x5b8eed) and the forces it caps
    each body's speeds (0x5b9294..0x5b93ab): angular over 40 rad/s (the
    square against the double 1600 at 0x70b928) scaled to 39 (0x70b920),
    linear over 1e6 (against 999999995904 at 0x70b918) to 999900
    (0x70b910). VERIFIED (the instructions). 0.5's quick step has no
    cap. INFERRED (the source).
  - SOR_LCP (0x5b8080) fills the row order with the `findex < 0` rows
    from the front and the rest from the back (0x5b82a1..0x5b82cb). Its
    two iteration loops (0x5b7ad0, 0x5b7df0, picked at 0x5b82d3) reorder
    when `(iteration & 7) == 7` (0x5b7af0, 0x5b7e24): m swaps of two rows
    each, an index `((x >> 17) * m) >> 15` off `x = x * 0x343fd +
    0x269ec3` (MSVC's `rand` constants), the seed read and stored
    through 0xcc781cc (0x5b7b10..0x5b7b7d). VERIFIED (the instructions).
    The fill from both ends is the one ODE ships from 0.8 on (0.5 to 0.7
    fill from the front), and 0.5 and 0.9 shuffle when `(iteration & 7)
    == 0`, with `dRandInt`. INFERRED (the sources).

### 10.4 The ODE core as ported

`vcod_cod4::ode` translates upstream ODE 0.5 (2004-05-29, under its BSD
licence) and takes IW's changes from iw3mp.exe 1.7 alone; the ragdoll
runs on it (10.7). Upstream function names below are 0.5's; that the
binary's functions are those is INFERRED (their shape against the
source), the rest is labelled per claim.

- **Precision.** IW's build is dSINGLE: every body, joint and row field
  is a DWORD the code loads with `fld DWORD` and writes with `fstp
  DWORD` (the SOR row at 0x5b7be6..0x5b7db2, the body step at
  0x5b8386..0x5b86be). VERIFIED (the instructions). The x87 runs at
  53 bits (11), so a value held on the stack between two stores rounds
  as a double does; the port computes in `f64` and rounds to `f32` at
  each of the binary's DWORD stores, `x87::F80` is not needed. Each
  three-term dot is `(p0 + p1) + p2` whatever order the products load
  in, the angular dot in the SOR stored before the linear one and both
  before the fold into `delta` (0x5b7c0f..0x5b7c4f). VERIFIED (the
  instructions). The port matches the store points in the stepper, the
  SOR, the joints' rows and the mass, rotation and Cholesky helpers;
  `sin`, `cos`, `atan2` and `sqrt` go through MSVC's `_CIsin` (0x68b8c0),
  `_CIcos` (0x68b9f0), `_CIatan2` (0x68b86a) and `_CIsqrt` (0x68aec0),
  where the port takes Rust's: equal after the store for `sqrt`, and for
  the others INFERRED to agree to the float.
- **The world.** Three worlds of 0x10110 at 0xd5ec6e0; `dWorldCreate`
  (0x5b7350) writes gravity 0 (+0x10), ERP 0.2 (+0x20), CFM 1e-5
  (+0x24), 20 iterations (+0x28) at over-relaxation 1.3 (+0x2c), the
  reorder seed 0x89abcdef (+0x30), the contacts' most correcting
  velocity `FLT_MAX` (+0x34) and surface layer 0 (+0x38), the sleep
  thresholds `0.001f * 0.001f` (+0x3c, +0x40), idle time 0 (+0x44), 10
  idle steps (+0x48) and sleep off (+0x4c). VERIFIED (the stores at
  0x5b735f..0x5b73c1). IW's order puts the seed and the contact
  parameters before the sleep test's, upstream's after. `Phys_Init`
  then turns sleep on and zeroes the idle steps (0x5a2d83). VERIFIED
  (the store). `dInfinity` is `FLT_MAX` (0x70af58, 0x70af5c). VERIFIED
  (the floats).
- **Per step.** `Phys_RunFrame` (0x5a6440) stores into the world before
  its one island pass (0x5b9d40): `phys_cfm` (1e-4), `phys_erp` (0.8),
  for world 2 `phys_mcv_ragdoll` (1000, the others `phys_mcv`) at
  +0x34, `phys_csl` (1) at +0x38, `phys_qsi` (15) at +0x28, the squares
  of `phys_autoDisableLinear` and `phys_autoDisableAngular` at +0x3c and
  +0x40, `phys_autoDisableTime` at +0x44, and gravity `(0, 0,
  phys_gravity)`. VERIFIED (the instructions). `Phys_PostStep` (0x5a6640)
  then clears every awake body's force and torque, empties the contact
  group and, in world 2, adds `avel * -0.01` (10.2): IW's stepper leaves
  the accumulators as they are. VERIFIED (the instructions). The port:
  `Params::ragdoll` (world 2 after `Phys_Init` and a frame at the dvars'
  defaults), `World::zero_forces`, `World::empty_contacts`. The damping,
  the 17..34 ms step scheduling (`Phys_RunToTime`, 0x5a6d50) and the
  tunnel and bounds checks after a frame (0x5a6c10) are the glue's.
- **Bodies.** `dBodyCreate` (0x5b6b10) takes a pooled body, upstream's
  defaults, the world's sleep test, and puts it at the head of the
  world's list; `dJointAttach` (0x5b72e0) puts the joint at the head of
  each body's list, as upstream's; joint init (0x5b71b0) puts a joint at
  the head of the world's. VERIFIED (the instructions). The island walk
  visits in those orders, so the port keeps them. `dBodySetMass`
  (0x5b6f70) inverts the inertia by a 3x3 Cholesky (0x5b6840, 0x5b6570,
  0x5b6680) storing every assignment, and falls back to the identity;
  `dBodySetRotation` (0x5b6ec0) goes through `dQfromR` (0x5b98b0),
  `dNormalize4` and `dQtoR`. VERIFIED (the instructions). `dxStepBody`
  (0x5b8380) moves the position, turns the quaternion by a finite
  rotation when flag 1 is set (`sinc` at 0x5b8320 switching to `1 -
  x^2 / 6` under 1e-4, the double at 0x70b178, with 1/6 a widened float,
  0x70b600) and by `dWtoDQ` otherwise, and calls no `dGeomMoved` (IW
  moves the geoms in `Phys_PostStep`). VERIFIED (the instructions). The
  ragdoll's bodies run finite rotation (0x5b7170) with gravity on; the
  finite rotation axis mode is not ported.
- **Mass.** `dMassSetCappedCylinder` (0x5b5cd0) is upstream's with its
  constants floats widened (pi 0x70b080, 4 pi / 3 0x70b258, 0.4
  0x70b250, 1/12 0x70b240), the two masses stored and the inertia's
  terms kept on the stack; `dMassAdjust` (0x5b5ec0) scales by a stored
  ratio. VERIFIED (the instructions). For a capsule bone the ragdoll
  calls it at density 1, direction 1, the length half the bone's, then
  adjusts to the bone's mass, and moves the body to the capsule's middle
  instead of translating the mass (0x5a56a0, 0x5a4ab0, 0x5a47c0).
  VERIFIED (the calls).
- **Joints.** No vtable: an int at joint +0x14 picks `getInfo1`
  (0x5b5b30) and `getInfo2` (0x5b5bd0) through the tables at 0x5b5bb4
  and 0x5b5c4c: 1 ball, 2 hinge, 4 contact, 6 a joint of six rows, 7
  one of none, anything else the angular motor, which IW creates as 8. VERIFIED (the
  tables, the creates). `getInfo2` writes `lo` and `hi` into the
  interleaved 0x90 rows at `row * rowskip` (`rowskip` 0x24 floats),
  `c`, `cfm` and `findex` by row. VERIFIED (0x5b3612..0x5b3663 and the
  contact's).
  - Ball (`setBall`, 0x5b2ab0) and its anchors (`setAnchors`, 0x5b2e30)
    are upstream's, `k = fps * erp` stored. Hinge: `hingeGetInfo1`
    (0x5b3840), `hingeGetInfo2` (0x5b38e0), its angle (0x5b30d0,
    0x5b3010) storing the sine, the arctangent and the angle, init
    0x5b3780. VERIFIED (the instructions).
  - The limit motor: `limot.set` (0x5b3140) stores the stops without
    upstream's order checks; `addLimot` (0x5b3250) reverses the motor's
    force at the high stop when `vel` is 0 as well as when it is
    positive (0x5b3403..0x5b3420), so the ragdoll's joint friction
    (`vel` 0) presses into either stop where 0.5's pushed body 1 off the
    high one. VERIFIED (the instructions). Later ODE has the same rule.
    INFERRED (the 0.7 source).
  - The angular motor: `amotorGetInfo1` (0x5b5680) keeps the Euler
    angles local (IW's joint has no angle field), and its inlined limit
    test reads an angle that is NaN as at the low stop; `amotorGetInfo2`
    (0x5b5750) holds `ax1 x ax2`, `ax1` and `ax0 x ax1`; the axes
    (0x5b5120), angles (0x5b52e0), reference vectors (0x5b55c0) and
    `dJointSetAMotorAxis` (0x5b5860) are upstream's. VERIFIED (the
    instructions). Only Euler mode is ported, the swivel's.
  - The contact: `contactGetInfo1` (0x5b4270) is upstream's;
    `contactGetInfo2` (0x5b4420) reads the most correcting velocity and
    the surface layer from world +0x34 and +0x38 through a pointer the
    stepper passes, and caps `k * depth` against the product before it
    is stored. IW's create (0x5b7210) copies the surface and the geom
    but not `fdir1`; the group (0x5b7270) empties last made first.
    VERIFIED (the instructions).
- **Islands** (0x5b9d40). The sleep test (0x5b9b70) is upstream's and
  runs first; with the idle steps 0 it is time alone. The walk tags
  bodies and joints as upstream's and wakes a body as it reaches it
  (0x5b9e08). VERIFIED (the instructions). IW's split (0x5b9cc0) then
  keeps at most 14 bodies, dropping the last, and drops one more body at
  a time while 74 joints or more remain; a joint goes with a dropped
  body (0x5b9f20, test 0x5b9c60) when its other end is dropped too or is
  the world, and one to a kept body stays, its dropped end's index read
  as 0, the island's first body. VERIFIED (the instructions). That the
  last is a slip is INFERRED. Dropped bodies and joints are untagged, so
  a later body in the list can start an island from them that step.
  VERIFIED (the instructions). Each body's island index goes into its
  joints (+0x24, +0x34) before the step (0x5b9e5d..0x5b9eb6). VERIFIED
  (the instructions).
- **QuickStep** (0x5b86e0). Upstream's, but: joints with no rows stay in
  the list; the rows, `lambda` and the constraint forces start at 0
  every step (0x5b8af1, 0x5b80bc), where 0.5 warm-starts from the last
  step's `lambda * 0.9`; and after the forces it slows a body turning
  faster than 40 rad/s to 39 (the doubles 1600 at 0x70b928 and 39 at
  0x70b920) and one moving faster than `sqrt(1e12)` to 999900 (0x70b918,
  0x70b910), the speed scaled after the solve, then steps it and clears
  its disabled flag (0x5b93f5). VERIFIED (the instructions).
- **SOR** (0x5b8080). Rows with `findex < 0` first in order, the rest
  filled from the back, so in reverse (0x5b8297..0x5b82cb). On each
  iteration with `(iter & 7) == 7`, `m` swaps of two drawn rows, each
  draw the MSVC step `s * 0x343fd + 0x269ec3` on the world's seed
  (through the pointer at 0xcc781cc, set at 0x5b870b) and the index
  `((s >> 17) * m) >> 15` (`__allmul`, `__allshr`, 0x5b7ad0..0x5b7b7d);
  the seed carries over every step since `Phys_Init`. Upstream 0.5
  shuffles with `dRandInt` on `(iter & 7) == 0`. A one-body island runs
  its own loop (0x5b7df0) that updates only body 0. A row's new `lambda`
  takes `hi` when `new - hi`, stored, is not below 0, and `lo` over it
  when `lo - new` is not below 0 (0x5b7cad..0x5b7d12). VERIFIED (the
  instructions). Whether the reorder is IW's or a later ODE patch's is
  INFERRED either way.
- **For the glue** (phase 3, read here): the hinge is made at 0x5a7150
  with `vel` 0, `fmax` the def's friction times 15, fudge 0.9,
  `phys_joint_cfm`, `phys_joint_stop_cfm`, `phys_joint_stop_erp` and the
  def's stops; the swivel at 0x5a7250 (ball) and 0x5a7510 (Euler motor,
  axis 2 on the parent, axis 0 on the child, the same eight settings
  per axis); every joint is made with the child bone first. The friction
  update (0x5be780) rewrites them with `fmax` the friction times
  `0.9 (1 - t)^2 + 0.1`, without the 15. VERIFIED (the instructions).
  A body's first step sees the update's, friction times 1 (10.7).
  VERIFIED (the instruction order).
- **Tests** (`ode::tests`, ours): the reorder's draws from the init seed
  against the formula by hand, SOR on a two-row LCP with a bound and on a
  friction row, a capsule's mass against its closed form, a hinge
  pendulum's period (2.016 s within 0.5%), a ball chain under -800
  holding within 3% of a link, a hinge and an Euler motor resting on
  their stops, friction pressing into either stop, a sliding contact
  stopping in `v / (mu g)`, the sleep's 53rd step at 17 ms and the
  14-body split. VERIFIED (the tests, ours). Let go level, the chain's
  lower link whips past 39 rad/s and the clamp opens its ball by 1.4
  units. VERIFIED (the run, ours). `ragdoll_ab` gates the port with its
  glue on two retail captures (10.7).

### 10.5 A ragdoll step by step on 1.7

Captured from the Windows client under Wine and gdb (docs/dev/retail.md,
"A ragdoll from spawn to rest"): the client's own player on mp_crash `war`,
put at (207, 2291, 296) by `setviewpos` and killed by `kill`, at
`fixedtime` 17. Addresses are iw3mp.exe's.

- Where the dump reads. `Ragdoll_Update` (0x5beca0) adds the frame's msec
  to the ragdoll clock at 0xd5ec534 (0x5beccb), runs each of the 32 slots
  (0x9d4 bytes from 0xcc78a00) through its state's handler from the table
  at 0x71e658 until the state (+0x24) holds, adds the msec to the slot's
  time in state (+0x28) and one to its frames (+0x2c), then calls
  0x5a6d50 for world 2 up to the clock (0x5bed24). A slot's bones (count
  +0x34) are 0x1c bytes from +0x38, the body each one's +8 (0x5bcdc0..0x5bce0b,
  stored at 0x5bc9e2). 0x5a6d50 runs at most two steps (0x5a6dfb..0x5a6e6b)
  and 0x5a6e69 follows each, with the step's msec in `ebx`. VERIFIED (the
  instructions).
- Each 17 ms frame ran one 17 ms step: `msecStep` stayed 17 with 12 bodies
  awake. The world's seed read 0x89abcdef when the first body appeared,
  changed every step while a body was awake and held on a step with every
  body asleep. VERIFIED (the captures).
- The 12 bodies of def 1 start with flags 0x11 (0x10 auto-disable) and 0.9
  s of idle time left, `phys_autoDisableTime`. A step under both speed
  thresholds takes 0.017 off a body's time, any other puts it back to 0.9,
  and a body's time goes on falling under 0 while another of the ragdoll
  is awake. All 12 set the disabled bit (0x4) on the same step, the first
  on which every body's time is under 0, and that step leaves their
  velocities as the step before left them. VERIFIED (the captures). That
  the joints make the ragdoll one island, which ODE's step disables or
  enables whole, is INFERRED (ODE's island walk).
- The frame after, the slot turns idle (state 5). Its entry (0x5beb10)
  destroys the joints (0x5bce20) and the bodies (0x5bce80, `dBodyDestroy`
  0x5b6d60), zeroing each bone's body (0x5bced4). VERIFIED (the
  instructions; the capture's idle step lists no body). Its call to save
  the pose (0x5bc4f0) runs only in states 3 and 4 and the state reads 5
  by then, so the pose held is the one state 4 saved the frame before
  (10.7). VERIFIED (the instruction order, 0x5bec5f before 0x5bec6b).
- Six takes on 2026-11-26, each the first ragdoll after its map load.
  The start differs from take to take, the root by up to 44 units: the
  death anim and the moment the corpse turns ragdoll vary. Four slept,
  after 70, 79, 96 and 167 steps (1.2 to 2.8 s). Two never did: the slot went idle at `ragdoll_max_life`, step
  266, 4505 ms in, as one body or another kept rising over
  `phys_autoDisableAngular`'s 1 rad/s (to 1.1..2.2 rad/s) while every body
  moved 1 to 4 units a second. From start to rest the root moved 2.5 to
  5.9 units, nearly all of it down. VERIFIED (the captures).
- The committed take, `crates/cod4/tests/fixtures/ragdoll/kill-mp_crash-1.7.txt`
  (format in that directory's README): the ragdoll clock read 11390 when
  `kill` went in and 13379 after the first step; the fastest body 47 u/s
  and 12.2 rad/s on the first step and 75 u/s on the fifth, every body at
  most 2 u/s and 0.6 rad/s from step 27 on, asleep on step 79 (1343 ms), the slot idle on step
  80. The root moved 4.78 units, all down; the bodies' centres at rest
  span 33.2 by 48.1 units across and 3.4 high (z 237.3 to 240.7).
  VERIFIED (the capture).
- Against vcod's solver (10, 10.2): vcod sleeps each body on its points'
  linear speeds alone; 1.7 sleeps the ragdoll whole on both speeds, and a
  resting body's angular jitter is what keeps a 1.7 ragdoll awake to
  `ragdoll_max_life`. VERIFIED (the captures against 10.2).

### 10.6 The ragdoll world's contacts as ported

`vcod_cod4::ode::collide` ports what makes world 2's contacts: IW's capsule
colliders over the clip map, the near callback and its contact pass, and
the self collision hook. Addresses are iw3mp.exe 1.7's, read with
`objdump`. The triangle test is ODE 0.5's (`collision_trimesh_ccylinder.cpp`,
Alen Ladavac's) as IW rebuilt it; that the functions are that file's is
INFERRED (their shape against the source), the rest is labelled per claim.
The ragdoll's world steps them (10.7).

- **The geom.** 0x5a8110 dispatches class 14 (the byte table at 0x5a89b0,
  the jumps at 0x5a8998) to its capsule arm 0x5a8721: kind 5 into the
  context (+0x34), the geom's +0x4c into the context's +4 (the rotation
  column that is the axis, the columns at +0x44), its radius (+0x50) into
  +0x8c and +0x90 and its +0x54 into +0x94, the half length of the
  segment; the box the walk takes is a cube of the larger of `r` and
  `2 r + h` (the double 2.0 at 0x70ae50) grown by 1 round the geom's
  position (0x5a8370), narrow when `2 r` is under
  `phys_narrowObjMaxLength`, which no capsule collider reads. VERIFIED
  (the instructions). 0x59e870 builds the colliders' block from the
  context: the segment's ends `c + h a` and `c - h a` (each `(-a) (-+h) +
  c` stored), the centre, `-a`, `r`, `r^2`, `h + r`, `h`. VERIFIED (the
  stores at 0x59e87a..0x59e92d). `Capsule`, `capsule::Seg`.
- **The walk** (`ClipMap::world_pieces`). Of each leaf `CM_BoxLeafnums`
  gives (0x4f2c60, up to 0x400): with its brush contents under the mask
  (0x2806c91, 0x5a882c), its bounds strictly over the box's (0x5a7bc0)
  and its brush tree walked by 0x5a7a60, each brush whose contents and
  bounds meet the box in the order the tree reaches them: a node's
  straddlers first, then the side above `dist` (when `dist` is not over
  the box's top), then the one below (when `dist` is not under the box's
  bottom, else the side above alone); then, with its terrain contents
  under the mask, each AABB tree whose root material's contents meet it
  (0x5a7ea0), nodes culled by 0x5a7c40 (the box's half size plus the
  node's under the distance from the geom's position to the node's
  centre on an axis), children in order, each partition once by a check
  count (0x5a7d8e), with its leaf node's material's surface flags
  (0x5a7dc4). VERIFIED (the instructions). A brush reached from two
  leaves is collided twice: 0x5a7a60 keeps no check count. VERIFIED (the
  instructions). 0x5a79b0 calls the brush collider only while the
  contacts have room; 0x59ed10 returns at once when they have none.
  VERIFIED (the compares at 0x5a79bd, 0x59ed2a). vcod's `world_solids`
  (cod4-gsc.md 3.6.1) sorts and dedups the brushes and triangles, which
  the box collider keeps; the capsule's walk is its own.
- **Brush faces.** A brush's faces are its six axial ones in 0x596ae0's
  order (`-x <= -mins.x`, `x <= maxs.x`, then y and z; face `i` reads
  `[i & 1][i >> 1]` of the brush's tables) and its sides. Each face's
  neighbours are a run of the zone's brush edges from the brush's
  `baseAdjacentSide` (+0x30), the axial faces' first and count at +0x34
  and +0x40, a side's at +8 and +0xa; a neighbour under 6 is an axial
  face, else side `i - 6`. VERIFIED (0x5998e3..0x599923). The winding
  (0x5998e0) is the meet of the face's plane with each pair of
  neighbours in turn, the last and the first meeting first, by Cramer's
  rule with each cross part, product, the determinant and its inverse
  stored; a meet whose determinant reads under 0.001 (0x70b278) is
  skipped, a corner within a unit of the last kept (the squared distance
  0x403310 against the float 1.0 at 0x6be570) is overwritten by the next,
  the last goes when it is within a unit of the first, fewer than three
  give none, and a face with more neighbours than the caller's room
  (0x100 from 0x59ea10) none either. VERIFIED (0x599a04..0x599c86). On
  mp_crash 53931 of the 54109 faces with edges wind clockwise seen from
  outside and the other 178 give none (`crashs_brush_windings_run_clockwise_from_outside`).
  VERIFIED (the test, ours). `BrushFaces`.
- **Against a brush** (0x59ea10). Each face's depth 0x59e940, the
  lesser of the segment's ends' `n . e - d - r`, or `-FLT_MAX` (0x70af5c)
  when the centre lies behind it; a depth of 0 or more ends the collide
  with no contact. The deepest face with edges is the reference (a later
  one only when strictly deeper); none, or a segment that neither crosses
  its plane nor comes within `r` of it with its nearer end (0x59e790),
  ends it too. VERIFIED (the instructions, 0x59ea70..0x59ebb3). Its
  winding goes to the triangle colliders as `(w0, w2, w1)` with three
  corners (0x59dd90), else as a fan: ends `(w0, w2, w1)` and
  `(w[n-3], w[n-1], w[n-2])` (0x59e130), `(w0, w[k], w[k-1])` for `k`
  from 3 to `n - 2` between (0x59e4a0), each with the reference's plane
  and surface flags (0x5a7970: the axial material at +0x24 or the side's,
  the material's +0x40), then one 0x59c710. VERIFIED (the pushes at
  0x59ec31..0x59ecda). The last end starts at `w[n-3]` (`eax` is
  `&w[n-3]` at 0x59ecc0), not `w0`, so a fan of four or more covers its
  face as two ears and the triangles between, open on their inner edges.
  VERIFIED (the instructions); that it is a slip is INFERRED.
- **Against triangles** (0x59ed10). Each triangle `t` of the partition:
  its plane `(t2 - t0) x (t1 - t0)` normalized by 0x4076d0 and its
  distance at `t0` (0x5a18c0), out of 0x596c70, which also culls it
  against a sphere of the context's +0x30 round the centre; with the
  capsule touching the plane (0x59e790) it goes to 0x59dd90 as
  `(t0, t2, t1)`; the partition's contacts go through one 0x59c710.
  VERIFIED (the calls and pushes at 0x59edfc..0x59ee35). The cull is not
  ported: its sphere holds the whole capsule, so it drops nothing that
  can touch. INFERRED (its radius, the box's half diagonal, 0x5a88cf).
- **The triangle test.** 0x59cd70 tests ODE's 19 axes in IW's order: 11,
  then 2 to 7, 10, 9, 8, 12 to 19 and the face's normal negated (1, not
  flipped) last; 0x59d4a0 (a fan's end) the normal first, then 3, 4, 6, 7,
  9, 10, 12, 13, 15, 16, 18, 19, leaving out the edge `v0 v1` and the
  corner `v0`; 0x59d9c0 (between) the normal, 3, 6, 9, 12, 15, 18, the
  edge `v1 v2` and the corner `v1` alone. VERIFIED (the ids pushed at
  each call to 0x59ca70). ODE's squared length gate of 1e-6 before each
  axis is gone; 0x59ca70 skips an axis under 1e-6 long (the double at
  0x70b508, where 0.5 has 1e-5f), stores the length, its inverse and the
  normalized axis, takes the capsule's reach `|a . n| h + r` and the
  triangle's centre and half extent, separates when their sum is under
  the centre's size, and keeps a strictly deeper overlap, turned to the
  triangle when its centre is negative and the axis may flip. VERIFIED
  (the instructions). The segment pushed out by `r` along the best
  normal (0x59dce0) is clipped (0x59c910: the meet `a - (a - b) t` over
  the end behind) behind the face and inside the triangle's edges, all
  three for 0x59dd90, `v1 v2` and `v2 v0` for an end, `v1 v2` alone
  between, each edge plane 1e-6 out (the float at 0x70b930; the far
  edge's distance less the double 1e-6); each end left is a local
  contact, its depth along the normal past the triangle's near side, at
  least 0 (0x4093c0), with the surface flags. VERIFIED (the instructions,
  0x59dd90..0x59e127, 0x59e130..0x59e490, 0x59e4a0..0x59e784).
- **Local contacts.** 32 of 0x24 bytes at 0xcc53f80 (0x59c5a6), emptied
  by each brush and each partition. 0x59c710 with two or more drops, of
  each pair whose positions and normals are within 0.001 on every axis
  (the squared difference against the double at 0x70b300), the shallower
  one (the first on a tie, 0x59c600), and copies the rest into the
  `dContactGeom`s while there is room, each normal and position's fourth
  float 0, the surface flags at +0x2c. VERIFIED (the instructions).
  0x5a95a0, the `dCollide` the near callback calls with the body's geom
  as `o1`, turns every normal round when the class pair's entry is
  reversed (0x5a9603..0x5a972d). VERIFIED (the instructions). That the
  capsule and world pair is reversed, as ODE's `dCreateGeomClass` sets a
  user class's colliders, is INFERRED; so the ported normals point out of
  the world into the capsule, as the joint pushing `b1` needs.
- **The near callback** (0x5a5f20, `near_world`). Bodies joined by a
  joint other than a contact do not collide (0x5a5f60..0x5a5f85). The
  collide has room for 0x80; none sets the lone body's state 2
  (0x5a612d). `mu` is `(friction1 + 0) + friction2` times
  `phys_frictionScale` (userdata +0x44, 0x5a6085), bounce the least of 1
  and each body's +0x48 (0x409400), `mu2` 0, `bounce_vel` 0.1 (0x6b72dc),
  mode 0x301c, and world 2's soft ERP and CFM `phys_contact_erp_ragdoll`
  and `_cfm_ragdoll` (0.3, 0.001), the others' `phys_contact_erp` and
  `_cfm` (0.8, 1e-5) (0x5a603d..0x5a6075). Five or more contacts go
  through 0x5a2320's cut (cod4-gsc.md 3.6.1, at most 15). VERIFIED (the
  instructions, the registrations). A ragdoll bone's preset has bounce
  0 and the bone's friction (0x5bc980..0x5bc9b6). VERIFIED (the stores).
  The impact callback after the pass (0x5a5de0) is cod4-gsc.md 3.6.7.
- **The contact pass** (0x5a2760, `contact_pass`). 0x5a2280 sets both
  bodies' state 1 when two contacts' stored normal dot is under -0.99
  (the double at 0x70b268); with `b1` in state 1, else `b2`, 0x5a25d0
  keeps one of each facing pair by that body's position; each contact
  whose point a body in state 1 is leaving (`dBodyGetPointVel` 0x5b70d0,
  normalized by 0x5b7460, along the normal over 0.01 at 0x6df494) is
  skipped. VERIFIED (cod4-gsc.md 3.6.1, the instructions). A contact joint
  is made only while the world holds under 0x1000 joints (`dxWorld` +0xc,
  0x5a2a34) and its group under 0x1000 contacts (0x5a2a43); past either
  the pass prints `Maximum number of ODE physics contact points
  exceeded.` (0x6f7300) and returns. VERIFIED (the instructions). The
  jitter (0x5a23e0) is ported for worlds 0 and 1 (cod4-gsc.md 3.6.7);
  world 2 keeps no region, as 0x5a7730 fills world 0's list alone
  (cod4-gsc.md 3.6). Under `phys_noIslands` (0xcc54410,
  default 0) the pass makes body pairs another way, not ported.
  VERIFIED (the registration at 0x5a3799).
- **Self collision** (0x5bd9c0, `selfcol`). For each ragdoll of the pool
  in state 3 or 4 and each `ragdoll_selfpair` of its def (+0xe88, two
  bytes each at +0xe8c): each bone's segment from its body's position
  moved by its object's offset in the body's frame (0x5a4740) along the
  body's x axis for its length (0x5bd800); their nearest points by Dan
  Sunday's clamped solve (0x5bd4e0: parallel under a determinant of 1e-6,
  a numerator under 1e-4 at 0x70b000 reads 0); each bone's radius (def
  bone +0x30) times `ragdoll_self_collision_scale` (1.2); with the points
  no farther apart than the two radii (equal counts, 0x5bdb99) and not
  closer than `sqrt(0.002)` (0x70b070, 0x5bd8df), one contact along
  `pb - pa`, half the overlap deep, at `pa` plus the normal times
  `r_a - depth`, `mu` bone `a`'s friction (def bone +0x40) with no
  `phys_frictionScale`, bounce 0, `phys_contact_erp` and `_cfm`, `b1`
  bone `b`'s body and `b2` bone `a`'s, through the contact pass
  (0x5a2b50 into 0x5a2760). VERIFIED (the instructions, the bone fields
  `ragdoll_bone` stores at 0x5babb3..0x5bac37).
- **Precision.** Each port stores where the binary writes a DWORD and
  keeps the x87's 53 bits between (10.4); the three-term dots load in
  orders that differ only in their first two terms, so each is
  `(p0 + p1) + p2`. VERIFIED (the instructions). The 0x5a2320 cut
  (`collide::reduce`) stores where the binary does too (cod4-gsc.md
  3.6.4).
- **Tests** (`ode::collide::tests`, `capsule_world`, ours): a level
  capsule half a unit into a box brush's top gives the segment's two ends
  0.5 deep along +z; across the box's edge, the outer one on the edge; on
  two triangles of a 30 degree slope, three contacts along the slope's
  normal (the segment cut at the shared edge, the two there one); the cut
  keeps 15 of 18 contacts in three clusters; two bones at their scaled
  radii touch at depth 0 and a hair farther do not; a capsule dropped on
  a floor brush in the ragdoll world at 17 ms rests 3.93 over it (radius
  5, the 1 unit surface layer and the soft CFM let it sink) and sleeps on
  the 53rd step its sleep test found it still; on mp_crash a level
  capsule under each flat `mp_tdm_spawn` floor meets it straight up, and
  terrain a tenth of a unit under a floor brush by spawn (1504, 512)
  adds contacts of its own. VERIFIED (the tests, ours). `ragdoll_ab`
  gates them on retail's captures (10.7): the reorder's seed, which
  moves once per row and iteration, holds retail's for the captures'
  first 15 and 189 steps, so each of those steps made as many contact
  rows as retail's. VERIFIED (the gate).
- **For the glue** (phase 3): a ragdoll body's geom is class 14 made at
  0x5a8e10 with the axis column, radius and half length the bone gives;
  the near callback runs per awake body against the world each step
  before the island pass, the self hook once per step, and
  `World::empty_contacts` after. The geom's +0x4c is the axis column
  1-based, 1 for a bone (x), and the phys object's offset (userdata +0)
  is `(-L/2, 0, 0)` (10.7). VERIFIED (0x5a8e10, 0x5a47c0).

### 10.7 The ragdoll as ported, and its gate

`vcod_cod4::ragdoll::pool` runs CoD4 1.7's ragdolls on the ODE port
(10.4, 10.6): the slots and their states, the build, world 2's steps and
the pose, read off iw3mp.exe 1.7 with `objdump`. It replaced the
position-based solver of 10 and 10.2 on 2026-11-26. Labels per claim.

- **The frame.** `Ragdoll_Update` (0x5beca0) returns unless
  `ragdoll_enable` is on and `cl_paused` (0x8c9400) is 0
  (0x5beca1..0x5becc9), adds the frame's msec to the ragdoll clock
  (0x5beccb), runs each live slot's state update until its state holds,
  then adds one to its frames (+0x2c) and the msec to its time in state
  (+0x28), and last stores the self collision hook 0x5bd9c0 at 0xcc54628
  and calls `Phys_RunToTime(0, 2, clock)` (0x5bed16..0x5bed24). VERIFIED
  (the instructions). The table at 0x71e658 holds an entry, an exit and an
  update per state; 0x5bec30 runs the old state's exit (0 refuses), writes
  the new state, runs its entry (0 puts the old one back) and zeroes the
  two counters. VERIFIED (0x5bec30..0x5bec80). `Ragdolls::update`,
  `transition`.
- **The states.** VERIFIED (the handlers' instructions):
  - 0, free: the entry (0x5be8d0) frees the joints and bodies and clears
    the slot; the exit (0x5be900) takes the def's counts.
  - 1, the DObj: the update (0x5beb90) gives up past 3 frames (`Ragdoll
    activation timed out waiting for dobj 0x%x`, 0x6f8cb0) and moves on
    once 0x5bc730 finds every def bone's names and every base lerp bone in
    the DObj; the exit (0x5be940) keeps their bone indices.
  - 2, the snapshots: the update (0x5be720) flips +0x2f0 and stores a
    snapshot (0x5bc5b0: each def bone's and base lerp bone's origin and
    raw quaternion, 0x5bbc30); on its second frame it keeps the time in
    state as the time between them (+0x30, the first snapshot's frame's
    msec) and goes to 3.
  - 3, the build: the entry (0x5be600) builds the bodies (0x5bcdc0) and
    the joints (0x5bcd60), places the bodies at the latest snapshot
    (0x5bc670), sets their velocities (0x5be060), runs the tunnel test
    (0x5be3a0) and the base lerp bones' first pose (0x5bbf90), then goes
    to 4, or to 0 when the tunnel test failed.
  - 4, simulating: the entry (0x5beb60) refuses while 0x5ba1f0's count of
    slots in 3 or 4, this one among them, has reached
    `ragdoll_max_simulating`, and sends the slot to 0 (to 5 from 5): at
    the default 16 at most 15 simulate. The update (0x5bebe0) goes to 5
    when 0x5be890 says idle (10), else saves the pose (0x5bc4f0) and sets
    the joints' friction (0x5be780, 10).
  - 5, idle: the entry (0x5beb10) frees the joints and bodies; the pose
    held is the one state 4 saved the frame before (10.5). The exit
    (0x5beaa0), to 2..4 only, needs the bones, room under the cap, and
    rebuilds the bodies and joints (0x5bd4a0) at the pose held, with no
    velocities and no tunnel test. Only a blast wakes a slot (0x5bdd5e,
    10.1).
  - The create (0x5ba270) refuses at the cap (0x5ba287) and binds def 1
    (0x5ba317).
- **The bodies** (0x5bc860, each def bone): its bone's base pose, the
  model's `baseMat` (`models[0]` +0x1c, 0x57f480), under the entity's
  origin and axes (0x5bbd50), the turn `Qent (x) Qbone` (0x5bb840), half
  round z for a mirrored bone (`q (x) (0, 0, 1, 0)`, 0x71e648); the length
  `L = |p1 - p0|` in that pose (0x5bca03..0x5bca2f). In world 2
  (0x5a4d90): finite rotation (0x5b7170), the rotation through the
  quaternion's axes (0x560fb0, 0x596b80, `dBodySetRotation`), a sphere's
  mass of the bone's (0x5b5c70), then the capsule (0x5a56a0):
  `dMassSetCappedCylinder(1, x, r, L/2)` adjusted to the bone's mass, a
  class 14 geom with axis column 1 (1-based: x), radius `r` and half
  length `L/2` (0x5a8e10), and the body moved from p0 by `R (L/2, 0, 0)`,
  the phys object's offset left at `(-L/2, 0, 0)` (0x5a47c0). VERIFIED
  (the instructions). So the mass's cylinder is half the bone, the
  geom's segment all of it, and the body's centre the bone's middle along
  its own x, which need not run through p1. VERIFIED (the same). The cog
  lerp (def bone +0x38) is read by none of it. VERIFIED (0x5bc860..). vcod
  takes the base pose off the rig's first model (its `baseMat`
  normalized on load) under the entity's origin and turn as drawn: equal
  but for rounding. INFERRED. Whose entity: 10.7.2.
- **The joints** (0x5bcb30, def order), built in the base pose before the
  placement: body 1 the joint's bone's, body 2 its parent's (def bone
  +0x44); the anchor `pos1 + R1 (-L/2, 0, 0)`, the bone's root (0x5a4740);
  a joint with no limit takes z, -1.5707964..1.5707964 and friction 0,
  written into the def (0x5bcbd4..0x5bcbff); each limit's axis turned by
  R1 (0x5bcc08..0x5bcc7b); `FMax` 15 times the friction (0x5bcc89). A
  hinge (0x5a7150): attach, anchor, axis, then the limit motor's
  settings (0x5a70c0); a swivel: a ball at the anchor (0x5a7250) and an
  Euler motor (0x5a7510), axis 2 the third limit's on body 2, axis 0 the
  first's on body 1; every motor axis lo, hi, speed 0, `FMax`, fudge 0.9,
  `phys_joint_cfm` 1e-4, `phys_joint_stop_cfm` 1e-4 and
  `phys_joint_stop_erp` 0.8 (registered 0x5a3d2c..0x5a3f3e). VERIFIED
  (the instructions, the registrations). A swivel with fewer than three
  limits reads stack the build never wrote for the rest (0x5bcc08);
  every stock swivel has three, and vcod reads zeros. VERIFIED (the
  instructions, the cfg). The limits are the base pose's, so a death
  anim's pose past one is pulled back: retail's max-life capture turns
  the neck 108 degrees against the upper spine (the shortest-arc twist
  from its first step). VERIFIED (the capture).
- **The first step's friction.** The build's `FMax` reaches no step:
  state 4's update in the build's own frame sets it to the friction
  times 1 before `Phys_RunToTime`. VERIFIED (the instruction order). The
  gate agrees: the 15 on the first step puts the bodies 0.012 units off
  retail's after it, against 2e-4. VERIFIED (the run, ours).
- **The placement and the velocities.** 0x5a5090 turns a body as its
  bone's snapshot (flipped for a mirrored bone) and puts its centre at
  `p0 - axes . offset`. 0x5be060, `dt` the time between the snapshots:
  `inv = (float)(1000 / dt)`; the linear velocity the bone root's move
  times `inv` (the root's, not the centre's); the earlier quaternion
  negated when its dot with the later is under 0, both normalized
  (0x5636e0), `dq = cur (x) prev*` normalized, its rotation vector
  (0x5bbb00: `xyz * 2a / sin a`, `a = acos w`, 0 when `|sin a|` is 1e-6
  or less), through 0x5bb8e0 (`(c (x) c*) (x) rv`) and times `inv *
  ragdoll_rotvel_scale`, written into the body. VERIFIED (the
  instructions, 0x5be06a..0x5be298).
- **The tunnel test** (0x5be3a0) on the latest snapshot: each bone from
  its root along its x for its length, and a bone that two or more bones
  name as parent (6 read at most) from its middle to each one's root,
  through `Ragdoll_BoneTrace` (10). A failure frees the slot, after the
  velocities. VERIFIED (0x5be3a0..0x5be5f3, 0x5be600..0x5be711).
- **The steps.** `Phys_RunToTime` (0x5a6d50) on world 2's clock
  (0xcc5461c): when the clock is behind the target, the bodies' positions
  and axes go into their phys objects (+0x10, +0x1c), then at most two
  steps, each `max(msecStep, owed / steps left)` ms (an unsigned divide),
  its seconds the msec times `0.001f` widened, until the clock reaches the
  target; nothing owed carries and no step is capped. After a frame's
  steps 0x5a6c10 checks each awake body, and the lerp
  `timeNowLerpFrac = (target - last) / (now - last)` is stored.
  VERIFIED (0x5a6d6d..0x5a6eb1). `Phys_RunFrame` (0x5a6440): the dvars
  (10.4), the hook (the self pairs of each slot in 3 or 4), each geom of
  world 2's space newest first whose body is awake into the near
  callback (0x5aa100), the world's step. VERIFIED (0x5a6458..0x5a6625).
  `Phys_PostStep` (0x5a6640): awake bodies' forces and torques cleared,
  the contacts emptied, world 2's angular damping (10.2), and the next
  step's length `17 + trunc(17 t)` for `t` the awake count less 32 over
  18 clamped to 0..1: 17 ms up to 32 awake bodies. VERIFIED
  (0x5a6665..0x5a67bb). A space collide between bodies runs only under
  `phys_interBodyCollision` (default 0). VERIFIED (0x5a65b8).
- **After a frame** (0x5a6c10, each awake body): a box sweep under the
  ragdoll mask from where the frame began to where the body is, half
  size off `r * 0.70710677` and `2 phys_csl` (0x5a6930), puts a body
  that went through the world back where it met it, its velocities
  zeroed when that is under 0.001 of the way; a body outside the world
  model's bounds is put to sleep. VERIFIED (0x5a6930..0x5a6cb9).
- **The pose** (0x5bc4f0, state 4 each frame before the steps): each def
  bone's root and turn lerped by `timeNowLerpFrac` between the last
  frame's start and end (0x5a6ec0), flipped back for a mirrored bone.
  VERIFIED. A base lerp bone (0x5bbf90) is set under its ragdoll bone
  (the cfg's fourth word, 0 when left out: the head, the wrists and the
  ankles go under the pelvis): its pose there as last drawn eased
  towards its base pose by the time in state over its msec (clamped to
  0..1), or the base pose at once past 400 units. VERIFIED (the
  instructions, 0x5bc050..0x5bc111 for the bones read). That the ease
  compounds, reading back last frame's output, is INFERRED (0x4352e0's
  skeleton). The DObj write (0x5bb580) sets those bones, and the rest
  take the anim's locals of the frame under their parents (10.7.2).
  VERIFIED (the instructions, the capture). vcod draws the rest with the
  locals fed that frame, and lerps the quaternions in `glam`. vcod's.
- **The glue.** `play::cod4_players` keeps one pool per def and clip map
  (a map load starts a new world 2, the seed back at 0x89abcdef), steps
  it once a frame before the bodies pose, feeds a corpse's death anim
  pose every frame, and draws the slot's pose once it simulates or
  holds (10.7.2). Its update runs before the frame's
  poses, so a slot snapshots the pose fed the frame before: a frame
  later than 1.7's, as far apart. vcod's.

**The gate** (`ragdoll_ab`, `COD4_DIR`; the stock def, mp_crash's clip
map, the base pose of `body_mp_usmc_assault`, whose def bone lengths
every MP body shares). Two takes (10.5): `kill-mp_crash-1.7.txt`, which
slept, and `kill-mp_crash-1.7-maxlife.txt`, which ran to
`ragdoll_max_life` and carries the bone lines, the entity at the build
and the skeleton (10.7.2); it is built under its entity on its own body
and head, the slept take at the origin. VERIFIED (the runs, ours):

| | slept | max life |
|---|---|---|
| asleep / idle, step | 79 / 80, retail's | none / 266, retail's |
| seed retail's through step | 15 | 195 |
| worst body, first step | 2e-4 units | 1.5e-5 (2e-4 at the origin) |
| worst body | 0.20 units, 4.1 deg | 0.015 to step 165; 0.31, 7.4 after |
| flags (sleep bit, timer) | retail's every step | retail's every step |
| rest root z | 239.075 (239.064) | 240.715 (240.652) |
| rest centres' spread | 33.16 x 48.04 x 3.37 (33.20 x 48.09 x 3.36) | 42.58 x 34.09 x 5.41 (42.56 x 34.23 x 5.28) |

- Each run starts from the take's start bits. The build off the max-life
  take's snapshots lands within 1 float step of retail's position, 11 of
  its quaternion, 9e-4 u/s and 5e-5 rad/s of its velocities, and on its
  mass bit for bit; built at the origin, 3 float steps and 1e-5 of the
  mass (`the_build_off_retails_snapshots`, 10.7.2). The dump's latest
  snapshot is read after state 4 saved its pose over it, so it carries
  that rounding. VERIFIED (the run, the dump's order).
- Where it parts: from the first step by about 2e-4 units built at the
  origin, through the joints' anchors and turns at rest, built in a base
  pose rounded off the model rather than under the corpse's entity; the
  max-life take built under its entity parts by 1.5e-5 (10.7.2).
  VERIFIED (the runs). The parting
  grows with each contact the two make differently: on the slept take
  the seed parts after step 15 and the bodies' speeds by 1.7 u/s on step
  16, where a step's contact rows first differ in number (the reorder
  draws once per row swapped, 10.4). VERIFIED (the runs). The max-life
  take's bodies jitter at rest over 1 rad/s and part slowly after step
  165. VERIFIED (the runs).
- At rest on mp_crash's floor retail's capsule ends sit up to 1.8 units
  under it (the unit surface layer and the soft contacts, 10.6); in
  `cod4_live_corpses_ragdoll_onto_the_floor` vcod's up to 2.6 where an
  end presses into a corner. VERIFIED (the captures against the clip
  map, the test).
- In `cod4_live_ragdolls_hold_still_once_down` 2 of 6 corpses ran to
  `ragdoll_max_life` at 4 ms frames and 3 of 6 at 100 ms, against 2 of
  retail's six takes (10.5). VERIFIED (the run).
- By eye (pending): on mp_crash or any map, kill a player standing in
  the open (`kill` with `sv_cheats 1`, or a bot) and watch the corpse
  fall and lie, at `com_maxfps 0` and `com_maxfps 20`: it should fold at
  the joints within their limits, settle without sliding, and the head,
  wrists and ankles ease towards their base pose over a second or two.

### 10.7.1 The frame glue per world

The steps above are one `Phys_*` layer that all three worlds run through,
indexed by world. I moved it out of `ragdoll::pool` into `ode::phys`
(`PhysWorld`, one per world), so worlds 0 and 1 can step on the ODE port
as world 2 does. The ragdoll pool keeps its slots and the self collision
hook and calls `PhysWorld::run_to_time` for world 2.

- **Per world.** `g_phys_msecStep` (0x71e798) is 17, 17, 17 at load, in
  `.data`, outside the 0x23cd0 bytes from 0xcc54478 that `Phys_Init`
  clears through 0x67c4a0 (0x5a2cf6); its floor (0x71e7a4) is 17, 17, 17
  and its ceiling (0x71e7b0) 67, 67, 34. VERIFIED (the tables, the
  call's arguments). That 0x67c4a0 is `memset` is INFERRED. `Phys_PostStep` indexes
  both by its world (`[ebx*4+0x71e7a4]`, `[ebx*4+0x71e7b0]` at 0x5a6791,
  0x5a679c), so worlds 0 and 1 step 17 ms up to 32 awake bodies, 42 at
  41 and 67 from 50. VERIFIED (the instructions). `Phys_Init`'s loop
  (0x5a2d1f..0x5a2d9b) gives every world the sleep on for new bodies
  (`dxWorld` +0x4c = 1) and 0 idle steps (+0x48), and zeroes the clock's
  start and time (the block's +0, +4). VERIFIED (the instructions). The
  lerp (+8) starts at 0 through the clear; the port's does too. INFERRED
  (the clear's identity).
- **The world's dvars.** `Phys_RunFrame` stores the step's seconds at the
  world's block +0x50 (0x5a6458) and the contacts' most correcting
  velocity off `phys_mcv` (0xcc78160, 20) for worlds 0 and 1 and
  `phys_mcv_ragdoll` (0xcc54470, 1000) for world 2 (the `cmp [esp+0x20],2`
  at 0x5a64bc). Its world hook is the pointer at the block's +0x90
  (0x5a65e6), which `Phys_Init` clears (0x5a2d86). VERIFIED (the
  instructions). That only the ragdoll code sets a hook is INFERRED.
- **The drag arm** (`Phys_DoBodyOncePerFrame` 0x5a62f0, called from
  `Phys_PostStep` at 0x5a6720 with the world and the block's +0x50): a
  disabled body returns 0 and is not counted. Worlds other than 2 scale
  the linear velocity by `max(1 - phys_dragLinear dt, 0)` and the
  angular by `max(1 - phys_dragAngular dt, 0)`, each factor stored and
  floored through 0x4093c0 (`b` when `a - b` stored is under 0, else
  `a`), each product stored (0x5a6310..0x5a63cf). World 2 takes the
  damping of 10.2 (0x5a63d0..0x5a6429). VERIFIED (the instructions).
  `phys_dragLinear` is the dvar at 0xcc54414, registered 0.03 (the float
  at 0x70bc24, name 0x6f7c90, 0x5a3fe0..0x5a40a0), `phys_dragAngular`
  0xcc54464, 0.5 (0x6be648, name 0x6f7cd0, 0x5a4091..0x5a4151). VERIFIED
  (the registrations).
- **The sweep size per class.** 0x5a6930 sizes its box off 0x5a9a20 on
  the body's first geom, which switches on the class through the byte
  table at 0x5a9b08: class 1 (box) takes half its least side (0x5a9a44,
  the double 0.5 at 0x70ace8), class 6 (transform) its inner geom
  (0x5aed10), classes 11..14 go to 0x5a67d0, the rest leave the box
  unset. VERIFIED (the tables). 0x5a67d0's arms (the table at 0x5a6918):
  11 and 12 scale the geom's bounds by the double at 0x70afb8 and go on
  to 0x561910, 13 (cylinder) takes `min(r * 0.70710677, +0x54)`
  (0x5a688b), 14 (capsule) `r * 0.70710677` (the double at 0x70ae70)
  unless `2 r + h` (the double 2.0 at 0x70ae50, `h` at +0x54) is less
  (0x5a68b7). The box is `-m..m` on each axis for 1, 13 and 14.
  VERIFIED (the instructions). The port has the box's, the cylinder's,
  the capsule's, the transform's and class 12's arms (`Geom::bounds`,
  cod4-gsc.md 3.6.4), and every class its collider (cod4-gsc.md 3.6.7).
- **The sweep box.** With `m` the box's top x: `m` under `2 phys_csl`
  (the sum stored) sweeps a point (corners -0.001 and 0.001, 0x70b460,
  0x70b278); else the box at 0.2 of the bounds (the double at 0x70b3e8,
  each corner stored), unless `m` less that box's top x, unstored, is
  under `2 phys_csl`, when it is `max(m - 2 phys_csl, 0.001)` each way
  (0x5a6a0a..0x5a6a4c). A point sweep's fraction loses `0.5 m` times
  0x403a90's inverse root of the move's squared length, computed as
  `(x / 2)` stored, then `y (1.5 - (x / 2) y y)` on the stack, stored
  (0x403a90..0x403acc), floored at 0 by 0x4093c0. The sweep runs when
  the body moved or the object's state is 0 or 1 (0x5a6a79..0x5a6ab2).
  VERIFIED (the instructions). I ported those three points as read: the
  constants as their widened floats, the unstored compare and the
  stack's inverse root. On both captures of 10.5 every body word on
  every step came out bit for bit as before the move. VERIFIED (the
  runs, ours).
- **`Phys_ObjAddForce`** (0x5a5900, the world in `esi`, the body as its
  argument, the force in `ecx`, the point in `edx`): `1000 / msecStep`
  of that world (`fild [esi*4+0x71e798]`, `fdivr` the double at
  0x70af28) stored, each force word times it stored, z first, then
  `dBodyAddForceAtPos` and the wake of cod4-gsc.md 3.6. VERIFIED (the
  instructions). `dBodyAddForceAtPos` (0x5b7000) adds each force word to
  +0x98..+0xa0 and `(p - x) x f` to the torque +0xa8..+0xb0, each
  component `(q1 f2 - q2 f1) + t0` and kin on the stack, stored once.
  VERIFIED (0x5b7000..0x5b70b7). That this is upstream ODE 0.5's
  `dBodyAddForceAtPos` with `dCROSS(tacc, +=, q, f)` is INFERRED (the
  upstream source); `Body::add_force_at_pos` ports it.
- **The blast's point.** `Ragdoll_ExplosionEvent` passes 0x5a5900 the
  bone's jittered centre: the body's position (+0xe0) copied to
  `[esp+0x44]`, each axis's `flrand` draw added and stored
  (0x5bddcc..0x5bde90), the pointer in `edx` at 0x5bdfab. VERIFIED (the
  instructions). 10.1 has the force at the centre; with the move the
  port pushes at the jittered point, so a blast now spins a bone by `j x
  F`, `j` the jitter. vcod's. How a thrown corpse looks is pending a
  hand check.
- **`Phys_ObjBulletImpact`** (0x5a5a20; arguments world, body, speed,
  scale on the stack, the direction in `eax`, the contact in `ecx`): the
  formula of cod4-gsc.md 3.6 at the binary's store points. 0x5a5990
  adds `phys_bulletUpBias` (0xcc54408) to the direction's z, stored,
  normalizes it (0x4076d0), and moves the contact out by `(p - x) *
  phys_bulletSpinScale` (0xcc54434), each product and sum stored
  (0x5a5990..0x5a5a1b). Then `v . dir` stored, `speed` less it stored,
  `r = p - x` stored, `n = r x dir` stored and normalized with its length
  `L` (0x5a5b5d); the body's `dMass` (17 words from +0x20, 0x5a5ab4) and
  rotation (+0x100) copied; `u = R n` stored, `I u` stored, `w . n`
  stored; the closing speed `cl - (w . n) L` stored; at or below 0 it
  returns (0x5a5d0a; a NaN goes on). `den = m + 0.5` stored, and when
  `L` is not 0 (0x5a5d21) `den + (L L m / 2) / (u . I u)` with `u . I u`
  stored; `J = cl m scale / den` stored, `J dir` stored, to 0x5a5900 at
  the moved point. VERIFIED (0x5a5a20..0x5a5db4). `phys::obj_bullet_impact`
  ports it; worlds 0 and 1 call it since they moved onto the port
  (cod4-gsc.md 3.6). vcod's.
- **0x5a2320's cut** moved with it into `ode::collide::reduce`, which the
  near callback calls for every world. vcod's.
- **Gates**: `ragdoll_ab` (unchanged), and the unit tests
  `ode::tests::each_worlds_msec_step_follows_its_awake_count`,
  `the_drag_arm_is_worlds_zero_and_ones_and_the_damping_world_twos`,
  `a_push_lands_as_a_force_over_one_step_at_its_point` and
  `a_bullet_through_the_centre_pushes_without_spin`. VERIFIED (the runs,
  ours).

### 10.7.2 The base pose under the entity, and the bones the ragdoll does not set

Read off iw3mp.exe 1.7 with `objdump` and measured on a retake of the
max-life take, `crates/cod4/tests/fixtures/ragdoll/kill-mp_crash-1.7-maxlife.txt`
(`RAGDOLL_SKEL=1`, docs/dev/retail.md, "A ragdoll from spawn to rest"):
an opfor rifleman (`body_mp_arab_regular_assault`, 54 bones, and
`head_mp_arab_regular_suren`, 62), killed as in 10.5, built on frame 584
and idle at `ragdoll_max_life` on frame 848. Every world 2 word of the
retake, its 3738 `world`, `start`, `bone`, `step`, `slot` and `body`
lines, repeats the 2026-11-26 take's but for the clocks. VERIFIED (the
two dumps).

- **Whose base pose.** 0x5bc860 takes the slot's pose holder through
  0x5ba030: the slot's own (+0x10) when it has one, else
  `0x84f2d8 + 0x1dc * entnum` (+0x8), the corpse's entity. 0x5bbe10
  turns its angles (+0x28) into axes (0x5635c0), and 0x5bbd50 puts the
  model's base pose of the bone (0x57f480) under those axes and its
  origin (+0x1c), 0x5bc901..0x5bc930. VERIFIED (the instructions). The
  take's `ent` line, read at 0x5bc930: origin (232, 2285, 236), angles 0.
  VERIFIED (the capture). Built there the port's bodies are 1.5e-5 units
  off retail's after the first step and 0.0015 through step 50, and the
  build's masses are retail's bit for bit; built at the origin, as the
  gate built it before, 2e-4, 0.0041 and 1e-5 of the mass. The first
  step's parting of 10.7 was the base pose's place. VERIFIED (the runs,
  ours: `ragdoll_ab::the_max_life_capture_against_the_port`,
  `the_build_off_retails_snapshots`). The client builds under the entity
  as drawn; the gate under the take's `ent`, and the slept take, which
  has none, at the origin.
- **The entity follows the root.** `CG_CalcEntityLerpPositions`'s tail
  (0x4350fb..0x43511a) calls 0x434fa0 when `ragdoll_enable` (0xcc789f8,
  registered with the name at 0x6f8464, 0x5ba36b) is on and `pos.trType`
  reads 9 to 11. 0x434fa0 makes the slot when the pose holds none
  (0x434f20, `Ragdoll_CreateRagdollForDObj` at 0x434f7f, the handle into
  +0x10 or, for the killcam's, +0x14) and then 0x5ba060 copies, from slot
  state 3 on, the latest snapshot's first bone's position (`j_mainroot`)
  into the entity's origin (+0x1c). VERIFIED (the instructions). The
  root bones of model 0 take that origin and the entity's axes
  (0x4468b0, from 0x446d40: 0x446914..0x446a2b). VERIFIED (the
  instructions). In the take `tag_origin` sits on `j_mainroot` every
  drawn frame after the build. VERIFIED (the capture). vcod (2026-11-27)
  moves the corpse's transform onto `Ragdolls::root` (the drawn pose's
  first def bone) while the slot draws, from state 4 where 1.7 moves it
  from state 3 (the build frame draws the death anim). What reads the
  entity's origin on the client: the model's transform (the skins are
  taken in it and drawn under it, so the move cancels in the world pose),
  the dynamic pass's LOD distance (`lod_origin`, the transform's
  translation), the light grid sample (`BodyFrame::light_origin`, the
  origin raised by the torso height and the stance) and the slot's
  `DObj::origin` (the idle exit's rebuild, whose bodies `place` then sets
  from the snapshot); all four take the root now. The view cull and the
  light boxes pose the skins' own box, the names read the head tag a
  corpse sets none of, and no attached model, sound or tag follows a
  corpse (`corpse_character` strips the attachments, a corpse fires no
  tag), so none of those read it. Nothing is skinned to `tag_origin`.
  INFERRED (the readers read in `cod4_client.rs` and `cod4_players.rs`,
  not run against retail's).
- **Each drawn frame's pose** (0x4352e0): 0x57de10, the controllers
  (0x446d40), then 0x580190. The controllers end with the ragdoll's DObj
  write (0x5bb580, called at 0x446de5 when the pose's +0xc is set and a
  handle is): from slot state 3, each def bone's and base lerp bone's
  latest snapshot pose, its position less the vector at 0x797648, each
  marked in the DObj's bits at +0x14 and +0x34 (0x5bb633..0x5bb718,
  0x5bb740..0x5bb82e). 0x580190 then runs `DObjCalcAnim` (0x5875f0) and,
  model by model, `DObjCalcSkel` (0x57fbd0) on the bones not yet in
  +0x34 (0x58019f..0x580325): every other bone is the anim's local this
  frame under its parent as posed, a ragdoll bone or not. VERIFIED (the
  instructions; 11 for the mask `DObjCalcAnim` reads). The DObj's bones
  are its models' `boneNames` in order, model by model (0x57f2b0).
  VERIFIED (the instructions, the take's `models` line). Nothing on the
  corpse's draw (0x445510, its anim call at 0x445586) reads the ragdoll
  state, so the death anim plays on under it. INFERRED (no branch on it
  in 0x445510..0x445633).
- **The take.** The skeleton after each drawn frame's last pose (the
  renderer's call at 0x642a35): the bones the slot does not set move
  under their parents for 20 frames after the build, `j_ball_ri` 26.8
  degrees, `j_coatfront_ri` 8.9, `j_wristtwist_le` 6.2,
  `torso_stabilizer` 6.0, and none moves from frame 605 to the end: the
  death anim ended under the ragdoll 340 ms after its build. This death
  anim keys the fingers still. VERIFIED (the capture). On an earlier take
  the player respawned on frame 607 (`scr_player_forcerespawn` 1) and
  the renderer stopped posing the corpse once the view left it; the
  committed take sets it to 0. VERIFIED (the captures).
- **vcod** (2026-11-27). `play::cod4_players` feeds the corpse's death
  anim pose every frame, not only until the slot simulates, and
  `Ragdolls::feed` sets each bone the slot does not set under the slot's
  pose with the fed locals at once, so the frame's anim is the frame's
  draw. Until then the client froze those bones at the build's locals:
  `j_ball_ri` would be drawn 26.8 degrees off retail's once its anim
  ended. The pose is the anim's only; its skins stay undrawn
  (cod4-client-perf.md 31). vcod's.
- **Gates.** `ragdoll_ab::the_unset_bones_take_the_frames_locals_under_the_ragdoll`
  builds the take under its entity, steps it as retail stepped it, feeds
  each drawn frame's skeleton as the DObj and holds every unset bone
  under its parent within 1e-3 units and 1e-3 degrees of retail's that
  frame (measured 1.2e-4, 2e-5); it also holds retail's 20 frames and
  26.8 degrees. `ragdoll::tests::pool::an_unset_bone_takes_the_locals_fed_that_frame`
  holds the feed's at once. VERIFIED (the runs, ours). How a corpse's
  feet, coat, hands and head look as it falls is pending a human's look.

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
