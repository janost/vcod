# vcod agent notes

Rust map viewer, client and dedicated server for Call of Duty 1 (2003), patch
1.1. README.md says what it does and doesn't do; update its feature and
limitation lists when a change moves either. This file holds what the code and
configs don't tell you: where the evidence lives, how to measure against
retail, and the gotchas already paid for.

## Layout

- Four crates. `crates/common` (`vcod-common`): formats, collision, pmove,
  weapons, animscript, `net/` (the 1.1 protocol, both directions).
  `crates/client` (`vcod`): window, renderer, HUD, fx, audio, prediction,
  `probe.rs`. `crates/server` (`vcod-server`). `crates/gsc` (`vcod-gsc`): the
  `.gsc` VM, which runs Activision's stock gametype and map scripts.
- Dependency rules: `common` imports no wgpu, winit or kira
  (`cargo tree -p vcod-common -i wgpu` is empty); `gsc` depends on neither
  `common` nor the client (`cargo tree -p vcod-gsc -e normal` shows only
  `anyhow` and `log`).
- `docs/protocol-1.1.md` is the wire reference; its last section lists every
  divergence from RTCW/Q3, so check it before assuming Q3 semantics.
  `docs/research/*.md` hold verified engine facts with binary addresses. Read
  the research doc before touching its subsystem, and extend it with what you
  learn instead of leaving the fact in a commit message.
- Gitignored and local to one machine: `private/` (GPL lineage sources, the
  retail 1.1d Linux server, Ghidra output, old plans), `docs/design/`,
  `docs/superpowers/`, `tmp/` (scratch; probe captures land there).
- Feature work happens on a branch in a worktree. Master is merge-only: `git
  fetch`, merge `origin/master` into the branch, then `git merge --no-ff` with
  `merge <branch>: <summary>`. Other sessions push to master too.
  Conventional prefixes (`feat:`, `fix:`, `docs:`, `test:`, `perf:`,
  `refactor:`, `style:`, `chore:`).

## Game data

- Both binaries expect to sit next to `CoDMP.exe` and read `main/` (`uo/` with
  `--mod-dir uo`). `COD_DIR` overrides the install, `--game-dir` overrides
  both (`crates/common/src/game_dir.rs`).
- `pak0-4` are identical between 1.1 and 1.5, so a 1.5 install works as the
  asset source. The netcode and RE notes are about the 1.1 binaries.

## Build, test, lint

- `cargo fmt`, `cargo clippy -D warnings` and `cargo test` clean before a
  commit. Set `COD_DIR` for the suite: tests that need paks go through
  `vcod_common::testing::game_fs()` and pass vacuously without it.
- `ci.yml` runs on ubuntu with no game data, so a new test needing paks returns
  early through `game_fs()` or uses `Pk3Fs::empty()`. `nightly.yml` rebuilds
  the rolling `nightly` release on master pushes that touch code;
  `workflow_dispatch` forces one.
- The all-maps census tests scope to stock `pak[0-9].pk3`. Map downloads drop
  third-party `zzz_*.pk3` into `main/`, and a census failure on one of those
  is not a regression.

## Code comments

- A comment earns its place where the code doesn't explain itself: a
  non-obvious invariant, a unit or sign convention, a workaround and what it
  works around. Usually one line.
- Facts that live in a doc stay there; the comment is the pointer.

## Running and visual checks

- `RUST_LOG` drives env_logger (default `info`); `WGPU_BACKEND` narrows the
  backend.
- F3 (`--debug-overlay`) shows frame, draw, net, vis and audio counters. The
  `audio` line reads `v N plays N miss N cull N drop N steal N N.NNms`: live
  voices, cues started, aliases not found, cues past `dist_max`, cues refused
  by a full pool, voices stolen. The `vis` line reads `vis: <mode> cells n/m
  soups a/b tris c/d props p/q occ o h X.XXms` (occluders built, portals they
  hid).
- F4 cycles culling `on -> locked -> off`; `locked` freezes the visible set so
  you can fly out and inspect it.
- Pre-existing noise: a few `vkAcquireNextImageKHR` fence validation errors
  per run on Vulkan, and 360-415 unique ShaderLib warnings per map load
  (tokens vcod skips by design).
- To reproduce a screenshot spot in fly mode, add a temporary
  `VCOD_POS="x y z yaw"` override to the spawn in `main.rs` and remove it
  before committing.
- Pixel and by-ear checks (aim sign, lean direction, shading, sound) belong to
  the human. Report them as pending.

## Measuring against retail

Retail is the oracle. When ours and retail disagree, retail is right, and the
answer goes into a research doc with the bytes.

### Servers

- `tools/run_server.sh [map] [+set ...]` runs the **retail** 1.1d Linux
  dedicated server (setup in the script header). Anything after the map goes
  to the engine verbatim.
- `tools/run_probe.sh <probe> [map]` boots retail with a
  `crates/gsc/tests/fixtures/semantics/probe_*.gsc` (or
  `client-probes/probe_*`) installed as the gametype and prints its `PROBE`
  lines. `tools/capture_probes.sh` reruns every probe into the
  `retail-captures.txt` that `crates/gsc/tests/semantics_ab.rs` compares the
  VM against. Read that directory's `README.md` before writing a probe: six
  engine behaviours dictate a probe's shape.
- `cargo run -p vcod-server -- <map>` runs **ours**. `--gametype-script
  <file>` runs a gametype from disk (how a client probe runs against ours),
  `--set NAME=VALUE` is retail's `+set`, `--bots N [--bots-shoot]` adds
  clients, `--trace` logs per-snapshot timing.
- Public servers: `codmaster.activision.com:20510` still answers
  `getservers 1 full empty`. A 60-100 s spectator capture during a round
  shows every combat event.

### The probe client

`vcod --net-probe <ip:port>` is a headless client that joins, prints a
per-second snapshot summary, and dumps captures to `tmp/`. Plain, it also logs
model lists, configstring and sound alias updates, unconsumed server commands,
trajectory changes and (at debug level) every configstring.

Each scripted mode (`--save-*`, `--probe-*`) is documented on its flag in
`crates/client/src/main.rs` (`vcod --help`): what it does, which fixture it
writes, which gsc probe it pairs with. A multi-shell recipe (gsc probe, target
client, shooter client, `+set` cvars, start order, `--probe-secs`) is in the
header of the fixture it produced. Rules that hold for every mode:

- **Committed fixtures are retail evidence.** Most modes write into
  `crates/server/tests/fixtures/` under a fixed name, so a run against ours
  overwrites the evidence. After one, move the files to `tmp/` and `git
  checkout` the directory. `--capture-tag` names a second run; tagged
  fixtures refuse to overwrite without `--overwrite-fixture`.
- **Every cmd carries the weapon `ps.weapon` says is held.** Retail reads a
  `cmd.weapon` that differs from `ps.weapon` as a holster request, and the
  byte rides only the full usercmd branch (forced by a `wbuttons`, `upmove` or
  `weapon` change). A switch holds the new index on every cmd until
  `ps.weapon` reads it, because the pickup half re-reads the byte on the frame
  the putaway ends. Captures from before 2026-09-02 carry fake putaways at
  reload, stance changes and jumps.
- **Fire is tapped, at least `fireTime` apart.** Stock rifles and pistols are
  semi-automatic; a held bit fires once. A grenade is held to arm and released
  to throw; a mounted MG reads the held bit.
- **Re-answer the team menu** after every gamestate past the first and every
  `n` under `dm`, because retail reruns `ClientConnect` and reopens the menu.
  `JoinProbe` does this; a new mode should go through it.
- **Space client commands past 800 ms.** Non-exempt commands (including bare
  `score`) inside the flood window are silently dropped; `team `, `score `
  and `mr ` (with the space) are exempt.
- An event lives four slots in the ring, so a transient needs a `!trace` per
  snapshot, not a settled sample. Two probes on one server see each other's
  entities, so a two-client question doesn't need two retail clients. The
  entity list is position-dependent (`--probe-pvs`).
- `--save-fixture` / `--save-snapshots` rewrite the parser's byte-exact
  captures in `crates/common/tests/fixtures/net/`. The `*-delta.bin` pair came
  from a lone spectator on `run_server.sh mp_carentan` with
  `SNAP_CAPTURE_TARGET` raised to ~400; the gate pins the frame counts, so a
  refresh hits the same count or updates the assertions.

### The A/B gates

`crates/server/tests/*_ab.rs` replay a retail fixture's inputs on ours and
diff the result. Where a gate tolerates rows, it names them in a `GAPS`
constant with the research section that explains each. Debug env vars
(`*_REPORT=1`, `*_TRACE=<ct>`, `*_FIXTURE=<path>`) are documented at the top
of the gate that has them.

## The server tick

`Server::tick` and `replay_moves` (`crates/server/src/server.rs`) follow
retail's `SV_Frame` / `ClientThink_real` / `ClientEndFrame` order, and the
comments cite the section that measured each step. In outline:

1. Console lines queued last frame run first (a `map` or `map_restart`
   reloads before anything else), then timeouts, bots, the clock.
2. Every packet since the last tick, in arrival order across clients; a
   packet's client commands before its usercmds. Each cmd is one
   `ClientThink_real`: pmove against the other capsules, host mirror, the
   cmd's shots, swings and throws traced and their damage callbacks run right
   there, then the touch and item passes.
3. Missiles, blasts, menu responses, `deliver_hits`, the script frame.
4. Script spawns, weapon / link / sim ops, then per slot the end frame, the
   aim trace and `commit_pose`, then turrets.
5. Outgoing commands, snapshots, archive.

Moving work across this order makes a snapshot or a later round read a
frame-old field. `docs/research/cod11-combat.md` 16 is the measurement behind
the per-cmd half.

`Cx::spawn` runs a script thread before the calling builtin's next
instruction; `Cx::spawn_then` adds `Host::spawn_returned` once that thread
first suspends. Only the `Cx` handed to a builtin honours either: a spawn
through `get_field`, `set_field` or `Vm::with_cx` is dropped and
`debug_assert`ed, so test such a builtin through `ScriptRuntime`.

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

### Evidence labels

Every claim in a research doc names the module, the virtual address and the
string or table it rests on, and carries its own label. VERIFIED: read out of
a binary, an asset or a live capture. INFERRED: read off control flow, which
includes instruction order and branch conditions ("then", "followed by",
"when that field is non-null"). One label per claim. A document may open with
a document-level VERIFIED default only when its provenance is uniform and
every exception is labelled; a section-level blanket is never allowed. Docs
carry derived facts (offsets, tables, enum orders), never pasted decompiler
output. `crates/common/tests/evidence_labels.rs` catches the mechanical cases.

## Gotchas already paid for

### Wire

- `entityState.weapon` is 1-based into configstring 7. The ammo and clip
  index tables also start at 1.
- `health`, `ammo[]`, `ammoclip[]` sit in the playerstate array blocks, and a
  retail client predicts its ammo counter from the clip it is sent.
- `eventSequence` counts after the write: the new events are `(prev..cur)`.
- `MAX_RELIABLE_COMMANDS` is 64, not 256. The server's drop notice is the
  reliable command `w "<reason>"`.
- The first usercmd of a message decodes against a base built from the
  playerstate (sight bit = `fWeaponPosFrac != 0`, stance from `eFlags`), not
  the last cmd sent. vcod's writer sends the full branch every cmd for that
  reason (`docs/protocol-1.1.md`).
- `eFlags` 0x8 flips on every spawn; leave it pinned and a client smears a
  respawn from the corpse. A respawn clears the event ring.
- The damage feedback fields never clear; `damageEvent`'s increment is the
  edge.
- A configstring range's first slot comes from its indexer: status icon, head
  icon and script menu scan from 0, localized strings and shaders from 1.
- A player's wire `solid` is packed at link time (after each cmd's move, at
  spawn), never at end frame, and the link clamps `-mins.z` to at least 1.

### Map cycle

- A map change is pull-shaped: the server sends only the OOB
  `loadingnewmap`, and resends the gamestate when a client's next message
  carries the old serverId. Nothing goes on the reliable stream.
- `sv_serverid` is two nibbles: a map load bumps the high one (skipping 0), a
  restart the low one. `console.rs` owns it.
- `map <current map>` is a restart, not a spawn.
- `map_restart` keeps the configstring table and the item registry; only a
  map load clears them.
- `game[]` and `pers[]` survive only `map_restart(1)` / `exitLevel(1)`, and
  entity handles inside `game[]` never survive. `dm` passes 0, `sd` passes 1.
- Time and score limits are script. The engine has no `CheckExitRules`;
  `g_intermissionDelay` and `nextmap` are read by nothing.
- The tick loop runs on an absolute schedule and catches up after an overrun;
  sleeping the remainder drifted `serverTime` 5-10% slow.

### Movement and collision

- Movement constants come from retail rodata (tables in
  `cod11-mantle.md` and `bsp-ibsp59-format.md`). Retail 1.1 MP has no
  mantling, no swimming and no water jump.
- 66 ms is a pmove chop, not a dt clamp; arrears past 1000 ms are dropped.
- One cmd step for every caller: `crates/common/src/pmove/cmd.rs` (`chop`,
  `player_step`). The server and the client predictor wrap it.
- The player is a capsule. A box sits `15 tan` higher on every grade.
- Terrain clips as a swept sphere, a patch as a Q3 facet; `collision.rs` picks
  by lump 24's record kind.
- Static props clip bullets, blasts and missiles, never a moving player.
  `shot_trace` and `missile_trace` see them, `box_trace` does not.
- A submodel's brushes clip only while its entity is linked. Script `delete`,
  `notSolid` and `_gameobjects` unlink them; a world no script runs on
  applies the rule through `CollisionWorld::unlink_script_brushes`.
- Wish speed multiplies the weapon's `moveSpeedScale` and the sight, back,
  strafe and lean scales: a plain carbine run is 224, sighted 89.7.

### Combat and items

- There is no grenade cook in 1.1 MP. `grenadeTimeLeft` is `fuseTime` at the
  pullback and 0 at the throw, and the fuse always runs in full from release.
- The fuse rides the fire event's parm inside vcod only; `player_step` writes
  0 to the wire ring, which is what retail sends.
- The explode rides the missile's own entity (`eType` flips to 0), so an
  `eType == 4` filter drops the explode frame.
- A stock frag bounces off a live player: `fraggrenade_mp` has `damage 0`.
- `setPlayerIgnoreRadiusDamage` is a level flag read only by the
  `radiusDamage` builtin.
- A blast walks `crate::area::AreaTree`, retail's entity area tree, so its
  victim order is link history: a client or turret link or unlink goes
  through `GameHost::area`, and a player `setOrigin`, spawn or death is an
  unlink then a link (combat doc 14.7).
- The body queue is eight entities at 64..71 with no timer; a corpse lives
  until its slot is reused.
- A `clipOnly` weapon (the frag) has no reserve entry on the wire.
- An item's `count` of 0 means unset; a drop writes -1 for empty.
- A `trigger_lookat` is never touched. It fires off `ClientEndFrame`'s aim
  trace, and a trigger-woken thread runs on the next frame's clock.

### Script

- gsc folds case for identifiers, fields, paths and event names but not for
  string values or array keys. A misrouted event name hangs its `waittill`
  silently. The two tests in `vm/sched.rs` catch a removed `fold_atom`.
- Unary `!` takes a string (`!"0"` is 1); an `if` on a string is fatal.
- A BSP key reaches script only if it is in the entity field table or
  `radiant/keys.txt`; anything else is dropped silently.
- Threads due in one frame resume newest-queued first; `wait 0` resumes at
  once; a notify's waiters run after the notifier's step, in start order.
- A thread's own `notify` doesn't fire its own `endon`.

### Assets, rendering, sound

- xanim translation keys are offsets from the bind pose. Root `tag_origin`
  sits at the feet.
- Foliage `@`/`_` skins carry inverted alpha, except `treeshdw_*`.
- Effect shaders live in `fxshaders/` in `pak5.pk3`, mostly additive. Some
  map paths have a leading slash.
- Sound alias csv columns bind by header name; a blank or 0 `dist_max` means
  `5 * dist_min`. The asphalt alias suffix is `asphault`, and vcod uses that
  spelling on purpose.
- Portal walk: a portal's plane faces out of its owning cell. vcod adds three
  over-marking rules (sliver skip, frustum-marked low cells, neighbour
  fixpoint) for the mp_ship decks. Leaf surfaces index terrain collision, not
  draw soups.
- `[profile.dev.package."*"] opt-level = 3` is for kira; unoptimized audio
  crackles.
