# Call of Duty 2 1.3: sound aliases, falloff and the cgame's sound events

How CoD2 1.3's client loads `soundaliases/*.csv`, what each of the 24 columns does, how a voice falls off with `.vfcurve` curves, and which alias each entity event plays. Written against CoD 1.1's `cod11-sound-system.md`; where this doc is silent, 1.1's mixer rules are vcod's stand-in.

## Sources

- The Mac 1.3 MP binary (`cod2mp_mac13_i386`, client and server in one, fully symbolized). Every address here is in its i386 slice. The Linux dedicated server has no cgame and no sound system.
- The stock data in the 1.3 install's iwds: `iw_06.iwd` (`iw_sound2.csv`, `volumemodgroups.def`), `iw_07.iwd` (the other six csvs, the `.vfcurve` and `.spkrmap` files) and `iw_15.iwd` (a newer `iw_multiplayer2.csv`, which replaces `iw_07`'s by name).
- vcod's loader (`crates/client/src/audio/cod2.rs`) and dispatch (`crates/client/src/play/cod2_sound.rs`), run over the install and the retail capture `crates/cod2/tests/fixtures/mp_toujane-spectate.bin`.

## 1. The alias files

`Com_LoadSoundAliases` (0xacc20) lists `soundaliases/*.csv` and loads each through `Com_LoadSoundAliasFile` (0xb969a) with the level's name and a system string. VERIFIED. The client passes `all_mp` (`CL_LoadSoundAliases`, 0x17ae88, `Com_LoadSoundAliases(map, "all_mp", 1)`). VERIFIED. The map name is the `maps/mp/` or `maps/` path's stem, lowercased. VERIFIED.

The file is read with `Com_Parse` in csv mode (`Com_SetCSV(1)`). VERIFIED. A line whose first cell is blank or starts with `#` is skipped. VERIFIED (the test on the first token). The first other line is the header; each header cell is matched case-insensitively against `g_pszSndAliasKeyNames` (`.data` 0x348780, 24 pointers). VERIFIED. A header cell that names no column, `compression` among them, binds index 0, which the field loader returns on at once. VERIFIED. A header without both `name` and `file` ends the file with `missing 'name' and/or 'file' columns`. VERIFIED (the two flags it sets for indices 1 and 3).

| Index | Column | What `Com_LoadSoundAliasField` (0xb78fa) does with a non-blank cell | Default |
|---|---|---|---|
| 1 | `name` | under 63 bytes, `[A-Za-z0-9_]` only, else the row errors | none |
| 2 | `sequence` | `atoi` | 0 |
| 3 | `file` | under 63 bytes, relative to `sound/` | none |
| 4 | `subtitle` | ASCII only | none |
| 5 | `vol_min` | `atof`, 0..1 else the row errors; also `vol_max` when no `vol_max` cell was read yet | 1 |
| 6 | `vol_max` | `atof`, 0..1 | `vol_min` |
| 7 | `vol_mod` | a group name of `volumemodgroups.def` (section 4); an unknown one is `Com_Error(1)` | 1 |
| 8 | `pitch_min` | `atof`; also `pitch_max` when no `pitch_max` cell was read yet | 1 |
| 9 | `pitch_max` | `atof` | `pitch_min` |
| 10 | `dist_min` | `atof` | 120 |
| 11 | `dist_max` | `atof` | 0 |
| 12 | `channel` | one of `g_pszChannelNames` (section 2), else the row errors | `auto` |
| 13 | `type` | `streamed` or `primed` stream, `loaded` loads, else the row errors | `loaded` |
| 14 | `loop` | `looping` or `nonlooping`, else the row errors | nonlooping |
| 15 | `probability` | `atof`, a float weight | 1 |
| 16 | `loadspec` | section 6 | admitted |
| 17 | `masterslave` | `master`, else a slave percentage `atof`, 0..1 | neither |
| 18 | `secondaryaliasname` | the `name` rules | none |
| 19 | `volumefalloffcurve` | a curve name (section 3); an unknown one is `Com_Error(1)` | the straight line |
| 20 | `startdelay` | `atoi`, milliseconds | 0 |
| 21 | `speakermap` | nothing: the switch has no arm for 21 | |
| 22 | `reverb` | flags for `fulldrylevel` and `nowetlevel` substrings | off |
| 23 | `lfe percentage` | `atof` clamped to 0..1 | 0 |

VERIFIED for the names and order (the pointer table, and the STABS enum `snd_alias_members_t`, `SA_NAME:1` .. `SA_LFEPERCENTAGE:23`), the defaults (the stores before the field loop), the ranges and error strings. INFERRED for "the row errors": a failed check sets the build's error byte at +0x149, and the row is not added when that byte is set.

A column given twice prints `Duplicate entries for the '%s' column` and errors the row. VERIFIED (the per-column seen bytes). A row whose `name` or `file` came out empty prints `alias entry missing name and/or file` and ends the whole file. INFERRED (the `return` after the print, before the next line is read).

After the fields, a row the `loadspec` admits and that carries no error is checked and finished: `pitch_min`/`pitch_max` and `vol_min`/`vol_max` are swapped into order; `pitch_min <= 0`, `vol_min < 0`, `dist_max < dist_min` and `dist_min <= 0` drop it with an error; a `dist_max` of 0 becomes `dist_min * 5` (`FLOAT_0032628c` reads 5.0); a `vol_mod` other than 1 multiplies both volumes, each clamped to 0..1. INFERRED for the order and the drops, off the branches; VERIFIED for the constants.

### 1.1 Grouping and duplicates

`Com_MakeSoundAliasesPermanent` (0xb6b18) merge-sorts the admitted rows by name (`AliasNameCompare`) and, within a name, by `sequence` (`Com_SortTempSoundAliases_r`, 0xb6924). VERIFIED. Two rows of one name and one sequence from the same csv print `duplicate alias` and both drop; from two csvs, the row of the csv whose name sorts first drops and the other stays. INFERRED (the merge's arms on `_I_stricmp` of the two build records' first field, the csv name). The rows of one name, from any csv, are one list the pick walks (section 5). VERIFIED (`Com_AddAliasList` adds a list per new name).

### 1.2 The stock data

vcod's loader over the 1.3 install reads 12961 alias names whose rows name 20215 distinct sound files, with no row error, no duplicate on any of the 15 stock MP maps, and no unknown group or curve. VERIFIED (`audio::cod2::tests::stock_cod2_aliases_load_and_resolve`). Six named files are in no iwd: five single-player ones and `misc/beep.wav`, which iw_multiplayer2.csv's `beep` row loads on every MP map. VERIFIED (same test). Header orders differ between the csvs (`iw_generic2.csv` puts `probability` third), so columns bind by name. VERIFIED. Paths are mixed case (`Voiceovers/US/...`) and resolve case-insensitively. VERIFIED.

## 2. Channels

`g_pszChannelNames` (`.data` 0x3487e0) lists eleven: `auto`, `auto2d`, `menu`, `weapon`, `voice`, `item`, `body`, `local`, `music`, `announcer`, `shellshock`. VERIFIED. `auto2d` is new against 1.1. `SND_IsAliasChannel3D` (0x49c8a) reads a channel as 2D when its bit is in 0x786: `auto2d`, `menu`, `local`, `music`, `announcer`, `shellshock`. VERIFIED.

## 3. Falloff curves

`Com_LoadSoundAliases` loads the curves once: slot 0 is the built-in straight line (`Com_InitDefaultSoundAliasVolumeFalloffCurve`, 0xb7658: name `""`, knots (0, 1) and (1, 0)), then every `soundaliases/*.vfcurve`, at most 15, named by the file name less `.vfcurve`. VERIFIED. A row without the column keeps slot 0 (`Com_GetDefaultSoundAliasVolumeFalloffCurve`, 0xad03c). VERIFIED. `Com_RegisterSoundAliasVolumeFalloffCurve` (0xacfc6) finds a name case-insensitively. VERIFIED.

The file (`Com_LoadVolumeFalloffCurve`, 0xb718a): the 8 bytes `SNDCURVE`, then `Com_Parse` tokens: the knot count, 2 to 8, then `x y` pairs, each value in 0..1. VERIFIED. A count other than the pairs read refuses the file. VERIFIED. A first knot other than (0, 1) or a last other than (1, 0) prints a warning and is set to those. VERIFIED. The stock curves:

| Curve | Knots |
|---|---|
| `default` | (0, 1) (1, 0) |
| `weapon1` | (0, 1) (0.0415, 0.6839) (0.2796, 0.4024) (0.6314, 0.1975) (1, 0) |
| `weapon2` | (0, 1) (0.025, 0.8) (0.2, 0.45) (0.4, 0.25) (1, 0) |
| `weapon_mg42` | (0, 1) (0.056, 0.7822) (0.2879, 0.5392) (0.6213, 0.2692) (1, 0) |
| `weapon_grenadebounce` | (0, 1) (0.1047, 0.8834) (0.2982, 0.5123) (0.6039, 0.1756) (1, 0) |
| `reaction` | (0, 1) (0.1047, 0.6834) (0.2982, 0.4123) (0.6039, 0.1756) (1, 0) |
| `tank` | (0, 1) (0.02, 0.3) (0.2, 0.2) (0.4, 0.1) (1, 0) |
| `tewsnd` | (0, 1) (0.5, 0.5) (1, 0) |

VERIFIED (iw_07.iwd).

`SND_Attenuate` (0x49c2e) takes the distance and the alias's `dist_min` and `dist_max`: 1 at or inside `dist_min`, 0 when the two are equal, 0 from `dist_max` on, else the curve at `(dist - dist_min) / (dist_max - dist_min)`. INFERRED (the branches). The curve's value (`Com_GetVolumeFalloffCurveValue`, 0xacbe0, into `GraphGetValueFromFraction`, 0x29976) is linear between the knot pair around the fraction, -1 past the last knot. INFERRED (the loop and the return). `SND_StartAlias3DSample` (0x57806) multiplies that by the voice's volume, its channel's volume and the master volume. INFERRED (the products after the call). The master volume is `g_snd` +0x24 (`DAT_010d67a4`), which `SND_Init` and `SND_Update` (0x4df64) set to the `soundFade` level times `snd_volume` times `FLOAT_00326234`, 0.75. VERIFIED for the constant (the `__TEXT` float at 0x326234) and the stores; INFERRED that every 2D, 3D and stream volume product reads +0x24 (`SND_Update2DChannel` 0x56f64 and its 3D and stream twins). So CoD2's 0.75 stands where 1.1's 0.8 (`FUN_0044d5c0`) does; vcod's `COD2_VOLUME_SCALE`.

## 4. Volume mod groups

`Com_LoadSoundAliasFile`'s first call reads `soundaliases/volumemodgroups.def`: the 15 bytes `VOLUMEMODGROUPS`, then name and value tokens to the end, at most 32 groups; `snd_refreshVolumeModGroups` rereads it. VERIFIED. The stock groups (iw_06.iwd): `ambience` 0.9, `bodyfall` 1.05, `chatteral` 1, `chatterax` 1, `element` 0.9, `emitter` 0.9, `explosion` 0.9, `foley` 0.7, `footstep` 0.9, `impact` 1.08, `interface` 0.9, `music` 0.9, `reactional` 1.26, `reactionax` 9.45, `vehicle` 0.9, `voiceover` 1, `wpnai` 1.02, `wpnplyr` 0.95, `na` 0.9. VERIFIED.

## 5. Playing an alias

The pick (`Com_PickSoundAlias`, 0xad97e; `Com_PickSoundAliasFromList`, 0xad450) walks the name's rows once, keeping each with odds of its `probability` over the sum so far, a weighted pick. INFERRED (the running sum and the `rand` compare). With three rows or more, a pick that lands on the row played last is redrawn among the others. INFERRED (the second loop, keyed on the per-row counter at +0x10 that each pick bumps).

`SND_PlaySoundAlias_Internal` (0x4b7b8):

- A row with a secondary alias picks that alias and plays it first, from the same entity and point; a secondary with a secondary of its own prints `a secondaryAlias cannot have a secondary Alias of its own` and does not play. INFERRED (the order of the calls; the error string is VERIFIED).
- A 3D row whose emitter is past `dist_max` does not start. INFERRED.
- A looping alias already playing on the same entity is moved and re-pitched, not restarted. INFERRED.
- A row on a channel past `menu` (index above 2) with no secondary stops whatever plays on the same entity and channel. INFERRED.
- Volume and pitch are drawn uniformly between the row's min and max. INFERRED (`randomf` lerps).
- A loaded row starts through `SND_StartAlias2DSample` or `SND_StartAlias3DSample` by channel, a streamed one through `SND_StartAliasStream`, both handed the row's `startdelay` (+0x3c). INFERRED.

### 5.1 Masters and slaves

`Com_MakeSoundAliasesPermanent` packs a row's flags into +0x2c: 1 looping, 2 master, 4 slave, 8 `fulldrylevel`, 0x10 `nowetlevel`, the channel at bits 7..10. VERIFIED (the bit stores after the field copy in `Com_MakeSoundAliasesPermanent`, 0xb6b18). The slave percentage is +0x30. INFERRED (the argument `SND_GetLerpedSlavePercentage` is handed).

`SND_Update` (0x4df64) walks the 3D, stream and 2D channels for one playing a master (the channel's byte +0x37d, past its start delay) and moves the lerp at `g_snd` +0x274 (`DAT_010d69f4`) by the frame's ms over `snd_slaveFadeTime`: up while a master plays, down otherwise, clamped to 0..1; a fade time of 0 sets it to 1 or 0 at once. VERIFIED for the dvar (`snd_slaveFadeTime`, default 500, 0..5000) and the arithmetic; INFERRED for what +0x37d marks. While the lerp is not 0, every non-master voice whose row has the slave bit has its volume multiplied by `SND_GetLerpedSlavePercentage` (0x49c02): `1 - (1 - pct) * lerp`. VERIFIED (the function and its seven callers in the channel start and update functions). 1.1 instead caps a slave at its percentage while any master plays (`cod11-sound-system.md` 4). vcod's `VoiceTable::use_slave_lerp` and `advance_slave_lerp`.

## 6. `loadspec`

`Com_LoadSoundAliasField`'s case 16 decides at load whether a row exists on the level. VERIFIED (the result is the build's +0x14a byte, which the add tests). A blank cell admits any level but `menu`. VERIFIED (the default store). Else, on the lowercased cell: a cell starting `!` starts out excluding (including on `menu`), any other including; the level's name found as a whole word (the byte before at most `!`, the byte after below `!`) keeps that; not found, it flips, unless the level is not `menu` and the cell's remainder after the last partial hit equals the system string, `all_mp` on the client. INFERRED (the branches). So `!mp_toujane` drops a row on mp_toujane only, `mp_toujane mp_burgundy` keeps it on those two only, and `all_mp` keeps it everywhere. INFERRED.

## 7. The cgame's sound events

### 7.1 Which events fire

`CG_CheckEvents` (0x1e614a): a temp entity (`eType` over 10) fires its event once, flagged in `cent->previousEvent`; any other entity fires the events of `events[i & 3]` with `eventParms[i & 3]` for each sequence number from its last to `eventSequence`, at most four, a smaller sequence first unwrapped by 256. VERIFIED (the arithmetic). `CG_ResetEntity` (0x1deb10), for an entity entering the snapshot, sets the last sequence to 0 for a general entity and a missile, so their whole ring replays, and to the current one for a player and most others. INFERRED (the `eType` arms). `CG_TransitionPlayerState` (0x1e4088) fires the playerstate ring's slots from `eventSequence - 4` that are past the old sequence, or inside the old window and changed. INFERRED.

### 7.2 What each event plays

`CG_RegisterSounds` (0x144cf8) looks up every per-surface alias as `<base>_<surface>` over `Com_SurfaceTypeToName`'s 23 names, the footstep events' order (`default` .. `asphalt`). VERIFIED. `CG_EntityEvent` (0x1e4650) plays, by event (CoD2 numbering, `vcod_cod2::net::events`):

| Events | Alias | Where |
|---|---|---|
| 1-23 `FOOTSTEP_RUN_*` | `step_run_<surf>` (with `cg_footsteps`), then `gear_rattle_run` | on the entity |
| 24-46 `FOOTSTEP_WALK_*` | `step_walk_<surf>`, `gear_rattle_walk` | on the entity |
| 47-69 `FOOTSTEP_PRONE_*` | `step_prone_<surf>`, `gear_rattle_walk` | on the entity |
| 70-92 `JUMP_*` | `step_run_<surf>`, `gear_rattle_run` | on the entity |
| 93-115 `LANDING_*` | `land_<surf>` | on the entity |
| 116-138 `LANDING_PAIN_*` | `land_<surf>`, `land_damage` | on the entity |
| 139 `FOLIAGE_SOUND` | `movement_foliage` | on the entity |
| 144 / 145 `ITEM_PICKUP` / `AMMO_PICKUP` | the weapon's `pickupSound` (else `weap_pickup`) / `ammoPickupSound` (else `weap_ammo_pickup`), the item index being the weapon's | on the entity |
| 146 `NOAMMO` | `player_out_of_ammo` unless the weapon is clip-only | on the entity |
| 150-153 `RELOAD*` | `reloadSound`, `reloadEmptySound` (each falling back to the other), `reloadStartSound`, `reloadEndSound` | on the entity |
| 154 / 155 / 156 / 157 | `raiseSound` (else `weap_raise`), `putawaySound` (else `weap_putaway`), `altSwitchSound`, `pullbackSound` | on the entity |
| 158-160, 175 `FIRE_WEAPON*` | `CG_FireWeapon` (0x1d823c): `fireSound`; the last shot `lastShotSound`, else `fireSound` | at the world `tag_flash`, else the entity |
| 176 / 177 `FIRE_QUADBARREL_*` | `CG_FireWeapon` twice | as above |
| 161 `RECHAMBER_WEAPON` | `rechamberSound` | on the entity |
| 163 `MELEE_SWIPE` | `meleeSwipeSound` | on the entity |
| 173 / 174 `MELEE_HIT` / `MELEE_MISS` | `melee_hit` / `melee_hit_other` | on `otherEntityNum` (174 at the event's own `lerpOrigin`, `CG_PlaySoundAlias`) |
| 179 / 180 `SOUND_ALIAS*` | configstring `parm + 590` | the entity's `pos.trBase` |
| 181 `SHOTGUN_HIT` | `bulletspray_small_<surf>` | world point |
| 182 `BULLET_HIT_SMALL`, 185 `BULLET_HIT_CLIENT_SMALL` | `bullet_small_<surf>` | world point |
| 183 `BULLET_HIT_LARGE` | `bullet_large_<surf>` | world point |
| 186 `BULLET_HIT_CLIENT_LARGE` | `bulletspray_small_<surf>` (`CG_BulletHitClientEvent` tests 185 and 183, so 186 falls to the spray) | world point |
| 181-183, 185, 186, after the hit | `whizby` (section 7.5) | world point |
| 187 `GRENADE_BOUNCE` | `grenade_bounce_<surf>` | world point |
| 188 `GRENADE_EXPLODE` | `grenade_explode_<surf>`, then the weapon's `projExplosionSound` | world point |
| 189 / 190 `ROCKET_EXPLODE*` | `rocket_explode_<surf>`, then `projExplosionSound` | world point |
| 191 `CUSTOM_EXPLODE` | `projExplosionSound`, unless `eFlags & 0x10000` and the blast's `es.time` is 50 ms old or more | world point |
| 197 `GRENADE_SUICIDE` | `grenade_explode_default` | world point |

VERIFIED for the `cgs` slots each arm reads, their names (`CG_RegisterSounds`) and the weapon keys behind each `cg_weapons` slot (`CG_RegisterWeapon`, 0x1d85cc, against the weapon field table at `.data` 0x348e1c: `fireSound` +0xa8, `fireSoundPlayer` +0xac, `lastShotSound` +0xc0, `projExplosionSound` +0x398). The surface is `es.surfType`, the "where" the arm's call (`CG_PlayEntitySoundAlias` on `es.number`, or `CG_PlaySoundAlias(0x3fe, lerpOrigin)`). VERIFIED. The bullet hit rows are as the comparisons in `CG_BulletHitEvent` (0x1db1f4) and `CG_BulletHitClientEvent` (0x1d9b78) read against the event number, which pairs `LARGE` with the small alias. INFERRED.

Each weapon key has a `...Player` twin (`fireSoundPlayer`, `reloadSoundPlayer`, ...) and each footstep, gear and landing alias a `_plr` one (`step_run_plr_<surf>`, `gear_rattle_plr_run`, `land_plr_<surf>`). The cgame takes the twin when `ps.pm_flags & 0xc00000` and the event's entity is `ps.clientNum`, the first-person body. INFERRED (the test opening `CG_EntityEvent` and `CG_FireWeapon`).

### 7.3 The ambient

`ambientPlay(alias, fade)` sets configstring 3 to `n\<alias>\t\<level.time + fade ms>` (`Scr_AmbientPlay`, 0x19c512). VERIFIED (the format string). `ambientStop(fade)` sets it to `t\<level.time + fade ms>` alone (`Scr_AmbientStop`, 0x19c61a). VERIFIED. `CG_StartAmbient` (0x143de6) picks configstring 3's `n` and hands `SND_PlayAmbientAlias` (0x4c806) the time left to `t`, 0 when past or while `cg.time` is 0. VERIFIED. It runs on the configstring's change (`CG_ServerCommand`'s `d 3`), at `CG_Init` and in `CG_MapRestart` after `SND_StopSounds`. VERIFIED (the three calls). mp_toujane's is `ambient_africa`, a streamed loop. VERIFIED (the capture's configstring 3; iw_sound2.csv).

`SND_PlayAmbientAlias` keeps two background stream slots, 0x21 and 0x22, and the current one's index (`DAT_010d69f0`, toggled `3 - n`); each slot has a target volume and a change per ms (`DAT_010d69d8` + 8n). VERIFIED. A null alias returns at once, so `ambientStop` on 1.3 stops nothing: the track plays until a stop of every sound. INFERRED (the null test; `Com_PickSoundAlias` of an empty name finds no list). On the busy current slot: the same alias pointer returns; the same file, loop flag, `pitch_min`, `pitch_max` and channel draws a new volume and ramps toward it from the old target over the fade (no change rate for 0). INFERRED (the compares). Otherwise the current slot fades out (stopped at once for a fade of 0, else its target goes to 0 at `-target / fade` per ms when it is above 0), the index flips, and `SND_StartBackground` (0x4c5c8) stops what still plays on the new slot and starts the alias there: a drawn volume as its target, from 0 at `volume / fade` per ms, or at that volume for a fade under 1. INFERRED. A row on a 3D channel or not streamed is a `Com_Error`. VERIFIED (the two messages). `SND_UpdateBackgroundVolume` (0x4a5d2) moves a slot's volume by its rate each frame and stops it on its target; one falling to a target of 0 ends the stream. INFERRED (the branches). vcod's `AudioSystem::set_cod2_ambient`, one voice per slot on `cues::AMBIENT_ENTITY` and `AMBIENT_ENTITY_2`.

### 7.4 The capture

The retail capture `mp_toujane-spectate.bin` (23 snapshots, a free spectator and one running client) carries one event: the running client's `EV_FOOTSTEP_RUN_DIRT` on entity 0 at server time 16050, which plays `step_run_dirt` and `gear_rattle_run`. VERIFIED (`play::cod2_sound::tests::retail_toujane_capture_plays_the_other_players_steps`).

### 7.5 The whizby

`CG_BulletHitEvent` (0x1db1f4) and `CG_BulletHitClientEvent` (0x1d9b78), after the hit's alias and effects, run `CG_WhizbySound` (0x1d99ca) from the shooter's muzzle (`otherEntityNum`, `CG_CalcMuzzlePoint` 0x1d7f72 on `tag_flash`) to the hit, when `cg_tracerchance` is above 0. VERIFIED (the calls and the test). `cg_tracerchance` defaults to 0.4 (`CG_Init`, 0x145214, its `Dvar_RegisterFloat` call). VERIFIED. The tracer beside it is skipped for the first-person body's own rounds and drawn with that chance; the whizby is neither. INFERRED (the tracer's test wraps only `CG_SpawnTracer`).

The whizby takes the shot's direction, the eye's distance along it from the muzzle, and plays `whizby` (`cgs` +0x2832c) at `ENTITYNUM_WORLD` 16 units back toward the muzzle from the shot's closest point to the eye, when that distance is at least 64, 64 short of the shot's length or more, and the closest point within 140 of the eye. INFERRED (the branches). The constants: `FLOAT_00326344` 64.0, `FLOAT_0032662c` 140.0, `FLOAT_00326630` -16.0. VERIFIED (the `__const` floats). The first-person body's muzzle is the view origin, so his own rounds never whizz. INFERRED (`CG_CalcMuzzlePoint`'s first arm, and the 64 test). A client body whose model has no flash tag gets its origin raised 11 under `eFlags` 0x8, 40 under 0x4, else 60 (`FLOAT_00326374`, `FLOAT_003262f4`, `FLOAT_00326380`). VERIFIED for the constants; INFERRED for the arms.

### 7.6 Loop and projectile sounds

`CG_ProcessEntity` (0x164bd4) runs `CG_EntityEffects` (0x163dd0) on every entity before its type's drawer. VERIFIED. That plays configstring `590 + es.loopSound` through `CG_PlaySoundAliasByName` at the entity's `lerpOrigin` each frame, when `loopSound` is set. VERIFIED. `ScrCmd_PlayLoopSound` (0x19e5f6) sets `loopSound` to `G_SoundAliasIndex`'s index. VERIFIED. The stock maps' `soundfx` emitters (`maps/mp/_fx.gsc`) and the S&D bomb's `bomb_tick` ride it. VERIFIED (the iwd scripts). `CG_Missile` (0x1646da) plays the weapon's `projectileSound` (`cg_weapons` +0xd4, def +0xa0) the same way each frame, unless `eFlags` 0x20 or before `es.time`. VERIFIED. Re-playing a looping alias on its entity keeps the voice (section 5).

### 7.7 Sound server commands

`CG_ServerCommand` (0x1e2ea4) switches on the first byte of the first word. VERIFIED. Its sound arms:

| Command | Sent by | Plays |
|---|---|---|
| `s <index>` | `ScrCmd_PlayLocalSound` (0x1c1fce), `%c %i` to the one client | configstring `590 + index` through `CG_PlayClientSoundAliasByName` (0x144a64), on the playerstate's `clientNum` at its origin; an index outside 1..256 or a count other than two words prints an error |
| `o <alias>` | `Scr_MusicPlay` (0x1991f8), to all | `SND_PlayMusicAlias` (0x4c9e6), only while the music stream slot is free |
| `p <ms>` | `Scr_MusicStop` (0x19c458), the fade rounded to ms | `SND_StopMusic` (0x4ca2c): 0 stops at once, else a fade that a second `p` leaves alone |
| `q <volume> <ms>` | `Scr_SoundFade` (0x19923c), `%c %f %i` | `SND_FadeAllSounds` (0x4cef2): 0 ms to 0 stops every sound |
| `r ...` | `setReverb` | `SND_SetEnvironmentEffects`, six words |

VERIFIED for the letters, the format strings and the index bound (`index - 1 < 0x100`); INFERRED for the music and fade arms' behaviour, which match 1.1's (`cod11-sound-system.md` 10). `playSoundOnPlayers`, which the stock `ctf`, `hq`, `sd` and `tdm` scripts each define, calls `playLocalSound` on every player of the team, so the announcer goes out as one `s` per listener. VERIFIED (the iwd scripts).

### 7.8 Notetrack sounds

`CG_UpdateViewWeaponAnim` (0x1dd3fe) walks the view weapon tree's notifies after the anim update and plays the weapon's `noteTrackSoundA`..`D` (`cg_weapons` +0x124..+0x130, def +0x100..+0x10c) on a notify named `noteTrackSoundA`..`D`, through `CG_PlayClientSoundAlias`. VERIFIED. The alias variable is not cleared between notifies, so every later notify of the same frame plays it again. INFERRED. `CG_ProcessClientNoteTracks` (0x16341c) reads the bodies' notifies for `anim_gunhand` only and plays nothing. VERIFIED. No stock 1.3 weapon file sets a `noteTrackSound` key, so retail plays no notetrack sound. VERIFIED (`play::cod2_sound::tests::stock_content_names_aliases_the_table_holds`); vcod has no notetrack path.

`CG_RegisterWeapon` (0x1d85cc) looks up the weapon file's alias keys def +0x98..+0xac and +0xc0..+0x10c, then `projExplosionSound` (+0x398); `loopFireSound`, `stopFireSound` and their `Player` twins (+0xb0..+0xbc), which five stock MG files set, are never looked up, so the client's `weaponInfo` keeps no alias for them. VERIFIED (the lookups skip def +0xb0..+0xbc). The 1.3 MP client plays no fire loop. INFERRED (no alias to play).

### 7.9 The census

Every name `CG_RegisterSounds` builds (14 per-surface bases over 23 surfaces and its 17 fixed names, with `CG_RegisterWeapon`'s fallbacks `weap_raise`, `weap_putaway`, `weap_pickup` and `weap_ammo_pickup`), every alias key of the 42 stock weapon files (266 set), and the 142 sound literals of the stock `maps/mp` scripts (direct to `playSound`, `playLocalSound`, `playLoopSound`, `playSoundOnPlayers`, `ambientPlay`, `musicPlay`, or routed through their `soundalias`-style variables) are loaded on every stock map they can play on, a map's own script on its map, and every loaded file decodes. VERIFIED (`stock_content_names_aliases_the_table_holds`).

### 7.10 Sound blend entities

`CG_ProcessEntity` sends `eType` 7 to `CG_SoundBlend` (0x16351c). VERIFIED (cod2-protocol.md's eType table). With both `eventParms[0]` and `eventParms[1]` (entity state +0xb8, +0xbc) set, it picks configstring 590 plus each and plays them through `SND_PlayBlendedSoundAliases` (0x4c1d0) at the entity's `lerpOrigin`, blended by `leanf` (+0xd4) lerped from the centity's previous copy to the current state by `cg.frameInterpolation`. VERIFIED (the offsets against the netfield table, `fields.rs`). `SND_PlayBlendedSoundAliases` validates the pair (`SND_ValidateSoundAliasBlend`, whose errors name the slave and both reverb flags) and plays it through `SND_PlaySoundAlias_Internal` with both rows, the call every play makes with one row twice. VERIFIED. The 1.3 game module has no script call that spawns a sound blend entity, so no stock server sends one. VERIFIED (no `sound_blend` or `soundblend` string in `cod2_lnxded`). The cgame's shellshock blends `shellshock_loop` and `shellshock_loop_silent` the same way (`CG_UpdateShellShock`, 0x1d7528). VERIFIED. vcod plays both through UO's blend voice (`AudioSystem::set_sound_blends`; `coduo-diff.md` 16 for its refresh and start rules), which CoD2's internal play is taken to match. INFERRED.

### 7.11 Subtitles

`CG_PlaySoundAliasByName`, `CG_PlaySoundAlias`, `CG_PlaySoundAliasAsMasterByName`, `CG_PlayClientSoundAliasByName` and `CG_PlayClientSoundAlias` (0x144474..0x144b3c), after a started play, print the picked row's subtitle (+4) through `CL_SubtitlePrint` (0x17ab76) for `max(cg_subtitleMinTime * 1000, the play's return)` ms at a width of `cg_subtitleWidthStandard` (`cg_subtitleWidthWidescreen` past an aspect), both 80. VERIFIED for the calls, the dvars (`cg_subtitles` 1, `cg_subtitleMinTime` 3.0, `cg_subtitlePosX` 120, `cg_subtitlePosY` 400, `cg_subtitleCharHeight` 12) and `FLOAT_00326180` (1000.0); INFERRED that the play returns the sound's length in ms. `CL_SubtitlePrint` looks the key up in `soundaliases/subtitle.st` (StringEd), printing the key itself when absent, onto console window 3 (`CL_ConsolePrint`). VERIFIED. `CG_Draw2D` draws window 3 under `cg_subtitles` through `Con_DrawSubtitles` (0x15dbd4) at (`cg_subtitlePosX`, `cg_subtitlePosY`) in lines `cg_subtitleCharHeight` high, newest at the bottom, whether or not the scoreboard is up. VERIFIED (the call after the bold messages, 0x1d1895). The stock csvs carry 1826 subtitled rows, all in `iw_voiceovers2.csv` and all with single-player `loadspec`s (`88ridge`, `libya`, `toujane`, ...), so no stock MP level loads one. VERIFIED (iw_07.iwd). vcod reads the key from `localizedstrings/subtitle.str` (`localized_english_iw09.iwd`), and a stream's length is unknown before decode, so a streamed row shows for the minimum.

### 7.12 Reverb

`setReverb(priority, roomtype, dry = 1, wet = 0.5, fade = 0)` sends `r <1|2> "<room>" <dry> <wet> <fade s>` to the one player (`PlayerCmd_SetReverb`, 0x1c1960; `snd_enveffectsprio_level` 1, `snd_enveffectsprio_shellshock` 2) and `deactivateReverb(priority, fade)` a `D` (`PlayerCmd_DeactivateReverb`, 0x1c1b42). VERIFIED. `CG_ServerCommand` turns six words of `r` into `SND_SetEnvironmentEffects(prio, room, dry, wet, floorf(fade * 1000 + 0.5))` and three of `D` into `SND_DeactivateEnvironmentEffects(prio, ms)`; other counts print an error. VERIFIED. No stock MP script calls either. VERIFIED (the iwd scripts).

Three priority slots of 32 bytes at `DAT_010d69f8` hold a room index, the dry and wet levels each as current, target and rate, and an active byte; slot 0 is `SND_Init`'s room 0, dry 1, wet 0, active. VERIFIED. `SND_SetEnvironmentEffects` (0x4a782) marks the slot active, looks the room up in `snd_roomStrings` (index 0 on a miss, with a list of the names), sets the slot's levels to fade from the ones in force to the new over the fade (1 ms under 1), and puts the slot in force unless a higher one is active. INFERRED (the branches). `SND_DeactivateEnvironmentEffects` (0x4a682) clears the slot; if it was in force, the next lower active slot takes over, its levels fading from the deactivated one's current over the fade. INFERRED. `SND_Update` steps the levels in force and calls `SND_UpdateReverbs` while the wet one moves. VERIFIED. `SND_SetRoomtype` (0x5681e) sets Miles' master room type and master reverb levels (1, wet); every sample and stream gets reverb levels (dry 1, the wet in force), or wet 0 under `snd_enableReverb` 0 or the row's `nowetlevel` flag. VERIFIED. The dry level fades and is read by no reverb or volume call; `fulldrylevel` (flag 8) is read only by the blend validation. INFERRED (no reader in the channel functions). `lfe percentage` is stored at alias +0x38 and read by no sound function. INFERRED (no read of alias +0x38 in the export's `SND_` channel functions). vcod's `audio::cod2::EnvEffects`.

## 8. Speaker maps

A `.spkrmap` file is `SPKRMAP`, then lines `<SOURCE> <SPEAKER> <gain>` for a mono and a stereo source onto two speakers, then the same onto six (`LEFT`, `RIGHT`, `CENTER`, `LFE`, `LEFTSURROUND`, `RIGHTSURROUND`). VERIFIED (iw_07.iwd's five files). The MP binary's alias loader stores nothing for the `speakermap` column (section 1). INFERRED (no arm for index 21). Neither `SPKRMAP` nor `spkrmap` is a string anywhere in the Mac binary. VERIFIED. vcod reads none.

## 9. `vid_restart` and `snd_restart`

`CL_Vid_Restart_f` (0x14b466) prints `Listen server cannot video restart.` and returns while `com_sv_running` is set. INFERRED (the branch on it). It calls `SND_StopSounds(1)` (the call at 0x14b4c1, its argument stored at 0x14b4ba), `CL_ShutdownCGame` (0x14b795) and `CL_ShutdownUI` (0x14b79a). VERIFIED. Past the listen-server test the stop is unconditional, ahead of both shutdowns, so it runs with a game up or not. INFERRED. `CL_StartHunkUsers` (0x147e22) starts the UI again (`CL_InitUI` at 0x147e5d), and `CL_InitCGame` (0x14b6d0) runs while the connection state is 6 or more. INFERRED (the unsigned compare before the call). The new cgame's configstring pass sets the ambient again. INFERRED.

`SND_StopSounds` (0x4ca92) spares the 2D and 3D channels under argument bit 8, stream channel 0x20 under bit 2 and channels 0x21..0x22 under bit 4, and keeps the reverb slots (7.12) under bit 1. INFERRED (the branches on each bit). `vid_restart`'s 1 therefore stops every 2D, 3D and stream channel, music included, and keeps the room. INFERRED. `CL_Snd_Restart_f` (0x14b812) saves the sound state (`SND_Save`), runs `SND_Shutdown`, `SND_Init`, then `CL_Vid_Restart_f`, and `SND_Restore`s the saved state after it. INFERRED (the call order).

`CL_ShutdownUI` (0x181a5c) clears bit 0x8 of the local client's key catchers (`andl $-0x9, 0x4(%eax)` at 0x181a8e). VERIFIED. `UI_Init` calls `Menus_CloseAll` (0x150e24) after loading `ui_mp/menus.txt`. INFERRED (the call order). `CL_MouseEvent` (0x187f66) hands the mouse to the UI only while that bit is set and `cl_bypassMouseInput` is 0 (`testb $0x8` at 0x187f77), and otherwise adds it to the cmd's mouse deltas. INFERRED (the branch). `CL_InitCGame` sets no catcher. INFERRED (no write to `cl` +4 in it). So after a `vid_restart` in a game the menus are closed and the mouse turns the view, as 1.1's does: vcod's `App::after_menu` for `Mode::Cod2` as for `Mode::Online`. INFERRED.

## 10. vcod divergences

- The table keeps every row and applies `loadspec` and the duplicate rule per map (`audio::cod2::for_map`), so one load serves every level.
- `probability` is kept as an integer weight 1000 times the float; the pick's ratios hold.
- Footsteps sit where the entity was at the snapshot and do not follow it. A fire sound starts at the drawn body's `tag_flash` and is carried with the body (`Source::Tag`); the first-person body's plays on the entity, pinned to the view, not at the view weapon's tag.
- The reverb is one kira reverb on the main track standing in for every Miles room type, its mix set by the wet level in force (7.12; the shellshock's room rides priority 2): a `nowetlevel` row is not kept dry, and the room name is not used. kira mixes stereo, so there is no LFE channel to send to, and retail reads `lfe percentage` nowhere.
- A row on a channel past `menu` replaces the entity's voice on that channel, as 1.1's does, whether or not it has a secondary alias.
- The whizby's muzzle is the shooter's drawn `tag_flash`, else his origin raised to his stance (section 7.5's tagless fallback); a shooter outside the snapshot gives none.
- The effects of the same events are `play::cod2_fx`'s.
- A sound blend entity plays through UO's blend voice (7.10).
- A streamed row's subtitle shows for `cg_subtitleMinTime` only (7.11); the subtitle cvars are not registered, their defaults are fixed.
- `EV_SOUND_ALIAS_AS_MASTER` plays as `EV_SOUND_ALIAS`: only a row's `master` flag makes a master (5.1).
- A non-looping `loopSound` alias plays once, not each frame.
