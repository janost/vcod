# Using vcod

Command lines, controls, the console and the menus. The README's
[How to play](../README.md#how-to-play) is the short version.

## The closest thing to a game

```
vcod-server mp_carentan --bots 4 --bots-shoot
vcod --connect 127.0.0.1:28960
```

Pick a team, pick a weapon, go get shot by a bot.

## Client

```
vcod                                   # main menu and server browser
vcod mp_pavlov                         # fly
vcod mp_pavlov --walk                  # walk, offline
vcod --list                            # every .bsp in the search path
vcod --connect <ip:port> --team axis --weapon kar98k_mp
```

- The map name is case-insensitive.
- `--team <allies|axis|autoassign|spectator>` and `--weapon <file>` answer
  the menus without showing them, for `--connect` and every console
  `connect`. If the menu refuses the weapon, you pick by hand.
- `--mod-dir uo` searches United Offensive's `uo/` over the `main/` beside
  it, as UO itself does, so `vcod --game-dir <UO install> --mod-dir uo
  --connect <ip:port> --team spectator` watches a UO server and
  `vcod mp_cassino --game-dir <UO install> --mod-dir uo` flies its maps.
  The client plays only servers of its install's game: with `--mod-dir uo`
  it refuses a 1.1 or 1.5 server, without it a UO one, and says which
  flag to use.
- `--game cod1|cod2|cod4` names the install's game; without it the game dir
  decides (`zone/` or `iw3mp.exe` is CoD4, `main/iw_*.iwd` or
  `CoD2MP_s.exe` CoD2, anything else CoD1). On a CoD2 install the client
  flies (`vcod <map>`: world and static models, the sky cube, the
  lightmapped phong surfaces lit by their normal, specular and detail
  maps, the sun, the map script's `setExpFog` or `setCullFog` on what
  its fogged technique fogs (radial, the sky clear), and the console in
  CoD2's fonts, models lit by the light grid's probe and the sun per
  vertex; no effects, no dynamic shadows, no normal or specular maps on
  models), spectates a CoD2 1.3 server, pure or not (`vcod --connect
  ip:port`: the server's team and weapon menus, or free flight through usercmds, the server's entity models,
  brush models, posed player bodies, the server's fog, the events'
  sounds and effects, the looping effects and the level's ambient;
  `--team allies|axis|autoassign`
  and `--weapon <name>` (e.g. `thompson_mp`) join and play, predicted,
  through CoD2's binds, X or Esc's menu choosing another weapon for the
  next spawn;
  a map change loads the new map) and probes one headless (`vcod
  --net-probe ip:port --probe-secs N`, `--probe-team` / `--probe-weapon`
  to join and play a 16 s scripted loop of moves, a jump, a crouch, shots,
  the sight and a weapon switch, prints a line a second, with what the
  scene built, the sound cues and effects started and the prediction's
  corrections, and fails on a parse error or a drop;
  `VCOD_COD2_RECORD=<dir>/<name>` records the run as `<name>.dm_1`).
  `vcod --game cod2 --probe-demo <file.dm_1> [--probe-timedemo]` plays a
  CoD2 demo headless and prints what it parsed and drew. Bare `vcod --game cod2` opens CoD2's stock main menu; its
  browser lists LAN servers, `master.cod.pm`'s CoD2 list (protocol 118)
  and favourites and joins one, Start New Server runs `vcod-server --game cod2` beside
  `vcod`, and Esc in a game opens the server's `ingame` menu.
  Every other mode exits with an error. On a CoD4 1.7 install (`zone/`
  beside `main/`) bare `vcod` opens 1.7's main menu from the `ui_mp` zone
  (mouse, arrows, Tab, Enter, Esc): Start New Server runs `vcod-server
  --game cod4` beside `vcod` and, with Dedicated No, joins it; Join Game
  lists LAN servers (Source: Local), favourites and, on Refresh List,
  `master.cod.pm`'s CoD4 list, and joins the one picked (Password sets
  its password; CoD4x servers stay out of the lists unless
  `ui_browserShowCoD4x` is 1, vcod's own cvar); Mods runs a `mods/<dir>` mod; Options and Controls set
  the options and binds vcod has; Select Profile creates, deletes and
  loads a profile in `players/profiles/` (the first start asks for one;
  the active profile's `vcod_mp.cfg` is the config, its `vcod_mpdata` the
  stats a join sends, stored plain and written back as they change;
  retail's `mpdata` is left alone); Quit quits;
  `connect ip:port` on the console joins a server, Esc or `disconnect`
  on the connect screen calls the join off, and leaving it comes back to
  the menu (docs/research/cod4-front-end.md).
  `vcod <map> --game cod4` flies the stock map
  `zone/english/<map>.ff`; `vcod --game cod4 --capture <file>` replays a
  `cod4_probe` capture (`crates/cod4/tests/fixtures/net/play-1.7-*.txt`),
  `vcod --game cod4 --connect ip:port` joins a 1.7 server through its
  team and class menus, drawn as the zones give them (the mouse or the
  arrows and Tab move the focus, a click or Enter picks, Esc runs the
  menu's `onEsc` or, with none up, the server's `g_scriptMainMenu`: the
  team menu, then the in-game `class` menu after a team pick; a menu vcod
  cannot evaluate is a numbered list, 1-9 pick), or on their own with `--team allies|axis|autoassign|spectator`
  and `--class assault_mp,0` (`specops_mp,0`, `heavygunner_mp,0`,
  `demolitions_mp,0`, `sniper_mp,0`, `customN,0`), and
  `--net-probe ip:port --game cod4 [--team T] [--class C]` does it
  headless, a line a second, printing the menus the server opens and the
  spawn (a playing team takes `assault_mp,0` unless `--class` says
  otherwise); every other CoD4 mode exits with an error. `--fs-game
  mods/<name>` loads a CoD4 map under a mod (its `mod.ff` and iwds over
  the stock ones); a joined server's `fs_game` wins over it. A map
  `zone/<language>/` lacks loads from `usermaps/<map>/<map>.ff`; under a
  mod that directory's iwds load too. Joining (and `--net-probe`)
  downloads the mod and usermap files the server references and the
  install lacks into the install, at the server's path; with the
  `cl_allowDownload` cvar at 0 (`vcod_mp.cfg`) the join is refused
  instead.
  `vcod-server <map> --game cod4 --game-dir <CoD4 install>` hosts a 1.7
  level (`war` unless `--gametype` or `--set g_gametype=` says
  otherwise, the `--set` winning): it loads the map's zone and the
  five common ones, runs `default_mp.cfg` out of its iwds
  under the `--set`s and the gametype's scripts, answers `getinfo`, `getstatus` and
  `rcon` (`--set rcon_password=...`; `status`, `serverinfo`, `systeminfo`,
  `dumpuser`, `say`, `tell`, `clientkick`, `kick` (`kick all`),
  `onlykick`, `tempBanClient`, `tempBanUser`, `banUser`, `banClient`,
  `unbanUser`, `heartbeat`, `killserver`, `set`, `setu`, `toggle`,
  `togglep`, `reset`, `setfromdvar`, `vstr`, `exec`, `cmdlist`, and
  `map`, `devmap`, `map_restart`, `fast_restart` and `map_rotate` with
  `sv_mapRotation`, which `server_map.cfg` fills with the stock maps), and
  takes the stats and the pure `cp` as retail does (`--set sv_pure=0`
  turns the check off). A client answers the team and class menus,
  spawns with its class's loadout, plays, and calls and casts votes
  (`--set g_allowVote=0` turns them off); a match that ends rotates to
  the next map. `--bots <n> [--bots-shoot]` adds bots
  ([docs/bots.md](bots.md#cod4)), and rcon's `addbot`, `removebot` (or
  `kickbot`), `listbots` and the `vc_bot_*` cvars manage them as on CoD1.
  It heartbeats only with `--set dedicated=2` and a master in `--set
  sv_master1=host[:port]`. It writes `games_mp.log` to
  `<home>/main/` (`<home>/<fs_game>/` under a mod), the home path the
  `--game-dir` unless `--set fs_homepath=` says otherwise; `--set g_log=`
  empty turns it off, `--set g_logSync=1` flushes every line, and `--set
  g_inactivity=<secs>` drops a player idle that long.
  `vcod-server <map> --game cod2` hosts a CoD2 1.3 map: it runs the
  stock gametype script, answers `getinfo`, `getstatus` and `rcon`
  (`--set rcon_password=...`; `status`, `kick`, `clientkick`, `tell`,
  `say`, `dumpuser`), runs the cheat commands under `--set sv_cheats=1`,
  checks the pure `cp` (`--set sv_pure=0`
  turns it off), takes `--set g_password=`, `sv_privateClients` with
  `sv_privatePassword`, `sv_minPing` / `sv_maxPing`, `sv_maxRate`,
  `sv_timeout` and `sv_floodProtect` as retail does, serves a non-stock
  iwd in its `main/` to a client that lacks it (`sv_allowDownload`, on by
  default; `sv_wwwDownload=1` with `sv_wwwBaseURL` redirects instead),
  and takes clients through the team and weapon menus to
  a spawn; players move, see each other and fight (shots, grenades,
  deaths and respawns). `--set g_gametype=` picks any stock gametype,
  `--gametype-script <file>` runs one off disk as on CoD1, and rcon's
  `map`, `map_restart`, `fast_restart` and `map_rotate` (with
  `sv_mapRotation`) change the level. `--bots <n> [--bots-shoot]` adds
  bots that play every stock gametype there ([docs/bots.md](bots.md#cod2)),
  and rcon's `addbot`, `removebot` (or `kickbot`), `listbots` and the
  `vc_bot_*` cvars manage them as on CoD1.
- `--vanilla` loads only the stock retail paks of the install's patch
  (`pak0`..`pak8` on 1.1, `pak0`..`pakf` on 1.5, UO's `pakuo*` and its
  `uomappack00` too, and their `localized_*` paks) and ignores every other
  pk3 in `main/` and `uo/`: third-party maps, `zzz_*` mods, downloads. It
  downloads nothing: a server whose map is not stock fails the load with a
  popup naming the server's paks, and on one that only adds files the
  client plays with the stock ones. It ignores `fs_game` (the Mods menu
  lists none), and Start New Server starts a `--vanilla` server. The
  read-only `vc_vanilla` cvar reads 1, and the start-up log line says
  `(vanilla)`.
- `--debug-overlay` (or F3) shows frame, draw, vis, net and audio counters.
- `--rtx-on` turns on every beyond-retail extra for one run (`vc_bicubic`,
  `vc_bloom`, `vc_shadows`, `vc_ssao`, `vc_sunShafts`, `vc_softParticles`,
  `vc_hdr`, `vc_grade`, `vc_vignette`, `vc_grain`) without saving them to
  the config. Despite the name, no rays were harmed.
- `--no-audio` runs silent; `--volume <0..1>` sets the master volume
  (`mss_volume`, default 0.8, also on the Sound screen).
- `--window WxH` opens a window of that size for the run, over `r_mode`
  and `r_fullscreen` (a `vid_restart` keeps it), and sets no cvar, so the
  saved config's mode is untouched.
- `--net-probe <ip:port>` is the headless probe client; `vcod --help`
  documents its modes.

### CD keys

Many public servers check CD keys. Such a server answers the challenge with `needcdkey` until its authorize server has seen your key, and vcod then shows the game's own "key not valid" or "awaiting key authorization" message instead of joining.

Give vcod the key one of two ways:

- `VCOD_CDKEY="XXXX-XXXX-XXXX-XXXX"`: the key's 16 characters (dashes, spaces and case don't matter; anything past the 16th is ignored). It is used for whichever game the install is.
- CoD 1.1 and 1.5 only: the retail file `main/codkey` in the install directory, which the 1.1 installer writes. 1.5, UO, CoD2 and CoD4 keep the key in the Windows registry, which vcod does not read, so use the env var there.

A server that checks no keys (a LAN server, vcod-server, most local retail servers) joins without one. vcod sends what the retail client sends, the key and, on CoD2 and CoD4, its MD5 hash, to the same authorize host (`codauthorize`, `coduoauthorize`, `cod2master` or `cod4master.activision.com`) before each challenge to a server off the LAN. It never logs, prints or stores the key.

## Server

```
vcod-server mp_carentan --port 28960 --hostname "my server" --gametype tdm
```

- Binds `0.0.0.0` and answers `getstatus` from anyone, like retail. Keep it
  on a LAN or behind a firewall you control.
- `--gametype` picks the script under `maps/mp/gametypes/` (default `dm`);
  `--max-clients` sets `sv_maxclients` (default 8).
- `--set NAME=VALUE` is retail's `+set`, repeatable:
  `--set scr_friendlyfire=1`, `--set sv_mapRotation="..."`. Each is set
  once, before the scripts load; one written after the map
  (`vcod-server mp_carentan --set ...`) runs again once the level is up,
  as retail's `+map x +set ...` does, so it outranks a script's load-time
  `setCvar`. A later level never puts it back.
- `--set fs_game=<dir>` runs a mod: its directory's paks go over `main/`,
  and clients are told to load it, as retail's `+set fs_game`.
  Under `--game cod4` it takes `mods/<name>`: the mod's `mod.ff` loads
  after the common zones, its iwds go over `main/`'s (a usermap's
  between them), the weapon files come off them, and configstrings 0 and
  1 name it as retail's 1.7 server does. A client lacking the mod's
  iwds or `mod.ff` downloads them (`sv_allowDownload`, on by default;
  `sv_wwwDownload=1` with `sv_wwwBaseURL` redirects).
- `--vanilla` loads only the stock retail paks, as the client's does, so
  `sv_paks` and `sv_referencedPakNames` name stock paks alone and a
  third-party map does not load. It refuses `--set fs_game`.
- `--set rconPassword=<pw>` turns rcon on. Under `--game cod2` and
  `--game cod4` it sets their `rcon_password`.
- It writes retail's `games_mp.log` to `<home>/main/` (`<home>/uo/` on
  UO, `<home>/<fs_game>/` under a mod), the home path being `--game-dir`
  unless `--set fs_homepath=` says otherwise; `--set g_log=` empty turns
  it off and `--set g_logSync=1` flushes every line. `--game cod2` does
  the same.
- `--ban-file <path>` keeps `banUser`/`banClient` bans across runs in
  retail's `ban.txt` shape, a guid and the cleaned name a line; an off-LAN
  client's guid is its IPv4 address as a signed integer (on CoD4, the
  client's GUID). A file in vcod's
  older `<ip> <name>` shape is rewritten into guid lines the first time
  the server opens it, after a copy of the old file is kept as `<path>.old`.
- `--set g_password=<pw>` makes the server private, checked as retail does
  at connect, `map_restart` and map change.
- `--set sv_privateClients=N --set sv_privatePassword=<pw>` reserves the
  first N slots for clients that send that password, as retail does.
- `--set scr_motd="..."` is the message of the day players read: on the
  team menu (1.1) or the server info screen before it (1.5, UO), in vcod
  and in the retail clients. `g_motd` reaches the client as configstring
  14 (UO 15), which no retail client draws, so it shows nowhere.
- `dedicated` defaults to 1, not retail's 2, so dev runs stay off the master
  list. `--set dedicated=2` heartbeats `master.cod.pm` (port 20510, 20610
  on UO, 20710 with `--game cod2`, which has no `sv_master` cvars) every
  three minutes and sends a flatline on Ctrl-C or `quit`, at most one
  packet a second to a master host.
- `--bots <n>` adds `n` bots in real client slots. The nav graph builds on
  its own thread on the first tick (a second or two on big maps); bots
  wander until it's ready. `--bots-shoot` lets them fight; without it
  `--bots` sets `vc_bot_shoot 0`. See [Bots](#bots) for managing them
  while the server runs.
- On a patch 1.5 install the server speaks 1.5 (protocol 6) and takes only
  1.5 clients; with `--mod-dir uo` on a UO install it speaks UO (protocol
  22). `--set protocol=1`, `6` or `22` overrides the install.
- `--gametype-script <file>` runs a gametype from disk instead of the paks.
- `--script-dir <dir>` (CoD4) reads the loose `.gsc` files under `<dir>`
  over the zones' scripts each level, as retail reads a mod's (a map's
  `maps/mp/<map>.gsc`, say).
- `--test-entities <n>` adds invisible entities that move on the wire, to
  exercise the entity encoding.
- `--trace` logs every snapshot per client (its `wall` gap since the last
  tick beside the `frames` that tick ran), and once 20 frames have run the
  loop's ticks and frames, the slowest tick and how far the loop fell
  behind its schedule (20 Hz; CoD4's `sv_fps`).

### Bots

The full guide, with the minimum player count, team balance and slots,
is [bots.md](bots.md).

Bots are a vcod extension; retail has none. Each joins like a client,
through the stock team and weapon menus, named `[BOT] <name>` from a pool
of 120 (`crates/server/src/botmgmt.rs`), never two alike. Manage them over
rcon (`--set rconPassword=<pw>`):

| Command | Does |
|---|---|
| `addbot [allies\|axis\|auto] [count] [skill N]` | Adds `count` bots (1). `auto`, the default, deals them by balance; a named team pins them there. `skill N` (0-3) fixes their skill level, which `vc_bot_skill` then leaves alone. |
| `removebot [name\|all\|count]` | Removes one bot by name (`[BOT] ` optional), all of them, or `count` (1), lowest score first. |
| `listbots` | Slot, team, score, skill level and name of each bot, then the counts and cvars. |

The `set`/`seta` cvars below apply within half a second, over rcon or
as `--set`:

| Cvar | Default | Does |
|---|---|---|
| `vc_bot_minplayers` | 0 | Keeps at least this many players, topping up with bots. A human counts once he picks a team; one joining takes a bot's place (off the larger team) and one leaving brings it back. 0 is off. |
| `vc_bot_autobalance` | 1 | In team gametypes (all but `dm` and `bel`) bots join the smaller team and move so the teams differ by at most one. A bot moves only while dead or spectating; humans and pinned bots never move. |
| `vc_bot_shoot` | 1 | 0 leaves the bots walking the map without fighting. |
| `vc_bot_names` | 1 | 0 names new bots `bot<slot>`. |
| `vc_bot_skill` | 1 | Skill level 0-3 (easy, normal, hard, expert) of every bot `addbot ... skill` didn't set: reaction time, turn rate, aim error, memory and hearing. |
| `vc_bot_vehicles` | 1 | United Offensive: 0 stops bots getting into vehicles. |

Bots never take a private slot (`sv_privateClients`). `addbot` can fill
the public slots; the floor leaves one free for a joining human.
`removebot` lowers the count `addbot` and `--bots` asked for, but
`vc_bot_minplayers` still tops the server up to its floor.

```
rcon addbot 4
rcon addbot axis 2
rcon set vc_bot_minplayers 8
rcon removebot NadeSpammer
rcon listbots
```

## Controls

Click to capture the mouse, Esc to release it.

### Fly mode

| Input | Action |
|---|---|
| W / A / S / D | Move |
| Space / Ctrl | Up / down |
| Shift | Speed boost |
| Scroll | Fly speed |

### Playing

The stock `config_mp.cfg` binds, with the sight on the right mouse button as
`+speed`. `bind MOUSE2 "toggle cl_run"` makes the sight a toggle.

| Input | Action |
|---|---|
| W / A / S / D | Move |
| Space | Stand up; jump when standing |
| C / Ctrl | Crouch / prone |
| Q / E | Lean (held) |
| LMB | Fire (semi-autos fire once per click) |
| RMB | Aim down the sight (held) |
| R | Reload |
| Shift | Melee |
| F | Use: pick up, mount an MG, plant, defuse, respawn |
| 1 / 2 / 3 / 4 | Primary, second primary, pistol, grenade |
| Scroll | Next / previous weapon |
| Tab | Scoreboard (held); Page Up / Page Down or the wheel scroll a long one |
| T / Y | Chat to everyone / your team |
| V | Quick message menu |
| M | Script menu: team, or weapon once you have a team |
| Esc | Script menu, as retail; its Main Menu tab opens the main menu (Back to Game, Disconnect, Quit) |

A script menu works as in CoD: the cursor shows, the item under it takes
the focus, a click or its digit picks it, Tab and the arrows step the
focus, Esc closes every script menu, and the stock menu sounds play. A
menu keeps the mouse captured and moves its cursor by the mouse's motion,
as CoD does; in a window the console lets the pointer go. The
quick message menus leave the mouse and every key but their digits on the
game. The main menu takes the mouse; Esc or Back to Game
returns to the game. As a spectator, Space rises and C sinks; as on retail,
E (lean right) sinks at 512 and Q (lean left) rises at 256.

### Walk mode

| Input | Action |
|---|---|
| W / A / S / D | Move |
| Space | Jump (no autohop) |
| Ctrl | Crouch (held) |
| Z | Toggle prone |
| Q / E | Lean |
| Shift | Slow walk |
| LMB / RMB | Fire / aim down sights |
| R | Reload |
| 1-7 | colt, thompson, mp40, mp44, enfield, kar98k, scoped kar98k |

### Console

`` ` `` or `~` toggles it; while it's down the game gets no keys. Up / Down
walk history, Tab completes, Page Up / Page Down scroll. In game, a line
without a leading `/` or `\` is chat, as in retail. `;` separates commands.

| Command | Does |
|---|---|
| `connect <ip:port>`, `disconnect`, `reconnect`, `quit` | What they say |
| `say`, `say_team` | Chat |
| `cmd <text>` | Send a raw client command |
| `rcon <command>` | Run a server console command with `rconPassword`; goes to the connected server, else to `rconAddress` (`ip[:port]`, port 28960 by default). The reply prints in the console |
| `rcon login <password>`, `rcon logout`, `rcon host <ip[:port]>` (CoD2, CoD4) | CoD2 1.3 and CoD4 1.7 have no `rconPassword` or `rconAddress`: log in once, then `rcon <command>` goes to the server joined, else to the host. The server's password is `rcon_password` (`--set rcon_password=...`; vcod-server also takes `rconPassword`) |
| `bind`, `unbind`, `unbindall`, `bindlist` | Binds |
| `set`, `seta`, `toggle`, `<cvar> [value]` | Cvars; `seta` also saves |
| `cvarlist`, `cmdlist`, `echo`, `clear` | The usual |
| `record [name]`, `stoprecord` | Record a demo to `demos/` in the game directory |
| `demo <name>` | Play one back; `.dm_1`, `.dm_2` or `.dm_3`, found without the extension. On CoD1 `timedemo 1` times it (CoD2 and CoD4 take `timedemo <name>`, next row), `cl_freezeDemo 1` pauses it, `nextdemo` holds a command to run when it ends |
| `timedemo <name>` (CoD2, CoD4) | On a CoD2 or CoD4 install `timedemo` is a command, as retail's: it plays `<name>.dm_1` 50 ms a frame and prints the frame count and fps. CoD4's demos live in `main/demos/`, and `timedemo 1` there looks for `demos/1.dm_1`, as 1.7 does |

Anything else goes to the server while connected (`callvote`, `kill`,
`follownext`). Bindable commands and key names are retail's (`+attack`,
`+speed`, `weaponslot pistol`, `MOUSE1`, `MWHEELUP`). The client cvars are
`name`, `cl_run`, `sensitivity`, `m_yaw`, `m_pitch`, `m_filter`,
`cl_freelook` (0 moves with the mouse's vertical motion unless `+mlook` is
held; `m_forward`, and `m_side` under `+strafe`), `cg_fov`
(cheat-protected, so 80 unless the server runs `sv_cheats 1`), `rate`
(25000; retail's first-run 5000 starves snapshots), `snaps`,
`scr_conspeed`, `mss_volume`, `r_mode` / `r_fullscreen` (applied at start
and by `vid_restart`, which also reloads the menus closed; see
[Window and fullscreen](#window-and-fullscreen)), `com_maxfps`
(retail's frame cap, `timedemo` included: `1000 / com_maxfps` whole ms a frame; vcod defaults to 0, no cap past vsync, where retail defaults to 85, about 90 fps),
`password`, `rconPassword` and `rconAddress` (neither saved), and the browser's `ui_netSource`,
`ui_joinGametype` and `ui_browserShow*`. `exec <file>` runs a config from the paks or `main/`,
and `ui_load` reloads the menus off the list `ui_menuFiles` names.
A config an older vcod wrote loads with each cvar still at an old vcod default (`r_aaSamples 4`, `name vcod`) reset to the current one; any other value stays, and to keep an old default, set it again (`set r_aaSamples 4`).
Binds only act while connected.

### Window and fullscreen

- `r_fullscreen 0` is a window, `1` a borderless window over the monitor
  the window is on, at the desktop's resolution, `2` exclusive fullscreen:
  the monitor switches to its mode closest to the `r_mode` size (the
  desktop's size when `r_mode` names none). Wayland has no exclusive
  mode, so `2` is borderless there. Alt+Enter flips between windowed and
  `1` and runs `vid_restart`.
- `r_mode` picks retail's 13 sizes (0 is 320x240, 3 640x480, 6 1024x768,
  11 856x480, 12 1920x1200). `r_mode -1` takes `r_customwidth` x
  `r_customheight`, or keeps the window's own size while either is 0,
  vcod's default: `r_customwidth 3440; r_customheight 1440; r_mode -1;
  vid_restart` asks for a 21:9 window or, with `r_fullscreen 2`, that mode.
- `r_aaSamples` is the scene's MSAA, 1 (none) or 4 (any value above 1),
  latched to `vid_restart`. The default is retail's 1 in every game
  (until 2026-10-29 CoD1's was 4); an old config's 4 loads as 1.
  Blended smoke costs about four times the fill at 4.
- `vc_widescreen 1` (default) draws a window wider than 4:3 Hor+: `cg_fov`
  and the zoom fovs are a 4:3 view's width, so 16:9 at `cg_fov 80` is
  about 96 degrees wide and 21:9 about 111, as tall as 4:3, and the HUD's
  side pieces sit at the window's edges. `vc_widescreen 0` keeps retail's
  horizontal fov (a wider window sees less above and below) and the whole
  HUD in the centred 4:3 box. Menus draw in that box either way.

- CoD4: `r_aspectRatio` (`auto`, `standard`, `wide 16:10`, `wide 16:9`;
  `vid_restart`) is the aspect the view is drawn at and stretched over
  the window, as 1.7 draws it. On an ultrawide display `vc_windowAspect 1`
  draws at the window's own aspect instead, wider at the sides, the HUD
  unstretched with its compass, score and weapon info at the screen's
  edges, the scoreboard centred and the XP bar across the bottom.

### Main menu and browser

Mouse to pick, double-click or Enter to join, the arrows, Home, End and
Page Up / Down with the cursor over the list to move the selection, the wheel or the scroll bar to
scroll, Esc to go back. The wheel scrolling a list is vcod's; CoD's wheel
scrolls none. Tab and the arrows no list takes move the menu's focus for
as long as the key is down, as CoD's do. Held keys do not repeat in a
menu, except typing and Backspace in a text field. Enter and the middle
button act on the item under the cursor, as CoD's do. Click Source to cycle Local, Internet and Favorites, and Game Type
to cycle All and the installed game types; right-click steps either back.
On a bind, click or Enter, then press the new key (Esc cancels, Backspace
clears); a third key replaces both of the old ones. Click a text field to
type into it; Enter or Esc finishes, Tab and the up and down arrows move
to the next field. In Start New Server, click Game
Type to cycle it (right-click goes back); the map list scrolls with its bar,
and with the cursor over it the arrows, Home, End and Page Up / Down move
the selection. The Mods list works the same way.

### Everywhere

| Input | Action |
|---|---|
| `` ` `` / ~ | Console |
| F3 | Debug overlay |
| F4 | Culling: on, locked, off |
