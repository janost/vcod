# Running and debugging

How the two binaries find their game, which modes each game runs, and the
debug overlays and env vars. The player-facing flags are in
`docs/usage.md`.

## Install and game

- Both binaries expect to sit next to `CoDMP.exe` and read `main/`. With
  `--mod-dir uo`, `uo/` sits over `main/`: any base other than `main` sits
  over its sibling `main/` (`pk3::base_dirs`).
- `COD_DIR` overrides the install; `--game-dir` overrides both.
- `--vanilla` keeps only the install patch's stock paks on the search path
  (`pk3::is_vanilla_pak`, `Pk3Fs::vanilla`).
- `--game cod1|cod2|cod4` overrides `game::Game::detect`: CoD4 by `zone/` or
  `iw3mp.exe`, CoD2 by `main/iw_*.iwd` or `CoD2MP_s.exe`, else CoD1.
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
- There is no stdin console. Admin commands go through rcon, refused until
  `--set rconPassword=...`.

## Logs and env vars

- `RUST_LOG` drives env_logger (default `info`). `WGPU_BACKEND` narrows the
  backend.
- `VCOD_FRAME_PROFILE=<path>`: per-frame CPU and GPU stage times
  ([testing.md](testing.md#performance)).
- `VCOD_TICK_TRACE`, `VCOD_TICK_PROFILE_MS`: the server tick's step log and
  slow ticks ([server-tick.md](server-tick.md#tracing-a-tick)).
- `VCOD_NETSIM="ping=100,jitter=20,loss=2"` gives the client a bad link
  without root. `vcod --net-probe ADDR --probe-clock` measures the clock on
  one.
- Map loads log their stages on one `load <what>: <total> ms (<stage> <ms>,
  ...)` line at info (`vcod_common::laps::Laps`): the client's world GPU
  state and CoD2 map, the CoD2 server's level and its parts.
- `RUST_LOG=vcod::shadows=debug` logs each timed static shadow redraw's GPU
  ms (`start-over` marks the square) and every 120 readbacks the smoothed
  dynamic, mask and CPU ms.

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
- To reproduce a screenshot spot in fly mode, add a temporary
  `VCOD_POS="x y z yaw"` override to the `FlyCamera::new` spawn in `main.rs`
  and remove it before committing.
- Pre-existing noise: a few `vkAcquireNextImageKHR` fence validation errors
  per run on Vulkan.
- A stock install loads with no shader parser warning
  (`stock_shaders_parse_without_warnings`). A warning means a third-party pak
  or syntax vcod misreads. Retail's own complaints and vcod's approximations
  log at debug (`RUST_LOG=vcod_common::shader=debug`).
