# vcod agent notes

Rust map viewer, client and dedicated server for Call of Duty 1 (2003), patch
1.1. README.md says what it does and doesn't do. This file holds what the code
and configs don't tell you: where the evidence lives, how to measure against
retail, and the traps already paid for.

Retail is the oracle. When ours and retail disagree, retail is right, and the
answer goes into a research doc with the bytes.

## Layout

| Crate | Package | Owns |
|---|---|---|
| `crates/common` | `vcod-common` | formats (bsp, xmodel, xanim, shader, pk3, menus), collision, vis, pmove, weapons, animscript, `net/` (the 1.1 protocol both ways, plus the master query) |
| `crates/client` | `vcod` | window, renderer, HUD, fx, audio, `console/` (drop-down console, cvars, binds), `frontend/` (stock main menu, server browser, options screens), `play/` (join, input, cmd clock, prediction), `probe.rs` |
| `crates/server` | `vcod-server` | `server.rs` (tick), `spectate.rs` (usercmds in, playerstate out), `game/` (script host, combat, entities, links), `bots.rs` + `nav.rs`, `rcon.rs`, `console.rs`, `cvars/`, `bans.rs`, `master.rs` (heartbeat), `area.rs` |
| `crates/gsc` | `vcod-gsc` | the `.gsc` compiler and VM (`vm/interp.rs`, `vm/sched.rs`) that runs Activision's stock gametype and map scripts |

- Dependency rules: `common` imports no wgpu, winit or kira; `gsc` depends on
  neither `common` nor the client. Check with `cargo tree -p vcod-common -e
  normal --depth 1` and `cargo tree -p vcod-gsc -e normal` (only `anyhow` and
  `log`; `vcod-common` is a gsc dev-dependency for the corpus tests).
- Grep these, don't read them whole: `client/src/probe.rs` (11k lines),
  `server/src/server.rs` (10k), `common/src/pmove.rs`, `client/src/renderer.rs`
  (5k each), `server/src/game/script.rs`, `client/src/main.rs`,
  `common/src/collision.rs`, `server/src/spectate.rs` (3k+ each).
  `docs/research/cod11-combat.md` is 300 KB; jump by section.
- Gitignored and local to one machine: `private/` (GPL lineage sources, the
  retail 1.1d Linux server, Ghidra output, old plans), `docs/design/`,
  `docs/superpowers/`, `tmp/` (scratch; probe captures land there).

## Research docs

`docs/protocol-1.1.md` is the wire reference; its last section lists every
divergence from RTCW/Q3, so check it before assuming Q3 semantics.
`docs/research/*.md` hold verified engine facts with binary addresses. Read
the subsystem's doc before touching it, and extend it with what you learn
instead of leaving the fact in a commit message.

| Subsystem | Doc |
|---|---|
| Handshake, configstrings, rcon, heartbeat, zombies, pings, rate, bans, `g_password`, private slots | `cod11-server-handshake.md` (Housekeeping section) |
| Map change, `map_restart`, `sv_serverid`, rotation | `cod11-map-cycle.md` |
| Shots, damage, death, grenades, blasts, hit locations, shot timing | `cod11-combat.md` |
| Movement, stances, constants | `cod11-mantle.md`, `bsp-ibsp59-format.md` (Movement constants) |
| Player clip, kerbs, stuck | `cod11-player-clip.md` |
| Movers, push, `linkTo` | `cod11-movers.md` |
| Items, pickups, drops | `cod11-items.md` |
| Mounted MGs | `cod11-turrets.md` |
| HUD elems, script menus, server cvar mirror (CS 140/204) | `cod11-hud-protocol.md` |
| Events and effects, module md5s | `cod11-events-and-fx.md`, `efx-grammar.md` |
| Sound | `cod11-sound-system.md` |
| gsc language and object model | `cod11-gsc-language.md`, `cod11-gsc-object-model.md` |
| `re` / `bel` gametypes | `cod11-gametypes-re-bel.md` |
| Spectator follow, killcam | `cod11-spectator-follow.md` |
| Client console, binds, `vcod_mp.cfg` | `cod11-console.md` |
| Main menu, server browser, options screens | `cod11-front-end.md` |
| Chat, quick chat | `cod11-chat.md`, `cod11-quick-chat.md` |
| Bots | `bot-navigation.md` (its Build times set the census floors), `bot-objectives.md` |
| Player models, anims, formats | `player-model-anim-system.md`, `xmodel-v14-format.md`, `xanim-v14-format.md` |
| Shader scripts, light grid, prop lighting | `cod11-shader-scripts.md`, `cod11-light-grid-and-leaf-lights.md` |
| Gamma, overbright, `r_intensity` | `cod11-gamma.md` |

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
- Before merging a feature or a fix, reread every line of AGENTS.md and
  README.md that touches what the branch changed (README's feature and
  limitation lists included), and commit a fix on the branch for each one the
  work made stale or wrong. The merge waits until both files match the code.
- Before a commit, run what CI runs: `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Set
  `COD_DIR` for the suite: tests that need paks go through
  `vcod_common::testing::game_fs()` and pass vacuously without it.
- `ci.yml` runs on ubuntu with no game data, so a new test needing paks
  returns early through `game_fs()` or uses `Pk3Fs::empty()`. `nightly.yml`
  rebuilds the rolling `nightly` release on master pushes that touch code
  (docs, tools and `*.md` are path-ignored); `workflow_dispatch` forces one.
- Ignored tests to run when you touch their area: `nav_census` (all stock nav
  graphs, ~3 min: `cargo test -p vcod-server --test nav_census -- --ignored`)
  and the two mp_ship six-bot tests in `server/tests/bots.rs` (~1-2 min together).
- The all-maps census tests scope to stock `pak[0-9].pk3`. Map downloads drop
  third-party `zzz_*.pk3` into `main/`, and a census failure on one of those
  is not a regression.
- `Server::new` seeds its RNG with a constant; only `main.rs` passes the clock
  through `Server::with_seed`. Tests that depend on spawn picks rely on that.
- Comments: one line where the code doesn't explain itself (an invariant, a
  unit or sign convention, a workaround and what it works around). Facts that
  live in a doc stay there; the comment is the pointer.

## Running

- Both binaries expect to sit next to `CoDMP.exe` and read `main/` (`uo/` with
  `--mod-dir uo`). `COD_DIR` overrides the install, `--game-dir` overrides
  both. `pak0-4` are identical between 1.1 and 1.5, so a 1.5 install works as
  the asset source; the netcode and RE notes are about the 1.1 binaries.
- Client modes: `vcod <map>` flies, `vcod <map> --walk` walks offline, bare
  `vcod` opens the stock main menu and browser, `vcod --connect ip:port`
  joins through the stock team menu (`--team spectator` spectates).
- `vcod-server <map>` runs ours. `dedicated` defaults to 1 (no heartbeat,
  unlike retail's 2); `--set dedicated=2` opts in. There is no stdin console:
  admin commands go through rcon, refused until `--set rconPassword=...`.
- `RUST_LOG` drives env_logger (default `info`); `WGPU_BACKEND` narrows the
  backend.
- F3 (`--debug-overlay`) shows frame, draw, vis, shader, fog, audio, camera
  and (online) prediction, net and snapshot counters. The `audio` line reads
  `audio vN plays N miss N cull N drop N steal N N.NNms`: live voices, cues
  started, aliases not found, cues past `dist_max`, cues refused by a full
  pool, voices stolen. The `vis` line reads `vis: <mode> cells n/m soups a/b
  tris c/d props p/q occ o h X.XXms` (occluders built, portals they hid).
  The `clock` line reads `clock: behind N  reset N fast N +N -N  extrap N`:
  newest snapshot time minus the drawn time, then `CL_AdjustTimeDelta`'s
  resets, fast and slow adjusts and the frames that reached the newest
  snapshot (docs/protocol-1.1.md, "The client's clock").
- `VCOD_NETSIM="ping=100,jitter=20,loss=2"` gives the client a bad link
  without root; `vcod --net-probe ADDR --probe-clock` measures the clock on
  one.
- F4 cycles culling `on -> locked -> off`; `locked` freezes the visible set so
  you can fly out and inspect it.
- Pre-existing noise: a few `vkAcquireNextImageKHR` fence validation errors
  per run on Vulkan, and 360-415 unique ShaderLib warnings per map load
  (tokens vcod skips by design).
- To reproduce a screenshot spot in fly mode, add a temporary
  `VCOD_POS="x y z yaw"` override to the `FlyCamera::new` spawn in `main.rs`
  and remove it before committing.
- Pixel and by-ear checks (aim sign, lean direction, shading, sound) belong to
  the human. Report them as pending.

## Measuring against retail

### Servers

- `tools/run_server.sh [map] [+set ...]` runs the **retail** 1.1d Linux
  dedicated server (setup in the script header). Anything after the map goes
  to the engine verbatim.
- `tools/run_probe.sh <probe> [map] [+set ...]` boots retail with a
  `crates/gsc/tests/fixtures/semantics/probe_*.gsc` (or
  `client-probes/probe_*`) installed as the gametype and prints its `PROBE`
  lines. `tools/capture_probes.sh > .../retail-captures.txt` reruns the
  top-level probes for `crates/gsc/tests/semantics_ab.rs`. Read that
  directory's `README.md` before writing a probe: six engine behaviours
  dictate a probe's shape.
- `tools/capture_cvars.py <port> <rconPassword>` prints a running retail
  server's cvar registry as the rows of `crates/server/src/cvars/registry.rs`.
- `cargo run -p vcod-server -- <map>` runs **ours**. `--gametype-script
  <file>` runs a gametype from disk (how a client probe runs against ours),
  `--set NAME=VALUE` is retail's `+set`, `--bots N [--bots-shoot]` adds
  clients, `--trace` logs per-snapshot timing.
- Public servers: `codmaster.activision.com:20510` still answers
  `getservers 1 full empty`. A 60-100 s spectator capture during a round
  shows every combat event.

### The probe client

`vcod --net-probe <ip:port>` is a headless client that joins, prints a
per-second snapshot summary, and dumps captures to `tmp/`. Each scripted mode
(`--save-*`, `--probe-*`) is documented on its flag in
`crates/client/src/main.rs` (`vcod --help`): what it does, which fixture it
writes, which gsc probe it pairs with. A multi-shell recipe (gsc probe, target
client, shooter client, `+set` cvars, start order, `--probe-secs`) is in the
header of the fixture it produced.

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
  or updates the assertions.

### The A/B gates

`crates/server/tests/*_ab.rs` replay a retail fixture's inputs on ours and
diff the result. Where a gate tolerates rows, it names them in a `*GAPS`
constant with the research section that explains each. Debug env vars
(`*_REPORT=1`, `*_TRACE=<ct>`, `*_DUMP=<path>`, `SLOPE_FIXTURE=<path>`) are
read near the top of each gate. Gates that need paks skip without `COD_DIR`.

## The server tick

`Server::tick` and `replay_moves` (`crates/server/src/server.rs`) follow
retail's `SV_Frame` / `ClientThink_real` / `ClientEndFrame` order, and the
comments at each step cite the section that measured it. The shape: console
lines (a `map` reloads first), timeouts, bots, pings, clock; every packet's
cmds in arrival order, each one a `ClientThink_real` with its shots traced and
their callbacks run inside the cmd (`cod11-combat.md` 16); menu responses,
the script frame; spawns, weapon, link and sim ops, mover push; missiles and
their blasts, which meet the links this frame's threads made (14.7); every
slot's end frame, then per slot the aim trace, `commit_pose` and
that slot's turret, whose rounds deliver inside its turn; outgoing commands,
snapshots, zombies, heartbeat.

Moving work across this order makes a snapshot or a later round read a
frame-old field.

`Cx::spawn` runs a script thread before the calling builtin's next
instruction; `Cx::spawn_then` adds `Host::spawn_returned` once that thread
first suspends. Only a `Cx` handed to a builtin (or to `spawn_returned`)
honours either: a spawn through `get_field`, `set_field` or `Vm::with_cx` is
dropped, so test such a builtin through `ScriptRuntime`.

## Traps already paid for

### Wire and netcode

- `entityState.weapon` is 1-based into configstring 7. The ammo and clip
  index tables also start at 1.
- `health`, `ammo[]`, `ammoclip[]` sit in the playerstate array blocks, and a
  retail client predicts its ammo counter from the clip it is sent.
- `eventSequence` counts after the write: the new events are `(prev..cur)`.
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
- The netchan scramble keeps `%`. The server reads client commands with the
  server-side string reader; the client reader's `%` mapping garbles every
  later message after a chat line containing one.
- A move message's ack is stamped with the newest frame already sent when the
  packet arrived, not the tick time, or loopback ping reads 21-50 ms.
- A cmd's clock is clamped to [-1000, +200] ms around the last frame's
  `level.time`, and the entering cmd seeds `commandTime` inside that window.
- Server commands queue until the next snapshot and squash like
  `SV_AddServerCommand` (rule in `docs/protocol-1.1.md`).
- Cvars: every `L` cvar latches until the next load, so a latched
  `g_gametype` must not reach serverinfo early. `devmap` sets `sv_cheats 1`
  after the load and `map` sets it back. The server ignores an OOB
  `disconnect`; a client honours one only after 3 s without a packet.

### Map cycle

- A map change is pull-shaped: the server sends only the OOB
  `loadingnewmap`, and resends the gamestate when a client's next message
  carries the old serverId. Nothing goes on the reliable stream.
- `map <current map>` is a restart, not a spawn.
- `map_restart` keeps the configstring table and the item registry; only a
  map load clears them.
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
- The ground trace runs origin ±0.25; a test that places a player exactly on
  a face lands allsolid and stuck. Place it 0.125 up.
- No render soup collides: model 0 clips through its brushes and lump 24
  (terrain partitions, patch facet grids). A patch's box is its subdivided
  grid grown by a unit, not its control points.
- Static props and 0x2080 kerb brushes clip bullets, blasts, missiles and
  items, never a moving player. `shot_trace`, `missile_trace` and
  `item_trace` see props, `box_trace` does not.
- A submodel's brushes clip only while its entity is linked. Script `delete`,
  `notSolid` and `_gameobjects` unlink them; a world no script runs on
  applies the rule through `CollisionWorld::unlink_script_brushes`.
- `setOrigin` and a teleport link at the unsnapped origin until the next cmd
  relinks.
- Wish speed multiplies the weapon's `moveSpeedScale` and the sight, back,
  strafe and lean scales: a plain carbine run is 224, sighted 89.7.

### Combat and items

- There is no grenade cook in 1.1 MP; "cooking" in the code means held.
  `grenadeTimeLeft` is `fuseTime` at the pullback and 0 at the throw, and the
  fuse always runs in full from release. The fuse rides the fire event's parm
  inside vcod only; the wire ring carries 0, as retail sends.
- The explode rides the missile's own entity (`eType` flips to 0), so an
  `eType == 4` filter drops the explode frame.
- A stock frag bounces off a live player: `fraggrenade_mp` has `damage 0`.
- A blast and the touch pass walk `crate::area::AreaTree`, retail's entity
  area tree, so victim and touch order is link history, not entity order. A
  client links through `GameHost::link_client`, everything else through
  `GameHost::link_entity`; an `origin` write on a non-client relinks it, a
  player `setOrigin`, spawn or death is an unlink then a link (combat doc
  14.7).
- Blast victims are measured per turn, after earlier callbacks ran. A client
  killed this frame keeps `takedamage` until its own end frame.
- A killed gunner keeps the turret until its own end-frame turn;
  `player_die` releases nothing.
- `setPlayerIgnoreRadiusDamage` is a level flag read only by the
  `radiusDamage` builtin.
- The body queue is eight entities at 64..71 with no timer; a corpse lives
  until its slot is reused.
- A `clipOnly` weapon (the frag) has no reserve entry on the wire.
- An item's `count` of 0 means unset; a drop writes -1 for empty.
- A `trigger_lookat` is never touched; it fires off `ClientEndFrame`'s aim
  trace. A `trigger_use` is never touched either (contents 0x200000); it
  fires off the use key's aim pick. A trigger-woken thread runs on the next
  frame's clock.

### Script

- gsc folds case for identifiers, fields, paths and event names but not for
  string values or array keys. A misrouted event name hangs its `waittill`
  silently.
- Unary `!` takes a string (`!"0"` is 1); an `if` on a string is fatal.
- A float prints `%g`-style (`1e-05`, `1.23457e+06`), not Rust's format.
- A BSP key reaches script only if it is in the entity field table or
  `radiant/keys.txt`; anything else is dropped silently.
- A brush model's `.model` reads `""` (its `*N` key goes to `s.index`).
- `setTimerUp` has no `> 0` check, unlike the other three timers; `bel`
  starts with `setTimerUp(0)`.
- Threads due in one frame resume newest-queued first; `wait 0` resumes at
  once; a notify's waiters run after the notifier's step, in start order.
- A thread's own `notify` doesn't fire its own `endon`.

### Assets, rendering, sound

- xanim translation keys are offsets from the bind pose. Root `tag_origin`
  sits at the feet.
- A non-player entity rotates through `AnglesToAxis`: positive pitch is nose
  down.
- A control bone (`back_*`, `neck`, `head`, `pelvis`) turns about the model's
  axes, not its own; about its own Y the body leans sideways. The torso pitch,
  torso yaw and legs yaw swing after the view on both server and client, and
  `tag_origin`'s local tag turns the body by the legs' yaw off the view, so a
  test that poses a turned body must step it to settle (combat doc 16.3, 16.4).
- The 24/30/32 pt fonts span two or three atlas pages per glyph, named in the
  `.dat`. HUD colours are display values, linearised once in the HUD pass.
- Effect shaders live in `fxshaders/` in `pak5.pk3`. Some map paths have a
  leading slash.
- The asphalt sound alias suffix is `asphault`, and vcod uses that spelling on
  purpose.
- Portal walk: a portal's plane faces out of its owning cell. vcod adds three
  over-marking rules in `common/src/vis.rs` for the mp_ship decks.

### Bots

- Nav build: `Prim::Tri` is terrain only, and `under_ground`'s ray counts
  terrain only, or a patch spar overhead makes a ladder foot read as
  underground. Minefields and `trigger_hurt` brushes are excluded.
- A* is resumable under a shared per-tick expansion budget, so a plan can
  take several ticks.

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

- Ghidra exports live under `private/ghidra/`. Grep the existing `.c` export
  before re-running; a full decompile takes ~15 minutes.
  `tools/re/ExportDecomp.java` makes one: `analyzeHeadless <dir> <proj>
  -import <dll>`, then `-process <dll> -noanalysis -scriptPath tools/re
  -postScript ExportDecomp.java`.
- `tools/re/`: `evtab.py` (`EV_*` table), `netfields.py`,
  `dump_field_table.py`, `xref.py`, `dump_script_fields.py`,
  `dump_builtins.py`, `dump_cvars.py`, `dump_itemlist.py`, and
  `annotate_func.py`, which resolves the PIC relocations that hide every call
  and cvar in a plain `objdump` of `game.mp.i386.so`. The dumpers resolve
  `.rel.data`: a pointer stored in `.data` reads as 0 in the file.
  `net-notes.md` is the server disassembly log.
- Find dispatch code by string: `CG_EntityEvent:%s`, `CG_EntityPreEvent:%s`
  (bullet impacts live in the pre-event), `fx/impacts/`. Sound-system entry
  points are catalogued in `docs/research/cod11-sound-system.md`.
