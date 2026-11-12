# Call of Duty 1.1 and 1.5

Call of Duty (2003) multiplayer, patch 1.1 (protocol 1) and patch 1.5
(protocol 6). This is where vcod started; CoD2 and CoD4 came later. Both
patches run off one code path, and where 1.5 differs, the line says so. United Offensive has its own page: [uo.md](uo.md). The other games and
what they all share are in [../features.md](../features.md).

## What works

### Client

Front end:

- Opens on the stock main menu and server browser, drawn from the game's
  own `ui_mp/*.menu` files.
- The browser's three sources are retail's. Local is a `getinfo`
  broadcast to ports 28960-28963. Internet is what `master.cod.pm` lists
  on port 20510 for the install's protocol (1 on 1.1, 6 on 1.5), asked at
  most once a second; a server whose `getinfo` names another protocol is
  dropped. Favorites live in `servercache.dat` beside `CoDMP.exe`, retail's
  file.
- The browser pings, filters (game type too), shows each source's last
  refresh, sorts by column, joins on double-click, and runs the stock
  password, server info, filter and favourite popups.
- Losing the server drops you back on the menu with the localized reason
  in the stock error popup.
- In a game, Esc opens the script menu; its Main Menu tab opens the stock
  in-game menu (Back to Game, Disconnect).
- The stock Mods menu lists the mod directories beside `main/` and
  switches to one, in a game too (not while a server you started with
  Dedicated No runs, as retail refuses). Menus load from the mod's
  `ui_mp/menus.txt` and `ui_mp/ingame.txt`, each tried under
  `cl_language`'s directory first (latched until `vid_restart`, as retail).
- Start New Server runs the stock create server menus (game type, the maps
  `mp/*.arena` lists for it, server and game type settings), then starts
  `vcod-server` from `vcod`'s directory with those settings and the Mods
  menu's mod. The server logs to `vcod-server.log` beside `vcod_mp.cfg`.
  Dedicated No joins it once it answers and stops it on disconnect; LAN
  and Internet leave you in the menus. The client kills any server it
  started when it exits.
- The stock Options and Multiplayer Options screens: binds, sensitivity,
  invert, free look and smoothing (`cl_freelook`, `m_filter`, `+mlook`,
  `+strafe`), name, rate, volume, video mode and full screen, brightness,
  texture detail, filter and quality, dynamic lights, sync, blood and
  sound quality, each applied at the restart retail applies it at. Choices
  persist in `vcod_mp.cfg`; settings vcod has no use for are kept and do
  nothing.
- CD key authorization as the retail client does it: `getKeyAuthorize` to
  the authorize server before the challenge, off the LAN only, with the
  key from `VCOD_CDKEY` or 1.1's `main/codkey`, and the stock `needcdkey`
  and refusal messages ([usage](../usage.md#cd-keys)).

Joining:

- Joins 1.1 and 1.5 servers from either install, switching protocol when
  the server refuses the install's connect. A pure 1.5 server wants a 1.5
  install as `COD_DIR`
  ([cod15-protocol-diff.md](../research/cod15-protocol-diff.md)).
- Downloads the paks a server has and you don't, matched by checksum.
  A 1.5 server's `sv_wwwBaseURL` redirect is fetched over plain `http://`
  (`cl_wwwDownload 0` turns it down; same doc, 14). A pak whose checksum
  is wrong is deleted before anything loads it; a bad redirected one is
  asked for again and a later redirect of it refused, so the server sends
  it over UDP.
- Sends pure servers its pak checksums.
- Takes every systeminfo key into its cvars as retail does
  (`cl_allowDownload`, `rate`, `snaps`, `fs_game`, ...). A forced
  `fs_game` layers the mod over `main/`, swaps in its menus and stays
  after you leave, as retail's does. A mod's script menus and `hud.menu`
  text show up.
- Follows a map change: loading screen, downloads, new map. Open menus
  close.

Playing:

- Move, jump, crouch, prone, lean, fire, sight, reload, melee, use and
  switch weapons on retail's default binds. Every stance bind the Options
  menu offers works as retail's: held crouch and prone (`+movedown`,
  `+prone`), `+moveup`, `togglecrouch`, `toggleprone`, `lowerstance` and
  `raisestance`. A usercmd is built every
  8 ms, like a 125 fps retail client, sent at `cl_maxpackets` 30 with
  `cl_packetdup` 1 off the LAN.
- Keeps its own weapon selection as `cg_weaponSelect` does: answers the
  server's switch and pickup commands, swaps off a dry weapon, takes
  `weapalt` and `weapon <name>`, and asks again for the gun a ladder took
  away. A select shows retail's weapon-slot strip.
- Predicts your movement by replaying unacknowledged cmds through the
  server's own step: free spectator, corpse (a linked one too) and
  intermission included. A correction eases out over 100 ms. On 1.5 it
  runs 1.5's movement (jump penalty, no prone dive, the stepped stance,
  the prone swing search, the footstep changes, `disableWeapon`) and the
  server's bob cvars ([cod15-movement.md](../research/cod15-movement.md)).
- Prediction carries you on a moving brush model and turns your view with
  its yaw, but never turns your position about it. Neither does retail's.
- Runs retail's client clock: it slews a millisecond or two per snapshot
  instead of stepping on jitter. `cl_timeNudge` works.
- The first-person weapon has your team's hands, sight zoom at the
  weapon's fov, and a sniper scope overlay that follows the sway and the
  hit kick. Your own fire, reload and footstep sounds play off the
  prediction. The gun bobs, breathes, lags a turn and shifts with speed
  and stance off the weapon file's keys; the eye bobs with the walk.
- Fire recoil off the weapon file's kick keys rides your cmd angles,
  rolls the view, springs back and clears on a respawn. A hit kicks the
  view along its direction, a landing dips it, and `earthquake` or a
  mounted MG's fire shakes it, on retail's curves.
- Mounted MG42s draw their fire anim and flash. On the gun the view rides
  `tag_player`, a hit still kicks it, and the crosshair becomes the gun's
  reticle. On 1.5 the view is held inside the gun's arcs, the gun turns
  with it and jitters as it fires.
- A script's `shellShock` plays retail's effect from its `.shock` file:
  view trail and shake, damped mouse, the walk slowed to 0.4, the mix
  ducked under the shellshock loop. `cg_shellshock <secs> [file]` tries
  one locally.

HUD and menus:

- The HUD the stock `hud.menu` lays out: crosshair that opens with
  spread, health, ammo, stance, compass with objectives and teammates,
  retail's pick-up and swap hints, hit direction with the red flash,
  chat, announcements, the "You killed" centre print.
- The gametype script's HUD elements (S&D clock, bomb icons, progress
  bar) in retail's fonts. Head icons over players, the talk balloon on a
  teammate typing or in a menu, the disconnected icon, the killcam marker.
- A spectator following someone sees their HUD, weapon and scope.
- On 1.5, `^8` and `^9` draw in the server's `g_TeamColor_Allies` and
  `g_TeamColor_Axis`, as 1.5's team chat uses them.
- The server's script menus (team, weapon, view map, call vote, quick
  messages) from their stock `.menu` files, driven by mouse and keys, or
  answered straight from `--team` and `--weapon`.
- Retail's scoreboard on Tab and through intermission: game type, map,
  host name and address, objective, team banners leading team first with
  totals and average ping, rows in the server's order with status icons,
  your row lit, and a scroll bar (page keys, wheel, `weapnext` /
  `weapprev`) once it overflows
  ([cod11-hud-protocol.md](../research/cod11-hud-protocol.md) 3.1).
- The stock quick message menu (V): the gametype script plays the voice
  line and sends the team chat line.
- A running vote shows as retail's cgame shows it (subject, seconds left,
  yes and no counts with the keys) and plays its sound. F1 and F2 vote;
  the call vote menu's restart, next map, map, type-and-map and kick picks
  call one.
- Retail's drop-down console on `` ` `` with the commands under
  [Console](../usage.md#console), `rcon` among them.

Spectating:

- Every player is an assembled, animated soldier. Kill feed, chat,
  scoreboard, sounds, tracers, impacts, muzzle flashes, ejected brass and
  script effects (S&D bomb, bombzone exploders, mines, `playLoopedFx`,
  `playFxOnTag`) come off the events retail reads.
- Tracers and another body's fire sound start at the shooter's posed
  muzzle tag (a mounted gun's own, a gunner's stance point) and turn with
  it; your own start at the viewmodel's muzzle. On 1.5 every body's spine
  and lean ease toward the view as 1.5's do.

Rendering and sound:

- Draws stock and custom maps with textures, lightmaps and props. Skies,
  water, fences, foliage and terrain blends run through the maps' own
  shader scripts, sun disc and fog included. Lamp glows face the camera
  (`autosprite`), flags flap, and a `.sun` file's flare, glare and blind
  follow the sun's visibility; `r_drawSun 0` hides the sun. `corona`
  entities glow around their lamps seen from below.
- Culls with retail's cells, portals and occluders. F4 freezes or disables
  culling so you can fly out and see what the camera drew.
- Models switch LODs with distance, zoom and Model Detail by retail's rule.
- Props are lit per vertex from the map's lights and light-visibility grid
  at load. Players, entity models and the viewmodel pick eight lights from
  the grid each frame, fx lights and the panzerfaust's rocket light among
  them ([cod11-light-grid-and-leaf-lights.md](../research/cod11-light-grid-and-leaf-lights.md)).
- The frame holds retail's framebuffer bytes and the gamma ramp doubles
  it once at the end; windowed it draws as retail's windowed mode, no
  doubling, lightmaps shifted at load ([cod11-gamma.md](../research/cod11-gamma.md)).
- Entity models and the viewmodel draw every stage of their material and
  sort among the world's surfaces by material sort. The world draws its
  shaders in retail's sorted order, stages together, no depth sort;
  effects join the same list.
- `deformVertexes` draws `wave`, `flap`, `bulge`, `move`, `normal`,
  `autosprite` and `autosprite2`, up to three per shader in script order.
  Entity models take `skins/` blocks and the same deforms but the sprites.
  A `type@` skin with a missing template or image draws the default
  shader. Dynamic lights go through retail's dlight pass
  ([cod11-shader-scripts.md](../research/cod11-shader-scripts.md)).
- Impact marks clip onto the world's triangles on the CPU, take the
  surface's lightmap, fade over the second half of their life and draw at
  their shader's sort and polygon offset, 1024 pieces at most.
  `map_restart` clears them, live effects, sounds, the objective text,
  centre print, vote lines, hit icons and message windows, as
  `CG_MapRestart` and the engine's `n`/`B` case do. `cg_shadows 1` (a
  cheat, off by default) draws the `markShadow` blob under players
  250-1024 units away
  ([cod11-events-and-fx.md](../research/cod11-events-and-fx.md) 9).
- Plays the map's ambient loop. Audio follows the engine on paper:
  falloff, panning, voice pools, stealing, ducking.

Demos:

- `record [name]` writes retail's demo format (`demos/<name>.dm_1` on 1.1,
  `.dm_2` on 1.5; no name counts up from `demo0000`) with retail's
  `RECORDING` line; `stoprecord` closes it. `demo <name>` plays either back
  through the online frame, the recorder drawn as a followed player.
- `timedemo 1` steps 50 ms a frame (paced by `com_maxfps`) and prints the
  frame rate. `cl_freezeDemo 1` holds the clock, `nextdemo` runs at the
  end, any key opens the main menu. A missing map is retail's
  `Couldn't load maps/mp/<map>.bsp`. `cg_autodemo 1` records on a 1.5
  server's `autoDemoStart` ([cod11-demos.md](../research/cod11-demos.md)).

### Server

Wire and housekeeping:

- Answers server browsers, accepts retail 1.1 clients, sends the gamestate
  and delta snapshots.
- Run from a 1.5 install it speaks 1.5 (protocol 6): 1.5's queries,
  cvars, menus and gametype scripts, and the configstring table 1.5b
  builds on mp_carentan
  ([cod15-protocol-diff.md](../research/cod15-protocol-diff.md) 13).
  One process speaks one patch, as each retail binary does.
- Entity culling follows each engine. On 1.5 a sounded entity rides past
  the PVS for 300 ms, a looping one while it loops, a sounded
  `script_origin` never, and cull fog drops what lies past its far
  distance (same doc, 15). The compass follows each `SV_inSnapshot`: a
  teammate off the map shows on 1.5 and not 1.1. An eye in a solid leaf
  is sent no entities.
- `getinfo` and `getstatus` answer byte for byte as retail, odd challenges
  included.
- Publishes retail's pak lists and checksums in the systeminfo and serves
  the listed paks at retail's pace. On 1.5 `sv_wwwDownload` redirects to
  `sv_wwwBaseURL`, connected or disconnected, and streams the pak after a
  failed www transfer. `--set sv_pure=1` checks each client's pak
  checksums as a pure retail server does.
- `sv_minPing` / `sv_maxPing` refuse off-LAN clients by challenge ping. An
  off-LAN client's messages wait out its rate and `snaps`. Big messages go
  one fragment a frame; a client not in the world gets retail's bare
  keepalive.
- rcon with retail's commands and replies, bans, the master heartbeat,
  zombie slots and retail-measured pings.
- `g_log` writes retail's `games_mp.log` (`InitGame`, `RestartGame`,
  `ShutdownGame`, chat, tell and pickup lines in each patch's form, the
  scripts' `logPrint`) under the home path
  ([cod11-map-cycle.md](../research/cod11-map-cycle.md) 9).
- On 1.5: the match timeout, 1.5's chat (`g_deadChat`, `say_squad`), and
  PunkBuster refused as the Linux 1.5b server refuses it.

Game:

- Runs Activision's own gametype and map scripts on `vcod-gsc`. Menus,
  spawns, scoring, rounds and limits come from the stock scripts. A
  runtime error ends its thread only under `developer 1`
  ([cod11-gsc-language.md](../research/cod11-gsc-language.md)).
- All five stock gametypes are checked against retail: `dm`, `tdm` and
  `sd` end to end, `re` and `bel` through their key events
  ([cod11-gametypes-re-bel.md](../research/cod11-gametypes-re-bel.md)).
- Shared pmove with capsule players that block and push each other.
  Falls stun and hurt. On 1.5 everyone moves by 1.5's rules, bots and
  their nav graph too.
- Bullets trace the world, players, static props and script models,
  through the stock damage callback with per-bone hit locations. Each body
  is posed as retail builds it for that trace. Rifle rounds go through
  players; every round goes through glass.
- Patches, terrain and brushes clip rounds and players in each server
  build's own arithmetic, and a patch only from the BSP leaves that list
  it, in retail's leaf order. So rounds, items and players slip through
  the seams retail's do, and terrain past a map's playable walls lets
  them through as retail's does.
- Shots, swings and throws start where each patch starts them: 1.1's
  truncated eye, 1.5's bobbed and leaned one. On 1.5 each shot kicks the
  server's gun spring, so a sighted burst climbs.
- Grenades are missiles that bounce (off script models, and off a
  player's posed bones at an eighth of their speed), rest (on a player
  too; on 1.5 they fall once he leaves) and explode with retail's falloff.
  The arc and bounce keep 1.1d's x87 arithmetic and 1.5b's float stores.
  A blast walks its victims in retail's area-tree order.
- The panzerfaust fires a rocket that flies, strikes and explodes as
  retail's. A missile at the sky is freed; one dropping fast into water
  splashes. Melee works.
- Corpses in the eight-slot body queue, dropped weapons, pickups by touch
  or use key, items on retail's arc and respawn timer.
- Mounted MG42s: mount, aim inside the arc (1.1's 15-degree step and view
  snap, 1.5's plain clamp), fire, dismount. A round passes the gun, not
  the gunner. 1.1d and 1.5b captures replay against ours.
- Map triggers (`trigger_damage` fires off rounds, swings, grenades and
  blasts), script movers that carry and shove players and items, and
  `linkTo` on script models, brush models, items, turrets, player tags and
  `attach`ed models' tags.
- Intermission, `map_restart` and `sv_mapRotation` as retail runs them, a
  rotation a script `setCvar`s included. Spectator follow and the killcam,
  replayed from a ring of archived frames.
- Votes with retail's kinds, refusals, configstrings, pass rule, 30 s
  timeout and 3 s delay: 1.1's seven kinds under `g_allowVote`, 1.5's
  twelve under their `g_allowVote*` gates. A script's `allowComplaint`
  opens the teamkill complaint, which `vote` answers.
- On 1.5 a prone body's pose, the bend a drop leaves it, a spawn's held
  prone, a spectator's spawn on his held keys and a frag landing on a
  prone back match retail captures. On both patches a lie-down idles
  crouched through its first leg
  ([cod15-movement.md](../research/cod15-movement.md) 4,
  [cod11-combat.md](../research/cod11-combat.md) 12.2).

### Bots

`--bots N [--bots-shoot]` and rcon's `addbot`, `removebot`, `listbots`
([bots.md](../bots.md)).

- They join through the stock menus and roam a nav graph built from
  pmove runs, ladders and jumps, proven under the patch's movement.
- Objectives: S&D plants and defuses, Retrieval carries and escorts,
  Behind Enemy Lines hunts. Only the two teammates nearest a shot go to
  it.
- With `--bots-shoot` they fight: a reaction delay, a turn cap, aim that
  settles and trails a crossing target, strafing on their sights. They
  spot enemies in a sight cone, keep their target and turn on whoever
  shoots them.
- They fight from low walls and corners, vary their peeks, hunt a lost
  enemy, back off hurt or dry to reload, fetch a nearby health pack, and
  throw frags at enemies behind cover (never at a teammate with friendly
  fire on). They run from live frags.
- Names drawn without repeats, team balance that moves only dead bots, a
  `vc_bot_minplayers` floor, four skill levels (`vc_bot_skill` or `addbot
  ... skill N`). The `vc_bot_*` cvars apply live.

They are still bad at it. So am I.

### Tools

- `vcod --net-probe <ip:port>`: headless client that joins, logs snapshots
  and writes captures, with scripted modes for shots, grenades, plants and
  more (`vcod --help`).
- `tools/run_server.sh` and `tools/run_probe.sh` run the retail 1.1d and
  1.5b Linux servers and `.gsc` probes against them; A/B tests replay
  retail captures on vcod-server.
- `tools/cod1_soak.sh` plays a client through every gametype per map
  against six shooting bots.

## Missing compared to retail

Front end:

- No listen server. Start New Server always runs a separate
  `vcod-server`; Dedicated No joins it as an ordinary client (it needs
  the password too), and LAN or Internet doesn't turn the window into a
  server console.
- A mod's UI DLL is ignored; only its menu files count
  ([cod11-front-end.md](../research/cod11-front-end.md) 16-17).
- The CD key menu doesn't open (the console says so). Set the key through
  `VCOD_CDKEY` or `main/codkey`.
- Options: the sound provider and NVIDIA fog are stored but ignored, and
  the language picker stays hidden.

Client:

- No PunkBuster; PunkBuster servers refuse vcod.
- No screenshots, so `cg_autoscreenshot` and `cl_avidemo` do nothing.
- No CoD1 demo has crossed between vcod and a retail client yet, either
  way ([cod11-demos.md](../research/cod11-demos.md) 7).
- `deformVertexes text` and `syncNormal` (no stock user) parse and do
  nothing. `portal` draws no portal view. NV/ATI hardware-path stages are
  dropped and three-unit stages give way to the scripts' two-unit ones,
  as on retail cards without them. mp_ship's deckflag has no file behind
  it, so a stand-in draws.
- Audio has had little checking by ear. If something sounds wrong, it
  probably is: [report it](../../README.md#reporting-bugs).

Server:

- `sv_pure` defaults to 0 (retail: 1).
- No stdin console. Admin goes through rcon, refused until
  `--set rconPassword=...`. After `killserver` the process sits there
  deaf.
- rcon runs `map`, `devmap`, `map_restart`, `map_rotate`, `status`,
  `clientkick`, `kick`, `banUser`, `banClient`, `dumpuser`, `serverinfo`,
  `systeminfo`, `say`, `set`, `seta`, `cvarlist`, cvar queries,
  `heartbeat`, `killserver` and `quit`; on 1.5 also `tempBanUser`,
  `tempBanClient`, `unbanUser`, `setdrawfriend`, `setfriendlyfire` and
  `setkillcam`. `scriptUsage` and `stringUsage` print retail's report with
  vcod's VM numbers.
- Bans: no authorize server. Bans live in 1.5's `ban.txt` (`--ban-file`
  keeps it), keyed on the guid an off-LAN address stands in for; retail
  1.1 hands them to Activision's authorize server. On 1.1 a ban refuses
  an off-LAN address and leaves the banned player connected until he
  leaves; on 1.5 it kicks an off-LAN player and does nothing to a LAN one.
  A temporary ban refuses for `sv_kickBanTime` seconds.
- `gameCompleteStatus` reaches only the masters the server resolved
  itself, as retail's does.
- The heartbeat to `master.cod.pm` has not been tried against the live
  master; tests heartbeat a loopback fake. If you try it, tell me how it
  went.
- No anti-cheat.

## Beyond retail

- `vc_widescreen 1` (archived, live, default on): Hor+ fov past 4:3, and
  the HUD's side pieces (compass, stance, chat, kill feed, votes left;
  health, ammo, weapon strip right) at the window's edges. Crosshair,
  hints and centre prints stay in the middle; menus stay in the centred
  4:3 box. `vc_widescreen 0` is retail's fixed horizontal fov with the
  whole HUD in the 4:3 box
  ([cod11-hud-protocol.md](../research/cod11-hud-protocol.md) 10).
- `r_fullscreen 1` is borderless at the desktop's resolution,
  `r_fullscreen 2` exclusive at the mode closest to `r_mode` (borderless
  on Wayland). `r_mode -1` with `r_customwidth` / `r_customheight` asks
  for any size. Alt+Enter flips `r_fullscreen`
  ([cod11-front-end.md](../research/cod11-front-end.md) 21).
- Graphics extras, all off by default. `--rtx-on` turns on all ten for
  one run.

  | Cvar | What it does |
  |---|---|
  | `vc_bicubic 1` | Lightmaps filtered with a cubic B-spline instead of bilinear, so low-res shadow edges stop stepping. |
  | `vc_bloom <strength>` | Glow around bright pixels; `vc_bloomThreshold` (0.8) sets where it starts. HUD excluded. |
  | `vc_ssao <strength>` | Screen-space ambient occlusion on lightmapped and ambient-lit surfaces. |
  | `vc_shadows 1` | Players, corpses, entity models, moved brush models and flags cast soft sun shadows on the world and each other. Foliage and fences cast through their alpha. Cast shadows match the map's baked darkness and don't darken already-shadowed points ([light grid doc](../research/cod11-light-grid-and-leaf-lights.md) 12). |
  | `vc_sunShafts <strength>` | Light shafts from the sun, on maps whose `.sun` gives it a flare. Feeds bloom. |
  | `vc_softParticles <scale>` | Smoke, dust, fire and explosion sprites fade where they meet walls and ground ([cod11-gamma.md](../research/cod11-gamma.md) 8). |
  | `vc_hdr 1` | Float world target with a tonemap, so stacked effects and the sun roll off instead of clipping. Takes effect on `vid_restart`. |
  | `vc_grade`, `vc_vignette`, `vc_grain` | Period-film colour grade, vignette and film grain, value is strength ([cod11-gamma.md](../research/cod11-gamma.md) 9). |

  `r_vc_depthprepass` (default 0 on CoD1) lays depth down first; same
  pixels, faster on some GPUs, slower on lavapipe.
- Asphalt footsteps play. Retail's engine asks for `asphalt` and the sound
  table spells it `asphault`, so retail's are silent. vcod uses the
  table's spelling, typo and all.
- Wall occlusion on sounds.
- Visibility draws a little more than retail on purpose. Retail assumes
  nobody looks over a cell's walls, and the mp_ship decks disagree.
- Dynamic lights add on top of a prop's vertex lighting with retail's
  falloff instead of competing for its eight light slots.
- A brush model that turned and turned back keeps a sliver of yaw on
  retail and drifts its riders about 0.02 units a frame. vcod's comes back
  to zero. I'm calling that a bug in retail.
- vcod-server serves only the paks it lists. Retail serves any file under
  its directories, `..` included. I'm not copying that one.
- Bots (above). Retail has none.
- The F3 overlay: frame, draw, vis, shader, fog, audio, camera, clock,
  prediction, net and snapshot counters ([usage.md](../usage.md)).
- `--vanilla` on both binaries keeps only the patch's stock paks on the
  search path; the client downloads nothing under it.

### Things that look like gaps and aren't

- No mantling, no swimming. Retail 1.1 MP has neither
  ([cod11-mantle.md](../research/cod11-mantle.md)).
- No grenade cooking on 1.1 or 1.5. The fuse runs in full from the
  release. United Offensive has one, and vcod does it there. If you
  remember cooking frags in 1.1, you remember UO.
- No doppler. The engine never gives a sound a velocity.
- Quick chat (`vsay`) does nothing on a stock install: its `.voice`
  tables only ship in mods.
- Show Compass and Team Overlay do nothing, as in retail. Faithfully
  reproduced.

## Debug and developer modes

- `vcod <map>` flies the map offline; `vcod <map> --walk` spawns an
  offline soldier with a seven-weapon loadout, an early proof of concept
  ([usage.md](../usage.md#fly-mode)).
