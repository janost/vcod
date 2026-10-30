# What vcod does, and what it doesn't

The long version of the README's summary. Where a research doc measured
something, the bullet links it.

## What works

### Map viewer

- Draws any stock or custom map with textures, lightmaps and props. Skies,
  water, fences, foliage and terrain blends run through the maps' own
  Q3-style shader scripts, sun disc and fog included. Lamp glows face the
  camera (`autosprite`), flags flap, and a `.sun` file's flare, glare and
  blind follow the sun's visibility; the sun shows only on frames the sky
  draws, and `r_drawSun 0` hides it. The map's `corona` entities glow
  around their lamps when seen from below, as retail draws them.
- Culls with retail's cells, portals and occluders. F4 freezes or disables
  culling so you can fly out and see what the camera was drawing.
- Props, players and every other model switch to their coarser LODs with
  distance, zoom and Model Detail, by retail's rule.
- Plays the map's ambient loop.
- On a CoD4 1.7 install, bare `vcod` opens 1.7's main menu, drawn and run
  from the `ui_mp` zone's menus: Start New Server starts `vcod-server
  --game cod4` and joins it (Dedicated No), its Server Name and Password
  fields typed, Mods runs a `mods/` mod, Options and Controls set what
  vcod has a cvar or bind for, Select Profile lists, creates, deletes
  and loads the profiles in `players/profiles` (a fresh install asks for
  one first, and the active profile holds the config), Quit asks first;
  Join Game opens the server browser and joins the server picked, with
  the password typed. Esc on the connect screen (or `disconnect`) calls
  the join off, where 1.7's Esc does nothing there. The menus play
  `mouse_over`, `mouse_click` and `music_mainmenu_mp`. Leaving a game or
  a server's drop comes back to it, the reason in `error_popmenu`
  ([cod4-front-end.md](research/cod4-front-end.md)).
- CoD4's Aspect Ratio (`r_aspectRatio`, taken at `vid_restart`) does what
  1.7 does: the view projects at the picked 4:3, 16:10 or 16:9 (Automatic
  rounds the window to one of the three) over the whole window, `cg_fov`
  a 4:3 view's width, so a 21:9 or 2.4:1 display stretches it, and
  fullscreen stretches the HUD with it. In a Start New Server game the
  in-game Graphics page greys, as on 1.7's listen server. `vc_windowAspect
  1` (vcod's, default 0) projects at the window's own aspect, Hor+ off
  the same 4:3 view, and the HUD unstretched: its left and right pieces
  (compass, score, weapon info) at the window's edges, its centred ones
  (scoreboard) on the window's middle, the XP bar's strip across the
  whole foot
  ([cod4-front-end.md](research/cod4-front-end.md) 10).
- On a CoD4 1.7 install, `vcod <map> --game cod4` flies a stock map read
  out of its zone: textures, lightmaps and the sun, static models, the
  sky cube, blends and alpha tests, the doorway lighteners and the light
  shafts' angle falloff, culled by the map's cells and portals. A census
  test holds every world and static model material of the 21 maps to its
  retail state bits. Fly mode also runs the map's createfx effects
  (settled as on a level a minute old) and plays its script's
  `ambientPlay` loop with the zones' sounds. The map's exp fog
  (`setExpFog`, configstring 9 in a session) and its vision set's film
  tweak and glow (`vision/<map>.vision`; in a session configstring 824,
  the prematch's grey `mpintro` easing to the map's over 2 s, and 825's
  night set under night vision) draw in fly mode and a session; `r_glow 0`
  (a cheat dvar, as on 1.7) turns the glow off at once. The
  world's lit surfaces take their normal, specular and detail maps under
  the sun; static models are lit off the map's light grid, as the
  `lp_*` model techniques read it, and the specular sets add their
  reflection probe. A world surface or static model on a spot or omni
  primary light takes it in the sun's place. With `sm_enable 1` (the
  default) the sun casts real-time shadows: world surfaces, static models
  and entities are drawn each frame into retail's two-partition sun shadow
  map, which the lit world, static models and entities read
  ([cod4-world.md](research/cod4-world.md) 3.6). Water draws as 1.7's
  `water_l_sun` over its moving height map. The effects' `zfeather`
  sprites fade where they meet the world (`r_zFeather`, Soften Smoke
  Edges), and up the sight a depth of field blurs what is near and the
  gun's own range as 1.7's `postfx_dof` does (`r_dof_enable`;
  [cod4-effects.md](research/cod4-effects.md) 14,
  [cod4-client-view.md](research/cod4-client-view.md) 12). The heat
  haze of explosions, fires and muzzle flashes bends a copy of the frame
  as 1.7's `distortion_scale_zfeather` does (`r_distortion`;
  [cod4-effects.md](research/cod4-effects.md) 17). Up to four spot
  lights in view (`sm_maxLights`) cast real-time shadows of the world,
  static models and players onto what they light, fading in and out as
  they come and go (`r_spotLightShadows`, `sm_spotEnable`;
  [cod4-world.md](research/cod4-world.md) 3.5). The world's distortion
  set (mp_overgrown's glass bricks) bends the same copy, and the world
  and static model `zfeather` sets (floodlight beams, lens flares) fade
  where they meet the world and near the eye
  ([cod4-world.md](research/cod4-world.md) 3, 9-11,
  [cod4-hud.md](research/cod4-hud.md) 13).
- CoD4 mods and usermaps: a map loads under `--fs-game mods/<name>`, or a
  joined server's `fs_game`, with the mod's `mod.ff` and iwds over the
  stock zones and `main/` as 1.7 orders them; a map missing from
  `zone/<language>/` loads from `usermaps/<map>/`, and under a mod that
  directory's iwds join the search path. The HUD and the script menus
  read the mod's `mod.ff` too (ModWarfare's class menus), the map's
  `<map>_load.ff` gives the connect screen its levelshot, and a server
  whose `fs_game` changes on a map change reopens the image search path.
  Under a mod the client predicts with the weapon files off the iwds, as
  the server reads them. A joined server's mod or usermap iwds and
  fastfiles the install lacks download into it before the map loads
  (`cl_allowDownload`, on by default as 1.7's), over UDP or a www
  redirect's plain `http://` under `cl_wwwDownload` (connected or
  disconnected; a failed fetch falls back to UDP), each kept unchecked
  as 1.7 keeps it and compared again on the next gamestate, where one
  still different ends the join; the connect screen draws the
  download's progress
  ([cod4-fastfile.md](research/cod4-fastfile.md) 13,
  [cod4-handshake.md](research/cod4-handshake.md) 11).
- CoD4's server browser, `ui_mp`'s `pc_join_unranked` run on the script
  menus: Local (a `getinfo` broadcast to 28960-28963), Internet
  (`getservers 6 full empty` to `masterServerName`, cod.pm's master by
  default where 1.7 asks Activision's) and Favorites (`servercache.dat`,
  of which vcod writes only the favourites), the pings, the filter popup,
  the column sorts, the Server Info popup's status, New and Del.
  Favorite, the password popup and Join Server, which joins with the
  `password` in the userinfo, behind the CoD4 front end's Join Game. The
  master is asked only by Refresh List on the Internet source
  ([cod4-handshake.md](research/cod4-handshake.md) 12).
- `vcod --game cod4 --capture <file>` replays a CoD4 datagram capture and
  `--connect` joins a 1.7 server (retail or vcod-server; pure `cp`
  included) as a spectator or a player: the server's script menus draw
  from the zones' menus as 1.7 draws them (the team menu, `changeclass`
  and `changeclass_offline`: the items' rects, materials, text and
  visibility expressions over the dvars, the profile's stats and the
  string tables), with a cursor; the mouse or the arrows and Tab move the
  focus, a click or Enter picks, Esc runs the menu's `onEsc`. With none
  up Esc opens the server's `g_scriptMainMenu`: the team menu before a
  team pick, after it the in-game `class` menu (the game type and its
  objective, Choose Class, Change Team, Controls, Options, Call Vote,
  Mute Players and Leave Game, whose Yes disconnects). Controls' pages
  list vcod's binds per command and rebind a row on a click and the next
  key (Backspace unbinds, Esc cancels), saved to `vcod_mp.cfg`; Options'
  rows set the cvars vcod has for them (sensitivity, invert mouse,
  free look, smooth mouse, brightness `r_gamma`, shadows `sm_enable`,
  bullet impacts as `cg_marks`, master volume, allow downloading, player
  name; video mode, refresh rate, anti-aliasing, sync and aspect ratio
  through the video restart popup; glow, specular, dynamic lights, model
  detail, water animation, texture filtering, anisotropy, show blood,
  enable console and depth of field at once, soften smoke edges as the
  page closes; the texture page's quality and resolutions through its
  `r_applyPicmip`, on the world's colour, normal and specular maps; ragdoll) and
  draw the rest greyed (dual video cards, the sound
  page's EQ, quality and speakers, voice chat, PunkBuster), multiplayer's rows shown as
  `ui_multiplayer` 1 picks them, the rows a listen server's `sv_running`
  greys greyed while a Start New Server game (Dedicated No) runs, and the
  language row under `developer`; Call Vote sends
  retail's `callvote` (restart, next map, a map from the arena list, a
  game type and map, a player's temp ban), which vcod-server runs as
  1.7 does; Mute Players toggles a player from the snapshot's list,
  tells the server and holds back his chat lines and his voice. Voice
  chat: `+talk` (Z) sends the microphone through the system's libspeex
  (narrowband, 8000 Hz, `sv_voiceQuality`; nothing sent or played
  without the library), the heard talkers play, the HUD's talker list
  and the scoreboard's column show who talks, Eavesdrop's enemies with
  its icon ([cod4-voice.md](research/cod4-voice.md)). The menus read the
  `set`s of the install's startup configs (`default_mp.cfg` and what it
  execs) under the server's dvars, as 1.7's one dvar table holds them:
  the team menu's Spectator row reads `scr_game_spectatetype` 1 there.
  Every expression of the 288 menus the HUD's zones and mp_crash carry
  evaluates; a menu with one vcod does not would show as a numbered list
  of what it answers (1-9 pick). The menus' `play` sounds play (the
  stock menus fade nothing). `--team` and `--class` (e.g. `assault_mp,0`) answer
  the team and class menus on their own
  ([cod4-hud.md](research/cod4-hud.md) 12). A player sends a cmd
  a frame off the binds: move, sprint, jump and the stances, attack, the
  sight (`+speed` or `toggleads`), melee (V, 1.7's default bind), use,
  reload, lean, hold breath, frag and smoke, and `weapnext` / `weapprev` over the held primaries,
  the weapon byte the client's own pick. The session draws the snapshot view on retail's client clock and lerp,
  players and corpses posed from their
  `legsAnim`/`torsoAnim` with their heads and held weapons, their aim's
  pitch, lean and prone pitches bending the spine and head as 1.7's
  controllers do, corpses going ragdoll onto the floor once the server's
  `startRagdoll` says so (`ragdoll.cfg`'s bodies under a position-based
  solver, not ODE; grenade, rocket and script blasts push them, as on 1.7
  bullets do not;
  [cod4-player-anims.md](research/cod4-player-anims.md) 10), dropped
  weapons, grenades and movers on their trajectories, the airstrike's
  jets and the helicopter (its rotors turning, its rotor wash and its
  damage stages' smoke and fire, and the script's `playFxOnTag` effects
  on any model's tags, [cod4-client-view.md](research/cod4-client-view.md)
  11), script models without the parts `hidePart` hid (a destructible
  car draws its intact or its cracked glass, never both,
  [cod4-client-view.md](research/cod4-client-view.md) 15), mounted guns
  with their barrel turned to the wire's gun angles,
  another gunner's gun playing its fire anim while it fires, and
  a gunner's eye at the gun's `tag_player`, another gunner's body placed
  on his gun without his own weapon in hand, the obituaries
  on the console (event names go to the debug log); the events' sounds out of the zones
  and iwds (footsteps, landings, shots, reloads, hits, bounces and
  explodes, a knife's cut, the map's ambient, entity loop sounds (a brush
  model's at its bounds' middle) and script `playSound` and
  `playSoundAsMaster`; `EV_STOPSOUNDS` stops an entity's voices; a shot
  sounds at the shooter's `tag_flash`; a voice started off its entity
  turns round it with the entity; each alias's falloff curve and,
  on 2D rows, its speaker map's stereo level; stereo output only) and their effects (muzzle flashes and shell ejects riding
  the posed weapon's `tag_flash` and `tag_brass`, impacts, explosions,
  the effects an element emits along its flight, a missile's smoke
  trail on its `tag_fx`)
  on the CoD2 effect path, every bullet traced client-side (our own
  from the eye, the others' from their muzzle) to its world impact,
  impact sound, bullet hole and `whizby`, with 1.7's tracers, looping effects for their whole
  life (`spawnFx` and `playLoopedFx` entities, and the map's createfx
  ambience: mp_crash's fires, smoke and dust start settled at load;
  each element spawns only inside its `spawnRange` of the camera, so an
  effect's near and far versions do not both draw, and inside the view
  where its flag asks, and fades over its distance ranges),
  the earthquake shaking the view, and the playerstate's shellshock: a
  flashbang's or concussion's white-out over a still of its first
  frame, the camera kick, the concussion's mouse caps, and the
  flashbang's tinnitus and sound mix, out of the zones' `.shock` records
  ([cod4-client-view.md](research/cod4-client-view.md) 8) ([cod4-sound.md](research/cod4-sound.md),
  [cod4-effects.md](research/cod4-effects.md))
  ([cod4-netfields.md](research/cod4-netfields.md) 12,
  [cod4-player-anims.md](research/cod4-player-anims.md),
  [cod4-client-view.md](research/cod4-client-view.md)). The move keys
  fly a free spectator (live, through its cmds) and the fly keys the
  camera (a capture); a followed view stays the snapshot's. A first-person view
  (our own body or a followed player) draws the held weapon's view
  model with the playerstate's hands at 1.7's `cg_fov` 65 and the
  sight's zoom, its clips off `weapAnim` (idle, fire, reload, raise and
  drop, sprint, the melee with its knife) under the sight layer, the
  attachments the weapon lacks hidden (`hideTags`), a reflex sight's dot
  projected off the view's angle to the lens, the gun eased by the move
  offsets, idle drift, bob and sway, and the own shots' flash and shell
  eject on its `tag_flash` and `tag_brass`, and the gun's recoil spring,
  damage kick and landing dip; its camo is not modelled. A shot's
  recoil kicks the view through the cmd's angles, a hit kicks the drawn
  view along its direction and a landing dips the eye
  ([cod4-client-view.md](research/cod4-client-view.md) 14). The HUD draws the
  playerstate's hudelems in the zones' fonts with their glow, the kill feed with weapon
  icons, `iPrintLn` and `iPrintLnBold` lines, chat, the obituary's centre
  print, `[{+cmd}]` key names in hudelem text, the rotating minimap
  compass with teammates (kept 1.5 s after they drop out, a
  `pingPlayer` flash, a talker's ping), radar enemies and objectives and
  its tickertape, the weapon HUD
  (stock, the clip's rounds as its bullets, weapon name, offhand icons
  and counts), the killcam's letterbox and title, the prematch's
  "Waiting for teams" / "Match begins in" text and countdown (hidden
  under a menu, the compass and ammo counter held back while 1.7's
  `ui_hud_hardcore` reads 1), the action slots (night vision, the
  grenade launcher, an earned hardpoint's icon and key), the UAV's sweep
  line and enemy pings on the compass, shot flashes held for the fire
  sound's length, the helicopter and
  airstrike planes there, the low clip's red pulse and a low reserve in
  red, the low-health overlay's pulse and the hit direction icons (none
  under a shellshock's blur or flash), the
  cursor hint (pick up, swap, throw back, a turret's or a trigger's
  `setHintString` text with the key), the grenade indicators around
  the crosshair, the XP bar and score bars, the mantle, hold breath and
  invalid command ("No Frag Grenades Remaining") hints, and the
  scoreboard on Tab and at intermission with the `scoreboard` menu's
  header (team scores, the game type, the time left), the talking icons,
  a long list scrolled by the wheel and the page keys with its scroll
  bar, the ping graph under `cg_scoreboardPingGraph 1`, asking a live
  server for scores (empty until one answers), the world waypoints (Bomb
  Squad's icon over an enemy claymore or C4) and the offscreen ones on
  the screen's edge with their pointer, the crosshair (the weapon's
  spread, Steady Aim's narrower, a scope's overlay, a mounted gun's own
  reticle, `cg_crosshairDynamic` following the gun as a sight comes
  up), and the full map
  while an airstrike's location is picked (the mouse moves the
  selector, fire confirms, Esc cancels) and in the team and class
  menus. A live join
  (the command line's or the main menu's) draws 1.7's connect screen (the
  levelshot, the glowing game type and map name, stage line, load bar,
  a download's progress, rate and time left, and a server's `print` or
  the CD key wait in yellow) while it connects and loads
  ([cod4-hud.md](research/cod4-hud.md)). The drop-down console draws in
  the zones' `consoleFont` and sends `say`, `cmd` and unknown commands to
  a live server; `disconnect` (Leave Game) and a server's drop return to
  the main menu, the reason in `error_popmenu`. Spot lights (drawn
  omni; no stock MP effect has one) are not modelled; a tracer draws
  as 1.7's corkscrew beam. Client-side bullets
  take the server's seeded spread (another player's off his legs
  anim's stance and sight, a mounted gun's off its `playerSpread`, the
  gun we man drawing no tracer, a shotgun's pellets one tracer roll
  a shot), stop on the players' boxes, where the
  server's flesh hit plays, and pierce the world with exit impacts
  (Deep Impact deepens it), and Dead Silence plays the quiet footsteps. Our own body, and a free spectator, is predicted: the
  client's cmds replayed through `vcod_cod4::pmove` over the map's clip
  map and the other players' boxes from the newest snapshot, the
  correction eased over `cg_errordecay`'s 100 ms and a stair step over
  `cg_viewZSmoothingTime`, so the eye, bob and view model move every
  frame instead of in the snapshots' 100 ms beats; the mantle and the
  weapon half predict with the rest, so our own shots, reloads, switches
  and footsteps play their events (flash, sound, view clip, client-side
  impact) the frame the predicted move raises them and not again when
  the snapshot carries them; item pickups stay the snapshot's
  ([cod4-client-view.md](research/cod4-client-view.md) 5). A capture's
  own view eases between its cmds, a replay-only smoothing (6). A lean
  moves the eye sideways as 1.7's does (7). `+actionslot` (5 by
  default) switches a rifle to its grenade launcher and back, or to the
  slot's weapon, and 1 (N) asks for the night vision goggles (9). The
  stock CoD4 keys bind: `+breath_sprint` (SHIFT), `+toggleads_throw`
  (MOUSE2), and `chatmodepublic` / `chatmodeteam` (T, Y), which open
  the say field ([cod4-hud.md](research/cod4-hud.md) 12.6).

### Walk mode (`--walk`), offline

- Spawns a soldier on a spawn point with retail movement: gravity, crouch,
  prone, stepping, leaning, wall sliding.
- Seven weapons with their own viewmodels, animations, sounds and reserve
  ammo. Hitscan shots with per-surface impacts and tracers.
- Footsteps on retail's cadence per surface, a body standing across two
  floor brushes stepping on the one retail's leaf walk reaches first; the
  landing sound scales with
  fall speed.

### Client

- Opens on the stock main menu and server browser, drawn from the game's own
  `ui_mp/*.menu` files. The browser's three sources are retail's: Local
  (a `getinfo` broadcast to ports 28960-28963), Internet (what the
  `master.cod.pm` master lists for the install's own protocol: 1 on
  1.1, 6 on 1.5 on port 20510, 22 on UO on 20610, 118 on CoD2 on 20710,
  at most one request a second; a server whose `getinfo` names another
  protocol is dropped) and Favorites (kept in `servercache.dat`
  beside `CoDMP.exe`, the file retail uses). It pings, filters (by game
  type too), shows when each source was last refreshed, sorts by
  column, joins on double-click, and runs the stock password, server info,
  filter and favourite popups. Losing the server drops you back on the menu
  with the localized reason in the stock error popup.
  In a game, Esc opens the script menu and its Main Menu tab the stock
  in-game main menu (Back to Game, Disconnect). The stock Mods menu lists
  the mod directories beside `main/` and switches to one, in a game too
  (not while a server you started with Dedicated No runs, as retail refuses),
  and the menus load from the mod's own `ui_mp/menus.txt` and
  `ui_mp/ingame.txt`, each file tried under `cl_language`'s directory first
  (latched, as retail: a change waits for `vid_restart`).
- `record [name]` writes retail's demo format (`demos/<name>.dm_1` on 1.1,
  `.dm_2` on 1.5, `.dm_3` on UO; no name counts up from `demo0000`) and
  `stoprecord` closes it, with retail's `RECORDING <name>: <n>k` line on
  screen meanwhile; `demo <name>` plays any of the three back through the
  online frame, the recorder drawn as a followed player with no
  prediction, as the cgame draws a demo. `timedemo 1` steps 50 ms a frame
  (paced by `com_maxfps`, as retail's is) and prints the frame rate at
  the end, `cl_freezeDemo 1` holds the
  clock, `nextdemo` runs once the demo ends, and any key opens the main
  menu. A demo fetches nothing: its map missing is retail's `Couldn't
  load maps/mp/<map>.bsp`. `cg_autodemo 1` records on a 1.5 or UO
  server's `autoDemoStart`, as the 1.5 cgame does
  ([cod11-demos.md](research/cod11-demos.md)). On a CoD2 install
  `record` / `stoprecord` write CoD2's `.dm_1` in a game (its own
  gamestate, then each message once a full frame came) and `demo <name>`
  or the `timedemo <name>` command plays one through the CoD2 session's
  frame, unpredicted; retail's demos play in vcod and vcod's in retail
  ([cod2-demos.md](research/cod2-demos.md)). On a CoD4 install `record`
  writes 1.7's typed records (an opening gamestate with every slot the
  client holds, the 256 client archive rows, then each message and the
  archive rows added since, once a full frame came) into
  `main/demos/` (the mod's `demos/` under `fs_game`), and `demo <name>`
  or `timedemo <name>` loads the demo's map behind the connect screen
  and plays it through the CoD4 session's frame, unpredicted, the
  recorder's own view off its archive rows; Esc leaves for the main menu
  ([cod4-demos.md](research/cod4-demos.md)).
- Start New Server runs the stock create server menus (game type, the maps
  `mp/*.arena` lists for it, server and game type settings), then starts
  `vcod-server` from the same directory as `vcod` with those settings and
  the Mods menu's mod. The server logs to `vcod-server.log` beside
  `vcod_mp.cfg`. Dedicated No joins it once it answers and stops it when
  the client disconnects; LAN and Internet leave the client in the menus.
  The client kills any server it started when it exits.
- The stock Options and Multiplayer Options screens: rebind keys, mouse
  sensitivity, invert, free look and mouse smoothing (`cl_freelook`,
  `m_filter`, with `+mlook` and `+strafe`), player name, rate, master
  volume, video mode and
  full screen, brightness, texture detail, filter and quality, dynamic
  lights, sync, blood and sound quality, applied at the restarts retail
  applies them at. Choices land in the console's binds and cvars and persist in
  `vcod_mp.cfg`; settings vcod has no use for are kept but do nothing.
- CD key authorization as each retail client does it (`getKeyAuthorize` to the game's authorize server before the challenge, off the LAN only), with the key from `VCOD_CDKEY` or 1.1's `main/codkey`, and the stock `needcdkey` and key refusal messages ([usage](usage.md#cd-keys)).
- Joins a 1.1 server (`--connect` or the browser): handshake, Huffman,
  netchan, delta snapshots, and pak downloads for whatever the server has and
  you don't, matched by checksum as retail does; a 1.5 or UO server's
  `sv_wwwBaseURL` redirect is fetched over plain `http://`, as retail's
  libwww fetches it ([cod15-protocol-diff.md](research/cod15-protocol-diff.md)
  14; `cl_wwwDownload 0` turns it down). A downloaded pak whose checksum is
  not the one the server lists is deleted before anything loads it, and a
  redirected one is asked for again, a later redirect of it refused so a
  1.5 server sends it over UDP. Pure servers take its pak
  checksums. Modded servers work too: every systeminfo key
  (`cl_allowDownload`, `rate`, `snaps`, `fs_game`, ...) is forced into the
  client's cvars as retail does, the forced `fs_game` layers its mod
  directory over `main/` and swaps in the mod's menus (and stays after you
  leave, as retail's does), and a mod's script menus and `hud.menu` text
  show up.
- `--vanilla` (client and server) puts only the install patch's stock paks
  on the search path and ignores third-party maps, `zzz_*` mods, downloads
  and `fs_game`; the client downloads nothing under it
  ([usage.md](usage.md)).
- Watches a United Offensive 1.51 server (protocol 22) with `--mod-dir uo`:
  it connects speaking UO's protocol and refuses a 1.1 or 1.5 server
  (without `--mod-dir uo` it refuses a UO one), loads the
  UO map and models from `uo/` over the 1.5 `main/`, and draws the players,
  items, script models, the stock UO script menus, the kill feed and the
  events UO shares with 1.1. Tanks and jeeps draw posed as UO's cgame poses
  them (hull tilt, turret and barrel, the gunner's gun, on your own view
  and inside its spans when you man it, the jeep's steering
  wheel, turned four times as far under your own hands, and front wheels),
  their treads and tyres scrolling with the vehicle's travel and their
  wheels kicking up the dust of the ground under them, never over another
  vehicle (`cg_vehicletrails`), and a seated player rides its seat in UO's seat
  blend, the jeep driver's steer included, with the head turned to the
  view as UO's cgame turns it; on 1.5 and UO every body's spine and lean
  ease toward the view as those patches' do
  ([coduo-diff.md](research/coduo-diff.md) section 18,
  [coduo-vehicles.md](research/coduo-vehicles.md) 9). UO's own
  events play their effects and sounds (bullet impacts by ammo type,
  artillery, smoke and tank explodes, deploy, fatigue, overheating, the
  gunner's fire, the MG fire loop), vehicle engines and MG loops crossfade
  as UO's sound blend entities do, tank shells whizz in flight, a
  vehicle MG's rounds draw the impacts and tracers UO's cgame traces
  itself and stop on vehicles, tracers follow UO's length gate and
  `sv_night` width, the flamethrower streams flame that bounces off
  bodies, leaves burning residue and reports its burns on you to the
  server (`fdc`), efx
  camera shakes shake the view, and the HUD adds the sprint and fatigue
  icon, the vehicle status icons and crosshair and vehicles on the compass
  (section 19).
- Plays on a United Offensive server on foot: `+sprint` (ALT, rebindable
  from the controls screen) sprints with UO's gun pose, breaths and
  footsteps, an LMG's sight deploys its bipod on sandbags, at a
  `trigger_mount` or prone, and the sight cooks a pulled grenade.
  Prediction runs UO's movement, 1.5's plus the sprint, the fatigue bar,
  the bipod's locks and yaw cap, the cook and automatic fire, replayed
  against retail captures
  ([coduo-movement.md](research/coduo-movement.md)).
- Drives, rides and fires United Offensive vehicles: the use key gets in
  and out, the weapon wheel changes seats, the stock keys drive, turn a
  tank's hull to the view (SPACE) and fire the main and alternate guns,
  and the camera sits where UO's does (a tank driver's chase camera and
  gunsight, the gunner on its gun with its sway and firing shake, a
  jeep's riders turning with it), with your own body drawn in the seat
  where retail draws it. Prediction holds a seat's view clamps and sight,
  replayed against retail captures
  ([coduo-vehicles.md](research/coduo-vehicles.md)).
- Draws the server's script menus (team, weapon, view map, call vote,
  quick messages) from their stock `.menu` files with their layout,
  pictures and sounds, driven by mouse and keys as CoD's UI drives them,
  or answers the team and weapon menus straight from `--team` and
  `--weapon`.
- Draws retail's scoreboard on Tab and through intermission: the panel
  with the game type, map, host name and address, the objective, the
  team banners leading team first with their totals and average ping,
  rows in the server's order with status icons and your own row lit,
  and the scroll bar with page keys, wheel and `weapnext`/`weapprev` once it overflows
  ([cod11-hud-protocol.md](research/cod11-hud-protocol.md) 3.1).
- The stock quick message menu (`V`): commands, statements and responses
  go to the gametype script, which plays the voice line and sends the team
  chat line.
- **Spectates.** Every player is an assembled, animated soldier. Kill feed,
  chat, scoreboard, sounds, tracers, impacts, muzzle flashes, ejected
  brass and script effects (the S&D bomb, the bombzone exploders, mines,
  `playLoopedFx`, `playFxOnTag`) come off the same events retail reads.
  Tracers start at the shooter's posed muzzle tag (a mounted gun's own,
  a gunner's stance point), on 1.1, 1.5 and UO alike, and so does another
  body's fire sound (UO's MG fire loop too), carried and turned along
  with it; our own starts at the viewmodel's muzzle.
- **Plays.** Move, jump, crouch, prone, lean, fire, aim down the sight,
  reload, melee, use, switch weapons, on retail's default binds. A usercmd
  is built every 8 ms, like a 125 fps retail client, and they go out at
  retail's `cl_maxpackets` 30 with `cl_packetdup` 1 off the LAN.
- Keeps its own weapon selection as retail's `cg_weaponSelect` does: it
  answers the server's switch and pickup commands, swaps off a dry weapon,
  takes `weapalt` and `weapon <name>`, and keeps asking for the gun a
  ladder took out of your hands. A select shows retail's weapon-slot strip,
  UO's with its satchel and binocular slots.
- Predicts your movement by replaying unacknowledged cmds through the same
  step the server runs, free-flying spectator, corpse (a linked one too) and
  intermission included. A correction eases out over 100 ms.
- A script's `shellShock` plays retail's effect from its `.shock` file: the
  view trails and shakes, the mouse damps, the walk slows to 0.4, the mix
  ducks under the shellshock loop. `cg_shellshock <secs> [file]` tries one
  locally.
- Draws and stamps cmds on retail's client clock, which slews a millisecond
  or two per snapshot instead of stepping on jitter; `cl_timeNudge` works.
- First-person weapon with your team's hands, sight zoom at the weapon's FOV,
  sniper scope overlay that follows the sway and the hit kick, and your own
  fire, reload and footstep sounds played off the prediction. The gun bobs,
  breathes, lags a turn and shifts with speed and stance off the weapon
  file's keys, as retail's cgame draws it, and the eye bobs with the walk.
- Fire recoil off the weapon file's view and gun kick keys: the view kick
  rides your cmd angles, as retail's does, rolls the view with it, springs
  back to centre and clears on a respawn.
- A hit kicks the view along its direction, a landing dips it, and a script
  `earthquake` or a mounted MG's fire shakes it, with retail's curves.
- Mounted MG42s with their fire anim and flash; on the gun the view rides
  `tag_player`, a hit still kicks it, and the crosshair turns into the
  gun's reticle. On 1.5 and UO the view is held inside the gun's arcs and
  the gun turns with it; on 1.5 the view jitters as the gun fires, on UO
  the eye sways and shakes and a hit does not kick it.
- The HUD the stock `hud.menu` lays out: crosshair that opens with spread,
  health, ammo, stance, compass with objectives and teammates, use hints with retail's pick-up and swap lines, hit
  direction with retail's red flash, chat, announcements and the "You killed" centre print. Also the gametype script's own HUD
  elements (S&D clock, bomb icons, progress bar) in retail's fonts, head
  icons over players, and over them the talk balloon on a teammate typing
  or in a menu, the disconnected icon and the killcam marker. A spectator following someone sees their HUD, weapon
  and scope. On 1.5 and UO `^8` and `^9` in any text draw in the server's
  `g_TeamColor_Allies` and `g_TeamColor_Axis`, as 1.5's team chat uses
  them.
- Shows a running vote as retail's cgame does (what it's for, the seconds
  left, the yes and no counts with the keys to press) and plays its sound;
  F1 and F2 vote yes and no, and the call vote menu's restart, next map,
  map, type-and-map and kick picks call one.
- Follows the server through a map change: loading screen, downloads, new
  map. Open menus close, as in CoD.
- Wide and ultrawide windows, a vcod extension (retail stretches its 4:3
  layout over any window): `vc_widescreen 1` (archived, live, default)
  widens the view past 4:3 and keeps 4:3's vertical fov (Hor+; `cg_fov`
  and every sight, scope and seat fov stay a 4:3 view's width), and moves
  the HUD's side pieces (compass, stance, chat, kill feed, votes to the
  left; health, ammo, weapon strip to the right) to the window's edges
  while the crosshair, hints and centre prints stay in the middle.
  `vc_widescreen 0` is retail's fixed horizontal fov with the whole HUD in
  the 4:3 box. Menus always draw in the centred 4:3 box, the cursor mapped
  through it; full-screen fades, backdrops and the damage flash cover the
  whole window (cod11-hud-protocol.md section 10).
- `r_fullscreen 1` is borderless on the window's monitor at the desktop's
  resolution, `r_fullscreen 2` exclusive at the monitor's mode closest to
  the `r_mode` size (borderless on Wayland); `r_mode -1` with
  `r_customwidth` / `r_customheight` asks for any size, and Alt+Enter
  flips `r_fullscreen` and restarts the video, as retail's does
  (cod11-front-end.md section 21).
- Retail's drop-down console on `` ` ``. It takes the commands under
  [Console](usage.md#console), retail's `rcon` among them; binds and archived cvars persist in `main/vcod_mp.cfg`.
- `vc_bicubic 1` (archived, live, default 0) filters lightmaps with a cubic
  B-spline instead of retail's bilinear, so low-res shadow edges stop
  stepping. A deliberate divergence from retail's look.
- `vc_bloom` (archived, default 0) adds a glow around bright pixels, which
  retail never had: the value is the strength (1 is subtle) and
  `vc_bloomThreshold` (default 0.8) the on-screen brightness where it
  starts. The HUD doesn't glow. Off by default to keep the retail look.
- Beyond retail, off by default: `vc_ssao` (archived, live) adds
  screen-space ambient occlusion to the lightmapped and ambient-lit
  surfaces, its value the strength (1 is subtle, 2 heavier). Retail has
  none; the blends, effects, sky, weapon and HUD are left alone.
- `vc_shadows 1` (off by default, live): players, corpses, entity models,
  moved brush models and waving flags cast soft sun shadows on the world
  and on each other, glass and water included; foliage, fences and
  grilles cast and block through their alpha, blended glass and water do
  not. A deliberate divergence: retail draws none. The baked lightmap
  shadows stay as they are, so a point the map already shadows is not
  darkened again, and a cast shadow is as dark as the map's own baked
  ones measure, per 1024-unit square where the lightmaps say enough
  ([cod11-light-grid-and-leaf-lights.md](research/cod11-light-grid-and-leaf-lights.md),
  section 12).
- `vc_sunShafts` (archived, live, default 0): light shafts radiate from
  the sun through gaps in whatever stands in front of the sky, the value
  their strength (1 is moderate). Only maps whose `.sun` file gives the sun
  a flare cast them; they fade as the sun leaves the screen, feed
  `vc_bloom`, and never cover the HUD. Retail has none.
- `vc_softParticles` (archived, live, default 0): smoke, dust, fire and
  explosion sprites fade out where they meet walls and ground instead of
  cutting a hard line into them. The value scales the fade distance (1 is
  half a sprite's size, at most 32 units). Retail has none on CoD1 (CoD2's
  own depth feather draws whatever the cvar); impact marks,
  world shader stages and the HUD are unchanged
  ([cod11-gamma.md](research/cod11-gamma.md), section 8).
- `r_vc_depthprepass` (live, not archived, default 1 on CoD2 and CoD4, 0
  on CoD1 and UO, whatever a config says): the opaque lit world
  and entity surfaces lay their depth down in a pass of their own first,
  then draw their colour under an EQUAL depth test. The pixels are the
  same; on a real GPU it took mp_crossfire from 85 to 93 fps at 1080p,
  on lavapipe it costs 10-15%, so set it to 0 there
  ([cod4-client-perf.md](research/cod4-client-perf.md), section 12).
  On a CoD4 frame at one sample with feathered effects in view it also
  writes their `floatZ`, as 1.7's prepass does, over 1.7's own set of
  materials, so the frame needs no depth copy (section 35;
  `VCOD_C4_FLOATZ=0` keeps the copy).
- `r_vc_normalMaps` and `r_vc_film` (live, default 1), debug switches
  for the profile's sweep: 0 stops CoD4's lit surfaces reading their
  normal maps, or drops the vision set's film grade. `r_fog 0` (cheat, as
  1.7's) draws no fog. `tools/cod4_perf.sh` with `SWEEP=1` measures each
  of these and the other optional pieces of a CoD4 frame a pass at a
  time ([cod4-client-perf.md](research/cod4-client-perf.md), section 25).
- `vc_hdr 1` (archived, takes effect on `vid_restart`, default 0) draws the
  world into a float target, so stacked effects, flares and the sun go past
  white and a tonemap rolls them off instead of clipping; ordinary surfaces
  keep retail's look, and bright overbright ones lose some of their clip.
  With bloom on, only those over-white pixels glow. `vc_grade`,
  `vc_vignette` and `vc_grain` (archived, live, default 0, value is the
  strength) add a period-film colour grade, a vignette and film grain.
  None of them touch the HUD. All are divergences from retail's look
  ([cod11-gamma.md](research/cod11-gamma.md), section 9).


### Dedicated server (`vcod-server`)

- `vcod-server <map> --game cod4` hosts a CoD4 1.7 `war` level a 1.7
  client joins: `getinfo`, `getstatus`, the challenge, the connect and the
  seven stats packets answered as retail answers them, the gamestate with
  retail's engine configstrings and the script's, the pure `cp` check
  ([cod4-handshake.md](research/cod4-handshake.md) 9). Off the LAN it
  refuses on `sv_minPing` / `sv_maxPing` and holds a client out of the
  world to a message a second as 1.7 does, but never asks the authorize
  server: retail answers an off-LAN challenge only after that server's
  `accept`, or once it has been silent for 20 minutes, where vcod-server
  answers at once (cod4-handshake.md 15). The stock scripts
  run the connect, the team and class menus (`mr`), the class's loadout off
  the client's stats block and the spawn, with retail's server commands in
  retail's order; a client on a team with no class follows a teammate
  ([cod4-gsc.md](research/cod4-gsc.md) 7). A spectator follows as
  `scr_game_spectatetype` allows: attack cycles to the next player, the
  sight to the one before, and where it may fly, melee lets go
  ([cod4-pmove.md](research/cod4-pmove.md) 15). Players and spectators move:
  each cmd runs 1.7's pmove as it arrives over the map's clip map and the
  other bodies, and a walker replaying a retail capture's cmds moves as on
  retail within the trace kernels' float model (0.04 units over 40 s)
  ([cod4-pmove.md](research/cod4-pmove.md) 15). A player mantles over a
  `MANTLE_ON` or `MANTLE_OVER` wall on 1.7's clips and timing, matched
  against a retail capture, and leaves 1.7's blockage at its end for the
  other players (17). The snapshots carry the
  script's entities but the hidden ones, unculled as on retail, and the
  players' bodies, their legs and torso anims picked by 1.7's anim script
  as retail picks them (a replayed walker's on every snapshot), with
  their view angles, lean and prone pitches as 1.7 copies them. Bullet
  weapons fire, burst, reload and sight as 1.7's `PM_Weapon` runs them; a
  round leaves from the eye with its walk bob and lean along the gun's
  angles (the sway, the movement tilt, the idle drift and the recoil
  spring, every round of a strafing, leaning capture through retail's
  line), traces the bodies' posed hit boxes through the antilag,
  penetrates walls and bodies by 1.7's depth table (a round through a
  mp_crash wall takes retail's damage) and goes on through script models
  and brush models (a destructible car's window breaks before rounds
  pass it), and the
  stock scripts' damage and killed callbacks, the obituary, the corpse
  (with a death anim, set down on the ground, its `startRagdoll` at
  retail's moment), the dropped weapon (its
  ammo handed to a player holding the same weapon) and the killcam (the
  world as it was, so the victim dies in it) follow, matched against a
  retail kill capture, entity numbers included
  ([cod4-combat.md](research/cod4-combat.md)). Frag, smoke and flash
  grenades are pulled, cooked and thrown, bounce (off the map's static
  models too, a replayed frag word for word as on retail, spin included:
  the server draws glibc `rand()` as 1.7's one stream, so a gate replays
  a capture from its seed, cod4-combat.md 10.9), and burst with radius
  damage; a flashbang blinds whom it reaches and a concussion grenade
  shocks and slows its victims, through the stock scripts'
  `shellShock` (10.8); the grenade launcher and the rocket launcher
  fire; the melee key swings and hurts;
  `kill` works under `sv_cheats` (10, 11). Every perk acts: the stock
  scripts' damage, loadout and Last Stand ones and the engine's (spread,
  reload, fire rate, breath, sprint, Deep Impact's depth, Martyrdom's
  dropped grenade, Last Stand's down state, Eavesdrop's enemy voice)
  ([cod4-gsc.md](research/cod4-gsc.md), "The perks"). Voice chat is
  relayed under `sv_voice 1` (off by default, as retail): the team, the
  dead to the dead, `voice_global`, `voice_deadChat`, `voice_localEcho`,
  Eavesdrop's 400 units, the Mute Players list, `isTalking()`
  ([cod4-voice.md](research/cod4-voice.md) 3). `score` gets retail's `b`
  board ([cod4-hud.md](research/cod4-hud.md) 7). A stat a script's
  `setStat` changes goes to the client as 1.7's `N`, into its profile
  ([cod4-gsc.md](research/cod4-gsc.md)); rcon's `status`, `dumpuser`,
  `say`, `tell`, the kicks (`kick` bans for `sv_kickBanTime`, `onlykick`
  does not), the `ban.txt` bans, `heartbeat`, `killserver`, the dvar
  commands (`seta` archiving, `dvar_bool`/`int`/`float`,
  `setdvartotime`), `writeconfig`, `vstr`, `exec` (a zone's rawfile,
  then the iwds and files in 1.7's search order) and `cmdlist` run as
  1.7's ([cod4-handshake.md](research/cod4-handshake.md) 14). `g_log` writes
  1.7's `games_mp.log` (the `InitGame`, `ShutdownGame`, `ExitLevel`,
  chat and weapon lines and the scripts' `J;`, `Q;`, `D;`, `K;`) under
  the home path, and `g_inactivity` warns and drops an idle player
  ([cod4-server-gaps.md](research/cod4-server-gaps.md)). The script's HUD
  elements (the prematch's "Waiting for teams" and "Match begins in" over
  their timer, the spawn hint, the score splash) go out in the
  playerstate, matched element for element against retail captures
  (5.1). A client that leaves, or reconnects into its own slot, leaves no
  stale player behind in the scripts. `sv_fps` (10..1000, default 20)
  sets the frame as 1.7's does, read every frame: `level.time` and the
  snapshots step `1000 / sv_fps`, and the scripts' `wait` counts frames,
  so at 40 a `wait 5` is 2.5 s, as on retail
  ([cod4-server-frame.md](research/cod4-server-frame.md) 8).
  `map`, `devmap`, `map_restart`, `fast_restart` and `map_rotate` over
  `sv_mapRotation` run as 1.7's do, so a match that ends rotates to the
  next map or restarts, a ranked (`sv_pure 1`) match included, through
  the intermission's scoreboard, and `callvote` and `vote` run 1.7's engine vote
  ([cod4-map-cycle.md](research/cod4-map-cycle.md)); a `--connect`ed
  vcod loads the next map from its new gamestate. Every builtin the stock
  scripts of the 21 stock maps call has a handler
  ([cod4-gsc.md](research/cod4-gsc.md) 3.1), so the maps' looping and
  triggered effects go out as `ET_LOOP_FX` and `ET_FX` entities. The
  killstreak hardpoints run the stock scripts: the airstrike's three
  `ET_PLANE` jets fly over and their bombs hurt whoever is under them, and
  the helicopter (`ET_HELICOPTER`) flies its map path, turns its gun on
  its targets, fires its cannon and rockets, pitches and rolls off its
  acceleration, wanders in the hover, and leaves; rounds, rockets and
  blasts hurt it, so it smokes and crashes when shot down (3.2). Rockets
  wobble, curve and accelerate as 1.7's do, an RPG bends toward the
  attractor the stock script puts on the helicopter, and the
  helicopter's guided rockets steer at their target (3.2, "Missiles
  under 1.7"; no retail capture of a rocket's flight yet), its model collision
  traces at the bind pose, and a physics launch or a physics explosion
  only reaches the wire. `bulletTrace` scores bodies' posed hit boxes and
  the helicopter, `getTagOrigin` on a player reads his posed bone; the
  sight and cone traces meet bodies as their link boxes (3.3).
  `--set fs_game=mods/<name>` runs a mod: its `mod.ff`, its scripts and
  iwds over the stock ones, its weapon files read off the iwds as
  retail's are, named in configstrings 0 and 1 as retail's server names
  them, the referenced iwds included
  ([cod4-fastfile.md](research/cod4-fastfile.md) 13). A client that
  lacks the mod's iwds or `mod.ff` downloads them as from retail
  (`sv_allowDownload`, on by default; `sv_wwwDownload=1` with
  `sv_wwwBaseURL` redirects instead;
  [cod4-handshake.md](research/cod4-handshake.md) 11).
  `--bots <n>
  [--bots-shoot]` adds vcod's bots, which join through the team and class
  menus, find their way over a graph of the map's clip map (mantles
  included), fight, play the `sd` and `sab` bomb (take, carry, plant,
  defuse), `dom`'s flags and `koth`'s radio, and rejoin each new level
  ([bots.md](bots.md#cod4)). The touch pass and the use key reach the
  game objects' triggers, plain `trigger_use` and dropped weapons (the
  use key swaps the gun in hand for one lying there), a planter is linked
  and raises the briefcase ([cod4-gsc.md](research/cod4-gsc.md) 9).
  The six stock maps' mounted SAWs (`misc_turret`) go out as 1.7's
  `ET_MG42`, show their use hint, mount on the use key, hold the view in
  their arcs, fire off the turret weapon file and let go on a second
  press, as the retail mp_backlot capture does, and stand the gunner
  where 1.7's turret anims put him, on the server and on every watching
  client; bots man one that covers a lane enemies use and throw a live
  frag at their feet back ([bots.md](bots.md), "CoD4"). A live frag lying near
  a player shows its hint and the frag key throws it back on its own
  fuse, as retail 1.7 does ([cod4-gsc.md](research/cod4-gsc.md) 9.2,
  9.3). No drivable vehicles.

- Answers server browsers, accepts retail 1.1 clients, sends the gamestate
  and delta snapshots. Run from a patch 1.5 install it speaks 1.5 instead
  (protocol 6): 1.5's queries, cvars, menus and gametype scripts, with
  the configstring table 1.5b builds on mp_carentan
  ([cod15-protocol-diff.md](research/cod15-protocol-diff.md) 13). On 1.5
  and UO the entity cull follows those engines: a sounded entity rides
  past the PVS for 300 ms, a looping one while it loops, a sounded
  `script_origin` never, and cull fog drops whatever lies past its far
  distance, matched against live 1.5b and UO runs (same doc, 15). The
  compass reads each engine's `SV_inSnapshot`: a teammate off the map
  shows on 1.5 and UO and not on 1.1, one past a cull fog shows on 1.5
  and UO, a sounded one leaves it. An eye in a solid
  leaf is sent no entities, on all three, as retail does.
- Hosts United Offensive 1.51 (protocol 22) from a UO install with
  `--mod-dir uo`: UO's queries, cvars, configstring layout, 63 weapons,
  means of death and event numbering, and UO's stock `dm`, `tdm`, `sd`,
  `ctf`, `dom`, `hq` and `bas` scripts. Players sprint, tire, deploy the bipod and cook grenades by
  UO's rules. An LMG shows the mount hint and deploys on the sandbag
  brushes and on a script's `spawnTriggerMount` as on retail
  ([coduo-movement.md](research/coduo-movement.md) 11). The join, a spawned player's playerstate and a kar98k's
  shots and reload match the retail 1.51b server
  ([coduo-diff.md](research/coduo-diff.md) 20). Artillery strikes,
  smoke grenades, satchel charges, the flamethrower's reported damage and
  mounted MG overheat run UO's rules too; the strike, the smoke, the bipod
  MG, the satchel and the overheat match retail's captures, and so does a
  satchel shot off or set off by another's blast (same doc, 21). The four UO
  modes' configstrings match retail's, and so do a flag pickup and capture
  in `ctf`, a flag taken and retaken in `dom`, a radio captured and
  neutralized in `hq` and a base breach in `bas`, HUD, compass and
  scoreboard included. So do the endings: `ctf`'s flag drop and 20 s
  return, `dom`'s domination bonus, `hq`'s wave scoring, `bas`'s bomb
  plant, defuse and blast, and in all four the round end, the score or
  round limit, intermission and the restart `exitLevel` falls back to;
  `ctf`'s flag returned by a defender's touch, the round clock and time
  limit in `ctf`, `dom`, `bas`, `hq` and `tdm`, `tdm`'s killcam, and a
  rotation a script sets that names another map (same doc, 22).
- Hosts United Offensive's tanks and jeeps: they spawn with retail's
  entity numbers and configstrings, the use key boards the one you look
  at, `nextvehslot`/`prevvehslot` change seats, and they drive, aim, fire
  shells and MGs, overheat, sound, take damage and run players over by
  the 1.51 game module's rules, a tank's driver shut against every round
  but his own tank's. Their hulls, posed per bone, stop rounds, grenades,
  shells and blasts; a jeep takes bullets and a struck vehicle a shell's
  whole damage as on retail, a vehicle driven into another shoves it, and
  one wedged seven seconds destroys itself, a wreck's respawn included.
  A vehicle's death puts its riders out as retail's does: each takes the
  5-point probe hit through his damage callback, and one it leaves whole
  looks for a dismount spot; the others' corpses slide out of the hull.
  A body inside a vehicle is pushed out of it before its move, as
  retail's `ClientThink_real` does: a moving one runs him over (damage
  and `vehicle_hit_person`), and a prone one under it takes 10 a cmd,
  both ahead of his move; a driverless one is itself the attacker. A seated driver's and gunner's body meets a level ray where retail's
  does, to a quarter unit, with the jeep's steer blend on the driver.
  A Panzer IV's and a Willys's drives replay on ours as retail's to the
  field, and so do the seat changes; a vehicle and its sound blends leave
  a client's snapshots where retail's do
  ([coduo-vehicles.md](research/coduo-vehicles.md) 10-14).
- Runs Activision's own gametype and map scripts on `vcod-gsc`, a VM for
  CoD's `.gsc` language that lives in this repo. Menus, spawns, scoring,
  rounds and limits all come from the stock scripts, not from Rust. A
  script runtime error ends its thread only under `developer 1`; below it
  the thread runs on, as retail's does
  ([cod11-gsc-language.md](research/cod11-gsc-language.md)). All five
  stock gametypes are checked against retail: `dm`, `tdm` and `sd` end to
  end, `re` and `bel` through their key events
  ([cod11-gametypes-re-bel.md](research/cod11-gametypes-re-bel.md)).
- Shared pmove with capsule players that block and push each other. Falls
  stun and hurt.
- Bullets trace the world, players, static props and script models,
  through the stock damage callback with per-bone hit locations, each
  body posed as retail builds it for that trace (on 1.5 and UO its spine
  eases a step a build, and a body a script just turned stands in the
  script's angles until its next cmd). Rifle
  rounds go through players, every round goes through glass. Curved
  patches, terrain and brushes clip a round or a moving player in the
  arithmetic of each patch's server build, and a patch only from the BSP leaves that list
  it, in retail's leaf order, so a round, an item or a player slips
  through the seams retail's does. Where two brushes or terrain
  contacts nearly tie, the leaf order picks the surface a body stands
  on. Terrain past a map's playable walls, which no leaf on the trace's
  path lists, lets rounds and items through as retail's does. A submodel's patches and terrain clip
  only while its entity is linked. Shots,
  swings and throws start from each patch's muzzle (1.1's truncated
  point, 1.5's and UO's bobbed and leaned one, a UO rider's 28 over his
  origin, a UO bipod's traced to the wall in front). On 1.5 and UO each shot kicks the
  server's gun spring, so a sighted burst climbs as retail's does. Melee
  works. Grenades are real
  missiles that bounce (off script models, and off a player's posed bones
  at an eighth of their speed), rest (on a player too, falling once he
  leaves on 1.5 and UO; the bounce and the arc keep 1.1d's x87
  arithmetic and 1.5b's and UO's float stores, so a frag rolls over
  terrain on retail's bits; on 1.5 and UO the bone trace takes the stores
  too, and the body it meets is the engine's own unnormalized pose and
  skeleton build, so a strike on a still prone body lands on retail's
  bit) and explode
  with retail's falloff, and a blast
  walks its victims in retail's area-tree order, so the guy in front still
  eats it for the guy behind. The panzerfaust (and UO's bazooka and
  panzerschreck) fires a rocket that flies, strikes and explodes as
  retail's does; a missile at the sky is freed on 1.1 and 1.5 and flies
  on through it on UO, and one that drops fast into water splashes (UO
  frees a smoke grenade there, and a UO grenade that goes off under water
  goes off on the surface). A rocket or a swing hurts a UO `setTakeDamage`
  model directly, and a swing at nothing raises no event.
- Corpses in the eight-slot body queue, dropped weapons, pickups by touch or
  use key, items that fly retail's arc and respawn on its timer.
- Search & Destroy end to end: plant, defuse, progress bar, compass
  objectives.
- Mounted MG42s: mount, aim inside the arc (1.1's 15-degree step and
  view snap, 1.5's and UO's plain clamp), fire, dismount; 1.1d, 1.5b and
  UO captures replayed against ours. A round passes the gun, as retail's
  does, not the gunner.
- Map triggers (a `trigger_damage` fires off rounds, swings, grenades,
  blasts and UO shells, measured on UO's mp_uo_stanjel and mp_foy's
  belltower; a script-spawned one is a copy of the world, measured on
  1.1d and 1.5b), script movers that carry and shove players and items, and
  `linkTo` on script models, brush models, items, turrets, player tags and
  tags on `attach`ed models.
- Intermission, `map_restart` and `sv_mapRotation` the way retail runs them,
  a rotation a script `setCvar`s included.
- Spectator follow and the killcam, replayed from a ring of archived frames.
- rcon with retail's commands and replies, bans, the master heartbeat,
  zombie slots and retail-measured pings. `g_log` writes retail's
  `games_mp.log` (`InitGame`, `RestartGame`, `ShutdownGame`, chat, tell
  and pickup lines in the patch's own forms, the scripts' `logPrint`)
  under the home path ([cod11-map-cycle.md](research/cod11-map-cycle.md)
  9). `getinfo` and `getstatus` answer
  byte for byte as the three retail servers do, odd challenges included.
- Votes (`callvote`, `vote`) with retail's kinds, refusals, configstrings,
  pass rule, 30 s timeout and 3 s delay before the command runs: 1.1's
  seven kinds under `g_allowVote`, 1.5's twelve under their own
  `g_allowVote*` gates, measured against both retail servers. Bots are
  not counted as voters, and a map vote needs the map on the server on
  1.1 too. A script's `allowComplaint` opens retail's teamkill
  complaint, which `vote` answers and the client prompts for.
- Publishes retail's pak lists and checksums in the systeminfo, serves the
  paks it lists to clients that lack them at retail's pace (on 1.5 and UO,
  `sv_wwwDownload` sends them to `sv_wwwBaseURL` instead, connected or
  disconnected, and after a failed www transfer streams the pak on 1.5 and
  redirects again on UO, as each retail server does), and with `--set sv_pure=1` checks each client's pak checksums the way a pure retail server
  does. `sv_minPing`/`sv_maxPing` refuse off-LAN clients by challenge ping,
  and an off-LAN client's messages wait out its rate and `snaps`. Big
  messages, the gamestate included, go one fragment per frame, and a client
  not yet in the world gets retail's bare keepalive (every frame on a LAN,
  a second apart off it).
- `--bots` adds bots that join through the stock menus and roam a nav graph
  built from pmove runs, ladders and jumps. They play the objectives: S&D
  plants and defuses, Retrieval carries and escorts, Behind Enemy Lines
  hunts. Only the two teammates nearest a shot go to it, so a big team
  spreads out instead of moving as one. With `--bots-shoot` they fight back, with a reaction delay, a turn
  cap and aim that settles on target and trails a target crossing their
  view, firing before it settles and strafing on their sights. They spot new enemies in a sight
  cone, keep their target, and turn on whoever shoots them. They fight from behind a low wall they duck
  under to reload or round a corner they lean out of, change how they
  peek from one peek to the next, and go after a lost enemy from a spot
  that looks onto where he was. A hurt bot, or one whose clip ran
  dry, backs off to a spot out of its enemy's sight, reloads and waits
  there, and a hurt one walks to a health pack lying near. A clip under
  half reloads once the fight goes quiet. A frag goes at an enemy behind
  cover the gun cannot reach, or where a standing one was lost from
  sight, lobbed so the missile code's own flight bursts on him, never by
  a teammate with friendly fire on, and a bot runs from a live frag it
  sees or hears land near, or one a dying player dropped. They are still
  bad at it. So am I.
  On a 1.5 or UO server the graph is proven under that patch's movement.
  UO bots sprint long straight legs while they have the breath, pick from
  UO's weapon menus and deploy a light machine gun's bipod to fire it, cook
  their frags so they burst near landing, throw one only along a flight
  that clears bodies and goes off clear of the thrower
  and run from live ones, and play
  the objectives of UO's `sd`, `ctf` (take, carry, return), `dom` (stand
  in the flag zones), `hq` (take and neutralize the radio) and `bas`
  (fetch a bazooka or panzerschreck lying about and breach a base with
  rockets from a spot with a shot at it, plant on a breached base and
  defuse), and board UO vehicles with the use key: a driver steers along
  the graph over ground its hull fits and holds for a teammate in its way,
  a tank's driver stops to lay the turret on an enemy or an enemy vehicle
  and fire, a gunner fires the MG, and a bot on foot with a launcher fires
  rockets at an enemy-crewed vehicle. The nav graph is rebuilt when the script
  deletes walls, as `bas` does to a breached base. A bot jumps through a
  window too low for a standing body with crouch held in the air, as
  out of mp_cassino's axis room and two rooms on mp_streets.
- Bot management over rcon: `addbot`, `removebot` and `listbots`, names
  like `[BOT] Sir Camps-a-Lot` drawn without repeats, team balance that
  moves only dead bots, and a `vc_bot_minplayers` floor a joining human
  takes a bot's place under, and four skill levels (`vc_bot_skill`, or
  per bot with `addbot ... skill N`). The `vc_bot_*` cvars apply live
  (docs/usage.md, "Bots").

## What it doesn't do

Retail does a lot more than this list. These are the gaps you'll hit first.

**Front end.** Main menu, browser and its popups, options, quit and error
popups work. The options screens' texture detail, filter and quality,
model detail, dynamic lights, sync, blood, wall marks, ejecting brass and
sound quality take effect as retail applies them; the sound provider and
NVIDIA fog are stored but ignored, and the language picker stays hidden.
Show Compass and Team Overlay do nothing, as in retail 1.1, 1.5 and UO. CD key prints
"not in vcod yet".
**CoD4x servers.** Most public CoD4 servers run CoD4x, which parks a
1.7 client and hands it to CoD4x's client-update backend; no 1.7 client
plays there. vcod refuses such a server at the challenge
([cod4-handshake.md](research/cod4-handshake.md) 13). CoD4x servers are
hidden: the browser leaves them out of Local, Internet and Favorites
unless vcod's own `ui_browserShowCoD4x` is 1, which lists them with
`[CoD4x]` before the name. vcod joins stock 1.7 servers only; CoD4x
support is open.
There is no listen server: Start New Server always runs a separate
`vcod-server`, Dedicated No joins it as an ordinary client (it needs the
password too), and LAN or Internet does not turn the window into a server
console.

**Client**

- Patch 1.1 and 1.5 servers (protocols 1 and 6) are joined from either
  install by switching protocol when one refuses the install's connect. A pure 1.5 server wants a 1.5 install as
  `COD_DIR`
  ([cod15-protocol-diff.md](research/cod15-protocol-diff.md)).
  Prediction runs 1.5's movement there (jump penalty, no prone dive, the
  stepped stance, the prone body's swing search, the footstep changes,
  `disableWeapon`), and the bobs take the server's bob cvars
  ([cod15-movement.md](research/cod15-movement.md)).
  PunkBuster servers refuse it (vcod runs no PunkBuster), and
  `cg_autoscreenshot` does nothing (no screenshots).
- On a CoD2 1.3 install `--connect` (or a join from the main menu's
  browser) decodes the gamestate and delta
  snapshots, sends the pure checksums
  ([cod2-protocol.md](research/cod2-protocol.md) 11), downloads the
  non-stock iwds the server references and the install lacks
  (`cl_allowDownload`; over UDP or a www redirect's HTTP under
  `cl_wwwDownload`) into its `main/` or the server's `fs_game`
  directory before the map loads, with the progress on the connect
  screen, then opens its search path again over them and the mod
  (cod2-protocol.md 17.5); a refused download or a map that does not
  load returns to the menu with retail's message, shows the server's
  script menus (server info, team, weapon, Esc's `ingame` and the mute
  and vote menus its buttons open) for the player to answer, or
  with `--team` (and `--weapon`) answers them itself, and plays or
  spectates, and draws the server's script
  models, turrets aimed by their controllers, weapon and health items,
  missiles, brush models, player bodies with the weapon in hand, and
  corpses under the server's fog ([cod2-protocol.md](research/cod2-protocol.md) 12), and the
  view weapon of your own or a followed body (hands and gun animated off
  the playerstate, swaying, bobbing, tilting and lagging the turn as
  retail's `CG_AddViewWeapon` places it, the sight clips crossfading over
  0.5 s but a main clip change a cut,
  [cod2-weapons.md](research/cod2-weapons.md) 6-7), the playerstate's
  hudelems and the stock `hud.menu` HUD: ammo counter, weapon name and
  mode icon, grenade counts, stance icon and the prone-blocked line,
  compass with north (sprung as it turns), teammates (a talking one
  flashing), objectives and fire pings (an MG42's gunner too), use and
  pickup hints (fading out as retail's), the low-health overlay, the red
  damage flash, damage direction icons, the crosshair spread with the
  weapon and a scope's overlay with its zoom (`adsZoomFov`, the gun
  hidden under it; an iron sight zooms the gun with the world); the view takes the damage kick, the scope's idle
  sway and bob, earthquake and MG42 shakes and the shellshock off
  `shock/*.shock` (screen blend, view kick, mouse scale and turn caps,
  sound mix and the cmd's 0x800; the console's `cg_shellshock <secs>
  [file]` too), the shot's view kick roll included, and a held breath
  down a scope plays its breaths, heartbeat and `hold_breath.shock` mix
  ([cod2-hud.md](research/cod2-hud.md) 16-18; no health bar, as
  retail's default). The gametypes' hudelems draw their
  timers, clocks (sd's bomb stopwatch), scaled bars (plant and defuse)
  and the waypoints over the world (ctf flags, sd bomb sites, hq radios,
  death icons); players carry their head icons, the disconnected icon
  and the talk balloon, and in the killcam the local player's body its
  "you" icon (no voice chat icon: retail never shows one); a teammate's
  name under the crosshair, coloured by the health vcod-server sends in
  `stats[4]` (`cg_drawCrosshairNames`); a spectator sees `SPECTATOR`,
  the follow keys and `following` with the followed name (19). A player sends CoD2's
  buttons through the console binds (`default_mp.cfg`'s on a CoD2
  install: fire, sight toggle, melee and hold breath, frag, smoke,
  binoculars, use, reload, lean, `gocrouch`, `goprone`, `+gostand`, and
  `weapnext` with the weapon byte the client's own selection; the
  mouse buttons and wheel too, a notch one press), opens the server's
  script menus in the window (Esc the `g_scriptMainMenu` one, X
  `openscriptmenu ingame changeweapon`, the weapon pick applying at the
  next spawn; drawn and clicked as the main menu is, not yet checked on
  a GPU), holds up CoD2's scoreboard on `+scores` from the server's `b`
  reply, draws the chat lines, the say field, the kill lines (icons
  flipped as each weapon says) and the centre print where CoD2 does
  ([cod2-hud.md](research/cod2-hud.md) 13-15), draws the vote lines off
  configstrings 15..18, forwards other console commands (`say`,
  `callvote`) to the server, and is
  predicted through the server's own move and weapon (the sight's rise,
  zoom and clips, shots and reloads at the frame rate) with retail's
  error decay and first-person eye bob and lean
  ([cod2-movement.md](research/cod2-movement.md) 18,
  [cod2-hud.md](research/cod2-hud.md) 18.1). It loads
  the new map on a map change, keeps the map through `map_restart`,
  `fast_restart` and sd rounds (1.8-1.10), and goes back to the main menu
  with the server's reason on a kick, a server quit or a timeout
  (`EXE_ERR_SERVER_TIMEOUT` after `cl_timeout`). Its clock follows CoD2's
  `CL_AdjustTimeDelta`, one snapshot behind the newest, and like the
  cgame it keeps the last 32 snapshots and draws each entity between the
  two around the frame's time, so a late or lost snapshot does not hold
  other players still; it sends a packet
  a frame on a LAN and 30 a second off it, each repeating the last
  packet's cmds (`cl_packetdup` 1), shows `Connection Interrupted` and
  the `net_disconnect` icon when the server stops running its cmds, draws
  the lagometer under `cg_drawLagometer`, sends
  a changed `name`, `rate` or `snaps` as `userinfo`, honours
  `VCOD_NETSIM`, and adds net, clock, lerp and prediction lines to F3
  ([cod2-protocol.md](research/cod2-protocol.md) 18); before
  the server names its `g_scriptMainMenu`, Esc opens the main menu over
  the game (Back to Game, Disconnect). The prediction climbs
  mantles as the server does and clips other players' bodies as the
  retail cgame does, a unit inside the server's box (brush models and
  item pickups are left out); a landing dips the view and the gun. The mantle hint follows
  `cg_drawMantleHint`, the stance hint prints `cg_hudStanceHintPrints`
  and the chat `cg_chatHeight`, `cg_chatTime`, `cg_hudChatPosition` and
  `cg_hudSayPosition`; a held board scrolls on the wheel, the page keys,
  `weapnext` and `weapprev`. A gunner sees from the gun's `tag_player` with
  the view held inside its arcs, and so does a spectator following one;
  other gunners are drawn on their guns in the gunner anims
  ([cod2-turrets.md](research/cod2-turrets.md) 9, 15). It plays the events'
  sounds (footsteps, fire, reloads, impacts and passing rounds,
  explosions, `playSound` aliases), entity and missile loop sounds,
  `playLocalSound` (the announcer), `musicPlay`, `soundFade`, the
  level's ambient (crossfaded between tracks), sound blend entities,
  slave ducking under a master, `setReverb`'s wet level and alias
  subtitles through CoD2's aliases and falloff curves, fire sounds and
  whizbys from the shooter's `tag_flash`
  ([cod2-sound.md](research/cod2-sound.md)), and the effects of
  the same events and the looping effects
  ([cod2-efx.md](research/cod2-efx.md) 6: bullet impacts from the impact
  table, explosions, `playFx`, other players' muzzle flashes ahead of
  the eye rather than on the weapon tag, other players' bullet tracers
  from their weapon's `tag_flash`, `playLoopedFx`), the
  `effect_zfeather*` sprites fading into the world behind them as
  retail's PC renderer does
  ([cod2-materials.md](research/cod2-materials.md) 9.5). Static props
  draw their normal and specular maps per pixel, flags wave, an
  objective material on a static prop is not drawn (9.6, 9.7, 9.2).
  Players, corpses and entity models cast retail's shadow cookies: the
  nearest for their size (up to 24, `sc_wantCount` 12 by default) take
  the sun off the lit world under them and off models drawn over it,
  under `sc_enable` (default 1) and the other `sc_*` dvars (9.8); the
  view weapon reads none. Bright pixels glow and the sky bleeds
  round its edges, retail's default `r_glow` (9.9), and a shellshock's
  trail is a quarter desaturated, as CoD2's `shellshock` material draws
  it (9.10). An in-game menu with `blurWorld` (the Esc, team, weapon,
  vote and server info menus) blurs the 3D view behind it as retail's
  late post effects do (9.13). mp_harbor's water runs retail's wave
  simulation (a Phillips spectrum through an FFT into a height map each
  frame) under its cube reflection, the sun's highlight and the fog
  (9.12). The snow on mp_downtown, mp_harbor, mp_leningrad and
  mp_railyard falls as retail's particle clouds, a thousand flakes a
  cloud, and stops at roofs (9.14). Bare `vcod --game
  cod2` runs CoD2's stock front end (rects placed by their screen
  alignment, [cod2-fonts-menus.md](research/cod2-fonts-menus.md) 4): the
  server browser lists LAN servers (a `getinfo` broadcast), the
  `master.cod.pm` list for protocol 118 and favourites in CoD2's eleven
  columns and joins one, Start New Server
  runs `vcod-server --game cod2`, and the options screens set the
  binds, mouse, graphics (the video modes and refresh rates the monitor
  lists) and `snd_volume`. The browser names a map by its arena's long
  name. Player profiles are created, picked and deleted as retail's
  (`players/<name>/`, `active.txt`), each with its own
  `players/<name>/vcod_mp.cfg`.
- `vcod-server --game cod2` hosts players who join a team, spawn and
  move (ladders included, the gun put away on the rungs; mantles over
  flagged walls, the gun down for the high ones, the climb's block
  keeping other players off the ledge; a prone body's pitch follows the
  ground and steep ground forces a crouch; a run through foliage
  rustles, [cod2-movement.md](research/cod2-movement.md) 10, 12;
  players block each other and overlapping ones are pushed apart,
  cod2-turrets.md 14.2), see
  each other animated (the anim script's legs and torso picks, the
  weapon's fire, reload, melee and drop torso anims, the hit flinches and
  stumbles; no turn anims, which retail leaves off,
  [cod2-player-anims.md](research/cod2-player-anims.md) 9), the map's
  entities, its ambient effects ([cod2-efx.md](research/cod2-efx.md) 6.6) and the
  script's HUD, and fight: rounds struck on the body's hit boxes, posed
  as retail's server poses each client's body for its traces (cod2-combat.md
  9.5; the spread's draws and the gun's sway in the shot's angles are
  vcod's own, section 9.3), damage
  through the stock callbacks, deaths with their death anim, corpses
  that fall until they rest and slide on their death anim's root motion, the respawn, offhand grenades and rockets
  with their blasts, the script's `radiusDamage`, damage triggers, the
  binoculars, and the weapon a death drops, picked up by touch or swapped
  with the use key, which selects and raises the new one ([cod2-combat.md](research/cod2-combat.md); gated against retail captures
  of the weapon states, rounds, damage and a frag's flight, section 9), and the
  map's mounted guns: the use key mounts and lets go, the barrel follows
  the view inside its arcs, the body is placed on the gun and the trigger
  fires a round a frame, on stand, duck and prone guns (mg42 and 30cal)
  and the script's `spawnTurret` ones; a manned gun refuses a second
  player, and a gunner killed on the gun, by a suicide or another
  player's round while he fires, lets it go as retail's does
  ([cod2-turrets.md](research/cod2-turrets.md) 13, 14).
  Weapons, guns and use triggers share retail's one use list: the cursor
  hint names the one in front, and a `+usereload` tap with nothing to use
  reloads ([cod2-gsc-diff.md](research/cod2-gsc-diff.md) 5.4).
  All five stock gametypes (dm, tdm, sd, ctf, hq) run on every stock map:
  sd's rounds with the bomb planted and defused on the use key, ctf's
  flags taken and captured, hq's radios held. `map`, `devmap`,
  `map_restart`, `fast_restart` and `map_rotate` over `sv_mapRotation`
  (the script's `map_restart(persist)` and `exitLevel`) cycle the level
  as retail's do, and joined clients follow
  ([cod2-map-cycle.md](research/cod2-map-cycle.md)). It answers retail's
  client commands: chat, `score`, `kill`, the spectator's follow
  (`follownext`, `followprev`, the attack and sight buttons; the
  follower is sent the followed player's playerstate; the stock killcam
  replays the killer's view out of the engine's frame archive and
  ends on a use press,
  [cod2-killcam.md](research/cod2-killcam.md)), `where`,
  `callvote` and `vote` (`g_oldVoting`'s engine vote or the script's;
  its own bots are not counted as voters),
  and under `sv_cheats` `god`, `demigod`, `notarget`, `noclip`, `ufo`,
  `give`, `take` and `setviewpos`; rcon also runs `status`, `serverinfo`,
  `systeminfo`, `dumpuser`, `say`, `tell`, the kicks and retail's bans
  (`kick` and `tempBan*` for `sv_kickBanTime`, `banUser`/`banClient` into
  `ban.txt`, `unbanUser`; [cod2-protocol.md](research/cod2-protocol.md)
  3.4.2, 17.7). `g_log` writes 1.3's `games_mp.log`
  ([cod11-map-cycle.md](research/cod11-map-cycle.md) 9). `give all`
  hands over every weapon the level registered, not retail's slot
  rule. Its housekeeping is retail's
  ([cod2-protocol.md](research/cod2-protocol.md) 17): `rate` and `snaps`
  pace an off-LAN client's messages and a fragmented message goes a
  fragment a turn, a dropped client turns zombie for `sv_zombietime`, a
  silent one times out on `sv_timeout`, `g_password`, the private slots
  (`sv_privateClients`, `sv_privatePassword`), a full server and
  `sv_minPing` / `sv_maxPing` refuse with retail's replies, `getinfo` and
  `getstatus` answer byte for byte as retail's, and it serves a
  non-stock iwd over UDP (`sv_allowDownload`) or redirects to
  `sv_wwwBaseURL` (`sv_wwwDownload`) as retail does. It serves only
  `.iwd` files under its game dir, where retail serves any file. It
  never asks the authorize server, so an off-LAN client gets its
  challenge at once and its address stands in for its ban guid
  (`--ban-file` keeps `ban.txt`). Retail `CoD2MP_s.exe` 1.3 joins it, pure or not:
  it passes the menus, spawns, moves, fires, dies to a bot, watches the
  killcam, kills an `addTestClient` client (which thinks on retail's
  random cmds), shows the scoreboard and chat, downloads a missing iwd
  over UDP or `sv_wwwBaseURL`, plays tdm to its score limit and sd past a
  round, and follows a `map_restart` and a map change, printing the same
  console lines it prints on a retail server
  ([cod2-protocol.md](research/cod2-protocol.md) 16).
  `--bots N [--bots-shoot]` adds bots that join through the stock menus
  and play every stock gametype with vcod's CoD1 bot brain (docs/bots.md,
  "CoD2"): they plant and defuse in `sd`, take, return and capture flags
  in `ctf`, take and hold the radio in `hq`, throw offhand frags, man
  a mounted gun with an enemy in its arc in `dm` and `tdm`, and
  climb the ladders and the mantles their graph proved. `addbot`,
  `removebot` (`kickbot`), `listbots` and the `vc_bot_*` cvars run there
  as on CoD1. They fight from cover with CoD2's leans, retreat, turn on
  a hit from out of sight and throw smoke to cover a plant, a defuse or
  a retreat.
- Demo playback has no `cl_avidemo` (no screenshots). No CoD1 demo has
  yet crossed between vcod and a retail client either way
  ([cod11-demos.md](research/cod11-demos.md) 7); CoD2's have, both ways.
  CoD2's `timedemo` writes no `demos/timedemo_*.csv`, a demo draws no
  `EV_STEP_VIEW` smoothing, and CoD2's front end has no demo list, as
  retail's has none ([cod2-demos.md](research/cod2-demos.md) 7). No
  retail CoD4 demo has been played in vcod nor vcod's in retail
  `iw3mp.exe`: the byte layout is read off the binary only, and the
  `RECORDING` line draws at CoD2's place, not 1.7's
  ([cod4-demos.md](research/cod4-demos.md) 7).
- Prediction carries you on a moving brush model and turns your view with
  its yaw, as retail's does, but never turns your position about it.
  Neither does retail's.
- A mod's UI DLL is ignored; only its menu files count
  ([cod11-front-end.md](research/cod11-front-end.md) sections 16-17).

**Server**

- The server and the client pose a moving body as retail does
  (ground-speed rate, phase, blend times, torso over legs).
- A brush model that turned and turned back keeps a sliver of yaw on retail
  and drifts its riders about 0.02 units a frame. vcod's comes back to zero.
  Yes, I'm calling that a bug in retail.
- `sv_pure` defaults to 0 where retail's default is 1, and a client may
  download only the paks the server lists. Retail serves any file under its
  directories, `..` included, so I'm keeping that one on purpose.
- rcon runs `map`, `devmap`, `map_restart`, `map_rotate`, `status`,
  `clientkick`, `kick`, `banUser`, `banClient`, `dumpuser`, `serverinfo`,
  `systeminfo`, `say`, `set`, `seta`, `cvarlist`, cvar queries, `heartbeat`,
  `killserver` and `quit`, and on 1.5 `tempBanUser`, `tempBanClient`,
  `unbanUser`, `setdrawfriend`, `setfriendlyfire` and `setkillcam`. A
  temporary ban refuses an off-LAN client for `sv_kickBanTime` seconds by
  the guid the address stands in for (retail's comes from the authorize
  server); a LAN client is only kicked, as on retail. `gameCompleteStatus` reaches only
  the masters it resolved itself, as retail's does; `scriptUsage` and
  `stringUsage` print retail's report with vcod's VM's numbers. Bans
  live in retail 1.5's `ban.txt` (`--ban-file` keeps it across runs),
  keyed on the guid an off-LAN address stands in for; retail 1.1 hands
  them to Activision's authorize server instead. On 1.1, like
  retail's, a ban refuses an address off the LAN and leaves the banned
  player connected until they leave on their own; on 1.5 it kicks an
  off-LAN player and does nothing to a LAN one, which has no guid. After
  `killserver` the process sits there deaf, since there is no stdin
  console.
- The heartbeat to `master.cod.pm` has not been tried against the live
  master; tests heartbeat a loopback fake.
- A server speaks one patch, 1.1, 1.5 or UO, as each retail binary does. On 1.5
  everyone, bots too, moves by 1.5's rules (as far as the client's
  prediction models them), and so do the bots' nav graph's walks.
  A 1.5 server runs the match timeout, 1.5's chat (`g_deadChat`,
  `say_squad`) and refuses PunkBuster as the Linux 1.5b server does.
  On 1.5 and UO a prone body's pose, the bend a drop leaves it, the
  heading a spawn lies a held prone down along, a spectator's spawn on
  his held keys, and a frag that lands on his back match the retail
  servers' captures; on all three a lie-down idles crouched through its
  first leg as retail's does
  ([cod15-movement.md](research/cod15-movement.md) 4,
  [cod11-combat.md](research/cod11-combat.md) 12.2).

**Rendering and sound**

- Props are lit per vertex from the map's lights and its light-visibility
  grid at load, as retail does. Dynamic lights add on top with retail's
  falloff instead of competing for a prop's eight light slots. Players,
  other entity models and the viewmodel pick their eight lights from the
  grid every frame, fx lights and the panzerfaust's black rocket light
  among them
  ([cod11-light-grid-and-leaf-lights.md](research/cod11-light-grid-and-leaf-lights.md)).
- The frame holds retail's framebuffer bytes and the gamma ramp doubles
  it once at the end, as retail's hardware ramp did; windowed it draws
  as retail's windowed mode does, with no doubling and lightmaps shifted
  at load ([cod11-gamma.md](research/cod11-gamma.md)). Entity models
  and the viewmodel draw every stage of their material (pickup glow,
  objective pulse, the grenades' environment map) and sort among the
  world's surfaces by material sort, as retail's draw list does. The world
  draws its shaders in retail's sorted order (sort, then first-stage
  images), each shader's stages together, with no depth sorting; effects
  join the same list by sort.
- Impact marks (bullet holes, scorches) clip onto the world's triangles on
  the CPU, take the surface's lightmap, fade over the second half of their
  life and draw at their shader's sort and polygon offset, 1024 pieces at
  most, as the retail cgame and renderer do. A `map_restart` clears them
  with every live effect and sound, empties the objective text and stands
  the player up running, and drops the centre print, the vote lines and
  the hit icons, as `CG_MapRestart` does, and the game and bold message
  windows, as the engine's `n`/`B` case does; the next snapshot then runs the
  respawn reset. `cg_shadows 1` (a cheat, off by
  default as in retail) drops the `markShadow` blob under players 250 to
  1024 units away. Brush models and props take none, as in retail; the
  dlight pass skips them
  ([cod11-events-and-fx.md](research/cod11-events-and-fx.md), section 9).
- `deformVertexes` draws `wave`, `flap`, `bulge`, `move`, `normal`,
  `autosprite` and `autosprite2`, up to three per shader in script order
  as retail runs them; `text` and `syncNormal` (no stock user) parse and
  do nothing. Entity models and the viewmodel take `skins/` script blocks
  and run the same deforms, all but the sprites (no stock MP entity has
  one). A `type@` skin whose template or image is missing draws retail's
  default shader. NV/ATI
  hardware-path stages are dropped, as retail did on cards without them,
  and stages that need three texture units give way to the scripts'
  two-unit ones. `portal` draws no portal view.
  Dynamic lights (muzzle flashes, blasts) light the world through retail's
  dlight pass: the projected dlight image over each opaque surface a light
  reaches, through its `perLight` stages, the copy of its texture or
  `dlightshader`. The ship's deckflag has no file behind it, so a stand-in
  draws ([cod11-shader-scripts.md](research/cod11-shader-scripts.md)).
- Visibility draws a bit more than retail on purpose. Retail assumes nobody
  looks over a cell's walls, and the mp_ship decks disagree.
- Audio follows the engine on paper (falloff, panning, voice pools, stealing,
  ducking) but hasn't been checked by ear against the real game. Wall
  occlusion is a vcod addition.

**Things that look like gaps and aren't**

- No mantling, no swimming. Retail 1.1 MP has neither
  ([cod11-mantle.md](research/cod11-mantle.md)).
- No grenade cooking on 1.1 or 1.5. The fuse runs in full from the
  release, however long you clutched it. United Offensive has one, and vcod
  does it there.
- No doppler. The 1.1 engine never gives a sound a velocity.
- Quick chat (`vsay`) does nothing on a stock install, same as retail: its
  `.voice` tables only ship in mods.
- Asphalt footsteps are silent in retail because the engine asks for
  `asphalt` and the sound table spells it `asphault`. vcod uses the table's
  spelling, so you can hear asphalt. Twenty-three years late, Infinity Ward,
  you're welcome.
