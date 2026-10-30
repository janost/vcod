# vcod agent notes

Rust map viewer, client and dedicated server for Call of Duty 1 (2003), patch
1.1. `docs/features.md` says what it does and doesn't do. This file holds what the code
and configs don't tell you: where the evidence lives, how to measure against
retail, and the traps already paid for.

Retail is the oracle. When ours and retail disagree, retail is right, and the
answer goes into a research doc with the bytes.

## Layout

| Crate | Package | Owns |
|---|---|---|
| `crates/common` | `vcod-common` | `mapworld` (the loaded map every reader takes; `mapworld::load` picks the loader), formats (bsp, xmodel, xanim, shader, pk3, menus), the neutral `Material` the renderer draws (CoD1 shaders compile into it), collision, `clipmap/` (the CoD2-family clip map and its trace kernels: `vcod_cod2::collision` builds one from a `.d3dbsp`, `vcod_cod4::collision` from a clipMap_t), vis, pmove, weapons, animscript, `net/` (the 1.1 protocol both ways, 1.5 and UO 1.51 as per-connection `net::protocol::Cod1Wire` descriptors, plus the master query; `net/wire.rs` the `Transport` every game shares (version, master, codec, netchan, stock paks) and the `WireGame` trait each game's descriptor implements (its `id()`, game and version together, never the version alone: CoD4 1.7 and CoD1 1.5 both speak protocol 6; its connectionless framing, gamestate and snapshot writers); `net/codec.rs` is the game-neutral bit reader and table-driven field codec, `net/cod1/` CoD1's tables, configstring layouts and 1.1 numbering) |
| `crates/client` | `vcod` | window, renderer, HUD, fx, audio, `console/` (drop-down console, cvars, binds), `frontend/` (stock main menu, server browser, options screens, Start New Server, the server's script menus), `play/` (join, input, cmd clock, prediction), `session.rs` (`App`'s game: `Session::Cod1`, whose state is still `App`'s `mode`/`world`/`hud`, or `Session::Cod2`, whose online or demo session plays in `Mode::Cod2` (an offline `vcod <map> --game cod2` stays `Mode::Fly`), or `Session::Cod4` (`cod4_client.rs` over `play/cod4.rs`: a watched CoD4 capture or spectator join, also in `Mode::Cod2`; `cod4_audio.rs` and `cod4_fx.rs` its events' sounds and effects, `cod4_hud.rs` its 2D layer, `cod4_scoreboard.rs` its scoreboard, `cod4_viewmodel.rs` its first-person view weapon, `play/cod4_predict.rs` its prediction); one call per frame step, input, console effect, renderer build and quit), `cod2_client.rs` (the CoD2 session, pump, draw, view, HUD, menus and those steps, borrowing `App` through `Cod2Frame`), `probe.rs` |
| `crates/server` | `vcod-server` | `server.rs` (CoD1's frame), `spectate.rs` (usercmds in, playerstate out), `game/` (script host, combat, entities, links; `game/profile.rs` holds `HostProfile`, one static per game version naming every rule the host tells the versions apart by; a game's own host state is `GameHost::ext`, a `HostExt` arm, CoD2's `Cod2HostState` read through `host.cod2()` / `cod2_mut()`, CoD4's `Cod4HostState` through `host.cod4()`; `game/cod4/` loads CoD4 1.7's zones and runs its level: the gametype's scripts, the clients' playerstates and their pmove over the map's clip map), `bots.rs` + `nav.rs`, `bots/driver.rs` (the bots' per-tick driver over `BotWorld`; CoD1's `impl` and view builders in `server/bot_world.rs`, CoD2's `impl` in `cod2/bots.rs` over the readers in `bots/cod2.rs`), `server/bot_admin.rs` (bot admin both servers call: the pass, `addbot`, `removebot`, `listbots`, over `AdminHost`), `rcon.rs`, `console.rs`, `cvars/`, `master.rs` (heartbeat), `area.rs`, `engine/` (state and housekeeping both servers share: `game` `Engine<G: GameModule>`, the server as engine core plus game module, with `engine::tick` / `engine::packet`, the `SV_Frame` and `SV_PacketEvent` order over the module's hooks, `engine::send_messages`, the one `SV_SendClientMessages` pump, and `engine::answer_status`, the `getinfo`/`getstatus` reply (CoD1's `Cod1Game` in `server/cod1_game.rs`, CoD2's `Cod2Game` in `cod2/mod.rs`); `core` `EngineCore`, the engine state each embeds as `core`; `slot` `Link`, a client slot's connection half, each client struct's `link`; `link` LAN test, rates, pacing, ping; `archive` the killcam ring; `rotation` `map_rotate`'s walk; `challenge` the challenge table; `bans` `ban.txt` and the temporary bans), `cod2/` (the CoD2 engine: handshake, gamestate, snapshots, spectators; `house.rs` zombies, downloads, heartbeat), `cod4/` (the CoD4 1.7 engine, `Cod4Game`: handshake with stats and pure, gamestate, snapshots, the level's clients through `game::cod4::level::Cod4Level` (connect, menus, team, class, spawn, follow; no pmove yet), its server-command types and squash; its wire is `vcod_cod4::net::wire::PROTOCOL_17`) |
| `crates/cod2` | `vcod-cod2` | CoD2 1.3's formats, wire, pmove and collision; `map.rs` is the map loader: a `.d3dbsp` parsed once (`MapFile`) into the `MapWorld` and the `ClipMap` (`LoadedMap`, built side by side), `ClipMap::build` reading `ClipLumps`, never the `D3dBsp`; the server's collision and combat and the client's prediction share one `Arc<ClipMap>` per load |
| `crates/cod4` | `vcod-cod4` | CoD4 1.7's formats: `zone.rs` reads a fastfile (`zone/<language>/*.ff`) up to its asset bodies (container, `XFile`, script strings, asset list); `walk.rs` walks every body block-exactly into `assets/` (one module per family; bulk data kept under `WalkOptions::keep_bulk`); `install.rs` finds the install through `COD4_DIR`; `map.rs` builds a map zone's render world, cells, static models, entities and range-checked collision counts; `image.rs` decodes zone pixels and `.iwi` v6; `source.rs` is CoD4's `AssetSource` (zones over the iwds); `anim.rs` turns xmodels and xanims into vcod-common's `Model` and `XAnim` and serves the player tree through `BodySource` (`AssetSource::decoded_anim`/`decoded_model`); `techset.rs` classifies techsets by name; `hud.rs` the HUD's server commands, scores, hudelems, `[{+cmd}]` directives, ammo slots and zone assets (fonts, images, strings, kill icons, message windows, compass, `weaponinfo`, rank tables); `sound.rs` the alias rows, `channels.def` and loaded sounds as `.wav`; `fx.rs` an effect element's `FxElemDef` and samples; `ragdoll.rs` the client ragdoll (`ragdoll.cfg`'s defs, the 1.7 dvars, the solver `play::cod4_players` runs on a corpse); `net/` the 1.7 wire (Huffman, netfield tables generated by `tools/re/cod4_netfields.py`, delta codec, gamestate and snapshot parse, writer, `pure.rs` the `cp` checksum, `stats.rs` the stats block, `wire.rs` `Cod4Wire`/`PROTOCOL_17` and the server's gamestate builder and snapshot writer, `events.rs` the event names generated by `tools/re/cod4_events.py`, `conn.rs` the join and move messages `examples/cod4_probe` and the client share); `pmove/` 1.7's player movement, seeded from CoD2's with its own playerstate (`player_step`, `PmWorld` over the clip map, `wire.rs` to the netfield words; `weapon.rs` the weapon half with the offhand and the melee, `anim.rs` the anim script the server's step runs through `player_step_animated`) |
| `crates/gsc` | `vcod-gsc` | the `.gsc` compiler and VM (`vm/interp.rs`, `vm/sched.rs`; `vm/profile.rs`, a game's `VmProfile`) that runs Activision's stock gametype and map scripts |

- Dependency rules: `common` imports no wgpu, winit or kira; `gsc` depends on
  neither `common` nor the client. Check with `cargo tree -p vcod-common -e
  normal --depth 1` and `cargo tree -p vcod-gsc -e normal` (only `anyhow` and
  `log`; `vcod-common` is a gsc dev-dependency for the corpus tests).
- Assets load by kind and name through `vcod_common::asset_source`
  (`load::<T>`, an `Asset` impl per type; kinds so far: weapons,
  localized strings, sound aliases, raw files, images, CoD1's shader
  library, CoD2's compiled materials, xmodels, xanims, effects, impact
  tables, menu files, maps). Scripts, `default_mp.cfg`,
  `.stype` templates and other text read as `RawFile`, and a reader that
  only reads takes `&dyn AssetSource`, not `&Pk3Fs`. A load is cached
  until `Pk3Fs::clear_references` (or an `overlay`) drops the cache, so
  within a level only its first load marks the pak it read. An `Image`
  (by path, extension included) builds uncached through `Asset::find`:
  the client's texture caches own it, and `assets::DecodedImages` keeps
  `&Pk3Fs` for its unflagged prefetch and `mark_read`. `ShaderLib::load`
  is cached (one parse per source; a `.shader` read marks no pak);
  `vcod_cod2::materials::load_lib` builds `Compiled` uncached, as the
  renderer owns its `MaterialLib`. A `Model` (v14 or v20, by its
  `xmodel/` name) builds uncached too (`model::load` is `Model::find`):
  the client's model caches and the server's hit rigs own it, and the
  renderer's `upload_dynamic_model` and the multicore prop load keep
  `&Pk3Fs`. An `XAnim` (CoD1's or CoD2's, by its `xanim/` name) builds
  uncached the same way (`xanim::load` is `XAnim::find`): every reader
  keeps its own clip cache (`xanim::load_cached`, `PlayerAnimSet`,
  `HitRigs::clip`, the view weapons). The client's `fx::efx::Effect`
  (by `fx/` path; a CoD2 source parses through `vcod_cod2::efx::Effect`,
  kind `Cod2Effect`) and both games' impact tables build uncached:
  `FxSystem`, the impact table static and `Cod2Fx` keep them. A
  `ui_menu::MenuFile` (by path, parsed in the source's game's grammar,
  `#include`s read) builds uncached: the UI and both HUDs keep its
  menus. `ui_menu::game_types` keeps `&Pk3Fs` for `file_list`'s
  search-path order. A map loads by name, uncached, as the level owns
  it: CoD1's `MapWorld` (`mapworld::find`, `maps/` then `maps/mp/`, as
  `Pk3Fs::resolve_map`) and CoD2's `vcod_cod2::map::MapFile` (kind
  `Cod2Map`, `maps/mp/<name>.d3dbsp`; `LoadedMap::find` builds both
  halves). The client's CoD2 window load (`load_cod2_map`) reads the
  bytes itself for `Com_LoadBsp`'s error and the parse's panic guard.
- Grep these, don't read them whole: `client/src/probe.rs` (11k lines),
  `server/src/server.rs` (10k), `common/src/pmove.rs`, `client/src/renderer.rs`
  (5k each), `server/src/game/script.rs`, `client/src/main.rs`,
  `common/src/collision.rs`, `server/src/spectate.rs` (3k+ each).
  `docs/research/cod11-combat.md` is 300 KB; jump by section.
- Gitignored and local to one machine: `private/` (GPL lineage sources, the
  retail 1.1d Linux server, old plans), `docs/design/`,
  `docs/superpowers/`, `tmp/` (scratch; probe captures land there).

## Research docs

`docs/protocol-1.1.md` is the wire reference; its last section lists every
divergence from RTCW/Q3, so check it before assuming Q3 semantics.
`docs/research/*.md` hold verified engine facts with binary addresses. Read
the subsystem's doc before touching it, and extend it with what you learn
instead of leaving the fact in a commit message.

| Subsystem | Doc |
|---|---|
| Handshake, configstrings, rcon, heartbeat, zombies, pings, rate, bans, `g_password`, private slots, ping limits, message pacing, downloads | `cod11-server-handshake.md` (Housekeeping section) |
| Patch 1.5 (protocol 6) against 1.1: tables, `pm_flags`, handshake, server commands, HTTP (www) downloads | `cod15-protocol-diff.md` |
| Patch 1.5 movement against 1.1: jump penalty, stance, lean, step, weapons | `cod15-movement.md` |
| United Offensive 1.51 (protocol 22): wire, configstrings, events, vehicles, sprint, gametypes, assets, scope | `coduo-diff.md` |
| UO vehicle seats: getting in and out, the seated cmd, prediction and the camera in a seat; the server's vehicles: spawn, seats, the drive step, guns, damage; wheel dust, a vehicle's death | `coduo-vehicles.md` |
| United Offensive movement against 1.5: sprint, fatigue, the LMG bipod and its mounts, the grenade cook, weapon time truncation | `coduo-movement.md` |
| CoD2 1.3 (protocol 118) wire against 1.1: handshake, netchan, svc/clc ops, usercmd, netfield tables and codec, configstrings, entity types and events, the `Cod2Wire` descriptor and its CoD1 shim, map change and restarts, the client's spectator and join, the client commands, cheats, follow, votes and console, vcod's server and its housekeeping (slots, passwords, ping limits, pacing, zombies, downloads, the master: 17) | `cod2-protocol.md` |
| CoD2 1.3 gsc against 1.1: builtin, method, field and spawn tables, `#include`, `waittillframeend`, compile-time typing, callbacks, engine notifies, the stock gametype census, the use key's one list (items, guns, use triggers) and cursor hint | `cod2-gsc-diff.md` |
| CoD2 1.3 map cycle on vcod-server: `map`, `map_restart`, `fast_restart`, `map_rotate`, `sv_serverid`, the round and intermission flows against retail's captures | `cod2-map-cycle.md` |
| CoD2 1.3 movement against CoD1: function map, stored floats, usercmd buttons, the trace without `endpos`, clip and slide plane order, inertia, jump cap, mantle, stance, flags, tunables, what a CoD2 `MoveRules` needs, what `vcod_cod2::pmove` ports, its A/B gate against retail (`moves_ab`) and its gaps, the client's prediction and first-person eye | `cod2-movement.md` |
| Map change, `map_restart`, `sv_serverid`, rotation | `cod11-map-cycle.md` |
| Shots, damage, death, grenades, blasts, hit locations, shot timing | `cod11-combat.md` |
| Movement, stances, constants | `cod11-mantle.md`, `bsp-ibsp59-format.md` (Movement constants) |
| CoD2 1.3 maps (`.d3dbsp`, IBSP 4): lumps, the collision model and what the trace walks, the trace A/B against retail (`trace_ab.rs`, `examples/trace_probe.rs`), lightmaps, light grid | `cod2-d3dbsp-format.md` |
| Player clip, kerbs, stuck | `cod11-player-clip.md` |
| Movers, push, `linkTo` | `cod11-movers.md` |
| Items, pickups, drops | `cod11-items.md` |
| Mounted MGs | `cod11-turrets.md` |
| HUD elems, the script menu handshake, server cvar mirror (CS 140/204) | `cod11-hud-protocol.md` |
| Events and effects, impact marks, module md5s | `cod11-events-and-fx.md`, `efx-grammar.md` |
| Sound | `cod11-sound-system.md` |
| gsc language and object model | `cod11-gsc-language.md`, `cod11-gsc-object-model.md` |
| `re` / `bel` gametypes | `cod11-gametypes-re-bel.md` |
| Spectator follow, killcam | `cod11-spectator-follow.md` |
| Client console, binds, `vcod_mp.cfg` | `cod11-console.md` |
| Demos: `record`, `demo`, the `.dm_<n>` format, `timedemo`, `nextdemo`, the `RECORDING` line, `cg_autodemo` | `cod11-demos.md`; CoD2's `cod2-demos.md`, CoD4 1.7's `cod4-demos.md` |
| Main menu, server browser, options screens, Start New Server, script menus in the UI | `cod11-front-end.md` |
| Chat, quick chat | `cod11-chat.md`, `cod11-quick-chat.md` |
| Votes: `callvote`, `vote`, `CheckVote`, the vote lines on the HUD | `cod11-votes.md` |
| CoD4 1.7 map cycle (`map`, `fast_restart`, `map_rotate`), the client's map change, votes, the menu answers' one-packet rule | `cod4-map-cycle.md` |
| Bots | `bot-navigation.md` (its Build times set the census floors), `bot-objectives.md`, `bot-combat.md` (skill levels, the `bot_match` harness, cover and lean, retreat, section 6's one table of them all, team spread, awareness, aim and footwork, varied peeks and hunts) |
| Player models, anims, formats | `player-model-anim-system.md`, `xmodel-v14-format.md`, `xanim-v14-format.md` |
| CoD2 1.3 xmodel / xmodelparts / xmodelsurfs v20 against v14: hit boxes, weights, materials, what the server loads | `cod2-xmodel-v20-format.md` |
| CoD2 1.3 player bodies: models from the client state, the `j_*` rig and its controls, the anim array, angle updater, controllers, lerp frames; the legs and torso anims pmove and `PM_Weapon` pick, the hit flinches and stumbles, the death anim and the corpse | `cod2-player-anims.md` |
| CoD2 1.3 weapon files: the loader's field table and types, the enum tables, what changed from 1.1, the stale stock keys; the view weapon's anim tree, rates and `weapAnim` mapping, clip indexes | `cod2-weapons.md` |
| CoD2 1.3 combat: `PM_Weapon`'s states and events, the sight and the spread, `FireWeaponAntiLag`, `Bullet_Fire_Extended`, `G_Damage`'s hit-location multipliers, `finishPlayerDamage`, `player_die` and `deathAnimDuration`, `cloneplayer`, `obituary`, grenades, rockets and blasts; what vcod-server's CoD2 level does | `cod2-combat.md` |
| CoD2 1.3 killcam: the frame archive, the archived follow copy and its time shift, the replay snapshot, the dead spawn at its end, retail A/B (`cod2_killcam_ab`) | `cod2-killcam.md` |
| CoD2 1.3 mounted guns: spawn, the use key's mount, pmove's view clamp, the cursor hint, the gunner's frame (aim, placement, fire, loop sound), release, the gunner's eye, prone and duck guns, `spawnTurret`, a gunner killed on the gun, other gunners on the client, the retail A/B (`cod2_turret_ab`) | `cod2-turrets.md` |
| CoD2 1.3 player HUD: `hud.menu`'s owner draws (ammo counter, weapon name, stance, compass, offhand, hints, low-health overlay), the crosshair, damage icons | `cod2-hud.md` |
| Shader scripts, light grid, prop and entity lighting | `cod11-shader-scripts.md`, `cod11-light-grid-and-leaf-lights.md` |
| Gamma, overbright, `r_intensity` | `cod11-gamma.md` |
| CoD2 `.iwd` search path, `.iwi` images (header, formats, the wavelet decoder and where its tables come from) | `cod2-iwi-iwd.md` |
| CoD4 1.7 fastfiles: the `IWffu100` container, `XFile` and its nine blocks, pointer tokens, script strings, the asset list and the 1.7 `XAssetType` table, every asset type's body layout and the 1.7 `Load_*` addresses, the zone census (`COD4_DIR`, `COD4_LNXDED_DIR`) | `cod4-fastfile.md` |
| CoD4 1.7 map worlds: world vertices and surfaces, cells and portals, static models, techset names, zone and `.iwi` v6 images, the collision map's indices, the map census | `cod4-world.md` |
| CoD4 1.7 wire: handshake, stats phase and the stats block, the pure `cp` check, netchan, Huffman, configstrings, what vcod-server's 1.7 server sends | `cod4-handshake.md` |
| CoD4 1.7 collision: the clip map as the shared kernels trace it, prebuilt leaf brush trees, indexed triangles and walkable edges, submodels, static model point clip, clip census | `cod4-collision.md` |
| CoD4 1.7 netfield tables, delta codec, gamestate end, snapshots, the move message | `cod4-netfields.md` |
| CoD4 1.7 player movement: the kept float model, flags and buttons, ground, step, moves, stance and prone, the view on the wire, the jump, the sprint, the dvar defaults, the moves A/B (`cod4_moves_ab`) and its gaps; the server's think, the spawn's own think, the breath, `placeSpawnPoint`, hidden entities and no PVS cull, LAN pacing, the walk replay (`cod4::wire_ab`) | `cod4-pmove.md` |
| CoD4 1.7 combat on vcod-server: the `WeaponDef` offsets, `PM_Weapon` and the sight, the shot, hit boxes, antilag, damage, corpse and death anim, obituary, dropped weapon and its ammo, the killcam's look back and entity archive, the combat A/B (`cod4_combat_ab`); the offhand, grenades and the launcher (missiles, bounce, `G_RadiusDamage`), the melee, `kill` | `cod4-combat.md` |
| CoD4 1.7 player bodies: xanim parts and their stream order, xmodel bones and skinning, the player anim index, heads and tags, trajectories, the anim script pmove runs on the server (`pmove::anim`), the corpse's ragdoll | `cod4-player-anims.md` |
| CoD4 1.7 HUD: the zones' fonts and HUD images, the server commands, the scores and scoreboard, the kill feed, chat, hudelems and their key names, the weapon HUD and its clip graphic, the compass and its pointers, the killcam, the script menus a join draws and answers (their expressions and scripts), the low-health overlay and hit direction icons | `cod4-hud.md` |
| CoD4 1.7 sound: alias lists and loaded sounds in the zones, streamed files, `channels.def`, the alias flags, falloff and speaker maps, the event sounds and where they play, entity loop sounds, `EV_SOUND_ALIAS` and its master arm, `EV_STOPSOUNDS`, the createfx sounds | `cod4-sound.md` |
| CoD4 1.7 effects: `FxEffectDef` and `FxElemDef`, the sampled graphs, the impact table, the event effects, the earthquake, on vcod's CoD2 effect path; looping elements, tag-riding flashes, the map's createfx ambience, `ET_FX` / `ET_LOOP_FX`, client-side bullets and `whizby` | `cod4-effects.md` |
| CoD4 1.7 watched view: the client clock (`CL_AdjustTimeDelta`), the snapshot pair and lerp, the teleport cuts, the view weapon (rig, `weapAnim` nodes, bob, sway, fov), prediction (`play::cod4_predict`: the cmd clock, error decay, step smoothing, the weapon half, own events off the predicted ring), the hardpoint jets and helicopter (`cod4_vehicles`: rotors, wash, stage smoke, `playFxOnTag` bolts) | `cod4-client-view.md` |
| CoD4 1.7 gsc: the dialect over CoD2's (`waittillmatch`, shifts), the builtin tables (`tools/re/cod4_builtins.py`), the handlers the `war` boot reaches, the configstring allocator's constant slots, the callbacks; the connect, menus, loadout and spawn on vcod-server, the server commands' types and squash, the follow (`cod4::wire_ab`); the hardpoint planes and helicopter, the script traces | `cod4-gsc.md` |
| Shellshock: `.shock` files, screen blend, kick, mouse, sound mix, `wbuttons` 0x4 | `cod11-shellshock.md` |
| CoD4 client frame time: `VCOD_FRAME_PROFILE`, `tools/cod4_perf.sh`, the per-map table, what to cut next | `cod4-client-perf.md` |
| CoD1 and UO client frame time: the profile's stages on that path, `cod4_perf.sh GAME=cod1/uo`, the before and after of the cuts, what is left | `cod11-client-perf.md` |
| CoD2 client frame time: `GAME=cod2 tools/cod4_perf.sh`, where a CoD2 frame goes, the cuts | `cod2-client-perf.md` |
| CoD4 client: what a 1.7 match shows that vcod still lacks, ranked | `cod4-gaps.md` |
| CoD4 server: what a 1.7 dedicated server does that vcod-server lacks, ranked | `cod4-server-gaps.md` |
| CoD4 1.7 front end: the `ui_mp` zone's main menu and its screens, the menu stack, profiles, Start New Server, Mods, `error_popmenu`, menu sounds and music | `cod4-front-end.md` |
| CoD2 effects: `.efx` grammar, channels and curves, CoD1 migration, spawn and motion, atlases, `Cloud`, `CameraShake`, the spectator's event and looped effects | `cod2-efx.md` |
| CoD2 1.3 materials: the binary material file, techsets, techniques, statemaps, state bits, sampler states, shader families, image names, surface flags the server reads | `cod2-materials.md` |
| CoD2 1.3 fonts (`fonts/*`, glyph records, scale), the menu keyword and script command tables, `rect` alignment, hudelem screen placement, the front end (LAN browser columns, menu-on-dvar scripts, `g_scriptMainMenu`) | `cod2-fonts-menus.md` |
| CoD2 1.3 sound: the 24-column alias csvs, `volumemodgroups.def`, `.vfcurve` falloff, `loadspec`, the pick and secondary aliases, the cgame's sound events, whizby, loop sounds, sound server commands and ambient | `cod2-sound.md` |

### Evidence labels

Every claim in a research doc names the module, the virtual address and the
string or table it rests on, and carries its own label. VERIFIED: read out of
a binary, an asset or a live capture. INFERRED: read off control flow, which
includes instruction order and branch conditions ("then", "followed by",
"when that field is non-null"). One label per claim. A document may open with
a document-level VERIFIED default only when its provenance is uniform and
every exception is labelled; a section-level blanket, including one in a
heading, is never allowed. Docs carry derived facts (offsets, tables, enum
orders), never pasted decompiler output. `crates/common/tests/evidence_labels.rs`
catches two mechanical shapes; its header lists what a reader still has to
check.

## Workflow

- Feature work happens on a branch in a worktree. Master is merge-only: `git
  fetch`, merge `origin/master` into the branch, then `git merge --no-ff` with
  `merge <branch>: <summary>`. Other sessions push to master too.
  Conventional prefixes (`feat:`, `fix:`, `docs:`, `test:`, `perf:`,
  `refactor:`, `style:`, `chore:`).
- Before merging a feature or a fix, reread every line of AGENTS.md,
  README.md and `docs/features.md` / `docs/usage.md` that touches what the
  branch changed (the feature and limitation lists included), and commit a
  fix on the branch for each one the work made stale or wrong. The README
  stays a short summary; detail goes in the two docs. The merge waits until
  all of them match the code.
- Before a commit, run what CI runs: `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Set
  `COD_DIR` for the suite: tests that need paks go through
  `vcod_common::testing::game_fs()` and pass vacuously without it.
- The suite is green under each install: `COD_DIR` at a 1.1 install runs the
  1.1 tests and gates, at a UO install (`uo/` beside 1.5's `main/`) the UO
  ones and whatever reads only data the two share. A test that asserts 1.1's
  assets or replays a 1.1 capture takes `testing::game_fs_11()`, `None` at a
  1.5 or UO install; a UO one takes `testing::uo_fs()`. One that asserts
  what the stock paks hold (menus, game types, a census) takes
  `testing::stock_fs()` / `stock_fs_11()`, the `--vanilla` search path,
  so a third-party pak in `main/` cannot fail it.
- `ci.yml` runs on ubuntu with no game data, so a new test needing paks
  returns early through `game_fs()` or uses `Pk3Fs::empty()`: no pak test
  and no A/B gate runs in CI, only on a machine with an install. `nightly.yml`
  rebuilds the rolling `nightly` release on master pushes that touch code
  (docs, tools and `*.md` are path-ignored); `workflow_dispatch` forces one.
- `renderer::pipeline_check` builds the real `Renderer` headless and draws a
  CoD1 and a CoD2 frame under wgpu validation; it needs a Vulkan (not GL)
  adapter with BC textures (lavapipe: `VK_DRIVER_FILES=<lvp_icd.json>
  WGPU_BACKEND=vulkan GALLIVM_PERF=nopt`) and otherwise skips, so run it
  after any shader or layout change.
- Each crate's integration tests are one binary named after the crate
  (`--test server`, `--test common`, `--test gsc`, `--test cod2`;
  `tests/main.rs`): a new file there needs its `mod` line, and a file's
  tests filter by its name (`-- vehicle_ab::`).
- Ignored tests to run when you touch their area, in release: `nav_census`
  (all stock nav graphs, ~8 s, 65 s of CPU: `cargo test --release -p
  vcod-server --test server -- --ignored nav_census::`) and the ignored
  tests in `server/tests/bots.rs` (`-- --ignored bots::`) (~5 s together, 40 s of CPU): three mp_ship
  six-bot runs, mp_ship's boat, mp_ship's head-on ladder pair, mp_ship's
  hold ladder top, mp_hurtgen's crease and mp_depot's roof ledges (a
  graph build each). With
  `COD_DIR` at a UO install the same two files carry UO's: `nav_census`'s
  19-map UO census (20-60 s by load, 340 s of CPU) and `bots.rs`'s
  mp_cassino six-bot tdm run, its walled-in spawns test and four
  mp_foy `bas` rounds (`uo_bots_breach_bases_until_a_bas_round_ends`,
  the two `four_*_bas_round` runs, `a_seeded_bas_match_replays_beside_another`),
  which hold the file at ~7 min in release, and `tests/bot_vehicles.rs` its bots boarding,
  driving and firing mp_cassino's tanks, rockets at one and
  mp_kharkov's jeep (~30 s); with a 1.1 install they
  return at once. `--example bot_vehicles` (UO) plays each seed with
  bots' vehicles off and on (`bot-objectives.md` 10.6). `cargo run
  --release -p vcod-server --example ship_stalls` is the stall detector
  behind `bot-navigation.md`'s tables (`UO=1` for a UO server), and
  `--example nav_build` the build-time table's (`UO=1`, `RULES=`,
  `GAME=cod4` for CoD4's graph over its clip map); its
  `digest` matches across two builds only when they built the same graph,
  the check for a collision change meant to move no trace.
- `COD2_DIR=... cargo test -p vcod --bins -- --ignored cod2_map_census` runs every stock CoD2 map through vcod-server, a spectating client, a headless frame (lavapipe env and `nice` as for `pipeline_check`; `VCOD_CENSUS_FRAMES=0` skips it) and a walk from each spawn, and writes its table, frames and per-map warnings to `tmp/cod2-map-census/` (`play/cod2_census.rs` header).
- `COD4_DIR=... cargo test -p vcod --bins -- --ignored cod4_map_sweep --nocapture` runs every stock CoD4 map under war, dm, sd, sab, dom and koth: per map a vcod-server with six shooting bots rotating through the six (30 s limits) and one client playing it through `Cod4Client::pump` into a headless renderer (lavapipe env as for `pipeline_check`). It writes the per-pair table (load, end, rotate, client spawn, script errors, bot stalls, client and server warnings, panics) and the ranked errors to `tmp/cod4-map-sweep/summary.md`; run one map a process (`VCOD_SWEEP_MAPS=<map>`, `VCOD_SWEEP_JOBS=1`, about 3 GB and 6 to 8 minutes each) under `taskset -c 0-7` with `LP_NUM_THREADS=1` (`play/cod4_sweep.rs` header). Results: `cod4-map-sweep.md`.
- The `cod4_live_*` tests (`COD4_DIR`) each run a CoD4 vcod-server and client in process, on `play::cod4`'s `tests::sim` clock: a test's loops count frames its own sleeps move, so under load it runs slower, never out of time, and only `sim::Clock::sleep`'s 60 s wall assert stops a stalled one. The 92 of them take about 5 min of wall at 4 test threads and peak near 10 GB. Two such test binaries side by side under one memory cap (a 1.1 and a UO pass of the same suite, both with `COD4_DIR` set) sit past it, swap, and crawl for an hour instead of failing: run the CoD4 client tests once, and leave `COD4_DIR` unset for a CoD1 or CoD2 pass. Only the client's sleeps move the clock, never the wall (a client that computes, loads or blocks still runs no server frame), and every serve loop builds the bots' graph inline; a helper thread on the wall (`live_www`'s HTTP server) holds the server's frames while it works, or the client's waits run the level minutes ahead of it. A live test that writes frames (`VCOD_C4_SHOTS`) runs through `tests::run_held`, each frame loop sleeping its 16 ms first; the client reads the session's clock through `play::cod4::clock_now`, never `Instant`. Three runs of one build then write the same bytes for every frame (cod4-client-perf.md 39).
- `tools/cod4_soak.sh <outdir> [map...]` (`COD4_DIR`, the lavapipe env; `WRAP` names a wrapper command, default `nice -n 10`) runs the soak, `cod4_soak` in the same file: one map a process, war, dm, sd, koth and sab at 3 min limits, the client playing as a player (walks nav paths, fires, throws, dies, respawns, opens the board and the Esc menu) through `main`'s whole CoD4 frame, render and HUD included. Its `soak.md` adds the player's counts, wgpu errors, problem debug lines, audio misses, client frames and server ticks past 100 ms and the resident set per level (a `soak rss:` line at each load; under `LD_PRELOAD` jemalloc with `MALLOC_CONF=prof:true` it also dumps a heap profile there, cod4-client-perf.md 23); about 20 minutes and 3.5 GB a map. Method and results: `cod4-map-sweep.md`, "The soak".
- `tools/cod1_soak.sh <outdir> [map...]` (`COD_DIR`, `UO=1`, `LVP_ICD`, `MINUTES`, `GAMETYPES`) is CoD1's soak: per map a vcod-server with six shooting bots rotating through every gametype the map's arena lists, and the real `vcod` on a nested `kwin_wayland --virtual` joined as axis and played by `VCOD_SOAK` (`play::cod1_soak`) through `main`'s whole frame; `tools/cod1_soak.py <outdir>` writes `soak.md` (stalls by part and action, first in-game frame, warnings, script errors, panics, server ticks, resident set per level). About 6 minutes a 1.1 map and 11 a UO map at `MINUTES=1` (`UO=1 JOBS=2`: all nineteen in 105 minutes). Method and results: `cod11-client-soak.md`. A level's gamestate prewarms its models, effects and their images (`prewarm_level` in `main.rs`): a new thing the client draws or plays off an event goes in that list, or its first use stalls a frame.
- `tools/cod2_soak.sh <outdir> [map...]` (`COD2_DIR`, the same env; `cod4_soak.sh` with `GAME=cod2`) runs the CoD2 soak, `play::cod2_soak`, on the same harness: per map vcod-server `--game cod2` with six shooting bots rotates through dm, tdm, sd, ctf and hq (`VCOD_SOAK_MINUTES`), and the client plays through `main`'s `Mode::Cod2` frame. Same `soak.md` columns plus the frames past a level's first 5 s that built a mark material or loaded an fx image; about 10 minutes and 2 GB a map at 2 min limits. Each map load logs a `soak rss:` line (resident set and malloc's live bytes); a repeated `VCOD_SWEEP_GAMETYPES` list runs a map round more than once. Method and results: `cod2-client-perf.md`, "The soak".
- The all-maps census tests scope to stock `pak[0-9].pk3`. Map downloads drop
  third-party `zzz_*.pk3` into `main/`, and a census failure on one of those
  is not a regression.
- `Server::new` seeds its RNG with a constant; only `main.rs` passes the clock
  through `Server::with_seed`. Tests that depend on spawn picks rely on that.
  Either way each load steps a fresh script seed, as retail's `G_InitGame`
  does (`cod11-map-cycle.md` 4.8), so a later level never repeats the
  first's draws. A gate that compares a later level's spawn with retail's
  replays retail's picks through `Server::pin_spawns`, as `gametypes_ab`
  does.
- Comments: one line where the code doesn't explain itself (an invariant, a
  unit or sign convention, a workaround and what it works around). Facts that
  live in a doc stay there; the comment is the pointer.

## Running

- Both binaries expect to sit next to `CoDMP.exe` and read `main/` (`uo/`
  over `main/` with `--mod-dir uo`: any base other than `main` sits over
  its sibling `main/`, `pk3::base_dirs`). `COD_DIR` overrides the install,
  `--game-dir` overrides both. `--vanilla` on either keeps only the
  install patch's stock paks on the search path (`pk3::is_vanilla_pak`,
  `Pk3Fs::vanilla`). `--game cod1|cod2|cod4` overrides
  `game::Game::detect` (CoD2 by `main/iw_*.iwd` or `CoD2MP_s.exe`, CoD4
  by `zone/` or `iw3mp.exe`); both refuse any game but CoD1 (the client
  runs CoD2 in fly mode or its main menu, and `--connect` / `--net-probe`
  and the menu's `connect` (`cod2_front.rs`) spectate or join a CoD2
  server through `play::cod2`, not `NetClient`, and a CoD4 one through
  `play::cod4`, its script menus `play::cod4_menus`, its keys
  `play::cod4_input`;
  `vcod-server --game cod2` runs `cod2::Cod2Server`, an engine beside
  `Server` driving `game::cod2::Cod2Level`, cod2-protocol.md 14;
  `vcod-server --game cod4` runs `cod4::Cod4Server` over
  `game::cod4::level::Cod4Level`: the scripts, the menus, the spawns and
  pmove, no combat yet, cod4-handshake.md 9, cod4-pmove.md 15), whose version
  is the install's `Cod1Wire` (`is_uo`, `is_v1`). `pak1-4` are identical between 1.1 and 1.5
  and `pak0` is not (its checksum differs), so a 1.5 install works as the
  asset source but not on a pure 1.1 server; the RE notes are about the 1.1 binaries. The client
  also joins 1.5 servers (protocol 6, `net::protocol::PROTOCOL_V6`,
  `cod15-protocol-diff.md`) and plays United Offensive 1.51 ones, vehicles included
  (protocol 22, `PROTOCOL_V22`, `coduo-diff.md` 18; run it with the UO
  install as `--game-dir` and `--mod-dir uo`): it connects with its
  install's protocol and switches when refused, between 1.1 and 1.5 only.
  A UO install refuses a 1.1 or 1.5 server and the others a UO one
  (`Transport::same_game`, `NetClient::connect_from`); the net probe still
  switches to any.
  The server speaks one protocol per process, the install's
  (`server::install_protocol`: 1.5's `pak0` checksum, UO's `uo/pakuo*`)
  unless `--set protocol=` says otherwise; its per-version tables (weapon
  list, item rows, means of death) are `server::defs::Defs::of(proto)` and
  its configstring blocks `proto.cs`, never the 1.1 constants.
  `configstrings_15_ab`, `timeout_15_ab`, `legs_ab`'s 1.5 walks (the stop and go too) and `turret_ab`'s 1.5 replay
  run only with `COD_DIR` at a 1.5 install (a UO one's `main/` is 1.5's);
  `moves_ab`'s UO replays (sprint, mounts and cook),
  `configstrings_uo_ab`, `playerstate_uo_ab`, `playerstate_combat_ab`'s
  UO capture,
  `mount_uo_ab`, `uo_weapons_ab`, `legs_ab`'s UO walks and `turret_ab`'s UO replay only with it
  at a UO install (its `uo/` beside `main/`,
  `testing::uo_fs`).
- Client modes: `vcod <map>` flies, `vcod <map> --walk` walks offline, bare
  `vcod` opens the stock main menu and browser, `vcod --connect ip:port`
  joins through the stock team menu (`--team spectator` spectates).
  vcod has no listen server: Start New Server spawns the `vcod-server`
  beside `vcod` (build both), and the client kills it when it exits.
- `vcod-server <map>` runs ours. `dedicated` defaults to 1 (no heartbeat,
  unlike retail's 2); `--set dedicated=2` opts in. There is no stdin console:
  admin commands go through rcon, refused until `--set rconPassword=...`.
- `RUST_LOG` drives env_logger (default `info`); `WGPU_BACKEND` narrows the
  backend.
- `VCOD_FRAME_PROFILE=<path>` on `vcod` writes each frame's CPU ms by
  stage (net, bodies, fx, hud, cull, encode...), its counts and its GPU
  ms per pass group (`frame_prof`). `tools/cod4_perf.sh <outdir> <map>
  <secs>` runs a CoD4 server with 12 shooting bots and a spectating
  client under it in a nested kwin (`SESSION=1`: a window in the
  current session, on a real GPU) and prints the summary
  (`GAMETYPE=war` holds the view still for a GPU A/B, `TURN=<deg/s>`
  turns it on its own for a moving view's CPU); `GAME=cod2` with
  `COD2_DIR` runs CoD2's, whose free spectator holds still on any
  gametype, and `GAME=cod1` or `GAME=uo` with `COD_DIR` a CoD1 or UO
  server, whose spectator stands still. `docs/research/cod4-client-perf.md`
  has the per-map table, `cod2-client-perf.md` CoD2's,
  `cod11-client-perf.md` the CoD1 and UO numbers. `SIZE=WxH` sizes the
  client's window (`vcod --window`) in both modes; `SESSION=1` without it
  keeps the game's saved video mode.
  `SWEEP=1 SESSION=1 tools/cod4_perf.sh <outdir> mp_crossfire 10` holds
  one view in one session and toggles one setting a pass (MSAA, prepass,
  sun shadow, feather, distortion, `fx_draw`, specular, normal maps, fog,
  glow, dof, film), printing a row a pass with the finer `gpu.<group>`
  stamps (world, smodel, ents, gun, fx, each post pass);
  `cod4-client-perf.md` 25 has the command and what each column is.
  It restores the install's `vcod_mp.cfg` files at the end.
- `VCOD_TICK_TRACE=<path>` on `vcod-server` writes the tick's step log
  (`tick_trace`): a line per `SV_Frame` step, named after the retail
  function, with the slot it ran for, in execution order.
  `VCOD_TICK_PROFILE_MS=<ms>` logs each tick past `<ms>` with its phases'
  times and its wall time split into run, queued and asleep
  (`tick_profile`; the CoD4 soak's `VCOD_SOAK_TICK_LOG_MS` and
  `bot_match`'s `SPIKE_MS` print the same, cod4-server-frame.md 9).
- F3 (`--debug-overlay`) shows frame, draw, vis, shader, fog, audio, camera
  and (online) prediction, net and snapshot counters. The `audio` line reads
  `audio vN plays N miss N cull N drop N steal N N.NNms`: live voices, cues
  started, aliases not found, cues past `dist_max`, cues refused by a full
  pool, voices stolen. The `vis` line reads `vis: <mode> cells n/m soups a/b
  tris c/d props p/q occ o h X.XXms` (occluders built, portals they hid).
  The `draw` line ends `lod a/b/c gone N tris T`: dynamic model
  instances drawn at each LOD, those past their last LOD (not drawn), and
  the triangles drawn.
  The `clock` line reads `clock: behind N  reset N fast N +N -N  extrap N`:
  newest snapshot time minus the drawn time, then `CL_AdjustTimeDelta`'s
  resets, fast and slow adjusts and the frames that reached the newest
  snapshot (docs/protocol-1.1.md, "The client's clock").
  On a CoD2 server F3 adds `net: ping P  snaps N/s  msgs N/s  out N/s
  lost N (total)  frag N/s (total)  rate R snaps S [lan]`, the clock line
  under CoD2's rules and a `pred` line (`Cod2Session::debug_lines`,
  cod2-protocol.md 18); `--net-probe --game cod2` prints them as `link`.
  The `ssao` line reads `ssao <vc_ssao|off>  gpu scene X.XXms ao Y.YYms`:
  GPU timestamps around the world passes and the SSAO passes alone,
  smoothed; absent where the adapter has no timestamp queries.
- On a CoD4 map F3's `sunshadow` line reads `sunshadow sm_enable  casters
  Ak/Bk tris + N ents  gpu X.XXms` (or `sunshadow off`): the world and
  static model caster triangles drawn into both sun shadow partitions of
  all, the entity surfaces drawn into each, and both passes' GPU ms
  (`cod4_sunshadow`, cod4-world.md 3.6).
- With `vc_shadows 1` F3 adds `shadow: ents N static Ak/Bk tris cut Ck/Dk
  xN cpu X.XXms gpu static X.XX dyn X.XX mask X.XXms`: casting entity
  surfaces, static caster triangles drawn at the last static redraw (a
  strip of the static layer, or the square around the eye after a load
  or a teleport) of the total, the alpha-tested ones of each, static
  redraws so far, and the CPU and GPU (timestamp) cost of each pass.
  `RUST_LOG=vcod::shadows=debug` logs each timed static redraw's GPU ms
  (`start-over` marks the square) and every 120 readbacks the smoothed
  dynamic, mask and CPU ms.
- `VCOD_NETSIM="ping=100,jitter=20,loss=2"` gives the client a bad link
  without root; `vcod --net-probe ADDR --probe-clock` measures the clock on
  one.
- F4 cycles culling `on -> locked -> off`; `locked` freezes the visible set so
  you can fly out and inspect it.
- Pre-existing noise: a few `vkAcquireNextImageKHR` fence validation errors
  per run on Vulkan. A stock install loads with no shader parser warning
  (`stock_shaders_parse_without_warnings`); one is either a third-party
  pak or syntax vcod misreads. Retail's own complaints and vcod's
  approximations log at debug (`RUST_LOG=vcod_common::shader=debug`).
- To reproduce a screenshot spot in fly mode, add a temporary
  `VCOD_POS="x y z yaw"` override to the `FlyCamera::new` spawn in `main.rs`
  and remove it before committing.
- Map loads log their stages on one `load <what>: <total> ms (<stage>
  <ms>, ...)` line at info (`vcod_common::laps::Laps`): the client's
  world GPU state and CoD2 map, the CoD2 server's level and its parts.
  `cod2_load_profile` (ignored, in `renderer/pipeline_check.rs`, run on
  lavapipe as `pipeline_check` is, in release) loads three CoD2 maps
  twice through the renderer, the prediction's collision and the first
  draw, and prints a digest of every texture uploaded and the paks the
  load marked: a load made faster keeps both. The CoD2 loads read
  ahead on every core (`assets::DecodedImages::prefetch`,
  `Pk3Fs::read_unflagged`), so a read for a pak's referenced flag goes
  through `Pk3Fs::read` or `mark_read`, never the unflagged read alone.
  A map load resumes the last level's gametype compile
  (`Carry::gametype`) once every file its `Loader` read reads the same
  (`LoaderState::current`); the key is that text, the gametype entry and
  the builtin set, so anything else a compile comes to depend on (a
  cvar, a new `CompileOptions` field) goes in `GametypeScripts`' check
  or a map change runs the old compile (cod11-map-cycle.md 4.7).
- Bot play is measured, not eyeballed: `cargo run --release -p vcod-server
  --example bot_match [seed...]` runs seeded bot-vs-bot matches (`MAPS=`,
  `GAMETYPE=`, `BOTS=`, `TICKS=`, `JOBS=`, `SET=`, `UO=1`, per-side
  `AXIS_SKILL=`/`ALLIES_SKILL=`, and one-sided `COVER=`, `RETREAT=`,
  `TEAMPLAY=`, `AWARE=`, `FOOTWORK=`, `WARY=` arms; `GAME=cod2` runs
  `vcod-server --game cod2`'s bots instead, split by weapon class and
  skill, `GUNS=0` keeping them off the mounted guns, docs/bots.md "CoD2";
  `GAME=cod4` CoD4's, with each level's line and every script error the
  levels raised, docs/bots.md "CoD4"; `DIGEST=1`
  or `DIGEST=<N>` adds a per-tick hash of every bot's cmd and body, the
  check that a refactor moved no bot; `NETDIGEST=<N>` one of every
  datagram sent, with an observer client joined, and `VCOD_TICK_TRACE=<dir>`
  the tick's step log, bot-combat.md section 1) and prints kills,
  deaths, damage, time-to-kill, wins, objective actions, lives, wounds
  survived, retreats, cover holds, peeks and hunts and the deaths after
  them, hits answered, deaths blindsided, stalls (split from deliberate holds), shots, hits, accuracy,
  sight to first shot, and fights and how they end, re-engagements included, per team, skill and arm. It is the one bot measuring tool:
  extend it rather than add another example. A bot behaviour change
  reports its before and after on the same seeds in
  `docs/research/bot-combat.md`, whose section 1 says what each number
  means. Retail has no bots, so this is the only yardstick.
- Pixel and by-ear checks (aim sign, lean direction, shading, sound) belong to
  the human. Report them as pending.

## Measuring against retail

### Servers

- `tools/run_server.sh [map] [+set ...]` runs the **retail** 1.1d Linux
  dedicated server (setup in the script header). Anything after the map goes
  to the engine verbatim. `COD_VERSION=1.5` runs the 1.5b server instead;
  point `COD_DIR` at a directory whose `main/` holds only the stock 1.5 paks
  (pak0-pakb and the localized ones), since third-party pk3s in a 1.5
  install override the stock gametypes and break the script compile.
  `COD_VERSION=uo` runs the United Offensive 1.51b server (default map
  `mp_cassino`); its `COD_DIR` needs the 1.5 `main/` and the UO `uo/`.
  The net probe joins it with the UO install as `--game-dir` and
  `--mod-dir uo`; `cargo run -p vcod-common --example handshake_probe` is
  the standalone decoder `coduo-diff.md` was written with.
- `tools/run_probe_cod2.sh` runs retail `cod2_lnxded` with the CoD2
  movement probe gametype inside `unshare -rn` beside
  `crates/cod2/examples/move_probe.rs`, which writes a
  `crates/cod2/tests/fixtures/moves/` capture (`cod2-movement.md` 17).
- `tools/run_probe.sh <probe> [map] [+set ...]` boots retail with a
  `crates/gsc/tests/fixtures/semantics/probe_*.gsc` (or
  `client-probes/probe_*`) installed as the gametype and prints its `PROBE`
  lines; `COD_VERSION` picks the server as `run_server.sh` does (UO's
  gametypes and log live under the homepath's `uo/`). A worktree has no
  `private/`; link the main checkout's in (it is gitignored). Without
  `COD_LNXDED_HOME` both scripts run in a homepath of the port's own,
  `private/homes/<server>-<port>` (printed on stderr; `games_mp.log` is
  there), so two runs on two ports share no log, and its archived config
  is dropped each start. `PRE_MAP_ARGS` holds `+set`s that must land
  before `+map`. The retail engine runs at most 32 `+`
  commands and folds the rest of the line into the 32nd's value;
  `run_probe.sh` spends twelve, so a recipe gets twenty `+set`s.
  `tools/capture_probes.sh > .../retail-captures.txt` reruns the
  top-level probes for `crates/gsc/tests/semantics_ab.rs`. Read that
  directory's `README.md` before writing a probe: six engine behaviours
  dictate a probe's shape.
- `tools/capture_cvars.py <port> <rconPassword>` prints a running retail
  server's cvar registry as the rows of `crates/server/src/cvars/registry.rs`.
- `cargo run -p vcod-server -- <map>` runs **ours**. `--gametype-script
  <file>` runs a gametype from disk (how a client probe runs against ours),
  `--set NAME=VALUE` is retail's `+set`, `--bots N [--bots-shoot]` adds
  clients, `--trace` logs per-snapshot timing.
- Public servers: vcod's browser and heartbeat use `master.cod.pm`, whose
  operator allowed it: ports 20510 (CoD1, protocols 1-6), 20610 (UO, 21
  and 22), 20710 (CoD2, 115-118), 20810 (CoD4). Ask
  `getservers <protocol> full empty` with one protocol, never 0 (all
  games), at most once a second (`net::master::master_send_allowed`);
  a list may come in several packets ending `\EOT`, the last `\EOF`.
  Retail's Activision masters are not asked for lists. The client's one
  packet to them is `getKeyAuthorize`, sent only with a key set
  (`VCOD_CDKEY`, 1.1's `main/codkey`) and only to a server off the LAN
  (`net::cdkey`); tests and probes on loopback never send it, and no
  test may set `VCOD_CDKEY`. A 60-100 s spectator
  capture during a round shows every combat event.

### The probe client

`vcod --net-probe <ip:port>` is a headless client that joins, prints a
per-second snapshot summary, and dumps captures to `tmp/`. Each scripted mode
(`--save-*`, `--probe-*`) is documented on its flag in
`crates/client/src/main.rs` (`vcod --help`): what it does, which fixture it
writes, which gsc probe it pairs with. A multi-shell recipe (gsc probe, target
client, shooter client, `+set` cvars, start order, `--probe-secs`) is in the
header of the fixture it produced.

- Both servers treat loopback and private addresses as LAN and skip rate
  pacing for them. `tools/offlan_capture.sh retail|ours <dir>` runs a
  server and the probe in two rootless network namespaces with public test
  addresses and logs every datagram; `--probe-rate` / `--probe-snaps` set
  the probe's userinfo.
- **Committed fixtures are retail evidence.** Most modes write into
  `crates/server/tests/fixtures/` under a fixed name, so a run against ours
  overwrites the evidence. After one, move the files to `tmp/` and `git
  checkout` the directory. `--capture-tag` names a second run; tagged
  fixtures refuse to overwrite without `--overwrite-fixture`.
- A new mode builds on the probe's helpers, which encode retail's input
  rules: the loop echoes `ps.weapon` in every cmd (a different byte reads as
  a holster request), a switch goes through `WeaponSwitch`, fire through
  `Pulse` (stock rifles and pistols are semi-automatic, so taps at least
  `fireTime` apart), and joining through `JoinProbe`, which re-answers the
  team menu after every gamestate and `n`. Captures from before 2026-09-02
  carry fake putaways at reload, stance changes and jumps.
- Space client commands past 800 ms yourself; nothing on the client enforces
  it. Retail silently drops non-exempt commands (including bare `score`)
  inside the flood window; `team `, `score ` and `mr ` (with the space) are
  exempt.
- An event lives four slots in the ring, so a transient needs a `!trace` per
  snapshot, not a settled sample. Two probes on one server see each other's
  entities, so a two-client question doesn't need two retail clients. The
  entity list is position-dependent (`--probe-pvs`).
- `--save-fixture` / `--save-snapshots` rewrite the parser's byte-exact
  captures in `crates/common/tests/fixtures/net/`. The `*-delta.bin` pair came
  from a lone spectator on `run_server.sh mp_carentan` with
  `SNAP_CAPTURE_TARGET` raised to 400 and the files renamed by hand; the gate
  in `net/snapshot.rs` pins the frame counts, so a refresh hits the same count
  or updates the assertions. `--capture-tag <tag>` writes the pair as
  `*-<tag>.bin` and captures for all of `--probe-secs`; the `*-1.5.bin` pair
  is two spawned probes on `COD_VERSION=1.5 tools/run_server.sh
  mp_carentan`, the `*-uo.bin` pair a following spectator beside an axis
  probe on `COD_VERSION=uo tools/run_server.sh` (recipe on
  `protocol_22_capture_reencodes_and_maps`), the `*-uo-tank.bin` and
  `*-uo-jeep.bin` pairs the driver of a `--probe-vehicle` crew
  (`coduo-diff.md` 18), the `*-uo-overheat.bin` pair the same crew with a
  `--probe-vehicle overheat` gunner, the `*-uo-quake.bin`, `*-uo-flame.bin`,
  `*-uo-arty.bin`, `*-uo-smoke.bin`, `*-uo-mg.bin`, `*-uo-satchel.bin`,
  `*-uo-bazooka.bin` and `*-uo-bazooka-draw.bin` (under gdb) pairs a
  spectator following a shooter under `client-probes/probe_uofx` (the arty, smoke, mg and satchel ones
  `--probe-uo-weapon`, the satchel with the smoke script;
  `coduo-diff.md` 19, 21), the `*-uo-turret.bin` pair a
  spectator following a `--save-turret` gunner on
  `client-probes/probe_turret` (recipe in `coduo-diff.md` 19), the
  `*-uo-wreck.bin` pair a spawned probe watching
  `client-probes/probe_vehwreck` kill a tank (`coduo-vehicles.md` 15), the
  `*-panzer.bin` and `*-panzer-wall.bin` pairs a spectator following a
  `--probe-weapsel` panzerfaust shooter on 1.1d under
  `client-probes/probe_panzer` (`cod11-combat.md` 13.6), the
  `*-15-turret.bin` pair a spectator following a `--save-turret` gunner
  on 1.5b under `client-probes/probe_turret` (`cod11-turrets.md` 14.3),
  the `*-turret-hit.bin` pair a 1.1d `--save-turret` gunner's own
  snapshots while a `--save-hit` probe shoots him off the gun (same
  section), the `*-follow-1.1.bin` pair a 1.1d spectator following a
  `--save-hit` shooter under `client-probes/probe_passthru`
  (`cod11-spectator-follow.md` 11.1).
  `cargo run -p vcod-common --example event_dump`
  prints a capture's events in the wire's numbering.
- `--probe-uo-weapon` and `--probe-vehicle` send cmds at 60 Hz (0, 16
  and 32 or 33 ms past each frame's time; `ProbeHold::sixty` where a held
  trigger's fire rate matters, cod15-movement.md 6), each
  holding the view of their first spawned snapshot (or of the use tap),
  rebased on `delta_angles`. Against a gsc `setPlayerAngles` every frame
  that rebase lags by the link, so where the rounds go rides the client's
  timing and repeats neither on retail nor on ours; in-process
  (`tests/common::ProbeHold`) only a frame's first cmd takes the gsc's
  view. `--probe-uo-weapon aim` (`ProbeHold::aim`) sends every cmd at the
  snapshot's view, which a gsc steers round for round: measure with that
  (`coduo-vehicles.md` 13). `--probe-uo-weapon direct` does the same and
  takes its buttons, lean and stance from the gsc's `iPrintLn("PROBE_IN
  ...")` lines, so a phase starts on the server's clock
  (`client-probes/probe_leanhit`).
- `--probe-vehicle seats|drive --save-slope` writes a seated capture whose
  snapshot lines carry the vehicle's whole entity state (`veh=`), what
  `vehicle_ab` replays on ours (recipe in `coduo-vehicles.md` 8 and 14).
  It starts 3 s past the weapon pick, after the seat; `--slope-from-spawn`
  logs from the spawn, whose pick sets the seat's `legsAnim` toggle (13).
- On 1.5 (protocol 6) and UO (22) every mode reads through the
  connection's protocol (`probe::wire()`): netfield tables, configstring
  blocks, the event ring and temp entities, means of death. Event ids and
  means of death print in 1.1's numbering, as `pm_flags` and `eFlags` do
  once decoded (a UO-only event as `EV_FOREIGN | id`; UO's
  `EV_BULLET_HIT` and projectile explodes, which the client keeps as
  `EV_UO_*`, as the small hit and the grenade and rocket no-marks
  explodes); `eType`,
  configstring slots and weapon indices stay the wire's. A fixture-writing mode refuses to run off 1.1 without
  `--capture-tag` (both servers load mp_carentan, so give each its own
  tag). Run on both retail servers: the join, `--save-playerstate`,
  `-motion`, `-combat`, `-grenade`, `-configstrings`, `--probe-target`
  with `--save-hit`, `--probe-follow`, `--probe-pvs`, `--probe-weapsel`,
  `--probe-compass`, `--probe-edge watch`; on 1.5 also `--probe-items`
  beside `COD_VERSION=1.5 tools/run_probe.sh client-probes/probe_itemdrop`.
  `run_probe.sh` takes `COD_VERSION=uo`, but of the modes that pair
  with a `client-probes/` gametype only `--save-combat` beside
  `probe_uofx`, `--save-turret` beside `probe_turret` (its
  `mp_carentan-dm-turret-uo.txt` committed, as 1.5's `-15.txt`, and
  `-victim-uo.txt`, which logs the target's entity),
  `--probe-flame` beside
  `probe_flame` (`coduo-diff.md` 19, "The flame report"), `--probe-items`
  beside `probe_itemdrop` and `probe_fragcount` (`cod11-items.md` 4.1,
  14.7) and
  `--save-scripted` beside `probe_ctf`, `probe_dom`, `probe_hq`,
  `probe_bas`, their `*end` endings, `probe_ctftime`, `probe_tdmend`,
  `probe_hqtime`, `probe_bastime`, `probe_domtime` and `probe_menuclock`,
  `--probe-say` beside `probe_threadorder` and `--probe-uo-weapon direct`
  beside `probe_rise`, have run there. On UO's mp_carentan the
  grenade script throws its one frag and the steps after it hold nothing.

### The A/B gates

`crates/server/tests/*_ab.rs` replay a retail fixture's inputs on ours and
diff the result. Where a gate tolerates rows, it names them in a `*GAPS`
constant with the research section that explains each. Debug env vars
(`*_REPORT=1`, `*_TRACE=<ct>`, `*_DUMP=<path>`, `SLOPE_FIXTURE=<path>`) are
read near the top of each gate. Gates that need paks skip without `COD_DIR`.
A gate on a client probe whose watcher sleeps before it polls runs the
server alone for `common::PROBE_JOIN_MS` before its client connects, as
retail's client joined late: a body's idle clip runs from the spawn, and a
spawn before the first poll put `linkto_ab`'s bones a quarter second off
retail's (`cod11-movers.md` 15).

The client's gate is `crates/client/src/play/replay.rs`: slope captures
replayed as our own body, one frame per cmd, and the committed snapshot
captures as a follower, through the prediction, `play::events::frame_events`,
the kick, the effect resolver and the sound cues, each shot played once.
A new step in the frame's event path goes in `frame_events` so the replay
runs it. `crates/client/src/play/loopback.rs` plays vcod-server itself, in
process, through the same frame (clock, cmd packets, prediction, events,
kick) and counts each round fired against the ammo the server took.

## The server tick

Every game runs on one skeleton: `Server` is `engine::Engine<Cod1Game>`,
`Cod2Server` `engine::Engine<Cod2Game>`, `Cod4Server`
`engine::Engine<Cod4Game>`, each an `engine::EngineCore`
(`core`) beside the game. `engine::tick` (`engine/game.rs`) is retail's
`SV_Frame` order, calling the game's `GameModule` hooks:

1. `frame_head` (CoD1's `Cbuf_Execute`, its `quit` and `killserver`
   stop), then, when a frame runs, `HEAD`'s order of the bots, the pings
   and (CoD2, CoD4) configstrings 0/1, each once.
2. Per owed frame (at least `MIN_FRAMES`, CoD1's 1): the clock, the
   queued cmds under `ThinkTiming::InFrame` (CoD1's `replay_moves`; CoD2
   thinks each cmd as its packet arrives), `run_frame`, the archive
   between frames, `frame_out`.
3. `SV_CheckTimeouts` on the advanced clock (the stale zombies freed
   there), then `engine::send_messages`, the one `SV_SendClientMessages`
   pump: `snapshots_begin` once, then in slot order each slot
   `message_due` gets `next_fragment` if one is left, else
   `send_client_snapshot`, then `snapshots_end`. The pump serves every
   state on its own slot's turn: a zombie's message and a client not in
   the world's bare one go out between their neighbours' snapshots
   (`cod11-server-handshake.md`, "The order of a frame's messages").
4. The last frame's archive under `ARCHIVE_AFTER_MESSAGES` (CoD1's
   `snapshots_end` archives its own); the heartbeat.

Under `POST_EACH_FRAME` (CoD4 1.7, `cod4-server-frame.md`) the timeouts,
messages, archive and heartbeat follow every owed frame instead, so a
hitch sends one message per frame, and under `FRAGMENTS_FIRST` (CoD4 1.7)
the pump sends every due slot's fragment before any new message.

A tick is one `SV_Frame`: every frame the wall clock owes runs back to
back with only the archive between them, then one round of timeouts and
messages. `main.rs`'s `serve<G: GameModule>` is the loop for both games:
it runs `drain_console` before it counts what is owed, so a slow
`map_restart` is caught up with a single snapshot (`cod11-map-cycle.md`
4.7), and hands each socket packet to `Engine::packet_read` with its
`engine::Arrival` (CoD1 stamps the move acks there, `Server::ack_stamp`).
`engine::packet` checks `accepts_packets`, then splits `SV_PacketEvent`
on `is_connectionless` into the game's `connectionless` and
`client_packet`. A step whose order differs between games is one of the
consts or hooks, never a branch on the game; moving work across this
order makes a snapshot or a later round read a frame-old field.

CoD1's frame (`Server::game_frame`, `Cod1Module::run_frame_cod1` in
`server/cod1_frame.rs`) follows `G_RunFrame` / `ClientThink_real` /
`ClientEndFrame`, and the comments at each step cite the section that
measured it: every packet's cmds in arrival order, each one a
`ClientThink_real` with its shots traced and their callbacks run inside
the cmd (`cod11-combat.md` 16); menu responses, the script threads;
spawns, weapon, link and sim ops; the entity pass, items, links, movers,
missiles, thinks, temp entity frees and linked clients one entity at a
time by number, each blast walked on its grenade's turn and each mover's
push on its own (14.7); every slot's end frame, then per slot the aim
trace, `commit_pose` and that slot's turret, whose rounds deliver inside
its turn; outgoing commands. A silent client goes on its sixth stale
frame; a zombie is freed in `SV_CheckTimeouts`. CoD2's order is in
`cod2-protocol.md` 14.1.

The engine state both games drive is `EngineCore`: transport, slot table,
configstrings and their sent copy, console queue, challenges, bans,
masters, rcon limiter, the game clock and `rand()`. A slot's netchan,
acks, frame and ping rings, pacing, fragments and timeout clock are its
`link` (`engine::slot::Link`). What both servers answer alike over it
(`tick_frames`, `handle_packet`, `push_console`, `take_outgoing`) is one
`impl<G: GameModule> Engine<G>`. The frame writers are the wire's
(`WireGame::write_gamestate`, `write_snapshot`); a game builds its frame
in its own numbering (CoD1 maps 1.1's once a snapshot). `getinfo` and
`getstatus` reply through `engine::answer_status`: the game's
`GameModule::status` text framed by `WireGame::oob_packet`. `Cod1Game`
(`server/cod1_game.rs`) still holds the engine state CoD1 keeps itself
(zombies, pure check, rcon password and redirect, the killcam archive,
`cfg`) beside its game module, `Cod1Game::module` (`Cod1Module`); `Lend`
is what the engine lends the module for one call.

`Cx::spawn` runs a script thread before the calling builtin's next
instruction; `Cx::spawn_then` adds `Host::spawn_returned` once that thread
first suspends. Only a `Cx` handed to a builtin (or to `spawn_returned`)
honours either: a spawn through `get_field`, `set_field` or `Vm::with_cx` is
dropped, so test such a builtin through `ScriptRuntime`.

## Traps already paid for

### Wire and netcode

- Protocol 6 (1.5) renumbered the top of `pm_flags`; the decoder rewrites
  it into 1.1's layout (`Cod1Wire::pm_flags_to_v1`), so a `PlayerState`
  never holds the raw 1.5 value. The writer maps it back
  (`Cod1Wire::pm_flags_from_v1`); 1.5-only state (`flags::PMF_MAY_CYCLE`,
  `PMF_MAY_FREE`, `PMF_WEAPON_DISABLED`) sits above 1.1's 19 bits, which a
  protocol 1 wire masks off. 1.5's jump penalty keeps its 0x2000, which 1.1
  never sets, and `predict::from_wire` turns it on under 1.5's rules
  only (`MoveRules::v15()`, which UO's `MoveRules::Uo` keeps). A 1.5 body's `eFlags` stance follows the eye's stepped target, so
  the predictor reads 1.5's stance off the flags. 1.5 and UO send `B` where 1.1 sends `n` for a
  `map_restart` that drops `pers[]` (`n` for one that keeps it), and its gametypes open `serverinfo_<gametype>` before the
  team menu, so a `t` index is not `g_scriptMainMenu`'s.
- The CD key goes to the authorize server on its own socket and thread
  (`net::cdkey::request_authorization`), never through the connection's
  transport, and never into a log: `CdKey`'s `Debug` hides it. 1.1, 1.5
  and UO send the bare key, CoD2 and CoD4 add `PB <md5>`, an MD5 from
  Activision's own initial state over the upper-cased (CoD2) or
  lower-cased (CoD4) key, the same hash their `getchallenge` carries
  (cod11-server-handshake.md, "CD key authorization (the clients)").
- A UO script model or turret carries its model in `xmodel`, not `index`
  (`G_DObjUpdate`); a brush model keeps its number in `index`.
- Protocol 22 (UO) sends an `eType` 12 entity (a vehicle) through a second
  netfield table behind a select bit after its number; a decoded
  `EntityState` keeps the main table's order whichever table carried it
  (`Cod1Wire::vehicle_field_slots`). Its `bits -100` fields are 16-bit
  angles held as f32 degrees; one equal to its base after quantizing is
  nudged a mantissa bit so a capture re-encodes. UO's `eType` 9..15 are its
  own (`flags::ET_VEHICLE_V22` and kin) and `ET_EVENTS` is 16: ask
  `Cod1Wire::event_of`, never `eType >= ET_EVENTS`. Its events, `eFlags`,
  `pm_flags` and means of death are mapped onto 1.1's at decode (the
  predictor's ring too: `predict::from_wire`, or every own shot plays
  twice, `coduo-movement.md` 15), the
  configstring blocks and the fixed slots past 11 (UO's fog is 13, its
  start time 14) are `Cod1Wire::cs`, never the `CS_*` constants, and the
  ammo and health items follow `Cod1Wire::max_weapons` weapon rows
  (`weapon_table::static_item`: 65..69, UO 129..133). UO's
  own events, its `EV_BULLET_HIT` and its projectile explodes keep their
  UO number under `EV_FOREIGN` (`EV_UO_*`), and its sprinting, winded and
  overheated `pm_flags` sit above bit 23 (`PMF_*_V22`), so a 1.1 handler
  never fires on one (`coduo-diff.md` 19).
- UO's sprint button is `buttons` 0x8 (`msg::BUTTON_SPRINT_V22`), sent to a
  UO server only. An LMG's sight flag is its bipod (`PlayerState::deployed`):
  nothing else sets 0x20 on one, and the deploy writes the stance itself
  (`coduo-movement.md` 7). A crouched `trigger_mount` reaches the client
  as hint 7 with value 0 (the server's own contents found it), so the
  predictor deploys it standing for a snapshot, as retail's would (11).
  The server's pmove point contents meet the mount triggers
  (`MoveWorld::with_boxes`); the client's never do.
- vcod-server builds every entity and playerstate in 1.1's numbering
  (events, `eFlags`, an event entity's `eType`) and maps it to the
  protocol's once a snapshot (`game::wire::entity_to_protocol`,
  `ps_to_protocol`; baselines too); `pm_flags` maps in the writer. A new
  wire path that skips them sends 1.1 numbers to a UO client.
  CoD2's server builds its entities in CoD2's numbers as it builds them
  (`HostProfile::entities`, `cod2_wire::COD2_ENTITY_NUMBERS`: types,
  `eFlags`, `trType`s, ring events), held in vcod's `EntityState` over
  118's table (`Cod2Wire::as_cod1_shim`, identity maps) until the engine
  takes `EntityState2`; `cod2_wire::to_cod2_entity` only copies. A CoD2
  builder that writes a 1.1 number sends it as is (cod2-protocol.md 15.9).
- Every playerstate field `predict::from_wire` reads, `ClientSim::to_wire`
  has to write: the predictor rebuilds from the snapshot each time, and a
  field left 0 replays differently from the server. A missing
  `iFatigueSoundTime` breathed on every snapshot and played rounds twice
  (coduo-movement.md 15); `predict_ab`'s 1.5 and UO side-by-side gates
  catch the next one.
- The stock pak set is the protocol's (`PakRules::id_pak_numbers`, the protocol's `transport.paks`: 1.1
  `pak0`..`pak8`, 1.5 `pak0`..`pakf`, UO's `pakuo` ones in `uo/` too), not the install's, and a retail
  server streams a stock pak anyone asks for. A 1.5 match timeout freezes
  snapshot serverTime, so the client clock counts snapshots by message
  number, not by time.
- 1.5 and UO list a stock pak in `sv_referencedPakNames` only when the
  level read a file from it (`Pk3Fs`'s read flags, cleared at each map
  load), and keep a systeminfo past 1023 bytes; 1.1 lists every pak. A
  load that stops reading a file can drop a pak from the list
  (`Server::level_paks`, handshake doc "The four cvars").
- `entityState.weapon` is 1-based into configstring 7. The ammo and clip
  index tables also start at 1.
- `health`, `ammo[]`, `ammoclip[]` sit in the playerstate array blocks, and a
  retail client predicts its ammo counter from the clip it is sent.
- `eventSequence` counts after the write: the new events are `(prev..cur)`.
  An entity entering the snapshot replays its ring from 0 when it is a
  general entity, a missile or a fresh corpse (`CG_ResetEntity`): a shell
  that strikes in its first step is only ever sent exploded.
- The cmd's weapon byte is the client's own selection (`cg_weaponSelect`,
  `play::input::PlayInput::selected`), never an echo of `ps.weapon`;
  `switchToWeapon` and a weapon pickup only send the client `a <weapon>`
  (combat doc 9.8). pmove reads a byte of 0 as retail does, a switch to
  empty hands (1.8), so every caller that steps a player (bots, nav walks
  on a bot's playerstate, test cmds: `common::holding`, `echo_weapon`)
  sends the weapon it holds.
- The first usercmd of a message decodes against a base built from the
  playerstate (sight bit = `fWeaponPosFrac != 0`, stance from `eFlags`), not
  the last cmd sent. vcod's writer sends the full branch every cmd for that
  reason.
- `eFlags` 0x8 flips on every spawn; leave it pinned and a client smears a
  respawn from the corpse. A respawn clears the event ring.
- The damage feedback fields never clear; `damageEvent`'s increment is the
  edge.
- A configstring range's first slot comes from its indexer: status icon, head
  icon and script menu scan from 0, localized strings and shaders from 1.
- A player's wire `solid` is packed at link time (after each cmd's move, at
  spawn), never at end frame.
- A client's entity goes out as its own end-frame turn copied it
  (`ClientSim::end_frame_entity`): a higher slot's turret round after that
  turn shows its knockback a snapshot later, and a victim it kills is still
  sent that frame (`cod11-turrets.md` 13.3).
- The netchan scramble keeps `%`. The server reads client commands with the
  server-side string reader; the client reader's `%` mapping garbles every
  later message after a chat line containing one.
- A move message's ack is stamped with the newest frame already sent when the
  packet arrived, not the tick time, or loopback ping reads 21-50 ms.
- On a 1.5 server `sv_time_ms` is the game clock (`level.time`, the
  snapshot's `serverTime`) and `svs_time_ms()` the network's; a match
  timeout (`timescale 0`) holds the first only, so whatever paces, pings
  or times a client out reads `svs_time_ms()` (`cod15-protocol-diff.md`
  12).
- A cmd's clock is clamped to [-1000, +200] ms around the last frame's
  `level.time`, and the entering cmd seeds `commandTime` inside that window.
- Server commands queue until the next snapshot and squash like
  `SV_AddServerCommand` (rule in `docs/protocol-1.1.md`).
- The frame loop sends a fragmented snapshot, download or gamestate
  message one fragment per tick, as retail does, and an off-LAN client
  waits out its rate and its `snaps` interval between messages. A test that
  reads one tick's packets sees only the first fragment of a big message.
  A client not in the world gets a 9-byte bare message on each turn (every
  tick on a LAN, 1 s apart off it), so such a test drains those too. That
  message and a zombie's go out on their own slot's turn in the one
  slot-order pass, so a lower slot's precedes a higher slot's snapshot.
- A pure server drops a client that enters the world without a valid `cp`
  (pak checksums keyed with the gamestate's `checksumFeed`), which on UO
  opens with the paks of `uo_cgame_mp_x86.dll` and `uo_ui_mp_x86.dll`
  (`Cod1Wire::pure_dlls`), not 1.1's modules its `main/` still holds. Extra
  arguments to `tools/run_server.sh` land after `+map`, so `+set sv_pure 1`
  there leaves `sv_paks` empty yet still drops; pass it before the map
  (`PRE_MAP_ARGS='+set sv_pure 1'`) to test a pure server.
- Cvars: every `L` cvar latches until the next load, so a latched
  `g_gametype` must not reach serverinfo early. `devmap` sets `sv_cheats 1`
  after the load and `map` sets it back. The server ignores an OOB
  `disconnect`; a client honours one only after 3 s without a packet.

### Map cycle

- A map change is pull-shaped: the server sends only the OOB
  `loadingnewmap`, and resends the gamestate when a client's next message
  carries the old serverId. Nothing goes on the reliable stream.
- `map <current map>` is a restart, not a spawn.
- A restart sends every configstring the incoming level wrote ahead of
  its `n`/`B`, one `d` per slot after the queue's squash: the fog
  `G_InitGame` clears and the map sets back still goes out, and
  `ExitLevel` zeroes the team scores in the game only, so the next
  level's `setTeamScore(..., 0)` goes out as `d 6 0`. The burst and the
  frame's diff go in write order, off `ConfigStrings::set`'s log: a
  game write that bypasses `set` is not possible (no `DerefMut`), so
  keep it that way (map-cycle doc 4.5).
- `map_restart` keeps the configstring table and the item registry; only a
  map load clears them. vcod's restart also keeps the compiled scripts
  (`Carry::scripts`); a test that swaps a script mid-level goes through
  `Server::overlay_script`, which drops them.
- `game[]` and `pers[]` survive only `map_restart(1)` / `exitLevel(1)`, and
  entity handles inside `game[]` never survive. `dm` passes 0, `sd` passes 1.
- Time and score limits are script. The engine has no `CheckExitRules`;
  `g_intermissionDelay` and `nextmap` are read by nothing.

### Movement and collision

- Movement constants come from retail rodata. Retail 1.1 MP has no mantling,
  no swimming and no water jump.
- One cmd step for every caller: `crates/common/src/pmove/cmd.rs` (`chop`,
  `player_step`). The server, the client predictor and bots wrap it.
- The player is a capsule. A box sits `15 tan` higher on every grade.
- A spectator collides: a (-8,-8,-8)..(8,8,16) capsule under mask 0x800011
  (no playerclip, no bodies), stepping in the air because its
  `groundEntityNum` reads 0 (protocol doc, "Spectator").
- The ground trace runs origin ±0.25; a test that places a player exactly on
  a face lands allsolid and stuck. Place it 0.125 up.
- A brush hit reports the struck side's material (lump 3), not the
  brush's: a caulk brush's faces carry the plaster or rock a surface reads.
- No render soup collides: model 0 clips through its brushes and lump 24
  (terrain partitions, patch facet grids). A patch's box is its subdivided
  grid grown by a unit, not its control points.
- Static props and 0x2080 kerb brushes clip bullets, blasts, missiles
  (clipmask 0x2802091 on all three) and items, never a moving
  player. `shot_trace`, `missile_trace` and `item_trace` see props,
  `box_trace` does not.
- A submodel's brushes, and the patches and terrain lump 27 gives it
  (bytes 32..40, its run in lump 23), clip only while its entity is
  linked. Script `delete`, `notSolid` and `_gameobjects` unlink them; a
  world no script runs on applies the rule through
  `CollisionWorld::unlink_script_brushes`.
- A brush entity with no `*N` (any script `spawn` of one) links inline model
  0 and clips as a copy of the whole world where it stands, pushes like any
  brush model, goes out with `index` 0 and is clipped by client prediction
  (`CollisionWorld::set_world_copy`, movers doc 19-20). Brush bounds are the
  lump's grown a unit (`bsp::Model::clip_bounds`), before the link's own
  unit. A turned copy or submodel is traced in its own frame: the box's
  centre turned, not the origin, so a pitch shifts a player's capsule, and
  the box axial there, not in the world. A capsule meets the axial sides
  as its upright world box, `|turned sphere offset| + r` a side, and a
  non-axial side with the offset's y negated, so on a roll it is not
  upright. The turn takes `AngleVectors`' stored rounding
  (`x87::rotation_matrix`; `x87::angles_to_axis` for `AnglesToAxis`),
  the same in every engine and game module of the three servers, never
  float products. pmove's turns take the move world's `Arith`
  (`x87::angle_vectors_in`): the Windows cgames multiply by a float
  `pi / 180` and store `sr * sp` (movers doc 22, "pmove's turns"). A trace an entity's clip cuts short of the world's ends where
  `SV_Trace` puts it, off the unshifted points (`sv_trace_endpos`), not
  where `CM_BoxTrace` would (movers doc 21, 22).
- A fall onto a slope slides as far as its cmd split lets it: a cmd that
  starts within the ground trace's reach turns the fall into ground speed.
  An A/B over one replays the client's three cmds a frame, never one
  (movers doc 21, "The slide down the slope").
- 1.5b's and United Offensive's engines are an unoptimized gcc 3.2.3
  build that stores every float variable, where 1.1d's keeps them on the
  stack. A parked vehicle's wheels read the last bit of the terrain point
  clip (`coduo-vehicles.md` 12, the second shove), patches are built
  with cracks along other facet seams, which a round slips through, and
  the brush clip stores its distances (`bsp-ibsp59-format.md`, "Patch
  collision on 1.5 and UO"), and so does the patch capsule arm and the terrain capsule clip, which
  reads the UO-built planes and edge frames; pmove's fall stores its gravity,
  its tail and the trace's end point, and `PM_ClipVelocity`,
  `VectorNormalize` and `PM_Friction` store theirs (`cod15-movement.md` 7), and
so do a missile's arc and bounce (`Missile::kept`, combat doc 12.5) and the
  locational bone trace (`BoneTraceCtx::arith`, 3.1; its hit point on a
  body keeps the stack, `combat::trace_point`). A 1.5 or
  UO server sets `CollisionWorld::set_arith(Arith::for_protocol(..))`;
  a replay world in a test sets it too. A 1.5 or UO body's bones for a
  trace or a tag link come from the engine's own unnormalized anim
  records and skeleton build (`PoseBuffer::stored_worlds`, combat doc
  3.1, "The pose on 1.5 and UO"), not the glam pose. The client's world runs
  `Arith::for_client()` on every protocol: the Windows clients are one
  MSVC build that stores some of those and keeps the rest (8), so a
  client replay of a Linux capture corrects in the last bits.
- A sweep, point or capsule, that meets a model-0 patch is clipped again
  in retail's BSP leaf order (`leafwalk.rs`): a patch clips only from a
  leaf on the path that lists it (mp_carentan's 69, 109, 283 and 284 are
  in no list and never clip; mp_railyard's 120 is missing from leaves its
  facets cross), and a point tests its unpushed fraction against the
  trace's, so a brush or patch from an earlier leaf can keep it out
  (`bsp-ibsp59-format.md`, "The leaf walk"). So is a sweep whose contact
  nearly ties another (brushes a float step apart, terrain within its
  1e-5 back-off of a brush or a partition, a submodel against the world)
  and a box stopped at fraction 0, a capsule stopped there on two
  contacts or starting all in solid, and a sweep stopped on terrain or a
  brush no leaf on its walk lists (out of the world, or a brush's
  pushed-out planes overreaching an unbevelled edge, it never clips):
  leaf order names the surface a body standing on two floors steps on.
- A trace whose start is its end is a position test: terrain reads
  `startsolid` only through `Terrain::position_test`. A push's candidate
  and spot tests are retail's box against brushes, not the capsule
  (`pusher_box_trace`, `box_position_test`), and the candidate test is a
  whole trace that must name the pusher: a body in the world, or met
  first by another entity on the area walk, is passed through
  (`push::names_pusher`, movers doc 12). A sine mover never stalls: it
  crushes a player it cannot place (movers doc 21). A `Pmove` clips only
  clients linked in the area tree: a blocked push leaves its player out
  until his own next cmd, and a two-client gate on that orders the
  clients' packets by the capture's `w=` send stamps (`push_candidate_ab`).
- One spectator anywhere turns `StuckInClient` off on 1.1 only; 1.5's and
  UO's scans pass over him, so two players set down on one spot part at
  once there, in a `rand()` direction a gate replays from the glibc seed
  recovered off the capture (`Server::pin_stuck_rand`, player-clip doc
  13.4).
- `setOrigin` and a teleport link at the unsnapped origin until the next cmd
  relinks.
- A brush model is traced at its live `r.currentAngles`, not its link's: a
  finished rotate leaves them truncated to 65536ths (60 reads 59.99634),
  so the clip turns by that, on all three servers (movers doc 22).
- A cmd's `Pmove` is one `movetrace::Pm`, handed to every chopped step's
  world (`MoveWorld::in_pmove`): 1.5's and UO's `PM_trace` drops bodies
  from its mask for the rest of the cmd once a trace starts in one, and
  its touch list is `ClientImpacts`' `"touch"` on both players, all three
  patches. Make one per cmd, never per step (cod11-player-clip.md 13).
- A UO vehicle seat's pmove moves nothing: the body is put at the seat
  tag in `ClientEndFrame`, so the predictor keeps the snapshot's origin,
  and the view's 12000 pitch clamp (16000 on foot) and the jeep
  passenger's arcs push `delta_angles` (`coduo-vehicles.md` 3). The
  server places the rider off the vehicle as posed before the frame's
  step, and the step reads the rider's last run cmd, not the last
  received (`coduo-vehicles.md` 11.4, 12). The cmds left in an exit's
  frame run linked and move nothing (`pm_type` 1 until `G_RunClient`,
  11.2) but stand the rider's legs up: 1.5's and UO's `PM_Footsteps`
  select a linked player's idle, airborne or not (14), and an undriven
  vehicle thinks on its own turn in the entity pass, by number, the
  driven ones after it, so a pushed vehicle moves the frame after the hit
  and what is linked to it moves with it (12, "The think's turn").
- A UO vehicle's `eType` 12 and its sound blends' 9 are not 1.1's types:
  the server builds them with `game::wire::ET_RAW` so the protocol map
  leaves the type alone and still maps the events. Vehicle clip brushes
  (contents 0x200) are in the collision world; only a vehicle's clipmask
  meets them.
- A vehicle's collmap clips as that vehicle's own model instance
  (`CollisionWorld::set_model_instance`; one inline model serves every
  vehicle of a kind), passed by its own traces as `Pusher::Copy(entity)`.
  Its brushes clip in lump order and a contact at fraction 0 ends them,
  so a later brush never reads a sweep all solid (`coduo-vehicles.md`
  11.7).
  Its hull against rounds, missiles and blast probes is the xmodel's
  collision posed per bone (`vehicle::placed_models`), not the collmap.
  The server poses it with `Scr_Vehicle_Controller`'s controls only
  (`vehiclepose::apply_hull`): the steering wheel and front wheels stay
  at their bind there, unlike the drawn pose. A body's broad box is the
  fixed (-64,-64,-32)..(64,64,72), and a bone past what the trace already
  met never scores (`coduo-vehicles.md` 13, "The silhouette").
- Wish speed multiplies the weapon's `moveSpeedScale` and the sight, back,
  strafe and lean scales: a plain carbine run is 224, sighted 89.7.
- Every mover's speed is `ps.speed` (`PlayerState::speed`): `g_speed` on
  1.1, the client's `maxspeed` on 1.5 and UO, 400 for a spectator. A wire
  playerstate built by hand without `speed` predicts a body that never
  moves (cod15-movement.md, "ps.speed"). Gravity is `ps.gravity`
  likewise, `(int)g_gravity` on all three: one built without `gravity`
  never falls. A map load writes `g_gravity` back to 800, so a `+set` of
  it on retail's command line never sticks ("ps.gravity").

### Combat and items

- A 1.1 shot, swing or throw starts at the truncated eye over the
  truncated origin; 1.5's and UO's start at the unsnapped origin's eye
  plus the walk bob, never truncated (`combat::muzzle_point_in`, combat
  doc 2.1).
- There is no grenade cook in 1.1 MP; "cooking" in the code means held.
  `grenadeTimeLeft` is `fuseTime` at the pullback and 0 at the throw, and the
  fuse always runs in full from release. UO has one, started only by the
  sight button's edge on a pulled grenade (`weapon::cook`,
  `coduo-movement.md` 12).
- A weapon file's millisecond keys are stored truncated
  (`weapon::stored_ms`: 1.8 is 1799), on all three Linux servers
  (`coduo-movement.md` 13); never round one. The fuse rides the fire event's parm
  inside vcod only; the wire ring carries 0, as retail sends.
- The explode rides the missile's own entity (`eType` flips to 0), so an
  `eType == 4` filter drops the explode frame.
- A missile's ground snap is a locational trace like its move: a frag
  rests on a player's posed bones with his number as its ground. 1.5 and
  UO drop a resting missile whose ground is not the world once nothing is
  under it (`Missile::falls`, combat doc 12.6); 1.1 leaves it hanging.
- A player's head model and his helmet each carry a `bip01 head` box,
  and the trace scores both: the grafted skeleton keeps an attachment's
  second box on a merged bone as a box-only child (combat doc 3.6). A
  standing body's torso yaw stays where his spawn left it until he moves,
  fires or goes prone (16.4), so a hit-pose capture swings it first.
- The engine's hit-box walk clips each box to the fraction already found:
  a higher priority beats a nearer box only when its bone comes first
  (`bonetrace::bone_trace`'s `limit`, combat doc 3.1). The aim block's
  sight sway reads the last frame's `level.time`, the cmd's (15.1).
- A stock frag bounces off a live player: `fraggrenade_mp` has `damage 0`.
  The move trace scores his posed bones under `bulletPriorityMap` and
  reflects off the struck bone's face, not his link box (combat doc 12.4).
  1.1's ground snap then lifts it 1.5 onto his bones; 1.5's and UO's snap
  onto the world only (12.2), and UO's bounce off a body keeps a tenth
  (coduo-diff.md 21). Off a surface 1.1 keeps 0.75 along it and 0.3 off
  it; 1.5 scales the whole reflection by 0.5 and UO by 0.4 (12.4).
- On 1.5 and UO a prone body keeps the bend its last airborne cmd sampled
  (cod15-movement.md 4), so a gate that drops a prone body sends the net
  probe's 16/16/18 ms cmds (`ProbeHold::framed`), or its wire body and
  every bone land a fraction of a unit off retail's. Keep it opt-in:
  forced on, the UO vehicle hit gates and 1.1's prone-rest gate fail.
- 1.5's and UO's `ClientSpawn` thinks on the client's last cmd, not 1.1's
  null one (`ClientSim::spawn_cmd`): a client holding prone lies down
  along the spawn point's yaw before the script turns him, and a
  spectator flies off the spot on his held keys (cod15-movement.md 4,
  "The spawn's lie-down"). The think runs twice in vcod: on a fresh sim
  in the `spawn` builtin, for the origin the spawning thread reads, and
  on the real sim in the frame's spawn pass, both through
  `ClientSim::spawn_think_on`. A playing client's `angles` then read (0,
  the think's view yaw, 0): 0 on 1.1, the spot's yaw through `SHORT2ANGLE`
  (180 is -180) on 1.5 and UO (cod11-gametypes-re-bel.md 8.6).
- 1.1's legs idle off `viewHeightLerpTarget` and move off the effective
  stance, never `pm_flags` alone (player-model-anim-system.md, "How
  retail picks the movetype"): a test that sets `ps.stance` by hand idles
  standing; `PlayerState::settle_stance` sets the whole stance.
- A blast, the touch pass, the use key and `positionWouldTelefrag` walk
  `crate::area::AreaTree`, retail's entity area tree, so victim and touch
  order is link history, not entity order, and an entity no link put in
  the tree is invisible to all four. A client links through
  `GameHost::link_client`, everything else through
  `GameHost::link_entity`; an `origin` write on a non-client relinks it, a
  player `setOrigin`, spawn, death or turret release (`TeleportPlayer`) is
  an unlink then a link (combat doc 14.7).
- Blast victims are measured per turn, after earlier callbacks ran. A client
  killed this frame keeps `takedamage` until its own end frame.
- `ClientEndFrame`'s dead arm (`pm_type` 6, no contents, off the wire)
  keys on `sessionstate` `dead`, not on health: a `suicide()` that stays
  `playing` keeps `pm_type` 0, and a test that kills through the vitals
  sets the session too (`ClientSim::session`, combat doc 5.1).
- `player_die` notifies the victim `death` with the attacker before the
  killed callback; UO's rank icon is destroyed on it (coduo-diff.md 22).
- A killed gunner keeps the turret until its own end-frame turn;
  `player_die` releases nothing.
- `setPlayerIgnoreRadiusDamage` is a level flag read only by the
  `radiusDamage` builtin.
- The body queue is eight entities at 64..71 with no timer; a corpse lives
  until its slot is reused.
- A temp entity takes its number from `G_Spawn`'s first-in-first-out free
  list like a script `spawn`, rides every snapshot, and is freed on the
  first entity pass more than 300 ms past its event (combat doc 14.7,
  "Entity numbers"). Engine notifies raised in the entity pass (`movedone`)
  wake their waiters on the next frame's threads.
- A body's head is a random draw from `xmodelalias/head_*`, and the `a`
  and `b` heads' boxes differ: a gate that reads head cells pins it
  (`probe_turret`'s `probe_head`, `cod11-turrets.md` 13.6).
- A `clipOnly` weapon (the frag) has no reserve entry on the wire.
- An item's `count` of 0 means unset; a drop writes -1 for empty.
- A `trigger_lookat` is never touched; it fires off `ClientEndFrame`'s aim
  trace. A `trigger_use` is never touched either (contents 0x200000); it
  fires off the use key's aim pick. A trigger-woken thread runs on the next
  frame's clock. A trigger's own `"trigger"` (touch, hurt, lookat,
  damage) goes through `G_RunFrame`'s queue, one per entity a round
  (`host.trigger_fires`, pushed through `GameHost::queue_trigger`, which
  refuses one past 0x100 for the caller to notify on the spot); what retail notifies on the spot (a touch, the
  use key, an item take, a mover's end, a blast's or a round's
  `"damage"` on an entity) goes in `host.engine_notifies`
  (`cod11-gsc-language.md`, "Engine notifies and the trigger queue").

### Script

- A game version's rule in the host, spawn, item, vote, field and print
  code reads a named `HostProfile` field (`game::profile`), never
  `profile.game`; the VM's go through `VmProfile` (`Vm::apply_profile`).
  The load picks the profile and `GameHost::new` takes it. A test that
  switches version calls `GameHost::set_profile`: a write to `defs` alone
  leaves the builtins and rules on the old version. CoD2's own state
  (`host.cod2()`) panics on a CoD1 host; `GameHost::new` and
  `set_profile` pick the `HostExt` arm off the game.
- gsc folds case for identifiers, fields, paths and event names but not for
  string values or array keys. A misrouted event name hangs its `waittill`
  silently.
- Unary `!` and a condition read a string as the number it spells (`!"0"`
  is 1, `if ("1")` passes); `""` or `"a"` there is fatal.
- A float prints `%g`-style (`1e-05`, `1.23457e+06`), not Rust's format.
- An array is a value on every engine: `b = a; b[k] = v` leaves `a`
  alone. The VM flags an array stored a second time and copies it at the
  next write (`Heap::share`, `Heap::own`); a write path that skips the
  `EnsureArray*`/`OwnLocal` ops writes into every holder. CoD4's
  `CountPlayers` shared `level.players` that way and flooded
  `spawnPerFrameUpdate` (cod4-gsc.md 10).
- A BSP key reaches script only if it is in the entity field table or
  `radiant/keys.txt`; anything else is dropped silently.
- A brush model's `.model` reads `""` (its `*N` key goes to `s.index`).
- `setTimerUp` has no `> 0` check, unlike the other three timers; `bel`
  starts with `setTimerUp(0)`.
- Threads due in one frame resume newest-queued first; `wait 0` resumes at
  once; a notify's waiters run after the notifier's step, in start order,
  and it reaches only the threads registered when it runs: a thread the
  notifier starts after it keeps its `endon` and its `waittill`.
- The script clock is not `level.time`: a thread due at a frame's
  `level.time` runs the frame after, so a `wait 1` from
  `Callback_StartGameType` at 0 wakes at 1050. Between frames the clock is
  the last frame's `level.time`; `Vm::set_time` starts it at a load. It is
  24-bit (`CLOCK_MASK`, wraps about 4.7 h into server uptime), and `wait`
  refuses a negative wait and one of 16777 s or more.
- A callback a cmd raises (damage, fall, `kill`) runs before the frame and
  reads the last frame's `getTime()`; nothing on that path may advance
  `host.level_time_ms`.
- A thread's own `notify` doesn't fire its own `endon`.
- A runtime error ends its thread only at `developer 1`, `run_probe.sh`'s.
  vcod-server's default `developer 0` runs on past it as retail does (the
  failed instruction's results undefined, a conditional jump not taken)
  and still lists it in `script_aborts` (cod11-gsc-language.md, "Runtime
  errors below developer 1"), so a gate replaying a probe capture that
  dies sets `developer 1`.
- `loadFx` of a new effect after the bootstrap's first `wait` is a script
  error (`GameHost::initializing`), and so is any `precacheModel`,
  `precacheShader`, `precacheString`, `precacheItem` or
  `precacheShellShock`, even of a name already precached; a test that
  precaches late fails there. So is `RegisterItem` of an item not yet
  registered (`giveWeapon`, `setWeaponSlotWeapon`, a script `spawn` of an
  item, a drop): a test that hands a player weapons by hand registers
  them first (`GameHost::precache_items_for_test`). Menus and icons may precache any time.
  Past that wait `setModel`, `attach`, `setViewModel`, `setShader`,
  `setText`, `label` and the objective icons only find a name already
  indexed (`Allocators::index_precached`), and `statusicon`, `headicon`
  and `shellshock` never index one, so a test that sets one fills its
  configstring first.
- `attach` of a pair the entity already carries, or of a seventh model,
  is a script error, and its tag indexes nothing: a player's tags take
  their configstrings at his own end frame (`client_attachments`;
  object-model doc 24). `addTestClient` runs the connect callback inside
  the call and the `begin` waiters after the caller's step (25).
- The weapon builtins queue `WeaponOp`s, and any pass may drain them
  (a vehicle round's callbacks drain every slot's). So the playerstate's
  ammo is the queue's alone: `self spawn()` queues its emptying as
  `WeaponOp::TakeAll` and the spawn pass keeps `ps.ammo`. A reset of
  ammo outside the queue lands out of order with the loadout, and a
  spawn came up with every weapon at 0 rounds (object-model doc 20).
- On 1.5 and UO every script thread runs before the frame's anim update,
  so a script `bulletTrace` poses bodies at the last frame's time
  (`GameHost::dobj_time_ms`); a UO spawn's `setModel` rebuilds the DObj
  in the thread, so its clips run a frame ahead (`ClientSim::dobj_ahead`,
  `cod11-turrets.md` 13.7). The two cancel until clips change.
- A thread due on the last frame's clock runs before the frame's anim
  update, and a player bone it asks for keeps that pose, with every bone
  above it, for the rest of the frame (`GameHost::dobj_posed`, movers doc
  15, "A bone asked for before the anim update"). Its `bulletTrace` and
  `radiusDamage` pose bodies at that clock too and fix every bone they
  pose for the frame, as a bone ask does (`GameHost::dobj_time_ms`,
  combat doc 3.6). A gate replaying a `--probe-uo-weapon direct` capture
  sends `ProbeHold::framed` cmds: on even ones `leanhit_ab`'s standing
  cells read 0.1 off. Where a `setOrigin` drop lands rides the exact cmd
  times, which `--probe-uo-weapon` and `--probe-vehicle` log as `CMDS`
  lines and `ProbeHold::times` replays.
- Every trace that poses a player, script or engine, builds his DObj
  through `GameHost::dobj_posed` (`combat::DobjStamps`): each client
  packet (on retail the bots' pass as one) and `G_RunFrame` start a
  stamp; on 1.5 and UO each build eases the controls a step from the
  last build's, so a body nothing traces keeps its controls. `setPlayerAngles` tilts the
  traced body by its pitch and roll until his next cmd (cod11-combat.md
  3.7). A new trace that skips the stamps poses a body retail never
  built.

### Assets, rendering, sound

- A coarse xmodel LOD ships no `xmodelparts`: its vertex bone ids index
  LOD 0's bones. A model past its last LOD distance is not drawn at all
  (xmodel doc, "LOD selection").
- xanim translation keys are offsets from the bind pose. Root `tag_origin`
  sits at the feet.
- A non-player entity rotates through `AnglesToAxis`: positive pitch is nose
  down.
- A control bone (`back_*`, `neck`, `head`, `pelvis`) turns about the model's
  axes, not its own; about its own Y the body leans sideways. The torso pitch,
  torso yaw and legs yaw swing after the view on both server and client, and
  `tag_origin`'s local tag turns the body by the legs' yaw off the view, so a
  test that poses a turned body must step it to settle (combat doc 16.3, 16.4).
- A player's clip plays at the rate its ground speed gives, starts at a
  phase off the clip it replaces or the level time, and blends 120-250 ms
  (30 into a fire anim) (`vcod_common::animclock`, movers doc 15). Its clock
  runs on level time and the slot number, so a pose A/B replays cmds on
  retail's level times. The client runs the same clock on render time and
  measures the rate on the newest snapshot's time and `trBase`. Both pose
  through `playerpose::pose_player`: the torso and legs clips blend per bone
  as two tree nodes, weighted 1 and 0.01 under a torso anim. A 1.5 or UO
  server samples a clip at the engine's node time, a float stepped by
  50 ms frames (`Layer::time`, `PoseInputs::node_time`; a UO seat's leaves
  each step their own, `SeatRamp::leaf_times`, and stand still with
  their weights while their legs node is faded out, `SeatRamp::fade_held`; an MG42 gunner's
  fire group its leaves' weighted mean, `set_sync_frequency`), not its seconds
  (combat doc 3.1, "The clip's float time"). A gunner's turret group is a
  node over row nodes (`skeleton::Blend`), each row normalized to its
  weight. On 1.5 and UO its rows and leaves carry their weights from
  frame to frame (`turretpose::GunnerTree`, turrets doc 13.9, "The
  weights across frames"): a trace before the next end frame reads last
  frame's goals, a row or leaf just added at 0.001 times its goal, one
  just dropped at its old weight. A node any setter starts from 0 ramps
  from `0.001 * goal` (`Ramp::raised`), so a clip just set never reads 0.
- The 24/30/32 pt fonts span two or three atlas pages per glyph, named in the
  `.dat`. HUD colours are display values, scaled into the framebuffer by
  the material's gen (`colour_scale`) in the HUD pass.
- The scene target holds retail's framebuffer bytes: textures upload
  without sRGB decode and blends run in byte space. The overbright x2 lives
  only in the gamma pass's ramp, so a pass that draws a display value
  scales it by `identityLight` (0.5 full screen, 1 windowed) first.
  `vc_hdr 1` swaps the world's target for a float one that keeps the same
  units and is tonemapped back to bytes before the HUD (`crate::post`).
- Effect shaders live in `fxshaders/` in `pak5.pk3`. Some map paths have a
  leading slash. UO's impact tables spell their explode paths with
  backslashes; `Pk3Fs` reads `\` as `/`, as the engine's lookup does
  (coduo-diff.md 19, "Backslashed rows").
- A UO rider poses through the server's `Controllers::rider` and
  `playerpose::seat_inputs` on both sides, but the cgame scales the
  jeep's steer by 1/60 where the server takes 1/45: a drawn seat is
  `BodySeat::drawn` (`coduo-vehicles.md` 9, "Other riders").
- A UO sound blend entity (`eType` 9) is never drawn, so `step`'s entity
  positions never move its voice: `AudioSystem::set_sound_blends` places
  it on its vehicle every frame (coduo-diff.md 16).
- The asphalt sound alias suffix is `asphault`, and vcod uses that spelling on
  purpose.
- The probes' flat floor on mp_carentan (`probe_legs`, `probe_bump`,
  `probe_passthru`, z -151.875 at y -376) is the map's bottom brush, 145
  under the garden terrain. A view from there shows walls floating over
  sky: the town from below, not a culling bug.
- Portal walk: a portal's plane faces out of its owning cell. vcod adds three
  over-marking rules in `common/src/vis.rs` for the mp_ship decks.
- No cell lists a brush model's soups: `WorldVis` marks them by the model's
  bounds against the frustum, and the renderer keeps only those whose model
  a snapshot entity carries (movers doc, "The client").

### Bots

- The per-tick driver (`bots::driver::step`) reads and writes the game
  only through `BotWorld`, one call per point of the tick, in CoD1's old
  `step_bots` order: a call moved across another moves rand draws, the
  enemy cache and the plan budget. Its state (`BotDriver`) stays in the
  game for the tick: the view builders, the vehicle code and the admin
  read `bots`, `nav` and `paths` mid-tick. Both games run it in one
  order; what a game's route leaves out the trait carries as a const
  (`YIELDS`, `PMOVE_PROBES`), and flipping one moves that game's draws. A per-game
  step goes in a hook (`feel`, `pre_route`, `routed`, `pre_think`,
  `keeps_alive`, `cmd`, `tick_done`), not in `step`.
- Nav build: `Prim::Tri` is terrain only, and `under_ground`'s ray counts
  terrain only, or a patch spar overhead makes a ladder foot read as
  underground. Minefields and `trigger_hurt` brushes are excluded.
- A* is resumable under a shared per-tick expansion budget, so a plan can
  take several ticks, and the body may have left the start's floor when it
  lands (`Follower::proved_start`).
- A body on the rungs off the ground sends three to six cmds a tick, bots
  and the graph's walks alike (`nav::tick_times`): the rungs let go 8 under
  a ladder's top, and whether the step-up clears the lip depends on the
  frame length. A walk that steps a ladder some other way proves edges no
  bot can follow.
- On the rungs a strafe held under a full-rate climb never turns the
  sideways pace the grab carried in, and only a released key bleeds it;
  bots let the key go while the body slides away from the rungs' middle.
- A bot backs down a drop the way `Gait::Creep` walks (`Bot::steer`), not
  `Gait::Backward`; re-prove a backed drop with the creep.
- A bot on a jump edge jumps by the walk's own rule (`nav::PinJump`:
  two pinned ticks, a bob, or right under the top; no `jump_clear` room
  test, pinned off its pace toward the top and, on the ground, off the
  distance moved too) and steers at
  `NavGraph::aim`. Every non-leap jump edge is walked again that way,
  aimed there (`NavGraph::reprove_jumps`, `nav::as_a_bot`); a change to
  the bot's jump goes in `PinJump` or `as_a_bot`, or the bot and the
  graph part. What a bot's jump adds (no room test, the ramp wait) stays
  out of the flood's walks: in them it reshapes the graph. A jump edge
  is run forward whatever its drop: only the forward walk jumps, so
  every jump edge was proved that way.
- A plan round avoided edges takes no risky edge (`nav::Search`); a
  risky drop is taken only by a plan that avoids nothing.
- A following bot runs at the graph's 190 units/s: on the ground its keys
  are cut by the weapon's `moveSpeedScale` (`nav::paced_key`), never on a
  jump's tick, in the air or on the rungs. A probe that runs the bot's
  own move holds the same key.
- `PM_CheckJump` refuses a jump inside 500 ms of the last, and the graph's
  walks set off on every edge with one to hand: a bot on a jump edge waits
  on its foot until the jump is ready (`Follower::jump_wait`), and on 1.5
  and UO until the jump penalty has run out (`nav::jump_ready`).
- Every walk's body moves by the rules `nav::with_rules` set on its thread
  (`nav::body`); the build, `par_map`'s workers and both servers' bot
  passes set them (CoD2's through its graph's `NavGraph::moving`). A walk that builds its `PlayerState` with
  `PlayerState::spawn` instead proves the edge under 1.1's. A game's own
  pmove goes in through `nav::with_step` (`nav::WalkStep`); every
  `player_step` in a walk dispatches to it when one is set. CoD2's
  (`bots::cod2::Cod2Walk`) is off (bot-combat.md 16, 17).
- A UO bot sprints only along `Follower::straight_run`: level, straight,
  no jump, leap or risky edge. A sprint takes no weapon scale, so the
  graph's 190 is 304 there; a turn or a drop at that pace was never
  proved.
- The follower pulls its waypoint past grid nodes only along plain
  edges (`Follower::pull`, `NavGraph::plain_edge`), and the walking
  view turns at `Skill::walk_turn_deg` only on a plain run
  (`Bot::walk_turn`): a new edge kind or gait the bot must take as its
  walk proved it is left out of both, or the bot meets it off the line
  and the view the graph proved it on. A view set after the walk's is
  seen as owned (`Bot::view_owned`) and handed back at once; one set
  before it, as the recall look is, flags itself.
- The driver fills `BotView::drop_ahead` only on the ticks
  `Bot::reads_drops` names; a new reader in the brain widens it.
- The graph's cache keys on which submodels clip, and the server rebuilds
  it once the script's brush links (`GameHost::brush_links`) hold still
  2 s (`Server::relink_nav`): a test that runs two gametypes on one map
  builds two graphs, and one that deletes a brush mid-level waits on a
  build. `bas` breaches delete a base's walls; a UO bot test on a `bas`
  map runs a second build. A CoD4 graph clips the brush entities standing
  once the level has run a frame (`bots::cod4::BrushedClip`), its cache
  key carrying their set; a test's `bot_world(.., Vec::new())` and
  `nav_build` clip the clip map alone.
- A vehicle moved since the build does not rebuild the graph: once it
  holds still 20 ticks its nodes are blocked for the followers
  (`Server::park_vehicles`, `Follower::blocked`), and a bot hemmed in by
  them reads as stranded. A test that parks a vehicle by hand waits those
  ticks before the bots see it.
- A CoD4 bot never writes a message, so the commands it reads must be
  marked sent (`reliable_sent`) as well as acknowledged: 1.7's
  `SV_AddServerCommand` squashes only unsent ones, and a squash of one
  already read moved the next level's team menu under the acknowledge,
  so no bot played past a map change.

## Reverse engineering

| File | Role |
|---|---|
| `main/cgame_mp_x86.dll` (1.1) | MP client game: event dispatch, effects. The authority for the client. |
| `main/game_mp_x86.dll` (1.1) | MP server game |
| `game.mp.i386.so` (1.1d Linux) | same as game_mp, with full symbols |
| `CoDMP.exe` (1.1) | client netfield tables, sound system, efx renderer |
| `cgamex86.dll` (1.1) | single-player; its `EV_*` ids diverge from 173 up. Wrong module for MP. |

Image bases: `0x30000000` cgame, `0x20000000` game DLLs, `0x00400000`
CoDMP.exe. md5s are in `docs/research/cod11-events-and-fx.md`.

- `tools/re/`: `evtab.py` (`EV_*` table), `netfields.py`,
  `dump_field_table.py`, `xref.py`, `dump_script_fields.py`,
  `dump_builtins.py`, `dump_cvars.py`, `dump_itemlist.py`, and
  `annotate_func.py`, which resolves the PIC relocations that hide every call
  and cvar in a plain `objdump` of `game.mp.i386.so`. The `dump_*` game
  module dumpers read 1.1d, 1.5b and UO alike through `gameso.py`, which
  finds each table by symbol or off its lookup function and resolves the
  relocations: a pointer stored in `.data` reads as 0 in the file.
  `net-notes.md` is the server disassembly log.
- CoD2 1.3's `cod2_lnxded` is stripped and links the game in; its names
  come off the Mac 1.3 MP binary's i386 slice, which keeps its symbols.
  `ExportFuncGraph.java` dumps each analyzed program's functions
  (`*.funcs.tsv`), `symmap.py` pairs them (its header lists the passes and
  why the size filter stays), `merge_zk.py` folds in zk_libcod's
  `functions.hpp` addresses for the rest, and `check_zk.py` scores a map
  against them.
- Ghidra prints a `fistp` under a `| 0xc00` control word as `ROUND`, and
  that store truncates toward zero; read the `fldcw` before porting one
  (the bob cycle and every `delta_angles` push are such stores). A float
  times a float constant feeding one stays unrounded on the x87 stack, so
  an `f32` product in Rust can land a step off; widen it to `f64`.
- Find dispatch code by string: `CG_EntityEvent:%s`, `CG_EntityPreEvent:%s`
  (bullet impacts live in the pre-event), `fx/impacts/`. Sound-system entry
  points are catalogued in `docs/research/cod11-sound-system.md`.
