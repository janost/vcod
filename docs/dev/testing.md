# Testing: suites, ignored gates, soaks, perf

AGENTS.md has the rules every commit follows. This file has the host
setup, the per-install gates, the ignored tests, and the census, soak, perf
and bot harness recipes.
The A/B gates against retail captures are in [retail.md](retail.md).

## Host setup

The build needs ALSA's headers on Linux (`alsa-sys` links it in its build
script) and `pkg-config` to find them:

| Distro family | Build | Lavapipe (headless render tests) |
|---|---|---|
| Debian, Ubuntu | `libasound2-dev pkg-config` | `mesa-vulkan-drivers` |
| Fedora | `alsa-lib-devel pkgconf-pkg-config` | `mesa-vulkan-drivers` |
| Arch | `alsa-lib pkgconf` | `vulkan-swrast` |

Lavapipe's ICD json is the `lvp_icd.*.json` the package puts in the Vulkan
loader's `icd.d/` directory (`/usr/share/vulkan/icd.d/` on these three).
Every script that draws takes its path as `LVP_ICD`; `VCOD_REQUIRE_GPU=1`
then makes a render test that finds no adapter fail instead of skip. CoD2
voice needs the system's libspeex at run time only.

The gates and tools shell out to more. Each script's header names what it
needs, and the soak, perf and off-LAN scripts stop on the first one missing
(`tools/lib.sh`):

| Tool | Used by |
|---|---|
| a 32-bit glibc (`libc6-i386`, `glibc.i686`, `lib32-glibc`) | `run_server.sh`, `run_probe.sh`, `capture_probes.sh`: the retail Linux servers; 1.5b and UO bring their own `libstdc++.so.5` |
| `gdb`, `gcc -m32` | `GDB_PY` runs (`retail_home.sh`), `run_probe_cod2.sh`'s `RAND_TRACE` |
| `objdump`, `readelf` (binutils), `python3` | `tools/re/` |
| `unshare`, `nsenter`, `taskset`, `ip`, `ss` | the retail servers' namespaces, the soaks, the off-LAN captures |
| `kwin_wayland`, `dbus-run-session` | `cod1_soak.sh`, `cod4_perf.sh`: the nested compositor |
| `unzip` | `cod1_soak.sh` (the maps' arenas) |
| ImageMagick 7, `jpegoptim` | `screenshots.sh` |

## Installs and which tests they run

| Env | Install | Runs |
|---|---|---|
| `COD_DIR` | CoD1 1.1 | the 1.1 tests and gates |
| `COD_DIR` | CoD1 1.5 (`main/` with stock paks only: `--example stock_tree`, below) | `configstrings_15_ab`, `timeout_15_ab`, `legs_ab`'s 1.5 walks (the stop and go too), `turret_ab`'s 1.5 replay |
| `COD_DIR` | UO (`uo/` beside 1.5's `main/`) | the UO tests and what reads only data 1.5 and UO share: `moves_ab`'s UO replays (sprint, mounts, cook), `configstrings_uo_ab`, `playerstate_uo_ab`, `playerstate_combat_ab`'s UO capture, `mount_uo_ab`, `uo_weapons_ab`, `legs_ab`'s UO walks, `turret_ab`'s UO replay |
| `COD2_DIR` | CoD2 1.3 | the CoD2 tests, the `cod2_map_census` and `cod2_soak` tools |
| `COD4_DIR` | CoD4 1.7 | the CoD4 tests, `cod4_live_*`, the `cod4_map_sweep` and `cod4_soak` tools |

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

A test finds its crate's files through `vcod_common::manifest_dir!()`,
`manifest_path!("/tests/...")` or `manifest_const!` (a `static`), never
`env!("CARGO_MANIFEST_DIR")`: that is frozen at compile time, and a worktree
seeded with another checkout's `target/` would read the seed's docs and
fixtures. `crates/common/tests/manifest_dir.rs` fails on a bare use;
`tmp/mg/rewrite-manifest-dir.py <root>` converts new ones.

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
  runs there; they run only on a machine with an install, save the taped
  replays ([Trace tapes](#trace-tapes)). It pins the
  toolchain (`RUST_TOOLCHAIN`, bumped on purpose), passes `--locked`, and
  also checks the workspace on Windows, macOS and the 1.95 MSRV, and runs
  the suite on Windows and macOS. A second Linux step runs the client's
  render tests on lavapipe with `VCOD_REQUIRE_GPU=1`: a headless test takes
  its adapter through `renderer::headless::test_adapter`, so one that finds
  none fails there. Lavapipe is installed in that step only: with it present
  the main suite drew on it too, and the client test binary died once on
  SIGSEGV, so the lavapipe step is `continue-on-error` until that crash is
  understood.
- `nightly.yml` rebuilds the rolling `nightly` release on master pushes that
  touch code (docs, tools and `*.md` are path-ignored). `workflow_dispatch`
  forces one. It runs `ci.yml` first and publishes only a green tip, fails
  the Linux build if a strong glibc symbol passes README's floor, and ships
  `THIRD-PARTY-LICENSES` (`tools/third_party_licenses.py`) and `SHA256SUMS`.
- Each crate's integration tests are one binary named after the crate
  (`--test common`, `--test cod2`, `--test cod4`, `--test gsc`, `--test
  server`; `tests/main.rs`). A file's tests filter by its name (`--
  vehicle_ab::`). The client's unit tests live in its lib: `cargo test -p
  vcod --lib`.

## Trace tapes

A trace tape lets a pmove A/B gate replay in CI with no install. At an
install the gate's replay runs once with a recorder between pmove and the
world, which writes every collision query pmove asks and the answer it got
to `tests/fixtures/tapes/<capture>.tape`. Without the install the same
replay answers each query off the tape, and the first query that is not the
next one on the tape panics with both, so an edit that changes which traces
pmove makes fails there. Each tape's header holds the report the live
replay gave, and the taped test fails unless the replay gives it back and
passes the gate's own checks.

The seams: `vcod_common::tracetape` holds the tape and CoD1's codec, which
`MoveWorld::with_tape` puts behind `box_trace` and `point_contents` (CoD2's
pmove traces through the same `MoveWorld`); `vcod_cod4::pmove::tape::Taped`
is a `PmWorld` on a tape. The tape is text, one query per line, floats in
their shortest round-trip form, with two shorthands: `^k` repeats the entry
`k` back and `.` repeats a token of the last query of its kind. It holds
query and answer numbers only, no geometry. A weapon list beside it
(`weapons-*.txt`, or the CoD4 tape's header) keeps the numeric keys of the
weapons the captures carry and the ammo names of the rest.

| Test | Captures | Tape size |
|---|---|---|
| `moves_ab::the_taped_captures_replay_without_the_paks` | `moves-1.1` (ground, air, crouch, prone, strafes), `patchwalk-kerb-1.1` (steps up the kerb's patches, slides along them) | 849 KB, 425 KB |
| `ladder_ab::the_taped_ladder_climbs_without_the_paks` | `mp_ship-dm-ladder.txt` (climb, back down, strafe, let go, off the top) | 510 KB |
| `moves_ab::cod4_taped_moves_replay_without_the_zones` (`--test cod4`) | `play-1.7-walk.txt`, unarmed | 202 KB |

A change that moves pmove's queries or its result on purpose re-records
them at the installs (each test records first, then replays the new tape):

```
RECORD_TAPES=1 COD_DIR=<abs 1.1 install> cargo test -p vcod-server --test server -- the_taped_
RECORD_TAPES=1 COD4_DIR=<abs> cargo test -p vcod-cod4 --test cod4 -- cod4_taped_
```

Then run the live gates (`moves_ab::`, `ladder_ab::`, `cod4_moves_ab`): a
tape replays what the live replay did, and only the live gate says it still
matches retail.

## The gate runner

`tools/gate.sh [area...]` runs fmt, `tools/feature_drift.sh`, both clippys
and the suite at each install the environment names: `COD_DIR` (1.1) with `COD2_DIR` and `COD4_DIR`,
then the server suite again at `COD_UO_DIR` (a stock-only UO tree) with
`COD4_DIR` unset. It sets `VCOD_REQUIRE_INSTALL` to match, so no install
test can skip. Each area adds the ignored gates of one row group below:
`nav`, `bots`, `render` (`LVP_ICD` points it at lavapipe), `cod2`, `cod4`.
A step passes only on `test result: ok` lines that ran tests. `CARGO_WRAP`
puts each cargo run under a wrapper (a memory-capped scope), `DRY=1`
prints the commands.

`tools/feature_drift.sh` fails when `cargo tree -p <member>` resolves a
third-party crate with other features than `cargo tree --workspace`: each
such crate, and every crate above it, would build once per selection. A
member's own dependency takes the workspace's features on its
`[workspace.dependencies]` line; a crate no member names is pinned in
`crates/workspace-hack/Cargo.toml`, which `vcod-common` depends on. Run it
after a dependency change.

## Other tools

The scripts under `tools/` no other section names:

| Tool | What it does |
|---|---|
| `cod4_perf.py` | Summarizes a `VCOD_FRAME_PROFILE` file or `cod4_perf.sh`'s output (Performance, below) |
| `bsp_append_entities.py`, `bsp_worldspawn_key.py` | Copy a CoD1 `.bsp` with entities appended or a worldspawn key added, the bytes `vcod_common::testing`'s `bsp_with_*` builds |
| `efx_chains.py` | A census of the stock `.efx` chaining keys (`efx-grammar.md`) |
| `pair_sweep.py` | Pairs a `--probe-sweep` run's hits with the labels the server logged |
| `cod4_handshake_probe.py` | Walks retail `cod4_lnxded` through the handshake and logs each datagram (`cod4-handshake.md`) |
| `udp_relay.py` | The UDP relay `offlan_capture.sh` runs |
| `reflow_md.py` | Rewraps a doc's prose to 78 columns in place |

`cod4_perf.sh` and `cod4_soak.sh` serve the other games too (`GAME=cod1`,
`uo`, `cod2`), despite the names.

## Client tools

The client's tools are `vcod::tools`, built outside `cfg(test)` under the
crate's `testing` feature, so an edit to one rebuilds no unit test. One
example runs them by name: `cargo run -p vcod --features testing
--example client_tools -- <tool>` (without the feature cargo skips it).

| Tool | Env | What | Where |
|---|---|---|---|
| `cod4_map_sweep` | `COD4_DIR` | every stock map under every gametype (CoD4 map sweep, below) | `tools/cod4_sweep.rs` |
| `cod4_soak`, `cod2_soak` | `COD4_DIR`, `COD2_DIR` | the soaks; `tools/cod4_soak.sh` runs them a map a process (Soaks, below) | `tools/cod4_sweep.rs`, `tools/cod2_soak.rs` |
| `cod2_map_census` | `COD2_DIR` | every stock CoD2 map (CoD2 map census, below) | `tools/cod2_census.rs` |
| `cod2_cull_profile`, `cod2_cell_walks_draw_alike`, `cod2_load_profile` | `COD2_DIR`, `VCOD_PROFILE_MAPS`, a Vulkan adapter (lavapipe will do) | the CoD2 cell walk's cost and holes, and the map load's stages (Performance, below) | `renderer/cod2_profile.rs` |

The rigs a tool shares with the live gates (`play/cod4_sweep.rs`,
`play/cod2_soak.rs`, the clock reads in `harness.rs`) stay in the crate
under `cfg(any(test, feature = "testing"))`.

## Refactor checks

`tools/move_check.sh <base> <head> <old-file>... [--into <path>]...`
proves a split commit only moved code: it compares the old files at
`<base>` with the files that received them at `<head>` (the old path, its
module directory, each `--into`), line by line after rustfmt. Its header
lists the differences it allows.

`tools/baselines.sh <out-dir>` records the `bot_match` digests, the
`nav_build` digests and each crate's test list at the installs the
environment names (`COD_DIR`, `COD_UO_DIR`, `COD2_DIR`, `COD4_DIR`; takes
`CARGO_WRAP` as `gate.sh` does). `tools/baselines.sh --compare <old-dir>
<new-dir>` prints what differs and exits 1 if anything does.

## Ignored tests: when to run them

`--ignored` always takes a name filter. Unfiltered it also starts the soaks
and the map sweep (minutes and gigabytes a map) and the two live browser
tests, which ask cod.pm's public master and every server it lists, and
broadcast on the LAN.

Every `#[ignore]` carries a reason (what it needs) and every ignored test is
named below, by name, prefix or `module::` filter; `--test common
ignored_tests_named` fails on one that is not.

| Touching | Run | Cost |
|---|---|---|
| pmove, collision or nav | `cargo test -p vcod-server --test server -- --ignored nav_census::` | ~15 s at 1.1; ~150 s at UO |
| the nav rebuild's walk memo, or a collision query entry point | `cargo test -p vcod-server --lib -- --ignored nav::memo::` (UO install) | ~30 s |
| bots | `cargo test -p vcod-server --test server -- --ignored bots::` | ~10 s at 1.1; ~6.5 min at UO |
| the bots' driver or its threads | `cargo test -p vcod-server --test server -- bot_threads::` (not ignored; each install it finds; `tdm` and `sd`, UO `ctf`) | ~15 s with the 1.1, CoD2 and CoD4 installs |
| UO bot vehicles | `-- --ignored bot_vehicles::` at a UO install | ~16 s |
| CoD4 bots | `cargo test -p vcod-server --lib -- --ignored cod4::bots::` (`COD4_DIR`) | ~4 s |
| CoD4 hit boxes per map | `--lib -- --ignored cod4_every_map_poses_both_teams` (`COD4_DIR`) | ~22 s |
| CoD2 gametypes | `--lib -- --ignored cod2_gametype_census` (`COD2_DIR`) | ~18 s |
| CoD2 play: level effects, model pipelines, view weapons, muzzle flash and brass | `cargo test -p vcod --lib -- --ignored play::cod2_soak::cod2_live_` (`COD2_DIR`, a Vulkan adapter) | each plays 16 ms frames for its seconds after the spawn, client and server on one `sim` clock (`harness::run_held`), with a 120 to 180 s deadline on it |
| 1.5 stuck seed | `--lib -- --ignored the_15_overlap_spectator_seed_is_the_only_fit` | ~2 s |
| shaders or render layouts | `cargo test -p vcod --lib -- renderer::pipeline_check` (not ignored: it skips without a Vulkan adapter; lavapipe env below) | |
| CoD2 maps, CoD2 load path | `client_tools cod2_map_census`, `cod2_load_profile` | |
| CoD4 maps, gametypes | `client_tools cod4_map_sweep` (below) | 6-8 min and ~3 GB a map |
| the CoD4x join (`net::conn`'s 21 dialect, `net::reliable::Link`) | `COD4X_DEDRUN=<the release's cod4x18_dedrun> cargo test -p vcod --lib -- --ignored --exact play::cod4::tests::cod4x::cod4x_live_join_spawns_and_leaves` (`COD4_DIR`; `COD4X_OUT=<dir>` keeps the capture) | ~12 s past the build |
| the CoD4x join's gamestate timeout (`conn::GAMESTATE_WAIT`) | the same with `--exact play::cod4::tests::cod4x::cod4x_live_withheld_gamestate_times_out` | ~22 s past the build |
| CoD4x downloads (`net::download`'s `XDownloadMsg`, `XAsk`, `Downloads::on_side`) | the same with `--exact play::cod4::tests::cod4x::cod4x_live_download_fetches_the_mods_files` (a mod under `fs_game mods/vcodx`, www, side channel and op 7) | ~55 s past the build |
| a CoD4x download refusal (`sv_allowDownload 0`) | the same with `--exact play::cod4::tests::cod4x::cod4x_live_download_refused_ends_the_join` | ~10 s past the build |
| CoD4x `sv_wwwDlDisconnected` (the reconnects on one qport) | the same with `--exact play::cod4::tests::cod4x::cod4x_live_www_disconnected_reconnects` | ~20 s past the build |

`nav_census` builds every stock nav graph (at UO, the 19-map UO census).
`server/tests/bots.rs` and `bot_vehicles.rs` hold one map scenario a test,
a graph build each; their doc comments name the map and the gametype, and
the UO ones return at once at a 1.1 install. In the `bas` rounds
`BAS_HEALTH=24500` plays stock base health, `BAS_MAP=all` the five stock
bas maps, `BOTS_SEED=` another seed. Each `#[ignore]` reason there quotes the test's
own dev-profile wall, run alone under `tmp/mg/heavy.sh` (2026-11-21): the
`bas` rounds take 2 to 3 min each, the rest under 45 s.

Measurements and by-hand tools, not gates. Each prints what it measured;
its doc comment says what it reads and which env vars drive it.

| Test | Area | Doc |
|---|---|---|
| `the_mover_replays_the_fixture_named_by_slope_fixture` | 1.1 slope replays (panics without `SLOPE_FIXTURE`) | `playerstate_slope_ab.rs` |
| `recover_game_seed`, `recover_spread_seed`, `recover_rocket_seed` | CoD4 and CoD2 seed searches | their doc comments |
| `leaf_order_census` | CoD1 leaf walk order (`LEAF_MAPS`, `LEAF_SAMPLES`) | bsp-ibsp59-format.md |
| `stock_sun_keep`, `regional_sun_keep` | CoD1 sun keep | cod11-light-grid-and-leaf-lights.md 12 |
| `client_tools cod2_cull_profile`, `cod2_cell_walks_draw_alike` (lavapipe), `cell_walks_side_by_side` (`vcod-cod2`) | CoD2 cell walk (`VCOD_PROFILE_MAPS`) | cod2-client-perf.md 8, 9 |
| `a_prone_view_flick_costs` | CoD2 prone view cost | cod2-movement.md 10 |
| `lit_pipeline_register_census` | CoD4 shader registers on a driver | cod4-client-perf.md 29 |
| `spawn_wave` | the gsc interpreter | "The gsc interpreter" below |

By-hand tools behind an env var: each is `#[ignore = "tool: set <VAR>"]`
and panics with that line when run without it.

| Test | Var | What | Where |
|---|---|---|---|
| `cod4_browser_against_a_named_server` | `COD4_BROWSER_SERVER=ip:port` (and `COD4_BROWSER_BROADCAST`) | the CoD4 browser's Local and Server Info lists against one server, retail in its namespace | `play/cod4_menus/browser.rs` |
| `bench_the_pose` | `SKEL_BENCH=1` | the CoD2 skeleton walk and build times (`COD2_DIR`) | `cod2/tests/skel_ab.rs` |
| `cull_census` | `CULL_CENSUS=<map>` (`CULL_OLD`, `CULL_TURN`) | CoD4 culled triangles over the spawn views | `cod4/tests/cull_census.rs` |
| `cull_census_view` | `CULL_VIEW=<spec>` | the same for one view | `cod4/tests/cull_census.rs` |
| `cull_techsets` | `CULL_TECHSETS=<map>` | triangles and batches by techset (cod4-client-perf.md 21) | `cod4/tests/cull_census.rs` |
| `cull_draws` | `CULL_DRAWS=<map>` (`CULL_DRAWS_VIEWS`) | 1.7's draw surface count over the views | `cod4/tests/cull_census.rs` |
| `an_env_named_capture_reencodes` | `COD4_REENCODE=<capture>` | any capture through the reencode gate | `cod4/tests/net_decode.rs` |
| `spawn_sweep` | `VIS_RAYS=<map>` | CoD4 visibility rays from the spawns | `cod4/tests/vis_rays.rs` |

Never run by an agent: `master_list_live` (cod.pm's public master and every
server it lists) and `lan_scan_live` (a LAN broadcast). Both panic unless
`VCOD_LIVE_NET=1` is set as well.

Times are dev-profile test wall on a 30-thread box, the binary built.
These gates run in the dev profile, which the suite has already built:
`--release` would compile vcod-common, vcod-cod4 and vcod-server a second
time for runs it speeds up by a few seconds. Measured 2026-11-21 (rustc
1.100, 8 jobs under `tmp/mg/heavy.sh`, test time from `finished in`):

| Gate | Dev | Release |
|---|---|---|
| `nav_census::` at 1.1 | 14.4 s | 11.8 s |
| `nav_census::` at UO | 150.0 s | 111.4 s |
| `bots::` at 1.1 | 9.6 s | 7.7 s |
| `bots::` at UO | 387.5 s | 334.4 s |
| `bot_vehicles::` at UO | 15.8 s | |
| `bot_threads::` (1.1, CoD2, CoD4) | 13.3 s | 10.5 s |
| `cod4::bots::` | 4.1 s | 2.9 s |
| `cod2_gametype_census` | 18.3 s | 15.6 s |
| `cod4_every_map_poses_both_teams` | 21.5 s | 17.7 s |
| rebuild of `--test server --lib` after an edit in vcod-common's `pmove()` | 7.5 s | 68.7 s |
| the same after an edit in vcod-server's `Bot::think` | 27.3 s | 60.0 s |
| cold release build of `--test server`, then `--lib` | | 75.5 s, 40.5 s |

Each row's dev run stays under the release rebuild it saves. Only the
whole set at both installs comes out about 40 s ahead in release (618 s
against 511 s and a 70 s rebuild), and the suite already pays the dev
build. `bot_match`, `nav_build` and the gsc bench are measurements, not
gates, and stay in release.

The bit-exact gates (`moves_ab`, `cod4_moves_ab`, `cod4_combat_ab`,
`trace_ab`, `nav_census`) were tuned on the host's glibc. Only `sinf` and
`cosf` are pinned (`vcod_common::libm`, cod4-pmove.md 13.2); `atan2`,
`tan`, `acos`, `pow` and the rest come from the platform's libm, whose
results Rust does not promise across versions. A gate that fails after a
glibc update or on another distro, with no code change, is that before it
is a regression: name `ldd --version` in the report.

CoD4's pmove self-pins (`moves_pins.rs`, `jump_pins.rs`, the
`pmove_single` cases, the charge pick and the legs' anim pick) hold the
model's own output where no capture reaches, not retail's. Each checks a
digest through `pmove::pin::Pin`; a miss prints the first steps and
fields that differ from `crates/cod4/tests/fixtures/pins/<name>.txt`.
After an intended change, update the digest and rerun with `PIN_DUMP=1`
to rewrite the fixtures.

## Headless render: pipeline_check

`renderer::pipeline_check` builds the real `Renderer` headless and draws a
CoD1, a CoD2 and a CoD4 frame under wgpu validation. Its gates are plain
`#[test]`s, not ignored: they need a Vulkan (not GL) adapter with BC
textures, and without one each prints `skipped` and passes, as every
headless render test does; `VCOD_REQUIRE_GPU=1` fails them instead, as
CI sets it on lavapipe. On lavapipe ([Host setup](#host-setup)):
`VK_DRIVER_FILES=$LVP_ICD WGPU_BACKEND=vulkan GALLIVM_PERF=nopt`. Run it
after any shader or layout change. A render test that passes headless can
still draw wrong live; a change to a draw path also needs a test through the
session or replay path.

What the frame checks see:

- `VCOD_FRAME_DIGEST` (`frame_digest.rs`) hashes what the frame hands the
  renderer (view, view weapon, entities, HUD list, sounds, effects,
  console), never a pixel. A shader-only change leaves it the same.
  Under it the menus' colour pulses and edit cursor blink run on the
  drawn `serverTime` (`Ui::set_digest_clock`), so a menu up during a
  replay digests the same twice.
- `VCOD_C1_SHOTS=<dir>` writes CoD1 frames as TGAs for a byte compare
  across builds. `cod1_renderer_builds_and_draws_without_validation_errors`
  writes `mp_carentan.tga` at the renderer's default identityLight 0.5
  (full screen) with every vcod pass on, `vc_bicubic` included.
  `cod1_identity_ramp_frame_matches_the_gamma_pass_and_the_prepass` writes
  `mp_carentan_windowed.tga` at identityLight 1 with `vc_bicubic` 0, the
  only frame through the windowed lightmap shift (`lightmap_lit`,
  cod11-gamma.md 4). Before it, f1808d2b9 changed every windowed CoD1
  lightmap and left both the digest and the one shot the same.

## CoD2 map census

`COD2_DIR=... cargo run -p vcod --features testing --example client_tools --
cod2_map_census` runs
every stock CoD2 map through vcod-server, a spectating client, a headless
frame (lavapipe env and `nice` as for `pipeline_check`;
`VCOD_CENSUS_FRAMES=0` skips it) and a walk from each spawn. It writes its
table, frames and per-map warnings to `tmp/cod2-map-census/`
(`tools/cod2_census.rs` header).

## CoD4 map sweep

`COD4_DIR=... cargo run -p vcod --features testing --example client_tools --
cod4_map_sweep` runs every stock CoD4 map under war, dm, sd, sab, dom and koth.
Per map: a vcod-server with six shooting bots rotating through the six
gametypes (30 s limits) and one client playing it through `Cod4Client::pump`
into a headless renderer (lavapipe env as for `pipeline_check`).

- Output: `tmp/cod4-map-sweep/summary.md`, the per-pair table (load, end,
  rotate, client spawn, script errors, bot stalls, client and server
  warnings, panics) and the ranked errors.
- Run one map a process (`VCOD_SWEEP_MAPS=<map>`, `VCOD_SWEEP_JOBS=1`) under
  `taskset -c 0-7` with `LP_NUM_THREADS=1` (`tools/cod4_sweep.rs` header).
- Results: `cod4-map-sweep.md`.

## CoD4 live tests

The `cod4_live_*` tests (`COD4_DIR`) each run a CoD4 vcod-server and client
in process, on `crate::harness`'s `sim` clock (`src/harness.rs`; the CoD4
joins and war pairs on it are `play/cod4/live.rs`).

- A test's loops count frames its own sleeps move. Under load it runs slower,
  never out of time; only `sim::Clock::sleep`'s 60 s wall assert stops a
  stalled one.
- The clock does not make loopback UDP reliable: a loaded machine can still
  lose a packet. An input whose edge rides one cmd (the location selector's
  confirm, `live_streaks::selector_attack`) is pressed again until its
  effect shows, or one lost packet hangs the test to its limit.
- A test takes one of `harness::live_slot`'s slots before it
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
- A live test runs its server through `harness::run_held`, never on a thread
  of its own pacing frames by `Instant`: under load such a client's
  deadlines run out before it spawns. The browser's and the front end's
  `TestServer` serves on a `sim` clock it puts the starting thread on, and
  a `cod4_loading::Loader`'s worker runs on its starter's clock: the thread
  polling the loader waits off the clock, so only the worker moves it. `play/cod4_sweep.rs`'s `rig` and
  `answer_menu` wait on the thread's `sim` clock, the wall off one (the
  sweep and the soak).
- `play::cod4::tests::cod4x`'s live tests are the exception: they join the
  CoD4x release on the wall clock. Each runs `tools/cod4x_capture.sh` and
  its own binary again inside that script's namespace
  (`COD4X_INNER_ADDR`, `COD4X_INNER_OUT` the capture's directory), where
  the second run joins (cod4x-protocol.md 18.3 to 18.5).
- CoD2's live tests (`cod2_live_*`, `COD2_DIR`) run on the same clock:
  `play::cod2::clock_now` is the session's, and `harness::run_held` serves a
  `Cod2Server` (`harness::Held`: `drain_console`, then the frame). A
  `Connecting`'s worker runs on its starter's clock, the starter waiting
  off it while the worker runs; the load pump (`Cod2Session::pump`) reads
  the clock but sleeps on the wall, so a map's load takes no clock time.
- A live test that writes frames (`VCOD_C4_SHOTS`) runs through
  `harness::run_held`, each frame loop sleeping its 16 ms first. The client
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
names a wrapper command, default `nice -n 10`) builds the `client_tools`
example and runs its `cod4_soak` (`tools/cod4_sweep.rs`).

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
- `tools/cod4_perf.sh <outdir> <map> <secs>` runs a server with 12
  shooting bots and a spectating client under it in a nested kwin, and
  prints the summary. It runs every game (`GAME=`), a real GPU in the
  current session (`SESSION=1`) and a one-setting-a-pass sweep (`SWEEP=1`);
  its header lists every knob, and `cod4-client-perf.md` 25 the sweep's
  columns.
- Per-game numbers: `cod4-client-perf.md` (the per-map table),
  `cod2-client-perf.md`, `cod11-client-perf.md`.
- `client_tools cod2_load_profile` (`renderer/cod2_profile.rs`, run on
  lavapipe as `pipeline_check` is, in release) loads three CoD2 maps twice
  through the renderer, the prediction's collision and the first draw, and
  prints a digest of every texture uploaded and the paks the load marked. A
  load made faster keeps both, unless it stops uploading an image nothing
  binds: then the paks hold and the frames stay byte for byte.

## Bots

Bot play is measured, not eyeballed. Retail has no bots, so these are the
only yardstick.

- `cargo run --release -p vcod-server --example bot_match -- [seed...]
  [KEY=VALUE...]` runs seeded bot-vs-bot matches in every game. It is the
  one bot measuring tool: extend it rather than add another example. Its
  `KNOBS` table is the one list of knobs, which an unknown key prints.
  `DIGEST=1` is the check that a refactor moved no bot; `TICKSTATS=insns`
  and `SPIKE_MS` the tick's cost on a loaded machine (cod4-server-frame.md
  9.1); `VCOD_BOT_THREADS=1` runs the bots' pass serial, with the threaded
  run's digests. `bot-combat.md` section 1 says what each printed number
  means.
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
after (cod4-server-frame.md 9.7).
