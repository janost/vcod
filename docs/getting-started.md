# Getting started

vcod is a client (`vcod`) and a dedicated server (`vcod-server`) for four
games: Call of Duty 1.1 and 1.5, United Offensive 1.51, Call of Duty 2 1.3
and Call of Duty 4 1.7. Both binaries read the game's own files from a
retail install, so you need to own the game: vcod ships no game files
and never will. Every flag, cvar and command is in [usage.md](usage.md);
bots are in [bots.md](bots.md).

## Point vcod at the install

Each binary looks for the install in this order:

1. `--game-dir <install>`
2. the `COD_DIR` environment variable
3. the directory the executable sits in

So either copy `vcod` and `vcod-server` into the install directory, or
set `COD_DIR` once. `COD_DIR` is the one variable the binaries read for
every game; `COD2_DIR` and `COD4_DIR` are read only by tests and tools.

The game is detected from the install's files. `--game cod1|cod2|cod4`
overrides the guess.

| Game | Install holds | Detected by | Extra flag |
|---|---|---|---|
| CoD 1.1 / 1.5 | `CoDMP.exe`, `main/pak*.pk3` | anything not CoD2 or CoD4; 1.5 by its `main/pak0.pk3` checksum | none |
| United Offensive 1.51 | 1.5's `main/` and `uo/pakuo*.pk3` | as CoD1 | `--mod-dir uo` on every command |
| CoD2 1.3 | `CoD2MP_s.exe`, `main/iw_*.iwd` | `main/iw_*.iwd` or `CoD2MP_s.exe` | none |
| CoD4 1.7 | `iw3mp.exe`, `zone/`, `main/` | `zone/` or `iw3mp.exe` | none |

## Quick start per game

The examples assume `COD_DIR` is set or the binaries sit in the install.
Hosting and joining work on one machine: the server answers on
`127.0.0.1:28960`.

### CoD 1.1 and 1.5

```
vcod                                                    # main menu and server browser
vcod --connect 1.2.3.4:28960                            # join through the team and weapon menus
vcod --connect 1.2.3.4:28960 --team axis --weapon kar98k_mp
vcod-server mp_carentan --gametype tdm --bots 6 --bots-shoot
```

A 1.1 install hosts 1.1 (protocol 1), a 1.5 install hosts 1.5 (protocol 6).
The client tries its install's protocol and switches between 1.1 and 1.5
when a server refuses it.

### United Offensive

```
vcod --mod-dir uo
vcod --mod-dir uo --connect 1.2.3.4:28960 --team allies
vcod-server mp_cassino --mod-dir uo --gametype tdm --bots 6 --bots-shoot
```

Without `--mod-dir uo` the install reads as plain 1.5. A UO client
refuses 1.1 and 1.5 servers and the others refuse UO servers.

### CoD2

```
vcod                                                    # CoD2's main menu and browser
vcod --connect 1.2.3.4:28960 --team allies --weapon thompson_mp
vcod-server mp_toujane --gametype tdm --bots 6 --bots-shoot
```

### CoD4

```
vcod                                                    # 1.7's main menu; the first start asks for a profile
vcod --connect 1.2.3.4:28960 --team allies --class assault_mp,0
vcod-server mp_crash --bots 6 --bots-shoot              # war unless --gametype says otherwise
```

Stock classes: `assault_mp,0`, `specops_mp,0`, `heavygunner_mp,0`,
`demolitions_mp,0`, `sniper_mp,0`, and the custom ones as `customN,0`.

## Recipes

### Play with friends on a LAN

1. On one machine: `vcod-server <map> --gametype tdm --max-clients 16`.
   It binds `0.0.0.0:28960`; `--port` picks another port.
2. On each player's machine, open `vcod` and pick the server from the
   browser's Local source, or run `vcod --connect <host-ip>:28960`.

Start New Server in the main menu does step 1 for you: it runs
`vcod-server` from beside `vcod`, so build or copy both. The client stops
that server when it exits.

Loopback and private addresses count as LAN: no rate limit, no CD key.

### Host a public server

```
vcod-server mp_carentan --gametype sd --hostname "my server" --max-clients 20 \
    --set dedicated=2 --set rconPassword=<pw> --ban-file ban.txt
```

- `dedicated=2` sends a heartbeat to `master.cod.pm` every three minutes
  (port 20510 for CoD1, 20610 for UO, 20710 for CoD2) and a flatline on
  shutdown. The default, 1, sends none.
- CoD4 sends heartbeats only to a master named in `sv_master1`: add
  `--set sv_master1=master.cod.pm` (port 20810 by default).
- On CoD2 and CoD4 the rcon password cvar is `rcon_password`;
  `--set rconPassword=` sets it too.
- The server answers `getstatus` from anyone. Open the UDP port on your
  firewall and nothing else.
- There's no anti-cheat. A public server runs on the honour system, and
  you know how that goes.

### Add and remove bots

From a client's console on a CoD1 or UO server:

```
rconPassword <pw>
rcon addbot 4
rcon addbot axis 2 skill 3
rcon removebot all
rcon set vc_bot_minplayers 10
```

On CoD2 and CoD4, log in first: `rcon login <pw>`, then the same
`rcon ...` lines. The full guide is [bots.md](bots.md).

### Record and play a demo

```
record mydemo        # in the console, while connected
stoprecord
demo mydemo          # plays demos/mydemo.dm_N back
```

Demos go to `demos/` in the game directory (the mod's, if one runs).
`timedemo` times playback: on CoD1 and UO `set timedemo 1` before
`demo`, on CoD2 and CoD4 `timedemo mydemo`. To play one from the command
line and quit at its end:

```
vcod --exec "set nextdemo quit" --exec "demo mydemo"
```

### Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `cannot open game data in <dir>` or `cannot read mod dir <dir>` | The install was not found. Pass `--game-dir` or set `COD_DIR` to the directory that holds `main/`. |
| `CoD2 support is not implemented yet` (or CoD4) | The mode is CoD1-only (`--walk`, `--list`), or the game was misdetected. Pass `--game cod1`, `cod2` or `cod4`. |
| `map '<name>' not found; similar: ...` | Map names are case-insensitive; `vcod --list` prints every CoD1 map in the search path. |
| `This is a United Offensive server: run vcod with --mod-dir uo` | Add `--mod-dir uo` (UO install). The reverse message means drop it. |
| A third-party pak breaks a map or the menus (CoD1, UO) | `--vanilla` on either binary loads only the patch's stock paks. |
| A server rejects the CD key | See [usage.md, CD keys](usage.md#cd-keys). LAN servers and vcod-server never ask. |
| A CoD4 join is refused for missing mod files | The client downloads the server's mod and usermap files unless `cl_allowDownload` is 0. |
