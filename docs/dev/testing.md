# Testing: suites, ignored gates, soaks, perf

AGENTS.md has the rules every commit follows. This file has the per-install
gates, the ignored tests, and the census, soak, perf and bot harness recipes.
The A/B gates against retail captures are in [retail.md](retail.md).

## Installs and which tests they run

| Env | Install | Runs |
|---|---|---|
| `COD_DIR` | CoD1 1.1 | the 1.1 tests and gates |
| `COD_DIR` | CoD1 1.5 (`main/` with stock paks only: `--example stock_tree`, below) | `configstrings_15_ab`, `timeout_15_ab`, `legs_ab`'s 1.5 walks (the stop and go too), `turret_ab`'s 1.5 replay |
| `COD_DIR` | UO (`uo/` beside 1.5's `main/`) | the UO tests and what reads only data 1.5 and UO share: `moves_ab`'s UO replays (sprint, mounts, cook), `configstrings_uo_ab`, `playerstate_uo_ab`, `playerstate_combat_ab`'s UO capture, `mount_uo_ab`, `uo_weapons_ab`, `legs_ab`'s UO walks, `turret_ab`'s UO replay |
| `COD2_DIR` | CoD2 1.3 | the CoD2 tests, `cod2_map_census` |
| `COD4_DIR` | CoD4 1.7 | the CoD4 tests, `cod4_live_*`, `cod4_map_sweep`, `cod4_soak` |

`cargo run -p vcod-common --example stock_tree -- <install> <out>` links
the stock-only tree the 1.5 and UO rows want: the paks `--vanilla` keeps,
under `<out>/main/` and `<out>/uo/`.

The helpers in `vcod_common::testing` (and `vcod_cod4::testing`) read
`COD_DIR`, `COD2_DIR`, `COD4_DIR` and `COD4_LNXDED_DIR`. Both modules,
`vcod_common::collision`'s test worlds and vcod-server's `test_*` and
`*_for_test` seams sit behind each crate's `testing` feature, which every
crate that calls them turns on from its dev-dependencies, so release
builds leave them out:

- Unset or empty: `None`, the test returns early and passes. Each test
  binary prints one `vcod testing: COD_DIR is unset, ...` line per reason
  on stderr, past libtest's capture, so a run shows what it skipped. The
  version pickers print one too (`COD_DIR is not a 1.1 install`, `COD_DIR
  has no uo/`).
- Set to a relative path (it would resolve in the crate's directory), a
  path without `main/` (`zone/` for CoD4), or a search path that opens no
  paks: the helper panics with the variable and the path.
- `VCOD_REQUIRE_INSTALL=<names>` makes a skip panic for each install it
  names: `cod1` (`COD_DIR` set), `cod11` (a 1.1 install there), `cod15`
  (a 1.5 `main/`, a UO install's included), `uo` (`uo/` beside `main/`),
  `cod2`, `cod4`, `cod4_lnxded`. A gate run sets
  the names of the installs it means to test, so a wrong env cannot pass
  with no pak test run: `cod1,cod11` at 1.1, `cod1,uo` at UO, plus `cod2`
  and `cod4` when those are set. An unknown name panics.

Pick the fixture helper by what the test asserts:

- Reads paks at all: `vcod_common::testing::game_fs()`. `None` without
  `COD_DIR`, so the test passes vacuously in CI.
- Asserts 1.1's assets or replays a 1.1 capture: `testing::game_fs_11()`,
  `None` at a 1.5 or UO install.
- Replays a 1.5b capture: `testing::game_fs_15()`, the install's `main/`
  when it is 1.5's (a UO install's is), `None` at 1.1.
- A UO test: `testing::uo_fs()`.
- Asserts what the stock paks hold (menus, game types, a census):
  `testing::stock_fs()` / `stock_fs_11()`, the `--vanilla` search path, so a
  third-party pak in `main/` cannot fail it.
- A CoD2 test: `testing::cod2_fs()`, `$COD2_DIR/main/`'s stock iwds alone
  (`iw_00`..`iw_24`, `localized_*_iwNN`). A map pack there (a `mp_*.iwd`,
  a `~~~.iwd` sorted last) overrides stock gametype and map scripts and
  joins `sv_iwdNames`, which the wire A/Bs compare with retail's.
- A CoD4 test: `vcod_cod4::testing::cod4_install()` (`COD4_DIR`), or
  `cod4_lnxded_install()` (`COD4_LNXDED_DIR`, the retail dedicated
  server's own `zone/english/`, which `zone_census` reads).

The all-maps census tests scope to stock `pak[0-9].pk3`. Map downloads drop
third-party `zzz_*.pk3` into `main/`; a census failure on one of those is not
a regression.

The CoD4 censuses in `vcod-cod4`'s `tests/` walk every stock map zone,
each its own way, through `vcod_cod4::testing::stock_maps` (21 zones;
`COD4_MAPS=<a,b>` narrows them). They stay unignored: the slowest,
`clip_census`, took 13.5 s alone in the test profile, and the whole
`--test cod4` binary 24 s at 4 threads (2026-11-18). Between 4 and 9 s
each: `part_edges`' cylinder drop, `mapworld_census`, `map_census`,
`look_census`, `drawsurf_census` and two of `sunshadow_census`'s.

`Server::new` seeds its RNG with a constant; only `main.rs` passes the clock
through `Server::with_seed`. Tests that depend on spawn picks rely on that.
Each load still steps a fresh script seed, as retail's `G_InitGame` does
(`cod11-map-cycle.md` 4.8), so a later level never repeats the first's draws.
A gate that compares a later level's spawn with retail's replays retail's
picks through `Server::pin_spawns`, as `gametypes_ab` does.

## CI and the test binaries

- `ci.yml` runs on ubuntu with no game data, so no pak test and no A/B gate
  runs there; they run only on a machine with an install. It pins the
  toolchain (`RUST_TOOLCHAIN`, bumped on purpose), passes `--locked`, and
  also checks the workspace on Windows, macOS and the 1.95 MSRV, and runs
  the suite on Windows and macOS. A second Linux step runs the client's
  render tests on lavapipe with `VCOD_REQUIRE_GPU=1`: a headless test takes
  its adapter through `renderer::headless::test_adapter`, so one that finds
  none fails there. Lavapipe is installed in that step only: with it present
  the main suite drew on it too, and the client test binary died once on
  SIGSEGV. The Windows/macOS suite and the lavapipe step are
  `continue-on-error`, the lavapipe step until that crash is understood.
- `nightly.yml` rebuilds the rolling `nightly` release on master pushes that
  touch code (docs, tools and `*.md` are path-ignored). `workflow_dispatch`
  forces one. It runs `ci.yml` first and publishes only a green tip, fails
  the Linux build if a strong glibc symbol passes README's floor, and ships
  `THIRD-PARTY-LICENSES` (`tools/third_party_licenses.py`) and `SHA256SUMS`.
- Each crate's integration tests are one binary named after the crate
  (`--test common`, `--test cod2`, `--test cod4`, `--test gsc`, `--test
  server`; `tests/main.rs`). A file's tests filter by its name (`--
  vehicle_ab::`). The client's tests live in its binary: `cargo test -p
  vcod --bins`.

## The gate runner

`tools/gate.sh [area...]` runs fmt, clippy and the suite at each install
the environment names: `COD_DIR` (1.1) with `COD2_DIR` and `COD4_DIR`,
then the server suite again at `COD_UO_DIR` (a stock-only UO tree) with
`COD4_DIR` unset. It sets `VCOD_REQUIRE_INSTALL` to match, so no install
test can skip. Each area adds the ignored gates of one row group below:
`nav`, `bots`, `render` (`LVP_ICD` points it at lavapipe), `cod2`, `cod4`.
A step passes only on `test result: ok` lines that ran tests. `CARGO_WRAP`
puts each cargo run under a wrapper (a memory-capped scope), `DRY=1`
prints the commands.

## Ignored tests: when to run them

`--ignored` always takes a name filter. Unfiltered it also starts the soaks
and the map sweep (minutes and gigabytes a map) and the two live browser
tests, which ask cod.pm's public master and every server it lists, and
broadcast on the LAN.

| Touching | Run | Cost |
|---|---|---|
| pmove, collision or nav | `cargo test --release -p vcod-server --test server -- --ignored nav_census::` | ~20 s at 1.1; ~80 s at UO |
| bots | `cargo test --release -p vcod-server --test server -- --ignored bots::` | ~10 s at 1.1; ~8 min at UO |
| the bots' driver or its threads | `cargo test --release -p vcod-server --test server -- bot_threads::` (not ignored; each install it finds; `tdm` and `sd`, UO `ctf`) | ~15 s a game |
| UO bot vehicles | `-- --ignored bot_vehicles::` at a UO install | ~12 s |
| CoD4 bots | `cargo test --release -p vcod-server --lib -- --ignored cod4::bots::` (`COD4_DIR`) | ~4 s |
| CoD4 hit boxes per map | `--lib -- --ignored cod4_every_map_poses_both_teams` (`COD4_DIR`) | 29 s of test, 7 min with the release build (2026-11-18) |
| CoD2 gametypes | `--lib -- --ignored cod2_gametype_census` (`COD2_DIR`) | ~18 s |
| 1.5 stuck seed | `--lib -- --ignored the_15_overlap_spectator_seed_is_the_only_fit` | ~2 s |
| shaders or render layouts | `cargo test -p vcod --bins -- renderer::pipeline_check` (not ignored: it skips without a Vulkan adapter; lavapipe env below) | |
| CoD2 maps, CoD2 load path | `cod2_map_census`, `cod2_load_profile` | |
| CoD4 maps, gametypes | `cod4_map_sweep` | 6-8 min and ~3 GB a map |

`nav_census` builds every stock nav graph (at UO, the 19-map UO census).
`server/tests/bots.rs` and `bot_vehicles.rs` hold one map scenario a test,
a graph build each; their doc comments name the map and the gametype, and
the UO ones return at once at a 1.1 install. In the `bas` rounds
`BAS_HEALTH=24500` plays stock base health, `BAS_MAP=all` the five stock
bas maps, `BOTS_SEED=` another seed.

Measurements and by-hand tools, not gates. Each prints what it measured;
its doc comment says what it reads and which env vars drive it.

| Test | Area | Doc |
|---|---|---|
| `the_mover_replays_the_fixture_named_by_slope_fixture` | 1.1 slope replays (panics without `SLOPE_FIXTURE`) | `playerstate_slope_ab.rs` |
| `recover_game_seed`, `recover_spread_seed`, `recover_rocket_seed` | CoD4 and CoD2 seed searches | their doc comments |
| `leaf_order_census` | CoD1 leaf walk order (`LEAF_MAPS`, `LEAF_SAMPLES`) | bsp-ibsp59-format.md |
| `stock_sun_keep`, `regional_sun_keep` | CoD1 sun keep | cod11-light-grid-and-leaf-lights.md 12 |
| `cod2_cull_profile`, `cod2_cell_walks_draw_alike` (lavapipe), `cell_walks_side_by_side` (`vcod-cod2`) | CoD2 cell walk (`VCOD_PROFILE_MAPS`) | cod2-client-perf.md 8, 9 |
| `a_prone_view_flick_costs` | CoD2 prone view cost | cod2-movement.md 10 |
| `lit_pipeline_register_census` | CoD4 shader registers on a driver | cod4-client-perf.md 29 |
| `spawn_wave` | the gsc interpreter | "The gsc interpreter" below |

Never run by an agent: `master_list_live` (cod.pm's public master and every
server it lists) and `lan_scan_live` (a LAN broadcast). Both panic unless
`VCOD_LIVE_NET=1` is set as well.

Times are release wall on a 30-thread box, the binary built.

The bit-exact gates (`moves_ab`, `cod4_moves_ab`, `cod4_combat_ab`,
`trace_ab`, `nav_census`) were tuned on the host's glibc. Only `sinf` and
`cosf` are pinned (`vcod_common::libm`, cod4-pmove.md 13.2); `atan2`,
`tan`, `acos`, `pow` and the rest come from the platform's libm, whose
results Rust does not promise across versions. A gate that fails after a
glibc update or on another distro, with no code change, is that before it
is a regression: name `ldd --version` in the report.

## Headless render: pipeline_check

`renderer::pipeline_check` builds the real `Renderer` headless and draws a
CoD1, a CoD2 and a CoD4 frame under wgpu validation. Its gates are plain
`#[test]`s, not ignored: they need a Vulkan (not GL) adapter with BC
textures, and without one each prints `skipped` and passes, as every
headless render test does; `VCOD_REQUIRE_GPU=1` fails them instead, as
CI sets it on lavapipe. On lavapipe:
`VK_DRIVER_FILES=<lvp_icd.json> WGPU_BACKEND=vulkan GALLIVM_PERF=nopt`. Run it
after any shader or layout change. A render test that passes headless can
still draw wrong live; a change to a draw path also needs a test through the
session or replay path.

## CoD2 map census

`COD2_DIR=... cargo test -p vcod --bins -- --ignored cod2_map_census` runs
every stock CoD2 map through vcod-server, a spectating client, a headless
frame (lavapipe env and `nice` as for `pipeline_check`;
`VCOD_CENSUS_FRAMES=0` skips it) and a walk from each spawn. It writes its
table, frames and per-map warnings to `tmp/cod2-map-census/`
(`play/cod2_census.rs` header).

## CoD4 map sweep

`COD4_DIR=... cargo test -p vcod --bins -- --ignored cod4_map_sweep
--nocapture` runs every stock CoD4 map under war, dm, sd, sab, dom and koth.
Per map: a vcod-server with six shooting bots rotating through the six
gametypes (30 s limits) and one client playing it through `Cod4Client::pump`
into a headless renderer (lavapipe env as for `pipeline_check`).

- Output: `tmp/cod4-map-sweep/summary.md`, the per-pair table (load, end,
  rotate, client spawn, script errors, bot stalls, client and server
  warnings, panics) and the ranked errors.
- Run one map a process (`VCOD_SWEEP_MAPS=<map>`, `VCOD_SWEEP_JOBS=1`) under
  `taskset -c 0-7` with `LP_NUM_THREADS=1` (`play/cod4_sweep.rs` header).
- Results: `cod4-map-sweep.md`.

## CoD4 live tests

The `cod4_live_*` tests (`COD4_DIR`) each run a CoD4 vcod-server and client
in process, on `play::cod4`'s `tests::sim` clock.

- A test's loops count frames its own sleeps move. Under load it runs slower,
  never out of time; only `sim::Clock::sleep`'s 60 s wall assert stops a
  stalled one.
- A test takes one of `play::cod4::tests::live_slot`'s slots before it
  builds its server, three unless `VCOD_LIVE_SLOTS` says more, so a run at
  any `--test-threads` holds at most three maps at about 2.5 GB each. The
  107 of them ran in 90 s at 8 test threads under heavy.sh's 14 GB cap.
- Run the CoD4 client tests once. Two such test binaries side by side under
  one memory cap (a 1.1 and a UO pass of the same suite, both with
  `COD4_DIR` set) swap and crawl for an hour instead of failing. Leave
  `COD4_DIR` unset for a CoD1 or CoD2 pass.
- Only the client's sleeps move the clock, never the wall. A client that
  computes, loads or blocks still runs no server frame, and every serve loop
  builds the bots' graph inline. A helper thread on the wall (`live_www`'s
  HTTP server) holds the server's frames while it works, or the client's
  waits run the level minutes ahead of it.
- A live test runs its server through `tests::run_held`, never on a thread
  of its own pacing frames by `Instant`: under load such a client's
  deadlines run out before it spawns. `play/cod4_sweep.rs`'s `rig` and
  `answer_menu` wait on the thread's `sim` clock, the wall off one (the
  sweep and the soak).
- A live test that writes frames (`VCOD_C4_SHOTS`) runs through
  `tests::run_held`, each frame loop sleeping its 16 ms first. The client
  reads the session's clock through `play::cod4::clock_now`, never
  `Instant`. Three runs of one build then write the same bytes for every
  frame (cod4-client-perf.md 39).

## Soaks

Each soak plays every stock map for minutes through the real client frame and
writes a `soak.md`. The soak, perf and off-LAN scripts first check the host
tools and the built binaries they run (`tools/lib.sh`) and exit 2 naming
the one missing.

### CoD4

`tools/cod4_soak.sh <outdir> [map...]` (`COD4_DIR`, the lavapipe env; `WRAP`
names a wrapper command, default `nice -n 10`) runs `cod4_soak` in
`play/cod4_sweep.rs`.

- One map a process: war, dm, sd, koth and sab at 3 min limits.
- The client plays as a player through `main`'s whole CoD4 frame, render and
  HUD included: it walks nav paths, fires, throws, dies, respawns, opens the
  board and the Esc menu.
- `soak.md` adds the player's counts, wgpu errors, problem debug lines, audio
  misses, client frames and server ticks past 100 ms, and the resident set
  per level (a `soak rss:` line at each load). Under `LD_PRELOAD` jemalloc
  with `MALLOC_CONF=prof:true` it also dumps a heap profile there
  (cod4-client-perf.md 23).
- About 20 minutes and 3.5 GB a map. Method and results: `cod4-map-sweep.md`,
  "The soak".

### CoD1 and UO

`tools/cod1_soak.sh <outdir> [map...]` (`COD_DIR`, `UO=1`, `LVP_ICD`,
`MINUTES`, `GAMETYPES`).

- Per map: a vcod-server with six shooting bots rotating through every
  gametype the map's arena lists, and the real `vcod` on a nested
  `kwin_wayland --virtual` joined as axis and played by `VCOD_SOAK`
  (`play::cod1_soak`) through `main`'s whole frame.
- `tools/cod1_soak.py <outdir>` writes `soak.md`: stalls by part and action,
  first in-game frame, warnings, script errors, panics, server ticks,
  resident set per level.
- About 6 minutes a 1.1 map and 11 a UO map at `MINUTES=1` (`UO=1 JOBS=2`:
  all nineteen in 105 minutes).
- Method and results: `cod11-client-soak.md`.

### CoD2

`tools/cod2_soak.sh <outdir> [map...]` (`COD2_DIR`, the same env;
`cod4_soak.sh` with `GAME=cod2`) runs `play::cod2_soak` on the same harness.

- Per map: vcod-server `--game cod2` with six shooting bots rotates through
  dm, tdm, sd, ctf and hq (`VCOD_SOAK_MINUTES`), and the client plays through
  `main`'s `Mode::Cod2` frame.
- Same `soak.md` columns, plus the frames past a level's first 5 s that
  built a mark material or loaded an fx image.
- Each map load logs a `soak rss:` line (resident set and malloc's live
  bytes). A repeated `VCOD_SWEEP_GAMETYPES` list runs a map round more than
  once.
- About 10 minutes and 2 GB a map at 2 min limits. Method and results:
  `cod2-client-perf.md`, "The soak".

## Performance

- `VCOD_FRAME_PROFILE=<path>` on `vcod` writes each frame's CPU ms by stage
  (net, bodies, fx, hud, cull, encode...), its counts and its GPU ms per pass
  group (`frame_prof`).
- `tools/cod4_perf.sh <outdir> <map> <secs>` runs a CoD4 server with 12
  shooting bots and a spectating client under it in a nested kwin, and
  prints the summary.
  - `SESSION=1`: a window in the current session, on a real GPU.
  - `GAMETYPE=war` holds the view still for a GPU A/B. `TURN=<deg/s>` turns
    it on its own for a moving view's CPU.
  - `GAME=cod2` with `COD2_DIR` runs CoD2's, whose free spectator holds still
    on any gametype. `GAME=cod1` or `GAME=uo` with `COD_DIR` runs a CoD1 or
    UO server, whose spectator stands still.
  - `SIZE=WxH` sizes the client's window (`vcod --window`) in both modes.
    `SESSION=1` without it keeps the game's saved video mode.
  - `SWEEP=1 SESSION=1 tools/cod4_perf.sh <outdir> mp_crossfire 10` holds one
    view in one session and toggles one setting a pass (MSAA, prepass, sun
    shadow, feather, distortion, `fx_draw`, specular, normal maps, fog, glow,
    dof, film), printing a row a pass with the finer `gpu.<group>` stamps
    (world, smodel, ents, gun, fx, each post pass). It restores the
    install's `vcod_mp.cfg` files at the end. `cod4-client-perf.md` 25 has
    the command and each column.
- Per-game numbers: `cod4-client-perf.md` (the per-map table),
  `cod2-client-perf.md`, `cod11-client-perf.md`.
- `cod2_load_profile` (ignored, in `renderer/pipeline_check.rs`, run on
  lavapipe as `pipeline_check` is, in release) loads three CoD2 maps twice
  through the renderer, the prediction's collision and the first draw, and
  prints a digest of every texture uploaded and the paks the load marked. A
  load made faster keeps both.

## Bots

Bot play is measured, not eyeballed. Retail has no bots, so these are the
only yardstick.

- `cargo run --release -p vcod-server --example bot_match [seed...]` runs
  seeded bot-vs-bot matches. It is the one bot measuring tool: extend it
  rather than add another example. Knobs:
  - `MAPS=`, `GAMETYPE=`, `BOTS=`, `TICKS=`, `JOBS=`, `SET=`, `UO=1`.
  - Per side `AXIS_SKILL=`/`ALLIES_SKILL=`; one-sided `COVER=`, `RETREAT=`,
    `TEAMPLAY=`, `AWARE=`, `FOOTWORK=`, `WARY=`, `SQUAD=`, `AIM=`, `TACTICS=`,
    `CONVERGE=`, `LANES=`, `FAR=`, `CUES=`, `TRAVEL=`, `CARRY=` arms
    (`dm` splits them by slot parity), and `ESCORT=`, `ROUTE=`, `HOLD=`,
    `SPRINT=`, `WATCH=`, `CLOSE=`, `NEAROUT=`, which turn on rules that
    are off by default (bot-objectives.md 14.12, 14.13). On `ctf` it counts the carries and how each
    ends (`ctf_carry_*`), and `CTF_TRACE=1` prints each carry's end.
  - `GAME=cod2` runs `vcod-server --game cod2`'s bots, split by weapon class
    and skill; `GUNS=0` keeps them off the mounted guns (docs/bots.md
    "CoD2"). `GAME=cod4` runs CoD4's, with each level's line and every script
    error the levels raised (docs/bots.md "CoD4").
  - `DIGEST=1` or `DIGEST=<N>` adds a per-tick hash of every bot's cmd and
    body, the check that a refactor moved no bot. `NETDIGEST=<N>` hashes
    every datagram sent, with an observer client joined.
    `VCOD_TICK_TRACE=<dir>` writes the tick's step log (bot-combat.md
    section 1). `SPIKE_MS` logs slow ticks (every game), with the ticking
    thread's instructions where the kernel lets a process count them, the
    number to compare on a loaded machine (cod4-server-frame.md 9.1); on
    CoD1 and CoD2 `RESTART=<n>` runs `map_restart` every `n` ticks, a spawn wave
    each (cod4-server-frame.md 9). `TICKSTATS=1` (with `JOBS=1`) prints each
    phase's mean, p50, p99 and max a tick; `TICKSTATS=insns` counts each
    phase's user-space instructions instead, the number to compare on a
    loaded machine. `VCOD_BOT_THREADS=1` runs the
    bots' pass serial: its digests match the threaded run's.

  It prints kills, deaths, damage, time-to-kill, wins, objective actions,
  lives, wounds survived, retreats, cover holds, peeks and hunts and the
  deaths after them, hits answered, deaths blindsided, stalls (split from
  deliberate holds), shots, hits, accuracy, sight to first shot and first
  hit, and fights and how they end, re-engagements included, per team,
  skill and arm.
  `bot-combat.md` section 1 says what each number means.
- A bot behaviour change reports its before and after on the same seeds in
  `docs/research/bot-combat.md`.
- `cargo run --release -p vcod-server --example ship_stalls` is the stall
  detector behind `bot-navigation.md`'s tables (`UO=1` for a UO server).
- `--example nav_build` makes the build-time table (`UO=1`, `RULES=`,
  `GAME=cod4` for CoD4's graph over its clip map). Its `digest` matches
  across two builds only when they built the same graph: the check for a
  collision change meant to move no trace.
- `--example bot_vehicles` (UO) plays each seed with bots' vehicles off and
  on (`bot-objectives.md` 10.6).
- `bot-navigation.md`'s Build times set the census floors.

## The gsc interpreter

`cargo test --release -p vcod-gsc --test gsc bench:: -- --ignored
--nocapture` runs `tests/bench.rs`: CoD2's `getSpawnpoint_DM` loops for
60 spawns over a table host, best of ten, and the result it must keep. A
change to the interpreter, the heap or the scheduler reports it before and
after (cod4-server-frame.md 9, "The spawn wave").
