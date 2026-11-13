# Testing: suites, ignored gates, soaks, perf

AGENTS.md has the rules every commit follows. This file has the per-install
gates, the ignored tests, and the census, soak, perf and bot harness recipes.
The A/B gates against retail captures are in [retail.md](retail.md).

## Installs and which tests they run

| Env | Install | Runs |
|---|---|---|
| `COD_DIR` | CoD1 1.1 | the 1.1 tests and gates |
| `COD_DIR` | CoD1 1.5 (`main/` with stock paks only) | `configstrings_15_ab`, `timeout_15_ab`, `legs_ab`'s 1.5 walks (the stop and go too), `turret_ab`'s 1.5 replay |
| `COD_DIR` | UO (`uo/` beside 1.5's `main/`) | the UO tests and what reads only data 1.5 and UO share: `moves_ab`'s UO replays (sprint, mounts, cook), `configstrings_uo_ab`, `playerstate_uo_ab`, `playerstate_combat_ab`'s UO capture, `mount_uo_ab`, `uo_weapons_ab`, `legs_ab`'s UO walks, `turret_ab`'s UO replay |
| `COD2_DIR` | CoD2 1.3 | the CoD2 tests, `cod2_map_census` |
| `COD4_DIR` | CoD4 1.7 | the CoD4 tests, `cod4_live_*`, `cod4_map_sweep`, `cod4_soak` |

`COD_DIR` must be absolute: a relative path resolves in the crate's directory
and every pak test passes vacuously.

Pick the fixture helper by what the test asserts:

- Reads paks at all: `vcod_common::testing::game_fs()`. `None` without
  `COD_DIR`, so the test passes vacuously in CI.
- Asserts 1.1's assets or replays a 1.1 capture: `testing::game_fs_11()`,
  `None` at a 1.5 or UO install.
- A UO test: `testing::uo_fs()`.
- Asserts what the stock paks hold (menus, game types, a census):
  `testing::stock_fs()` / `stock_fs_11()`, the `--vanilla` search path, so a
  third-party pak in `main/` cannot fail it.
- A CoD2 test: `testing::cod2_fs()`, `$COD2_DIR/main/`'s stock iwds alone
  (`iw_00`..`iw_24`, `localized_*_iwNN`). A map pack there (a `mp_*.iwd`,
  a `~~~.iwd` sorted last) overrides stock gametype and map scripts and
  joins `sv_iwdNames`, which the wire A/Bs compare with retail's.

The all-maps census tests scope to stock `pak[0-9].pk3`. Map downloads drop
third-party `zzz_*.pk3` into `main/`; a census failure on one of those is not
a regression.

`Server::new` seeds its RNG with a constant; only `main.rs` passes the clock
through `Server::with_seed`. Tests that depend on spawn picks rely on that.
Each load still steps a fresh script seed, as retail's `G_InitGame` does
(`cod11-map-cycle.md` 4.8), so a later level never repeats the first's draws.
A gate that compares a later level's spawn with retail's replays retail's
picks through `Server::pin_spawns`, as `gametypes_ab` does.

## Ignored tests: when to run them

| Touching | Run | Cost |
|---|---|---|
| pmove, collision or nav | `cargo test --release -p vcod-server --test server -- --ignored nav_census::` | ~8 s, 65 s CPU at 1.1; 20-60 s, 340 s CPU at UO |
| bots | `cargo test --release -p vcod-server --test server -- --ignored bots::` | ~5 s at 1.1; ~7 min at UO |
| UO bot vehicles | `-- --ignored bot_vehicles::` at a UO install | ~30 s |
| shaders or render layouts | `renderer::pipeline_check` (lavapipe env below) | |
| CoD2 maps, CoD2 load path | `cod2_map_census`, `cod2_load_profile` | |
| CoD4 maps, gametypes | `cod4_map_sweep` | 6-8 min and ~3 GB a map |

What each holds:

- `nav_census`: all stock nav graphs. At UO, the 19-map UO census.
- `server/tests/bots.rs` at 1.1: three mp_ship six-bot runs, mp_ship's boat,
  mp_ship's head-on ladder pair, mp_ship's hold ladder top, mp_hurtgen's
  crease and mp_depot's roof ledges (a graph build each).
- `bots.rs` at UO: mp_cassino's six-bot tdm run, its walled-in spawns test
  and four mp_foy `bas` rounds
  (`uo_bots_breach_bases_until_a_bas_round_ends`, the two `four_*_bas_round`
  runs, `a_seeded_bas_match_replays_beside_another`).
- `tests/bot_vehicles.rs` at UO: bots boarding, driving and firing
  mp_cassino's tanks, rockets at one, and mp_kharkov's jeep. With a 1.1
  install it returns at once, as the UO files of the other two do.

## Headless render: pipeline_check

`renderer::pipeline_check` builds the real `Renderer` headless and draws a
CoD1 and a CoD2 frame under wgpu validation. It needs a Vulkan (not GL)
adapter with BC textures and otherwise skips. On lavapipe:
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
- About 100 of them take about 5 min of wall at 4 test threads and peak near
  10 GB.
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
writes a `soak.md`.

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
    `TEAMPLAY=`, `AWARE=`, `FOOTWORK=`, `WARY=`, `SQUAD=`, `AIM=` arms
    (`dm` splits them by slot parity).
  - `GAME=cod2` runs `vcod-server --game cod2`'s bots, split by weapon class
    and skill; `GUNS=0` keeps them off the mounted guns (docs/bots.md
    "CoD2"). `GAME=cod4` runs CoD4's, with each level's line and every script
    error the levels raised (docs/bots.md "CoD4").
  - `DIGEST=1` or `DIGEST=<N>` adds a per-tick hash of every bot's cmd and
    body, the check that a refactor moved no bot. `NETDIGEST=<N>` hashes
    every datagram sent, with an observer client joined.
    `VCOD_TICK_TRACE=<dir>` writes the tick's step log (bot-combat.md
    section 1). `SPIKE_MS` logs slow ticks.

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
