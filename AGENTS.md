# vcod agent notes

Rust map viewer, spectator client and dedicated server for Call of Duty 1
(2003), patch 1.1. Usage, controls and known limitations are in README.md; this
file holds what the code and configs do not confess: where the evidence lives,
how to verify visually, how to debug the netcode, and how the reverse
engineering setup works.

## Layout and where facts live

- Cargo workspace. `crates/common` (`vcod-common`): formats (`bsp.rs`, `xmodel.rs`,
  `xanim.rs`, `animtree.rs`, `pk3.rs`, `assets.rs`), `collision.rs`, `props.rs`, `pmove.rs`,
  `weapon.rs`, `skeleton.rs`, `game_dir.rs`, `testing.rs` and `net/` (the CoD
  1.1 protocol,
  both directions). `crates/client` (`vcod`): window, renderer, entities, hud,
  fx, audio, `probe.rs`. `crates/server` (`vcod-server`): the dedicated server.
  `crates/gsc` (`vcod-gsc`): a virtual machine for CoD's script language
  (`.gsc`) — lexer, parser, bytecode compiler, instruction loop, thread
  scheduler and cross-file loader. At map load the dedicated server loads the
  gametype and map scripts and runs Activision's stock bootstrap to
  completion, which is what fills the configstring table; the rest of the
  shipped gameplay scripts wait on clients existing and on the builtins those
  paths call. `vcod-gsc` must not depend on
  `vcod-common` either, same rule as `common` itself: `cargo tree -p vcod-gsc
  -e normal` shows only `anyhow` and `log`. Nothing in `common` may import
  wgpu, winit or kira; `cargo tree -p vcod-common -i wgpu` proves it. `cargo
  build -p vcod` / `-p vcod-server` build one; plain `cargo build` both.
- `docs/protocol-1.1.md` is the wire-protocol reference. `docs/research/*.md`
  hold verified format and engine facts with binary addresses as evidence. Read
  the research doc before touching the subsystem it covers; extend it when you
  learn something new instead of leaving the fact in a commit message.
- `private/`, `docs/design/` and `tmp/` are gitignored. `docs/design/` holds
  the per-project design documents. `private/` is for material that must
  not ship: the GPL sources read as lineage (Quake III Arena, RTCW-MP, ioq3,
  CoDExtended), the retail 1.1d Linux dedicated server and its homepath,
  Ghidra decompilations, and old task plans. `tmp/` is scratch: probe captures
  land there. `private/` and `docs/design/` exist only on my machine, so a
  public clone does not have them.
- Feature work happens on a branch in a worktree; master is merge-only.
  Conventional commit prefixes (`feat:`, `fix:`, `docs:`, `test:`, `perf:`,
  `style:`, `chore:`).

## Game data

- Both binaries expect to sit inside the CoD 1.1 install, next to `CoDMP.exe`,
  and read the paks from `main/` (or `uo/` with `--mod-dir uo`). `COD_DIR`
  overrides the install directory, `--game-dir` overrides both
  (`crates/common/src/game_dir.rs`).
- Game assets in `pak0-4` are identical between 1.1 and 1.5, so a 1.5 install
  serves as asset source. The 1.1 binaries are what the netcode and the
  reverse-engineering notes are about.

## Build, test, lint

- `cargo build`, `cargo test`, `cargo fmt`, `cargo clippy` clean before a commit.
- Tests that need game data go through `vcod_common::testing::game_fs()`, which
  reads `COD_DIR` and returns `None` when `$COD_DIR/main` is missing, so a green
  run on a machine without the game proves nothing about parsers. Set `COD_DIR`
  when running the suite. Net parser tests read the committed captures in
  `crates/common/tests/fixtures/net/` and run anywhere.
- CI is two workflows in `.github/workflows/`. `ci.yml` runs fmt, clippy
  (`-D warnings`) and the suite on every PR and master push, on ubuntu with
  no `COD_DIR`, so anything that hard-requires game data breaks it; a test
  that needs the paks goes through `game_fs()` and returns early, or uses
  `Pk3Fs::empty()`. `nightly.yml` builds release binaries for linux amd64,
  windows amd64 and macos arm64 and replaces the rolling `nightly` release
  with them. It skips master pushes that only touch docs or `tools/`
  (`paths-ignore`); `workflow_dispatch` forces a build when you want one
  anyway.
- The all-maps material and parser census tests scope to stock `pak[0-9].pk3`
  because live-server map downloads drop third-party `zzz_*.pk3` files into
  `main/`; a failing census on a custom pak is not a regression.

## Code comments

- A comment earns its place only where the code alone does not explain
  itself: a non-obvious invariant, a unit or sign convention, a workaround
  and what it works around. Code that reads plainly gets none.
- Keep each one as short as it can be, typically one line.
- Facts that live in `docs/research/*.md`, `docs/protocol-1.1.md` or another
  doc stay there; the comment, if any, is the pointer, not a copy.

## Running and visual verification

- `RUST_LOG` drives env_logger (default `info`). wgpu picks the backend;
  `WGPU_BACKEND=vulkan|gl|dx12|metal` narrows it.
- `--debug-overlay` or F3 at runtime: frame time, worst frame, draw stats, net
  interp misses and anim restarts per second, `ev seen/unk` and the `audio`
  line (format under Gotchas).
- F4 cycles culling `on -> locked -> off`. `locked` freezes the visible set
  so you can fly out and see what the camera was drawing; `off` is the
  unculled A/B. The F3 `vis` line reads `vis: <mode> cells n/m soups a/b
  tris c/d props p/q occ o h  X.XXms`, where `occ o` counts occluder volumes
  built for the visited cells and `h` the portals they hid (On mode only).
- A handful of `vkAcquireNextImageKHR` fence validation errors per run are
  pre-existing noise on the Vulkan backend.
- Screenshots: capture the active window only; whichever tool the desktop
  offers. Key injection for F3 and the like needs a tool that works under the
  session's display server.
- To reproduce a spot from a screenshot in fly mode, a temporary `VCOD_POS="x y z yaw"`
  override in `main.rs` spawn is the fastest path; remove it before committing.
- Pixel-level checks (aim sign, lean direction, prop shading) are for the human
  to eyeball; flag them as pending rather than declaring them verified from
  code reading.

## Netcode debugging

- `--net-probe <ip:port>` (`crates/client/src/probe.rs`) is the headless
  client: connects, prints a one-line snapshot summary each second (serverId,
  delta base, ps.origin, entity count), nudges forward for 2 s every 30 s so
  the log shows whether the server still applies moves, dumps captures to
  `tmp/`, exits on drop. It also prints the model list at gamestate and flags
  per second any player/corpse whose body is not a `playerbody_*`, any client
  body `modelindex` change, each corpse's appear/vanish with its lifetime and
  the dead client's `modelindex`, and any moving map prop, so a "wrong model"
  report can be chased without the GUI. For audio it prints the ambient configstring
  3, the `CS_SOUNDS` (524+) alias block at gamestate and every later update
  inside it, each `EV_SOUND_ALIAS` with its resolved alias name and origin,
  every `loopSound` transition, and every server command the client does not
  consume (`s <idx>` carries the announcer alias). It also resolves every
  drained event into sound cues against the map's alias table and prints
  `audio: <n> cues, <n> alias misses` per second; weapon-file cues (fire,
  reload) are not resolved headlessly, since the probe loads no weapon files.
  At debug level it also dumps every non-empty configstring as `cs[i] = …`,
  which is how the retail configstring tables in the research docs were taken.
  `--probe-pvs` joins the same way and walks a route, printing the entity list
  at each station and every entity that appeared or vanished in between with
  the position it happened at; that is what established that the entity list
  is position-dependent (`docs/protocol-1.1.md`, "Which entities a client is
  sent"). Two probes on one server see each other's entities, so a
  two-client entity question does not need two retail clients.
  `--save-combat` joins the same way and runs a scripted weapon sequence
  (single shot, sustained fire, reload, fire crouched, fire prone), writing
  `crates/server/tests/fixtures/playerstate/<map>-<gametype>-combat.txt`. The
  fire bit is tapped, not held: the stock `m1carbine_mp` is semi-automatic and
  a held bit fires one shot and then nothing. A shot is a transient the event
  ring overwrites within four slots, so that fixture carries a `!trace` line
  per snapshot instead of a settled sample. It is also the one mode with the
  stall response on -- a walk that stops against geometry turns 45 degrees and
  tries again -- since every other scripted mode holds an exact input and must
  not wander. What the two committed captures measured is in
  `docs/research/player-model-anim-system.md`, "The weapon channel: what writes
  `torsoAnim`"; run it on two maps, because the map picks the weapon and the
  bolt-action mosin on mp_pavlov is what exposed the rechamber the carbine
  never reaches.
  `--save-ads` is the same machine on a sight script: sight held, released,
  held through a reload (after a shot, since a full clip refuses one), held
  through a shot and while walking, plus a hip shot and a walk for the
  spread counter; it writes `<map>-<gametype>-ads.txt` with `fWeaponPosFrac`
  and `aimSpreadScale` on every `!trace` line, and the same gate replays it.
  It is the capture that found the usercmd delta base (Gotchas).
  `--save-grenade` is the same machine on a grenade script: one melee swing
  with the rifle the join chose, a switch to the frag, a cooked throw, a cook
  held past the pin, a cook cancelled by switching back mid-hold, and a throw
  aimed at the ground. It writes `<map>-<gametype>-grenade.txt` with
  `grenadeTimeLeft` and `weaponDelay` on every `!trace` line and a `!missile`
  line after each trace that had a grenade on the wire. Two step shapes are
  new and both are on the `!input` line so a gate replays them: a held input
  rather than a tapped one (`press_buttons`, `press_ms`), since a grenade is
  cooked by holding the trigger and thrown by the release, and a weapon
  switch (`switch_weapon`, `switch_ms`). The switch is not one cmd: retail's
  pickup half reads `cmd.weapon` again on the frame the putaway ends
  (`docs/research/cod11-combat.md`, 1.8), so a byte reverted before then
  leaves the old weapon in hand, and the probe holds the index on every cmd
  until `ps.weapon` carries it. A first retail capture measured the one-cmd
  version doing nothing: the frag never arrived and the cook fired the rifle.
  The `!missile` line records every entity that
  reads `eType` 4 or read it earlier in the run and has not left the wire yet:
  the explode flips the missile's own `eType` to 0 and adds its event there,
  so an `eType`-4 filter drops the explode frame
  (`docs/research/cod11-combat.md`, section 13). The header's `# grenade` line
  carries the frag's configstring 7 index and the origin and view the script
  started from, which is the spot a replay has to throw from.
  Every capture cmd carries the weapon the playerstate says the client holds.
  A cmd with `weapon` 0 is not neutral: retail reads a `cmd.weapon` differing
  from `ps.weapon` as a request to holster, and the byte travels only in the
  full usercmd branch, which a `wbuttons`, `upmove` or `weapon` change forces.
  Captures taken before 2026-09-02 therefore carry a putaway at the reload
  key, at a stance change and at a jump that no input asked for, and their
  reload key never reloaded, which two research docs wrote up as retail
  behaviour. A new probe mode must set the byte.
  `--probe-target` and `--save-hit` are the two halves of the hit capture, one
  probe each, writing
  `crates/server/tests/fixtures/playerstate/<map>-<gametype>-hit-target.txt`
  and `-hit-shooter.txt`. The target stands still, sends the `kill` client
  command every 45 s and presses use 3 s after each death, which is what puts a
  death, a corpse, an obituary and a respawn in the capture without needing the
  shooter to land a shot; the shooter walks toward it and fires once it has a
  clear eye-to-eye trace through the map's collision. Start the target first
  and give it a longer `--probe-secs` than the shooter. Under `dm` the shooter
  never gets a shot at a player: the deathmatch spawn picker puts a respawning
  client at the point farthest from the other one, and the walk does not cross
  a town in the 150 s it is given, so the committed `dm` shooter fixture opens
  with `# BROKEN no line of sight after 150 s` and holds the death half only.
  The hits come from `tdm` with friendly fire on
  (`tools/run_server.sh mp_carentan +set g_gametype tdm +set scr_friendlyfire 1`,
  both probes `--probe-team allies`): team deathmatch spawns a player next to
  its team, so the two start a few hundred units apart, and `scr_friendlyfire 1`
  is what lets a teammate's bullet do full damage. Anything after the map name
  is passed to the engine verbatim, which takes its `+set` arguments in any
  order. Each fixture is named for the gametype it was taken under, and both
  pairs are committed. What they measured is in
  `docs/research/cod11-combat.md`, section 8.
  Both modes point at any server, ours included, and both write into the same
  committed fixture names, so a run against `vcod-server` overwrites the
  retail evidence: move the two files out to `tmp/` afterwards and
  `git checkout` the directory. What such a run measured about vcod is in
  `cod11-combat.md` section 9. The death half needs only `--probe-target`,
  since the target's own `kill` command does the killing; the hit half needs a
  map small enough for the walk to cross, which mp_carentan is not in `dm`,
  for ours and for retail alike.
  `--probe-sweep` turns `--save-hit` into a measurement instead of a capture:
  the shooter taps once per entry of a static table of pitch offsets around
  the aim at the target's eye, echoes the offset as `pitchOffset` on each
  `!trace` line, and writes no fixture, so a sweep cannot clobber the
  committed evidence. Pair those lines with the `sHitLoc` the server's own
  `games_mp.log` `D;` records carry and every vertical hit-location boundary
  falls out of the two; run it against retail and against ours and the
  answers are directly comparable. An offset is spent only on a tap that had
  a live target to hit, so give both probes a long `--probe-secs`. The sweep
  walks to within 120 units before its first tap (`SWEEP_RANGE`), because at
  350 the hip cone blurs a tap over nine units and labels nothing; the
  target probe still writes its own fixture, so `git checkout` the fixture
  directory after a run against ours. `tools/pair_sweep.py` pairs the two
  probe logs with the server's `D;`/`K;` lines and prints the height each
  hit crossed the victim at; both runs and what they settled are in
  `docs/research/cod11-combat.md`, section 3.4.
  `--probe-sway` is the sight-sway measurement: the combat machine on a
  script of eight scoped shots standing still, the view turned down the
  longest clear sightline from the spawn, printing every bullet-impact temp
  entity's origin beside the eye and view it left from. With
  `--probe-weapon kar98k_sniper_mp` (`--probe-team axis` on carentan) the
  shot has no spread, so the angle between the raw view and the eye-to-impact
  ray is the sway retail put on it, and `docs/research/cod11-combat.md`
  section 15 is what the same run against retail and against ours read.
  It writes no fixture.
  `--probe-melee`, `--probe-grenade` and `--probe-grenade-death` swap the hit
  pair's bullet script for another one. Melee walks to within `MELEE_RANGE`
  (40 units; retail's swing reaches 64) and taps the melee bit through the
  same three firing phases. Grenade walks to within `GRENADE_RANGE` (300),
  switches to the frag, holds the trigger a second, releases at the target's
  feet, watches the missile out for 8 s and then throws a second one, barely
  cooked, at the ground beside it. `--probe-grenade-death` is that with a
  `kill` sent 500 ms into the cook, which is what puts the grenade a death
  drops on the wire. Each names both halves after itself --
  `<map>-<gametype>-melee-shooter.txt`, `-grenade-target.txt` and so on -- so
  the flag goes to the `--probe-target` half too, or that half writes over the
  committed bullet capture. The shooter's fixture gains `grenadeTimeLeft` and
  `weaponDelay` on every trace, the same `!missile` lines the lone capture
  carries, and an `!event` line per pullback, melee swipe, hit, miss, bounce
  and explode with the entity that carried it, which is what says whether
  retail put one on a temp entity or on the missile's own ring. All seven of
  these fixtures are committed retail evidence and a run against ours
  overwrites them: move them to `tmp/` and `git checkout` the directory after.
  `--save-mapchange` and `--save-roundrestart` are the map-cycle captures.
  They record the wire rather than one playerstate: every gamestate with its
  `serverId`, every serverCommand with its reliable sequence, every
  out-of-band packet and one `!trace` per snapshot whose watched fields
  moved, all interleaved by `ms` into
  `crates/server/tests/fixtures/netchan/<map>-<gametype>-<role>.txt`, named
  for the map the run started on. `--save-mapchange` stands still through a
  map end and the rotation that follows; `--save-roundrestart` is a pair,
  the `--probe-target` half killing itself 20 s in to end the round and the
  other half walking up and only watching. The recipe and the cvars each
  needs are in its own fixture's header. Retail never pushes the `b`
  scoreboard, it only answers `score`, so `--save-mapchange` asks every 2 s
  and every `b` in that fixture is an answer; the round-restart shooter asks
  for none and its fixture carries none. Both are retail evidence and a run
  against ours overwrites them: move the files to `tmp/` and `git checkout`
  the directory after.
  A probe that crosses a gamestate or a map restart has to re-answer the
  stock menus: retail reruns `ClientConnect` on both and reopens the team
  menu under the indices the last one used, so `JoinProbe` clears them on
  every gamestate past the first and on every `n`. Without that the probe
  sits on the menu for the whole rest of the run. The dm capture shows the
  restart case; sd, whose `pers[]` survives, reopens no menu and the clear
  is inert there.
  `--probe-slope` walks the `--probe-pvs` route with the sight held and
  counts, per second and for the run, the snapshots off the ground, the
  sight-ramp reversals, the `EV_STEP_VIEW` (143) events with their parms and
  the mean speed; it writes no fixture, and the same run against retail and
  against ours is the comparison. `--probe-cmd-ms N` sets the interval the
  probe sends usercmds at (default 16; a 125 fps retail client sends every
  8 ms), for every mode. Retail's deathmatch spawn for a lone client is
  random and often indoors, so a run that covers ground takes a few tries;
  read `moved` off the per-second summary before trusting a total.
  `--save-slope` is that walk written down: every usercmd as it went on the
  wire and every snapshot's origin, velocity, ground entity, view angles and
  sight fraction, interleaved, to
  `crates/server/tests/fixtures/playerstate/<map>-<gametype>-slope-<ms>ms.txt`
  (`--capture-tag` for a second run). `playerstate_slope_ab.rs` replays the
  cmds on our mover twice, once free-running from the first snapshot and
  once rebased on retail's state at every snapshot, and diffs the origin at
  each snapshot's `commandTime`; `SLOPE_REPORT=1` prints every row and the
  worst spots with the normal under them, `SLOPE_FIXTURE=<path>` with the
  ignored test replays a run kept in `tmp/`, and `SLOPE_TRACE=<ct>` prints
  ours cmd by cmd into that clock. The rebased error is what a retail client
  predicting on our snapshots sees as a correction, so it is the number a
  view twitch report turns into. Both committed fixtures are retail
  evidence and a run against ours overwrites them.
  `--probe-team <allies|axis>` picks which team the stock menu is answered
  with, and on its own makes the probe join and then report the roster
  (`num:team=N "name"`) once a second, writing no fixture; two probes with
  it on opposite teams is how `clientState.team`'s four values were measured
  (`docs/research/clientstate-wire-format.md`).
  `--probe-secs N` extends the default 65 s; a few minutes spans an SD round
  restart. Add `--save-fixture` or `--save-snapshots` only when the capture is
  meant to replace the committed evidence; the flag docs in
  `crates/client/src/main.rs` say why the two are separate. `--save-snapshots`
  stops at `SNAP_CAPTURE_TARGET` (24) messages, which is enough to pin the
  uncompressed connect-time frames but not a single delta. The
  `gamestate-delta.bin` / `snapshots-delta.bin` pair
  (`writer_reproduces_the_captured_snapshots_byte_for_byte`, snapshot.rs) came
  from raising that cap locally and running `--net-probe` with
  `--save-fixture --save-snapshots` for ~400 messages against
  `tools/run_server.sh mp_carentan`, as a lone spectator that never joins a
  team. Each snapshot fixture is a run of `[u32 message_num][u32 len][len
  bytes]` triples (`SnapshotCapture`, `crates/common/src/net/mod.rs`); each
  payload is `[u32 reliableAcknowledge][huffman block]`. The gate pins the
  frame counts exactly (400 steady, 399+ of them deltas), so a refresh has to
  hit the same count or the assertions need updating alongside it.
- A map change is a re-sent gamestate on the live netchan; the net client
  applies it while `Active` and clears its snapshot ring. The client's per-map
  state lives in `App.world`, the renderer's `WorldGpu` and `Phase::Live`;
  `loading.rs` is the pure download/load state machine the redraw loop steps.
- Two local servers, don't confuse them. `tools/run_server.sh [map]` runs the
  **retail** 1.1d Linux dedicated binary (not in the repo; see the script
  header for where it goes and what it needs). It is the oracle for every wire
  question: when ours and retail disagree, retail is right, and the answer
  goes in a research doc with the bytes. It answers the handshake, the
  gamestate, and sends snapshots to a lone spectator too, with no need to
  join a team (`crates/common/tests/fixtures/net/snapshots-delta.bin` is
  exactly that capture). It is also what retakes the configstring gate's
  fixtures: `tools/run_server.sh <map>` in one shell, `--net-probe
  127.0.0.1:28960 --save-configstrings` in another, which writes
  `crates/server/tests/fixtures/configstrings/<map>-<gametype>.txt` for
  `crates/server/tests/configstrings_ab.rs`. `cargo run -p vcod-server -- <map>` runs **ours**:
  the handshake, the gamestate, client commands and moves, and snapshots
  delta-compressed against the client's acked frame, with pmove-driven
  spectator flight, `--test-entities` for scripted packet entities and
  `--set NAME=VALUE` (retail's `+set`, e.g. `--set scr_friendlyfire=1` for
  a teammate kill). A snapshot's entity list is the map's own: placed weapons,
  script models and mounted MGs, culled per client against the BSP's PVS the
  way retail culls, so what a client is sent depends on where it stands. Other
  clients are in it too, each animated by the animscript machine
  (`crates/common/src/animscript.rs`, and
  `docs/research/player-model-anim-system.md` for what the retail captures
  measured): stance, direction, strafing, the jump and the landing all pick an
  index out of `mp/playeranim.script`, and a swing draws among the
  `meleeattack` clause's lines. What the machine does not cover yet is the two
  turn movetypes and the mounted-MG anims. A shot is a trace against the world
  and every live player's box, a hit runs the stock
  `CodeCallback_PlayerDamage`, and `finishPlayerDamage` is where health,
  knockback, the pain and death events and `CodeCallback_PlayerKilled`
  happen (`crates/server/src/game/combat.rs`, `docs/research/cod11-combat.md`).
  A kill puts a corpse in the eight-slot body queue at entities 64..71, sends
  the obituary on both wires, scores it, drops the dead player's weapon as an
  item, and the victim respawns on the use key. A melee swing is the same
  trace over 64 units, with `MOD_MELEE` damage and its own hit or miss event.
  A grenade is a real missile entity (`crates/server/src/game/missile.rs`,
  `docs/research/cod11-combat.md` 11 to 14): the pullback arms it, the release
  spawns an `eType` 4 that flies on a gravity trajectory, bounces off world
  and props, comes to rest, and explodes on its own ring at the end of its
  fuse, with the blast walking live clients through retail's linear falloff
  and `CanDamage`'s five-trace fraction. A player killed mid-cook drops the
  live one. A level ends the way retail's does, in script: `exitLevel` and
  `map_restart` queue a console line, and the console
  (`crates/server/src/console.rs`) runs `map`, `map_restart` and `map_rotate`
  off `sv_mapRotation`, so a `dm` time limit reaches the intermission, the
  intermission holds every client at `pm_type` 5 for the script's own wait,
  and the next map's gamestate goes out on the live netchan
  (`docs/research/cod11-map-cycle.md`). Not modelled: item pickup and the
  killcam. What a client still gets nothing of is movers, which no code
  spawns. A probe run against it reproduces the retail death capture
  field for field except for two: the `EV_RAISE_WEAPON` the death frame does
  not raise, and the `legsAnim` the respawn frame carries a frame late
  (`docs/research/cod11-combat.md` section 9). What the map-cycle probes
  measured of it is `docs/research/cod11-map-cycle.md` section 8.
- The tick, in order: the console drains first (a `map`, `map_restart` or
  `map_rotate` line an earlier frame's script queued reloads the level before
  anything else runs), then expired clients, then each client's queued usercmds
  (`replay_moves`, one pmove step per cmd, which is where the weapon machine
  queues a frame's shots, swings and throws), then those themselves (a trace
  each, an impact temp entity and a hit per player struck), then the missiles
  fly and any due fuse explodes, then the blasts become hits, then the client
  commands that start a script thread (`kill`, `mr`), which the packet pass
  only queues because it runs before the clock advances, then
  `deliver_hits` so the damage callback has run before script, then the
  script frame, then the sim ops the
  script left (spawns, weapon gives and switches, the damage the callback
  did), then the host-to-sim mirrors (weapons held, origin, health, the damage
  feedback `P_DamageFeedback` computes from the health the hit left), then the
  entities are built once and culled and written per client. Move anything
  across that order and a snapshot reads a frame-old field.
- `Cx::spawn` is for a builtin that needs script to run before its caller's
  next instruction: the queued thread starts the moment the builtin returns,
  which is how `finishPlayerDamage`'s killing hit gets
  `CodeCallback_PlayerKilled` to have written `self.sessionstate` before the
  line after it reads the field. Only the `Cx` a builtin is handed honours it.
  A `Cx` from `get_field`, `set_field` or `Vm::with_cx` has no defined point
  at which a thread could run, so a spawn queued through one of those is
  dropped and `debug_assert`ed against; test such a builtin through
  `ScriptRuntime`, never through `with_cx`.
- `tools/run_probe.sh <probe> [map]` drives the same retail binary as the
  gsc oracle: it drops one `crates/gsc/tests/fixtures/semantics/probe_*.gsc`
  in as a gametype script, boots the server, and prints the `PROBE` lines
  the script logged. Anything after the map goes to the engine verbatim,
  which is how the three `probe_persist_*` probes get the `sv_mapRotation`
  they need to have a map to load after ending their own; `PROBE_SECS` is
  `SECS` under the name those recipes use.
  `tools/capture_probes.sh` runs every probe that way and
  writes the combined `retail-captures.txt` the A/B test in
  `crates/gsc/tests/semantics_ab.rs` compares vcod's VM against. It passes no
  engine arguments, so those three sections are taken one at a time and
  pasted in at their sorted position. Both need
  the same setup `run_server.sh` documents; a full capture takes a couple of
  minutes because every probe boots the server. Read that directory's
  `README.md` before writing a new probe: six engine behaviours dictate its
  shape, and each costs a wasted run to rediscover.
- Live captures so far came from populated public servers (a TDM server on
  2026-08-24, an S&D server on 2026-08-25); a 60-100 s capture during a round
  is enough to see every combat event. The master at
  `codmaster.activision.com:20510` still answers `getservers 1 full empty`,
  which is the quick way to find a populated server of the right gametype.
- Every protocol divergence from RTCW/Q3 is listed in one section at the end of
  `docs/protocol-1.1.md`. Check it before assuming Q3 semantics for any field.
- `tools/re/net-notes.md` is the disassembly log for the Linux server binary
  (`objdump -d`, addresses virtual); `tools/re/dump_field_table.py` prints a
  netfield table from a VA. Known table addresses are in its docstring.
  `tools/re/dump_script_fields.py` and `tools/re/dump_builtins.py` dump the
  gsc script field tables, the `spawns` classnames and the five builtin
  tables out of `game.mp.i386.so`; the findings and record layouts are in
  `docs/research/cod11-gsc-object-model.md`. Both resolve `.rel.data`,
  which is mandatory: a pointer stored in `.data` reads as 0 in the file
  because the relocation supplies it, and reading the raw dwords makes every
  function pointer in every one of these tables look null.

## Reverse engineering the client

Binaries from the installs and what each is good for:

| File | Role |
|---|---|
| `main/cgame_mp_x86.dll` (1.1) | MP client game: event dispatch, effect/tracer/muzzle selection. The authority for vcod. |
| `main/game_mp_x86.dll` (1.1) | MP server game: who writes which state field |
| `CoDMP.exe` (1.1) | client-side netfield tables, sound system, efx renderer |
| `game.mp.i386.so` (1.1d Linux dedicated) | same as game_mp but with full symbols |
| `cgamex86.dll` (1.1) | single-player cgame. Wrong module: `EV_*` ids diverge from 173 up, exactly where impacts live. |
| `main/cgame_mp_x86.dll` (1.5) | for diffing; the `EV_*` table is byte-identical to 1.1 |

Image bases: `0x30000000` cgame DLLs, `0x20000000` game DLLs, `0x00400000` CoDMP.exe.
md5s of every module are in `docs/research/cod11-events-and-fx.md`.

Ghidra does the decompiling; keep projects and exports under `private/ghidra/`:

- `tools/re/ExportDecomp.java` dumps every function with `// --- name @ addr`
  markers into one `.c` file. Grep that before re-running Ghidra, a full
  decompile takes ~15 minutes. Pattern:
  `analyzeHeadless <projdir> <proj> -import <dll>` once, then
  `analyzeHeadless <projdir> <proj> -process <dll> -noanalysis -scriptPath tools/re -postScript ExportDecomp.java`.
- `tools/re/evtab.py <dll>` prints the `EV_*` pointer table in index order
  straight from the PE; `tools/re/netfields.py` finds `{name, offset, bits}`
  tables; `tools/re/xref.py <pe> <va>` finds raw immediates equal to a VA.
  These run in seconds and give the enumeration order with an address to cite.
- Find dispatch code by string: `CG_EntityEvent:%s` and `CG_EntityPreEvent:%s` in
  cgame, `fx/impacts/` for effect paths. Bullet-impact handling is in
  `CG_EntityPreEvent`, not `CG_EntityEvent`.
- Sound-system entry points in CoDMP.exe (alias csv loader, falloff, panning,
  channel pools, stream slots) are catalogued with their VAs in
  `docs/research/cod11-sound-system.md`; start there rather than
  grepping the decompilation.

Evidence discipline: every claim in a research doc names the module, the virtual
address, and the string or table it rests on, and carries its own label.
VERIFIED is what was read out of a binary, an asset or a live capture. INFERRED
is anything read off control flow, and **instruction sequencing and branch
conditions are control flow**: "followed by", "then", "when that field is
non-null" all belong under INFERRED however plainly the instructions read. A
label covers one claim, never a section, because a section is a mix and the
blanket then covers claims it should not. One exception, and only one: a
document may open with a document-level default ("everything here is VERIFIED
unless labelled otherwise") when its provenance really is uniform and every
exception in it carries its own label; the four format and handshake docs
that do are accurate. A blanket over a section, including one
appended to its heading, stays forbidden whether or not the document carries
such a default.
`crates/common/tests/evidence_labels.rs`
catches the two mechanical shapes of this and its doc comment says what it
cannot catch, which is most of it; a reader is still the enforcement. Research
docs carry facts derived from the binaries (offsets, tables, enum orders),
never pasted decompiler output or disassembly listings.

## Gotchas already paid for

- xanim translation keys are offsets from the bind pose; viewmodel rigs have all-zero
  binds, which is why absolute treatment ever looked right.
- Foliage `@`/`_` masked skins carry inverted alpha except `treeshdw_*`; the fix
  inverts DDS blocks in place (`assets.rs`).
- `entityState.weapon` is a 1-based index into configstring 7.
- Root `tag_origin` sits at the feet; the Q3 waist offset does not apply.
- Shader scripts for effects live in `fxshaders/` inside `pak5.pk3`, not `scripts/`;
  most of them are additive (`blendfunc GL_ONE GL_ONE`), most `scripts/` shaders are
  alpha. Some map paths carry a leading slash.
- Sound alias csv columns bind by header name, not by position:
  `dialog_generic.csv` shuffles them. A blank or `0` `dist_max` means
  `5 * dist_min`, not "no cutoff" as the csv legend claims.
- The alias-side surface suffix for asphalt is `asphault`. The engine spells it
  `asphalt`, so retail asks for a name no csv row has and asphalt is silent in
  game; vcod maps to the csv spelling on purpose and is audible there.
- `[profile.dev.package."*"] opt-level = 3` in `Cargo.toml` is for kira:
  symphonia and cpal crackle and take seconds per decode unoptimized. It costs
  one cold build and applies to wgpu/winit too.
- gsc folds case for identifiers, field names, file paths and event names
  (`intern_folded`) but not for string values or array keys (`intern_exact`);
  both halves are measured against retail. A misrouted event name fails
  silently: the `waittill` never sees its `notify` and the thread hangs with
  nothing logged. The two tests in `vm/sched.rs` are written to fail when
  either `fold_atom` call is removed; re-run that mutation if you touch the
  fold sites.
- `MAX_RELIABLE_COMMANDS` is 64 on CoD 1.1, not RTCW's 256: CoDExtended's
  `shared.h:135` and the `& 63` masks in `SV_UserMove` (cod_lnxded 0x8086fa4)
  agree. Both rings, and the scramble key that indexes them, are sized off it.
- The server's per-client drop notice is the reliable command `w "<reason>"`
  (`SV_DropClient` 0x8085cf4). Bare `disconnect` only travels client to server.
- 66 ms is a pmove chop, not a dt clamp. `Pmove` walks `ps.commandTime` up to
  the cmd's `serverTime` in steps of at most 66, each its own `PmoveSingle`,
  and drops only the arrears past 1000 ms, so a client that hitches for half a
  second gets the whole gap simulated. The `msec = min(msec, 200)` in
  `ClientThink_real` looks like the clamp and is not: it never reaches the
  mover. `docs/protocol-1.1.md`, "How long a cmd is simulated for".
- The retail client omits unchanged usercmd fields (change-bit 0, angles
  included), and a compact cmd carries no upper button, stance, `up` or
  weapon bits at all. What "unchanged" is relative to is not the previous
  cmd: `SV_UserMove` builds the base for each message's first cmd out of the
  client's playerstate (`docs/protocol-1.1.md`, "The base cmd is built from
  the playerstate"), whose sight bit is `fWeaponPosFrac != 0` and whose
  stance bits come from `eFlags`. So a client that chains from its own last
  sent cmd hands a released sight back to the server for as long as the
  fraction is non-zero; vcod's writer sends the full branch every cmd for
  that reason. vcod's server still decodes against its stored last received
  cmd, which agrees with retail's base in every case measured so far (the
  divergence list at the end of the protocol doc says where it would not),
  and decoding against `NULL_USERCMD` instead was the retail client's
  "spectator flash" (one-frame view snaps, invisible until yaw first went
  nonzero).
- A portal's plane faces out of its owning cell; the walk skips a portal when
  the eye is past it (`n·eye - dist > 1`). Two vcod additions to the walk:
  clipped portal polygons narrower than `SLIVER_EPS` are skipped (slivers at
  shared edges made cones flicker), and cells whose top is below the eye are
  marked with the camera frustum (the graph treats portal-less walls as full
  height, which fails once the eye looks over them). A third, from the same
  mp_ship deck that motivated the second: sightlines over low geometry
  (bulwarks, rails) still under-mark, so after the walk every cell sharing a
  portal with a visited cell is frustum-tested to a fixpoint (`visible`).
  The soup lump is laid out
  `[cull-group soups][cell-tree soups][submodel soups]`, and leaf surfaces
  (lump 23) index the terrain collision partitions, not draw soups.
- The F3 `audio` line reads `v N plays N miss N cull N drop N steal N N.NNms`: live
  voices, cues started, aliases not in the table, cues already past `dist_max`
  when they fired, cues refused because every pool slot was held by
  higher-priority voices (or kira's own cap hit), and voices evicted by the
  steal rule.
- A stock map load logs roughly 360-415 unique ShaderLib warnings, each once;
  the corpus is full of tokens vcod skips by design (hw-path stages, fog
  keywords), so the count is noise. The F3 `shader:` line shows it as
  `warns`; shader-script facts live in `docs/research/cod11-shader-scripts.md`.
- `--connect` opens the window before the gamestate arrives; the
  connecting/loading phases draw HUD text only.
- Movement constants come from retail rodata, not community lore: the table
  and the two deliberate divergences are in docs/research/cod11-mantle.md and
  bsp-ibsp59-format.md ("Movement constants"). Mantling does not exist in
  retail 1.1 MP; cod11-mantle.md is the negative result.
- A configstring range's first slot comes from its indexer, never from a
  doc's summary. The status icon, head icon and script menu indexers scan
  from `i = 0`; the localized-string and shader ones scan from `i = 1`.
  Reading one convention onto all five puts three ranges a slot high, which
  is what the table in `docs/research/clientstate-wire-format.md` used to do.
- Unary `!` takes a string where an `if` condition refuses every string:
  `!"0"` is `1` and `!"1"` is `0`, while `if ("a")` is a fatal
  `cannot cast "a" to bool`. That asymmetry is measured, and it is what lets
  `_teams::restrictPlacedWeapons` run on a stock server rather than killing
  it at map load (`docs/research/cod11-gsc-language.md` §9).
- A BSP entity key reaches script only if it is in the entity field table or
  in `radiant/keys.txt`; anything else is dropped at load, silently, exactly
  as retail drops it. So a script reading a Radiant key nobody registered
  gets `undefined` and no warning. The three-way split and both tables are
  in `docs/research/cod11-gsc-object-model.md`.
- The attack bit is pulsed, not held. Every stock rifle and pistol is
  semi-automatic, and a held bit fires one round and then nothing: the
  semi-auto latch pins `weaponTime` at 1 until the trigger is released
  (`cod11-combat.md` 1.4). Anything scripted that means to fire twice taps
  twice, with the tap at least `fireTime` apart.
- A capture cmd must carry the weapon the playerstate says the client holds.
  A `weapon` byte of 0 is not neutral: retail reads a `cmd.weapon` that
  differs from `ps.weapon` as a request to holster, so a probe sending 0 fakes
  a putaway at every input that forces the full usercmd branch (the reload
  key, a jump, a stance change). Three research paragraphs were written off
  that artifact before it was found.
- `eventSequence` is written *after* the event slot it counts, and the drain
  is `(prev..cur)`, not `(prev+1..=cur)`: retail writes `events[seq & 3]` and
  then increments, so the new slots are the ones below the sequence.
  `crates/common/src/net/events.rs` had the off-by-one and the client read
  every event one slot late.
- `health`, `ammo[]` and `ammoclip[]` are in the playerstate's array blocks,
  not among its scalar fields, and a retail client predicts its ammo counter
  from the clip it is sent: get the clip wrong and the HUD counts wrong on the
  client before any server frame disagrees. The block layout is in
  `docs/protocol-1.1.md`.
- The ammo and clip index tables start at **1**, not 0. The retail spawn
  loadout reads `clip=3:7,6:3,10:15 ammo=3:56,10:400`, which is colt at clip
  index 3, frag at 6 and carbine at 10; numbering from 0 puts every count one
  slot low and the client shows another weapon's ammo.
- The body queue is eight entities at 64..71 and has no lifetime timer. A
  corpse lives until its slot is reused, which the retail capture shows
  directly: the first corpse is still on the wire 190 s later.
- `eFlags` bit `0x8` is the teleport bit and it flips on every *spawn*, not
  every life: the connect's own `spawnSpectator`, the respawn's spectator
  frame and the intermission camera each consume a flip, and a level boundary
  clears the bit with the rest of the playerstate. A client breaks
  interpolation on the changed word, so leave it pinned and a retail client
  smears a respawning player from its corpse to its new spawn. The respawn
  clears the event ring with it (retail's first frame of a new life reads
  `eventSequence` 0). The spawn-for-spawn reading of the two committed
  captures is in `docs/research/cod11-map-cycle.md`, 8.2.
- A `clipOnly` weapon has no reserve at all. The frag's file reads
  `clipOnly 1` with `maxAmmo 3`, and retail's spawn line carries `clip=6:3`
  with no `ammo` entry for that index; writing the reserve anyway puts a
  count on the wire retail never sends.
- The four damage-feedback fields (`damageEvent`, `damageCount`, `damageYaw`,
  `damagePitch`) never clear. `damageEvent` increments and the other three
  hold the last hit's values until the next one, so a client cannot tell "no
  damage this frame" from "the same damage as last frame" by reading them; the
  increment is the edge.
- There is no cook in 1.1 MP. `grenadeTimeLeft` takes the held weapon's
  `fuseTime` at the pullback and 0 at the throw and nothing between: no
  countdown, no pin, no auto-throw, and the fuse from release to explode is
  the full `fuseTime` however long the trigger was held. The two committed
  grenade captures show no third value in 700-odd traces. A design that reads
  the field as a timer is reading RTCW's.
- A grenade's fuse rides the `EV_FIRE_WEAPON` / `EV_FIRE_WEAPON_LASTSHOT`
  parm, and only inside vcod: the pmove step clears `grenadeTimeLeft` on the
  same frame it raises the event, so the server would read 0 back if it went
  looking. `Attack::Throw` therefore has to be taken off the raised event
  during `replay_moves`, before the sim moves on. It must not travel:
  `eventParms[i]` is an 8-bit netfield and a 4000 ms fuse arrives as 160,
  where retail's throw frame reads `eventParms=0,0,0,0`, so `ClientSim::step`
  writes 0 to the ring for those two events and keeps the fuse in the returned
  `PmEvent`.
- The explode rides the missile's own entity, not a temp entity. It flips its
  `eType` to 0, sets `eFlags` 256 and writes `EV_GRENADE_EXPLODE` on its own
  ring, so anything filtering entities on `eType == 4` drops exactly the frame
  the explosion is on.
- Static props are in the server's collision world, not only the client's
  prediction world. Retail's grenade comes to rest 35 units up on a cart the
  bare BSP does not have, so `World::from_bsp` takes the paks. A prop's
  triangles arrive without their material, which is why a bounce off one
  carries `eventParm` 0 where retail carries the surface type.
- A stock frag bounces off a live player rather than detonating on it.
  `fraggrenade_mp` spells `damage` 0, and retail's direct-hit `MOD_GRENADE`
  arm is gated on that field, so the contact applies the soft damping and the
  fuse keeps running.
- `setPlayerIgnoreRadiusDamage` is a flag on `level`, not on a client. Only
  the `radiusDamage` builtin reads it, and it then skips every client for that
  one call; a grenade's own blast never consults it. One bool on the host is
  the whole of it.
- A weapon switch holds `cmd.weapon` at the new index until `ps.weapon` reads
  it. Retail's pickup half takes the byte off the cmd of the frame the putaway
  ends on, so a byte sent once is reverted before the swap lands and the old
  weapon stays in hand. A first retail capture of the one-cmd version measured
  the frag never arriving and the cook firing the rifle instead.
- A map change is pull-shaped. `SV_SpawnServer` sends no gamestate at all: the
  only thing that leaves the server is the out-of-band `loadingnewmap` line to
  every client at `CS_PRIMED` or above, and the gamestate goes out when that
  client's next message arrives still carrying the old serverId and
  `SV_ExecuteClientMessage` resends it. So a map change that pushes anything
  on the reliable stream is wrong by construction
  (`docs/research/cod11-map-cycle.md` 3.1).
- `sv_serverid` is two nibbles and the high one skips zero. A map load bumps
  the high nibble and keeps the low one, a restart bumps only the low one, and
  a high nibble that wraps to 0 is bumped again, so 16 goes to 17 across a
  restart and to 33 across a map change. `crates/server/src/console.rs` owns
  the arithmetic and both paths share it; the client reads the value back out
  of the systeminfo configstring, not out of the gamestate header.
- A probe has to re-answer the stock team menu after every gamestate and,
  under `dm`, after every restart. Retail reruns `ClientConnect` on both and
  reopens the menu under the indices the last one used, so `JoinProbe` clears
  them on every gamestate past the first and on every `n`. Under `sd`, whose
  `pers[]` survives, no menu reopens and the clear is inert.
- `game[]` survives a level boundary only when the caller passed `savePersist`,
  and an entity handle stored in it never survives at all. `map_restart(1)`
  and `exitLevel(1)` keep `game[]` and every client's `pers[]`, `map_restart(0)`
  and `exitLevel(0)` free both, `level` is always new, and every entity handle
  inside `game[]` is dropped whichever way the flag went. `dm.gsc` passes 0 and
  `sd.gsc` passes 1, which is the whole reason a `dm` client is put back
  through the team menu after a restart and an `sd` client is not.
- Time limits and score limits are script, never engine. Neither binary
  contains the string `timelimit`, `scorelimit` or `fraglimit`, and the game
  module exports no `CheckExitRules`: every "the round is over" decision in
  CoD 1.1 MP is a gametype script calling `exitLevel()` or `map_restart()`. A
  server that hard-codes either in Rust is adding a rule retail does not have.
  `g_intermissionDelay` is dead the same way: it is registered in
  `gameCvarTable` and has no other reference in the module, so nothing reads
  it, and `nextmap` is registered, set once inside the map load, and read by
  nothing: the Q3 convention of the gametype writing it and the engine
  executing it does not exist here, and `map_rotate` is the whole rotation.
- `map <the map already serving>` is a restart, not a spawn. The engine
  compares the requested name against the current one and takes
  `SV_MapRestart_f`, so the low nibble moves and no gamestate goes out. A
  rotation whose first entry names the map it is already on therefore restarts
  before it ever changes map, which is what the retail capture shows
  (`docs/research/cod11-map-cycle.md` 4.2).
- The player is a capsule, not a box. `ClientThink_real` hands pmove
  `trap_TraceCapsule` (the three trace slots at pm+0xe8/0xec/0xf0), and
  the retail captures sit at the point height on every grade where a box
  sits `15 tan` higher: one unit on a 4-degree street, which a predicting
  retail client corrected on every snapshot as a view twitch. Along a
  diagonal wall retail slides at 15 where a box's corner is 5 units inside.
  `collision.rs` sweeps every prim as Q3's capsule shape
  (`docs/research/cod11-mantle.md`, "The player is a capsule").
  `--save-slope` plus `crates/server/tests/playerstate_slope_ab.rs` is the
  measurement: retail's own cmd stream replayed on our mover from retail's
  own state at every snapshot.
- The wish speed is not `g_speed`. The walk cmd scale multiplies by the
  weapon's `moveSpeedScale` (1.18 on the carbine), by `walkSpeedScale` 0.4
  while the sight is held (`pm_flags` 0x80), by `backSpeedScale` 0.7 and
  `strafeSpeedScale` 0.8 on those axes and by `leanSpeedScale` 0.4 on a
  lean, so a sighted carbine walk is 89.7 and a plain run 224 (the mantle
  doc, "The wish speed"). The motion gate never compared velocity, which is
  how 190 flat survived for a month.
- A thread's own `notify` does not fire its own `endon`. A thread that
  `endon`s an event and then notifies that event itself survives and runs on;
  every *other* thread's `endon` on it still kills. Measured with
  `probe_endon_self`, and it is what lets `dm.gsc`'s `endMap` reach its
  `exitLevel` at all: ours used to kill the thread there and the map never
  ended.
- Client commands are flood-protected, and a bare `score` is not exempt. A
  non-exempt command opens an 800 ms window in which every further
  non-exempt one from an active client is dropped before the game sees it;
  the exemptions are the prefixes `team `, `score ` and `mr `, space
  included (`docs/protocol-1.1.md`, "Client commands are flood-protected").
  The round-restart target probe sends `score` every 2 s and `kill` every
  45 s, and retail silently refused every other `kill` for landing inside
  that window, which read as a sessionstate rule for a whole afternoon. A
  probe that pairs commands spaces them past the window, and ours drops
  them the same way now.
- A `map_restart` keeps the engine's configstring table and the item
  registry; only a map load clears them. `sd.gsc` precaches its weapons
  only while `game["gamestarted"]` is unset, so a restart that carries
  `game[]` registers none of them itself, and a table rebuilt from that
  run re-allocated the dropped weapon's model, configstring 8 and the
  elimination string at fresh slots on the first kill after it
  (`docs/research/cod11-map-cycle.md`, 4.6).
- The tick loop runs on an absolute schedule and catches up after an
  overrun, the way `SV_Frame` does. Sleeping the remainder of each tick let
  every sleep overshoot accumulate, and under load `serverTime` ran 5-10%
  slow against a probe's wall clock: a map-change capture that expected the
  rotation at 120 s ran out of its 150 s before it came.
