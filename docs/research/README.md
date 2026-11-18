# Research docs

Verified engine facts, with the binary addresses, assets and captures they rest on. Read a subsystem's doc before touching its code, and add what you learn to it. Numbers in a row (`(9)`, `(13-19)`) are the doc's sections for the blocks named before them.

## Evidence labels

Every claim names the module, the virtual address and the string or table it rests on, and carries its own label:

- VERIFIED: read out of a binary, an asset or a live capture.
- INFERRED: read off control flow, which includes instruction order and branch conditions ("then", "followed by", "when that field is non-null").

One label per claim. A document may open with a document-level VERIFIED default only when its provenance is uniform and every exception is labelled. A section-level blanket, including one in a heading, is never allowed. Docs carry derived facts (offsets, tables, enum orders), never pasted decompiler output. `crates/common/tests/evidence_labels.rs` catches two mechanical shapes; its header lists what a reader still has to check.

## CoD1 1.1 (the base the other games diff against)

| Doc | Covers |
|---|---|
| [../protocol-1.1.md](../protocol-1.1.md) | The 1.1 wire; its last section lists every divergence from RTCW/Q3. Check it before assuming Q3 semantics |
| [clientstate-wire-format.md](clientstate-wire-format.md) | `clientState_t`, the third snapshot stream: `team`, the configstring map, entity types on the wire, inline submodels |
| [cod11-server-handshake.md](cod11-server-handshake.md) | Handshake, configstrings, rcon, the stdin console, heartbeat, zombies, pings, rate, bans, passwords, private slots, message pacing, downloads, CD key authorize (the Housekeeping section) |
| [cod11-map-cycle.md](cod11-map-cycle.md) | `map`, `map_restart`, `sv_serverid`, rotation, the restart's configstring burst, compile carry-over. The client's side of `map_restart` (`CG_MapRestart`) is `cod11-events-and-fx.md` 9.8 |
| [cod11-combat.md](cod11-combat.md) | Shots, damage, death, grenades, blasts, hit locations, shot timing, the view weapon's offsets |
| [cod11-mantle.md](cod11-mantle.md) | Movement: why 1.1 has no mantle; stances, ladders, jump, step-up, slide; movement constants. 1.1's stored floats and the Windows client's arithmetic: `cod15-movement.md` 7 and 8 |
| [cod11-player-clip.md](cod11-player-clip.md) | Player clip, kerbs, getting stuck, `StuckInClient` |
| [cod11-movers.md](cod11-movers.md) | Movers, push, `linkTo`, brush model clip, player anim clock |
| [cod11-items.md](cod11-items.md) | Items, pickups, drops |
| [cod11-turrets.md](cod11-turrets.md) | Mounted MGs: mount, aim, fire, the gunner's pose |
| [cod11-hud-protocol.md](cod11-hud-protocol.md) | Single-letter server commands and the script menu handshake (0), obituaries (1, 2), the scoreboard (3, 4), status configstrings and the server cvar mirror (CS 140/204) (5), fonts (6), the `^N` palette (7), hudelems (8), the native player HUD: crosshair, scope, compass, stance, cursor hint, damage direction (9), wide windows (10) |
| [cod11-events-and-fx.md](cod11-events-and-fx.md) | Entity events and their effects, impact marks (9), `CG_MapRestart` (9.8), module md5s |
| [efx-grammar.md](efx-grammar.md) | The `.efx` effect file grammar and how the 1.1 client plays it |
| [cod11-sound-system.md](cod11-sound-system.md) | Alias csvs, channels, falloff and panning, whizby, event aliases, footsteps, streams |
| [cod11-gsc-language.md](cod11-gsc-language.md) | gsc language semantics, threads, the script clock, engine notifies, runtime errors |
| [cod11-gsc-object-model.md](cod11-gsc-object-model.md) | gsc object model: entities, fields, arrays, builtins' side effects |
| [cod11-gametypes-re-bel.md](cod11-gametypes-re-bel.md) | The `re` and `bel` gametypes |
| [cod11-spectator-follow.md](cod11-spectator-follow.md) | Spectator follow, killcam (12), what `ClientSpawn` leaves in the playerstate (13) |
| [cod11-console.md](cod11-console.md) | Client console, binds, `vcod_mp.cfg` |
| [cod11-demos.md](cod11-demos.md) | `record`, `demo`, the `.dm_<n>` format, `timedemo`, `nextdemo`, `cg_autodemo` |
| [cod11-front-end.md](cod11-front-end.md) | Main menu, server browser and master query, options screens, Start New Server, script menus in the UI; the error popup (15), modded servers' systeminfo cvars (16), the Mods menu with its cgame restart and `snd_restart` (17), list boxes (19), focus, edit fields and menu keys (20), video modes (21), the motd (22), the CD key menu (23), the menu precompiler (24) |
| [cod11-chat.md](cod11-chat.md) | Chat |
| [cod11-quick-chat.md](cod11-quick-chat.md) | Quick chat |
| [cod11-votes.md](cod11-votes.md) | `callvote`, `vote`, `CheckVote`, the vote lines on the HUD |
| [cod11-shellshock.md](cod11-shellshock.md) | `.shock` files, screen blend, kick, mouse, sound mix |
| [cod11-shader-scripts.md](cod11-shader-scripts.md) | Shader scripts: census, the grammar vcod implements, runtime math, sort order, sky; vertex deforms, model skin scripts, world shader types, the sun, coronas, `r_flares` (8) |
| [cod11-light-grid-and-leaf-lights.md](cod11-light-grid-and-leaf-lights.md) | Light grid (lumps 19, 30, 32), prop and entity lighting; dynamic lights on models (11), shadows (12), entity models (13) |
| [cod11-gamma.md](cod11-gamma.md) | Gamma, overbright, `r_intensity`, stage colours and display doubling |
| [player-model-anim-system.md](player-model-anim-system.md) | Player models and anims: animscript, movetype pick, xanim header flags and the flagged layouts, pose |

The CoD1 view weapon: `cod11-combat.md` (its offsets), `xanim-v14-format.md` (the sight layer and the hip pose), `xmodel-v14-format.md` (the view fov). vcod's own render options, with no retail side: `cod11-gamma.md` 6 to 9 (`vc_ssao`, `vc_bicubic`, `vc_softParticles`, `vc_hdr`, `vc_grade`, `vc_vignette`, `vc_grain`), `cod11-light-grid-and-leaf-lights.md` 12 (`vc_shadows`), `cod11-hud-protocol.md` 10 (wide windows), `cod11-shader-scripts.md` 8 (`vc_sunShafts`).

## CoD1 1.5

| Doc | Covers |
|---|---|
| [cod15-protocol-diff.md](cod15-protocol-diff.md) | Protocol 6 against 1.1: tables, `pm_flags`, handshake, server commands, HTTP downloads; the match timeout (12); the server side: `getinfo`, cvars, builtins, squad chat (13); the snapshot builder's fog cull (15) |
| [cod15-movement.md](cod15-movement.md) | 1.5 movement against 1.1: jump penalty, stance, lean, step, weapons; stored floats (7); the Windows clients, 1.1's arithmetic included (8) |

## United Offensive 1.51

| Doc | Covers |
|---|---|
| [coduo-diff.md](coduo-diff.md) | Protocol 22: wire, configstrings, events, vehicles on the wire, sprint, gametypes, assets; sound and its blend entities (16); events, effects, flame, the turret view and the HUD (19); vcod-server hosting UO: stage 1 (20), weapons, missiles, artillery and the flamethrower (21), gametypes (22); HTTP downloads (23); systeminfo, pak lists and pure DLL names (24) |
| [coduo-vehicles.md](coduo-vehicles.md) | Vehicle seats in and out, the seated cmd, prediction and camera; the server's spawn, drive step, guns, damage, wheel dust, death and wrecks |
| [coduo-movement.md](coduo-movement.md) | UO movement against 1.5: sprint, fatigue, LMG bipod and mounts, grenade cook; weapon time truncation, which holds on every CoD1 Linux server (13) |

## CoD2 1.3

| Doc | Covers |
|---|---|
| [cod2-protocol.md](cod2-protocol.md) | Protocol 118 against 1.1: handshake, netchan, ops, usercmd, netfields, configstrings, entities, events, `Cod2Wire` and its CoD1 shim, map change, the client's join and commands, follow, votes, vcod's server and its housekeeping (14-17) |
| [cod2-gsc-diff.md](cod2-gsc-diff.md) | gsc against 1.1: builtin, method, field and spawn tables, `#include`, `waittillframeend`, typing, callbacks, notifies, the use key and cursor hint |
| [cod2-map-cycle.md](cod2-map-cycle.md) | Map cycle on vcod-server against retail captures: `map`, `map_restart`, `fast_restart`, `map_rotate`, rounds, intermission |
| [cod2-movement.md](cod2-movement.md) | Movement against CoD1: stored floats, buttons, clip order, mantle, stance, `vcod_cod2::pmove`, its A/B (`moves_ab`), prediction, first-person eye |
| [cod2-combat.md](cod2-combat.md) | `PM_Weapon`, sight and spread, antilag, bullets, hit-location damage, death, obituary, grenades, rockets, blasts |
| [cod2-killcam.md](cod2-killcam.md) | Frame archive, the archived follow copy, the replay snapshot, its A/B |
| [cod2-turrets.md](cod2-turrets.md) | Mounted guns: mount, view clamp, the gunner's frame, release, `spawnTurret`, the A/B |
| [cod2-player-anims.md](cod2-player-anims.md) | Player bodies: models, `j_*` rig, anim array, controllers, legs and torso anims, flinches, death anim, corpse; the server's DObj pose (11), the trace through the hit boxes (12) |
| [cod2-weapons.md](cod2-weapons.md) | Weapon files: field table, enums, changes from 1.1; the view weapon's anim tree |
| [cod2-hud.md](cod2-hud.md) | Player HUD: `hud.menu` owner draws, crosshair, damage icons; the scoreboard (13), chat (14), centre print (15), shellshock (16), the view and its kicks (17), the view weapon's place (18), world sprites, head icons and crosshair names (19) |
| [cod2-fonts-menus.md](cod2-fonts-menus.md) | Fonts, menu keywords and script commands, `rect` alignment, hudelem placement, the front end |
| [cod2-sound.md](cod2-sound.md) | Alias csvs, `volumemodgroups.def`, `.vfcurve` falloff, sound events, whizby, loop sounds |
| [cod2-efx.md](cod2-efx.md) | `.efx` grammar against CoD1, curves, spawn and motion, atlases, clouds, camera shake, the spectator's effects |
| [cod2-materials.md](cod2-materials.md) | Binary materials, techsets, techniques, statemaps, sampler and state bits, surface flags; what each family computes (9): light grid probe, depth feather, shadow cookies, glow, water, menu blur, particle clouds |
| [cod2-d3dbsp-format.md](cod2-d3dbsp-format.md) | `.d3dbsp` (IBSP 4) lumps, the collision model, the trace A/B (`trace_ab.rs`), lightmaps, light grid |
| [cod2-xmodel-v20-format.md](cod2-xmodel-v20-format.md) | xmodel / xmodelparts / xmodelsurfs v20 against v14: hit boxes, weights, materials |
| [cod2-iwi-iwd.md](cod2-iwi-iwd.md) | `.iwd` search path, `.iwi` images and the wavelet decoder |
| [cod2-demos.md](cod2-demos.md) | Demos: what CoD2 keeps and changes from CoD1; screenshots and `cl_avidemo` (8) |
| [cod2-voice.md](cod2-voice.md) | Voice chat: the wire, the relay rules, the codec at 8192 Hz, push to talk, the talker list, the mute list, the retail capture |

## CoD4 1.7

| Doc | Covers |
|---|---|
| [cod4-handshake.md](cod4-handshake.md) | Handshake, stats phase and block, pure `cp` (8), netchan, Huffman, configstrings, what vcod-server sends (9), downloads (11), the server browser (12), CoD4x servers (13), the console and rcon (14), off the LAN: authorize, `requeststats`, pacing (15) |
| [cod4-netfields.md](cod4-netfields.md) | Netfield tables, delta codec, gamestate end, snapshots, the move message |
| [cod4-server-frame.md](cod4-server-frame.md) | `SV_Frame`'s order: cmds, zombies, archive, message order; `sv_fps` (8); vcod-server's tick cost, measured: slow ticks (9), full servers of every game (10), 32 bots (11), off-LAN pacing (12) |
| [cod4-map-cycle.md](cod4-map-cycle.md) | `map`, `fast_restart`, `map_rotate`, the client's map change, votes, the menu answers' one-packet rule |
| [cod4-gsc.md](cod4-gsc.md) | gsc: dialect over CoD2's, builtin tables, configstring slots, the `war` boot, callbacks, connect to spawn, menus, loadout, server commands and squash, follow, hardpoints, script traces, launched parts on the client |
| [cod4-pmove.md](cod4-pmove.md) | Player movement: flags, ground, step, stance, jump, sprint, dvars, the moves A/B (`cod4_moves_ab`), the server's think and spawn, `placeSpawnPoint`, the walk replay (`cod4::wire_ab`) |
| [cod4-combat.md](cod4-combat.md) | Combat on vcod-server: `WeaponDef`, `PM_Weapon`, the shot, hit boxes, antilag, damage, corpse, dropped weapons, killcam, the A/B (`cod4_combat_ab`), the capture gates (`capture_ab`), grenades, launcher, melee, `kill` |
| [cod4-collision.md](cod4-collision.md) | The clip map: leaf brush trees, triangles, submodels, static model clip, the clip census |
| [cod4-player-anims.md](cod4-player-anims.md) | xanim parts, xmodel skinning, anim index, heads and tags, the anim script (`pmove::anim`), ragdoll, the view weapon's tree |
| [cod4-client-view.md](cod4-client-view.md) | Client clock (`CL_AdjustTimeDelta`), snapshot lerp, teleport cuts, view weapon, prediction (`play::cod4_predict`), lean (7), shellshock (8), hardpoint jets and helicopter (11), depth of field (12), the gunner's view (13), recoil and kicks (14), hidden parts (15), third person (16) |
| [cod4-hud.md](cod4-hud.md) | Fonts, server commands, scoreboard, kill feed, chat, hudelems, weapon HUD, compass, killcam, the script menus a join answers, damage overlays |
| [cod4-sound.md](cod4-sound.md) | Alias lists, loaded and streamed sounds, `channels.def`, falloff, event and loop sounds, createfx sounds |
| [cod4-effects.md](cod4-effects.md) | `FxEffectDef`, impact table, event effects, looping and tag-riding effects, createfx, client bullets and `whizby` |
| [cod4-voice.md](cod4-voice.md) | Voice chat: packets, relay rules, codec, talker list, mute |
| [cod4-front-end.md](cod4-front-end.md) | `ui_mp` main menu, menu stack, profiles, Create a Class and its stat commands, Start New Server, Mods, menu sounds and music, the Options pages (9), aspect and HUD scale (10) |
| [cod4-demos.md](cod4-demos.md) | Demos: what 1.7 keeps and changes from CoD2; screenshots and `cl_avidemo` (10) |
| [cod4-fastfile.md](cod4-fastfile.md) | `IWffu100` fastfiles: `XFile` blocks, pointer tokens, the `XAssetType` table, every asset body and its `Load_*`, the zone census; mods and usermaps, where a zone loads from (13) |
| [cod4-world.md](cod4-world.md) | Map worlds: vertices, surfaces, cells, portals, static models, the light grid and model lighting (3.1, 3.2), primary lights (3.5), the sun shadow map (3.6), techsets, images, collision indices, reflection probes (9.7), water (9.8), fog, vision sets and glow (11), draw order (12), dynamic entities (13), the map census |
| [cod4-gaps.md](cod4-gaps.md) | What a 1.7 client match shows that vcod still lacks |
| [cod4-server-gaps.md](cod4-server-gaps.md) | What a 1.7 dedicated server does that vcod-server still lacks |
| [cod4-map-sweep.md](cod4-map-sweep.md) | Every map under six gametypes on vcod-server and the client; the soak |

## Bots

| Doc | Covers |
|---|---|
| [bot-navigation.md](bot-navigation.md) | The nav graph (stock maps carry no path nodes), following, memory and hearing; stall tables and build times (the census floors) |
| [bot-objectives.md](bot-objectives.md) | What each gametype asks of a bot and the brain that plays it: CoD1, UO (vehicles included), CoD2 and CoD4 gametypes, bot management, team play |
| [bot-combat.md](bot-combat.md) | Skill levels, the `bot_match` harness and its numbers (1), cover, retreat, aim, footwork, before and after tables |
| [bot-measurements-cod2-cod4.md](bot-measurements-cod2-cod4.md) | Older CoD2 and CoD4 `bot_match` runs, moved out of `docs/bots.md` (vcod's own measurements, no retail claims) |

## Formats

| Doc | Covers |
|---|---|
| [bsp-ibsp59-format.md](bsp-ibsp59-format.md) | CoD1 `.bsp` (IBSP 59) lumps, patch and terrain collision, the leaf walk, movement constants |
| [xmodel-v14-format.md](xmodel-v14-format.md) | CoD1 xmodel descriptor, parts, surfs, collision, LOD selection, the model basis, the view fov |
| [xanim-v14-format.md](xanim-v14-format.md) | CoD1 xanim layout, timing, the view weapon's sight layer and hip pose, sampling, rotation key rebuild |

CoD2 and CoD4 formats sit under their games above.

## Perf

| Doc | Covers |
|---|---|
| [cod11-client-perf.md](cod11-client-perf.md) | CoD1 and UO frame time: profile stages, cuts, what is left |
| [cod11-client-soak.md](cod11-client-soak.md) | The CoD1 and UO client soak over every map and gametype |
| [cod2-client-perf.md](cod2-client-perf.md) | CoD2 frame time and the CoD2 soak; retail's compiled lit and cookie shaders (8), per-vertex fog and the cell walk (9), probe weight cubes and the shadow read (10), cookie receivers (11), the depth prepass (12) |
| [cod4-client-perf.md](cod4-client-perf.md) | CoD4 frame time: `VCOD_FRAME_PROFILE`, `tools/cod4_perf.sh`, the per-map table |
