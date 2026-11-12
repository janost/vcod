# Call of Duty 4

vcod plays and hosts Call of Duty 4: Modern Warfare multiplayer, patch 1.7
only (the wire's protocol 6, told apart from CoD1 1.5's by the game). It
needs a CoD4 install: `zone/<language>/*.ff` fastfiles beside `main/` and
its iwds. Put the binaries in it, or point `--game-dir` or `COD_DIR` at
it (`COD4_DIR` is only for tests and tools). The game is detected
from `zone/` or `iw3mp.exe`; `--game cod4` forces it. How to run each
mode is in [usage.md](../usage.md).

Research docs live in [docs/research](../research/); the links below name
the section that measured each claim.

## What works

### Client

**Front end** ([cod4-front-end.md](../research/cod4-front-end.md)). Bare
`vcod` opens 1.7's main menu, drawn and run from the `ui_mp` zone's menus
with its `mouse_over`, `mouse_click` and `music_mainmenu_mp` sounds.

- Join Game: the `pc_join_unranked` browser. Local (a `getinfo`
  broadcast to 28960-28963), Internet (`getservers 6 full empty` to
  `masterServerName`, `master.cod.pm` by default) and Favorites
  (`servercache.dat`; vcod writes only the favourites). Pings, the filter
  popup, column sorts, Server Info's status, New and Del. Favorite, the
  password popup. The master is asked only on Refresh List
  ([cod4-handshake.md](../research/cod4-handshake.md) 12).
- Start New Server starts `vcod-server --game cod4`; Dedicated No joins
  it. Server Name and Password are typed into it. In such a game the
  Graphics page greys as on 1.7's listen server.
- Mods runs a `mods/<name>` mod.
- Select Profile lists, creates, deletes and loads the profiles in
  `players/profiles/`. A fresh install asks for one first; the active
  profile holds the config.
- Options and Controls set what vcod has a cvar or bind for; the rest
  draw greyed.
- Esc on the connect screen, or `disconnect`, calls the join off. Leaving
  a game or a server's drop returns to the menu with the reason in
  `error_popmenu` (7).
- Aspect Ratio (`r_aspectRatio`, at `vid_restart`) projects at 4:3,
  16:10 or 16:9 over the whole window, as 1.7 does; fullscreen stretches
  the HUD with it (10).

**Mods, usermaps, downloads** ([cod4-fastfile.md](../research/cod4-fastfile.md)
13, [cod4-handshake.md](../research/cod4-handshake.md) 11):

- `--fs-game mods/<name>` or a joined server's `fs_game` loads the mod's
  `mod.ff` and iwds over the stock zones and `main/` in 1.7's order. The
  HUD and script menus read `mod.ff` too (ModWarfare's class menus).
  Prediction uses the mod's weapon files off its iwds, as the server does.
- A map missing from `zone/<language>/` loads from `usermaps/<map>/`.
  `<map>_load.ff` gives the connect screen its levelshot.
- Missing mod or usermap iwds and fastfiles download before the map
  loads (`cl_allowDownload`, on by default), over UDP or a www redirect's
  `http://` under `cl_wwwDownload`; a failed fetch falls back to UDP. A
  file still different on the next gamestate ends the join, as on 1.7.

**Joining and the script menus** ([cod4-hud.md](../research/cod4-hud.md) 12):

- `--connect` or the browser joins a stock 1.7 server (retail or
  vcod-server), stats phase and pure `cp` included, as a player or a
  spectator.
- The server's script menus draw from the zones as 1.7 draws them: the
  team menu, `changeclass`, `changeclass_offline`, with rects, materials,
  text and visibility expressions over the dvars, the profile's stats and
  the string tables. Every expression of the 288 menus the HUD's zones
  and mp_crash carry evaluates (12.2). Mouse, arrows and Tab move focus;
  click or Enter picks; Esc runs `onEsc`.
- With no menu up, Esc opens `g_scriptMainMenu`: the team menu, then the
  in-game `class` menu (Choose Class, Change Team, Controls, Options, Call
  Vote, Mute Players, Leave Game).
- Controls rebinds a row on a click and the next key, saved to
  `vcod_mp.cfg`. Options sets sensitivity, invert, free look, smoothing,
  `r_gamma`, `sm_enable`, `cg_marks`, volume, downloads, name, video mode,
  refresh, anti-aliasing, sync, aspect, glow, specular, dynamic lights,
  model detail, water animation, texture filtering and quality,
  anisotropy, blood, console, depth of field, soft smoke edges and
  ragdoll (12.6).
- Call Vote sends retail's `callvote` (restart, next map, a map, a game
  type and map, a temp ban). Mute Players mutes a player's chat and voice.
- `--team` and `--class` (e.g. `assault_mp,0`) answer the menus on their
  own.

**Input.** A cmd a frame off the binds: move, sprint, jump, the stance
binds (all but `+stance`), attack, the sight (`+speed`, `toggleads`,
`+toggleads_throw`), melee (V),
use, reload, lean, hold breath (`+breath_sprint`), frag and smoke,
`weapnext` / `weapprev`. `+actionslot` switches to the grenade launcher,
the slot's weapon or the night vision goggles. T and Y open the say field.
The weapon byte is the client's own pick
([cod4-client-view.md](../research/cod4-client-view.md) 9,
[cod4-hud.md](../research/cod4-hud.md) 12.6).

**Netcode and prediction**
([cod4-client-view.md](../research/cod4-client-view.md)):

- The snapshot view on retail's client clock (`CL_AdjustTimeDelta`) and
  lerp, with its teleport cuts.
- Our own body and a free spectator are predicted: cmds replayed through
  `vcod_cod4::pmove` over the map's clip map and the other players'
  boxes. Corrections ease over `cg_errordecay`'s 100 ms, stair steps over
  `cg_viewZSmoothingTime`. Mantles and the weapon half predict too, so
  our own shots, reloads, switches and footsteps play the frame the
  predicted move raises them, and not again from the snapshot. Item
  pickups stay the snapshot's (5).
- A lean moves the eye sideways as 1.7's does (7).

**Bodies and entities**
([cod4-player-anims.md](../research/cod4-player-anims.md)):

- Players and corpses posed from `legsAnim` / `torsoAnim` with heads and
  held weapons. Aim pitch, lean and prone pitches bend the spine and head
  as 1.7's controllers do.
- Corpses go ragdoll on the server's `startRagdoll`: `ragdoll.cfg`'s
  bodies under a position-based solver. Grenade, rocket and script
  blasts push them; bullets do not, as on 1.7 (10).
- Dropped weapons, grenades and movers on their trajectories. Script
  models without their `hidePart` parts, so a damaged car draws intact or
  cracked glass, never both (cod4-client-view.md 15).
- The airstrike's jets and the helicopter with turning rotors, rotor
  wash, damage-stage smoke and fire, and `playFxOnTag` effects on any
  model's tags (11).
- Mounted guns with barrels at the wire's gun angles and their fire anim.
  A gunner's eye sits at `tag_player`; other gunners sit on their guns.

**View weapon and view**
([cod4-client-view.md](../research/cod4-client-view.md) 4, 14):

- The held weapon's view model at `cg_fov` 65 with the sight's zoom. Its
  clips follow `weapAnim` (idle, fire, reload, raise, drop, sprint, melee
  with the knife) under the sight layer. `hideTags` hides missing
  attachments. A reflex sight's dot projects off the view angle.
- Move offsets, idle drift, bob and sway; own flash and shell eject on
  `tag_flash` and `tag_brass`; recoil spring, damage kick, landing dip.
- Shot recoil kicks the view through the cmd's angles; a hit kicks it
  along its direction.
- Shellshock from the zones' `.shock` records: a flashbang's or
  concussion's white-out over a still of its first frame, the camera
  kick, the concussion's mouse caps, the flashbang's tinnitus and sound
  mix (8). Earthquakes shake the view.

**Rendering** ([cod4-world.md](../research/cod4-world.md)):

- World and static models out of the map's zone: textures, lightmaps,
  the sun, the sky cube, blends and alpha tests, doorway lighteners and
  light shafts' angle falloff, culled by cells and portals. A census
  holds every world and static model material of the 21 stock maps to
  its retail state bits.
- Normal, specular and detail maps under the sun. Static models lit off
  the light grid as the `lp_*` techniques read it; specular sets add
  their reflection probe. A surface on a spot or omni primary light takes
  it in the sun's place.
- Sun shadows (`sm_enable`, default 1): world, static models and
  entities in retail's two-partition sun shadow map (3.6).
- Spot light shadows: up to four in view (`sm_maxLights`), fading in and
  out (`r_spotLightShadows`, `sm_spotEnable`; 3.5).
- Exp fog (`setExpFog`) and the vision set's film tweak and glow:
  `mpintro`'s grey easing to the map's over 2 s, and the night set under
  night vision. `r_glow 0` and `r_fog 0` are cheats, as on 1.7 (11).
- Water as 1.7's `water_l_sun` over a moving height map. The world's
  distortion glass (mp_overgrown's bricks) and `zfeather` sets
  (floodlight beams, lens flares) (9).
- Depth of field down the sight (`r_dof_enable`), heat haze from
  explosions, fires and muzzle flashes (`r_distortion`), and `zfeather`
  sprite fades (`r_zFeather`)
  ([cod4-effects.md](../research/cod4-effects.md) 14, 17).
- `vc_windowAspect 1` (vcod's) projects at the window's own aspect
  instead (see Beyond retail).

**HUD** ([cod4-hud.md](../research/cod4-hud.md)):

- Hudelems in the zones' fonts with glow. Kill feed with weapon icons,
  `iPrintLn` and `iPrintLnBold`, chat, the obituary's centre print,
  `[{+cmd}]` key names.
- Rotating minimap compass: teammates (kept 1.5 s after they drop out),
  `pingPlayer` flashes, talkers, radar enemies, the UAV sweep, shot
  flashes held for the fire sound, objectives, the tickertape, the
  helicopter and airstrike planes (15).
- Weapon HUD: the clip as bullets, weapon name, offhand icons and
  counts, a low clip's red pulse, a low reserve in red. The action slots
  (night vision, grenade launcher, earned hardpoints).
- Low-health overlay and hit direction icons (19), cursor hints with key
  names (20), grenade indicators (21), mantle, hold breath and invalid
  command hints, XP and score bars (23).
- The crosshair with the weapon's spread (Steady Aim's narrower), scope
  overlays, a mounted gun's reticle, `cg_crosshairDynamic` (17).
- World waypoints (Bomb Squad's icon over enemy claymores and C4) and
  offscreen ones with a pointer.
- The killcam's letterbox and title; the prematch's countdown text. The
  compass and ammo counter hide under `ui_hud_hardcore`.
- The scoreboard on Tab and at intermission: header, team scores, game
  type, time left, talking icons, scrolling, `cg_scoreboardPingGraph`
  (7).
- The full map while picking an airstrike's location.
- The connect screen: levelshot, glowing titles, stage line, load bar,
  download rate and time left, a server's `print` and the CD key wait
  (18).
- The drop-down console in `consoleFont`; `say`, `cmd` and unknown
  commands go to the server.

**Sound** ([cod4-sound.md](../research/cod4-sound.md)). Zone and iwd
sounds through each alias's falloff curve and, on 2D rows, its speaker
map's stereo level: footsteps (Dead Silence's quiet ones), landings,
shots at the shooter's `tag_flash`, reloads, hits, bounces, explodes, the
knife, the map's ambient, entity loop sounds (a brush model's at its
middle), `playSound` and `playSoundAsMaster`. `EV_STOPSOUNDS` stops an
entity's voices. A voice started off its entity turns round it.

**Effects** ([cod4-effects.md](../research/cod4-effects.md)), on the
CoD2 effect path:

- Muzzle flashes and shell ejects on the posed weapon's tags, impacts,
  explosions, effects emitted along an element's flight, missile smoke
  trails on `tag_fx`.
- Looping effects for their whole life: `spawnFx`, `playLoopedFx` and
  the map's createfx ambience (mp_crash's fires start settled). Each
  element spawns inside its `spawnRange` and fades over its distances.
- Client-side bullets: every round traced from the eye (ours) or the
  muzzle (others') with the server's seeded spread, stopping on players'
  boxes and script models, piercing the world with exit impacts (Deep
  Impact deepens it), with impact sounds, holes, `whizby` and 1.7's
  corkscrew tracers (12).

**Voice chat** ([cod4-voice.md](../research/cod4-voice.md)). `+talk` (Z)
sends the microphone through the system's libspeex (narrowband, 8000 Hz,
`sv_voiceQuality`); talkers play, and the talker list and scoreboard
show who talks. Without libspeex nothing is sent or played.

**Demos** ([cod4-demos.md](../research/cod4-demos.md)). `record` writes
1.7's typed records into `main/demos/` (the mod's under `fs_game`).
`demo <name>` and `timedemo <name>` load the map behind the connect
screen and play it unpredicted, the recorder's view off its archive rows.

### Server

`vcod-server <map> --game cod4` runs `cod4::Cod4Server` over
`game::cod4::level::Cod4Level`. The level defaults to `war`
(`--gametype` or `--set g_gametype=` picks another).

**Handshake** ([cod4-handshake.md](../research/cod4-handshake.md)):

- `getinfo`, `getstatus`, the challenge, the connect and the seven stats
  packets as retail answers them; the gamestate with retail's engine
  configstrings and the script's; the pure `cp` check (9).
- Off the LAN it refuses on `sv_minPing` / `sv_maxPing` and holds a
  client out of the world to a message a second, as 1.7 does (15).
- `sv_fps` (10..1000, default 20) sets the frame, read every frame; the
  scripts' `wait` counts frames as on retail
  ([cod4-server-frame.md](../research/cod4-server-frame.md) 8).
- Mods (`--set fs_game=mods/<name>`) and their downloads over UDP
  (`sv_allowDownload`) or `sv_wwwBaseURL` (11). `--script-dir` reads
  loose `.gsc` files over the zones'.
- `--set dedicated=2` heartbeats `master.cod.pm` (or the master
  `sv_master1` names) every 180 s.

**Scripts** ([cod4-gsc.md](../research/cod4-gsc.md)). The stock gsc runs
on vcod's VM. Every builtin the stock scripts of the 21 stock maps call
has a handler (3.1).

- The connect, team and class menus (`mr`), the loadout off the
  client's stats block, the spawn, with retail's server commands in
  retail's order. A client on a team with no class follows a teammate (7).
- `setStat` sends 1.7's `N` to the client's profile.
- The script's HUD elements go out in the playerstate, matched element
  for element against retail captures
  ([cod4-hud.md](../research/cod4-hud.md) 5.1).
- Every perk acts: the scripts' damage, loadout and Last Stand ones, and
  the engine's (spread, reload, fire rate, breath, sprint, Deep Impact,
  Martyrdom's dropped grenade, Last Stand's down state, Eavesdrop)
  ("The perks").
- Looping and triggered map effects go out as `ET_LOOP_FX` and `ET_FX`.
- Hardpoints: the airstrike's three `ET_PLANE` jets and their bombs; the
  helicopter (`ET_HELICOPTER`) flies its path, aims and fires its cannon
  and rockets, pitches and rolls off its acceleration, wanders in the
  hover, and smokes and crashes when shot down (3.2).
- `bulletTrace` scores bodies' posed hit boxes and the helicopter;
  `getTagOrigin` reads a player's posed bone (3.3).
- The use key and touch pass reach game object triggers, `trigger_use`
  and dropped weapons (swap the gun in hand). A planter raises the
  briefcase (9).
- The six stock mounted SAWs (`misc_turret`, `ET_MG42`) show their hint,
  mount on use, hold the view in their arcs, fire off the turret's weapon
  file and let go on a second press, matching the retail mp_backlot
  capture (9.2). A live frag lying near a player shows its hint, and the
  frag key throws it back on its own fuse (9.3).

**Movement** ([cod4-pmove.md](../research/cod4-pmove.md)). Each cmd runs
1.7's pmove as it arrives, over the clip map and the other bodies. A
walker replaying a retail capture moves as on retail within the trace
kernels' float model (0.04 units over 40 s) (15). Mantles over
`MANTLE_ON` and `MANTLE_OVER` walls on 1.7's clips and timing, leaving
1.7's blockage for other players (17). Bodies carry the legs and torso
anims 1.7's anim script picks, with view angles, lean and prone pitches.
Spectators follow as `scr_game_spectatetype` allows: attack to the next
player, the sight to the one before, melee to fly free (15).

**Combat** ([cod4-combat.md](../research/cod4-combat.md)), matched
against a retail kill capture, entity numbers included:

- Bullet weapons fire, burst, reload and sight as `PM_Weapon` runs them.
- A round leaves the eye with walk bob and lean along the gun's angles
  (sway, movement tilt, idle drift, recoil spring), through retail's line
  on every round of a strafing, leaning capture (3.2).
- Rounds trace posed hit boxes through the antilag and penetrate walls
  and bodies by 1.7's depth table (a round through a mp_crash wall takes
  retail's damage). They meet script models' collision triangles and
  brush models, and go on past them (13.1).
- Destructible cars and barrels break under rounds and blasts (13).
- The stock damage and killed callbacks, the obituary, the corpse (death
  anim, set down on the ground, `startRagdoll` at retail's moment), the
  dropped weapon (its ammo handed to a player holding the same gun), and
  the killcam with the world as it was.
- Frag, smoke and flash grenades are pulled, cooked, thrown and bounce,
  off static models too. A replayed frag matches retail word for word,
  spin included: the server draws glibc `rand()` as 1.7's one stream
  (10.9). Blasts do radius damage; flashbangs blind and concussions shock
  and slow (10.8). The grenade launcher and the rocket launcher fire.
- Rockets wobble, curve and accelerate as 1.7's do; an RPG bends toward
  the helicopter's attractor; the helicopter's guided rockets steer.
- Melee swings and hurts; `kill` works under `sv_cheats` (10.4, 10.5).
- Retail captures gate a rocket's flight, a grenade's throw back, a
  prone death and a killcam on a helicopter-style kill (15). A hit's push
  and timers land a frame late, and a dropped weapon falls straight, not
  from the hand (12, 15).

**Voice** ([cod4-voice.md](../research/cod4-voice.md) 3). Relayed under
`sv_voice 1` (off by default, as retail): team, dead to dead,
`voice_global`, `voice_deadChat`, `voice_localEcho`, Eavesdrop's 400
units, mute lists, `isTalking()`.

**Map cycle** ([cod4-map-cycle.md](../research/cod4-map-cycle.md)).
`map`, `devmap`, `map_restart`, `fast_restart` and `map_rotate` over
`sv_mapRotation` (`server_map.cfg` fills it with the stock maps). A match
that ends rotates or restarts through the intermission's scoreboard,
ranked (`sv_pure 1`) included. `callvote` and `vote` run 1.7's engine
vote. A client that leaves or reconnects into its own slot leaves no
stale player in the scripts.

**Admin and logs** ([cod4-handshake.md](../research/cod4-handshake.md)
14, [cod4-server-gaps.md](../research/cod4-server-gaps.md)):

- `score` gets retail's `b` board ([cod4-hud.md](../research/cod4-hud.md) 7).
- rcon: `status`, `serverinfo`, `systeminfo`, `dumpuser`, `say`, `tell`,
  `clientkick`, `kick` (bans for `sv_kickBanTime`; `kick all`),
  `onlykick`, `tempBanClient`, `tempBanUser`, `banUser`, `banClient`,
  `unbanUser` (`ban.txt`, `--ban-file`), `heartbeat`, `killserver`, the
  dvar commands (`set`, `seta`, `setu`, `toggle`, `togglep`, `reset`,
  `setfromdvar`, `dvar_bool`/`int`/`float`, `setdvartotime`),
  `writeconfig`, `vstr`, `exec` (a zone's rawfile, then iwds and files
  in 1.7's order), `cmdlist`, and the map commands.
- `g_log` writes 1.7's `games_mp.log`: `InitGame`, `ShutdownGame`,
  `ExitLevel`, chat and weapon lines and the scripts' `J;`, `Q;`, `D;`,
  `K;`. `g_inactivity` warns and drops idle players.

### Bots

`--bots N [--bots-shoot]` adds bots that join through the team and class
menus and play with vcod's shared bot brain
([bots.md](../bots.md#cod4)):

- Find their way over a graph of the map's clip map, mantles included.
- Play the `sd` and `sab` bomb (take, carry, plant, defuse), `dom`'s
  flags and `koth`'s radio, and rejoin each new level.
- Man a SAW covering a lane enemies use; throw a live frag at their feet
  back.
- See an enemy only past every car that would stop the round.
- `addbot`, `removebot` (`kickbot`), `listbots` and the `vc_bot_*`
  cvars work as on CoD1.

### Tools and test gates

Most need `COD4_DIR` and pass vacuously without it.

| Tool | What it checks |
|---|---|
| `moves_ab` (`crates/cod4/tests`) | pmove against retail move captures ([cod4-pmove.md](../research/cod4-pmove.md)) |
| `cod4_combat_ab`, `shot_aim_ab` | shots, damage and kills against a retail capture ([cod4-combat.md](../research/cod4-combat.md)) |
| `cod4::wire_ab` | the server's walk replay and follow against retail |
| `hardpoint_ab`, `planefx_ab`, `prematch_ab`, `oob_ab` | hardpoints, plane effects, the prematch HUD, connectionless replies |
| census tests (`crates/cod4/tests`) | every stock map's materials, clip, cull, sun shadow, sounds, effects, HUD assets |
| `cod4_live_*` | a vcod-server and client in process on a simulated clock |
| `cod4_map_sweep` (ignored) | every stock map under war, dm, sd, sab, dom and koth with bots and a playing client ([cod4-map-sweep.md](../research/cod4-map-sweep.md)) |
| `tools/cod4_soak.sh` | the client playing a whole match per map through `main`'s frame |
| `tools/cod4_perf.sh` | client frame time, `SWEEP=1` per setting ([cod4-client-perf.md](../research/cod4-client-perf.md)) |
| `GAME=cod4 bot_match`, `nav_build` | seeded bot matches and the bots' graph ([bots.md](../bots.md#cod4)) |

## Missing compared to retail

Ranked by how much a player notices. The research-side lists are
[cod4-gaps.md](../research/cod4-gaps.md) (client) and
[cod4-server-gaps.md](../research/cod4-server-gaps.md) (server).

1. **CoD4x servers.** Most public CoD4 servers run CoD4x, which parks a
   1.7 client and hands it to CoD4x's own update backend. vcod refuses
   them at the challenge and hides them in the browser unless
   `ui_browserShowCoD4x` is 1 ([cod4-handshake.md](../research/cod4-handshake.md)
   13). vcod joins stock 1.7 servers only, which are rarer than they
   should be.
2. **Ranks and unlocks start fresh.** vcod keeps its own stats file per
   profile (`vcod_mpdata`) and never reads retail's `mpdata`. The main
   menu reads a fresh profile's stats, so Create a Class is greyed and
   Rank & Challenges shows nothing earned
   ([cod4-front-end.md](../research/cod4-front-end.md) 4). Your retail
   rank is safe, and no use here.
3. **Physics debris stays put.** A `physicsLaunch`ed part (a car's door
   or hood) keeps the spot it stood on: the client runs no rigid-body
   physics for `TR_PHYSICS`. Ragdolls use vcod's position-based solver,
   not ODE, so corpses fall differently
   ([cod4-gsc.md](../research/cod4-gsc.md) 3.2,
   [cod4-player-anims.md](../research/cod4-player-anims.md) 10).
4. **View weapon details.** No camo; every gun draws its first model.
   The rocket on a launcher and the goggles model are not drawn, and
   Sleight of Hand does not speed the view clips.
5. **Mantle anims on others.** The server's anim script skips the mantle
   movetypes, so another player climbing a wall plays no mantle anim
   ([cod4-pmove.md](../research/cod4-pmove.md) 17.5).
6. **Stereo only.** A 5.1 or 7.1 device gets the stereo mix on its front
   pair ([cod4-sound.md](../research/cod4-sound.md) 5). 139 alias rows
   whose curve no zone holds fall off straight.
7. **No third-person view**, so our own third-person tracer start is not
   modelled ([cod4-effects.md](../research/cod4-effects.md) 12.3).
8. **Demos untested against retail.** No retail demo has played in vcod,
   nor vcod's in `iw3mp.exe`. The `RECORDING` line draws at CoD2's place
   ([cod4-demos.md](../research/cod4-demos.md) 7).
9. **Small menus.** The CD key menus are missing; Single Player's Yes
   does nothing (this is a multiplayer client); the `winvoice_*` voice
   settings are not read.
10. **Server-side differences.** vcod-server never asks the authorize
    server, so it answers an off-LAN challenge at once where 1.7 waits on
    the authorize server (cod4-handshake.md 15). The helicopter's flight
    has no move trace or touch and its collision traces at the bind pose
    (cod4-gsc.md 3.2). No PunkBuster (a stock dedicated server runs none
    either).

Pending human checks: pixel and by-ear comparisons with retail.

## Beyond retail

- Bots on war, dm, sd, sab, dom and koth.
- `vc_windowAspect 1` projects at the window's own aspect, Hor+ off the
  4:3 view, with the HUD unstretched: compass, score and weapon info at
  the window's edges, centred pieces in the middle
  ([cod4-front-end.md](../research/cod4-front-end.md) 10).
- Esc on the connect screen calls the join off. 1.7's Esc does nothing
  there, which never made sense.
- The browser's Internet list comes from `master.cod.pm`.
- `r_vc_depthprepass` (default 1): a depth-only pass first, which also
  writes the feathered effects' `floatZ`, so the frame needs no depth
  copy ([cod4-client-perf.md](../research/cod4-client-perf.md) 12, 35).
  `r_vc_normalMaps` and `r_vc_film` are debug switches for the perf sweep.
- `VCOD_FRAME_PROFILE` and `tools/cod4_perf.sh` measure the frame.

## Debug and developer modes

- `vcod <map> --game cod4` flies a stock map offline: the world, the
  map's createfx effects (settled as on a level a minute old), its
  `ambientPlay` loop, fog and vision set. It runs no game.
- `vcod --game cod4 --capture <file>` replays a `cod4_probe` datagram
  capture; the fly keys move the camera.
- `vcod --net-probe ip:port --game cod4 [--team T] [--class C]` joins
  headless and prints the menus the server opens and the spawn.
