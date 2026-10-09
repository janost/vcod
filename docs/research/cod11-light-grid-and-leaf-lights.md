# Static-model lighting: lights, leaf lists and the light-visibility grid (lumps 19, 30, 32)

How CoD 1.1 MP lights `misc_model` props and entity models, and the BSP
data it reads.
Evidence is `CoDMP.exe` 1.1 (image base `0x00400000`, virtual addresses),
read in the Ghidra export and the `objdump` disassembly, and the stock
paks. Each claim carries its own label. An earlier version of this doc said
lump 30 held 48-byte lights, that nothing in MP read lump 32, and that props
were lit by their `lightingPrecalc` tint alone. All three were wrong: the
48-byte loader it cited (0x4db240) is the lump-27 model loader, and the
grid readers below were missed.

## 1. When retail lights a static model

- VERIFIED: the entity pass of `LoadMap` hands each `misc_model` to 0x4dbae0,
  which reads `origin`, `model`, `angle`/`angles`, `modelscale_vec` (else
  `modelscale`) and `lightingPrecalc` (default `"1 1 1"`, string at
  0x54d2fc) and registers the model through 0x504a20.
- INFERRED: 0x4dbae0 skips any model whose name starts `xmodel/shadow_`
  (a branch on `strnicmp` against that 14-byte prefix), so retail registers
  no static model for those names and never draws them. vcod skips them too
  (`props::is_unregistered`). Section 12 has the rest.
- VERIFIED: 0x504a20 stores the previous head of the list at 0x14072ec in
  `record+0x9c` and the record as the new head. INFERRED: 0x5050a0 walks
  that list from its head calling 0x504cc0 on each, so models are lit in
  reverse entity order.
- VERIFIED: 0x504cc0 stores `(mins + maxs) * 0.5` of the placed model's
  bounds at `draw+0xc` and passes it to 0x4b6210, whose outputs land at
  `draw+0xb8` (light count), `draw+0xbc` (sky factor), `draw+0xc0` (weights)
  and `draw+0x184` (light pointers). INFERRED: the bounds from 0x5029d0 are
  the AABB of every vertex taken through the scaled axis (its loop over the
  posed vertices).
- VERIFIED: the same function stores `identityLight * c * 255` of each
  `lightingPrecalc` channel, clamped to 0..255, through 0x538be0 into the
  bytes at `draw+0x6c` (0x504ea7). 0x538be0 is MSVC's truncating
  `__ftol2`.
- INFERRED: lighting therefore happens once at load. The light pick
  (0x4b7450, section 6) runs the first time a static-model surface is built
  into the surface cache (`r_smc_enable`, 0x505260 calling 0x4e57e0), off the
  data 0x504cc0 stored.

## 2. Lump 30: 72-byte light records

- VERIFIED: `LoadMap` calls `R_LoadLights` (0x4db620) at 0x4dd380 with
  the lump-30 header, next to the "Loading lights..." print; the function
  references "R_LoadLights: funny lump size" (0x54d3f8), divides the length
  by 0x48 and allocates 0x88 bytes per runtime light. INFERRED: a length
  not divisible by 0x48 is a fatal error (the branch to that print).
- VERIFIED, stock census of the 12 MP maps: types 1 (12, one per map), 4
  (462), 5 (35, mp_powcamp) and 7 (13: mp_railyard, mp_powcamp, mp_harbor,
  mp_chateau).

File record, 18 words (VERIFIED from the loader's reads):

| Word | Meaning |
|---|---|
| 0 | type |
| 1..3 | colour (float) |
| 4..6 | origin |
| 7..9 | direction (the sun: unit vector towards it) |
| 10 | type 3: linear falloff; 4 and 7: quadratic; 5: cosine of the cone |
| 11 | types 4 and 7: constant falloff; 5: cone exponent (an int) |
| 12 | type 7: cosine of the cone |
| 13 | type 7: cone exponent (an int) |

Runtime record (0x88 bytes), VERIFIED from 0x4db620's stores:

| Offset | Field |
|---|---|
| +0x00 | type |
| +0x04..+0x0c | colour normalised by its luminance |
| +0x10 | luminance of `colour * identityLight` (weights 0.299, 0.587, 0.114 at 0x5405f8) |
| +0x14..+0x20 | ambient RGBA: `colour * identityLight * 0.1`, alpha 1 |
| +0x24..+0x30 | diffuse RGBA: `colour * identityLight * 0.8`, alpha 1 |
| +0x44..+0x4c | origin, or direction for the sun |
| +0x50 | w: 0 directional, 1 positional |
| +0x54..+0x5c | cone axis, the file direction negated |
| +0x60, +0x64, +0x68 | constant, linear, quadratic falloff |
| +0x6c | cone exponent |
| +0x70 | cone half-angle in degrees, `acos(word) * 57.2958`; 180 is no cone |

Per type (INFERRED, the switch on the type in 0x4db620): 1, the sun: ambient 0, diffuse
from the worldspawn (section 3), luminance recomputed from it, direction
words 7..9, w 0, constant 1; the loader keeps a pointer to it at
`world+0x114` (the last type-1 light wins). 2: quadratic 1. 3: linear from
word 10. 4: quadratic word 10, constant word 11. 5: quadratic 1, cone from
`acos(word 10)` (0x4db8d8), exponent word 11. 7: quadratic word 10, constant
word 11, cone from `acos(word 12)` (0x4db927), exponent word 13.

## 3. Worldspawn keys

INFERRED, from the first block of the entity pass at 0x4dbf50 (strings
0x54d28c..0x54d1c8 compared case-insensitively, stores at 0x11a31c0 to
0x11a31f0, `world` being 0x11a30e8):

- `ambient` (default 0; above 2.0 it warns "ambient too big" and scales by
  4/255), `_color` (normalised), `diffuseFraction` (default 0.5), `suncolor`
  (normalised), `sundiffusecolor` (normalised; `suncolor` when absent),
  `sunlight` (default 1), `sundirection`.
- World ambient (`world+0xd8`): `identityLight * ambient * _color` when
  both are non-zero, else 0.
- With `t = (sunlight - ambient) * identityLight`: the sun light's diffuse
  (`world+0xe8`) is `suncolor * (1 - diffuseFraction) * t`, and the sky
  colour (`world+0xf8`) is `sundiffusecolor * diffuseFraction * t`, its
  luminance at `world+0x108`.

## 4. Lump 19 and the leaf's light list

- VERIFIED: the leaf loader (0x4db4e1-0x4db548) copies the 36-byte leaf's
  cluster (byte 0) to `leaf+0xc`, its first light index (byte 28) to
  `leaf+0x14` and its count (byte 32) to `leaf+0x18`. INFERRED: when the
  count is non-zero and the first lump-19 entry is negative it steps past
  that entry and sets `leaf+0x10` to 1, and "R_LoadNodesAndLeafs: too many
  lights in leaf" (0x54d421) fires above 15 lights.
- INFERRED: `leaf+0x10` means the leaf sees the sky: only the sky sampling
  (0x4b4ff0) and the sky factor's 0.25 floor (0x4b6210) read it.
- INFERRED: the point-to-leaf walk is 0x50b390, where `dot(p, normal) -
  dist <= 0` takes the back child.

## 5. Lump 32: the light-visibility cache

- VERIFIED: 0x4b7c00 compares the length with 0x300000 and copies into a
  table at 0x0ca28d0 of 262144 eight-byte slots (key, state, sun, mask), read
  as 8192 buckets of 32 by 0x4b5420. Each file slot is 12 bytes: `u32 key`,
  `u8 state`, five sun bytes, a pad byte, `u16 mask`; the copy takes the sun
  byte at `4 + r_diffuseSunSteps`. INFERRED: the five sun bytes are the
  samples for one to five steps.
- VERIFIED, mp_carentan census: 37644 used slots, 32513 with state 1 and
  5131 with state 2; the sun bytes top out at 2, 8, 18, 32, 50, twice the
  square of the step count.
- VERIFIED, the arithmetic in 0x4b5420: the key is `((x & 0x3ff) | (y << 10)) << 12 |
  (cluster & 0xfff)` and the bucket `(rev(z) + y * 0xc41 - x * 0xc3d) &
  0x1fff`, `rev` reversing the bits inside each byte (0x4b4de0). x, y, z are
  grid indices, `cluster` the cluster of the model centre's leaf.
- VERIFIED: every sampled mp_carentan slot I tested (5369, a seventh of
  them) fits its bucket with some z in the map's range under that hash.
- INFERRED, 0x4b5420's control flow: a lookup scans its bucket; an empty
  slot or a full bucket is a miss. A full bucket shifts its 31 first slots down one
  (`memmove` of 0xf8 bytes) and the new sample takes slot 0. A miss traces
  (section 7) and stores the result. The lookup returns the mask for state
  1 and -1 otherwise, and the sun byte either way.

## 6. Sampling and picking lights

VERIFIED: the grid constants below are the floats at 0x5690fc (-131072),
0x568ea0 (1/32) and 0x569174 (1/64).

INFERRED, 0x4b6210's control flow on the model centre `p`:

- A centre in a leaf with cluster < 0 gets the sun alone, weight 1, sky
  factor 1. A leaf with no lights and no sky flag gets nothing.
- Grid cells are 32 x 32 x 64 from (-131072, -131072, -131072): `x =
  fistp(p.x + 131072 - 0.5) >> 5` and `frac = (p.x + 131072) / 32 - x`;
  z uses `>> 6` and 1/64.
- The eight corners carry trilinear weights. A corner whose lookup returns
  a mask adds its weight to a running total, `weight * sun * 0.5 / steps^2`
  to the sky factor (`steps` is `r_diffuseSunSteps`), and its weight to each
  leaf light whose bit is set.
- A total under 0.98 and above 0 scales the sky factor and every light
  weight by `1 / total`, except the sun's, which gains `1 - total` instead.
  A sky-flag leaf floors the sky factor at 0.25.

INFERRED, the control flow of 0x4b69f0 as 0x4b7450 calls it with the stored
lights and weights (and the scene's dynamic lights):

- With a non-zero sky factor, sky luminance and `r_diffuseSunQuality`: at
  quality 1, or with more than `r_maxEntLights` lights, one directional
  light from above (direction (0, 0, 1)) with ambient and diffuse each half
  the sky colour. Otherwise two: ambient 0.75 and diffuse 0.25 of the sky
  from above, and diffuse -0.25 of it from below (direction (0, 0, -1)) with
  luminance -0.25 of the sky's. Both take the sky factor as weight; the factor left for the
  vertex pass becomes 0.
- Each weighted light gets a key: 1e19 for a negative luminance, else
  `luminance * 2^r_overBrightBits / (c + l d + q d^2) * weight` at the
  centre (0x4b5e30; no falloff for a directional light), plus
  `2^r_overBrightBits * skyLuminance * skyFactor` for types 1 and 8. Keys
  below `r_minEntLightIntensity` drop; the rest go into a list of at most 8
  sorted strongest first, a new key ahead of an equal one, a ninth pushing
  the weakest out.
- 0x4b5ed0 folds lights past `r_maxEntLights` into one; with both at 8 it
  never runs for a static model.

Cvar defaults, VERIFIED from the registration strings; the clamps are
INFERRED from 0x4b4d10 and 0x4f0780:
`r_maxEntLights` "8" (clamped to at least quality + 1 and at most the GL
light count), `r_diffuseSunSteps` "3" (1..5), `r_diffuseSunQuality` "2"
(0..2), `r_minEntLightIntensity` "0.02", `r_overBrightBits` "1" (forced to 0
without hardware gamma).

## 7. A missed sample

INFERRED, 0x4b5190's control flow, run at the grid point `g = ((x - 4096) * 32, (y - 4096)
* 32, (z - 2048) * 64)`:

- State 2 when `g`'s leaf has cluster < 0 or when a trace from `g` nudged
  0.01 towards the model centre to the centre is blocked (0x426130, mask
  0x2001).
- Otherwise state 1, and for leaf light `i` bit `i`: a directional light
  is seen when the ray from `g + dir * 0.1` to `g + dir * 32768` leaves
  without starting solid and either runs its full length or hits a
  `SURF_SKY` (4) surface (0x4b4f20); a positional light when the ray from
  `g` nudged 0.1 towards it reaches it (0x4b4f90).
- For a sky-flag leaf (0x4b4ff0): a `steps x steps` fan of rays from `g` up
  0.1 to `g + (dx, dy, 32768)`, `dx` and `dy` spaced 32768 apart (16384 above
  3 steps) and centred; each that gets out adds 2 to the sun byte and sets
  mask bit 0x8000.

## 8. The vertex colour

INFERRED, 0x4e5210's control flow, called per vertex by the surface cache's
`lightingDiffuse` case (0x4e57e0, case 10) with the world position and the
normal taken through the scaled axis and renormalised by 0x42db90
(`VectorNormalize`):

- A map with no lump-19 entries (`world+0x118` zero) colours every vertex
  `identityLight` (byte 127).
- Otherwise `colour = sky * skyFactor + worldAmbient`, plus per picked
  light `(max(0, N . L) * diffuse + ambient) * atten`. A directional light
  uses its stored direction and `atten = weight`. A positional light uses
  the unit vector to it and `atten = weight / ((d q + l) d + c)`; with a
  cone, the cosine between that vector and the cone axis must exceed the
  sine table entry `(cutoff + 90) * 1024 / 360` (a quantised cosine) and
  then multiplies in `exponent` times, else `atten` is 0.
- Each channel is `fistp(colour * 255)` clamped to 0..255.

## 9. Which stage gets which colour

- VERIFIED, the string loads and stores at 0x4f8c69-0x4f8e34: the `rgbGen`
  keywords map to stage values
  wave 8, const and constLighting 0xc, identity 2, identityLighting 1,
  entity 3, oneMinusEntity 4, vertex 6, exactVertex 5, lightingAmbient 9,
  lightingDiffuse 10, oneMinusVertex 7, lightingPrecalc 0xb.
- INFERRED: a prop skin named `type@name` loads the template
  `shadertypes/model/<type>.stype` (0x4fc0c0 builds `@model/<type>`); a skin
  with no `@` and no script gets an implicit stage with `rgbGen
  lightingDiffuse` (0x4fc440 stores 10 in its -1 "model skin" case).
- VERIFIED, stock `shadertypes/model/*.stype`: 50 stages say
  `lightingDiffuse`; `foliage_detail` says `lightingPrecalc`; `cloth_light`
  and `glass_light` `identityLighting`; the two `objective_incomplete`
  types `constLighting`.
- INFERRED, 0x4e57e0's switch: the surface cache colours case 1 with the
  `identityLight` byte, case 0xb with the stored precalc bytes and case 0xc
  with the stage constant.
- INFERRED: `radialNormals` (0x547048), which four foliage types carry, is
  matched and its line skipped (0x4fab30).

## 10. vcod

`crates/common/src/static_light.rs` ports sections 2 to 8 and
`props::build` applies section 9:

- Lighting is computed at map load with retail's defaults and one
  overbright bit (`identityLight` 0.5, as `renderer.rs` assumes), in reverse
  entity order so misses fill the cache in retail's order. The bounds centre
  is the AABB of the baked LOD-0 vertices.
- Dynamic lights add per vertex on top of the baked colour (section 11).
- Misses trace through a `CollisionWorld` built from the world brushes
  alone, with no static models, the first time a sample is missing.
- `shadow_*` placements are not drawn (section 12).

VERIFIED, vcod measurement 2026-10-09 (release build, all 12 stock maps):
most models find all eight corners in lump 32 (mp_carentan 425 of 592,
mp_chateau 858 of 883); partial misses come in fours, one z layer, or with
the centre one x cell over. INFERRED: the compile sampled a slightly
different centre for those, or a different version of the map; retail would
trace the same misses. Lighting mp_carentan's 592 models, 844 traced
samples included, takes about 0.5 s; there is no per-frame cost. The mean
vertex byte over all prop vertices lands near `identityLight *
lightingPrecalc` (mp_carentan 62 against 49, mp_brecourt 66 against 60).

## 11. Dynamic lights on models

- VERIFIED, `RE_AddLightToScene` at 0x4e9b00 (the refexport slot stored at
  0x4b4b64): it returns without a renderer, with 32 lights queued already,
  or with an intensity at or below 0. Otherwise it fills a 0x88-byte record
  at `0x80000 + n * 0x88` in the scene buffer: type 2, the colour raw at
  +0x04, `intensity^2 / 32` at +0x10 (0x568ea0 is 1/32), ambient 0, diffuse
  `identityLight * intensity^2 / 32 * colour` at +0x24, the origin at
  +0x44, w 1, constant falloff 0.001 (0x3a83126f), linear 0, quadratic 1,
  cone 180 degrees (none), and the intensity itself at +0x74.
- VERIFIED: `RE_RenderScene` (0x4e9cb0) publishes the frame's lights twice
  from `r_dynamiclight` (0x16c3868, default "1" at 0x5685e4): the count at
  0x16c5798 (the scene's +0x160, with the array at +0x164) is zero when the
  cvar is 0, and the count at 0x16c5794 is zero unless the cvar is 1.
  INFERRED: 0x16c5794 feeds the world's dlight bits (0x4b59f0) and
  0x16c5798 the model light pick, so `r_dynamiclight 2` lights models only.
- VERIFIED, 0x4b69f0 before the sky lights: each scene light whose squared
  distance to the model's lighting origin is at most `4 * intensity^2`
  (0x569080 is 4.0), so within twice its intensity, joins the candidates
  with weight 1 and sets the model's `+0xa0` flag. It then competes for the eight slots like any other
  light (section 6).
- VERIFIED, the static-model draw at 0x505260: a surface with no cached
  colours calls the light pick (0x4b7450) and builds the cache (0x4e57e0)
  only when `+0xa0` is clear. A cached surface whose material has bit
  0x18 set at `+0x54` calls the pick again and draws its uncached
  surface when a dynamic light reached the model. INFERRED: a prop near a
  dynamic light is relit each frame through the GL lighting path
  (`cod11-gamma.md` section 5) with the dynamic light among its eight, and
  goes back to its cache once the light is gone; which materials carry
  bit 0x18 is not traced, vcod takes it to be the `lightingDiffuse` skins.
- INFERRED: GL lighting gives a dynamic light `max(0, N . L) * diffuse /
  (d^2 + 0.001)` per vertex: at an intensity of 800 (the grenade's
  `Light`), one framebuffer unit at 100 units, a quarter at 200.
- Entity models take the same pick every frame (section 13).

vcod (`vs_prop` in `shader.wgsl`):

- A prop vertex of a `lightingDiffuse` skin (vertex alpha 255) adds every
  fx light within twice its radius by the formula above to its baked
  colour, clamped at 1, then doubles for the display. The fx `Light`
  block's size stands in for the intensity (INFERRED: the efx renderer's
  call is not traced). On a prop the dynamic light does not push a baked
  light out of the eight slots; on an entity model it does (section 13).
- The world keeps vcod's own fx-light falloff; retail's world dlights
  (0x4b59f0's bits) are not ported.

## 12. Shadows

- VERIFIED: the stock MP maps place no `xmodel/shadow_*` model. The 23
  `shadow_*` xmodels in `pak0.pk3` (trees, shrubs, `shadow_crate`) only
  serve the map compiler's lightmap shadows, and 0x4dbae0 drops their
  placements (section 1). `mp_powcamp` and `mp_rocket` carry the world
  shader `textures/common/shadow`, drawn as world geometry.
- VERIFIED: `cg_shadows` is registered by the renderer's cvar block
  (0x4b3300, `"0"` at 0x56871c, flags 0x201) and by the cgame (`cgame_mp_x86.dll`
  table entry at 0x30074a24: vmCvar 0x301d9380, default `"0"`, flags 0x201).
- INFERRED, the cgame's player shadow 0x30027630: with `cg_shadows` 0 it
  returns at once. At 1 it traces down from the player and, unless bit 1
  of the entity's word at +8 (`eFlags` in Q3's layout) is set, drops the
  `markShadow` shader (registered through 0x30030b70) as a temporary mark
  of radius 16 under the player, its grey level faded with the height. Value 2 is the renderer's stencil path (`"<stencil shadow>"`).
- So retail's default draws no blob shadow under players. vcod draws none
  and does not read `cg_shadows`.

## 13. Entity models

- VERIFIED, 0x4b7320's disassembly: it runs once per refEntity per frame
  (it returns when the byte at `+0xa1` is set and sets it), takes the
  lighting origin from `+0x0c` when bit 7 of the byte at `+0x04` is set
  (Q3's `RF_LIGHTING_ORIGIN`, 0x80) and from `+0x44` (the origin)
  otherwise, and calls 0x4b7290. With `cg_shadows` 2 (cvar pointer
  0x16c36f8, name at 0x557b80) it also stores a direction at `+0xa4`, read
  by nothing else in this section.
- VERIFIED, 0x4b7290's disassembly: with a map loaded (0x16c4d54), leaf
  lights present (`world+0x98`) and `r_entFullbright` 0 (0x16c39e4, name at
  0x557dfc, default "0") it calls 0x4b6210 on the origin (section 6) and
  then 0x4b69f0 with the scene (section 11); otherwise it stores a light
  count of 0 at `+0xcc`.
- VERIFIED: 0x4b69f0 appends each scene light within twice its intensity
  after the leaf's lights and before the sky lights, weight 1 (the loop
  over `scene+0x160` / `+0x164` writing at `param_4 + n * 4`), then sorts
  all candidates into eight slots (section 6). So a dynamic light pushes
  the weakest map light out of an entity's eight. The sky lights are built
  per entity, at `+0x19c` and `+0x224`.
- INFERRED, 0x50e3e0's call at 0x50e81c: the xmodel entity add calls
  0x4b7320 for every entity model it draws.
- VERIFIED, 0x4d64e0 (called from the surface draw at 0x4d696c and
  0x4d6adb when the material's `+0x54` has a bit of 0x18 set): it sets
  `GL_LIGHT_MODEL_AMBIENT` (0xb53) to `sky * skyFactor + worldAmbient`
  (`world+0xf8` times the entity's `+0x110`, plus `world+0xd8`), loads the
  matrix at 0x16c5408 through the pointer at 0x16c4020 when it has lights,
  and calls 0x4d63a0 per light. 0x4d63a0 sets `GL_AMBIENT` and
  `GL_DIFFUSE` to the light's `+0x14` and `+0x24` times its weight,
  `GL_SPECULAR` `+0x34`, `GL_POSITION` `+0x44` (w at `+0x50`),
  `GL_SPOT_DIRECTION` `+0x54`, `GL_SPOT_EXPONENT` `+0x6c`,
  `GL_SPOT_CUTOFF` `+0x70` and the three attenuations `+0x60..+0x68`.
- INFERRED: the loaded matrix is the world view, so light positions are in
  world space, and the call through 0x16c3d00 on `0x4000 + i` for every
  slot past the count is `glDisable`.
- VERIFIED, the GL init at 0x4b2bf7..0x4b2c6f: light model ambient (0, 0,
  0, 1), local viewer 0, two-sided 0; the material's ambient and diffuse
  (0x1602, both sides) are (1, 1, 1, 1), its specular and emission (0, 0, 0,
  1) and its shininess 0.
- VERIFIED: 0x4d64e0 raises the ambient to `[0x16c55b0] *
  r_entMinLight` (0x16c37c0, name at 0x557de8, default ".15") by its
  luminance when bit 0x20 of the refEntity's `+0x04` is set.
- INFERRED: 0x16c55b0 is `identityLight`, and none of the cgame
  refEntities in the table below sets bit 0x20.
- INFERRED, so an entity model's vertex is GL's fixed-function colour:
  `ambient + sum over lights of atten * spot * (weight * ambient_i +
  max(0, N . L) * weight * diffuse_i)`, clamped to 0..1, times the
  texture, and the display doubles it (`cod11-gamma.md` section 5). A
  directional light has `atten` 1; a positional one `1 / (c + l d + q
  d^2)`. GL's spot test is `dot(-L, spot direction) >= cos(cutoff)`, while
  the static vertex pass (section 8) tests `dot(L, +0x54)`: with the same
  `+0x54` the two disagree in sign, so a cone light lights static models
  in its cone and entity models on its back side. vcod keeps both as
  retail sets them.

The lighting origins the cgame (`cgame_mp_x86.dll`) sets, VERIFIED from
the refEntity stores before each `0x3d` (add refEntity) trap:

| Draw | renderfx | Lighting origin |
|---|---|---|
| Player (`ET_PLAYER`, 0x30028210) and corpse (`ET_CORPSE`, 0x30028400) | `0x80`, plus 2 for one's own body | lerped origin, z plus the next state's `fTorsoHeight` (centity `+0x1d4`) plus 12 when `eFlags` has 0x40 (prone, float at 0x300693fc), 20 with 0x20 (crouch, 0x30069448), 32 otherwise (0x30069408) |
| Turret (`ET_TURRET`, 0x3001b2e0) | `0x80` | origin, z plus 32 (0x30069408) |
| View weapon (0x30036cf0) | `0x8c` | playerstate origin (`+0x14`), z plus `viewHeightCurrent` (`+0xd0`) |
| Item (0x3001adb0) | 0 | the origin |
| General and script mover (0x3001ab50, 0x3001b710) | 0x80 only with `eFlags` 0x10000 (0x3001aaa0) | then the centity's `+0x210`, else the origin |

INFERRED: centity `+0xf0` starts the next entity state (its `eFlags` at
`+0xf8`), so `+0x1d4` is that state's `fTorsoHeight` (offset 228).

vcod (`StaticLighting::entity_lights`, `client/src/entity_light.rs`,
`gl_lighting` in `dynamic_model.wgsl` and `viewmodel.wgsl`):

- The renderer keeps the map's `StaticLighting` after the props are lit,
  so entity samples share the props' cache. Each frame every distinct
  lighting origin is picked once (a player's parts share one) with the
  fx lights as scene lights, and the vertex shader applies the GL formula
  above per vertex. The viewmodel's lights are moved into view space.
- The `eFlags` 0x10000 lighting origin of general entities is not ported;
  they light at their origin. Every entity skin is treated as lit; the
  0x18 material bit is not traced (section 11).
- A map without leaf lights draws entity models at `identityLight`.
- VERIFIED, vcod measurement 2026-10-09 (release build): 2.8 us per pick
  on mp_carentan's open ground, cache misses included.

