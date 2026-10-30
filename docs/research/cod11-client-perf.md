# CoD1 and UO client frame time

How the vcod client spends a frame on a CoD1 1.1 or United Offensive
server, measured with `tools/cod4_perf.sh` (`GAME=cod1` / `GAME=uo`), and
what was cut. Sections 1 to 3 measure vcod's own code on one machine; the
retail facts in sections 4 to 6 carry addresses and labels. Unless a row
says "real card", numbers come from a lavapipe (llvmpipe) adapter, so the
GPU columns are CPU rasterization: they rank the GPU work and are not a
real card's milliseconds. The profile's stages and GPU groups are in
`cod4-client-perf.md` section 1.

## 1. The tool on CoD1

`VCOD_FRAME_PROFILE` covers the CoD1 session's frame (`Mode::Online` in
`main.rs`), with CoD4's stage names and file format, so
`tools/cod4_perf.py` reads it unchanged:

- `net`: packets, snapshots, demo reads, the prediction.
- `pose`: `entities::build_instances` (snapshot entities interpolated and
  placed), less its player bodies, which are `bodies`; later in the frame
  the brass and the other effect models handed to the renderer.
- `events`: the snapshot's events, then the HUD frame's inputs, the
  event effects and sounds, tracers.
- `vm`: the view: recoil, kick, bob, shellshock, the gun.
- `fxstep` / `fx`: the effect step; the chained, wheel and looped effects.
- `sound`, `fxdraw`, `hud`: loop sounds and the sound step; quads,
  marks, lights and fog handed over; the HUD build.
- `scene` stays 0 on CoD1. `models` counts the dynamic model instances,
  `particles` the live effect particles.

`GAME=cod1 COD_DIR=<1.1 install> tools/cod4_perf.sh <out> mp_carentan 30`
(or `GAME=uo COD_DIR=<UO install> GAMETYPE=tdm ... mp_cassino`) runs
vcod-server with 12 shooting bots and the client joined as a spectator,
which on CoD1 and UO stands still at the map's spectator spot: the view
is fixed (the same triangles and soups every frame), so a run is an A/B of
the scene, not of where a followed bot looks. `SESSION=1` runs on the
desktop's own GPU, as on CoD4.

## 2. Before and after (640x452, 4x MSAA, 12 bots, 25 s, the first 5 s after the join dropped)

The box was shared with other jobs and lavapipe's GPU time moved up to 2x
between runs of one build (mp_carentan's `gpu.opaque` 108 ms in one run,
56 in the next), so each build ran alternately with the other on the same
map, two rounds for mp_carentan and mp_cassino. Stage ms are mean / p50.
"cpu p50" is the frame's wall time less `pace` and `submit` (on lavapipe
`submit` waits on the rasterizer).

| Map | Build | bodies | cull | pose | cpu p50 | GPU opaque p50 | draws | entdraws | models |
|---|---|---|---|---|---|---|---|---|---|
| mp_carentan | before | 0.54/0.48, 0.58/0.52 | 0.99/0.89, 1.17/0.92 | 0.19/0.17 | 2.34, 2.67 | 58, 63 | 741 | 354 | 143 |
| mp_carentan | after | 0.34/0.33, 0.52/0.40 | 1.03/0.89, 1.18/0.94 | 0.18/0.17 | 2.26, 2.68 | 57, 64 | 703 | 317 | 143 |
| mp_cassino (UO tdm) | before | 0.34/0.33, 0.48/0.39 | 0.11/0.10 | 0.33/0.19 | 1.46, 1.74 | 20, 22 | 622 | 259 | 118 |
| mp_cassino (UO tdm) | after | 0.28/0.24, 0.31/0.26 | 0.10/0.10 | 0.33/0.19 | 1.32, 1.41 | 20, 20 | 623 | 273 | 119 |
| mp_depot | before | 0.78/0.43 | 0.37/0.34 | 0.16/0.14 | 1.70 | 42 | 585 | 242 | 112 |
| mp_depot | after | 0.57/0.31 | 0.27/0.26 | 0.15/0.13 | 1.38 | 43 | 563 | 229 | 108 |

(Draw counts move with the bots in view; the spectator's triangles do
not: 64118 on mp_carentan, 26647 on mp_cassino, 58901 on mp_depot.)

1. `bodies`: a drawn body posed through `PoseBuffer::new` kept the 1.5/UO
   engine records and walked them on every blend, though only the 1.5 and
   UO servers' bone traces read them. The client poses through
   `playerpose::pose_player_drawn` and `PoseBuffer::drawn`
   (`cod4-client-perf.md` 7, item 1), turret rigs included, and composes a
   body's bone worlds once for its head, flash and brass tags and every
   model's skin, where each attached model (`skin_matrices`) and tag
   (`bone_world`) composed them again. `bodies` p50 falls by a quarter to a
   third.
2. `cull`: `WorldVis::visible`'s walk built a cell's occluder volumes on
   every visit, and the walk revisits cells (mp_carentan's spectator spot
   visits its 22 cells 137 times). The volumes depend on the cell and the
   eye alone, so each is built once a walk. mp_depot, whose cells
   carry occluders, cuts 0.34 -> 0.26 ms; mp_carentan has none and stays.
   Over a sweep of 25,000 views on mp_carentan, mp_harbor, mp_brecourt and
   mp_depot the visible soups, cells and walk stats hash the same before
   and after; the sweep's mp_depot cost fell 0.186 -> 0.123 ms a view.
3. CoD1 and UO entity surfaces draw through CoD4's `upload_vm_model` and
   `ent_run`, so a run of one instance's surfaces on one skin is one draw,
   and the per-pass GPU stamps run on this path too.

Pixel output: the frame digest (`frame_digest.rs`: the view, every
dynamic model's transform and bone matrices, HUD, sound, effects) of the
1.1 and UO bot demos under `timedemo` is identical before and after,
843 and 847 frames, and the renderer's code is unchanged, so the frames
are too.

## 3. What is left

- The frame on lavapipe is the rasterizer: `gpu.opaque` (the main pass
  up to the split) is over 90% of the wall time, the gamma blit 1.4 ms, every
  other group under 0.1. The CPU frame is 1.3-2.7 ms at p50, of which
  `cull` (mp_carentan's walk: 137 cell visits, 3741 tree nodes tested)
  and `bodies` are the largest. The walk's revisits are the retail walk's
  (each path narrows the frustum differently); cutting them changes what
  is drawn.
- `build` and `acquire` spikes (p99 4-24 ms on mp_cassino) land on the
  frames the GPU is behind on: `queue.write_buffer` and the acquire wait
  on lavapipe's queue, as `cod4-client-perf.md` 7 item 4 found.
- What a real card spends is not known. The profile to take is
  `SESSION=1 GAME=cod1 COD_DIR=<1.1 install> tools/cod4_perf.sh <out>
  mp_carentan 30` (and `GAME=uo ... GAMETYPE=tdm mp_cassino`) at the
  desktop's own size: if `gpu.opaque` leads, the question is fill
  (overdraw of the shader-sorted world; the 4x MSAA default is
  gone, section 4); if `submit` and
  `encode` lead, the draw count (~700, half of them entity surfaces).

The connect-to-first-frame load times, and the load profile that splits them, are in cod4-client-perf.md section 34 (all four games).

## 4. The GPU frame against retail's renderer (P16)

Retail CoD1 is a Q3-lineage OpenGL renderer: one draw per shader stage
over a shader-sorted list, a texture and lightmap pair collapsed into one
multitexture draw, no depth prepass, and the gamma ramp (with the
overbright shift) in the display hardware (`cod11-gamma.md`). Each of
vcod's CoD1 and UO passes against that. Runs are `tools/cod4_perf.sh` on
lavapipe at 1280 x 772, 12 shooting bots, 30 s with the first 10 dropped,
`r_gamma` 1, each install a scratch copy of the stock paks with its own
`vcod_mp.cfg` holding `r_aaSamples`.

### Multisampling

- CoDMP.exe asks for no multisampling. `R_Register` (0x4b3300) registers
  `r_ati_fsaa_samples` at 0x4b33e4 with the default "1" (0x5685e4) and
  flags 1, and stores the handle at 0x16c39a4 (0x4b3407); no instruction
  in the module reads 0x16c39a4. VERIFIED (the disassembly).
- The pixel format comes from GDI's `ChoosePixelFormat` (called in
  0x461390), whose descriptor has no sample field; the
  `wglChoosePixelFormatARB` string sits only in the GL entry point names
  and their error lines, and no decompiled function calls it. VERIFIED
  (the decompile and strings).
- vcod's default is retail's 1 in every game (`Shell::register_aa_samples`;
  it was 4 on CoD1). An unstamped vcod config's
  `seta r_aaSamples "4"` loads as 1; any other value it holds stays
  (cod11-console.md 8). VERIFIED (ours).
- The level shift: polygon edges alias as retail's do; a pixel inside a
  surface takes the same fragment either way. INFERRED.

### The depth prepass

- `r_vc_depthprepass` (cod4-client-perf.md 12) on CoD1 takes no world
  draws (only CoD2's and CoD4's lit draws qualify, `takes_prepass`), so it
  drew only the entities' first stages, 86 to 107 a frame on mp_carentan
  and 5 to 15 on mp_cassino, then drew them again under EQUAL. VERIFIED
  (ours, the profile's `prepass`).
- Retail has none. vcod's is off by default on CoD1 and UO
  (`Shell::register_depth_prepass`), on for CoD2 and CoD4. It is not
  archived (cod2-client-perf.md 12); a config's value is dropped at
  start. VERIFIED (ours).
- The bytes are the same either way:
  `cod1_identity_ramp_frame_matches_the_gamma_pass_and_the_prepass`
  (`pipeline_check.rs`) draws mp_carentan with a body, an effect, a mark
  and the HUD with the prepass on and off, at one sample and at four, and
  compares the frames byte for byte. VERIFIED (lavapipe).

### Draws, binds and stages

- World draws are already retail's shape: one draw per batch (shader x
  lightmap page) per stage, in the sorted list's order (`order_draws`),
  each batch's visible soups gathered into one index range. Two batches
  never share a bind group (each is its own texture and lightmap pair),
  so there is nothing to join. VERIFIED (the code).
- A two-stage lightmapped surface is one draw. On mp_carentan all but 6
  of the 194 materials have no script stages and draw texture times
  lightmap in one fragment (`Pass::Opaque`); of the 6 scripted ones (8
  stages) only `paper_fleur_wcoldpapr1_nodlight` is CollapseMultitexture's
  lightmap-then-filter row (0x4fb530, `shader.rs` `collapse`) and draws
  as two. mp_harbor: 4 scripted, none collapse; mp_depot: none scripted;
  mp_cassino: 7, none; mp_foy: 16, the same pair. VERIFIED (a scratch
  census over the maps' materials and the shader library).
- Collapsing that one pair would move the wallpaper by a rounding step
  for no measurable cost, so it stays two draws. INFERRED.
- Binds: mp_carentan sets about 1.5 bind groups a draw (856 for 596 at
  one sample, no prepass), the entity stages binding their image and
  parameter groups on every draw (`draw_ent_stages`); `encode` is 0.2 ms
  at p50. VERIFIED (lavapipe). The frame is the rasterizer's, so these
  are not cut. INFERRED.

### The gamma pass

- Full screen, retail's ramp doubles every byte after the 2D has blended
  in the framebuffer, so vcod's ramp must run after the HUD: the gamma
  pass stays there. INFERRED (cod11-gamma.md 4).
- Windowed at `r_gamma` 1 (vcod's default, `r_fullscreen` 0) the ramp is
  the identity, and a frame with no post pass, bloom, shafts, `vc_ssao`
  or `vc_shadows` resolves the scene and draws the 2D straight into
  the swapchain (`scene_direct` in `Renderer::render`), as section 30 of
  cod4-client-perf.md did for the post pass. The test above compares the
  frames with and without the gamma pass, byte for byte. VERIFIED
  (lavapipe).
- It needs the swapchain in `Rgba8Unorm`. Lavapipe's Wayland surface
  lists `Bgra8UnormSrgb, Bgra8Unorm, Rgb10a2Unorm, Rgba16Unorm,
  Rgba16Float` (the client logs the list and its pick at start), so
  these runs keep the pass: `gpu.gamma` reads 3.9 to 4.7 ms in every run
  here, a fifth of mp_cassino's GPU frame. VERIFIED (lavapipe). Whether a
  real card's surface lists `Rgba8Unorm` is the pending check below.

### Before and after

`base` is the build before these changes, `new` the build with them; aa is
`r_aaSamples`, pp the prepass; one row per run, repeated runs comma-separated, means / p50 in
ms. VERIFIED (lavapipe).

| Map | Build | aa | pp | wall | `gpu.opaque` | `gpu.gamma` | draws | prepass | bind groups |
|---|---|---|---|---|---|---|---|---|---|
| mp_carentan | base | 4 | on | 131.0 / 130.3 | 121.6 / 120.7 | 4.25 | 706 | 107 | 1262 |
| mp_carentan | base | 4 | off | 117.6 / 116.7 | 109.9 / 109.3 | 4.02 | 666 | 0 | 999 |
| mp_carentan | base | 1 | on | 88.5 / 82.7 | 77.5 / 73.2 | 4.42 | 627 | 86 | 1094 |
| mp_carentan | base | 1 | off | 82.4 / 82.6, 79.6 / 78.9 | 71.8 / 71.8, 71.2 / 70.7 | 4.4-4.7 | 654, 662 | 0 | 939, 956 |
| mp_carentan | new | 1 | off | 79.2 / 78.2, 77.2 / 76.0, 75.9 / 74.7 | 70.5 / 70.0, 69.5 / 68.5, 68.1 / 67.3 | 4.1-4.4 | 596-686 | 0 | 856-1032 |
| mp_cassino | base | 4 | on | 37.4 / 36.5 | 29.9 / 29.5 | 4.24 | 309 | 15 | 401 |
| mp_cassino | base | 4 | off | 35.8 / 34.0 | 28.5 / 26.8 | 4.30 | 290 | 0 | 342 |
| mp_cassino | base | 1 | on | 21.2 / 20.5 | 14.1 / 13.7 | 4.20 | 278 | 5 | 335 |
| mp_cassino | base | 1 | off | 21.0 / 20.6, 22.0 / 22.0 | 14.1 / 13.9, 13.6 / 13.3 | 4.2 | 302, 284 | 0 | 361, 329 |
| mp_cassino | new | 1 | off | 22.1 / 21.5, 20.5 / 20.1, 20.0 / 19.7 | 13.8 / 13.5, 13.8 / 13.4, 13.0 / 12.7 | 3.9-4.2 | 284-303 | 0 | 344-372 |

- One sample is most of the cut: `gpu.opaque` falls by over a third on
  mp_carentan and by half on mp_cassino, the wall time likewise (7.6 to
  13 fps, 27 to 50 fps). VERIFIED (the table).
- The prepass off takes 6 to 12 ms (8 to 10%) off mp_carentan's
  `gpu.opaque` and its p95 from 100 to 77 at one sample; mp_cassino, with
  a handful of entity draws in view, moves by under 1.5 ms. Draw and bind
  counts follow the bots in view. VERIFIED (the table).
- base and new at the same settings read the same within the runs'
  spread, as they should with the gamma fold off on this surface.
  VERIFIED (the table).

### Overdraw

`frags` stays empty here: lavapipe gives the profile no pipeline
statistics query. VERIFIED (lavapipe). Retail draws the same sorted list
with no prepass, and every CoD1 world fragment is one or two texture
reads, so a world prepass (the vertex work twice over 64807 triangles on
mp_carentan) is not where the time goes. INFERRED.

### Pending on a real card

On a real card, windowed, from a release build (`cargo build --release -p
vcod -p vcod-server`):

`SESSION=1 GAME=uo COD_DIR=<UO install> GAMETYPE=tdm tools/cod4_perf.sh /tmp/p16 mp_cassino 30 --exec-in-game "set r_gamma 1"`

`/tmp/p16/client.log`'s `swapchain` line says whether the surface took
`Rgba8Unorm` (then `gpu.gamma` reads near 0). An old config's `r_aaSamples
4` loads as 1 (cod11-console.md 8); no config sets `r_vc_depthprepass`.
Compare `gpu.opaque` and fps with section 3's run. Pending.

## 5. Fill per pixel against retail (P27)

On a real card at 1600 x 900 vcod's mp_carentan spectator frame
was GPU-bound (`gpu.opaque` 1.51 of 1.64 ms, 570 fps), where retail
1.1 under Wine ran about 700 fps at 2048 x 1536: about twice vcod's
cost per pixel. VERIFIED (real-card runs).

### Where the main pass's fragments go

`tools/cod4_perf.py` prints the `p` lines' means as a `frags` row
(`VCOD_FRAG_STATS`, or the profile on) and their sum over the window's
pixels; CoD1's static models count apart as `props`
(`frame_prof::Frag::Props`, `Renderer::is_prop_draw`). mp_carentan's
spectator spot, lavapipe, 1600 x 872, before this change: world
1525521, props 8074969, every other kind under 30000, 6.90 screens in
all. VERIFIED (ours). The world is 1.05 screens, as retail's sorted
list draws it; the props are 5.8 screens, nearly all of them the
foliage of one tree standing in front of the spot.

What each kind's fragment costs (shader doc; section 4): a world
batch is one `fs_main` draw, texture times lightmap, which is retail's
collapsed multitexture pass; 6 of mp_carentan's 194 materials take the
stage path. A prop is one texture read times the baked vertex colour,
retail's `rgbGen lightingDiffuse` single stage. So per fragment vcod
matches retail; the excess was the count. INFERRED.

### Static models were drawn two-sided

- `GL_Cull` (CoDMP.exe 0x4d5670) takes the shader's cull type in esi:
  2 is `glDisable(GL_CULL_FACE)` (0xb44), 1 `glCullFace(GL_BACK)`
  (0x405), 0 `glCullFace(GL_FRONT)` (0x404); the mirror flag at
  0x16d8c50 swaps the two faces. VERIFIED (the disassembly).
- The type is the shader's dword at +0xa8 (the parse buffer 0x11e6778,
  its cull 0x11e6820): every stage iterator loads `[shader+0xa8]` into
  esi before the call (0x500c26 in `RB_StageIteratorGeneric`, 0x5057cc,
  0x505a5c and 0x505cc9 in the static model surface's). VERIFIED (the
  disassembly). `cull`'s parse sets it to 2 on one triple of names and
  to 1 on another (decompile at the "missing cull parms" warning), the
  strings `none`, `twosided`, `disable` and `back` beside them. VERIFIED;
  which names make which triple is INFERRED from Q3's `ParseShader`.
- A shader starts zeroed (0x4fc6ba, `rep stos` over 0x66 dwords from
  0x11e6778), so a skin with no `cull` draws one-sided. VERIFIED (the
  disassembly). No 1.1 model shadertype says `cull` (all of
  `shadertypes/model/*.stype` in pak0-6); UO's `*_cullnone*`,
  `cloth_maskcullnone`, `foliage_blend2` and the propellers do.
  VERIFIED (the paks).
- vcod culls a prop whose skin's template is not `cull none`
  (`prop_skin_two_sided`), on CoD1 and UO only. Drawing every prop with
  culling off (the old `prop_pipeline`) draws inner foliage faces retail
  does not. An xmodel's faces wind opposite to the map's, so the cull is
  `Face::Front`. A scripted skin (`skins/` block) keeps the stage path
  and its two sides. VERIFIED (ours).
- Frames: 26 views (each map's intermission spot and spawns at four
  yaws, mp_carentan and mp_harbor, lavapipe, 320 x 240) are byte for
  byte the same as a build forced to `Face::Front`. Against the old
  two-sided build up to 1224 bytes a view differ: the inner faces of
  foliage and branches seen through the cut, which retail does not
  draw. `Face::Back` instead differed by up to 23296 pixels, whole
  props missing. VERIFIED (a scratch test over `Renderer::headless`).
  Pending: a hand check of a tree on mp_carentan against retail.

### The alpha test where nothing is cut

`fs_main` and `fs_prop` discard under alpha 0.5, which keeps a GPU
from writing depth before the shader runs. A CoD1 world or prop image
none of whose uploaded levels holds alpha under 128 (opaque world types
are forced to 255 at load) draws through `fs_main_solid` /
`fs_prop_solid`, the same colour with no discard (`Pass::Solid`,
`Pass::Prop { solid }`, `uploads_cutout`); a sample filtered from
texels all at 128 or more never falls under 0.5, so no pixel changes.
VERIFIED (the frame comparison above). On lavapipe it moves nothing:
an earlier scratch build with no discard in `fs_main` at all read
the same `gpu.opaque`. What it saves a real card is pending. INFERRED.

### Before and after

Lavapipe, 1600 x 872, one sample, 12 shooting bots, 30 s less the first
10, alternating builds, two rounds. mp_harbor's spectator spot is a
random pick among several (tris 39645 and 27474), so its rows compare
only within a round. VERIFIED (ours).

| Map | Build | fps | wall | `gpu.opaque` | world frags | props frags | screens |
|---|---|---|---|---|---|---|---|
| mp_carentan | base | 9.8, 9.9 | 102.5, 100.9 | 93.4, 92.0 | 1525521, 1525245 | 8074969, 8074704 | 6.90 |
| mp_carentan | new | 12.5, 12.9 | 79.7, 77.3 | 70.3, 68.0 | 1525188, 1519165 | 4400910, 4400910 | 4.28 |
| mp_harbor (round 1) | base | 22.8 | 43.9 | 35.6 | 1324889 | 34114 | 1.08 |
| mp_harbor (round 1) | new | 25.3 | 39.6 | 32.2 | 1324891 | 22234 | 0.98 |
| mp_harbor (round 2) | base | 23.8 | 42.0 | 33.5 | 1339911 | 35 | 1.02 |
| mp_harbor (round 2) | new | 23.0 | 43.4 | 34.6 | 1339911 | 17 | 1.09 |

- mp_carentan's props fall by 45% and the frame by a quarter. mp_harbor's
  spots see few props; its rows move within the runs' spread.
- What stays: 4.4 million prop fragments, the tree's foliage cards
  front-facing and stacked; retail draws those too. INFERRED.

### Left as they are

- The stage shader's branches: on mp_carentan 188 of 194 materials
  never reach `fs_stage` (section 4), so splitting it per stage kind
  (as P10/P15 did for CoD2 and CoD4) would not touch the fill. INFERRED.
- Texture formats: the `.dds` images upload as their own BC blocks and
  the rest as RGBA8 at `r_texturebits` 0, retail's default (0x4b3300);
  `cod11-front-end.md`'s defaults and `prepare`'s 16-bit path follow
  retail. INFERRED.
- The gamma pass: 0.12 ms on a real card, kept (section 4).
- Entity draws (306): CPU work on a GPU-bound frame; not this item.

### Pending on a real card

From a release build (`cargo build --release -p vcod -p vcod-server`):

`SESSION=1 GAME=cod1 COD_DIR=<1.1 install> tools/cod4_perf.sh /tmp/p27 mp_carentan 30`

and compare fps, `gpu.opaque` and the `frags` row with the 570 fps run. A card that keeps early depth without the discard shows it in
`gpu.opaque` beyond the prop cut. Pending.

## 6. The props' fill at mp_carentan's spectator spot (P29)

After section 5 a real card read 659.6 fps at 1600 x 900
(`gpu.opaque` 1.23 ms) with `frags` world 1612909, props 6270472,
dlight 37722. The tool's "5.51 screens" is every kind's sum over the
window (7.93 M over 1.44 Mpx), not the props alone, which are 4.35.
VERIFIED (a real-card run; `cod4_perf.py`).

### What draws them

A scratch pass (one fragment query per prop draw, the live spot: the
one `mp_deathmatch_intermission`, which `dm.gsc`'s `spawnSpectator`
picks, at its origin and angles, 1600 x 872) reads 4436099 prop
fragments, the live run's 4400910. VERIFIED (lavapipe). By skin:
`foliage_masked@spikeybushsparse` 1953986,
`foliage_masked@spikeybushfull` 1524745,
`foliage_masked@tree1_detail1` 660079, `oak1vert#0` 180976, every
other skin under 45000. The two spikeybush skins are
`xmodel/FullSpikeyShrub`, placed at `modelscale 2` 245, 269, 311, 346,
460 and 536 units from the eye. VERIFIED (the map's entity lump).

- LOD. `FullSpikeyShrub` lists 1500 / 2500 / 0. At these distances
  retail's pick (`lodDist = lodScale * d / 2`, xmodel-v14-format.md
  "LOD selection") is LOD 0 whatever `r_lodscale` up to 3; vcod draws
  LOD 0. VERIFIED (the descriptor; the rule).
- Cull. Retail adds a static model when its box passes the visited
  cells' frustums (0x4e4440 over a cell's list at +0x38, 0x4e42b0 the
  plane tests); vcod's `prop_visible` is the same test. Every bush
  stands in front of the eye. INFERRED.
- Most of these fragments are a foliage card's cut texels: the shader
  runs and discards them. Retail's `alphaFunc` costs the same pixels.
  INFERRED (`foliage_masked.stype`: one stage, `alphaFunc`, `depthWrite`).

### Draw order

Retail sorts a shader's surfaces by entity number (the world's is
0x3fe, its shifted 0x3fe00, stored at 0x16c5344 / 0x16c5348 by
0x50b427 and 0x50b431), and a static model's number is its place in the
`RE_AddRefEntityToScene` calls; the add loop at 0x50b4c4 walks the
cells in index order and each cell's list in turn. VERIFIED (the
disassembly). INFERRED: so retail's order within one skin follows the
cell list, not the distance. vcod's follows the entity lump.

Measured, not kept: gathering a skin's placements nearest first cuts
the prop fragments 3.7% on mp_carentan (4400910 to 4238041) and 4.6% on
mp_brecourt (2918881 to 2785908), and moved `gpu.opaque` by nothing
outside lavapipe's spread over two alternating rounds. It shifts up to
12 bytes a view on mp_brecourt (2 of 5 maps, coincident placements'
depth ties), an order no retail rule names. So it stays out. VERIFIED
(lavapipe, 30 s runs; a 209-view frame comparison).

### The view, not the fragment

The same spot at 4:3 (1024 x 768, `cg_fov` 80 unwidened, retail's
modes) reads world 954119 and props 1847127: 3.56 fragments a pixel,
against 4.29 at 1600 x 872 (`vc_widescreen`'s Hor+ fov 98, which takes
in more of the near bushes at the sides). VERIFIED (lavapipe). Retail
at 2048 x 1536 then shades about 11 M fragments a frame where vcod at
1600 x 900 shades 7.9 M (the card's count), so at retail's 700 fps and
vcod's 1.23 ms `gpu.opaque` the cost per fragment differs by about
1.2x, not 2x, if retail is GPU-bound at all (unknown). INFERRED.

### dlight and cull

- dlight: 10000 to 130000 fragments a frame (0.01 to 0.1 screens), the
  bots' muzzle flashes and explosions through retail's projected pass
  (0x500a10, section 4). Not a fill cost. VERIFIED (lavapipe).
- cull's p95 (0.54 to 0.58 ms on lavapipe, 0.61 on the card, p50 0.01):
  the cached walk (`cull_key`) misses whenever the predicted eye moves
  by one float step (384, -624, 192.00002 against 192.0), which a
  standing spectator's replay gives on about one frame in six; a miss
  runs the full walk, which retail runs every frame. VERIFIED (a
  logging build). Keying the cache on a rounded eye would reuse a walk
  from another eye; left as is.

### Pending on a real card

Retail at a 16:9 view would answer the per-pixel question; it offers
4:3 modes only. The comparable vcod run is 4:3:
`SESSION=1 SIZE=1024x768 GAME=cod1 COD_DIR=<1.1 install> tools/cod4_perf.sh /tmp/p29 mp_carentan 30`
against retail at 1024 x 768 (a 4:3 window needs no `vc_widescreen`).
Pending.
