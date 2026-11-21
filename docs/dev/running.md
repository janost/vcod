# Running and debugging

How the two binaries find their game, which modes each game runs, and the
debug overlays and env vars. The player-facing flags are in
`docs/usage.md`.

## Install and game

- Both binaries expect to sit next to `CoDMP.exe` and read `main/`. With
  `--mod-dir uo`, `uo/` sits over `main/`: any base other than `main` sits
  over its sibling `main/` (`pk3::base_dirs`).
- `COD_DIR` overrides the install for every game; `--game-dir` overrides
  both. `COD2_DIR`, `COD4_DIR` and `COD4_LNXDED_DIR` are read by tests and
  tools only (`docs/dev/testing.md`).
- `--vanilla` keeps only the install patch's stock paks on the search path
  (`pk3::is_vanilla_pak`, `Pk3Fs::vanilla`).
- `--game cod1|cod2|cod4` overrides `game::Game::detect` (AGENTS.md's game
  table lists the markers it looks for).
- A CoD1 install's version is its `Cod1Wire` (`is_uo`, `is_v1`). `pak1-4` are
  identical between 1.1 and 1.5 and `pak0` is not (its checksum differs), so
  a 1.5 install works as the asset source but not on a pure 1.1 server. The
  CoD1 RE notes are about the 1.1 binaries.

## Client modes

The game is the detected install's (or `--game`'s).

| Game | Command | Mode |
|---|---|---|
| CoD1, UO | bare `vcod` | the stock main menu and server browser |
| CoD1, UO | `vcod --connect ip:port` | joins through the stock team menu (`--team spectator` spectates) |
| CoD1, UO | `vcod <map>` | flies over the map |
| CoD2 | bare `vcod` | CoD2's main menu (`cod2_front.rs`); its `connect` joins |
| CoD2 | `--connect` | spectates or plays through `play::cod2`, not `NetClient` (`Mode::Cod2`) |
| CoD2 | `vcod <map>` | flies (`Mode::Fly`); no walk |
| CoD4 | bare `vcod` | 1.7's main menu (`cod4_front`, `Session::Cod4Menu`) |
| CoD4 | `--connect` | plays or spectates through `play::cod4` (script menus `play::cod4_menus`, keys `play::cod4_input`) |
| CoD4 | `--capture <file>` | watches a committed datagram capture |
| CoD4 | `vcod <map>` | flies over the map's zone (`cod4_fly`) |
| CoD1, UO | `vcod <map> --walk` | debug only: an early offline walk mode |

- The client joins 1.1, 1.5 (protocol 6, `net::protocol::PROTOCOL_V6`,
  `cod15-protocol-diff.md`) and UO 1.51 servers, vehicles included (protocol
  22, `PROTOCOL_V22`, `coduo-diff.md` 18; run it with the UO install as
  `--game-dir` and `--mod-dir uo`). It connects with its install's protocol
  and switches when refused, between 1.1 and 1.5 only.
- A UO install refuses a 1.1 or 1.5 server and the others a UO one
  (`Transport::same_game`, `NetClient::connect_from`). The net probe still
  switches to any.
- vcod has no listen server. Start New Server spawns the `vcod-server` beside
  `vcod` (build both), and the client kills it when it exits.

## Server

- `vcod-server <map>` runs vcod's server. `--game cod2` runs `cod2::Cod2Server`
  over `game::cod2::Cod2Level` (cod2-protocol.md 14). `--game cod4` runs
  `cod4::Cod4Server` over `game::cod4::level::Cod4Level` (cod4-handshake.md
  9, cod4-pmove.md 15; what it still lacks: `cod4-server-gaps.md`).
- A CoD1 server speaks one protocol per process, the install's
  (`server::install_protocol`: 1.5's `pak0` checksum, UO's `uo/pakuo*`),
  unless `--set protocol=` says otherwise. Its per-version tables (weapon
  list, item rows, means of death) are `server::defs::Defs::of(proto)` and
  its configstring blocks `proto.cs`, never the 1.1 constants.
- `dedicated` defaults to 1 (no heartbeat, unlike retail's 2). `--set
  dedicated=2` opts in.
- Admin commands go through the stdin console (`--no-console` turns it
  off; CoD1's line editor is `tty_console.rs`) or rcon once `--set
  rconPassword=...` is set (`docs/usage.md`, "Stdin console").

## Logs and env vars

- `RUST_LOG` drives env_logger (default `info`). `WGPU_BACKEND` narrows the
  backend. Every `VCOD_*` variable is in the table below.
- `VCOD_NETSIM="ping=100,jitter=20,loss=2"` gives the client a bad link
  without root. `vcod --net-probe ADDR --probe-clock` measures the clock on
  one.
- Map loads log their stages on one `load <what>: <total> ms (<stage> <ms>,
  ...)` line at info (`vcod_common::laps::Laps`): the client's world GPU
  state and CoD2 map, the CoD2 server's level and its parts.
- `RUST_LOG=vcod::shadows=debug` logs each timed static shadow redraw's GPU
  ms (`start-over` marks the square) and every 120 readbacks the smoothed
  dynamic, mask and CPU ms.

### Every env var

`env_vars_documented` (`crates/common/tests`) fails on a `VCOD_*` or
`VC_*` name in `crates/` that this table lacks. The A/B gates' `*_REPORT`,
`*_TRACE` and `*_DUMP` knobs are [retail.md](retail.md)'s; `bot_match`'s
are its `KNOBS` table's.

| Variable | Read by | Does |
|---|---|---|
| `VCOD_FRAME_PROFILE=<path>` | `vcod` | Per-frame CPU and GPU stage times (`frame_prof`, [testing.md](testing.md#performance)). |
| `VCOD_GPU_GROUPS=0` | `vcod` | With the frame profile, no per-pass-group GPU stamps (cod4-client-perf.md). |
| `VCOD_FRAG_STATS` | `vcod` | Per-frame fragment counters read back (cod11-client-perf.md, cod4-client-perf.md). |
| `VCOD_FRAME_DIGEST=<path>` | `vcod` | A line per drawn game frame, its parts hashed: two `timedemo` runs must match (`frame_digest.rs` header). |
| `VCOD_LOAD_PROFILE=<path>` | `vcod` | Appends each map load's profile, start to first frame, to the file (`vcod_common::laps`). |
| `VCOD_NETSIM="ping=,jitter=,loss="` | `vcod` | A simulated bad link (`net::netsim`). |
| `VCOD_CDKEY=<key>` | `vcod` | Overrides the install's CD key ([usage.md](../usage.md#cd-keys)). |
| `VCOD_VOICE_INPUT=tone` | `vcod` | A 440 Hz tone in place of the microphone (cod2-voice.md). |
| `VCOD_NO_DYNENTS=1` | `vcod` | CoD4 dynamic entities not drawn, for a frame time A/B (cod4-client-perf.md 41); F3's `cod4 dynents` line counts them. |
| `VCOD_PERF_TURN=<deg/s>` | `vcod` | Turns a CoD4 spectator's view on its own (`tools/cod4_perf.sh`'s `TURN`). |
| `VCOD_HEAP_TRIM=0` | `vcod` | No heap trim after a load's first frame, for the kit's A/B of what the next load pays for it (cod4-client-perf.md 34.6). |
| `VCOD_WRITE_ALL=1` | `vcod` | The entity light sets, instances, bones and CoD4 volume blocks written whole each frame, not their changed span, for the `finish` A/B (cod4-client-perf.md 42.1). |
| `VCOD_C4_DRAW_ORDER=0`, `VCOD_C4_ENTITY_CULL=0`, `VCOD_C4_FLOATZ=0`, `VCOD_WORLD_BUNDLES=0`, `VCOD_ENT_BUNDLES=0` | `vcod` | CoD4 renderer A/B switches back to the older path, each named in cod4-client-perf.md. |
| `VCOD_LIT_SPLIT=0` | `vcod` | CoD2 lit draws not split by light kind (cod2-client-perf.md). |
| `VCOD_SOAK=<path>`, `VCOD_SOAK_SEED=<n>` | `vcod` | The CoD1 soak player and its seed (`play::cod1_soak`, `tools/cod1_soak.sh`). |
| `VCOD_COD2_RECORD=<dir>/<name>` | `vcod --net-probe --game cod2` | Records the run as `<name>.dm_1` ([usage.md](../usage.md)). |
| `VCOD_COD2_TIMEOUT=<secs>`, `VCOD_COD2_LOAD_STALL=<ms>` | CoD2 probe | The join's timeout; a stall before the map load (cod2-protocol.md). |
| `VCOD_COD2_TRACE_WEAPON=1` | CoD2 probe | A line per snapshot with the weapon's state, clip and reserve. |
| `VCOD_PROBE_CMDS="<secs>:<cmd>\|..."` | CoD2 probe | Sends each command that far in and prints the server commands back. |
| `VCOD_PROBE_KILL=<secs>` | CoD2 probe | Sends `kill` that far in. |
| `VCOD_PROBE_SNAPS_AT=<secs>:<snaps>` | CoD2 probe | Changes the `snaps` userinfo mid-game. |
| `VCOD_PROBE_REWEAPON=<weapon>`, `VCOD_PROBE_INGAME=<response>` | CoD2 probe | Answers the weapon menu, or the `ingame` menu, 17 s in (`ProbeScript`). |
| `VCOD_PROBE_NO_BODIES`, `VCOD_PROBE_STILL` | CoD2 probe | No body prediction; no scripted movement (cod2-movement.md). |
| `VCOD_COD2_HUD_DUMP=<prefix>`, `VCOD_COD2_HUD_AT=<secs>,...` | CoD2 probe | Writes the HUD's quads at those times as `<prefix>-<secs>.quads` for `VCOD_HUD_QUADS`. |
| `VCOD_TICK_TRACE=<path or dir>` | `vcod-server`, `bot_match` | The tick's step log ([server-tick.md](server-tick.md#tracing-a-tick)). |
| `VCOD_TICK_PROFILE_MS=<ms>` | `vcod-server` | Logs each tick past it with its phases. |
| `VCOD_BOT_THREADS=<n>` | `vcod-server` | The threads the bots' pass plans on, `1` serial ([server-tick.md](server-tick.md#the-bots-threads)). |
| `VC_NAV_COD4=1` | `vcod-server` | CoD4 bots walk the nav graph's CoD4 step, off by default (traps.md, Bots). |
| `VCOD_REQUIRE_INSTALL=<names>` | tests | Turns an install skip into a failure ([testing.md](testing.md)). |
| `VCOD_REQUIRE_GPU=1` | tests | Turns a headless render test's adapter skip into a failure ([testing.md](testing.md#headless-render-pipeline_check)). |
| `VCOD_LIVE_SLOTS=<n>` | CoD4 live tests | How many test servers run at once, 3 by default ([testing.md](testing.md#cod4-live-tests)). |
| `VCOD_LIVE_NET=1` | `master_list_live`, `lan_scan_live` | Lets the two ignored tests that contact a public master or the LAN run. |
| `VCOD_C4_SHOTS=<dir>` | CoD4 live tests | Writes their frames ([testing.md](testing.md#cod4-live-tests)). |
| `VCOD_SWEEP_MAPS`, `VCOD_SWEEP_GAMETYPES`, `VCOD_SWEEP_JOBS`, `VCOD_SWEEP_LEVEL_SECS`, `VCOD_SWEEP_OUT` | `cod4_map_sweep`, the soaks | Which maps and gametypes, how many at once, each level's length, the output directory (`play/cod4_sweep.rs`). |
| `VCOD_SOAK_MINUTES`, `VCOD_SOAK_RENDER_HZ`, `VCOD_SOAK_TICK_LOG_MS`, `VCOD_SOAK_CLIENT_LOG_MS`, `VCOD_SOAK_TICK_WATCH` | CoD4 and CoD2 soaks | `tools/cod4_soak.sh`'s header. |
| `VCOD_CENSUS_MAPS`, `VCOD_CENSUS_SECS`, `VCOD_CENSUS_OUT`, `VCOD_CENSUS_FRAMES=0` | `cod2_map_census` | Its maps, time a map, output directory; no headless frame (`play/cod2_census.rs`). |
| `VCOD_CENSUS_VIEWS` | `renderer/cod4_draws_live.rs` | The views the joined-draws test holds byte for byte. |
| `VCOD_PROFILE_MAPS`, `VCOD_WALK_SPAWNS=<n>` | `cod2_cull_profile`, `cod2_cell_walks_draw_alike`, `cell_walks_side_by_side` | Maps to measure; spawns a map (default 8). |
| `VCOD_HUD_SHOT`, `VCOD_MENU_SHOT`, `VCOD_MENU_SHOT_CLICKS`, `VCOD_MENU_SHOT_WAIT`, `VCOD_HUD_QUADS`, `VCOD_UI_SHOT` | `renderer/hud_shot.rs` | Write CoD2 HUD, menu and scoreboard frames to look at; each test's doc comment says what. |
| `VCOD_FRONT_PPM=<path>` | `cod4_front` test | Saves the drawn Start New Server frame. |
| `VCOD_BODY_TGA`, `VCOD_LIGHTGRID_TGA`, `VCOD_PROBE_STAGE_TGA` | `cod2_light.rs` tests | Save the lit body, the light grid and the probe stage images. |
| `VCOD_VERSION`, `VCOD_GIT_SHA` | the build (`crates/version.rs`) | The version string; `VCOD_GIT_SHA` stands in for `git` where there is none. |

## The F3 overlay

F3 (`--debug-overlay`) shows frame, draw, vis, shader, fog, audio, camera and
(online) prediction, net and snapshot counters.

| Line | Reads |
|---|---|
| `audio vN plays N miss N cull N drop N steal N N.NNms` | live voices, cues started, aliases not found, cues past `dist_max`, cues refused by a full pool, voices stolen |
| `vis: <mode> cells n/m soups a/b tris c/d props p/q occ o h X.XXms` | `occ` occluders built, `h` portals they hid |
| `draw ... lod a/b/c gone N tris T` | dynamic model instances drawn at each LOD, those past their last LOD (not drawn), triangles drawn |
| `clock: behind N  reset N fast N +N -N  extrap N` | newest snapshot time minus the drawn time, then `CL_AdjustTimeDelta`'s resets, fast and slow adjusts and the frames that reached the newest snapshot (docs/protocol-1.1.md, "The client's clock") |
| `ssao <vc_ssao\|off>  gpu scene X.XXms ao Y.YYms` | GPU timestamps around the world passes and the SSAO passes alone, smoothed; absent without timestamp queries |
| `shadow: ents N static Ak/Bk tris cut Ck/Dk xN cpu X.XXms gpu static X.XX dyn X.XX mask X.XXms` (with `vc_shadows 1`) | casting entity surfaces; static caster triangles drawn at the last static redraw (a strip of the static layer, or the square around the eye after a load or a teleport) of the total; the alpha-tested ones of each; static redraws so far; CPU and GPU cost of each pass |

On a CoD2 server F3 adds `net: ping P  snaps N/s  msgs N/s  out N/s  lost N
(total)  frag N/s (total)  rate R snaps S [lan]`, the clock line under CoD2's
rules and a `pred` line (`Cod2Session::debug_lines`, cod2-protocol.md 18).
`--net-probe --game cod2` prints them as `link`.

On a CoD4 map F3's `sunshadow sm_enable  casters Ak/Bk tris + N ents  gpu
X.XXms` (or `sunshadow off`) line shows the world and static model caster
triangles drawn into both sun shadow partitions of all, the entity surfaces
drawn into each, and both passes' GPU ms (`cod4_sunshadow`, cod4-world.md
3.6).

## Other debug keys and habits

- F4 cycles culling `on -> locked -> off`. `locked` freezes the visible set
  so you can fly out and inspect it.
- To reproduce a screenshot spot in fly mode, copy the F3 camera line's
  numbers into `vcod <map> --pos "x y z yaw pitch"`.
- Pre-existing noise: a few `vkAcquireNextImageKHR` fence validation errors
  per run on Vulkan.
- A stock install loads with no shader parser warning
  (`stock_shaders_parse_without_warnings`). A warning means a third-party pak
  or syntax vcod misreads. Retail's own complaints and vcod's approximations
  log at debug (`RUST_LOG=vcod_common::shader=debug`).
