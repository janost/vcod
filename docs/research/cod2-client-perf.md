# CoD2 client frame time

How vcod's CoD2 client spends a frame in live play, and what was cut.
Sections 1 to 7 measure vcod's own code, not retail. Sections 8 to 12
compare vcod's shaders and passes with retail's, at the cited addresses.
The tool and the profile format are `cod4-client-perf.md` section 1's.
GPU columns are lavapipe (llvmpipe) rasterizing on 4 CPU threads: they
rank the GPU work and are not a real card's milliseconds.

## 1. The tool

`GAME=cod2 COD2_DIR=<install> tools/cod4_perf.sh <outdir> <map> <secs>`
runs `vcod-server --game cod2` with `BOTS` (12) shooting bots and a
spectating client, as the CoD4 run does (`SESSION=1` for a real GPU). The
client joins free. A CoD2 spectator on vcod-server holds still at the
map's spectator spot on every gametype, so the world triangles and soups
are the same frame to frame and run to run, and the GPU groups compare
across builds.

The session's stages, profiled as CoD4's are:

| stage | what |
|---|---|
| `net` | `Cod2Client::pump` |
| `pose` | entity models and brush models, `Cod2Draw::draw` |
| `bodies` | `Cod2Players::build_instances` |
| `vm` | the view weapon's frame |
| `events` | the events and their sound cues |
| `fx` | their effects |
| `scene` | `Cod2Client::view`'s view effects |
| `hud` | the HUD |

The counts add `cookiedraws`: the shadow cookie pass's draws, casters and
receivers.

## 2. Where a frame goes (640x372, lavapipe, dm, the spectator spot)

| Run | fps | CPU ms (no submit) | cull | bodies | net | GPU cookies | GPU opaque | GPU total | draws | cookie draws | world tris |
|---|---|---|---|---|---|---|---|---|---|---|---|
| mp_toujane, 12 bots, before | 16.9 | 2.77 | 0.55 | 0.64 | 0.32 | 16.0 | 32.2 | 55.3 | 361 | 78 | 73360 |
| mp_toujane, 12 bots, after | 17.1 | 2.44 | 0.25 | 0.55 | 0.28 | 16.7 | 32.0 | 55.1 | 267 | 42 | 73360 |
| mp_carentan, 20 bots, before | 14.3 | 5.04 | 3.24 | 0.53 | 0.25 | 19.5 | 37.5 | 63.8 | 379 | 51 | 144054 |
| mp_carentan, 20 bots, after | 13.2 | 3.89 | 1.62 | 0.61 | 0.35 | 22.6 | 40.5 | 70.7 | 405 | 51 | 144054 |

The machine was shared with other builds; GPU means move 10-20% between
runs of one build (mp_toujane's base ran 15.8 and 16.0 ms of cookies, 37.0
and 32.2 of opaque). The fps follows the GPU; the CPU stages are the
signal.

- The CPU frame is 2.4 to 3.9 ms; a real card draws CoD2 GPU-bound.
- On lavapipe the GPU is the opaque main pass (55-60%) and the shadow
  cookies (25-30%). Of the cookies, the receiver pass (the visible world
  drawn again into the dynamic shadow) is about 9 ms and the casters about
  7 (`sc_debugReceiverCount 0` takes cookies to 6.8 ms, `sc_enable 0` to
  0); `sc_blur 0` changes nothing measurable. The opaque pass drew in
  29.8 ms without cookies, within the noise of with.

## 3. The cuts

1. `cull`: mp_carentan 3.24 -> 1.62 ms, mp_toujane 0.55 -> 0.25. The cell
   walk revisited cells through several portal paths (230 visits over 28
   cells, at the walk's `8 * cells` cap), and each visit tested every soup
   of the cell's tree against the narrowed frustum, marked or not. A marked
   soup now skips its test (`WorldVis::mark_tree`). The marks are an OR,
   so the set is the same (2610 soups; 144054 world triangles and 3249
   soups in the client both runs). The walk alone, from
   mp_global_intermission, 50 rounds: mp_carentan 2.06 -> 0.84 ms,
   mp_toujane 0.17 -> 0.08. CoD1 and CoD4 walk the same code.
2. Cookie draws, 78 -> 42 on mp_toujane. A caster's consecutive surfaces
   whose indices follow on (the u16-rebased models of
   `cod4-client-perf.md` 7.2) draw as one range, and receiving world
   batches whose gathered ranges abut draw as one. Same triangles, same
   order; the GPU time did not move on lavapipe.

Tried and dropped:

- A light-space square per cookie tested before the receiver's per-cookie
  lookup (a bit-exact pre-reject): the cookie group stayed at 15 ms. The
  receiver pass's cost is its geometry, not the loop.
- A per-placement list of the cells a static model's box overlaps, in
  place of `prop_visible`'s scan of every cell: no change; mp_carentan has
  28 cells.

Already in from the CoD4 work: the drawn-pose path (`PlayerBody::pose`
poses without the engine records) and same-skin entity runs (`entdraws`
about 110 for 92-99 instances).

Pixel output: `cod2_capture_frame_draws_lit` (the retail mp_toujane
capture's frame from the spawn and from behind the body, cookies on)
writes byte-identical PPMs before and after under `VCOD_C4_SHOTS`.

Connect-to-first-frame load times, and the load profile that splits them,
are in cod4-client-perf.md section 34 (all four games).

## 4. What to measure on a real card

Pending: run `SESSION=1 GAME=cod2` at the desktop's size and read whether
the receiver pass (a second geometry pass over the visible world) and the
casters still take a quarter of the GPU, and whether `gpu.opaque` is fill
(it scales with the window size) or draws. The next CPU cut is the rest of
the cell walk's node tests on maps with many portal paths.

## 5. The soak

`tools/cod2_soak.sh <outdir> [map...]` (`COD2_DIR`, the lavapipe env;
`tools/cod4_soak.sh` with `GAME=cod2`) runs `play::cod2_soak::cod2_soak`,
one map a process, every stock map without arguments. It is the CoD4 soak
(`cod4-map-sweep.md`, "The soak") on the same harness
(`play::cod4_sweep`: `Game`, `SoakServer`, `Player`, `Tally`).

- Server: per map, vcod-server `--game cod2` loads it with six shooting
  bots and rotates through dm, tdm, sd, ctf and hq (dm again last) at
  `VCOD_SOAK_MINUTES` limits, sd at one round of that length.
- Client: joins as `--connect --game cod2 --team autoassign` does (the
  session answers the team and weapon menus) and plays through `main`'s
  `Mode::Cod2` arm: `App::cod2_level`, `App::cod2_script_menus`,
  `Session::pump`, `scene`, `events`, `audio`, `effects`, `view`,
  `draw_view`, `ViewTail` and the HUD quads, rendered at 10 Hz into a
  320 x 240 lavapipe target.
- Player: walks the bots' nav graph, fires in bursts, throws a frag every
  10 to 20 s, holds the scoreboard (`+scores` through
  `Cod2Client::apply`) and opens the Esc menu (`Cod2Client::escape`, the
  server's `g_scriptMainMenu`).
- A CoD2 level's end is `endMap`'s intermission: every player's `pm_type`
  5.

`soak.md` counts per pair what the CoD4 soak counts (spawns, deaths,
frags, board and Esc opens, nav paths, frames, client frames past 100 ms
by part, wgpu validation errors, audio misses, problem notes, server ticks
past 100 ms, the resident set at the level's first and last sample), plus
the frames past a level's first 5 s that built a mark material or loaded
an fx image (`first-use builds`). The client's parts are `pump`, `hud`
(`scene`: entity models, bodies, the view weapon's frame, the HUD build),
`events` (sounds, effects, view effects), `view weapon` (`draw_view`),
`fx and sound` (`ViewTail`) and `hud upload`.
`VCOD_FRAME_PROFILE=<path>` adds the frame profile by stage.

### 5.1 What a level registers up front

Retail builds a level's assets inside the loading screen, so none loads
in the frame that first draws it:

- `CG_RegisterGraphics` (Mac `cod2mp_mac13_i386` 0x14374a) registers
  configstring models 1 to 255 up to the first empty slot, configstring
  effects 1 to 63 up to the first empty slot,
  `fx/props/american_smoke_grenade.efx`, the impact table
  (`CG_RegisterImpactEffects`) and `fx/impacts/flesh_hit_noblood.efx`.
  INFERRED (control flow of the decompile).
- `CG_RegisterWeapon` (Mac 0x1d85cc) registers each CS 7 weapon's world
  and projectile models and its view flash, world flash, shell eject,
  last-shot eject and projectile explosion effects. INFERRED (control flow
  of the decompile).

vcod does the same once a gamestate:

- `Cod2Draw::preload` loads and uploads the models (the configstring names
  carry `xmodel/`, which `model_at` strips).
- `Cod2Fx::level_effects` lists the effects; `FxSystem::preload` parses
  them and every effect they chain to.
- The renderer builds their mark and sprite materials (`prewarm_marks`,
  `prewarm_fx`).

`cod2_live_level_effects_and_models_build_nothing_in_play` holds it on a
live client: by the spawn every configstring model is loaded, and marks
then land with no mark material registered and no fx image loaded.

The CoD4 soak's other finding, a session that kept every parsed snapshot,
does not apply: `vcod_cod2::net::Parser` keeps a `PACKET_BACKUP` (32)
ring, and `NetView::push` drops a kept snapshot once `CL_GetSnapshot`
could no longer return it.

### 5.2 Results (15 maps x dm, tdm, sd, ctf, hq; 2 min limits, lavapipe)

All 75 pairs load, end, rotate and spawn the client. None panics or logs a
script error, a wgpu validation error, an audio miss or a server tick past
100 ms. One process runs one map's five levels, server included.

| gametype | frames | client frames > 100 ms | worst ms | first-use builds | RSS over the level (MB, mean) |
|---|---|---|---|---|---|
| dm (first) | 100723 | 34 | 853 | 0 | +1068 (the first load) |
| tdm | 101611 | 0 | 74 | 0 | +34 |
| sd | 63105 | 14 | 178 | 2 (worst 14 ms) | +49 |
| ctf | 102532 | 0 | 89 | 0 | +26 |
| hq | 103660 | 0 | 89 | 0 | -56 |

Without the preloads, the first-use column read 10 to 18 frames on every
map's first level (worst 9 to 47 ms) and 1 or 2 on later ones, and the sd
bomb and ctf flag models loaded in the frame they first showed.

The sweep ran before the preload stripped `xmodel/` off configstring names
(`model_at`): every dm logged 100 to 150 `live entity model 'xmodel/...':
xmodel/xmodel/... not found` warnings, the preload missed every
configstring model, and the entity path (keyed without `xmodel/`) loaded
them on first use. A rerun of mp_toujane and mp_harbor with the fix logs
one client warning a dm, the counts otherwise as above, and two lines:

- `shock/hold_breath.shock` names `cg_shock_viewKickFadeTime`, which
  `play::shock` does not know (every dm). Retail warns the same and
  ignores it (cod2-hud.md 16).
- mp_toujane ctf: `flag_carry` grafted a bone named `''` as a new root.
  The server indexed the flag's tag after the frame's configstrings went
  out; fixed (cod2-player-anims.md 1).

What is left:

- Every slow client frame is a level's start: the frame that brought the
  gamestate and the few after it (`pump`, `hud`, `events` and `view
  weapon` while the level's models, materials and effects build; on
  lavapipe `submit` adds 50 to 200 ms of texture upload). Retail builds
  the same assets behind its loading screen. The soak counts a frame that
  brings a gamestate as a level start. sd's slow frames were its second
  gamestate (section 6 has their cause and the cut).
- The resident set grew 26 to 49 MB a level of the same map after the
  first and dropped on some. Section 5.3 splits it: the server holds
  still; the client's player rigs copied every body's meshes once per
  weapon, now shared.
- Every dm logs 13 client notes: the CoD2 front end asks for menu files
  the stock iwds do not hold (`ui_mp/ingame.menu`,
  `ui_mp/ingame_options.menu` and nine more) and the `shadowoverlay`
  technique set. The eleven menus are absent from 1.3 and retail skips
  them silently (cod2-fonts-menus.md 2); vcod logs them at trace.
- A frame at 60 to 70 ms in steady play is lavapipe's `submit`, not the
  CPU frame.

### 5.3 The resident set level by level

The soak prints `soak rss: <map> level <n> after <gametype>: <rss> MB,
malloc in use <heap> MB` at each map load (`heap_mb`: glibc's
`mallinfo2`, live blocks plus mmapped ones). Runs on mp_toujane, one
minute levels, the five gametypes round two and a half times (12 levels).

The server alone (`vcod-server --game cod2`, six shooting bots, its own
process, `/proc/<pid>/status` VmRSS every 2 s over 24 levels): 86 MB on
the first level, 96 to 125 MB over the next ten, then 112 to 115 MB on
every level from the 15th to the 24th. It does not grow.

The soak's process, client and server (MB at each load), before the rig
change:

| level | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| RSS before | 2041 | 2098 | 2120 | 2130 | 2155 | 2160 | 2165 | 2169 | 2174 | 2177 | 2181 |
| malloc before | 900 | 911 | 940 | 922 | 939 | 944 | 959 | 977 | 992 | 991 | 991 |
| RSS after | 2021 | 2069 | 2087 | 2097 | 2103 | 2106 | 2117 | 2104 | 2106 | 2101 | 2103 |
| malloc after | 865 | 912 | 923 | 905 | 911 | 933 | 910 | 919 | 939 | 938 | 942 |

About 1.1 GB of the resident set is outside malloc (lavapipe's device
memory, the mapped binaries) and holds still over the run. A jemalloc heap
profile at each load (`LD_PRELOAD` with `prof:true`, `prof.dump` from the
soak, `jeprof --base`) put the live heap's growth from level 2 to 12 at
108 MB: 56 MB in the client's player rigs (`PlayerRig::assemble`,
`vcod_cod2::model::bake_surface`), 31.5 MB in the sound bank
(`SoundBank::get`), 0.4 MB on the server's side. Under jemalloc the
resident set wandered 2250 to 2450 MB with no trend.

- The rigs: `Cod2Players` keys a rig by its character with the held weapon
  as an attachment, so each body, head and weapon seen builds one, and
  each loaded and baked its own body and head meshes. A new level brings
  new pairs (other teams' weapons, the sd and ctf carriers) until every
  pair a map's bots use has been seen; level 12 still added some.
  `PlayerRig::assemble_with` takes the parts from a `RigParts` map shared
  across a session's rigs (keyed by model and whether `ignoreCollision`
  dropped its hit boxes), so a new pair costs a skeleton and the weapon.
  The GPU side was already one upload per part (`Cod2Players::handles`).
  `rigs_of_one_body_share_its_meshes` holds it. The CoD4 client
  (`play::cod4_players`) shares parts the same way; the CoD2 and CoD4
  servers' hit rigs still call `assemble`.
- The sound bank keeps every file it decoded (`audio::bank`), keyed by
  file, and fills as later levels play sounds the earlier ones did not. It
  is bounded by the files the install ships and is not cleared on a map
  change: a cleared bank decodes in the frame that first plays a sound.

After the rig change the resident set holds at 2101 to 2117 MB from level
6 to 12, where it had still risen 26 MB; malloc's live bytes still creep
about 3 MB a level (the bank) under a noise of 20 MB.

## 6. The sd level's first frame

Section 5.2's slow sd frame is not the `map_restart(true)`. Timed per call
(`tools/cod2_soak.sh` with `VCOD_SWEEP_GAMETYPES=tdm,sd`, one minute
levels, mp_toujane, mp_harbor and mp_carentan, lavapipe):

- The slow frame brings sd's gamestate: the rotation from the last
  gametype to sd on the same map, a full `SV_SpawnServer` (`map` of the
  loaded map; the map stays loaded, and section 5.1's preload runs again
  on the new configstrings). It took 127 to 153 ms, all in `hud`
  (`Cod2Client::scene`'s `Cod2Draw::preload`).
- Of that, 109 to 124 ms is one model, `mp_tntbomb_obj` (sd.gsc's
  `precacheModel`): its parse is 0.3 ms, its upload one new entity
  pipeline pair. It is the level's first model with a `SrcAlpha One`
  stage, and lavapipe compiles a pipeline pair on the calling thread in
  about 110 ms (`EntityMaterials::pipeline`, `[World, View]`). The first
  level pays the same for its first opaque, `SrcAlpha OneMinusSrcAlpha`
  and `Zero SrcColor` models (100 to 160 ms each), hidden in its first
  frame's second.
- The `B` / `n` of each sd round (`fast_restart`) costs nothing: the frame
  that brings one ran 0.2 to 0.4 ms in `pump` on all three maps.
  `take_restart` clears the effects and nothing else; no configstring,
  model, material or GPU buffer is rebuilt.

A census of every stock xmodel's materials (732, over 1534 models) draws
with five pipeline keys (blend, depth write, depth test), all `LessEqual`:

| key | stages |
|---|---|
| opaque | 642 |
| `SrcAlpha OneMinusSrcAlpha` | 57 |
| `SrcAlpha One` | 27 |
| `Zero SrcColor` | 5 |
| `One One` | 1 |

`cod2_model_pipelines_cover_every_stock_model` holds it. The preload
builds the five first (`COD2_MODEL_PIPELINES`,
`Renderer::prewarm_entity_pipelines`), each pair on its own thread
(`par_build`), so a later level's new model builds no pipeline. Nothing
drawn changes: the pipelines are the ones the stages built before.
`cod2_live_same_map_gamestate_builds_no_model_pipeline` joins vcod-server
on mp_toujane dm, rotates to sd on the same map after the spawn and counts
the pipelines built past the spawn: one before the cut (`SrcAlpha One`),
none after, its sd frames then at most 2.6 ms.

The same soak after the cut (client CPU ms, the soak's parts):

| map | sd worst before | sd worst after | sd frames > 100 ms before / after |
|---|---|---|---|
| mp_toujane | 129 | 16 | 1 / 0 |
| mp_harbor | 127 | 57 | 1 / 0 |
| mp_carentan | 153 | 59 | 1 / 0 |

What was left over 50 ms in sd was the `view weapon` part
(`Cod2Client::draw_view`, `Renderer::set_viewmodel`), 50 to 75 ms on
lavapipe, in tdm too, at spawns and in play, not on the gamestate or a
`B` / `n`. Section 7 has the cause and the cut. The first level's first
frame (800 to 1100 ms) is section 5.2's level start, unchanged.

The CoD1, UO and CoD4 paths do not run this: `Cod2Draw::preload` is
called only from `Cod2Client::scene`, and `EntityMaterials::prewarm` only
from there.

## 7. The view weapon's first draw

Timed per call (mp_harbor, `VCOD_SWEEP_GAMETYPES=tdm,sd`, one minute
levels, `VCOD_SOAK_CLIENT_LOG_MS=30`):

- Each slow frame was a weapon's first showing: `Renderer::set_viewmodel`
  uploading a view model part not in its cache (`vm_upload`: the mesh,
  its skin images through `texture_settings::prepare`, their mip chains).
  The frag (`viewmodel_germangrenade2`) took 53 ms, the mp40 48 ms, the
  Russian frag 22 ms, a rifle with new hands 8 ms. The skins
  (`resolve_viewmodel_skins`) cost under 0.6 ms, the rig's parse and clips
  (`Cod2View::rig`) under 5 ms. Nothing uploaded these models ahead:
  CoD1's `prewarm_viewmodels` walks CoD1's configstring 8 rigs, which a
  CoD2 session never runs.
- In tdm the first frag or pistol came during play, in sd at the round's
  spawn; both read as `view weapon` frames of 48 to 53 ms.

Retail builds a weapon's view models with the level:

- `CG_RegisterItems` (Mac 0x1d9652) walks configstring 8's item bits and
  calls `CG_RegisterItemVisuals` (0x1d9580) for each set one, which for a
  weapon item calls `CG_RegisterWeapon` (0x1d85cc). INFERRED (control flow
  of the decompile).
- `CG_RegisterWeapon` registers the weapon file's `handModel` and
  `gunModel` (`CL_RegisterModel` of `xmodel/` + each name) and builds the
  view anim tree, once: a second call returns on the registered flag
  (`weaponInfo` +0xa8). `CG_UpdateViewWeaponAnim` (0x1dd3fe) calls it every
  frame for the drawn weapon, so a weapon no item registered builds on
  first draw there. INFERRED (control flow of the decompile).
- The hands a character script's `setViewmodel` names (every stock one
  `xmodel/viewmodel_hands_*`, six of them) are configstring models, which
  `CG_RegisterGraphics` registers (section 5.1). VERIFIED (the stock
  `character/*.gsc` scripts).

vcod does the same at each gamestate: `Cod2Client::scene` hands
`cod2_viewmodel::level_view_models` (each configstring 7 weapon's
`gunModel` and `handModel`, and every `xmodel/viewmodel_*` configstring
model) to `Renderer::preload_viewmodel`, which fills the upload cache
`set_viewmodel` reads. Draw order and the drawn models do not change: the
skins still resolve in `set_viewmodel`, in the same order. On mp_toujane
dm the preload is 21 models in 20 ms, inside the level's first frame
(section 5.2's level start).

`cod2_live_view_weapons_build_nothing_in_play` joins vcod-server on
mp_toujane, spawns, switches to the pistol, throws a frag and switches
back, and counts the parts `set_viewmodel` uploaded itself
(`Renderer::viewmodel_first_uses`): 6 before the preload, 0 after, the
worst `view weapon` part 0.2 ms.

## 8. The lit and cookie shaders against retail's compiled ones (P15)

As `cod4-client-perf.md` 21, 26, 28 and 29 did for CoD4's opaque pass:
count what vcod's CoD2 world, static model and shadow cookie shaders fetch
and compute against retail's compiled shaders, then cut where vcod does
more.

### Retail's compiled shaders

The 1.3 iwds ship HLSL source only (`materials/shaders/*.hlsl` in
`iw_07.iwd`; no `shader_bin/` and no compiled bytecode in any iwd), and
`gfx_d3d_mp_x86_s.dll` carries D3DX9's compiler (`Microsoft (R) D3DX9
Shader Assembler 5.04.00.2904`), so the PC client compiles each
technique's shaders at load. VERIFIED (`unzip -l` of every iwd, `strings`
of the DLL).

The Mac 1.3 build embeds its translation of each compiled shader as an ARB
program, followed by the name of every HLSL file that uses it
(cod2-materials.md 0). `tools/re/cod2_macarb.py` writes them out and
counts them; its `--count` leaves out the declarations and what the
translation adds (the fixed-function fog stand-in at a fragment program's
end, `MOV oFog.x, 1`, and the `.vsa` programs' `Null` clears). Per pixel,
at the Mac binary's file offsets of the names. VERIFIED (program text):

| technique (techset) | name at | instr | fetch | ALU |
|---|---|---|---|---|
| `lmap` (`phong_replace`) | 0x2a560c | 28 | 8 | 20 |
| `lmap_s` (`phong_replace_spec`) | 0x2ab35c | 40 | 10 | 30 |
| `lmap_dtl` (`phong_replace_detail`) | 0x2a9b78 | 32 | 9 | 23 |
| `lmap_s_dtl` (`phong_replace_spec_detail`, 59% of world triangles, section 8 of cod2-materials.md) | 0x2abad4 | 44 | 11 | 33 |
| `lprobe_smc_s` (cached static model) | 0x2bd628 | 39 | 5 | 34 |
| `lprobe_smc_s_dtl` | 0x2bdde0 | 43 | 6 | 37 |
| `shadowcookie_caster` | 0x2c5af0 | 3 | 0 | 3 |
| `shadowcookie_receiver` | 0x2c5dc0 | 6 | 1 | 5 |
| `shadowcookie_blur` | 0x2c59d0 | 9 | 4 | 5 |

- `lmap.pse` reads eight: the normal map, the sun visibility, the dynamic
  shadow (`TXP`), the weights at the normal, the three coefficient images
  and the colour map. The specular map and the specularity table come only
  with `_s`, the detail map only with `_dtl`. VERIFIED (program text).
- The fog is per vertex: `lmap_s_efog.vsn` ends `MAD oFog, r0.w, c11.x,
  c11.y` (name at 0x2829a8) and the D3D pixel shader has none, as
  `lib/fog.hlsl`'s `Fog_VertexCalc` writes the factor to the `FOG` output
  and the fixed-function fog blends it (`FOG_SEMANTIC` is `FOG` off the
  Xbox 360, `lib/globals.hlsl`). VERIFIED (program text, HLSL). Section 9
  corrects which program that `MAD` belongs to.
- A receiver pixel costs one fetch and five ALU per cookie, and retail
  draws a cookie's receivers once for that cookie: the receiving surfaces
  whose bounds pass the cookie's five planes (section 9.8 of
  cod2-materials.md, 0x10027980). The vertex program is the transform and
  `shadowLookupMatrix` (`shadowcookie_receiver.vsn`, 0x29b384, 7
  instructions). VERIFIED (program text; the cull from 9.8).

### vcod's, before and after

`renderer::shader_ops` (`SHADER_OPS=1 cargo test -p vcod --bins
cod2_shader_ops -- --nocapture`), the counter of `cod4-client-perf.md` 26,
exp fog, the sun lighting. "Runs" is the material's path with every switch
known; "compiled" is what its pipeline's fragment stage holds with the
frame's switches unknown. VERIFIED (counter):

| path | retail fetch / ALU | vcod runs before: fetch / ALU / slots | after | compiled before: fetch / slots / branches | after (split) |
|---|---|---|---|---|---|
| `lmap` | 8 / 20 | 10 / 32 / 40 | 8 / 30 / 36 | 61 / 345 / 83 (`fs_main`) | 22 / 56 / 16 |
| `lmap_s` | 10 / 30 | 10 / 36 / 45 | same | same | 31 / 71 / 23 |
| `lmap_dtl` | 9 / 23 | 11 / 35 / 43 | 9 / 33 / 39 | same | 24 / 61 / 17 |
| `lmap_s_dtl` | 11 / 33 | 11 / 39 / 48 | same | same | 33 / 76 / 24 |
| `lprobe_smc_s` | 5 / 34 | 6 / 45 / 54 | same | 29 / 99 / 37 (`fs_smodel`) | 27 / 80 / 23 |
| `lprobe_smc_s_dtl` | 6 / 37 | 7 / 48 / 57 | same | same | 29 / 85 / 24 |

The compiled counts take both arms of `sample_mipped` per map, of which a
draw takes one. The ALU over retail's is the per-pixel radial fog
(retail's is per vertex), identityLight and the alpha test switch. The
static model's sixth fetch is the cache's two z layers (`fs_smodel`),
where retail reads one volume texture. VERIFIED (counter, program text).

### What changed

1. CoD2's `lmap*` and `lprobe_smc*` draws take split pipelines
   (`LitPath`, section 29 of `cod4-client-perf.md`): each material's
   switches and alpha test as pipeline constants, so the fragment stage
   holds its own arm and not every CoD4 path (`fs_main` compiled 345 slots
   and 83 branches for every CoD2 surface). `VCOD_LIT_SPLIT=0` keeps the
   one shader. A frame sets 95 pipelines on mp_toujane where it set 65 to
   76, 137 on mp_carentan where 81 to 93 (the profile's `pipelines`).
   VERIFIED (counter, profiles).
2. `sun_specular` reads the specular map and `$specularity` only under
   `M_SPECULAR`, so `lmap` and `lmap_dtl` fetch eight and nine, as retail.
   It had read both and thrown them away. VERIFIED (counter).
3. The cookie receivers. vcod draws every visible receiving surface once,
   and its pixel loops over every cookie (matrix, square test, fetch);
   retail draws each cookie's receivers apart, culled to that cookie. Each
   receiver draw now carries, as its first instance, the cookies whose
   lookup square the box of its batch's visible soups can reach
   (`Cookies::receiver_masks`, 1/1024 of margin), built once a gather;
   `fs_receive` walks that mask in cookie order. A cookie off the mask
   would fail the square test at every pixel of the draw, so the dynamic
   shadow is the same. Abutting ranges still draw as one under their masks
   together: mp_toujane 38 to 40 cookie draws, mp_carentan 53 to 56, as
   before. `receiver_mask_keeps_every_cookie_a_point_in_the_box_reaches`
   holds the mask against 16000 points. VERIFIED (the test, profiles).
   Section 12 later moved the masks to each soup.

Not changed here:

- The per-vertex fog: done in section 9.
- The cookie casters and the blur match retail's work: one draw per
  caster surface run per cookie (`shadowcookie_caster` writes the depth,
  no fetch) and four bilinear taps a texel for the blur. VERIFIED
  (program text, `cod2_cookie.wgsl`).
- `cull` (the CPU stage over 0.5 ms, mp_carentan 1.5 ms): the cell walk
  visits 227 cells of 28 from the spectator's spot (`cod2_cull_profile`,
  ignored: walk 1.95 ms in the test build, the static model test
  0.004 ms). P13's held cull (`cod4-client-perf.md` 31) keeps the last
  cull while the view holds still, as a parked spectator does: 0.03 ms. A
  moving view still pays the walk (section 9). VERIFIED (profiles, the
  test's log).

### Frames and timings

`cod2_capture_frame_draws_lit` under `VCOD_C4_SHOTS` (spawn and behind the
body, cookies on): byte for byte the same before (base 043551a3) and
after. VERIFIED (md5s).

`GAME=cod2 tools/cod4_perf.sh`, 640 x 372, lavapipe, 12 bots, dm, 25 s;
base, after, and after with `VCOD_LIT_SPLIT=0`, one after another, twice
(ms, means of the two rounds). VERIFIED (profiles):

| map | run | fps | encode | gpu cookies | gpu opaque | gpu total |
|---|---|---|---|---|---|---|
| mp_toujane | base | 19.4 | 0.23 | 15.0 | 27.2 | 48.9 |
| mp_toujane | after | 20.6 | 0.31 | 14.0 | 25.4 | 46.0 |
| mp_toujane | after, unsplit | 20.2 | 0.27 | 14.3 | 26.2 | 46.9 |
| mp_carentan | base | 13.5 | 0.20 | 22.7 | 40.1 | 70.0 |
| mp_carentan | after | 13.2 | 0.37 | 22.7 | 41.3 | 71.7 |
| mp_carentan | after, unsplit | 14.2 | 0.35 | 21.2 | 39.0 | 66.7 |

Those `after` runs computed the masks every frame (the 0.15 ms in
`encode`). Off the gather's boxes: mp_toujane 22.3 fps, encode 0.21,
cookies 13.2, opaque 23.6, total 42.2; mp_carentan 14.9 fps, encode 0.21,
cookies 20.9, opaque 37.9, total 65.0. Two runs of one build spread 10-20%
here (shared machine), so lavapipe shows no change outside it, as for
CoD4: its GPU time is the fill and the receivers' geometry, not register
pressure or the loop. The CPU stages other than `cull` stay under 0.4 ms.
INFERRED from the spread.

Pending, on a real card: `SESSION=1 GAME=cod2 COD2_DIR=<install>
PORT=28995 tools/cod4_perf.sh <out> mp_carentan 30`, then the same with
`VCOD_LIT_SPLIT=0` in front, reading `gpu.cookies`, `gpu.opaque` and fps.

## 9. The fog per vertex, the cell walk as retail's (P19)

Section 8's leftovers: the lit pixel's radial fog and identityLight, the
model and effect shaders' fog, and the `cull` stage's cell walk.

### Retail's fog

Every fogged CoD2 technique computes the fog per vertex. `lib/fog.hlsl`'s
`Fog_VertexCalc` writes `Fog_VertexFactor` (`lib/fogcalc.hlsl`: the
radial view distance, `useRadialFog` true; `exp(d * fogConsts.z)` under
`FOG_EXP`, `d * fogConsts.x + fogConsts.y` under `FOG_LINEAR`) into the
`FOG` output off the Xbox 360. The pixel shader holds no fog; the
fixed-function fog blends the interpolated factor. `lightmap.hlsl`,
`lightprobe.hlsl`, `lightprobe_sm.hlsl`, `vertex_color_simple.hlsl`,
`water.hlsl` and the other fogged libraries all call it. VERIFIED (HLSL,
`iw_07.iwd`).

The Mac build's programs do the same. `lmap_a_efog.vsn` (name at 0x2806a0)
and `vertcol_simple_efog.vsn` (0x29c87c) end `EX2 oFog, r0.w` on the
distance times `c11.z` and log2 e; `lmap_lfog.vsn` (0x282958) ends
`MAD oFog, r0.w, c11.x, c11.y`; each fragment program ends by clamping
`fragment.fogcoord` to 0..1 and `LRP`ing toward `state.fog.color`.
VERIFIED (program text). The `MAD` section 8 quotes under
`lmap_s_efog.vsn`'s name is the linear program's text; the exp ones are
the `EX2` texts. INFERRED (the extractor keys a name to the text before
it).

No CoD2 shader declares identityLight (cod2-materials.md 9.3), and vcod
draws CoD2 at identityLight 1 (`gamma::overbright_bits`); CoD4 likewise.
VERIFIED (HLSL; code).

### What changed

1. The world's `vs_main` (`cod2_world.wgsl`) writes `VsOut::fog`: the fog
   colour times one less the keep, and identityLight times the keep
   (`vertex_fog`). `finish` is one multiply-add, `colour * fog.w +
   fog.rgb`, where the pixel used to take the length, the exp,
   identityLight's multiply and the lerp. A `nofog` material keeps
   `(0, identityLight)`. Static models (`fs_smodel`), CoD4 sets and the
   water through this file take the same; `cod2_water.wgsl` keeps its keep
   per vertex.
2. Entities (`dynamic_model.wgsl`), effect quads (`fx.wgsl`) and the stage
   path's decals and props (`shader.wgsl`'s `vs_main` and `vs_stage`)
   write the fog amount per vertex on CoD2 (`fog_range.z`, the radial
   flag, set only for CoD2) and read it in the pixel. CoD1 and CoD4 still
   fog per pixel there. CoD1's dlight pass is unchanged.
3. Entity `lprobe*` stages (`cod2_model.wgsl`'s `cod2_probe_sided`) read
   the detail map only under `_dtl` and the specular map and
   `$specularity` only under `_s`, as `lprobe.pse` fetches neither. They
   had read both and thrown them away.
4. The alpha test: section 8's split pipelines already carry it as a
   pipeline constant (`LIT_TEST`), so a lit pixel runs no switch. The
   unsplit pipeline (`VCOD_LIT_SPLIT=0`) and the entity stages
   (`stage.flags`, uniform per draw) still branch; retail tests alpha in
   the fixed-function state, outside the shader. Not changed.
5. CoD2 walks its cells as retail does (`MapWorld::portals_only`, below).

The counter (`SHADER_OPS=1 cargo test -p vcod --bins cod2_shader_ops --
--nocapture`, section 8's), before and after, beside the Mac programs'
`tools/re/cod2_macarb.py --count`. "Runs" is what a path executes,
"compiled" the split pipeline's fragment stage with the frame's switches
unknown. VERIFIED (counter, program text):

| path | retail fetch / ALU | runs before: fetch / ALU / slots | runs after | compiled before: fetch / slots / branches | compiled after |
|---|---|---|---|---|---|
| `lmap` | 8 / 20 | 8 / 30 / 36 | 8 / 23 / 25 | 22 / 56 / 16 | 22 / 37 / 14 |
| `lmap_s` | 10 / 30 | 10 / 36 / 45 | 10 / 29 / 34 | 31 / 71 / 23 | 31 / 52 / 21 |
| `lmap_dtl` | 9 / 23 | 9 / 33 / 39 | 9 / 26 / 28 | 24 / 61 / 17 | 24 / 42 / 15 |
| `lmap_s_dtl` | 11 / 33 | 11 / 39 / 48 | 11 / 32 / 37 | 33 / 76 / 24 | 33 / 57 / 22 |
| `lprobe_smc_s` | 5 / 34 | 6 / 45 / 54 | 6 / 38 / 43 | 27 / 80 / 23 | 27 / 61 / 21 |
| `lprobe_smc_s_dtl` | 6 / 37 | 7 / 48 / 57 | 7 / 41 / 46 | 29 / 85 / 24 | 29 / 66 / 22 |
| world `vs_main` (CoD2) | 0 / 42 (`lmap_s_dtl.vsn`) | 0 / 13 / 17 | 0 / 23 / 30 | | |
| entity `lprobe` (`fs_main`) | 5 / 25 (`lprobe.pse`, 0x2ad690) | 6 / 65 / 73 | 3 / 51 / 54 | | |
| entity `lprobe_s` | 7 / 35 (`lprobe_s.pse`, 0x2b5c44) | 6 / 70 / 79 | 5 / 58 / 64 | | |
| entity `lprobe_s_dtl` | 8 / 38 | 6 / 72 / 81 | 6 / 61 / 67 | | |
| entity `vs_main` | 0 / 117 (`lprobe_s.vsn`, skinned; section 10 corrects this) | 0 / 68 / 72 | 0 / 79 / 86 | | |
| effect `vertcol_simple` (`fx.wgsl`) | 1 / 5 (0x2c7168) | 1 / 15 / 19 | 1 / 4 / 5 | | |
| effect `vs_main` | 0 / 29 | 0 / 1 / 1 | 0 / 12 / 15 | | |

Two costs were left over retail's, both cut in section 10. INFERRED
(counter breakdown, `SHADER_OPS=2`):

- A lit world pixel read the dynamic shadow as `dyn_shadow`: a size query,
  the clamp and `1 - x`, 5 ALU, where retail's `TXP` takes the screen
  coordinates the vertex wrote.
- An entity's probe light computed the eight corner weights (about 25
  ALU) where retail reads them from its two weight cubes (two
  `TEX ... CUBE` in `lprobe.pse`).

The CoD4 rows of `cod4_world_shader_ops` lose a slot each (the lerp became
the multiply-add). Its entity rows now count CoD4's forward-distance fog,
as the renderer sets `fog_range.z` 0 on CoD4 (they counted CoD2's radial
before). VERIFIED (counter).

Fogged pixels move by the difference between the keep interpolated from
the vertices and the keep at the pixel, most on long triangles in dense
fog. `cod2_capture_frame_draws_lit` (mp_toujane, 320 x 240): from the
spawn 33450 pixels moved, 29593 of them by one level, none by more than 4;
from behind the body 21269, none by more than 2. These views' frames match
between the two cell walks (below), so the fog is the whole change.
VERIFIED (the frames against section 8's). Where a triangle crosses a
linear fog's ends vcod clamps each vertex's factor, the D3D9 `FOG`
output's range. INFERRED (no shader text carries the clamp). The Mac
programs clamp after the interpolation instead. VERIFIED (program text).

### The cell walk

Retail CoD2 walks the portals through a nearest-first queue, as CoD4 1.7
does (cod4-world.md 9.1). VERIFIED (decompile; the constants read out of
the Mac binary).

- `R_VisitPortalsForCell` (Mac 0xf042e) chops each portal of the cell by
  the entering portal's plane and the frustum, appends the chopped points
  to that portal's gather, and pushes a portal not yet queued by its
  furthest point along the view.
- `R_VisitPortals` (0xf14c0) pops the nearest, builds the convex hull of
  its gather (`Com_ConvexHull`, at most 0x40 points) and visits its cell.
- A portal whose plane the eye is past is skipped. One within 0.125 of the
  eye whose winding holds the eye is entered at once under the same
  frustum (rodata 0x326438, -0.125; `WindingContainsCoplanarPoint`). The
  queue holds 0x100.

Before the visit `R_VisitPortals` projects the hull through the
view-projection. VERIFIED (decompile; constants 0x3261bc 0.25, 0x3261c8
0.125).

- A point under 0.125 of clip w queues the cell's portals.
- Otherwise the NDC bounding rectangle times 0.25, and past it the
  shoelace area times 0.125, is held against `r_portalMinClipArea` (0.02,
  registered as 0x3ca3d70a). A smaller hull visits its cell with
  `DpvsClipChildren` 0.
- Such a visit queues no portal: `R_GetFurtherCellList_r` (0xef68a) lists
  the cells beyond the cell's portals depth first, each once (a stack
  array of 384), every portal chopped by the same entering plane and
  frustum, and their surfaces are added under that frustum.

`r_portalWalkLimit` defaults to 0, no limit. VERIFIED (dvar registration).

vcod's CoD2 used to run the CoD1 recursive walk: a portal opened once per
path to it, then CoD1's over-marking passes (cells under the eye's height,
cells next to a visited one). From mp_carentan's intermission view that is
237 visits of 28 cells, past its `8 * cells` cap. The revisits are not
retail's, so the set could not be kept. CoD2 now takes the queued walk
(`MapWorld::portals_only`) with the small-portal rule
(`MapWorld::portal_min_clip_area`, `vis::clips_children`,
`WorldVis::further_cells`), which CoD1 and CoD4 leave off (0). Without the
rule, the queue reopened portals on mp_carentan's intermission view 2450
times (6.5 ms). vcod's hull may wind either way, so it takes the area's
size; retail's side planes need its hull wound one way on screen, which
makes its sum positive. INFERRED (`R_GetSidePlaneNormals`, 0xeea9c,
crosses consecutive eye vectors). 1.7 has the same rule at the same
default, plus a depth floor CoD2 lacks (cod4-world.md 9.9). VERIFIED
(iw3mp.exe disassembly).

`cell_walks_side_by_side` (`vcod-cod2`, ignored) walks both from every
spawn round the compass (eye 60 up, eight yaws) and from each map's
`mp_global_intermission` view, release build, on a loaded machine (times
move 20-50% run to run). Means a view. VERIFIED (the test's log):

| map | cells | recursive: soups / visits / ms | queued: soups / visits / ms | only recursive / only queued | intermission visits, ms: recursive, queued |
|---|---|---|---|---|---|
| mp_breakout | 10 | 1723 / 18.1 / 0.416 | 1647 / 17.0 / 0.340 | 104.7 / 28.4 | 80, 0.937; 74, 0.726 |
| mp_brecourt | 10 | 362 / 6.6 / 0.044 | 277 / 4.8 / 0.037 | 85.0 / 0.1 | 24, 0.112; 30, 0.081 |
| mp_burgundy | 18 | 600 / 27.8 / 0.161 | 486 / 16.5 / 0.089 | 127.0 / 13.2 | 50, 0.136; 57, 0.119 |
| mp_carentan | 28 | 910 / 28.2 / 0.260 | 449 / 15.6 / 0.153 | 476.1 / 15.2 | 237, 1.854; 47, 0.193 |
| mp_dawnville | 27 | 853 / 21.5 / 0.199 | 443 / 13.3 / 0.141 | 436.2 / 26.7 | 88, 0.559; 98, 0.450 |
| mp_decoy | 2 | 227 / 1.5 / 0.013 | 220 / 1.4 / 0.011 | 6.5 / 0.0 | 3, 0.015; 3, 0.015 |
| mp_downtown | 4 | 1374 / 2.2 / 0.085 | 1293 / 1.9 / 0.078 | 80.5 / 0.2 | 5, 0.103; 5, 0.100 |
| mp_farmhouse | 25 | 685 / 14.7 / 0.111 | 590 / 11.3 / 0.096 | 95.7 / 0.7 | 24, 0.166; 26, 0.170 |
| mp_harbor | 18 | 1377 / 16.6 / 0.201 | 746 / 11.4 / 0.177 | 641.1 / 9.6 | 45, 0.437; 34, 0.528 |
| mp_leningrad | 4 | 1019 / 3.0 / 0.050 | 986 / 2.8 / 0.044 | 33.5 / 0.0 | 5, 0.040; 5, 0.040 |
| mp_matmata | 7 | 1131 / 10.4 / 0.518 | 1024 / 8.3 / 0.408 | 138.1 / 31.6 | 16, 0.861; 10, 0.534 |
| mp_railyard | 23 | 1164 / 19.8 / 0.251 | 568 / 8.3 / 0.145 | 616.3 / 19.9 | 43, 0.332; 45, 0.346 |
| mp_rhine | 30 | 1663 / 20.5 / 0.475 | 721 / 11.0 / 0.307 | 977.1 / 35.2 | 72, 2.087; 24, 0.715 |
| mp_toujane | 11 | 668 / 21.5 / 0.241 | 570 / 18.9 / 0.186 | 116.0 / 17.3 | 53, 0.229; 57, 0.222 |
| mp_trainstation | 58 | 1184 / 34.1 / 0.350 | 729 / 18.3 / 0.236 | 509.7 / 55.0 | 100, 0.450; 107, 0.366 |

`cod2_cell_walks_draw_alike` (vcod, ignored, lavapipe) draws 16 spawns a
map at four yaws under both walks, 960 views: 59 differ. VERIFIED (the
frames).

- Two are holes in the recursive walk that the queued walk fills: on
  mp_toujane (3172 1413 146, yaw 180) the recursive walk drew sky where
  the far houses stand (14829 pixels), on mp_burgundy (932 -592 96, yaw
  180) a pole and a house (8328).
- The other 57 differ by 1 to 102 scattered pixels. In the three largest
  compared side by side (mp_harbor 100, mp_trainstation 102, mp_carentan
  56) they are specks of the test's snow cloud; no wall or floor is
  missing.

### Frames and timings

`GAME=cod2 tools/cod4_perf.sh`, 640 x 372, lavapipe, 12 bots, dm, 25 s,
the spectator held at the intermission spot. Base: section 8's build (code
at 6d5064b1), three rounds, the third alternating with the change's
first; after: three rounds. Means. VERIFIED (profiles):

| map | run | fps | net | bodies | cull | gather | encode | gpu cookies | gpu opaque | gpu blend | gpu total | draws | soups | world tris |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mp_toujane | base | 17.1 | 0.36 | 0.34 | 0.01 | 0.02 | 0.27 | 17.6 | 31.2 | 4.3 | 58.1 | 327 | 1827 | 73360 |
| mp_toujane | after | 21.3 | 0.30 | 0.33 | 0.01 | 0.03 | 0.20 | 13.8 | 24.4 | 2.7 | 44.4 | 269 | 1783 | 72883 |
| mp_carentan | base | 13.8 | 0.26 | 0.27 | 0.03 | 0.03 | 0.21 | 21.9 | 41.3 | 3.8 | 70.8 | 332 | 3249 | 144054 |
| mp_carentan | after | 18.0 | 0.27 | 0.30 | 0.03 | 0.03 | 0.20 | 16.5 | 31.2 | 2.6 | 53.3 | 307 | 2381 | 119971 |

Two runs of one build still spread 10-30% (the base's mp_toujane rounds
ran 23.1, 14.7 and 13.6 fps, the change's 22.5, 20.3 and 21.2); on
mp_carentan every round after (16.6 to 18.8) beat every base round (12.0
to 15.1). The held view keeps `cull` at its held cost; the walk a moving
view pays is the table above (mp_carentan's intermission view 1.85 to
0.19 ms). mp_carentan's spectator now draws 2381 soups and 119971 world
triangles where it drew 3249 and 144054: the CoD1 passes marked the rest,
and the opaque pass is that much lighter. The fog's share of the GPU
change cannot be told apart from the walk's at this spread. INFERRED (the
spread).

Pending, on a real card, this build and its base with the spectator at
mp_carentan's intermission spot: `SESSION=1 GAME=cod2 COD2_DIR=<install>
PORT=28995 tools/cod4_perf.sh <out> mp_carentan 30`, reading fps and
`gpu.opaque`. Pending by eye, flying (`vcod mp_toujane --game cod2`, and
mp_burgundy): the far houses from mp_toujane's spawn at 3172 1413 146
looking west and the house and pole from mp_burgundy's at 932 -592 96
looking west, which the old walk dropped, and any wall or roof that pops
in or out while turning in town, which the new walk would cause.

## 10. The probe weight cubes and the dynamic shadow's read (P23)

Section 9's two leftovers, and the entity vertex stage against retail's.

### The weight cubes

`lprobe.pse` (name at Mac 0x2ad690) reads the eight corner weights as two
`TEX ... CUBE` at the world normal, `$lightgridweights0` and `1`, then
dots each with two of `lightGridColorsR0..B1`. VERIFIED (program text;
`lib/lightprobe.hlsl`'s `ProbeLighting`).

The cubes are built at startup, not loaded. `Image_Load` (Mac 0xfd52a)
matches the two names and calls `Image_GenerateCubemapFunction` (0xfcda6)
with size 0x20, the image number and `Image_GetLightGridWeightsForVector`
(0xfc8d4), which fills each texel from its direction alone; A8R8G8B8, one
level a face. No iwd holds them. VERIFIED (decompile; what the function
computes and the level count are in cod2-materials.md 9.2). So they are
the same on every map.

vcod builds them once the same way (`vcod_cod2::lightgrid::weight_cubes`)
and uploads them as B8G8R8A8 (the bytes as retail lays them out) into a
two-layer cube array, group 3 binding 7
(`entity_material::grid_weight_cubes`). `cod2_model.wgsl`'s
`probe_grid_cubes` samples it for every `lprobe*` pixel, entities and the
view weapon. The per-vertex fallbacks (`gl_lighting`'s probe set, the view
weapon's vertex light) keep the computed eased weights (`probe_grid`).
VERIFIED (code). The sampler is linear, clamped; which filter retail's
code sampler 5/6 sets was not read. INFERRED.

What moves. A texel holds the eased weights at its centre rounded to
1/255 (`floor(w * 255 + 0.5)`), and the filter reads them linearly between
texel centres, where vcod had evaluated the ease exactly.

- Over 4000 normals spread round the sphere, the cubes read bilinearly
  against the computed weights: mean 0.11 of a level a weight, worst 3.95
  levels, the worst where the basis cube's faces meet (the projection onto
  the longest axis has a crease there that the filter rounds)
  (`weight_cubes_hold_the_eased_weights`). VERIFIED (the test's log).
- vcod's cube read on the GPU is seamless across faces, which D3D9's was
  not; that differs only within half a texel of a face's edge. INFERRED
  (the APIs).
- `cod2_capture_frame_draws_lit` (mp_toujane, 320 x 240): from the spawn
  no pixel moved; from behind the body 301 pixels moved, each by one
  level, scattered over the body and its rifle and nowhere else. VERIFIED
  (the frames against the base).

This is the retail pixel's own quantization and filtering, so it moves
vcod toward retail; the lit pixel's ALU falls to retail's count.

### The dynamic shadow's read

Retail's lit vertex writes `screenCoords`
(`Transform_ClipSpacePosToTexCoords`, `lib/transform.hlsl`: the clip
position times `clipSpaceLookupScale` plus w times
`clipSpaceLookupOffset`), and the pixel reads
`tex2Dproj( dynamicShadowSampler, screenCoords )`: `lprobe.vsn` (name at
Mac 0x282e6c) `MUL r1, r0.w, c28; MAD oT4, r0, c27, r1`, `lprobe.pse` and
`lmap.pse` one `TXP` each, the shadow straight onto the sun. VERIFIED
(program text, HLSL).

vcod's pixel used to ask the target's size, clamp the pixel's position to
it, load, and take one minus the value (vcod's target held the share
taken). Now both `vs_main`s (world and entity) write
`screen = (0.5 (x + w), 0.5 (w - y), 0, w)` and the pixel samples at
`screen.xy / screen.w`. The target holds retail's value (the share left:
made white, cleared white, each receiver writing the product of its
cookies' keeps), so there is no `1 - x`. VERIFIED (code). Frames match
byte for byte: with the weight cubes left out, both captured views match
the base exactly. VERIFIED (the frames).

### The entity vertex stage

Retail's `lprobe*.vsn` hold no bone palette: one `worldViewProjection`
(`c23`) and one world 3 x 3 (`c4`), no `ADDRESS` use and no `ARL` in any
`.vsn` (`lprobe.vsn` is 22 instructions). VERIFIED (program text).
Skinning is the CPU's: `R_SkinXModelCmd` (Mac 0xd23de),
`R_SkinXSurfaceSkinned` (0xd1b58) and the skinned vertex cache
(`R_LockSkinnedCache` 0xd0030) write posed vertices that the rigid program
then draws. VERIFIED (symbols). That every skinned surface takes that path
is INFERRED (no skinned vertex program exists to take another).

vcod skins on the GPU (`dynamic_model.wgsl`'s bone storage buffer), so its
`vs_main` (83 ALU) carries the skin that retail spends on the CPU.
Section 9's 117 for retail was the 4079-byte program whose names run from
`lprobe_flag_s_lfog.vsn` to `lprobe_s.vsn` (Mac 0x289b04 to 0x289b3c): its
sine and cosine tables are the flag wave's, so it is `lprobe_flag*`'s,
not a skinned `lprobe_s`. INFERRED (the constants; the extractor keys a
name to the text before it). Not changed: a GPU skin is cheaper for vcod
than a CPU one feeding a dynamic buffer each frame.

### Counts

`SHADER_OPS=2 cargo test -p vcod --bins cod2_shader_ops -- --nocapture`
against `tools/re/cod2_macarb.py --count`, fetch / ALU / slots. VERIFIED
(counter, program text):

| path | retail fetch / ALU | before | after |
|---|---|---|---|
| `lmap` runs | 8 / 20 | 8 / 23 / 25 | 8 / 20 / 22 |
| `lmap_s` runs | 10 / 30 | 10 / 29 / 34 | 10 / 26 / 31 |
| `lmap_s_dtl` runs | 11 / 33 | 11 / 32 / 37 | 11 / 29 / 34 |
| world `vs_main` | 0 / 42 (`lmap_s_dtl.vsn`) | 0 / 23 / 30 | 0 / 27 / 34 |
| entity `lprobe` | 5 / 25 | 3 / 51 / 54 | 5 / 24 / 27 |
| entity `lprobe_s` | 7 / 35 | 5 / 58 / 64 | 7 / 31 / 37 |
| entity `lprobe_s_dtl` | 8 / 38 | 6 / 61 / 67 | 8 / 34 / 40 |
| entity `vs_main` | 0 / 22 (`lprobe.vsn`, skinned on the CPU) | 0 / 79 / 86 | 0 / 83 / 90 |

`dyn_shadow` fell from 5 to 2 and its size query is gone (the `queries`
column 1 to 0 on every lit path); `probe_grid`'s 33 left the pixel for two
fetches and `probe_colours`' dots. Both vertex stages gain the four of
`screen`. VERIFIED (`SHADER_OPS=2` breakdown).

### Frames and timings

`GAME=cod2 tools/cod4_perf.sh`, 640 x 372, lavapipe, 12 bots, dm, 25 s,
the spectator at the intermission spot; base (c302d012) and after, three
rounds each, alternated. Means. VERIFIED (profiles):

| map | run | fps (rounds) | encode | gpu cookies | gpu opaque | gpu blend | gpu total |
|---|---|---|---|---|---|---|---|
| mp_toujane | base | 17.7 (15.4, 16.8, 21.0) | 0.28 | 16.9 | 29.7 | 3.1 | 54.2 |
| mp_toujane | after | 16.9 (16.1, 19.2, 15.5) | 0.28 | 17.1 | 30.7 | 4.1 | 56.4 |
| mp_carentan | base | 13.3 (11.8, 14.5, 13.5) | 0.30 | 22.3 | 41.1 | 4.8 | 72.8 |
| mp_carentan | after | 14.2 (13.7, 14.5, 14.4) | 0.23 | 21.7 | 38.1 | 3.5 | 67.6 |

Both maps' differences sit inside the spread of one build's rounds
(mp_toujane's base ran 15.4 to 21.0): lavapipe shows no change. The
spectator's view holds few lit models, so the weight cubes' share of the
opaque pass is small there, and lavapipe's GPU time is fill, not ALU.
INFERRED (the spread). On a real card the lit model pixel runs about half
the ALU it did. INFERRED (the counts above).

Pending, on a real card, this build and its base: `SESSION=1 GAME=cod2
COD2_DIR=<install> PORT=28995 tools/cod4_perf.sh <out> mp_carentan 30`,
reading fps and `gpu.opaque`. Pending by eye: a player body and the view
weapon in sun and in shade (their light should look the same as before,
one level off at most), and a body's shadow cookie on the street and a
body standing in it (unchanged).

## 11. The cookie receivers over the world's prepass (P28)

On a real card (mp_carentan, 12 bots, spectator, 1920 x 1080) a frame took
3.22 ms of GPU, of which the cookies took 1.05, and 2.45 at 3840 x 1600:
fill, not geometry. What retail does per cookie, against what vcod does.

### Retail's cookie frame

From `gfx_d3d_mp_x86_s.dll` (cod2-materials.md 9.8 holds the rest):

- The cookie is 128 x 128 `A8R8G8B8` with a 128 x 128 D24S8 depth
  (0x10022f80, 0x10023020, 0x10022cc0), one at a time. At most 24 a frame,
  the candidate list's slots (0x100280f0), the cut-off sliding toward
  `sc_wantCount` 12 with `sc_wantCountMargin` 1; `sc_blur` 2 rounds.
  VERIFIED (the decompile, the dvar pushes 0x1000b57d..0x1000b729). vcod's
  tiles are the same size, its pick and blur the same; nothing to cut
  there.
- The receivers are a geometry pass of their own, per cookie, not folded
  into the lit pass: for each cookie 0x10027d20 queues a draw-surfaces
  command (cmd 0x17, `FUN_10021c90`) with that cookie's receiver list and
  technique 32 (`shadowcookie receiver`), after its casters' command with
  technique 31. VERIFIED (the command ids written by 0x10021c90 and the
  call's arguments 0x1f, 0x20).
- The list is culled per cookie by surface bounds: 0x10027980 keeps the
  frame's visible surfaces with technique 32, not of the caster's entity,
  whose box passes the five planes at 0x101904a8 (`FUN_10027930`, five
  plane tests of the box's nearest corner). VERIFIED (the decompile).
- No scissor and no stencil: the only rectangle in the cookie's commands
  is the viewport (cmd 0xd, `FUN_10021ff0`), set to 1, 1, 126, 126 around
  the casters and back to 0, 0, 128, 128 before the blur. VERIFIED (the
  command ids 0x10027d20 writes). That the receivers test the scene's
  depth from the world's prepass, LessEqual with no write
  (`shadowcookie_receiver.sm`), is cod2-materials.md 9.8's claim:
  INFERRED (`r_depthPrepassModels` 0, the statemap).

Retail draws every cookie's receivers again; vcod draws the visible
receivers once, each draw testing only the cookies its box reaches
(section 8), so on geometry vcod already does less. It did more on fill:
its receiver pass ran ahead of the depth prepass on a depth of its own,
cleared each frame and written, so every overdrawn receiver fragment ran
`fs_receive`.

### What changed

With `r_vc_depthprepass` on (CoD2's default) and one sample:

1. The depth prepass draws the world's part.
2. The cookie receivers draw into the dynamic shadow over that depth,
   LessEqual and no depth write, as retail's.
3. The entities' part of the prepass loads the depth and adds theirs
   (`Renderer::cookie_receivers_on_prepass`, `PrepassParts`).

The receivers' own depth is neither cleared nor written, and a receiver
fragment behind the world's nearest opaque surface is rejected before its
shader. `vs_receive`'s position is `@invariant`, as `vs_main`'s, the same
product of the same matrices. With the prepass off or MSAA on, the
receivers keep their own depth. The profile's group `receivers` times
them inside the prepass, so `gpu.cookies` holds the casters and the blur
alone and the receivers sit in `gpu.opaque`. VERIFIED (code, the
profile).

Pixels: `cod2_cookie_receivers_over_the_prepass` draws mp_toujane's
capture frame with the prepass on, receivers on their own depth and over
the prepass. From the spawn (no cookie in view) and from behind the body
(6325 pixels under the cookies both ways) the frames are byte for byte the
same, and so is `cod2_capture_frame_draws_lit` (prepass off) against the
base. VERIFIED (the tests, md5s). A pixel can move where a receiver lies
behind a nearer opaque world surface that receives no cookie: over the
prepass that surface's pixel reads the white the target cleared to, where
on the receivers' own depth it read the hidden receiver's shadow. That is
retail's rule (INFERRED, the same depth test); neither view has such a
pixel.

Tried and dropped: scissoring the receivers to the screen rectangles the
cookies can darken (each cookie's column along the sun cut by the
receiving batches' boxes, joined to four rectangles). From the
spectator's spot the rectangles covered the whole target (`cookiepx` 763
to 1308 per mille of the screen, summed), since a batch's box is every
visible soup of its material and a dozen cookies sit across the view, and
the repeated draws cost more than they cut (mp_toujane's cookies 14.0 to
17.8 ms -> 17.6 to 25.4). VERIFIED (profiles).

### Frames and timings

`GAME=cod2 tools/cod4_perf.sh`, 640 x 372, lavapipe, 12 bots, dm, 25 s,
the spectator at the intermission spot; base (8b540e93) and after, three
rounds each, alternated. Means of the frames past the first third.
`receivers` for the base is its `gpu.cookies` less the after run's (the
casters and blur, the same work). VERIFIED (profiles):

| map | run | fps (rounds) | gpu cookies | receivers | gpu opaque | gpu total |
|---|---|---|---|---|---|---|
| mp_toujane | base | 22.3 (23.2, 21.3, 22.5) | 13.2 | ~8.5 | 23.3 | 42.0 |
| mp_toujane | after | 22.1 (20.5, 24.2, 21.7) | 4.7 | 6.5 | 31.5 | 42.4 |
| mp_carentan | base | 17.2 (16.2, 18.1, 17.3) | 16.6 | ~11.1 | 32.5 | 55.3 |
| mp_carentan | after | 16.3 (16.7, 14.3, 18.0) | 5.5 | 9.4 | 45.4 | 57.8 |

On lavapipe the receivers fall by about a fifth and the frame's total
stays within the spread of one build's rounds: lavapipe rasterizes the
receivers' geometry on the CPU (section 2), and that is unchanged. On a
real card, where they are fill (2.45 ms at 3840 x 1600), the saving is the
depth clear and write of a screen-sized target and the overdrawn receiver
fragments. INFERRED (the spread, the real card's scaling).

Pending, on a real card, this build and its base: `SESSION=1 GAME=cod2
COD2_DIR=<install> PORT=28995 SIZE=1920x1080 tools/cod4_perf.sh <out>
mp_carentan 30`, reading fps, `gpu.cookies` plus the `receivers` group
(base: `gpu.cookies` alone) and `gpu.total`. Pending by eye: a body's
cookie shadow on the street and on a wall, the same as before.

## 12. Retail's prepass, the stale config and the receivers per soup (P30)

A real-card run (mp_carentan, 12 bots, 1920 x 1080) counted `prepass 0`,
so section 11's receivers drew on their own depth there.

### Retail's prepass

From `gfx_d3d_mp_x86_s.dll` (image base 0x10000000): the main view's
scene, `FUN_10025430`, reached from 0x10025770 through 0x100256f0 when
`r_fullbright` (0x101ce6d8), `r_rendererInUse` dx7 and `r_debugShader`
(0x101ce6b0) are all off.

- It opens with 0x10024e80. With `r_zFeather` (0x101ce694, default 1), a
  dx9 renderer and the `floatZ` target (0x101d489c), it sets target 6,
  clears it through 0x10024e30 and draws the scene's list with technique
  2, `build floatz` (pushes 0, 2 before the 0x10021c90 call at
  0x10024f2a). Without the feather, and only when its third argument is 1
  (the check at 0x10024f62), it sets target 7, clears, and draws the list
  with technique 0, `depth prepass` (pushes 0, 0 before the call at
  0x10024ff2). VERIFIED (the disassembly, the dvar registrations). That
  argument is the flag 0x10025451 tests before the cookies' 0x10027d20,
  set at 0x10025770 from `sc_enable` (0x101ce7cc, default 1) on a non-dx7
  renderer. INFERRED (the register flow through 0x100256f0).
- `build_floatz.sm` writes depth (`depthWrite Enable`, its test the
  material's), and `nocolor.sm`, the depth prepass's, writes depth for an
  unblended material and no colour. VERIFIED (the files, `iw_07.iwd`).
- Then, under `sc_enable`, the cookies (0x10027d20: casters, then each
  cookie's receivers, section 11), then sky (technique 1) and lit (6) over
  the same list (the 0x10021c90 calls at 0x10025505 and 0x10025510), then
  the emissive pass (0x18 or 0x15, 0x10025611). One sorted list carries
  the opaque and the decal surfaces in CoD2; no separate decal list is
  drawn. The prepass takes the list's draws with the same two arguments as
  the lit pass. INFERRED (the registers carried into 0x10024e80, `ebx` and
  its second argument).
- Types 0 and 2 leave models out under `r_depthPrepassModels` 0
  (cod2-materials.md 4.1), so retail's prepass is the world's. That the
  lit and receiver passes test that depth is INFERRED (a colourless
  technique 0 is only of use if its depth stays).

A stock PC client always lays the world's depth before the cookies and the
lit pass: as `build floatz` by default, as the depth prepass with
`r_zFeather 0`. vcod's CoD2 default of `r_vc_depthprepass 1` has retail's
shape: world part before the receivers, entities after.

### Why it read 0

`r_vc_depthprepass` was archived, and an earlier build defaulted it to 0
on CoD2. A run of that build wrote `seta r_vc_depthprepass "0"` into the
profile's `vcod_mp.cfg`, and every later start kept it (`register_kept`).
INFERRED (the history; that config was not available).

Fix: `Shell::register_depth_prepass` registers it after the config at the
game's default, not archived, so a config's line is dropped and no longer
written (`a_config_s_depth_prepass_gives_way_to_the_game_s_default`). It
stays live: `set r_vc_depthprepass 0` holds for the session.

### The receivers per soup

Retail's receivers per cookie keep only the surfaces whose box passes that
cookie's five planes (0x10027980, section 11). vcod's single pass tested,
per world batch, the cookies the box of all its visible soups reaches, and
drew every receiving batch. A batch's box spans a material across the
view, so nearly every batch tested every cookie at every pixel.

Now each receiving soup keeps its own gathered range and box
(`WorldGpu::cookie_runs`), tests only the cookies its own box reaches, and
draws in runs of abutting soups of one mask. Over the prepass, where a
receiver writes no depth, a soup no cookie reaches is not drawn, as
retail's lists leave it out. On the receivers' own depth it still draws,
since its depth hides what is behind it; without the prepass there is no
retail way to drop it, as retail's receivers always test the world's
depth. VERIFIED (code, the tests below).

Pixels: `cod2_capture_frame_draws_lit` and
`cod2_cookie_receivers_over_the_prepass` write md5s identical to the base
(behind e5b60f35..., spawn 2f7ed08a..., own depth and over the prepass
alike; 6325 pixels under the cookies). VERIFIED (the tests, md5s).

### Timings

`GAME=cod2 tools/cod4_perf.sh`, 640 x 400, lavapipe, 12 bots, dm, 25 s,
the spectator at the intermission spot, prepass on in both; base
(d06a1335) and after, alternated, means past the first third. Rounds 4
and 5 ran while the machine was busier, so they are listed apart.
`receivers` and `cookies` (casters and blur) are the profile's groups.
VERIFIED (profiles):

| map | rounds | run | fps | receivers | cookies | gpu opaque | gpu total | cookie draws |
|---|---|---|---|---|---|---|---|---|
| mp_toujane | 1-3 | base | 21.1 (21.3, 22.8, 19.1) | 6.80 | 5.00 | 32.9 | 45.2 | 38 |
| mp_toujane | 1-3 | after | 24.3 (27.2, 24.9, 20.7) | 1.94 | 5.01 | 27.4 | 39.2 | 153 |
| mp_carentan | 1-3 | base | 17.1 (16.6, 17.4, 17.2) | 9.80 | 5.41 | 43.4 | 56.4 | 52 |
| mp_carentan | 1-3 | after | 16.3 (17.2, 14.9, 16.8) | 2.06 | 6.76 | 42.1 | 58.8 | 175 |
| mp_carentan | 4-5 | base | 10.9 (11.2, 10.6) | 13.43 | 9.24 | 64.3 | 88.3 | 55 |
| mp_carentan | 4-5 | after | 13.3 (12.5, 14.1) | 2.64 | 8.70 | 50.6 | 72.0 | 163 |

The receivers fall by 70 to 80 per cent on both maps, every round. The
frame's total follows on mp_toujane and in the busier carentan rounds,
and stays inside one build's spread in the quiet ones, where lavapipe's
other passes swing by more than the receivers saved. Receiver draws rise
from about 50 to about 160 a frame (runs of soups of one mask against one
run per batch). INFERRED (the spread). On a real card the receivers were
fill (section 11), and fill is what the per-soup masks and the dropped
soups cut.

Pending, on a real card, this build and its base, each with `SESSION=1
GAME=cod2 COD2_DIR=<install> PORT=28995 SIZE=1920x1080
tools/cod4_perf.sh <out> mp_carentan 30`, reading fps, `gpu.opaque`,
`gpu.cookies` and the `prepass` count (non-zero now whatever the config
holds). The base reads the config's value: give it `--exec-in-game "set
r_vc_depthprepass 1"` as a last argument for a like-for-like pair; a third
run of the base without it shows what the stale 0 cost.
