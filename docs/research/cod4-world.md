# CoD4 1.7 map worlds: render world, cells, images, collision, entities

What `vcod_cod4::map` builds out of a walked map zone (the walk:
`docs/research/cod4-fastfile.md`), and how `vcod_cod4::image` and
`vcod_cod4::source` find an image's pixels.

Sources, each claim naming its own:

- The 21 stock MP map zones of the 1.7 client install
  (`zone/english/mp_*.ff`, `_load` zones aside) and its iwds
  (`main/iw_00..13.iwd`, `main/localized_english_iw00..06.iwd`), read by
  the gates `crates/cod4/tests/map_census.rs`, `iwi_census.rs` and
  `map_spots.rs` (`COD4_DIR`).
- `iw3mp.exe`, the 1.7 client (md5 `8c40cde102a9d8297a8674cb149f96ed`),
  for renderer-only facts. Addresses are virtual (image base 0x400000;
  `.rdata` at file offset 0x291000 maps to 0x691000).

VERIFIED: read out of a 1.7 zone, an iwd or a 1.7 binary. INFERRED: read
off control flow. An INFERRED claim that names no 1.7 address is not yet
confirmed on 1.7.

## 1. Render vertices and surfaces

- `GfxWorldVertex` is 0x2C: xyz (0), binormal sign (0xC), colour (0x10),
  texture coords (0x14), lightmap coords (0x1C), packed normal (0x24),
  packed tangent (0x28). INFERRED (`GfxWorldVertex`). The 0x2C stride is
  VERIFIED (the vertex array is `0x2C * vertexCount` bytes in every stock
  map zone).
- The colour is stored B, G, R, A. INFERRED (copied through
  `Byte4CopyBgraToVertexColor`). The data cannot tell the order: 161060 of
  mp_crash's 166418 vertices read 0xFFFFFFFF and the rest differ only in
  alpha. VERIFIED.
- A packed unit vector unpacks as `(b[i] - 127) * (b[3] + 192) / 32385`,
  the scale `(b[3] - -192.0) / 32385.0` stored as a float before the
  three products. VERIFIED (iw3mp.exe 0x624003..0x6240a4 on a world
  vertex's normal at +0x24; 127.0, 32385.0 and -192.0 are the doubles at
  0x70af78, 0x70af80, 0x70af88). The SSE code at 0x64eed9 does the same in
  another order: subtract (127, 127, 127, -192) at 0x6ADFE0, divide by
  (127, 127, 127, 255) at 0x6ADFF0, then the fourth lane scales the rest.
  VERIFIED. Every normal and tangent of mp_crash unpacks to length
  0.994..1.007. VERIFIED (`map_spots`).
- `GfxSurface` is 0x30: `srfTriangles_t` (`vertexLayerData` 0,
  `firstVertex` 4, `vertexCount` u16 8, `triCount` u16 0xA, `baseIndex`
  0xC), material 0x10, `lightmapIndex` 0x14, `reflectionProbeIndex` 0x15,
  `primaryLightIndex` 0x16, `flags` 0x17, bounds 0x18. INFERRED.
- Indices are relative to the surface's `firstVertex`: every index of
  every stock surface is below its `vertexCount`. VERIFIED (`map_census`;
  on mp_crash only 237 of 5558 surfaces would also fit absolute indices).
  That the draw sets the stream source at `44 * firstVertex` with base
  vertex 0 is INFERRED.
- A surface without a lightmap has `lightmapIndex` 31; every other value
  is below the map's lightmap count. VERIFIED (all 21 maps; 47 of
  mp_crash's surfaces). No mp_crash surface holds more than 128
  triangles. VERIFIED.

## 2. Cells, portals and the surface trees

- Layouts. INFERRED.

  | Struct | Size | Fields |
  |---|---|---|
  | `GfxCell` | 0x38 | bounds, `aabbTreeCount` 0x18, `aabbTree` 0x1C, `portalCount` 0x20, `portals` 0x24, `cullGroupCount` 0x28, `cullGroups` 0x2C, `reflectionProbeCount` u8 0x30, `reflectionProbes` 0x34 |
  | `GfxPortal` | 0x44 | writable part 0..0xC, `DpvsPlane` (four coefficients) 0xC, `cell` 0x20, `vertices` 0x24, `vertexCount` u8 0x28, `hullAxis[2]` 0x2C |
  | `GfxAabbTree` | 0x2C | bounds, `childCount` 0x18, `surfaceCount` 0x1A, `startSurfIndex` 0x1C, no-decal pair 0x1E and 0x20, `smodelIndexCount` 0x22, `smodelIndexes` 0x24, `childrenOffset` 0x28 |

- A portal's `cell` is an offset token into the map's own cell array, on
  an entry boundary; the walk turns it into a cell index. No stock portal
  carries a cell inline, and every portal has at least three vertices.
  VERIFIED (all 21 maps, 3214 portals).
- A tree's `smodelIndexes` is inline or an offset into an earlier tree's
  list; the walk copies the list either way. Every index is below the
  map's static model count. VERIFIED (all 21).
- A tree's surface range indexes `dpvs.sortedSurfIndex` (u16 at
  `GfxWorldDpvsStatic` + 0x48), not the surfaces: every range ends inside
  that array and every entry is a surface. Children start
  `childrenOffset` bytes after the tree, a multiple of 0x2C, inside the
  cell's tree array. VERIFIED (all 21).
- A cell's cull groups are below `cullGroupCount`. VERIFIED (all 21).
- A brush model (`GfxBrushModel` 0x38: `surfaceCount` u16 0x30,
  `startSurfIndex` u16 0x32) with no surfaces keeps 0xFFFF as its start;
  the others' ranges fall inside the surfaces. VERIFIED (all 21;
  mp_crash's model 1 is the first such).
- mp_killhouse and mp_shipment have one cell and no portal, mp_countdown
  two cells and no portal. VERIFIED.

## 3. Static models

`GfxStaticModelDrawInst` 0x4C: `cullDist` 0, placement (origin 4, axis
rows 0x10..0x34, scale 0x34), xmodel 0x38, `smodelCacheIndex[4]` 0x3C,
`reflectionProbeIndex` 0x44, `primaryLightIndex` 0x45, `lightingHandle`
0x46, `flags` 0x48. INFERRED. Every stock static model names an xmodel.
VERIFIED.

`GfxStaticModelInst` 0x1C (`dpvs.smodelInsts`): bounds (0, 0xC) and
`groundLighting` (0x18, four bytes). INFERRED. 1804 of mp_crash's 4038
static models carry a non-zero `groundLighting`, 14962 of mp_creek's 22311
(the grass). VERIFIED (`lightgrid_census`).

### 3.1 The light grid

`GfxLightGrid` at `GfxWorld` + 272. INFERRED; the array sizes VERIFIED
(the walk reads every stock map zone to its end).

| Offset | Field |
|---|---|
| 272 | `hasLightRegions` |
| 276 | `sunPrimaryLightIndex` |
| 280, 286 | `mins`, `maxs` u16[3] |
| 292, 296 | `rowAxis`, `colAxis` |
| 300 | `rowDataStart` u16 per row |
| 304, 308 | `rawRowDataSize`, `rawRowData` |
| 312, 316 | `entryCount`, `entries` (`GfxLightGridEntry` 4 bytes: `colorsIndex` u16, `primaryLightIndex` u8, `needsTrace` u8) |
| 320, 324 | `colorCount`, `colors` (`GfxLightGridColors` 0xA8, 56 RGB triples) |

- A row (`rawRowData[4 * rowDataStart[r]]`) is a 12-byte head (`colStart`,
  `colCount`, `zStart`, `zCount` u16, `firstEntry` u32) and runs of
  (columns, z count, base z). The base z is one byte, two when `zCount`
  exceeds 255; a run of z count 0 is two bytes. On all 21 maps the runs'
  columns add up to `colCount`, each row's entries start where the last
  row's ended, and the rows cover every entry. VERIFIED (`lightgrid_census`,
  `LightGrid::check_rows`).
- 0x6279b0 reads the head at `rawRowData` (grid + 0x24) plus 4 times
  `rowDataStart` (grid + 0x1c), the row and column axes at + 0x14 and
  + 0x18, `mins`/`maxs` at + 8 and + 0xe, the entries (4 bytes) at + 0x2c.
  It steps a z-count-0 run by 2 bytes, any other by 3, or 4 when `zCount`
  is above 0xff. VERIFIED (0x6279b0-0x627b47). `R_CalcModelLighting`
  (0x628990) reads `hasLightRegions` at + 0, the sun's index at + 4 and
  `colorCount` at + 0x30. VERIFIED.
- All 21 maps have `hasLightRegions` set and the sun as primary light 1.
  VERIFIED (census).
- 0x6279b0 finds, for a grid point, the entries at `z` and `z + 1` in its
  column and in the next one along the column axis (all compares
  unsigned). VERIFIED (0x6279b0-0x627d34):
  - None when the row is outside `mins..=maxs`, its `rowDataStart` is
    0xffff, or the column (less `colStart`) plus 1 is past `colCount`.
    None when `z` (less `zStart`) plus 1 is past `zCount`; under the floor
    when below `zStart`.
  - Column -1 (before the row's first) borders the first run: only the
    next column's pair, under the floor below the first run's base.
  - Otherwise it walks the runs to the column's, adding each passed run's
    columns times its z count to `firstEntry`. In a run of z count `n` and
    base `b` the entry for height `z` is `first + col n + z - b`, present
    while `z - b < n`; under the floor below `b`.
  - A column in an empty run has none, and is under the floor below the
    top (base plus z count) of the next run when that run is not empty.
  - The next column is the run's next one, or the first of the next run
    unless the column is the row's last.
- `R_LightGridLookup` (0x627e90): the cell is `(floor(p) + 0x20000) >> 5`
  along x and y, `>> 6` up (0x401020 the floor, 0x68b360 the truncation,
  `sar`); the fraction is `(p - (-131072)) / 32` (`/ 64` up) less the cell
  (-131072 at 0x70b860, 1/32 at 0x70ae10, 1/64 at 0x70b100), on the row
  axis, column axis and up. Corner `c`'s weight is the product of the
  three fractions or their complements (bit 4 row axis, 2 column axis, 1
  up). Corners 0-3 come from the cell's grid point, 4-7 from the next
  along the row axis (0x6279b0 twice, 0x628025 and 0x628044); the point
  is under the floor when either call found it so. VERIFIED
  (0x627e90-0x628051). That vcod's `f32` arithmetic matches assumes the
  x87 runs at single precision, as Direct3D 9 sets it by default.
  INFERRED.
- The corners in order: one with no entry is skipped, one under 0.001
  (0x70b278) dropped. When its `needsTrace` has the corner's bit, 0x627d50
  traces from `p` to the corner moved 0.01 (0x70afb8) toward `p`, through
  `CM_BoxSightTrace` (0x4f66d0), zero box, mask 0x2001. The corner is the
  cell times 32, 32, 64 less 131072, plus 32, 32 or 64 on its bits' axes
  (0x70b120, 0x70b118, 0x70b700); the direction is 0x4076d0's times the
  reciprocal of its length. The first corner that sees `p` drops the
  corners before it (0x67c4a0 zeroes their 4 c bytes) and takes the vote;
  after it a corner that does not see `p` is dropped. Every other kept
  corner votes: any light beats a held 0 or a held 255, a 0 or a 255 never
  wins, else the strictly heavier weight wins. VERIFIED
  (0x627d50-0x627e8d, 0x62806e-0x628185).
- `R_CalcModelLighting` (0x628990): a vote of 255 is the sun; with light
  regions, a vote for any non-sun light is the model's own. Over the kept
  corners it sums the weights, the weights on that light, and those on no
  light or (under some light) on 255; the last only when 0x6288a0 passes
  for the light (`primaryLights` + 0x44 a light) and the point. Colour
  sets add their weights in first-seen order. The share is on-light over
  on-light plus off-light: 1 when nothing is off but something is on, 0
  under no light. One set is handed on as is; more go to 0x6278b0 scaled
  by 1 over the total. VERIFIED (0x6289de-0x628c17).
- 0x627790 turns weights into 256ths, `(w * scale * 256 + 0.5)` truncated
  (256 at 0x70aed8, 0.5 at 0x70ace8, `fistp` under 0xc00), the first
  largest taking what is left of 256. 0x627810 multiplies and sums each
  set's bytes in 16 bits (0x627600, 0x627680) and stores `(sum + 127) >>
  8` (0x627700). The primary weight is `(share * 255 + 0.5)` truncated
  (255 at 0x70aee0, 0x6275a0, 0x6278b0). VERIFIED. The blend runs when
  the patch is drawn; 0x627930 copies a one-set patch's set and blends the
  others first. VERIFIED (0x627930-0x6279a2).
- No corner kept: set 1 (set 0 under the floor) at weight 1 under the
  sun; the last set under no light when there are not that many sets.
  Under mode 1 (entities, 0x62ee1b) with `r_showMissingLightGrid`
  (`[0xd569734]`, stored at 0x62abba) on, a point under the floor takes
  the last set under no light. VERIFIED (0x628aa5-0x628b6d). Static models
  pass mode 0 (0x62f06a). VERIFIED. No stock static model's lighting
  origin falls back to the missing colour. VERIFIED (census).

vcod: `vcod_cod4::lightgrid` (`LightGrid::lighting_at`), mode 0, dvar off.
0x6288a0 is not ported: a corner on no light or 255 always counts against
the light. `render_pins`' `the_light_grid_lookup_holds_its_bits` pins the
outputs at every static model, spawn and a lattice on mp_crash and
mp_backlot.

### 3.2 The model lighting volume

The `mc_l_*` sets' lit techniques are `lp_*` (light probe). VERIFIED
(`code_post_gfx_mp`'s and mp_backlot's zones, `--example techset_shaders`,
`tools/re/d3d9dis.py`):

| Set, technique | Shaders |
|---|---|
| `mc_l_sm_r0c0` 8 | `lp_sun_tc0_dtex_sm3.hlsl`, `lp_sun_r0c0_sm3.hlsl` |
| 15 (static models, instanced) | `lp_i_sun_*` |
| `mc_ambient_t0c0` | `lp_amb_sun_*` |

- `modelLightingSampler` (s4) is a volume (`dcl_volume`). The pixel shader
  reads it at `baseLightingCoords + n * lightingLookupScale / max(|n.x|,
  |n.y|, |n.z|)`, `n` the normalized normal (normal mapped on `n0`), and
  lights the diffuse by `2 * texel.rgb + texel.a * saturate(dot(sunPosition,
  n)) * sunDiffuse`. The specular is the world's (9.6) with `texel.a` as
  visibility. `lp_amb_*` reads the texel at the base coords and adds
  `texel.a * sunDiffuse` without `N.L`. Colour, fog and detail map as the
  world's. VERIFIED (bytecode of `lp_sun_r0c0_sm3`,
  `lp_i_sun_r0c0d0n0s0_sm3`, `lp_i_amb_sun_t0c0_sm3`).
- The instanced vertex shader takes the base coords from a vertex stream
  times 1/256, the plain one from the `baseLightingCoords` constant (c8).
  VERIFIED (bytecode).
- `lightingLookupScale` is (1.5 / 256, 1.5 * `invImageHeight`, 0.375, 0):
  1.7 stores 0.005859375 (0x70b428), the image height's inverse
  (0xd573a00) times the double 1.5 (0x70ada0), 0.375 (0x70b424) and 0.
  VERIFIED (0x5f9ca3-0x5f9cc7).
- The volume is 256 texels wide and four deep. Entry `i`'s 4 x 4 x 4 block
  sits at x `4 (i & 63)`, y `4 (i >> 6)`; `baseLightingCoords` is its
  centre, `((4 (i & 63) + 2) / 256, (4 (i >> 6) + 2) / height, 0.5)`, so
  the lookup reaches the centres of the block's outer texels. INFERRED
  (`R_SetModelLightingCoords`, `R_InitModelLightingImage`;
  `R_SetModelLightingLookupScale`'s scale matches the 1.7 store above).
- 1.7 makes the image at 0x62f6dd: `IDirect3DDevice9::CreateVolumeTexture`
  (vtable + 0x60), width 0x100, height the image height (0xd573a10),
  depth 4, one level, usage 0, format 0x15 (`D3DFMT_A8R8G8B8`), default
  pool; a second in the system pool follows at 0x62f797. VERIFIED. That
  the second is `r_altModelLightingUpdate`'s staging copy is INFERRED.
- The image height is set at 0x62f3f7, its inverse stored at 0x62f42a:
  the entry count `maxClientViews << 10` (0xcc9d110 shifted at 0x62f379)
  rounded up past 0x800 static model entries to a power of two, four
  texel rows to 64 entries. VERIFIED (the stores). The function is
  `R_InitModelLightingGlobals` (0x62f370). INFERRED.
- `R_SetModelLightingCoords` (0x62eb30) takes handle `h`, entry `h - 1`,
  and writes x `(4 ((h - 1) & 63) + 2.0) / 256` (2.0 at 0x70ae50, 1/256 at
  0x70b1f0), y `(((h - 1) >> 4) & ~3) + 2.0` times the inverse height, z
  0.5 (0x6be648). VERIFIED. Static models take entries from 0 and scene
  models theirs past the static ones (`xmodel.baseIndex`), in one image.
  INFERRED.
- 0x626e10 writes a block's 64 texels as dwords: alpha the patch's
  primary weight, then the colour's red, green, blue; four texels a row,
  rows stepped by `[0xd552400]`, slices by `[0xd5523fc]` (0x626fea,
  0x6271b5, 0x62736b). Replayed with those as the row pitch and the slice
  pitch less three rows, texel `x + 4 y + 16 z` holds the colour
  `lightgrid::BLOCK_COLORS` names: the 56 on the faces, the eight inner
  texels repeating face texels. VERIFIED (the stores, 0x626e10-0x6275a0);
  the two pitches INFERRED.
- A ground-lit static model fills all 64 texels with its
  `groundLighting`. A static model is looked up at its
  `GfxStaticModelInst` bounds' centre. INFERRED
  (`R_SetModelGroundLighting`, `R_SetStaticModelLighting`).

vcod: `vcod_cod4::mapworld::model_lighting` fills block `i` for static
model `i` (retail hands blocks out as models come into view), tracing
through the level's `ClipMap`; each static model vertex's lightmap coords
are its block's centre. The renderer uploads a 256 x height x 4 RGBA
volume (`renderer::cod2`'s `upload_volume`, group 2 binding 8), and
`cod2_world.wgsl`'s `fs_cod4_model` reads it in one linearly filtered
fetch, as `lp_*`'s `texld` of s4 does. mp_creek's 22311 models stand 1396
texels, inside the 3D limit of 2048. The lit world pass binds 17 images,
one past WebGPU's default 16 a stage, so the device asks for the adapter's
`max_sampled_textures_per_shader_stage` (`renderer::device_limits`).
Entities have their own volume (3.3). Gate:
`cod4_model_shades_its_volume_block_as_the_hlsl_does` (lavapipe, against
the HLSL in bytes).

### 3.3 Entities and the view model

- The weapons' and hands' `mc_l_sm_r0c0n0s0` in `common_mp` draws its lit
  view through technique 8 (`lp_sun_s_tc0n0_dtex_sm3.hlsl`,
  `lp_sun_r0c0n0s0_sm3.hlsl`, the static models' plain pixel shader,
  3.2). Technique 9 is the sun shadow map's `lp_sm_sun_*`, 10 to 13 the
  spot and omni lights' `lp_*spot*` and `lp_*omni*`. VERIFIED (`--example
  techset_shaders common_mp mc_l_sm_r0c0n0s0`).
- Every scene model takes a volume block each frame. `R_AllocModelLighting`
  keeps a model's block while its lighting origin is unchanged; else it
  picks a free one, the model's reflection probe
  (`R_CalcReflectionProbeIndex`: the nearest of its cell's probes) and its
  non-sun primary light (`R_GetNonSunPrimaryLightForBox`), and fills it
  through `R_CalcModelLighting` (0x628990) in mode 1. INFERRED
  (`R_AllocModelLighting_Box`, `R_AllocModelLighting`).
- Mode 1 differs from mode 0 only under `r_showMissingLightGrid` (no
  corner found, or under the floor, gives the missing colour). VERIFIED
  (3.1). 1.7 registers `r_showMissingLightGrid` (name at 0x7054a0) off and
  cheat protected: value byte 0 at 0x62ab81, flags 0x80 at 0x62aba2.
  VERIFIED. So an entity's lookup reads as a static model's.
- Lighting origins 1.7's cgame hands `R_AddDObjToScene` (0x5f7b60), the
  drawn origin raised by a double:

  | Entity | Raise | Where | Label |
  |---|---|---|---|
  | player | 12 with `eFlags` 8 (prone), 20 with 4 (crouched), else 32; render flags 4 | 0x445441-0x445468 (doubles 0x70b238, 0x70b378, 0x70b120) | VERIFIED |
  | corpse | the same three; render flags 0 | 0x4455fa-0x44560f | VERIFIED |
  | `CG_General` (0x43287b), `CG_ScriptMover` (0x43376b) | 4, through `CG_LockLightingOrigin` | see below | VERIFIED (the calls); which function is which INFERRED |
  | item | 4 | 0x432950 | VERIFIED (constant); kind INFERRED |
  | missile | 4 | 0x432f77, double 0x70b340 | VERIFIED (constant); kind INFERRED |
  | turret | 32, render flags 4, after the `viewlocked_entNum` test at 0x432e3d | 0x432e63 | VERIFIED (constant); kind INFERRED |
  | vehicle | 32 | 0x44f857 | VERIFIED (constant); kind INFERRED |

  `CG_LockLightingOrigin` (0x432780) with `eFlags` 0x400 copies the
  entity's latched origin out, then falls through to clear the latch and
  hand out the origin plus 4 in every case (0x4327ea-0x432810). VERIFIED.
- The view weapon: the playerstate's origin, z raised by the float at
  playerstate + 0x118, then `AddLeanToPosition` (0x5728c0) with the yaw at
  + 0x10c, the lean at + 0x5c, 16 (0x6be660) and 20 (0x6df498); entity
  0x3ff, render flags 3 (0x456dd7-0x456e3c). VERIFIED. That + 0x118 is
  `viewHeightCurrent` and + 0x5c `leanf` is INFERRED (`CG_AddViewWeapon`).

vcod: `MapWorld::entity_lighting` (`vcod_cod4::mapworld::entity_lighter`)
looks a block up per distinct lighting origin each frame, traced through
the clip map, into the frame's entity volume (`entity_material::Cod4Volume`,
256 x 64 x 4: 1024 blocks, 1.7's entry limit for one view; set `i` takes
block `i`, the view model the last). `entity_light::pack_cod4` packs the
sun and the block's centre into the instance's light set;
`cod2_model.wgsl`'s `cod4_model` (the entity and view model passes' probe
path, `entity_material::cod4`) reads the block in one filtered fetch, as
`fs_cod4_model` does. vcod keeps two volumes where 1.7 has one image: the
world loader builds the static one, the entity pass writes its own each
frame. The view weapon is lit at the eye before its floor
(`Scene::light_eye`), a body at `BodyFrame::light_origin`, the rest at
`cod4_client::light_origin`. The set carries the model's reflection probe
(9.7), whose cube the specular adds; a spot or omni light the lookup
settles on lights it in the sun's place (3.5). Entities receive the sun
and spot shadow maps (3.5, 3.6). The install's iwd search path reads as
CoD2's (`Pk3Fs::game`), so `entity_material::load` tells a CoD4 set by its
compiled `Cod4Lit`; trap: without that every CoD4 model material takes
CoD2's path and draws unlit. Gates:
`cod4_entity_set_lights_a_normal_mapped_sphere_as_the_hlsl_does`
(lavapipe, against the HLSL), the census (9.5, through
`entity_material::load` on the iwd path), `cod4_live`'s gun and body lit
sunny against shaded (lavapipe, the session's loaders), and
`lightgrid_census`: a player on any stock spawn point is never lit by the
missing colour. VERIFIED (census).

### 3.4 Flags: `mc_l_*_flag_*`

- The flag sets (`mc_l_sm_flag_t0c0`, `_t0c0s0`, `_t0c0n0`, `_t0c0n0s0`
  and their `hsm` twins) live in `common_mp`; a map zone carries name
  stubs (mp_bloc, mp_citystreets). Lit techniques are `lp_flag_*`:
  technique 8 of `mc_l_sm_flag_t0c0n0s0` is
  `lp_flag_sun_s_tc0n0_dtex_sm3.hlsl` and `lp_flag_sun_t0c0n0s0_sm3.hlsl`,
  10 to 13 the spot and omni ones, 4 `vertcol_simple_fog_flag_dtex`.
  VERIFIED (`--example techset_shaders common_mp mc_l_sm_flag_t0c0n0s0`).
  The waves' constants (`def c12`..`c19`) are the same bytes in techniques
  7, 8, 10 and 12. VERIFIED (bytecode).
- The vertex shader, with `f = (1 - u, 1 - v)` and `t` gameTime.w. VERIFIED
  (bytecode of `lp_flag_sun_s_tc0n0_dtex_sm3`, `mc_l_sm_flag_t0c0`'s
  technique 8 vertex shader):
  - Three phases `f . (3.6230519, 0.25334847) + 5.6548667 t`, `f .
    (8.250306, -1.4547516) + 9.424778 t`, `f . (19.577713, 5.2458324) +
    7.5398226 t`, each wrapped by `frc(p / 2pi + 0.5) * 2pi - pi` before
    `sincos`.
  - The vertex moves along its normal by `a (7.5 sin p0 + 3 sin p1 + 1.7
    sin p2)`, `a` the vertex colour's alpha.
  - The normal tilts to `N + 0.025 a (27.172890 cos p0 + 24.750919 cos p1
    + 33.282112 cos p2) T + 0.025 a (1.9001135 cos p0 - 4.3642545 cos p1 +
    8.917915 cos p2) B`, `B` the packed frame's `cross(N, T)` times the
    binormal sign (position w). The binormal handed on is `cross(tilted N,
    T)` times the sign; the tangent is unchanged.
  - The colour handed on is the vertex's RGB with alpha 1.
- Read as waves the constants are CoD2's (`lib/flag.hlsl`,
  cod2-materials.md 9.7): scales 1.73, 0.75, 0.31 at 4, -10 and 15
  degrees, speeds 0.9, 1.5, 1.2, amplitudes 1.5, 0.6, 0.34 times 5. Each
  slope constant is 5 times the amplitude times the wave's rate along u or
  v, scaled by 0.025 where CoD2 takes the `atan` of amplitude times rate
  over 2pi. INFERRED (arithmetic on the constants).
- The pixel shader is the plain set's (3.2) except the sun's diffuse,
  `texel.a * |dot(sunPosition, n)| * sunDiffuse`: both faces lit, the
  specular unchanged. VERIFIED (bytecode of `lp_flag_sun_t0c0n0s0_sm3`,
  `mc_l_sm_flag_t0c0`'s `lp_flag_sun_t0c0_sm3`).
- gameTime is `(sin 2pi frac t, cos 2pi frac t, frac t, t)`, `t` the
  scene's time in seconds. INFERRED (`R_SetGameTime`).

vcod: `vcod_cod4::techset::classify` reads `flag` as a lit set's token
(`TechClass::flag`), `Cod4Lit::flag` carries it, and `cod4_flag.wgsl`'s
`cod4_flag_wave` runs the vertex shader for the entity pass
(`dynamic_model.wgsl`, `DEFORM_COD4_FLAG`, model space) and for static
models (`cod2_world.wgsl`, `M_FLAG`, on the baked world vertex; the push
is not scaled by the static model's scale). The time is the renderer's
shader clock. Gates: `cod4_flag_wave_matches_the_hlsl` (lavapipe, against
the bytecode's arithmetic), the census (9.5).

### 3.5 Spot and omni primary lights

- `R_AllocModelLighting` (0x62eca0) takes a block, calls
  `R_CalcReflectionProbeIndex` (0x62ee03), then its callback for the
  model's non-sun primary light (0x62ee14), and hands that light to
  `R_CalcModelLighting` (0x628990, with the grid at `GfxWorld` + 0x110),
  whose return it stores as the lighting info's primary light (0x62ee34).
  The box callback (0x62ee70) calls 0x5fc5c0 with the mid point and half
  size `R_AllocModelLighting_Box` (0x62eeb0) makes: `(mins + maxs) * 0.5`
  (double at 0x70ace8) and `mid - mins`. VERIFIED. That the box is the
  scene entity's cull bounds is INFERRED
  (`R_AddAllSceneEntSurfacesCamera`).
- 0x5fc5c0 (`R_GetNonSunPrimaryLightForBox`) walks the lights from
  `sunPrimaryLightIndex + 1` (`GfxWorld` + 0xd8) below
  `primaryLightCount` (+ 0xdc), `ComPrimaryLight`s 0x44 apart from
  `[0x1435cc4]`, and returns the first not culled, or 0. A spot (type 2)
  with `cosHalfFovExpanded` (+ 0x34) at or above 0 takes 0x562fd0
  (origin + 0x1c, dir + 0x10, radius + 0x28, the expanded cosine); any
  other the sphere test 0x562ea0 (origin, radius). Then 0x5fb780 with the
  light's `GfxLightRegion` (`GfxWorld` + 0x240, 8 bytes a light). VERIFIED
  (0x5fc5c0-0x5fc6a7).
- 0x562ea0 culls when the squared sum of `max(|mid - origin| - half, 0)`
  per axis exceeds `radius^2`. VERIFIED. 0x562fd0 runs the same test, then
  takes the box corner `delta - half * sign(dir)` (`delta` the mid point
  less the origin; sign 1 at or above 0, -1 from 0x70ad40 below) and culls
  when its dot with `dir` is at or above 0. VERIFIED (0x562fd0-0x5631af).
- Past that, the perpendicular to the cone through that corner and the
  separating axis `dir + cos / sqrt(perp^2 sin^2) perp`: kept when
  `dist^2 sin^2 >= perp^2 cos^2`, else culled when the box lies wholly
  past that axis. INFERRED (the cone cull; the 1.7 bytes past 0x5631b7
  follow the same order). `dir` points back at the light, so the corner's
  test culls a box behind the spot. INFERRED (read off the test).
- 0x5fb780: a region of no hulls culls nothing; else the box (mid point
  less the light's origin) is culled when every hull culls it (0x5fb490 on
  each, 0x50 apart). VERIFIED. 0x5fb490 culls when the box's centre on any
  of the k-DOP's nine axes (x, y, z, x + y, x - y, x + z, x - z, y + z,
  y - z) is at least the hull's half size plus the box's own away from the
  hull's mid point. The first seven axes VERIFIED (0x5fb490-0x5fb65e). The
  last two and the hull's extra axes (each a direction, mid point and half
  size, 0x14) INFERRED (`R_CullBoxFromLightRegionHull`).
- A static model draws by its stored `primaryLightIndex` (0x45), its
  lookup's return dropped. INFERRED (`R_AddStaticModelsCamera`,
  `R_SetStaticModelLighting`).
- The light's constants (`R_SetLightProperties`, 0x649200, a `GfxLight`).
  VERIFIED (0x649215-0x6493b5):

  | Constant | Value |
  |---|---|
  | `lightPosition` | origin less the eye offset, w `1 / radius` |
  | `lightDiffuse`, `lightSpecular` | colour times `r_diffuseColorScale` (`[0xd569660]`), `r_specularColorScale` (`[0xd5695c0]`) |
  | `lightSpotDir` | dir, w 0 |
  | `lightSpotFactors` (spot) | `1 / (cosInner - cosOuter)`, `-cosOuter` times that, the exponent as an integer, the shadow fade |

  `r_diffuseColorScale` (name at 0x7051d0) registers 1, as
  `r_specularColorScale` does: the 1.0 stored ahead of the call at
  0x62a953. INFERRED (instruction order). That the `GfxLight` copies the
  `ComPrimaryLight`'s fields at the same offsets (cosines at 0x2c and
  0x30, the exponent a byte at 2) is INFERRED.
- `lp_omni_r0c0n0s0_sm3` (technique 12 of `mc_l_sm_r0c0n0s0`) and
  `lp_spot_r0c0n0s0_sm3` (10). VERIFIED (bytecode, `common_mp`):
  - `L` is the unit vector from the surface to `lightPosition`.
  - The colour is `attenuationSampler` at `saturate(distance *
    lightPosition.w)` on both coordinates; for a spot times `pow(s,
    factors.z)` with `s = saturate(L . spotDir * factors.x + factors.y)`,
    0 where `s` is at or below 0; all times the block's alpha.
  - Diffuse `2 * texel.rgb + colour * saturate(L . n) * lightDiffuse`; the
    specular the sun's with `L` and `lightSpecular` in its place. No sun.
  - Techniques 11 and 13 add the spot's shadow map. VERIFIED (shader
    names).
- The world sets' `lm_omni_r0c0_sm3` (technique 12 of `wc_l_sm_r0c0`) and
  `lm_spot_r0c0_sm3` (10) light as the model ones but add to the
  secondary's light (9.2): colour by attenuation times the primary
  lightmap's red, the spot's cone, `saturate(L . n)` and `lightDiffuse`;
  no sun. The attenuation is `lightmapSamplerSecondary` at
  `saturate(distance * lightPosition.w) * lightFalloffPlacement.xy +
  lightFalloffPlacement.zw`. VERIFIED (bytecode). The placement is (def
  image width / 512, 0, its `lmapLookupStart` / 512, 0), the def's row
  copied into the secondary. INFERRED (`R_SetShadowableLight`). Technique
  7 (`lm_r0c0_sm3`, `lp_t0c0n0_sm3`) reads no primary lightmap and no
  sun: the secondary's light, or twice the model block's colour, alone.
  VERIFIED (bytecode).
- A surface's technique follows its `primaryLightIndex`'s light type:
  index 0 (no light) technique 7, the sun 8 (9 with its shadow map), a
  spot 10, an omni 12. INFERRED (`R_UpdateDrawMethod`, `litTechType`).
  Lit world surfaces by stored index, VERIFIED
  (`sunshadow_census`'s `world_surfaces_by_primary_light`):

  | Map | Index 0 | Sun | Spot | Omni |
  |---|---|---|---|---|
  | mp_crash | 3183 | 1893 | 57 | 0 |
  | mp_carentan | 10790 | 3625 | 755 | 362 |
  | mp_bog, mp_killhouse and eight others | | | none past the sun | |

  525 of mp_crash's static models store index 0. VERIFIED.
- Every stock light def's attenuation image is 32 x 1 (`falloff_linear`,
  `falloff_quadratic`, `falloff_none`), sampler byte 98. VERIFIED
  (`lightgrid_census`'s walk). Every stock spot or omni light names one.
  VERIFIED (`lightgrid_census`). mp_carentan has 48 spot or omni lights;
  1774 of its 5092 static models store one, and a light their lookup
  takes reaches 40 of its 95 spawn points' boxes. VERIFIED (census).

vcod:

- `vcod_cod4::primarylight::PrimaryLights::for_box` is the choice over
  `Map::light_regions` (the walk keeps the hulls).
  `mapworld::entity_lighter` takes the instances' posed box (each lighting
  origin's union of `posed_box`; the view weapon's box is its lighting
  origin). `mapworld::model_lights` builds each light's constants with the
  def image's first row (`cod4_fly::attenuation_row`), read as the 1-high
  image's linear filter, clamped; the sampler state is not read.
- An entity's set carries the light (`entity_light::pack_cod4`) for
  `cod2_model.wgsl`'s `cod4_model`. A static model batch splits by primary
  light (`MapWorld::soup_lights`, `mesh::Batch::light`); its group's `Sun`
  uniform carries the light for `fs_cod4_model`.
- A world surface keeps its own index (`MapWorld::soup_lights`): its page
  group's `Sun` carries its spot or omni light, the sun, or no light
  (`SunUniform::none`), and `cod4_lit` draws technique 12, 10, 8 or 7 by
  it. A static model on index 0 draws 7 too.
- The world sets' falloff is read off the def's row as the models' is.
  Every stock lightmap page's secondary holds each light def's row in its
  first row from `lmapLookupStart`, laid out as
  `R_CopyLightDefAttenuationImage` copies it for the page's width (a
  1024-wide page, mp_carentan's first, takes each pair lerped at a quarter
  and three quarters); the lookup through the page's sampler reads within
  1.5 / 255 of the def's row through a linear clamped filter. VERIFIED
  (`sunshadow_census`'s `secondary_lightmaps_hold_the_falloff_rows`, 22
  rows). That the lightmap samplers are state 0x62, linear and clamped, is
  INFERRED (`R_DrawBspDrawSurfsLit`).
- Not ported: `lp_flag_spot_*`'s face (taken as the sun's `|N.L|`). Spot
  shadow maps (11, 13): below.

Gates: `cod4_spot_and_omni_light_a_sphere_as_the_hlsl_does`,
`cod4_model_shades_its_volume_block_as_the_hlsl_does`,
`cod4_world_surface_takes_its_primary_light_as_the_hlsl_does` (lavapipe),
`primarylight`'s unit tests, `lightgrid_census`.

#### The spot's shadow map (techniques 11 and 13)

- The bools and the floats register through `Dvar_RegisterBool`'s call
  (0x56c350); a float's value, minimum and maximum are the constants
  loaded ahead of it. VERIFIED (the disassembly, the constants):

  | Dvar | Name at | Registered at | Default, range | Flags |
  |---|---|---|---|---|
  | `r_spotLightShadows` | 0x705d00 | 0x62b4bb | bool, `ebx` | 0x80 |
  | `r_spotLightSModelShadows` ("Enable static model shadows for spot lights.") | 0x705d44 | 0x62b501 | bool, `ebx` | 0x80 |
  | `r_spotLightEntityShadows` | 0x705d88 | 0x62b547 | bool, `ebx` | 0x80 |
  | `sm_spotEnable` ("Enable spot shadow mapping from script") | 0x70692c | 0x62c11d | bool, `ebx` | 0x1080 |
  | `sm_spotShadowFadeTime` | 0x7069cc | 0x62c1b8 | 1 in 0.01..5 (0x6df494, 0x6cf674) | |
  | `sm_lightScore_eyeProjectDist` | 0x706a50 | 0x62c216 | 64 in 0..1024 (0x70b128, 0x70ba70) | |
  | `sm_lightScore_spotProjectFrac` | 0x706af8 | 0x62c270 | 0.125 in 0..1 (0x700be8) | |
  | `sm_polygonOffsetScale` | 0x706b30 | 0x62c2ce | 2 in 0..8 (0x700c0c, 0x6c6300) | |
  | `sm_polygonOffsetBias` | 0x706b60 | 0x62c32c | 0.5 in 0..32 (0x6be648, 0x6c62fc) | |
  | `sm_maxLights` | 0x706970 | 0x62c15a | stores 4 in its value slot, pushes 4 and 0, type 5 | |

  The bools share `ebx` as the default with `r_zFeather` and
  `r_distortion` (default 1), so their default 1 is INFERRED.
  `sm_maxLights`' default 4, range 0..4 and 5 as the int type are INFERRED
  (`dvarType_t`).
- `lm_sm_spot_r0c0_sm3` (technique 11 of `wc_l_sm_r0c0`, mp_crash): the
  vertex shader writes `shadowLookupMatrix` (c24..c27) times the world
  position. The pixel shader reads `shadowmapSamplerSpot` (s4) four times
  with `texldp` at that point moved by `+- spotShadowmapPixelAdjust.xy`
  and `.zw` times its w, compares each with the point's unprojected z (1
  at or above, else 0), averages the four, lerps from the primary
  lightmap's red to that average by `lightSpotFactors.w`, and multiplies
  the spot's colour by it; the rest is technique 10's. VERIFIED (bytecode,
  its arguments: code sampler 8 on s4, code constant 0x32 on c10,
  argument 0x4000054 on the matrix). `lp_sm_spot_t0c0n0s0_sm3` (technique
  11 of `mc_l_sm_t0c0n0s0`) does the same from the model block's alpha, on
  diffuse and specular. VERIFIED (bytecode).
- Each `ComPrimaryLight` carries a byte after its type, 0 or 1:
  mp_carentan 1 on 39 spots and 0 on one, mp_cargoship 0 on 3 of 13
  spots, mp_bloc on 1 of 14; every omni light 1. VERIFIED (`--example
  spot_lights`). That it is `canUseShadowMap`, which a spot must hold to
  be a candidate, is INFERRED (`ComPrimaryLight`,
  `R_AddPotentiallyShadowedLight`). That the shadowed light view takes a
  spot's cone, so vcod shadows no omni light, is INFERRED.
- The comparison takes the lookup's z unprojected, so the map holds clip
  z (world units up to the far plane), not z over w, and the polygon
  offset's `(bias / 4, scale)` (`R_DrawSpotShadowMapArray`) is in those
  units. INFERRED (read off the test).
- Retail's choice of shadowed spots per frame, out of the lights some
  visible surface stores. INFERRED (`R_ChooseShadowedLights`,
  `R_ShadowedSpotLightScore`, `R_FadeOutShadowHistoryEntries`,
  `R_AddShadowedLightToShadowHistory`, `R_AddSpotShadowsForLight`,
  `R_SetViewParmsForLight`, `R_GetSpotShadowLookupMatrix`):
  - Score: radius times the colour's intensity (0.2989, 0.587, 0.114) over
    one plus the distance from a point `sm_lightScore_eyeProjectDist`
    ahead of the eye to one `sm_lightScore_spotProjectFrac` of the radius
    down the beam. The best `sm_maxLights` are kept.
  - History, each frame: an entry whose light no visible surface stores
    goes at once. One still stored starts fading out (down `dt /
    sm_spotShadowFadeTime` a frame from the next, gone under 0.01) unless
    it is a candidate again, where it fades in (capped at 1). A candidate
    not held joins while fewer than `sm_maxLights` are held, at the
    frame's step when its light was stored last frame, else at 1. A frame
    at last frame's time moves nothing.
  - Each held entry takes a tile in order, its fade `lightSpotFactors.w`.
    A spot draws into a 512 tile of a 512 x 2048 map (pixel adjust
    (1/2048, 1/4096, 1/1024, -1/8192)) from its origin down the beam, near
    plane 1, far its radius, the outer cone's tangent both ways. The
    lookup is the light's view projection with x onto 0..1 and y onto its
    tile. `R_GetAllowShadowMaps` gates all of it.

vcod: `crate::cod4_spotshadow` keeps retail's history (`History`).

- The used lights are the primary lights of the frame's visible world and
  static model batches (`mesh::Batch::light`; entities do not mark
  theirs); candidates are their `canUseShadowMap` spots, best scored
  first, up to `sm_maxLights`.
- `SunShadowPass::encode_spot` draws, from each held light, the sun's
  casters (world surfaces; static models under `r_spotLightSModelShadows`,
  alpha-tested ones cut) and, under `r_spotLightEntityShadows`, the
  entities into its 512 tile of the column right of the sun's partitions
  in their atlas (the world pipeline has no sampled texture slot left for
  a map of its own), storing clip z over the far plane. Lookups and taps
  run in retail's 512 x 2048 map coordinates, each tap kept to its tile.
- `cod4_spot_shadow` (`cod4_sunshadow_recv.wgsl`) takes technique 11 in
  `cod4_lit` and `fs_cod4_model` and 13 in `cod4_model` for an `sm` set
  whose spot holds a tile. The maps run only under `sm_enable`,
  `r_spotLightShadows` and `sm_spotEnable`.
- A tile's static casters draw into a depth cache of its own, redrawn when
  its light, the casters, `r_spotLightSModelShadows` or the polygon offset
  change; each frame copies it into the atlas and draws over it the
  entities its light's frustum reaches (cod4-client-perf.md 9).
- `--example spot_lights` lists each map's spots and the surfaces that
  store them. A change of held lights logs `cod4 spot shadow: lights [..]`
  at debug.
- Gates, VERIFIED (the tests, ours):
  `cod4_live_spot_light_shadows_its_ground` joins vcod-server's war on
  mp_crash, holds a bot on the tiles under light 3 (60 -684 255) and checks
  the light is held and the bot's shadow darkens the tiles against
  `r_spotLightEntityShadows` 0.
  `cod4_live_spot_shadows_hold_several_lights_and_fade_out` looks over
  mp_carentan's house lights from (-760 1900 230): lights 3, 8 and 5 take
  tiles; at `sm_maxLights` 1 the other two keep theirs at fade 0.75 a
  frame later and are gone after the fade.
- Cost, VERIFIED (our runs): `tools/cod4_perf.sh` on mp_carentan's dm (40
  s, lavapipe, 1280x772, 4x MSAA, `sm_enable 1` set in game) read the sun
  and spot passes at 64.6 GPU ms with `sm_spotEnable` 1 and 60.7 with 0,
  about 4 ms for the spot's tile. With the history, `sm_maxLights` 4 read
  61.1 (up to four tiles held, `[15, 23, 35, 4]` among them) and 1 read
  57.1; the views followed a bot, so they differ. Not measured on a real
  GPU.

### 3.6 The sun shadow map

How a lit set with `sm` in its name (`wc_l_sm_*`, `mc_l_sm_*`) draws the
sun's shadow while it is on: technique 9 (`lm_sm_sun_*_sm3.hlsl` for the
world, `lp_sm_sun_*_sm3.hlsl` for a model) over an atlas technique 3
(`build_shadowmap_color_*`) fills each frame. Shaders are mp_crash's
(`--example techset_shaders mp_crash wc_l_sm_r0c0` and `mc_l_sm_t0c0n0`,
`tools/re/d3d9dis.py`).

The receivers. VERIFIED (bytecode of `lm_sm_sun_tc0_sm3.hlsl`,
`lp_sm_sun_tc0n0_dtex_sm3.hlsl`; `def c5`, `c1.w`, `c6`, `c7`; `ifc_ge`
1, `ifc_lt` 0.75):

- The vertex shader writes `shadowLookupMatrix` (c24..c27) times the
  world position into texcoord `s` (four `dp4`s), so `s` is affine in the
  world position.
- The 4-tap test reads `shadowmapSamplerSun` with `texldl` at `s.xy` plus
  (-1/2048, 1/8192), (1/4096, 1/4096), (-1/4096, -1/4096), (1/2048,
  -1/8192); a tap is 1 where its red is at or past `s.z` (`cmp` on the
  difference); the four average (`dp4` with 0.25).
- `s.w` at or below 1 reads the near partition at `s.xy`; past 0.75 the
  far one is read too at `s.xy * shadowmapSwitchPartition.w +
  shadowmapSwitchPartition.xy`, blended toward the far by `4 s.w - 3`.
  Past 1 the far one alone at those coordinates, with the edge `max(|x -
  0.5| shadowmapScale.x, |y - 0.75| shadowmapScale.y)`: at or past 8 the
  baked visibility as is, else the far test blended into it by
  `saturate(edge - 7)`.
- The baked visibility is the primary lightmap's red on the world and the
  model block's alpha on a model. The world shader takes no sun where the
  primary reads 0 (`cmp` on `-abs(r1.x)`, then `ifc_ne`); the model shader
  has no such test. In the near partition the test replaces the baked
  visibility outright. The result scales the sun's `N.L` diffuse as the
  baked visibility does under technique 8.

The casters:

- `shadowmap_build.hlsl` writes the clip position and its z into a
  texcoord; `shadowmap_build_sm3.hlsl` stores `z + shadowmapPolygonOffset.x
  + shadowmapPolygonOffset.y (|ddx z| + |ddy z|)` into the colour target
  (`dsx`, `dsy`). The `atest` variants read the colour map first.
  VERIFIED (bytecode of `wc_l_sm_r0c0`'s technique 3).
- A material casts when it has a technique 3 state entry. Over the 21
  stock maps' world and static model materials those are the lit `r0` and
  `t0` sets except the flag ones, both `shadowcaster` sets and
  `mc_ambient_t0c0`. Every one draws both faces, alpha-tested at 128 when
  its set is a `t0` (or `mc_shadowcaster`, `mc_ambient_t0c0`), polygon
  offset field 0 or 1. VERIFIED (`crates/cod4/tests/sunshadow_census.rs`).
- `R_DrawSunShadowMap` gives partition `p` the viewport (0, 1024 p, 1024,
  1024): the atlas stacks near over far, 1024 x 2048. INFERRED.
- 0x65ad10 sets code constant 9 (`shadowmapPolygonOffset`) per partition
  to (`sm_polygonOffsetBias` (`[0xd569850]`) times 0.25 (double at
  0x70b058) times the partition's float at +0x5798,
  `sm_polygonOffsetScale` (`[0xd5695f0]`), 0, 0); on the other arm, when
  `[0xd5696c0]` reads 0 and `[0xcc9f470]` reads 3, to (bias times 4
  (0x70b340) times that float, 0, 0, 0). VERIFIED (0x65ad92-0x65ae0f).
  That the float is the partition's projection `m22` and the arm is
  `r_rendererInUse` 0 with the colour build technique is INFERRED
  (`R_DrawSunShadowMap`).

The dvars:

- `sm_sunSampleSizeNear` (name at 0x706b8c) registers 0.25 (0x70b2b4) in
  0.0625 (0x700be0) to 32 (0x6c62fc), handle `[0xd569738]`. VERIFIED (the
  float loads at 0x62c335-0x62c390).
- `sm_enable` (0x7068c0) registers the byte in `bl`, which `mov ebx, 1` at
  0x629df3 sets. INFERRED (register flow).
- `sm_polygonOffsetScale` (0x706b30) registers 2.0 (0x700c0c),
  `sm_polygonOffsetBias` (0x706b60) 0.5 (0x6be648). INFERRED (which load
  precedes which call).

Where the partitions go (`R_SetupSunShadowMaps`, 0x639900):

- It calls 0x63dc10 (`R_GetSunAxes`), 0x6386c0
  (`R_SetupSunShadowMapProjection`), per partition 0x638240 and 0x638040
  with the sample size times `[0xcc9f440]` between the two, then 0x6397a0
  (`R_GetSunShadowLookupMatrix`). VERIFIED (the calls in
  0x639900-0x639ac7).
- 0x63dc10: forward is the vector at front end data + 0x11e6e0 negated;
  the up seed is z, or x when forward's squared horizontal length is below
  0.1 (double at 0x70b040); right is up cross forward, normalized; up is
  forward cross right. VERIFIED (0x63dc10-0x63dcdb). That vector is the
  sun light's direction, toward the sun. INFERRED (`R_GetSunAxes`).
- 0x638240: `m00` and `m11` are 2 over the sample size times 1024
  (0x70b1e8; 2.0 at 0x70ae50), `m30` and `m31` the snapped view origin's
  clip position, `m22` 1 over (far - near + 2), `m32` -(near - 1) times
  that, `m33` 1. VERIFIED (0x638240-0x6382d3). Near and far are the
  world's bounds along forward. INFERRED (`R_GetSceneExtentsAlongDir`).
- 0x6382f0: the near sample size times `zNear` (view parms + 0x138) times
  its argument; forward (+0x110) times the reciprocal of that; w the view
  origin (+0x100) dotted with the result, negated. The result is 1 where
  the near partition's reach along the view ends. VERIFIED
  (0x6382f0-0x638380). The argument is 1023 over the near box's longer
  side (`[esp+0xb4]`, 0x63976a). INFERRED.
- 0x6397a0: x maps clip -1..1 onto 1/2048 .. 1 + 1/2048, y onto 0.5 +
  1/4096 .. 1/4096 (doubles at 0x70aec8, 0x70aec0, 0x70aeb8, 0x70aeb0; 0.5
  at 0x70ace8); z is the near partition's clip z, w the fraction above.
  Each of the first two columns is the near view-projection's column
  times the scale plus its fourth column times the shift; the third is
  copied, the fourth the fraction. VERIFIED (0x6397a0-0x6398fa).
- 0x6386c0 puts the view origin and the near plane's corners (0x637d90) on
  the sun's plane, x `-(p . right)`, y `p . up`. The near box spans the
  corners then the origin; the far box the corners then each corner
  pulled toward the origin by 0.75 (0x70b0c8) over the partition ratio
  (`[0xcc9f440]`); boxes seed at plus and minus `FLT_MAX` (0x70af58,
  0x70af5c). VERIFIED (0x6386c0-0x638d3e). The origin snaps to
  `floor(o / snap) * snap` (0x401020, 0x68b360), the snap the scale's
  numerator (`[0xcc9f438]`) times the far sample size, the near one
  (`sm_sunSampleSizeNear`) times the ratio. VERIFIED (0x638d89-0x638e1f).
- The far square runs from `(1024 - sunShadowSize) / 2` to 1024 less
  that, its middle texel their mean (512). The fit is 1023 (0x70b9b8)
  over the near box's longer side; the far share is the size times 1/1024
  (0x70af38). The near viewport's width and height are `ceil(1024 /
  maxSize * extent - 0.001)` (0x70af98, ceil 0x6715d0), at partition +
  0x1ec and + 0x1f0. VERIFIED (0x638e1f-0x638fe8).
- Per axis (x when the origin lies below the near box's middle, y when
  above; 0x639009, 0x639263). VERIFIED (0x639009-0x6394c4):
  - First arm: the near partition starts at 0; the origin's texel is its
    distance from that side (x the low, y the high) times the fit, plus 1
    (1.0 at 0x70ad08). The far partition's texel is `begin + 1 + distance
    * fit * share`, start `max(floor(fit * gap + begin), begin)` for the
    far box's start edge past the near box's, length the far end less the
    start.
  - Other arm: the near viewport ends at 1024 and the texel is 1023 less
    the distance to the other side times the fit. The far texel is `end -
    1 - distance * fit * share`, start `begin`, length `ceil(end - fit *
    gap) - begin` for the near box's end edge past the far box's, capped
    at the size.
  - A centre (`sm_sunShadowCenter` not 0 0 0; three `fucom`s at
    0x638eb5-0x638edd) makes the far texel 512 plus (first arm) or minus
    the origin's offset from the centre over the far sample size, the
    start `begin`, and on the other arm the length the size.
  - Without `rg.sunShadowFull` the far viewport copies the near one.
- The far viewport is at partition + 0x3e4 .. + 0x3f0. VERIFIED. 1.7
  stores a negative far length as is; vcod's empty span is 0
  (`Fit::place`).
- Each partition's origin texel is `floor(shift.x + texel.x)` and
  `floor(texel.y - shift.y)`, the shift the snapped origin less the origin
  over the partition's sample size, times 1/1024.
  `shadowmapSwitchPartition` is (far x - near x w, (far y - near y w + 1)
  / 2, 0, w), w the ratio's reciprocal; `shadowmapScale` (16, 32, 0, 0)
  over the scale (0x70b1f8, 0x70b120); each partition's clip origin `(2 x
  - 1, 1 - 2 y)` (2.0 at 0x70ae50). The sun's view matrix's columns are
  right negated, up and forward, the translation the snapped origin
  negated. VERIFIED (0x6394ca-0x639720). That vcod's `f32` arithmetic
  matches assumes the x87 at single precision. INFERRED.
- A surface or model whose primary light is the sun draws technique 9
  while the map is on, else 8. INFERRED (`R_UpdateDrawMethod`,
  `litTechType`).

The far partition's scale (`R_GenerateSortedDrawSurfs`, 0x5f9900 on):

- `rg.sunShadowFull` (`[0xcc9f430]`) is `r_rendererInUse` (`[0xd5696c0]`)
  equal to 1, shader model 3 (0x5f994a-0x5f995a). VERIFIED. With it set,
  `sm_sunShadowScale` (`[0xd569844]`) is replaced by the nearest fraction
  `num / den`, `den` 1 to 10, `num = (int) floor(den * scale + 0.5)`, the
  first with the least `|num / den - scale|` kept. `rg.sunShadowmapScale`
  (`[0xcc9f434]`) takes the fraction, `[0xcc9f438]` its numerator,
  `rg.sunShadowSize` (`[0xcc9f43c]`) `ceil(scale * 1024)`,
  `rg.sunShadowPartitionRatio` (`[0xcc9f440]`) `4 / scale`. Without it the
  size is 1024, both scales 1, the ratio 4.0 (0x6bdf00). VERIFIED
  (0x5f9966-0x5f9afe; floor 0x401020, fabs 0x401000, ceil 0x6715d0, 1024
  at 0x70b1e8, 4 at 0x70b340).
- `sm_sunShadowScale` (name at 0x706c08, "Sun shadow scale optimization")
  registers 1 (`fld1` at 0x62c3da) with 0.25 (0x70b2b4) loaded beside it,
  flags 0x1080. `sm_sunShadowCenter` (0x706bd4, "Sun shadow center, 0 0 0
  means don't override") registers a vec3 of 0 in -FLT_MAX..FLT_MAX
  (0x70af58, 0x70af5c), flags 0x1080, handle `[0xd569768]`. VERIFIED
  (0x62c3a3-0x62c438). That 0.25 is the scale's minimum and 1 its maximum
  is INFERRED (argument order).
- In the projection the scale numerator times the far sample size is the
  view's snap. The far square is `sunShadowSize` texels centred in its
  1024, its texels scaled by `size / 1024`; a centre other than 0 0 0
  puts the square's middle (texel 512) on that point instead of fitting
  it to the view; `shadowmapScale` is (16, 32) over the scale. VERIFIED
  (0x6386c0, above).

The casters' cull (`R_SetupSunShadowMaps`'s partition loop,
0x6399b0-0x639a9d):

- Per partition: 0x638240 (the projection), 0x638040
  (`R_SunShadowMapClipSpaceClipPlanes`: the bounding polygon at partition
  + 0x58, the partition index, the sample size), then 0x60b850
  (`R_SetupShadowSurfacesDpvs`) with its planes. VERIFIED (the calls).
- `R_SetupSunShadowBoundingPoly` (0x637e80, called at 0x639730 and
  0x639765) takes the hull (`Com_ConvexHull`, 0x55ad30) of the near box's
  five points (near plane corners, view origin) or the far box's eight
  (corners, corners pulled toward the view), scales each by `2 / maxSize`
  about the view origin (2.0 at 0x70ae50) onto the partition's snapped
  clip origin, and marks a point near when it is one of the points from
  index 4 on, compared for equality. VERIFIED (0x637e80-0x637f86). So
  scaled, the near polygon is the view frustum on the sun's plane cut
  where the near partition's reach ends, the far one between 0.75 of that
  reach and the far partition's. INFERRED (the arithmetic).
- 0x637fc0 (`R_SunShadowMapBoundingPoly`) shifts the points by the snap,
  `2 / (sampleSize * 1024)` times the view's offset from its snapped
  origin. VERIFIED (0x637fc0-0x638035).
- 0x638040 makes a plane of each edge, `(y1 - y0, x0 - x1, 0, -(a x0 + b
  y0))`: on the near partition for every edge, on the far one only for
  edges with a near point when `rg.sunShadowFull` is set. On the far
  partition with it set it adds four planes of the square at plus and
  minus `rg.sunShadowmapScale`, each coefficient times that scale (-1 and
  -2 at 0x70b2d8, 0x70b3f8). VERIFIED (0x638040-0x638227).
- 0x60b850 takes the clip planes to the world through the partition's
  view-projection (0x605e70, `R_FrustumClipPlanes`). It adds the
  partition's near region plane (`[0xd064f00]`, 0x14 a partition) when its
  normal dotted with the sun's forward (shadow view + 0x110) is below 0;
  on the near partition (or with `sunShadowFull` clear) its far region
  plane (`[0xd064ed8]`) under the same test; and each of the view's four
  side planes (`[0xd070148]`, 0x14 apart) under it. VERIFIED
  (0x60b850-0x60ba50).
- The region planes (0x638390): the near ones (`[0xd064f00]`,
  `[0xd064f14]`) are the partition fraction (the lookup's w row) less 0
  and 0.75 (0x70b0c8) in w, each times the reciprocal of its normal's
  length; the far ones (`[0xd064ed8]` on) the fraction less 1 and less the
  ratio, times -1 (0x70b2d8) over that length. 0x605e70 scales each clip
  plane brought to the world by its reciprocal length too. VERIFIED
  (0x638390-0x6386bf, 0x605f14-0x605f64).
- A cell, a surface tree node and a static model are dropped when their
  box lies at or past one plane (`<= 0` at its corner farthest along the
  normal). INFERRED (`R_AddWorldSurfacesFrustumOnly`).
- `sm_strictCull` (name 0x706c34, handle `[0xd569620]`) walks only the
  cells `cellCasterBits` lists for a visible cell, the bits built at load
  by a portal walk from each cell toward the sun
  (`R_GenerateShadowMapCasterCells`). `sm_fastSunShadow` (0x706c54,
  `[0xd5696d4]`) walks a presorted surface list while the sun's direction
  holds. INFERRED. Both register with flags 0x80 (0x62c46a, 0x62c4b0).
  VERIFIED.

vcod:

- `vcod_cod4::sunshadow::setup` (0x639900's steps; `Fit::place` per axis)
  is the placement and the cull planes (`Partition::cull`) at
  `sm_sunSampleSizeNear`, `sm_sunShadowScale` and `sm_sunShadowCenter`
  (`Settings`), with `rg.sunShadowFull` set; `visibility` is the
  receivers' arithmetic on the CPU.
- The client (`crate::cod4_sunshadow`, `sm_enable` 1) draws a
  `Depth32Float` atlas each frame. The casting world and static model
  soups group in 128-unit cells (`sunshadow::caster_groups`: solid ones,
  then by alpha-tested material); a cell draws into a partition when its
  planes keep its box, each partition's viewport a scissor, the rest
  cleared to 1. Then every live entity whose posed box the partition's
  planes keep draws its casting surfaces (`CookieRole::sun_caster`),
  skinned, an alpha-tested one through its first tested stage's image at
  128.
- The pixel writes the stored depth through `frag_depth`, so the depth
  test runs on the offset depth where retail's tests the plain one and
  writes the offset into R32F. The casters shift half a texel up and left
  (D3D9's texel centres against wgpu's), so the lookup's half texel lands
  where retail's does.
- `cod2_world.wgsl`'s `cod4_lit` and `fs_cod4_model` and
  `cod2_model.wgsl`'s `cod4_model` read it through
  `cod4_sunshadow_recv.wgsl` (camera group bindings 18, 19); the view
  model reads none.
- Each partition's static casters draw into a depth cache of their own,
  redrawn only when the partition's matrix, viewport, offset, cull planes
  or casters change; each frame copies it into the atlas and draws the
  entities over it (cod4-client-perf.md 10).
- Not ported: `sm_strictCull`'s caster cells, `sm_fastSunShadow`'s list,
  retail's per-surface and per-static-model cull (the 128-unit cells stand
  in). Spot shadow maps (11, 13): 3.5.
- F3's `sunshadow` line gives the world triangles drawn of all, the
  entity surfaces and the passes' GPU ms.

Caster cull, from each stock spawn point's standing view (80 degrees
across at 16:9, along the spawn's yaw), thousands of caster triangles
drawn on average. VERIFIED (`sunshadow_census`'s
`caster_culling_from_the_spawn_views`):

| Map (casters) | Near, far under 512-unit squares | Near, far under the planes at 128 |
|---|---|---|
| mp_crash (940) | 120, 262 | 28, 121 |
| mp_bog (773) | 109, 222 | 16, 104 |

The lavapipe frame over mp_crash in `pipeline_check` drew 537k triangles
in 60.7 ms under the squares and 73k in 47.9 ms under the planes, the
image the same to the byte sum. Not measured on a real GPU.

Static models cast at the LOD the camera picks, as 1.7's sun shadow pass
does: its loop (0x63b3d0) reads the corrected rigid ramp (0xcc9f338,
0xcc9f33c at 0x63b3da, 0x63b3ea). VERIFIED. It culls and picks per
partition as the camera pass (0x63ac70) does. INFERRED (its compare at
0x63b4fe; `R_AddAllStaticModelSurfacesRangeSunShadow`). That the spot pass
(`R_AddAllStaticModelSurfacesSpotShadow`) picks by the camera's distance
too, so vcod's spot tiles take the same picks, is INFERRED.
`sunshadow::smodel_casters` keeps every LOD's casting runs;
`SunShadowPass::pick_smodel_lods` picks them each frame through
`StaticLod::pick` (9.10), and the caches key on the picked models. From
the spawn views the partitions hold 36k static model triangles a view on
mp_crash and 29k on mp_bog at the picks, 70k and 78k at LOD 0
(`caster_culling_from_the_spawn_views`). The sun shadow gate's world holds
no static model, so the picks do not move its atlas.

Gates: `cod4_floor_takes_the_sun_shadow_map_as_the_hlsl_does` (lavapipe:
the atlas read back, three views against the bytecode's test),
`vcod_cod4::sunshadow`'s unit tests (the scale's fraction, the half
scale's square, the planes keeping the view's spot in the near partition
and dropping what lies behind it under a sun ahead), `sunshadow_census`,
`cod4_renderer_builds_and_draws_without_validation_errors`.

## 4. Technique sets by name

The world surfaces of the 21 maps use 34 techsets. VERIFIED. The names
spell what they draw; `vcod_cod4::techset::classify` reads them (INFERRED
from the names alone, not the shaders):

| Part | Meaning |
|---|---|
| `wc_`, `mc_` | world set, model set; a leading `,` marks a name stub resolved from an earlier zone |
| `l`, then `sm` and letter-digit codes | lit, sun shadow map; `r0` opaque, `t0` alpha test, `b0` blend, `a0` add; `c0` colour, `n0` normal, `s0` specular, `d0` detail map. Most used: `wc_l_sm_r0c0n0s0` (235878 surfaces over the 21 maps) |
| `unlit` and a suffix | none, `replace`, `nofog`, `distfalloff` read as opaque; `alphatest`, `blend`, `add`, `falloff_add`, `multiply`. The state bits override the name's blend (section 8): `distfalloff` doubles the framebuffer (9.4). `multiply` (decals) is the second most used family (37145 surfaces) |
| `mc_effect_[zfeather_][falloff_]add`, with `_nofog`, `_eyeoffset` (`mc_effect_add_eyeoffset` too) | static models' effect sets: unlit add sets whose vertex shader adds the named extras (9.5) |
| `effect_*` | the effects' own sets, same grammar without the prefix (`effect_zfeather_add_nofog`); `effect`, `effect_zfeather`, `effect_falloff` without the add are blended sets whose shaders do not premultiply (cod4-effects.md 13) |
| `sky`, `water`, `distortion_scale_zfeather` | their own paths |
| `tools`, `shadowcaster` | draw nothing in the lit view |

Unclassified: `wc_default` (20 surfaces) and `wc_ambient_t0c0` (266).
VERIFIED (`map_census`). Both draw through the stage path under their
state bits (9.5). A set's actual blend lives in its material's state bits
(section 8).

A texture def's `nameHash` is `R_HashString` of the sampler name, `h = (c
| 0x20) ^ (33 * h)`: colorMap 0xA0AB1041 (semantic 2), detailMap
0xEB529B4D (2), normalMap 0x59D30D0F (5), specularMap 0x34ECCCB3 (8). The
formula is INFERRED. The four values match mp_crash's materials. VERIFIED.

## 5. Images

- A zone image whose load def has a nonzero `resourceSize` carries its
  pixels: each face's levels largest first, face after face, filling
  `resourceSize` exactly. The level count is `Image_CountMipmaps`': one
  under the load def's flag 0x2 (`IMG_FLAG_NOMIPMAPS`), else down to 1x1.
  INFERRED (`Load_Texture`). VERIFIED by size on every stock map's zone
  images. The reflection probes (A8R8G8B8 64x64 cubes, flags 4) hold seven
  levels per face (131064 bytes) while their `levelCount` byte says 1.
  VERIFIED.
- `format` is a `D3DFORMAT`: the stock map zones use 21 (A8R8G8B8, stored
  B G R A), 22 (X8R8G8B8), 50 (L8) and `DXT1`, `DXT3`, `DXT5`. Only the
  lightmaps, reflection probes and `$outdoor` carry pixels in a map zone
  (mp_crash: 22 of its 823 images; over the 21 maps 64 lightmap images,
  407 probes, 21 `$outdoor`). VERIFIED.
- A lightmap is two zone images: `*lightmap<n>_primary`, L8 at twice the
  width (mp_crash: 1024 x 2048), and `*lightmap<n>_secondary`, A8R8G8B8
  (512 x 2048). VERIFIED. Shaders: 9.2.
- Every other image's pixels are `images/<name>.iwi` in the iwds, named as
  the zone spells it (`~ch_asphalt01_spc-rgb&ch_asph~5b25deb6`), a stub's
  `,` dropped. VERIFIED (`map_census`: every image a stock map names is in
  its zone or the iwds, but two).
- The two are the water materials' `watersetup0` (12 maps) and
  `watersetup1` (one), with no pixels anywhere. VERIFIED (their absence).
  The engine makes them at run time from the material's `water_t`.
  INFERRED (`Load_Texture` creates a category-water image without data).
- `.iwi` version 6 keeps version 5's 28-byte header (`IWi`, version,
  format, flags, three u16 dimensions, four u32 `fileSizeForPicmip`) and
  format numbers (1-5 bitmaps, 6-10 wavelet, 11-13 DXT1/3/5), and stores
  the smallest level first. `fileSizeForPicmip[k]` is the file's size with
  `k` levels dropped. VERIFIED (`iwi_census`: all 6561 iwis on the search
  path decode, each read stopping where those sizes say). The stock iwds
  use formats 1, 2, 3, 4, 6, 7, 8, 9, 11, 12 and 13; 48 are cubes.
  VERIFIED.
- The flags add 0x10 streaming, 0x20 legacy normals, 0x40 and 0x80 clamp
  to v5's 1, 2, 4, 8. INFERRED. Stock files carry 0x20, 0x40, 0x80, 0xC0,
  0xC3, 0xC6 among others. VERIFIED.
- The wavelet decoder is CoD2's: `iw3mp.exe` carries its three prefix-code
  tables byte for byte (FNV-1a hashes as CoD2's) at 0x6A1E08, 0x6A5E08
  and 0x6A9E08. VERIFIED. vcod reads them from the install's `iw3mp.exe`;
  the 25 wavelet iwis decode to clean pictures (looked at: `sun_flare`,
  `auto_adjust`, `logo_cod2`).

## 6. Collision map

`vcod_cod4::map::check_clip` decodes every stored pointer of the clip map
into an index and range-checks every index; the 21 maps pass with 58960
(mp_shipment) to 885110 (mp_carentan) checks. VERIFIED. Layouts INFERRED
(`cbrushside_t`, `cNode_t`, `cLeaf_t`, `cLeafBrushNode_s`,
`CollisionPartition`, `CollisionAabbTree`, `cmodel_t`, `cbrush_t`). What
the data showed:

- The clip map's `planes` slot is an offset token into the render world's
  `dpvsPlanes.planes`, which the zone loads first: one plane array serves
  both. VERIFIED (mp_crash; `map_spots` compares them).
- Brush sides, nodes and brushes store real pointers (offset tokens) to
  planes, sides and edges, each on an entry boundary. VERIFIED.
- `numClusters` is 1 on every stock MP map, with one PVS row of
  `clusterBytes` (mp_crash 120), while a leaf's `cluster` (i16 at 0x28)
  runs past 1 (mp_crash up to 947): a cluster is a bit of that one row.
  VERIFIED (every leaf cluster is below `8 * clusterBytes`).
- A leaf brush node with a positive `leafBrushCount` is a leaf whose list
  is inline or an offset into the map's leaf brushes. The last node's list
  holds brush number `numBrushes`, the box model's leaf naming the box
  brush. VERIFIED (only each map's last node does it). That the number is
  the box brush is INFERRED (Q3's `CM_InitBoxHull` numbers it so).
- A node with a negative count tests `node + 1` then its children, one
  with 0 only its children, each `childOffset` nodes on. INFERRED
  (`CM_PointContentsLeafBrushNode_r`). Every such offset stays in the
  array. VERIFIED.
- A brush with no adjacent-side edges (all six `edgeCount` 0) may point
  `baseAdjacentSide` anywhere; stock ones point at the brush sides' start
  (mp_countdown brush 6785, mp_crossfire). VERIFIED. Every brush with
  edges keeps its six axial runs inside the edge array. VERIFIED.
- A partition's borders pointer names the border array; an AABB tree with
  children names `childCount` trees from `u`, one without names a
  partition. VERIFIED (in range on all 21).
- `leafsurfaces` is empty on mp_crash. VERIFIED.

The trace over these records, and its census: `cod4-collision.md`.

## 7. Entities and the mp_crash spot values

The entity string (`MapEnts`, inside the clip map) parses with the `{
"key" "value" }` grammar after its trailing NUL is dropped; every map's
first block is `worldspawn`. VERIFIED (all 21).

mp_crash (`map_spots`, VERIFIED):

| What | Value |
|---|---|
| name, bounds | `maps/mp/mp_crash.d3dbsp`, (-4160, -4736, 0) .. (4864, 5632, 2176) |
| render | 166418 vertices, 347493 indices, 5558 surfaces, 134 world materials (278 with the static models'), 23 cells, 180 portals, 4038 static models, one lightmap |
| entities | 309 (19 `mp_tdm_spawn`) |
| surface 0 | `wc/me_wire_black` (`wc_l_sm_r0c0`, colour map `me_wire_black`, a 256 x 32 DXT iwi), 14 triangles at index 214692 |
| static model 0 | `me_corrugated_metal4x8` at (1332, -14, 292.5) |
| collision | 11400 planes, 10978 brushes, 21774 sides, 2130 nodes, 2165 leafs, 12075 leaf brush nodes, 19627 leaf brushes, 2508 AABB trees, 1028 partitions, 10220 verts, 14977 triangles, 34 submodels, 516 dynamic entities, 296199 indices checked |

The `map_obj` example exports it to OBJ + MTL with the colour maps as TGA;
its top-down view shows the town's streets and blocks.

## 8. State bits

A material's `stateBitsEntry[34]` names, per technique type, an entry of
its `GfxStateBits` array (`loadBits[2]`). `vcod_cod4::state` decodes one;
`draw_state` takes the entry of `TECHNIQUE_LIT` (7), else
`TECHNIQUE_UNLIT` (4), else `TECHNIQUE_EMISSIVE` (5). The technique
numbers are INFERRED (`MaterialTechniqueType`). That 7 is the lit one is
VERIFIED by its pixel shader's name, `lm_r0c0_sm3` (9.2).

The bit layout is INFERRED:

| Word | Bits | Field |
|---|---|---|
| 0 | 0x0000000F | colour source factor |
| 0 | 0x000000F0 | colour destination factor |
| 0 | 0x00000700 | colour blend op, 0 for no blending |
| 0 | 0x00000800 | alpha test off |
| 0 | 0x00003000 | alpha test: 0x1000 `> 0`, 0x2000 `< 128`, 0x3000 `>= 128` |
| 0 | 0x0000C000 | cull: 0x4000 none, 0x8000 back, 0xC000 front |
| 0 | 0x07FF0000 | the alpha channel's factors and op, same layout |
| 0 | 0x18000000 | colour and alpha writes |
| 1 | 0x00000001 | depth write |
| 1 | 0x00000002 | depth test off |
| 1 | 0x0000000C | depth test: 0 always, 4 less, 8 equal, 0xC less-equal |
| 1 | 0x00000030 | polygon offset: 1 and 2 the decal offsets, 3 the shadow map's |
| 1 | 0xFFFFFFC0 | stencil |

The tables that turn the fields into D3D9 render states sit in
`iw3mp.exe`'s `.rdata` in 60 copies, the first at 0x6A1270: cull (0, 1,
3, 2: none `D3DCULL_NONE`, back `D3DCULL_CCW`, front `D3DCULL_CW`), blend
factors 0..10 and ops 0..5 each onto the same `D3DBLEND`/`D3DBLENDOP`
value, depth tests (8, 2, 3, 4: `ALWAYS`, `LESS`, `EQUAL`, `LESSEQUAL`),
two stencil tables 1..8. VERIFIED (bytes). Which field indexes which
table is INFERRED.

On mp_crash the stock world sets' lit states agree with their names, one
word pair per set except the polygon offset. VERIFIED (the `state` unit
test pins them):

| Set | Word 0 | Word 1 | Reads |
|---|---|---|---|
| `wc_l_sm_r0c0*` | 0x18128812 | 0xD | no blend, back cull, depth write |
| `wc_l_sm_t0c0*` | 0x1812B012 | 0xD | alpha `>= 128` |
| `wc_l_sm_b0c0*` | 0x19289162 | 0xC | (One, InvSrcAlpha), no depth write |
| `wc_unlit_multiply` | 0x19288931 | 0xC | (Zero, SrcColor) |
| `wc_unlit_falloff_add` | 0x1812492A | 0xC | (InvDestColor, One), no cull |
| `wc_unlit_distfalloff` | 0x19288939 | 0xC | (DestColor, SrcColor) |
| `wc_sky` | 0x18128812 | 0xC | no blend, no depth write |

A word 1 of 0x1D carries polygon offset 1. VERIFIED (mp_crash's model
sets).

### 8.1 Premultiplied colour

The add and `b0` pixel shaders write colour times alpha, so a source
factor of One reads as SrcAlpha on an unpremultiplied colour:

| Shader | Sets | Writes | Label |
|---|---|---|---|
| `vertcol_simple_add_fog.hlsl` | `wc_unlit_add`, `wc_unlit_falloff_add` (technique 7, mp_backlot, mp_crash) | `rgb = lerp(fogColor, tex * vertex, fog) * a`, `a = tex.a * vertex.a` | VERIFIED (bytecode, `--example techset_shaders`, `tools/re/d3d9dis.py`) |
| `vertcol_simple_add.hlsl` | `mc_effect_falloff_add_nofog` (technique 4, mp_backlot) | the same without fog | VERIFIED (bytecode) |
| `zfeather_add_nf.hlsl` | `mc_effect_zfeather_*` (mp_bog) | the same without fog, alpha also times the depth feather | VERIFIED (bytecode) |
| `lm_b0c0d0n0_sm3.hlsl` | `wc_l_sm_b0c0d0n0` (mp_bog) | lit colour times `tex.a * vertex.a`, written with that alpha | VERIFIED (bytecode) |
| `lm_a0c0_sm3.hlsl` | `wc_l_sm_a0c0` (mp_bloc) | colour scaled by vertex alpha only, alpha 0 | VERIFIED (bytecode) |

The other `b0` sets' shaders, stubs in the map zones, are INFERRED to
match `lm_b0c0d0n0_sm3`.

vcod's stage colour is not premultiplied: a premultiplying set's (One, d)
draws as (SrcAlpha, d). (InvDestColor, One), where no factor can carry
the alpha, draws through `fs_stage_premul`
(`StageVariant::AdditiveInvDst`), which multiplies it in. The add shaders
lerp to `fogColor` before the alpha; vcod fogs an additive stage to
black, the same where the fog colour is black.

## 9. Fly mode: the map as vcod's world

`vcod mp_crash --game cod4` loads the zone (`cod4_fly`), converts it with
`vcod_cod4::mapworld::to_mapworld` into the neutral `MapWorld` and draws
it through the CoD2 world path with the materials `vcod_cod4::materials`
compiles. `mapworld_census` converts all 21 stock maps and range-checks
every index the renderer and the cell vis follow. VERIFIED (census).

### 9.1 Soups, cells and the cell lookup

- Soup `k` is `sortedSurfIndex[k]`'s surface, so a tree's or cull group's
  range is a soup range. No surface is reached twice by the leaf ranges
  and cull groups, and every surface is reached by one of them or by a
  brush model. VERIFIED (all 21).
- A surface's `vertexCount` reaches far past the vertices its triangles
  use: mp_crash's surfaces sum to 36176362 against 172936 used. VERIFIED.
  Each soup copies the span its indices use and rebases them.
- An inner tree's range covers its children's (mp_crash cell 0's root
  holds 0+2283 over nine children). VERIFIED. Only a leaf's surfaces and
  static models are added. INFERRED (`R_AddAabbTreeSurfacesInFrustum_r`).
  vcod lays each cell's trees out in preorder and gives inner nodes no
  soups.
- A portal's `DpvsPlane` is `coeffs . (p, 1)`, positive out of the owning
  cell: the owning cell's centre lies behind it on 158 of mp_crash's 180
  portals. VERIFIED.
- `R_CellForPoint` walks `dpvsPlanes.nodes` as u16 entries from the
  first: an entry no larger than `cellCount` is a leaf naming cell `entry
  - 1` (0 for none); a larger one tests plane `entry - cellCount - 1` and
  steps 2 entries on in front (`d > 0`), `node[1]` entries on behind or on
  it. INFERRED. The walk puts 22 of mp_crash's 23 cell centres in their
  own cell and 5057 of its 5558 surface centres in the cell whose tree
  lists them (348 in none). VERIFIED. vcod turns the walk into a tree of
  nodes and leafs; the census checks the two agree on every fifth surface
  centre of every map. VERIFIED.
- Static models draw as soups: each placement's first LOD turned into the
  world (`origin + scale * (x forward + y left + z up)`, the placement's
  axis rows), one cull group a placement, listed by the cells whose trees
  list the model. That `XSurface` vertices are in model space is INFERRED
  (a placement carries no bones). LOD 0 is `numsurfs` surfaces from
  `surfIndex`. INFERRED (`XModelLodInfo`).
- Packed texture coords are two 16-bit floats, u in the high half: sign,
  5-bit exponent and 10-bit mantissa go into an f32's with the exponent
  plus 112; only an all-zero half reads 0 (no denormal or infinity case).
  VERIFIED (iw3mp.exe 0x5648e0).
- `skySurfIndices` names lit surfaces (mp_crash: corrugated metal,
  planks), not the sky's. VERIFIED. The sky is the `wc_sky` material's
  surfaces.

### 9.2 Lightmaps

The pixel shader of `wc_l_sm_r0c0`'s technique 7, `lm_r0c0_sm3.hlsl` in
mp_crash's zone (its constant table names the samplers;
`tools/re/d3d9dis.py`, `--example techset_shaders`). VERIFIED. With `(u,
v)` the vertex's lightmap coords:

- `A = secondary(u, v / 2)`, `B = secondary(u, v / 2 + 0.5)`: the merged
  secondary's two halves;
- `x = 4.08 A.a - 2.08`, `y = 4.0645161 B.a - 2.0645161`;
- light `= A.rgb + B.rgb * saturate(1 / sqrt(1 + x^2 + y^2))`;
- colour `= colorMap * vertex rgb * light`, then the fog lerp; alpha 1.

Technique 8 (`lm_sun_r0c0_sm3`) adds `primary(u, v).r *
saturate(dot(sunPosition, normalize(normal))) * sunDiffuse`. VERIFIED
(same zone). `B`'s weight is the z of the normalized `(x, y, 1)`: the
secondary holds ambient light plus one direction's, the primary the sun's
visibility. INFERRED.

`R_LoadLightmaps` stacks the BSP's 512 x 512 lightmaps into tiles, `wide`
across and `high` down, the secondary's `A` halves over its `B` halves,
the primary at twice the resolution, and rescales each surface's lightmap
coords into its tile. INFERRED. On mp_crash no surface has triangles in
two tiles, and 2 of its 115526 triangles touch a tile edge. VERIFIED.

vcod splits each lightmap into its tiles, one CoD2 lightmap page each:
the tile's `A` and `B` texels as stored in the page's first two
coefficient images, the light above in the third and in the page's RGB
form (the stage pipeline's), the tile's primary as the sun visibility,
and comworld primary light `sunPrimaryLightIndex` as the sun. The lit
pipeline reads them as 9.6 says. That `sunDiffuse` and `sunPosition` are
that light's colour and direction is INFERRED. Its direction is
worldspawn's `sundirection` as a vector (mp_crash: -50 240, (-0.321,
-0.557, 0.766)). VERIFIED. The RGB form clamps the light to 1; the lit
pipeline does not. 1432 of mp_crash's 524288 secondary texel pairs exceed
it, at most 1.876. VERIFIED.

### 9.3 Materials

`vcod_cod4::materials` gives each material one stage: colour map times
vertex colour (and the lightmap on a lit world set) under the section 8
state (blend, alpha test, cull, depth write and test, polygon offset). A
lit world set that replaces, alpha tests or blends draws through the CoD2
lit pipeline (9.2). (One, InvSrcAlpha) draws as (SrcAlpha, InvSrcAlpha):
the `b0` shaders premultiply (8.1). The multiply decals draw as CoD2's
`mul.hlsl`, whose name the technique shares. INFERRED. `wc_sky` draws as
CoD2's cube sky: `sky.hlsl` samples `skyMapSampler`, a cube, along the
vertex's world position as a direction. VERIFIED (mp_crash's bytecode).

### 9.4 The doorway lighteners (`distfalloff`)

mp_crash's `wc/hdrportal_lighten` (`wc_unlit_distfalloff`, sort key 43,
colour map `$white`) covers the doorways and windows between its
buildings and the outside: 30 surfaces, each listing the openings of one
wall (surface 5541 is the door at x 506..566, y 1112, z 137..239 of the
building that is cell 9). VERIFIED (`map_spots`).

- Lit technique 7 is `vertcol_simple_dfalloff_fog`: vertex shader
  `vertcol_simple_fog_df.hlsl`, pixel shader `vertcol_simple_fog.hlsl`.
  VERIFIED (mp_crash's zone, `--example techset_shaders mp_crash
  wc_unlit_distfalloff`, `tools/re/d3d9dis.py`).
- The vertex colour is the vertex colour times `lerp(falloffEndColor,
  falloffBeginColor, saturate(falloffParms.x * d + falloffParms.y))`, `d`
  the xy distance from the eye (`inverseWorldViewMatrix`'s translation,
  c28.w and c29.w) to the vertex's texture coords; the alpha is the vertex
  alpha times the same weight. The pixel shader multiplies the colour map
  by it and lerps to the fog colour. VERIFIED (bytecode).
- A triangle's three vertices carry one texture coordinate, an xy point
  inside the surface's bounds (surface 5538's face at y 1930..1974 reads y
  1952). VERIFIED (`map_spots`, all 30 surfaces). That the point is the
  centre of the opening it covers is INFERRED from those values.
- Constants: `falloffBeginColor` (0.811765, 0.811765, 0.803922, 1),
  `falloffEndColor` (0.498039, 0.498039, 0.498039, 0.5), `falloffParms`
  (0.005882353, -0.1764706, ...); name hashes (section 4) 0x3D05A1F2,
  0x6B1DA6FA, 0xBDDE5CF5. VERIFIED.
- Drawn (DestColor, SrcColor) (section 8), the framebuffer comes out times
  twice the colour: about 1 (0.996) within 30 units of an opening, rising
  to 1.62 from 200 units on, so the outside seen through a doorway from
  deep inside is brightened. The weight's direction is VERIFIED (the
  constants). The intent, an eye adapted to the dark, is INFERRED.

vcod draws such a material (`Material::dist_falloff`) through the stage
pipeline's `Multiply2x` variant, (Dst, Src) on the colour with the
destination alpha kept, the falloff in `StageParams::falloff` applied in
`shader.wgsl`'s `vs_stage`. Trap: drawn as the alpha blend, the white
colour map at alpha 1 paints every doorway white.

### 9.5 The material census

`renderer::cod4_census` (client tests, `COD4_DIR`) takes every world and
static model surface of the 21 maps and compares what its draw
technique's state bits (section 8) and set name (section 4) say retail
draws with what vcod's pipelines draw for its compiled material: blend,
premultiply (8.1), depth write, alpha test, cull, polygon offset, the
vertex shader's colour (distance or angle falloff, eye offset, feather),
fog and lighting, and a lit set's normal, specular, probe and detail
inputs (9.6). 100 combinations over 285304 surfaces; a differing
combination fails unless the allow list names its set and reason.
VERIFIED (census). It also holds vcod's sort to the blended draws'
`sortKey` order (9 to 48 on these maps). VERIFIED (census). What it made
vcod change:

- Light shafts and hotspots, `wc_unlit_falloff_add` (139 surfaces) and the
  static models' `mc_effect_*falloff_add*` (323):
  `vertcol_simple_fog_foa.hlsl` (mp_crash) turns the normal into the
  world, takes `c = dot(normalize(vertex - eye), normal)` and `w =
  saturate(falloffParms.z * c^2 + falloffParms.w)`, and writes the vertex
  colour times `w * lerp(falloffEndColor, falloffBeginColor, w)`, vertex
  alpha unchanged. VERIFIED (bytecode); `*_foa_dtex` and
  `zfeather_foa_nf_eo_dtex` read the same. A sheet seen edge on fades out:
  `wc/ch_godray01`'s `falloffParms.zw` are (2.128, -0.064), so `w` is 0
  below `c^2 = 0.03`. Most draw (InvDestColor, One), 8.1's screen blend;
  `wc/ch_hotspot02` and kin (One, One). Without the falloff they draw at
  full strength as (SrcAlpha, One).
- The add sets' (One, One) draws as (SrcAlpha, One) (8.1): `wc_unlit_add`
  (364 surfaces, the cargo container numbers and lit windows),
  `mc_unlit_add`, the effect sets.
- The `eyeoffset` effect sets (194 static model surfaces, floodlight
  flares and beams): `zfeather_foa_nf_eo_dtex.hlsl` moves the clip
  position's z and w `eyeOffsetParms.x` nearer (15 on
  `mc/gfx_floodlight_beam_godray75`, 20 on the lens flare, 12 on
  `godray80`) and scales x and y by `(w - e) / w`, so the vertex keeps its
  pixel and draws in front of the lamp it sits in. VERIFIED (bytecode,
  constants). vcod moves the vertex `e` nearer along its view ray.
- `wc_distortion_scale_zfeather` (mp_overgrown's `wc/office_brickglass`,
  20 surfaces). Techniques 5 and 6 (`distortion_scale_zfeather.hlsl`)
  sample the colour map at the screen position bent by the normal map,
  under an opaque state; technique 4, which `draw_state` reads, is a plain
  `vertcol_simple`. VERIFIED (bytecode). That the emissive technique (5)
  is the one drawn, its colour map the resolved frame, is INFERRED
  (`TECHNIQUE_EMISSIVE`). Technique 5's vertex shader is the effects'
  (cod4-effects.md 17) with the tangent read off the vertex (`v4`, decoded
  by `(1/127, -1)`), the binormal `tangent x normal` (`v3`), and the clip
  z passed on for the depth test. Its pixel shader writes the copy at the
  bent lookup where the copy's `floatZ` there is not nearer than that z,
  else at the plain one, times the vertex blue. VERIFIED (bytecode,
  mp_overgrown's zone, `--example techset_shaders mp_overgrown
  wc_distortion_scale_zfeather`). vcod: `renderer::cod4_worldfx` draws the
  stage past the frame's split through `vs_world_distort` and
  `fs_world_distort` (`shader.wgsl`) over `crate::scene_copy` and the soft
  particles' depth, the map's tangent stream in vertex buffer 1; under
  `r_distortion` 0 it draws none, as retail skips a technique that needs
  the copy. `cod4_live_overgrown_glass_bends_the_scene` joins
  vcod-server's war on mp_overgrown, faces a pane and checks the glass
  bends the frame against `r_distortion` 0. VERIFIED (the test, ours).
- The `zfeather` effect sets on world surfaces and static models (287
  surfaces: the floodlight beams, lens flares and god rays of
  `mc_effect_zfeather_falloff_add_nofog*`): `zfeather_foa_nf_eo_dtex.hlsl`
  and `zfeather_add_nf.hlsl` (mp_crash) fold `saturate(w *
  featherParms.x)` into the vertex alpha and multiply colour and alpha by
  `saturate(|abs(floatZ) * featherParms.x - w * featherParms.x|)`.
  VERIFIED (bytecode). vcod: `cod4_worldfx` draws a blended stage of such
  a set past the split through `fs_stage_feather` (the alpha), `_rgb` (the
  colour too, for (One, One) and colour multiplies) or
  `fs_stage_premul_feather`, reading the soft particles' depth; the near
  fade runs per pixel on the view depth, not per vertex. A stage sorted
  before the split, or under `r_zFeather` 0, draws plain.
  `cod4_live_crash_floodlight_beams_feather` faces a mp_crash lens flare
  on the live path and checks it fades against `r_zFeather` 0. VERIFIED
  (the test, ours).
- The `nofog` sets keep `Material::nofog`.

The allow list (`ALLOWED`), what vcod still draws unlike retail:

- `wc_ambient_t0c0` (119): `vertcol_shaded_fog.hlsl` lerps the colour to
  `materialColor.rgb` by `materialColor.a`. VERIFIED (bytecode, mp_creek).
  vcod draws the colour map times the vertex colour.
- Material stubs (`,mc/lambert1`, 55 static model surfaces on 7 maps):
  the material lives in another zone, which the map load does not read.

`cod4_weapon_and_effect_materials_draw_as_retail` allows no weapon or
entity set past those (`ENTITY_ALLOWED` is empty: the entity passes bind
the probes, 9.7); its effect list allows the fx pass's (InvDestColor,
One) drawn as a plain add (cod4-effects.md 9).

### 9.6 Normal, specular and detail maps

The pixel shader of `wc_l_sm_r0c0n0s0`'s technique 8,
`lm_sun_r0c0n0s0_sm3.hlsl` in mp_crash's zone; its vertex shader
`lm_sun_s_tc0n0_sm3.hlsl` passes the normal, tangent, binormal
(`cross(normal, tangent)` times the vertex's sign) and eye-relative
position. VERIFIED (bytecode, `--example techset_shaders`,
`tools/re/d3d9dis.py`). With `A`, `B`, `x`, `y` as in 9.2:

- The normal map's alpha and green decode as the direction does: `nx =
  4.08 n.a - 2.08`, `ny = 4.0645161 n.g - 2.0645161`.
- Light `= A.rgb / |(nx, ny, 1)| + B.rgb * saturate((x nx + y ny + 1) /
  (|(x, y, 1)| |(nx, ny, 1)|))`: `B` by the cosine between the lightmap's
  direction and the normal, `A` by the normal's z. A flat normal gives
  9.2's light.
- World normal `normalize(N + nx T + ny B)`; sun diffuse `primary.r *
  saturate(dot(sunPosition, n)) * sunDiffuse`.
- Specular, `v` the normalized eye-to-surface vector and `r = v - 2 (v .
  n) n`: `primary.r * saturate(exp((dot(r, sunPosition) - 0.99925) *
  (exp2(9.3775177 spec.a) + 7))) * envMapParms.w * sunSpecular`, plus the
  reflection probe's cube at `r`, mip `6 - 8 spec.a`, times its alpha; the
  sum times `spec.rgb` and the fresnel `lerp(envMapParms.x, envMapParms.y,
  (1 - |v . n|)^envMapParms.z)`.
- Colour `= colorMap * vertex rgb * light + specular`, then the fog.

`lm_sun_r0c0d0n0s0_sm3.hlsl` (`wc_l_sm_r0c0d0n0s0`) adds the detail map
first: `(detailMap(uv * detailScale) + colorMap - 0.5) * vertex`, as
CoD2's `_dtl`. VERIFIED (bytecode). A set's last name part says which maps
it reads: `n0` normal, `s0` specular (and the probe), `d0` detail
(`vcod_cod4::materials::lit_inputs`). INFERRED (the sets' shader names).

`R_SetSunConstants` stores `sunDiffuse` as the sun's colour and
`sunSpecular` as it times `r_specularColorScale`. INFERRED.
`r_specularColorScale` (name at 0x705180) registers 1: the 1.0 stored at
0x62a89e ahead of the call at 0x62a8f6. INFERRED (instruction order).

vcod: every lit world set's `PhongLit` carries `Cod4Lit` (normal map,
`envMapParms`) and the specular and detail switches; `cod2_world.wgsl`'s
`cod4_lit` runs the arithmetic above, the probe as 9.7 binds it. Gate:
`cod4_normal_and_specular_mapped_surface_matches_the_hlsl`. The census
(9.5) checks each lit set's inputs.

### 9.7 Reflection probes

- `GfxReflectionProbe` is 0x10: origin, then the cube image at 0xC.
  INFERRED. The walk reads the image there on every stock map. VERIFIED.
- Every probe image is a 64 x 64 A8R8G8B8 cube of seven levels a face in
  the map's zone (section 5). Every world surface's and static model's
  `reflectionProbeIndex` is below the map's probe count (mp_crash 19
  probes, mp_cargoship 47, mp_creek 8). VERIFIED (`mapworld_census`).
- The `s0` pixel shaders read `reflectionProbeSampler` (s1, `dcl_cube`)
  with `texldl` at the reflected view `r`, level `6 - 8 spec.a`, and add
  its colour times its alpha to the sun's specular before the specular map
  and fresnel multiply (9.6, 3.2). VERIFIED (bytecode of
  `lm_sun_r0c0n0s0_sm3`, `lp_i_sun_r0c0d0n0s0_sm3`). A draw binds its
  surface's or static model's probe. INFERRED (the field names).
- A scene model's probe (`R_CalcReflectionProbeIndex`, 0x6077c0): the cell
  its lighting origin is in (`R_CellForPoint`, 0x607140), then the nearest
  of that cell's probes by squared distance, the first on a tie (`bestDist
  > dist`, from `FLT_MAX` at 0x70af58; 0x607490, probes at `GfxWorld` +
  0xe8 stepped by 0x10). Outside every cell, the nearest of all probes but
  probe 0 (0x607710, from index 1 below `GfxWorld` + 0xe4).
  `R_AllocModelLighting` calls it as it takes a block (0x62ee03) and keeps
  the index at `GfxLightingInfo` + 1 (0x62ee11). VERIFIED.
- The static models' load (0x63ff40) runs the same two functions at the
  model's bounds' centre and stores the index at `GfxStaticModelDrawInst`
  + 0x44. VERIFIED. On all 21 maps every stock static model's stored probe
  is the one `ProbeFinder::at` picks at its lighting origin. VERIFIED
  (`lightgrid_census`).

vcod:

- World: `MapWorld::reflection_probes` holds the cubes with their levels,
  `soup_probes` each soup's probe. World batches split by probe as well as
  material and lightmap (`mesh::build_batches`; every soup probe is 0 off
  CoD4), and a specular set's group 2 binds its batch's cube.
  `cod2_world.wgsl`'s `cod4_specular` samples it through `r_textureMode`'s
  clamped sampler; the sampler state retail sets on the probe is not read.
  Gate: `cod4_model_shades_its_volume_block_as_the_hlsl_does` (the probe's
  face along `r`).
- Entities and the view model: `vcod_cod4::map::ProbeFinder` picks the
  probe at the lighting origin, `entity_light::pack_cod4` writes its index
  into the light set, and `cod2_model.wgsl`'s `cod4_model` samples it out
  of every probe bound as one cube array (`EntityMaterials::set_probes`,
  group 3 binding 5). Gate:
  `cod4_entity_set_lights_a_normal_mapped_sphere_as_the_hlsl_does` (a
  two-layer array, layer 1's face along `r`).
- The array takes six layers a probe; wgpu's default limit is 256 layers,
  42 probes. Trap: mp_cargoship has more, so on the default limit the
  array failed to build and every entity bind group with it (1974
  validation errors in a `war` level of the map sweep,
  `cod4-map-sweep.md`). VERIFIED (the sweep, lavapipe). Both devices ask
  for the adapter's own `max_texture_array_layers`, and a probe past the
  device's limit is left out. VERIFIED
  (`entity_material::tests::probes_past_the_layer_limit_leave_a_valid_array`).

### 9.8 Water: `water_l_sun`

- `wc_water`'s lit techniques (7, 8, 14, 15) are all `water_l_sun.hlsl`
  (mp_backlot). The vertex shader passes the texture coords, eye-relative
  position and fog; the pixel shader reads `normalMapSampler`,
  `reflectionProbeSampler` (a cube), `waterColor`, `envMapParms`,
  `sunPosition` and `sunDiffuse`, no colour map and no lightmap. VERIFIED
  (bytecode, `--example techset_shaders mp_backlot wc_water`,
  `tools/re/d3d9dis.py`).
- With `v` the normalized eye-to-surface vector and `N(uv)` the normal
  map's red. VERIFIED (bytecode):
  - coords move to `uv + (0.5 - N(uv / 2)) v.xy 0.0234375`;
  - height `h(uv)` is three octaves, `N` at `uv`, `3.7 uv`, `13.69 uv`
    weighted 1, 0.6, 0.36 (`defi` 3, `rep`);
  - normal `normalize(h(uv + (1/256, 0)) - h(uv), h(uv + (0, 1/256)) -
    h(uv), 1)`; `r = v - 2 (v . n) n`;
  - reflection: the probe at `(r.x, r.y, |r.z|)` times its alpha times 4,
    saturated; `f = saturate(lerp(envMapParms.x, envMapParms.y, (1 - |v .
    n|)^envMapParms.z))`;
  - colour `lerp(waterColor n.z, reflection, f) + (f max(r . sunPosition +
    0.00075, 0))^64 sunDiffuse envMapParms.w`, then fog; alpha the vertex
    alpha.
- No time constant reaches the shaders: the water moves through its normal
  map, the runtime `watersetup` image (section 5), rebuilt from the
  material's `water_t`. INFERRED (`R_SetPixelSamplerFromMaterial` uploads
  a water map's texture before binding it).
- The zone stores `water_t`'s spectrum, `H0` (M x N complex) and `wTerm` (M
  x N), inline; all 21 maps' water materials carry it, square and a power
  of two. VERIFIED (`mapworld_census`). Every stock water material carries
  `waterColor` and `envMapParms` (mp_backlot's `wc/day_water_mud`: (0.4,
  0.35, 0.3, 1) and (0, 0.45, 4, 2.5)). VERIFIED (`cod4_census`).
- 1.7's frequencies at time `t` (0x616240): `t` times the double
  162.9746551513672 (0x70b208) stored as a float; each texel with a
  non-zero `wTerm` at phase `(int)(wTerm * that)` under the truncating
  control word, `& 0x3ff`; its `H0` real part times the sine table at `(p
  + 255) & 0x3ff`, its imaginary part at `p`. VERIFIED (0x616240-0x6162dc).
- Then an FFT of each row and, transposed, each column (0x616370, `_FFT`
  at 0x56f2f0, tables at 0xd4f4100 and 0xd4f3900). VERIFIED (the calls).
  That `_FFT` is CoD2's radix-4-first transform is INFERRED.
- The pixels (0x616420): the magnitude, stored, times the stored `1 / (M
  N)`, 1 past 1, times 255.99899 (0x70b5c8, a double), truncated.
  VERIFIED (0x616420-0x61655c). The mips box-filter as CoD2's. INFERRED
  (the mip build). The time is the scene's `floatTime` in seconds.
  INFERRED.

vcod: `Cod4Light::Water` draws `wc_water` through `cod2_world.wgsl`'s
`fs_cod4_water` with its batch's probe (9.7); `vcod_cod2::water::
WaterSetup::from_spectrum` and `levels_cod4` rebuild each water material's
L8 height map at the frame's time, bound as its normal map. Gates:
`cod4_water_shades_as_water_l_sun` (lavapipe, against the HLSL),
`a_cod4_spectrum_of_one_wave_is_flat`.

### 9.9 The cell walk

- `R_AddWorldSurfacesPortalWalk` is 0x60afd0. It clears the cell bits and
  returns at once under `r_skipPvs` (dvar 0xd56977c, name 0x705f0c). For
  an eye in no cell it adds every cell's surfaces and cull groups against
  the view frustum; in a cell under `r_singleCell` (dvar 0xd5695e0, name
  0x706098, read at 0x60b042) it adds that cell alone; otherwise it calls
  the portal walk (0x60a2f0) and returns. INFERRED (control flow,
  0x60aff6-0x60b094). No pass after the walk marks a cell the portals did
  not reach. INFERRED (the same).
- vcod's cell walk (`vcod_common::vis`) adds two passes after its portal
  walk for CoD1's mp_ship decks: cells under the eye are frustum-tested
  whole, and so is every cell sharing a portal with a visited one. A CoD4
  world sets `MapWorld::portals_only` and skips both. At the war spectator
  spot on mp_crossfire (the intermission, 3804 -988 196, yaw 325) the walk
  then reaches 18 cells instead of 30, and the live client draws 263289
  triangles instead of 427247. VERIFIED (vcod's walk on the zone;
  `tools/cod4_perf.sh`).
- The portal walk queues portals; it does not recurse per path. A cell's
  portals still in view are chopped to its planes and their points added
  to the portal's hull. A portal not yet queued goes on a min-heap keyed by
  its winding's furthest point along the view plane; one already queued
  only gathers the points (and loses its single parent). Each pop opens
  the portal once, through the convex hull of everything gathered, and
  visits its cell. An eye within 1/8 unit of a portal's plane and inside
  its winding enters it unnarrowed. The heap holds 256 (`More than %i
  queued portals`, 0x70031c, beside `r_portalWalkLimit` 0x706100 and
  `r_portalMinRecurseDepth` 0x7061dc). INFERRED (`R_VisitPortals`,
  `R_VisitPortalsForCell`, `R_EnqueuePortal`). The three strings are
  VERIFIED in iw3mp.exe.
- A CoD4 world walks the queue (`WorldVis::walk_queued`). Trap: a
  recursive walk that opens a portal once per path and stops at 8 visits a
  cell spends its budget in mp_overgrown's sky cell (cell 0, 80 portals).
  From the ground floor of the two-storey house at -718.5 -3784.5 -48
  (cell 24), yaw 225, pitch 25, it spent 241 visits before the stair
  hole's portal to cell 21 and drew the upper floor's walls as sky; 103 of
  the views a ring round every spawn looks along hit the cap. The queue's
  walk visits 6 cells from that view and reaches 21. Across 2748 eyes the
  views where a ray's first front face is a soup the walk dropped fell
  from 238 to 235, none new; the rest are sight lines through hedges and
  fences no portal covers. VERIFIED (`vis_rays`,
  `overgrown_stair_hole_shows_the_upper_floor`).
- `R_PortalClipPlanes` is 0x606680. A winding of more than 10 points gets
  the bevel planes alone (`cmp $0xa` at 0x606697). VERIFIED. After the
  side planes it copies the near plane (`dpvsGlob.nearPlane`, read at
  0x606be3), takes the winding's nearest point along it (0x605af0,
  `R_NearestPointOnWinding` by its name) and, when that is past 0, pulls
  the plane's distance in by it (0x606c21-0x606c38), then appends the far
  plane when one is set (0xd065394, 0x606c3f). VERIFIED (the stores). So a
  cell entered through a portal culls what lies nearer than the opening.
- vcod's `walk_queued` keeps the camera's near plane. Moved as 1.7 moves
  it, the drawn triangles across mp_crossfire's 5880 spawn views
  (`cull_census`) fell 0.5% (34456 to 34299 mean, 110820 to 110451 over
  the heaviest 5%), and the `vis_rays` ring sweep on mp_overgrown (2748
  eyes) gained 15 missed rays in 2 views: wall and decal surfaces only the
  sky cell (cell 0) lists, below its floor portal at z -64 in cell 1, from
  -568.8 734.8 -116 yaw 45 pitch -20 (soups 2573, 2583, 2595) and -815.9
  444.8 -112 yaw 225 pitch -20. Through portal 99 the sky cell's hull is a
  sliver 600 units out, so its near plane moves past them. VERIFIED
  (vcod's walk on the zone). Whether 1.7 drops them too is pending a
  retail screenshot at those spots; until then vcod keeps them.
- The small-portal rule is CoD2's (cod2-client-perf.md 9).
  `r_portalMinClipArea` registers at 0x62b7f6 (name 0x706174, float,
  default 0.02 from 0x70b880, flags 0, dvar at 0xd569778), help text
  "Don't clip child portals by a parent portal smaller than this fraction
  of the screen area." (0x706118). VERIFIED (disassembly, rodata).
- 0x605fd0 projects the hull through the view-projection: a point under
  0.125 of clip w (0x700be8) leaves the children clipped; otherwise the NDC
  rectangle times 0.25 (0x70b058, 0x606143) under the dvar unclips them,
  and past it the shoelace sum times 0.125 (0x70b038, 0x606226) is held
  against it again. VERIFIED (the compares at 0x606151 and 0x606234, the
  stores of 0 and 1). `R_PortalClipPlanes` (0x606680) calls it whenever
  the dvar is over 0 (0x6066f0..0x606722). VERIFIED.
- The walk (0x60a2f0) then forces the flag on for a portal whose stored
  depth (byte +2) is under `r_portalMinRecurseDepth` (0x60a6ca..0x60a6e0;
  int, default 2, 0 to 100, registered at 0x62b83c, name 0x7061dc, dvar at
  0xd55b0e4) and hands it to the cell visit (0x609f50) with that depth
  plus one. A visit with the flag off lists the cells beyond (0x609ea0)
  instead of queueing portals (0x609fcb). VERIFIED (disassembly). A portal
  keeps the depth of the visit that first queued it, lowered to a later
  visit's when that is shallower (0x60a289, 0x60a2aa); the walk starts at
  depth 0 (the push at 0x60a33c). VERIFIED.
- vcod's queued walk takes both (`MapWorld::portal_min_clip_area`,
  `portal_min_recurse_depth`). A portal the eye stands in is entered at
  the visit's depth plus one, where 1.7 adds one to the portal's own
  stored byte. INFERRED (the push at 0x60a0b9). Across the `cull_census`
  spawn views the drawn triangles rise 0.4% on mp_crossfire (34456 to
  34589 mean), 0.6% on mp_crash and 1.5% on mp_overgrown: the cells beyond
  a small portal take the parent's frustum, not their own portal's.
  VERIFIED (vcod's walk on the zones, `CULL_OLD=1` against the default).
- No stock map script sets a cull distance (`setCullDist`, configstring 6,
  `R_SetCullDist`): mp_crash's, mp_creek's, mp_crossfire's and
  mp_overgrown's rawfiles have none. VERIFIED (the zones). The far plane is
  then `r_zfar`, 0 by default, and the walk has none: a view across the
  map draws to the map's edge in 1.7 too. INFERRED (`R_GetFarPlaneDist`,
  `R_SetupDpvsForPoint`).
- The heavy views are long views, not over-marking. mp_crossfire's
  heaviest 5% of spawn views (110k triangles, 2.9k soups, 16 cells against
  a median of 25k) look along the main street from the east end (5232 -888
  240, yaw 225 and 270): 22 to 24 cells, 554 static models at the 9.10
  picks (80k of the 150k triangles), each under its `cullDist` (1000000
  for these). 1.7's rules draw the same set except the near plane.
  INFERRED (`cull_census`, vcod's walk against this section's and 9.10's
  rules).
- With vis off the renderer draws every static model at LOD 0; with it on,
  at the 9.10 picks. The trees that change between the two modes change
  LOD, not visibility. VERIFIED (renderer `cull`; the same view's walk
  misses nothing).
- The white rectangle on mp_overgrown (-1553 -3414 -61, yaw 61) is a
  static model, `com_barrier_tall1` at -1418 -3317.5 -139
  (`mc/mtl_barrier`, `mc_l_sm_r0c0n0s0`; its maps `barrier`,
  `barrier_norm` and the composite spec map all decode). The first ray
  hits it in both vis modes. VERIFIED (`vis_rays` scratch picks). Through
  the live draw path (lavapipe, `cod4_live`'s harness) it is textured and
  lit to about 0.87 of white before the vision set: the light grid gives
  (50, 52, 43) below and up to (71, 74, 66) above, the sun's colour (0.92,
  0.93, 0.72); mp_overgrown's film tweak then clips it to white, with or
  without the specular. VERIFIED (the scratch frames). Whether retail's
  barrier is as bright is pending a retail screenshot at that spot.

### 9.10 Static model LODs and `cullDist`

- `R_UpdateLodParms` is 0x5fadf0. The LOD origin is the view origin (the
  locked one under `r_lockPvs`); the rigid ramp is `scale =
  r_lodScaleRigid * f`, `bias = r_lodBiasRigid * f`, `f` the refdef's
  float at +0x14 (`tanHalfFovY`) times the double 2.118673086166382 at
  0x70b220. VERIFIED (0x5fae2f-0x5fae7f: the `fmul` by 0x70b220, the
  stores to +0xc and +0x10). `r_lodScaleRigid` is the dvar at 0xd569790,
  `r_lodBiasRigid` at 0xd56972c. VERIFIED (their registrations store
  there, 0x62a2ac and 0x62a314). The corrected copy the static model
  passes read is 0xcc9f32c: origin, then rigid scale at 0xcc9f338 and bias
  at 0xcc9f33c. VERIFIED (0x5faeac, 0x63b3da, 0x63b3ea).
- The camera's static model pass (0x63ac70) takes every placement the
  cell walk marked and computes `d = |origin - lodOrigin| * scale + bias`
  (`GfxStaticModelDrawInst` origin +4). VERIFIED (0x63adb2-0x63ae29). A
  placement with `d` at or past its `cullDist` (+0) is unmarked and not
  drawn. INFERRED (compare and jump at 0x63ae31-0x63ae3d). Under it, `d /
  scale` (the placement's scale, +0x34) picks the first of the model's
  `numLods` (`XModel` +0xc4) whose `lodInfo[].dist` (from +0x28, 0x1c
  apart) is greater; with none the placement is unmarked. INFERRED
  (0x63aed8-0x63af35). This pass has no test for a distance of 0, unlike
  `XModelGetLodForDist` (0x5911c0), which the entity path calls
  (0x604ebd). INFERRED (the same).
- vcod: each placement's every LOD is a run of soups inside its cull group
  (`MapWorld::static_lods`); the renderer's cull keeps the run
  `StaticLod::pick` names at `mapworld::cod4_lod_ramp`'s ramp. The
  `vc_shadows` casters and an unculled view (F4 off) draw LOD 0 alone; the
  sun shadow casters take the camera's picks (3.6). At the war spot on
  mp_crossfire the live client draws 133671 triangles instead of 263289.
  VERIFIED (`tools/cod4_perf.sh`). The sun shadow pass's own loop
  (0x63b3d0) culls and picks per partition the same way; vcod's cached
  casters do not cull per partition. INFERRED (its compare at 0x63b4fe).

## 10. Open

- The vertex layer data.
- Entities marking their primary light used for the spot history (3.5);
  the view model's sun shadow map (3.6).
- The census's allow list (9.5): `wc_ambient_t0c0`'s tint, material stubs.
  `wc_l_sm_a0c0` (12 surfaces) draws (One, One) without its vertex alpha
  (8.1).
- Water: the sampler state retail sets on the height map and the probe
  (9.8). Fog and vision sets: section 11.
- Collision: nothing open here (cod4-collision.md).

## 11. Fog and vision sets

What a level draws over its surfaces: the exp fog of configstring 9 and
the film tweak and glow of the vision set configstring 824 names
(`vcod_cod4::look`, the client's `cod4_look`). How `setExpFog` and
`visionSetNaked` write the two slots: `cod4-gsc.md`.

### 11.1 Fog

- Configstring 9 reads `<start> <density> <r> <g> <b> <ms>`, the density
  `ln 2 / halfwayDist`: the prematch capture's `500 0.000198042 0.501961
  0.501961 0.45098 0` for mp_crash's `setExpFog(500, 3500, ...)`.
  VERIFIED (capture; `cod4-gsc.md`).
- The cgame turns the colour into bytes (`v * 255` rounded to nearest)
  and reads fog off when no second token follows the first, the first
  then being the fade out in ms. INFERRED (`CG_ParseFog`).
- `R_SetFrameFog` (0x63db00) stores `fogColor` from the colour bytes and
  `fogConsts` (0, 1, -density, start * density), the start at +0xc, the
  density at +0x10. VERIFIED (0x63db2f-0x63dbdf: the `fldz`, `fld1`,
  `fchs` and `fmul` stores to code constants 0x2a and 0x29). The other
  arm, taken when `r_fog` (0xd569848) reads 0, stores `fogConsts` (0, 1,
  0, 0). INFERRED (the branch at 0x63db09).
- Every fogged world technique's vertex shader computes `fog =
  saturate(exp2((d * fogConsts.z + fogConsts.w) * log2 e))`, `d` the
  eye-relative position's length; the pixel shader writes `lerp(fogColor,
  colour, fog)`. The colour keeps `exp(-density * (d - start))` of itself
  past `start`, all of it nearer, half at `start + halfwayDist` (mp_crash:
  4000 units). VERIFIED (`wc_l_sm_r0c0`'s `lm_tc0_sm3.hlsl` and
  `lm_r0c0_sm3.hlsl` bytecode, `--example techset_shaders`,
  `tools/re/d3d9dis.py`).
- The sky is never fogged: `wc_sky`'s `sky.hlsl` reads no `fogConsts`.
  VERIFIED (bytecode). A `nofog` set's shaders read none either (9.5).
  VERIFIED.
- A new fog fades from the drawn one over its time, lerping start,
  density and colour bytes on the cgame's clock; the first fog, or one
  while the drawn density is 0, snaps. INFERRED (`R_SwitchFog`,
  `R_UpdateFrameFog`). vcod fades the colour as floats.
- `r_fog` defaults to 1. INFERRED (its registration).
- vcod: the start rides `FogParams::start` into the camera's
  `fog_range.w`; every shader's exp branch keeps `exp(-density * max(d -
  start, 0))`, radial on CoD4 as on CoD2 (the lit world and static model
  sets per vertex as retail's, `cod2_world.wgsl`'s `VsOut::fog`,
  cod4-client-perf.md 26; entities and the rest per pixel). A session
  fades to configstring 9 as the HUD sees it change; fly mode takes the
  art script's call (`maps/createart/<map>_art.gsc`, else
  `maps/mp/<map>.gsc`). Gates: `carentan_fog_keeps_82_percent_at_2000_units`
  (mp_crash's fog keeps 74.3% at 2000) and
  `the_prematch_capture_names_the_fog_and_both_sets`.
- Census (`tests/look_census.rs`): 19 of the 21 stock maps' scripts set a
  fog; mp_cargoship's call is commented out and mp_killhouse has none.
  mp_strike spells its colour as `233/255`. VERIFIED (the zones).

### 11.2 Vision sets

- The vision files are rawfiles `vision/<name>.vision`: 97 in
  `common_mp`, among them `default`, `mpintro` and the first sixteen
  maps'; mp_broadcast, mp_carentan, mp_crash_snow, mp_creek and
  mp_killhouse carry theirs in their own zone. VERIFIED (the zones).
- `visionDefFields` at 0x727610: sixteen (name, offset, type) rows.
  VERIFIED (the table, `tools/re/cod4_vision_fields.py`):

  | Offset | Field | Type |
  |---|---|---|
  | 0 | `r_glow` | bool |
  | 4 | `r_glowBloomCutoff` | float |
  | 8 | `r_glowBloomDesaturation` | float |
  | 12, 16 | `r_glowBloomIntensity0`, `1` | float |
  | 20, 24 | `r_glowRadius0`, `1` | float |
  | 28, 32 | `r_glowSkyBleedIntensity0`, `1` | float |
  | 36 | `r_filmEnable` | bool |
  | 40, 44, 48 | `r_filmBrightness`, `r_filmContrast`, `r_filmDesaturation` | float |
  | 52 | `r_filmInvert` | bool |
  | 56, 68 | `r_filmLightTint`, `r_filmDarkTint` | vec3 |

- A file is one key and a quoted value a line, keys case-blind and each
  read once; a bool reads `%i`, a float `%f`, a vec3 `%f %f %f`; an
  unknown key (`cg_fov`) is skipped. A missing file reads `default`'s.
  INFERRED (the vision file parse, the raw file read). mp_bloc,
  mp_cargoship, mp_shipment and mp_vacant leave out four glow keys.
  VERIFIED (census).
- Configstring 824 reads `"<name>" <ms>`: `"mpintro" 0` at the join,
  `"mp_crash" 2000` when the prematch ends at level time 19850. VERIFIED
  (capture). The cgame eases to a set named with a time by `sin(frac * pi
  / 2)` on every number, bools taking the new set's at once; the first
  set, or one with no time, snaps; until a set is named it draws
  `default`. INFERRED (`CG_VisionSetStartLerp_To`, `LerpFloat`'s
  `VISIONSETLERP_TO_SMOOTH`).
- Fly mode draws `vision/<map>.vision`, the set the game type names after
  the prematch.
- Configstring 825 is the night channel (`"default_night" 1000` in the
  handshake capture, `cod4-handshake.md`), blended as 824 is. The cgame
  draws it while `CG_LookingThroughNightVision`: the predicted
  playerstate's `weapFlags` 0x40, or weapon state 25 (wear) or 26 (remove)
  inside its weapon's power-up and power-down times. A channel no
  configstring named turns film and glow off. The refdef's glow takes the
  set's `r_glowBloomIntensity0` and `r_glowRadius0`. INFERRED
  (`CG_VisionSetApplyToRefdef`, `CG_LookingThroughNightVision`). vcod
  reads 825 and draws its set while the drawn playerstate's `weapFlags`
  has 0x40 (`cod4_look::night_vision`); the wear and remove states and
  `VisionFadeValue`'s fade are not read.

### 11.3 The film tweak

- `postfx_color.hlsl` (`code_post_gfx_mp`'s `postfx_color` set): `out = (c
  * colorBias.w + lum) * (colorTintBase + colorTintDelta * lum) +
  colorBias.rgb`, `lum = dot(c, (0.299, 0.587, 0.114))`. VERIFIED
  (bytecode).
- `R_UpdateColorManipulation` (0x5f8e90): `d = max(desaturation,
  1/4096)`, `colorBias = (brightness + 0.5 - contrast / 2, ..., 1/d - 1)`,
  `colorTintBase = darkTint * contrast * d`, `colorTintDelta = (lightTint
  - darkTint) * contrast * d`. VERIFIED (0x5f8ef6-0x5f9062; 0.5 at
  0x70ace8, 1/4096 at 0x70b464). Its other stores are bias (0, 0, 0,
  4095), base 1/4096, delta 0. VERIFIED (0x5f8e9c-0x5f8eec; 4095 at
  0x70b468). The first runs when the film's enable byte is set, the second
  when not; `invert` negates the scale and adds 1 to the bias. INFERRED
  (the branches at 0x5f8e9a and 0x5f8f73).
- The film's fields sit at view info +0x5648 (enable), +0x564c
  brightness, +0x5650 contrast, +0x5654 desaturation, +0x5658 invert,
  +0x565c dark tint, +0x5668 light tint. VERIFIED (same range).
- `R_SetFilmInfo` (0x5f9070) copies the scene's film or reads the
  `r_filmTweak*` dvars (the branch on `r_filmUseTweaks`, 0xd5695d8, at
  0x5f907b; INFERRED), then computes `desaturation *= (1 - desaturation) *
  r_desaturation + desaturation`, `contrast *= r_contrast`, `brightness +=
  r_brightness`. VERIFIED (0x5f9128-0x5f9179). Those three dvars default
  to 1, 1 and 0, so a stock setup draws the set as written. INFERRED
  (instruction order: the values 1, 0 and 1 stored at 0x62c962, 0x62c9bf
  and 0x62ca1c reach the registrations of `r_contrast`, `r_brightness` and
  `r_desaturation`, names at 0x706f04, 0x706f28, 0x706f50).
- mp_crash's film (contrast 1.4, brightness 0.134, desaturation 0.265,
  light tint (1.179, 1.086, 1.024), dark tint (0.7, 1.0215, 1.265)) lifts
  black to -0.066, clamped to 0, and white to the light tint times 1.4
  less 0.066. `mpintro`'s (contrast 1.8, desaturation 1) is the grey
  prematch. VERIFIED (the rawfiles; `look::tests`).
- vcod runs it in the post pass (`post.wgsl`'s `film`) on the display
  value, clamped as the 8-bit target clamps, before vcod's own grade.
  Gate: `post::tests::film_runs_as_retail_postfx_color`.

### 11.4 Glow

- Eight stock maps' sets turn the glow on: mp_bloc, mp_bog, mp_cargoship,
  mp_citystreets, mp_convoy, mp_crash_snow, mp_pipeline, mp_vacant;
  `default` and mp_crash leave it off. Level-0 radii run from 1
  (mp_citystreets) to 25.27 (mp_bog, mp_convoy). VERIFIED (`look_census`,
  which prints each set's cutoff, desaturation, intensity and radius).
- `R_SetGlowInfo` (0x5f9190) keeps the glow at view info +0x5674
  (enable), +0x5678 cutoff, +0x567c desaturation, +0x5680 intensity,
  +0x5684 radius, from the `r_glowTweak*` dvars when `r_glowUseTweaks` is
  set (branch at 0x5f919c), else from the scene. With the intensity not 0
  it stores `glowSetup` (cutoff, `1 / (1 - cutoff)`, 0, desaturation) at
  +0x6630 and `glowApply` (0, 0, 0, intensity) at +0x6640. VERIFIED
  (0x5f921d-0x5f9286).
- `glow_consistent_setup.hlsl` (`code_post_gfx_mp`): four taps at the
  target texel's corners (`uv +- renderTargetSize.zw / 2`), each `c`
  weighted by `saturate(lum(c) - glowSetup.x) * glowSetup.y` times `c`
  through the film tweak's arithmetic (11.3: `colorBias`,
  `colorTintBase`, `colorTintDelta`), averaged, then lerped to its luma by
  `glowSetup.w`; alpha 1. `glow_apply_bloom.hlsl` writes the texel times
  `glowApply.w` under (One, One). The `filter_symmetric_N` sets add N tap
  pairs of `filterTap[i].w * (tex(uv + filterTap[i].xy) + tex(uv -
  filterTap[i].xy))`. VERIFIED (bytecode; the materials' technique 4 state
  bits through `--example techset_shaders code_post_gfx_mp <set>`).
- `RB_GlowFilterImage` (0x651970) scales the radius by the post effect
  target's width over the resolved scene's (target 11 over 4 in the
  render target table at 0xd573eb0, 20 bytes a row, width at +12) and
  calls `RB_ApplyGlowFilter` (0x651860). That scales it by 0.25 again
  (0x70b058), turns it into pixels as `sceneHeight * r / 480` (480.0 at
  0x70ae90), x times the scene pixel aspect (0xcc9d0fc), and lays the
  chain out on the target's own size, the setup its first pass. VERIFIED
  (0x651860-0x651962, 0x651970-0x6519e3).
- The chain (`RB_GenerateGaussianFilterChain`, 0x651310) and its points
  (0x650e70) carry CoD2's constants: 1.3895605 (0x70b6f4), 6.4977503 and
  its square 42.22076 (0x70b6e8, 0x70b6d8, doubles), 0.3295051 (0x70b6f0),
  -0.5 (0x70b0b0), 0.001 (0x70b278). VERIFIED. The rest of the layout
  matches `cod2-materials.md` 9.9. INFERRED (the constants,
  `RB_GaussianFilterPoints1D`).
- The post effect and ping-pong targets are a quarter of the scene. The
  frame runs the film tweak (`postfx_color`, from the resolved scene),
  then the glow from the same resolved scene, then adds it over the film.
  INFERRED (`R_InitRenderTargets`, `RB_ProcessPostEffects`). No sky bleed
  set exists in `code_post_gfx_mp`. VERIFIED (the set list).
- The glow draws while the set enables it and its intensity and radius are
  not 0. INFERRED (`R_UsingGlow`, which also needs `r_glow` and
  `r_glow_allowed`).
- `r_glow` ("Enable glow.", name 0x6ce344) registers at 0x62c669 as a
  bool of `bl`, flags 0x80; `r_glow_allowed` ("Allow glow.", 0x706cec) at
  0x62c5dc of `bl`, flags `ebx`; `r_glow_allowed_script_forced`
  (0x706d34) at 0x62c623 of 0, flags 0x1000. VERIFIED (the pushes ahead of
  each `call 0x56c350`). `ebx` is 1 through that function, so both default
  on and `r_glow_allowed` is archived. INFERRED (as `r_zFeather`'s and
  `r_distortion`'s registrations read, cod4-effects.md 17).

vcod: `cod2_glow::plan_cod4` lays the chain out and `Cod2Glow` runs it
(`fs_setup4` the setup, reading the world as display values clamped to
1); the post pass adds the result at the intensity after the film, the
8-bit framebuffer's clamp before the add. A CoD4 level never runs CoD2's
glow. vcod registers `r_glow` as 1.7 does, on and cheat protected, and
draws the glow only with it and `r_glow_allowed` both on. Gates:
`cod4_plan_scales_the_radius_twice`, `cod4_glow_matches_the_hlsl`
(lavapipe, the chain against the HLSL in bytes),
`post::tests::glow_adds_after_the_film`, the CoD4 `pipeline_check` frame
under mp_bog's glow, and `cod4_live_bloc_vision_set_glows`: it joins
vcod-server's war on mp_bloc (cutoff 0.23), waits for the 824 set to carry
the glow and checks the frame blooms against `r_glow 0` (console, under
`sv_cheats 1`, read by the frame's `cod4_options`): 88065 of 245760 pixels
moved at the spawn's view. VERIFIED (the test, ours). mp_bog's cutoff of
0.93 blooms only a fire's brightest flicker: one run moved 348 pixels, the
next 0 over 395 frames round the spawn. VERIFIED (our runs).

## 12. The draw-surf key and the lit list's order

What order 1.7 draws a frame's lit surfaces in, and how vcod's world
batches follow it (`vcod_cod4::drawsurf`, cod4-client-perf.md 33).

### 12.1 The key

A `GfxDrawSurf` is a 64-bit key. Its fields, low bit first, as the code
that packs them shifts and masks them:

| bits | field | packed at |
|---|---|---|
| 0..15 | `objectId` | 0x61a728 (`mov WORD PTR [ecx],bp`) |
| 16..23 | `reflectionProbeIndex` | 0x61a767 (byte 2) |
| 24..28 | `customIndex` | 0x61a72f (`and al,0x1f`, `shl 0x18`, `and 0xe0ffffff`); 0x6216a1 |
| 29..39 | `materialSortedIndex` | 0x6216cd (`and 0x7ff`, `shld`/`shl 0x1d`, `and 0x1fffffff`, `and 0xffffff00`) |
| 40..41 | prepass | 0x621681 (`and 3`, `shl 8` into the high dword, `and 0xfffffcff`) |
| 42..49 | `primaryLightIndex` | 0x61a4be, 0x61a773 (`shl 0xa` into the high dword, `and 0xfffc03ff`) |
| 50..53 | surface type | 0x61a752 (`and 0xffefffff`, `or 0x2c0000`); 0x5f8556 (`and 0xffdbffff`, `or 0x180000`) |
| 54..59 | `primarySortKey` | 0x621642 (`and dl,0x3f`, `shl 0x16` into the high dword) |

VERIFIED (the disassembly's shifts and masks). The field names are
`GfxDrawSurfFields`'s, INFERRED.

- `Material_Sort`'s numbering pass (0x621600..0x6216f9) clears each
  material's key at `Material`+8 and writes `sortKey` (`Material`+5), the
  prepass key (0x6211d0's return), `gameFlags` 0x40 as `customIndex`, and
  the material's place in the sorted list as `materialSortedIndex`.
  VERIFIED (the stores). The list is sorted first by `std::sort`
  (0x640ba0) with comparator 0x621250 (`push 0x621250` at 0x62160f).
  VERIFIED (the push). That the numbering follows the sort is INFERRED
  (instruction order).
- A world surface's key (0x61a450, filling the array at `GfxWorld`+0x2a0
  by surface index) is its material's key with the surface's
  `primaryLightIndex` (`GfxSurface`+0x16) in bits 42..49. VERIFIED (the
  loads and stores). So a world draw surface carries no probe and no
  lightmap in its key.
- The camera walk (0x63c4d0) reads each visible surface's key out of that
  array and marks the primary light it names in a bit set at 0xcff4280
  (`shr eax,0xa`, `and 0xff` on the high dword, 0x63c575). VERIFIED (the
  arithmetic).
- `R_SortDrawSurfs` (0x61a8d0, ten callers) is a quicksort, a selection
  sort under nine entries (0x61abe0), comparing the 64-bit key unsigned,
  high dword first. VERIFIED (`jb`/`ja` on the high dword, `jbe` on the
  low). So a sorted list ascends by sort key, surface type, primary light,
  prepass, material, `customIndex`, probe and object. INFERRED (the bit
  order above). A second sort (0x61aa20, one caller at 0x6368f8) flips
  bits 54..59 before comparing (`not`, `shl 0x16`, `xor`, `and 0xfc00000`,
  `xor`), so sort keys descend there. VERIFIED (the arithmetic). What that
  list is was not chased.

### 12.2 The material order

`Material_Compare` (0x621250) as the sort reads it, first difference
wins. INFERRED (control flow):

1. A material with a lit technique (`techniques[7]`, set+0x28) first.
2. Lit: `sortKey` ascending, a lightmapped one (`gameFlags` 0x2) first.
   Unlit: one with an emissive technique (`techniques[5]`, set+0x20)
   first, then `sortKey`.
3. The prepass key (0x6211d0) ascending.
4. A depth writer (`stateFlags` 0x8) first.
5. The lit (else emissive) technique's first pass's pixel shader name
   (`strcmp`).
6. Its pixel constants (0x621200 into 0x64cbf0), for a lit material only
   when it writes depth.
7. Its vertex shader name.
8. Not read on 1.7: the technique set's name and the material's.

- 0x6211d0: 3 when the set has no `techniques[0]` and no `[1]`, 2 with
  `[1]` alone; with `[0]`, 3 when `stateFlags` 0x4, else 1 unless the
  technique's flags (+4) hold 0x4, then 0. VERIFIED (the arithmetic:
  `neg`/`sbb`/`add 3`, `shr 2`/`not`/`and 1`).
- 0x64cbf0 per pass: past the per-primitive (+0xc) and per-object (+0xd)
  arguments, of the stable ones (+0xe) it skips those of a type below 5
  and collects the code constant index (`WORD` at +4) of each of type 5.
  0x64cb30 then skips below 6 and collects type 6 (the material's constant
  at `Material`+0x48 whose hash matches +4, its literal at +0x10) and type
  7 (the literal +4 points to), each by destination register (+2) through
  0x64cae0. VERIFIED (the loads and compares). The comparison: the code
  index counts, the indices, the literal counts, then per literal the
  register and the four floats, smaller first (`fcompp`, `test ah,0x41`
  returns -1). INFERRED (control flow).
- Checked against every stock map's zone, whose world surfaces were sorted
  by materials in this order when the zone was linked (12.3):
  `drawsurf_census` resolves each stub technique set from `common_mp` and
  holds the order's ranks to the zone's material runs. Over the 21 maps
  5750 neighbouring material pairs, all in order (mp_crossfire's 494;
  without the pixel constants 69 of those were out). VERIFIED (the census;
  per-map counts in its output). The comparator reads each set through
  `MaterialTechniqueSet`+8 (`remappedTechniqueSet`) before its techniques;
  vcod reads the set's own, which the census bears out for the stock
  zones. INFERRED.

### 12.3 The world's surfaces and the lit list

- `R_CompareSurfaces` (0x5ff060), the comparator `R_SortSurfaces`
  (0x5ff180) hands its sort (`push 0x5ff060` at 0x5ff208) over the
  0x30-byte surfaces: lit first, emissive, the material's
  `primarySortKey` (high dword `>> 0x16 & 0x3f`), `primaryLightIndex`
  (+0x16), `materialSortedIndex` (bits 29..39), `reflectionProbeIndex`
  (+0x15), `lightmapIndex` (+0x14), `firstVertex` (+4), then the word at
  +8. VERIFIED (the fields it loads). The order of the compares is
  INFERRED (control flow).
- A zone's world surfaces (model 0's run; the brush models' surfaces
  follow it) are stored in that order: on all 21 stock maps the surfaces
  of one (sort key, primary light) group fall into runs of one material
  each, no material's run splits, and within a run probe, lightmap and
  first vertex rise. VERIFIED (`drawsurf_census`). The camera walk visits
  surfaces by index, so the world draws in that order. INFERRED (9.9's
  walk).
- The draw-surf list merge is iw3mp.exe 0x63cce0: of the lists from a
  first index on that hold anything, it takes the lowest primary sort key
  among their heads (`key >> 54 & 0x3f`, 0x63cd7d..0x63cdb8), copies each
  list's run of that key in list order (0x63cc80), and repeats until all
  are empty; a lone list is copied whole (0x63ce24). VERIFIED (the
  instructions). The camera's lit stage merges lists 0, 1 and 2 (0x5fa2a6
  pushes the count 3, 0x5fa2d8 the first index 0). VERIFIED.
- List 1's count (0xcff438c) is stored by address only at 0x63b086, in
  0x63ac70; list 2's (0xcff4390) only at 0x607c12, in 0x607800; 0x607c60
  stores the count of a list its caller names (0x607e77). VERIFIED (the
  stores by address). That list 0 is the world's surfaces, 1 the static
  models' and 2 the scene entities' (brush models among them) is
  INFERRED, so per key the world draws first, then static models, then
  entities. The static models' list is sorted by the key
  (`R_SortDrawSurfs`). INFERRED.

vcod: `MapMaterial::order` holds each material's keys and
`Map::resolve_material_orders` fills a stub set's from the client's zones.
`to_mapworld` gives every soup a key in `MapWorld::soup_order`
(`drawsurf::soup_key`): the sort key, the list, then for the world the
surface index, for the other two the primary light, the material's rank,
`customIndex`, the probe and the static model or surface. The renderer
orders a CoD4 map's world batches by their float sort, then their first
soup's key (`cod4_places`), so the primary light comes before the
material as in 1.7's key. `VCOD_C4_DRAW_ORDER=0` at load keeps the CoD1
sorted shader list's order.
