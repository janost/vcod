# Module map

The long form of AGENTS.md's layout table: which module owns what, per crate.
Grep the module header comments for more; each one names its research doc.

## vcod-common (`crates/common`)

- `mapworld`: the loaded map every reader takes. `mapworld::load` picks the
  loader.
- Formats: bsp, xmodel, xanim, shader, pk3, menus. CoD1 shaders compile into
  the neutral `Material` the renderer draws.
- `collision`, `vis`, `pmove` (`pmove/cmd.rs` is the one cmd step), weapons,
  animscript.
- `pmove.rs` keeps the shared tunables and re-exports the rest, so every
  `pmove::X` path holds. `pmove/` splits along retail's functions: `state`
  (the playerstate, cmd input, rules and cvars), `frame` (`PmoveSingle`, its
  arms and the dead arms), `stance` (`PM_CheckDuck`, `PM_ViewHeightAdjust`),
  `prone`, `angles` (the angle folds), `lean`, `ground` (`PM_GroundTrace`,
  water), `walk` (friction, cmd scale, accelerates, `PM_WalkMove`,
  `PM_AirMove`), `jump`, `ladder`, `slide` (`PM_SlideMove`,
  `PM_StepSlideMove`), `footsteps`, `land` (`PM_CrashLand`), `spectator`
  and `uo` (deploy, sprint, fatigue). `aim`, `cmd`, `movers`, `predict` and
  `weapon` are the client- and server-side steps around them. The tests sit
  under `pmove/tests/`, one file per seam.
- `collision.rs` keeps the public types, `CollisionWorld` and the
  always-compiled test worlds; `collision/` holds `masks` (contents and
  mask bits), `clip` (the primitive clips, no world state), `build`
  (`CollisionWorld::build`, the BVH), `entities` (model instances, poses,
  world copies, links), `query` (the trace and contents entry points),
  `reach` (the boxes a thread's queries swept, and `ClipState`) and `walk`
  (the BVH walk and the leaf-order replay).
- `dobjanim`: `DObjCalcAnim`'s tree walk and folds, CoD2's and CoD4's;
  each game brings its arithmetic and leaf sampler (`dobj.rs`,
  `xanimcalc.rs`).
- `clipmap/`: the CoD2-family clip map and its trace kernels.
  `vcod_cod2::collision` builds one from a `.d3dbsp`, `vcod_cod4::collision`
  from a clipMap_t.
- `pmshared`: what CoD2's and CoD4's pmove ports share: `pml_t`, the
  touch list and the `PmState` playerstate trait. `pmladder` is their one
  ladder, generic over `LadderPm`, which each game's `pmove/ladder.rs`
  implements with its hooks. `x87` holds both ports'
  vector helpers and angle folds, each rounding under its own name.
- `asset_source`: assets by kind and name (`docs/dev/assets.md`).
- `net/`: the 1.1 protocol both ways, 1.5 and UO 1.51 as per-connection
  `net::protocol::Cod1Wire` descriptors, plus the master query.
  - `net/wire.rs`: the `Transport` every game shares (version, master,
    codec, netchan, stock paks) and the `WireGame` trait each game's
    descriptor implements: its `id()` (game and version together, never the
    version alone: CoD4 1.7 and CoD1 1.5 both speak protocol 6), its
    connectionless framing and snapshot writer (the gamestate writer is
    each wire's own method).
  - `net/codec.rs`: the game-neutral bit reader and table-driven field codec.
  - `net/ps_arrays.rs`: the playerstate's closing array blocks, CoD1's and
    CoD2's one wire.
  - `net/msec.rs`: `Msec`, the wire and level clock every game's snapshot
    and frame carries; its arithmetic wraps as retail's `int` does.
  - `net/voice.rs`: voice chat's out-of-band packets, CoD2's and CoD4's.
  - `net/cod1/`: CoD1's tables, configstring layouts and 1.1 numbering.
- `testing`: the install fixtures tests take (`docs/dev/testing.md`).

## vcod-cod2 (`crates/cod2`)

CoD2 1.3's formats, wire (`net/`), pmove and collision.

- `map.rs` is the map loader: a `.d3dbsp` parsed once into the `MapWorld`
  and the `ClipMap` (`docs/dev/assets.md`).
- `materials.rs`, `material/` (binary materials, techsets), `efx.rs`,
  `font.rs`, `hudelem.rs`, `xmodel.rs`, `model.rs`, `weapon.rs`, `player/`,
  `water.rs`, `outdoor.rs`, `lightgrid.rs`.

## vcod-cod4 (`crates/cod4`)

CoD4 1.7's formats and wire.

- `zone.rs` reads a fastfile (`zone/<language>/*.ff`) up to its asset bodies
  (container, `XFile`, script strings, asset list).
- `walk.rs` walks every body block-exactly into `assets/` (one module per
  family; bulk data kept under `WalkOptions::keep_bulk`).
- `install.rs`: an install's zones, iwds, `fs_game` and usermaps. Tests
  and tools find it through `COD4_DIR`; the binaries through `COD_DIR` or
  `--game-dir`.
- `map.rs` builds a map zone's render world, cells, static models, entities
  and range-checked collision counts. `mapworld.rs`, `drawsurf.rs`,
  `lightgrid.rs`, `primarylight.rs`, `sunshadow.rs` feed the renderer.
- `image.rs` decodes zone pixels and `.iwi` v6.
- `source.rs` is CoD4's `AssetSource` (zones over the iwds).
- `anim.rs` turns xmodels and xanims into vcod-common's `Model` and `XAnim`
  and serves the player tree through `BodySource`
  (`AssetSource::decoded_anim` / `decoded_model`).
- `techset.rs` classifies techsets by name. `materials.rs` builds them.
- `hud.rs`: the HUD's server commands, scores, hudelems, `[{+cmd}]`
  directives, ammo slots and zone assets (fonts, images, strings, kill icons,
  message windows, compass, `weaponinfo`, rank tables). `scriptmenu.rs` and
  `menuexp.rs` the script menus and their expressions.
- `sound.rs`: the alias rows, `channels.def` and loaded sounds as `.wav`.
- `fx.rs`: an effect element's `FxElemDef` and samples; `fx_phys.rs` a
  physics model element's spawn off `fx_randomTable` as a world 1 body
  (cod4-effects.md 9.1.1).
- `ragdoll.rs`: the client ragdoll (`ragdoll.cfg`'s defs, the 1.7 dvars, the
  solver `play::cod4_players` runs on a corpse).
- `ode/`: ODE 0.5 as iw3mp.exe 1.7 built it (cod4-player-anims.md 10.4),
  the ragdoll's rigid body core, `ode/collide/` its world's contacts
  (IW's capsule colliders, the near callback, the self collision hook,
  0x5a2320's cut, 10.6; the brush geom's, `brushgeom.rs`, cod4-gsc.md
  3.6.4), `ode/phys.rs` the frame glue per world
  (stepping, drag, the after-frame sweep, the pushes, 10.7.1), and
  worlds 0 and 1's geoms (`ode/geom.rs`) and body creation off a model
  and preset (`ode/obj.rs`, cod4-gsc.md 3.6.3). All three worlds run on
  it: world 0 the dynamic entities (`dynent.rs`), world 1 the launched
  parts and the effects' physics models (`play::cod4_physics`), world 2
  the ragdolls.
- `collision/`: the clip map from a clipMap_t; `pieces.rs` the world
  collider's walk and a brush's faces and windings.
- `net/`: the 1.7 wire.
  - Huffman (`huffman.rs`), netfield tables generated by
    `tools/re/cod4_netfields.py` (`fields.rs`), delta codec, gamestate and
    snapshot parse, writer.
  - `pure.rs` the `cp` checksum, `stats.rs` the stats block.
  - `wire.rs`: `Cod4Wire` / `PROTOCOL_17`, the server's gamestate builder
    and snapshot writer.
  - `events.rs`: the event names generated by `tools/re/cod4_events.py`.
  - `flags.rs`: the `pm_type`, `eFlags`, team, `solid` and weapon class
    values as the wire words carry them.
  - `conn.rs`: the join and move messages `examples/cod4_probe.rs` and the
    client share.
  - `voice.rs`, `demo.rs`, `download.rs`, `http.rs`, `browser.rs`.
- `pmove/`: 1.7's player movement, seeded from CoD2's with its own
  playerstate. `player_step`, `PmWorld` over the clip map, `wire.rs` to the
  netfield words, `weapon.rs` the weapon half with the offhand and the melee,
  `anim.rs` the anim script the server's step runs through
  `player_step_animated`. It shares the collision kernels, the x87 helpers
  `vcod_common::pmshared` and the ladder (`vcod_common::pmladder`) with
  CoD2; the rest stays forked.

## vcod-gsc (`crates/gsc`)

The `.gsc` compiler and VM that runs Activision's stock gametype and map
scripts: `bytecode.rs` (the ops, the stack walk, and `prepare`'s fused ops
and stack sizes `Vm::install` stores), `vm/interp.rs`, `vm/sched.rs` (with
its notify and pick indexes),
`vm/profile.rs` (a game's `VmProfile`), `heap.rs` (structs, and arrays as
a list plus a map), `fxhash.rs` (the hasher its maps use).
`tests/bench.rs` is the interpreter's microbench.

## vcod-server (`crates/server`)

- `engine/`: what every server shares. The server tick order is
  `docs/dev/server-tick.md`.
  - `game.rs`: `Engine<G: GameModule>`, the server as engine core plus game
    module, with `engine::tick` / `engine::packet` (the `SV_Frame` and
    `SV_PacketEvent` order over the module's hooks),
    `engine::send_messages` (the one `SV_SendClientMessages` pump) and
    `engine::answer_status` (the `getinfo`/`getstatus` reply).
  - `core.rs` `EngineCore`, the engine state each game embeds as `core`.
  - `slot.rs` `Link`, a client slot's connection half, each client struct's
    `link`; `Conn`, the whole CoD2 and CoD4 slot, the game's half its
    `extra`. `commands.rs` CoD2's and CoD4's server-command queue
    (`SV_AddServerCommand`); CoD1's is `Client::queue_server_command`.
  - `conn.rs` what CoD2 and CoD4 run alike between a datagram and the
    level, over `ConnGame`'s hooks: packet acceptance, the command
    sequence, the reliable ring, pacing and fragments, the snapshot's
    delta, the script's commands out, `getinfo`, `getstatus`, rcon,
    `connect` up to `ClientConnect`, the shared console arms, voice
    queueing.
  - `bots.rs` CoD2's and CoD4's bot glue over `BotGame`: adding a bot,
    the reliable commands a bot reads, `AdminHost`.
  - `download.rs` serving a file: `DownloadFile`, the block window,
    `nextdl`, `retransdl` and the `wwwdl` ladder over each game's
    `DownloadWire`; each game keeps its first write (CoD1's in
    `crate::download`).
  - `link.rs` the LAN test, rates, pacing, ping. `archive.rs` the killcam
    ring. `rotation.rs` `map_rotate`'s walk. `challenge.rs` the challenge
    table. `ratelimit.rs` the query limiter. `bans.rs` `ban.txt` and the temporary bans. `gamelog.rs`
    `games_mp.log`.
- CoD1: `server.rs` (the types, construction, the packet entry and the
  test helpers several seams share), `server/cod1_game.rs` (`Cod1Game`),
  `server/cod1_frame.rs`, `spectate.rs` (usercmds in, playerstate out),
  `server/cod1_think.rs` (`ClientThink_real` for each packet since the
  last frame, in arrival order).
  `impl Server` is split by seam under `server/`, each file with its tests:
  - `frame.rs` `enter_world`, the tick, the game clock, `game_frame`,
    `replay_moves`; `snapshots.rs` the world frame, snapshot build and send,
    the archive; `follow_frame.rs` the end frame's follow pass.
  - `connectionless.rs` getinfo, getstatus, rcon, getchallenge, connect;
    `client_msg.rs` a client's message parse, commands, moves, votes, say;
    `slots.rs` gamestate, server commands, drops, zombies, pings, timeouts.
  - `level.rs` level load, scripts, cvars, serverinfo; `map_cycle.rs`
    `spawn_server`, `map_restart`, settle frames, baselines;
    `configstring_sync.rs` the configstring diff and send;
    `console_cmds.rs` the console's commands, kick and ban, status.
  - `sim_ops.rs` the script runtime and the client sims mirrored both ways;
    `vehicles.rs` UO's seat, vehicle fire and crush glue; `breach.rs` the
    bots' breach aim and shell arcs.
  - `test_hooks.rs` the `test_*` and `pin_*` seams and the readers the gates
    call.
- `cod2/`: the CoD2 engine, `Cod2Game` in `cod2/mod.rs`: handshake,
  gamestate, snapshots, spectators. `house.rs`: zombies, the download's first write,
  heartbeat. `voice.rs`: the voice relay.
- `voice.rs`: the voice queue, mute list and relay rules CoD2 and CoD4
  share.
- `cod4/`: the CoD4 1.7 engine, `Cod4Game`: handshake with stats and pure,
  gamestate, snapshots, downloads (`download.rs`), the console and OOB
  (`oob.rs`), voice relay (`voice.rs`), map cycle (`cycle.rs`), bots
  (`bots.rs`), its server-command types and squash. The level's clients go
  through `game::cod4::level::Cod4Level`. Its wire is
  `vcod_cod4::net::wire::PROTOCOL_17`.
- `game/`: the script host, combat, entities, links.
  - `game/host.rs` `GameHost`, the CoD side of `vcod_gsc::Host`: builtin
    dispatch and the entity field routing over retail's field tables.
    `game/entity.rs` the script object table (gentities and hudelems,
    numbered as the wire numbers them).
  - CoD1's level parts: `spawn.rs` (the BSP entity lump to objects),
    `combat.rs` (bullets, swings, hit locations), `missile.rs`
    (grenades), `item.rs` (items, the drop ring, pickups), `mover.rs` and
    `mover/` (the scriptent mover verbs), `trigger.rs` (the triggers and
    the touch test), `turret.rs`, `vote.rs`, `teams.rs`, and `vehicle/`
    (UO's vehicles: definitions, pool, seats, drive, guns, damage).
  - `game/profile.rs`: `HostProfile`, one static per game version naming
    every rule the host tells the versions apart by.
  - A game's own host state is `GameHost::ext`, a `HostExt` arm: CoD2's
    `Cod2HostState` through `host.cod2()` / `cod2_mut()`, CoD4's
    `Cod4HostState` through `host.cod4()`, in the game's own files; code
    every game shares takes `as_cod2()` / `as_cod4()`.
  - `game/cod2*.rs`: CoD2's level (`Cod2Level`): combat, corpses, items,
    turrets, killcam archive, gametypes.
  - `game/cod4/`: loads CoD4 1.7's zones and runs its level: the gametype's
    scripts, the clients' playerstates and their pmove over the map's clip
    map, combat (`combat.rs`, `bullet.rs`), missiles, entity damage,
    corpses, turrets, vehicles, the use key, weapons.
  - What CoD2's and CoD4's levels share: `game/use_list.rs` (the use
    list's constants, score, sort and cuts), `game/mantle_block.rs` (the
    mantle blockages), `game/turret.rs` (`GunInfo`, `gun_list`,
    `manned_gun`), `crate::follow` (the follow cycle, `StopFollowing`'s
    spot).
  - `game/script.rs` + `game/script/`: `ScriptRuntime`, the VM and host
    of one level. `load.rs` the level load and compile cache, `frame.rs`
    the frame and entity pass, `damage.rs`, `touch.rs` (the touch and use
    passes, cursor hints), `turret.rs`, `movers.rs`, `clients.rs` (per-slot
    mirrors, sessions, client events), `objectives.rs` (what the bots
    read), `testing.rs` the test seams, `tests.rs`.
  - `game/builtins/`: the script builtins per game.
  - `game/slot.rs` `ClientSlot`, a slot checked where it is made
    (`of_entity` for a script entity); `game/commands.rs` the game's queued
    server commands, each to a `Recipient` (`All` or one `ClientSlot`).
- Bots: `bots.rs` + `nav.rs`, and `bots/driver.rs`, the per-tick driver over
  `BotWorld`. `bots.rs` holds `Skill`, `Bot`, its think loop (`think`,
  `think_body`, `goal`) and the getters; the brain's seams sit beside it in
  `bots/`: `view.rs` (`BotView`, the gametype views, `BotBody`), `sight.rs`
  (sight cone, the enemy pick all three games fill, aim point, frags near,
  gunfire, the hit latch, noises, remembered spots),
  `objective.rs` (`ObjTarget`, guard posts, plant, defuse, carry, breach;
  UO's `ctf`, `dom`, `hq` and `bas` in `objective/uo.rs`), `menus.rs` (the
  menu replies, the weapon menus), `grenade.rs` (the throw's stages; CoD2's
  and CoD4's lob), `gun.rs` (CoD2's and CoD4's mounted-gun helpers),
  `bridge.rs` (CoD2's and CoD4's playerstate to and from the CoD1 body:
  the nav walks' step, the follower's body, the mantle walk, the key map),
  `fight.rs` (tracking, aim, trigger, footwork, weapon swaps) and `walk.rs`
  (the wander brain, steering, stalls, ladders). CoD1's `impl` and view
  builders are in `server/bot_world.rs`, CoD2's `impl` in `cod2/bots.rs`
  over the readers in `bots/cod2.rs`, CoD4's in `cod4/bots.rs` over
  `bots/cod4.rs`. CoD2's and CoD4's bots are one `GameBots` each
  (`bots/game_bots.rs`: the driver, the admin's ledger, the game's own
  part), and `engine/bots.rs` is the server glue the two share (adding a
  bot, `AdminHost`) over a `BotGame`. `server/bot_admin.rs` is the bot
  admin every server calls (the pass, `addbot`, `removebot`, `listbots`)
  over `AdminHost`; `botmgmt.rs` the name pool, the bot cvars and the
  planner that turns a roster into joins, drops and team moves.
  `server/bot_vehicles.rs` is the server half of UO's bots in vehicles
  (`bots::vehicle`).
  - `nav.rs` + `nav/`: `graph.rs` the graph (nodes, edges, components,
    lookups, paths), `build.rs` its build passes and ladders, `cache.rs`
    the per-map cache and `NavJob`, `memo.rs` the walks a rebuild reuses,
    `disk.rs` the disk cache, `search.rs`
    A*, `follower.rs` the path follower (`follower/reprove.rs` whether the
    edge under way holds, `follower/yields.rs` giving way), `walk.rs` the
    pmove walks with their thread-locals and `par_map`, `probe.rs` the
    world probes round a step.
- `rcon.rs`, `console.rs`, `stdin_console.rs` (stdin lines into the
  console queue), `tty_console.rs` (CoD1's tty line editor), `cvars/`, `master.rs` (heartbeat), `area.rs`
  (the entity area tree), `tick_trace.rs`, `tick_profile.rs`.

## vcod (`crates/client`)

- Window, renderer (`renderer.rs`, `renderer/`), HUD, fx, audio.
  `renderer.rs` holds `Renderer`, `new`, `build`, the frame types and the
  shared formats; `renderer/` the rest:
  - `frame.rs` the cull, the scene targets, the `FramePlan` (the frame's
    shape, decided once) and `render` with its phases.
  - `world_load.rs` `WorldGpu`, `load_world` and its helpers, unload.
  - `world_encode.rs` the world's draws into a pass or a bundle, the scene
    pass's world and entity items, the depth prepass, the dlights.
  - `shadow_encode.rs` `vc_shadows`' maps, CoD2's cookies and their
    receivers, the sun shadow.
  - `stage.rs` `StageParams`, the stage flags and deforms, the stage
    pipelines and bind groups.
  - `draw_list.rs` the CPU draw list: draw kinds, the sorted shader order,
    the merge.
  - `bundles.rs` render bundle recording, the bind group cache,
    `PassCensus`.
  - `dynamic.rs` entities and the view weapon: upload, placement, LOD,
    light pick.
  - `prop_skin.rs` how a prop's or model's skin draws, the images it loads.
  - `upload.rs` texture uploads and the test-only upload digest.
  - `fx_pass.rs` effect quads, their stages and textures, the fx lights.
  - `hud_pass.rs` the 2D layer: the HUD and debug text passes, HUD
    textures.
  - `sky_pass.rs` the farbox, cloud dome and sun sprite; `fog.rs` the fog.
  - `settings.rs` resize, the cvar setters, the counts and F3 lines.
  - `mark_pass.rs` impact marks and coronas; `cod2.rs` the CoD2 and CoD4
    lit world; `headless.rs` a windowless `Renderer`.
  - `gpu_util.rs` the wgpu descriptor builders every pass shares: bind
    group layout entries, the clamp sampler, `Fullscreen` pipelines,
    one-attachment passes, the world vertex layout.
  - `tests.rs` the unit tests; `pipeline_check.rs` and `*_live.rs` the GPU
    gates.
    `frame_rig.rs` the headless frames they and `cod2_profile.rs` (the CoD2
    `client_tools` profiles) share.
- `hud/`: CoD1's HUD (chat, messages, kill feed, scoreboard, hudelems,
  script menus, UO's parts) as quads for `Renderer::set_hud_quads`.
- `fx/`: effects (`efx.rs` the grammar, `sim.rs` the particles,
  `registry.rs` which event plays what, CoD2's and CoD4's effects and
  trails).
- `audio/`: sound for every game and the front end (aliases, banks,
  voices, streams, occlusion).
- `entities.rs`: CoD1 snapshot entities to drawables and the legs and
  torso anim driver.
- `cod2_*.rs`: CoD2's client parts, `cod2_client.rs` the session (below),
  `cod2_scene.rs` the snapshot as draws, `cod2_players.rs` the bodies,
  `cod2_hud.rs` the 2D layer over `cod2_player_hud.rs` and
  `cod2_world_hud.rs`, `cod2_viewmodel.rs`, `cod2_light.rs`,
  `cod2_glow.rs`, `cod2_front.rs` the front end.
- `console/`: drop-down console, cvars, binds.
- `frontend/`: stock main menu, server browser, options screens, Start New
  Server, the server's script menus; `frontend/cod2.rs` CoD2's. `mod.rs`
  holds `Ui`, `UiEffect` and the constants; `load.rs` the menu files and
  building a `Ui`, `stack.rs` the open menus, `script.rs` the menu scripts
  and `uiScript`, `input.rs` mouse, keys and focus, `lists.rs` the list
  boxes and the browser's poll, `paint.rs` the `Painter` and drawing.
  `scan.rs` is the socket, master query, LAN broadcast and cache write
  both server browsers (this one and CoD4's `play/cod4_menus/browser.rs`)
  run on; `item_rules.rs` the item rules `Ui` and CoD4's `ScriptMenus`
  share.
- `play/`: join, input, cmd clock, prediction, events, the replay and
  loopback gates, the client rigs the soaks and the sweep play through
  (`cod4_sweep.rs`, `cod2_soak.rs`) and the live gates on them.
- `tools/`: the CoD4 map sweep, the CoD4 and CoD2 soaks and the CoD2 map
  census (`vcod::tools`, built under the `testing` feature outside
  `cfg(test)`). `examples/client_tools.rs` runs them and
  `renderer/hud_shot.rs`'s CoD2 shots and `renderer/cod2_profile.rs`'s profiles by name.
- `lib.rs`: the module list, the arguments, `main`, `App` and its
  commands; `main.rs` only calls its `main`. `window_event`
  hands keys, pointer and wheel to `input_events.rs` and a redraw to
  `redraw.rs` (`App::redraw`): the clock, the level and join polls, the
  render cvars, the mode's frame (`fly_frame`, `cod2_frame`, `walk_frame`),
  `frame_ui`'s 2D layer, then the overlay, render and exec lines. CoD1's
  online frame is `online_frame.rs` (the read, `online_events`,
  `online_cmds`, the connect and load phases); its live frame is
  `live_frame.rs`, whose steps (`LiveCx::pose` through `LiveCx::flame`)
  run in the cgame's order.
- `session.rs`: `App`'s game, one call per frame step, input, console
  effect, renderer build and quit.
  - `Session::Cod1`, whose state is still `App`'s `mode`/`world`/`hud`.
  - `Session::Cod2`, whose online or demo session plays in `Mode::Cod2` (an
    offline `vcod <map>` on CoD2 stays `Mode::Fly`). `cod2_client.rs` is the
    session, pump, draw, view, HUD, menus and those steps, borrowing `App`
    through `session::ClientFrame`.
  - `Session::Cod4`: `cod4_client.rs` over `play/cod4.rs`, a watched CoD4
    capture or a live join (as a player or a spectator), also in
    `Mode::Cod2`. `play/cod4.rs` holds `Cod4Session`, `step` and the
    accessors; `play/cod4/` its `join.rs` (connects, downloads, probe,
    drop text), `demo.rs`, `scene.rs` (`scene()` and the drawn types),
    `events.rs` (`Fired`, the snapshot walk) and `cmd.rs` (our cmds, the
    weapon picks, the selector); its tests are `play/cod4/tests.rs` and
    `tests/*.rs` by area, their rigs `live.rs` (`live_join*`,
    `live_spectate`, `live_war_pair*`). The live harness every game's live
    tests share is `harness.rs` (the sim clock, `serve`, `run_held`,
    `live_slot`). `cod4_audio.rs` and `cod4_fx.rs` play its events' sounds
    and effects, `cod4_hud.rs` its 2D layer (`cod4_hud/`: `assets.rs`,
    `fonts.rs`, `env.rs` the menus' expression env, `board.rs`,
    `messages.rs`, `weapon_info.rs` the owner draws, `compass.rs`,
    `ticker.rs`, `world.rs`, `elems.rs` the hudelems, `text.rs`, and
    `menus.rs`, `connect.rs`, `fullmap.rs`, `hints.rs`, `grenades.rs`),
    `cod4_scoreboard.rs` its
    scoreboard, `cod4_viewmodel.rs` its first-person view weapon,
    `play/cod4_predict.rs` its prediction, `play/cod4_physics.rs` its
    launched parts and physics effect models.
  - `Session::Cod4Menu`: 1.7's front end (`cod4_front.rs`), which a join
    hands over to `Session::Cod4` and leaving the game takes back.
- `voice.rs`: CoD2's and CoD4's voice chat: the session's link, Speex,
  the microphone and the talkers' streams.
- `probe.rs`: the headless probe client (`docs/dev/retail.md`): `probe()`'s
  loop, `Save`, the wire it speaks and the helpers the modes share
  (`hold_view_yaw`, `aim_at`, `escape_ctl`). Each mode lives in `probe/`:
  - `fixture.rs` the shared writers (gamestate, snapshots, configstrings,
    playerstate, scripted), the one file write every fixture takes
    (`write_fixture_file`, `write_capture`), the `!cmd` usercmd fields
    (`cmd_fields`) and the pickup and turret `[phase]` blocks; `watch.rs` the
    default run's `ProbeWatch`; `join.rs` the menu handshake;
    `download.rs` `--probe-download`.
  - `motion.rs`, `combat.rs` (combat, ADS, sway, lean, grenade),
    `hit.rs` (shooter and target), `mapchange.rs` (map change and round
    restart), `pickup.rs`, `turret.rs`, `bump.rs`, `sd.rs` (plant and
    defuse), `triggers.rs`, `pvs.rs` (entities and PVS route), `ladder.rs`
    (ladder and weapsel): one capture each, with its fixture writer.
  - `slope.rs` the `--save-slope` capture; `scripts.rs` the walks it runs
    (`InputStep`, `InputScript`, `moves_script` and the rest).
  - `observe.rs` the print-only modes: follow, fall, compass, items, ride,
    killcam.
