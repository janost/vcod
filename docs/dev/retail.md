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
    break the script compile.
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
- `tools/run_probe.sh <probe> [map] [+set ...]` boots retail with a
  `crates/gsc/tests/fixtures/semantics/probe_*.gsc` (or
  `client-probes/probe_*`) installed as the gametype and prints its `PROBE`
  lines. `COD_VERSION` picks the server as `run_server.sh` does (UO's
  gametypes and log live under the homepath's `uo/`).
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
- `--bots N [--bots-shoot]` adds clients.
- `--trace` logs per-snapshot timing.

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
writes, which gsc probe it pairs with. A multi-shell recipe (gsc probe,
target client, shooter client, `+set` cvars, start order, `--probe-secs`) is
in the header of the fixture it produced. `--net-probe --game cod2` and
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

Most modes write into `crates/server/tests/fixtures/` under a fixed name, so
a run against vcod overwrites the evidence. After one, move the files to
`tmp/` and `git checkout` the directory. `--capture-tag` names a second run;
tagged fixtures refuse to overwrite without `--overwrite-fixture`.

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
| `*-15-turret.bin` | a spectator following a `--save-turret` gunner on 1.5b under `client-probes/probe_turret` (`cod11-turrets.md` 14.3) |
| `*-turret-hit.bin` | a 1.1d `--save-turret` gunner's own snapshots while a `--save-hit` probe shoots him off the gun (same section) |
| `*-follow-1.1.bin` | a 1.1d spectator following a `--save-hit` shooter under `client-probes/probe_passthru` (`cod11-spectator-follow.md` 11.1) |
| `*-demo.bin`, `*-demo-1.5.bin`, `*-demo-uo.bin`, `demo*.dm_<n>` | `--save-demo --probe-slope --probe-team allies` beside a `--probe-slope --probe-team axis` probe, on 1.1d and 1.5b mp_carentan and UO mp_cassino. The snapshot file holds every message, and the `.dm_<n>` is the probe's own `tmp/probe-<tag>.dm_<n>` (`cod11-demos.md` 9.2) |

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

### Receive order

Two probes on one server answer the same snapshot within microseconds,
so neither their stamps nor their send clocks (`w=`) say whose packet
the server ran first. The server says, under `+set showpackets 1`:

- VERIFIED, the three servers: each client packet goes `recvfrom` (1.1d
  0x80c723f, 1.5b 0x80d4c4b, UO 0x80c9f4b) to `SV_PacketEvent` (1.1d
  0x808c870), which calls `Netchan_Process` (1.1d 0x80804f0 at 0x808c9ba,
  1.5b 0x8083890 at 0x8093c0c, UO 0x8084378 at 0x8094b6d) and then
  `SV_ExecuteClientMessage` (1.1d 0x80872ec at 0x808ca57, 1.5b 0x808cdd0
  at 0x8093ce6, UO 0x808dba1 at 0x8094c47), whose `SV_UserMove` (1.1d
  0x8086fa4, 1.5b 0x808cade, UO 0x808d8af, the `cmdCount` strings) runs
  each cmd stamped past the client's last one. `Netchan_Process` prints
  `%s recv %4i : s=%i` (1.1d 0x8080691, 1.5b 0x80839b7, UO 0x808449f)
  under `showpackets`, ahead of its `Out of order packet` test (1.1d
  0x80806e7), so the console has one line per packet in the order the
  server ran them. INFERRED, off the call order: a `recv` line's cmds ran
  before the next line's.
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
- VERIFIED, the three `-turn` captures (2026-11-13): every cmd of both
  probes was read (6420, 6300 and 6360 cmds), each client's in the order
  it sent them, and no `Out of order` or `Dropped` line printed. Within a
  tie either client's packet came first, about half the time each.
  Ordering the two clients by `w=` disagrees with the server on 3, 1 and
  2 pairs, all sent 1 to 3 microseconds apart. 1.5b's
  `mp_carentan-dm-turret-victim-15-recv.txt` read all 1020 of its
  gunner's cmds in send order (cod11-turrets.md 13.3).

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
    `probe_hqtime`, `probe_bastime`, `probe_domtime` and `probe_menuclock`;
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
