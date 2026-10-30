# CoD4 client frame time

How the vcod CoD4 client spends a frame in live play, measured with
`tools/cod4_perf.sh`, and what was cut. A measurement log of vcod's own
code: where a section names a retail fact it carries its iw3mp.exe
address and label; everything else is ours. Most numbers are from a
lavapipe (llvmpipe) adapter, whose GPU columns are CPU rasterization:
they rank GPU work, they are not a real card's milliseconds. Real-card
numbers say so.

Superseded rounds are kept as one line each with their headline delta;
the current state of each piece is in the latest section that touches
it.

Current real-card reference points (1920 x 1080, one sample):

| Where | vcod | Retail 1.7, same PC | Section |
|---|---|---|---|
| mp_crossfire dm fight, 12 bots, following a bot | 280 fps, wall 3.57 ms, `gpu.total` 3.07 | 190-230 fps | 38 |
| mp_crash sedan spot, standing (`-279 1396 293 -15 28`) | 187 fps base, 314 `zfeather0`, 329 `fxdraw0` (before 37's fixes) | 220-260 fps | 37 |

## 1. The tool

`VCOD_FRAME_PROFILE=<path>` on `vcod` (`crates/client/src/frame_prof.rs`)
writes per drawn frame:

- CPU ms of each stage (exclusive; nested spans pause their outer one):
  `pace` (the `com_maxfps` sleep), `net` (the session's step: packets,
  snapshots, prediction, menus), `scene`, `bodies` (player bodies posed
  and skinned), `pose` (other models and brush models), `vm`, `events`,
  `fx` (the session's effect starts), `fxstep`, `sound`, `fxdraw` (quads,
  clouds, marks, lights handed over), `hud`, `ui`, `cull` (the vis walk),
  `gather` (visible indices gathered, ordered and uploaded, dlights),
  `build`, `acquire`, `encode`, `finish` (wgpu's `CommandEncoder::finish`,
  section 31), `submit`.
- Counts: draws in the merged list, model instances, particles, world
  triangles, visible soups, and later additions: `entdraws` (7),
  `feathered` (19), `pipelines`, `bindgroups` (21), `bundled`,
  `rebundled` (31), `calls`, `shadowdraws`, `huddraws` (38), `entcalls`,
  `entbundled`, `prebundled` (40).
- A few frames late, GPU ms per pass group off timestamps between passes
  (needs `TIMESTAMP_QUERY_INSIDE_ENCODERS`): `vcshadow` (vc_shadows),
  `cookies`, `sunshadow`, the main scene as `opaque` (main pass up to the
  split), `split` (shadow mask, SSAO, depth and scene copies), `blend`
  (main pass after the split), `post` (post passes and HUD), and `gamma`
  (gamma and present blit). `cod4_perf.py` sums the four scene groups as
  `gpu.scene` and prints the counts of the GPU-slowest 5% of frames beside
  the mean's. The finer in-pass groups are section 25's.
- `VCOD_FRAG_STATS` (or the profile) adds a `p <frame> world ents gun
  marks fx fxsoft dlight` line: the main pass's fragment shader
  invocations by draw kind (a pipeline statistics query per run of one
  kind, `frame_prof::FragStats`). A fragment is a pixel's worth whatever
  the MSAA, so a count over the screen's pixels is its overdraw (20).

`tools/cod4_perf.sh <outdir> <map> <secs>` runs vcod-server `--game cod4`
with 12 shooting bots and a windowed client joined as a spectator, both in
a loopback-only `unshare -rn`, the client in a nested `kwin_wayland
--virtual`, and prints `tools/cod4_perf.py`'s summary (mean, p50, p95, p99
per stage; `cod4_perf.py <outdir>` reads its `profile.txt`).

- On `dm` (the default) the spectator follows a bot, so two runs differ;
  on `war` it stands still at the spectator spot, repeatable run to run
  (two mp_crossfire runs: GPU scene 1072 and 1019 ms). Use `war` for a GPU
  A/B, `dm` for play.
- Extra arguments go to `vcod`; `--exec-in-game "set <cvar> <v>"` toggles
  a setting for an A/B. `SERVER_ARGS` goes to vcod-server (`--set
  sv_cheats=1` for `setviewpos` and the cheat dvars). `BOTS=`,
  `GAMETYPE=`, `TURN=` (36), `SIZE=` and `SWEEP=1` (25).
- `SESSION=1` runs both in the current desktop session: the client a
  window on the desktop's own display and GPU (no namespace, no nested
  kwin, whose private dbus session tripped over a desktop's portals), the
  server on 127.0.0.1:`PORT`.

The held view used throughout: mp_crossfire war, `SERVER_ARGS="--set
sv_cheats=1"`, `--exec-in-game "setviewpos 5232 -888 180 225 0"`, the
main street from the east end, among `cull_census`' heaviest views.

## 2. Every stock map (dm, 12 bots, 640x372, 4x MSAA, 25 s, the first 5 s after the join dropped)

The first-pass baseline over all 21 maps, measured before the cuts of
section 6 on. CPU ms is the sum of the CPU stages less `pace` and
`submit` (on lavapipe `submit` waits on the rasterizer). mp_cargoship's
row is from after the probe fix (section 4).

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

CPU frame 3.8 to 8.9 ms on every map (110 to 260 fps CPU-bound on a real
card). Heaviest CPU: mp_creek, mp_bloc, mp_crossfire, mp_overgrown
(8.4-8.9 ms); heaviest GPU (lavapipe): mp_crossfire, mp_cargoship,
mp_creek, mp_overgrown.

## 3. What the GPU time follows (mp_crossfire, war spectator, fixed view)

First pass, before section 6:

| Run | GPU scene ms | world tris |
|---|---|---|
| base | 1072 | 427247 |
| base again | 1019 | 427247 |
| `r_zFeather 0` | 1093 | 427247 |
| `r_glow_allowed 0` | 1206 | 427247 |
| `r_dof_enable 0` | 1193 | 427247 |
| `sm_enable 0` (dm, moving view) | 520 vs 618 | 171905 vs 194176 |

Feather, glow and depth of field cost nothing measurable; the scene
followed the world triangles (dm frames at 170k draw in ~500 ms, the war
spot's 427k in ~1050 ms). The sun shadow was 40-90 ms a frame on every
map (cut in 10).

## 4. Fixes

- mp_cargoship crashed the client at load: its 47 reflection probes take
  282 layers in the entity probe cube array, past wgpu's default 256
  `max_texture_array_layers`. The device asks for the adapter's layers
  (cod4-world.md 9.7).
- `--exec-in-game` never ran in a CoD4 game (it waited on CoD1's and
  CoD2's frame clock); it now runs once a CoD4 view draws
  (`Session::cod4_in_game`).

## 5. What to cut next, ranked

The first pass's list, every item since taken:

1. World triangles drawn: sections 6 and 21 (cod4-world.md 9.9, 9.10).
2. The sun shadow pass: sections 10 and 11.
3. `bodies`: sections 7, 16, 31, 36, 38.
4. Effects: sections 6, 32, 33.
5. `gather` and `cull`: sections 6, 31, 36.
6. `events` and `net`: section 16.

The p99 `other` spikes (~12 ms) were first-use uploads inside the
measured window: section 17.

## 6. The cuts from section 5's items 1, 4 and 5

War spectator spot, 640x372, lavapipe. Triangle counts are the signal;
CPU and GPU means moved 20-40% run to run under a shared box.

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

- Below ~250k triangles lavapipe's GPU time stops following them: at 134k
  the war spot still drew in 500-650 ms (fill, MSAA, blended effects,
  post passes).
- Every LOD of every placement is baked, so the vertex buffer grows
  (mp_creek 8.9M vertices, 393 MB; mp_killhouse 6.9M, 303 MB); the device
  asks for the adapter's `max_buffer_size`.
- `cull` was unchanged (0.4-1.3 ms): the walk, not the gather (cut in 36).

## 7. A real-GPU profile: what each stage was (2026-10-27)

Real card, `SESSION=1`, mp_crossfire, 1920x1080, 4 bots, 66 fps: wall
15.1 ms (p95 29.3), bodies 3.6, submit 3.0, acquire 2.3 (p95 15.1),
gpu.scene 12.4 (p95 27.5), 2208 draws. Cuts measured on lavapipe at
480x272 (war, 4 bots, 134873 triangles both runs): fps 1.8 -> 2.3, wall
544 -> 426 ms, `bodies` 4.73 -> 2.64 (p50 3.90 -> 0.31), entity draws 1135
-> 224, GPU opaque 485 -> 376.

1. `bodies`: 96% of a pose was the 1.5/UO engine records' walk
   (`PoseBuffer::blend_tree`'s `walk_records`), which only the 1.5 and UO
   servers' stored-arithmetic bone trace reads. A drawn body poses without
   it (`PoseBuffer::drawn`, `playerpose::pose_player_drawn`,
   `PlayerBody::pose`), bit for bit the same
   (`skeleton::tests::a_drawn_pose_matches_the_recorded_one`). Bone lookup
   is a name map, `bone_world` walks one chain, `posed` composes the
   worlds once for all the rig's models: 124 us a body -> 10.
2. Entity draws: a run of one instance's consecutive surfaces on one
   one-stage skin draws as one `draw_indexed` over its joined index range
   (`renderer::ent_run`; indices rebased onto the shared vertex buffer,
   one bind group per distinct skin, `upload_vm_model`), never across the
   split, the feather copy or the dlights. Same triangles, same order.
3. `gpu.scene` on lavapipe is all opaque pass (split 0.01 ms, blend 0.04,
   post 0.6). Its heavy frames are big outdoor views: the GPU-slowest 5%
   gather 142583 triangles over 5421 soups against a mean of 72691 over
   2590.
4. `acquire`: `r_swapInterval 0` (what `cod4_perf.sh` sets) is
   `AutoNoVsync` with `desired_maximum_frame_latency` 2, so an acquire
   blocks only while two frames are queued; its spikes are the GPU behind
   (acquire p95 15 plus the ~12 ms CPU frame is gpu.scene's p95 27.5).
   Nothing waits at the wrong point.

What a ragdoll costs: section 36.

## 8. Close to a car on mp_crash (playtest 2026-10-26)

Superseded by sections 15, 20, 32, 35 and 37. The playtest's F3 by a
damaged `vehicle_80s_sedan1_red_destructible_mp` (-279 1396 293, yaw -15,
pitch -28) read 10.9 fps, GPU scene 89 ms, 168 world draws, 42 model
instances, 132168 triangles, `fx p913 m108 l7`. VERIFIED (the
screenshot). The car's LOD 0 is eight surfaces, 2608 triangles: the body
(`mc/mtl_80s_econ_red`, 2248), the intact and shattered glass inside and
out (`mtl_80s_econ_glass_outside`/`_inside`,
`com_glass_shattered_outside`/`_inside`, 50 to 94 each, set
`mc_l_sm_b0c0s0`), and the engine, rubber and destroyed parts. VERIFIED
(the zone through `cod4_fly`). All four glass layers drew; retail hides
one pair by `hidePart`, as vcod now does (15, cod4-client-view.md 15).

## 9. The spot light shadow tiles (mp_crash's tiled room)

Real-card playtest: 66 fps with `sm_enable` 0, 54 with 1 under spot light
3 (60 -684 255). A spot tile now draws its static casters into a depth
cache of its own only when its light, casters,
`r_spotLightSModelShadows` or polygon offset change; each frame copies it
into the atlas and draws the entities its light's frustum reaches
(`cod4_sunshadow::SpotStatic`, cod4-world.md 3.5). Lavapipe (war, 4 bots,
1280x772, 4x MSAA, `setviewpos 60 -684 225 67 40`): the held spots' cost
over the sun alone 10-12 ms -> 2.4 ms; `gpu.total` 129-133 -> 115.

## 10. The sun shadow's partitions (mp_crash's tiled room)

A partition's origin snaps to its texels (`sunshadow::setup`), so a still
view keeps its matrix, viewport and cull planes bit for bit. Each
partition draws its static casters into a depth cache of its own only
when one of those or the casters change (`cod4_sunshadow::SunStatic`),
then each frame copies the cache in and draws the entities its planes
keep. The atlas is the same bit for bit (the sun shadow gate encodes a
still view twice). Same setup as 9: `gpu.sunshadow` 39-44 -> 6-7 ms,
`gpu.total` 110-123 -> 75-83 (the held view now costs what `sm_enable` 0
did, 74.1). `gpu.sunshadow` times the atlas passes only; a cache redraw
runs ahead of them. Moving view (dm following a bot): `gpu.sunshadow`
36.9 -> 25.0, `gpu.total` 335 -> 274.

## 11. The shadow casters' static model LODs

The casters keep every LOD and pick each frame at the camera's distance,
as retail's sun and spot passes do (cod4-world.md 3.6). Static model
caster triangles inside the partitions from the spawn views: mp_crash 70k
-> 36k, mp_bog 78k -> 29k. A held view redraws nothing either way; the
moving-view runs could not separate the change from their views' spread,
and CPU `encode` rose 0.27-0.41 -> 0.60-0.65 ms (a draw per picked run).

## 12. The opaque depth prepass (`r_vc_depthprepass`)

vcod's own dvar, default 1, not archived (so a config's 0 is dropped,
cod2-client-perf.md 12); nothing in retail. With it on the frame opens
with a depth-only pass (no colour target) over the opaque lit world draws
(`cod2_world.wgsl`'s lightmapped, static model and model volume entries;
water left out) and the first stage of every entity surface whose stage
is unblended, depth written and LessEqual. The scene pass loads that
depth, and those draws take a pipeline that tests EQUAL and writes no
depth; every other draw, the gun and the dlights keep their state. Draws
that alpha test, carry a polygon offset or blend never take it.
`vs_main`'s position is `@invariant` in both shaders, so the two passes'
depths are the same bits. Under a feather CoD4 runs it as the `floatZ`
prepass (35); CoD2 keeps entities out.

- Pixels: `cod4_live_depth_prepass_draws_the_same_pixels` draws
  mp_crossfire's long view (5232 -888 240, yaw 225: 149759 triangles) and
  mp_crash's sunny spot with the MP5 and a body (124100), 640 x 384, 4x
  MSAA, sun shadow on, off and on: byte-identical frames (719 and 451
  prepass draws). `pipeline_check`'s CoD4 frame draws it single-sampled
  without a validation error. VERIFIED (lavapipe, ours).
- Lavapipe loses 10-40% (held war view: `gpu.total` 309 -> 427): it
  rasterizes on the CPU and already rejects hidden fragments early within
  a mostly front-to-back draw order, so the second pass adds vertex and
  binning work and saves little shading. INFERRED.
- Real card (2026-10-27, mp_crossfire, 1920x1080, 4x MSAA, sun shadows
  on, one run each). VERIFIED (real-card runs, ours):

| run | frames | fps | prepass draws | gpu.opaque mean / p50 / p95 / p99 | gpu.total mean / p99 |
|---|---|---|---|---|---|
| off | 3712 | 85.4 | 0 | 9.92 / 8.78 / 20.93 / 35.39 | 11.27 / 36.88 |
| on | 4073 | 93.0 | 1010 | 7.98 / 7.23 / 13.61 / 41.91 | 9.61 / 43.39 |

  The opaque pass drops 1.9 ms mean (19%) and 7.3 ms p95; the p99 rise is
  one run each, noise until repeated. Default 1 since. At one sample the
  trade may differ (21). INFERRED.

## 13. The world fx: feathered beams and distortion glass

A visible `zfeather` world or static model stage, or distortion glass,
splits the frame for its copies (`renderer::cod4_worldfx`, cod4-world.md
9.5). Lavapipe at mp_crash's lens flare (304 -701 199, yaw 125, pitch 20)
and mp_overgrown's glass bricks (-406 -95 -108, yaw 163, pitch -53):
the glass's copy is `gpu.split`'s 4 ms (the MSAA resolve into the scene
copy), the frame inside the noise; the flare's 15 ms was the bots' noise
(section 14).

## 14. The feather's cost again, without bots

Same flare view, `BOTS=0`, interleaved runs: `r_zFeather` 1 read 183-192
ms, 0 read 175-192, a forced split with nothing feathered 179. The split
and the per-fragment depth fetch are inside lavapipe's 5-10 ms run spread;
nothing cut. A shader that skipped the depth fetch on fragments adding
nothing showed no gain and was not kept.

## 15. Close to a car again, at 1920 x 1080

Real-card playtest by the damaged sedan: 100+ fps elsewhere, about 20 by
its window with `r_distortion` 0 and Soften Smoke Edges off, 11 with both
on. Lavapipe (war, no bots, 1920 x 1080, 4x MSAA, `setviewpos -279 1396
233 -15 28` beside the red sedan at -235 1394 230), the car damaged
through vcod-server's `--script-dir` (a copy of `maps/mp/mp_crash.gsc`
that notifies the nearest car `damage` 2 s in: 45 to each window, 260 to
the body for its smoke, 560 through fire to the explosion). Cracked car
with smoke: `gpu.total` 163.8 -> 153.5 ms, draws 1145 -> 605, entdraws 88
-> 55, particles 946 -> 278. Exploded: 224.3 -> 206.6 (177.6 at `fx_draw`
0).

- "parts": the surfaces of parts `hidePart` hid are not drawn
  (cod4-client-view.md 15): the four glass layers were two too many (33
  more entity surfaces).
- "cull": each effect element's spawn is culled by its `spawnRange` from
  the camera (cod4-effects.md 5). Without a camera the fx system had
  spawned every element, an effect's far LOD on top of its near one. The
  car's own smoke is a few particles (`smoke/car_damage_whitesmoke`: one
  sprite each 0.4 s, 5 s long, about 12 at once); the rest is mp_crash's
  createfx dust, mist and smoke.
- Overdraw at the exploded car (fx quads' screen area summed): 55 screens
  without the cull, 30 with it (`gfx_smk_white_atlas` 43 and 14); turned
  away 8 and 2. Each layer is a feathered, depth-reading sprite at 4
  samples: fill a real card pays and lavapipe's per-pixel cost hides.
- `fx_draw` is 1.7's cheat dvar (registered at iw3mp.exe 0x49943a,
  "Toggles drawing of effects after processing"; VERIFIED); vcod reads
  it: 0 runs the effects and draws none.

## 16. `bodies`, `events` and `net`: per-frame clones and lookups (P3)

Live harness (dm, 12 bots, a `Cod4Client::pump`ed spectator, lavapipe
320x200) and a pose bench (60348 poses). CPU ms mean: mp_crossfire `net`
0.06 -> 0.03, `events` 0.07 -> 0.02, `bodies` 0.62 -> 0.45; pose 9.43 ->
7.34 us a body, the skin matrix and tag digest unchanged.

- `net`: `live_frame` no longer clones the gamestate every frame (the
  script menus borrow it), builds the demo archive row only when a packet
  goes, and a default `PlayerState` only without a snapshot.
- `events`: configstring 7's weapon list is split once per thread, keyed
  by its bytes, handed out as one `Arc<[String]>`; the HUD keeps its ammo
  slots while that `Arc` is the same.
- `bodies`: each body keeps its rig and its `tag_flash`/`tag_brass` bones
  while its models and held weapon stay, and the rig keeps its clips'
  bindings (`skeleton::BindCache`, by clip). CoD2's (`cod2_players`) and
  CoD1/UO's drawn bodies (`entities`, through `Assembly`) take the same
  cache; the servers' traces bind afresh (`PoseInputs::binds` `None`).
- `bodies`' mean and p95 are the ragdolls' (a running ragdoll's step was
  0.26-0.69 ms a frame against 13 us for a live body's pose).
- Left: the blend itself (8 us a body, `node_sums`' three per-bone
  vectors a node) and `scene`'s `ent_u32`, a field looked up by name about
  1500 times a frame (0.05 ms).

## 17. The first frame after the join

The soak (cod4-map-sweep.md, "The soak") found one 600-1300 ms frame on
the first draw after every join. Frame 0 on mp_crossfire, mp_bloc,
mp_killhouse: 763/738/728 ms -> 2-3 ms; frame 1: 436-569 ms -> 0. The
causes: the level's HUD and script menu zones loaded on `pump`'s first
call (610-650 ms), the unused CoD1 Huffman tree built on the first move
(30-42 ms), the level's effects taken (90 ms), and each entity model's
first upload (3-280 ms a model).

`Cod4Client::prewarm` (from `App::cod4_join_frame`, `App::cod4_level`,
after a renderer rebuild, and the soak's `rig`) runs the HUD zones and
menus (`load_hud`), the effects (`take_level_effects`) and uploads every
model of the level's entity model set before the first frame; `pump`
still calls the first two, no-ops once done. The Huffman tree is built
with the gamestate (`Conn::answer_gamestate`) and `clmsg` reads `parse`'s.
The prewarm takes about 1.7 s on mp_crossfire (HUD 0.62, effects 0.09, 37
models 1.0), behind the connect screen. Retail registers the level's
models in `CG_Init`, before its first frame. INFERRED (the Q3 lineage's
`CL_InitCGame`; not read out of iw3mp.exe).

Gate: `play::cod4_sweep::cod4_live_join_builds_its_models_with_the_load`
(a live client beside two bots on mp_crash: once prewarmed, nothing but a
part-hidden model or the stand-in box uploads in three seconds of play).
CoD2 and CoD1/UO upload bodies on first use through their own paths
(section 34 batches those).

## 18. A shot car's first damaged draw

Trap: each hidden-part state (`hidePart`, cod4-client-view.md 15) was
built as its own model, keyed `<name>#<partBits>`, with its own buffers,
textures, bind groups and entity materials, uploaded on its first draw in
play. mp_crash's sedans start with ten later-state tags hidden (`[0x242a,
0x92480000, 0, 0]`, `retail_sedans_draw_with_their_hidden_parts`), so
even an intact car uploaded in play.

`Renderer::dynamic_model_hiding` now makes a handle whose LOD 0
`DynModel` shares the prewarmed model's buffers, bind groups and
`EntSkin`s (each `VmSurface` keeps its source index, `src`) and lists only
the kept surfaces; its bone boxes come from the kept surfaces' vertices.
Nothing goes to the GPU; drawn surfaces, order, materials and index
ranges match the old upload's. `pump`'s CPU in a new state's first frame:
2.4-2.6 ms -> 0.3-0.6. Gate:
`play::cod4_sweep::cod4_live_shot_car_uploads_nothing_in_play` (fires 150
ms bursts at a sedan 160 units off until it shows four states; no dynamic
model LOD uploads past the prewarm's count, `Renderer::dynamic_uploads`).

## 19. mp_crossfire's prepass draws fell from 1010 to 333 (P4)

A real-card run (bundle 0bcbf7dc, dm, 12 bots, 1920 x 1080) read 78.0 fps,
`gpu.opaque` 11.00 (p95 29.49), `acquire` 5.47 (p95 21.5), 1587 draws,
238 entity draws, 333 prepass draws, against section 12's 93.0 fps and
1010. Not a regression: with `r_zFeather` 1 and mp_crossfire's ambient
smoke in nearly every view, the entities stayed out of the prepass on
feathered frames (about 650 of its draws), and the two runs differed in
length, view and feather state. In the range b4e4e170..0bcbf7dc only
4bf35188 moved the counts (part bits, section 18, and the `spawnRange`
cull: draws 1860 -> 1260, particles 593 -> 284). Section 20 since keeps a
CoD4 frame's entities in the prepass under a feather; `feathered` counts
CoD2 frames only.

What makes vcod's opaque pass dearer than retail's, ranked then:

1. MSAA. `r_aaSamples` is registered at iw3mp.exe 0x62b1cb with default
   and minimum 1 (`ebx` 1 from 0x629df3), maximum 16. VERIFIED (the
   disassembly). vcod's default on CoD4 is now 1
   (`Shell::register_aa_samples`); `vcod_mp.cfg` archives `r_aaSamples`,
   and an unstamped config's 4 loads as 1 (cod11-console.md 8).
2. The feather keeping entities out of the prepass: fixed in 20 and 35.
   Retail has its own `r_depthPrepass` ("Enable depth prepass (usually
   improves performance)", registered at 0x62b9b3, a bool, default 0).
   VERIFIED.
3. Draw count: sections 21, 33, 38, 40.
4. The big outdoor views the cull lets through (7 item 3). INFERRED.

Already as retail: the sun shadow's four point taps, anisotropy at
`r_texFilterAnisoMin` 1 (`TextureFilter::cod4_aniso`), BC textures
uploaded compressed, no per-sample shading. INFERRED (the code).

## 20. Soft smoke by the intact sedan, and `sv_cheats` on the client

Real-card playtest at `setviewpos -279 1396 233 -15 28` by mp_crash's
intact red sedan: 15.5 fps by default, 30 with Soften Smoke Edges and
`r_distortion` off, 45 once the car is destroyed; retail 1.7 on the same
PC draws the spot at 220-260 fps, with no smoke or fire on the intact car
(VERIFIED, a screenshot).

- No damage effect plays on an intact car in vcod. The fx quads there
  (a scratch dump through `cod4_worldfx_live`) are mp_crash's createfx
  ambience: `gfx_smk_white_atlas`, `gfx_mist`, the fog wisps, lens
  flares, `gfx_fire_roar_pm`, their centres 390-1290 units off, 0 to 0.67
  screens. The nearest placement is `dust_wind_slow` 274 units off, the
  nearest smoke (`hallway_smoke_light`) 566. VERIFIED (ours).
- Fill on lavapipe (war, no bots, 1920 x 1052, 4x MSAA, `--team
  allies`): 1.5 screens of world (3118026 fragments, constant), under half
  a screen of effects, no feathered sprite. The fill is far too small to
  explain 15 fps, so the playtest's cost is not fill at this view.
  INFERRED. Fx fragments 820993 -> 481619 with this section's change.
- A CoD4 feather copied its depth before the first model, as CoD2's
  `floatZ` (cod2-materials.md 9.5). That split the pass a second time and
  left entities out of the prepass. It now copies at the split, after the
  opaque entities and the gun, and keeps entities in the prepass: 1.7's
  `floatZ` is drawn by the depth prepass over the lit and decal lists
  (`R_DepthPrepassCallback`, `TECHNIQUE_BUILD_FLOAT_Z`, INFERRED), so it
  holds the models too. The copy is one full-screen pass (sample 0 of the
  MSAA depth into an R32F target, `soft_particles.wgsl`). Section 35 then
  replaced it at one sample.
- Trap: the CoD4 client never applied configstring 1, so its `sv_cheats`
  stayed 0 and every cheat dvar (`fx_draw`, `r_glow`) stayed locked;
  `setviewpos` worked only because the server checks it.
  `Cod4Session::take_systeminfo` hands out configstring 1 whenever it
  differs from the last taken (the gamestate, or a later `d 1`), and the
  frame runs it through `Shell::apply_systeminfo`, as
  `CL_SystemInfoChanged` (iw3mp.exe 0x473ab0) does: `sv_cheats` 0 puts
  every cheat dvar back, then every key is force-set; a demo sets none.
  INFERRED (CoD1's `CL_SystemInfoChanged`; 1.7's at that address not
  re-read). Gate:
  `play::cod4::cod4_live_cheat_dvars_follow_the_servers_sv_cheats`.
  VERIFIED (ours).

## 21. Draws, binds and the lit pixel on mp_crossfire (P5)

`renderer::cod4_draws_live::cod4_live_crossfire_draw_census`: vcod-server's
war on mp_crossfire, the session's renderer at 1920 x 1080, one sample,
prepass on, the eye at three views, each frame drawn with
`Renderer::draw_joins` off and on and held byte for byte. `PassCensus`
counts every draw call of the prepass and the main pass by kind and every
`set_pipeline` and `set_bind_group`. The first two views are
`cull_census`' heaviest (151k and 146k triangles against a mean of 34k
and a median of 25k over 5880 spawn views). VERIFIED (lavapipe, ours):

| view | tris | joins | draws | world | ents | fx | prepass world | prepass ents | pipelines | bind groups |
|---|---|---|---|---|---|---|---|---|---|---|
| 5232 -888 240, yaw 225 | 128090 | off | 2174 | 887 | 115 | 86 | 620 | 460 | 78 | 4273 |
| | | on | 1104 | 887 | 65 | 86 | 34 | 26 | 73 | 1423 |
| same, yaw 270 | 135516 | off | 2173 | 896 | 115 | 92 | 604 | 460 | 87 | 4269 |
| | | on | 1121 | 896 | 65 | 92 | 36 | 26 | 81 | 1463 |
| 3990 -1110 60, yaw 180 (median) | 26010 | off | 967 | 171 | 115 | 93 | 122 | 460 | 41 | 1848 |
| | | on | 305 | 171 | 10 | 93 | 21 | 4 | 37 | 402 |

Six sky draws each. Lavapipe's frame time does not move (104-141 ms):
fill, not calls. Behind `draw_joins` (on by default):

1. Prepass world draws join: depth-only LessEqual does not depend on
   order, so per set (world or a moved brush model), sidedness and flag
   wave (`M_FLAG`, the one switch `vs_main`'s position reads) the
   gathered ranges that touch draw as one.
2. Prepass entity surfaces join: an instance's rigid surfaces
   (`EntStageDraw::rigid`: no deform, no flag) whose indices follow on draw
   as one, whatever their skins.
3. World draws rebind groups 1 and 2 only when the draw needs other ones
   (`WorldGroups`, cleared at each pipeline set).
4. An entity whose posed box misses the frustum is skipped
   (`instance_seen`): the box over every LOD's vertices, posed by the
   bones, grown by a unit and a hundredth of its reach for weights that
   sum short of 1. A deforming or waving instance always draws; shadow
   casters keep every instance.

The main pass's world draws are one a batch, keyed by material,
lightmap, probe and primary light (mean 239 batches over 87 materials
across the spawn views). One draw per material would need texture arrays
for group 2 and would change coplanar pixels under LessEqual and EQUAL.
1.7's surface key carries the same four fields (cod4-world.md 12.1), so
its lit list splits where vcod's does. Static models are baked into the
world batches (6), drawn merged where 1.7 instances them. INFERRED.

Techsets' share of the drawn triangles over those views (`vcod-cod4`'s
`cull_techsets`, `CULL_TECHSETS=mp_crossfire`): `mc_l_sm_r0c0n0s0` 25.7%,
`wc_l_sm_r0c0n0s0` 23.8%, `mc_l_t0c0n0s0` 9.9%, `mc_l_sm_t0c0n0s0` 9.1%,
`mc_l_sm_r0c0n0` 7.4%, `wc_unlit_multiply` 7.2%. VERIFIED (ours).

The lit pixel: retail's technique 9 reads seven maps on the world
(`lm_sm_sun_r0c0n0s0_sm3.hlsl`: normal, specular, probe `texldl`,
primary, secondary twice, colour; 19 fetches and 112 other instructions in
the bytecode) and five on a static model (`lp_sm_sun_r0c0n0s0_sm3.hlsl`:
normal, colour, specular, the lighting volume, probe; 17 and 99), plus
four shadow taps, eight in the partition fade band, none where the
primary reads 0. VERIFIED (bytecode, `--example techset_shaders`,
`tools/re/d3d9dis.py`). vcod's `cod4_lit` had read nine (the detail map
whatever `M_DETAIL`, and CoD2's `t_dyn_shadow`, which no CoD4 map draws);
each shader now reads a map only under the switch that uses it, as retail.
VERIFIED (the WGSL; the 14 `cod4_live` frames byte for byte). The model
volume fetch went from two to one in 27.

Retail registers `r_depthPrepass` default 0 (19); its `zprepass`
technique (0) is `transform_only.hlsl` with `null.hlsl`. VERIFIED
(bytecode names).

Pending, real card: `r_aaSamples` 1 against 4 at the held view (each after
`vid_restart`); `r_vc_depthprepass` 0 against 1 at one sample; and a GPU
capture of one held frame (per-draw time of the main pass: fill, vertex
work or idle gaps between small draws; the 887 world draws average 145
triangles). Section 25's sweep runs the first two.

## 22. World draws per material against 1.7's (P6)

1.7 issues about four times as many world calls as vcod at the held view,
so joining vcod's batches further is not where the gap is. VERIFIED for
the dvar; INFERRED for the draw path (control flow):

- `r_pretess` ("Batch surfaces to reduce primitive count", name at
  iw3mp.exe 0x704a18, description 0x7049ec) registers at 0x62a245 as a
  bool with default `bl` = 1 (set at 0x629df3), handle `[0xd5697ec]`.
  VERIFIED (the disassembly).
- The camera walk (0x63c4d0) visits the lit surfaces in `sortedSurfIndex`
  order and starts a draw surface on each change of draw-surf key or every
  128 surfaces (`cmp eax,0x80` at 0x63c5ad). INFERRED.
- With `r_pretess` on (`[0xd5697ec]+0xc` read at 0x63c448), 0x63c220
  copies the run's indices into the pretess buffer and writes an entry per
  change of `firstVertex` (`GfxSurface`+4), lightmap (+0x14) or probe
  (+0x15); surfaces are 0x30 bytes (`lea *3; shl 4` at 0x63c25b).
  INFERRED.
- The lit pretess draw (0x655384 loop) sets the probe (0x6324a0) and
  lightmap (0x631eb0) on a change and the stream at `firstVertex * 0x2c`
  (0x6554b2) for every entry, one indexed draw each. INFERRED.

So 1.7 joins only neighbours in `sortedSurfIndex` sharing a key and a
vertex base; vcod's `build_batches` puts every soup of a key into one
contiguous range at load (absolute u32 indices). `vcod-cod4`'s
`cull_draws` (`CULL_DRAWS=mp_crossfire`, `CULL_DRAWS_VIEWS=...`) over the
soups vcod's walk keeps. VERIFIED (ours; the 1.7 columns run the model
above, so INFERRED for retail):

| view | ours world | ours, 1.7's lightmap key | 1.7 world (r_pretess 1) | 1.7, r_pretess 0 | ours static models |
|---|---|---|---|---|---|
| 5232 -888 240, yaw 225 | 818 | 699 | 3327 | 3389 | 215 |
| same, yaw 270 | 796 | 681 | 3493 | 3540 | 205 |
| 3990 -1110 60, yaw 180 (median) | 130 | 112 | 354 | 358 | 56 |
| mean of 5880 spawn views | 188 | 164 | 575 | 587 | 51 |

- mp_crossfire has 8646 surfaces behind 14920 `sortedSurfIndex` entries;
  3848 sorted neighbours share a `firstVertex`, and no run of one key
  changes vertex base, so with `r_pretess` 1.7 makes one draw per draw
  surface. VERIFIED (the census).
- Not done: vcod splits mp_crossfire's 2 lightmaps into 3 tiles
  (`mapworld::to_mapworld`), adding 119 batches at the held view (818
  against 699). Joining them needs array textures for the pages and a
  layer per vertex in a shader CoD2 shares, for about 15% of world calls.
  INFERRED.
- The live frame's 887 world calls (21) against this table's 818 are its
  fx stages and moved brush models; the difference was not chased.

## 23. The resident set level by level, and the player rigs

Method: the soak (`tools/cod4_soak.sh`, one-minute levels,
`VCOD_SWEEP_GAMETYPES=war,dm,sd,koth,sab` round twice and once more, 12
levels a map, lavapipe), its `soak rss:` line at each load, and a
jemalloc heap profile at each load (`LD_PRELOAD` jemalloc,
`MALLOC_CONF=prof:true,lg_prof_sample:17`; the soak calls `prof.dump`
through `mallctl` when the process has one, `cod4_sweep::jemalloc_dump`),
read with `jeprof --base` level 2 against level 12.

A rotation to another gametype on one map keeps the map loaded
(`Cod4Session::take_map_change`), so `Cod4Players` and its rigs live over
every level of one map; a map change starts them over
(`Cod4Client::new_map`).

mp_crash, live heap at a load (MB, jemalloc's sampled profile):

| | level 2 | level 7 | level 12 | growth 2 to 12 |
|---|---|---|---|---|
| everything, before | 1719 | | 1852 | 133 |
| everything, after | 1672 | 1736 | 1767 | 95.5 |
| player rigs, before | 16.5 | 21.3 | 21.3 | 4.8 |
| player rigs, after | 4.5 | 4.8 | 4.9 | 0.4 |
| test-only row log, before | 4.1 | 36.4 | 71.9 | 68 |
| test-only row log, after | | | 0.1 | 0 |
| sound bank (`SoundBank::get`) | 42 / 36.5 | 67 / 60 | 68.5 / 68.5 | 26 / 32 |

- Rigs keyed by (character, held weapon) each baked their own body and
  head (21 MB once every pair was seen, then flat). `Cod4Players` takes
  its parts from one `RigParts` kept as long as its rigs
  (`PlayerRig::assemble_with`, CoD2's fix, `cod2-client-perf.md` 5.3): a
  new pair costs its skeleton and gun. Gate:
  `a_body_under_two_weapons_shares_its_meshes` (two rigs share every part
  but the gun, and the second rig's pose equals one built in a session
  that never saw the first).
- Trap: the growth was a test-binary artifact. The audio system's `rows`
  (`#[cfg(test)]`, each started voice's row for `AudioSystem::mix`) kept
  every voice ever started, about 7 MB a level; it now follows the live
  voices as `resumable` does. The shipped client never had it.
- Left: the sound bank keeps every decoded file, bounded by the install,
  as on CoD2. The snapshot list (`net::parse::Client::snapshots`, shed at
  twice `KEEP_SNAPSHOTS`) reaches capacity by level 5 and holds (49 MB of
  `Vec` growth inside the first five levels).
- malloc's live bytes on mp_backlot from level 2 to 12: +301 MB before,
  +58 MB after, inside a 100 MB band from level 3 on. Resident set (glibc)
  4.4-4.7 GB with lavapipe holding about 3 GB outside the heap; its drops
  to about 2.6 GB were the run's 10 GB memory cap swapping lavapipe's
  pages. Under jemalloc it wanders 4.8-5.3 GB either way.
- The server's hit-pose rigs (`game::cod4::combat`) key on the character
  without a weapon (one rig per body and head pair) and stay on
  `assemble`.

## 24. The frame's passes and targets against 1.7's (P9)

The passes are not where the real card's gap went: vcod's frame wrote
about 16 MB more than 1.7's, a few hundredths of a millisecond at desktop
bandwidth; the gap was inside the opaque pass (`gpu.opaque` 11.0 of
12.45 ms then).

vcod's frame at the held view (5232 -888 240, yaw 225), 1920 x 1080, one
sample, CoD4's defaults, counted by
`renderer::cod4_passes_live::cod4_live_crossfire_frame_writes` (each
attachment once whatever its overdraw; `FrameWrite`, tests only).
VERIFIED (lavapipe, ours):

| write | target | bytes |
|---|---|---|
| sun shadow atlas: cached static layer blitted into each 1024 partition, entities over it | Depth32Float 1536 x 2048 | 8388608 |
| spot shadow tiles, the same per tile | 512 x 512 Depth32Float a tile | 4194304 |
| depth prepass | Depth32Float 1920 x 1080 | 8294400 |
| scene pass, first half: sky, opaque, decals | Rgba8Unorm + Depth32Float | 16588800 |
| feather depth copy at the split (gone at one sample, 35) | R32Float | 8294400 |
| scene pass, second half: blends, fx, gun, flare | Rgba8Unorm + Depth32Float | 16588800 |
| post pass (film) into the gamma scene, then the 2D (now into the swapchain, 30) | Rgba8Unorm | 8294400 |
| gamma pass into the swapchain (now skipped, 30) | surface sRGB format | 8294400 |
| uploads: camera, entity lights, instances and bones | | 25328 |
| total | | 78963440 |

The median view drew 74769136 bytes (no spot tiles). Neither static
layer redrew on the held frames, and the gathered indices uploaded
nothing. No glow, no depth of field, no distortion copy at these views;
every scene target is 8 bits a channel (`vc_hdr` 1, Rgba16Float, is
vcod's own and off).

1.7's frame:

- The scene is the back buffer. `R_InitFrameBufferRenderTarget`
  (0x630650) asks for `D3DFMT_A8R8G8B8` (0x15 pushed at 0x63066d for
  "Requested frame buffer to be %s", 0x707b3c), shares target 1 into
  target 2 (0x630660..0x630668: `R_RENDERTARGET_FRAME_BUFFER` into
  `R_RENDERTARGET_SCENE`), then makes one full-screen target in the
  device's returned format (image type 9, 0x63070a: the resolved scene).
  VERIFIED (the constants); what the calls do is INFERRED.
- `R_InitRenderTargets` (0x630740) makes a full-screen `D3DFMT_R32F`
  target (0x72 pushed at 0x63078f) under the dvar at `[0xd5695c4]`
  (`r_floatz`, name 0x706320, registered at 0x62b92f), shares the
  resolved scene into two more ids (0x6307bb, 0x6307c8), and makes the
  saved screen in the frame buffer's format, the sun's shadow map (1024,
  two partitions, 0x630822) and the spot's (512, four tiles, 0x630838).
  VERIFIED (the constants); the ids' meanings INFERRED. No floating-point
  colour target besides `floatZ`.
- No gamma pass: `r_gamma` goes to the device's gamma ramp. INFERRED
  (`R_SetColorMappings`).
- With a feathered effect in view (`needsFloatZ`) 1.7 runs the depth
  prepass whatever `r_depthPrepass`, drawing the lit and decal lists into
  `floatZ` and the depth buffer; at one sample the scene then clears only
  its colour. INFERRED (`RB_Draw3DInternal`, `R_DepthPrepassCallback`;
  the addresses are in 35).
- Film and depth of field run as a resolve and one full-screen pass back,
  the glow at a quarter size each way (`RB_ProcessPostEffects`,
  INFERRED): the same two full-screen writes as vcod's post pass.
- `r_vsync` is registered at 0x62b20e (name 0x705a3c), a bool with flags
  0x21 and default `bl` = 1 from 0x629df3. VERIFIED (the disassembly).
  vcod's `r_swapInterval` defaults to 1 (wgpu's `AutoVsync`), as retail.
  One back buffer, `PresentationInterval` 1 or immediate by `r_vsync`
  (`R_SetD3DPresentParameters`, INFERRED); vcod asks for frame latency 2.

1.7 at the held view: 8.4 MB sun map, 4.2 spot tiles, 8.3 depth, 8.3
`floatZ`, 8.3 colour, 8.3 resolve, 8.3 film, about 54 MB plus the shadow
casters' overdraw (1.7 redraws every caster every frame; vcod blits a
cached layer). vcod's extra was the gamma pass and the split's second
store of colour and depth. INFERRED.

Changed, no pixel moves: a single-sampled renderer no longer allocates a
multisampled colour target (`create_msaa_view` makes one texel; 8.3 MB of
video memory at 1080p, 16.6 under `vc_hdr`); the post-process passes keep
their bind groups (`DofPass` until a resize, `Cod2Glow` by source and sky
views, `SceneCopy`'s single-sample blit, `PostPass::bind_glow`).
VERIFIED (36 frames byte for byte; the 19 that differ are live-session
frames, section 39).

## 25. One-command sweep: a setting a pass at one view (P7)

Measures what each optional piece of the frame costs, in one client
session at one held view.

### The command

After `cargo build --release -p vcod -p vcod-server`:

```
SWEEP=1 SESSION=1 COD4_DIR=<install> SIZE=1920x1080 \
  tools/cod4_perf.sh <out> mp_crossfire 10
```

`BOTS=12` adds the fight (12 shooting bots on war; the held view stays
put). It prints one tab-separated row a pass and writes `<out>/sweep.tsv`;
`tools/cod4_perf.py --sweep <out>` prints it again from `profile.txt`.
`base` and `base2` bound the run-to-run drift. `PASSES="..."` picks the
passes, `VIEW="x y z yaw pitch"` another view (only mp_crossfire has a
default), `SETTLE` the wait before measuring (default 3 s).

`SIZE` is the client's window (`vcod --window`, no cvar written): the
nested display is SIZE plus 64 each way and the window opens at SIZE in
both modes; the profile's `# size` line names the size drawn. Before
2026-10-29 it sized only the nested display, so a `SESSION=1` run took the
saved mode and a nested run drew SIZE less kwin's 28-row title bar
(1280x772 at the default).

### What it does

- vcod-server on `war`, no bots, `sv_cheats 1` (for `fx_draw`, `r_glow`,
  `r_fog` and `setviewpos`, which the client honours off the systeminfo,
  section 20); the client joins as a spectator held by `setviewpos 5232
  -888 180 225 0`.
- At the join the client sets every swept dvar to its default and
  `r_aaSamples` to 1 (retail's default; a config may hold 4), then
  `vid_restart`s. Each pass sets its dvar, waits `SETTLE` s, measures
  `<secs>` s, sets it back. `aa4` sets the view again after its
  `vid_restart`.
- The queue is `vcod --exec-after "SECS LINE"`, each line run SECS after
  the last one ran (a `vid_restart`'s rebuild included); every line lands
  in the profile as `# exec <frame> <line>`, and `echo sweep <pass>
  measure|end` lines bound a pass's frames.
- Setting an archived dvar rewrites `vcod_mp.cfg`; the script copies every
  `vcod_mp.cfg` under the install aside first and restores them at exit.

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
| normalmap0 | `r_vc_normalMaps 0` | vcod's |
| fog0 | `r_fog 0` | retail's, cheat |
| glow0 | `r_glow 0` | retail's, cheat |
| dof0 | `r_dof_enable 0` | retail's |
| film0 | `r_vc_film 0` | vcod's |

The vcod-side switches, each 1 (the normal path) by default:

- `r_fog`: 1.7 registers it at 0x62a50c (`Dvar_RegisterNew` 0x56c350,
  name at 0x704cb8, flags 0x80, `DVAR_CHEAT`). VERIFIED (iw3mp.exe). Its
  default 1 and that 0 draws no fog are INFERRED (`R_RegisterDvars`).
  vcod's 0 writes fog mode 0 into the camera block, so no world, model or
  effect shader fogs; the clear colour and the cull keep the fog's.
- `r_vc_normalMaps`: 0 clears `M_NORMAL` in every CoD4 lit material's
  uniform (`Cod2World::set_normal_maps`), so a set lights by its vertex
  normal and reads no normal map. 1.7 has its own `r_normalMap` (name
  at 0x7050e4, pushed at 0x62a86b). VERIFIED (iw3mp.exe). That one swaps
  in a flat map and still reads it. INFERRED (`R_RegisterDvars`). So it
  would not show the read's cost, and vcod does not register it.
- `r_vc_film`: 0 drops the vision set's film grade, so a frame with no
  glow and no depth of field runs no post pass.

### The columns

`fps` and `wall` are the pass's mean frame; `gpu.total` and the pass
groups (`gpu.sunshadow`, `gpu.opaque`, `gpu.split`, `gpu.blend`,
`gpu.post`) are section 1's. Then the finer groups (`frame_prof::Group`,
`q` lines): a timestamp wherever the work changes group, in the encoder
between passes and inside the main pass (`TIMESTAMP_QUERY_INSIDE_PASSES`),
each span charged to the group it opened. Only groups some pass spent
time in print:

- In `gpu.opaque`: `prepass` (the depth prepass), `sky` (the main pass's
  clear and the sky), `world` (world surfaces and the static models'
  unlit and effect stages), `smodel` (the static models' lit sets,
  `fs_cod4_model`), `ents` (entity surfaces), `gun`, `marks`, `fx`,
  `fxsoft`, `dlight`.
- Between and after: `shadowmask`, `ssao`, `softcopy` (the feather's depth
  copy), `scenecopy` (distortion's colour copy), `dof`, `shafts`, `glow`,
  `bloom`, `postfx`, `shock`, `hud`, `flare`.
- `draws` and `prepass` are the merged list's and the prepass's draw
  counts, `frags` the main pass's fragment shader invocations (pipeline
  statistics, `p` lines), `samples` the MSAA count.

Vulkan writes a timestamp at the bottom of the pipe, so a group's time
runs from the end of the last group's work to the end of its own; draws
that overlap on the card land in the later group. The main pass's groups
sum to about `gpu.opaque` plus `gpu.blend`. Every stamp inside a pass
costs a little; `VCOD_GPU_GROUPS=0` drops the finer groups.

### Lavapipe, end to end (2026-10-28)

`SWEEP=1 SIZE=640x400 SETTLE=4 tools/cod4_perf.sh <dir> mp_crossfire 8` on
lavapipe (4 threads), nested kwin, no bots. The milliseconds rank nothing
on a real card; the run shows the columns and what each switch moves.
VERIFIED (lavapipe, ours):

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

(`samples` from a second run of `base aa4 base2`, after the `# size` line
learnt to note a sample change.)

- What each switch moves: `aa4` the samples and every in-pass group
  (lavapipe shades per sample); `prepass0` the prepass draws and group;
  `sunshadow0` `gpu.sunshadow`; `zfeather0` the split, depth copy and soft
  effects (they draw as plain `fx`); `fxdraw0` 116 draws and a quarter of
  the fragments; `film0` the post pass.
- `distortion0`, `glow0` and `dof0` move nothing at this view (no glass
  or sprite to distort, no glow in the vision set, no sighted weapon).
  `specular0`, `normalmap0` and `fog0` change no counter, only pixels:
  `cod4_live_sweep_switches_change_the_frame` draws the held view (640 x
  384, 4x MSAA) under each (`fog0` under a stand-in exp fog): `specular0`
  moves 134543 pixels, `normalmap0` 136553, `fog0` 85858, `film0` 245311,
  and the defaults draw the first frame again byte for byte. VERIFIED
  (lavapipe, ours).
- `base` against `base2`: 50.7 and 51.5, the drift between passes.
- Pending: the command above on the real card, for which of these it pays.

## 26. The opaque shaders' arithmetic against 1.7's (P8)

### The counters

- Retail: `tools/re/d3d9dis.py --count` over the technique passes
  `--example techset_shaders` writes (`wc_l_sm_r0c0n0s0` out of
  mp_crossfire, the `mc_*` sets out of `common_mp`). It counts
  instructions (no `dcl`, `def` or comment), fetches, flow control, and
  slots with the ps/vs 3.0 macros expanded (`nrm`, `pow` 3, `lrp` 2,
  `m4x4` 4, `sincos` 8). Both arms of each branch are in the bytecode, so
  it is the shader's size, not one pixel's path.
- vcod: `renderer::shader_ops` (`SHADER_OPS=1 cargo test -p vcod --bins
  shader_ops -- --nocapture`; `SHADER_OPS=2` adds each function's share).
  It writes a material's uniform switches into the WGSL as literals,
  parses it with naga and walks the IR from the entry point: calls
  inlined, a known branch takes one arm, an unknown one both, a counted
  loop its trips, constants folded. `slots` weights naga's expressions as
  ps_3_0 expands the same macros. It keeps dead code the driver would
  drop (`surface`'s CoD2 normal on a CoD4 path, about 8 slots).

### The table

Technique 9 is `lm_sm_sun_*` / `lp_sm_sun_*` (sun and shadow map), 7 no
primary light, 8 the sun without the map, 11 a spot with its map. `(+n)`
are `textureNumLevels`/`textureDimensions` queries, left out of vcod's
slots. Columns: before this section, after it, and current (after 27's
volume fetch and 28's taps; split per pipeline in 29). VERIFIED (retail:
bytecode; vcod: the counter):

| path | retail ps instr / fetch / flow / slots | retail vs slots | vcod fs fetch / alu / slots before | after 26 | current slots |
|---|---|---|---|---|---|
| `wc_l_sm_r0c0n0s0` t9 | 131 / 19 / 11 / 140 | 37 | 19 (+3) / 202 / 244 | 19 / 152 / 191 | 125 |
| `wc_l_sm_r0c0n0s0` t7 | 41 / 6 / 0 / 48 | 33 | 7 (+3) / 85 / 101 | 7 / 74 / 87 | 87 |
| `wc_l_sm_r0c0n0s0` t8 | 57 / 7 / 0 / 64 | 33 | 7 (+3) / 85 / 101 | 7 / 74 / 87 | 87 |
| `wc_l_sm_r0c0n0s0` t11 | 89 / 12 / 0 / 99 | 37 | 11 (+3) / 170 / 206 | 11 / 159 / 192 | 192 |
| `mc_l_sm_r0c0n0s0` t9 | 116 / 17 / 8 / 125 | 47 | 18 (+4) / 201 / 243 | 18 (+1) / 151 / 190 | 110 |
| `mc_l_sm_r0c0n0s0` t11 | 78 / 10 / 0 / 88 | 47 | - | 10 (+1) / 159 / 192 | 178 |
| `mc_l_sm_r0c0n0` t9 | 92 / 15 / 8 / 96 | 46 | 16 (+3) / 173 / 209 | 16 (+1) / 123 / 156 | 76 |
| `mc_l_t0c0n0s0` t8 | 53 / 5 / 0 / 61 | 43 | 6 (+4) / 85 / 101 | 6 (+1) / 74 / 87 | 73 |
| world `vs_main` (CoD4 set) | | | 0 / 13 / 17 | 0 / 12 / 15 (fog included) | |
| entity `lp_sm_sun` (`dynamic_model` `fs_main`) | as `mc_*` t9 | | one fetch and one size query more | 16 / 368 / 408 | 119 |
| entity `vs_main` | | | 108 alu | 69 alu / 73 slots | |

vcod's vertex stage reads float positions and frames where retail decodes
packed normals (`mad v3, c8.xxxy, c8.zzzw` and the `cross` for the
binormal), so its 15 slots against 33-47 are not a gap.

### What changed

1. Size queries: `sample_image` asked `textureNumLevels` on every fetch.
   The material uniform's free `flags.w` carries which maps have mipmaps
   (`MIP_*`, read off the view's texture at bind) and the world shader
   calls `sample_mipped`. Same pixels.
2. Sun shadow taps: `cod4_sun_pcf` scales `at` once, adds the taps in
   texels and compares a `vec4` of the four depths, as retail's `add`,
   `cmp`, `dp4 0.25`. The taps are 2^-k of the atlas and its size 2^k
   texels, so the sum rounds the same: same pixels. 135 slots for three
   calls -> 96 (then 28).
3. Fog per vertex, a retail mismatch fixed: every fogged 1.7 technique
   computes `saturate(exp2((d * fogConsts.z + fogConsts.w) * log2 e))` in
   the vertex shader (`exp o3.w_sat` in `lm_sm_sun_s_tc0n0_sm3.hlsl` and
   `lp_sm_sun_s_tc0n0_dtex_sm3.hlsl`; cod4-world.md 11.1) and the pixel
   lerps by the interpolated value. A CoD4 set's `vs_main` now writes it
   (`VsOut::fog`) and `cod4_lit`/`fs_cod4_model` lerp by it
   (`finish_fog`); CoD2 and water keep per-pixel fog. Fogged frames move
   1-3 levels on up to a few thousand pixels of 640 x 384. VERIFIED
   (bytecode; the frames).
4. Normal-mapped normal, a last-bits retail mismatch fixed: 1.7 takes `N +
   x T + y B` off the vertex's frame (`mov r1, v2; mad r1, r0.x, v4, r1;
   mad r1, r0.y, v3, r1; nrm`). vcod had sent CoD2's `N - T - B, 2T, 2B`;
   a CoD4 set's vertex now sends N, T and B as they are (and skips the
   half angle no CoD4 path reads), and the pixel does retail's two `mad`s.
   Frames move 1-2 levels on a few pixels. VERIFIED (bytecode; the
   frames).
5. Entities: `fs_main` no longer fetches CoD2's dynamic shadow for a CoD4
   probe stage, and `vs_main` no longer runs CoD2's probe grid per vertex
   for a CoD4 set. Same pixels.

Frames (items 1, 2, 5 byte for byte; 3 and 4 as stated) on the 38
deterministic `cod4_live` frames of the time. VERIFIED.

### What is left, ranked by slots

1. The entity's lighting block: done in 27.
2. The shadow taps: done in 28.
3. One pipeline for techniques 7 to 13: done in 29.
4. Precision: 1.7's lit shaders are `_pp` throughout, vcod's f32. Modern
   drivers mostly run D3D9 `_pp` at full precision. INFERRED. Open.

## 27. The model lighting block in one volume fetch (P11)

1.7 reads a model's block with one `texld` of `modelLightingSampler` (s4,
`dcl_volume`): `texld r1_pp, r1, s4` in `mc_l_sm_r0c0n0s0` technique 9's
pixel shader, at the block centre `v7` plus the normal over its longest
axis times `lightingLookupScale` (`mul r1.xyz, r1, c5; mad r1.xyz_pp, r1,
r0.y, v7`). VERIFIED (bytecode, `tools/re/d3d9dis.py`). The image is one
256 x height x 4 A8R8G8B8 volume (`CreateVolumeTexture` at 0x62f6dd,
cod4-world.md 3.2). VERIFIED.

vcod now does the same: entities and the view model write their blocks
into a 256 x 64 x 4 RGBA volume (`entity_material::Cod4Volume`, group 3
binding 6; the light set carries the block's centre, not its 64 texels)
and `cod4_block` is one `textureSampleLevel`; the static models' volume is
a 3D texture (group 2 binding 8) read by `model_volume` in one fetch. The
lit world pass then binds 17 images, so `renderer::device_limits` asks for
the adapter's `max_sampled_textures_per_shader_stage`. Slots: entity t9
408 -> 185, entity t11 440 -> 217, static model t9 190 -> 176 (current
values in 26's table). The hardware filter weights at fixed-point
precision, so lit models move up to 2 levels (4 on the `sweep_*` frames),
inside the HLSL gates
(`cod4_entity_set_lights_a_normal_mapped_sphere_as_the_hlsl_does`, 2
levels; `cod4_model_shades_its_volume_block_as_the_hlsl_does`, 3).
VERIFIED (lavapipe). The static model paths keep one `textureDimensions`
(the volume's height varies by map).

## 28. The sun shadow taps in a comparison sampler (P12)

Retail's taps: `wc_l_sm_r0c0n0s0` technique 9 (mp_crossfire) runs each
call as four `add`s of a tap to `s`, four `texldl` of
`shadowmapSamplerSun` (s4, `dcl_2d`), one `add` of `-s.z` to the four
reds, one `cmp` to 1 or 0 and a `dp4` with 0.25: 11 instructions plus four
`mov`s gathering the reds, 14 slots. The taps are `def c8` (1/2048,
-1/8192) and `def c7.z` (1/4096) both ways: (-0.5, 0.25), (0.25, 0.5),
(-0.25, -0.5), (0.5, -0.25) texels of the 1024 x 2048 map. The compare is
ALU work; the sampler only filters (point) and addresses (clamp).
VERIFIED (bytecode). Its point filter and clamp are a reading of the
technique's sampler state. INFERRED.

`cod4_sun_pcf` now takes four `textureSampleCompareLevel` through a
`sampler_comparison` (camera group binding 20,
`cod4_sunshadow::receive_sampler`: nearest, clamp to edge, `LessEqual`)
at `s.xy * (2/3, 1)` plus the taps in atlas uv (the partitions are the
left 1024 of the 1536 wide atlas), then a `dot` with 0.25. The taps ride
the coordinate because `offset` takes whole texels. The spot taps
(`cod4_spot_shadow`) are unchanged: their clamp to the light's 512 tile
keeps four lights' tiles apart in one column. Slots: one call 32 -> 10
(retail 14, whose compare and gathering cost slots here), the t9 paths 66
fewer each (26's current column).

Two roundings changed, no rule: a tap's texel was `floor((at + tap) *
1024)` in f32 and is now the sampler's `floor(uv * 1536)` at sub-texel
precision, so a lookup on a texel edge can land a texel over; and the
sampler clamps at the atlas's 1535, not the sun's 1023, so a tap past the
sun's right edge reads the spot column, reached only in the far
partition's edge fade where the map's weight goes to 0. Deterministic
frames moved by 1 pixel each; an outdoor wall's shadow edge (the first
`mp_tdm_spawn` at 20 up, turned 90) by 38 pixels, one column. VERIFIED
(the frames).

## 29. One lit pipeline per path, as 1.7 compiles a technique (P10)

`fs_main` and `fs_cod4_model` (`cod2_world.wgsl`) served techniques 7 to
13 off uniforms, where 1.7 compiles one shader per technique and
material combination. Now:

- `cod2_world.wgsl` reads the switches through `switches()`,
  `alpha_test()` and `light_kind()`: with the override `LIT_SPLIT` they
  return the pipeline constants `LIT_FLAGS`, `LIT_TEST`, `LIT_LIGHT`,
  without it `mat.flags.x`, `mat.flags.y` and `sun.src_col.w`. `vs_main`
  reads no constant, so the prepass keeps one vertex stage and its
  `@invariant` depths.
- A CoD4 world or model set (`LitEntry::Main` or `Cod4Model` with
  `PhongLit::cod4`) takes a pipeline keyed by `renderer::cod2::LitPath`:
  its material's `M_*` and `AT_*` and its group's light kind (none, sun,
  omni, spot). CoD2, static props and water keep the unsplit shader. The
  prepass's EQUAL pipelines split the same way.
- The load collects every key its binds ask for and compiles them in
  `Cod2Builder::finish_keeping` on eight threads off one queue
  (`par_build_on`, 34) before the first frame; each is kept in `LitShared`
  across loads. `r_vc_normalMaps 0`'s pipelines are made on the first
  switch off. A lit pipeline bound after another keeps the groups and the
  tangent buffer bound.
- `VCOD_LIT_SPLIT=0` at load keeps the one shader, for an A/B.

What a path runs is unchanged (`split_pipelines_run_their_path_alone`).
What a fragment stage compiles (every arm the walk cannot fold, the frame's
switches unknown), VERIFIED (counter):

| path | retail slots | runs (slots / fetch) | compiled unsplit: fetch / slots / branches | compiled split |
|---|---|---|---|---|
| `wc_l_sm_r0c0n0s0` t9 | 140 | 125 / 19 | 61 / 345 / 83 (`fs_main`) | 40 / 145 / 27 |
| `wc_l_sm_r0c0n0s0` t7 | 48 | 87 / 7 | same | 28 / 106 / 22 |
| `wc_l_sm_r0c0n0s0` t8 | 64 | 87 / 7 | same | 28 / 106 / 22 |
| `wc_l_sm_r0c0n0s0` t11 | 99 | 192 / 11 | same | 32 / 211 / 26 |
| `mc_l_sm_r0c0n0s0` t9 | 125 | 110 / 17 | 44 / 272 / 63 (`fs_cod4_model`) | 38 / 130 / 26 |
| `mc_l_sm_r0c0n0s0` t11 | 88 | 178 / 9 | same | 30 / 197 / 26 |
| `mc_l_sm_r0c0n0` t9 | 96 | 76 / 15 | same | 29 / 89 / 18 |
| `mc_l_t0c0n0s0` t8 | 61 | 73 / 5 | same | 26 / 92 / 22 |

The split fetch counts include both arms of `sample_mipped` per map. The
opaque sets' alpha test is a constant 0, so their split stages hold no
`discard`. Whether the driver allocates fewer registers is INFERRED: no
profiler here. `lit_pipeline_register_census` (ignored) builds the
unsplit stages and every row's split one behind a `LIT_REGS` line for a
driver that prints shader statistics.

Load (lavapipe, the `cod2 lit:` line): mp_crossfire 87 lit pipelines, 57
prepass, 205 ms compiling split (unsplit 8 and 5, 29 ms); mp_crash 60, 42,
134 ms (unsplit 27 ms). One at a time on the loading thread the split had
taken 1266 and 825 ms. Per frame: 618-621 pipeline binds (73-81 unsplit),
cut to 118-135 by 33's draw order. Frames: split against unsplit byte for
byte in one build. VERIFIED.

Pending, real card: the held view with and without `VCOD_LIT_SPLIT=0`,
reading `gpu.opaque` and the load time; with a GPU profiler, the `cod2 lit
pipeline` draws' register count and occupancy, split against unsplit.

## 30. The gamma pass folded into the post pass, the feather copy kept (P14)

### The gamma pass

- 1.7 has no overbright bits: iw3mp.exe's dvar names hold `r_gamma`
  (0x7046d8) and `r_ignorehwgamma` (0x7046f8) and no `r_overBrightBits`;
  the only "overbright" string is a spot light dvar's description.
  VERIFIED (strings). `gamma::overbright_bits` says 0 for CoD4 whatever
  the window, as for CoD2.
- At `r_gamma` 1 with no overbright bits the ramp is the identity
  (`GammaPass::identity`). A frame that runs the post pass (a CoD4 level's
  film, glow or depth of field, or `vc_hdr`) then draws it and the 2D
  straight into the swapchain, cleared to black first, and skips the gamma
  pass (`Renderer::render`'s `gamma_direct`). Conditions: the swapchain is
  `Rgba8Unorm` (the window asks for it where the surface lists it, else an
  sRGB format, where the gamma pass linearises), it is opaque (the 2D's
  blends leave alpha below 1, which the gamma pass overwrote), and no
  shellshock capture reads the gamma scene that frame. A CoD1 or CoD2
  frame without the post pass keeps the gamma pass (its glow, `blurWorld`,
  bloom and shafts work in the gamma scene); so does every game with
  `r_gamma` off 1 or CoD1's overbright doubling (cod11-gamma.md 4).
  VERIFIED (ours).
- `pipeline_check`'s offscreen target is `Rgba8Unorm` too; its readback
  swizzles to BGRA with alpha 255, as an opaque swapchain shows.

Frame writes (24's census): held view 78963440 -> 70669040 bytes, median
74769136 -> 66474736, the gamma pass's 8294400 and a full-screen read of
the gamma scene gone. VERIFIED (ours). Frames: the 46 deterministic ones
byte for byte, `saw_mounted` one pixel by one level (the old offscreen
target was `Bgra8UnormSrgb`, so every frame went through a linearise and
an sRGB encode; INFERRED). VERIFIED (frames).

Pending, real card: `r_gamma` 1 at the held view, `gpu.total` against
before; then the Brightness slider should still brighten the picture (the
gamma pass comes back off 1).

### The feather copy, not folded

At a feather the copy at the split held every depth-writing draw before
it; vcod's prepass then took only opaque lit world draws without alpha
test, polygon offset or water (`Cod2World::takes_prepass`) plus the
entities' opaque first stages. Per frame (a scratch count over
`cod4_worldfx_live` and `cod4_live_crossfire_frame_writes`) the prepass
missed 1-52 alpha-tested, 0-29 offset and 0-9 other opaque depth writers
against 29-620 it took. VERIFIED (ours). A `floatZ` from that prepass
would have faded sprites over foliage, fences and decals against the
depth behind them. Section 35 folds it at one sample with 1.7's own
prepass set.

## 31. The CPU frame: world bundles, the held cull, ragdolled corpses (P13)

Real-card CPU side then (7, 19): about 6-7 ms, `submit` 2.07, `encode`
0.96, `bodies` 1.21, `cull` 0.56, `hud` 0.46, `fxdraw` 0.45, `fxstep`
0.31, `gather` 0.30; retail 4.3-5.3 ms a frame.

`finish` apart from `submit`: wgpu 30 defers an encoder's work (a pass
only queues commands; `CommandEncoder::finish` validates, tracks every
resource and records into the driver, `wgpu-core`
`command::encode_commands`). The profile times `finish` between `encode`
and `submit`; on lavapipe `submit` is the rasterizer's wait, `finish` CPU
only.

What changed:

1. World runs replay from render bundles (`WorldBundles`, `renderer.rs`).
   The main pass's items between two non-world items or pass breaks are
   one run; each frame `emit_world_draw` records the run into a
   `Recording` (a handle clone each, no validation). A run whose commands
   equal last frame's at the same place replays its bundle, made on the
   second frame the commands held; one that changed replays its commands
   into the pass, so a view that changes every frame never pays for a
   bundle it uses once (trap: a bundle on every change lost 0.24 ms
   `encode` for 0.15 `finish` in the fight). A run binds its vertex, index
   and camera groups itself. CoD4 only; `VCOD_WORLD_BUNDLES=0` turns it
   off. A bundled run's static models stamp under `world` in the GPU
   groups.
2. The cull is kept while the eye, the view and the LOD ramp hold bit for
   bit (`Renderer::cull`'s inputs): a held view walks no cells.
3. A corpse whose ragdoll runs or sleeps no longer poses and skins the
   death anim under it (`Cod4Players::posed`) and keeps no tags.

Lavapipe (640 x 372, one sample, mp_crossfire): held view `finish` 0.54
-> 0.45 ms (405 of 405 world draws bundled, three runs), `cull` 0.22 ->
0.00; fight `bodies` 2.94 -> 2.15 (p50 2.58 -> 0.52), within the views'
spread elsewhere. VERIFIED (lavapipe, ours).

Not done then: sorting world draws by pipeline (done in 33); posing only
visible bodies (an unseen body still casts a sun shadow into the view and
fires its tracers and flashes from its tags; open); a HUD and `fxdraw`
cache (33 cut the HUD's dvar reads instead).

## 32. Effects culled and faded as 1.7's, and the feather's extra (P18)

cod4-effects.md 18 has the spawn and draw path with addresses. vcod had
culled an element's spawn by its `spawnRange` only; 1.7 also culls a flag
0x4 element's spawn against the view frustum and scales every element's
alpha by its `fadeInRange` / `fadeOutRange`. Both rules are in. At the
sedan spot that drops or fades (VERIFIED, `--example fx_ranges`):

- `fire/firelp_small_pm` element 2 (`gfx_smk_white_atlas`), the nearest
  fire's smoke 1182 units off: alpha 23/255 (fade in 1000 + 200). vcod had
  drawn it whole, the smoke column over the town retail's frame lacks.
- `dust/dust_wind_slow_yel_loop` element 1 (`gfx_smk_white_atlas`), 1600
  units off behind the view: frustum culled (radius 250).

Nothing else there is culled by 1.7 and drawn by vcod: no cell, portal or
visibility test stands between an effect and its pixels (cod4-effects.md
18).

Sweep (25) on lavapipe at the sedan and crossfire views, 1920 x 1080: the
fill does not move with the change within the runs' spread (fragment
counts swing 15-fold between passes of one build as ambient dust drifts
through). Here feathered sprites do draw (`fxsoft` up to 1.05 million
fragments); `r_zFeather` 0 saved 4-10 ms in each build, the split (2.1-3.3
ms), the depth copy (2.3-3.5 ms) and the feathered blend. VERIFIED (ours).

Per fragment 1.7's `zfeather.hlsl` and vcod's `fs_soft*` match (one
`floatZ` read, shaded once a pixel under MSAA, blended per sample). The
extra was per frame: 1.7 writes `floatZ` in its depth prepass, vcod split
the pass and copied (cut in 35). On a real card the split's store and load
are 33 MB a frame at 1080p and one sample, 133 MB at four. INFERRED (the
sizes). Copying the depth attachment as a texture may also leave a card's
depth buffer decompressed for the rest of the frame. INFERRED, not
measured.

Pending, by eye: whether the town still shows smoke by the sedan, against
retail's screenshot.

## 33. 1.7's draw order, the HUD's dvars and the marks' gather (P17)

### The draw order

A CoD4 batch draws at its first soup's place in 1.7's order (primary
light above material, the world list in the zone's surface order,
`materialSortedIndex` by `Material_Compare`; cod4-world.md 12;
`MapWorld::soup_order`, `cod4_places`). `VCOD_C4_DRAW_ORDER=0` at load
keeps CoD1's sorted shader list order. `cod4_live_crossfire_draw_census`,
joined draws, old order then new. VERIFIED (census):

| view | pipelines | bind groups |
|---|---|---|
| 5232 -888 240, yaw 225 | 618 -> 118 | 1415 -> 1529 |
| same, yaw 270 | 621 -> 135 | 1455 -> 1583 |
| 3990 -1110 60, yaw 180 | 130 -> 46 | 393 -> 415 |

At `cod4_perf.sh`'s held view (1280 x 772): 292 -> 69 pipelines, 878 ->
924 bind groups. Same draws; material groups rise about 8% as a
material's batches under two lights are now apart, as retail binds them.

Pixels moved only where two surfaces meet at one depth and the later draw
takes the pixel under LessEqual or EQUAL: mp_crash's arcade floor overlay
(501-756 pixels, up to 195 levels), mp_crossfire's bullet-hole decals on
the plaster wall through the left window (1745-2016 pixels, up to 55
levels), and 1-14 pixels along seams elsewhere. Where depths tie to the
bit retail's order decides those pixels, so the new frames take its side.
VERIFIED (the frames). Which is closer to retail's pixels needs a retail
capture of the same view, not made.

### The HUD

`Cod4Hud::server_dvar` was 0.23-0.28 ms of `hud.build`'s 0.39-0.43: the
menus ask 30-54 dvars a frame, each copying all 128 name slots into
Strings. It compares the configstrings in place (`configstring_bytes`):
0.06 ms, `hud.build` 0.19. No cache: configstrings change under `d` and
fades run off render time. VERIFIED (scratch timers).

### `fxdraw`

The fight's 0.34-0.45 ms: quads 0.08-0.13 (sort 0.02), upload 0.07-0.09,
clouds and lights under 0.01, marks 0.17-0.27, of which placing the
frame's impacts 0.04-0.20 for 0.6-0.7 impacts a frame.
`marks::mark_fragments` now skips a triangle wholly past one side of the
gather box grown by 1 (the clip epsilon is 0.5), and `chop` keeps its
distances on the stack: about 0.03 ms an impact. The marks code is shared
with CoD1 and CoD2; the fragments are the same. VERIFIED (scratch timers).

Lavapipe (1280 x 772, one sample): held `hud` 0.49 -> 0.30 ms; fight
`hud` 0.43 -> 0.23, `fxdraw` 0.45 -> 0.29. VERIFIED (lavapipe, ours).

Pending, real card: the held view with and without `VCOD_C4_DRAW_ORDER=0`
(`encode`, `finish`, `submit`, `gpu.opaque`, `pipelines`, `hud`); by eye,
mp_crash's arcade floor and mp_crossfire's plaster wall against retail.

## 34. Connect to the first frame in game, every game (P20)

### The tool

Every load logs one timeline at info, `load profile <what>: <ms> ms to the
first frame`, then a line a step (start ms off the load's start, length,
step): the window, the handshake, the gamestate, each map load's laps
(`vcod_common::laps::Laps`), the level prewarms, the first frame drawn in
game. A load starts at the process start under `--connect`, at a
`connect`, or at a map change, and ends at the first frame whose session
is live (`laps::begin_load`, `laps::end_load`).
`VCOD_LOAD_PROFILE=<path>` appends the same lines to a file. VERIFIED
(log).

### Before and after

`tools/cod4_perf.sh` (lavapipe, two shooting bots), process start with
`--connect`, three runs a side alternated, medians, ms. Lavapipe compiles
pipelines and uploads on the CPU, so those steps are larger than a real
card's. VERIFIED (log):

| | CoD1 mp_carentan | UO mp_cassino | CoD2 mp_toujane | CoD4 mp_crossfire |
|---|---|---|---|---|
| connect to first frame, before | 2410 | 3157 | 2234 | 7577 |
| after | 1731 | 2364 | 1956 | 6986 |
| level prewarm (CoD2: level models), before | 935 | 1092 | 372 | 1508 |
| after | 250 | 306 | 266 | 643 |
| CoD4 map load on the join thread, before / after | | | | 1073 / 808 |

What changed:

- `Renderer::upload_dynamic_models` uploads a level's models as one batch
  in the same order: skin pipeline keys read first
  (`EntityMaterials::skin_pipeline_keys`) and built at once
  (`EntityMaterials::prewarm`, `par_build`), images decoded on every core
  (CoD1's mipped too, `prepared_skins`). The CoD1/UO
  (`EntityScene::prewarm`), CoD2 (`Cod2Draw::preload` through
  `entities::resolve_models`) and CoD4 (`Cod4Client::prewarm`) prewarms
  take it; viewmodel preloads decode their skins the same way.
- The mark prewarm builds its stages' pipelines at once
  (`MarkPass::prewarm`): CoD1 marks 88 -> 31 ms.
- CoD4: the stock zones (`common_mp`, the sound zones) are walked during
  the handshake on their own threads (`cod4_fly::walk_stock`), or beside
  the map's zone on a map change; the HUD's assets come off the same walks
  (`cod4_fly::hud_assets`, gate `hud_assets_match_their_own_walk`). Each
  zone is walked once a load (before, the HUD walked the four HUD zones
  and the map's again, 530 ms).
- The CoD4 lit pipelines compile on eight threads off one queue
  (`par_build_on`).

Nothing is built twice (`by_skin`, `uploaded`, the model caches, the vm
cache), and the upload and material slot orders are unchanged.

Not done: the window and renderer (350-620 ms) and the CoD4 world's GPU
state (800-1000 ms) are mostly lavapipe's pipeline compiles, which a real
driver caches on disk. A second load of the same map re-walks its zones
(a kept `Walked` holds the whole zone and doubles the resident set). CoD2's
first frame (500-650 ms) is lavapipe's first draw of every pipeline.
CoD4's handshake (3.2 s) is the gamestate's fragments 350 ms apart at
`sv_maxRate` 5000 as 1.7 paces them (cod4-handshake.md 3), not a client
cost.

### What 1.7 does at load

- Zones load on a database thread: the setup at 0x50b480 calls
  `CreateThread` (import 0x6911b4) with entry 0x48ade0 stored at
  0x14e8a00; `"Failed to create database thread"` (0x6d39d4) is pushed at
  0x4ff2f6 on failure. VERIFIED (disassembly). That the main thread draws
  the loading screen meanwhile is INFERRED from the thread split.
- `"Loading fastfile %s"` (0x6d38a4) is printed at 0x48a0bf for each zone
  in turn. VERIFIED (disassembly).
- `r_preloadShaders` registers at 0x62d42e (dvar pointer 0xd569744; help
  text at 0x707618: "Force D3D to draw dummy geometry with all shaders
  during level load; may fix long pauses at level start."). 0x488bb2
  tests its value after a loaded asset (0x489e50 with type 5) and, when
  set, stores it in a 1024-entry ring at 0xd53e5e0. VERIFIED (disassembly
  and string). That the ring feeds the dummy draws is INFERRED. So 1.7
  leaves shader warm-up to a dvar; vcod's prewarm is that warm-up always
  on.

Pending, real card: the `load profile` line before and after on each
game's join; the first shot, frag and tracer should not hitch.

## 35. `floatZ` written in the depth prepass, no split and copy (P22)

### What 1.7's prepass draws

All iw3mp.exe, read off the disassembly.

- `R_DepthPrepassCallback` (0x658f40, handed to the draw call at
  0x6590fb) turns the scissor on (render state 0xae), then reads the
  view's `needsFloatZ` byte (`GfxViewInfo`+0x13a8, 0x658fa6). Set, it
  binds render target 5 (`mov eax,0x5`, 0x658fb4, into 0x632b60), draws
  the clear quad with the material at 0xcc9a2cc (0x636e90), writes (0, 0,
  0, 1) into the code constant at `+0xb60` (`depthFromClip`,
  0x658fe0..0x658ff9) and takes technique 1; clear, target 6 and
  technique 0. It then draws two surface lists through `R_DrawSurfs`
  (0x612260), `GfxViewInfo`+0x6304 and +0x632c, each a copy with the
  technique patched in (0x659021, 0x65904e). VERIFIED (the code). The
  names (`R_RENDERTARGET_FLOAT_Z`, `TECHNIQUE_BUILD_FLOAT_Z`, `litInfo`,
  `decalInfo`) are INFERRED.
- `R_SetupMaterial` (0x648f10) sends a technique of 1 or less to
  `R_SetPrepassMaterial` (0x648df0): a draw surface's prepass key (bits
  40..41, `shr ecx,0x8; and ecx,0x3` on the high dword) of 3 draws
  nothing; the material is `sortedMaterials[materialSortedIndex]`
  (0xcc98280), on key 0 with `stateFlags` 0x1 (`Material`+0x3d) the one at
  0xcc9a2e0 (`depthPrepassMaterial`); a null `techniques[1]` (`+0x40`,
  `+0xc + 4 * technique`) draws nothing. VERIFIED (the code).
- So the prepass draws the lit and decal lists, world, static models and
  entities alike: every surface whose material has prepass key 0, 1 or 2
  (0x6211d0, cod4-world.md 12.2) and a `build floatz` technique. Blended
  materials have key 3 (every blended world material of mp_crash and
  mp_crossfire, a scratch census of `MaterialOrder::prepass`). VERIFIED
  (the census).
- Key 1 is the alpha-tested ones: their technique 1 is
  `build_floatz_test`, whose pixel shader (`floatz_build_atest.hlsl`)
  `texkill`s on the colour map's alpha times the vertex alpha minus
  0.50196 and writes the vertex shader's `depthFromClip` dot as `oC0`; its
  state has no alpha test and no polygon offset, as every technique 1
  state on those maps. VERIFIED (`--example techset_shaders` on
  `wc_l_sm_t0c0n0s0` through `tools/re/d3d9dis.py`, and the states). A
  model material with a polygon offset in its lit state draws there
  unoffset when its key allows (`mc/mtl_brush_toujanebigbushy`, key 2)
  and not at all with key 3 (`mc/mtl_canopy_metal`). VERIFIED (the keys).
- The view model draws in the lit list behind its depth hack
  (`primarySortKey` lowered, `R_AddDObjSurfaces`), so 1.7's `floatZ`
  likely holds the gun too. INFERRED. vcod's prepass leaves the gun out;
  where the gun covers a pixel the sprites behind it fail the depth test
  either way.

### What vcod does now

A CoD4 frame at one sample with a feathered effect or world surface in
view and `r_zFeather` 1 runs the prepass as a `floatZ` prepass
(`Renderer::encode_depth_prepass`): the R32F target is its colour
attachment, cleared to the far plane, and each draw writes its view depth
(`fs_floatz` in `cod2_world.wgsl` and `dynamic_model.wgsl`, the same
`@invariant` depth linearised). It takes the world and entity draws whose
CoD4 material 1.7's prepass draws (`MaterialOrder::draws_floatz`,
`Material::floatz`), alpha-tested ones with the material's test; water
stays out (its own vertex stream). The scene pass neither splits for the
feather nor copies; the effects read the prepass's `floatZ` from the first
item on. Draws the prepass laid down untested draw their colour under
EQUAL, now only those in 1.7's set. Left on the copy: four samples (a
fragment's write is not the copy's sample 0), `vc_softParticles` (it fades
every sprite, not 1.7's), distortion alone, `r_zFeather` 0, CoD2, and
`VCOD_C4_FLOATZ=0` for an A/B.

### Every feathered pixel against the copy

`cod4_live_floatz_prepass_against_the_copy` (lavapipe, 960 x 540, one
sample) draws each view with the copy and with the prepass, comparing
frames and both `floatZ`s texel by texel, and `r_zFeather` 0 under both.
VERIFIED (ours):

| view | prepass draws copy / floatZ | feathered pixels | frame pixels moved | `floatZ` texels moved |
|---|---|---|---|---|
| sedan, `-279 1396 233 -15 28` | 5 / 4 | 0 | 0 | 0 |
| crossfire held, `5232 -888 180 225 0` | 46 / 30 | 0 | 0 | 0 |
| mp_crash, facing a feathered surface | 11 / 15 | 15595 | 0 | 818 farther |
| mp_crossfire, facing a feathered surface | 20 / 8 | 16918 | 0 | 818 farther |

- At the two held views the effects drew no feathered pixel on the
  captured frame (the smoke drifts), so the test also faces a feathered
  world surface or sprite from 160 units.
- `r_zFeather` 0 draws the same bytes on both paths at all four views.
- The 818 texels are one rectangle at the screen's middle, 161 units off
  in the copy and 178-185 in the prepass: a polygon-offset surface in
  front of the faced one is in the copy and not in `floatZ`. The only
  offset lit material both maps share is `mc/mtl_canopy_metal`, key 3,
  which 1.7 leaves out too. INFERRED (which material). No frame pixel
  moves.
- The joined prepass draws now follow 1.7's set: key 3 and sets without
  technique 1 (`wc_shadowcaster` among them) out, alpha-tested and key 2
  draws in.

Sweep (25) on lavapipe, 1920 x 1080, sedan and crossfire views, against
`VCOD_C4_FLOATZ=0`. VERIFIED (ours): with the feather on, `gpu.split`
(2.9-3.3 ms) and `gpu.softcopy` (3.1-3.6) go to 0 and `gpu.opaque` grows
2-4.4 ms as the old blend half stamps there; `gpu.total` falls 3-9 ms
(sedan 89.9 -> 84.3, crossfire 97.8 -> 88.7); crossfire's prepass falls 6
ms, 58 joined draws -> 40; fps up 0.5-1.0. `aa4` keeps the copy and holds
still. The copy's depth-as-texture read is gone with it.

Pending, real card: section 32's sedan sweep with `VCOD_C4_FLOATZ` 0 and
1 (`VIEW="-279 1396 233 -15 28" PASSES="base zfeather0 fxdraw0 aa4 base2"
VCOD_FRAG_STATS=1`): if `base` now comes near `zfeather0`, the split and
copy were the playtest's halving. By eye: soft smoke against fences,
foliage and wires by the sedan, against retail's.

## 36. The CPU frame with the view moving (P24)

### The tool

`TURN=<deg/s>` on `tools/cod4_perf.sh` sets `VCOD_PERF_TURN` on the
client, which yaws the view by that rate times the frame's time on every
live frame, as a held key would (`cod4_client.rs`, `perf_turn`).
`CULL_TURN=<degrees>` on `cull_census` (`vcod-cod4`, `CULL_CENSUS=<map>`)
turns level from every spawn point in steps of that and prints the walk's
mean time, how many steps kept the last step's cell set, soup set and
(since 38) batch set, and the triangles drawn against those of the
visible cells whole. `CULL_OLD=1` walks without 1.7's small-portal rule.

### Where the moving frame goes

A 90 deg/s turn on mp_crossfire's held view at 18 fps (lavapipe): `cull`
0.13-0.16 ms (the cell walk and the static LOD picks), `gather` 0.10-0.15
(ranges 0.02, index list with cookie boxes 0.05-0.08, order 0.01, upload
0.015-0.02; every turned frame changes the soup set), `build` 0.09-0.13
(entity instances, bones and light sets 0.075, joined entity sort 0.02).
In the walk itself the cells' cull groups took 82% on mp_overgrown: a
CoD4 static model placement is one cull group and a cell lists hundreds
to thousands. VERIFIED (scratch timers, `cull_census`, ours).

### What changed

1. A cell's cull groups are a box tree built at load
   (`WorldVis::cell_groups`, eight groups a leaf, split at the median
   along the longest axis); a box out of view skips its groups. The marks
   equal the flat list's (asserted over every `cull_census` view on
   mp_crash, mp_overgrown, mp_bloc and CoD2's `cell_walks_side_by_side`).
   Walk mean at 5 degree steps: mp_crossfire 0.086 -> 0.061 ms,
   mp_overgrown 0.364 -> 0.182. VERIFIED (`cull_census`, ours).
2. 1.7's `r_portalMinClipArea` and `r_portalMinRecurseDepth`
   (cod4-world.md 9.9): fidelity; the walk's time does not move.
3. The ragdoll steps as 1.7's (below).

Tried and dropped: kept chop buffers (within spread); skipping planes a
parent box lies wholly inside of (the groups were the cost, not the
nodes).

### A world bundle per cell set

`CULL_TURN=1`, every spawn point, level. VERIFIED (`cull_census`, ours):

| map | same cells | same soups | tris drawn | tris of the cells whole |
|---|---|---|---|---|
| mp_crossfire | 93.2% | 11.1% | 40859 | 236856 |
| mp_overgrown | 93.9% | 16.3% | 86968 | 376413 |
| mp_crash | 95.0% | 11.4% | 60571 | 291419 |

At 5 degrees the cell set holds on 75-78% of steps, at 7 on 69%. A bundle
keyed by the cell set would draw the cells whole, 4.3-5.8 times the
triangles 1.7 draws (its leaves and static models are frustum culled
too). Not done: it trades 0.7 ms of CPU for several times the vertex
work.

### The ragdoll's steps

- 1.7's ragdoll world steps in `Phys_RunToTime` (iw3mp.exe
  0x5a6dfb..0x5a6e6b): `maxIter` 2 (0x5a6dfb), each step `max(msecStep,
  (timeNow - timeLastUpdate) / maxIter)` (0x5a6e06..0x5a6e19), its time in
  seconds to `Phys_RunFrame` (0x5a6440), `timeLastUpdate` advanced by it,
  until it reaches `timeNow`. VERIFIED (disassembly).
- `g_phys_msecStep` is 0x71e798 (17, 17, 17), set each frame to `min +
  (max - min) * f` from the tables at 0x71e7a4 (17, 17, 17) and 0x71e7b0
  (67, 67, 34) (0x5a6791..0x5a67bb). VERIFIED (.data, disassembly). That
  `f` rises from 0 past 32 awake bodies, and that the ragdoll world is the
  third, is INFERRED (`Phys_RunFrame`, `PHYS_WORLD_RAGDOLL`).
- `ragdoll_fps` (int, default 20, 0 to 100, registered at 0x5ba3f8, dvar
  at 0xcc789ec, "Ragdoll update frames per second") is stored and never
  read: no other instruction names 0xcc789ec. VERIFIED (disassembly).
- So 1.7 steps 17 ms at most twice a frame: at 200 fps one step every
  third or fourth frame, at 10 fps two 50 ms steps; the bodies between
  steps are interpolated (`timeNowLerpFrac`). INFERRED (the loop).
- vcod had stepped 1000/60 ms up to twelve times a frame. It now steps as
  1.7 (17 ms, at most two a frame, the first `max(17, owed / 2)`, the
  second the rest when a step is still owed), carrying a point's velocity
  across a longer step and the damping per sixtieth of a second, a step
  capped at 100 ms (vcod's; a hitch loses time). At 200 fps the count is
  as before; at lavapipe's 10-18 fps a third. The corpses' spin at rest
  and the head's twist are the solver's, not the step rule's
  (cod4-player-anims.md 10.2).

Lavapipe (1280 x 772, one sample, mp_crossfire): turn within spread
(`cull` 0.15 -> 0.14, `encode` 0.45 -> 0.39); fight `bodies` 3.68 -> 1.97
ms, fps 9.8 -> 12.8 (each fight follows another bot). VERIFIED (lavapipe,
ours). Left on a moving view above 0.1 ms: `encode` and `finish` (no
bundle holds), `hud` (0.3), `gather`'s index copy and `build`'s entity
uploads.

Pending, real card: `TURN=180 GAMETYPE=war BOTS=0` at the held view,
reading `cull`, `gather`, `build`, `encode`, `finish`; `BOTS=12
GAMETYPE=dm` for `bodies`. By eye: a corpse falling at a high frame rate
should look as before; at a low one it takes fewer, longer steps.

## 37. The sedan's fill: its prepass set and its glass (P26)

Real card at section 35's sedan spot with the eye at standing height
(`VIEW="-279 1396 293 -15 28"`, 1920 x 1080, one sample), before this
section: `base` 187 fps, `gpu.ents` 3.73 ms, 10.99 M fragments;
`zfeather0` 314 fps, `ents` 1.93, 6.40 M; `fxdraw0` 329 fps, `ents` 1.88,
4.47 M; retail 220-260. The view is the sedan's side filling the screen.

Two fixes, VERIFIED (the sweep's `p` lines on lavapipe; the per-surface
counts through `cod4_live_sedan_entity_fill`):

1. Stub sets resolved late. With the feather on the sedan dropped out of
   the `floatZ` prepass: `cod4_fly::load_from` resolved the stub technique
   sets (`Map::resolve_material_orders`) before `entity_models` and
   `sound_and_fx` added the entity and effect materials
   (`Map::add_material`), and the sedan's are stubs in mp_crash's zone, so
   they kept key 3 and no `build floatz` technique. Out of the prepass the
   body drew under LessEqual and the world behind it shaded under EQUAL
   first: 1.7 million more world and 1.96 million more entity fragments
   (the real card's 4.6 million and 1.85 ms). The resolve runs again once
   every material is in; `world` and `ents` now read the same with the
   feather on and off. Lavapipe `base`: world 2173511 -> 467410
   fragments, `gpu.ents` 112.6 -> 66.5 ms.
2. Entity back-face cull. The sedan (`vehicle_80s_sedan1_red_destructible_mp`,
   32 drawn surfaces here) is 20 opaque surfaces (`mc/mtl_80s_econ_red`,
   `mtl_80s_econ_green_rubber`, `mtl_engine`, `mtl_80s_econdestroy_green`;
   `mc_l_sm_r0c0n0*`, prepass key 0, a `build_floatz_dtex` technique) and
   12 blended ones (`mc/mtl_80s_econ_glass_outside`, sort 39, and
   `_glass_inside`, sort 38; `mc_l_sm_b0c0s0`, `(One, InvSrcAlpha)`, alpha
   test `> 0`, back cull, no depth write, key 3). VERIFIED (the zone, a
   scratch dump through `MaterialOrder`). The glass is two coincident
   sheets a window, outside facing out and inside facing in, both back
   culled (`GFXS0_CULL_BACK`, handed to D3D as `D3DCULL_CCW`;
   cod4-world.md 8), so from outside 1.7 draws the near windows' outside
   sheets and, through them, the far ones' inside sheets. INFERRED (the
   states). vcod drew every entity two-sided (`ent_pipeline`'s
   `cull_mode: None`, kept for CoD1's guns and hands). A CoD4 material
   whose lit state culls back faces now culls on a world entity
   (`Material::entity_cull`, the entity pipelines' fourth key; the prepass
   and `floatZ` pipelines in a culled pair too); the view model stays
   two-sided; `VCOD_C4_ENTITY_CULL=0` keeps the old draw. Sedan fragments
   4985775 -> 3479892, glass 2.96 -> 1.43 million; lavapipe `ents`
   3879983 -> 2792511.

After both the sedan draws 2.05 million fragments of body (about a screen
under EQUAL) and 1.43 million of glass, the same surfaces and layers as
1.7. Its body's 1.7 technique 9, `lp_sm_sun_r0c0n0s0_sm3`, is 108 pixel
shader instructions with 17 texture reads, where vcod's `cod4_model` and
`cod4_sun_pcf` read 4 maps and 4 taps. VERIFIED (`--example
techset_shaders` on `mc_l_sm_r0c0n0s0`, `tools/re/d3d9dis.py`; vcod's
WGSL). Nothing drawn twice is left there.

Frames: ten differ in both on-off pairs and never between runs of one
mode: a parked sedan's windows (`scope_bare` 4473 pixels, `dof_off`/`_on`
394, `scope_live` 412, `tracer_off` 219: one glass sheet, the seats seen
through), a body's silhouette back face (`crash_lit_*`,
`mp_crash_prepass_*`, 5 pixels), `saw_mounted` one pixel; `sedan_fill` up
to 243 levels over the windows. VERIFIED (frames).

Pending, real card: the sedan sweep (`VIEW="-279 1396 293 -15 28"
PASSES="base zfeather0 fxdraw0 base2" VCOD_FRAG_STATS=1`) with
`VCOD_C4_ENTITY_CULL` 1 and 0: `base` and `zfeather0` should read the same
`ents` and fragments. By eye: the windows against retail's at the same
spot (one glass layer, the seats visible).

## 38. The real card's CPU frame: binds, shadow casters, ragdoll skins (P25)

The current real-card fight (bundle 10b339d0, `BOTS=12 SIZE=1920x1080
SESSION=1`, dm on mp_crossfire, following a bot): 280 fps, wall 3.57 ms,
`gpu.total` 3.07, the CPU stages summing to the wall: `finish` 0.67,
`encode` 0.44, `submit` 0.33, `bodies` 0.52 (p95 1.11), `hud` 0.23; 691
draws, 857 bind groups. Retail 190-230 fps.

The profile adds `calls` (draw calls in the prepass and main pass, a
bundle's counted; `draws` counts sorted items, a joined entity run once),
`shadowdraws` (the sun and spot passes' calls) and `huddraws` (the HUD's
texture runs).

Where `encode` and `finish` went (lavapipe fight, the encoder cut into
four command buffers, ms a frame). VERIFIED (scratch timers, ours):

| part | encode | finish |
|---|---|---|
| shadow maps, cookies, sun and spot shadows | 0.22-0.31 | 0.08-0.17 |
| depth prepass | 0.03-0.07 | 0.03-0.08 |
| main scene pass | 0.08-0.22 | 0.17-0.50 |
| post passes, HUD | 0.02 | 0.04 |

Inside the shadow part: the static LOD picks 0.06-0.07, the sun
partitions' static layer keys 0.07-0.09 (a cull test and hash over every
caster static model, each partition, each frame), the entity casters
0.01, the spot tiles 0.02-0.05.

What changed:

1. Entity draws in the main pass and prepass take their instance as
   `first_instance` over the whole instance buffer, bound once per run; a
   model's buffers stay bound across its instances; a stage rebinds its
   image and parameter groups only when they change (`EntBound`,
   `renderer.rs`).
2. The sun and spot passes' entity casters (`draw_entities`,
   `cod4_sunshadow.rs`) bind the instance and bone buffers once, set
   pipeline, image and buffers only on change, and join an instance's next
   surface on the same buffers and image when its indices follow on (a
   depth-only pass keeps the nearest depth in any order).
3. A sun partition's or spot tile's static layer keeps the static models
   it drew and their LOD picks; while its other inputs hold the layer
   holds while their picks do, a compare instead of the cull and the
   hash.
4. A ragdoll keeps its skins against a version its step and an explosion
   bump, the rig and the corpse's transform: stepping 17 ms at a time
   (36), at 280 fps it moves one frame in five and an asleep one never.
5. The grenade indicators' ceiling trace runs only when a frag passes the
   indicator's other tests (it ran every frame, 0.02-0.08 ms on
   lavapipe).

Lavapipe: bind groups per draw item 1.07 -> 0.98 (turn) and 1.13-1.34 ->
0.90-1.27 (fights); `encode` and `finish` 0.03-0.07 ms lower at the same
view. VERIFIED (lavapipe, ours). `cull_census`' batch set (material and
lightmap) holds on 40.3% of one-degree steps on mp_crossfire (68.7% at
0.3 degrees) against 11.1% and 20.0% for the soup set; a bundle keyed on
it would still rebuild its index ranges each frame. VERIFIED (ours).

Not done:

- Multi-draw indirect and push constants: the world's draws change
  material groups between draws (two bind groups each), so a multi-draw
  needs bindless materials across every lit pipeline; push constants
  would replace the dynamic-offset stage parameters, not the material
  groups that dominate.
- Posing only seen bodies (31).

Pending, real card: the fight (`COD4_DIR=<install> BOTS=12
SIZE=1920x1080 SESSION=1 tools/cod4_perf.sh <out> mp_crossfire 30`) base
against this, reading `encode`, `finish`, `submit`, `bodies`, `hud`,
`calls`, `shadowdraws`, `bindgroups`; then the `TURN=180` held view. By
eye: a ragdolled corpse should fall as before.

## 39. The live frames repeat run to run (TOOL-LIVEDET)

Every "byte for byte" claim rests on the frames two runs of one build
agree on. At d06a1335 two runs of `VCOD_C4_SHOTS` over `cargo test -p vcod
--bins cod4_live` (97 tests, 85 TGA frames and 8 `floatZ` dumps) differed
on 45 of 93 files, every one from a test that joins a live vcod-server.
Sections 24 to 38 judge a change only on the frames outside that set.
VERIFIED (frames, lavapipe, ours).

| Cause | Fix | Files held across two runs |
|---|---|---|
| The renderer, heli and SAW live tests (`renderer::cod4_{worldfx,distort,dof,scope,spot,spot2}_live`, `cod4_vehicles`, `cod4_client::saw_tests`) ran vcod-server in a loop on `Instant::now` beside a client on the wall clock, so which snapshot a capture saw rode lavapipe's frame time and the other test threads | `play::cod4::tests::run_held`: server and client on one `tests::sim` clock held for the client's whole run, only the client's 16 ms sleeps moving it; each frame loop sleeps first (a pass that `continue`d past its sleep ran again at the same time for ever); the SAW test's spectator steps inside the gunner's frames | 48 to 84 |
| `Cod4Client::now_ms` (the session's `step` clock) and the script menus' clock read `Instant`, wall time even under the sim clock; the heli test failed every run on the sim clock (no `explosions/clusterbomb`) | both read `play::cod4::clock_now`, the wall's outside a test, the sim's inside one | 84 to 90 |
| The bots' nav graph builds on a thread and lands when done, so the spot tests' bot set off at a wall-chosen tick | `run_held` sets `wait_for_graph`: the graph lands on the tick that asks | 90 to 93 |

After all three, two runs at d06a1335 held 93 of 93 files, and three runs
of the merged branch (`cargo test -p vcod --bins cod4`, 295 tests, 95
`cod4_live` files) held all 95, about 520-540 s of wall each at four test
threads. VERIFIED (frames).

`cod4_live_saw_gunner_view` failed about one run in three ("9 client
rounds, 27 server rounds"): on the wall clock a slow frame let snapshots
go by uncounted, and the client's step ran on its own wall `now_ms`. Held,
every 16 ms client frame reads every 50 ms server frame's snapshot.
VERIFIED (the test, ours).

What is left:

- `play::cod4_menus`' `tableLookup` random row and the join's qport read
  `SystemTime`; neither reaches a frame these tests write. INFERRED.
- Audio voices' end times run on `Instant` (`audio::voices`); the tests'
  mixers are silent. INFERRED.
- `cod4_join_menu_draws_offscreen`'s `join.tga` (not a `cod4_live` test)
  differs every run: its browser pings a wall-clock server thread and
  draws the ping. INFERRED.
- Rule: a new live test that writes frames goes through `run_held` with
  the `tests::{clk_now, clk_sleep, since}` clock and sleeps at the top of
  each frame loop.

## 40. Entity runs and the depth prepass from render bundles (P32)

1. `WorldBundles` (`renderer.rs`) keeps any pass's runs: the main pass's
   world runs, its entity runs (the non-gun entity items between two
   other items or stops), and the depth prepass's world draws and entity
   surfaces, each pass with its own targets (no colour target, or
   `floatZ`'s R32F). A run bundles on the second frame its commands held
   (31). `draw_ent_stages` and the prepass write through `DrawSink`, so a
   run records or encodes from the same code. A bundled run binds its
   camera, bones and instance buffer itself. CoD4 only;
   `VCOD_ENT_BUNDLES=0` turns it off (`VCOD_WORLD_BUNDLES=0` still the
   world's).
2. The profile counts `entcalls` (entity draw calls in the prepass and
   main pass, the gun's left out), `entbundled` (from a bundle) and
   `prebundled` (prepass world draws from a bundle).

How often the lists hold (lavapipe, mp_crossfire, the last two thirds of
each run). VERIFIED (lavapipe, ours):

| run | size | fps | entity calls | bundled | prepass world bundled |
|---|---|---|---|---|---|
| fight | 320 x 180 | 55 | 35.0 | 67.1% | 3.5 of 18.2 prepass draws |
| fight | 1920 x 1080 | 8 | 87.8 | 59.0% | 6.1 of 49.2 |
| held | 320 x 180 | 64 | 32.3-38.7 | 99.8% | 7.5-9.0 of 16.8-20.1 |
| turn | 320 x 180 | 80 | 9.0-10.4 | 35.8-36.0% | 0.1 of 7.1-8.1 |

A fight's list holds between most frames even at lavapipe's 18-125 ms
frames; at the card's 3.6 ms it holds longer. The prepass's world draws
are one run over the view's soup set, so they hold only while the view
does. Held view: `encode` and `finish` 0.01-0.02 ms lower each at the same
draws; turn within spread (`rebundled` 0.2); the fights follow different
bots (`finish` per call 0.99 -> 0.96 us at 320 x 180, 0.90 -> 1.19 at
1920 x 1080, where a bundle is made on most frames). VERIFIED (lavapipe,
ours). Frames: 95 of 96 files byte for byte on and off; `join.tga` is
39's measured ping.

Not done: a run split per body. An entity's instance index is its draw's
`first_instance` over every placed entity in LOD order, so a body that
enters or leaves renumbers every body after it; with two thirds of the
fight's calls already bundled, a split could add under a tenth of
`finish`'s entity share.

Pending, real card: the fight with and without `VCOD_ENT_BUNDLES=0`,
reading `encode`, `finish`, `submit`, `calls`, `entcalls`, `entbundled`,
`prebundled`, `rebundled`; then the held view. If the fight's `finish`
rises with it on, bundles are made more often than they replay, and the
fix is a longer hold before a bundle, not the per-body split.
