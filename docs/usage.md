# Using vcod

Command lines, controls, the console and the menus, for all four games.
New here: [getting-started.md](getting-started.md) is shorter, with the
install layout, a quick start per game and recipes. Bots: [bots.md](bots.md). What each
game supports: [docs/features/](features/) ([CoD1](features/cod1.md),
[UO](features/uo.md), [CoD2](features/cod2.md), [CoD4](features/cod4.md)).

- [Install and game](#install-and-game)
- [Client](#client)
- [CD keys](#cd-keys)
- [Server](#server)
- [Controls](#controls), [console](#console), [window](#window-and-fullscreen), [menus](#main-menu-and-browser)
- Per game: [CoD 1.1 and 1.5](#cod-11-and-15), [United Offensive](#united-offensive), [CoD2](#cod2), [CoD4](#cod4)
- [Debug and developer modes](#debug-and-developer-modes)

## Install and game

Both binaries share these flags.

| Flag | Does |
|---|---|
| `--game-dir <dir>` | The install. Default: `$COD_DIR`, else the executable's directory. |
| `--game cod1\|cod2\|cod4` | The install's game. Default: detected. `zone/` or `iw3mp.exe` is CoD4, `main/iw_*.iwd` or `CoD2MP_s.exe` CoD2, anything else CoD1. |
| `--mod-dir uo` | United Offensive: `uo/` searched over the `main/` beside it, as UO does. Default `main`. |
| `--vanilla` | CoD1 and UO: only the patch's stock paks (`pak0`..`pak8` on 1.1, `pak0`..`pakf` on 1.5, UO's `pakuo*` and `uomappack00`, and their `localized_*` paks). Every other pk3 in `main/` and `uo/` is ignored. |

`COD_DIR` is the only variable the binaries read. `COD2_DIR` and
`COD4_DIR` point tests and tools at those installs.

With `--vanilla` the client downloads nothing: a server on a non-stock
map fails the load with a popup naming its paks, and one that only adds
files plays with the stock ones. `fs_game` is ignored (the Mods menu
lists none), Start New Server starts a `--vanilla` server, `vc_vanilla`
reads 1 and the start-up log says `(vanilla)`. The server's `--vanilla`
lists only stock paks in `sv_paks` and `sv_referencedPakNames`, will not
load a third-party map and refuses `--set fs_game`.

## Client

| Command | Does |
|---|---|
| `vcod` | Main menu and server browser, in the install's game. |
| `vcod --connect <ip:port>` | Join through the server's menus. |
| `vcod <map>` | Fly around a map offline. Map names are case-insensitive. |
| `vcod --list` | CoD1: every `.bsp` in the search path. |

| Flag | Does |
|---|---|
| `--team <allies\|axis\|autoassign\|spectator>` | Answer the team menu without showing it, for `--connect` and every console `connect`. |
| `--weapon <file>` | CoD1, UO, CoD2: answer the weapon menu, e.g. `kar98k_mp`. If the menu refuses it, you pick by hand. |
| `--class <response>` | CoD4: answer the class menu, e.g. `assault_mp,0`. |
| `--fs-game mods/<name>` | CoD4: load a map under a mod. A joined server's `fs_game` wins. |
| `--debug-overlay` | Start with the F3 overlay up. |
| `--rtx-on` | Turn on every beyond-retail extra for this run, unsaved: `vc_bicubic`, `vc_bloom`, `vc_shadows`, `vc_ssao`, `vc_sunShafts`, `vc_softParticles`, `vc_hdr`, `vc_grade`, `vc_vignette`, `vc_grain`. No rays are traced. |
| `--no-audio` | Run silent. |
| `--volume <0..1>` | Master volume (`mss_volume`, default 0.8, kept in the config). |
| `--window WxH` | A window of that size for this run, over `r_mode` and `r_fullscreen` (a `vid_restart` keeps it). Sets no cvar. |
| `--exec "<line>"` | A console line to run once the window is up. Repeatable, in order. |
| `--exec-in-game "<line>"` | As `--exec`, after the first frame that draws a game: `--connect ADDR --exec-in-game "record x"` records from the join. |
| `--exec-after "<secs> <line>"` | A queue of console lines, each `secs` after the one before. |

Start New Server in the menu runs `vcod-server` from beside `vcod` and
joins it; build both. The client stops that server when it exits. vcod
has no listen server.

## CD keys

Many public servers check CD keys, so dig out the jewel case. Until the game's authorize server has
seen your key, such a server answers `needcdkey` and vcod shows the
game's own "key not valid" or "awaiting key authorization" message.

Give vcod the key one of two ways:

- `VCOD_CDKEY="XXXX-XXXX-XXXX-XXXX"`: the key's 16 characters (dashes,
  spaces and case don't matter; anything past the 16th is ignored), for
  whichever game the install is.
- CoD 1.1 and 1.5 only: the retail file `main/codkey`, which the 1.1
  installer writes. 1.5, UO, CoD2 and CoD4 keep the key in the Windows
  registry, which vcod does not read; use the variable there.

With a key set, before each challenge to a server off the LAN, vcod sends
what the retail client sends to the game's authorize host:

| Game | Host | Sent |
|---|---|---|
| CoD 1.1, 1.5 | `codauthorize.activision.com:20500` | the key |
| UO | `coduoauthorize.activision.com:20600` | the key |
| CoD2 | `cod2master.activision.com:20700` | the key and its MD5 |
| CoD4 | `cod4master.activision.com:20800` | the key and its MD5 |

Without a key nothing is sent. vcod never logs, prints or stores the key.
A server that checks no keys (LAN, vcod-server, most local retail
servers) joins without one. vcod-server checks no keys.

## Server

```
vcod-server <map> [--port 28960] [--hostname vcod] [--max-clients 8] [--gametype <gt>]
```

| Flag | Does |
|---|---|
| `--port <n>` | UDP port, default 28960. Binds `0.0.0.0`. |
| `--hostname <name>` | `sv_hostname`, default `vcod`. |
| `--max-clients <n>` | `sv_maxclients`, default 8, at most 64. Bots count. |
| `--gametype <gt>` | The script under `maps/mp/gametypes/`. Default `dm` (CoD4 `war`). |
| `--set NAME=VALUE` | Retail's `+set`, repeatable. |
| `--bots <n>`, `--bots-shoot` | Start with `n` bots; without `--bots-shoot` they never fire ([bots.md](bots.md)). |
| `--ban-file <path>` | Keep `banUser`/`banClient` bans across runs. |
| `--gametype-script <file>` | CoD1, UO, CoD2: run a gametype from disk instead of the paks. |
| `--script-dir <dir>` | CoD4: loose `.gsc` files read over the zones' scripts each level, as retail reads a mod's. |
| `--vanilla` | See [Install and game](#install-and-game). |

The server answers `getstatus` from anyone, as retail does. Keep it on a
LAN or behind a firewall you control. There is no stdin console: admin
goes through rcon, so set `rconPassword` before you need it.

### `--set`

Each `--set` is applied once, before the scripts load. One written after
the map (`vcod-server mp_carentan --set ...`) runs again once the level
is up, as retail's `+map x +set ...` does, so it outranks a script's
load-time `setCvar`. A later level never puts it back.

Common ones:

| Cvar | Does |
|---|---|
| `rconPassword` | Turns rcon on (refused until set). CoD2 and CoD4 call it `rcon_password`; either name works. |
| `g_password` | Private server, checked at connect, `map_restart` and map change. |
| `sv_privateClients`, `sv_privatePassword` | Reserve the first N slots for clients that send that password. |
| `sv_mapRotation` | The map cycle `map_rotate` walks. |
| `fs_game` | Run a mod: its paks go over `main/` and clients are told to load it. CoD4: `mods/<name>` (below). |
| `dedicated` | 1 by default (retail's is 2), so dev runs stay off the master list. 2 sends heartbeats (below). |
| `protocol` | CoD1: `1`, `6` or `22` overrides the install's protocol. |
| `g_log`, `g_logSync` | `games_mp.log` (below): empty `g_log` turns it off, `g_logSync 1` flushes every line. |
| `fs_homepath` | Where the log goes. Default the `--game-dir`. |
| `sv_allowDownload` | On by default: serve the server's non-stock paks to a client that lacks them. `sv_wwwDownload 1` with `sv_wwwBaseURL` redirects instead. |
| `sv_pure` | CoD2, CoD4: 0 turns the pure `cp` check off. |

### Log

`games_mp.log`, retail's format, goes to `<home>/main/` (`<home>/uo/` on
UO, `<home>/<fs_game>/` under a mod), `<home>` being `--game-dir` unless
`fs_homepath` says otherwise. All four games write it.

### Master server

At `dedicated 2` the server sends a heartbeat to `master.cod.pm` every
three minutes, at most one packet a second to a master host:

| Game | Master |
|---|---|
| CoD1 | `master.cod.pm:20510` |
| UO | `master.cod.pm:20610` |
| CoD2 | `master.cod.pm:20710` |
| CoD4 | `master.cod.pm:20810` (`sv_master1` overrides it) |

CoD1, UO and CoD2 send a flatline on Ctrl-C or `quit`.

### Bans

`--ban-file` keeps bans in retail's `ban.txt` shape: a guid and the
cleaned name a line. An off-LAN client's guid is its IPv4 address as a
signed integer (CoD4: the client's GUID). A file in vcod's older
`<ip> <name>` shape is rewritten into guid lines the first time the
server opens it, after a copy is kept as `<path>.old`. Without
`--ban-file` a ban lasts until the process exits.

### rcon

From any client's console (`rcon <command>` after `rconPassword <pw>`,
or `rcon login <pw>` on CoD2 and CoD4) or a retail one.

| Game | Commands |
|---|---|
| all | `status`, `serverinfo`, `systeminfo`, `dumpuser`, `say`, `kick`, `clientkick`, `banUser`, `banClient`, `unbanUser`, `tempBanUser`, `tempBanClient`, `map`, `devmap`, `map_restart`, `map_rotate`, `set`, `seta`, `heartbeat`, `quit`, `addbot`, `removebot`, `listbots` |
| CoD1, UO | `killserver`, `cvarlist`, `gamecompletestatus`, `scriptusage`, `stringusage`, `setdrawfriend`, `setfriendlyfire`, `setkillcam`, a bare cvar name to read it |
| CoD2 | `tell`, `onlykick`, `fast_restart`, `sets`, `kickbot`, a bare cvar name |
| CoD4 | `tell`, `onlykick` (`kick all` too), `fast_restart`, `killserver`, `sets`, `setu`, `toggle`, `togglep`, `reset`, `setfromdvar`, `vstr`, `exec`, `wait`, `writeconfig`, `dvar_bool`, `dvar_int`, `dvar_float`, `setdvartotime`, `touchfile`, `cmdlist`, `kickbot` |

`devmap` sets `sv_cheats 1` after the load and `map` sets it back.

### Bots

`--bots <n> [--bots-shoot]`, rcon's `addbot`, `removebot` and
`listbots`, and the `vc_bot_*` cvars (`vc_bot_minplayers`,
`vc_bot_autobalance`, `vc_bot_shoot`, `vc_bot_names`, `vc_bot_skill`,
`vc_bot_vehicles`, UO only) work alike on all four games. The guide is
[bots.md](bots.md).

### Other cvars

- `scr_motd` is the message of the day: on the team menu (1.1) or the
  server info screen before it (1.5, UO), in vcod and the retail clients.
  `g_motd` reaches the client as configstring 14 (UO 15), which no retail
  client draws.
- `sv_maxclients` is latched: `rcon set sv_maxclients 32` then `rcon
  map_restart`.

## Controls

Click to capture the mouse, Esc to release it.

### Playing

The stock `config_mp.cfg` binds, the sight on the right mouse button as
`+speed`. `bind MOUSE2 "toggle cl_run"` makes the sight a toggle. CoD2
and CoD4 use their own games' default binds.

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

A script menu works as in the game: the cursor shows, the item under it
takes the focus, a click or its digit picks it, Tab and the arrows step
the focus, Esc closes every script menu, and the stock menu sounds play.
A menu keeps the mouse captured and moves its cursor by the mouse's
motion; in a window the console lets the pointer go. The quick message
menus leave the mouse and every key but their digits on the game. As a
spectator, Space rises and C sinks; as on retail, E (lean right) sinks at
512 and Q (lean left) rises at 256.

### Fly mode

| Input | Action |
|---|---|
| W / A / S / D | Move |
| Space / Ctrl | Up / down |
| Shift | Speed boost |
| Scroll | Fly speed |

### Everywhere

| Input | Action |
|---|---|
| `` ` `` / ~ | Console |
| F3 | Debug overlay |
| F4 | Culling: on, locked, off |
| Alt+Enter | Windowed / borderless fullscreen |

## Console

`` ` `` or `~` toggles it; while it's down the game gets no keys. Up /
Down walk history, Tab completes, Page Up / Page Down scroll. In game, a
line without a leading `/` or `\` is chat, as in retail. `;` separates
commands.

| Command | Does |
|---|---|
| `connect <ip:port>`, `disconnect`, `reconnect`, `quit` | What they say |
| `say`, `say_team` | Chat |
| `cmd <text>` | Send a raw client command |
| `rcon <command>` | CoD1, UO: run a server command with `rconPassword`, on the connected server, else on `rconAddress` (`ip[:port]`, port 28960 by default). The reply prints in the console |
| `rcon login <pw>`, `rcon logout`, `rcon host <ip[:port]>` | CoD2, CoD4: these have no `rconPassword` or `rconAddress`. Log in once; then `rcon <command>` goes to the server joined, else to the host |
| `bind`, `unbind`, `unbindall`, `bindlist` | Binds |
| `set`, `seta`, `toggle`, `<cvar> [value]` | Cvars; `seta` also saves |
| `cvarlist`, `cmdlist`, `echo`, `clear` | The usual |
| `exec <file>` | Run a config from the paks or `main/` |
| `ui_load` | Reload the menus off the list `ui_menuFiles` names |
| `record [name]`, `stoprecord` | Record a demo to `demos/` in the game directory (the mod's first) |
| `demo <name>` | Play one back (`.dm_1`, `.dm_2` or `.dm_3`, found without the extension). `cl_freezeDemo 1` pauses it; `nextdemo` holds a command to run at its end |
| `timedemo` | CoD1, UO: `set timedemo 1` before `demo` times it. CoD2, CoD4: `timedemo <name>` is a command, as retail's: it plays `<name>.dm_1` 50 ms a frame and prints the frame count and fps. CoD2 also writes `demos/timedemo_<map>_mode_<r_mode>.csv`, a `<frame>,<ms>` line a frame |

Anything else goes to the server while connected (`callvote`, `kill`,
`follownext`). Bindable commands and key names are retail's (`+attack`,
`+speed`, `weaponslot pistol`, `MOUSE1`, `MWHEELUP`). Binds act only
while connected.

Client cvars:

| Cvar | Notes |
|---|---|
| `name`, `cl_run`, `sensitivity`, `m_yaw`, `m_pitch`, `m_filter`, `m_forward`, `m_side` | As retail |
| `cl_freelook` | 0 moves with the mouse's vertical motion unless `+mlook` is held (`m_forward`; `m_side` under `+strafe`) |
| `cg_fov` | Cheat-protected: 80 unless the server runs `sv_cheats 1` |
| `rate`, `snaps` | `rate` defaults to 25000; retail's first-run 5000 starves snapshots |
| `com_maxfps` | Retail's frame cap, `timedemo` included (`1000 / com_maxfps` whole ms a frame). vcod's default is 0, no cap past vsync; retail's is 85 |
| `scr_conspeed`, `mss_volume` | Console slide speed, master volume |
| `r_mode`, `r_fullscreen`, `r_aaSamples`, `vc_widescreen` | [Window and fullscreen](#window-and-fullscreen) |
| `password`, `rconPassword`, `rconAddress` | The last two are not saved |
| `ui_netSource`, `ui_joinGametype`, `ui_browserShow*` | The browser's filters |

The config is `vcod_mp.cfg` (CoD1 and UO in `main/` or `uo/`, CoD2 and
CoD4 in the active profile). A config an older vcod wrote loads with
each cvar still at an old vcod default (`r_aaSamples 4`, `name vcod`)
reset to the current one; any other value stays. To keep an old
default, set it again (`set r_aaSamples 4`).

## Window and fullscreen

- `r_fullscreen 0` is a window, `1` a borderless window over the monitor
  the window is on, at the desktop's resolution, `2` exclusive
  fullscreen at the monitor mode closest to the `r_mode` size (the
  desktop's size when `r_mode` names none). Wayland has no exclusive
  mode, so `2` is borderless there. Alt+Enter flips between windowed and
  `1` and runs `vid_restart`.
- `r_mode` picks retail's 13 sizes (0 is 320x240, 3 640x480, 6 1024x768,
  11 856x480, 12 1920x1200). `r_mode -1` takes `r_customwidth` x
  `r_customheight`, or keeps the window's own size while either is 0
  (the default). `r_customwidth 3440; r_customheight 1440; r_mode -1;
  vid_restart` asks for a 21:9 window or, with `r_fullscreen 2`, that
  mode.
- `vid_restart` applies `r_mode` and `r_fullscreen` and reloads the
  menus closed.
- `r_aaSamples` is the scene's MSAA, 1 (none, the default) or 4 (any
  value above 1), latched to `vid_restart`. Blended smoke costs about
  four times the fill at 4.
- `vc_widescreen 1` (default) draws a window wider than 4:3 Hor+:
  `cg_fov` and the zoom fovs are a 4:3 view's width, so 16:9 at
  `cg_fov 80` is about 96 degrees wide and 21:9 about 111, and the HUD's
  side pieces sit at the window's edges. `vc_widescreen 0` keeps
  retail's horizontal fov and the whole HUD in the centred 4:3 box.
  Menus draw in that box either way.
- CoD4 instead: `r_aspectRatio` (`auto`, `standard`, `wide 16:10`,
  `wide 16:9`; `vid_restart`) is the aspect the view is drawn at and
  stretched over the window, as 1.7 does. `vc_windowAspect 1` draws at
  the window's own aspect, for ultrawide displays: wider at the sides,
  the HUD unstretched with its compass, score and weapon info at the
  screen's edges.

## Main menu and browser

- Mouse to pick, double-click or Enter to join. With the cursor over a
  list, the arrows, Home, End and Page Up / Down move the selection; the
  wheel or the scroll bar scrolls it. The wheel is vcod's: the games'
  wheel scrolls no list, and I wasn't going to copy that.
- Tab and the arrows no list takes move the menu's focus while the key
  is down. Held keys do not repeat, except typing and Backspace in a text
  field. Enter and the middle button act on the item under the cursor.
- Esc goes back.
- Click Source to cycle Local, Internet and Favorites, and Game Type to
  cycle All and the installed game types; right-click steps either back.
  Internet lists `master.cod.pm`'s servers for the install's game.
- A bind: click or Enter, then press the new key (Esc cancels, Backspace
  clears); a third key replaces both old ones.
- A text field: click to type; Enter or Esc finishes, Tab and the up and
  down arrows move to the next field.
- Start New Server: click Game Type to cycle it (right-click goes back);
  the map list and the Mods list scroll and select as a server list does.

## Per game

### CoD 1.1 and 1.5

- The client joins 1.1 (protocol 1) and 1.5 (protocol 6) servers. It
  connects with its install's protocol and switches when refused.
- A 1.5 install works as the client's asset source for 1.1 servers, but
  its `pak0` differs from 1.1's, so a pure 1.1 server drops it.
- The server speaks its install's protocol only: 1.1, or 1.5 when
  `main/pak0.pk3` is 1.5's. `--set protocol=` overrides it.
- Bots, rcon and the cvars above all apply.

### United Offensive

- Add `--mod-dir uo` to every command, with `--game-dir` (or `COD_DIR`)
  at the install that holds 1.5's `main/` and UO's `uo/`.
- `vcod --mod-dir uo` opens UO's menu; `vcod mp_cassino --mod-dir uo`
  flies a UO map; `--connect` plays a UO server, vehicles included.
- A UO client refuses 1.1 and 1.5 servers, and a 1.1 or 1.5 client a UO
  one, each naming the flag to use.
- `vcod-server <map> --mod-dir uo` speaks UO (protocol 22) and runs UO's
  gametypes (`ctf`, `dom`, `hq`, `bas` beside the 1.5 ones) and vehicles.
  `vc_bot_vehicles 0` keeps bots out of vehicles.

### CoD2

Client:

- Bare `vcod` opens CoD2's stock main menu. Its browser lists LAN
  servers, `master.cod.pm`'s CoD2 list (protocol 118) and favourites.
  Start New Server runs `vcod-server --game cod2`. Esc in a game opens
  the server's `ingame` menu.
- `vcod --connect ip:port` joins a CoD2 1.3 server, pure or not, through
  its team and weapon menus, or spectates. `--team` and `--weapon`
  (e.g. `thompson_mp`) answer them. X or Esc's menu picks another weapon
  for the next spawn. A map change loads the new map.
- `vcod <map>` flies a map.
- Other modes (`--walk`, `--list`) exit with an error.

Server (`vcod-server <map> --game cod2`, or a CoD2 install):

- Runs the stock gametype scripts (`--gametype` or `--set g_gametype=`;
  `--gametype-script` as on CoD1). Players join through the team and
  weapon menus, move, fight, chat and vote.
- Checks the pure `cp` (`sv_pure 0` turns it off). Takes `g_password`,
  `sv_privateClients` with `sv_privatePassword`, `sv_minPing`,
  `sv_maxPing`, `sv_maxRate`, `sv_timeout` and `sv_floodProtect` as
  retail does.
- Serves a non-stock iwd in its `main/` to a client that lacks it.
- Runs the cheat commands under `sv_cheats 1`.
- Bots play every stock gametype ([bots.md](bots.md#cod2)).

### CoD4

Client:

- Bare `vcod` opens 1.7's main menu from the `ui_mp` zone (mouse,
  arrows, Tab, Enter, Esc). The first start asks for a profile.
  - Start New Server runs `vcod-server --game cod4` and, with Dedicated
    No, joins it.
  - Join Game lists LAN servers (Source: Local), favourites and, on
    Refresh List, `master.cod.pm`'s CoD4 list. Password sets a server's
    password. CoD4x servers stay out of the lists unless
    `ui_browserShowCoD4x` is 1 (vcod's own cvar).
  - Mods runs a `mods/<dir>` mod. Options and Controls set the options
    and binds vcod has.
  - Select Profile creates, deletes and loads a profile in
    `players/profiles/`. The active profile's `vcod_mp.cfg` is the
    config, its `vcod_mpdata` the stats a join sends and the main menu
    reads (rank, unlocks, the Create a Class picks), stored plain and
    written back as they change. Retail's `mpdata` is left alone. The
    custom classes' names are the archived cvars `customclass1`..`5`.
  - `connect ip:port` on the console joins a server. Esc or `disconnect`
    on the connect screen calls the join off.
- `vcod --connect ip:port` joins a 1.7 server through its team and class
  menus, drawn as the zones give them. The mouse or the arrows and Tab
  move the focus, a click or Enter picks, Esc runs the menu's `onEsc` or,
  with none up, the server's `g_scriptMainMenu`. A menu vcod cannot
  evaluate shows as a numbered list (1-9 pick). `--team` and `--class`
  answer them unseen.
- `vcod <map>` flies `zone/english/<map>.ff`. A map missing from
  `zone/<language>/` loads from `usermaps/<map>/<map>.ff`; under a mod
  that directory's iwds load too.
- Joining downloads the mod and usermap files the server references and
  the install lacks, into the install at the server's path. With
  `cl_allowDownload 0` the join is refused instead.
- Other modes exit with an error.

Server (`vcod-server <map> --game cod4`, or a CoD4 install):

- Loads the map's zone and the five common ones, runs `default_mp.cfg`
  from the iwds under the `--set`s, then the gametype's scripts (`war`
  unless `--gametype` or `--set g_gametype=` says otherwise; the `--set`
  wins).
- Takes the stats and the pure `cp` as retail does (`sv_pure 0` turns
  the check off).
- Clients answer the team and class menus, spawn with their class's
  loadout, play, and call and cast votes (`g_allowVote 0` turns votes
  off). A match that ends rotates to the next map; `server_map.cfg`
  fills `sv_mapRotation` with the stock maps.
- `fs_game mods/<name>`: the mod's `mod.ff` loads after the common zones,
  its iwds go over `main/`'s (a usermap's between them), and
  configstrings 0 and 1 name it, as retail does. A client lacking the
  mod's files downloads them.
- `g_inactivity <secs>` drops a player idle that long.
- Bots: [bots.md](bots.md#cod4).

## Debug and developer modes

- `vcod <map> --walk` (CoD1): an early offline walk mode. WASD, Space
  jump, Ctrl crouch, Z prone, Q / E lean, Shift slow walk, mouse fire and
  sight, R reload, 1-7 pick a weapon.
- `vcod --net-probe <ip:port>` is a headless client that joins, prints a
  line a second and writes captures. `vcod --help` documents its modes.
  `--probe-secs` sets how long it stays (default 65). On CoD2
  `--probe-team` / `--probe-weapon` make it join and play a scripted
  loop; `VCOD_COD2_RECORD=<dir>/<name>` records the run as
  `<name>.dm_1`. On CoD4 `--team` and `--class` make it join.
- `vcod --game cod2 --probe-demo <file.dm_1> [--probe-timedemo]` plays a
  CoD2 demo headless and prints what it parsed and drew.
- `vcod --game cod4 --capture <file>` replays a `cod4_probe` capture
  (`crates/cod4/tests/fixtures/net/play-1.7-*.txt`) on its own clock.
- F3 (`--debug-overlay`) shows frame, draw, vis, net and audio counters;
  F4 cycles culling on, locked (the visible set frozen) and off.
- `VCOD_NETSIM="ping=100,jitter=20,loss=2"` gives the client a bad link.
- `VCOD_FRAME_PROFILE=<path>` writes each frame's CPU and GPU time by
  stage.
- Server: `--trace` logs every snapshot per client and a per-second tick
  summary; `--test-entities <n>` adds invisible moving entities to
  exercise the entity encoding; `VCOD_TICK_TRACE=<path>` writes the
  tick's step log; `VCOD_TICK_PROFILE_MS=<ms>` logs every tick slower
  than that.
- `RUST_LOG` sets the log level (default `info`); `WGPU_BACKEND` narrows
  the graphics backend.
