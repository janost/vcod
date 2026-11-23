# Measuring against retail

Retail is the oracle. This file covers running the retail servers, capturing
with the probe client, the committed fixtures, and the A/B gates that replay
them on vcod.

## Retail servers

- `tools/run_server.sh [map] [+set ...]` runs the retail 1.1d Linux dedicated
  server (setup in the script header). Anything after the map goes to the
  engine verbatim.
  - `COD_VERSION=1.5` runs the 1.5b server. Point `COD_DIR` at a directory
    whose `main/` holds only the stock 1.5 paks (pak0-pakb and the localized
    ones): third-party pk3s in a 1.5 install override the stock gametypes and
    break the script compile. `cargo run -p vcod-common --example stock_tree
    -- <install> <out>` links such a tree (`main/`, and `uo/` on a UO
    install) from the stock paks `--vanilla` keeps.
  - `COD_VERSION=uo` runs the United Offensive 1.51b server (default map
    `mp_cassino`). Its `COD_DIR` needs the 1.5 `main/` and the UO `uo/`. The
    net probe joins it with the UO install as `--game-dir` and `--mod-dir uo`.
    `cargo run -p vcod-common --example handshake_probe` is the standalone
    decoder `coduo-diff.md` was written with.
- `tools/run_probe_cod2.sh` runs retail `cod2_lnxded` with the CoD2 movement
  probe gametype inside `unshare -rn` beside
  `crates/cod2/examples/move_probe.rs`, which writes a
  `crates/cod2/tests/fixtures/moves/` capture (`cod2-movement.md` 17).
  `RAND_TRACE=1` preloads `tools/re/cod2_rand_trace.c` (built with `gcc
  -m32`) and appends every `srand()` and `rand()` the server made to the
  capture: the seed and the draws a replay pins (`cod2-combat.md` 9.6).
- Run the retail CoD2 and CoD4 Linux servers only inside `unshare -rn`.
  A new namespace's loopback is down: run `ip link set lo up` inside it
  before a probe client joins `127.0.0.1`.
- `tools/cod4x_capture.sh <server binary> <out dir> [client command]` runs
  a CoD4x release server (or 1.7's `cod4_lnxded`) on `$COD4_DIR`'s data in
  its own rootless namespace, the install read-only, every master and
  update name on 127.0.0.2, and logs every packet on `lo` through
  `tools/lo_capture.py` while the client command runs
  (`cod4x-protocol.md` 16).
- `tools/run_probe.sh <probe> [map] [+set ...]` boots retail with a
  `crates/gsc/tests/fixtures/semantics/probe_*.gsc` (or
  `client-probes/probe_*`) installed as the gametype and prints its `PROBE`
  lines. `COD_VERSION` picks the server as `run_server.sh` does (UO's
  gametypes and log live under the homepath's `uo/`).
- The retail packages are not in the repo. `COD_LNXDED` names the server
  binary (`COD2_LNXDED` for CoD2); unset, the scripts look in the gitignored
  `private/retail/` (`cod-lnxded-1.1d/`, `cod-lnxded-1.5b/`,
  `coduo-lnxded-1.51b/`, each with its `lib/`).
- A worktree has no `private/` (it is gitignored): link the main checkout's
  in.
- Without `COD_LNXDED_HOME` both scripts run in a homepath of the port's own,
  `private/homes/<server>-<port>` (printed on stderr; `games_mp.log` is
  there), so two runs on two ports share no log. Its archived config is
  dropped each start.
- The retail engine splits its command line on `+`, so a homepath with a `+`
  in it truncates. From a worktree whose path holds one, point
  `COD_LNXDED_HOME` at an absolute path without it.
- `PRE_MAP_ARGS` holds `+set`s that must land before `+map`.
- `GDB_PY=tools/re/gdb_<name>.py` runs a CoD1 server under gdb with that
  script sourced, in both scripts (`tools/re/gdb_*.py` say what they log
  and which `GDB_*` they read). `run_probe_cod2.sh` has its own `GDB_PY`
  for the CoD2 scripts, which start the server themselves.
- The retail engine runs at most 32 `+` commands and folds the rest of the
  line into the 32nd's value. `run_probe.sh` spends twelve, so a recipe gets
  twenty `+set`s.
- `tools/capture_probes.sh > .../retail-captures.txt` reruns the top-level
  probes for `crates/gsc/tests/semantics_ab.rs`. Read that directory's
  `README.md` before writing a probe: six engine behaviours dictate a probe's
  shape.
- `tools/capture_cvars.py <port> <rconPassword>` prints a running retail
  server's cvar registry as the rows of
  `crates/server/src/cvars/registry.rs`.

## vcod's server for the same run

`cargo run -p vcod-server -- <map>` runs vcod's.

- `--gametype-script <file>` runs a gametype from disk (how a client probe
  runs against vcod).
- `--set NAME=VALUE` is retail's `+set`.
- `--bots N [--bots-peaceful]` adds clients.
- `--trace` logs per-snapshot timing.

## A retail client against vcod-server

A 1.1 `CoDMP.exe` joining vcod-server (the first pass is
`cod11-server-handshake.md`, "Retail client check"). Two shells:

```
RUST_LOG=debug cargo run -p vcod-server -- mp_carentan
CoDMP.exe +set r_fullscreen 0 +connect 127.0.0.1:28960     # from the 1.1 install
```

Pass: the server logs `connected`, `gamestate, N bytes` and `sent its first
move`, with no `usercmd not parsed` at debug level; the client loads the map;
neither side prints `EXE_SERVER_IS_DIFFERENT_VER`.

| Symptom | Suspect |
|---|---|
| `usercmd not parsed` at debug level | the full-field usercmd branch, INFERRED from disassembly only; table in `tools/re/net-notes.md` |
| `Illegible server message N` right after the map loads | the missing `svc_EOF` terminator (`ServerNetchan::transmit`) |
| the client disconnects right after the gamestate with another error | the configstring table in `crates/server/src/configstrings.rs` |
| no `connectResponse` | `parse_connect` (`crates/common/src/net/connectionless.rs`) |

## Public servers and the master

- vcod's browser and heartbeat use `master.cod.pm`, whose operator allowed
  it: ports 20510 (CoD1, protocols 1-6), 20610 (UO, 21 and 22), 20710 (CoD2,
  115-118), 20810 (CoD4).
- Ask `getservers <protocol> full empty` with one protocol, never 0 (all
  games), at most once a second (`net::master::master_send_allowed`). A list
  may come in several packets ending `\EOT`, the last `\EOF`.
- Retail's Activision masters are not asked for lists. The client's one
  packet to them is `getKeyAuthorize`, sent only with a key set
  (`VCOD_CDKEY`, 1.1's `main/codkey`) and only to a server off the LAN
  (`net::cdkey`). Tests and probes on loopback never send it, and no test may
  set `VCOD_CDKEY`.
- A 60-100 s spectator capture during a round shows every combat event.

## The probe client

`vcod --net-probe <ip:port>` is a headless client that joins, prints a
per-second snapshot summary, and dumps captures to `tmp/`. Each scripted mode
(`--save-*`, `--probe-*`) is documented on its flag in
`crates/client/src/main.rs` (`vcod --help`): what it does, which fixture it
writes, which gsc probe it pairs with. The fixture paths are fixed at build
time under the checkout the binary was built from, so the fixture modes need
a source build; a release binary writes to its build machine's path. A multi-shell recipe (gsc probe,
target client, shooter client, `+set` cvars, start order, `--probe-secs`) is
in the header of the fixture it produced. Some header text is fixed, not
read off the run: the "Retail CoD <build>" label (picked from the protocol
version, so a vcod-server gets it too), most `sv_maxclients 8, sv_pure 0`
lines, the slope recipes' `<spot>` and `<yaw>`, and the mp_carentan and
port 28970 recipes in the sd, pickup, turret and weapsel headers. Correct
them by hand before committing a fixture. `--net-probe --game cod2` and
`--game cod4` probe those servers through `play::cod2` and `play::cod4`.

- Both servers treat loopback and private addresses as LAN and skip rate
  pacing for them. `tools/offlan_capture.sh retail|ours <dir>` runs a server
  and the probe in two rootless network namespaces with public test
  addresses and logs every datagram. `--probe-rate` / `--probe-snaps` set the
  probe's userinfo. `tools/cod4_offlan_capture.sh` does the same for CoD4,
  with a stand-in authorize server that accepts every client, one join per
  `sv_fps:snaps` pair (cod4-server-frame.md 12).
- Build a new mode on the probe's helpers, which encode retail's input rules:
  - the loop echoes `ps.weapon` in every cmd (a different byte reads as a
    holster request);
  - a switch goes through `WeaponSwitch`;
  - fire goes through `Pulse` (stock rifles and pistols are semi-automatic,
    so taps at least `fireTime` apart);
  - joining goes through `JoinProbe`, which re-answers the team menu after
    every gamestate and `n`.

  Captures from before 2026-09-02 carry fake putaways at reload, stance
  changes and jumps.
- Space client commands past 800 ms yourself; nothing on the client enforces
  it. Retail silently drops non-exempt commands (including bare `score`)
  inside the flood window. `team `, `score ` and `mr ` (with the space) are
  exempt.
- An event lives four slots in the ring, so a transient needs a `!trace` per
  snapshot, not a settled sample.
- Two probes on one server see each other's entities, so a two-client
  question doesn't need two retail clients. The entity list is
  position-dependent (`--probe-pvs`).

### Fixtures are retail evidence

Most modes write into `crates/server/tests/fixtures/` under a fixed name.
A write onto a file that is already there, tagged or not, is refused
without `--overwrite-fixture`, so a run against vcod cannot replace the
evidence by accident; the refused capture is kept under `tmp/` with the
fixture's file name. Retaking a fixture is `--overwrite-fixture`;
`--capture-tag` names a second run. `--save-entities` is the exception:
each run's samples join its file's. A probe that ends on an error still
sends `disconnect`, so the server does not hold a ghost player.

`--save-fixture` / `--save-snapshots` rewrite the parser's byte-exact
captures in `crates/common/tests/fixtures/net/`.

- The `*-delta.bin` pair came from a lone spectator on `run_server.sh
  mp_carentan` with `SNAP_CAPTURE_TARGET` raised to 400 and the files renamed
  by hand. The gate in `net/snapshot.rs` pins the frame counts, so a refresh
  hits the same count or updates the assertions.
- `--capture-tag <tag>` writes the pair as `*-<tag>.bin` and captures for all
  of `--probe-secs`.

| Pair | Recipe |
|---|---|
| `*-1.5.bin` | two spawned probes on `COD_VERSION=1.5 tools/run_server.sh mp_carentan` |
| `*-uo.bin` | a following spectator beside an axis probe on `COD_VERSION=uo tools/run_server.sh` (recipe on `protocol_22_capture_reencodes_and_maps`) |
| `*-uo-tank.bin`, `*-uo-jeep.bin` | the driver of a `--probe-vehicle` crew (`coduo-diff.md` 18) |
| `*-uo-overheat.bin` | the same crew with a `--probe-vehicle overheat` gunner |
| `*-uo-quake.bin`, `*-uo-flame.bin`, `*-uo-arty.bin`, `*-uo-smoke.bin`, `*-uo-mg.bin`, `*-uo-satchel.bin`, `*-uo-bazooka.bin`, `*-uo-bazooka-draw.bin` (under gdb) | a spectator following a shooter under `client-probes/probe_uofx`; arty, smoke, mg and satchel with `--probe-uo-weapon`, the satchel with the smoke script (`coduo-diff.md` 19, 21) |
| `*-uo-turret.bin` | a spectator following a `--save-turret` gunner on `client-probes/probe_turret` (`coduo-diff.md` 19) |
| `*-uo-wreck.bin` | a spawned probe watching `client-probes/probe_vehwreck` kill a tank (`coduo-vehicles.md` 15) |
| `*-panzer.bin`, `*-panzer-wall.bin` | a spectator following a `--probe-weapsel` panzerfaust shooter on 1.1d under `client-probes/probe_panzer` (`cod11-combat.md` 13.6) |
| `*-sd-far.bin` | a spectator `client-probes/probe_lookat` holds out of bombzone_A's PVS (`probe_far 1`) while the `--probe-defuse` / `--probe-plant` pair plays the S&D round on 1.1d (`cod11-sound-system.md`, the playSound broadcast) |
| `*-15-turret.bin` | a spectator following a `--save-turret` gunner on 1.5b under `client-probes/probe_turret` (`cod11-turrets.md` 14.3) |
| `*-turret-hit.bin` | a 1.1d `--save-turret` gunner's own snapshots while a `--save-hit` probe shoots him off the gun (same section) |
| `*-follow-1.1.bin` | a 1.1d spectator following a `--save-hit` shooter under `client-probes/probe_passthru` (`cod11-spectator-follow.md` 11.1) |
| `*-demo.bin`, `*-demo-1.5.bin`, `*-demo-uo.bin`, `demo*.dm_<n>` | `--save-demo --probe-slope --probe-team allies` beside a `--probe-slope --probe-team axis` probe, on 1.1d and 1.5b mp_carentan and UO mp_cassino. The snapshot file holds every message, and the `.dm_<n>` is the probe's own `tmp/probe-<tag>.dm_<n>` (`cod11-demos.md` 9.2) |
| `*-doors-11.bin`, `*-doors-15.bin`, `*-doors-uo.bin` | a `--probe-uo-weapon direct` client under `client-probes/probe_doors` on mp_carentan with six submodels' entities replaced by `crates/server/tests/fixtures/doors/mp_carentan-doors.ent`, on 1.1d, 1.5b and UO; recipe in the headers of `crates/server/tests/fixtures/doors/mp_carentan-dm-doors*.txt` (`cod11-movers.md` 26) |
| `*-dyingpose.bin`, `*-dyingpose-1.5.bin`, `*-dyingpose-uo.bin` | a `--probe-team axis --probe-yaw 90` client under `client-probes/probe_dyingpose` on mp_carentan, on 1.1d, 1.5b and UO; recipe in the headers of `crates/server/tests/fixtures/dyingpose/mp_carentan-dm{,-1.5,-uo}.txt` (`cod11-combat.md` 5.2, 16.4) |
| `*-corpseblast.bin`, `*-corpseblast-1.5.bin`, `*-corpseblast-uo.bin` | a `--probe-team axis --probe-uo-weapon throw` client under `client-probes/probe_corpseblast` on mp_carentan, on 1.1d, 1.5b and UO; recipe in the headers of `crates/server/tests/fixtures/dyingpose/mp_carentan-dm-corpseblast*.txt` (`cod11-combat.md` 5.2) |
| `*-follow-stance.bin`, `*-follow-stance-uo.bin` | a `--probe-follow --save-snapshots` spectator (client 1) following a `--probe-fall --probe-fall-prone 90` client 0 under `client-probes/probe_pronedrop` (`+set probe_teleport 1`) on mp_carentan, on 1.1d and UO (`cod11-mantle.md`, "The forced-stance events"; the probe's recipe in `client-probes/README.md`) |
| `*-skyrocket.bin`, `*-skyfrag.bin` | `client-probes/probe_skyshot`'s sky part on 1.1d mp_carentan by the recipe in the gsc's header: `+set probe_weapon panzerfaust_mp` with `--probe-uo-weapon aim` for the rocket, `fraggrenade_mp` with `throw` for the frag (`cod11-combat.md` 13.6) |
| `*-uo-blasttank.bin`, `*-uo-fragtank.bin`, `*-uo-blastjeep.bin`, `*-uo-fragjeep.bin` | the `--probe-uo-weapon direct` shooter under `client-probes/probe_vehblast`, a bazooka or a frag on an empty Panzer IV on mp_cassino or Willys on mp_peaks; recipe in the headers of `crates/server/tests/fixtures/vehicles/{mp_cassino-blast-tank,mp_peaks-blast-jeep}-{rocket,frag}-uo.txt` (`coduo-vehicles.md` 13) |
| `*-uo-mine.bin` | the watcher beside a `--probe-fall` victim under `client-probes/probe_minefield` (`+set probe_field 1`) on UO mp_pavlov; recipe in the header of `crates/server/tests/fixtures/triggers/mp_pavlov-uo-mine-victim.txt` |
| `*-uo-satchelshot.bin`, `*-uo-satchelchain.bin` | the `--probe-uo-weapon direct` shooter under `client-probes/probe_satchelshot` on UO mp_cassino, the chain with `+set probe_shoot 0 +set probe_turn 10`; recipe in the headers of `crates/server/tests/fixtures/missiles/mp_cassino-satchel{shot,chain}-uo.txt` (`coduo-diff.md` 21) |
| `*-uo-shellfuse.bin` | the `--probe-vehicle sit` driver under `client-probes/probe_shellfuse` on UO mp_kharkov; recipe in the header of `crates/server/tests/fixtures/vehicles/mp_kharkov-shell-fuse-uo.txt` (`cod11-combat.md` 13.7) |
| `*-uo-shellveh.bin` | the `--probe-vehicle sit` driver of a 2026-10-15 run of `client-probes/probe_vehicle_shell` on UO mp_kharkov; the header of `crates/server/tests/fixtures/vehicles/mp_kharkov-tank-shell-uo.txt` gives a later run's recipe, whose log repeats that run's (`coduo-vehicles.md` 13) |

`cargo run -p vcod-common --example event_dump` prints a capture's events in
the wire's numbering.

### Probe timing modes

- `--probe-uo-weapon` and `--probe-vehicle` send cmds at 60 Hz (0, 16 and 32
  or 33 ms past each frame's time; `ProbeHold::sixty` where a held trigger's
  fire rate matters, cod15-movement.md 6). Each holds the view of its first
  spawned snapshot (or of the use tap), rebased on `delta_angles`.
- Against a gsc `setPlayerAngles` every frame, that rebase lags by the link,
  so where the rounds go rides the client's timing and repeats neither on
  retail nor on vcod. In-process (`tests/common::ProbeHold`) only a frame's
  first cmd takes the gsc's view.
- `--probe-uo-weapon aim` (`ProbeHold::aim`) sends every cmd at the
  snapshot's view, which a gsc steers round for round: measure with that
  (`coduo-vehicles.md` 13).
- `--probe-uo-weapon direct` does the same and takes its buttons, lean and
  stance from the gsc's `iPrintLn("PROBE_IN ...")` lines, so a phase starts on
  the server's clock (`client-probes/probe_leanhit`).
- `--probe-vehicle seats|drive --save-slope` writes a seated capture whose
  snapshot lines carry the vehicle's whole entity state (`veh=`), what
  `vehicle_ab` replays on vcod (recipe in `coduo-vehicles.md` 8 and 14). It
  starts 3 s past the weapon pick, after the seat. `--slope-from-spawn` logs
  from the spawn, whose pick sets the seat's `legsAnim` toggle (13).

- `--probe-fall`, `--probe-ride`, `--probe-vehicle`, `--probe-uo-weapon`
  and `--probe-items` print `CMDS` lines (every cmd's time, input changes,
  each snapshot's `commandTime`) and one `SPAWNED` line (the first spawned
  snapshot's time, origin, view and velocity). A gate replays them on the
  capture's clock: ours runs alone until its spawn or placement lands on
  the capture's time (`vehicle_die_ab`, `vehicle_runover_ab`,
  `itemdrop_ab`), then sends the cmds through `ProbeHold::framed` with
  `times` and `ran`. A placement in the spawn's frame keeps the velocity
  the cmds run at the spawn point left, which the gate hands ours from
  `SPAWNED` (`Server::test_set_client_velocity`, cod11-combat.md 2.1).

### Receive order

Two probes on one server answer the same snapshot within microseconds, so
neither their stamps nor their send clocks (`w=`) say whose packet the
server ran first. Run it with `+set showpackets 1`: its console then has
one `recv` line per packet in the order the server ran them
(cod11-server-handshake.md, "Receive order", has the addresses and the
captures that checked it).

- The print names no client. Each probe's `CMDS` lines carry the netchan
  sequence of each cmd's message (`q=`, `--save-turret`'s `!cmd` lines
  too), and the line's `s=` is that sequence; two clients joined seconds
  apart sit about 180 sequences apart. `Client <n> connecting` marks
  where slot n's sequences start.
- `tools/recv_order.sh <homepath>/<game dir>/console_mp_server.log`
  prints the order as the `RECV` lines a fixture carries, and
  `common::cmds_read` matches them to each client's cmds. Run the server
  and both probes inside one `unshare -rn` (the recipe in
  `mp_carentan-leanhit-turn.txt`'s header).

### On 1.5 and UO

On 1.5 (protocol 6) and UO (22) every mode reads through the connection's
protocol (`probe::wire()`): netfield tables, configstring blocks, the event
ring and temp entities, means of death.

- Event ids and means of death print in 1.1's numbering, as `pm_flags` and
  `eFlags` do once decoded. A UO-only event prints as `EV_FOREIGN | id`. UO's
  `EV_BULLET_HIT` and projectile explodes print as the client keeps them
  (`EV_UO_*`): as the small hit and the grenade and rocket no-marks explodes.
- `eType`, configstring slots and weapon indices stay the wire's.
- A fixture-writing mode refuses to run off 1.1 without `--capture-tag`. Both
  servers load mp_carentan, so give each its own tag.
- A fixture-writing mode also ends at the gamestate when the serverinfo's
  `mapname` or `g_gametype` holds anything but letters, digits and `_`: the
  two name the fixture file.
- Run on both retail servers: the join, `--save-playerstate`, `-motion`,
  `-combat`, `-grenade`, `-configstrings`, `--probe-target` with
  `--save-hit`, `--probe-follow`, `--probe-pvs`, `--probe-weapsel`,
  `--probe-compass`, `--probe-edge watch`. On 1.5 also `--probe-items` beside
  `COD_VERSION=1.5 tools/run_probe.sh client-probes/probe_itemdrop`.
- `run_probe.sh` takes `COD_VERSION=uo`. Of the modes that pair with a
  `client-probes/` gametype these have run there:
  - `--save-combat` beside `probe_uofx`;
  - `--save-turret` beside `probe_turret` (its
    `mp_carentan-dm-turret-uo.txt` committed, as 1.5's `-15.txt`, and
    `-victim-uo.txt`, which logs the target's entity);
  - `--probe-flame` beside `probe_flame` (`coduo-diff.md` 19, "The flame
    report");
  - `--probe-items` beside `probe_itemdrop` and `probe_fragcount`
    (`cod11-items.md` 4.1, 14.7);
  - `--save-scripted` beside `probe_ctf`, `probe_dom`, `probe_hq`,
    `probe_bas`, their `*end` endings, `probe_ctftime`, `probe_tdmend`,
    `probe_tdmskip`, `probe_hqtime`, `probe_bastime`, `probe_domtime`,
    `probe_tdmrank` and
    `probe_menuclock`;
  - `--probe-say` beside `probe_threadorder`;
  - `--probe-uo-weapon direct` beside `probe_rise`.
- On UO's mp_carentan the grenade script throws its one frag and the steps
  after it hold nothing.

## The A/B gates

`crates/server/tests/*_ab.rs` replay a retail fixture's inputs on vcod and
diff the result. CoD2's and CoD4's gates sit beside their code
(`crates/server/src/game/cod2_*_ab.rs`, `crates/server/src/cod4/*_ab.rs`)
and in their crates (`crates/cod2/tests/*_ab.rs`, `crates/cod4/tests/moves_ab.rs`).

- Where a gate tolerates rows, it names them in a `*GAPS` constant with the
  research section that explains each.
- Debug env vars (`*_REPORT=1`, `*_TRACE=<ct>`, `*_DUMP=<path>`,
  `SLOPE_FIXTURE=<path>`) are read near the top of each gate.
- Gates that need paks skip without `COD_DIR`.
- A CoD1 gate boots through `common::Boot` (map, gametype or its own
  `common::cfg` config, a probe's gsc as the gametype script, cvars, the
  wire, and a `prep` hook for spawn pins, a seed or a start clock) and
  drives one probe client through `common::run_probe`, its per-frame cmds
  and server commands through `common::ProbeHooks` (`DirectInput` for
  `--probe-uo-weapon direct`). A capture recipe's `+set`s come from
  `common::recipe_sets`. The gates that patch the map's bytes, build their
  world by hand or drive two clients or a custom frame keep their own
  boot or loop.
- A gate on a client probe whose watcher sleeps before it polls runs the
  server alone for `common::PROBE_JOIN_MS` before its client connects, as
  retail's client joined late. A body's idle clip runs from the spawn, and a
  spawn before the first poll put `linkto_ab`'s bones a quarter second off
  retail's (`cod11-movers.md` 15).

### The client's gates

- `crates/client/src/play/replay.rs` replays slope captures as vcod's own
  body, one frame per cmd, and the committed snapshot captures as a follower,
  through the prediction, `play::events::frame_events`, the kick, the effect
  resolver and the sound cues, each shot played once. A new step in the
  frame's event path goes in `frame_events` so the replay runs it.
- `crates/client/src/play/loopback.rs` plays vcod-server itself, in process,
  through the same frame (clock, cmd packets, prediction, events, kick) and
  counts each round fired against the ammo the server took.
