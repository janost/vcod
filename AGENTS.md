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
  bolt-action mosin on mp_pavlov is what exposed the rechamber and
  empty-reload states the carbine never reaches.
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
  spectator flight and `--test-entities` for scripted packet entities (no
  restarts yet). A snapshot's entity list is the map's own: placed weapons,
  script models and mounted MGs, culled per client against the BSP's PVS the
  way retail culls, so what a client is sent depends on where it stands. Other
  clients are in it too, each animated by the animscript machine
  (`crates/common/src/animscript.rs`, and
  `docs/research/player-model-anim-system.md` for what the retail captures
  measured): stance, direction, strafing, the jump and the landing all pick an
  index out of `mp/playeranim.script`. What the machine does not cover yet is
  the combat events (fire, reload, melee, pain, death), the two turn movetypes
  and the mounted-MG anims. What a client still gets nothing of is movers,
  missiles and corpses, which no code spawns.
- `tools/run_probe.sh <probe> [map]` drives the same retail binary as the
  gsc oracle: it drops one `crates/gsc/tests/fixtures/semantics/probe_*.gsc`
  in as a gametype script, boots the server, and prints the `PROBE` lines
  the script logged. `tools/capture_probes.sh` runs every probe that way and
  writes the combined `retail-captures.txt` the A/B test in
  `crates/gsc/tests/semantics_ab.rs` compares vcod's VM against. Both need
  the same setup `run_server.sh` documents; a full capture takes a couple of
  minutes because every probe boots the server. Read that directory's
  `README.md` before writing a new probe: three engine behaviours dictate its
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
  `shared.h:135` and the `& 63` masks in `SV_UserMove` (cod_lnxded 0x8087043)
  agree. Both rings, and the scramble key that indexes them, are sized off it.
- The server's per-client drop notice is the reliable command `w "<reason>"`
  (`SV_DropClient` 0x8085cf4). Bare `disconnect` only travels client to server.
- The retail client omits unchanged usercmd fields (change-bit 0, angles included, relative to its previous sent cmd); the server must decode each message's delta chain against its stored last received cmd, not `NULL_USERCMD`, or every omission reads as zero — this was the retail client's "spectator flash" (one-frame view snaps, invisible until yaw first went nonzero).
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
