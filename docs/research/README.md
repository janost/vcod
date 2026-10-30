# Research docs

Verified engine facts, with the binary addresses, assets and captures they rest on. Read a subsystem's doc before touching its code, and add what you learn to it. The wire reference for CoD 1.1 is `../protocol-1.1.md`; its last section lists every divergence from RTCW/Q3.

Every claim names the module, address and string or table it rests on and carries one evidence label: VERIFIED when read out of a binary, an asset or a live capture; INFERRED when read off control flow, instruction order or branch conditions. A section-level blanket label is never allowed, and a document-wide default only where the doc's provenance is uniform and every exception is labelled (AGENTS.md, "Evidence labels").

## CoD1 1.1

| Doc | Covers |
|---|---|
| [cod11-server-handshake.md](cod11-server-handshake.md) | Handshake, configstrings, rcon, heartbeat, zombies, pings, rate, bans, passwords, private slots, message pacing, downloads, CD key authorize |
| [clientstate-wire-format.md](clientstate-wire-format.md) | The `clientState_t` snapshot stream, `team`, the configstring map, entity types on the wire, inline submodels |
| [cod11-map-cycle.md](cod11-map-cycle.md) | `map`, `map_restart`, `sv_serverid`, rotation, the restart's configstring burst, compile carry-over |
| [cod11-combat.md](cod11-combat.md) | Shots, damage, death, grenades, blasts, hit locations, shot timing, the view weapon's offsets |
| [cod11-mantle.md](cod11-mantle.md) | Why 1.1 has no mantle; ladders, jump, step-up, slide; movement constants |
| [cod11-player-clip.md](cod11-player-clip.md) | Player clip, kerbs, getting stuck, `StuckInClient` |
| [cod11-movers.md](cod11-movers.md) | Movers, push, `linkTo`, brush model clip, player anim clock |
| [cod11-items.md](cod11-items.md) | Items, pickups, drops |
| [cod11-turrets.md](cod11-turrets.md) | Mounted MGs: mount, aim, fire, the gunner's pose |
| [cod11-hud-protocol.md](cod11-hud-protocol.md) | Hudelems, the script menu handshake, server cvar mirror (CS 140/204), crosshair, widescreen |
| [cod11-events-and-fx.md](cod11-events-and-fx.md) | Entity events and their effects, impact marks, module md5s |
| [efx-grammar.md](efx-grammar.md) | The `.efx` effect file grammar and how the 1.1 client plays it |
| [cod11-sound-system.md](cod11-sound-system.md) | Alias csvs, channels, falloff and panning, whizby, event aliases, footsteps, streams |
| [cod11-gsc-language.md](cod11-gsc-language.md) | gsc language semantics, threads, the script clock, engine notifies, runtime errors |
| [cod11-gsc-object-model.md](cod11-gsc-object-model.md) | gsc object model: entities, fields, arrays, builtins' side effects |
| [cod11-gametypes-re-bel.md](cod11-gametypes-re-bel.md) | The `re` and `bel` gametypes |
| [cod11-spectator-follow.md](cod11-spectator-follow.md) | Spectator follow, killcam |
| [cod11-console.md](cod11-console.md) | Client console, binds, `vcod_mp.cfg` |
| [cod11-demos.md](cod11-demos.md) | `record`, `demo`, the `.dm_<n>` format, `timedemo`, `nextdemo`, `cg_autodemo` |
| [cod11-front-end.md](cod11-front-end.md) | Main menu, server browser, options screens, Start New Server, script menus in the UI |
| [cod11-chat.md](cod11-chat.md) | Chat |
| [cod11-quick-chat.md](cod11-quick-chat.md) | Quick chat |
| [cod11-votes.md](cod11-votes.md) | `callvote`, `vote`, `CheckVote`, the vote lines on the HUD |
| [cod11-shellshock.md](cod11-shellshock.md) | `.shock` files, screen blend, kick, mouse, sound mix |
| [cod11-shader-scripts.md](cod11-shader-scripts.md) | Shader scripts: census, the grammar vcod implements, runtime math, sort order, sky |
| [cod11-light-grid-and-leaf-lights.md](cod11-light-grid-and-leaf-lights.md) | Light grid, prop and entity lighting |
| [cod11-gamma.md](cod11-gamma.md) | Gamma, overbright, `r_intensity` |
| [player-model-anim-system.md](player-model-anim-system.md) | Player models and anims: animscript, movetype pick, xanim header flags, pose |

## CoD1 1.5

| Doc | Covers |
|---|---|
| [cod15-protocol-diff.md](cod15-protocol-diff.md) | Protocol 6 against 1.1: tables, `pm_flags`, handshake, server commands, HTTP downloads |
| [cod15-movement.md](cod15-movement.md) | 1.5 movement against 1.1: jump penalty, stance, lean, step, weapons, stored floats |

## United Offensive 1.51

| Doc | Covers |
|---|---|
| [coduo-diff.md](coduo-diff.md) | Protocol 22: wire, configstrings, events, vehicles, sprint, gametypes, assets |
| [coduo-vehicles.md](coduo-vehicles.md) | Vehicle seats, the seated cmd, prediction and camera; server vehicles: spawn, drive, guns, damage, wrecks |
| [coduo-movement.md](coduo-movement.md) | UO movement against 1.5: sprint, fatigue, LMG bipod and mounts, grenade cook, weapon time truncation |

## CoD2 1.3

| Doc | Covers |
|---|---|
| [cod2-protocol.md](cod2-protocol.md) | Protocol 118: handshake, netchan, ops, netfields, configstrings, entities, events, vcod's server and its housekeeping |
| [cod2-gsc-diff.md](cod2-gsc-diff.md) | gsc against 1.1: builtins, methods, fields, `#include`, callbacks, the use key |
| [cod2-map-cycle.md](cod2-map-cycle.md) | `map`, `map_restart`, `fast_restart`, `map_rotate` against retail captures |
| [cod2-movement.md](cod2-movement.md) | Movement against CoD1, `vcod_cod2::pmove`, the moves A/B, client prediction |
| [cod2-combat.md](cod2-combat.md) | `PM_Weapon`, spread, antilag, bullets, damage, death, grenades, rockets |
| [cod2-killcam.md](cod2-killcam.md) | Frame archive, archived follow copy, the replay snapshot |
| [cod2-turrets.md](cod2-turrets.md) | Mounted guns: mount, view clamp, the gunner's frame, release, A/B |
| [cod2-player-anims.md](cod2-player-anims.md) | Player bodies: models, rig, anim array, controllers, death anims, corpse |
| [cod2-weapons.md](cod2-weapons.md) | Weapon files: field table, enums, view weapon anim tree |
| [cod2-hud.md](cod2-hud.md) | `hud.menu` owner draws, crosshair, damage icons |
| [cod2-fonts-menus.md](cod2-fonts-menus.md) | Fonts, menu keywords and script commands, alignment, the front end |
| [cod2-sound.md](cod2-sound.md) | Alias csvs, volume mod groups, falloff curves, sound events, loop sounds |
| [cod2-efx.md](cod2-efx.md) | `.efx` grammar against CoD1, spawn and motion, atlases, clouds, camera shake |
| [cod2-materials.md](cod2-materials.md) | Binary materials, techsets, techniques, statemaps, sampler states |
| [cod2-d3dbsp-format.md](cod2-d3dbsp-format.md) | `.d3dbsp` (IBSP 4) lumps, the collision model, trace A/B, lightmaps, light grid |
| [cod2-xmodel-v20-format.md](cod2-xmodel-v20-format.md) | xmodel v20 against v14: hit boxes, weights, materials |
| [cod2-iwi-iwd.md](cod2-iwi-iwd.md) | `.iwd` search path, `.iwi` images and the wavelet decoder |
| [cod2-demos.md](cod2-demos.md) | Demos: what CoD2 keeps and changes from CoD1 |

## CoD4 1.7

| Doc | Covers |
|---|---|
| [cod4-handshake.md](cod4-handshake.md) | Handshake, stats phase, pure check, netchan, Huffman, configstrings, what vcod-server sends |
| [cod4-netfields.md](cod4-netfields.md) | Netfield tables, delta codec, gamestate end, snapshots, the move message |
| [cod4-server-frame.md](cod4-server-frame.md) | `SV_Frame`'s order: cmds, zombies, archive, message order |
| [cod4-map-cycle.md](cod4-map-cycle.md) | `map`, `fast_restart`, `map_rotate`, the client's map change, votes |
| [cod4-gsc.md](cod4-gsc.md) | gsc dialect, builtin tables, the `war` boot, callbacks, connect to spawn, hardpoints |
| [cod4-pmove.md](cod4-pmove.md) | Player movement, the server's think, spawn, moves A/B |
| [cod4-combat.md](cod4-combat.md) | `WeaponDef`, `PM_Weapon`, shots, hit boxes, antilag, damage, death, grenades, melee |
| [cod4-collision.md](cod4-collision.md) | The clip map: leaf brush trees, triangles, submodels, static model clip |
| [cod4-player-anims.md](cod4-player-anims.md) | xanim parts, xmodel skinning, anim index, the server's anim script, ragdoll |
| [cod4-client-view.md](cod4-client-view.md) | Client clock, snapshot lerp, view weapon, prediction, hardpoint vehicles |
| [cod4-hud.md](cod4-hud.md) | Fonts, server commands, scoreboard, kill feed, hudelems, compass, killcam, script menus |
| [cod4-sound.md](cod4-sound.md) | Alias lists, loaded and streamed sounds, `channels.def`, event and loop sounds |
| [cod4-effects.md](cod4-effects.md) | `FxEffectDef`, impact table, event effects, looping and createfx effects |
| [cod4-voice.md](cod4-voice.md) | Voice chat: packets, relay rules, codec, talker list, mute |
| [cod4-front-end.md](cod4-front-end.md) | `ui_mp` main menu, menu stack, profiles, Start New Server, Mods |
| [cod4-demos.md](cod4-demos.md) | Demos: what 1.7 keeps and changes from CoD2 |
| [cod4-fastfile.md](cod4-fastfile.md) | `IWffu100` fastfiles: `XFile` blocks, pointers, asset list, body layouts |
| [cod4-world.md](cod4-world.md) | Map worlds: surfaces, cells, portals, static models, techsets, images, collision indices, fog, draw order |
| [cod4-gaps.md](cod4-gaps.md) | What a 1.7 client match shows that vcod lacks, ranked |
| [cod4-server-gaps.md](cod4-server-gaps.md) | What a 1.7 dedicated server does that vcod-server lacks, ranked |
| [cod4-map-sweep.md](cod4-map-sweep.md) | Every map under six gametypes on vcod-server and the client; the soak |

## Bots

| Doc | Covers |
|---|---|
| [bot-navigation.md](bot-navigation.md) | The nav graph (stock maps carry no path nodes), following, memory and hearing; stall tables and build times |
| [bot-objectives.md](bot-objectives.md) | What each gametype asks of a bot and the brain that plays it: CoD1, UO (vehicles included), CoD2 and CoD4 gametypes, bot management, team play |
| [bot-combat.md](bot-combat.md) | Skill levels, the `bot_match` harness and its numbers, cover, retreat, aim, footwork |

## Formats

| Doc | Covers |
|---|---|
| [bsp-ibsp59-format.md](bsp-ibsp59-format.md) | CoD1 `.bsp` (IBSP 59) lumps, patch and terrain collision, the leaf walk, movement constants |
| [xmodel-v14-format.md](xmodel-v14-format.md) | CoD1 xmodel descriptor, parts, surfs, collision, LOD selection, the view fov |
| [xanim-v14-format.md](xanim-v14-format.md) | CoD1 xanim layout, timing, the sight layer, sampling, rotation key rebuild |

CoD2 and CoD4 formats sit under their games above.

## Perf

| Doc | Covers |
|---|---|
| [cod11-client-perf.md](cod11-client-perf.md) | CoD1 and UO frame time: profile stages, cuts, what is left |
| [cod11-client-soak.md](cod11-client-soak.md) | The CoD1 and UO client soak over every map and gametype |
| [cod2-client-perf.md](cod2-client-perf.md) | CoD2 frame time and the CoD2 soak |
| [cod4-client-perf.md](cod4-client-perf.md) | CoD4 frame time: `VCOD_FRAME_PROFILE`, `tools/cod4_perf.sh`, the per-map table |
