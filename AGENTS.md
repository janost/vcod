# vcod agent notes

vcod is a Rust client, dedicated server and map viewer for four Call of Duty
games. `docs/features.md` says what it does and doesn't do. This file holds
what the code and configs don't tell you: where the evidence lives, how to
measure against retail, and the rules every change follows.

Retail is the oracle. When vcod and retail disagree, retail is right, and the
answer goes into a research doc with the bytes.

| Game | Version | Protocol | Install env | Detected by |
|---|---|---|---|---|
| Call of Duty | 1.1 | 1 | `COD_DIR` | default |
| Call of Duty | 1.5 | 6 | `COD_DIR` | 1.5's `pak0` checksum |
| United Offensive | 1.51 | 22 | `COD_DIR` (`uo/` beside 1.5's `main/`) | `uo/pakuo*`, `--mod-dir uo` |
| Call of Duty 2 | 1.3 | 118 | `COD2_DIR` | `main/iw_*.iwd`, `CoD2MP_s.exe` |
| Call of Duty 4 | 1.7 only | 6 | `COD4_DIR` | `zone/`, `iw3mp.exe` |

CoD1 1.5 and CoD4 1.7 both speak protocol 6: a wire is named by its game and
version together (`WireGame::id()`), never the version alone. Both binaries
run all three engines (CoD1, CoD2, CoD4); `--game cod1|cod2|cod4` overrides
the detection. CoD4 work rests on the 1.7 binaries (`objdump`, `tools/re`),
live captures, the research docs and the A/B gates.

## Layout

| Crate | Package | Owns |
|---|---|---|
| `crates/common` | `vcod-common` | what the games share: `mapworld` (the loaded map; `mapworld::load` picks the loader), CoD1 formats (bsp, xmodel, xanim, shader, pk3, menus), the neutral `Material`, collision, `clipmap/` (the CoD2-family clip map and trace kernels), vis, pmove, weapons, animscript, `asset_source`, `net/` (CoD1 1.1/1.5/UO as `net::protocol::Cod1Wire` descriptors, the master query, `wire.rs`'s `Transport` and `WireGame` trait, `codec.rs`'s game-neutral field codec, `cod1/` tables) |
| `crates/cod2` | `vcod-cod2` | CoD2 1.3 formats, wire, pmove and `collision/` (a `ClipMap` from a `.d3dbsp`); `map.rs` parses a map once into `MapWorld` and `ClipMap` |
| `crates/cod4` | `vcod-cod4` | CoD4 1.7: fastfiles (`zone.rs`, `walk.rs`, `assets/`), `install.rs`, map worlds, images, `source.rs` (its `AssetSource`), anims, HUD assets, sound, fx, ragdoll, launched parts' physics, `collision/` (a `ClipMap` from a clipMap_t), `net/` (the 1.7 wire), `pmove/` |
| `crates/gsc` | `vcod-gsc` | the `.gsc` compiler and VM (`vm/interp.rs`, `vm/sched.rs`, `vm/profile.rs`'s per-game `VmProfile`) that runs the stock gametype and map scripts |
| `crates/server` | `vcod-server` | the dedicated server: `engine/` (`Engine<G: GameModule>`, shared by all three games), CoD1's `server.rs` and `server/`, `cod2/`, `cod4/` (each game's engine module), `game/` (the script host and each game's level; `game/profile.rs` `HostProfile`), `bots/` and `nav.rs` (one driver over `BotWorld`) |
| `crates/client` | `vcod` | window, renderer, HUD, fx, audio, `console/`, `frontend/`, `play/` (join, input, prediction, events), `session.rs` (`Session::Cod1`, `Cod2`, `Cod4`, `Cod4Menu`; `cod2_client.rs`, `cod4_client.rs`), `probe.rs` (the headless probe client) |

The module map per crate (engine, game modules, bots, client sessions) is
`docs/dev/layout.md`.

Grep these, don't read them whole: `client/src/probe.rs`,
`client/src/renderer.rs`, `server/src/server.rs` (13-15k lines each),
`client/src/play/cod4.rs`, `server/src/nav.rs` (10-12k),
`common/src/pmove.rs`, `client/src/main.rs`, `server/src/bots.rs` (9-10k),
`server/src/game/script.rs`, `common/src/collision.rs`,
`server/src/spectate.rs` (5-7k). Research docs past 150 KB
(`cod11-combat.md` at 600 KB, `bot-navigation.md`, `cod2-protocol.md`,
`bot-objectives.md`, `cod11-movers.md`): jump by section.

Gitignored and local to one machine: `private/` (local reference material,
the retail 1.1d Linux server and its homepaths, old plans) and `tmp/`
(scratch; probe captures land there).

### Dependency rules

- `common` imports no wgpu, winit or kira. Check with `cargo tree -p
  vcod-common -e normal --depth 1`.
- `cod2` and `cod4` depend on `common` only among vcod's crates, and on no
  wgpu, winit or kira either.
- `gsc` depends on neither `common` nor the client: `cargo tree -p vcod-gsc
  -e normal` lists only `anyhow` and `log`. `vcod-common` is a gsc
  dev-dependency for the corpus tests.
- Assets load by kind and name through `vcod_common::asset_source`
  (`load::<T>`). A reader that only reads takes `&dyn AssetSource`, not
  `&Pk3Fs`. Which kinds cache and who owns each: `docs/dev/assets.md`.

## Before you change code

Read the pointer that matches what you are doing:

| When you are about to | Read |
|---|---|
| touch wire, map cycle, movement, collision, combat, items, script, assets, rendering, sound or bots | that game's section of `docs/dev/traps.md`, the traps already paid for |
| move work inside a server frame, add a `GameModule` hook | `docs/dev/server-tick.md` |
| add an asset kind or change a loader's cache | `docs/dev/assets.md` |
| run an install's gates, an ignored test, a census, soak, perf or bot run | `docs/dev/testing.md` |
| run a retail server, write a probe or a fixture, add an A/B gate | `docs/dev/retail.md` |
| read a retail binary or port disassembly | `docs/dev/reverse-engineering.md` |
| run the binaries, read the F3 overlay, set debug env vars | `docs/dev/running.md` |
| find which module owns something | `docs/dev/layout.md` |
| learn a subsystem's retail behaviour | its research doc (index below) |

Player-facing docs: `docs/features.md`, `docs/usage.md` (flags),
`docs/bots.md`.

## Workflow

- Feature work happens on a branch in a worktree. Master is merge-only:
  `git fetch`, merge `origin/master` into the branch, then `git merge
  --no-ff` with `merge <branch>: <summary>`. Other sessions push to master
  too.
- Commit messages take conventional prefixes (`feat:`, `fix:`, `docs:`,
  `test:`, `perf:`, `refactor:`, `style:`, `chore:`).
- Before merging a feature or a fix, reread every line of AGENTS.md,
  README.md, `docs/features.md` and `docs/usage.md` that touches what the
  branch changed (the feature and limitation lists included). Commit a fix on
  the branch for each one the work made stale or wrong. The README stays a
  short summary; detail goes in the two docs. The merge waits until all of
  them match the code.
- Comments: one line where the code doesn't explain itself (an invariant, a
  unit or sign convention, a workaround and what it works around). Facts that
  live in a doc stay there; the comment is the pointer.
- Pixel and by-ear checks (aim sign, lean direction, shading, sound) belong
  to the human. Report them as pending.

### What CI runs

Before a commit, run what CI runs:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Set `COD_DIR` (absolute) for the suite. Tests that need paks go through
  `vcod_common::testing::game_fs()` and pass vacuously without it.
  `COD2_DIR` and `COD4_DIR` do the same for their games.
- The suite is green under each install. `COD_DIR` at a 1.1 install runs the
  1.1 tests and gates; at a UO install (`uo/` beside 1.5's `main/`) the UO
  ones and whatever reads only data the two share. Which helper a test takes
  (`game_fs_11`, `uo_fs`, `stock_fs`) and which gates need which install:
  `docs/dev/testing.md`.
- `ci.yml` runs on ubuntu with no game data. A new test needing paks returns
  early through `game_fs()` or uses `Pk3Fs::empty()`: no pak test and no A/B
  gate runs in CI, only on a machine with an install.
- `nightly.yml` rebuilds the rolling `nightly` release on master pushes that
  touch code (docs, tools and `*.md` are path-ignored). `workflow_dispatch`
  forces one.
- Each crate's integration tests are one binary named after the crate
  (`--test common`, `--test cod2`, `--test cod4`, `--test gsc`, `--test
  server`; `tests/main.rs`). A new file there needs its `mod` line, and a
  file's tests filter by its name (`-- vehicle_ab::`). The client's tests
  live in its binary: `cargo test -p vcod --bins`.
- Some gates are `#[ignore]`d for their cost. Run the one for your area when
  you touch it: nav and pmove (`nav_census`), bots (`bots::`), shaders and
  layouts (`renderer::pipeline_check`), CoD2 maps (`cod2_map_census`), CoD4
  maps (`cod4_map_sweep`). Commands and costs: `docs/dev/testing.md`.

## Evidence labels

Every claim in a research doc names the module, the virtual address and the
string or table it rests on, and carries its own label.

- VERIFIED: read out of a binary, an asset or a live capture.
- INFERRED: read off control flow, which includes instruction order and
  branch conditions ("then", "followed by", "when that field is non-null").

One label per claim. A document may open with a document-level VERIFIED
default only when its provenance is uniform and every exception is labelled.
A section-level blanket, including one in a heading, is never allowed. Docs
carry derived facts (offsets, tables, enum orders), never pasted decompiler
output. `crates/common/tests/evidence_labels.rs` catches two mechanical
shapes; its header lists what a reader still has to check.

Research docs are cited by section number from code comments and other docs
(`combat doc 12.4`, `coduo-diff.md 16`). Keep numbered sections and file
names stable; extend a doc with what you learn instead of leaving the fact
in a commit message.

## Research docs

`docs/research/*.md` hold verified engine facts with binary addresses. Read
the subsystem's doc before touching it.

### CoD1 1.1 (the base the other games diff against)

| Subsystem | Doc |
|---|---|
| The wire; its last section lists every divergence from RTCW/Q3 | `docs/protocol-1.1.md` (check it before assuming Q3 semantics) |
| `clientState_t`, the third snapshot stream | `clientstate-wire-format.md` |
| Handshake, configstrings, rcon, heartbeat, zombies, pings, rate, bans, passwords, private slots, pacing, downloads, CD key | `cod11-server-handshake.md` (Housekeeping section) |
| Map change, `map_restart`, `sv_serverid`, rotation | `cod11-map-cycle.md` |
| Shots, damage, death, grenades, blasts, hit locations, shot timing | `cod11-combat.md` |
| Movement, stances, constants; the BSP, patch collision, the leaf walk | `cod11-mantle.md`, `bsp-ibsp59-format.md` |
| Player clip, kerbs, stuck | `cod11-player-clip.md` |
| Movers, push, `linkTo` | `cod11-movers.md` |
| Items, pickups, drops | `cod11-items.md` |
| Mounted MGs | `cod11-turrets.md` |
| HUD elems, script menu handshake, server cvar mirror (CS 140/204) | `cod11-hud-protocol.md` |
| Events, effects, impact marks, module md5s | `cod11-events-and-fx.md`, `efx-grammar.md` |
| Sound system and its entry points | `cod11-sound-system.md` |
| gsc language and object model | `cod11-gsc-language.md`, `cod11-gsc-object-model.md` |
| `re` / `bel` gametypes | `cod11-gametypes-re-bel.md` |
| Spectator follow, killcam | `cod11-spectator-follow.md` |
| Client console, binds, `vcod_mp.cfg` | `cod11-console.md` |
| Demos: `record`, `demo`, `.dm_<n>`, `timedemo`, `nextdemo`, `cg_autodemo` | `cod11-demos.md` |
| Main menu, browser, options, Start New Server, script menus | `cod11-front-end.md` |
| Chat, quick chat | `cod11-chat.md`, `cod11-quick-chat.md` |
| Votes: `callvote`, `vote`, `CheckVote`, the HUD vote lines | `cod11-votes.md` |
| Shader scripts, light grid, prop and entity lighting | `cod11-shader-scripts.md`, `cod11-light-grid-and-leaf-lights.md` |
| Gamma, overbright, `r_intensity` | `cod11-gamma.md` |
| Shellshock: `.shock` files, blend, kick, mouse, sound mix | `cod11-shellshock.md` |
| Player models, anims, formats | `player-model-anim-system.md`, `xmodel-v14-format.md`, `xanim-v14-format.md` |
| Client frame time (1.1 and UO) | `cod11-client-perf.md` |
| Client soak (1.1 and UO) | `cod11-client-soak.md` |

### CoD1 1.5 and United Offensive 1.51

| Subsystem | Doc |
|---|---|
| 1.5 wire against 1.1: tables, `pm_flags`, handshake, server commands, HTTP downloads | `cod15-protocol-diff.md` |
| 1.5 movement against 1.1: jump penalty, stance, lean, step, weapons | `cod15-movement.md` |
| UO wire (protocol 22), configstrings, events, vehicles on the wire, sprint, gametypes, assets | `coduo-diff.md` |
| UO vehicles: seats in and out, the seated cmd, prediction and camera, the server's spawn, drive step, guns, damage, wheel dust, death | `coduo-vehicles.md` |
| UO movement against 1.5: sprint, fatigue, LMG bipod and mounts, grenade cook, weapon time truncation | `coduo-movement.md` |

### CoD2 1.3

| Subsystem | Doc |
|---|---|
| Wire against 1.1: handshake, netchan, ops, usercmd, netfields, configstrings, entities, events, `Cod2Wire` and its CoD1 shim, map change, the client's join and commands, follow, votes, vcod's server and its housekeeping (14-17) | `cod2-protocol.md` |
| gsc against 1.1: builtin, method, field and spawn tables, `#include`, `waittillframeend`, typing, callbacks, notifies, the use key and cursor hint | `cod2-gsc-diff.md` |
| Map cycle on vcod-server: `map`, `map_restart`, `fast_restart`, `map_rotate`, rounds, intermission | `cod2-map-cycle.md` |
| Movement against CoD1: stored floats, buttons, clip order, mantle, stance, `vcod_cod2::pmove`, its A/B (`moves_ab`), prediction, first-person eye | `cod2-movement.md` |
| Maps (`.d3dbsp`, IBSP 4): lumps, collision, the trace A/B (`trace_ab.rs`), lightmaps, light grid | `cod2-d3dbsp-format.md` |
| xmodel / xmodelparts / xmodelsurfs v20 against v14: hit boxes, weights, materials | `cod2-xmodel-v20-format.md` |
| Player bodies: models, `j_*` rig, anim array, controllers, legs and torso anims, flinches, death anim, corpse | `cod2-player-anims.md` |
| Weapon files: field table, enums, changes from 1.1; the view weapon's anim tree | `cod2-weapons.md` |
| Combat: `PM_Weapon`, sight and spread, antilag, bullets, hit-location damage, death, obituary, grenades, rockets, blasts | `cod2-combat.md` |
| Killcam: the frame archive, the follow copy, the replay, its A/B | `cod2-killcam.md` |
| Mounted guns: mount, view clamp, the gunner's frame, release, `spawnTurret`, the A/B | `cod2-turrets.md` |
| Player HUD: `hud.menu` owner draws, crosshair, damage icons | `cod2-hud.md` |
| `.iwd` search path, `.iwi` images | `cod2-iwi-iwd.md` |
| Effects: `.efx` grammar, curves, spawn and motion, atlases, the spectator's effects | `cod2-efx.md` |
| Materials: binary file, techsets, techniques, statemaps, state bits, surface flags | `cod2-materials.md` |
| Fonts, menu keywords and scripts, `rect` alignment, hudelem placement, the front end | `cod2-fonts-menus.md` |
| Sound: alias csvs, `volumemodgroups.def`, `.vfcurve` falloff, sound events, whizby, loops | `cod2-sound.md` |
| Demos | `cod2-demos.md` |
| Voice chat: the wire, the relay rules, the codec at 8192 Hz, push to talk, the talker list, the mute list, the retail capture | `cod2-voice.md` |
| Client frame time, the soak | `cod2-client-perf.md` |

### CoD4 1.7

| Subsystem | Doc |
|---|---|
| Fastfiles: `IWffu100` container, `XFile` blocks, pointer tokens, the `XAssetType` table, every asset body and its `Load_*`, the zone census | `cod4-fastfile.md` |
| Map worlds: vertices, surfaces, cells and portals, static models, techset names, images, the map census | `cod4-world.md` |
| Handshake, stats phase and block, pure `cp`, netchan, Huffman, configstrings, what vcod's server sends, its console | `cod4-handshake.md` |
| Netfields, delta codec, snapshots, the move message | `cod4-netfields.md` |
| The server frame (`SV_Frame`) order | `cod4-server-frame.md` |
| Collision: the clip map, leaf brush trees, triangles, submodels, static model clip, the clip census | `cod4-collision.md` |
| Player movement: flags, ground, step, stance, jump, sprint, dvars, the moves A/B (`cod4_moves_ab`), the server's think and spawn, `placeSpawnPoint`, the walk replay (`cod4::wire_ab`) | `cod4-pmove.md` |
| Combat on vcod-server: `WeaponDef`, `PM_Weapon`, the shot, hit boxes, antilag, damage, corpse, dropped weapons, killcam, the A/B (`cod4_combat_ab`), the capture gates (`capture_ab`), grenades, launcher, melee, `kill` | `cod4-combat.md` |
| Player bodies: xanim parts, skinning, anim index, heads and tags, the anim script (`pmove::anim`), ragdoll | `cod4-player-anims.md` |
| HUD: fonts, server commands, scoreboard, kill feed, chat, hudelems, weapon HUD, compass, killcam, the script menus a join answers, damage overlays | `cod4-hud.md` |
| Sound: alias lists, streamed files, `channels.def`, falloff, event and loop sounds, createfx sounds | `cod4-sound.md` |
| Effects: `FxEffectDef`, impact table, event effects, looping and tag-riding effects, createfx, client bullets and `whizby` | `cod4-effects.md` |
| The watched and played view: clock (`CL_AdjustTimeDelta`), lerp, teleport cuts, view weapon, prediction (`play::cod4_predict`), hardpoint jets and helicopter | `cod4-client-view.md` |
| gsc: dialect over CoD2's, builtin tables, configstring slots, callbacks, connect, menus, loadout and spawn, server commands and squash, follow, hardpoints, script traces, launched parts on the client | `cod4-gsc.md` |
| Map cycle (`map`, `fast_restart`, `map_rotate`), the client's map change, votes, the menu answers' one-packet rule | `cod4-map-cycle.md` |
| Front end: `ui_mp` main menu, menu stack, profiles, Create a Class and its stat commands, Start New Server, Mods, menu sounds and music | `cod4-front-end.md` |
| Voice chat | `cod4-voice.md` |
| Demos | `cod4-demos.md` |
| Map x gametype sweep, the soak | `cod4-map-sweep.md` |
| Client frame time: `VCOD_FRAME_PROFILE`, `tools/cod4_perf.sh`, the per-map table | `cod4-client-perf.md` |
| What the client still lacks, ranked | `cod4-gaps.md` |
| What the server still lacks, ranked | `cod4-server-gaps.md` |

### Bots (all games)

| Subsystem | Doc |
|---|---|
| Nav graph, build times (the census floors) | `bot-navigation.md` |
| Objectives per gametype and game | `bot-objectives.md` |
| Combat, skill levels, the `bot_match` numbers (section 1), before and after tables | `bot-combat.md` |
