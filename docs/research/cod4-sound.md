# CoD4 1.7: sound aliases, loaded sounds, the event sounds

How a 1.7 client finds and plays a sound: the alias lists in the zones,
the samples they point at, the channels, and the alias names the cgame
plays for each entity event. Code: `crates/cod4/src/sound.rs` (the
aliases, `channels.def`, the `.wav` wrap), `crates/client/src/cod4_audio.rs`
(the client's alias table, the event arms); gates:
`crates/cod4/tests/sound_census.rs`, the replay gate in
`crates/client/src/play/cod4.rs` (`spectator_capture_plays_its_sounds_and_effects`,
`kill_captures_play_their_hits`) and `cod4_audio`'s decode test.

## Sources

- `iw3mp.exe` 1.7 (image base 0x400000, `.rdata` at file offset
  0x291000), its strings and the code that references them, found by
  scanning `.text` for each string's address. Asset layouts are
  `cod4-fastfile.md` 10's.
- The zones of a 1.7 install, walked with their bulk data
  (`COD4_DIR`).
- A claim labelled INFERRED that names no 1.7 address is not yet
  confirmed on 1.7.
- The captures of `cod4-netfields.md` 8 and 10.

## 1. Where the aliases and samples live

- A client loading an MP map holds its sound aliases in
  `localized_common_mp` (2769 lists: footsteps, weapons, impacts, the
  voices) and the map's own zone (mp_crash: 149 lists, its ambient and
  emitters among them); `common_mp`, `code_post_gfx_mp` and `ui_mp`
  hold none and `localized_code_post_gfx_mp` 15. VERIFIED (the walk).
- Across `code_post_gfx_mp`, `localized_code_post_gfx_mp`,
  `localized_common_mp` and `mp_crash` there are 2930 lists, 5723 rows
  on loaded sounds and 2734 on streamed files. VERIFIED (the census).
- A loaded sound (`LoadedSound`, 0x2C) keeps its samples in the zone with
  no file header: `data_len` is `samples * block_size`. Every one of the
  749 carrying data in those zones is `format` 1 (PCM), 16-bit, mono or
  stereo. VERIFIED (the census wraps each, the decode test decodes
  each).
- A loaded sound whose name starts with `,` carries no data (format,
  rate and length 0): it names one an earlier zone loaded (the map
  zone's `,weapons/grenade/mrk_grenade_2.wav`). VERIFIED (the walk);
  that the engine resolves it by name is INFERRED.
- A streamed row names a directory and a file; the file is
  `sound/<dir>/<file>` in the iwds (`iw_07`..`iw_11`, the
  `localized_english_iw*` voices), `.wav` or `.mp3`. Every streamed row
  of those zones finds its file. VERIFIED (the census).
- vcod wraps each loaded sound as a RIFF `.wav` and puts it on the
  search path's overlay at `sound/<name>`, where an alias row's file
  reads; the audio system's bank decodes all 749 and the mp_crash
  ambient's mp3. VERIFIED (`cod4_audio`'s decode test).

## 2. `snd_alias_t` (0x5C)

Name 0, subtitle 4, secondary 8, chain 12, sound file 16, sequence 20,
volume min and max 24, 28, pitch 32, 36, distance min and max 40, 44,
flags 48, slave percentage 52, probability 56, LFE 60, centre 64,
start delay 68, falloff curve 72, envelope 76..84, speaker map 88.
VERIFIED for the slots the walk follows (16, 72, 88, `cod4-fastfile.md`
10); the float fields' order is INFERRED and fits the values
(`step_run_asphalt`: volume 0.288..0.342, pitch 0.85..1.05, distance
50..1200; VERIFIED).

The flags, read off the stock rows:

| Bits | Meaning | Evidence |
|---|---|---|
| 0x1 | looping | every `emt_*` and `ambient_*` row has it, no footstep or shot does. VERIFIED (the rows); the meaning is INFERRED |
| 0x2, 0x4 | master, slave | INFERRED |
| 0x8, 0x10 | full dry level, no wet level | INFERRED |
| 0xc0 | file type: 0x40 loaded, 0x80 streamed | every row's bits match its `SoundFile.type`. VERIFIED (the census) |
| 0x3f00 | channel, a row of `channels.def` | the footsteps' 0xb is `body`, the shots' 0x12 `weapon`, `ambient_crash`'s 0x18 `ambient`. VERIFIED (the rows against section 3) |

The falloff: `SND_Attenuate` gives 1 inside `distMin`, 0 at or past
`distMax`, and between them the curve's value at
`(dist - distMin) / (distMax - distMin)`. INFERRED. vcod
plays every row along its zone curve so (`VfCurve::attenuate`). Of the
8457 rows across the mp_crash zones, 2994 carry the straight two-knot
curve, the rest one of six bent ones, and 139 a curve handle no walked
zone holds, which vcod plays straight. VERIFIED (the census
`sound_census::mp_crash_falloff_and_speaker_fields`). The envelope
fields (76..84, `envelopMin`, `envelopMax`, `envelopPercentage`) are read
by the loaders only, never by the mixer. INFERRED;
vcod reads them nowhere.

The speaker map (`SpeakerMap`, 0x198): `isDefault` at 0, its name at 4,
then `channelMaps[2][2]` at 8, each `MSSChannelMap` 0x64 (`speakerCount`,
six `{speaker, numLevels, levels[2]}` of 0x10), indexed by [source is
stereo][output is multichannel]. INFERRED; the
values fit: every map's stereo-source, stereo-output row is speaker 0
at (1, 0) and speaker 1 at (0, 1) or half that. VERIFIED (the census).
`SND_ApplyChannelMap` sets a 2D voice's levels from it, each speaker at
the larger of its two; a 3D voice takes the LFE and centre sends
instead, and only on a multichannel output (`SND_Apply3DSpatializationTweaks`).
INFERRED. In the mp_crash zones the maps are the default
(7699 rows), `radio` (641), `ambience` (47), `music` (36), `amb_front`
and `amb_rear` (17 each); on a stereo output every one sends a mono
source 0.5 to each front speaker and a stereo one whole, except
`amb_rear`, which halves the stereo source; its rows are the maps'
2D `_sur` ambients (`ambient_crash_sur`). 588 rows carry an LFE send and
895 a centre one. VERIFIED (the census). vcod mixes to stereo: a 2D row
plays at its map's stereo level against the default's
(`cod4_audio::stereo_speaker_gain`, gate
`a_speaker_map_scales_a_2d_stereo_row`), so the `_sur` rows play at
half, and the LFE and centre sends do nothing, as on a stereo retail
output. VERIFIED (ours).

## 3. The channels

`soundaliases/channels.def` is a rawfile in `code_post_gfx_mp`: 33 rows
of `name, priority, is3d, restricted, pausable, maxvoices` under `#`
comments, `physics` first, `shellshock` last; a row's index is the
flags' channel. VERIFIED (the zone). `iw3mp.exe` references the name
`soundaliases/channels.def` (0x6f2d1c) from 0x565ad4 and eleven other
sites. VERIFIED.

The fourth column is `restricted` (the default) or `unrestricted`; the
file's own header reads it as "whether channel subject one sound per
channel per entity rule". `auto`, `auto2d`, `menu`, `physics`, the
bullet, `element` and `vehicle` rows are unrestricted, the rest
restricted. VERIFIED (the zone). The promotion's `mp_level_up`,
`MP_hit_alert`, `mp_killstreak_jet` and `mp_challenge_complete` ride
`auto2d` (channel 7). VERIFIED (their alias flags).

vcod plays a 2D row (`is3d` blank or `2d`) flat on its `Local`, `Menu`,
`Music`, `Announcer` or `Shellshock` channel by name, an unrestricted 2D
row on `Auto2d`, which never replaces a voice, and a 3D one on `Weapon`,
`Voice`, `Item`, `Body` or `Auto`. Before `Auto2d`, every 2D row on our
own entity shared `Local`, so the next kill's hit alert cut the
promotion's sting a frame after it began (gate
`an_unrestricted_2d_cue_does_not_cut_the_last`). The mapping is vcod's;
retail's per-channel voice limits and priorities are not modelled.

## 4. The event sounds

`CG_RegisterSounds` sits at 0x43e140 in `iw3mp.exe`: the references to
`land_damage` (0x43e147), `grenade_explode` (0x43e14e), `bullet_small`
(0x43e171), `step_run` (0x43e207), `step_run_plr` (0x43e216), `land`
(0x43e264), `land_plr` (0x43e273) and `gear_rattle_run` (0x43e2fc) run
in that order inside one function. VERIFIED. A per-surface name is
`<prefix>_<surface>` (`%s_%s` at 0x6c2518, `%s_default` at 0x6c4d30
referenced from 0x43df04). VERIFIED (the strings and the reference);
that the registration loops over the 29 surfaces is INFERRED.

The surface names are `infoParms`' (0x71fbbc, 20-byte rows of name,
then the surface bits): `bark` carries 0x100000, `asphalt` 0x1600000
(22), `paintedmetal` 0x1c00000 (28), so surface `n` is `n << 20`.
VERIFIED. The 29 in order are `default`, `bark` .. `wood`, `asphalt`,
`ceramic`, `plastic`, `rubber`, `cushion`, `fruit`, `paintedmetal`, the
order of `EV_LANDING_*` (`cod4-netfields.md` 9). VERIFIED.

What each event plays follows. The arms are `CG_EntityEvent`'s, INFERRED. That every name below is a stock alias is VERIFIED (the
replay gate resolves each it meets).

- `EV_FOOTSTEP_RUN` and `EV_JUMP`: `step_run_<surface>` on the entity,
  the surface the event's parm, and `gear_rattle_run`; `_SPRINT`,
  `_WALK` and `_PRONE` likewise (`gear_rattle_walk` for the last two).
  The capture's parms are 17 (rock) and 22 (asphalt) and play
  `step_run_rock` and `step_run_asphalt`. VERIFIED (the replay gate).
- `EV_LANDING_<surface>`: `land_<surface>`; the pain ones add
  `land_damage`.
- Dead Silence: `CG_EntityEvent` reads the entity's `clientNum` (0 when
  outside 0..63, 0x435dc7..0x435dce) and adds 29 to the surface index
  when that client's `clientinfo.perks` carries bit 8 (0x435de9..0x435dfc);
  `CG_RegisterSounds` fills those slots from `qstep_sprint`,
  `qstep_run`, `qstep_walk`, `qstep_prone` (and `_plr`), `qland` and
  `qland_plr` (0x43e370..0x43e3fc), and the gear from
  `qgear_rattle_<kind>` (0x43e425..0x43e4dc), which the gear arm (0x435980)
  picks by the same flag. `land_damage` has no quiet one. VERIFIED (the
  code). vcod reads the snapshot's client states (`Fired::perks`); live
  on vcod-server our own steps with `offline_class2_mp` play
  `qstep_run_plr_rock` and `qgear_rattle_plr_run` and the enemy's own
  plain ones. VERIFIED (the gate
  `cod4_live_dead_silence_steps_quiet_and_bomb_squad_shows_the_trigger`).
- The player's own view (the playerstate's ring, vcod's rule; retail
  tests `otherFlags` and the client number) plays the `_plr` names:
  `step_run_plr_<surface>`, `land_plr_<surface>`,
  `gear_rattle_plr_run`, a weapon's `*SoundPlayer`.
- `EV_FIRE_WEAPON` / `_LASTSHOT` / `_MG42`: the weapon's `fireSound`
  (`fireSoundPlayer` for the player, `lastShotSound*` on the last
  shot): `weap_m16_fire_npc` for the walker's shots. VERIFIED (the
  replay gate).
- The reload, raise, putaway, pullback, rechamber, melee, empty and
  detonate events: the weapon's matching slot; pickups the picked
  weapon's (`eventParm`).
- `EV_MELEE_HIT` (a swing's contact with a body, on its event entity):
  `melee_knife_hit_body` on the struck entity (`otherEntityNum`) when
  the parm carries the knife bit, else the weapon's `meleeHitSound`,
  else `melee_hit`; `EV_MELEE_MISS` (the world) `melee_knife_hit_other`,
  `meleeMissSound` or `melee_hit_other` at the point. The four names
  are `CG_RegisterSounds`' strings at 0x6cba98, 0x6cbaa4, 0x6cbab4 and
  0x6cbacc in `iw3mp.exe`, and the zones' aliases. VERIFIED (the strings,
  `cod4_audio::tests::a_knife_contact_plays_its_hit_sound_on_the_body`);
  the arms are INFERRED. Of `common_mp`'s weapons with an
  `meleeDamage`, 101 carry the knife model (the M16 among them) and 10
  do not. VERIFIED (`weapon_dump`). vcod-server sends the bit as the
  parm: a live knife swing at a `cod4_probe` target sent `EV_MELEE_HIT`
  parm 1, `otherEntityNum` 0 (cod4-combat.md 10.6's recipe). VERIFIED
  live. Before this vcod played only the swipe.
- `EV_BULLET_HIT`: by the weapon's `impactType` (`WeaponDef` +312, 1
  small, 2 large, 3 AP, 4 shotgun, 5 grenade bounce, 6 grenade explode,
  7 rocket explode, 8 dud): `bullet_small_<surface>`,
  `bullet_large_...`, `bullet_ap_...`, `bulletspray_small_...`, the
  `_exit` ones under impact flag 4 (`un1`). The offset is INFERRED
  and fits every MP weapon: pistols and SMGs 1, rifles 2, the
  Barrett 3, the frag 6. VERIFIED (`fx_census`).
- `EV_BULLET_HIT_CLIENT_SMALL` / `_LARGE`: `bullet_small_<surface>` /
  `bullet_large_<surface>` at the hit: `bullet_large_flesh` in the kill
  captures. VERIFIED (the replay gate).
- `EV_GRENADE_BOUNCE`: the weapon's `bounceSound[surface]`
  (`grenade_bounce_asphalt`, `_concrete`); `EV_GRENADE_EXPLODE` and the
  rocket and custom explodes the impact type's sound
  (`grenade_explode_concrete`) and `projExplosionSound`. VERIFIED (the
  replay gate's names).
- `EV_SOUND_ALIAS` and `_AS_MASTER` (a script `playSound`,
  `playSoundAsMaster`) play configstring 1342 + the parm on the entity,
  the second through `SND_PlaySoundAliasAsMaster`, whose voice is a
  master whatever its row's flags say (`treatAsMaster`), so it ducks the
  slaves while it lasts; an entity's `loopSound` plays 1342 + it at its
  drawn origin every frame (`CG_AddEntityLoopSound`), a brush model's
  (`solid` 0xffffff) at that plus the middle of its `GfxBrushModel`'s
  bounds, unturned; and the map's createfx sounds play on sound entities
  1024 on (cod4-effects.md 11). INFERRED. Every voice on an entity keeps the
  offset its start had from it, in the entity's axes, and follows it
  (`SND_SetChannelStartInfo`, `SND_GetCurrent3DPosition`). INFERRED. 1.7's server has `playsoundasmaster` (both binaries'
  strings) and the stock `_utility::play_sound_in_space` and
  `_hardpoints::playSoundinSpace` call it with their `master` argument.
  VERIFIED (the strings, `common_mp`'s rawfiles).
- `EV_STOPSOUNDS` stops every voice on its entity (`CG_StopSoundsOnEnt`,
  `SND_StopSoundsOnEnt`), loops included; a loop the entity still names
  starts again on the next frame's refresh. INFERRED. Nothing
  in 1.7 MP sends it: the name `EV_STOPSOUNDS` is in `iw3mp.exe`'s
  event names (0x6bfd78) but neither `iw3mp.exe` nor `cod4_lnxded`
  holds a `stopsounds` method name, and the server's method table
  (`cod4_names.rs`) lists none. VERIFIED (the strings). vcod plays all
  three arms so (`AudioSystem::play_as_master`, `stop_entity`, the
  loop's `Source::Tag` under CoD4's table, `cod4_audio::loop_origin`
  with the clip map's submodel bounds for the brush model's; gates
  `audio::tests::cod4_stops_an_entity_plays_a_master_and_offsets_a_loop`,
  `cod4_audio::tests::a_brush_models_loop_plays_at_its_middle`). VERIFIED
  (ours). The clip bounds stand in for the render bounds; no stock map
  script plays a loop on a brush model (`common_mp`'s and the maps'
  rawfiles). VERIFIED.
- `CG_FireWeapon` starts the fire sound at the shooter's `tag_flash`:
  the view weapon's for the player's view, else the drawn body's (or the
  gun's), its trajectory's point without one. INFERRED. vcod:
  the drawn tags `play::cod4_players` and the guns keep, our view
  weapon's for our own ring (`cod4_audio::place`), held at that offset.
  VERIFIED (ours, `events_place_their_voices_as_the_cgame_does`).
- The playerstate's own ring plays on the drawn client's entity. vcod
  played it on entity 0, so a client in any other slot heard its own
  steps and shots follow player 0; `cod4_audio::place` moves them to the
  playerstate's `clientNum`. VERIFIED (ours, the same gate). vcod hands the loops to the audio system's loop
  voices (`cod4_audio::loop_sounds`); the gate
  `cod4_audio::tests::loop_sounds_and_sound_aliases_read_their_configstrings`
  holds the reads. mp_crash's captures send no sound alias configstring
  and its createfx file places no sound, so nothing stock plays them.
  VERIFIED (cod4-handshake.md 5, cod4-effects.md 11).
- A client-side bullet's impact sound and `whizby`: cod4-effects.md 12.
- The footstep's surface is the ground trace's `surfaceFlags` bits
  20-24, 0 (silent) under 0x2000 (`PM_GroundSurfaceType`;
  INFERRED). Every footstep of the walk, spectator and kill-target
  captures but one names the surface a trace down from the event's
  origin meets in vcod's clip map (17 `rock`, 22 `asphalt`); the one
  reads caulk at a curb's edge. VERIFIED (the gate
  `play::cod4::tests::footstep_surfaces_match_the_clip_map`). On a
  64-unit grid over mp_crash every floor's surface type is its clip
  material's: `me_floor_concrete` and `me_stonefloor01` 5, `ch_tile_floor03`
  16, `me_woodplanks01` 21, and the trash-strewn interiors'
  `me_trash01` 6 (`dirt`), which plays `step_run_dirt`, while `caulk`
  floors are 0 and silent. VERIFIED (a scratch survey over the clip
  map's 0x48-byte material rows).
- Configstring 821 (`n\ambient_crash\t\0`) names the map's ambient
  (`cod4-handshake.md` 5); `ambient_crash` is a looping streamed row on
  the `ambient` channel with a rear-speaker secondary,
  `ambient_crash_sur`. VERIFIED (the zone).

- The master volume is vcod's `mss_volume`: a CoD4 shell registers no
  `snd_volume`, and the frame once picked the cvar by the search path,
  which reads as CoD2's for `.iwd` files, so the master read 0 and every
  voice was silent. The gate
  `cod4_audio::tests::kill_capture_sounds_reach_the_mixer_audible`
  follows the kill capture's ambient and shots to the mixer with a gain
  and non-silent decoded frames. VERIFIED (the gate).

- The player's view plays a weapon arm's `*Player` field alone: an
  empty one plays nothing, with no fall back to the field for others
  (EV_RELOAD, 0x43625b: `reloadSoundPlayer` +0x1b4 or `reloadSound`
  +0x1b0 by the player-view flag; EV_NOAMMO, 0x43633b). VERIFIED (the
  code); the other plain arms are shaped alike, INFERRED. The fire arms
  differ: `fireSoundPlayer` when set, else `fireSound`, and the last
  shot's `lastShotSoundPlayer`, else `lastShotSound`. INFERRED (`CG_FireWeapon`). A stock gun's `reloadSoundPlayer` is empty and its
  `reloadSound` the whole NPC reload (`m16_mp`: `weap_m16_reload_npc`).
  VERIFIED (the zone). vcod fell back to it and played the NPC reload
  whole at the reload's start: the reload sounded early.
- The player's reload sounds through `notetrackSoundMap` (`WeaponDef`
  +232, 16 key script strings, and +264, 16 values):
  The notetrack step (0x454ae0) takes the view model's notetracks
  the frame's anim update crossed, skips `end`, plays the night vision's
  sounds on `NVG_on_powerup` and `NVG_off_powerdown`, and
  The notetrack sound step (0x454a70) plays each value whose key is
  the note's lower case as a client sound. VERIFIED (the code). `m16_mp`
  maps `weap_m16_lift_plr`, `_clipout_plr`, `_clipin_plr`,
  `_chamber_close_plr` and `melee_knife_stab` each to itself, and
  `viewmodel_m16_reload` (60 frames at 30) carries `weap_m16_clipout_plr`
  at 0.2 and `weap_m16_clipin_plr` at 0.633, so at its 2 s `reloadTime`
  the clip comes out 0.4 s in and goes in at 1.25 s. VERIFIED (the
  zone). vcod: `cod4_viewmodel::Cod4ViewFrame::sounds` over the main
  clip (the sight layers carry no notes), gate
  `an_m16_reload_sounds_its_clip_out_and_in_off_its_notetracks`.
- `playLocalSound` sends `s <index>`, and `stopLocalSound` `k <index>`:
  `CG_LocalSound` (0x44bc70, the `s` of `CG_DeployServerCommand`'s table
  at 0x44c530) wants exactly two arguments and an index in 1..256, and
  plays configstring 1342 + index on the drawn playerstate's client at
  its origin (0x44bcc1..0x44bcde); `k` (0x44bd10) stops it. VERIFIED (the
  code). Retail's prematch sends `d 1343 mp_spawn_usa` and `s 1`,
  `US_1mc_team_deathmtch` as `s 2` and `US_1mc_boost` as `s 3`: the
  spawn music and the leader dialog. VERIFIED (`play-1.7-prematch.txt`).
  vcod dropped them, so the announcer never spoke; it plays them as
  local voices (`Cod4Hud::take_local_sounds`), gate
  `prematch_leader_dialog_plays_as_local_sounds`.

- The master volume: `SND_Init` registers `snd_volume` (string
  0x6fa0f4, "Game sound master volume") through `Dvar_RegisterFloat`
  (0x56c350, called at 0x5c6c3e) at 0.8 (the float at 0x6cdd4c), range
  0 to 1, flags 1 (archived), and stores its dvar at 0xcc8e244. The
  master gain at 0xcc92274 is its value times the double 0.75 at
  0x70b0c8 (0x5c6ff0..0x5c7001 at start-up), and on each change times
  the master fade at 0xcc92278 as well (0x5c4fa4..0x5c4fbc). VERIFIED
  (the disassembly and the bytes; the gate
  `sound_census::master_volume_is_snd_volume_times_three_quarters` reads
  them off the install's `iw3mp.exe`). So 1.7's scale is CoD2's 0.75 and
  its default master 0.8, which vcod's shell gives `mss_volume`: a voice
  plays at `0.8 * 0.75` of its row's volume by default, as on retail.
  vcod names the scale `audio::COD4_VOLUME_SCALE` (gate
  `audio::tests::a_cod4_table_plays_at_the_1_7_master_scale`); the
  master fade (`soundFade`) is CoD2's. VERIFIED (the tests, ours).
- Looping voices live only while refreshed: `SND_UpdateLoopingSounds`
  stops every 3D, 2D and stream channel whose alias is looping (flag 1)
  and whose `looptime` the frame did not renew, and only
  `SND_ContinueLoopingSound` (an entity's `loopSound`, the ambient)
  renews it; a looping channel a one-shot path started plays one frame.
  A continued loop starts its looping secondary beside it on each
  refresh, so the secondary lives as long as its primary. A primary
  past its `distMax` starts nothing, the secondary still started.
  INFERRED.
  The stock rows this touches, all looping: `cobra_helicopter_dying_loop`
  (450..4500) names `cobra_helicopter_dying_layer` (500..8000) as its
  secondary, and the jets' run a chain, `veh_mig29_dist_loop`
  (8500..20000) to `veh_mig29_mid_loop` (4000..9000) to
  `veh_mig29_mid_loop2` (3500..8000) to `veh_mig29_close_loop`
  (2500..4500) to `veh_mig29_close_loop2` (2500..4500). VERIFIED (the
  zones' alias rows, `cod4_audio::table`). Retail walks the chain:
  `SND_PlaySoundAlias_Internal` follows the secondary's own secondaries
  up to 10 links (`cmp [esp+0x10],0xa` at iw3mp.exe 0x5c3b66), refuses
  one that leads back to the alias it started from with "Error: Infinite
  recursion in secondary aliases sequenced together." (0x6f9678, pushed
  at 0x5c3bf5), and else plays the secondary through itself (0x5c3bdb),
  which starts its own in turn. VERIFIED (the instructions). vcod: under
  CoD4's table (`AudioSystem::set_cod4_aliases`) `play_pair` starts every
  link of the chain from the same place, stops at an alias already
  started or past 10, and the loop keeps every link's voice and stops
  them with it; CoD2's rule (one secondary, refused when it names its
  own) stays for CoD2. So the jets' run plays all five loops, each to
  its own `distMax`, where vcod played `dist_loop` alone (`mid_loop` has
  its own and was refused). Gate:
  `audio::tests::a_cod4_loop_starts_its_whole_secondary_chain`. VERIFIED
  (ours). How the strike sounds is pending a hand check.
- vcod: the loop path keeps a loop's secondary voice with it and stops
  both when the entity leaves the snapshot or its `loopSound` changes
  (`AudioSystem::reconcile_loops`), drops a secondary whose primary did
  not start, and `Cod4Client` runs `AudioSystem::reap_loops` once a frame
  after the loops: every looping voice that no loop, ambient, music,
  held, blend or shellshock owns stops. Before, a secondary was started
  untracked: it rang on after its entity left (`cobra_helicopter_dying_layer`
  after the crash, `veh_mig29_close_loop2` after each strike, one more
  each time), and a primary past its range started its secondary again
  every snapshot; a looping row an event or effect started rang on as
  well (INFERRED, the code before). Gates: `audio::tests::a_loops_secondary_ends_with_it`,
  `audio::tests::cod4_reaps_the_loops_nothing_refreshed` and the live
  `cod4_vehicles::tests::cod4_live_hardpoints_draw_smoke_and_sound`,
  which keeps the silent mixer's loops (`AudioSystem::keep_silent_loops`)
  and finds none but the ambient 25 s after the strike. VERIFIED (ours).

- A looping voice tied to an entity stops once the entity is freed,
  leaves the snapshot (`hide`) or drops its `loopSound`, its secondaries
  with it, on that frame's reap. VERIFIED live (the gate
  `cod4_audio::tests::cod4_live_entity_loops_stop_when_their_entity_goes`:
  vcod-server running `crates/cod4/tests/fixtures/gsc/loop_stop`, our
  client through `Cod4Client::pump`).

### 4.1 A voice on a turning entity

- `SND_SetChannelStartInfo` (iw3mp.exe 0x5c29b0) takes a voice's offset
  only for an entity voice (`sndEnt` not 0xffff, 0x5c29ca) on a 3D
  channel (`channels.def`'s `is3d` byte, 0x5c29e7): the start point less
  the entity's origin, dotted with each of its three axes, into the
  channel's offset (chaninfo at 0xcc94440, 0x8c a channel, offset at
  +0x78..+0x80); else the offset is 0. `SND_GetCurrent3DPosition`
  (0x5c28d0, six callers) puts the voice at the origin plus the offset
  along the axes each time it is placed. Both read the entity through
  0x43dd30 (`CG_GetSoundEntityOrientation`): below entity 1024 the
  centity's origin at +0x1c and `AnglesToAxis` (0x5635c0) of its angles
  at +0x28 (centity array 0x84f2d8, 0x1dc each); from 1024 (the
  createfx sounds) a point off the table at 0x739160 and the axes of the
  vector at 0xd8b2ed0. VERIFIED (the instructions). That +0x1c and +0x28
  are `pose.origin` and `pose.angles`, the drawn ones, is INFERRED
  (`CG_CopyEntityOrientation`).
- vcod: `Cod4Client::entity_angles` hands the mixer each entity's drawn
  angles where it drew, else its angle trajectory at the drawn time, and
  `main.rs`' CoD4 frame passes them to `AudioSystem::step`, so a voice
  started off its entity (a fire sound at a helicopter's or a gun's
  `tag_flash`, a brush model's loop at its middle) swings round it as
  it turns. Before, the frame passed no angles and every offset stayed
  in world axes. Gate: `cod4_audio::tests::cod4_live_voice_turns_with_its_entity`
  (vcod-server running `crates/cod4/tests/fixtures/gsc/sound_turn`, a
  `script_origin` turning a full circle, our client through
  `Cod4Client::pump`). VERIFIED (ours).

### 4.2 The output's speakers

- At start-up (0x5733e2) 1.7 asks Miles for its speaker layout
  (`AIL_speaker_configuration`, import 0x691450) and sets the
  multichannel flag at 0xcc147b4 when it reports more than two channels
  or none (0x5733eb..0x5733fb). VERIFIED (the instructions).
- `SND_ApplyChannelMap` (0x573d40) picks the speaker map's row
  `[source is stereo][flag]` (0x573d43..0x573d66: `(stereo * 2 + flag)
  * 0x64 + 8`), sets each of its `speakerCount` speakers by position to
  the larger of its two levels and hands them to
  `AIL_set_sample_channel_levels` (import 0x69149c, called 0x573de3).
  VERIFIED (the instructions).
- `SND_Apply3DSpatializationTweaks` (0x573df0) does nothing on a stereo
  output (0x573dff). On a multichannel one it reads the voice's levels
  (`AIL_sample_channel_levels`, 0x573e07), sets every one to 1, scales
  them by `1 - centre` when the row's centre send (+0x40) is not 0, then
  puts the centre send in level 2 and the LFE send (+0x3c) in level 3
  (0x573e60..0x573e75). VERIFIED (the instructions). So on a stereo
  output a 3D voice is Miles' own stereo pan of its position and a 2D one
  its map's stereo-output row, which is what vcod plays (section 2).
- How Miles pans a 3D voice over five or seven speakers is inside
  `mss32.dll`, not `iw3mp.exe`. INFERRED (no such code in the exe).

## 5. What vcod does not do yet

- A multichannel output. vcod mixes through kira, whose mixer is
  stereo (`kira::Frame` is left and right; its cpal backend writes
  silence to every channel past two), so a 5.1 or 7.1 device gets
  vcod's stereo mix on its front pair, as a stereo retail output does
  (4.2). Retail's multichannel arms would need a mixer of vcod's own:
  the map's `[source][1]` rows for a 2D voice, the centre and LFE sends
  of 4.2 for a 3D one, and a pan of a 3D voice over the speakers, which
  1.7 leaves to Miles and so has no 1.7 reference.
- The 139 rows whose curve no walked zone holds fall off straight.
