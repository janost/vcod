# vcod

Call of Duty (2003) multiplayer, rebuilt from scratch in Rust. A client and
a dedicated server that read your copy's pk3s and speak the original wire
protocol. Patch 1.1, patch 1.5 and United Offensive 1.51 all work, against
vcod's own server or the retail ones. Nobody ever wrote that protocol down,
so I dug it out of the binaries.

Call of Duty 2 (patch 1.3) and Call of Duty 4 (patch 1.7) run on the same
client and server. Both are newer and less complete than CoD 1;
[docs/features.md](docs/features.md) says what works.

[![mp_railyard on a public server, 3840x1600](docs/screenshots/ultrawide-mp_railyard-s.jpg)](docs/screenshots/ultrawide-mp_railyard.jpg)

[![mp_harbor on a public server, 3840x1600](docs/screenshots/ultrawide-mp_harbor-s.jpg)](docs/screenshots/ultrawide-mp_harbor.jpg)

[![Twelve bots fighting over one wall on mp_pavlov, 3840x1600](docs/screenshots/ultrawide-mp_pavlov-bots-s.jpg)](docs/screenshots/ultrawide-mp_pavlov-bots.jpg)

All three run at 21:9 with the F3 overlay up. Click one for the full size.

## Read this first: it plays, and it still has opinions

You can play it. Start a server, join it, pick a team, shoot bots or
friends, plant the bomb, drive a tank in United Offensive. Most evenings it
feels like Call of Duty. Now and then it finds a bug the 2003 game never
had, and occasionally one it had and I copied on purpose.

- **Retail is the referee.** Movement, shots, grenades, collision, snapshots
  and the handshake are replayed against captures from the real 1.1, 1.5
  and UO servers, down to the float rounding of each build. When vcod and
  retail disagree, retail wins and the bytes go into a research doc.
- **It still has bugs.** Fewer than it used to, and different ones from
  the 2003 game. Please [report them](#reporting-bugs).
- **It's a hobby.** I work on it while poking at a 2003 engine is fun. No
  roadmap, no "1.0", no promises.
- **It needs your copy of the game.** No assets ship here, and none will.
- **Hosting is for friends, for now.** There's no anti-cheat, the rcon knows
  a few dozen commands, and the bots are the only players it has been
  tested against at length. A LAN or a few friends is fine. Your public
  server's regulars deserve better.
- **Linux is the only platform I run.** Windows and macOS builds exist and
  nobody has told me whether they work. Godspeed.

## What works

The short tour. The long one, gaps included, is
[docs/features.md](docs/features.md).

- **The client** opens on the stock main menu and server browser, built from
  the game's own menu files. It joins 1.1, 1.5 and UO servers, downloads
  missing paks, and plays: movement and prediction, every stock weapon with
  its viewmodel, sights, recoil and scopes, the HUD, scoreboard, chat,
  votes, quick messages, mounted MGs, and UO's sprint, bipods, cooked
  grenades, tanks and jeeps.
- **The server** hosts whichever patch its install is. It runs Activision's
  own gametype scripts on vcod's own `.gsc` VM, so `dm`, `tdm`, `sd`, `re`,
  `bel` and UO's `ctf`, `dom`, `hq` and `bas` play by the stock rules.
  Bullets, grenades, rockets, vehicles, items, movers, the killcam, map
  rotation and rcon all work. Retail clients can join it.
- **Bots** (`--bots`) join through the menus, find their way around the
  map, play the objectives and, with `--bots-shoot`, fight back. They're bad
  at it. So am I. rcon's `addbot`, `removebot` and `vc_bot_minplayers`
  manage them on a running server ([docs/bots.md](docs/bots.md)).
- **Extras retail never had**, all off by default: soft sun shadows, SSAO,
  bloom, HDR, sun shafts, soft particles and a film grade. `--rtx-on` turns
  them all on for one run. Despite the name, no rays were harmed.
- **Widescreen and ultrawide.** Retail keeps the same horizontal field of
  view on any window, so a wider screen loses picture at the top and
  bottom, and its HUD stays in a 4:3 box in the middle. vcod widens the
  view instead (Hor+): 16:9 and 21:9 see more at the sides, and the
  compass, kill feed and ammo counter move out to the window's edges.
  `vc_widescreen 0` brings back retail's. Borderless and exclusive
  fullscreen both work, and Alt+Enter flips between windowed and
  fullscreen.

## How to play

1. **Own the game.** Call of Duty with patch 1.1 or 1.5, or United
   Offensive 1.51.
2. **Get vcod.** Grab the archive for your OS from the
   [nightly release](https://github.com/janost/vcod/releases/tag/nightly)
   (or [build it](#building-from-source)). Put `vcod` and `vcod-server`
   next to `CoDMP.exe`. If you'd rather not, point `COD_DIR` or
   `--game-dir` at the install.
3. **Play against bots.** The quickest way to a fight:

   ```
   vcod-server mp_carentan --bots 6 --bots-shoot --gametype tdm
   vcod --connect 127.0.0.1:28960
   ```

   Pick a team, pick a weapon, go get shot.
4. **Play with friends.** Run plain `vcod` and use Start New Server in the
   main menu. Dedicated No starts a server and puts you in it. Friends on
   your LAN find it under Local in the browser, anyone else connects by
   address once you open UDP 28960. The server stops when you leave. Start New Server has no bots. Use the
   command line above for those.
5. **Join someone else's server.** The browser lists the Internet, the LAN
   and your favourites, or use `vcod --connect <ip:port>`. PunkBuster
   servers will refuse you, since vcod runs no PunkBuster. If vcod acts
   weird on a public server, leave before the regulars notice.

**United Offensive** reads `uo/` over the `main/` beside it. Run both
binaries from the UO install with `--mod-dir uo`, e.g.
`vcod-server mp_cassino --mod-dir uo --bots 6 --bots-shoot` and
`vcod --mod-dir uo --connect 127.0.0.1:28960`. A server on a 1.5 install
speaks 1.5 on its own.

Mods or downloaded maps acting up? `--vanilla` on either binary loads
only the stock paks and ignores every other pk3
([docs/usage.md](docs/usage.md)).

Controls are the stock ones: WASD, Space to jump, C and Ctrl to crouch and
go prone, Q and E to lean, right mouse for the sight, F to use, M for the
team and weapon menu, `` ` `` for the console. UO adds Alt to sprint. Every
key, flag and console command is in [docs/usage.md](docs/usage.md).

## Reporting bugs

Use the [bug report form](https://github.com/janost/vcod/issues/new/choose).
It asks for what I need to reproduce it, and the first thing is the version
line. Every build names itself (`vcod 0.1.0-7924565c linux-amd64`) in the
window title, the main menu, the console (`version`), the F3 overlay,
`vcod --version` and the first line of the log. Press F3 before a
screenshot, so I can see where you were standing.

## Known gaps

The ones you'll hit first. The full list is in
[docs/features.md](docs/features.md#what-it-doesnt-do).

- No listen server. Start New Server always runs a separate `vcod-server`.
- No PunkBuster.
- A mod's UI DLL is ignored. Its menu files work.
- Audio follows the engine on paper but has had little checking by ear.
- Visibility draws a bit more than retail on purpose. Retail assumes nobody
  looks over a cell's walls, and the mp_ship decks disagree.

And a few that look like gaps and aren't. Retail 1.1 has no mantling, no
swimming and no grenade cooking, and its engine never gives a sound a
velocity, so no doppler either. Asphalt footsteps are silent in retail
because the engine asks for `asphalt` and the sound table spells it
`asphault`. vcod uses the table's spelling, so you can hear asphalt.
Twenty-three years late, Infinity Ward, you're welcome.

## Building from source

Rust 1.90 or newer. The client needs a GPU with BC (DXT) texture
compression, and wgpu picks the backend (`WGPU_BACKEND=vulkan|gl|dx12|metal`
narrows it). The server needs no GPU.

```
cargo build --release
COD_DIR=/path/to/CallOfDuty cargo test
```

That gives `target/release/vcod` and `target/release/vcod-server`. Without
`COD_DIR` the tests that need game data return early and pass, so a green
run without the game proves nothing about the parsers. CI runs that way,
because it can't ship the game either. A build outside a git checkout reads
its sha from `VCOD_GIT_SHA`, or calls itself `unknown`.

## How I know it's right (when it is)

The retail dedicated servers are the oracle. When vcod and retail disagree,
retail wins and the disagreement goes into a research doc.

- A headless probe client (`vcod --net-probe`) joins a server and records
  what it sees, with a few dozen scripted modes: shoot someone, throw a
  grenade, plant the bomb, drive a tank into another tank.
- Retail's recordings are committed as fixtures. A/B tests replay the same
  inputs on vcod's server and diff the result snapshot by snapshot.
- Small `.gsc` probes run on retail and on vcod's VM, and the suite compares
  their output.

That catches a lot. Anything about how it looks or sounds still needs a
human squinting at a screen, which is where you come in.

## Documentation

- [docs/features.md](docs/features.md): everything vcod does and doesn't.
- [docs/usage.md](docs/usage.md): command lines, server options, controls,
  the console, the menus.
- [docs/bots.md](docs/bots.md): running bots, the rcon commands, the
  minimum player count, team balance and slots.
- [docs/protocol-1.1.md](docs/protocol-1.1.md): the 1.1 wire protocol, both
  directions, with every divergence from RTCW/Q3 at the end.
- [docs/research/](docs/research/): about forty documents on file formats,
  movement, combat, vehicles, sound, scripts, bots and the rest. Every claim
  names the binary address it rests on and says whether it was verified or
  inferred. I'd call this the most useful part of the repo.
- [AGENTS.md](AGENTS.md): the contributor guide. Layout, measuring against
  retail, and the traps I already paid for.

The code is four crates. `vcod-common` holds formats, collision, movement
and the protocol, with no GPU or audio. `vcod` is the client, `vcod-server`
the server, and `vcod-gsc` the script VM, which depends on nothing else here.

## Some numbers, for fun

As of mid-October 2026:

- about 270,000 lines of Rust,
- about 3,000 tests,
- about 450,000 words of protocol and research notes, twice the length of
  Moby-Dick with a worse plot, except the twist where grenade cooking
  turned out not to exist until United Offensive,
- one asphalt footstep restored.

## Why

Mostly to see whether it could be done. CoD 1 descends from id Tech 3, and
the Quake III and RTCW sources are public, but CoD's wire protocol was never
documented. Every field width, enum order and place Infinity Ward wandered
off from RTCW had to come out of the binaries and be checked against the
retail server. Also, watching a 2003 game come back to life in a window you
built yourself is a lot of fun.

### This is an AI-driven project

An AI coding agent wrote most of the code and docs here, under my direction.
I decide what to build, review what comes back, run it against the real game
and the retail servers, and do the pixel and by-ear checks the agent can't.
The research docs label each fact verified or inferred so you can tell them
apart. Treat the code as a working prototype, not a reference
implementation.

## Lineage and inspiration

- [Quake III Arena](https://github.com/id-Software/Quake-III-Arena) and
  [Return to Castle Wolfenstein](https://github.com/id-Software/RTCW-MP) by
  id Software, GPL. CoD1 descends from RTCW-MP, and its netcode, movement and
  animation code follow those sources closely. Where vcod ports a routine,
  the comment names the file it came from.
- [ioquake3](https://github.com/ioquake/ioq3), for a cleaner reading of the
  same netcode.
- [CoDExtended](https://github.com/xtnded/codextended), a GPL server
  extension for CoD1 1.1 whose struct layouts were the starting point for the
  netfield tables.
- [cod-asset-importer](https://github.com/mauserzjeh/cod-asset-importer), a
  GPL Blender add-on. The xmodel triangle-strip decoder is ported from it.
- [zk_libcod](https://github.com/ibuddieat/zk_libcod), an MIT CoD2 1.3
  server library. `tools/re` reads its list of named 1.3 addresses to name
  functions in the retail server; some CoD2 docs use those names.

### Consulted, not copied

These projects were read as references while working out CoD4. vcod contains
no code from them. Where earlier CoD4 code had followed one of them too
closely, I had it audited function by function and rewritten from the retail
1.7 binaries and captures, with the retail gates proving the behaviour held.

- [kisakcod](https://github.com/jm2/kisakcod), GPLv3, a reconstruction of
  the CoD4 1.8 multiplayer executable. Used for function names and as a
  cross-check of behaviour. 1.7 is the target: the research docs cite 1.7
  addresses, and a claim not yet confirmed on 1.7 says so.
- [CoD4x Server](https://github.com/callofduty4x/CoD4x_Server), AGPLv3, for
  how CoD4x servers differ on the wire, so vcod can recognise and refuse
  them.
- [COD4iOS-COD4NS](https://github.com/DevZer0D4Y/COD4iOS-COD4NS), GPLv3, as a
  cross-check of the fastfile layout.

### Libraries

- [wgpu](https://github.com/gfx-rs/wgpu),
  [winit](https://github.com/rust-windowing/winit) and
  [kira](https://github.com/tesselode/kira) for graphics, windowing and
  audio.

## Legal

vcod is not affiliated with or endorsed by Activision or Infinity Ward. Call
of Duty is a trademark of Activision Publishing, Inc. This repository
contains no game assets, game code or binaries; it reads the files of a copy
you own. The reverse engineering was done to interoperate with the game's own
files and servers. The research notes document file formats and the network
protocol for that purpose: layouts and addresses recovered from the
binaries, no copied code. The projects under "Consulted, not copied" were
read as references, and no code from them is included. The screenshots show art owned by Activision.

Quake III Arena and Return to Castle Wolfenstein are trademarks of id
Software. Their GPL sources, and the other ported code, are credited in
[NOTICE](NOTICE).

## License

GPL-3.0-or-later; see [LICENSE](LICENSE). The network code was written with
the RTCW (GPLv3) and Quake 3 (GPLv2-or-later) sources as reference, so the
project is GPL to stay compatible with them.
