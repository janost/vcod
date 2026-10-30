# CoD2 1.3 materials

CoD2 dropped CoD1's text shader scripts (`cod11-shader-scripts.md`) for a
compiled chain: a binary `materials/<name>` names a technique set, the
`.techset` text maps each render pass type to a technique, the `.tech` text
gives each technique's passes (a statemap, a vertex stream routing, a vertex
and a pixel shader with their arguments), the `.sm` text rewrites the
material's render state per pass, and `materials/shaders/*.hlsl` is shader
model 1.1/2.0 source compiled at load. `materials_dx7/` holds the fixed
function fallback.

## 0. Modules and tools

| Module | md5 | Role |
|---|---|---|
| `cod2mp_mac13_i386` (Mac 1.3 MP, i386 slice, Mach-O, STABS) | `b30966627b680de20bf35603c562a3f6` | every address below unless marked; image base 0 |
| `gfx_d3d_mp_x86_s.dll` (Windows 1.3 MP renderer, image base `0x10000000`) | `2402bf2072f9255b85e3d23297d1d0ed` | string cross-checks |
| `cod2_lnxded` (Linux 1.3 dedicated server) | `fefed4c4909fb7c5c13b13a003f5ccf7` | what the server reads |

The Mac build draws through a Direct3D 9 shaped layer over OpenGL: its
`RB_SetSampler` (0x000ce2ce) calls a `SetSamplerState`-like vtable slot with
D3D9's `D3DSAMPLERSTATETYPE` numbers 1, 2, 3, 5, 6, 7 (VERIFIED, decompile).
The Windows DLL carries the same loader strings in `.rdata`:
`materials/techniquesets/%s.techset` at 0x10194b1c,
`materials_dx7/techniquesets/%s.techset` at 0x10194b40,
`materials/techniques/%s.tech` at 0x10194f48, `materials/statemaps/%s.sm`
at 0x10195778, `Too many labels in technique set` at 0x10194acc,
`lightprobe smodel cached exp fog` at 0x10194cd9, `vs_main` at 0x10195248,
`mtlStencilFuncBack` at 0x10195b90, `specularitySampler` at 0x10196848
(VERIFIED). So the text formats and their names are shared; the code
constants `OpenGLworldViewProjectionMatrix` and kin exist on the Mac only
(VERIFIED, absent from the DLL). The Mac binary also embeds its own
pre-translated ARB programs, 208 `*.vsa`, 208 `*.vsn` and 209 `*.pse`
names beside 220 `!!ARBvp1.0` and 130 `!!ARBfp1.0` texts (VERIFIED,
`strings`), keyed by the HLSL file names (`lmap_a.vsa`); a second reference
for the shader math, Mac side only. Every fact below is Mac side unless it
names the DLL or an asset.

Tools, run on the stock paks extracted in iwd order (`iw_00..15`, then the
`localized_english_iw*` ones; later names win):

- `tools/re/cod2_matparse.py <materials dir>`: parses every binary material,
  tracks ownership of every byte and fails on an overlap; writes
  `mats.json`.
- `tools/re/cod2_techparse.py <materials dir>`: parses every dx9 techset,
  technique and statemap with the grammars of sections 4 to 6; prints the
  keyword census; writes `tech.json`.
- `tools/re/cod2_mapcensus.py <maps/mp dir> <xmodel dir> mats.json
  tech.json`: the census of section 8.
- `vcod_cod2::material` parses the same files from the iwds; its
  `crates/cod2/tests/material_stock.rs` (with `COD2_DIR`) repeats the
  counts and the census below.
- `vcod_cod2::materials` compiles each material into vcod's neutral
  `Material`: the lit technique of 4.1, its first pass statemap applied
  to the material's `stateBits`, read as one CoD1-style stage (colour
  map, plus the lightmap under the `lmap*` techniques). That stage is an
  approximation, not a port of section 9. What draws closer to retail:

  | Material | Carries | Drawn by |
  |---|---|---|
  | `lmap` with only `_a`, `_s`, `_dtl` suffixes | its switches and `detailScale` (`PhongLit`) | the CoD2 world pipeline (`renderer/cod2.rs`, `cod2_world.wgsl`), as 9.1 |
  | sky | its cube's image path | `sky_cube.wgsl` |
  | probe sets | `Material::probe` | `cod2_model.wgsl` (9.4, 9.6) |
  | `water_l_sun` | `WaterLit` | the CoD2 water pipeline (9.12) |
  | `particle_cloud*` | | the cloud pass (9.14) |

  Texture scroll on world surfaces and every other family stay on the
  stage. The stock materials of 24 sets (objective, the renderer's
  post-effect materials, distortion, the shadow cookie sets) take the
  colour-map-only fallback; `materials::fallback_use` says which of them
  default play shows (9.11), and `crates/cod2/tests/materials_lib.rs`
  reports them.

## 1. Loading

- `Material_LoadFile` (0x000d2f74) formats `materials/%s` (VERIFIED,
  decompile). `Material_Load` (0x001057d0) reads the file whole into one
  allocation and passes it to `Material_FinishLoadingInstance` (0x0010413a),
  which adds the buffer's address to the offset fields in place (INFERRED,
  call order and the `param_1 + *(int *)param_1` stores). The file is the
  in-memory `MaterialObj` with offsets where pointers go.
- A techset not yet loaded is read from `materials/techniquesets/%s.techset`,
  or from `materials_dx7/...` when `r_rendererInUse` is 2 (INFERRED, the
  string choice in 0x0010413a). `r_rendererInUse` takes `none`, `dx9`, `dx7`
  (`_s_technologyNames`, VERIFIED, table read).
- Technique, statemap and shader are cached by name
  (`Material_FindTechnique` 0x000d33a4, `Material_FindStateMap` 0x000d32e4,
  `Material_FindShader` 0x000d3408; VERIFIED, names). A technique file is
  read only when one of its labels has `g_useTechnique[type]` set
  (INFERRED, the branch at 0x001046bc).
- After loading, every pass's shader arguments are checked against the
  material (`Material_ValidatePassArguments` 0x0010107e; INFERRED, the
  loop at the end of 0x0010413a).
- The world loader registers a soup's material with
  `Material_Register(name, 9)` from lump 0's 72-byte record (`R_LoadSurfaces`
  0x000e4672; VERIFIED, decompile).

## 2. The binary material file

Little endian. The layout is STABS `MaterialObj` (68 bytes) over
`MaterialInfoObj_s` (44 bytes), and the offsets the loader rebases are
0x00, 0x04, 0x38, 0x3c, 0x40, each texture's name and `u`, each
constant's name (VERIFIED, STABS and 0x0010413a). All 3765 stock materials
(`materials/` plus its `gfx/`, `fonts/`, `skins/`, `ui/` subtrees; 3621 at
the top level) parse to exact consumption: header, tables and strings own
every byte once (VERIFIED, `cod2_matparse.py`).

| Off | Type | Field | Stock values (VERIFIED, census) |
|---|---|---|---|
| 0x00 | u32 ofs | `name` | equals the file name in all 3765 |
| 0x04 | u32 ofs | `refImageName` (Radiant preview image) | |
| 0x08 | u16 | `hashIndex` | 0 in the file |
| 0x0a | u16 | `sortedIndex` | 0 in the file |
| 0x0c | u8 | `gameFlags` | 0x3 2631, 0x0 985, 0x4 48, 0xc 39, 0x1 30, 0x8 22, 0x7 6, 0x5 4 |
| 0x0d | u8 | `sortKey` | 3 2347, 27 652, 11 402, 8 74, 26 49, 9 45, 4 40, 14 39, ... |
| 0x0e | u8 | `textureAtlasRowCount` | 1, except 18 `gfx_*` atlases (2 or 4) |
| 0x0f | u8 | `textureAtlasColumnCount` | 1, 2 or 4 |
| 0x10 | f32 | `maxDeformMove` | 0 |
| 0x14 | u8 | `deformFlags` | 0 |
| 0x15 | u8 | `usage` (editor category) | 0..26 |
| 0x16 | u16 | `toolFlags` | 0x10, 0x20, 0x40, 0x80 dominate |
| 0x18 | u32 | `locale` (editor locale bits) | 0x800 on `dawnville2_*`, 0x2000 on `mtl_caen_*` |
| 0x1c | u16 | `autoTexScaleWidth` | the colour map's size, e.g. 512 |
| 0x1e | u16 | `autoTexScaleHeight` | |
| 0x20 | f32 | `tessSize` | 0 |
| 0x24 | u32 | `surfaceFlags` | section 11 |
| 0x28 | u32 | `contents` | section 11 |
| 0x2c | u32[2] | `stateBits` | section 3 |
| 0x34 | u16 | `textureCount` | 0..4 |
| 0x36 | u16 | `constantCount` | 0 or 1 |
| 0x38 | u32 ofs | `techniqueSet` (its name) | 64 names |
| 0x3c | u32 ofs | `textures` | always 0x44 |
| 0x40 | u32 ofs | `constants` | |

- The string table follows the tables; strings are NUL terminated and
  shared by offset (`colorMap` once per file).
- The 64 set names are 63 of the 77 dx9 `.techset` files plus
  `shadowoverlay`, a set only `materials_dx7/` ships (VERIFIED, census).
- Field names come from STABS (VERIFIED). What `usage`, `toolFlags` and `locale` mean
  is INFERRED from the census (`usage` 4 on floors, 7 on plaster walls, 24 on
  skies; `toolFlags` 0x1 on `occluder`, 0x2 on `portal_nodraw`, 0x4 on
  `origin`), and nothing in the renderer was seen reading them.
- `gameFlags` 0x8 marks a sky: `R_LoadSurfaces` errors on two different
  materials with it in one map (`map has at least two different skies`)
  and takes that material's `colorMap` as the sky cube (INFERRED, the
  `& 8` branch in 0x000e4672; the error string VERIFIED).
  0x4 on tools and water: the world cell walk skips such surfaces (INFERRED,
  `(gameFlags & 4) == 0` tests in `R_CellSurfaces` and
  `R_AABBTreeSurfaces_r`). 0x1 and 0x2 have no reader identified; 0x3 rides
  2631 materials, nearly all `phong_*` (25 `phong_blend` ones carry 0), and
  0x1 alone the code materials (`$default`, the
  `filter_*` ones) (VERIFIED, census).
- `sortKey` orders materials: `Material_Compare` (0x000d2be2) compares byte
  0x0d, then the technique set pointer; `Material_Sort` (0x000d2fe0) sorts
  the loaded list with it (INFERRED, decompile). Stock keys by techset:
  3 opaque (`phong_replace*`, `2d`), 4 `sky`, 5/7 sun and backdrops
  (`unlit_blend`), 8..16 world decals (`phong_blend*`, `effect_multiply`), 25
  distortion and z-feathered effects, 26 effects, 27 2D, tools and
  effects (VERIFIED, census).

### 2.1 Texture table

12-byte `MaterialTextureDef` records at 0x44 (VERIFIED, STABS and stride
0xc in 0x0010413a):

| Off | Type | Field |
|---|---|---|
| 0 | u32 ofs | `name` (the shader's sampler name), registered as a string |
| 4 | u8 | `samplerState` (2.2) |
| 5 | u8 | `semantic` |
| 6 | u8 | `unused_0`, 0 in all 8409 records |
| 7 | u8 | `unused_1`, 0 in all 8409 |
| 8 | u32 ofs | `u`: the image name, or the water def when `semantic` is 5 |

- Names and semantics seen (VERIFIED, census): `colorMap` 2 (and 0 on the
  557 2D materials), `detailMap` 2, `normalMap` 3, `specularMap` 4,
  `cubicEnvMap` 2, and `normalMap` 5 on the six `water*` materials. Texture
  sets: colour+normal 1283, colour alone 1044, colour+detail+normal+spec 548,
  colour+normal+spec 522, colour+detail+normal 341, none 21, the water set 6.
- `semantic` goes to `Image_Register(name, semantic, flags)` as the image's
  semantic (VERIFIED, the call's arguments in 0x0010413a). `Image_PicmipForSemantic` (0x000e6d74)
  gives semantics 0 and 1 no picmip and picks a different picmip global for
  2, 3 and 4 (INFERRED: the colour, bump and spec picmip dvars). With
  `r_rendererInUse` 2 a semantic 3 or 4 texture gets no image (INFERRED,
  the branch in 0x0010413a): dx7 has no normal or spec maps.
- Semantic 5 points `u` at a 32-byte water def (STABS `MaterialWaterDef`):
  `textureWidth` i32, `horizontalWorldLength`, `verticalWorldLength`,
  `amplitude`, `windSpeed`, `windDirection[2]` f32, `map` (0 in the file;
  the loader stores the image `R_LoadWaterSetup` builds there). Stock rows:
  64, 37, 37, 0.06..0.1, 50..76, (1,0) or (0.447,0.894) (VERIFIED, census).
  The water def sits between the texture table and the constants, which is
  why `constants` is not always `0x44 + 12 * textureCount` (VERIFIED,
  `water_duhoc`).

### 2.2 The sampler state byte

`RB_SetSampler` (0x000ce2ce) decodes it as below (INFERRED, the masks and
branches in the decompile; the D3D names are the sampler state numbers it
passes). The tables it indexes are VERIFIED (read).

| Bits | Meaning |
|---|---|
| 0x03 | min/mag filter: index into `_s_filterTable` (0x0032ab50) = {0, 1, 2, 3}; 3 takes the device's anisotropic min and mag filters |
| 0x0c | mip filter: `(state >> 2) & 3` into the same table (0 none, 1 point, 2 linear) |
| 0x10 | clamp U (address 3) instead of wrap (1) |
| 0x20 | clamp V |
| 0x40 | clamp W |

- When the low nibble is 0, the byte is ORed with
  `_defaultSamplerStateTable[r_textureMode]` (INFERRED, the branch). The
  table (0x0032ab60) holds `nearest` 1, `linear` 2, `bilinear` 6,
  `trilinear` 10, `anisotropic` 11 (VERIFIED, table and `_textureModeNames`
  0x0034a2d8 read).
- Stock bytes (VERIFIED, census of 8409 records): 0 (default) 7157, 0x30
  (clamp both, default filter) 710, 0x20 255, 0x37 (clamp, anisotropic, point
  mip) 131, 0x02 49, 0x07 37, 0x10 25, 0x01 22, 0x32 11, 0x06 7, 0x31 5.
  The water `normalMap` is 0x06 and the water `cubicEnvMap` 0x30.

### 2.3 Constants

20-byte `MaterialConstantDefObj`: name ofs u32, then `literal` f32[4]
(VERIFIED, STABS, stride 0x14). Stock: `detailScale` 889 (16,16 197 times;
8,8 159), `featherParms` 41 (0.025, 40 most), `waterColor` 6,
`distortionScale` 5 (VERIFIED, census). A technique reads one through
`material.<name>`.

## 3. stateBits

Two u32 words. The bit groups come from `_s_stateMapSrcBitGroup`
(0x0036ba60: name, value table, mask word 0, mask word 1) and its value
tables (VERIFIED, tables read):

| Word | Mask | Field | Values |
|---|---|---|---|
| 0 | 0x0000000f | `mtlSrcBlend` | 1 Zero, 2 One, 3 SrcColor, 4 InvSrcColor, 5 SrcAlpha, 6 InvSrcAlpha, 7 DestAlpha, 8 InvDestAlpha, 9 DestColor, 10 InvDestColor, 12 BlendFactor, 13 InvBlendFactor |
| 0 | 0x000000f0 | `mtlDestBlend` | same, `<< 4` |
| 0 | 0x00000700 | `mtlBlendOp` | 0 Disable, 1 Add, 2 Subtract, 3 RevSubtract, 4 Min, 5 Max (`<< 8`) |
| 0 | 0x00003800 | `mtlAlphaTest` | 0x800 Always, 0x1000 GT0, 0x2000 LT128, 0x3000 GE128 |
| 0 | 0x0000c000 | `mtlCullFace` | 0x4000 None, 0x8000 Back, 0xc000 Front |
| 0 | 0x000f0000 | `mtlSrcBlendAlpha` | factor `<< 16` |
| 0 | 0x00f00000 | `mtlDestBlendAlpha` | factor `<< 20` |
| 0 | 0x07000000 | `mtlBlendOpAlpha` | op `<< 24` |
| 0 | 0x08000000 | `mtlColorWriteRgb` | |
| 0 | 0x10000000 | `mtlColorWriteAlpha` | |
| 0 | 0x20000000 | `mtlFog` | Enable |
| 0 | 0x80000000 | wireframe (destination group only) | |
| 1 | 0x00000001 | `mtlDepthWrite` | |
| 1 | 0x0000000e | `mtlDepthTest` | 2 Disable, 4 LessEqual, 8 Equal, 0xc Always |
| 1 | 0x00000030 | `mtlPolygonOffset` | 0, 0x10, 0x20 |
| 1 | 0x000000c0 | `mtlStencil` | 0 Disable, 0x40 OneSided, 0xc0 TwoSided |
| 1 | 0x00000700 | front pass op | Keep, Zero, Replace, IncrSat, DecrSat, Invert, Incr, Decr (`<< 8`) |
| 1 | 0x00003800 | front fail op | same `<< 11` |
| 1 | 0x0001c000 | front z-fail op | same `<< 14` |
| 1 | 0x000e0000 | front func | Never, Less, Equal, LessEqual, Greater, NotEqual, GreaterEqual, Always (`<< 17`) |
| 1 | 0x00700000.. | back ops and func | the front fields `<< 16` |

- The four back-face source entries carry the front masks (0x700, 0x3800,
  0x1c000, 0xe0000) while their values sit 16 bits higher (VERIFIED, table);
  no stock statemap tests them.
- Stock words (VERIFIED, census): `28128812 00000005` (2161: no blend, One
  Zero, alpha test Always, cull back, rgb write, fog on, depth LessEqual and
  write) on opaque materials; `28128965 00000014`/`..04` (SrcAlpha
  InvSrcAlpha, polygon offset 1, no depth write) on world decals; `28125165
  00000002` (alpha blend, GT0, cull none, depth test off) on 2D;
  `28129131 00000014` (Zero SrcColor, the multiply) on `effect_multiply`;
  `2812b012`/`2812a012` (GE128/LT128) on alpha tested foliage.
- When the backend's flag at `backEnd+0x4bd` is set, a pass's word 1 loses
  its polygon offset bits (INFERRED, the branch in `RB_DrawSingleTechnique`
  0x000f7e66);
  `r_drawDecals 0` skips draw surfaces whose material has polygon offset
  bits (INFERRED, the `& 0x30` test on the material's byte 0x30 in
  `RB_RenderDrawSurfList` 0x000d5630).

## 4. Technique sets

Grammar, read off the parser in 0x0010413a (INFERRED); all 77 stock dx9
techsets parse with it (VERIFIED, `cod2_techparse.py`):

```
techset   := ( label+ name ';' )*
label     := '"' typename '"' ':'
```

- Quotes are kept by the tokenizer; a quoted token is a label, looked up in
  `Material_TechniqueTypeForName`'s table (0x0036cb20, 34 names); an unknown
  one fails the set. More than 34 labels before a name fails (`Too many
  labels in technique set`). Several labels may share one technique
  (INFERRED, the parser's branches). A type with no label has no technique
  and the material is not drawn in that pass (INFERRED:
  `RB_RenderDrawSurfList` 0x000d5630 skips a draw surface whose
  `techniques[type]` is null).
- Three stock sets no material uses name techniques no pak holds:
  `phong_alphatest_detail_scroll` (five, one spelled `light_poin_dtlt`),
  `phong_alphatest_spec_detail_scroll` (two) and
  `phong_replace_detail_scroll` (the three `lprobe_dtl_scroll*`); every
  set a material names resolves (VERIFIED, census).
- `MaterialTechniqueSet` is a name and 34 technique pointers (140 bytes,
  STABS); a `MaterialTechnique` is name, `flags` u16, `passCount` u16 and at
  most 4 passes (INFERRED, the `== 4` exit in 0x0010413a; the struct
  layouts VERIFIED from STABS).

The 34 types, in table order, with STABS enum names (VERIFIED):

| # | Label | # | Label |
|---|---|---|---|
| 0 | depth prepass | 17 | lightprobe smodel cached exp fog |
| 1 | sky | 18 | pointlight no fog |
| 2 | build floatz | 19 | pointlight linear fog |
| 3 | unlit no fog | 20 | pointlight exp fog |
| 4 | unlit linear fog | 21 | emissive no fog |
| 5 | unlit exp fog | 22 | emissive linear fog |
| 6 | lightmap no fog | 23 | emissive exp fog |
| 7 | lightmap linear fog | 24 | emissive feathered no fog |
| 8 | lightmap exp fog | 25 | emissive feathered linear fog |
| 9 | lightprobe no fog | 26 | emissive feathered exp fog |
| 10 | lightprobe linear fog | 27 | fakelight normal |
| 11 | lightprobe exp fog | 28 | fakelight view |
| 12 | lightprobe smodel no fog | 29 | solid wireframe |
| 13 | lightprobe smodel linear fog | 30 | shaded wireframe |
| 14 | lightprobe smodel exp fog | 31 | shadowcookie caster |
| 15 | lightprobe smodel cached no fog | 32 | shadowcookie receiver |
| 16 | lightprobe smodel cached linear fog | 33 | debug bumpmap |

### 4.1 Which technique a pass runs

- `R_RenderScene` (0x000c5e5c) queues the sorted draw surfaces with a base
  type per pass (INFERRED, call order; the type arguments VERIFIED): 1 (sky) then 6 (lit), the sun,
  then per point light partition a full-screen quad and type 18, then 21
  (emissive), then post effects. `r_fullbright` queues 3 (unlit) instead,
  `r_debugShader` 33, `r_showTris` 29.
- `RB_RenderDrawSurfList` (0x000d5630) turns the lit base into the real
  type per draw surface (INFERRED, the branches in the decompile): a surface with a lightmap
  stays 6 (lightmap); a surface without one (lightmap index 0x1f out of
  `R_DecomposeSort` 0x000c50a2) takes 9 (lightprobe) for an entity or the
  world's non-lightmapped soups, 15 (smodel cached) for the cached static
  model list (entity slot 0x7ff), 12 (smodel) for an entity whose type is 2.
  Every type from 3 to 26 then adds `RB_FogOffset()` (0x000faa68): 0 with
  `r_fog` off, else the scene's fog mode, 1 linear or 2 exp (INFERRED, the
  value read from the backend data).
- Types 0 and 2 (depth prepass, build floatz) skip models unless
  `r_depthPrepassModels` is set (INFERRED, the branch at 0x000d5630).
- The emissive pass runs 24 (feathered) in place of 21 when `r_zFeather`
  is set and the `floatZ` target exists (VERIFIED, `gfx_d3d_mp_x86_s.dll`
  0x10024bc0 returns 0x18 or 0x15 on those two; 9.5).

### 4.2 The scene's fog

- `R_SetFog` (0x000d286c) fills a 0x20-byte fog record at `rg + 0x146c +
  index * 0x20`: type, start and end times, colour bytes, near, far,
  density. A density of 1 or more makes it type 1 (linear, density stored
  as 1), anything less type 2 (exp); the colour is stored as the bytes
  b, g, r, 0xff (INFERRED, the stores). `R_SwitchFog` (0x000d2936) aims the
  live fog at a record over a transition, and `R_RenderScene` lerps
  near, far, density and the colour bytes toward it while the live type
  equals the target's, else takes the target at once; the frame's result
  goes to the backend data at `+4` (type) and `0x219cf8..0x219d10`
  (INFERRED, the copies and the lerp in the decompile).
- The cgame reads configstring 12 in `CG_ParseFog` (0x001e273c): with a
  second token it parses seven numbers (near, far, density, r, g, b,
  transition ms), turns each colour into a byte as `floor(c * 255 + 0.5)`,
  sets record 1 and switches to it over the transition; a lone token
  switches to record 0 (no fog) over that many ms (INFERRED, the parse and
  the calls). The server writes the string in `Scr_SetFog` (lnxded
  0x0811571c) as `%g %g %g %g %g %g %.0f`; `setExpFog` passes near 0, far
  1 and its density, `setCullFog` its near and far with density 1.0
  (VERIFIED, the format string and the 0x3f800000 literal).
- `RB_SetIteratorFog` (0x000fab32) loads `fogConsts` (9.1) and the code
  constant `fogColor` as the colour bytes over 255, alpha included, and
  sets `D3DRS_FOGCOLOR` (0x22) to the colour, or to black when the
  destination blend is One (`dxState + 0x2000 & 0xf0 == 0x20`, so an
  additive draw fades out); `RB_UpdateFogColor` (0x000faa8c) redoes that
  choice per draw (INFERRED, the branches).
- Which technique a draw takes is the no-fog type plus `RB_FogOffset()`
  (4.1). The fogged techniques' first passes use the statemaps `fogged`,
  `additive_fogged`, `effect_add_fogged` or `zfeather_add_fogged`, each of
  which differs from its `*nofog` twin only in the `fog` group:
  `passthrough` against `Disable` (VERIFIED, the `.sm` files). So a fogged
  draw fogs exactly when the material's own `stateBits` carry 0x20000000,
  and an unfogged scene never fogs. `vertcol_mul_lfog` and `_efog` keep
  `nofog` for the multiply and fog through their second, `multiply_fog`
  pass (VERIFIED, `.tech` files).
- Over the 77 stock technique sets and the types 3, 6, 9 and 21 they
  label, 155 name a `nofog` technique without fog and `fogged` ones with
  it, 44 name `nofog` techniques for all three (`vertcol_simple`, the 2D,
  post and filter sets), and the linear and exp labels always name
  techniques with the same statemaps (VERIFIED, census of the techset and
  tech files). Every stock material therefore fogs alike under linear and
  exp fog (VERIFIED, `crates/cod2/tests/materials_lib.rs`). `distortion`
  labels only `unlit no fog` among the unlit types, so under fog retail
  draws nothing there (INFERRED, the null check in 4.1).
- The sky (type 1) takes no offset and never fogs (INFERRED, 4.1's range).
- On mp_carentan under its `setExpFog(0.0001, 0.55, 0.6, 0.55)` 118,281
  world triangles fog, 61 (`$default`) do not, and 10 are sky (VERIFIED,
  `materials_lib.rs`). A wall 2000 units out keeps exp(-0.0001 * 2000)
  = 0.82 of its colour (INFERRED, `fogcalc.hlsl`).
- vcod compiles each material's fogged technique beside its no-fog one
  and keeps the answer in `Material::nofog`
  (`vcod_cod2::materials::{fog_technique, technique_fogs}`); the
  renderer's fog mode stands for the offset. Every CoD2 draw (lit, stage,
  model, effect) measures the radial distance; a CoD2 model also honours
  `nofog` and the black fog of an additive blend.

## 5. Techniques (dx9)

Grammar, read off 0x0010413a, `Material_LoadPassStateMap` 0x001035a0,
`Material_LoadPassShader` 0x001039b6 and `Material_SetPassShaderArguments`
0x001023a2 (INFERRED); all 258 stock `.tech` parse with it (VERIFIED,
`cod2_techparse.py`):

```
tech     := pass{1,4}
pass     := '{' 'stateMap' '"' smname '"' ';'
                route{0,16}
                'vertexShader' version '"' file '"' '{' arg* '}'
                'pixelShader'  version '"' file '"' '{' arg* '}'
            '}'
route    := 'vertex' '.' dest '=' 'code' '.' source ';'
dest     := 'position' | 'normal' | 'color' '[' 0..1 ']' | 'texcoord' '[' 0..7 ']'
source   := 'position' | 'normal' | 'tangent' | 'binormal' | 'color'
          | 'texcoord' '[' 0..1 ']'
arg      := name ( '[' row ( ',' row )? ']' )? '=' value ';'
value    := 'material' '.' name
          | 'sampler' '.' codesampler
          | 'constant' '.' name
          | 'code' '.' codeconst
          | float1..float4 literal
```

- Routes sort into a vertex declaration (`Material_AllocVertexDecl`
  0x000d31d0). Source codes: position 0, normal 1, color 2, texcoord[n]
  3+n, binormal 5, tangent 6; destination codes: position 0, normal 1,
  color[n] 2+n, texcoord[n] 4+n (VERIFIED, the constants the decompile
  stores per keyword).
- Stock routes (VERIFIED, census): position, normal, color[0],
  texcoord[0] from texcoord[0]; the lightmap techniques put texcoord[1]
  (lightmap coords) in texcoord[1], tangent in texcoord[2], binormal in
  texcoord[3]; the light probe and point light ones put tangent in
  texcoord[1] and binormal in texcoord[2].
- `version` is a float, 1.1 or 2.0 in stock (213 techs 2.0/2.0, 40 1.1/1.1,
  7 1.1/2.0); the profile is `vs_%i_%i`/`ps_%i_%i`, the entry `vs_main` /
  `ps_main`, the file `materials/shaders/<file>`, compiled with the macro `PC`
  (VERIFIED, the format strings and macro in 0x001039b6). Stock passes per technique: 256 have one, two have
  two (`vertcol_mul*`'s multiply plus fog fix-up) (VERIFIED, census).
- Arguments seen in stock (VERIFIED, census): `colorMapSampler =
  material.colorMap` 226, `normalMapSampler = material.normalMap` 163,
  `detailMapSampler = material.detailMap` and `detailScale =
  material.detailScale` 83 each, `specularMapSampler = material.specularMap`
  64, `featherParms = material.featherParms` 24, `colorMapSampler =
  sampler.feedback` 14, `outdoorMapSampler = sampler.outdoor` 7,
  `cubeMapSampler = material.cubicEnvMap` 3, `waterColor =
  material.waterColor` 3, and a handful of `sampler.resolvedPostSun`,
  `sampler.resolvedScene`, `sampler.shadowCookie`, `sampler.sky`,
  `constant.materialColor`.
- A shader uniform the technique leaves unset is bound by name: first
  against `_s_defaultCodeSamplers` (0x0036c9e0), then `_s_codeConsts`
  (0x0036c020), then `_s_defaultCodeConsts` (0x0036c820); a uniform still
  unbound fails the pass (`Undefined shader constant(s) in %s`) (INFERRED,
  the lookup order in 0x001023a2). The three tables are VERIFIED (read).
  So the lighting inputs never appear in a `.tech`.
- Default samplers (VERIFIED, table): `specularitySampler` (source 3),
  `shadowCookieSampler` 12, `feedbackSampler` 13, `dynamicShadowSampler` 19,
  `floatZSampler` 21, `attenuationSampler` 17, `lightmapWeightSampler` 7,
  `lightmapSamplerR`/`G`/`B` 8/9/10, `lightmapSamplerSun` 11,
  `lightGridWeightSampler0`/`1` 5/6, `smodelLightingSampler` 4. Sources are
  STABS `MaterialTextureSource` (`TEXTURE_SRC_CODE_*`).
- Default constants (VERIFIED, table): `eyePosition`/`Forward`/`Left`/`Up`
  139..142, `nearPlaneOrg`/`Dx`/`Dy` 143..145, `lightPosition0/1` 131/132,
  `lightColor0/1` 133/134, `lightAmbient0/1` 135/136, `lightSpecular0/1`
  137/138, `lightGridColorsR0..B1` 146..151. `_s_codeConsts` also names
  `fogConsts` 156, `fogColor` 157, `materialColor` 155, `gameTime` 130,
  `baseLightingCoords` 152, `lightingLookupScale` 153,
  `outdoorFeatherParms` 178, `filterTap[8]` 163, `renderTargetSize` 171,
  `clipSpaceLookupScale/Offset` 174/175, `particleCloudColor/Matrix`
  176/177, and the matrix family 188..239 (`world`, `view`, `projection`,
  their products, each plain, inverse, transpose and inverse-transpose,
  plus `shadowLookup`, `lightGridLookup`, `worldOutdoorLookup`).

## 6. Statemaps

Grammar, read off `Material_ParseRuleSet` 0x00103020 and
`Material_ParseRuleSetConditionTest` 0x00102ed4 (INFERRED); all 27 stock
`.sm` parse with it (VERIFIED, `cod2_techparse.py`):

```
statemap := group{11}        // fixed order below
group    := groupname '{' rule+ '}'
rule     := cond ( ':' cond )* ':' body ';'
cond     := 'default' | test ( '&&' test )*
test     := srcfield '==' value
body     := 'passthrough' | value ( ',' value )*
```

- The 11 groups, in the order the loader demands: `alphaTest`, `blendFunc`
  (op, src, dst), `separateAlphaBlendFunc` (op, src, dst), `cullFace`,
  `depthTest`, `depthWrite`, `colorWrite` (rgb, alpha), `fog`,
  `polygonOffset`, `stencil` (mode, then front func, pass, fail, z-fail,
  back func, pass, fail, z-fail: nine values), `wireframe` (INFERRED, the
  call sequence in 0x001035a0 and the destination tables' entry order). The
  destination tables at 0x0036b140..0x0036ba00 are VERIFIED (read). A
  body value ORs its value into `set` and its row's mask into the clear
  word (word 0 when the row's word 0 mask is set, else word 1), a
  condition ORs into `mask` and `value` the same way (INFERRED, the
  stores in 0x00103020 and 0x00102ed4). Two rows clear more than they
  set: the rgb source factor's mask is 0x70f and the back stencil rows
  clear the front masks (VERIFIED, table). At most
  256 rules a group (INFERRED, the `0x100 <` check).
- Each rule compiles to `MaterialStateMapRule` {mask[2], value[2], set[2],
  keep[2]} (32 bytes; VERIFIED, STABS, which names the last
  `stateBitsClear`). The applier is `RB_DrawSingleTechnique` (0x000f7e66):
  starting from the material's `stateBits`, for each of the 11 groups in
  order it takes the first rule whose `(material bits & mask) == value`
  and sets `bits = (bits & keep) | set`; the test reads the material's own
  words (`+0x2c`, `+0x30`), never the copy earlier groups rewrote; no match logs `No rule in stateMap '%s' rule
  set %i matched the current material state for material '%s'` (INFERRED,
  the loop and branches in the decompile). `passthrough` keeps the group's
  bits (INFERRED).
- When the hardware lacks separate alpha blending or a blend op, the loader
  rewrites the parsed rules (INFERRED, the device-flag branches after the
  blend groups in 0x001035a0).
- Under that compile and applier, every pass of every technique each stock
  material's set names finds a rule in all 11 groups for that material's
  `stateBits` (VERIFIED, `material_stock.rs`).
- Stock conditions seen: `default`; `mtlBlendOp == Disable`;
  `mtlCullFace == Back|Front|None`; `mtlDepthTest == LessEqual` (alone and
  `&& mtlDestBlend == Zero`); `mtlPolygonOffset == 0`; `mtlSrcBlend == One
  && mtlDestBlend == Zero`; `mtlAlphaTest == Always && mtlBlendOp == Add &&
  mtlSrcBlend == SrcAlpha && mtlDestBlend == InvSrcAlpha|One` (VERIFIED,
  census). Every group of every stock statemap ends in `default` except
  `build_shadowmap`'s `cullFace`, which lists all three faces and flips
  Back and Front.
- What the common ones do (VERIFIED, the `.sm` files): `fogged`/`nofog` keep the
  material's state, turn alpha test GT0 on for an alpha-blended material
  whose test is Always, write depth only when blending is off, write alpha
  only without polygon offset, and set the alpha channel's blend to
  (Add, InvDestAlpha, One) when blending; `additive_*` (point lights) force
  (Add, InvDestAlpha, One), depth Equal (LessEqual for an opaque
  LessEqual material) and no depth write; `nocolor` (depth prepass) writes
  no colour; `multiply_fog` is the second pass of the multiply decals.

## 7. The dx7 fallback

`materials_dx7/` holds 70 techsets and 35 techniques for
`r_rendererInUse dx7` (VERIFIED, file counts). Its pass grammar replaces
the shaders with up to two `texture[n] = material.x | sampler.x`, optional
`texcoord[n] = genEyeDirCoords() | texScroll()`, pass options
`gridLighting`, `projectToInfinity`, `ambientLighting`, `objectiveGlow`,
`fogToBlack` (`_s_passOptionsDx7` 0x0036bbe0, VERIFIED, table read) and up
to eight `stage[n].rgb|.a = func(args)` combiner stages (INFERRED, the
parser in 0x0010413a and `Material_LoadPassTextureStateDx7` 0x00101bfe).
Stock files use `modulate`, `modulate2x`, `select`, `dotProduct3` and
`blendCurrentAlpha` (VERIFIED, files); the parser also takes the
`complement` and `alphaReplicate` argument modifiers (VERIFIED, strings in
0x00101bfe). A
port that targets shaders needs only the dx9 side.

## 8. Census on the stock MP maps

`cod2_mapcensus.py` over the 15 stock `maps/mp/*.d3dbsp`: lump 7 (16-byte
soups: material s16, lightmap s16, first vertex u32, vertex count u16,
index count u16, first index u32; record size VERIFIED in `R_LoadSurfaces`
0x000e4672) weighted by triangles, and every `misc_model` in lump 37's
entity text weighted by the materials its xmodel file names (VERIFIED,
census). Six `misc_model`s spell their xmodel in another case
(`prop_barrel_BLACK_snow` four times, `prop_barrel_WHITE` twice). The
Python tool on a case-sensitive extract drops them and counts 38,249
references; the engine's case-blind lookup counts 38,267 (VERIFIED,
`material_stock.rs`). The table uses the Python count; the 18 do not move
its first two rows.

World, 1,646,795 triangles, 83% of them lightmapped:

| Techset | Share | Cumulative |
|---|---|---|
| phong_replace_spec_detail | 59.0% | 59.0% |
| effect_multiply | 16.0% | 75.0% |
| phong_replace_detail | 5.9% | 80.9% |
| phong_blend_spec_detail | 5.7% | 86.5% |
| phong_replace_spec | 4.6% | 91.2% |
| phong_blend | 3.1% | 94.2% |
| phong_replace | 1.6% | 95.8% |
| phong_blend_detail | 1.4% | 97.2% |
| phong_blend_spec, phong_add, effect_multiply_nofog, phong_alphatest* , unlit_replace, default, sky, water, effect_add | 2.8% | 100% |

Static models, 38,249 material references:

| Techset | Share | Cumulative |
|---|---|---|
| ambient_alphatest | 52.3% | 52.3% |
| phong_replace_spec | 21.0% | 73.3% |
| phong_alphatest_detail | 6.3% | 79.6% |
| phong_replace_detail | 4.2% | 83.8% |
| phong_alphatest | 3.9% | 87.7% |
| phong_replace | 3.1% | 90.8% |
| phong_replace_spec_detail, phong_alphatest_spec, effect_multiply, phong_blend, ... | 9.2% | 100% |

Through section 4.1 those land on these techniques (exp fog shown; every
family has `_lfog` and unfogged twins) (VERIFIED, techset files):

| Family | Techniques | Draws |
|---|---|---|
| lightmap phong | `lmap`, `lmap_s`, `lmap_dtl`, `lmap_s_dtl`, `lmap_a*`, `lmap_add` | lightmapped world, ~83% of world triangles |
| static model probe | `lprobe_smc*` (`_s`, `_dtl`, `_a`) | cached static models |
| static model ambient | `lprobe_smc_amb_a`, `lprobe_sm_amb_a` | `ambient_alphatest` foliage, half of model references |
| dynamic probe | `lprobe*` (`_s`, `_dtl`, `_a`, `_scroll`, `_flag`) | entities: players, weapons, script models; unlit world soups |
| multiply decal | `vertcol_mul` (2 passes) | `effect_multiply` world overlays, 16% of world triangles |
| depth prepass | `zprepass`, `zprepass_atest` | every opaque phong surface |
| point light | `l_point*` | dynamic lights, additive |
| unlit / effects | `vertcol_simple`, `vertcol_shaded*`, `effect_add`, `zfeather*` | `2d`, `effect*`, fullbright |
| sky | `sky` | the sky cube |
| water | `water_l_sun` | the six water materials |

The first five carry the world and its props; with the prepass, point
lights, effects, sky and water these ten families cover every stock MP
surface but `default`, `objective`, `distortion`, `particle_cloud*` and the
post-process filters (VERIFIED, census).

## 9. What the families compute

Described off `materials/shaders/lib/*.hlsl`; no source is copied here.
`materials/shaders/` holds 215 entry files and 28 library files (VERIFIED,
file counts). Each entry file sets `USE_SPECULAR`, `USE_ALPHA`, `USE_DETAIL`, `FOG_EXP` or
`FOG_LINEAR`, `TEX_SCROLL`, `CACHED_SMODEL` and the like, then includes one
library file (VERIFIED, `lmap.hlsl`, `lmap_efog.hlsl`). Suffixes: `_s` specular, `_a` alpha output, `_dtl` detail map,
`_scroll` texture scroll, `_efog`/`_lfog` exp or linear fog in the vertex
shader, `_pfog` pixel fog, which `lib/transform.hlsl` turns on only under
`XENON` (VERIFIED, file names and defines).

Shared pieces:

- Diffuse: `colorMap` at the vertex texcoords, times the vertex colour.
  With a detail map, `detail.rgb - 0.5` sampled at `uv * detailScale.xy` is
  added to the colour's rgb before the multiply (VERIFIED, `lib/shade.hlsl`).
- Normal: the vertex shader carries `N - T - B`, `2T`, `2B` (all through
  `worldMatrix`); the pixel shader samples the normal map's `.wy` (the `PC`
  channel choice: x in alpha, y in green) and forms `N' + x*2T + y*2B`,
  i.e. `N + (2x-1)T + (2y-1)B`, normalized (VERIFIED, `lib/normalmap.hlsl`,
  `lib/transform.hlsl`). So the map's x and y are unsigned in [0,1] and z
  is an implicit 1 before normalizing.
- Sun: `lightPosition0` is the sun direction, `lightColor0` its colour,
  `lightSpecular0` its specular colour. Diffuse is `lightColor0 *
  max(0, N.L)`. Specular samples `specularMap` (rgb colour, alpha the
  exponent), forms `N.H` with H the normalized half vector of the eye and
  sun directions (per vertex, renormalized per pixel), and reads the
  strength from the code-built 2D `specularitySampler` at
  (exponent, N.H); result `lightSpecular0 * spec.rgb * strength`. The
  specular term is scaled by `lerp(0.3, 1, sunVisibility)` (VERIFIED,
  `lib/phong.hlsl`).
- Sun visibility multiplies the diffuse term; it is the dynamic shadow
  texture projected at screen coordinates times the family's own
  visibility (below) (VERIFIED, `lib/lightmap.hlsl`, `lib/lightprobe.hlsl`).
- Fog: distance is the radial length of the view space position; exp fog
  is `exp(dist * fogConsts.z)`, linear `dist * fogConsts.x + fogConsts.y`,
  both the fraction of surface colour kept (VERIFIED, `lib/fogcalc.hlsl`,
  `lib/fog.hlsl`). On PC the fogged techniques pair a fogged vertex shader
  with the unfogged pixel shader (`lmap_efog.tech` uses `lmap_efog.hlsl`
  and `lmap.hlsl`), so the factor leaves through the `FOG` output register
  (VERIFIED, files). Fixed-function fog then blends toward `fogColor`, gated
  by stateBit 0x20000000 (INFERRED, D3D9 vertex fog semantics). A WGSL port
  does `mix(fogColor, colour, f)` in the fragment stage.
- Alpha: 1 unless `USE_ALPHA`, then the diffuse alpha (VERIFIED,
  `lib/lightmap.hlsl`).

Per family:

- Lightmap phong (`lib/lightmap.hlsl`): the world's lightmap is three
  RGBA coefficient textures (R, G, B channels of four basis lights each)
  and a sun visibility texture, all at the lightmap coords. A weight
  texture (`$lightmapweights`, code-built by `Image_LoadLightmapWeights`
  0x000fd152) is sampled at the raw normal map sample (x, y) to give four
  basis weights; each colour channel is `dot(coeffs, weights)`. Sun
  visibility is the lightmap sun channel times the projected dynamic
  shadow. Output `diffuse * (lightmap + sunDiffuse) + sunSpecular`
  (VERIFIED, file).
- Dynamic probe (`lib/lightprobe.hlsl`): two cube maps
  (`lightGridWeightSampler0/1`, code-built) give eight weights for the
  world normal; colour is the dot of those with the eight per-entity
  constants `lightGridColorsR0..B1`. Sun visibility is `lightColor0.a`
  times the projected dynamic shadow. Output as the lightmap family
  (VERIFIED, file).
  `_flag` variants (`lib/lightprobe_flag.hlsl`, `lib/flag.hlsl`) move the
  vertex with three sine waves driven by `gameTime.w` along the texcoords
  and light both faces (VERIFIED, files).
- Static model probe (`lib/lightprobe_sm.hlsl`): a 3D lighting texture
  (`smodelLightingSampler`) holds the static models' baked light; the normal
  is turned by `lightgridLookupMatrix`, projected onto the cube face of its
  largest axis, scaled by `lightingLookupScale` and offset by the model's
  base coords (cached: a per-vertex `lightingCoords / 32768 + (0,0,0.5)`;
  uncached: the constant `baseLightingCoords`). rgb is the diffuse light,
  alpha the sun visibility (VERIFIED, file).
- Static model ambient (`lib/lightprobe_sm_ambient.hlsl`): no normal map,
  no specular. The 3D lighting sample at the base coords lights the texel:
  `tex * vcolor * light.rgb + tex * vcolor * lightColor0.rgb * light.a`
  (VERIFIED, file).
- Multiply decal (`mul*.hlsl`): pass 1 writes `Cm = (1 - vcolor.a) +
  tex.rgb * vcolor.rgb * vcolor.a`, alpha 1, so the vertex alpha fades the
  decal to white and the colour map's alpha is never read; the material's
  blend (Zero, SrcColor; `nofog` statemap passes it through) multiplies
  the framebuffer by it (VERIFIED, `mul.hlsl`).
  Pass 2 (`multiply_fog` statemap) adds `fogColor * (1 - f)` scaled by
  `(1 - decal)` to repair the fog the multiply darkened (the derivation is
  in the library file's header comment) (VERIFIED, `mul*.hlsl`,
  `lib/multiply_fog.hlsl`, `vertcol_mul_efog.tech`). The colour maps are
  light: the dark stains on mp_carentan's roofs and walls
  (`v_dec_darkstain_01`, `_08`) average about 170 and 200 per channel, and
  toujane's window shadows (`adobe_top`) 225 (VERIFIED, DXT endpoints
  averaged), so a multiply drawn as an alpha blend paints them near white.
  On mp_carentan 8 `effect_multiply` materials cover 26,729 world
  triangles, on mp_toujane 3 cover 4,404 (VERIFIED, census in
  `crates/cod2/tests/world_blends.rs`). vcod draws them through the
  stage path's `Multiply` variant (`fs_multiply`, `Material::vertex_multiply`)
  in one pass, fading the factor to white with the fog in place of pass 2.
- Blend layer (`phong_blend*` sets, `lmap_a*` techniques): the material's
  own SrcAlpha/InvSrcAlpha passes the `nofog` statemap (alpha test GT0
  added), and `USE_ALPHA` makes the output alpha the diffuse alpha, colour
  map alpha times vertex alpha, so the vertex alpha is the layer's weight
  over the base drawn before it (sort keys 8..11 against the base's 3)
  (VERIFIED, `nofog.sm`, `lmap_a.hlsl`, `lib/lightmap.hlsl`, census).
- Depth prepass (`zprepass.hlsl`): position only, colour 0, colour writes
  off (VERIFIED, file). `zprepass_atest` runs `vertcol_simple.hlsl` under
  the same `nocolor` statemap, so the material's alpha test cuts the depth
  (VERIFIED, `zprepass_atest.tech`).
- Point light (`lib/light_point.hlsl`): per-vertex light delta
  `(lightPosition0.xyz - worldPos) / lightPosition0.w`; Phong with the
  normalized delta as L; attenuation is a 1D texture at
  `saturate(length(delta))`; the `additive_*` statemaps make it additive
  (VERIFIED, files).
- Unlit and effects (`lib/vertex_color_simple.hlsl`,
  `lib/vertex_color_shaded.hlsl`, `lib/z_feather.hlsl`): texture times
  vertex colour; `vertcol_shaded` then lerps rgb toward `materialColor.rgb`
  by `materialColor.a`; the eye-offset variant pulls clip z and w toward the
  eye by a constant; z-feather fades alpha by the depth gap to the scene
  depth in `floatZSampler` scaled by `featherParms.x` and near the camera,
  and the outdoor variant also by `outdoorMapSampler` height (VERIFIED,
  files).
- Sky (`sky.hlsl`): direction `position - eyePosition` with w 0 so it sits
  at infinity; colour is the cube `colorMap` along it, alpha 0 (VERIFIED,
  file).
- Water (`lib/water.hlsl`): a code-generated height texture (from the water
  def, `R_LoadWaterSetup` 0x000e7082) gives a normal by finite
  differences after one parallax step; the reflection samples
  `cubicEnvMap`; base colour `waterColor * N.z` lerped to the reflection by
  its alpha, plus `pow(max(0, R.L), 64) * lightColor0` (VERIFIED, file).
  That the height texture is the one built from the water def is INFERRED
  from the def's `map` field being filled by `R_LoadWaterSetup`.

### 9.1 The lit family's code inputs

What the lightmap phong family reads besides the material's own maps, as
vcod builds it.

- `$lightmapweights` is 32 x 32 A8R8G8B8 (`Image_LoadLightmapWeights`
  0x000fd152; size and format VERIFIED, the `Image_Setup` arguments). The
  constants it reads are 0.5 at 0x326190, 1/32 at 0x326430, 1 at
  0x326188, -0.9553166 (minus acos(1/sqrt 3)) at 0x326444 and the doubles
  0.4774648 (3/(2 pi)) at 0x340cd8 and 0.75 at 0x340ce0 (VERIFIED). For
  texel (column c, row r) it takes x = 2(c + 0.5)/32 - 1, y = 2(r + 0.5)/32
  - 1 and z = sqrt(max(0, 1 - x^2 - y^2)); byte 0 is t = 1 - acos(z) /
  0.9553166 clamped to [0, 1], and bytes 1, 2 and 3 share 1 - t by the
  azimuth a = atan2(y, x) * 3/(2 pi) - 0.75 wrapped to [0, 3), linear
  between byte 1 at a = 0, byte 2 at 1 and byte 3 at 2, each byte the
  weight times 255 rounded (INFERRED, the loop's arithmetic and branches).
  So the four basis lights are straight up and three lights 54.7 degrees
  off it, 120 degrees apart, byte 1's at azimuth 90 degrees, and a flat
  normal map texel (0.5, 0.5) weights byte 0 alone, the coefficient the
  fixed-function path takes (cod2-d3dbsp-format.md, lump 1) (INFERRED,
  from the formula). The shader samples it at the normal map's raw `.wy`
  and dots it with each coefficient texel (section 9); both images are
  A8R8G8B8, so a dot over the memory bytes in the same order is the same
  sum, which is how vcod uploads them (INFERRED, from the two formats).
- `$specularity` is 32 x 256 L8 (`Image_BuildSpecularityMap` 0x00109f2c,
  called with 0 from `Image_Load` 0x000fd52a; size and format VERIFIED).
  Column i holds N.H to the power ln(0.1) / ln(cos(((180 - 175 i/30)/4)
  degrees)), the power that brings the lobe to 0.1 at that half-angle, 45
  degrees at column 0 and 1.25 at column 30; column 31 takes the power 30;
  row j is N.H = j/255 and the byte is the result times 255 rounded
  (INFERRED, the loop; constants 30 at 0x32636c, -175 at 0x340d10, 180 at
  0x340c18, pi/720 at 0x340d18, 0.1 as a float widened, 30 at 0x340d00, all
  VERIFIED).
- The sun's constants come from `R_AddCmdLightProperties` (0x000c8950):
  `lightPosition0` is the `GfxLight` direction (`AngleVectors` of
  `sundirection`) with w 0, `lightColor0` its colour with alpha 1 and
  `lightSpecular0` that colour times `r_specularColorScale` (default 1)
  (INFERRED, the copies and the multiply). The dvar's default 1.0 is in
  its registration in `R_RegisterDvars` (0x000c8f98) (VERIFIED). The colour is the one
  `R_InterpretSunLightParseParamsIntoLights` builds (cod2-d3dbsp-format.md,
  lump 1).
- `fogConsts` is (-1/(far - near), far/(far - near), -density, 0),
  written by `RB_SetIteratorFog` (0x000fab32) at `backEnd+0x1c0`, with
  `far` the fog's own unless `r_zfar` is set (INFERRED, the stores and the
  sign-bit xor). The xor mask at 0x32b780 is -0.0 (VERIFIED). So exp fog
  keeps exp(-density * d) of the colour and linear fog (far - d)/(far -
  near). `R_SetFog` (0x000d286c) takes a density of 1 or more as linear
  (INFERRED, the `1.0 <= param_7` branch).
- The Mac's ARB translation of `lmap_s_dtl.pse` (its name at file offset
  0x2abad4, the program right after it) computes what the HLSL does: the
  normal sample's alpha times `2T` plus green times `2B` plus `N - T - B`,
  normalized; the weight image read at (alpha, green); colour plus detail
  plus -0.5 times the vertex colour; the specular strength read at
  (specular alpha, `N.H`) and scaled by `0.7 * vis + 0.3`; output
  `diffuse * (lightmap + vis * lightColor0 * N.L) + specular` (VERIFIED,
  program text). No extra scale or bias sits on the normal or the detail.
  `cod2_world.wgsl` is the same sum; `renderer::cod2`'s
  `detail_and_normal_mapped_surface_matches_the_hlsl` checks one texel of a
  detail-, normal- and specular-mapped surface against it worked by hand.
- Retail on a software rasterizer agrees with vcod on a sunlit wall:
  `CoD2MP_s.exe` under Wine (wined3d on llvmpipe, default settings,
  `r_picmip` 0 on all three image classes) and vcod on lavapipe, a
  spectator parked at mp_toujane (3172, 1413, 146) yaw 215 at 928x580 with
  `cg_fov` 86.3 to match vcod's width, give the plaster wall the same mean
  (154.9 against 155.1) and the same high-pass deviation at Gaussian radii
  1, 2, 4 and 8 px (4.88/4.87, 8.35/8.28, 11.33/11.27, 13.27/13.23)
  (VERIFIED, the two captures). A gametype that only spawns a spectator at
  cvar-given coordinates and holds him there (`setOrigin`,
  `setPlayerAngles` every half second) puts both clients on one spot.

### 9.2 Model lighting: the light grid probe

What the `lprobe*` techniques read and how vcod draws them. The grid
lookup itself, its sample points and the meaning of its records are in
cod2-d3dbsp-format.md, lumps 2 and 3.

- The probe basis is the eight corners of a cube turned so that world up
  is its (1, 1, 1) corner. `lightGridLookupMatrix` (mac 0x32b7e0, and the
  same literal in `lib/lightprobe_sm.hlsl`) has the rows (0.8164966,
  -0.4082483, -0.4082483), (0, 0.7071068, -0.7071068) and (0.5773503,
  0.5773503, 0.5773503), and a world vector `v` turns to `v.x * row0 +
  v.y * row1 + v.z * row2` (VERIFIED, `_AxisTransformVector` mac 0x2bc62
  and the HLSL `mul(worldNormal, lightgridLookupMatrix)`). So corner 7
  is straight up, corner 0 straight down, corners 1, 2, 4 at a world z of
  -1/3 and 3, 5, 6 at +1/3, each three 120 degrees apart in yaw
  (INFERRED, the matrix is orthonormal).
- `$lightgridweights0` and `$lightgridweights1` are 32 x 32 A8R8G8B8
  cube maps (VERIFIED, `Image_GenerateCubemapFunction` mac 0xfcda6 calls
  `Image_Setup` with format 0x15; `Image_Load` mac 0xfd52a passes 0x20 and
  the image number 0 or 1). Each texel's direction goes through
  `Image_GetLightGridWeightsForVector` (mac 0xfc8d4): turned by the matrix,
  divided by its largest absolute component (`_Vec3MajorAxis`, which
  compares squares), each axis mapped by `t = 0.5 a + 0.5` and eased by
  `t^2 (3 - 2t)` (constants 0.5, -2, 3 at mac 0x326190, 0x3261e0,
  0x3262d8); the z term's scale is -0.5 for image 0 and 0.5 for image 1
  (mac 0x3261f4), so image 0 holds the cube's z -1 corners and image 1
  its z +1 ones. The four bytes, in memory order, are `(1 - fx) fy fz`,
  `fx (1 - fy) fz`, `(1 - fx)(1 - fy) fz`, `fx fy fz`, each times 255
  floored after adding 0.5 and clamped (VERIFIED, the function).
  Sampled as A8R8G8B8 (bytes B, G, R, A), the shader's `.x, .y, .z, .w`
  are then the corners (x, y) = (-1, -1), (+1, -1), (-1, +1), (+1, +1):
  corner `j` = bit 0 x, bit 1 y, with image 0 corners 0-3 and image 1
  corners 4-7 (INFERRED, the D3D channel order; it matches the grid's
  missing-data table, where value `j` is coloured by its own bits, and
  the census that value 7 is the brightest on 13 of 15 maps).
- Both cubes are made with `Image_Setup` flags 7 (VERIFIED,
  `Image_GenerateCubemapFunction` mac 0xfcda6): bits 0 and 1 skip picmip,
  and bit 1 becomes `Image_CreateCubeTexture`'s (mac 0xe6c9a) level count,
  `(flags >> 1) & 1` = 1, handed to the device's `CreateCubeTexture`
  (vtable + 100) as `Levels`: one 32 x 32 level a face (VERIFIED,
  `Image_Setup` mac 0x109218 and the decompile). The texel
  bytes are fixed: the function reads nothing but its direction and the
  image number, so the cubes are the same on every map (VERIFIED, the
  function). vcod builds them on the CPU the same way
  (`vcod_cod2::lightgrid::weight_cubes`, B8G8R8A8 for A8R8G8B8) and the
  entity probe stages sample them (cod2-client-perf.md 10); the
  per-vertex fallbacks keep the eased weights computed.
- `ProbeLighting` (`lib/lightprobe.hlsl`) dots the two weight samples at
  the world normal with `lightGridColors{R,G,B}0` and `..1` (VERIFIED,
  file). The result is the grid's eight colours weighted by the eased
  cube coordinates of the normal: a normal along a corner takes that
  corner alone, a level one blends the four corners around it (INFERRED,
  from the weights).
- The sun term is `lightColor0 * max(0, N.L)` times `lightColor0.a`,
  the grid's sun visibility (VERIFIED, `RB_SetupLighting` mac 0xf6b86
  passes it as the constant's w; the use is `lib/lightprobe.hlsl`). The
  colour is the map's sun light, the lightmap family's (9.1).
- Static models (`lib/lightprobe_sm.hlsl`, the `lprobe_sm*` and
  `lprobe_smc*` techniques) read their lighting from one 3D texture,
  `*smodel_lighting`, which `R_CacheStaticModelLighting` (mac 0x107512)
  fills with a 2 x 2 x 2 block per model, the corner `j` at (x, y, z) =
  bits 0, 1, 2 of `j`, each texel the three channel values and the sun
  visibility as bytes, so clamped to [0, 1] (VERIFIED, the loops and the
  clamps). `R_FinishStaticModelLightingCache` (mac 0x106f82) sets
  `lightingLookupScale` to half a texel on each axis (0.5 / width, 0.5 /
  height, 0.25 of a depth of 2) (VERIFIED). The normal pushed onto the
  cube then lands on the block's texel centres and the trilinear filter
  weights the corners linearly, without the entities' easing (INFERRED).
  The sun visibility comes from the same texel's alpha (VERIFIED, file).
  Under `r_rendererInUse` 2 (dx7) the cache keeps the floats instead
  (VERIFIED).
- What a static model caches (`R_LoadEntities` mac 0xe2b6c, VERIFIED
  unless marked): the grid at its bounds' centre, or, when its xmodel's
  flag bit 0 is set and the entity has a `gndLt` key, the ground colour:
  `gndLt` is read as `%02x%02x%02x%02x`, red, green, blue and sun over
  255, and every corner takes the rgb (`R_GetStaticModelLightingFromGround`
  mac 0x107070). Then `directLightScale` scales the sun visibility and
  `indirectLightScale` the 24 values (`R_ScaleStaticModelLighting` mac
  0x106970), each 1 without the key and an error when negative; the keys
  compare without case.
- Census: 253 `misc_model`s on mp_toujane carry `gndLt` (VERIFIED, the
  entity text); only the 32 grass and shrub descriptors set the flag
  (cod2-xmodel-v20-format.md), so the rest are lit from the grid.

What vcod draws (`crates/client/src/cod2_light.rs`, `vcod_cod2::lightgrid`):

- Entities and bodies: each instance's sample (with the sight traces
  through a `CollisionWorld` built from the map) is packed into the light
  set `dynamic_model.wgsl` reads, flagged by an ambient w of -1; a probe
  stage (`Material::probe`) is lit per pixel by `cod2_model.wgsl`'s
  `cod2_probe`, the technique of 9.4. Static props whose set has an
  `lprobe_smc*` technique draw it per pixel off a static model cache
  (9.6). The others (the `ambient_*` sets, a set with a stage that is
  neither opaque nor alpha-blended): each placement's cached sample is
  evaluated with the linear weights and the sun per vertex at load, times
  the model's vertex colour, and stored in the prop's vertex colour
  (`prop_colour`) with the model's alpha; the stage path draws it as
  `lightingDiffuse` (`entity_material::lit_stages`), so it rescales from
  identityLight 0.5 as `vs_prop` does.
- An entity surface names its material directly (`mtl_russian_body`,
  no `skins/` script and no image of that name; VERIFIED, the
  playerbody's surface list and the paks). `entity_material::load`
  takes it from the compiled library by that name. Its technique set
  carries a lightmap technique as well as the probe one
  (`mtl_russian_body`: `lmap_s` beside its `lprobe` entry; VERIFIED,
  the compiled set), and the library compiles the lightmap one first
  (4.1), so the entity path redraws a set with a probe technique
  (`ModelLight::Probe`) as its colour map times the vertex colour times
  the probe: no lightmap bundle, `lightingDiffuse`. Looking for a `skins/`
  script instead finds none and draws the default image (mean 24 of 255),
  a near-black silhouette.
- A static prop surface whose set has no probe technique (the
  `effect_multiply` decals, `effect_add` glows, `unlit_*` sets) keeps the
  colour its xmodelsurfs vertices store, which those techniques read
  (`vertcol_mul.hlsl`, `vertex_color_simple.hlsl`, above). The crates'
  stencilled labels (`mtl_dak_crate_decals` on `prop_crate_dak1`,
  `mtl_europe_crate_decals`) store 231..251 grey at alpha 255 (VERIFIED,
  the xmodelsurfs); lit by the grid instead, the label multiplies the wood
  under it by the half-scale light, a near-black rectangle. The census of every stock model material outside the opaque sets, by
  the stage pipeline each draws with, is
  `crates/client/src/renderer/prop_blends.rs`.
- Every lit model technique multiplies by the model's vertex colour:
  the techniques route `vertex.color[0] = code.color` (`lprobe_smc.tech`,
  `lprobe_smc_amb.tech`; VERIFIED, the files) and `ColoredTexturedDiffuse`
  (`lib/shade.hlsl`) is the colour map times it, the diffuse that both
  the probe and the sun's `N.L` multiply in `lib/lightprobe.hlsl` and
  `lib/lightprobe_sm.hlsl` (VERIFIED, the files). The stock models store
  far from white: over LOD 0 of every stock xmodel the `phong_*` and
  `ambient_*` sets' vertices average a red of 174 to 248 per set (106 on
  the three `phong_alphatest_scroll` materials, 134 on the `ambient_*`
  ones), 203 on the 1,602,587 vertices of `phong_replace_spec`, the
  biggest (VERIFIED, census `stock_model_sets_take_their_light` in
  `prop_blends.rs`). Dropping the colour draws a white model:
  mp_toujane's `prop_crate_dak1` body (`mtl_dak_crate`, its greys
  averaging 162) stores a mean red byte of 64.4 lit white and 42.2 with
  its greys (VERIFIED, `toujane_crate_labels_keep_the_model_colours`).
  The entity's GPU path is checked against the CPU formula on lavapipe
  (`probe_shader_lights_a_model_as_the_cpu_does`, a white and a 125 grey
  sphere).
- The `ambient_*` sets (19 stock materials on 39,687 LOD 0 vertices;
  VERIFIED, census) name `vertcol_shaded*` as their `lightprobe`
  techniques and `lprobe_smc_amb*` as their `lightprobe smodel cached`
  ones (VERIFIED, the `.techset` files). vcod marks them
  `ModelLight::Ambient` (`vcod_cod2::materials::model_light`): an entity
  draws the compiled `vertcol_shaded` stage, colour map times vertex
  colour, unlit; a static prop takes the corners' mean plus the sun's
  colour times its visibility, times the vertex colour
  (`ambient_prop_colour`). `vertcol_shaded` also lerps toward
  `materialColor` by its alpha. Besides `RB_SetMaterialColorCmd`
  (0x000d4e6a), which copies the value `R_AddCmdSetMaterialColor` (mac
  0x000c88c2, reached only through its refexport slot) queued, the
  constant's only writes are the `r_showFbColorDebug` and `sc_showDebug`
  overlays (VERIFIED, every store to 0x012d6730 in the decompile).
  `_GetRefAPI` (mac 0x000caa68) puts it at offset 0x90 of the refexport
  table (`_re`, 0x00d40980; VERIFIED, the stores), and no call in the
  decompile reads that slot through the client's copy (`PTR__re`), where
  `R_AddCmdDrawStretchPic`'s 0x94 is read nine times (VERIFIED, grep; a
  copy held in a register would hide from it, INFERRED). The constant
  sits at `_backEnd + 0x1b0` (code constant 0x9b of the array at
  0x012d5d80; `_backEnd` is 0x012d6580, its time at +0x3bc being
  0x012d693c, INFERRED). That address lies inside the 0x36e90 bytes
  `RB_InitBackendGlobalStructs` (0x000d45ca) zeroes (VERIFIED, the
  memset). So the alpha is 0 and the lerp keeps the texture times the
  vertex colour; vcod draws it so (INFERRED).
- The `objective` set names only the lightmap and `lightprobe` types
  (VERIFIED, `objective.techset`). So an `objective` material on a static
  model draws nothing: the static model types 12 and 15 find no
  technique and retail skips the draw (INFERRED, 4.1). vcod leaves those
  surfaces out of the props (`props::SkinDraw::skip`) rather than draw
  the compiled stage, the colour map added.
- Not drawn: `CG_General`'s locked lighting origin (`eFlags` 0x400). The
  dynamic shadow on the sun is 9.8's.

### 9.3 No overbright: framebuffer bytes are display values

- `R_RegisterDvars` (mac 0x000c8f98) registers `r_overbrightBits`
  (default 1, range 0..1), and its one reader is `R_EndCubemapShot`
  (mac 0x000e8b2e), which gamma-corrects a cubemap shot's pixels when the
  bits are above 0 and the device has gamma (VERIFIED, the only read of
  the dvar's pointer in the decompile).
- `R_SetColorMappings` (mac 0x000cb168) builds the device ramp from
  `r_gamma` alone: entry i is `floor(65535 * pow(i / 255, 1 / r_gamma) +
  0.5)`, or `i * 0x101` at gamma 1, with no shift (VERIFIED, the loop;
  `RB_EndFrame` 0x000d44e8 calls it when `r_gamma` or `r_ignorehwgamma`
  changed). CoD1's ramp shifts left by the overbright bits
  (cod11-gamma.md, section 2).
- No shader file declares an identityLight or overbright constant
  (VERIFIED, `lib/globals.hlsl` and a search of `materials/shaders/`), so
  every technique writes the value it computes.
- So a CoD2 framebuffer byte is what the display shows, before gamma
  (INFERRED, from the three). vcod draws CoD2 with overbright bits 0,
  identityLight 1, in full screen too (`gamma::overbright_bits`): the
  lit model stages, scaled by identityLight, and the unlit ones
  (`vertcol_*`, the colour map times the vertex colour, unscaled) both
  show the HLSL's value. The trap: CoD1's full-screen overbright ramp
  doubles every unlit model stage (the `effect_add` glows, the `unlit_*`
  and `ambient_*` entity sets, the static props' stage-path decals) and
  lets a lit colour above 1 survive where retail's framebuffer clamps it.

### 9.4 Model techniques: normal and specular maps, the objective pulse

- An entity draws its set's `lightprobe` technique: `lprobe` plus any
  of `_a`, `_s`, `_dtl` and `_scroll` (VERIFIED, the `.techset` files).
  Over the stock materials whose set has one, 915 take plain `lprobe`,
  401 `_s`, 336 `_a`, 210 `_dtl`, the rest the combinations, 15 a
  `_scroll`, and 4 `lprobe_flag_a_s` (VERIFIED, census
  `model_materials_take_their_probe_technique` in
  `crates/cod2/tests/materials_lib.rs`). The technique routes position,
  colour, texcoord, normal, and the tangent and binormal as texcoords 1
  and 2 (VERIFIED, `lprobe_s.tech`); `lprobe_a.tech` binds no specular
  map.
- `lib/lightprobe.hlsl`'s pixel shader (VERIFIED, the file): the normal
  is section 9's normal-map decode at the pixel; the light is
  `ProbeLighting` at that normal plus `lightColor0 * max(0, N.L)` times
  the sun visibility; the colour is `diffuse * light + specular`, alpha
  the diffuse alpha under `_a`, else 1. The specular is `lib/phong.hlsl`'s
  (section 9) with the half vector built per vertex from the eye and
  `lightPosition0`, times `lerp(0.3, 1, visibility)`. The probe is
  per pixel, so a normal map shades a model as it does the world.
- `TexScroll` (`lib/texscroll.hlsl`) lowers t by `gameTime.z` under
  `TEX_SCROLL` (VERIFIED, the file).
- `gameTime` is code constant 130 (0x012d65a0 = 0x012d5d80 + 130 * 16):
  `RB_RenderDrawSurfList` (mac 0x000d5630) writes it, when the surface's
  material time changes, as (sin 2 pi f, cos 2 pi f, f, t) with t the
  view's time (`RB_BeginViewCmd` 0x000d51f4 copies it from the view
  command, `backEnd + 0x3bc`) less the entity's material time (`GfxEntity
  + 0x5c`, 0 for the world) and f its fraction (VERIFIED, the stores; 2
  pi is the double at 0x340c78, 6.283185307179586). That t is seconds is
  INFERRED (`lib/flag.hlsl`'s notes; the cgame's clock).
  vcod takes the frame's clock plus the instance's material clock offset.
- `objective_base.tech` (the six techniques of the `objective` set,
  VERIFIED) routes no colour and draws under `objective.sm`: blend Add,
  SrcAlpha, One, no depth write, fog off (VERIFIED, the files).
  `objective_base.hlsl`: luminance `dot(0.333, colour map)`, facing
  `-normalize(viewNormal).z`, `k = 0.5 - sin((facing * 0.5 +
  gameTime.z) * 2 pi) * 0.5`, colour `lerp((0.25, 0.15, 0, 1), (1, 1,
  0.5, 1), k) * (luminance + 0.5)`, alpha the colour map's (VERIFIED,
  the file). D3D's view space looks down +z, so the facing is the
  normal's turn toward the eye (INFERRED); vcod reads it as minus the
  world normal along the view forward. 31 stock materials use the set,
  `mtl_tntbomb_obj` (the S&D bomb's `mp_tntbomb_obj`) among them
  (VERIFIED, census). Under the dx7 renderer `RB_DrawSingleTechnique`
  (mac 0x000f7e66) lerps `r_objectiveColorDx7Min` to `Max` by `0.5 -
  sin(2 pi frac(t)) * 0.5` into the sampler constant instead (VERIFIED,
  the code; -0.5 at 0x3261f4, 0.5 at 0x326190).
- vcod: `vcod_cod2::materials::probe_of` reads the switches into
  `Material::probe` and `model_light` marks the `objective` set
  `ModelLight::Objective`; `stage_params` turns them into the stage flags
  `cod2_model.wgsl` reads (`STAGE_FLAG_COD2_*`, `STAGE_FLAG_OBJECTIVE`),
  the detail scale riding the stage's unused eye offset. The model vertex
  carries the xmodel's tangent and binormal through the base pose
  (`model::Vertex::tangent`), skinned as the normal is. A probe stage's
  normal, specular and detail maps load raw (no texture gamma) into its
  group 3 beside `$specularity` (`EntityMaterials::param_groups`).
  `probe_stage_lights_a_normal_mapped_sphere_as_the_hlsl_does` and
  `objective_pulse_matches_the_hlsl` (`cod2_light.rs`) draw both on a
  software adapter and match a CPU evaluation of the HLSL within 2 of
  255 per channel (on mp_carentan's sunlit street sample with `COD2_DIR`).
- The view weapon: `CG_AddViewWeapon` lights it at the eye (9.2's
  entity path; the eye as its lighting origin is INFERRED, vcod's CoD1
  rig's). vcod's CoD2 view weapon draws the grid sample at the eye
  through the same `cod2_model.wgsl` (`viewmodel.wgsl`'s CoD2 arm, its
  normals turned back to the world by the view's inverse), with the
  model's vertex colour, not CoD1's static lighting.

### 9.5 The depth feather

The feather is the PC renderer's (`gfx_d3d_mp_x86_s.dll`, section 0). The Mac build registers no `r_zFeather`
and its `R_RenderScene` queues only type 21 for the emissive pass
(VERIFIED, the Mac decompile).

- `r_zFeather` is a bool registered default 1, flags 0x2000 (VERIFIED,
  the push of 1 before its name at 0x1000b4c1, stored to 0x101ce694).
  `r_depthPrepassModels` is a bool default 0 (VERIFIED, 0x1000b339).
- At init, when the renderer is not the dx7 one (`r_rendererInUse` != 2)
  and `r_zFeather` is set, it makes the `floatZ` render target, format 0x72
  (`D3DFMT_R32F`), and keeps it at 0x101d489c (VERIFIED, 0x10023163 to
  0x10023216). Turning the dvar on later prints "Because r_zFeather was 0
  during renderer initialization" (VERIFIED, string at 0x10196a68,
  pushed at 0x10022297).
- 0x10024bc0 returns the emissive pass's type: 0x18 (24, emissive
  feathered) when `r_zFeather` and `floatZ` are both set, else 0x15 (21)
  (VERIFIED, the function's bytes). So a stock PC client draws every
  effect through its set's "emissive feathered" technique. Every stock
  set with an emissive technique names one (VERIFIED, census below), so
  none drops out of the pass.
- `floatZ` holds the clip z of the nearest opaque surface: `build floatz`
  writes `position.z` of `worldViewProjectionMatrix` (VERIFIED,
  `lib/float_z_build.hlsl`), after a clear to `262144 * 2 * 1.732`
  (VERIFIED, `floatz_clear.hlsl`). Models skip that pass under the default
  `r_depthPrepassModels 0` (INFERRED, 4.1's branch), so the world and its
  brush models fill it.
- The projection is `InfinitePerspectiveMatrix` (Mac 0x0002b7a6; the PC
  renderer's twin at 0x1005c000 stores the same 0x3f7fdf40 at `[2][2]`):
  `[2][2]` = 0.99950027, `[2][3]` = 1, `[3][2]` = 0.99950027 * -near
  (VERIFIED, rodata 0x003261f8), near being `r_znear`, default 4, min
  0.001, max 16 (VERIFIED, 0x1000acef's pushes). So clip z is
  `0.9995 * (view depth - 4)`, linear in depth, and clip w the view depth
  (INFERRED, the matrix).
- `lib/z_feather.hlsl` (VERIFIED, file): the vertex shader keeps
  `tz = clip.z * featherParms.x` and multiplies the vertex alpha by
  `saturate(tz)`, a fade over the first `1 / featherParms.x` units past
  the near plane; the pixel shader reads `floatZ` at the projected screen
  position and multiplies the output alpha by
  `saturate(abs(floatZ * featherParms.x - tz))`. With 0.025 a sprite
  fades out over the last 40 units before the surface it cuts. The
  `_outdoor` variant also multiplies by `saturate((pos.z - outdoor
  height) * outdoorFeatherParms.x)`, the height read from the code-built
  outdoor map (VERIFIED, file).
- Only alpha fades: `zfeather.tech` passes `nofog` (the material's own
  blend) and `zfeather_add.tech` `zfeather_add_nofog`, which forces
  `Add, SrcAlpha, One` (VERIFIED, files). A blend that reads no source
  alpha would show no feather; no stock feathered material has one
  (INFERRED, census blends).
- Census over the stock materials (VERIFIED, `feather_census.rs` with
  `COD2_DIR`): 33 materials feather, 225 other emissive ones do not.
  `effect_zfeather` 10, `effect_zfeather_nofog` 12, `effect_zfeather_add`
  2, `effect_zfeather_add_nofog` 8, `effect_zfeather_outdoor` 1;
  `featherParms.x` 0.025 on 20 of them, the rest 0.0125, 0.1, 0.2, 0.25
  and 1/3. `gfx_mist` (`fx/dust/dust_wind_eldaba.efx`, mp_carentan's
  street dust) is `effect_zfeather` at 0.025. The other emissive sets
  (`effect`, `effect_add`, `effect_nofog`, `effect_multiply*`,
  `effect_add_eyeoffset`) name their plain technique for type 24 as
  well; `distortion` and `particle_cloud_outdoor` name
  `distortion_floatz` and `particle_cloud_outdoor`, which vcod does not
  draw by their HLSL either way (9.11, 9.13).
- vcod: `pick_technique` takes "emissive feathered" first, and a
  `zfeather*` technique sets `Material::feather` to `featherParms.x`. The
  fx pass (`renderer::fx_feather`, `fx.wgsl`) carries it on the quad
  when the stage's blend reads source alpha, folds the near term into
  the vertex alpha, and reads `soft_particles`' linear copy of the MSAA
  depth (sample 0, taken after the last opaque or decal draw) for the
  gap, `0.9995 * |scene - sprite| * featherParms.x`; the near offsets
  cancel. A frame with a feathered run splits for the copy whatever
  `vc_softParticles` is.
  `fx_feather::feathered_sprite_fades_into_the_wall_as_the_hlsl_does`
  draws a sprite through a slanted wall with the fx pass's own
  pipelines on a software adapter and matches a CPU evaluation of the
  HLSL within 0.5 of 255.
- With a feathered run in the frame the copy goes before the first
  entity or view weapon surface in the sorted list
  (`renderer::feather_copy_at`, never past the split), so the feather
  reads the world, not the players and entities: retail's `floatZ` leaves
  players out, so a sprite cuts hard against one. Static props are world draws in vcod and stay in the copy;
  whether retail's static models write `floatZ` is open
  (`r_depthPrepassModels` gates "models", 4.1).
- The feathered outdoor term is not drawn. One stock material,
  `gfx_rain_mist` (`fx/misc/rain_mist.efx`), takes
  `effect_zfeather_outdoor`, and four snowflake clouds take
  `particle_cloud_outdoor` (VERIFIED, `mats.json` census); no stock MP
  map's d3dbsp or `maps/mp/` script names `rain_mist` (VERIFIED, grep of
  every iwd), so no stock MP level reaches the feathered term. The
  clouds' test is ported (9.14). The height map comes from
  `R_GenerateOutdoorImage` (mac 0x00108460, downward traces into a
  0x40000-byte image over the bounds `R_RegisterOutdoorImage` mac
  0x00108248 gathers from the world's surfaces; VERIFIED, the decompile),
  with `r_outdoorFeather` default 8, `r_outdoorAwayBias` 32 and
  `r_outdoorDownBias` 0 (VERIFIED, the registration pushes 0x41000000,
  0x42000000, 0).

### 9.6 Static models per pixel: `lprobe_smc*`

- A probe set's static model technique (`lightprobe smodel cached`, type
  15) is `lprobe_smc` plus the same `_a`, `_s` and `_dtl` switches as its
  `lightprobe` one; the `_scroll` and flag sets' cached technique carries
  the switches without them (VERIFIED, census
  `model_materials_take_their_probe_technique`, every stock probe set).
  `lprobe_smc_s.tech` routes the normal and specular maps, the tangent
  and binormal, and `code.texcoord[1]` as texcoord 3, the lighting
  coords (VERIFIED, file).
- `lib/lightprobe_sm.hlsl` under `CACHED_SMODEL` (VERIFIED, file): the
  pixel's normal is the normal-mapped one (`Normal_GetWorldSpace`, 9.4);
  `ProbeLighting` turns it by `lightgridLookupMatrix`, divides by its
  largest absolute component, scales by `lightingLookupScale`, adds
  `baseLightingCoords` (the lighting coords / 32768, z plus 0.5) and
  reads `smodelLightingSampler` there; the light is that rgb plus
  `lightColor0 * max(0, N.L)` times the texel's alpha, the specular
  `lib/phong.hlsl`'s times `lerp(0.3, 1, alpha)`. So a static model is
  shaded per pixel with its normal and specular maps; only the probe
  colours are cached, per model.
- `R_CacheStaticModelLighting` (mac 0x00107512) stores each value as
  `floor(v * 255 + 0.5)` clamped to 0..255, the sun visibility in the
  texel's first byte and the three colour rows after it (VERIFIED, the
  stores). The D3D A8R8G8B8 read puts the first byte in alpha (INFERRED,
  9.2's channel order).
- vcod (`cod2_light::SmodelCache`, `cod2_world.wgsl`'s `fs_smodel`,
  `renderer::cod2::Cod2Builder::bind_smodel`): the cache is two RGBA8
  layers (the z -1 corners, the z +1 ones), 128 blocks a row by
  placement index, a vertex's lightmap coords its block's centre. The
  lookup moves half a texel per axis along the pushed normal and blends
  the layers by its z, as the 3D texture's trilinear filter would. A
  prop of a probe set whose stages are all opaque or alpha-blended draws
  through the CoD2 lit pipeline with its model's colour and tangent frame
  (`props::Props::vert_tangent`); the rest keep 9.2's per vertex bake.
  `static_model_shades_its_cached_block_as_the_hlsl_does`
  (`renderer/cod2.rs`) draws a normal and specular mapped surface on a
  software adapter against the HLSL evaluated on the CPU over the cache's
  bytes, within 2 of 255 (159, 94, 56 against 158.8, 94.0, 56.3), its
  neighbouring blocks white so a lookup that leaves its block shows.

### 9.7 Flags: `lprobe_flag*`

- Four stock materials take `phong_flag_alphatest_spec`
  (`mtl_flag_american`, `_british`, `_german`, `_russian`; VERIFIED,
  `mats.json`), whose `lightprobe` technique is `lprobe_flag_a_s`. Its
  fogless `lightprobe smodel` type names `lprobe_flag_sm_a_s` ("smodels
  don't support the flag stuff yet ... ok this one does, but only this
  one"), its cached type `lprobe_smc_a_s` (VERIFIED, the `.techset`).
  The models are `prop_flag_*`, their `_carry` ctf flags and the Higgins
  boat (VERIFIED, grep of the xmodels).
- `Flap` (`lib/flag.hlsl`, VERIFIED, file) runs on the model-space
  vertex before `worldMatrix`. With `U = 1 - s` and `V = 1 - t`, three
  waves (scale 1.73, 0.75, 0.31; angle 4, -10, 15 degrees; speed 0.9,
  1.5, 1.2; amplitude 1.5, 0.6, 0.34) at phase `2 pi / scale * (cos a *
  U + sin a * V) + 2 pi * gameTime.w * speed` push the position along the
  normal by `5 * vertex alpha * sum(amp * sin)`. The normal tilts by
  `atan(sum(amp * k * cos) / 2 pi) * alpha` along the tangent and the
  binormal, its normal part `sqrt(1 - nu^2 - nv^2)`; the colour handed on
  is the vertex's `rbg` with alpha 1. The Mac's `lprobe_flag_a_s.vsa`
  takes that root as `RSQ` then `RCP` (VERIFIED, the embedded ARB text
  at file offset 0x25fc04), which read the size, so a slope past 45
  degrees gives a number rather than NaN. The sun's diffuse is
  `Phong_DoubleSidedDiffuseLighting`, `max(-N.L, N.L)` (VERIFIED,
  `lib/lightprobe_flag.hlsl`).
- vcod: `ProbeLit::flag` marks the technique, `stage_params` sets
  `deform_kinds[3]` to `DEFORM_COD2_FLAG`, and `dynamic_model.wgsl`
  runs `cod2_flap` (`cod2_model.wgsl`) before the entity's transform and
  the two-sided sun per pixel. `flag_flap_matches_the_hlsl`
  (`cod2_light.rs`) checks the offset and the normal at 8191 texcoord and
  alpha samples against the HLSL on the CPU within 1 of 255 (the offset
  spanning more than 10 units), the colour swizzle and the two-sided sun
  on lavapipe. A static flag prop draws its cached `lprobe_smc_a_s`
  unwaved, as retail's cached type does.

### 9.8 Shadow cookies

Retail's PC renderer lets the scene's model entities cast soft sun shadows
on the world, one 128 x 128 "cookie" each; vcod ports it
(`crates/client/src/renderer/cod2_cookie.rs`, `cod2_cookie.wgsl`). The
Mac build dead-strips the front end (its `R_AddCmdShadowCookie`,
`R_ShadowCandidatePred` and `R_DynamicShadowType` symbols sit at address 0,
VERIFIED, `llvm-nm`), so the logic below is read off
`gfx_d3d_mp_x86_s.dll` (a Ghidra decompile, unnamed functions by address)
and the Mac back end.

The dvars.

- `R_RegisterDvars` (Mac 0x000c8f98) registers `sc_enable` bool 1
  (flags 0x2000), `sc_blur` 2 (0..4), `sc_count` 24 (0..24),
  `sc_debugCasterCount` 24, `sc_debugReceiverCount` 24, `sc_showOverlay`
  0, `sc_showDebug` 0, `sc_wantCount` 12 (0..24), `sc_wantCountMargin` 1,
  `sc_fadeRange` 0.25 (0..1), `sc_shadowInRate` 2 (0..20),
  `sc_shadowOutRate` 5 (0..20), `sc_length` 400 (1..2000),
  `sc_offscreenCasterLodBias` 0 and `sc_offscreenCasterLodScale` 20, all
  but `sc_enable` flags 0x2080 (VERIFIED, the decompile; the DLL pushes the
  same values at 0x1000b57d..0x1000b729 and stores the dvar pointers at
  0x101ce7cc `sc_enable`, 0x101ce838 `sc_blur`, 0x101ce7b0 `sc_count`,
  0x101ce818 `sc_debugCasterCount`, 0x101ce640 `sc_debugReceiverCount`,
  0x101ce720 `sc_wantCount`, 0x101ce734 `sc_wantCountMargin`, 0x101ce774
  `sc_fadeRange`, 0x101ce620 `sc_shadowInRate`, 0x101ce7d8
  `sc_shadowOutRate`, 0x101ce618 `sc_length`, VERIFIED, the stores after
  each call). Nothing in the DLL reads 0x101ce7b0, so `sc_count` does
  nothing; the 24 is a constant (VERIFIED, no reader in the disassembly).
- `configure_mp.csv` (`iw_00.iwd`) overrides `sc_enable` per GPU on a first
  run or a changed machine (`Com_SetRecommended` mac 0x0002f086, called
  when `com_recommendedSet` is clear or the file's checksum changed;
  VERIFIED, the decompile): 1 on `GeForce 6*`, `GeForce 68*`, the
  catch-all `GeForce`, `Radeon 9*`, `Radeon X*` and the catch-all `Radeon`
  rows, 0 on older cards and the `*` row; `r_zfeather` 1 on the same later
  rows (VERIFIED, file). So a stock PC client on a current NVIDIA or AMD
  card draws the cookies (INFERRED, the rows' wildcards matching a current
  card's name). vcod registers the dvars on a CoD2 install at the
  renderer's defaults, `sc_enable` 1.

Which entities cast.

- With `sc_enable` set (and the renderer not in its dx7 mode, dvar
  0x101ce710 != 2), the scene's frame (0x10025770) builds the candidate
  list (0x100280f0), sorts it by `R_ShadowCandidatePred` (0x10027560:
  `a.ratio < b.ratio`) and picks the cookies (0x10028330) (VERIFIED, the
  decompile).
- A candidate is a scene entity of `reType` 0 without renderFx 8 and whose
  renderFx `& 0xe00` is not 0x200; its ratio is its distance from the view
  origin over its bounds' longest side (the largest of the three by
  square), times 0.25 for `& 0xe00 == 0x600`, times 4 for 0x400, 0 for
  0x800; a zero side never casts. The 24 smallest ratios under 1e8 are
  kept (VERIFIED, 0x100280f0). `CG_Player` and `CG_Corpse` add their body
  with renderFx 0x80, `CG_Missile` 0x40 over its weapon's flags, the view
  weapon 0x8c (VERIFIED, Mac 0x001e6834, 0x001e78ea, 0x001646da,
  0x001d9c8e). Players, corpses, script models, items, missiles and
  model movers cast and the view weapon does not (INFERRED from those
  flags; no cgame path read sets 0x200, 0x400 or 0x800).
- The pick (0x10028330) walks the sorted list against a cut-off ratio
  (`shadowCookieGlob`, 10 at a map load: `R_ResetShadowCookies` Mac
  0x000f99fc). An entry under the cut-off gets a cookie, faded by
  `(cutoff - ratio) / (cutoff * sc_fadeRange)` past
  `(1 - sc_fadeRange) * cutoff` (1 before it or at ratio 0). At the first
  entry past the cut-off, index `i`: `i > sc_wantCount + margin` lowers
  the cut-off by `sc_shadowInRate` a second, `i < sc_wantCount - margin`
  raises it by `sc_shadowOutRate`; 24 under it lowers it, an empty slot
  leaves it; the step is the refdef time since the last frame in seconds,
  the floor 0.0001 (VERIFIED, the decompile). vcod ports the list, the
  sort and the pick (`CookieClock`); a candidate's box there is its parts'
  posed box (each bone's vertex box under its skin matrix), not the DObj
  bounds call retail makes (INFERRED to agree on the longest side).

The cookie.

- The light frame (0x100275a0): forward is minus the world's
  `sunLight.position` (the DLL reads world + 0xb8, `GfxWorld.sunLight` at
  byte 180 plus `position` at 4 in the Mac STABS; VERIFIED both), the up
  hint world z unless forward's `x^2 + y^2 < 0.1`, then x; side
  `normalize(up x forward)`, up `forward x side` (VERIFIED).
- The view (0x10027680) projects the caster's world box (`0x10027ac0`
  reads the entity's bounds at +0x14, VERIFIED): x along minus the side, y
  along up, each over the box's 8 corners scaled by `2 / (max - min) *
  0.9375` about their middle; z the depth along forward about the box's
  middle over `sc_length`. Its projection is diag(0.984375, 0.984375,
  0.5) with z offset 0.5, so the clip x, y reach 0.9228 and the depth is
  0.5 at the box's middle, 0 and 1 one `sc_length` before and past it
  (VERIFIED, the constants 0.9375 at 0x1019bf80 and 0x3f7bffff, 0.5,
  0.5 written at 0x100278bb..0x100278d3).
- `shadowLookupMatrix` is that view-projection with its y negated and x
  and y mapped by 0.5 onto 0..1 (VERIFIED, 0x10027ac0's loops over the
  copy); `R_AddCmdShadowCookie` (cmd 9, 0x10021ee0) carries it and the
  fade, which `RB_SetShadowCookieCmd` (Mac 0x000d5160) puts in the
  matrix constant and the fourth float of the constant after the cookie
  size (`shadowParms.a`, INFERRED from the shader reading
  `shadowParms.a`).
- The cookie is `$shadow_cookie`, 128 x 128 `D3DFMT_A8R8G8B8` (0x15) with
  a 128 x 128 D24S8 depth, and `$shadow_cookie_blur` the same (VERIFIED,
  0x10022f80, 0x10023020, 0x10022cc0). Per cookie (0x10027d20): set the
  matrix and fade, clear colour and depth to (0.5, 0.5, 0.5, 0) and 1, set
  the viewport to 1, 1, 126, 126, draw the caster's surfaces with
  technique 31 (`shadowcookie caster`), set it back to 0, 0, 128, 128,
  blur when `sc_blur` is set (cmd 28), then draw the receivers with
  technique 32; a cookie draws only when it has caster and receiver
  surfaces and its index is under `sc_debugCasterCount`, its receivers
  only under `sc_debugReceiverCount` (VERIFIED, the decompile and the
  command ids of `RB_RenderCommandTable`, Mac 0x0036ace0). The viewport
  scales the already shrunk projection again, so the caster lands
  `62.016 * x` texels off the middle where the lookup reads `63 * x`
  (INFERRED, the arithmetic; vcod draws it the same way).
- `shadowcookie_caster.hlsl` writes the lookup's z in rgb with alpha 1:
  the cookie holds the casters' nearest depth along the sun, alpha 1 under
  them and 0 elsewhere, the outer texel ring always cleared (VERIFIED,
  the shader and the viewport).
- `RB_BlurShadowCookieCmd` (Mac 0x000d98bc) runs `sc_blur` rounds of two
  passes: copy the target to the blur image, then stretch it back over
  texels 1..127 with texcoords 1.5/128..127.5/128, then 0.5/128..126.5/128
  (VERIFIED, the decompile, `FLOAT_003261e4` = 2.0). Under GL's pixel
  centres the first pass reads half a texel up and left, the second half a
  texel back (INFERRED). `shadowcookie_blur.hlsl` averages four bilinear
  taps one texel left, right, up and down (VERIFIED).

The receivers and the dynamic shadow.

- The receivers (0x10027980) are this frame's visible draw surfaces whose
  technique set has technique 32, not of the caster's own entity, and
  either world triangles (`SF_TRIANGLES`, type 6, Mac STABS) or an entity
  with renderFx 0x100, whose bounds pass five planes off the cookie's view
  (x and y within the clip square, depth past the box's middle) (VERIFIED,
  the decompile and the plane table at 0x101904a8). No cgame path read
  sets 0x100, so only BSP surfaces receive (INFERRED). The
  `phong_replace*` sets, `ambient_replace` and `phong_add` name technique
  32, every other set none (VERIFIED, `cookie_census`: 1,885 materials).
- They draw into render target 3, `$dynamic_shadows`, screen sized
  (0x10023110; RT 3 is `dx + 0x2c30 + 3 * 0x14`, the slot 0x101d485c the
  image pointer goes to), cleared white before the first cookie's
  receivers (VERIFIED, 0x10027d20's `SetRenderTarget(3)` and clear).
  `shadowcookie_receiver.sm` blends Zero, InvSrcColor with LessEqual, no
  depth write, polygon offset 2 (VERIFIED), and the shader's colour is
  `alpha * shadowParms.a * saturate((z - cookie.z) * 64 + 0.5)` (VERIFIED):
  each cookie multiplies the pixel by one minus that.
- `lib/lightmap.hlsl`, `lib/lightprobe.hlsl` and `lib/lightprobe_flag.hlsl`
  multiply the sun's visibility by `tex2Dproj( dynamicShadowSampler,
  screenCoords )`; `lib/lightprobe_sm.hlsl` (static models) does not
  (VERIFIED, the HLSL). `RB_GetTextureFromCode` (Mac 0x000f66d6) hands
  code sampler 0x13 (`dynamicShadowSampler`, 19 in 5's default table)
  render target 3's image under `sc_enable`, else `rgp + 0x1008`, and
  sampler 0xc (`shadowCookieSampler`) render target 4's, the cookie
  (VERIFIED, the decompile; that `rgp + 0x1008` is white INFERRED). So
  a cookie takes only the sun's diffuse and specular, never the
  lightmap's own light, and a model reads the world surface behind it on
  screen: its own shadow on the wall behind darkens a body, a body in
  front of a shadowed floor too (INFERRED, the depth the receivers test
  against is the world's prepass, `r_depthPrepassModels` 0).

vcod's port.

- The 24 cookies are tiles of one 768 x 512 atlas, drawn and blurred as
  above; every receiving world surface (the world's and the moved brush
  models' visible ranges) draws once into vcod's dynamic shadow with
  every cookie's factor multiplied in the shader, ahead of the main pass:
  over the world's depth prepass, LessEqual with no write, as retail's,
  when `r_vc_depthprepass` is on with one sample, else the nearest
  receiver by a depth of its own (cod2-client-perf.md 11). Each world
  soup tests only the cookies its own box reaches, and over the prepass
  a soup no cookie reaches is not drawn, as retail's per-cookie lists
  leave it out (cod2-client-perf.md 12). It holds retail's (a fresh target
  is made white, a cleared one cleared white). `cod2_world.wgsl`'s
  lightmapped surfaces and `dynamic_model.wgsl`'s probe-lit models take
  the sun times it, read through the screen coordinates their vertex
  writes, as `tex2Dproj` (cod2-client-perf.md 10).
- Measured on lavapipe against a CPU evaluation (`cod2_cookie` tests): a
  caster's texels hold its depth to a byte, each blur pass matches the
  CPU's to a byte, the dynamic shadow over two overlapping cookies to two
  bytes.
- Not ported: the view weapon reads no dynamic shadow; non-receiving world
  surfaces (the alpha-tested sets) do not hide a receiver behind them in
  the dynamic shadow; the receiver surface cull and the "has receivers"
  check (per pixel nothing outside a cookie changes anyway); the casters'
  offscreen LOD dvars (a caster draws at its drawn LOD and casts nothing
  past its last); `sc_showOverlay` and `sc_showDebug`; retail's 8-bit
  blend per cookie (vcod multiplies in float, one rounding).

### 9.9 The glow: `r_glow`

- The PC renderer registers `r_glow` and `r_distortion` as bools, default
  1, flags 0x2001 (VERIFIED, `gfx_d3d_mp_x86_s.dll` 0x1000b757..0x1000b782:
  push 0x2001, push 1, push the name at 0x10192810 / 0x10192818), and the
  floats `r_glowRadius0` 5, `r_glowRadius1` 12, `r_glowSkyBleedIntensity0`
  0.5, `r_glowSkyBleedIntensity1` 0, `r_glowBloomIntensity0` 1,
  `r_glowBloomIntensity1` 1, `r_glowBloomCutoff` 0.9 and
  `r_glowBloomDesaturation` 0.75 (VERIFIED, the default pushed before
  each name, 0x1000b799..0x1000b87e), and `r_blur` 0 (VERIFIED,
  0x1000b743). `configure_mp.csv` sets `r_glow` 1 on the same GeForce 6
  and later and Radeon X rows as `sc_enable` (VERIFIED, the file's
  `r_glow` column). So a current card runs the glow (INFERRED, 9.8's
  wildcards).
- Every stock MP map's `maps/mp/<map>.gsc` sets
  `r_glowbloomintensity0`/`1` (0 on mp_decoy, 0.25 on mp_breakout,
  mp_carentan, mp_rhine and mp_toujane, else 1) and
  `r_glowskybleedintensity0` (VERIFIED, the scripts). `GScr_SetDvar`
  (Mac 0x0019cf90) sets the dvar in the server's process, adding flag
  0x400 only on a third argument (VERIFIED, the decompile). The Linux
  dedicated server holds no `r_glow` string (VERIFIED, `strings`). So a
  client of a dedicated server keeps the registered defaults (INFERRED).
  vcod draws the defaults.
- `R_InitRenderTargets` (Mac 0x000ea47c) makes target 2 (the resolved
  scene) at full size and targets 8 to 12 at a quarter of the width and
  height, `vidConfig >> 2` (VERIFIED, the shift 2 passed to
  `R_InitFullscreenRenderTargetImage` 0x000ea2e2).
- `RB_ApplyLatePostEffectsCmd` (Mac 0x000da2f6), with `r_glow` set and
  `r_fullbright` clear, copies the back buffer into target 2, sets
  `glowSetup` to (cutoff, `1 / (1 - cutoff)`, 0, desaturation) and takes a
  level's radius when its bloom intensity is above 0, or the level has a
  sky and its sky bleed is above 0 (VERIFIED, the code). The radii go
  smaller first; a swap flag records it and picks which level's
  intensities the first apply reads (VERIFIED).
- `RB_GlowFilterImage` (0x0010654a) scales both radii by target 9's width
  over target 2's, blurs level 0 from the resolved scene into target 9
  (`RB_ApplyGlowFilter` 0x001060de: a first `glow_setup` pass when the
  source is target 2), then, with a second radius, blurs target 9 again by
  `sqrt(r1^2 - r0^2)` into target 9 (VERIFIED, the calls and the
  square root). Both levels' images are target 9's (`backEnd + 0x4d4` and
  `+ 0x4d8` store the same `dx + 9 * 0x14 + 0x2c30`), so both applies read
  the wider blur (VERIFIED, the stores). Passes ping-pong targets 11 and
  12 and the last writes target 9 (VERIFIED, `local_24` = {0xb, 0xc}).
- `RB_GaussianFilterPoints1D` (0x00105878): sigma is the radius in
  destination texels (weight `exp(-0.5 x^2 / r^2)`); each half tap pairs
  texels `2i` and `2i + 1` (offset half a texel when the source is an even
  multiple of the destination) into one bilinear read at their weighted
  position over the source size, the centre texel's weight halved as both
  the + and - reads take it; the weights are normalised to 0.5 when they
  sum past 0.001, and the count is cut after the first one under 0.01,
  which stays (VERIFIED, the loop; 0.5 at 0x326190, 0.01 at 0x3262f0,
  0.001 at 0x326210).
- `RB_GenerateGaussianFilterChain` (0x00105d0a) on a same-size pass: done
  when both radii are under 0.3295 (0x326454); one 2D pass when they are
  within 0.3295 and their mean is at most 1.38956 (0x32644c); else a 1D
  pass along the wider axis (y on a tie), which takes all of it under
  6.49775 (0x326458) or leaves `sqrt(r^2 - 42.2208)` (0x32645c); at most
  16 passes (VERIFIED, the branches and constants). That a 1D pass blurs
  by the part it takes, `min(r, 6.49775)`, is INFERRED (the quadrature
  sum; the decompile hides the register). `RB_GenerateGaussianFilter2D`
  (0x00105ac0) takes two half taps per axis into eight (s, t, 0, wx * wy)
  taps over all four quadrants, drawn with `filter_symmetric_8`; a 1D pass
  draws `filter_symmetric_<count>` from `rgp + 0x10ac + 4 * count`
  (VERIFIED, the material slots; the DLL's table at 0x1018e988 names
  `rgp`'s materials in the Mac's offset order, `$default` at 0x102c and
  `shellshock` at 0x10d0 on both).
- The applies: one per level, full screen into the back buffer, reading
  target 9; `glowApply` is (sky bleed or 0 without a sky, 0, 0, bloom),
  and the material `glow_apply_sky_bleed` when the sky bleed is not 0,
  else `glow_apply_bloom` (VERIFIED, `rgp + 0x10d8` / `+ 0x10dc`). Their
  blends are One, One and One, InvSrcAlpha (VERIFIED,
  `fallback_sets_are_classified_and_code_blends_match`).
- That `renderTargetSize` is the bound target's size is INFERRED
  (`RB_SetRenderTarget` 0x000ce9ce stores the target's size).
- The HLSL (VERIFIED, the files): `glow_setup` reads four bilinear taps
  half a `renderTargetSize` texel off each axis; a tap whose luminance (0.299, 0.587, 0.114) reaches the
  cutoff gives `1 + (c - 1) * rescale`, else 0, times its non-sky fraction
  `saturate(4 * max(scene.a, postSun.a))`, alpha the sky fraction; the
  mean is desaturated by `glowSetup.w`. `filter_symmetric_*` sums `w *
  (tex(uv + o) + tex(uv - o))`. `glow_apply_bloom` writes `blur.rgb *
  bloom`; `glow_apply_sky_bleed` samples the sky cube along the pixel's
  near-plane ray and writes alpha `saturate(2 * blur.a) * skyBleed`,
  colour `sky * alpha + blur.rgb * bloom`.
- With the defaults on a map with a sky, level 0 (radius 1.25 quarter
  texels at a width divisible by 4) is one 2D pass after the setup and
  level 1 a vertical and a horizontal 1D pass; the first apply bleeds the
  sky at 0.5 and adds the blur, the second adds it again (INFERRED, the
  above; `default_plan_is_two_levels`).
- The Mac flips the last pass's texcoords when the pass count is even, and
  scales every texcoord by size over the next power of two (VERIFIED,
  the code); both are its OpenGL layer's (INFERRED) and vcod takes
  neither.
- vcod (`crate::cod2_glow`, `cod2_glow.wgsl`): the same plan on every CoD2
  map, after the sun shafts and before `vc_bloom`, on the resolved world.
  The sky writes alpha 0 as `sky.hlsl` does, which the setup reads as sky;
  the world's opaque passes write 1. vcod has no post-sun copy: the
  scene's own alpha stands for `max(scene.a, postSun.a)`, which differs
  where an effect over the sky raised retail's alpha (vcod's additive
  effects leave it) or a blend lowered it over a surface (it stays above
  0.25 there, so the fraction saturates either way; INFERRED).
  `glow_chain_matches_the_hlsl` runs the default plan on a 96x64 scene of
  bright, dull and sky texels on a software adapter against the HLSL on
  the CPU with every target stored in bytes: within 4 of 255, 99% within
  2 (one step in the quarter target shows twice, both applies reading it).
  Not taken: the D3D9 half-texel offset of a full-screen quad (open).

### 9.10 The shellshock material

- The renderer's code materials are `rgp` slots registered off the
  DLL's name table at 0x1018e988 (VERIFIED, the table): `$default`,
  `stencil_plane`, `white`, `$additive`, `$glare_blind`, `$point`,
  `$line`, `clear_alpha_stencil`, `shadowclear`, `shadowcookieoverlay`,
  `shadowcookieblur`, `shellshock`, `color_channel_mixer`,
  `frame_color_debug`, `frame_alpha_debug`, `feedbackblend`,
  `feedbackreplace`, `glow_setup`, `glow_apply_bloom`,
  `glow_apply_sky_bleed`, `filter_symmetric_1` to `_8`.
- The DLL's render command 10 (0x10031180, CoD1's `RB_BlendSavedScreen`)
  draws the saved screen full screen with `shellshock` (slot 0x101d1bd0),
  the saved image as its feedback sampler (0x101d48ac into 0x1098435c),
  white with alpha `byte << 24 | 0xffffff`, the byte 255 times `0.01`
  raised to the time since the save over the decay, capped at 0.99
  (VERIFIED, the disassembly; the Mac's `RB_BlendSavedScreenCmd` 0x000d968e
  builds the same colour byte-swapped). So CoD2's screen blend is CoD1's
  capture and blend (cod2-hud.md 16) through this material.
- `shell_shock.hlsl`: the feedback times the vertex colour, its rgb
  `lerp(c, dot(c, (0.299, 0.587, 0.114)), 0.25)`, alpha the vertex
  alpha; the `shellshock` material blends SrcAlpha, InvSrcAlpha (VERIFIED,
  the file and `fallback_sets_are_classified_and_code_blends_match`).
- vcod: `screen_blend` takes the capture a quarter of the way to its
  luminance on a CoD2 map (`COD2_DESATURATE`).
  `cod2_blend_desaturates_the_capture_as_the_hlsl_does` matches the HLSL
  on a software adapter within 1 of 255.

### 9.11 Sets no surface draws

`materials::fallback_use` classifies every set that takes the colour-map
fallback; only the visible ones log a warning (9.13). The rest:

- Drawn by vcod another way: `glow_*` and `filter_symmetric_*` (9.9),
  `shell_shock` (9.10), `shadowcookieblur` (9.8), and `objective`.
- The other cookie sets: the `shadowclear` code material (slot 0x101d1b4c)
  is the floatZ target's clear quad (`0x10024e30`), `shadowcookieoverlay`
  (0x101d1b50) draws only under `sc_showOverlay` (`0x10028070`), and
  `shadowcookieblur` (0x101d1b54) is the blur's stretch (`0x10030d70`)
  (VERIFIED, the DLL's only reads of the three slots). No pak holds
  `shadowoverlay.techset`, and no code slot names a material of it
  (VERIFIED, `materials_lib` and the DLL's strings). The `objective` set names
  `objective_base` for its lightmap and light probe types only (VERIFIED,
  the `.techset`): models draw its pulse (9.4); a static model has no
  cached technique and draws nothing (9.2); no stock MP map's world names
  an `_obj` material (VERIFIED, `strings` of the 15 d3dbsps), so a world
  surface of the set exists only on third-party maps, where vcod draws
  its colour map added.
- `passthru_vertalpha` (`feedbackblend`, `vertcol_notexalpha.hlsl` on the
  feedback sampler): the late post effects' blur composite, run when
  `r_blur` (0) or the view's blur is above 0 (VERIFIED,
  `RB_ApplyLatePostEffectsCmd`); the script menus' `blurWorld 5.0` (19
  stock MP menus: team, weapon, ingame, callvote, serverinfo; VERIFIED).
  The menu's blur reaches the view through `CL_GetMenuBlurRadius`
  (0x00172970), which `CG_CalcViewValues` (0x001d3b28) folds with the cgame display
  context's own as `sqrt(a^2 + b^2)` into the refdef (VERIFIED, its
  store to `cg + 0x285c0`); that this is the
  command's radius is INFERRED. Ported (9.13).
- `passthru_alpha` (`feedbackreplace`): `RB_TouchAllImages` (0x00111406)
  draws every image once into an 8x8 rect (VERIFIED), a warm-up.
- `color_channel_mixer`: the late post effects' `r_showFbColorDebug` 2 and
  `sc_showDebug` views only (VERIFIED, `rgp + 0x108c`'s readers).
- `clear_alpha_stencil`: a full-screen quad before each point light's
  type 18 pass (`R_RenderScene` 0x000c5e5c; VERIFIED), `null.hlsl`, no
  colour.
- `stencil_plane`: read by `RB_StencilPlanesCmd` (0x000dca78), queued by
  `R_AddCmdSetStencilPlanes`, which nothing in the Mac decompile calls
  (VERIFIED, the only reference is its own `.eh` clone).
- `grain_overlay` (`overlay_grain`): no MP menu, script or module names
  it (VERIFIED, grep of `ui_mp/`, `maps/mp/` and `strings` of
  `CoD2MP_s.exe` and the DLL).
- `distortion` (`gfx_distortion_heat`, `_noise`, `_ring_heavy`,
  `_ring_light`, `_ripple`): the 12 effects that name one are tank and
  vehicle fires, the tank muzzle flashes and the player heat waves; no
  `loadfx` in `maps/mp/`, no `weapons/mp/` flash and none of the 44
  effects those reach names one (VERIFIED, grep of the iwds' `maps/mp/`,
  `weapons/mp/` and `fx/`, following each effect's `fx/` references). So
  stock MP draws none although `r_distortion` defaults to 1 (INFERRED). `distortion_floatz.hlsl` reads
  `sampler.resolvedPostSun`, the copy `RB_ApplyEarlyPostEffectsCmd`
  (0x000d68f8) takes when `r_distortion` or `r_glow` is on (VERIFIED).

### 9.12 Water: `water_l_sun`

- One stock MP map draws water, mp_harbor's `water_elalamein`
  (VERIFIED, `strings` of the 15 d3dbsps): opaque (stateBits 0x28128812,
  sort 3), `waterColor` (0.2, 0.3, 0.4, 1), cube `elalamein_water_ft`
  (sampler byte 0x30) and a semantic 5 `normalMap` (sampler byte 0x06:
  linear, point mip, wrap) whose water def is 64, 37, 37, amplitude 0.06,
  wind 76, direction (1, 0) (VERIFIED, `mats.json`).
- The `water` set names `water_l_sun`, `_lfog` and `_efog` for the
  lightmap and lightprobe types; the fogged two draw under `fogged.sm`,
  the plain one under `nofog.sm`, and none routes a code constant beyond
  the sun (VERIFIED, the `.techset` and `.tech` files).
- `R_LoadWaterSetup`'s caller (mac 0x0010459e) fills a 0x44-byte `water_t`
  from the def: +0xc and +0x10 both `textureWidth`, +0x14
  `horizontalWorldLength`, +0x18 `verticalWorldLength`, +0x1c 800.0
  (0x44480000), +0x20 `windSpeed`, +0x24/+0x28 `windDirection`, +0x2c
  `amplitude` (VERIFIED, the stores). `R_LoadWaterSetup` (0x000e7082)
  shares one setup between equivalent defs, at most 16 (VERIFIED, the
  `R_WatersEquivalent` call and the `more than %i waterMap` string with
  0x10).
- `R_CreateWaterSetup` (0x00109c00), per texel in row order from
  `-N/2`, column inner from `-M/2` (VERIFIED, the loops): `k_row =
  (2 pi / (N * Lx)) * row`, `k_col = col * (2 pi / (M * Lz))` (2 pi the
  double at 0x340c78); one `GaussianRandom` pair drawn first; `k.d =
  k_row * dir.x + k_col * dir.y`; where `k.d <= 0`, `H0` and `omega` are
  0; else `H0 = sqrt(A * exp(-1 / ((V^2)^2 / g * k^2)) / k^6 * (k.d)^2 *
  0.5) * A * (gr, gi)` and `omega = sqrt(sqrt(k^2) * g)` (VERIFIED, the
  code; -1 at 0x326194, 0.5 at 0x326190). So the amplitude enters twice
  and the Phillips length is `V^4 / g` where the textbook has `V^2 / g`
  squared (INFERRED, the expression).
- `GaussianRandom` (0x0002cf48): the polar method on `2 r / 2^31 - 1` for
  two `rand()` draws (4.656613e-10 at 0x3261e8), retried while the
  square sum passes 1, scaled by `sqrt(-2 ln s / s)` (-2.0 at 0x340c40)
  (VERIFIED, the code). Which `rand()` state the load meets is not
  recoverable, so vcod's realization is its own: glibc `rand()` seeded 1,
  one stream for the map's setups (INFERRED that retail's is one stream
  too; the setups run in material load order).
- `RB_UploadWaterTexture` (0x000fa340) runs once per backend frame (the
  frame counter at `water_t + 0`) and only under `r_drawWater` (bool,
  default 1; VERIFIED, the registration and the early return). Per texel
  with `omega != 0`: `p = floor(omega * t * 162.97466 + 0.5)` (1024 / 2 pi
  at 0x340cd0), `re = H0.re * sin[(p + 0xff) & 0x3ff]`, `im = H0.im *
  sin[p & 0x3ff]`, the table 1024 steps of `sin(i * 0.3515625 deg)`
  filled at renderer start (0x326408, 0x340c28) (VERIFIED, the code). It
  multiplies the parts apart, not as a complex rotation (VERIFIED). `t` is
  the view's time, `backEnd + 0x3bc` (9.4; seconds INFERRED).
- `_FFT` (0x000fa080), an in-place radix-2 over `1 << bits` values a
  stride apart: bit reversal off `_FFT_Init`'s 8-bit table, a radix-4
  first stage, then butterflies by the 256-step twiddles `(cos, sin)(2 pi
  i / 256)` (0.0245437 at 0x340cc8), the positive exponent, unscaled
  (VERIFIED, the code). It runs along each of the N rows, then down each
  of the M columns (VERIFIED, the two loops). With no frequency shift
  the field carries a `(-1)^(x + y)` sign the magnitude drops (INFERRED).
- On dx9 (`r_rendererInUse` not 2) the image is L8 (D3DFMT 0x32, full mip
  chain; VERIFIED, `R_CreateWaterMap` 0x000fccce): each texel
  `floor(min(|h| / (M * N), 1) * 255 + 0.5)` (1.0 at 0x326188, 255 at
  0x32618c), each lower level `R_DownsampleMipMapBilinear`'s truncated
  2 x 2 mean (`>> 2`) (VERIFIED, the code). The dx7 arm instead writes
  X8R8G8B8 normals off the height differences (127.5 at 0x326440)
  (VERIFIED), which `materials_dx7`'s `water_lightmap` reads; vcod takes
  the dx9 arm.
- `lib/water.hlsl` (VERIFIED, the file): one parallax step `err = 0.5 -
  tex(uv * 0.5).r`, `uv += err * eyeDir.xy * 1.5 / 64` with `eyeDir` the
  normalized world vector from the eye; a three-octave turbulence
  (lacunarity 3.7, rate 0.6) of the map's red at `uv`, `+ (0.5 / 128, 0)`
  and `+ (0, 0.5 / 128)`; normal `normalize(h10 - h00, h01 - h00, 1)`
  taken as world space; `R = reflect(eyeDir, n)`, the highlight
  `max(0, R . lightPosition0)^64 * lightColor0` before `R.z = |R.z|`; the
  colour `lerp(waterColor * n.z, cube(R).rgb, cube(R).a)` plus the
  highlight, alpha the vertex alpha, then the fog. No lightmap and no
  sun visibility.
- vcod: `vcod_cod2::water::WaterSetup` ports the setup, the per-frame
  field and the L8 levels in f32 operation order; the renderer
  (`renderer/cod2_water.rs`, `cod2_water.wgsl`) uploads the levels each
  frame at the frame's clock and draws the HLSL with the sun of 9.1 and
  the world's fog, scaled by identityLight as the lit family is. The
  stage keeps the colour-map draw for the passes that read stages.
  `fft_is_the_unscaled_inverse_dft` holds the FFT to a direct inverse DFT
  within 1e-3; `water_plane_matches_the_hlsl` renders a plane over a
  smooth height map and a six-colour cube on lavapipe against the HLSL on
  the CPU at each pixel centre: 99.7% of channels within 4 of 255 with
  and without exp fog, off cube seams, the rest where MSAA samples
  straddle a face edge (worst 108).

### 9.13 The menu blur

`RB_ApplyLatePostEffectsCmd` (0x000da2f6) runs it after the glow.

- It runs when `r_blur` (default 0) or the command's radius is above 0,
  and blurs at `sqrt(r_blur^2 + radius^2)` (VERIFIED, the branch at
  0x000da441). The units are 480-high virtual ones (INFERRED, from the
  conversion below).
- Under `GetVirtualWidthFromRealWidth(3.0)` (0x0018e934) the radius is
  raised to it and the composite's alpha byte is `floor(r / least * 255
  + 0.5)` (0x32618c is 255.0, 0x326190 0.5), else 255 (VERIFIED, the
  code at 0x000daad1). The scale `SetScreenScaling` (0x0018eb38) stores
  for it is `640 / (height * 4 / 3 / cls + 0x2b46f8)` (VERIFIED); that
  this makes `3 * 480 / height`, three real pixels, is INFERRED (the
  divisor read as the pixel aspect, 1).
- `RB_GaussianFilterImage` (0x0010669e) takes y as `r * (view + 0x3c) /
  480` (0x3263f8 is 480.0) and x as y times `vidConfig + 0x14`
  (VERIFIED, the disassembly at 0x001066c3). `+0x14` is
  `aspectRatioPixel` (VERIFIED, the stabs' `vidConfig_t`); `view +
  0x3c` the view's height in pixels is INFERRED.
- It runs `RB_GenerateGaussianFilterChain` from target 2 (`dx +
  0x2c58`, full size) into target 8 (`dx + 0x2cd0`), ping-ponging 11
  and 12 (VERIFIED). Target 8 is a quarter of the screen like 9, 11 and
  12 (VERIFIED, `R_InitRenderTargets`' shift 2 for its image). Target 2
  is the copy taken at the top of the command, before
  `RB_GlowFilterImage` (VERIFIED, the `StretchRect` into
  `Image_GetSurface(dx + 0x2c58)`), so the blur reads the scene without
  the glow.
- The chain (0x00105d0a) compares the source's and the target's width,
  then height (VERIFIED, 0x00105d28 and 0x00106093). When they differ it
  first takes a 2D pass (`RB_GenerateGaussianFilter2D` 0x00105ac0, one
  radius in `xmm0`, the x pairs from the source's to the target's width,
  the y pairs the heights; VERIFIED, 0x00105ae9..0x00105b11) at `m =
  min(rx, ry)`, or 1.389560 (0x32644c) when `m` is larger, and leaves
  `sqrt(r^2 - m^2)` (1.930878 at 0x326450 past the cap) per axis, scaled
  by the target's size over the source's (VERIFIED, 0x00105d31..
  0x00105da0). The final 2D pass of a small, alike pair takes their mean
  (VERIFIED, `(rx + ry) * 0.5` in `xmm0` at 0x00105e24). 9.9's 1D rules
  follow.
- The composite draws `feedbackblend` full screen over target 8
  (VERIFIED, 0x000da4d5): `passthru_vertalpha`, `vertcol_notexalpha.hlsl`
  (colour times texture, alpha the vertex colour's; VERIFIED, the
  iwd), stateBits 0x28128865, SrcAlpha and InvSrcAlpha (VERIFIED,
  `mats.json`). Its vertex colour is `0xffffff00 | alpha`, the alpha in
  the low byte (VERIFIED, 0x000daaf9). The Mac packs a colour alpha
  first in memory (VERIFIED, `R_ConvertColorToBytes` 0x000f568c stores
  alpha at byte 0). That the low byte is so the alpha is INFERRED.
- vcod: `cod2_glow::blur_plan` and `Cod2Glow::encode`, the radius from
  `frontend::Ui::blur_world` (cod2-fonts-menus.md 2). The chain runs on
  the scene before the glow, the composite after it. `r_blur` is not
  registered and the cgame display context's radius is taken as 0.
  Measured on lavapipe against a CPU evaluation of the HLSL
  (`cod2_glow::tests::blur_matches_the_hlsl`): a 10.7-pixel blur within
  2 of 255, a ramped blur over the glow within 3.

### 9.14 Particle clouds

- `cloud_snowflake01`..`04` (`particle_cloud_outdoor`) are the `Cloud`
  elements of `snow_light_mp_downtown`, `snow_light_mp_railyard` and
  `snow_wind_cityhall`, which mp_downtown, mp_harbor, mp_leningrad and
  mp_railyard loop (VERIFIED, the effects and `maps/mp/*_fx.gsc`). No
  other stock material takes `particle_cloud` or `particle_cloud_outdoor`
  (VERIFIED, `mats.json` census), and no stock effect's `Cloud` names
  another material (VERIFIED, the eight `fx/misc/` effects with a `Cloud`
  element). Their state bits are `effect`'s, 0x28128765, a
  `SrcAlpha, InvSrcAlpha` blend (VERIFIED, `mats.json`). Every fog and
  feather slot of both sets names the set's one technique, whose statemap
  is `nofog` and whose HLSL computes no fog (VERIFIED, the `.techset`,
  `.tech` and `.hlsl` files).
- The buffer: `R_CreateParticleCloudBuffer` (0x000ca59a) fills one static
  vertex buffer of 0x1000 vertices (stride 0x14: position, texcoord) and
  an index buffer of 0x800 triangles at startup: flake `i * 128 + j * 16
  + k` (i, j < 8, k < 16) at `((r + i) * 0.25 - 1, (r + j) * 0.25 - 1,
  (r + k) * 0.125 - 1)`, `r = rand() * 2^-31` drawn x, y, z in turn,
  each of its four vertices at that point with texcoords (0,0), (0,1),
  (1,0), (1,1) (`_cornerTexCoords` 0x32ab00) and indices 0 1 2 2 1 3
  (`_quadIndices` 0x32aae0) (VERIFIED, the code and the tables). So a
  cloud is 1024 flakes jittered one per cell of an 8 x 8 x 16 grid over
  the -1..1 cube (INFERRED, from the formula).
- The particle: `Cloud::Cloud` (0x000a98e6) draws a spin axis, up to four
  `flrand(-1, 1)` triples until one has a squared length in 0.01..1,
  normalized, else (1, 0, 0) (VERIFIED). `Cloud::Update` (0x000a6638)
  evaluates `size` into the draw's scale (+0x98), the flake width
  (+0x88) from `size2` when the template's `nonUniformScale` byte is set,
  else `size`, and the flake length (+0x8c) from `length` (its random
  weight the `flrand(0, 1)` `FX_AddCloud` stores at +0x260) when
  `useLength` is set, else the width; it adds `rotationDelta * ms * 0.01`
  degrees to the angle (+0x44) and turns the axes (+0x48) that far about
  the spin axis (`AngleAxisToQuat` 0x0002dc44, which halves the angle
  through pi/360, then `QuatToAxis` 0x0002b5f0) (VERIFIED). Stock snow
  has `nonUniformScale 1` and no `useLength`: a 300-unit scale and square
  1.5-unit flakes (VERIFIED, `snow_light_mp_downtown.efx`). `Cloud::Draw`
  (0x000a069e) adds a type-6 scene entity (VERIFIED).
- The draw: the backend's entity loop sets the world matrix of every
  entity it draws through `RB_SetWorldMatrixForEntity` (0x000cf8c2),
  `_MatrixSet44`'s rows the axes times +0x38 (the cloud's scale) and the
  origin last (VERIFIED, the functions; that a type-6 entity reaches it
  is INFERRED, `RB_RenderDrawSurfList` 0x000d5630 calls it for every
  entity surface).
  `RB_TessEntity` (0x000ff3cc) case 6 calls `RB_TessParticleCloud`
  (0x000fdb0a), which draws the whole buffer with the material's
  technique (VERIFIED). It sets `particleCloudColor` to the entity's
  colour bytes over 255 and `particleCloudMatrix` to `(width, 0, 0,
  length)`, except when width and length differ and the origin moved
  more than 0.001 since the last frame: then the motion direction times
  the length, turned into view space (`Vec3RotateTranspose` by the view
  matrix, x right, y up, from `_MatrixForViewer` 0x0002b87e), stretches
  the flake: `xy = width * (vy, -vx) / |v|`, `zw = v`, or `width * v /
  |v|` when `|v|` is under the width. That stretch runs only when `vx >=
  0.001` or `vy >= 0.001`, signed: a flake moving left and down on screen
  stays a `(width, 0, 0, length)` rectangle (VERIFIED, the `ucomiss`
  pair at 0x000fdd8c).
- `lib/particle_cloud.hlsl` (VERIFIED, file): `vs_main` moves each
  vertex from its flake's view position by `(u - 0.5) * xy + (v - 0.5) *
  zw` in view x and y before the projection; `ps_main` is the colour map
  times `particleCloudColor`, and under `OUTDOOR` alpha 0 where the
  flake's `worldOutdoorLookupMatrix` position has a z under the outdoor
  map's red at its xy. The lookup point is the flake's, not the corner's.
- `Cloud::Cull` (0x000a0870) culls the cloud by a sphere at its origin of
  the scale plus the larger of width and length, through
  `FxHelper::CullSphere` (0x000ab0d0), which rejects a sphere wholly
  behind any of the first n frustum planes (VERIFIED). The cube's
  corners reach `scale * sqrt(3)`, past the sphere (INFERRED).
- The outdoor map (VERIFIED, the decompile and the globals):
  `R_RegisterOutdoorImage` bounds the world's surfaces but those whose
  material's `gameFlags` has 0x8 (sky), sets an axis no surface reached
  (min still 131072) to 0..0, grows an axis thinner than a unit by half
  a unit each way, and with `outdoorMapSize` (512, 512, 256) (0x32b960)
  keeps `scale = (size - 1) / extent`, its reciprocal and `-min * scale`
  per axis; `worldOutdoorLookupMatrix` maps the box onto 0..1.
  `R_GenerateOutdoorImage` traces a point (`CM_BoxTrace`, model 0, mask
  0x2001) per texel from `max z + 1` to `min z - 1` at `x = (col + 0.5 -
  offset) / scale` (row likewise for y) and stores
  `floor(hit z * scale + offset)` clamped to 0..255 into a 512 x 512 L8
  image (`Image_Generate2D` format 0x32). A texel's world point is
  `(col + 0.5) / 511` of the box where the lookup reads `(col + 0.5) /
  512` (INFERRED, from the two formulas). On mp_downtown the box is
  (-1857, -4864, -128)..(5184, 3328, 1204) and the image takes the
  traces in 70 ms in a release build (VERIFIED,
  `vcod_cod2::outdoor` tests).
- vcod: `fx::sim` keeps a cloud as a particle and `FxSystem::clouds`
  hands the renderer each live one far to near as `fx::cod2_cloud::FxCloud`
  (world rows, origin, `particleCloudMatrix`, colour, cull sphere). The
  fx pass's `fx_cloud` draws vcod's own 1024-flake buffer (retail's
  formula on vcod's random draws, since retail's `rand()` state at
  renderer start is unknowable) once per cloud through `fx_cloud.wgsl`,
  the HLSL's port, in the fx run list by material sort after the quads
  of that sort. The four stock materials draw as the vertex-colour family
  (colour map times the vertex colour, which carries
  `particleCloudColor`), never fogged. The outdoor image is
  `vcod_cod2::outdoor::OutdoorMap::generate` over the load's one
  `ClipMap` (the prediction's) and `MapFile::outdoor_bounds`, built on a
  worker the first frame a map draws a cloud (no cloud draws until it
  is); its bounds skip
  surfaces whose bsp material has the sky surface flag 0x4, which on the
  four snowing maps is exactly the materials whose file has `gameFlags &
  8` (VERIFIED, `the_sky_flag_matches_the_material_files_on_the_snow_maps`).
  vcod samples it bilinear, clamped; retail's `sampler.outdoor` filter is
  not read. The cull tests the sides and near plane. Where vcod differs:
  the motion stretch reads the velocity, not the last frame's origin;
  every cloud material takes the outdoor test (all four stock ones are
  `particle_cloud_outdoor`). `renderer::fx_cloud::tests` draws a turned,
  stretched cloud half under a roof through the pass's pipelines on
  lavapipe against the HLSL on the CPU (rasterized with
  perspective-correct texcoords, blended on an 8-bit target): within 2 of
  255 over 90% of the pixels, the rest on an edge, a texel seam or the
  roof's line, with the roofed half's flakes dropped.

## 10. Images

- `Image_LoadFromFile` reads `images/<name>.iwi` (VERIFIED, the
  `"%s%s%s", "images/", name, ".iwi"` format in the decompile). It rejects a file
  of another iwi version (INFERRED, the branch); stock files are version 5
  (VERIFIED on three files). The material's image name is the iwi's base name; all 8403
  stock image references resolve to a file in `images/` (VERIFIED, census).
- `$`-names are code images: `Image_Load` (0x000fd52a) builds `$white`,
  `$black`, `$identitynormalmap`, `$specularity`, `$outdoor`,
  `$lightmapweights`, `$lightgridweights0`, `$lightgridweights1`,
  `$watercolor`, `$sunhalfangle` (VERIFIED, strings in the function); stock
  materials reference `$identitynormalmap` 1355 times, `$white` 13,
  `$black` once, and iwi files for those three exist in the paks
  (VERIFIED, census).
- `~`-names are images the converter generated by packing channels
  (INFERRED from the names): normal maps `~<src>-gggr` (1339 references),
  spec maps `~<rgb src>-rgb&<alpha src>-l-11` and kin, where the alpha
  source is a `_cosine`/`_cp` exponent map, `$white` or a scaled constant
  (`-r-25g-25b-25`); names past about 40 characters end in `~<8 hex>`. The
  colour endpoints of every sampled DXT block in `~case_normal-gggr` (DXT3)
  and `~bunkervent_bump-gggr` (DXT5) are grey, r = g = b (VERIFIED, 64
  blocks each), which fits rgb = source green and alpha = source red, the
  `.wy` the shader reads.

## 11. Surface flags and contents: what the server reads

- The Linux server never opens a material file: `cod2_lnxded` holds no
  `materials/` string (VERIFIED). It reads the map's lump 0, 72-byte
  records of name[64], `surfaceFlags` u32 at +0x40, `contents` u32 at +0x44,
  in `CMod_LoadMaterials` (0x080527e4; VERIFIED, record size and the two
  byte-swapped fields). The same 72-byte record is CoD1's
  (`bsp-ibsp59-format.md`).
- The lump's `surfaceFlags` equal the material file's in all 3343 records of
  the 15 stock MP maps; `contents` differ in 1731 of them, because the map
  compiler adds bits per brush (0x8000000 detail most often, 0x2080 on 269
  rows) and repeats a name with each variant (1500 repeated names)
  (VERIFIED, census). So collision reads the lump, never the file.
- `infoParms` (0x00348820) is 52 rows of a name and four u32 columns
  (VERIFIED, table read). Calling the columns clearSolid, surfaceFlags,
  contents and toolFlags is INFERRED from their values and from
  `Com_SurfaceTypeFromName`, which reads the second as surface flags. The
  table gives the bits. Surface types live in `surfaceFlags &
  0x1f00000`, `>> 20`: 1 bark, 2 brick, 3 carpet, 4 cloth, 5 concrete,
  6 dirt, 7 flesh, 8 foliage, 9 glass, 10 grass, 11 gravel, 12 ice,
  13 metal, 14 mud, 15 paper, 16 plaster, 17 rock, 18 sand, 19 snow,
  20 water, 21 wood, 22 asphalt; `opaqueglass` is glass's 9 without glass
  contents. `Com_SurfaceTypeFromName` (0x000bcad0) and
  `Com_SurfaceTypeToName` (0x000bcb46) index the first 22 rows.
- The other surface flag bits and every content bit, read off the same
  table, are tabled in cod2-d3dbsp-format.md, "Contents and surface flags"
  (VERIFIED, table).
- Readers, by name in the Mac decompile (VERIFIED, the `& 0x1f00000`
  sites; the Linux build has the same masks in `PM_GroundSurfaceType`
  0x080e2220, `PM_FootstepEvent` 0x080e5402, `G_RunMissile` 0x0810e0ce,
  `G_ExplodeMissile` 0x0810d970, `Scr_BulletTrace` 0x08113818 and four
  unnamed functions): `PM_GroundSurfaceType` (Mac 0x0006b6ee),
  `PM_FootstepEvent`, `PM_GroundTrace`, `Bullet_Fire_Extended` (Mac
  0x001c867e), `G_RunMissile`, `G_BounceMissile`, `G_ExplodeMissile`,
  `Scr_GrenadeExplosionEffect`, `Scr_BulletTrace`.
- What they do with it (INFERRED, their branches): `PM_GroundSurfaceType`
  returns 0 when the ground has 0x2000 nosteps, else the surface type, which
  steps and lands use. `Bullet_Fire_Extended` sends no impact on a 0x4 sky
  hit, sends the hit's surface type in the impact event (7, flesh, when the
  hit entity's type is 2), and when the hit's contents carry 0x10 (glass)
  moves the start past the hit and fires again, so a bullet goes through
  glass. The missile functions put the surface type in their events and
  `G_RunMissile` tests for type 20 (water). `Scr_BulletTrace` returns it
  to script. Shot traces use clipmask 0x2802831 (VERIFIED,
  `G_LocationalTrace`'s argument in `Bullet_Fire_Extended`).

## 12. Open questions

- What `gameFlags` 0x1 and 0x2 select.
- The Windows DLL's own addresses for the loader and applier: only strings
  were matched here.
- The world vertex's lightmap coords are rescaled per lightmap by a
  20-byte record (`R_LoadSurfaces` 0x000e4672). cod2-d3dbsp-format.md,
  lump 1, reads it as the `r_optimizeLightmaps` page merge's scale and
  offset, a load-time choice; the record's field layout is not worked out.
