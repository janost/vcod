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
- **Don't run it as a public server for real players.** It has no rcon, no
  anti-cheat and no admin tooling. It has a lot of opinions about the order
  in which a tick runs.

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
  sight to the weapon's own FOV, and plays your own fire, reload and footstep
  sounds off the prediction. The snapshot that confirms them later stays
  quiet.
- Draws the HUD that the stock `hud.menu` lays out: crosshair that opens with
  spread, health, ammo, stance, compass with objectives, use hints and hit
  direction. It also draws the gametype script's own HUD elements, such as
  the S&D clock, the bomb icons and the progress bar.
- Follows the server through a map change: loading screen, downloads, new
  map.

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
  from the stock scripts, not from Rust. `dm`, `tdm` and `sd` are the
  gametypes I have checked against retail.
- Movement on the shared pmove, with players as capsules that block and push
  each other the way retail's do.
- Combat: bullets trace the world, players and static props. Hits go through
  the stock damage callback with per-bone hit locations. Rifle rounds pass
  through players and every round passes through glass. Melee works, and
  grenades fly as real missiles that bounce, rest and explode with retail's
  falloff.
- Deaths leave corpses in the eight-slot body queue and drop the dead player's
  weapon. Players pick up weapons, ammo and health packs by touch or the use
  key.
- Search & Destroy end to end: plant, defuse, progress bar, objectives on the
  compass.
- Mounted MG42s: mount with use, aim inside the gun's arc, fire, dismount.
- Map triggers (`trigger_multiple`, `trigger_hurt`, `trigger_use`,
  `trigger_lookat`), and script movers whose trajectories reach the wire.
  A moving brush model carries the players on it and shoves the ones in its
  way, and a player linked to a moving entity rides it.
- Intermission, `map_restart`, and `sv_mapRotation` the way retail runs them,
  with the next map's gamestate sent on the live connection.
- Spectator follow mode and the killcam, replayed out of a ring of archived
  frames.
- `--bots` adds debug bots that join through the stock menus and wander.
  `--bots-shoot` makes them fight. They are bad at it.

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

**No front end.** No main menu, no server browser, no options screen, no
console, no key rebinding. You get command-line flags and the binds below.

**Client**

- Mounted MGs work on the server, but the vcod client doesn't draw the gun.
- The HUD skips a few retail pieces: the followed player's health, ammo and
  compass while following, friendly players on the compass, the weapon mode
  icon, the stance-change flash, the mounted gun's reticle and the
  fixed-width fonts
  ([docs/research/cod11-hud-protocol.md](docs/research/cod11-hud-protocol.md),
  section 9).
- No sniper scope overlay, online or in walk mode.
- Only protocol 1 (patch 1.1). 1.5 and United Offensive servers won't talk to
  it.
- Prediction clips against the map and other players, but not against moving
  script entities.

**Server**

- No fall damage. You get the landing stun, and the ground forgives you
  everything else.
- `re` (Retrieval) and `bel` (Behind Enemy Lines) run through the same
  script path, but I haven't checked either against retail.
- Item respawn, an item's launch arc and `CONTENTS_NODROP` aren't modelled.
- A moving brush model pushes players only, not items, grenades or corpses.
- No rcon, no anti-cheat, no PunkBuster, no master server heartbeat.

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

- The first positional argument is the map name (case-insensitive).
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
  showing it. `--weapon <name>` does the same for the weapon menu, with a
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
- `--bots <n>` adds `n` debug bots, each in a real client slot, alternating
  allies and axis. `--bots-shoot` lets them engage the nearest visible enemy,
  reload and throw frags.
- `--gametype-script <file>` runs a gametype script from disk instead of the
  paks. The probe recipes use it.
- `--test-entities <n>` adds entities that move on the wire, to exercise the
  packet-entity encoding. A client draws nothing for them.
- `--trace` logs one line per snapshot per client.
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

The binds follow retail's `config_mp.cfg`, except the right mouse button,
which retail binds to `toggle cl_run` and vcod uses for the sight.

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
| 1-6 | Weapon: colt, thompson, mp40, mp44, enfield, kar98k |

### Everywhere

| Input | Action |
|---|---|
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
