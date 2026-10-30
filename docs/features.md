# What vcod does, and what it doesn't

vcod is one client (`vcod`) and one dedicated server (`vcod-server`) for
four Call of Duty multiplayer games. Each game has its own page: what
works, what is missing against retail, and what vcod adds.

| Game | Patch | Protocol | Client | Server | Bots | Page |
|---|---|---|---|---|---|---|
| Call of Duty | 1.1, 1.5 | 1, 6 | Plays | Hosts | Yes | [features/cod1.md](features/cod1.md) |
| United Offensive | 1.51 | 22 | Plays, vehicles included | Hosts, vehicles included | Yes | [features/uo.md](features/uo.md) |
| Call of Duty 2 | 1.3 | 118 | Plays | Hosts | Yes | [features/cod2.md](features/cod2.md) |
| Call of Duty 4 | 1.7 | 6 | Plays (stock 1.7 servers, not CoD4x) | Hosts | Yes | [features/cod4.md](features/cod4.md) |

## What every game shares

### Picking the game

- Both binaries pick the game from the install: CoD2 by `main/iw_*.iwd`
  or `CoD2MP_s.exe`, CoD4 by `zone/` or `iw3mp.exe`, otherwise CoD1.
  `--game cod1|cod2|cod4` overrides it. United Offensive is CoD1 with
  `--mod-dir uo`.
- The client speaks the install's protocol and refuses another game's
  server. CoD1 1.1 and 1.5 switch to each other when refused.
- `COD_DIR` or `--game-dir` points at the install when the binaries don't
  sit beside the game's executable ([usage.md](usage.md)).

### Client

- Bare `vcod` opens the game's own main menu and server browser, drawn
  from its menu files. Start New Server spawns `vcod-server` beside
  `vcod`; there is no listen server.
- The browser's Internet source is `master.cod.pm`, one protocol per
  query, at most one query a second: port 20510 for CoD1, 20610 for UO,
  20710 for CoD2, 20810 for CoD4. Local and Favorites work as retail's.
- Joins retail servers and vcod-server alike, downloads missing paks or
  iwds (UDP or an HTTP redirect), plays or spectates through the server's
  script menus, and predicts its own movement through the same pmove the
  server runs.
- CD key authorization as each retail client does it, off the LAN only,
  with the key from `VCOD_CDKEY` ([usage.md](usage.md#cd-keys)).
- `record`, `demo`, `timedemo` and `stoprecord` in each game's own demo
  format ([cod11-demos.md](research/cod11-demos.md),
  [cod2-demos.md](research/cod2-demos.md),
  [cod4-demos.md](research/cod4-demos.md)).
- Retail's drop-down console, binds and cvars, saved to `vcod_mp.cfg`.
- F3 shows frame, draw, vis, audio, clock, net and prediction counters;
  F4 freezes or disables culling.
- Hor+ widescreen and ultrawide. CoD1 and UO take `vc_widescreen`
  ([features/cod1.md](features/cod1.md#beyond-retail)); CoD4 has its own
  `r_aspectRatio` and `vc_windowAspect`
  ([features/cod4.md](features/cod4.md)).
- Graphics extras retail never had (`vc_ssao`, `vc_shadows`, `vc_bloom`,
  `vc_hdr` and more, all off by default; `--rtx-on` turns them all on)
  are listed in [features/cod1.md](features/cod1.md#beyond-retail).
- `VCOD_FRAME_PROFILE` writes per-frame CPU and GPU times by stage;
  `tools/cod4_perf.sh` drives it for every game (`GAME=cod1|uo|cod2`).

### Server

- One engine core runs every game's tick in retail's `SV_Frame` order:
  packets, frames, timeouts, the one snapshot pump, the heartbeat. Each
  game plugs its own module into it.
- Runs Activision's own gametype and map scripts on `vcod-gsc`, a `.gsc`
  compiler and VM in this repo, with each game's builtins and dialect.
  Menus, spawns, scoring, rounds and limits come from the stock scripts.
- `map`, `map_restart`, `map_rotate` over `sv_mapRotation`, votes, rcon
  (refused until `--set rconPassword=...`), bans in `ban.txt`
  (`--ban-file`), `g_log`, pings, rate and `snaps` pacing, zombies,
  downloads and `getinfo` / `getstatus` as each retail server does them.
- `dedicated` defaults to 1 (no heartbeat); `--set dedicated=2` opts in.
- No stdin console, no authorize server, no anti-cheat.

### Bots

`vcod-server --bots N [--bots-shoot]` and rcon's `addbot`, `removebot`
and `listbots` work in all four games. One driver runs them over a nav
graph built from each game's own pmove; they join through the stock
menus, play the objectives and fight. Retail has no bots
([bots.md](bots.md)).

### Measuring against retail

Retail's servers are the oracle. A headless probe client
(`vcod --net-probe`) records what a retail server sends, the captures are
committed as fixtures, and the A/B tests replay their inputs on
vcod-server and diff the result. `.gsc` probes run on retail and on vcod's
VM and the suite compares their output. [AGENTS.md](../AGENTS.md) has the
details.
