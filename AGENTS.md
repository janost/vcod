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
  Nothing in `common` may import wgpu, winit or kira; `cargo tree -p vcod-common
  -i wgpu` proves it. `cargo build -p vcod` / `-p vcod-server` build one; plain
  `cargo build` both.
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
  tris c/d props p/q  X.XXms`.
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
  body `modelindex` change, and any moving map prop, so a "wrong model" report
  can be chased without the GUI. For audio it prints the ambient configstring
  3, the `CS_SOUNDS` (524+) alias block at gamestate and every later update
  inside it, each `EV_SOUND_ALIAS` with its resolved alias name and origin,
  every `loopSound` transition, and every server command the client does not
  consume (`s <idx>` carries the announcer alias). It also resolves every
  drained event into sound cues against the map's alias table and prints
  `audio: <n> cues, <n> alias misses` per second; weapon-file cues (fire,
  reload) are not resolved headlessly, since the probe loads no weapon files.
  At debug level it also dumps every non-empty configstring as `cs[i] = …`,
  which is how the retail configstring tables in the research docs were taken.
  `--probe-secs N` extends the default 65 s; a few minutes spans an SD round
  restart. Add `--save-fixture` or `--save-snapshots` only when the capture is
  meant to replace the committed evidence; the flag docs in
  `crates/client/src/main.rs` say why the two are separate.
- A map change is a re-sent gamestate on the live netchan; the net client
  applies it while `Active` and clears its snapshot ring. The client's per-map
  state lives in `App.world`, the renderer's `WorldGpu` and `Phase::Live`;
  `loading.rs` is the pure download/load state machine the redraw loop steps.
- Two local servers, don't confuse them. `tools/run_server.sh [map]` runs the
  **retail** 1.1d Linux dedicated binary (not in the repo; see the script
  header for where it goes and what it needs). It is the oracle for every wire
  question: when ours and retail disagree, retail is right, and the answer
  goes in a research doc with the bytes. It answers the handshake and
  gamestate but sends no snapshots to a lone spectator; snapshots need a
  player in the game. `cargo run -p vcod-server -- <map>` runs **ours**, still
  a skeleton: the handshake, the gamestate, client commands and moves, no
  snapshots, so a connected client sits at the loaded map until it times out.
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
address, and the string or table it rests on, and states which behaviour was
VERIFIED live versus inferred from decompilation. An unverified inference is
labelled as such so the next reader can test it. Research docs carry facts
derived from the binaries (offsets, tables, enum orders), never pasted
decompiler output or disassembly listings.

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
- `MAX_RELIABLE_COMMANDS` is 64 on CoD 1.1, not RTCW's 256: CoDExtended's
  `shared.h:135` and the `& 63` masks in `SV_UserMove` (cod_lnxded 0x8087043)
  agree. Both rings, and the scramble key that indexes them, are sized off it.
- The server's per-client drop notice is the reliable command `w "<reason>"`
  (`SV_DropClient` 0x8085cf4). Bare `disconnect` only travels client to server.
- A portal's plane faces out of its owning cell; the walk skips a portal when
  the eye is past it (`n·eye - dist > 1`). Two vcod additions to the walk:
  clipped portal polygons narrower than `SLIVER_EPS` are skipped (slivers at
  shared edges made cones flicker), and cells whose top is below the eye are
  marked with the camera frustum (the graph treats portal-less walls as full
  height, which fails once the eye looks over them). The soup lump is laid out
  `[cull-group soups][cell-tree soups][submodel soups]`, and leaf surfaces
  (lump 23) index the terrain collision partitions, not draw soups.
- The F3 `audio` line reads `v N plays N miss N cull N drop N steal N N.NNms`: live
  voices, cues started, aliases not in the table, cues already past `dist_max`
  when they fired, cues refused because every pool slot was held by
  higher-priority voices (or kira's own cap hit), and voices evicted by the
  steal rule.
- `--connect` opens the window before the gamestate arrives; the
  connecting/loading phases draw HUD text only.
- Movement constants come from retail rodata, not community lore: the table
  and the two deliberate divergences are in docs/research/cod11-mantle.md and
  bsp-ibsp59-format.md ("Movement constants"). Mantling does not exist in
  retail 1.1 MP; cod11-mantle.md is the negative result.
