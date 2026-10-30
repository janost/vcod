# CoD4 client frame time

How the vcod CoD4 client spends a frame in live play, measured with
`tools/cod4_perf.sh`, and what to cut next. Everything here is vcod's own
code measured on this branch, not a retail fact; no binary address is
involved. The numbers are from a lavapipe (llvmpipe) adapter, so the GPU
columns are CPU rasterization: they rank the GPU work, they are not a
real card's milliseconds.

## 1. The tool

`VCOD_FRAME_PROFILE=<path>` on `vcod` (`crates/client/src/frame_prof.rs`)
writes per drawn frame the CPU ms of each stage (exclusive, nested spans
pause their outer one): `pace` (the `com_maxfps` sleep), `net` (the
session's step: packets, snapshots, prediction, menus), `scene`,
`bodies` (player bodies posed and skinned), `pose` (other models and
brush models), `vm`, `events`, `fx` (the session's effect starts),
`fxstep`, `sound`, `fxdraw` (quads, clouds, marks, lights handed over),
`hud`, `ui`, `cull` (the vis walk), `gather` (the visible indices
gathered, ordered and uploaded, dlights), `build`, `acquire`, `encode`,
`submit`; then counts (draws in the merged list, model instances,
particles, world triangles, visible soups); and, a few frames late, GPU
ms per pass group off timestamps written between the passes (needs
`TIMESTAMP_QUERY_INSIDE_ENCODERS`): `vcshadow` (vc_shadows), `cookies`,
`sunshadow`, then the main scene in four (section 7): `opaque` (the
main pass up to the split), `split` (shadow mask, SSAO, depth and scene
copies), `blend` (the main pass after the split), `post` (post passes and
the HUD), and `gamma` (gamma and present blit). `cod4_perf.py` sums the
four as `gpu.scene`, and prints the counts of the GPU-slowest 5% of the
frames beside the mean's.

`tools/cod4_perf.sh <outdir> <map> <secs>` runs vcod-server `--game cod4`
with 12 shooting bots and a windowed client joined as a spectator, both
in a loopback-only `unshare -rn`, the client in a nested
`kwin_wayland --virtual`, and prints `tools/cod4_perf.py`'s summary (mean,
p50, p95, p99 per stage; `cod4_perf.py <outdir>` reads its `profile.txt`). On `dm` (the default) the spectator follows a
bot, so the view moves with the fight and two runs differ; on `war` it
stands still at the spectator spot, the view repeatable run to run (two
mp_crossfire runs: GPU scene 1072 and 1019 ms). Use `war` for an A/B of
GPU work, `dm` for play. Extra arguments go to `vcod`; `--exec-in-game
"set <cvar> <v>"` toggles a setting for an A/B. `SESSION=1` runs both in
the current desktop session instead, the client a window on the desktop's
own display and GPU (no namespace, no nested kwin, whose private dbus
session tripped over a desktop's portals), the server on
127.0.0.1:`PORT`.

## 2. Every stock map (dm, 12 bots, 640x372, 4x MSAA, 25 s, the first 5 s after the join dropped)

CPU ms is the sum of the CPU stages above less `pace` and `submit` (on
lavapipe `submit` waits on the rasterizer). mp_cargoship's row is from
after the probe fix (section 4); before it the client panicked at load.

| Map | Frames | CPU ms | net | bodies | pose | events | fx (all) | cull | hud+ui | GPU sun shadow | GPU scene | draws | world tris | particles |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mp_backlot | 68 | 6.42 | 0.76 | 1.56 | 0.43 | 0.45 | 1.59 | 0.63 | 0.26 | 43 | 343 | 2015 | 101752 | 1493 |
| mp_bloc | 67 | 8.73 | 0.67 | 1.72 | 0.08 | 0.79 | 3.88 | 0.61 | 0.29 | 58 | 421 | 951 | 168481 | 572 |
| mp_bog | 82 | 7.82 | 0.54 | 1.71 | 0.22 | 0.95 | 2.88 | 0.49 | 0.26 | 69 | 429 | 1021 | 185180 | 610 |
| mp_broadcast | 68 | 6.47 | 0.47 | 1.58 | 0.25 | 0.68 | 1.92 | 0.52 | 0.28 | 62 | 449 | 1568 | 198133 | 1459 |
| mp_carentan | 84 | 5.04 | 0.47 | 1.30 | 0.27 | 0.41 | 1.18 | 0.57 | 0.22 | 68 | 313 | 1575 | 121435 | 1468 |
| mp_cargoship | 56 | 6.75 | 0.88 | 1.45 | 0.22 | 0.85 | 1.57 | 0.57 | 0.24 | 43 | 807 | 1186 | 160041 | 2019 |
| mp_citystreets | 65 | 6.34 | 0.50 | 1.46 | 0.70 | 0.40 | 1.73 | 0.42 | 0.21 | 44 | 329 | 3125 | 105193 | 1709 |
| mp_convoy | 44 | 7.24 | 0.53 | 1.41 | 0.39 | 0.86 | 1.92 | 1.09 | 0.24 | 49 | 544 | 1560 | 304766 | 669 |
| mp_countdown | 67 | 4.67 | 0.42 | 1.32 | 0.12 | 0.49 | 0.97 | 0.52 | 0.22 | 36 | 352 | 1105 | 223576 | 1133 |
| mp_crash | 71 | 6.06 | 0.50 | 1.57 | 0.31 | 0.76 | 1.40 | 0.66 | 0.22 | 49 | 488 | 1702 | 170495 | 1219 |
| mp_crash_snow | 85 | 5.14 | 0.39 | 1.38 | 0.28 | 0.47 | 1.18 | 0.58 | 0.23 | 39 | 353 | 1544 | 136216 | 1086 |
| mp_creek | 33 | 8.88 | 0.64 | 1.42 | 0.09 | 1.87 | 1.79 | 2.23 | 0.23 | 60 | 745 | 870 | 792127 | 1922 |
| mp_crossfire | 33 | 8.66 | 0.81 | 1.46 | 0.41 | 1.02 | 2.29 | 1.54 | 0.24 | 48 | 917 | 2951 | 337912 | 1481 |
| mp_farm | 61 | 6.00 | 0.67 | 1.53 | 0.09 | 0.99 | 1.33 | 0.59 | 0.27 | 76 | 533 | 690 | 128926 | 1517 |
| mp_killhouse | 86 | 3.92 | 0.33 | 1.32 | 0.06 | 0.46 | 0.86 | 0.24 | 0.23 | 36 | 291 | 551 | 103038 | 870 |
| mp_overgrown | 62 | 8.41 | 0.68 | 1.54 | 0.08 | 1.23 | 2.32 | 1.69 | 0.24 | 89 | 563 | 929 | 419127 | 561 |
| mp_pipeline | 86 | 6.37 | 0.45 | 1.54 | 0.20 | 0.79 | 1.74 | 0.71 | 0.27 | 54 | 389 | 1411 | 203067 | 1417 |
| mp_shipment | 114 | 3.76 | 0.31 | 1.36 | 0.07 | 0.38 | 0.74 | 0.19 | 0.29 | 35 | 218 | 663 | 92060 | 996 |
| mp_showdown | 99 | 4.22 | 0.35 | 1.54 | 0.06 | 0.34 | 0.93 | 0.23 | 0.24 | 41 | 229 | 849 | 72082 | 1025 |
| mp_strike | 91 | 5.39 | 0.37 | 1.38 | 0.23 | 0.50 | 1.45 | 0.62 | 0.21 | 39 | 370 | 1783 | 185679 | 1500 |
| mp_vacant | 75 | 4.99 | 0.45 | 1.50 | 0.32 | 0.44 | 0.93 | 0.30 | 0.40 | 35 | 299 | 1527 | 94737 | 1072 |

The CPU frame is 3.8 to 8.9 ms on every map: a real card would draw
110 to 260 fps CPU-bound. The heaviest CPU maps are mp_creek, mp_bloc,
mp_crossfire and mp_overgrown (8.4-8.9 ms); the heaviest GPU (lavapipe)
are mp_crossfire, mp_cargoship, mp_creek and mp_overgrown.

## 3. What the GPU time follows (mp_crossfire, war spectator, fixed view)

| Run | GPU scene ms | world tris |
|---|---|---|
| base | 1072 | 427247 |
| base again | 1019 | 427247 |
| `r_zFeather 0` | 1093 | 427247 |
| `r_glow_allowed 0` | 1206 | 427247 |
| `r_dof_enable 0` | 1193 | 427247 |
| `sm_enable 0` (dm, moving view) | 520 vs 618 | 171905 vs 194176 |

The zfeather, glow and depth of field passes cost nothing measurable;
the scene follows the world triangles gathered (dm frames at 170k tris
draw in ~500 ms, the war spot's 427k in ~1050 ms). The sun shadow is
40-90 ms a frame on every map when on.

## 4. Fixes

- mp_cargoship crashed the client at load: its 47 reflection probes take
  282 layers in the entity probe cube array, past wgpu's default 256
  `max_texture_array_layers`. The device now asks for the adapter's
  layers. Another branch fixed the same bug the same way in parallel;
  the merge keeps that version.
- `--exec-in-game` never ran in a CoD4 game (it waited on CoD1's and
  CoD2's frame clock), so nothing could set a cvar after the join; it now
  runs once a CoD4 view draws (`Session::cod4_in_game`).

No frame-time cut landed: the CPU stages are small and spread (section
2), and the GPU's one big cost (5) is a culling question, not a waste
in the draw path.

## 5. What to cut next, ranked

1. World triangles drawn. The war spectator spot on mp_crossfire gathers
   427k triangles over 6278 soups; lavapipe's scene time follows that
   count. Check the cell/portal walk and the per-soup frustum test for
   over-marking on the big open maps (crossfire, creek, overgrown,
   cargoship) against retail's `R_CullCells` (cod4-world.md), and cull
   static models by distance (`lodDist`) per instance.
2. The sun shadow pass, 40-90 ms on lavapipe. It culls caster ranges
   against each partition already; the F3 `sunshadow` line's caster
   counts against retail's per-partition caster lists say whether it
   draws more.
3. `bodies` (1.3-1.7 ms for 12 players, about 0.12 ms each): the anim
   tree and skinning per body per frame; the rig lookup clones the
   character key every frame (`cod4_players::posed`).
4. Effects (`fx`+`fxstep`+`fxdraw`, 0.7-3.9 ms, mp_bloc worst): every
   live particle's quad carries a cloned shader `String` and the
   renderer resolves its sort by name each frame (`set_fx_quads`).
5. `gather` and `cull` (0.2-2.2 ms, mp_creek worst): the visible
   indices are gathered on the CPU and uploaded (up to ~5 MB) every
   frame even when the visible set did not change.
6. `events` (0.3-1.0 ms) and `net` (0.4-0.9 ms).

The p99 `other` spikes (~12 ms) are first-use uploads (models,
effects' textures) inside the measured window.

## 6. The cuts from section 5's items 1, 4 and 5

War spectator spot, 640x372, lavapipe, `tools/cod4_perf.sh`. The box was
shared with other jobs (load 16-19 for some runs), so the CPU and GPU
means move 20-40% run to run on every stage; the triangle and draw
counts and the stage the fix touched are the signal.

| Change | Map | world tris | GPU scene ms | stage ms (p50) |
|---|---|---|---|---|
| base | mp_crossfire | 427247 | 634 632 782 | |
| cell walk by portals alone (cod4-world.md 9.9) | mp_crossfire | 263289 | 667 556 501 | |
| static model LODs and `cullDist` (cod4-world.md 9.10) | mp_crossfire | 133671 | 636 613 502 | |
| both | mp_creek | 388888 -> 161428 | 248 -> 161 | |
| both | mp_overgrown | 517223 -> 231932 | 290 -> 270 | |
| both | mp_killhouse | 100811 -> 52087 | 256 -> 473 (one run each) | |
| effect shader names shared (`Arc<str>`) | mp_crossfire | | | fxdraw 0.71 -> 0.61 |
| gather skipped when the visible set holds still | mp_crossfire | | | gather 0.38 -> 0.10 |

- The world triangles fall by two thirds on mp_crossfire and mp_creek,
  but lavapipe's GPU scene time does not follow them below ~250k: at
  134k triangles the war spot still draws in 500-650 ms. Section 3's
  reading (the scene follows the triangles) held for the moving dm
  view; at the fixed spot the remaining cost is elsewhere (fill: MSAA,
  blended effects over the spot's view, the post passes). Measure that on
  a real card before cutting further.
- The static model LODs bake every LOD of every placement, so the
  vertex buffer grows: mp_creek 8.9M vertices (393 MB), mp_killhouse
  6.9M (303 MB). The device asks for the adapter's `max_buffer_size`.
- `cull` is unchanged (0.4-1.3 ms): the walk itself, not the gather.

## 7. A real-GPU profile: what each stage was (2026-10-27)

The profile (SESSION=1, mp_crossfire, 1920x1080, 4 bots, 66 fps): wall
15.1 ms mean, p95 29.3; bodies 3.6; submit 3.0; acquire 2.3, p95 15.1;
gpu.scene 12.4, p95 27.5; 2208 draws. Measured here on lavapipe at
480x272 with 4 bots; `war` for the A/B (a fixed view: the same 134873
triangles and 4596 soups in both runs), the CPU stages real.

| Run (war, mp_crossfire) | fps | wall ms | bodies mean / p50 | encode | entity draws | draws | GPU opaque |
|---|---|---|---|---|---|---|---|
| before | 1.8 | 544 | 4.73 / 3.90 | 0.61 | 1135 | 2286 | 485 |
| after | 2.3 | 426 | 2.64 / 0.31 | 0.46 | 224 | 2242 | 376 |

1. `bodies`: 96% of a posed body was the 1.5/UO engine records'
   walk (`PoseBuffer::blend_tree`'s `walk_records`, which clones the
   whole tree and the records), kept on every pose for the stored-
   arithmetic bone trace only the 1.5 and UO servers read. A drawn body
   now poses without them (`PoseBuffer::drawn`,
   `playerpose::pose_player_drawn`, `PlayerBody::pose`); the locals come
   out bit for bit the same (`skeleton::tests::a_drawn_pose_matches_the_recorded_one`).
   With it the skeleton's bone lookup became a name map, `bone_world`
   walks one chain, not every bone (the controllers asked for six of
   those a body), and `posed` composes the worlds once for all the rig's
   models. One cloned body posed 8 times a frame off the spectator
   capture: 124 us a body before, 10 after. What is left of `bodies`'
   mean is the ragdolls: on lavapipe a frame runs up to 12 of their
   60 Hz substeps, each 4 rounds of a trace per point, 1-3 ms a frame
   while one falls; at a real card's frame rate that is one substep.
2. `submit` and the draws: 1134 of the 2208 draws were entity surfaces,
   and 923 of those were one instance's next surface on the same skin
   (mp_crossfire's 48-surface models, 23 of them in view). A model whose
   vertices fit u16 indices now uploads its indices rebased onto the
   shared vertex buffer and one bind group per distinct skin
   (`upload_vm_model`), and the draw loop draws a run of one instance's
   consecutive surfaces on one one-stage skin as one `draw_indexed`
   over their joined index range (`renderer::ent_run`), never across
   the split, the feather copy or the dlights. The same triangles go out
   in the same order: entity draws 1135 -> 224. The `entdraws` count is
   new in the profile. On the real card this is what `submit` (wgpu's
   per-draw tracking at finish and submit) and `encode` scale with;
   expect both to fall by about the share of draws gone.
3. `gpu.scene`: the split stamps (section 1) show the opaque main pass
   is all of it on lavapipe: split 0.01 ms, blend 0.04, post 0.6, no
   redundant pass. Its heavy frames are the views the cull lets more of
   the world through: on dm the GPU-slowest 5% of frames gather 142583
   triangles over 5421 soups against a mean of 72691 over 2590, the
   same draws within 25%. The p95 of 27 ms is section 5 item 1's
   culling question on the big outdoor views, not a waste in the pass.
4. `acquire`: `r_swapInterval 0` (what `cod4_perf.sh` sets) is
   `AutoNoVsync` with `desired_maximum_frame_latency` 2, so an acquire
   blocks only while two frames are queued on the GPU. Its spikes are
   the frames the GPU is behind on: acquire p95 15 ms plus the ~12 ms
   CPU frame is gpu.scene's p95 27.5. Nothing waits on the GPU at the
   wrong point; the wait goes when (3) does. No change.

Pixel output: the lavapipe frames of `cod4_live_gun_and_body_take_the_light_grid`
(lit sun and shade), `cod4_renderer_builds_and_draws_without_validation_errors`
(spawn and above) and `cod4_live_saw_gunner_view` (mounted, aimed up)
are byte-identical before and after (`VCOD_C4_SHOTS`); its `saw_firing`
frame differs between two runs of the same build too (the live
server's timing), so it does not count.

## 8. Close to a car on mp_crash (playtest 2026-10-26)

- The playtest's F3 by a damaged `vehicle_80s_sedan1_red_destructible_mp`
  (-279 1396 293, yaw -15, pitch -28) read 10.9 fps, GPU scene 89 ms,
  168 world draws, 42 model instances, 132168 triangles, `fx p913 m108
  l7`. VERIFIED (the screenshot).
- The car's LOD 0 is eight surfaces, 2608 triangles: the body
  (`mc/mtl_80s_econ_red`, 2248), the intact and shattered glass inside
  and out (`mtl_80s_econ_glass_outside` and `_inside`,
  `com_glass_shattered_outside` and `_inside`, 50 to 94 each, the sets
  `mc_l_sm_b0c0s0`), and the engine, rubber and destroyed parts.
  VERIFIED (the zone through `cod4_fly`). All four glass layers drew;
  retail hides one pair by the parts' `hidePart`s, which vcod now does
  too (section 15, cod4-client-view.md 15).
- Through the live draw path on lavapipe (`cod4_live`'s harness, 640 x
  384, 4x MSAA) that view takes the same time with the glass alone as
  with nothing, and the whole car about as much as the world it hides:
  no surface of it costs out of line. VERIFIED (a scratch run, ours;
  lavapipe's timings are CPU time and noisy). The 89 ms is not
  reproduced without a real adapter: the next step is `SESSION=1
  tools/cod4_perf.sh` at that spot on the playtest machine, with
  `r_distortion 0` and Soften Smoke Edges off, to split the GPU time
  between the car's glass and the 913 particles in view (its damage
  smoke). Pending.

## 9. The spot light shadow tiles (mp_crash's tiled room)

A playtest on a real card read 66 fps with `sm_enable` 0 and
54 with 1 under mp_crash's spot light 3 (60 -684 255); `sm_enable` gates
the sun's partitions and the spot tiles both. `tools/cod4_perf.sh` takes
`SERVER_ARGS` now, so `--set sv_cheats=1` and `--exec-in-game
"setviewpos 60 -684 225 67 40"` hold the view there (war, 4 bots,
lavapipe, 1280x772, 4x MSAA, 25 s, the first 8 dropped). GPU ms, mean
(`gpu.sunshadow` holds the sun's passes and the spot tiles):

| build | `sm_enable` / `sm_spotEnable` | gpu.sunshadow | gpu.total |
|---|---|---|---|
| before | 0 / - | 0.0 | 74.1 |
| before | 1 / 0 | 46.1 | 124.4 |
| before | 1 / 1 | 58.5, 56.5 (two runs) | 133.1, 129.1 |
| after | 1 / 0 | 38.9 | 107.0 |
| after | 1 / 1 | 41.3 | 115.1 |

- Before, the held spots (lights 2 and 3) cost 10-12 ms over the sun
  alone: each tile redrew every static caster in its light's frustum
  every frame. After, 2.4 ms: a tile draws its world surfaces and static
  models into a depth cache of its own only when its light (or the
  casters, `r_spotLightSModelShadows`, the polygon offset) changes, and
  each frame copies the cache into the atlas and draws only the
  entities whose posed box its light's frustum reaches
  (`cod4_sunshadow::SpotStatic`, cod4-world.md 3.5). The F3 line counts
  the static redraws.
- The sun is the bigger share of the shadows' cost: 39-46 ms of
  lavapipe's 107-124, both partitions redrawn every frame from ~1.2M
  caster triangles (cod4-world.md 3.6). The two "sun only" runs differ
  by 7 ms on one build's code path, the runs' noise.
- A real card is not measured here.

## 10. The sun shadow's partitions (mp_crash's tiled room)

Section 9's runs put the sun's two partitions at 39-46 ms of lavapipe's
107-124 ms there. A partition's origin snaps to its texels
(`sunshadow::setup`), so while the view holds still its matrix,
viewport and cull planes hold still bit for bit. Each partition now
draws its world surfaces and static models into a depth cache of its
own only when one of those (or the casters) changes
(`cod4_sunshadow::SunStatic`), and each frame copies the cache into the
atlas and draws over it the entities whose posed box its planes keep.
The atlas is the same bit for bit (the sun shadow gate encodes a still
view twice and compares). Same setup as section 9 (`sm_enable` 1, both
spots held), GPU ms, mean:

| build | gpu.sunshadow | gpu.total |
|---|---|---|
| before (C4-93) | 39.4, 44.0 (two runs) | 109.9, 122.9 |
| after | 5.8, 6.8 | 74.6, 83.0 |

- `gpu.sunshadow` times the atlas passes only; a cache redraw runs
  ahead of them, outside it. `gpu.total` is the honest number: the held
  view now costs what `sm_enable` 0 did (74.1, section 9).
- A moving view redraws the caches on the frames the partitions move.
  On dm following a bot (12 bots, 30 s, noisy): `gpu.sunshadow` 36.9 ->
  25.0, `gpu.total` 335 -> 274.
- Done in section 11: retail's sun casters take each static
  model's LOD from the camera (`R_AddAllStaticModelSurfacesRangeSunShadow`: the rigid LOD ramp, `cullDist`), where vcod casts LOD 0 for
  every one. From the spawn views of mp_crash, mp_carentan and
  mp_crossfire the static models' triangles inside the partitions' planes
  are 83k, 82k and 47k at LOD 0 against 37k, 34k and 21k at the picked
  LOD (the world's 40k, 97k, 44k). That changes the map's texels toward
  retail's, and the caches would key on the picks.
- A real card is not measured here.

## 11. The shadow casters' static model LODs

Retail's sun and spot shadow passes draw each static model at the LOD
the camera's distance picks (cod4-world.md 3.6); vcod cast LOD 0. Now
the casters keep every LOD and pick each frame. From the spawn views the
static model caster triangles inside the partitions fall from 70k to
36k a view on mp_crash and from 78k to 29k on mp_bog. The texels move
toward retail's (far models' shadows from their coarse LODs). With the
caches of section 10 a held view redraws nothing either way, so the
gain shows on the frames a moving view redraws them. dm on mp_crash
following a bot (30 s, two runs each, before = C4-95): `gpu.sunshadow`
25.0, 31.1 -> 32.6, 36.2 ms and `gpu.total` 281, 311 -> 352, 369 ms,
but `gpu.scene`, which this does not touch, moved as much (250, 276 ->
314, 327) with other views (60-64k against 72-77k world triangles a
frame): these runs cannot see the change. CPU `encode` rose from
0.27-0.41 to 0.60-0.65 ms (a draw per picked static model run, the
picks each frame). A real card is not measured.

## 12. The opaque depth prepass (`r_vc_depthprepass`)

vcod's own dvar, default 1 (0 until the real-GPU run below; archived
until cod2-client-perf.md 12, now not, so a config's 0 is dropped);
nothing in retail. With it on the
frame opens with a depth-only pass (no colour target) over the opaque
lit world draws (`cod2_world.wgsl`'s lightmapped, static model and model
volume entries; water left out) and the first stage of every entity
surface whose stage is unblended, depth written and LessEqual. The scene
pass then loads that depth, and those draws (only those) take a
pipeline that tests EQUAL and writes no depth; every other draw, the
gun and the dlights keep their state. Draws that alpha test, carry a
polygon offset or blend never take it. Under a CoD2 feather the
entities stay out, since the feather's depth copy must hold the world
alone. `vs_main`'s position is `@invariant` in both shaders, so the two
passes' depths are the same bits.

- Pixels: `cod4_live_depth_prepass_draws_the_same_pixels` draws
  mp_crossfire's long view (5232 -888 240, yaw 225: 149759 triangles)
  and mp_crash's sunny spot with the MP5 and a body (124100), 640 x 384,
  4x MSAA, sun shadow on, off and on: byte-identical frames (719 and 451
  prepass draws). `pipeline_check`'s CoD4 frame draws it single-sampled
  without a validation error. VERIFIED (lavapipe, ours).
- Lavapipe, that test's back-to-back frames: mp_crossfire 70-79 ms off,
  77-87 on; mp_crash 85-90 off, 97-103 on. A loss of 10-15%. VERIFIED
  (lavapipe, ours).
- `tools/cod4_perf.sh`, mp_crossfire, 1280x772, 4x MSAA, lavapipe (the
  held view `setviewpos 5232 -888 180 225 0` on war with 4 bots, which
  gathers 65720 triangles; dm with 12 bots following one). GPU ms, mean.
  The merge gate held the machine for every run but the first, so the
  numbers are noisy; the direction matched in every pair. VERIFIED
  (lavapipe, ours):

| run | prepass draws | gpu.opaque | gpu.total | fps |
|---|---|---|---|---|
| war, held, off | 0 | 295 | 309 | 3.2 |
| war, held, on | 860 | 409 | 427 | 2.3 |
| dm, off | 0 | 348 | 386 | 2.5 |
| dm, on | 999 | 430 | 465 | 2.1 |

- Why lavapipe loses: it rasterizes on the CPU, tile by tile, and
  already rejects hidden fragments early within a draw order that is
  mostly front to back after the sort; the second pass doubles the
  vertex and binning work for the opaque set and saves little shading.
  INFERRED.
  A real card's early-Z and its shading cost are what the dvar is for.
- Real GPU (the 2026-10-27 playtest): `SESSION=1 tools/cod4_perf.sh
  /tmp/perf mp_crossfire 60` at 1920x1080 with `--exec-in-game "set
  r_vc_depthprepass 0"` and then `1`, one run each, sun shadows on in
  both. VERIFIED (real-card runs, ours):

| run | frames | fps | prepass draws | gpu.opaque mean / p50 / p95 / p99 | gpu.total mean / p99 |
|---|---|---|---|---|---|
| off | 3712 | 85.4 | 0 | 9.92 / 8.78 / 20.93 / 35.39 | 11.27 / 36.88 |
| on | 4073 | 93.0 | 1010 | 7.98 / 7.23 / 13.61 / 41.91 | 9.61 / 43.39 |

  The mean opaque pass drops 1.9 ms (19%) and p95 7.3 ms; the p99 rose,
  one run each, so it is noise until repeated. The default is 1 since
  then. On the playtest's eye the frames looked the same and the frame
  rate felt the same.

## 13. The world fx: feathered beams and distortion glass (C4-98)

A visible `zfeather` world or static model stage, or the world's
distortion glass, now splits the frame for its copies
(`renderer::cod4_worldfx`, cod4-world.md 9.5), where before a frame
with no effect sprite needing them drew in one pass.
`tools/cod4_perf.sh` (war, 4 bots, lavapipe, 1280x772, 4x MSAA, 25 s,
the first 8 dropped, `sv_cheats 1`), the view held by `setviewpos` on
mp_crash's lens flare (304 -701 199, yaw 125, pitch 20) and on
mp_overgrown's glass bricks (-406 -95 -108, yaw 163, pitch -53), each
with its dvar on and off. GPU ms, mean:

| view | dvar | gpu.opaque | gpu.split | gpu.blend | gpu.total |
|---|---|---|---|---|---|
| mp_crash flare | `r_zFeather` 1 | 164.8 | 0.02 | 29.4 | 206.7 |
| mp_crash flare | `r_zFeather` 0 | 178.9 | 0.01 | 0.01 | 191.1 |
| mp_overgrown glass | `r_distortion` 1 | 52.6 | 4.0 | 4.3 | 71.8 |
| mp_overgrown glass | `r_distortion` 0 | 63.9 | 0.02 | 0.01 | 76.8 |

- The 15 ms between the two mp_crash rows is the bots' noise, not the
  feather: section 14 measures it again without bots and finds none.
- The glass's copy is the 4 ms of `gpu.split` (the MSAA resolve into
  the scene copy); the frame total sits inside the noise.
- A real card is not measured here.

## 14. The feather's cost again, without bots (C4-102)

Section 13 read about 15 ms for `r_zFeather` at mp_crash's lens flare,
with 4 bots moving through the view. The same spot with no bots
(`BOTS=0`, war, lavapipe, 1280x772, 4x MSAA, 25 s, the first 8
dropped), two release builds of one tip, runs interleaved. Mean
`gpu.total`, ms:

| run | `r_zFeather` 1 | `r_zFeather` 0 | 0, split forced |
|---|---|---|---|
| set A | 184.4, 192.3, 182.8 | 177.9, 187.1, 174.7 | 179.1, 178.8 |
| set B | 185.3, 183.3, 184.8 | 189.9, 187.9, 191.9 | |

- The split itself costs nothing to see: forcing it with nothing
  feathered (a scratch build that set `soft_on`) reads as the unsplit
  frame, 179.0 against 179.9 (set A). The pass's end and restart around
  the soft depth copy is not where the time goes.
- With the feather on, set A reads 6.6 ms over off and set B 5.4 ms
  under it: the feather's per-fragment depth fetch is inside the runs'
  noise (about 5-10 ms between runs of one build here).
- A shader that skips the depth fetch for a fragment adding nothing
  (zero alpha or black) read 221.5 (an outlier), 184.9 and 188.6
  interleaved with set B's on column: no gain to see, so it is not
  kept.
- Nothing is cut: there is no cost to find at this spot on lavapipe.
  A real card is not measured.

## 15. Close to a car again, at 1920 x 1080 (C4-106)

The playtest of section 8 again (a real card, 1920 x 1080: 100+ fps,
about 20 by the damaged sedan's window with `r_distortion` 0 and Soften
Smoke Edges off, 11 with both on). `tools/cod4_perf.sh` (war, no bots,
lavapipe, 1920 x 1080, 4x MSAA, 30 s, the first 12 dropped), the view
held by `setviewpos -279 1396 233 -15 28` beside the red sedan at
-235 1394 230, and the car damaged through vcod-server's new
`--script-dir`: a copy of `maps/mp/mp_crash.gsc` that, 2 s in, notifies
the nearest car `damage` (45 to each window, cracking it; 260 to the
body, its white smoke; or 560, through its fire to the explosion). GPU
ms, mean (p95):

| build | car | dvars | gpu.total | draws | entdraws | particles |
|---|---|---|---|---|---|---|
| before | smoke, cracked | | 163.8 (179.6) | 1145 | 88 | 946 |
| before | smoke, cracked | distortion, feather 0 | 166.5 (181.7) | 1141 | 88 | 958 |
| parts | smoke, cracked | | 164.4 (176.3) | 970 | 55 | 939 |
| parts | smoke, cracked | distortion, feather 0 | 161.9 (176.3) | 985 | 55 | 997 |
| parts | intact | | 159.3 (172.0) | 958 | 55 | 920 |
| parts | smoke, view turned 180 | | 117.2 (191.2) | 866 | 55 | 934 |
| parts, cull | smoke, cracked | | 153.5 (168.5) | 605 | 55 | 278 |
| before | exploded | | 224.3 (424.9) | 1113 | 94 | 978 |
| parts | exploded | | 191.8 (231.9) | 959 | 64 | 971 |
| parts, cull | exploded | | 206.6 (349.0) | 590 | 64 | 295 |
| parts, cull | exploded | `fx_draw` 0 | 177.6 (201.6) | 584 | 64 | 285 |

"parts" hides the surfaces of the parts `hidePart` hid
(cod4-client-view.md 15); "cull" culls each effect element's spawn by
its `spawnRange` from the camera (cod4-effects.md 5). Two release builds
of one tip with the cull off read 164.4 and 168.8: lavapipe's noise
between runs is about 5 ms here, more on the exploded car, whose p95
swings with its debris.

- The four glass layers were two too many: the cracked car drew 33 more
  entity surfaces (88 against 55). On lavapipe they cost nothing to
  see; the glass is a plain lit blend (`mc_l_sm_b0c0s0`, sorts 38 and
  39) over a few hundred thousand pixels.
- The particles were the spawn cull's. Without a camera the fx system
  spawned every element of every effect, an effect's far LOD on top of
  its near one: 946 particles in view at that spot fall to 278, and the
  draws from 1145 to 605. The car's own smoke is a few of them
  (`smoke/car_damage_whitesmoke` is one sprite each 0.4 s play, 5 s
  long, about 12 at once); the rest is mp_crash's createfx dust, mist
  and smoke.
- Overdraw, the fx quads' area clipped to the screen summed (the live
  harness `cod4_worldfx_live`'s session at its renderer's aspect, the
  same spot, the exploded car a few seconds after its damage): 55 screens without the cull, 30 with it,
  of which `gfx_smk_white_atlas` 43 and 14; the same eye turned away 8
  and 2. Against the car the smoke covers the screen many times over,
  and each layer is a feathered, depth-reading sprite at 4 samples: the
  fill a real card spends, which lavapipe's per-pixel cost hides.
- The distortion pass and the feather read inside the noise here
  (`gpu.split` stays 0.02 ms: nothing at that spot copies the frame).
- `fx_draw` is 1.7's cheat dvar (registered at iw3mp.exe 0x49943a,
  "Toggles drawing of effects after processing"; VERIFIED), and vcod
  reads it now: 0 runs the effects and draws none, the split between the
  particles and the rest on a real card.
- Pending, on a real card: `SESSION=1 tools/cod4_perf.sh` at the spot
  (above's `SERVER_ARGS`, `--set sv_cheats=1`), the build before against
  this one, then `--exec-in-game "set fx_draw 0"`.


## 16. `bodies`, `events` and `net`: per-frame clones and lookups (P3)

Section 5's items 3 and 6. A live harness (vcod-server's dm on the map
with 12 shooting bots, one spectator `Cod4Client::pump`ed as `main`
runs a frame, lavapipe at 320x200, 25 s, the first 5 dropped, no GPU
work profiled; a scratch test, not committed) and a pose bench (the
spectator capture's bodies, each posed 12 times a frame, 60348 poses)
timed each step. CPU ms per frame, mean (p95), release build:

| stage | mp_crossfire before | after | mp_bloc before | after |
|---|---|---|---|---|
| net | 0.06 (0.11) | 0.03 (0.08) | 0.07 (0.11) | 0.04 (0.10) |
| events | 0.07 (0.12) | 0.02 (0.07) | 0.06 (0.17) | 0.03 (0.14) |
| bodies | 0.62 (1.94) | 0.45 (1.14) | 0.28 (0.40) | 0.32 (0.72) |

The pose bench: 9.43 us a body before, 7.34 after; its digest of every
skin matrix and weapon tag is the same in both. `bodies`' mean and p95
are the ragdolls' (section 7): a running ragdoll's step is 0.26-0.69 ms
a frame on the measuring machine against 13 us for a live body's pose,
so the two runs differ by how many corpses fell in their window, not by the cut.

- `net`: `live_frame` cloned the whole gamestate (configstrings and
  baselines, 45-60 us) every frame to hand the script menus the new
  server commands, built the demo archive row (cloning the predicted
  playerstate) on frames that send no packet, and built a default
  `PlayerState` (its HUD element arrays) for a cmd with a snapshot in
  hand. The menus now borrow the source's gamestate; the row is built
  only when a packet goes; the default only without a snapshot.
- `events`: `weapon_list` split configstring 7 into fresh `String`s
  about 7 times a frame (5 us each), and the HUD rebuilt the weapons'
  ammo slots off it as often. The split is kept per thread, keyed by
  the configstring's bytes, and handed out as one `Arc<[String]>`; the
  HUD keeps the slots while that `Arc` is the same one.
- `bodies`: per body per frame the character was cloned twice (the
  armed key and the rig map's entry), the rig hashed by it, every clip
  bound to the skeleton by bone names (`Skeleton::bind`, 4 us of a 14 us
  pose) and `tag_flash` / `tag_brass` found by a case-folded walk of the
  bones. Each body keeps its rig and the two tag bones while its models
  and held weapon stay, and the rig keeps its clips' bindings
  (`skeleton::BindCache`, by clip, the clip held so its address stays
  its own). CoD2's bodies (`cod2_players`, which also found `tag_flash`
  and `j_head` by name) and CoD1/UO's drawn bodies (`entities`, through
  `Assembly`) take the same cache; the servers' traces bind afresh
  (`PoseInputs::binds` `None`), as before.
- Left: the blend itself (8 us a body, `node_sums`' three per-bone
  vectors a node) and `scene`'s `ent_u32`, which looks a field up by
  name over the entity table, about 1500 times a frame (0.05 ms). Both
  are next if `bodies` or `scene` matter on the real card.

Output: the App-level digest gate (a local script: CoD2 retail
and bot demos, CoD1 1.1 and UO timedemos, every frame's view, models,
HUD, cues and effects hashed) matches the recorded base frame for frame.
Pending, on a real card: `SESSION=1 tools/cod4_perf.sh <out> mp_crossfire 30`
before and after (12 bots); expect `net` and `events` to halve and
`bodies`' p50 to fall by about a fifth.

## 17. The first frame after the join (C4-110)

The soak (cod4-map-sweep.md, "The soak", item 3) found one 600 to
1300 ms frame on the first draw after every join. `tools/cod4_soak.sh`'s
test (`cod4_soak`, war only, 18 s, six shooting bots, lavapipe at
320x240, `VCOD_FRAME_PROFILE`) on 737e33f7, with a scratch timer around
each step the first frames ran. CPU ms (the stages from `pace` to
`acquire`; lavapipe's `encode` and `submit` left out):

| | mp_crossfire before | after | mp_bloc before | after | mp_killhouse before | after |
|---|---|---|---|---|---|---|
| frame 0 | 763 | 3 | 738 | 2 | 728 | 3 |
| of it `net` | 670 | 0 | 650 | 0 | 637 | 0 |
| of it `events` | 91 | 2 | 87 | 2 | 90 | 2 |
| frame 1 | 550 | 0 | 436 | 0 | 569 | 0 |
| of it `bodies` | 522 | 0 | 433 | 0 | 567 | 0 |
| worst frame after | 140 | 11 | 9 | 12 | 24 | 10 |

What each was, mp_crossfire:

- `net`, 670 ms: not the gamestate. `pump` loaded the level's HUD and
  script menu zones (`Cod4Hud::load_under`, 610 to 650 ms) on its first
  call, and the first move's `compress_ops` built the CoD1 Huffman tree
  `MsgWriter` asks for and never uses (`parse::UNUSED_HUFF`, 30 to
  42 ms; `clmsg` held a second copy of it).
- `events`, 90 ms: the level's effects taken and their sprites and mark
  materials prewarmed (section 4's C4-104 prewarm), on the first frame.
- `bodies` and `pose`, 430 to 570 ms: each entity model's first upload
  (`Renderer::upload_dynamic_model`: the skins' images decoded and
  mipped, the vertex buffers): 3 to 280 ms a model, a body or a head the
  first time a player draws, a car the first time it does. The posing
  itself was under a millisecond.

The fix runs all three once the map has loaded and before the first
frame (`Cod4Client::prewarm`, from `App::cod4_join_frame`,
`App::cod4_level` and after a renderer rebuild; the soak's `rig` too):
the HUD zones and menus (`load_hud`), the effects
(`take_level_effects`), and every model in the level's entity model set
uploaded by name. `pump` still runs the first two, each a no-op once
done. The Huffman tree is built once with the gamestate
(`Conn::answer_gamestate` forces it) and `clmsg` reads `parse`'s. The
prewarm takes about 1.7 s on mp_crossfire (HUD 0.62, effects 0.09,
37 models 1.0), behind the connect screen on a join. Retail
registers the level's models in `CG_Init`, before its first frame
(INFERRED, the Q3 lineage's `CL_InitCGame`; not read out of
iw3mp.exe).

A model drawn with parts hidden (a shot car, keyed by its part bits)
still uploaded the first time that state drew: mp_crossfire's cars
took 16 to 208 ms on lavapipe before, the 140 above. Section 18 ends
that.

Gate: `play::cod4_sweep::cod4_live_join_builds_its_models_with_the_load`
(a live client on vcod-server's mp_crash beside two bots: once the rig
has prewarmed, the menus are loaded and nothing but a part-hidden model
or the stand-in box uploads in three seconds of play; fails without the
prewarm). CoD2 (`cod2_players::build_instances`) and CoD1/UO
(`entities`) upload their bodies on first use through their own paths;
this change does not touch them.

Pending, on a real card: join any CoD4 map through the browser;
the connect screen stays up about a second longer and the first frame
in game should not hitch.

## 18. A shot car's first damaged draw (C4-113)

Cause: `Cod4Client::handle` built each hidden-part state as a new
model. It cloned the xmodel less the surfaces whose `XSurface.partBits`
meet the entity's `partBits` (`hidePart`, cod4-client-view.md 15) and
sent the result through `Renderer::upload_dynamic_model`, keyed
`<name>#<partBits>`. So each state got its own vertex and index
buffers, its skins' textures and bind groups, and its own entity
materials. Section 17's prewarm uploads only the whole model, and
mp_crash's sedans start with the setup's ten later-state tags hidden
(`[0x242a, 0x92480000, 0, 0]`, `retail_sedans_draw_with_their_hidden_parts`),
so even an intact car's first draw uploaded in play. The same path
hides C4-106's glass pairs (section 15): one function serves both.

Fix: a hidden-part state is a view of the prewarmed upload.
`Renderer::dynamic_model_hiding` makes a new handle whose LOD 0
`DynModel` shares the whole model's buffers, bind groups and `EntSkin`s
(each `VmSurface` keeps its source surface index, `src`) and lists only
the kept surfaces. Its bone boxes come from the kept surfaces'
vertices, and its other LODs are the whole model's, as before. Nothing
goes to the GPU. The drawn surfaces, their order, their materials and
their index ranges match the old per-state upload's, so nothing drawn
changes.

Gate: `play::cod4_sweep::cod4_live_shot_car_uploads_nothing_in_play`.
A live client joins vcod-server's mp_crash war, `setviewpos`es 160
units off the nearest sedan and fires 150 ms bursts at its doors until
the car has shown four states. No dynamic model LOD uploads past the
prewarm's count (`Renderer::dynamic_uploads`), the stand-in box aside.
Lavapipe, 320 x 240, `pump`'s CPU ms in the frame each new state first
drew:

| state (`partBits[0..2]`) | before | after |
|---|---|---|
| `242a 92480000` (setup) | 0.4 | 0.3 |
| `242a 92500000` | 2.4 | 0.4 |
| `242a 92580000` | 2.6 | 0.6 |
| `242a 92590000` | 2.4 | 0.3 |

Before the fix the gate fails, with six states uploaded in play
(three sedan colours' setup states and the shot car's three). Here the
per-state cost is small, I think because section 17's prewarm had
already decoded the skins' images (`model_images`). Section 17's 16 to
208 ms were measured before that prewarm existed. Each state still
rebuilt its buffers, textures and materials.

Pending, on a real card: shoot a car on mp_crash. Each window
cracking or breaking should not hitch, and the glass should show the
same pair as before (cracked or broken, never both). Pending.

## 19. mp_crossfire's prepass draws fell from 1010 to 333 (P4)

A real-GPU run on bundle 0bcbf7dc (`SESSION=1`, mp_crossfire,
dm, 12 bots, 30 s, 1920 x 1080) read 78.0 fps, `gpu.opaque` 11.00 mean
(p95 29.49), `acquire` 5.47 (p95 21.5), 1587 draws, 238 entity draws
and 333 prepass draws. Section 12's "on" run read 93.0 fps and 1010
prepass draws.

### The range b4e4e170..0bcbf7dc

Release builds of b4e4e170 (prepass on by default), 4bf35188 and
0bcbf7dc, `tools/cod4_perf.sh` on lavapipe (1280 x 772, 4x MSAA, war,
no bots, `sv_cheats 1`, view held by `setviewpos 5232 -888 180 225 0`).
Per frame, the commonest value (every one of these frames gathers the
same 70670 triangles over 2282 soups and 23 models). VERIFIED (lavapipe,
ours):

| build | draws | entdraws | prepass | particles (mean) |
|---|---|---|---|---|
| b4e4e170 | 1854-1867 | 184 | 831 | 593 |
| 4bf35188 | 1253-1264 | 115 | 739 | 283 |
| 0bcbf7dc | 1258-1266 | 115 | 739 | 285 |

- The whole change is 4bf35188: a destructible car draws only its
  `partBits` state's surfaces (section 18, cod4-client-view.md 15),
  where it drew the intact and broken parts both, and the effects'
  `spawnRange` cull drops far emitters. The frame draws less; nothing
  after it moves a counter. VERIFIED (the runs above). With 4 bots the two ends read the same way
  (entdraws 213 against 144, prepass 866 against 774).
- Nothing in the range cuts a third of the prepass.

### What does: a feathered effect keeps the entities out

Section 20 (C4-114) since keeps a CoD4 frame's entities in the prepass under a feather; what follows is the state before it, and `feathered` now counts CoD2 frames only.

With `r_zFeather` 1 the entities stay out of the prepass on any frame
a feathered effect draws (section 12, `ent_prepass`): the feather's
depth copy holds the world alone. The prepass draws each entity surface
on its own (`encode_depth_prepass`; the scene pass joins runs, which
`entdraws` counts), so on mp_crossfire's 48-surface models the
entities are about two thirds of it. The profile now counts `feathered`
(1 on a frame whose entities were kept out). dm, 12 bots following
one, 40 s, lavapipe, 4x MSAA. VERIFIED (lavapipe, ours):

| build, `r_zFeather` | frames drawn | feathered | prepass mean | entdraws |
|---|---|---|---|---|
| this branch, 0 | 74 | 0 | 818 | 234 |
| this branch, 1 | 83 | 82 | 329 (988 on the one other) | 229 |
| b4e4e170, 1 | 49 | (no counter) | 279 | 258 |

- mp_crossfire's ambient smoke is in nearly every view, so with the
  feather on the prepass is the world alone, about 330 draws: the
  real-card run's 333. b4e4e170 reads the same, so this predates the range.
  VERIFIED (the runs above).
- So section 12's 1010-draw run had no feathered effect in view or
  `r_zFeather` 0, and today's had the feather on. Which, the profile
  cannot tell; its `feathered` column will from now on. INFERRED.
- 93 against 78 fps is then two runs of different length, view and
  feather state, not a code regression. Keeping the entities in the
  prepass under a feather means a world-only depth copy taken some
  other way (the alpha-tested world draws that sort before the models
  write depth in the main pass), a change to what the feather reads if
  done naively. Not done. INFERRED.

### Against retail: the opaque pass

A baseline on a real-card PC: retail 1.7 under Wine on
mp_crossfire at 1920 x 1080 runs 190-230 fps; vcod's `gpu.opaque` is
11 ms of a 12.5 ms GPU frame. What likely makes it dearer, ranked:

1. MSAA. `r_aaSamples` is registered at iw3mp.exe 0x62b1cb with a
   default and minimum of 1 (`ebx` 1 from 0x629df3) and a maximum of
   16. VERIFIED (the disassembly). vcod registered CoD4's at CoD1's 4,
   so a 1080p frame shaded, stored and resolved four samples a pixel
   where retail's default stores one. The default is now 1 on CoD4
   (`Shell::register_aa_samples`). `vcod_mp.cfg` archives
   `r_aaSamples`; an unstamped vcod config's 4 loads as 1 and any other
   value stays (cod11-console.md 8).
2. The feather and the prepass (above): with smoke in view the
   entities' ~650 surface draws shade without the prepass's early
   depth. Retail has its own `r_depthPrepass` ("Enable depth prepass
   (usually improves performance)", registered at 0x62b9b3, a bool,
   default 0, VERIFIED) and a "build floatz" pass; whether its floatz
   holds models is not read. Needs a real-card A/B of `r_zFeather` 0
   and 1 at one view. INFERRED.
3. Draw count: 1587 draws a frame, the entities' prepass unbatched.
   CPU and driver cost more than GPU; a real-card capture would show
   whether the GPU idles between them. INFERRED.
4. The big outdoor views the cull lets through (section 7 item 3,
   section 5 item 1): the slowest frames gather twice the mean's
   triangles. INFERRED.

Already as retail: the sun shadow's four point taps
(`cod4_sunshadow_recv.wgsl`, section 12's pixels), anisotropy at
`r_texFilterAnisoMin` 1 (`TextureFilter::cod4_aniso`), BC textures
uploaded compressed, no per-sample shading. INFERRED (the code).

`acquire` (5.47 mean, p95 21.5) is the GPU being behind with two
frames queued (section 7 item 4); it falls with the GPU frame.
INFERRED.

Pending, on a real card at one held view (`SERVER_ARGS="--set
sv_cheats=1"` and `--exec-in-game "setviewpos 5232 -888 180 225 0"`):
`r_aaSamples` 1 against 4 (each after `vid_restart`), then `r_zFeather`
0 against 1, reading `gpu.opaque`, fps and the `feathered` column.
Pending.

## 20. Soft smoke by the intact sedan, and `sv_cheats` on the client (C4-114)

The playtest (real card, 1920 x 1080, `setviewpos -279 1396 233 -15 28`
by mp_crash's intact red sedan): 15.5 fps by default, 30 with Soften
Smoke Edges and `r_distortion` off, 45 once the car is destroyed.
Retail 1.7 on the same PC draws that spot at 220-260 fps, with no smoke
or fire on the intact car (VERIFIED, a screenshot).

- No damage effect plays on an intact car in vcod. The fx quads at that
  spot (a scratch dump through `cod4_worldfx_live`'s live session, three
  frames 3 s apart) are mp_crash's createfx ambience and nothing else:
  `gfx_smk_white_atlas`, `gfx_mist`, the fog wisps, the lens flares,
  `gfx_fire_roar_pm`, their quads' centres 390 to 1290 units off the
  car, 0 to 0.67 screens of them. The nearest placement is
  `dust_wind_slow` 274 units off; the nearest smoke
  (`hallway_smoke_light`) is 566. VERIFIED (ours).
- Fragment counts. `VCOD_FRAG_STATS` (or `VCOD_FRAME_PROFILE`) has the
  renderer count the main pass's fragment shader invocations by draw
  kind (a pipeline statistics query per run of one kind,
  `frame_prof::FragStats`). The profile gets a `p <frame> world ents gun
  marks fx fxsoft dlight` line, `fxsoft` being the feathered effect
  runs. A fragment is a pixel's worth whatever the MSAA, so a count over
  the screen's pixels is its overdraw. `tools/cod4_perf.sh` (war, no
  bots, lavapipe, 1920 x 1052, 4x MSAA, 30 s, the first 12 dropped,
  `--team allies` and the playtest's `setviewpos`). Means:

| build | `r_zFeather` | gpu.total ms | world frags | ents | fx | fxsoft |
|---|---|---|---|---|---|---|
| before | 1 | 172.6 | 3118026 | 2514 | 820993 | 0 |
| after | 1 | 168.4 | 3118026 | 2514 | 481619 | 0 |
| before | 0 | 263.7 | 3118026 | 2514 | 383538 | 0 |
| after | 0 | 244.3 | 3118026 | 2514 | 190010 | 0 |

  The view holds still (the world's count is the same in every frame
  of every run). The frame is 1.5 screens of world and under half a
  screen of effects, and no feathered sprite draws at all. On
  lavapipe the feather costs nothing here, and the 90 ms between the
  `r_zFeather` rows is run-to-run noise (the slower runs ran while
  other builds held the machine). The fx counts move with the
  ambient particles' phase. This fill is far too small to explain
  15 fps on a real card, so the playtest's cost is not fill at this
  view. INFERRED.
- What changed in the frame: a CoD4 feather now copies its depth at
  the split, after the opaque entities and the gun, and keeps the
  entities in the depth prepass. 1.7's `floatZ` is drawn by the depth
  prepass over the lit and decal lists (`R_DepthPrepassCallback`,
  `TECHNIQUE_BUILD_FLOAT_Z`, INFERRED), so it holds the
  models too. Before, vcod copied before the first model, as CoD2's
  `floatZ` does (cod2-materials.md 9.5). That split the pass a second
  time (the MSAA colour and depth stored and loaded again) and left
  the entities out of the prepass in any frame with a feathered sprite
  in it. The copy is one full-screen pass per frame (sample 0 of the
  MSAA depth into an R32F linear target, `soft_particles.wgsl`); no
  draw copies, and no shader loops over samples.
- `fx_draw` and `r_glow` "cheat protected" under the server's
  `sv_cheats 1`: the CoD4 client never applied configstring 1, so its
  `sv_cheats` stayed at the ROM default 0 and every cheat dvar stayed
  locked. `setviewpos` worked because the server checks it. 1.7's
  `fx_draw` is a cheat dvar (iw3mp.exe 0x49943a, section 15), so the
  flag is right. Fix: `Cod4Session::take_systeminfo` hands out
  configstring 1 whenever it differs from the last one taken (the
  gamestate, or a later `d 1` from an rcon `sv_cheats`), and the frame
  runs it through `Shell::apply_systeminfo`, as `CL_SystemInfoChanged`
  (iw3mp.exe 0x473ab0) does: `sv_cheats` 0 puts every cheat dvar back,
  then every key is force-set. A demo sets none. INFERRED (CoD1's
  `CL_SystemInfoChanged`; I did not re-read 1.7's at that address).
  `play::cod4::cod4_live_cheat_dvars_follow_the_servers_sv_cheats` joins
  vcod-server started with `sv_cheats 1`, sets `fx_draw 0` and `r_glow 0`,
  sends `sv_cheats 0` to the server's console (the two go back to 1 and
  `fx_draw 0` is refused), then `sv_cheats 1` (it sets again). VERIFIED
  (ours).
- Pending, on a real card at the spot (`--set sv_cheats=1`):
  `VCOD_FRAG_STATS=1 SESSION=1 tools/cod4_perf.sh` and the `p` lines
  of its profile, with `r_zFeather` 1 and 0, then `fx_draw 0` (it is
  settable now). Where the `fxsoft` counts stay near 0 and the fps
  still doubles with the feather off, the cost is in the frame's
  structure (the split), not fill. Pending.

## 21. Draws, binds and the lit pixel on mp_crossfire (P5)

Retail 1.7 draws mp_crossfire at 190-230 fps on a real card at 1920
x 1080; vcod's real-GPU profile read `gpu.opaque` 11.0 ms of a 12.5 ms
GPU frame and about 1590 draws (section 19), at `r_aaSamples` 4 then.
Without that card, three questions: how many calls a frame makes and
what batching allows, what each call rebinds, and what a lit pixel does
against retail's technique for the same material.

### The census

`renderer::cod4_draws_live::cod4_live_crossfire_draw_census`: vcod-server's
war on mp_crossfire, us joined, the session's renderer at 1920 x 1080,
one sample, `r_vc_depthprepass` 1, the eye held at three views, each
frame drawn with `Renderer::draw_joins` off and on and held byte for
byte. `PassCensus` counts every draw call of the depth prepass and the
main scene pass by kind, and every `set_pipeline` and `set_bind_group`;
the profile has the last two as `pipelines` and `bindgroups`. The views:
`cod4_perf.sh`'s held one (5232 -888 240, yaw 225), the same spot turned
to 270, and one at the census median (3990 -1110 60, yaw 180). The first
two are `cull_census`' heaviest views (151k and 146k triangles, against
a mean of 34k and a median of 25k over 5880 spawn views). VERIFIED
(lavapipe, ours):

| view | tris | joins | draws | world | ents | fx | prepass world | prepass ents | pipelines | bind groups |
|---|---|---|---|---|---|---|---|---|---|---|
| 225 | 128090 | off | 2174 | 887 | 115 | 86 | 620 | 460 | 78 | 4273 |
| 225 | 128090 | on | 1104 | 887 | 65 | 86 | 34 | 26 | 73 | 1423 |
| 270 | 135516 | off | 2173 | 896 | 115 | 92 | 604 | 460 | 87 | 4269 |
| 270 | 135516 | on | 1121 | 896 | 65 | 92 | 36 | 26 | 81 | 1463 |
| median | 26010 | off | 967 | 171 | 115 | 93 | 122 | 460 | 41 | 1848 |
| median | 26010 | on | 305 | 171 | 10 | 93 | 21 | 4 | 37 | 402 |

Six sky draws each. Lavapipe's frame time does not move (104-141 ms a
frame either way): it is the fill, not the calls. A real card's CPU
`encode` and `submit` scale with the calls and binds (section 7); its
GPU front end may too.

What changed, all behind `draw_joins` (on by default):

1. The prepass's world draws join. The pass writes only the nearest
   depth (LessEqual, no colour), so its result does not depend on the
   draw order. Per set (the world or a moved brush model),
   sidedness and flag wave (`M_FLAG`, the one material switch
   `vs_main`'s position reads), the gathered ranges go in index order
   and the ones that touch draw as one. A material's batches sit side by
   side in the gathered buffer (`build_batches`' key leads with the
   material): 620 draws become 34.
2. The prepass's entity surfaces join. It drew each surface alone, 460
   for mp_crossfire's 23 cars and props (48-surface models); an
   instance's rigid surfaces (`EntStageDraw::rigid`: no deform, no
   flag) whose indices follow on draw as one, whatever their skins: 26.
3. The world draws rebind groups 1 and 2 only when the draw needs other
   ones (`WorldGroups`, cleared at each pipeline set). Batches of one
   material on several lightmaps, probes or primary lights keep the
   material's group.
4. The view skips an entity whose posed box misses the frustum
   (`instance_seen`): the box over every LOD's vertices, posed by the
   bones, grown by a unit and a hundredth of its reach for weights that
   sum short of 1. A deforming or waving instance always draws, and the
   shadow casters keep every instance. 115 entity draws become 65 at the
   heavy views and 10 at the median one.

What batching does not allow: the main pass's world draws are one a
batch, the batch keyed by material, lightmap, reflection probe and
primary light. The census over the 5880 spawn views reads a mean of 239
batches over 87 materials: one draw per material would take 2.7 times
fewer calls, but the lightmap page, the probe cube and the light's
uniform sit in group 2, so joining them means texture arrays, and the
draw order (material sort, then the sorted shader list) decides
coplanar pixels under LessEqual and EQUAL. 1.7's surface key carries the
same four fields: `GfxDrawSurfFields`' `reflectionProbeIndex`,
`primaryLightIndex`, `customIndex` (5 bits, the lightmap) and
`materialSortedIndex` (INFERRED), so its lit list
splits where ours does. The static models are baked into the world
batches already (section 6), so they draw merged across placements
where 1.7 draws them instanced. INFERRED.

The techsets' share of the drawn triangles over those views
(`vcod-cod4`'s `cull_techsets`, `CULL_TECHSETS=mp_crossfire`):
`mc_l_sm_r0c0n0s0` 25.7%, `wc_l_sm_r0c0n0s0` 23.8%, `mc_l_t0c0n0s0`
9.9%, `mc_l_sm_t0c0n0s0` 9.1%, `mc_l_sm_r0c0n0` 7.4%,
`wc_unlit_multiply` 7.2%. VERIFIED (ours, off the zone's materials).

### The lit pixel against retail's

Retail's pixel shaders for the two leading sets
(`--example techset_shaders`, `tools/re/d3d9dis.py`; the static model
set's techniques are in `common_mp`, mp_crossfire's zone holds a stub).
Technique 9 (sun and shadow map), `texld`/`texldl` counted along the
path a pixel takes. VERIFIED (bytecode):

- `lm_sm_sun_r0c0n0s0_sm3.hlsl` (world): the normal, specular, probe
  (`texldl`), primary lightmap, the secondary twice and the colour map,
  seven; the shadow map's four taps, eight in the band where the near
  partition fades into the far one, none where the primary lightmap
  reads 0. 19 fetches and 112 other instructions in the bytecode.
- `lp_sm_sun_r0c0n0s0_sm3.hlsl` (static model): the normal, colour,
  specular, the model lighting volume (`modelLightingSampler`, a volume
  texture, `dcl` 0xa0000000) and the probe, five; then four or eight
  taps. 17 fetches and 99 other instructions in the bytecode.

vcod's `cod4_lit` read nine maps where retail reads seven: the detail
map on every pixel whatever `M_DETAIL`, and CoD2's dynamic shadow
(`t_dyn_shadow`, a screen-sized texture no CoD4 map draws into); its
`fs_cod4_model` read the detail map too, and so did the entities'
`cod4_model` along with the normal map whatever the set's switch. Each
now reads a map only under the switch that uses it, and the CoD4 world
does not read the dynamic shadow: seven and four or eight, as retail.
The model path reads eight: the lighting volume is a 2D image of four z
slices (`model_volume`), two fetches where retail's volume texture takes
one. The shadow taps match (section 19). VERIFIED (the WGSL; the
`cod4_live` frames, all fourteen, are byte for byte the same before and
after under `VCOD_C4_SHOTS`).

Extra passes: with `r_vc_depthprepass` 1 vcod draws the opaque world and
entities twice; retail registers `r_depthPrepass` default 0 (section
19), and its `zprepass` technique (0) is `transform_only.hlsl` with
`null.hlsl`. VERIFIED (bytecode names). A real-card measurement
that made the prepass vcod's default was at 4x MSAA (section 12); at one
sample the trade may differ. INFERRED.

### What still needs a real card, ranked

1. `r_aaSamples` 1 against 4 at the held view (section 19, item 1),
   after `vid_restart`: the largest expected change, and the 11 ms was
   measured at 4.
2. `r_vc_depthprepass` 0 against 1 at one sample: the prepass now costs
   about 60 calls instead of about 1080, but it still rasterizes every
   opaque triangle twice.
3. `gpu.opaque` and `submit` before (bundle 0bcbf7dc) and after this
   change at the held view: the calls and binds fell by half and two
   thirds. If `gpu.opaque` moves, the card's front end was waiting on
   them.
4. A GPU capture (RenderDoc or Nsight) of one held frame: per-draw GPU
   time of the main pass, to tell fill from vertex work and from idle
   gaps between small draws (the 887 world draws average 145
   triangles).

Pending, on a real card: `SESSION=1 SERVER_ARGS="--set
sv_cheats=1" tools/cod4_perf.sh <out> mp_crossfire 30 --exec-in-game
"setviewpos 5232 -888 180 225 0"`, then the same with `--exec-in-game
"set r_vc_depthprepass 0"` added, reading `gpu.opaque`, `submit`, fps,
`draws`, `pipelines` and `bindgroups`. Pending.

## 22. World draws per material against 1.7's (P6)

Section 21 left one question: whether joining the main pass's world
batches per material (texture arrays for the lightmap, probe and light)
would bring vcod nearer what retail issues. It would go past it: 1.7
draws four times as many world calls as vcod at the held view.

### How 1.7 draws the lit world

- `r_pretess` ("Batch surfaces to reduce primitive count", name at
  iw3mp.exe 0x704a18, description 0x7049ec) registers at 0x62a245 as a
  bool with default `bl` = 1 (set at 0x629df3), handle `[0xd5697ec]`.
  VERIFIED (the disassembly).
- The camera walk (0x63c4d0) visits the lit surfaces in
  `sortedSurfIndex` order and starts a draw surface on each change of a
  surface's draw-surf key or every 128 surfaces (`cmp eax,0x80` at
  0x63c5ad). INFERRED (control flow).
- With `r_pretess` on (`[0xd5697ec]+0xc` read at 0x63c448), 0x63c220
  copies the run's indices into the pretess buffer and writes an entry
  per change of the surface's `firstVertex` (`GfxSurface`+4), lightmap
  (+0x14) or probe (+0x15); surfaces are 0x30 bytes
  (`lea *3; shl 4` at 0x63c25b). INFERRED (control flow).
- The lit pretess draw (0x655384 loop) sets the probe (0x6324a0) and
  lightmap (0x631eb0) on a change and the stream at `firstVertex * 0x2c`
  (0x6554b2) for every entry, one indexed draw each. INFERRED (control
  flow).

So 1.7 joins only surfaces that sit next to each other in
`sortedSurfIndex`, share a key and share a vertex base. vcod's
`build_batches` puts every soup of a key into one contiguous range at
load, with absolute u32 indices, so a frame draws one call per key
present whatever the order.

### The count

`vcod-cod4`'s `cull_draws` (`CULL_DRAWS=mp_crossfire`,
`CULL_DRAWS_VIEWS=...`): over the soups vcod's cell walk keeps, ours is
one draw per batch; 1.7's is the walk above run over the same visible
surfaces. Static model soups are counted apart (1.7 draws those on its
own path, not modelled). VERIFIED (ours; the 1.7 column is the model
above, so INFERRED for retail):

| view | ours world | ours, 1.7's lightmap key | 1.7 world (r_pretess 1) | 1.7, r_pretess 0 | ours static models |
|---|---|---|---|---|---|
| 5232 -888 240, yaw 225 | 818 | 699 | 3327 | 3389 | 215 |
| same, yaw 270 | 796 | 681 | 3493 | 3540 | 205 |
| 3990 -1110 60, yaw 180 (median) | 130 | 112 | 354 | 358 | 56 |
| mean of 5880 spawn views | 188 | 164 | 575 | 587 | 51 |

- mp_crossfire has 8646 surfaces behind 14920 `sortedSurfIndex`
  entries. 3848 sorted neighbours share a `firstVertex`, and in every
  run of one key the vertex base never changed, so with `r_pretess` 1.7
  makes one draw per draw surface. VERIFIED (the census).
- The 239 batches of section 21 are these 188 world and 51 static model
  batches.
- The one join left beside retail's key is the lightmap: vcod splits
  mp_crossfire's 2 lightmaps into 3 tiles (`mapworld::to_mapworld`), so
  the tile in the key adds 119 batches at the held view (818 against
  699). Taking that back means array textures for the pages and a layer
  in each vertex in a shader that CoD2 shares. The frames would hold
  byte for byte (layers sample as separate textures do), but it saves
  about 15% of the world calls at a view where vcod already issues a
  quarter of retail's. Not done. INFERRED.

### Draws before and after

No renderer change, so section 21's census stands: 1104 draws (887
world) at the held view and 305 (171 world) at the median one, with
`draw_joins` on. Its 887 world calls against this table's 818 batches
are the live frame's draw list (fx stages, moved brush models) against
the cull walk alone; I did not chase the difference. INFERRED.

On a real card: nothing new to measure from this item. Section 21's
real-card list still holds; if its item 3 shows `gpu.opaque` waiting on
calls, this section's tile join is the next one.

## 23. The resident set level by level, and the player rigs (C4-118)

CoD2's client built one rig per (character, held weapon) pair and each
baked its own body and head (`cod2-client-perf.md` 5.3). `Cod4Players`
keys its rigs the same way (`play::cod4_players`, the held weapon
inserted as the first attachment), and called plain
`PlayerRig::assemble`. Measured with the soak (`tools/cod4_soak.sh`,
one-minute levels, `VCOD_SWEEP_GAMETYPES=war,dm,sd,koth,sab` round
twice and once more, 12 levels a map, lavapipe), its `soak rss:` line
at each load, and a jemalloc heap profile at each load (`LD_PRELOAD`
jemalloc, `MALLOC_CONF=prof:true,lg_prof_sample:17`; the soak calls
`prof.dump` through `mallctl` when the process has one,
`cod4_sweep::jemalloc_dump`), read with `jeprof --base` level 2 against
level 12.

A rotation to another gametype on the same map keeps the map loaded
(`Cod4Session::take_map_change`), so `Cod4Players` and its rigs live on
over every level of one map; a change of map starts them over
(`Cod4Client::new_map`).

mp_crash, the live heap at a load (MB, jemalloc's sampled profile):

| | level 2 | level 7 | level 12 | growth 2 to 12 |
|---|---|---|---|---|
| everything, before | 1719 | | 1852 | 133 |
| everything, after | 1672 | 1736 | 1767 | 95.5 |
| player rigs, before | 16.5 | 21.3 | 21.3 | 4.8 |
| player rigs, after | 4.5 | 4.8 | 4.9 | 0.4 |
| test-only row log, before | 4.1 | 36.4 | 71.9 | 68 |
| test-only row log, after | | | 0.1 | 0 |
| sound bank (`SoundBank::get`) | 42 / 36.5 | 67 / 60 | 68.5 / 68.5 | 26 / 32 |

- The rigs do duplicate: four pairs' worth of body and head by level 2,
  21 MB once every pair the bots use has been seen (level 6), and then
  no growth. `Cod4Players` now takes its parts from one `RigParts`, kept
  as long as its rigs (one map's levels) (`PlayerRig::assemble_with`, the CoD2 fix), so a new pair
  costs its skeleton and the gun: 4.5 to 4.9 MB. Unlike CoD2 they were
  not the growth: they stop growing once a map's pairs are seen.
- The growth was a test-binary artifact: the audio system's `rows`
  (`#[cfg(test)]`, the row of each started voice for `AudioSystem::mix`)
  kept every voice ever started, about 7 MB a level, and the soak runs
  as a test. It now follows the live voices as `resumable` does; the
  shipped client never had it.
- The sound bank keeps every file it decoded, bounded by the install,
  as on CoD2 (left, same reason). The snapshot list
  (`net::parse::Client::snapshots`, shed at twice `KEEP_SNAPSHOTS`)
  reaches its capacity by level 5 and holds (49 MB of `Vec` growth
  inside the first five levels).

The resident set (glibc, `soak rss:` MB at each load; lavapipe holds
about 3 GB of it outside the heap):

| level | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| mp_backlot RSS before | 4425 | 4525 | 4562 | 4617 | 4660 | 4665 | 4528 | 2621 | 2684 | 2731 | 2733 |
| mp_backlot malloc before | 1751 | 1791 | 1789 | 1865 | 1878 | 1906 | 1938 | 1911 | 1981 | 2012 | 2052 |
| mp_backlot RSS after | 4426 | 4538 | 4547 | 4484 | 4506 | 4371 | 4382 | 4391 | 4395 | 4364 | 2645 |
| mp_backlot malloc after | 1788 | 1764 | 1818 | 1841 | 1893 | 1785 | 1843 | 1834 | 1889 | 1822 | 1846 |
| mp_crash RSS before (jemalloc) | 4908 | 4839 | 5073 | 5267 | 4982 | 4924 | 4887 | 4939 | 5007 | 5065 | 5290 |
| mp_crash RSS after (jemalloc) | 4863 | 4889 | 5021 | 5043 | 4904 | 5277 | 4807 | 4950 | 4992 | 4847 | 4778 |

malloc's live bytes on mp_backlot rose 301 MB from level 2 to 12
before, every few levels a new high, and 58 MB after, inside a 100 MB
band from level 3 on. The resident set's drops to about 2.6 GB are the
run's 10 GB memory cap (across the soak's process and its build)
pushing lavapipe's pages to swap, not the client; under jemalloc the
resident set wanders 4.8 to 5.3 GB either way. mp_strike's runs were
stopped to fit the available slots.

What is drawn does not change: the parts are the same `model::load`
output with the same hit boxes dropped, shared by `Rc`.
`a_body_under_two_weapons_shares_its_meshes` (the spectator capture's
walker under the AK-47 then the M16) holds that the two rigs share
every part but the gun, and that the second rig's pose (transform and
every model's skin matrices) equals that of the same rig built in a
session that never saw the first. CoD4 has no frame digest
(`frame_digest` covers CoD1 and CoD2).

The server's hit-pose rigs (`game::cod4::combat`) key on the character
without a weapon, so they hold one rig per body and head pair and were
left on `assemble`.

## 24. The frame's passes and targets against 1.7's (P9)

The question: whether vcod's CoD4 frame runs passes, copies, float
targets or per-frame rebuilds that 1.7 does not, enough to explain the
real card's 12.45 ms `gpu.total` against retail's 3.9-5.9 ms frame on
mp_crossfire at 1920 x 1080. It does not: the passes write about
16 MB a frame more than retail's, a few hundredths of a millisecond at
a desktop card's bandwidth. The gap is inside the opaque pass
(`gpu.opaque` 11.0 of 12.45, section 21's lit pixel), not around it.

### vcod's frame, measured

`renderer::cod4_passes_live::cod4_live_crossfire_frame_writes`:
vcod-server's war on mp_crossfire, us joined, the session's renderer at
1920 x 1080, one sample, CoD4's defaults (`r_vc_depthprepass` 1,
`sm_enable` 1, the map's vision set, `vc_hdr`, `vc_bloom`, `vc_ssao`,
`vc_shadows` and `vc_sunShafts` 0), the eye held at section 21's held
view and its median one. The renderer logs every pass, copy and
upload of the frame with the bytes it stores (`FrameWrite`, tests
only): a pass counts each attachment once, whatever its overdraw.
VERIFIED (lavapipe, ours), the held view (5232 -888 240, yaw 225):

| write | target | bytes |
|---|---|---|
| sun shadow atlas: the cached static layer blitted into each 1024 partition, the entities over it | Depth32Float 1536 x 2048 (two 1024 partitions, spot tiles beside) | 8388608 |
| spot shadow tiles, the same per tile | 512 x 512 Depth32Float a tile | 4194304 |
| depth prepass | Depth32Float 1920 x 1080 | 8294400 |
| scene pass, first half: sky, opaque, decals | Rgba8Unorm + Depth32Float | 16588800 |
| feather depth copy at the split | R32Float | 8294400 |
| scene pass, second half: blends, fx, gun, flare | Rgba8Unorm + Depth32Float | 16588800 |
| post pass (the vision set's film) into the gamma scene, then the 2D | Rgba8Unorm | 8294400 |
| gamma pass into the swapchain | the surface's sRGB format | 8294400 |
| uploads: camera, entity lights, instances and bones | | 25328 |
| total | | 78963440 |

The median view drew the same list without the spot tiles: 74769136
bytes. The static layers redrew on neither held frame (the sun's
partitions key on their texel-snapped matrix, section 10), and the
gathered world indices uploaded nothing: a frame that keeps last
frame's cell set keeps its index range. No glow (mp_crossfire's vision
set has none at these views), no depth of field (no sights), no
distortion copy. Every scene target is 8 bits a channel: `vc_hdr` 1
(Rgba16Float) is vcod's own and off by default.

### 1.7's frame

- The scene is the back buffer. `R_InitFrameBufferRenderTarget`
  (0x630650) asks for `D3DFMT_A8R8G8B8` (0x15 pushed at 0x63066d for
  "Requested frame buffer to be %s", 0x707b3c) and shares target 1 into
  target 2 (0x630660..0x630668: `R_RENDERTARGET_FRAME_BUFFER` into
  `R_RENDERTARGET_SCENE`), then makes
  one full-screen target of the format the device returned (image type
  9, 0x63070a: the resolved scene). VERIFIED (the constants); what the
  calls do is INFERRED (control flow).
- `R_InitRenderTargets` (0x630740) makes a full-screen `D3DFMT_R32F`
  target (0x72 pushed at 0x63078f) under the dvar at `[0xd5695c4]`
  (`r_floatz`, name 0x706320, registered at 0x62b92f), shares the
  resolved scene into two more ids (0x6307bb, 0x6307c8), makes the
  saved screen in the frame buffer's format, the sun's shadow map (1024,
  two partitions, 0x630822) and the spot's (512, four tiles, 0x630838).
  VERIFIED (the constants); the ids' meanings INFERRED. No floating-point colour target besides `floatZ`.
- No gamma pass: `r_gamma` goes to the device's gamma ramp. INFERRED
  (`R_SetColorMappings`). vcod's gamma pass writes
  the swapchain from the gamma scene (`crate::gamma`), one full-screen
  read and write retail does not make.
- With a feathered effect in view (`needsFloatZ`) 1.7 runs the depth
  prepass whatever `r_depthPrepass`: it draws the lit and decal lists
  into `floatZ` and the depth buffer, and at one sample the scene then
  clears only its colour and draws on that depth. INFERRED
  (`RB_Draw3DInternal` and `R_DepthPrepassCallback`). vcod's prepass writes depth only and copies the R32F depth
  at the split (section 20), one full-screen pass where 1.7 writes the
  same texels inside the prepass; the bytes match.
- The film and the depth of field run as a resolve (a copy of the back
  buffer into the resolved scene) and one full-screen pass back into
  the back buffer, the glow at a quarter size each way
  (`RB_ProcessPostEffects`, INFERRED). vcod's post pass reads
  the world target and writes the gamma scene: the same two
  full-screen writes.
- The present: `r_vsync` is registered at 0x62b20e (name 0x705a3c), a
  bool with flags 0x21 and default `bl`, which is 1 from 0x629df3 for
  the whole run. VERIFIED (the disassembly). vcod's `r_swapInterval`
  defaults to 1 (wgpu's `AutoVsync`), so the default is retail's and
  stays. One back buffer, `PresentationInterval` 1 or immediate by
  `r_vsync` (`R_SetD3DPresentParameters`, INFERRED); vcod
  asks wgpu for a frame latency of 2.

1.7's frame at the held view, the same targets counted once each:
8.4 MB sun map, 4.2 spot tiles, 8.3 depth, 8.3 `floatZ`, 8.3 colour,
8.3 resolve, 8.3 film: about 54 MB plus whatever the shadow casters'
overdraw writes (1.7 redraws every caster every frame; vcod blits a
cached static layer). vcod's 79 MB are those plus the gamma pass and
the split's second store of colour and depth, which a desktop card's
immediate renderer does not pay as a full write. INFERRED.

### What changed (no pixel moves)

- A single-sampled renderer no longer allocates the multisampled colour
  target it never draws into (`create_msaa_view` makes one texel): 8.3
  MB of video memory at 1920 x 1080, 16.6 under `vc_hdr`. No frame
  write changes.
- The post-process passes stop building bind groups a frame: the CoD4
  depth of field's six (`DofPass`, kept until a resize), the glow's
  and `blurWorld`'s one a step (`Cod2Glow`, kept by source and sky
  views), the distortion copy's single-sample blit (`SceneCopy`) and
  the post pass's glow group (`PostPass::bind_glow`, kept while it binds
  the same view). The main frame built none already: its two
  `create_buffer` calls only grow a buffer. CPU only.
- Not changed: the gamma pass (drawing the 2D straight into the
  swapchain means every HUD pipeline in the surface's format and an
  identity ramp only), the feather copy (folding it into the prepass
  as a colour target needs the alpha-tested and decal draws in the
  prepass too, or the copy holds less than it does now), the frame
  latency (the acquire wait is the GPU behind, section 7 item 4; a
  third frame in flight adds latency, not throughput).

The frames: every `renderer::cod4_` test run at one sample (the
`cod4_live` helpers and the five live tests built at 4 switched to 1 for
the run) before and after, under `VCOD_C4_SHOTS`: 36 frames byte for
byte the same, among them the depth of field (`dof_on`), the scope,
the distortion glass (the single-sample copy's blit), the feathered
beams and smoke, the spot tiles and both prepass pairs. 19 differ, all
from tests that join a live vcod-server; 16 of them differ between two
runs of the after build too, and the other three (`spot_*`) show the
held bot's legs in another phase of its idle. The session frames,
`cod4_passes_live`'s included, are no pixel gate. VERIFIED (lavapipe,
ours).

Pending, on a real card: the passes are not where the 7 ms
go. Check that the config holds `r_aaSamples 1` and `vc_hdr 0`
(either one doubles every scene write above), then section 21's list.

## 25. One-command sweep: a setting a pass at one view (P7)

The real card runs retail 1.7 on mp_crossfire at 190-230 fps and vcod
at 78, GPU-bound, and draws and texture fetches are not the gap
(sections 21, 22). The sweep measures what each optional piece of the
frame costs on that card, in one client session at one held view.

### The command

On the real card, from the checkout, after `cargo build --release -p
vcod -p vcod-server`:

```
SWEEP=1 SESSION=1 COD4_DIR=<install> SIZE=1920x1080 \
  tools/cod4_perf.sh /tmp/sweep mp_crossfire 10
```

`BOTS=12` adds the fight (12 shooting bots on war; the held view stays
put). It prints one tab-separated row a pass and writes it to
`/tmp/sweep/sweep.tsv`; `tools/cod4_perf.py --sweep /tmp/sweep`
prints it again from `profile.txt`. Run it twice: `base` and `base2`
bound the run-to-run drift.

`SIZE` is the client's window (`vcod --window`, no cvar written). Until
2026-10-29 it sized only the nested display, so a `SESSION=1` run took
each game's saved mode (the CoD4 sweep's 1920x1080 came from its own
cfg), and a nested run drew SIZE less kwin's 28-row title bar (1280x772
at the default). Now the nested display is SIZE plus 64 each way and the
window opens at SIZE in both modes; the profile's `# size` line, the
summary's header, names the size drawn.

### What it does

- vcod-server on `war`, no bots, `sv_cheats 1` (for `fx_draw`,
  `r_glow`, `r_fog` and `setviewpos`, which the client honours off the
  systeminfo, section 20), and the client joined as a spectator held by
  `setviewpos 5232 -888 180 225 0` (`VIEW=` another; only mp_crossfire
  has a default).
- At the join the client sets every swept dvar to its default and
  `r_aaSamples` to 1 (retail's default; a config may hold 4), then
  `vid_restart`s. Each pass sets its dvar, waits `SETTLE` s (default
  3), measures `<secs>` s, sets the dvar back. A `vid_restart` resets
  the view, so `aa4` sets it again. The queue is `vcod
  --exec-after "SECS LINE"`, each line run SECS after the last one ran
  (a `vid_restart`'s rebuild included); every line lands in the profile
  as `# exec <frame> <line>`, and the `echo sweep <pass> measure|end`
  lines bound a pass's frames.
- Setting an archived dvar rewrites `vcod_mp.cfg`; the script copies
  every `vcod_mp.cfg` under the install aside first and puts them back
  at exit.

| pass | sets | dvar |
|---|---|---|
| base, base2 | nothing | |
| aa4 | `r_aaSamples 4`, `vid_restart` | retail's (section 19) |
| prepass0 | `r_vc_depthprepass 0` | vcod's (section 12) |
| sunshadow0 | `sm_enable 0` | retail's |
| zfeather0 | `r_zFeather 0` | retail's |
| distortion0 | `r_distortion 0` | retail's |
| fxdraw0 | `fx_draw 0` | retail's, cheat |
| specular0 | `r_specular 0` | retail's |
| normalmap0 | `r_vc_normalMaps 0` | vcod's, new |
| fog0 | `r_fog 0` | retail's, cheat, new in vcod |
| glow0 | `r_glow 0` | retail's, cheat |
| dof0 | `r_dof_enable 0` | retail's |
| film0 | `r_vc_film 0` | vcod's, new |

The new switches, each 1 (the normal path) by default:

- `r_fog`: 1.7 registers it at 0x62a50c (`Dvar_RegisterNew` 0x56c350,
  the name at 0x704cb8, flags 0x80, `DVAR_CHEAT`). VERIFIED (iw3mp.exe).
  Its default 1 and that 0 draws no fog are INFERRED (`R_RegisterDvars`). vcod's 0 writes fog mode 0 into the camera
  block, so no world, model or effect shader fogs; the clear colour and
  the cull keep the fog's.
- `r_vc_normalMaps`: 0 clears `M_NORMAL` in every CoD4 lit material's
  uniform (`Cod2World::set_normal_maps`), so the set lights by its
  vertex normal and reads no normal map, as one without a map does.
  1.7 has its own `r_normalMap` (the name at 0x7050e4, pushed at
  0x62a86b). VERIFIED (iw3mp.exe). That one swaps in a flat map and
  still reads it. INFERRED (`R_RegisterDvars`). So it would
  not show what the read costs, and vcod does not register it.
- `r_vc_film`: 0 drops the vision set's film grade, so a frame with no
  glow and no depth of field runs no post pass (`gpu.post`'s film half).

### The columns

`fps` and `wall` are the pass's mean frame; `gpu.total` and the pass
groups (`gpu.sunshadow`, `gpu.opaque`, `gpu.split`, `gpu.blend`,
`gpu.post`) are section 1's. Then the finer groups (`frame_prof::Group`,
`q` lines): a timestamp wherever the work changes group, in the encoder
between passes and inside the main pass
(`TIMESTAMP_QUERY_INSIDE_PASSES`), each span charged to the group it
opened. Only groups some pass spent time in print:

- In `gpu.opaque`: `prepass` (the depth prepass), `sky` (the main
  pass's clear and the sky), `world` (world surfaces, and the static
  models' unlit and effect stages), `smodel` (the static models' lit
  sets, `fs_cod4_model`), `ents` (entity surfaces), `gun`, `marks`,
  `fx`, `fxsoft`, `dlight`.
- Between and after: `shadowmask`, `ssao`, `softcopy` (the feather's
  depth copy), `scenecopy` (distortion's colour copy), `dof`, `shafts`,
  `glow`, `bloom`, `postfx`, `shock`, `hud`, and `flare`.
- `draws` and `prepass` are the merged list's and the prepass's draw
  counts, `frags` the main pass's fragment shader invocations
  (pipeline statistics, `p` lines), `samples` the MSAA count.

Vulkan writes a timestamp at the bottom of the pipe, so a group's time
runs from the end of the last group's work to the end of its own;
draws that overlap on the card land in the later group. The main
pass's groups sum to about `gpu.opaque` plus `gpu.blend`. Every stamp
inside a pass costs a little; `VCOD_GPU_GROUPS=0` drops the finer
groups to compare `gpu.total` without them.

### Lavapipe, end to end (2026-10-28)

`SWEEP=1 SIZE=640x400 SETTLE=4 tools/cod4_perf.sh <dir> mp_crossfire 8`
on lavapipe (4 threads), nested kwin, no bots. The milliseconds are
CPU rasterization and rank nothing on a real card; the run shows the
columns are there and what each switch moves. VERIFIED (lavapipe,
ours):

| pass | frames | gpu.total | sunshadow | opaque | split | post | prepass | sky | world | smodel | ents | fxsoft | softcopy | postfx | draws | prepass draws | frags | samples |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| base | 149 | 50.7 | 6.6 | 39.2 | 0.49 | 0.88 | 15.9 | 7.9 | 1.7 | 12.5 | 3.2 | 0.24 | 0.59 | 0.68 | 1033 | 58 | 512k | 1 |
| aa4 | 32 | 256.2 | 6.5 | 237.0 | 0.65 | 0.84 | 25.2 | 15.0 | 11.8 | 116.0 | 78.0 | 0.93 | 0.83 | 0.62 | 1035 | 58 | 567k | 4 |
| prepass0 | 209 | 35.0 | 6.8 | 22.9 | 0.49 | 0.83 | 0 | 8.7 | 1.7 | 11.6 | 3.2 | 0.25 | 0.60 | 0.66 | 1036 | 0 | 584k | 1 |
| sunshadow0 | 178 | 42.2 | 0.01 | 37.4 | 0.50 | 0.83 | 15.2 | 7.3 | 1.6 | 12.1 | 3.2 | 0.26 | 0.56 | 0.65 | 1035 | 58 | 536k | 1 |
| zfeather0 | 165 | 45.7 | 5.7 | 38.1 | 0.02 | 0.76 | 15.0 | 7.2 | 1.3 | 11.8 | 2.4 | 0 | 0 | 0.61 | 1032 | 58 | 564k | 1 |
| fxdraw0 | 164 | 46.0 | 6.3 | 37.6 | 0.02 | 0.85 | 15.1 | 7.8 | 1.5 | 10.7 | 2.5 | 0 | 0 | 0.68 | 917 | 58 | 391k | 1 |
| film0 | 161 | 46.7 | 5.8 | 37.0 | 0.45 | 0.01 | 14.9 | 7.2 | 1.6 | 12.2 | 3.0 | 0.07 | 0.53 | 0 | 1036 | 58 | 548k | 1 |
| base2 | 146 | 51.5 | 6.9 | 38.9 | 0.61 | 0.97 | 15.8 | 7.8 | 2.0 | 12.1 | 3.4 | 0.31 | 0.78 | 0.72 | 1032 | 58 | 546k | 1 |

(`samples` from a second run of `base aa4 base2`, after the `# size`
line learnt to note a sample change; the first printed the start-up
config's 4 on every row.)

- What each switch moves: `aa4` the samples and every in-pass group
  (lavapipe shades per sample); `prepass0` the prepass draws and group;
  `sunshadow0` `gpu.sunshadow`; `zfeather0` the split, the depth copy
  and the soft effects (they draw as plain `fx`); `fxdraw0` 116 draws
  and a quarter of the fragments; `film0` the post pass.
- `distortion0`, `glow0` and `dof0` move nothing at this view: no
  `scenecopy`, `glow` or `dof` group ran in any pass, so it holds no
  distortion glass or sprite, the vision set has no glow, and the
  depth of field needs a sighted weapon. `specular0`, `normalmap0` and
  `fog0` change no counter; they change the pixels:
  `cod4_live_sweep_switches_change_the_frame` draws the held view
  (640 x 384, 4x MSAA) under each, `fog0` under a stand-in exp fog (the
  test has no session to hand the level's over): `specular0` moves
  134543 pixels, `normalmap0` 136553, `fog0` 85858, `film0` 245311, and
  the defaults draw the first frame again byte for byte. VERIFIED
  (lavapipe, ours).
- `base` against `base2`: `gpu.total` 50.7 and 51.5, the drift between
  passes here.
- What lavapipe cannot say is which of these the real card pays for;
  that is a real-card run of the command above.

## 26. The opaque shaders' arithmetic against 1.7's (P8)

Sections 21 and 22 left draws and fetches matching retail or ahead of
it. This section counts what a lit pixel and vertex compute.

### The counters

- Retail: `tools/re/d3d9dis.py --count` over the technique passes
  `--example techset_shaders` writes (`wc_l_sm_r0c0n0s0` out of
  mp_crossfire, the `mc_*` sets out of `common_mp`). It counts the
  instructions (no `dcl`, `def` or comment), the fetches, the flow
  control, and slots with the ps/vs 3.0 macros expanded (`nrm`, `pow` 3,
  `lrp` 2, `m4x4` 4, `sincos` 8). Each branch's both arms are in the
  bytecode, so it is the shader's size, not one pixel's path.
- vcod: `renderer::shader_ops` (`SHADER_OPS=1 cargo test -p vcod --bins
  shader_ops -- --nocapture`; `SHADER_OPS=2` adds each function's
  share). It writes a material's uniform switches into the WGSL as
  literals, parses it with naga and walks the IR from the entry point:
  calls inlined, a known branch takes one arm, an unknown one both (as
  the bytecode counts), a counted loop its trips, constants folded.
  `slots` weights naga's expressions as ps_3_0 expands the same macros.
  It does not remove dead code the driver would (`surface`'s CoD2
  normal on a CoD4 path, about 8 slots).

### The table

Technique 9 is `lm_sm_sun_*` / `lp_sm_sun_*` (sun and shadow map), 7 no
primary light, 8 the sun without the map, 11 a spot with its map.
The `(+n)` are the `textureNumLevels` and `textureDimensions` queries,
left out of vcod's slots. VERIFIED (retail: bytecode; vcod: the counter,
before and after this section's changes):

| path | retail ps instr / fetch / flow / slots | retail vs slots | vcod fs fetch / alu / slots before | vcod after |
|---|---|---|---|---|
| `wc_l_sm_r0c0n0s0` t9 | 131 / 19 / 11 / 140 | 37 | 19 (+3 size queries) / 202 / 244 | 19 / 152 / 191 |
| `wc_l_sm_r0c0n0s0` t7 | 41 / 6 / 0 / 48 | 33 | 7 (+3) / 85 / 101 | 7 / 74 / 87 |
| `wc_l_sm_r0c0n0s0` t8 | 57 / 7 / 0 / 64 | 33 | 7 (+3) / 85 / 101 | 7 / 74 / 87 |
| `wc_l_sm_r0c0n0s0` t11 | 89 / 12 / 0 / 99 | 37 | 11 (+3) / 170 / 206 | 11 / 159 / 192 |
| `mc_l_sm_r0c0n0s0` t9 | 116 / 17 / 8 / 125 | 47 | 18 (+4) / 201 / 243 | 18 (+1) / 151 / 190 |
| `mc_l_sm_r0c0n0s0` t11 | 78 / 10 / 0 / 88 | 47 | - | 10 (+1) / 159 / 192 |
| `mc_l_sm_r0c0n0` t9 | 92 / 15 / 8 / 96 | 46 | 16 (+3) / 173 / 209 | 16 (+1) / 123 / 156 |
| `mc_l_t0c0n0s0` t8 | 53 / 5 / 0 / 61 | 43 | 6 (+4) / 85 / 101 | 6 (+1) / 74 / 87 |
| world `vs_main` (CoD4 set) | | | 0 / 13 / 17 | 0 / 12 / 15 (fog included) |
| entity `lp_sm_sun` (`dynamic_model` `fs_main`) | as `mc_*` t9 | | one fetch and one size query more (item 5) | 16 / 368 / 408 |
| entity `vs_main` | | | 108 alu | 69 alu / 73 slots |

vcod's vertex stage reads float positions and frames where retail
decodes packed normals (`mad v3, c8.xxxy, c8.zzzw` and the `cross` for
the binormal), so its 15 slots against 33-47 are not a gap.

### What changed

1. Size queries. `sample_image` asked `textureNumLevels` on every
   fetch, three or four a lit pixel (colour, normal, specular, detail);
   a size query issues through the texture unit on most cards. The
   material uniform's free `flags.w` now carries which of its maps have
   mipmaps (`MIP_*`, read off the view's texture at bind), and the
   world shader calls `sample_mipped`. Same arm, same pixels. VERIFIED
   (the frames).
2. The sun shadow taps. `cod4_sun_pcf` scaled each tap's `at + tap` to
   texels and summed four selects; it now scales `at` once, adds the
   taps in texels and compares a `vec4` of the four depths, as retail's
   `add`, `cmp`, `dp4 0.25` do. The taps are 2^-k of the atlas and its
   size 2^k texels, so the sum rounds the same, and the 0/1 sum is
   exact: same pixels. 135 slots for the three calls became 96. VERIFIED
   (the frames).
3. The fog per vertex, a retail mismatch. Every fogged 1.7 technique
   computes `saturate(exp2((d * fogConsts.z + fogConsts.w) * log2 e))`
   in the vertex shader (`exp o3.w_sat` in `lm_sm_sun_s_tc0n0_sm3.hlsl`
   and `lp_sm_sun_s_tc0n0_dtex_sm3.hlsl`; 11.1 in cod4-world.md) and the
   pixel shader lerps by the interpolated value. vcod took the length
   and the exp per pixel. A CoD4 set's `vs_main` now writes it
   (`VsOut::fog`) and `cod4_lit` / `fs_cod4_model` lerp by it
   (`finish_fog`); CoD2 and the water keep theirs. The fogged frames
   move by 1-3 levels on up to a few thousand pixels of 640 x 384.
   VERIFIED (bytecode; the frames).
4. The normal mapped normal, a retail mismatch in the last bits. 1.7
   takes `N + x T + y B` off the vertex's frame (`mov r1, v2; mad r1,
   r0.x, v4, r1; mad r1, r0.y, v3, r1; nrm`). vcod's vertex shader sent
   CoD2's `N - T - B, 2T, 2B` and `cod4_normal` rebuilt `N` from them;
   a CoD4 set's vertex now sends N, T and B as they are (and skips the
   half angle no CoD4 path reads), and the pixel does retail's two
   `mad`s. Frames move by 1-2 levels on a few pixels. VERIFIED (bytecode;
   the frames).
5. Entities: `fs_main` fetched CoD2's dynamic shadow (and asked its
   size) for every probe stage, the CoD4 ones that never read it; and
   `vs_main` ran CoD2's probe grid per vertex for a CoD4 set, whose
   per-pixel `cod4_model` ignores the vertex light. Both now run only
   where read. Same pixels. VERIFIED (the frames).

The frames: `VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live`
(92 tests, 58 frames). Twenty of them (the live-session ones: the
crossfire census, bricks, glow, haze, heli, saw firing, scope hip, the
spots, tracer) differ between two runs of one build, so a change is
judged on the other 38, which held byte for byte across items 1, 2 and
5, and moved only as items 3 and 4 say.

### What is left, ranked by slots

1. (Done in section 27.) The entity's lighting block: `cod4_block` reads its eight corners
   out of the light set's storage buffer, each `cod4_texel` an index,
   a five-way switch and an unpack, 225 of the 408 slots, where 1.7
   does one `texld` of `modelLightingSampler`, a volume texture
   (section 21). The light set is passed by value (41 vec4s), and the
   dynamic indexing into a copy may sit in scratch memory on some
   drivers. INFERRED. Fixing it means the blocks in a 3D texture (a
   per-frame atlas, hardware filtered as retail is, so pixels move).
2. The shadow taps: 32 slots a call against retail's 14 (four
   `texldl` on a point sampler, its addressing clamping). vcod's taps are
   `textureLoad` with `floor`, an int conversion and a clamp to the
   partition each. A non-filtering sampler would take that on the
   sampler but round at the texel edges as the hardware does and clamp
   at the atlas, not the partition: pixels would move. Done in
   section 28.
3. One pipeline for techniques 7 to 13: `fs_main` and `fs_cod4_model`
   branch on uniforms (the light kind, the shadow map, the switches)
   where 1.7 compiles a shader a technique, so the driver allocates
   registers for the heaviest arm (the spot's four tiles, three shadow
   calls) on every lit pixel. Uniform branches cost little; the register
   count may cost occupancy. INFERRED, and only a GPU profiler's
   register and occupancy readout can rank it. Override constants per
   light kind would split it.
4. Precision: 1.7's lit shaders are `_pp` throughout, vcod's f32.
   Modern drivers mostly run D3D9 `_pp` at full precision. INFERRED.

Pending, on a real card: the section 21 command at the held
view (`SESSION=1 SERVER_ARGS="--set sv_cheats=1" tools/cod4_perf.sh
<out> mp_crossfire 30 --exec-in-game "setviewpos 5232 -888 180 225 0"`)
on this branch against its base, reading `gpu.opaque`; and, if one is to
hand, a Radeon GPU Profiler or Nsight capture of one frame, for the
register count and occupancy of the `cod2 lit pipeline` draws (item 3).

## 27. The model lighting block in one volume fetch (P11)

Section 26's first item. 1.7 reads a model's lighting block with one
`texld` of `modelLightingSampler` (s4, `dcl_volume`): `texld r1_pp, r1,
s4` in `mc_l_sm_r0c0n0s0` technique 9's pixel shader, at the block
centre `v7` plus the normal over its longest axis times
`lightingLookupScale` (`mul r1.xyz, r1, c5; mad r1.xyz_pp, r1, r0.y, v7`).
VERIFIED (bytecode, `tools/re/d3d9dis.py`). The image is one
256 x height x 4 A8R8G8B8 volume (`CreateVolumeTexture` at 0x62f6dd,
cod4-world.md 3.2). VERIFIED.

### What changed

- Entities and the view model: the frame's blocks go into a 256 x 64 x 4
  RGBA volume (`entity_material::Cod4Volume`, group 3 binding 6), block
  `i` for light set `i`, the view model's the last; the light set carries
  the block's centre instead of its 64 texels, and `cod4_block` is one
  `textureSampleLevel` at the centre plus `q` times (1.5/256, 1.5/64,
  0.375). Before, `cod4_block` gathered eight corners out of the light
  set, each an index, a five-way switch and an unpack.
- Static models: the volume is a 3D texture (group 2 binding 8) and
  `model_volume` one fetch, where it took two 2D fetches of the slices
  stacked down one image and a `mix`. The lit world pass then binds 17
  images, past WebGPU's default 16, so `renderer::device_limits` asks for
  the adapter's `max_sampled_textures_per_shader_stage`.

The hardware filter weights at the sampler's fixed-point precision where
the shader blended in f32, so pixels move by up to 2 levels on lit
models. Both paths stay inside their gates against the HLSL worked on
the CPU (`cod4_entity_set_lights_a_normal_mapped_sphere_as_the_hlsl_does`,
2 levels; `cod4_model_shades_its_volume_block_as_the_hlsl_does`, 3).
VERIFIED (lavapipe).

### The counters

`SHADER_OPS=1`, before and after (tex / alu / slots); retail from
section 26's table. VERIFIED:

| path | retail slots | before | after |
|---|---|---|---|
| entity `lp_sm_sun` (t9) `fs_main` | 125 (`mc_*` t9) | 16 / 368 / 408 | 17 / 147 / 185 |
| entity `lp_sm_spot` (t11) `fs_main` | 88 | 8 / 406 / 440 | 9 / 185 / 217 |
| `mc_l_sm_r0c0n0s0` t9 `fs_cod4_model` | 125 | 18 / 151 / 190 | 17 / 139 / 176 |
| `mc_l_sm_r0c0n0s0` t11 | 88 | 10 / 159 / 192 | 9 / 147 / 178 |
| `mc_l_sm_r0c0n0` t9 | 96 | 16 / 123 / 156 | 15 / 111 / 142 |
| `mc_l_t0c0n0s0` t8 | 61 | 6 / 74 / 87 | 5 / 62 / 73 |

The static model paths keep their one `textureDimensions` (the volume's
height varies by map). The entity's 185 against the static model's 176
is `cod2_probe_sided`'s prologue and the light set read through the
storage buffer.

### The frames

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (94 tests) on
this branch's base and tip: 8 of 66 frames held byte for byte, and
every frame outside section 26's live-session list moved by at most 4
levels, on the lit models (`crash_lit_sun`: 20833 pixels of 640 x 384,
at most 2, on the body, the gun and the static models; the `sweep_*`
frames at most 4). The live-session frames differ between runs of one
build (section 26), so they say nothing here. VERIFIED.

Pending, on a real card: section 26's held-view command on this
branch against its base, reading `gpu.opaque`; and a look at a player
and the view weapon in sun and shade, which should read as before.

## 28. The sun shadow taps in a comparison sampler (P12)

Section 26 left `cod4_sun_pcf` at 32 slots a call against 1.7's 14.

### Retail's taps

`wc_l_sm_r0c0n0s0` technique 9's pixel shader (mp_crossfire,
`--example techset_shaders`, `tools/re/d3d9dis.py`) runs each call as
four `add` of a tap to `s`, four `texldl` of `shadowmapSamplerSun` (s4,
`dcl_2d`), one `add` of `-s.z` to the four reds, one `cmp` to 1 or 0
and a `dp4` with 0.25: 11 instructions and four `mov`s gathering the
reds, 14 slots. The taps are `def c8` (1/2048, -1/8192) and `def c7.z`
(1/4096) both ways, (-0.5, 0.25), (0.25, 0.5), (-0.25, -0.5) and (0.5,
-0.25) texels of the 1024 x 2048 map. The compare is ALU work: the
sampler only filters (point) and addresses (clamp). VERIFIED
(bytecode). Its point filter and clamp are section 26's reading of the
technique's sampler state. INFERRED.

### What changed

`cod4_sun_pcf` now takes four `textureSampleCompareLevel` through a
`sampler_comparison` (camera group binding 20,
`cod4_sunshadow::receive_sampler`: nearest, clamp to edge,
`LessEqual`, so a tap is 1 where the stored depth is at or past `s.z`)
at `s.xy * (2/3, 1)` plus the taps in atlas uv (the partitions are the
left 1024 of the 1536 wide atlas), and a `dot` with 0.25. The compare
moves into the sampler, which Vulkan, D3D12 and Metal all have; 1.7's
D3D9 path did it with `cmp`. `textureSampleCompareLevel`'s `offset`
takes whole texels, and retail's taps are fractions of one, so they
ride the coordinate. The spot taps (`cod4_spot_shadow`) are unchanged:
their clamp to the light's 512 tile keeps four lights' tiles apart in
one column.

Slots, `renderer::shader_ops` (`SHADER_OPS=2`), before and after.
VERIFIED (the counter):

| path | retail slots | vcod before | vcod after |
|---|---|---|---|
| `cod4_sun_pcf`, one call | 14 | 32 | 10 |
| `wc_l_sm_r0c0n0s0` t9 `fs_main` | 140 | 191 | 125 |
| `mc_l_sm_r0c0n0s0` t9 `fs_cod4_model` | 125 | 190 | 124 |
| `mc_l_sm_r0c0n0` t9 `fs_cod4_model` | 96 | 156 | 90 |
| entity `lp_sm_sun` t9 `fs_main` | | 408 | 342 |

Over section 27's volume fetch (both merged) the t9 rows read
`mc_l_sm_r0c0n0s0` 110, `mc_l_sm_r0c0n0` 76 and the entity's
`lp_sm_sun` 119 slots, each 66 under section 27's own after. VERIFIED
(the counter).

vcod's 10 is under retail's 14 because the compare and the reds'
gathering cost no slot here; a card that has no compare in its sampler
unit (none current) would spend them in the driver's code. INFERRED.

### The pixels

Two changes of rounding, no change of rule:

- A tap's texel was `floor((at + tap) * 1024)` in f32; the sampler
  takes `floor(uv * 1536)` at the hardware's sub-texel precision, with
  `uv = at * 2/3 + tap * 2/3`. A lookup on a texel edge can land a
  texel over.
- The clamp: `textureLoad` kept x at 1023, the sun's edge, as retail's
  own 1024 wide texture clamps; the sampler clamps at the atlas's 1535,
  so a tap past the sun's right edge reads the spot column. Only the
  far partition's edge fade reaches there (edge 7 to 8), where the map's
  weight goes to 0.

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins -- cod4` (lavapipe),
before and after, 69 frames: 41 byte for byte, the deterministic ones
that moved (`mp_crash_spawn`, `mp_crash_prepass_*`, `crash_lit_*`,
`saw_up`) by 1 pixel each, and the rest are the live sessions section 26
found differ between two runs of one build (the spot frames in the
tiled room among them: two runs of this build differ by 15-19k pixels
there). With the fly test's views held for the comparison: mp_crash's
tiled room (37 -739 245, yaw 67.5, and the reverse) did not move; an
outdoor wall with a plant's and a roof's sun shadow (the first
`mp_tdm_spawn` at 20 up, turned 90) moved 38 pixels, one column of
them, by a texel step along a shadow edge. VERIFIED (the frames).

Pending, on a real card: section 26's held-view command on this
branch against its base, reading `gpu.opaque`.

## 29. One lit pipeline per path, as 1.7 compiles a technique (P10)

Section 26's item 3: `fs_main` and `fs_cod4_model` (cod2_world.wgsl)
served techniques 7 to 13 with every material switch, the alpha test and
the light kind read off uniforms, one pipeline per variant slot and
sidedness; 1.7 compiles one shader per technique and material
combination (section 26's bytecode, `wc_l_sm_r0c0n0s0` t7, t8, t9 and
t11 four shaders).

### The change

- cod2_world.wgsl reads the switches through `switches()`,
  `alpha_test()` and `light_kind()`: with the override `LIT_SPLIT` they
  return the pipeline constants `LIT_FLAGS`, `LIT_TEST` and `LIT_LIGHT`,
  without it `mat.flags.x`, `mat.flags.y` and `sun.src_col.w` as before.
  `vs_main` reads no constant, so the depth prepass keeps one vertex
  stage and its `@invariant` depths.
- A CoD4 world or model set (`LitEntry::Main` or `Cod4Model` with
  `PhongLit::cod4`) takes a pipeline keyed by `renderer::cod2::LitPath`:
  its material's `M_*` and `AT_*` and its group's light kind (none, sun,
  omni, spot). CoD2, static props and water keep the unsplit shader.
  The prepass's EQUAL pipelines split the same way.
- The load collects every key its binds ask for and compiles them at
  `Cod2Builder::finish_keeping`, eight at a time on their own threads,
  before the first frame (the C4-110 and C2-MR prewarm's shape). Each
  is kept in `LitShared` across loads.
- `r_vc_normalMaps 0` takes the path without `M_NORMAL`; its pipelines
  are made on the first switch off (a sweep switch, not a gameplay one).
- A lit pipeline bound after another keeps the groups and the tangent
  buffer bound (same layout).
- `VCOD_LIT_SPLIT=0` at load keeps the one shader, to measure against.

### Counts

`renderer::shader_ops` (`SHADER_OPS=1`): what a path runs is unchanged
(section 26's paths with section 27's one volume fetch and section
28's sun shadow taps; `split_pipelines_run_their_path_alone` holds it
for every row). What a pipeline's fragment stage compiles,
every arm the walk cannot fold counted ("static", the size the driver
allocates for), the frame's switches (`r_specular`, the fog mode, the
sun shadow map drawn, `flags.w`'s mipmaps) left unknown in both.
VERIFIED (counter):

| path | retail slots | runs (slots / fetch) | compiled before: fetch / slots / branches | compiled split |
|---|---|---|---|---|
| `wc_l_sm_r0c0n0s0` t9 | 140 | 125 / 19 | 61 / 345 / 83 (`fs_main`) | 40 / 145 / 27 |
| `wc_l_sm_r0c0n0s0` t7 | 48 | 87 / 7 | same | 28 / 106 / 22 |
| `wc_l_sm_r0c0n0s0` t8 | 64 | 87 / 7 | same | 28 / 106 / 22 |
| `wc_l_sm_r0c0n0s0` t11 | 99 | 192 / 11 | same | 32 / 211 / 26 |
| `mc_l_sm_r0c0n0s0` t9 | 125 | 110 / 17 | 44 / 272 / 63 (`fs_cod4_model`) | 38 / 130 / 26 |
| `mc_l_sm_r0c0n0s0` t11 | 88 | 178 / 9 | same | 30 / 197 / 26 |
| `mc_l_sm_r0c0n0` t9 | 96 | 76 / 15 | same | 29 / 89 / 18 |
| `mc_l_t0c0n0s0` t8 | 61 | 73 / 5 | same | 26 / 92 / 22 |

The split fetch counts include both arms of `sample_mipped` (mipped or
not, `flags.w`) for each map, which a draw takes one of. The opaque
sets' alpha test is now a constant 0, so their split stages hold no
`discard`. Whether the driver then allocates fewer registers is
INFERRED: no profiler here (lavapipe only; RADV's statistics need
Mesa's amdgpu drm-shim, which the Arch package does not ship).
`lit_pipeline_register_census` (ignored) builds the unsplit stages and
every row's split one behind a `LIT_REGS` line for a driver that prints
its shader statistics.

### Load and frame

mp_crossfire (`cod4_live_crossfire_draw_census`) and mp_crash
(`cod4_live_spot_light_shadows_its_ground`), lavapipe, from the
`cod2 lit:` load line. VERIFIED (log):

| map | lit pipelines (split) | prepass | ms compiling, split | unsplit (`VCOD_LIT_SPLIT=0`) |
|---|---|---|---|---|
| mp_crossfire | 87 (87) | 57 | 205 | 8, 5 prepass, 29 ms |
| mp_crash | 60 (60) | 42 | 134 | 8, 5 prepass, 27 ms |

Built one at a time on the loading thread the split took 1266 and
825 ms; eight at a time, as above. A real driver's compile per pipeline
differs, and its disk cache keeps the second load's.

Per frame at the census's views, joined draws: 618 to 621 pipeline
binds (73 to 81 unsplit), bind groups 1425 to 1456 (1414 to 1449). The
binds follow the material sort (`order_draws`), so a pipeline change
lands wherever retail's would change technique. lavapipe's
encode-to-readback ms did not move beyond its run-to-run spread
(82 to 127 ms either way). VERIFIED (census).

Frames: `VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live`
(94 tests, 66 frames), base, split, `VCOD_LIT_SPLIT=0` and split
again. 21 differ between two runs of one build (section 26's list
with `crossfire_writes_1` and `_2`, new since), and of the other 45,
43 hold byte for byte across all four. `saw_mounted` moves one pixel by
one level between split and unsplit, and `heli_without` eight pixels by
at most two between the base and this branch in both modes: lavapipe's
LLVM compiles the reshaped shader to different roundings, the values
the shader reads being the same. Again after sections 27 and 28 merged,
split against `VCOD_LIT_SPLIT=0` in one build: all 45 byte for byte.
VERIFIED (frames).

Pending, on a real card: the section 21 command at the held
view (`SESSION=1 SERVER_ARGS="--set sv_cheats=1" tools/cod4_perf.sh
<out> mp_crossfire 30 --exec-in-game "setviewpos 5232 -888 180 225 0"`)
twice, once with `VCOD_LIT_SPLIT=0` in the client's environment,
reading `gpu.opaque`; the map's load time either way; and with a
profiler to hand (RGP, Nsight), the `cod2 lit pipeline` draws' register
count and occupancy, split against unsplit.

## 30. The gamma pass folded into the post pass, the feather copy kept (P14)

Section 24 left two full-screen passes 1.7 does not run: the gamma pass
and the feather's depth copy at the split.

### The gamma pass

- 1.7 has no overbright bits. iw3mp.exe's dvar names hold `r_gamma`
  (0x7046d8) and `r_ignorehwgamma` (0x7046f8) and no
  `r_overBrightBits`; the only "overbright" string is a spot light
  dvar's description. VERIFIED (strings). vcod already gave CoD4 none
  (the window and every CoD4 test pass `fullscreen` false);
  `gamma::overbright_bits` now says 0 for CoD4 whatever the window, as
  for CoD2.
- At `r_gamma` 1 with no overbright bits the ramp is the identity
  (`GammaPass::identity`) and the gamma pass a copy of the gamma scene.
  A frame that runs the post pass (a CoD4 level's film, glow or depth of
  field, or `vc_hdr`) then draws it and the 2D straight into the
  swapchain, cleared to black first, and skips the gamma pass
  (`Renderer::render`'s `gamma_direct`). Conditions: the swapchain is
  `Rgba8Unorm` (the window now asks for it where the surface lists it,
  else an sRGB format as before, where the gamma pass linearises), it is
  opaque (the 2D's blends leave alpha below 1, which the gamma pass
  overwrote), and no shellshock capture reads the gamma scene that
  frame. Without the post pass the glow, `blurWorld`, bloom and shafts
  work in the gamma scene, so a CoD1 or CoD2 frame without it keeps the
  gamma pass; with `r_gamma` off 1 or CoD1's overbright doubling every
  game keeps it (cod11-gamma.md 4). VERIFIED (ours).
- `pipeline_check`'s offscreen target is `Rgba8Unorm` too and its
  readback swizzles to BGRA with alpha 255, as an opaque swapchain shows.

`cod4_live_crossfire_frame_writes` (section 24's views, lavapipe, one
sample, 1920 x 1080). VERIFIED (ours):

| view | before | after |
|---|---|---|
| held (5232 -888 240, yaw 225) | 78963440 | 70669040 |
| median (3990 -1110 60, yaw 180) | 74769136 | 66474736 |

The 8294400 bytes are the gamma pass's write; the post pass writes the
same texels into the swapchain instead of the gamma scene. A
full-screen read of the gamma scene goes too.

### The feather copy, not folded

1.7 writes `floatZ` in the depth prepass (section 24). vcod's prepass
takes only the opaque lit world draws that are not alpha tested, carry
no polygon offset and are not water (`Cod2World::takes_prepass`), plus
the entities' opaque first stages; at a feather the copy at the split
holds every depth-writing draw before it. Counted at the split over
`cod4_worldfx_live` and `cod4_live_crossfire_frame_writes` (a scratch
count, not committed), depth-writing world draws before the split that
the prepass does not take, per frame: 1 to 52 alpha tested, 0 to 29
with polygon offset, 0 to 9 other opaque (water, the CoD4 model sets),
against 29 to 620 in the prepass. VERIFIED (ours). A `floatZ` written by
the prepass would miss all of those, so the feathered sprites over
foliage, fences and decals would fade against the depth behind them.

Folding it means the alpha-tested and offset draws in the prepass with
a fragment stage (the alpha test and the R32F write, depth bias added
to the written value), at one sample only (an MSAA colour target
resolves by averaging, the copy takes sample 0). It would not cut
bytes: the R32F write is the same 8294400 either way. What it could cut
is the split it forces, the scene's colour and depth stored and loaded
once more (16588800 bytes at the held view), but only where no
distortion, `vc_ssao` or `vc_shadows` splits the frame anyway. Those
pipelines live in `renderer/cod2.rs`'s lit set, and the brief left it at
that. INFERRED. Section 35 folds it at one sample, after 1.7's own
prepass set.

### The frames

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins -- cod4` (288 tests,
69 frames), base against this branch: 46 byte for byte the same; the 23
that differ are section 26's live-session list (the crossfire census,
bricks, glow, haze, heli, saw firing, scope hip, the spots, tracer),
`crossfire_writes_1` and `_2`, `join` and `saw_mounted`. A second run
of this branch differs from the first in the same frames bar
`saw_mounted`, so those are the live sessions' own spread: local
patches of moved bodies or effects, never the whole frame a ramp change
would move. `saw_mounted` moves one pixel by one level in both runs of
this branch. The old offscreen target was `Bgra8UnormSrgb`, so every
frame went through the gamma pass's linearise and the target's sRGB
encode; a byte that round trip missed by one would read so. INFERRED.
VERIFIED (frames).

Pending, on a real card: `r_gamma` 1 at the held view of
section 21, `gpu.total` against before; then move the Brightness slider
and check the picture brightens (the gamma pass comes back off 1).

## 31. The CPU frame: world bundles, the held cull, ragdolled corpses (P13)

A real-card profile (sections 7 and 19) put the CPU side of a
CoD4 frame at about 6-7 ms: `submit` 2.07, `encode` 0.96, `bodies` 1.21,
`cull` 0.56, `hud` 0.46, `fxdraw` 0.45, `fxstep` 0.31, `gather` 0.30.
Retail draws mp_crossfire at 190-230 fps on that PC (4.3-5.3 ms a
frame). Everything here is vcod's own code; no binary address is
involved.

### `finish` apart from `submit`

wgpu 30 defers a command encoder's work: a pass only queues its
commands, and `CommandEncoder::finish` validates them, tracks every
resource they touch and records them into the driver
(`wgpu-core` `command::encode_commands`). The old `submit` stage held
that, `Queue::submit` and the present (which waits while two frames are
queued, section 7 item 4). The profile now times `finish` on its own,
between `encode` and `submit`. On lavapipe `submit` is the rasterizer's
wait; `finish` is CPU only.

### What changed

1. World runs replay from render bundles (`WorldBundles`,
   `renderer.rs`). The main pass's items between two non-world items or
   pass breaks (the feather copy, the split, the dlights) are one run.
   Each frame `emit_world_draw` records the run's commands into a
   `Recording` (a handle clone each, no validation) instead of the pass.
   A run whose commands equal last frame's run at the same place
   replays its bundle, made on the second frame the commands held; one
   that changed replays its commands into the pass one by one, so a
   view that changes every frame never pays for a bundle it uses once.
   A bundle starts with no state bound, so a run binds its vertex,
   index and camera groups itself, as the loop did after any non-world
   item; the census counts the same binds. CoD4 only;
   `VCOD_WORLD_BUNDLES=0` turns it off. The profile's `bundled` counts
   the world draws replayed from a bundle, `rebundled` the bundles made.
   The GPU groups stamp a run once, as its first item's group, so a
   bundled run's static models count under `world`.
2. The cull is kept while the eye, the view and the LOD ramp hold bit
   for bit (`Renderer::cull`'s inputs; `WorldVis` does not change after
   the load): a held view walks no cells.
3. A corpse whose ragdoll runs or sleeps no longer poses and skins the
   death anim under it each frame (`Cod4Players::posed`): the ragdoll's
   bones were drawn and the anim's thrown away. A corpse fires no
   weapon, so it keeps no tags.

### Numbers

`tools/cod4_perf.sh`, lavapipe, 640 x 372, one sample, mp_crossfire.
Held: war, no bots, `sv_cheats 1`, `setviewpos 5232 -888 180 225 0`,
30 s. Fight: dm, 12 shooting bots, the spectator following one, 30 s;
each run follows another bot, so its counts and stages move run to run
by more than the cuts. Mean (p50) ms. VERIFIED (lavapipe, ours):

| run | cull | gather | build | encode | finish | bodies | world draws bundled |
|---|---|---|---|---|---|---|---|
| held, base 043551a3 (4 samples) | 0.22 (0.22) | 0.03 | 0.11 | 0.47 | (in submit) | 0.01 | |
| held, `VCOD_WORLD_BUNDLES=0` | 0.00 | 0.04 | 0.12 | 0.51 (0.49) | 0.54 (0.53) | 0.01 | 0 |
| held, this branch | 0.00 | 0.03 | 0.11 | 0.48 (0.46) | 0.45 (0.44) | 0.01 | 405 of 405 |
| fight, base 043551a3 | 0.19 | 0.14 | 0.26 | 0.54 | (in submit) | 2.94 (2.58) | |
| fight, `VCOD_WORLD_BUNDLES=0` | 0.16 | 0.16 | 0.23 | 0.57 (0.57) | 0.70 (0.67) | 3.29 (3.45) | 0 |
| fight, this branch | 0.11 | 0.14 | 0.22 | 0.50 (0.48) | 0.54 (0.51) | 2.15 (0.52) | 145 |

- At the held view the 405 world draws are three runs (the opaque world,
  the world between the entities' sorts, the world after them), all
  bundled from the second frame on: `finish` falls by a sixth. The rest
  of `finish` is the entities' 80 joined draws, the effects, the
  prepass and the post passes.
- A first cut made a bundle on every change; in the fight that moved
  `encode` up by 0.24 ms and `finish` down by 0.15, a loss, so a bundle
  now waits for its commands to hold a frame.
- `bodies`' mean is the ragdolls' steps: a scratch timer over the fight
  read 0.35-0.45 ms a frame for 20 bodies' poses (about 20 us each) and
  0.8-7 ms for the running ragdolls, whose 60 Hz substeps pile up at
  lavapipe's 10-17 fps. The corpse change took 8 ragdolled corpses'
  poses (about 0.13 ms a frame) out of that.

Frames: `VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (94
tests, 66 frames), this branch twice and once with
`VCOD_WORLD_BUNDLES=0`. The two runs of one build differ on 21 frames
(section 29's live-session list, `bricks_on` among them this time);
bundles against none differ on those and `saw_mounted`, one pixel by
one level, and `cod4_live_saw_gunner_view` run three times either way
writes one of two `saw_mounted` frames in both modes, so it is one of
the unrepeatable ones. The other 45 hold byte for byte. The cull and the
corpse changes feed the same inputs to the same draws. VERIFIED
(frames). The saw test failed once in three runs in each mode under
the machine's load ("9 client rounds, 27 server rounds").

### Not done

- Sorting the world draws by pipeline. The draw order is the material
  sort and, within a material, the batch key (lightmap, probe, primary
  light); the order decides coplanar pixels under LessEqual and EQUAL
  (section 21). A material's batches under different light kinds take
  different split pipelines (section 29), so the binds alternate; 1.7's
  draw-surf key puts `primaryLightIndex` above `materialSortedIndex`
  (`GfxDrawSurfFields`, INFERRED), which would group
  them. Changing vcod's key is a fidelity change with its own frames to
  check, not a CPU cut.
- Posing only visible bodies: an unseen body still casts a sun shadow
  into the view and fires its tracers and flashes from its tags.
- The HUD (0.3-0.4 ms) and `fxdraw`: both rebuild from the session every
  frame off the render time (fades, timers, the compass); a cache keyed
  on their inputs is wider than this change and those files are being
  edited in parallel.

Pending, on a real card: `SESSION=1 SERVER_ARGS="--set
sv_cheats=1" tools/cod4_perf.sh <out> mp_crossfire 30 --exec-in-game
"setviewpos 5232 -888 180 225 0"` with and without `VCOD_WORLD_BUNDLES=0`
in the environment, reading `encode`, `finish`, `submit` and `bundled`;
then the same with `BOTS=12 GAMETYPE=dm` and no view held. `finish`
apart from `submit` says how much of the old 2.07 ms was wgpu's
encoding and how much the present.

## 32. Effects culled and faded as 1.7's, and the feather's extra (P18)

The playtest of section 20 again: 15.5 fps by mp_crash's intact red
sedan on a real card, 30 with Soften Smoke Edges off, 45 with the car
destroyed; retail 220-260 with no smoke in its screenshot.

### What 1.7 drops there

cod4-effects.md 18 has the spawn and draw path with addresses. vcod
culled each element's spawn by its `spawnRange` only; 1.7 also culls a
flag 0x4 element's spawn against the view frustum and scales every
element's alpha by its `fadeInRange` / `fadeOutRange`. At the sedan
spot that drops or fades (VERIFIED, `--example fx_ranges`):

- `fire/firelp_small_pm` element 2 (`gfx_smk_white_atlas`), the
  nearest fire's smoke 1182 units off: alpha 23/255 (fade in 1000 +
  200). vcod drew it whole: the smoke column over the town retail's
  frame lacks.
- `dust/dust_wind_slow_yel_loop` element 1 (`gfx_smk_white_atlas`), 1600
  units off behind the view: frustum culled (radius 250).

Nothing else at that spot is culled by 1.7 and drawn by vcod: no cell or
portal test, no visibility trace stands between an effect and its
pixels (cod4-effects.md 18). Both rules are in now.

### The sweep, before and after

`SWEEP=1` (section 25), lavapipe, 1920 x 1080, war, no bots, `PASSES="base
zfeather0 fxdraw0 aa4 base2"`, 8 s a pass after 4, `VCOD_FRAG_STATS=1`,
the two views held by `setviewpos`. GPU ms mean and the passes' mean
fragments (`p` lines), VERIFIED (ours):

| view | build | pass | fps | gpu.total | gpu.split | gpu.softcopy | gpu.blend | fx frags | fxsoft frags |
|---|---|---|---|---|---|---|---|---|---|
| sedan | before | base | 12.0 | 80.1 | 2.87 | 3.12 | 3.90 | 0 | 279867 |
| sedan | before | zfeather0 | 13.1 | 73.6 | 0.03 | 0 | 0.02 | 180857 | 0 |
| sedan | before | fxdraw0 | 13.8 | 69.7 | 0.02 | 0 | 0.02 | 0 | 0 |
| sedan | before | aa4 | 7.3 | 133.6 | 2.72 | 2.92 | 17.34 | 0 | 698772 |
| sedan | before | base2 | 13.8 | 70.0 | 2.28 | 2.41 | 2.19 | 0 | 67505 |
| sedan | after | base | 13.0 | 74.6 | 2.42 | 2.55 | 4.94 | 0 | 612430 |
| sedan | after | zfeather0 | 13.7 | 70.5 | 0.02 | 0 | 0.02 | 498107 | 0 |
| sedan | after | fxdraw0 | 14.5 | 66.5 | 0.02 | 0 | 0.02 | 0 | 0 |
| sedan | after | aa4 | 8.5 | 115.7 | 2.12 | 2.25 | 11.28 | 0 | 275824 |
| sedan | after | base2 | 12.4 | 76.4 | 2.49 | 2.61 | 6.41 | 0 | 1051358 |
| crossfire | before | base | 10.1 | 95.5 | 2.93 | 3.10 | 8.52 | 0 | 785010 |
| crossfire | before | zfeather0 | 9.4 | 103.2 | 0.02 | 0 | 0.07 | 1142607 | 0 |
| crossfire | before | base2 | 9.2 | 102.9 | 3.25 | 3.52 | 11.32 | 0 | 1234092 |
| crossfire | after | base | 11.7 | 83.1 | 2.18 | 2.28 | 6.70 | 0 | 615951 |
| crossfire | after | zfeather0 | 11.3 | 85.7 | 0.02 | 0 | 0.06 | 1348653 | 0 |
| crossfire | after | base2 | 10.9 | 85.1 | 2.20 | 2.31 | 9.16 | 0 | 1231973 |

- The fragment counts swing 15-fold between passes of one build (the
  sedan's base2 67505 against base 279867 before): the ambient dust
  spirals and wisps drift through the held view, and those 1.7 culls
  nothing of. The fade leaves a sprite's fragments where they were; it
  changes its alpha. At these two views the frustum test drops one
  far element. So the fill does not move with this change, within the
  runs' spread. INFERRED (the counts).
- Here, unlike section 20, feathered sprites draw (`fxsoft` up to 1.05
  million fragments, half a screen at 1080p), and the world's count
  holds still (3097447). The effects are at most a quarter of the frame's
  fragments.
- `r_zFeather` 0 saves 4 to 10 ms on lavapipe in each build: the split
  (2.1 to 3.3 ms, the scene's colour and depth stored and loaded again),
  the depth copy (2.3 to 3.5 ms) and the feathered blend.
- Crossfire's after rows read 12 to 18 ms under before, the lavapipe
  spread between two runs (cod4-client-perf.md 14); nothing changed
  there but the fade.

### The feathered quad against 1.7's

- Per fragment the two match: 1.7's `zfeather.hlsl` reads `floatZ` once
  (cod4-effects.md 14), vcod's `fs_soft*` loads the R32F copy once; both
  shade once a pixel under MSAA and blend per sample.
- The extra is per frame. 1.7 writes `floatZ` in its depth prepass
  (`R_DepthPrepassCallback` binds `R_RENDERTARGET_FLOAT_Z` with
  `TECHNIQUE_BUILD_FLOAT_Z` when the view `needsFloatZ`, INFERRED): no split, no copy. vcod splits the
  main pass at the copy and copies sample 0 of the depth there (section
  20): 4.4 to 6.4 ms of 70-100 on lavapipe at one sample. Folding the
  copy into vcod's prepass needs its alpha-tested, offset and water
  draws in the prepass (section 30), the world draw order P17 owns, so it
  is not cut here. On a real card the split's store and load are 33 MB
  a frame at 1080p and one sample, 133 MB at four. INFERRED (the sizes).
- The playtest's halving with Soften Smoke Edges off is larger than any
  of this. One cost lavapipe cannot show: the copy samples the depth
  attachment as a texture each frame, which on some cards decompresses
  the depth buffer before the read and leaves it uncompressed for the
  rest of the frame's depth tests. INFERRED, not measured.

### Pending on a real card

On the real card, after `cargo build --release -p vcod -p vcod-server`,
one command runs both builds' question at the sedan spot (the feather,
the effects, four samples):

```
SWEEP=1 SESSION=1 COD4_DIR=<install> SIZE=1920x1080 VCOD_FRAG_STATS=1 \
  VIEW="-279 1396 233 -15 28" PASSES="base zfeather0 fxdraw0 aa4 base2" \
  tools/cod4_perf.sh /tmp/sedan mp_crash 10
```

If `zfeather0` still doubles the fps while `gpu.fxsoft` stays small, the
cost is the split and copy (`gpu.split`, `gpu.softcopy`), and folding
the copy into the prepass is the next step (section 35 does it at one
sample). Whether the town still
shows smoke by the sedan is a look against retail's screenshot. Pending.

## 33. 1.7's draw order, the HUD's dvars and the marks' gather (P17)

Section 31's "Not done": the world's binds alternating between split
pipelines, the HUD (0.3-0.4 ms) and `fxdraw` (0.45 ms on a real
card).

### The draw order

vcod ordered a CoD4 map's world batches by CoD1's sorted shader list
(material sort, then bundle image numbers) and, within a material, by
the batch key, so a material's batches under the sun, a spot and no
light sat together and each light kind's split pipeline (section 29)
came and went per material. 1.7's draw-surf key puts the primary light
above the material (cod4-world.md 12.1, now read from iw3mp.exe), the
world list draws in the zone's surface order (`R_CompareSurfaces`,
12.3), and `materialSortedIndex` follows `Material_Compare`, which
`drawsurf_census` holds to all 21 stock zones (12.2). A CoD4 batch now
draws at its first soup's place in that order (`MapWorld::soup_order`,
`cod4_places`); `VCOD_C4_DRAW_ORDER=0` at load keeps the old order.

`cod4_live_crossfire_draw_census` (lavapipe, 1920 x 1080, prepass on),
joined draws, old order then new. VERIFIED (census):

| view | pipelines | bind groups |
|---|---|---|
| 5232 -888 240, yaw 225 | 618 -> 118 | 1415 -> 1529 |
| same, yaw 270 | 621 -> 135 | 1455 -> 1583 |
| 3990 -1110 60, yaw 180 | 130 -> 46 | 393 -> 415 |

At `cod4_perf.sh`'s held view (1280 x 772): 292 -> 69 pipelines, 878 ->
924 bind groups. The pipelines fall by four fifths; the material groups
rise by about 8%, since a material's batches under two lights are now
apart, as retail binds them. The draws are the same.

Frames: `VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (94
tests, 66 frames), new order twice and `VCOD_C4_DRAW_ORDER=0` once. 21
differ between the two runs of one build (section 29's list). Of the
other 45, 18 hold byte for byte, `saw_mounted` is section 31's
two-state frame, and 26 move, all where two surfaces meet at one depth
and the later draw takes the pixel under LessEqual or the EQUAL pass:
- mp_crash (`crash_lit_sun`, `crash_lit_shade`, `crash_aniso1`,
  `crash_aniso16`, `mp_crash_prepass_0`/`_1`): 501 to 756 pixels, up to
  195 levels, on the floor under the arcade at the right, where an
  overlay lies on the floor and the stripes of their depth ties change.
- mp_crossfire (`mp_crossfire_prepass_0`/`_1`, `sweep_base`,
  `sweep_base2`, `sweep_film0`, `sweep_fog0`, `sweep_normalmap0`,
  `sweep_specular0`): 1745 to 2016 pixels, up to 55 levels, the bullet
  hole decals on the plaster wall through the left window, overlapping
  decals of one sort key now drawn in retail's order.
- `beam0_off`/`_on`, `glass0_front_off`/`_on`, `glass_prepass0`/`1`,
  `dof_off`/`_on`, `saw_up`, `spots_four`, `spots_one`, `tracer_off`:
  1 to 14 pixels each along such seams.
Where depths tie to the bit, retail's order decides these pixels, so the
new frames take its side; where the two triangulations' depths differ
by a rounding, both orders show stripes, and retail's would show its
own. VERIFIED (the frames); which is closer to retail's pixels needs a
retail capture of the same view, not made.

### The HUD

Scratch timers in a release build (not kept) over the held view and the
fight put `Cod4Hud::server_dvar` at 0.23-0.28 ms of `hud.build`'s
0.39-0.43 ms: the menus' `visible when` and dvar reads ask 30 to 54 a
frame, and each copied all 128 name slots into Strings before comparing.
It now compares the configstrings where they lie
(`configstring_bytes`): 0.06 ms for the same calls, `hud.build`
0.19 ms. No cache: the configstrings change under `d` commands and the
HUD's fades run off the render time. VERIFIED (scratch timers).

### `fxdraw`

The same timers split the fight's 0.34-0.45 ms: building the quads
0.08-0.13 (the sort 0.02), uploading them 0.07-0.09, the clouds and the
lights under 0.01 together, and the marks 0.17-0.27, of which placing
the frame's impacts 0.04-0.20 ms for 0.6 or 0.7 impacts a frame.
`marks::mark_fragments` chopped every triangle of each surface whose box
met the mark's, each `chop` allocating two Vecs. A triangle wholly past
one side of the gather box grown by 1 (the clip epsilon is 0.5) can
leave nothing, so it is skipped unchopped, and `chop` keeps its distances
on the stack: about 0.03 ms an impact afterwards. The marks' vertex
rebuild (0.02-0.07 ms for 500-2500 vertices) and grouping (0.02-0.06)
stay. VERIFIED (scratch timers). The marks code is shared with CoD1 and
CoD2; the fragments are the same.

### Numbers

`tools/cod4_perf.sh`, lavapipe, 1280 x 772, one sample, mp_crossfire, 30 s. Held: war, no bots, `sv_cheats 1`,
`setviewpos 5232 -888 180 225 0`. Fight: dm, 12 shooting bots, the
spectator following one; each run follows another bot. Before is this
branch with `VCOD_C4_DRAW_ORDER=0` and the scratch timers, after is the
merged branch. Mean (p50) ms. VERIFIED (lavapipe, ours):

| run | hud | fxdraw | encode | finish | build | cull | gather | bodies |
|---|---|---|---|---|---|---|---|---|
| held, before | 0.49 (0.46) | 0.16 (0.15) | 0.56 (0.44) | 0.48 (0.46) | 0.14 | 0.00 | 0.03 | 0.01 |
| held, new order only | 0.43 (0.41) | 0.16 (0.15) | 0.46 (0.43) | 0.48 (0.47) | 0.12 | 0.00 | 0.04 | 0.01 |
| held, after | 0.30 (0.29) | 0.16 (0.14) | 0.79 (0.44) | 0.53 (0.45) | 0.11 | 0.00 | 0.03 | 0.02 |
| fight, before | 0.43 (0.32) | 0.45 (0.36) | 0.88 (0.72) | 0.94 (0.75) | 0.24 | 0.20 | 0.22 | 4.00 |
| fight, after | 0.23 (0.23) | 0.29 (0.25) | 0.61 (0.58) | 0.73 (0.70) | 0.22 | 0.25 | 0.24 | 3.29 |

- The held view's `encode` and `finish` hold their p50: its three world
  runs replay from bundles (section 31), so the fewer pipeline binds
  inside them cost lavapipe's encoder little. The after run's `encode`
  mean carries a few 2-4 ms frames (p95 2.82) the p50 does not.
- The fight's runs follow different bots (1172 draws before, 1042
  after), so its `encode`, `finish` and `bodies` move with the view, not
  the change.
- What else stays above 0.2 ms: `bodies` in the fight (the ragdolls' 60
  Hz substeps piling up at lavapipe's 10 fps, section 31), `encode` and
  `finish` (wgpu's recording, section 31), `cull`, `gather` and `build`
  in the fight at 0.2-0.25, and `events`, whose p50 is under 0.1 but
  whose first-use loads spike to 9 ms in some fights. Not cut here.

Pending, on a real card, two runs each:
`SESSION=1 SERVER_ARGS="--set sv_cheats=1" GAMETYPE=war BOTS=0
tools/cod4_perf.sh <out> mp_crossfire 30 --exec-in-game "setviewpos 5232
-888 180 225 0"`, once as is and once with `VCOD_C4_DRAW_ORDER=0` in the
environment, reading `encode`, `finish`, `submit`, `gpu.opaque`,
`pipelines` and `hud`; then `BOTS=12 GAMETYPE=dm` without the view held
for `hud` and `fxdraw`. A by-eye check: mp_crash's arcade floor and
mp_crossfire's bullet-holed plaster wall against retail at the same
view.

## 34. Connect to the first frame in game, every game (P20)

### The tool

Every load logs one timeline at info, `load profile <what>: <ms> ms to
the first frame`, then a line a step (start ms off the load's start,
its length, the step): the window, the handshake, the gamestate, each
map load's laps (`vcod_common::laps::Laps`, folded in on their own),
the level prewarms, the first frame drawn in game. A load starts at
the process start under `--connect`, at a `connect`, or at a map
change, and ends at the first frame whose session is live
(`laps::begin_load`, `laps::end_load`). `VCOD_LOAD_PROFILE=<path>`
appends the same lines to a file. VERIFIED (log).

The numbers below: `tools/cod4_perf.sh` (lavapipe, nested kwin,
loopback `unshare -rn`, two shooting bots), a process start with
`--connect`, three runs a side alternating old and new binaries
(local scripts; medians). Lavapipe compiles
pipelines and uploads on the CPU, so its pipeline and upload steps
are larger than a real card's; the load average was 7-10 from other
jobs. VERIFIED (log).

### Before and after

| | CoD1 mp_carentan | UO mp_cassino | CoD2 mp_toujane | CoD4 mp_crossfire |
|---|---|---|---|---|
| connect to first frame, before | 2410 | 3157 | 2234 | 7577 |
| after | 1731 | 2364 | 1956 | 6986 |
| level prewarm (CoD2: level models), before | 935 | 1092 | 372 | 1508 |
| after | 250 | 306 | 266 | 643 |
| CoD4 map load on the join thread, before / after | | | | 1073 / 808 |

What each step held before, mp_carentan and mp_crossfire:

- The level prewarm's models (CoD1 865 ms of 983, CoD4 917 of 1525):
  each model's upload built its skins' entity pipeline pairs one at a
  time on the calling thread (about 400 ms on CoD1, 330 on CoD4) and
  decoded its skins alone (CoD1 about 300 ms).
- CoD4's HUD zones (530 ms of the prewarm): `HudAssets::load` walked
  the four stock HUD zones and the map's again after the map's load had
  walked them.
- CoD4's map load: `common_mp` and the three sound zones walked one
  after another after the map's zone (170 + 340 ms).
- CoD4's handshake (3.2 s) is the gamestate's fragments, 350 ms apart
  at `sv_maxRate` 5000 as 1.7 paces them (cod4-handshake.md 3); not a
  client cost, left as it is.

### What changed

- `Renderer::upload_dynamic_models`: a level's models upload as one
  batch, in the same order. Their skins' pipeline keys are read first
  (`EntityMaterials::skin_pipeline_keys`) and built at once
  (`EntityMaterials::prewarm`, `par_build`), their images decoded on
  every core (CoD1's mipped too, `prepared_skins`, emptied after the
  batch). The CoD1/UO (`EntityScene::prewarm`), CoD2
  (`Cod2Draw::preload` through `entities::resolve_models`) and CoD4
  (`Cod4Client::prewarm`) prewarms take it. Viewmodel preloads decode
  their skins the same way.
- The mark prewarm builds its stages' pipelines at once
  (`MarkPass::prewarm`): CoD1 marks 88 to 31 ms.
- CoD4: the stock zones (`common_mp`, the sound zones) are walked
  during the handshake on their own threads (`cod4_fly::walk_stock`),
  or beside the map's zone on a map change, and the HUD's assets come
  off the same walks (`cod4_fly::hud_assets`, gate
  `hud_assets_match_their_own_walk`: the HUD reads no bulk data, so a
  walk keeping it gives the same assets). Each zone is walked once a
  load.
- The CoD4 lit pipelines compile on eight threads off one queue
  (`par_build_on`) instead of in batches of eight.

Nothing is built twice: a skin, pipeline, image or model already held
is skipped as before (`by_skin`, `uploaded`, the model caches, the vm
cache). The upload order and the materials' slot order are unchanged.

### Not done

- The window and renderer (350-620 ms) and the CoD4 world's GPU state
  (800-1000 ms, `finish` the lit pipelines and the sun shadow casters)
  are mostly lavapipe's pipeline compiles; a real driver caches them
  on disk across runs.
- A second load of the same map: the zone walks are not kept (a
  `Walked` holds the whole zone; the resident set doubles), and a
  same-map gamestate on CoD2 already builds no model pipeline
  (`cod2_live_same_map_gamestate_builds_no_model_pipeline`).
- CoD2's first frame in game (500-650 ms) is lavapipe's first draw of
  every pipeline; the frame's CPU stages are small.

### What 1.7 does at load

- Zones load on a database thread: the setup at 0x50b480 calls
  `CreateThread` (import 0x6911b4) with the entry 0x48ade0 stored at
  0x14e8a00, and `"Failed to create database thread"` (0x6d39d4) is
  pushed at 0x4ff2f6 when it fails. VERIFIED (disassembly). That the
  main thread draws the loading screen while it runs is INFERRED from
  the thread split.
- `"Loading fastfile %s"` (0x6d38a4) is printed at 0x48a0bf for each
  zone in turn. VERIFIED (disassembly).
- `r_preloadShaders` registers at 0x62d42e (its dvar pointer at
  0xd569744; help text at 0x707618: "Force D3D to draw dummy geometry
  with all shaders during level load; may fix long pauses at level
  start."). VERIFIED (disassembly and string). 0x488bb2 tests its value
  after a loaded asset (0x489e50 with type 5) and, when set, stores it
  in a 1024-entry ring at 0xd53e5e0. VERIFIED (disassembly). That the
  ring feeds the dummy draws the help text names is INFERRED. So 1.7
  leaves the shader warm-up to a dvar; vcod's prewarm is that warm-up
  always on.
- 1.7's own load time on the measuring machine is not measurable (no GPU).

Pending, on a real card: join each game's map through the
browser (or `SESSION=1 ... tools/cod4_perf.sh`) and read the
`load profile` line in the console log, before and after this branch;
then the first shot, frag and tracer in play should not hitch.

## 35. `floatZ` written in the depth prepass, no split and copy (P22)

Sections 30 and 32 left the feather's split and depth copy: 4.4 to 6.4
ms of 70-100 on lavapipe, and on a real card 15.5 fps by mp_crash's
sedan against 30 with Soften Smoke Edges off.

### What 1.7's prepass draws

All iw3mp.exe, read off the disassembly.

- `R_DepthPrepassCallback` (0x658f40, handed to the draw call at
  0x6590fb) turns the scissor on (render state 0xae), then reads the
  view's `needsFloatZ` byte (`GfxViewInfo`+0x13a8, 0x658fa6). Set, it
  binds render target 5 (`mov eax,0x5`, 0x658fb4, into 0x632b60), draws
  the clear quad with the material at 0xcc9a2cc (0x636e90), writes
  (0, 0, 0, 1) into the code constant at `+0xb60` (`depthFromClip`,
  0x658fe0..0x658ff9) and takes technique 1; clear, target 6 and
  technique 0. It then draws two surface lists through `R_DrawSurfs`
  (0x612260), `GfxViewInfo`+0x6304 and +0x632c, each a copy with the
  technique patched in (0x659021, 0x65904e). VERIFIED (the code). The
  names (`R_RENDERTARGET_FLOAT_Z`, `TECHNIQUE_BUILD_FLOAT_Z`, `litInfo`,
  `decalInfo`) are INFERRED.
- `R_SetupMaterial` (0x648f10) sends a technique of 1 or less to
  `R_SetPrepassMaterial` (0x648df0): the draw surface's prepass key
  (bits 40..41, `shr ecx,0x8; and ecx,0x3` on the high dword) of 3
  draws nothing; the material is `sortedMaterials[materialSortedIndex]`
  (0xcc98280), on key 0 with `stateFlags` 0x1 (`Material`+0x3d) the one
  at 0xcc9a2e0 (`depthPrepassMaterial`); a null
  `techniques[1]` (`+0x40`, `+0xc + 4 * technique`) draws nothing.
  VERIFIED (the code).
- So the prepass draws the lit and decal lists, world, static models
  and entities alike, every surface whose material has prepass key 0, 1
  or 2 (0x6211d0, cod4-world.md 12.2) and a `build floatz` technique.
  Blended materials have key 3 (every blended world material of mp_crash
  and mp_crossfire, a scratch census of `MaterialOrder::prepass`).
  VERIFIED (the census). Key 1 is the alpha-tested ones: their
  technique 1 is `build_floatz_test`, whose pixel shader
  (`floatz_build_atest.hlsl`) `texkill`s on the colour map's alpha times
  the vertex alpha minus 0.50196 and writes the vertex shader's
  `depthFromClip` dot as `oC0`; its state has no alpha test and no
  polygon offset, as every technique 1 state on those maps. VERIFIED
  (`--example techset_shaders` on `wc_l_sm_t0c0n0s0` through
  `tools/re/d3d9dis.py`, and the states). A model material with a
  polygon offset in its lit state draws there unoffset when its key
  allows (`mc/mtl_brush_toujanebigbushy`, key 2) and not at all with key
  3 (`mc/mtl_canopy_metal`). VERIFIED (the keys).
- The view model draws in the lit list behind its depth hack
  (`primarySortKey` lowered, `R_AddDObjSurfaces`), so
  1.7's `floatZ` likely holds the gun too. INFERRED. vcod's
  prepass leaves the gun out; where the gun covers a pixel the sprites
  behind it fail the depth test either way.

### What vcod does now

A CoD4 frame at one sample with a feathered effect or world surface in
view and `r_zFeather` 1 runs the prepass as a `floatZ` prepass
(`Renderer::encode_depth_prepass`): the R32F target the copy used is its
colour attachment, cleared to the far plane, and each draw writes its
view depth (`fs_floatz` in `cod2_world.wgsl` and `dynamic_model.wgsl`,
the copy's own linearisation of the same `@invariant` depth). It takes
the world and entity draws whose CoD4 material 1.7's prepass draws
(`MaterialOrder::draws_floatz`, `Material::floatz`), alpha-tested ones
with the material's test; water stays out (its own vertex stream). The
scene pass then neither splits for the feather nor copies; the effects
read the prepass's `floatZ` from the first item on. The draws the
prepass laid down untested still draw their colour under EQUAL, now
only those 1.7's set holds. Left as before: four samples (a fragment's
write is not the copy's sample 0), `vc_softParticles` (it fades every
sprite, not 1.7's), the distortion alone, `r_zFeather` 0, CoD2, and
`VCOD_C4_FLOATZ=0`, which keeps the copy for an A/B.

### Every feathered pixel against the copy

`cod4_live_floatz_prepass_against_the_copy` (lavapipe, 960 x 540, one
sample) draws each view with the copy and with the prepass, the frames
and the two `floatZ`s compared texel by texel, and `r_zFeather` 0 under
both. VERIFIED (ours):

| view | prepass draws copy / floatZ | feathered pixels | frame pixels moved | `floatZ` texels moved |
|---|---|---|---|---|
| sedan, `-279 1396 233 -15 28` | 5 / 4 | 0 | 0 | 0 |
| crossfire held, `5232 -888 180 225 0` | 46 / 30 | 0 | 0 | 0 |
| mp_crash, facing a feathered surface | 11 / 15 | 15595 | 0 | 818 farther |
| mp_crossfire, facing a feathered surface | 20 / 8 | 16918 | 0 | 818 farther |

- At the two held views the session's effects drew no feathered pixel on
  the frame the test took (the sedan's smoke drifts, section 32), so the
  test also faces a feathered world surface or sprite from 160 units.
- `r_zFeather` 0 draws the same bytes on both paths at all four views.
- The 818 texels on both maps are one rectangle at the screen's middle,
  161 units off in the copy and 178 to 185 in the prepass: a surface
  drawn with a polygon offset in front of the faced one is in the copy
  and not in `floatZ`. The world's undrawn depth writers at that view
  were one opaque offset lit draw (a scratch print of the draws
  `takes_floatz` refuses), and the only offset lit material both maps
  share is `mc/mtl_canopy_metal`, key 3, which 1.7 leaves out of
  `floatZ` too. INFERRED (which material). No frame pixel moves: the
  feathered surface behind it fails the depth test there.
- The joined draw counts move with 1.7's set: the copy path's prepass
  took the opaque, untested, unoffset lit draws joined by sidedness and
  flag wave; the `floatZ` prepass takes 1.7's materials joined by
  sidedness and alpha test, which leaves out key 3 and sets without
  technique 1 (`wc_shadowcaster` among them) and adds the alpha-tested
  and key 2 draws.

### The sweep, before and after

`SWEEP=1` (section 25), lavapipe, 1920 x 1080, war, no bots, `PASSES="base
zfeather0 aa4 base2"`, 8 s a pass after 3, `VCOD_FRAG_STATS=1`, the views
held by `setviewpos`; before is this branch with `VCOD_C4_FLOATZ=0`. GPU
ms mean. VERIFIED (ours):

| view | build | pass | fps | gpu.total | gpu.opaque | gpu.prepass | gpu.split | gpu.softcopy | gpu.blend | gpu.fxsoft | prepass draws | fx frags | fxsoft frags |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| sedan | before | base | 10.70 | 89.92 | 59.18 | 4.99 | 3.31 | 3.55 | 5.62 | 0.33 | 19 | 0 | 469699 |
| sedan | before | zfeather0 | 11.37 | 83.65 | 62.30 | 4.78 | 0.02 | 0.00 | 0.02 | 0.00 | 19 | 514951 | 0 |
| sedan | before | aa4 | 7.16 | 136.62 | 94.89 | 7.45 | 2.85 | 3.13 | 18.09 | 5.04 | 19 | 0 | 802841 |
| sedan | before | base2 | 11.71 | 82.16 | 53.84 | 4.12 | 2.98 | 3.24 | 4.93 | 0.18 | 19 | 0 | 440069 |
| sedan | after | base | 11.47 | 84.32 | 63.14 | 5.00 | 0.03 | 0.00 | 0.02 | 0.38 | 17 | 0 | 208892 |
| sedan | after | zfeather0 | 12.00 | 80.58 | 60.17 | 4.56 | 0.03 | 0.00 | 0.02 | 0.00 | 19 | 695522 | 0 |
| sedan | after | aa4 | 7.08 | 137.98 | 93.35 | 7.26 | 2.71 | 2.94 | 21.50 | 5.07 | 19 | 0 | 1198692 |
| sedan | after | base2 | 12.21 | 78.96 | 58.20 | 4.44 | 0.02 | 0.00 | 0.02 | 0.35 | 17 | 0 | 386522 |
| crossfire | before | base | 9.91 | 97.76 | 64.46 | 16.88 | 2.95 | 3.18 | 8.33 | 0.33 | 58 | 0 | 680867 |
| crossfire | before | zfeather0 | 10.11 | 95.64 | 73.33 | 16.18 | 0.06 | 0.00 | 0.06 | 0.00 | 58 | 1345087 | 0 |
| crossfire | before | base2 | 9.76 | 98.19 | 62.97 | 16.18 | 3.00 | 3.12 | 10.22 | 0.31 | 58 | 0 | 1198729 |
| crossfire | after | base | 10.89 | 88.71 | 66.53 | 10.96 | 0.02 | 0.00 | 0.06 | 0.76 | 40 | 0 | 660593 |
| crossfire | after | zfeather0 | 10.09 | 95.87 | 73.66 | 16.58 | 0.02 | 0.00 | 0.07 | 0.66 | 58 | 1317726 | 0 |
| crossfire | after | base2 | 10.79 | 88.91 | 67.30 | 9.96 | 0.02 | 0.00 | 0.06 | 0.68 | 40 | 0 | 1464009 |

- With the feather on, `gpu.split` (2.9 to 3.3 ms) and `gpu.softcopy`
  (3.1 to 3.6) go to 0; `gpu.blend` was the scene pass after the split
  and its draws now stamp under `gpu.opaque`, which grows by 2 to 4.4 ms.
  The sedan's base passes rise 0.5 to 0.8 fps, crossfire's 1.0; the
  `zfeather0` passes did not change path and stay within the runs'
  spread.
- `gpu.total` at the feather falls 3 to 9 ms; crossfire's prepass falls
  6 ms with it, its 58 joined draws now 40 (1.7's set leaves out the
  shadow caster and the other key 3 and techniqueless sets).
- `aa4` keeps the copy (four samples) and holds still, within spread.
- The fragment counts (`p` lines, `VCOD_FRAG_STATS`) swing between
  passes of one build as in section 32: the ambient dust drifts through
  the held views. `fxsoft` counts the feathered sprites' fragments, and
  they shade the same whichever path wrote `floatZ`.
- Lavapipe does not show what section 32 suspected of a real card: the
  copy sampled the depth attachment as a texture each frame, which can
  leave a card's depth buffer decompressed for the rest of the frame.
  That read is gone with the copy. INFERRED, not measured.

### Pending on a real card

On the real card, after `cargo build --release -p vcod -p vcod-server`,
section 32's sedan sweep with and without the old path:

```
for f in 0 1; do VCOD_C4_FLOATZ=$f SWEEP=1 SESSION=1 COD4_DIR=<install> \
  SIZE=1920x1080 VCOD_FRAG_STATS=1 VIEW="-279 1396 233 -15 28" \
  PASSES="base zfeather0 fxdraw0 aa4 base2" \
  tools/cod4_perf.sh /tmp/sedan$f mp_crash 10; done
```

If `base` now comes near `zfeather0` with `VCOD_C4_FLOATZ=1`, the split
and copy were the playtest's halving. By eye: soft smoke against
fences, foliage and wires by the sedan, against retail's.

## 36. The CPU frame with the view moving (P24)

Sections 31 and 33 cut the held view: its cull is kept and its world
runs replay from bundles. In a fight the view moves every frame, so
neither holds. This section measures that frame and cuts what it can.

### The tool

`TURN=<deg/s>` on `tools/cod4_perf.sh` sets `VCOD_PERF_TURN` on the
client, which yaws the view by that rate times the frame's time on every
live frame, as a held key would (`cod4_client.rs`, `perf_turn`).
`CULL_TURN=<degrees>` on `cull_census` (`vcod-cod4`, `CULL_CENSUS=<map>`)
turns level from every spawn point in steps of that and prints the
walk's mean time, how many steps kept the last step's cell set and soup
set, and the triangles drawn against those of the visible cells whole.
`CULL_OLD=1` walks without 1.7's small-portal rule.

### Where the moving frame goes

Scratch timers (release, not kept) over a 30 s turn at 90 deg/s on
mp_crossfire, `war`, no bots, from `setviewpos 5232 -888 180 225 0`,
lavapipe at 18 fps, ms a frame. VERIFIED (scratch timers, ours):

- `cull` (0.13-0.16): the cell walk and the static models' LOD picks
  (`Renderer::cull`), 0.13-0.16 of it.
- `gather` (0.10-0.15): the visible ranges listed 0.02, gathered into
  the index list with the cookie boxes 0.05-0.08, ordered 0.01, the
  indices uploaded 0.015-0.02. Every turned frame changes the soup set
  (below), so nothing of it is skipped.
- `build` (0.09-0.13): the entities' instances, bones and light sets
  uploaded 0.075 (0.11-0.14 in the fight), the joined entity draws
  sorted 0.02.

In the walk itself (`cull_census`, release, 5 degree steps) the cells'
cull groups took 82% on mp_overgrown and most of the rest on
mp_crossfire: a CoD4 static model placement is one cull group, a cell
lists hundreds to thousands, and each visit tested every group's box.

### What changed

1. A cell's cull groups are a box tree built at load
   (`WorldVis::cell_groups`, eight groups a leaf, split at the median
   along the longest axis); a box out of view skips its groups. Every
   group under a box lies inside it, so the marks are the flat list's:
   an assert comparing the two over every view of `cull_census` on
   mp_crash, mp_overgrown and mp_bloc (10 degree turns) and over CoD2's
   `cell_walks_side_by_side` held. The walk's mean at 5 degree steps:
   mp_crossfire 0.086 to 0.061 ms, mp_overgrown 0.364 to 0.182. VERIFIED
   (`cull_census`, ours).
2. 1.7's `r_portalMinClipArea` and `r_portalMinRecurseDepth`
   (cod4-world.md 9.9). A fidelity change; the walk's time does not
   move with it (0.077 and 0.072 ms at 1 degree on mp_crossfire).
3. The ragdoll's step is 1.7's (below).

Tried and dropped: the portal chops into two kept buffers instead of a
Vec a plane (0.086 to 0.082 ms, within the spread), and a tree walk
that skips the planes a parent box lies wholly inside of (no change: the
nodes are not the cost, the groups were).

### A world bundle per cell set

`CULL_TURN=1` (one degree a step, a 200 deg/s turn at 200 fps), every
spawn point, level. VERIFIED (`cull_census`, ours):

| map | same cells | same soups | tris drawn | tris of the cells whole |
|---|---|---|---|---|
| mp_crossfire | 93.2% | 11.1% | 40859 | 236856 |
| mp_overgrown | 93.9% | 16.3% | 86968 | 376413 |
| mp_crash | 95.0% | 11.4% | 60571 | 291419 |

At 5 degrees the cell set holds on 75-78% of the steps, at 7 on 69%. The
cell set holds while turning; the soup set does not, since the trees'
leaves and the static models come and go at the frustum's edge. A
bundle keyed by the cell set would have to draw the cells whole, 4.3 to
5.8 times the triangles 1.7 draws (its leaves and static models are
frustum culled too, 9.9). Not done: that trades the CPU's 0.7 ms of
`encode` and `finish` for several times the vertex work on the card.

### The ragdoll's steps

1.7's ragdoll world steps in `Phys_RunToTime` (iw3mp.exe 0x5a6dfb..
0x5a6e6b): `maxIter` 2 (0x5a6dfb), each step `max(msecStep, (timeNow -
timeLastUpdate) / maxIter)` (0x5a6e06..0x5a6e19), its time in seconds
to `Phys_RunFrame` (0x5a6440), `timeLastUpdate` advanced by it, until
it reaches `timeNow`. VERIFIED (disassembly). `g_phys_msecStep` is
0x71e798 (17, 17, 17), set each frame to `min + (max - min) * f` from
the tables at 0x71e7a4 (17, 17, 17) and 0x71e7b0 (67, 67, 34)
(0x5a6791..0x5a67bb). VERIFIED (.data, disassembly). That `f` rises from
0 past 32 awake bodies, and that the ragdoll world is the third, are
INFERRED (`Phys_RunFrame` and `PHYS_WORLD_RAGDOLL`).
`ragdoll_fps` (int, default 20, 0 to 100, registered at 0x5ba3f8, dvar
at 0xcc789ec, "Ragdoll update frames per second") is stored and never
read: no other instruction names 0xcc789ec. VERIFIED (disassembly).

So 1.7 steps 17 ms at most twice a frame: at 200 fps one step every
third or fourth frame, about 59 a second, at 10 fps two 50 ms steps.
INFERRED (the loop above). The bodies drawn between steps are
interpolated (`timeNowLerpFrac`, INFERRED). vcod stepped 1000/60 ms up to twelve times a frame,
six at 10 fps. It now steps as 1.7 does (17 ms, at most two a frame, the
first `max(17, owed / 2)`, the second the rest when a step is still
owed), carrying a point's velocity across a longer step and the damping
per sixtieth of a second, a step capped at 100 ms (vcod's; a hitch loses
time, as before). At 200 fps the count is the same as before; at
lavapipe's 10-18 fps a third. The corpses' spin at rest and the head's
twist (C4-121) are the solver's, at 4 ms frames and at 100 ms alike, not
the step rule's (cod4-player-anims.md 10.2).

### Numbers

`tools/cod4_perf.sh`, lavapipe, 1280 x 772, one sample, mp_crossfire,
30 s, base (88909543) and this branch alternated. Turn: `TURN=90`,
`war`, no bots, `sv_cheats 1`, `setviewpos 5232 -888 180 225 0`. Fight:
dm, 12 shooting bots, the spectator following one (a different bot each
run: base drew 1220 draws, 88k triangles, this branch 778 and 53k).
Mean (p50) ms. VERIFIED (lavapipe, ours):

| run | cull | gather | build | encode | finish | bodies | fps |
|---|---|---|---|---|---|---|---|
| turn, base | 0.15 (0.14) | 0.15 (0.10) | 0.13 (0.09) | 0.45 (0.38) | 0.38 (0.34) | 0.01 | 18.5 |
| turn, this branch | 0.14 (0.12) | 0.13 (0.09) | 0.10 (0.09) | 0.39 (0.38) | 0.35 (0.33) | 0.01 | 18.7 |
| fight, base | 0.23 (0.28) | 0.22 (0.25) | 0.20 (0.21) | 0.58 (0.62) | 0.73 (0.79) | 3.68 (3.26) | 9.8 |
| fight, this branch | 0.12 (0.12) | 0.14 (0.15) | 0.20 (0.20) | 0.51 (0.50) | 0.58 (0.56) | 1.97 (1.57) | 12.8 |

- Two earlier fights on base read `bodies` 7.28 (6.12) at 8.6 fps, and
  one on the ragdoll change alone 1.72 (1.52) at 10 fps. The fight's
  other stages move with the bot it follows, more than with the cuts.
- On mp_crossfire the walk is a small part of `cull`, so the turn's
  `cull` falls by 0.01-0.02 ms; mp_overgrown's walk was five times
  crossfire's.
- What is left on a moving view, all above 0.1 ms: `encode` and
  `finish` (wgpu's recording of every world draw, no bundle holds),
  `hud` (0.3), `gather`'s index copy and the entity uploads in `build`.

Frames: `VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (95
tests, 67 frames), base once and this branch twice. The two runs of this
branch differ on 22 frames (section 29's live-session list and
`crash_sedan`); base against this branch differs on the same 22 and on
`saw_mounted` (one pixel by one level, section 31's two-state frame).
The other 44 hold byte for byte: no fixed view moved with the small-
portal rule, and no fixed frame holds a ragdoll. VERIFIED (frames).

Pending, on a real card, after `cargo build --release -p vcod
-p vcod-server`, base and this branch alternately:

```
SESSION=1 COD4_DIR=<install> TURN=180 GAMETYPE=war BOTS=0 \
  SERVER_ARGS="--set sv_cheats=1" tools/cod4_perf.sh /tmp/turn mp_crossfire 30 \
  --exec-in-game "setviewpos 5232 -888 180 225 0"
```

reading `cull`, `gather`, `build`, `encode` and `finish`; then
`BOTS=12 GAMETYPE=dm` without `TURN` and the view, for `bodies`. A
by-eye check: a corpse falling at a high frame rate should look as it
did; at a low one it now takes fewer, longer steps.

## 37. The sedan's fill: its prepass set and its glass (P26)

A real card at section 35's sedan spot with the eye raised to
standing height (`VIEW="-279 1396 293 -15 28"`, 1920 x 1080, one sample):
`base` 187 fps, `gpu.ents` 3.73 ms, 10.99 M fragments; `zfeather0` 314
fps, `ents` 1.93, 6.40 M; `fxdraw0` 329 fps, `ents` 1.88, 4.47 M. Retail
draws the spot at 220-260. The view is the sedan's side filling the
screen, its windows over the seats.

### What drew only with the feather on

`SWEEP=1` there on lavapipe (`PASSES="base zfeather0 fxdraw0 base2"`,
`VCOD_FRAG_STATS=1`), the main pass's fragment shader invocations by
kind, mean per frame. VERIFIED (ours):

| build | pass | world | ents | fx | fxsoft | `gpu.ents` ms | `gpu.prepass` ms | fps |
|---|---|---|---|---|---|---|---|---|
| before | base | 2173511 | 5836462 | 0 | 1268891 | 112.6 | 4.1 | 4.44 |
| before | zfeather0 | 468739 | 3879983 | 995032 | 0 | 56.6 | 7.4 | 6.58 |
| before | fxdraw0 | 468739 | 3879983 | 0 | 0 | 53.1 | 7.7 | 6.76 |
| stub sets resolved | base | 467410 | 3879983 | 0 | 775048 | 66.5 | 12.8 | 5.44 |
| stub sets resolved | zfeather0 | 468739 | 3879983 | 1419129 | 0 | 60.8 | 7.6 | 6.14 |
| and culled | base | 467426 | 2792511 | 0 | 1021032 | 56.3 | 8.7 | 7.86 |
| and culled | zfeather0 | 468755 | 2792511 | 989476 | 0 | 56.6 | 6.6 | 7.85 |
| and culled | fxdraw0 | 468755 | 2792511 | 0 | 0 | 55.9 | 7.5 | 7.48 |

- No effect drew under `ents`. With the feather on the sedan dropped out
  of the depth prepass: the `floatZ` prepass (section 35) takes an
  entity surface only when its material is in 1.7's set
  (`Material::floatz`), and the sedan's materials read prepass key 3.
  `cod4_fly::load_from` resolves the stub technique sets
  (`Map::resolve_material_orders`) before `entity_models` and
  `sound_and_fx` add the entity and effect materials (`Map::add_material`),
  and the sedan's are stubs in mp_crash's zone, so they kept key 3 and no
  `build floatz` technique. Out of the prepass, the sedan's body drew
  under LessEqual instead of EQUAL, and the world behind it, which the
  prepass no longer hid, shaded under EQUAL before the body covered it:
  1.7 million more world fragments and 1.96 million more entity ones,
  the real card's 4.6 million and 1.85 ms. VERIFIED (the counts; the
  sedan's 20 opaque surfaces under EQUAL, before 0 of them when the
  feather is on, per `cod4_live_sedan_entity_fill`).
- The resolve now runs again once every material is in
  (`cod4_fly::load_from`, after the entity models and the effects). The
  rows after it: `world` and `ents` the same with the feather on and off.

### What 1.7 draws there

- The sedan (`vehicle_80s_sedan1_red_destructible_mp`, 32 drawn surfaces
  at this view after its part bits hide the wreck's) is 20 opaque
  surfaces in `mc/mtl_80s_econ_red`, `mtl_80s_econ_green_rubber`,
  `mtl_engine`, `mtl_80s_econdestroy_green` (`mc_l_sm_r0c0n0*`, prepass
  key 0, a `build_floatz_dtex` technique), and 12 blended ones,
  `mc/mtl_80s_econ_glass_outside` (sort 39) and `_glass_inside` (sort
  38), `mc_l_sm_b0c0s0`, state `(One, InvSrcAlpha)`, alpha test `> 0`,
  back cull, no depth write, prepass key 3. VERIFIED (the zone, a
  scratch dump of the model's materials through `MaterialOrder`). So 1.7
  lays the body into `floatZ` and its depth in the prepass, and draws the
  glass after, as vcod now does.
- The glass is two coincident sheets a window, the outside one facing
  out and the inside one facing in, both back culled (`GFXS0_CULL_BACK`,
  which `s_cullTable` hands D3D as `D3DCULL_CCW`; cod4-world.md 8). From
  outside the car 1.7 draws the outside sheet of the near windows and,
  through them, the inside sheet of the far ones. INFERRED (the states;
  the entity draw takes its material's state as the world's does).
- vcod drew every entity two-sided (`ent_pipeline`'s `cull_mode: None`,
  kept for CoD1's guns and hands), so each window drew both sheets: per
  surface the inside and outside sheets read the same 74953, 26325,
  389959 and 966351 fragments (`cod4_live_sedan_entity_fill`, lavapipe,
  1920 x 1080). VERIFIED (ours). A CoD4 material whose lit state culls
  back faces now culls on a world entity (`Material::entity_cull`, the
  entity pipelines' fourth key, the prepass and `floatZ` pipelines in a
  culled pair too); the view model stays two-sided.
  `VCOD_C4_ENTITY_CULL=0` keeps the old draw. The sedan's fragments fall
  from 4985775 to 3479892, the glass's from 2.96 to 1.43 million.

### The rest of `ents` there

After both, the sedan draws 2.05 million fragments of body (one shading
a pixel under EQUAL, about a screen) and 1.43 million of glass (two
blended layers where a near window shows a far one): the same surfaces
and layers as 1.7. Its body's 1.7 technique on a sun-shadowed model,
`lp_sm_sun_r0c0n0s0_sm3` (technique 9), is 108 pixel shader
instructions with 17 texture reads (the colour, normal and specular
maps, the lighting volume, the probe and the sun shadow map's taps),
where vcod's `cod4_model` and `cod4_sun_pcf` read 4 maps and 4 shadow
taps. VERIFIED (`--example techset_shaders` on `mc_l_sm_r0c0n0s0` and
`tools/re/d3d9dis.py`; vcod's WGSL). So the remaining `ents` is the
sedan filling the screen at retail's per-pixel work or less; nothing
drawn twice is left there.

### Frames

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (97 tests, 93
frames), the cull on twice and off twice. 36 to 40 frames differ
between two runs of one mode (the live sessions' spread, section 29).
Ten differ in both on-off pairs and never between runs of one mode:
`scope_bare` (4473 pixels), `dof_off`/`dof_on` (394), `scope_live`
(412), `tracer_off` (219): a parked sedan's windows, one glass sheet
instead of two, the seats now seen through them; `crash_lit_sun`,
`crash_lit_shade`, `mp_crash_prepass_0`/`_1` (5 pixels on a body's
silhouette, a back face no longer drawn) and `saw_mounted` (one pixel,
section 31's two-state frame). `sedan_fill` moves by up to 243 levels
over the windows, beyond its 32-level spread. VERIFIED (frames).

### Pending on a real card

On the real card, the same sweep with the entity cull on and off:

```
for c in 1 0; do VCOD_C4_ENTITY_CULL=$c SWEEP=1 SESSION=1 \
  COD4_DIR=<install> SIZE=1920x1080 VCOD_FRAG_STATS=1 \
  VIEW="-279 1396 293 -15 28" PASSES="base zfeather0 fxdraw0 base2" \
  tools/cod4_perf.sh /tmp/sedan$c mp_crash 10; done
```

`base` and `zfeather0` should now read the same `ents` and fragments.
By eye: the sedan's windows against retail's at the same spot (one
glass layer, the seats visible).

## 38. The real card's CPU frame: binds, shadow casters, ragdoll skins (P25)

A real-card fight (bundle 10b339d0, `BOTS=12 SIZE=1920x1080
SESSION=1`, dm on mp_crossfire, following a bot) ran at 280 fps: wall
3.57 ms, `gpu.total` 3.07, the CPU stages summing to the wall. `finish`
0.67, `encode` 0.44, `submit` 0.33, `bodies` 0.52 (p95 1.11), `hud`
0.23; 691 draws, 857 bind groups. Retail plays the map at 190-230 fps.
Everything here is vcod's own code; no binary address is involved.

### The counts

The profile adds three counts: `calls` (draw calls in the depth prepass
and the main scene pass, a bundle's counted; `draws` counts the sorted
items, a joined entity run once), `shadowdraws` (the sun and spot
shadow passes' calls) and `huddraws` (the HUD's texture runs).

### Where `encode` and `finish` go

Scratch timers (release, not kept) over the lavapipe fight, the frame's
encoder cut into four command buffers, ms a frame. VERIFIED (scratch
timers, ours):

| part | encode | finish |
|---|---|---|
| shadow maps, cookies, sun and spot shadows | 0.22-0.31 | 0.08-0.17 |
| depth prepass | 0.03-0.07 | 0.03-0.08 |
| main scene pass | 0.08-0.22 | 0.17-0.50 |
| post passes, HUD | 0.02 | 0.04 |

Inside the shadow part, before this change: the static models' LOD
picks 0.06-0.07, the sun partitions' static layer keys 0.07-0.09 (a
cull test and a hash over every caster static model, each partition,
each frame, to decide the layer holds), the entity casters 0.01, the
spot tiles 0.02-0.05. `finish` follows the main pass's commands.

`cull_census`' turn now also counts the steps that keep the set of draw
batches (material and lightmap): at one degree a step on mp_crossfire
40.3%, at 0.3 degrees 68.7%, against 11.1% and 20.0% for the soup set.
A world bundle keyed on the batch set would still have to rebuild its
index ranges each frame. VERIFIED (`cull_census`, ours).

### What changed

1. Entity draws in the main pass and the prepass take their instance
   as `first_instance` over the whole instance buffer, which is bound
   once per run; a model's vertex and index buffers stay bound across
   its instances. An entity stage rebinds its image group and its
   parameter group (with its dynamic offset) only when they change
   (`EntBound`, `renderer.rs`). Same draws, same order.
2. The sun and spot passes' entity casters (`draw_entities`,
   `cod4_sunshadow.rs`) bind the instance and bone buffers once, set
   pipeline, image and buffers only where they change, and join an
   instance's next surface on the same buffers and image when its
   indices follow on: a depth-only pass keeps the nearest depth in any
   order, so the texels are the same.
3. A sun partition's or spot tile's static layer keeps the static
   models it drew and their LOD picks. While its other inputs hold, the
   same models pass its cull, so the layer holds while their picks do:
   a compare instead of the cull and the hash.
4. A ragdoll keeps its skins against a version its step and an
   explosion bump, the rig and the corpse's transform: 1.7 steps it 17
   ms at a time (section 36), so at 280 fps it moves one frame in five
   and an asleep one never; the other frames reuse its matrices, which
   are the last step's pose (cod4-player-anims.md 10.2).
5. The grenade indicators' ceiling trace (the ceiling probe) runs only
   when a frag passes the indicator's other tests. It was traced every
   frame (0.02-0.08 ms on lavapipe's scratch timers).

### Numbers

`tools/cod4_perf.sh`, lavapipe, 1920 x 1080, one sample, mp_crossfire,
30 s, base (10b339d0) and this branch alternated. Turn: `TURN=90`,
`war`, no bots, `sv_cheats 1`, `setviewpos 5232 -888 180 225 0`. Fight:
dm, 12 shooting bots, the spectator following one; each run follows
another bot, so the fight's draws vary from 686 to 1339 between runs.
Mean ms. VERIFIED (lavapipe, ours):

| run | draws | bind groups | encode | finish | build | hud | bodies |
|---|---|---|---|---|---|---|---|
| turn, base | 410 | 438 | 0.46 | 0.39 | 0.13 | 0.29 | 0.01 |
| turn, this branch | 409 | 401 | 0.43 | 0.35 | 0.11 | 0.25 | 0.01 |
| fight 1, base | 1339 | 1511 | 0.78 | 0.82 | 0.27 | 0.21 | 2.25 |
| fight 1, this branch | 700 | 892 | 0.57 | 0.73 | 0.25 | 0.26 | 2.92 |
| fight 2, base | 686 | 905 | 0.56 | 0.62 | 0.24 | 0.29 | 2.07 |
| fight 2, this branch | 1014 | 989 | 0.50 | 0.60 | 0.22 | 0.19 | 1.90 |
| fight 3, base | 730 | 977 | 0.54 | 0.62 | 0.21 | 0.32 | 1.69 |
| fight 3, this branch | 1145 | 1029 | 0.47 | 0.64 | 0.23 | 0.20 | 1.13 |

- Bind groups per draw item: base 1.07 (turn) and 1.13-1.34 (fights),
  this branch 0.98 and 0.90-1.27. The draw calls themselves do not
  change; on lavapipe `encode` and `finish` move by 0.03-0.07 ms at the
  same view, within a run's spread in the fights.
- `bodies` on lavapipe is the ragdolls' steps at 6-8 fps (section 36);
  at the card's 280 fps most frames step none, which is where item 4
  saves (a scratch timer over the fight: 0.36 ms a frame posing about
  14 live bodies, 0.7-1.7 ms the ragdolls including their steps).

### Not done

- Multi-draw indirect and push constants. The world's draws change
  material groups between draws (two bind groups each), so a multi-draw
  needs the materials bindless (texture arrays), a shader and layout
  change across every lit pipeline; push constants would replace the
  dynamic-offset stage parameters, one bind each, not the material
  groups that dominate.
- A bundle for the entities or the prepass: their bones and instances
  are rewritten every frame but bound by buffer, so a bundle could hold;
  their command lists change whenever a body enters, leaves or changes
  LOD, and the fight's list changed on most lavapipe frames. Worth
  trying with the real card's counts.
- Posing only seen bodies: an unseen body still casts a sun shadow and
  fires its tags (section 31).

### Frames

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4_live` (96 tests, 92
frames, two test threads), base twice and this branch once. The two
base runs differ on 32 frames; base against this branch on 31, every
one of them among those 32. The other 60 hold byte for byte. VERIFIED
(frames).

### Pending on a real card

On the real card, after `cargo build --release -p vcod -p vcod-server`,
base and this branch alternately:

```
COD4_DIR=<install> BOTS=12 SIZE=1920x1080 SESSION=1 \
  tools/cod4_perf.sh /tmp/t6b mp_crossfire 30
```

reading `encode`, `finish`, `submit`, `bodies`, `hud` and the new
`calls`, `shadowdraws` and `bindgroups`; then the fixed turn above
(`TURN=180 GAMETYPE=war BOTS=0 SERVER_ARGS="--set sv_cheats=1" ...
--exec-in-game "setviewpos 5232 -888 180 225 0"`), whose draws hold
from run to run. By eye: a ragdolled corpse should fall as before.

## 39. The live frames repeat run to run (TOOL-LIVEDET)

Every "byte for byte" claim above rests on the frames two runs of one
build agree on. At d06a1335 two runs of `VCOD_C4_SHOTS` over `cargo test
-p vcod --bins cod4_live` (97 tests, 85 TGA frames and 8 `floatZ` dumps)
differed on 45 of the 93 files and held 48; every one of the 45 came from
a test that joins a live vcod-server, none from the 60-odd held-view
renderer frames. VERIFIED (frames, lavapipe, ours).

### The causes, and what each fix bought

| Cause | Fix | Files held across two runs |
|---|---|---|
| The renderer, heli and SAW live tests (`renderer::cod4_{worldfx,distort,dof,scope,spot,spot2}_live`, `cod4_vehicles`, `cod4_client::saw_tests`) ran vcod-server in a loop on `Instant::now` beside a client thread on the wall clock: how many server frames passed between two client frames, and so which snapshot a capture saw, rode lavapipe's frame time and the other test threads | `play::cod4::tests::run_held`: the server and the client on one `tests::sim` clock held for the client's whole run, so only the client's 16 ms sleeps move it; each frame loop sleeps first (a pass that `continue`d past its sleep ran again at the same time for ever); the SAW test's spectator steps inside the gunner's frames instead of on a thread of its own | 48 to 84 |
| `Cod4Client::now_ms` (the live session's `step` clock) and the script menus' clock read `Instant` themselves, wall time even under the sim clock: the SAW gun's jitter frames and the heli test moved with it, and the heli test failed every run on the sim clock (no `explosions/clusterbomb`) | both read `play::cod4::clock_now`, the wall's outside a test, the sim's inside one. A client change with no effect in play | 84 to 90 |
| The bots' nav graph builds on a thread and lands when that thread is done, so the spot tests' bot set off at a wall-time-chosen tick (its legs in another phase, section 24) | `run_held` sets `wait_for_graph`: the graph lands on the tick that asks for it | 90 to 93 |

The middle and last rows split by one extra run each: the harness alone
held 84 (saw, heli, `scope_hip`, `tracer_on` and the three `spot_*`
frames still moved); the spot pair with the graph wait off held its own
two runs but drew other bytes than with it on, where a full run's build
races the other tests. VERIFIED (frames).

After all three: two runs at d06a1335 held 93 of 93 files byte for
byte, and three runs of one build of the branch merged with
its base (`cargo test -p vcod --bins cod4`, 295 tests,
95 `cod4_live` files) held all 95, about 520-540 s of wall each at four
test threads (430 s before for `cod4_live` alone: the held clock runs
the client and the server one after the other). VERIFIED (frames).

### The SAW test's flake

`cod4_live_saw_gunner_view` failed about one run in three with "9 client
rounds, 27 server rounds". Its client counted fire events off the
snapshots it read; on the wall clock a slow lavapipe frame let several
snapshots go by, and the client's step ran on its own wall `now_ms`
besides. Held, every 16 ms client frame reads every 50 ms server frame's
snapshot: five runs alone and two full runs passed with the same three
frames each time. VERIFIED (the test, ours).

### What is left

- `play::cod4_menus`' `tableLookup` random row and the join's qport read
  `SystemTime`; neither reaches a frame these tests write. INFERRED.
- The audio voices' end times run on `Instant` (`audio::voices`); the
  tests' mixers are silent and draw nothing off them. INFERRED.
- `cod4_join_menu_draws_offscreen`'s `join.tga` (not a `cod4_live`
  test) differs on each run: its browser pings a wall-clock server
  thread and draws the ping it measured. INFERRED.
- A new live test that writes frames goes through `run_held` with the
  `tests::{clk_now, clk_sleep, since}` clock and sleeps at the top of
  each frame loop.

## 40. Entity runs and the depth prepass from render bundles (P32)

Section 38 left the entities and the prepass out of section 31's
bundles. Their instances and bones are rewritten every frame but bound
by buffer, so a bundle holds while the command list does: the same
models at the same LODs in the same instance slots, in view, on the
same skins. Everything here is vcod's own code; no binary address is
involved.

### What changed

1. `WorldBundles` (`renderer.rs`) keeps any pass's runs: the main
   pass's world runs as before, its entity runs (the non-gun entity
   items between two other items or stops), and the depth prepass's
   world draws and its entity surfaces, each pass with its own targets
   (the prepass has no colour target, or `floatZ`'s R32F). A run
   bundles on the second frame its commands held, as section 31's.
   `draw_ent_stages` and the prepass write through `DrawSink`, so a run
   records its commands or encodes them into the pass from the same
   code. Same draws, same order; a bundled run binds its camera, bones
   and instance buffer itself, two bind groups a run, as the old loop
   did after any non-entity item. CoD4 only; `VCOD_ENT_BUNDLES=0` turns
   it off (`VCOD_WORLD_BUNDLES=0` still turns off the world's).
2. The profile counts `entcalls` (entity draw calls in the prepass and
   the main pass, the gun's left out), `entbundled` (of them, those
   from a bundle) and `prebundled` (prepass world draws from a bundle).

Not done: a run split per body. An entity's instance index is its draw's
`first_instance`, and the instance list is every placed entity in LOD
order, so a body that enters or leaves the list (spawned, freed, past its
last LOD) renumbers every body after it and changes every sub-list behind
it; only frustum and LOD switches would keep the others. With two thirds
of the calls already bundled in the fight (below), what a split could add
is under a tenth of `finish`'s entity share.

### How often the lists hold

`tools/cod4_perf.sh`, lavapipe, mp_crossfire, 30 s, the last two
thirds of each run's frames. Fight: dm, 12 shooting bots, the spectator
following one. Held: war, no bots, `sv_cheats 1`, `setviewpos 5232 -888
180 225 0`. Turn: the same with `TURN=90`. Means a frame. VERIFIED
(lavapipe, ours):

| run | size | fps | entity calls | bundled | prepass world bundled |
|---|---|---|---|---|---|
| fight | 320 x 180 | 55 | 35.0 | 67.1% | 3.5 of 18.2 prepass draws |
| fight | 1920 x 1080 | 8 | 87.8 | 59.0% | 6.1 of 49.2 |
| held | 320 x 180 | 64 | 32.3-38.7 | 99.8% | 7.5-9.0 of 16.8-20.1 |
| turn | 320 x 180 | 80 | 9.0-10.4 | 35.8-36.0% | 0.1 of 7.1-8.1 |

A fight's list holds between most frames even at lavapipe's frame
rates, where a frame is 18-125 ms of bot movement; at the card's 280
fps a frame is 3.6 ms, so it holds longer. The prepass's world draws are
one run with the view's soup set in its ranges, so they hold only while
the view does (the held view's other prepass draws are its entities).
The turning view keeps a third of its few entity calls: the bodies that
stay in view keep their run's commands only when no other one comes or
goes.

### Numbers

Same runs, `VCOD_ENT_BUNDLES=0` and on alternated. Mean ms. VERIFIED
(lavapipe, ours):

| run | size | encode | finish | calls | entcalls |
|---|---|---|---|---|---|
| held, off | 320 x 180 | 0.168, 0.143 | 0.186, 0.161 | 235, 220 | 39.6, 37.2 |
| held, on | 320 x 180 | 0.151, 0.126 | 0.168, 0.150 | 230, 192 | 38.7, 32.3 |
| turn, off | 320 x 180 | 0.162, 0.168 | 0.142, 0.146 | 122, 116 | 10.2, 9.7 |
| turn, on | 320 x 180 | 0.172, 0.163 | 0.150, 0.137 | 123, 107 | 10.4, 9.0 |
| fight, off | 320 x 180 | 0.162 | 0.214 | 217 | 21.5 |
| fight, on | 320 x 180 | 0.175 | 0.194 | 202 | 35.0 |
| fight, off | 1920 x 1080 | 0.589 | 0.654 | 726 | 129.7 |
| fight, on | 1920 x 1080 | 0.651 | 0.718 | 604 | 87.8 |

- The held view's `encode` and `finish` fall by 0.01-0.02 ms each, run
  for run, at the same draws: its entity and prepass calls all replay.
- The turn moves within its spread either way: it bundles few calls
  and makes a bundle about one frame in five (`rebundled` 0.2).
- Each fight follows another bot, so its draws differ by more than the
  cut (21.5 against 35.0 entity calls). Its `finish` per call moves
  from 0.99 to 0.96 us at 320 x 180 and from 0.90 to 1.19 at 1920 x
  1080, where a frame is seven times longer, the lists change more and
  a bundle is made on most frames (`rebundled` 1.0, 0.2 off);
  the card's 280 fps is the case to read, not these.

### Frames

`VCOD_C4_SHOTS` over `cargo test -p vcod --bins cod4` (295 tests, 96
files, four test threads) on this branch merged with section 39's held
clock, `VCOD_ENT_BUNDLES=0` once and on once: 95 of 96 files hold byte
for byte. The one that differs is `join.tga`, the browser's measured
ping (section 39, "What is left"). Before that clock landed, two runs
off differed on 45 of 93 `cod4_live` files and on against either off
run only on files among those 45. VERIFIED (frames).

### Pending on a real card

On the real card, after `cargo build --release -p vcod -p vcod-server`,
with `VCOD_ENT_BUNDLES=0` in the environment and without, alternately:

```
COD4_DIR=<install> BOTS=12 SIZE=1920x1080 SESSION=1 \
  tools/cod4_perf.sh /tmp/p32 mp_crossfire 30
```

reading `encode`, `finish`, `submit`, `calls`, `entcalls`, `entbundled`,
`prebundled` and `rebundled`; then the held view (`GAMETYPE=war BOTS=0
SERVER_ARGS="--set sv_cheats=1" ... --exec-in-game "setviewpos 5232 -888
180 225 0"`), whose draws hold from run to run. If the fight's `finish`
rises with it on, the bundles are made more often than they replay
(`rebundled` against `entbundled`), and the per-body split above is not
the fix: a longer hold before a bundle is.
