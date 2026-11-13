# Call of Duty 2

vcod plays and hosts Call of Duty 2, patch 1.3 (protocol 118). It needs a
CoD2 install: `main/` with the stock `iw_*.iwd` files. Put the binaries
in it, or point `--game-dir` or `COD_DIR` at it (`COD2_DIR` is only for
tests and tools). The game is detected from `main/iw_*.iwd` or
`CoD2MP_s.exe`; `--game cod2` forces it. How to run each mode is in
[usage.md](../usage.md).

Research docs live in [docs/research](../research/); the links below name
the section that measured each claim.

## What works

### Client

**Front end.** Bare `vcod --game cod2` runs CoD2's stock main menu, its
rects placed by their screen alignment
([cod2-fonts-menus.md](../research/cod2-fonts-menus.md) 4).

- The server browser lists LAN servers (a `getinfo` broadcast),
  `master.cod.pm`'s protocol 118 list and favourites, in CoD2's eleven
  columns, maps by their arena long name. It joins the server picked.
- Start New Server runs `vcod-server --game cod2` beside `vcod`.
- The options screens set binds, mouse, graphics (the monitor's video
  modes and refresh rates) and `snd_volume`.
- Player profiles are created, picked and deleted as retail's
  (`players/<name>/`, `active.txt`), each with its own `vcod_mp.cfg`.

**Join, download, spectate, play.** `--connect ip:port`, the browser or
`connect` on the console:

- Decodes the gamestate and delta snapshots and sends the pure `cp`
  checksums ([cod2-protocol.md](../research/cod2-protocol.md) 11).
- Downloads the non-stock iwds the server references and the install
  lacks (`cl_allowDownload`; UDP, or a www redirect's HTTP under
  `cl_wwwDownload`) into `main/` or the server's `fs_game`, with progress
  on the connect screen, then reopens the search path over them
  (cod2-protocol.md 17.5). A refused download or a map that will not load
  returns to the menu with retail's message.
- Shows the server's script menus (server info, team, weapon, Esc's
  `ingame`, mute and vote) for the player to answer. `--team` and
  `--weapon` answer them on their own.
- Spectates (free flight through usercmds, or following a player) or
  plays. A player sends CoD2's buttons through the console binds
  (`default_mp.cfg`'s): fire, sight toggle, melee, hold breath, frag,
  smoke, binoculars, use, reload, lean, `gocrouch`, `goprone`,
  `+gostand`, `weapnext`, and the Options menu's other stance binds
  (`+movedown`, `+prone`, `+moveup`, the toggles, `lowerstance`,
  `raisestance`); not `+stance`. The weapon byte is the client's own pick. X
  opens `changeweapon` for the next spawn.
- Follows a map change, keeps the map through `map_restart`,
  `fast_restart` and `sd` rounds, and returns to the main menu with the
  server's reason on a kick, a server quit or `cl_timeout`.
- Forwards `say`, `callvote` and other unknown commands to the server.
  `rcon login`, `rcon logout` and `rcon host` stand in for 1.3's missing
  `rconPassword` ([usage.md](../usage.md), "Console").

**Netcode and prediction.**

- The clock is CoD2's `CL_AdjustTimeDelta`, one snapshot behind the
  newest. The last 32 snapshots are kept and each entity is drawn
  between the two around the frame's time.
- One packet a frame on a LAN, 30 a second off it, each repeating the
  last packet's cmds (`cl_packetdup` 1). `Connection Interrupted` and the
  `net_disconnect` icon show when the server stops running cmds;
  `cg_drawLagometer` draws the lagometer. A changed `name`, `rate` or
  `snaps` goes out as `userinfo` (cod2-protocol.md 18).
- Our own body is predicted through the server's own pmove and weapon
  code: the sight's rise, zoom and clips, shots and reloads at the frame
  rate, retail's error decay, first-person bob and lean, mantles, and
  other players' bodies clipped a unit inside the server's box as the
  cgame does. Brush models and item pickups are left out of the
  prediction ([cod2-movement.md](../research/cod2-movement.md) 18,
  [cod2-hud.md](../research/cod2-hud.md) 18.1).
- `VCOD_NETSIM` simulates a bad link; F3 adds net, clock, lerp and
  prediction lines.

**Rendering.**

- Script models, turrets aimed by their controllers, weapon and health
  items, missiles, brush models, player bodies with the weapon in hand,
  and corpses, under the server's fog (cod2-protocol.md 12).
- Bodies posed off `legsAnim` and `torsoAnim` with the `j_*` rig's
  controls ([cod2-player-anims.md](../research/cod2-player-anims.md)).
- The view weapon of our own or a followed body: hands and gun animated
  off the playerstate, swaying, bobbing, tilting and lagging the turn as
  `CG_AddViewWeapon` places it. The sight clips crossfade over 0.5 s; a
  main clip change cuts, as retail's does
  ([cod2-weapons.md](../research/cod2-weapons.md) 6-7).
- Static props draw their normal and specular maps per pixel. Flags
  wave. An objective material on a static prop is not drawn
  ([cod2-materials.md](../research/cod2-materials.md) 9.2, 9.6, 9.7).
- Retail's shadow cookies: players, corpses and entity models (the
  nearest 12 by default, up to 24) take the sun off the lit world and
  models under them, under `sc_enable` and the other `sc_*` dvars (9.8).
- `r_glow` bloom and sky bleed (9.9); the shellshock trail's quarter
  desaturation (9.10); `blurWorld` menus blur the 3D view (9.13).
- mp_harbor's water runs retail's wave simulation: a Phillips spectrum
  through an FFT into a height map each frame, under its cube reflection,
  the sun's highlight and the fog (9.12).
- Snow on mp_downtown, mp_harbor, mp_leningrad and mp_railyard falls as
  retail's particle clouds, a thousand flakes a cloud, stopping at roofs
  (9.14).
- The `effect_zfeather*` sprites fade into the world behind them (9.5).

**HUD.** The playerstate's hudelems and the stock `hud.menu`
([cod2-hud.md](../research/cod2-hud.md)):

- Ammo counter, weapon name and mode icon, grenade counts, stance icon
  and the prone-blocked line.
- Compass with north (sprung as it turns), teammates (a talking one
  flashing), objectives and fire pings, an MG42 gunner's included.
- Use and pickup hints, fading as retail's. The cursor hint names the
  item, gun or use trigger in front, from retail's one use list
  ([cod2-gsc-diff.md](../research/cod2-gsc-diff.md) 5.4).
- Low-health overlay, red damage flash, damage direction icons.
- Crosshair spread with the weapon, gone once the sight is all the way
  up. A scope's overlay with its zoom (`adsZoomFov`, the gun hidden under
  it); an iron sight zooms the gun with the world. `cg_drawGun 0` (a
  cheat) hides the gun and its flash and keeps the crosshair up through
  the sight.
- Gametype hudelems: timers, clocks (sd's bomb stopwatch), plant and
  defuse bars, world waypoints (ctf flags, sd bomb sites, hq radios,
  death icons).
- Head icons, the disconnected icon and the talk balloon. The killcam's
  "you" icon over the local player. A teammate's name under the
  crosshair, coloured by health (`cg_drawCrosshairNames`).
- A spectator's `SPECTATOR`, follow keys and `following <name>` (19).
- The scoreboard on `+scores`, scrolled by the wheel and page keys;
  chat lines, the say field, kill lines with icons flipped as each weapon
  says, the centre print (13-15). The vote lines off configstrings 15..18.
- `cg_drawMantleHint`, `cg_hudStanceHintPrints`, `cg_chatHeight`,
  `cg_chatTime`, `cg_hudChatPosition`, `cg_hudSayPosition`.
- No health bar, as retail's default.

**View effects.** Damage kick, the scope's idle sway and bob, the shot's
kick roll, a landing dip, earthquakes and MG42 shake. A demo's or a
followed player's eye bobs, dips on a landing and leans as a played one
does; a followed one eases its steps too, which a demo skips as
retail's does ([cod2-demos.md](../research/cod2-demos.md) 5). Shellshock from
`shock/*.shock`: screen blend, view kick, mouse scale and turn caps,
sound mix and the cmd's 0x800 (`cg_shellshock <secs> [file]` on the
console too). A held breath down a scope plays its breaths, heartbeat
and `hold_breath.shock` mix (cod2-hud.md 16-18).

**Mounted guns.** A gunner sees from the gun's `tag_player` with the view
held inside its arcs, and so does a spectator following one. Other
gunners draw on their guns in the gunner anims
([cod2-turrets.md](../research/cod2-turrets.md) 9, 15).

**Sound** ([cod2-sound.md](../research/cod2-sound.md)). CoD2's aliases
and falloff curves: footsteps, fire, reloads, impacts, passing rounds,
explosions, `playSound`, entity and missile loops, `playLocalSound` (the
announcer), `musicPlay`, `soundFade`, the level's ambient crossfaded
between tracks, sound blend entities, slave ducking under a master,
`setReverb`'s wet level, alias subtitles, the view weapon's notetrack
sounds (`noteTrackSoundA`..`D`, which no stock weapon sets). Fire sounds
and whizbys come from the shooter's `tag_flash`.

**Effects** ([cod2-efx.md](../research/cod2-efx.md) 6). Bullet impacts
from the impact table, explosions, `playFx`, `playLoopedFx`, the map's
ambient effects, tracers from the shooter's `tag_flash`. Muzzle flashes
and brass play on the guns' tags: the view weapon's flash and shell
eject on its `tag_flash` and `tag_brass` (no flash under a scope's
overlay), other players' and turrets' on theirs, a bolt action's brass
at the rechamber. A shot's flash waits for its body's next draw, as
retail's flag does (6.8).

**Voice chat** ([cod2-voice.md](../research/cod2-voice.md)). `+talk` (Z)
sends the microphone through the system's libspeex (narrowband at
CoD2's 8192 Hz, the server's `sv_voiceQuality`); each talker plays on a
stream of his own. The talk icon, the talker list, the scoreboard's voice
column and the `muteplayer` menu's mute list work as retail's. Without
libspeex nothing is sent or played.

**Demos** ([cod2-demos.md](../research/cod2-demos.md)). `record` and
`stoprecord` write CoD2's `.dm_1` in a game. `demo <name>` and `timedemo
<name>` play one through the session's frame, unpredicted. `timedemo`
writes retail's `demos/timedemo_<map>_mode_<r_mode>.csv`. Retail's demos
play in vcod and vcod's in retail. Retail's front end has no demo list,
and vcod none either. `cl_avidemo <fps>` steps the clock a
fixed `1000 / fps` ms a frame and writes each as a TGA.

**Screenshots** ([cod2-demos.md](../research/cod2-demos.md) 8).
`screenshot` and `screenshotJPEG` write `screenshots/shotNNNN.tga` (top-down)
or `.jpg` (quality 90) under the game folder, one count for both; a lone
argument names the file, `silent` drops the `Wrote` line. The shot holds
the frame before `r_gamma`, as D3D's front buffer does. `screenshot
levelshot` writes a resampled 128 x 128 `levelshots/<map>.tga`,
`screenshot savegame <name>` a 512 x 512 `<name>.jpg`.

### Server

`vcod-server <map> --game cod2` runs `cod2::Cod2Server` over
`game::cod2::Cod2Level` ([cod2-protocol.md](../research/cod2-protocol.md)
14).

**Handshake and housekeeping** (cod2-protocol.md 17):

- `getinfo` and `getstatus` answer byte for byte as retail's.
- The pure `cp` check (`sv_pure 0` turns it off), `g_password`, private
  slots (`sv_privateClients`, `sv_privatePassword`), a full server and
  `sv_minPing` / `sv_maxPing` refuse with retail's replies.
- `rate` and `snaps` pace off-LAN clients; a fragmented message goes a
  fragment a turn; a dropped client is a zombie for `sv_zombietime`; a
  silent one times out on `sv_timeout`; `sv_floodProtect`.
- Serves any file a client asks for from `fs_homepath`, then the game
  dir, over UDP (`sv_allowDownload`; stock paks refused as retail does),
  or redirects to `sv_wwwBaseURL` (`sv_wwwDownload`).
- `--set dedicated=2` heartbeats `master.cod.pm:20710` every three
  minutes.
- `g_log` writes 1.3's `games_mp.log`
  ([cod11-map-cycle.md](../research/cod11-map-cycle.md) 9).
- Retail `CoD2MP_s.exe` 1.3 joins it, pure or not: menus, spawn, move,
  fire, die to a bot, watch the killcam, kill an `addTestClient` client,
  scoreboard, chat, iwd download over UDP or HTTP, tdm to its score
  limit, sd past a round, a `map_restart` and a map change, with the same
  console lines as on a retail server (16).

**Scripts and gametypes.** The stock gsc runs on vcod's VM
([cod2-gsc-diff.md](../research/cod2-gsc-diff.md)). All five stock
gametypes (dm, tdm, sd, ctf, hq) run on every stock map: sd's bomb
planted and defused on the use key, ctf's flags taken and captured, hq's
radios held. `--gametype-script <file>` runs one off disk.

**Movement** ([cod2-movement.md](../research/cod2-movement.md)). Each cmd
runs `vcod_cod2::pmove` over the map's clip map, gated against retail
captures (`moves_ab`, section 17):

- Ladders, the gun put away on the rungs.
- Mantles over flagged walls, the gun down for the high ones, the climb's
  block keeping others off the ledge (9).
- A prone body's pitch follows the ground; steep ground forces a crouch
  (10). Foliage rustles (12).
- Players block each other; overlapping ones are pushed apart
  ([cod2-turrets.md](../research/cod2-turrets.md) 14.2).
- Bodies animate off the anim script's legs and torso picks, the
  weapon's fire, reload, melee and drop anims, and the hit flinches and
  stumbles. No turn anims, which retail leaves off
  ([cod2-player-anims.md](../research/cod2-player-anims.md) 9).

**Combat** ([cod2-combat.md](../research/cod2-combat.md), gated against
retail captures in section 9):

- Rounds strike the hit boxes of each body posed as retail's server
  poses it for the trace, through the antilag (9.5).
- The spread, the gun spring's kick and the anim picks draw from glibc's
  `rand()`, seeded each level as retail's `G_InitGame` seeds it, every
  draw where retail makes it (9.6). The script's `randomInt`,
  `randomIntRange`, `randomFloat` and `randomFloatRange` draw from the
  second generator the rockets' wobble draws from, seeded alike (2.6.1).
- Damage through the stock callbacks; death anims; corpses that fall,
  rest and slide on their death anim's root motion; the respawn.
- Offhand frags and smoke, rockets that wobble off their line as
  retail's do (not under `setStableMissile`) and their blasts, which pass
  the body or entity the rocket struck and stop at static models and
  entities as retail's do, the melee's swing with retail's reach and
  width, its corner traces, the script's `radiusDamage`, damage triggers,
  binoculars, the sniper's breath hold.
- The weapon a death drops, picked up by touch or swapped on the use key.
- The killcam replays the killer's view out of the engine's frame
  archive and ends on a use press
  ([cod2-killcam.md](../research/cod2-killcam.md)).

**Mounted guns** ([cod2-turrets.md](../research/cod2-turrets.md) 13, 14).
Use mounts and lets go; the barrel follows the view inside its arcs; the
trigger fires a round a frame. Stand, duck and prone guns (mg42 and
30cal) and the script's `spawnTurret` ones. A manned gun refuses a second
player; a gunner killed on the gun lets it go.

**Map cycle** ([cod2-map-cycle.md](../research/cod2-map-cycle.md)).
`map`, `devmap`, `map_restart`, `fast_restart` and `map_rotate` over
`sv_mapRotation`, the script's `map_restart(persist)` and `exitLevel`.
Joined clients follow.

**Client commands and rcon** (cod2-protocol.md 3.4.2, 17.7):

- Voice chat's relay under `sv_voice 1`: a talker's frames reach his
  team (everyone in dm, the dead only the dead), `voice_global`,
  `voice_deadChat` and `voice_localEcho` as retail's, `muteplayer` and
  `unmuteplayer`, `isTalking` (cod2-voice.md 3).
- Chat, `score`, `kill`, `where`, the spectator's follow (`follownext`,
  `followprev`, attack and sight; the follower gets the followed
  player's playerstate).
- `callvote` and `vote`, `g_oldVoting`'s engine vote or the script's.
  Bots never vote.
- Under `sv_cheats`: `god`, `demigod`, `notarget`, `noclip`, `ufo`,
  `give`, `take`, `setviewpos`.
- rcon: `status`, `serverinfo`, `systeminfo`, `dumpuser`, `say`, `tell`,
  `kick`, `clientkick`, `tempBan*` (for `sv_kickBanTime`), `banUser`,
  `banClient`, `unbanUser` into `ban.txt` (`--ban-file` keeps it), and
  the map commands. Each runs from stdin too (cod2-protocol.md 17.8).

### Bots

`--bots N [--bots-shoot]` adds bots that join through the stock menus and
play every stock gametype with vcod's shared bot brain
([bots.md](../bots.md#cod2)):

- Plant and defuse in sd; take, return and capture flags in ctf; take
  and hold the radio in hq.
- Throw offhand frags; throw smoke to cover a plant, a defuse or a
  retreat.
- Man a mounted gun with an enemy in its arc in dm and tdm.
- Climb the ladders and mantles their graph proved.
- Fight from cover with CoD2's leans, retreat when hurt, turn on a hit
  from out of sight.
- `addbot`, `removebot` (`kickbot`), `listbots` and the `vc_bot_*` cvars
  work as on CoD1.

### Tools and test gates

Retail is the oracle; these replay retail captures on vcod or sweep the
stock content. Most need `COD2_DIR` and pass vacuously without it.

| Tool | What it checks |
|---|---|
| `moves_ab` (`crates/cod2/tests`) | pmove against retail move captures ([cod2-movement.md](../research/cod2-movement.md) 17) |
| `trace_ab`, `examples/trace_probe.rs` | the clip map's traces against retail ([cod2-d3dbsp-format.md](../research/cod2-d3dbsp-format.md)) |
| `cod2_combat_ab`, `cod2_hitpose_ab`, `cod2_antilag_ab`, `cod2_pickup_ab` | shots, hit poses, antilag and pickups ([cod2-combat.md](../research/cod2-combat.md) 9) |
| `cod2_killcam_ab`, `cod2_turret_ab` | the killcam and mounted guns |
| `cod2::wire_ab` | the server's wire against a retail session (cod2-protocol.md 15) |
| `cod2_map_census` (ignored) | every stock map through the server, a spectating client, a headless frame and a walk from each spawn |
| `tools/cod2_soak.sh` | per map, six bots through dm, tdm, sd, ctf, hq with the client playing ([cod2-client-perf.md](../research/cod2-client-perf.md), "The soak") |
| `GAME=cod2 tools/cod4_perf.sh` | client frame time ([cod2-client-perf.md](../research/cod2-client-perf.md)) |
| `GAME=cod2 bot_match` | seeded bot matches ([bots.md](../bots.md#cod2)) |
| `tools/run_probe_cod2.sh` | retail `cod2_lnxded` with a movement probe gametype; `RAND_TRACE=1` logs its every `rand()` |

## Missing compared to retail

Ranked by how much a player notices, worst first.

1. **PunkBuster and CD key checks.** vcod runs no PunkBuster. vcod-server
   never asks the authorize server: an off-LAN client gets its challenge
   at once and its IP stands in for its ban guid. It does not send the
   master the status retail sends every 10 minutes. The client reads no
   CD key from the registry; set `VCOD_CDKEY` or enter it in the CD key
   menu, which keeps it until vcod quits ([usage.md](../usage.md), "CD
   keys").
2. **Download names.** vcod-server refuses a `download` name with a `..`
   part and never reads from the filesystem root; retail serves both
   (cod2-protocol.md 17.5). A stock client never asks for either.
3. **Voice options.** `winvoice_mic_reclevel` scales the captured voice
   where 1.3 sets the Windows mixer's recording volume;
   `winvoice_mic_mute` (the mixer's playback mute for the microphone,
   which vcod never plays back), `winvoice_save_voice` and the options
   page's record level meter do nothing
   ([cod2-voice.md](../research/cod2-voice.md) 4, 7).

Pending human checks: the in-game script menus have not been checked on
a GPU, and sound and voice chat have not been compared by ear.

## Beyond retail

- Bots, on every stock gametype. Their smoke throws are vcod's own rule:
  CoD1's bots throw none ([bots.md](../bots.md#cod2)).
- The browser asks `master.cod.pm` for its Internet list.
- `VCOD_NETSIM`, the F3 net and prediction lines, and the headless
  `--net-probe` and `--probe-demo` clients.
- `r_vc_depthprepass` (default 1 on CoD2): a depth-only pass before the
  lit world, same pixels, faster on most GPUs.

## Debug and developer modes

- `vcod <map> --game cod2` flies a map offline: world and static models,
  the sky cube, the sun, lightmapped surfaces with their normal,
  specular and detail maps, and the map script's fog. It runs no game.
- `vcod --game cod2 --net-probe ip:port` joins headless and prints a line
  a second; `--probe-team` / `--probe-weapon` play a 16 s scripted loop.
  `VCOD_COD2_RECORD=<dir>/<name>` records the run.
- `vcod --game cod2 --probe-demo <file.dm_1> [--probe-timedemo]` plays a
  demo headless.
