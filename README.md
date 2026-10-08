# vcod

A from-scratch reimplementation of bits of Call of Duty (2003), patch 1.1,
written in Rust. It has a map viewer, a client that spectates and plays on
real 1.1 servers, and a dedicated server that a retail 1.1 client can join and
play on. It reads the game's own pk3 archives and speaks the original 1.1 wire
protocol. Nobody ever wrote that protocol down, so I recovered it from the
binaries.

![mp_pavlov in fly mode](docs/screenshots/fly-mp_pavlov.jpg)

![Walk mode with the kar98k viewmodel](docs/screenshots/walk-mp_pavlov.jpg)

![Spectating a public TDM server](docs/screenshots/spectate.jpg)

## Read this first: it's a toy

vcod is a hobby project. I build it because poking at a 2003 game engine is
fun, and I stop when something stops being fun.

- **It is not a playable game.** You can run around, shoot people and plant a
  bomb, and some evenings it feels close. It is still a pile of research
  scaffolding with a renderer on top. Expect missing pieces, rough edges and
  the occasional soldier stuck in a wall.
- **Retail feature parity is not a goal.** There is no roadmap to a "1.0" and
  no plan to cover everything CoD 1.1 does. If vcod ends up fully playable one
  day, that will be an accident and I'll take the credit.
- **It will not replace your copy of the game.** You need that copy to run
  vcod at all.
- **Don't run it as a public server for real players.** It has no
  anti-cheat and its rcon knows a handful of commands. It has a lot of
  opinions about the order in which a tick runs. Unlike retail it does not
  heartbeat the Activision master unless you pass `--set dedicated=2`.

If you want to play Call of Duty, play Call of Duty. If you want to see how
it works under the hood, or watch a 2003 game boot inside a window someone
built from the bytes up, this might be your kind of thing.

## What it does

### Map viewer

- Opens any stock or custom map from an installed copy of the game and draws
  it with textures, lightmaps and props. Skies, water, fences, foliage and
  terrain blends go through the maps' own Q3-style shader scripts, with the sun
  disc and the map's fog.
- Visibility follows the retail cells, portals and occluders, with F4 to
  freeze or disable culling and fly out to see what the camera was drawing.
- The map's ambient sound loop plays.

### Walk mode (`--walk`), offline

- Spawns you as a soldier on a spawn point with the retail movement code:
  gravity, crouch, prone, stepping, leaning, wall sliding.
- Six weapons with their own viewmodels, xanim clips, sounds and reserve ammo.
  Shots are hitscan with per-surface impact effects and tracers.
- Footsteps follow retail's cadence per surface, and the landing sound picks
  its alias from fall speed.

### Client (`--connect`)

- Joins a CoD 1.1 server: handshake, Huffman coding, netchan, delta
  snapshots, the lot. It downloads any pak the server references and the
  install lacks, the way the retail client does.
- Answers the stock team and weapon menus, either through a simple list
  built from the game's own `.menu` files or from `--team` and `--weapon`.
- **Spectates.** Every player is an assembled soldier playing the animations
  the server drives. Kill feed, chat, scoreboard, sounds, tracers, impacts and
  muzzle flashes come from the same events the retail client reads.
- **Plays.** Move, jump, crouch, go prone, lean, fire, aim down the sight,
  reload, melee, use and switch weapons, on retail's default binds. It sends
  a usercmd every 8 ms, the way a 125 fps retail client does.
- Predicts your own movement by replaying every unacknowledged cmd through
  the same movement step the server runs, against the map and the other
  players. A correction eases out over 100 ms.
- Draws your weapon in first person with the hands your team gets, zooms the
  sight to the weapon's own FOV, puts a sniper scope's overlay up where the
  swaying, hit-kicked gun points, and plays your own fire, reload and footstep
  sounds off the prediction. The snapshot that confirms them later stays
  quiet.
- Draws mounted MG42s turned by the barrel angles the server sends, with
  their fire anim and muzzle flash. On the gun, the view rides the gun's
  `tag_player` and the HUD swaps the crosshair for the gun's reticle.
- Draws the HUD that the stock `hud.menu` lays out: crosshair that opens with
  spread, health, ammo, fire-mode icon, stance with its change flash, compass
  with objectives and teammates, use hints and hit direction, chat top left,
  game messages over the compass and announcements over the crosshair. A
  spectator following a player sees that player's HUD, weapon, zoom and
  scope. It also draws the gametype script's own HUD elements, such as the
  S&D clock, the bomb icons and the
  progress bar, in retail's fonts, fixed-width slots included.
- Follows the server through a map change: loading screen, downloads, new
  map.
- Has retail's drop-down console on `` ` `` / `~`: the log, chat and server
  prints scroll in it, and it takes `connect`, `disconnect`, `name`, `bind`
  and the rest of the commands under [Console](#console). Binds and archived
  cvars persist in `main/vcod_mp.cfg`. Losing the server or failing a load
  drops you to the console instead of closing the window.

I have played it against my own server and against the retail Linux 1.1d
dedicated server running locally. I have spectated public servers with it. I
have not joined a public server as a player, and I'd rather you didn't
either: the people on it signed up for Call of Duty.

### Dedicated server (`vcod-server`)

- Answers server browsers, accepts retail 1.1 clients, sends the gamestate
  and delta-compressed snapshots.
- Runs Activision's own gametype and map scripts on `vcod-gsc`, a virtual
  machine for CoD's `.gsc` script language that lives in this repo. Team
  menus, spawn points, scoring, round logic, time and score limits all come
  from the stock scripts, not from Rust. All five stock gametypes are
  checked against retail: `dm`, `tdm` and `sd` end to end, `re` (Retrieval)
  through a pickup, a drop, the timeout return, a drop on death and a
  capture, and `bel` (Behind Enemy Lines) through the team swap on a kill
  ([docs/research/cod11-gametypes-re-bel.md](docs/research/cod11-gametypes-re-bel.md)).
- Movement on the shared pmove, with players as capsules that block and push
  each other the way retail's do. Falls stun and hurt, scaled by the
  `bg_fallDamageMinHeight` and `bg_fallDamageMaxHeight` cvars.
- Combat: bullets trace the world, players and static props. Hits go through
  the stock damage callback with per-bone hit locations. Rifle rounds pass
  through players and every round passes through glass. Melee works, and
  grenades fly as real missiles that bounce, rest and explode with retail's
  falloff. A blast walks its victims (players and MG42s) in the order of
  retail's entity area tree, so players in a line shield each other the way
  they do there. Bullets pass through mounted MG42s, as on retail.
- Deaths leave corpses in the eight-slot body queue and drop the dead player's
  weapon. Players pick up weapons, ammo and health packs by touch or the use
  key. Dropped and spawned items fly retail's arc and land on what they hit,
  an item that lands in a `CONTENTS_NODROP` brush is freed, and an item
  flagged to respawn comes back on retail's timer.
- Search & Destroy end to end: plant, defuse, progress bar, objectives on the
  compass.
- rcon, the master heartbeat and the two-second zombie slot a dropped client
  keeps, timed against retail.
- A teammate out of view still shows on the compass, with its quick-chat
  flash, packed into the playerstate the way retail packs it.
- Mounted MG42s: mount with use, aim inside the gun's arc, fire, dismount.
- Map triggers (`trigger_multiple`, `trigger_hurt`, `trigger_use`,
  `trigger_lookat`), and script movers whose trajectories reach the wire.
  A moving brush model carries the players and items on it and shoves the
  players in its way, and a player linked to a moving entity rides it.
  `linkTo` works on script models, origins, brush models and items too,
  including to a tag on a player's model. The client draws a
  brush model where its entity is, with its baked lightmap, so a hidden or
  deleted one is gone, and draws an item resting on a mover riding it.
- Intermission, `map_restart`, and `sv_mapRotation` the way retail runs them,
  with the next map's gamestate sent on the live connection.
- Spectator follow mode and the killcam, replayed out of a ring of archived
  frames.
- `--bots` adds debug bots that join through the stock menus, pick a random
  weapon the menu and the `scr_allow_*` cvars allow, and roam the map along
  a navigation graph built from pmove runs, ladders and jumps included and
  minefields left out, heading for an
  enemy they see and toward gunfire, turret fire and blasts they hear. In
  S&D each team spreads over both bombzones, attackers plant and one
  defender defuses while the rest cover. In Retrieval attackers pick the
  objective up and carry it to its goal while defenders guard.
  `--bots-shoot` makes them fight with a reaction delay, a capped turn rate
  and aim error that settles while they hold a target, draw the pistol when
  the primary runs dry up close, and chase a lost enemy to where it was
  last seen. They are still bad at it.

### The research

The part I'd call the most useful lives in [docs/](docs/). It holds the
1.1 wire protocol in both directions, and about two dozen research documents
on file formats, movement, combat, sound, HUD, script semantics, turrets,
items and more. Every claim names the module and address it rests on and says
whether it was verified against a binary or live capture, or inferred.

## How I know it's right (when it is)

The retail 1.1d Linux dedicated server is the oracle. When vcod and retail
disagree, retail wins and the disagreement goes into a research doc.

- A headless probe client (`vcod --net-probe`) joins a server and records what
  it sees. It has a few dozen scripted modes: shoot someone, throw a grenade,
  plant the bomb, crawl prone up a hill, get stuck inside another player.
- Those recordings against retail are committed as fixtures. A/B tests replay
  the same inputs on vcod's server and diff the playerstate, snapshot by
  snapshot.
- Script semantics are measured the same way. Small `.gsc` probes run on the
  retail server and on `vcod-gsc`, and the test suite compares their output.

This catches a lot. It does not catch everything, and anything that is about
how something looks or sounds still needs a human squinting at a screen.

## What it doesn't do

The list of what retail does and vcod doesn't is longer than this. These are
the gaps you're most likely to run into.

**No front end.** No main menu, no server browser, no options screen. You get
command-line flags and the console: `connect`, binds and a few client cvars
(see [Console](#console)).

**Client**

- The HUD skips a few retail touches: the compass's spring, the stance key
  hints, the weapon name timing out after a switch and the hit icon's jitter
  ([docs/research/cod11-hud-protocol.md](docs/research/cod11-hud-protocol.md),
  section 9).
- No head icons: the server sends `iHeadIcon`, but the client draws no icon
  over a player's head, so a Retrieval carrier or a `scr_drawfriend`
  teammate looks like anyone else.
- Only protocol 1 (patch 1.1). 1.5 and United Offensive servers won't talk to
  it.
- Prediction carries you with a moving brush model you stand on but not
  with its rotation, which retail's client doesn't either; a turning mover's
  rider is corrected at each snapshot.

**Server**

- The bots don't play `bel`.
- `linkTo` on a turret stops with an error, and `enableLinkTo` is missing;
  no stock MP script uses either. An entity linked to a tag on a player's
  model sits within a few units of where retail puts it, and does not
  follow the body's swing after `setPlayerAngles`.
- A brush model that has turned and turned back keeps a sliver of yaw on
  retail, which drifts what it carries by about 0.02 units a frame; vcod's
  comes back to exactly zero.
- rcon runs `map`, `map_restart`, `map_rotate`, `status`, `clientkick`,
  `heartbeat` and `quit`; nothing else (`say`, `kick <name>`, `set`, ...).
  `status` prints a ping of 0. A heartbeat reaches the master, which probes
  back, but I haven't seen vcod listed yet.
- No anti-cheat, no PunkBuster.

**Rendering and sound**

- Props get one colour from the compiler's per-entity `lightingPrecalc` tint.
  The engine samples its light grid per vertex.
- Shadow-decal props (`shadow_tree_*`, `shadow_crate`) draw as coplanar,
  depth-biased decals on the ground.
- Shader scripts cover skies, water, blends, the sun disc and the ocean's
  `deformVertexes wave`. The other `deformVertexes` forms parse and do
  nothing, and NV/ATI hardware-path stages are dropped, as retail dropped them
  on cards without those extensions. Engine-generated `$dlight` images and the
  ship's deckflag texture have no file behind them, so a generated blob and a
  white pixel stand in
  ([docs/research/cod11-shader-scripts.md](docs/research/cod11-shader-scripts.md)).
- Visibility draws a little more than retail on purpose. Retail assumes
  nobody looks over a cell's walls, and the mp_ship decks prove otherwise.
  Outside every cell, in fly mode above the map, only the frustum culls.
- Audio follows the retail engine on paper (falloff, panning, 32/32/8 voice
  pools with priority stealing, ducking) but hasn't been checked by ear
  against the real game. Wall occlusion, about -12 dB, is a vcod addition
  retail doesn't have.
- Quick chat (`vsay`) is handled and does nothing on a stock install, same as
  retail: the `.voice` tables it looks up only ship in mods
  ([docs/research/cod11-quick-chat.md](docs/research/cod11-quick-chat.md)).

**Things that look like gaps and aren't**

- No mantling. Retail 1.1 MP doesn't have it either
  ([docs/research/cod11-mantle.md](docs/research/cod11-mantle.md)).
- No grenade cooking. Retail 1.1 MP doesn't have it either. The fuse runs
  from the release, however long you held the trigger.
- No doppler. The 1.1 engine never sets a velocity on a sound.
- Asphalt footsteps are silent in retail because the engine asks for
  `asphalt` and the sound table spells it `asphault`. vcod uses the table's
  spelling, so in vcod you can hear asphalt. I consider this my one gameplay
  improvement over Infinity Ward.

## Requirements

- A purchased, original copy of Call of Duty (2003) with patch 1.1. This
  repository contains no game data. The 1.5 patch ships the same `pak0-4.pk3`
  assets, so a 1.5 install works as the asset source; the netcode is 1.1 only.
- Rust 1.90 or newer.
- For the client, a GPU and driver with BC (DXT) texture compression. wgpu
  picks a backend (Vulkan, DX12, Metal, GL), and
  `WGPU_BACKEND=vulkan|gl|dx12|metal` narrows the choice. The server needs no
  GPU.
- Linux is the only platform I run. Windows and macOS builds exist and are
  untested.
- A default audio output device if you want sound. Without one the client
  logs a warning and runs silent.

## Building and installing

Prebuilt binaries for Linux amd64, Windows amd64 and macOS arm64 are on the
[nightly release](https://github.com/janost/vcod/releases/tag/nightly), rebuilt
from `master` on every push that touches code. It's a rolling tag: the assets
are replaced in place, so the download URLs never change and the previous
build is gone. Linux and macOS ship as `.tar.zst`, Windows as `.zip`.

To build it yourself:

```
cargo build --release
```

That gives you `target/release/vcod` (client) and
`target/release/vcod-server`. `cargo build -p vcod` or `-p vcod-server`
builds one of them.

The binaries expect to sit inside the game install, next to `CoDMP.exe`, and
read the paks from its `main/` subdirectory. Copy them there, set
`COD_DIR=/path/to/CallOfDuty`, or pass `--game-dir`. `--game-dir` beats
`COD_DIR`, which beats the executable's own directory, with the working
directory as the last resort.

### Running the tests

```
COD_DIR=/path/to/CallOfDuty cargo test
```

Without `COD_DIR`, the tests that need game data return early and report ok,
so a green run on a machine without the game proves nothing about the
parsers. The protocol tests read committed captures and run anywhere. CI runs the suite without game data, since it can't ship any.

## Usage

### The closest thing to a game

Two terminals:

```
vcod-server mp_carentan --bots 4 --bots-shoot
vcod --connect 127.0.0.1:28960
```

Pick a team, pick a weapon, go get shot by a bot.

### Client

```
vcod mp_pavlov
vcod mp_pavlov --walk
vcod --list
vcod --connect <ip:port>
vcod --connect <ip:port> --team axis --weapon kar98k_mp
vcod mp_pavlov --game-dir /path/to/CallOfDuty
```

- The first positional argument is the map name (case-insensitive). With no
  map and no `--connect`, the window opens on a full-screen console.
- `--list` prints every `.bsp` in the search path instead of opening a window.
- `--mod-dir` picks which subdirectory's pk3s to index: `main` (default) or
  `uo` for United Offensive. Only one directory mounts at a time, so a UO map
  whose art ships in `main/` shows missing textures. I've flown noville this
  way and it renders apart from that. Anything else in UO is untested.
- `--walk` starts at a player spawn point as a collidable soldier. The map
  needs a spawn entity.
- `--connect ip:port` joins a server. To find one, the master server at
  `codmaster.activision.com:20510` still answers `getservers 1 full empty`.
- `--team <allies|axis|autoassign|spectator>` answers the team menu without
  showing it, for `--connect` and for every console `connect`. `--weapon <name>` does the same for the weapon menu, with a
  weapon file name such as `m1carbine_mp`. If the menu refuses the weapon, it
  reopens and you pick by hand.
- `--debug-overlay` (or F3 at runtime) shows frame time, draw stats, net and
  audio counters.
- `--no-audio` runs silent; `--volume <0..1>` sets the master volume.
- `--net-probe ip:port` is the headless probe client. It prints what it
  receives and dumps captures to `tmp/`. Its many modes are documented in
  [AGENTS.md](AGENTS.md).

### Server

```
vcod-server mp_carentan --port 28960 --hostname "my server" --gametype tdm
```

- The server binds `0.0.0.0` and, like retail, answers `getstatus` from anyone
  and honours an out-of-band `disconnect` by source address. Keep it on a LAN
  or behind a firewall you control.
- `--gametype` picks the script under `maps/mp/gametypes/` (default `dm`).
- `--max-clients` sets `sv_maxclients` (default 8).
- `--set NAME=VALUE` sets a cvar before the scripts load, retail's `+set`.
  Repeatable, e.g. `--set scr_friendlyfire=1`. Set `sv_mapRotation` this way
  to get a rotation.
- `dedicated` defaults to 1, where retail's is 2, so dev runs stay off the
  master list. `--set dedicated=2` sends a heartbeat to `sv_master1`
  (`codmaster.activision.com`) every three minutes and a flatline on Ctrl-C
  or `quit`.
- `--set rconPassword=<pw>` turns on rcon (`rcon <pw> status` from a client
  console or any rcon tool).
- `--bots <n>` adds `n` debug bots, each in a real client slot, alternating
  allies and axis. The first tick with bots starts building the map's
  navigation graph on a thread of its own (a second or two on the big stock
  maps); the server keeps ticking and the bots wander until it is ready.
  `--bots-shoot` lets them engage the nearest visible enemy: semi-autos tap,
  automatics fire in bursts, sights go up at range, and they strafe and
  crouch while fighting. They also reload and throw frags, and go looking
  where a lost enemy was last seen.
- `--gametype-script <file>` runs a gametype script from disk instead of the
  paks. The probe recipes use it.
- `--test-entities <n>` adds entities that move on the wire, to exercise the
  packet-entity encoding. A client draws nothing for them.
- `--trace` logs one line per snapshot per client, and once a second the
  slowest tick and how far the loop ran behind its 20 Hz schedule.
- `--game-dir`, `--mod-dir` and `COD_DIR` work as they do for the client.

## Controls

Click to capture the mouse, Esc to release it, mouse to look around.

### Fly mode (default)

| Input | Action |
|---|---|
| W / A / S / D | Move forward / left / back / right |
| Space | Move up |
| Ctrl | Move down |
| Shift | Speed boost |
| Scroll | Adjust fly speed |

### Playing (`--connect`)

These are the default binds, the stock `config_mp.cfg` ones with the sight on
the right mouse button as `+speed`. The console's `bind` changes them; `bind
MOUSE2 "toggle cl_run"` makes the sight a toggle, as retail's option does.

| Input | Action |
|---|---|
| W / A / S / D | Move forward / left / back / right |
| Space | Stand up from crouch or prone; jump when standing |
| C | Crouch |
| Ctrl | Prone |
| Q / E | Lean left / right (held) |
| LMB | Fire (semi-automatic weapons fire once per click) |
| RMB | Aim down the sight (held) |
| R | Reload |
| Shift | Melee |
| F | Use: pick up a weapon, mount an MG, plant or defuse, respawn after a death |
| 1 / 2 / 3 / 4 | Weapon slot: primary, second primary, pistol, grenade |
| Scroll | Next / previous weapon |
| Tab | Scoreboard (held) |
| T / Y | Type a chat line to everyone / your team; Enter sends, Escape drops it |

Your position is predicted while you play. While you spectate, are dead,
follow someone or sit through the intermission, it comes from the server. As a
spectator, Space rises while held and C sinks until Space is pressed.

| Input | Action |
|---|---|
| M | Open the main script menu: the team menu, or on stock gametypes the weapon menu once you have a team |
| 0-9 | Pick the menu row bound to that key |
| Up / Down, Enter | Move the menu selection, pick it |
| Esc | Close the menu |

While a menu is open these keys go to it, so the digits pick a row instead of
a weapon and Esc closes the menu rather than releasing the mouse. W / A / S /
D still move.

### Walk mode (`--walk`)

| Input | Action |
|---|---|
| W / A / S / D | Move forward / left / back / right |
| Space | Jump (re-press to jump again, no autohop) |
| Ctrl | Crouch (held) |
| Z | Toggle prone |
| Q / E | Lean left / right |
| Shift | Slow walk |
| LMB | Fire |
| RMB | Aim down sights (held) |
| R | Reload |
| 1-7 | Weapon: colt, thompson, mp40, mp44, enfield, kar98k, scoped kar98k |

### Console

`` ` `` or `~` drops it down, and again (or Esc) puts it away. While it is down
the game gets no keys and the mouse is released. Up / Down walk the last 32
lines, Tab completes a command or cvar name, Page Up / Page Down and the wheel
scroll. In game, a line without a leading `/` or `\` is said as chat, as in
retail; anything else runs as a command. Several commands go on one line
separated by `;`.

| Command | Does |
|---|---|
| `connect <ip:port>` | Leave any server and join this one |
| `disconnect` | Leave the server for the console |
| `reconnect` | Join the last server again |
| `quit` | Leave and close the window |
| `say <text>`, `say_team <text>` | Chat to everyone / your team |
| `cmd <text>` | Send `<text>` to the server as a client command |
| `bind <key> [command]` | Bind a key, or show its bind; `unbind <key>`, `unbindall`, `bindlist` |
| `set`, `seta <cvar> <value>` | Set a cvar; `seta` also saves it. `<cvar>` alone prints it, `<cvar> <value>` sets it |
| `toggle <cvar> [values...]` | Flip a cvar between 0 and 1, or step through the values |
| `cvarlist`, `cmdlist` | List the cvars and commands, optionally by prefix |
| `echo`, `clear` | Print a line; empty the scrollback |

Any other command goes to the server while connected, as retail forwards it
(`callvote`, `vote yes`, `kill`, `follownext`). Bindable commands are the
stock ones: `+forward`, `+back`, `+moveleft`, `+moveright`, `+gostand`,
`gocrouch`, `goprone`, `+leanleft`, `+leanright`, `+attack`, `+speed` (the
sight), `+melee`, `+activate`, `+reload`, `weaponslot
<primary|primaryb|pistol|grenade>`, `weapnext`, `weapprev`, `+scores`,
`messagemode`, `messagemode2`, `toggleconsole`. Key names are retail's
(`MOUSE1`, `MWHEELUP`, `CTRL`, `SPACE`, `KP_ENTER`, letters and digits).

The client's cvars are `name`, `cl_run` (1: the sight key aims while held, 0:
while released) and `scr_conspeed` (how fast the console slides). Binds only
act while connected; fly and walk mode keep their fixed keys. Esc, M (the
script menu), F3 and F4 are fixed.

### Everywhere

| Input | Action |
|---|---|
| `` ` `` / ~ | Console |
| F3 | Toggle the debug overlay |
| F4 | Culling: on, locked (freeze the visible set), off |

## How it's put together

A Cargo workspace of four crates:

- `crates/common` (`vcod-common`): file formats (BSP, xmodel, xanim, pk3,
  shaders, sound aliases), collision, movement, weapons and the 1.1 protocol
  in both directions. Shared by client and server. It imports no wgpu, winit
  or kira.
- `crates/client` (`vcod`): window, renderer, HUD, effects, audio, prediction
  and the probe client. wgpu, winit and kira.
- `crates/server` (`vcod-server`): the dedicated server.
- `crates/gsc` (`vcod-gsc`): lexer, parser, bytecode compiler and VM for
  CoD's script language. It depends on nothing else in the workspace.

[AGENTS.md](AGENTS.md) is the contributor guide. It's long, and it holds the
lessons the code doesn't make obvious.

## Some numbers, for fun

As of October 2026:

- about 140,000 lines of Rust,
- about 1,900 tests,
- about 220,000 words of protocol and research notes, which is more than
  most novels and has fewer plot twists, except for the part where the
  grenade cook turned out not to exist,
- one asphalt footstep restored.

## Why

Mostly to see whether it could be done. Call of Duty 1 is an id Tech 3
descendant, and the Quake III Arena and Return to Castle Wolfenstein sources
are public, but the 1.1 wire protocol has never been documented. Every field
width, every enum order and every place Infinity Ward diverged from RTCW had to
be recovered from the binaries and confirmed against the retail server. The
result is in [docs/protocol-1.1.md](docs/protocol-1.1.md).

The other reason is that watching a 2003 game come back to life in a window
you built yourself is a lot of fun.

### This is an AI-driven project

Most of the code and documentation in this repository was written by an AI
coding agent working under my direction. I decide what to build, review what
comes back, run it against the real game and the retail server, and do the
pixel-level and by-ear checks the agent can't. The research docs label each
fact as verified against a binary or capture, or inferred from a
decompilation, so you can tell the two apart. Treat the code as a working
prototype, not a reference implementation.

## Documentation

- [docs/protocol-1.1.md](docs/protocol-1.1.md): the CoD 1.1 wire protocol,
  both directions, with every divergence from RTCW/Q3 listed at the end.
- [docs/research/](docs/research/): per-subsystem notes recovered from the
  binaries and checked live. File formats (BSP, xmodel, xanim, efx), the
  clientState stream, player models and animation, events and effects, the
  HUD protocol, the sound system, shader scripts, the server handshake,
  movement and player clipping, combat, items, turrets, movers, map cycling,
  spectator follow and the killcam, and the gsc language and object model.
- [tools/re/](tools/re/): the scripts used to pull tables out of the binaries
  (event enum, netfield tables, xrefs, script field and builtin tables, a
  Ghidra export script) and the disassembly notes for the Linux server.
- [AGENTS.md](AGENTS.md): working notes for contributors and coding agents.
  Layout, test setup, every probe mode, the reverse-engineering workflow, and
  a long list of gotchas I paid for so you don't have to.

## Lineage and inspiration

- [Quake III Arena](https://github.com/id-Software/Quake-III-Arena) and
  [Return to Castle Wolfenstein](https://github.com/id-Software/RTCW-MP) by
  id Software, GPL. CoD1 is an RTCW-MP descendant and its netcode, movement
  and animation code follow those sources closely. Where vcod ports a routine,
  the comment names the file it came from.
- [ioquake3](https://github.com/ioquake/ioq3), for a cleaner reading of the
  same netcode.
- [CoDExtended](https://github.com/xtnded/codextended), a GPL server
  extension for CoD1 1.1 whose reverse-engineered struct layouts were the
  starting point for the netfield tables.
- [cod-asset-importer](https://github.com/mauserzjeh/cod-asset-importer),
  a GPL Blender add-on for CoD assets. The xmodel triangle-strip decoder is
  ported from it and the xmodel layout was cross-checked against it.
- [wgpu](https://github.com/gfx-rs/wgpu), [winit](https://github.com/rust-windowing/winit)
  and [kira](https://github.com/tesselode/kira) for graphics, windowing and audio.

## Legal

vcod is not affiliated with or endorsed by Activision or Infinity Ward. Call
of Duty is a trademark of Activision Publishing, Inc. This repository contains
no game assets, no game code and no binaries from the game; it reads the files
of a copy you own. The reverse engineering was done to interoperate with the
game's own files and servers. The research notes document file formats and the
network protocol for that purpose: they contain layouts and addresses
recovered from the binaries, and no copied code. The screenshots above show
art owned by Activision.

Quake III Arena and Return to Castle Wolfenstein are trademarks of id
Software. Their GPL sources, and the other ported code, are credited in
[NOTICE](NOTICE).

## License

GPL-3.0-or-later; see [LICENSE](LICENSE). The network code was written with
the RTCW (GPLv3) and Quake 3 (GPLv2-or-later) sources as reference, so the
project is GPL to stay compatible with them.
