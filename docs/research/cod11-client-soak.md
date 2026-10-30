# The CoD1 client soak (1.1 and United Offensive 1.51)

What a player hits in the CoD1 client over every stock map and gametype:
`tools/cod1_soak.sh` runs vcod-server with six shooting bots and the real
`vcod` binary joined through the stock menus, and `tools/cod1_soak.py`
reads what both left. The CoD4 soak (`cod4-map-sweep.md`, "The soak")
found the client keeping every snapshot and building marks and effect
images on first use; this document asks the same of CoD1.

## The harness

`tools/cod1_soak.sh <outdir> [map...]` (`COD_DIR`, `UO=1`, `LVP_ICD`,
`MINUTES`, `GAMETYPES`; its header says what each does) runs one map at a
time inside a loopback-only `unshare -rn`. vcod-server loads the map with
`--bots 6 --bots-shoot --trace` and an `sv_mapRotation` naming the map
once per gametype its arena lists (`mp/*.arena` in the install's paks),
the first again last, each at a `MINUTES` time limit and one round. A
windowed `vcod` on a nested `kwin_wayland --virtual` (lavapipe, 640 x 480)
joins as axis with the kar98k (every stock map's axis is german) with `VCOD_SOAK` and `VCOD_FRAME_PROFILE` set, so every
frame runs through `main`'s whole CoD1 frame: the net pump, the cmds, the
prediction, the scene, the events, the effects and sounds, the HUD, the
UI and the render.

`VCOD_SOAK=<path>` (`play::cod1_soak`) is the player. Alive, it walks
forward, turns to a new heading every 2 to 6 s and when held up, jumps when
stuck, fires in bursts, aims down the sight, crouches or goes prone and
stands again, switches weapon slots, reloads, taps use (a UO seat), opens
the scoreboard for 1.5 s every 15 to 30 s and the Esc menu for 2 s every
25 to 45 s. Dead, it taps fire. It writes one line per event, stamped with
the frame profile's frame number: the level's map and gametype with the
resident set and wall clock, the first frame with a snapshot, spawns,
deaths, what it did, audio misses and the resident set every 5 s.

`tools/cod1_soak.py <outdir>` joins those with the frame profile and the
two logs into `soak.md`, one row per (map, gametype): the player's counts;
client frames whose CPU side passed 100 ms (every stage from `net` to `ui`
and the time outside every span; the render, `cull` to `submit`, is
lavapipe's and is counted apart) by the stage that took longest and what
the player did that frame; the first in-game frame after the level's
gamestate; warnings and debug lines that name a problem, on each side;
script errors; panics; server seconds whose slowest tick passed 100 ms
(`--trace`'s `tick:` line); the resident set at the level's start and end.

The CoD2 and CoD4 soaks (`play::cod4_sweep`, `play::cod2_soak`) run a
session's `pump` in a test into a headless renderer. CoD1's frame has no
such pump: it is `main.rs`'s `App` itself, menus, HUD and render
included, so this soak drives the real binary instead and shares only
the frame profile with them.

## Findings before the fixes

mp_carentan tdm and dm at 1 min, on c45c7433 with the level prewarm off
(`tmp`, not committed):

1. The snapshots. vcod's CoD1 client parses into `SnapshotRing`, 32
   slots indexed by message number (`vcod_common::net::snapshot`,
   `RING_CAP`), so a live session never holds more than 32. VERIFIED
   (the code). The resident set settles after the first level: 519 MB at
   the join, 967 at the second level, 1065 at the third, 1080 at the
   fourth. Retail's ring is `cl.snapshots[PACKET_BACKUP]` of 32 too.
   INFERRED (the Q3 lineage's `client.h`; not read out of CoDMP.exe). No
   fix.
2. First use. The client read an effect's `.efx` and its sprite images,
   built a mark shader, and loaded and uploaded an xmodel (a player part,
   a dropped weapon, a grenade) in the frame that first played or drew
   it. The frame profile put them in `fxdraw` (32 ms, the first impact),
   `bodies` (124 ms, the first frame that posed the bots: the animtree
   and their parts) and `pose` (109 ms, frame 183: the first world model
   of its kind) on lavapipe.
3. The load. A level's gamestate loads the map, its world GPU state and
   its viewmodels inside one frame (`net`, 250 to 420 ms), behind the
   loading screen the frames before it drew. Retail loads behind its
   loading screen too (`CG_Init`'s `LOADING... %s` steps,
   cgame_mp_x86.dll 0x30020da0). VERIFIED (the strings and the calls in
   the decompilation). Not a stall a player meets in play.

## The fix: the level prewarm

Retail's `CG_RegisterGraphics` (cgame_mp_x86.dll 0x30020da0) registers,
under `LOADING... - server models`, every model configstring from 0x10d
(269) up to the first empty one, through `trap_R_RegisterModel`
(0x30030920 with 7), then every effect configstring from 0x30d (781)
through syscall 0xde, the shell shocks from 0x44d, the impact table
(0x3001a7e0) and `fx/impacts/flesh_hit_noblood.efx`. VERIFIED (the
decompilation). A level's models and effects are read before
its first frame.

vcod now does the same when a gamestate's map has loaded and on an
in-game `vid_restart`'s fresh cgame (`main.rs`, `prewarm_level`):

- `EntityScene::prewarm` loads the player animtree and uploads every
  xmodel in the model configstrings, held both as an entity model and as a
  player part.
- `fx::registry::level_effects` lists every `.efx` the level's events can
  play: the impact table's rows, the flash fallbacks,
  `flesh_hit_noblood`, the effect configstrings, configstring 7's
  weapons' flash, eject and explosion effects, and on UO the wheel dust.
  `FxSystem::preload` reads them and everything they chain to, and hands
  back the shaders their elements draw; the renderer builds the `Decal`
  ones' mark materials (`Renderer::prewarm_marks`) and loads the rest's
  images and the tracer's (`Renderer::prewarm_fx`), the CoD4 soak's
  calls. It hands back the xmodels their `Model` elements throw (shell
  casings, debris) too, which `EntityScene::fx_model` uploads: the first
  1.1 sweep met one in play, a 112 ms `pose` frame on mp_brecourt dm, the
  first brass of its kind.

On mp_carentan the prewarm takes 131 models, 49 effects, 12 mark and 57
sprite shaders, 900 ms on the first level and 350 to 560 ms on a later
one, inside the load frame. After it the same run had no in-play frame
past 100 ms: `bodies` peaked at 3 ms, `pose` at 6, `fxdraw` at 2. The
resident set is higher by the models (1396 MB after the first level)
and stays flat after it: 1396, 1389, 1431, 1445, 1472, 1459 over six
levels.

The app digest gate (a local script, not committed) on the 1.1
and UO demos, the prewarm on against off on one build with the model
handle left out of the `ents` hash (the prewarm uploads in configstring
order, so every handle moves): `view`, `vm`, `hud` and `con` match on
every frame. `ents` differs on the first 226 of 843 frames (1.1) and 304
of 847 (UO) and then matches: with the prewarm a body's parts are
resident at the gamestate, without it they come in over several frames
under the assembly's per-frame load budget. `snd` and `fx` differ on two
1.1 frames (140 and 144), not traced.

Gate: `fx::sim::tests::a_preloaded_level_plays_its_effects_without_reading_a_file`
(the level list's effects, spawned and stepped after the preload over an
empty search path, read nothing).

## The sweep

### 1.1

`COD_DIR` at the 1.1 install, `MINUTES=1`, every stock map (twelve) under
every gametype its arena lists, 70 levels, 65 minutes of play, on the
fixed tree:

- No client frame past 100 ms in play. The one per level is the load
  frame (`net`, 454 to 1776 ms, the map, its GPU state, the viewmodels
  and the prewarm). The prewarm took 102 to 171 models, about 50
  effects, 6 effect models, 14 mark and 59 sprite shaders, 249 to 1299
  ms (median 380) of it.
- The first in-game frame after a gamestate drew 1 to 7 ms after it.
- No server second whose slowest tick passed 100 ms outside a map load.
  The bot nav graph builds in a rotation's load (mp_ship's took 2.8 s)
  and is excluded with it.
- No panic, no script error, no wgpu validation error.
- The resident set: about 515 MB at the join, 1290 to 1550 after the
  first level, then flat: a map's last level ends within 100 MB of its
  first (mp_carentan 1429 to 1505, mp_hurtgen 1498 to 1580, mp_powcamp
  1464 to 1437).
- The player spawned 105 times and died 85 over the 70 levels.
- Problem lines, none a vcod bug:
  - `menu file not found` for nine `ui_mp/ingame*.menu`,
    `ui_mp/in_rec_restart.menu` and two `ui/options_*.menu`: 1.1's
    `ui_mp/ingame.txt` (pak0) lists menus no stock pak holds. VERIFIED
    (the pak listing).
  - `unknown sound alias` `grenade_bounce_<surface>` and `land_default`:
    1.1's `iw_sound.csv` has only `grenade_bounce_default` and no
    `land_default`, and the cgame builds both names off the surface
    (`cod11-sound-system.md` 7c), so retail plays nothing there either.
    VERIFIED (the alias table).
  - mp_depot's `textures/normandy/windows/glass@window` has no image in
    the stock paks; the scripts parse warning `skins/gradient` and the
    absent `.sun` files are known debug lines.
  - The server: `spawn point entity is in solid` on mp_powcamp (a stock
    spawn point; vcod-server warns, retail does not check).

### United Offensive 1.51

`UO=1 JOBS=2 MINUTES=1`, `COD_DIR` at a stock-only UO tree, every stock
UO map (nineteen) under every gametype its arena lists (dm, tdm, sd, re,
bel, hq, ctf, dom, and bas on mp_foy, mp_italy, mp_kursk, mp_ponyri and
mp_rhinevalley), the ten whose BSP places vehicles (`vehicletype` keys:
tanks, jeeps, flak 88s on mp_berlin, mp_cassino, mp_foy, mp_italy,
mp_kharkov, mp_kursk, mp_peaks, mp_ponyri, mp_rhinevalley and
mp_uo_hurtgen) among them: 167 levels, 195 minutes of play, on a2f0aa6d
(the 2 x 2 DXT1 fix, 0c7674b2, in). 105 minutes of wall clock.

- No panic, no wgpu validation error. mp_kharkov, which panicked on its
  sky in the first UO run, loads and plays six gametypes.
- No client frame past 100 ms in play. The one per level is the load
  frame (`net`, 577 to 5897 ms, the slowest mp_rhinevalley's hq), the
  prewarm (126 to 252 models, 253 to 267 effects) 338 to 2661 ms (median
  568) of it.
- The first in-game frame after a gamestate drew 1 to 54 ms after it.
- The resident set: about 525 MB at the join; a map's last level ended
  between 857 MB (mp_uo_harbor) and 2453 MB (mp_foy), and inside a map it
  moves by the level's assets, not by the levels played: mp_cassino 1682
  to 1833 over nine levels, mp_foy 2243 to 2453 over ten.
- Server ticks past 100 ms inside a level: 73 levels, 107 to 622 ms (the
  slowest mp_sicily's ctf), each the gametype's `map_restart` once the
  round starts (`==== RestartGame ====`), which reloads the game in one
  frame, as in the first run.
- The player spawned 228 times and died 209.

Three were vcod's, fixed on this branch and rerun live (mp_uo_stanjel,
mp_uo_carentan and mp_berlin, ctf and dom, all three gone):

1. `maps/mp/_birdbush::birdbush:34` script errors on mp_uo_stanjel,
   four a trigger and up to 141 in 23 s: the stock script adds `(int)
   self.script_noteworthy`, a string, and vcod's `(int)` refused strings.
   Retail's casts read a string's numeric prefix (`cod11-gsc-language.md`,
   "Casting a string"; `probe_cast_string`).
2. `o_ctf_flagicon_*: graft bone '' not in the base skeleton` on four
   maps' ctf: the flag carrier's `TAG_HELMETSIDE` took its configstring at
   the snapshot build, a frame after the snapshot naming it, so the
   client grafted the icon by its nameless root for a frame. The tag now
   takes its index in `ClientEndFrame` (`cod11-gsc-object-model.md` 24).
3. `fx /fx/map_trenches/mortar_mz_nest.efx: not found` on mp_berlin: an
   effect chained with a leading slash, which the engine's file lookup
   skips (`coduo-diff.md` 19, "A leading slash").

The rest, none a vcod defect as far as read:

- `maps/mp/gametypes/tdm::Callback_PlayerKilled:815` on mp_kursk and
  mp_sicily: stock UO `tdm.gsc` reads the unset `level.teamkill_penalty`
  on a death to no player (`cod11-gsc-language.md`, "Runtime errors below
  developer 1"). Retail at `developer 0` runs on past it too.
- `mf_ptrs41.efx` and `mf_ptrs41_turret_v.efx` not found, on every map:
  as in the first run, the stock paks hold them only under `lofx/`.
- `world_hurt_me` and `land_default` and four `grenade_bounce_<surface>`
  aliases unknown: `world_hurt_me` is `SP_trigger_hurt`'s compiled-in
  default, which no UO alias table lists; the rest as in the 1.1 sweep.
- `blend of "weap_mg42_loop" and "weap_mg42_loop": not one sound file`:
  UO's `generic_mp_sound.csv` gives that alias two variants on different
  files, and each half of a blend picks its own; retail's check
  (0x452a20) prints the same warning.
- `textures/notexture` (mp_berlin, mp_arnhem),
  `textures/decals/decal_clamp@grunge` and mp_uo_depot's
  `textures/normandy/windows/glass@window` have no stock image; absent
  `.sun` files (mp_hurtgen's, mp_depot's, mp_powcamp's, named by UO's
  remakes) and the two shader parse lines are debug.
- The server: `spawn point entity # is in solid` on mp_berlin, mp_foy,
  mp_sicily and mp_uo_depot (vcod-server warns, retail does not check).
- `cg_shock_viewKickFadeTime` unknown to the shock parser, as on retail.

The `map_restart` tick is the one cost left inside a level; it is the
server's, not a stall a client meets in play. Not measured on retail.
