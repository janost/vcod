# CoD 1.1 shader scripts

What CoD's material scripts contain, what vcod implements of them, and where
it diverges on purpose. VERIFIED means measured against the stock assets or a
binary, or seen in the running client; INFERRED means read off the RTCW
sources or asset shape. RTCW references are RTCW-MP's `src/renderer/*.c`, the
closest public ancestor of the CoD 1.1 renderer. vcod's parser and runtime
math live in `crates/common/src/shader.rs`, the drawing in
`crates/client/src/renderer.rs` and `crates/client/src/sky.rs`.

## 1. What shader scripts are and where they live

CoD 1.1 keeps the Q3/RTCW material-script system: a BSP soup material name is
looked up in a shader library parsed from `scripts/*.shader` blocks across all
mounted paks; a name with no block is used as a plain image path. Block
syntax is Q3's `{ keyword args ... { stage } }`. CoD-specific placements:

- World materials resolve from `scripts/*.shader` in `main/` paks; every
  authored stock block sits in pak4, pak5, pak8 or pak9 of a 1.5 install
  (VERIFIED by census, §2).
- Effect shaders live in `fxshaders/` inside `pak5.pk3`, not `scripts/`, and
  are almost all additive (`blendfunc gl_one gl_one`).
- United Offensive adds its own scripts under `uo/`, e.g. the MP sky blocks in
  `uo/pakuo01.pk3 : scripts/gmi_sky.shader` (VERIFIED: the
  `textures/skies/noville` block read out of that archive).

vcod parses every script entry into `ShaderLib` at load
(`shader.rs` `ShaderLib::load`, once per search path through
`asset_source::load`). An unused block costs only memory.

The renderer never reads a `Shader`: `ShaderLib::materials` compiles each
block into the game-neutral `material::Material` (`Material::from_shader`),
which resolves the sort (section 5), the polygon offset, the dlight draw
(section 3) and the sorting stage's images, and keeps the stages as parsed.
`renderer/material_keys.rs` pins what the renderer derives from every stock
block and `.stype` template.

## 2. Census

Method: parse every `.shader` entry from the stock `pak[0-9].pk3` of a 1.5
install (assets identical between 1.1 and 1.5) and cross-reference block names
against the soup-material set of every map in those paks.
`crates/common/tests/shader_corpus.rs` re-measures the pins on every run
(needs `COD_DIR`) and fails when parser changes move them. It scopes to stock
paks, since server downloads drop third-party `zzz_*.pk3` files into `main/`.

Headline numbers on that install (all VERIFIED):

| measure | value |
|---|---|
| distinct authored blocks in stock paks | 604 |
| distinct world material names across stock maps | 2441 |
| of those with an authored block | 301 (rest are implicit image paths) |
| world-referenced block instances, summed over maps | 302 |

Stage-keyword counts over the stages those world blocks expand to (a block has
one or more stages):

| keyword | share / count |
|---|---|
| rgbGen | 92.9%: exactVertex 180, vertex 88, identity 21, identityLighting 15, const 12, constLighting 11, wave 2 |
| blendFunc | 65.3% |
| alphaGen | 37.3%: vertex 111, const 19 |
| tcMod | 40.7%: scroll 166, scale 93, turb 57, transform 9, rotate 1, stretch 0 |
| tcGen | 26%: vector 90; nv_* only behind `requires` gates; environment ungated in mp_ship's six door and window blocks (§3) and stalin_1wet |
| depthWrite | 18.4% |
| alphaFunc | 18.6%: every occurrence GE128 |
| animMap | one block |

blendFunc pairs in world blocks: src_alpha/one_minus_src_alpha 141,
one/one 51, src_alpha/one 18, dst_color/zero 17, plus the shorthands
(`add`, `filter`, `blend`). `nextbundle` appears in roughly 140 world blocks
(water `$lightmap` bundles, terrain blends, foliage, facades, windows, flags):
two-bundle stages are core CoD grammar.

A `$lightmap` bundle must carry the same overbright as the implicit
texture-times-lightmap path, or coplanar surfaces split in brightness:
`scripts/terrain.shader` overlays (`rgbGen exactVertex` + `alphagen vertex`
over `map $lightmap` `blendFunc filter`, polygonOffset) sit on base ground
soups whose material is the raw image path with no script at all, and the
pair must shade identically for the vertex-alpha cross-fade to read as one
surface (mp_brecourt: 19 staged overlay soups over 65 implicit base soups).
So vcod applies its x2 overbright to `$lightmap` bundles in `fs_stage`
(`shader.wgsl`).
A stage with no `$lightmap` bundle takes that x2 itself unless it, or a
later `$lightmap` stage, multiplies the framebuffer (`cod11-gamma.md`
section 5).

Sky: every stock MP map carries at least one sky-parms material among its soup
materials (pinned by the corpus test's skyless assertion), and every stock
skyParms line is `skyParms env/<name> 512 -`: farbox plus cloud dome at height
512, never an inner box.

## 3. Grammar subset implemented

The parser accepts the keywords below; anything else inside a stage warns once
per (block, message) via `WarnSet` (`shader.rs`) and is skipped. Script
damage retail's own parser warns about, and keywords vcod reads but
approximates, go to `WarnSet::note_once` (debug) instead, so a stock install
of 1.1, 1.5 or UO loads with no shader warning (`shader_corpus.rs`,
`stock_shaders_parse_without_warnings`). A keyword's arguments are the rest
of its line (below). RTCW line anchors from `tr_shader.c` unless noted.

| token | RTCW anchor | vcod notes |
|---|---|---|
| map / clampmap | ParseStage :529 | image resolved through `assets::resolve_bundle_image` (`assets.rs`); the load flags before the name are read (below) |
| animMap | no RTCW form (CoD's own) | `animMap <fps>`, then one `map` line per frame (below) |
| multiplyImage | no RTCW keyword (CoD-only) | folded into the stage's gens (below) |
| nextbundle | no RTCW keyword (CoD-only) | opens bundle 1 of the same stage (`shader.rs` nextbundle arm) |
| blendFunc | ParseStage :529 | full factor pair plus shorthands add/filter/blend; after `nextbundle` the bundle's texture environment instead (below) |
| alphaFunc GE128 | ParseStage :529 | cutout band; GE128 is the only form the corpus uses |
| rgbGen / alphaGen forms | ParseStage :529 | exactVertex, vertex, identity, identityLighting, const(Lighting), wave; alphaGen vertex/const/entity |
| tcMod scroll/scale/turb/transform/rotate/stretch | ParseTexMod :321 | runtime math in §4 |
| tcGen (or `texGen`) vector / lightmap / environment | ParseStage :529 | vector basis dots world position in the VS; environment reflects the view (below); other forms drop the stage with a warning |
| depthFunc lequal / equal / always | no RTCW keyword (CoD-only) | world stages still draw `LessEqual`; entity and viewmodel stages honour it (below) |
| depthWrite | ParseStage :529 | explicit keyword beats the blend default (`depth_write()` in `shader.rs`) |
| cull / polygonOffset / nopicmip / nomipmaps / surfaceParm | ParseShader :1367 | surfaceParm sets water/sky flags among others |
| portal | ParseShader :1367 | sort 1; vcod draws no portal view |
| sort | sort-name table tr_shader.c :1224-1242 | mapping in §5 |
| skyParms | ParseSkyParms :1152 | farbox + cloud dome; see §6 |
| sunfile | no RTCW anchor (CoD-only) | argument stored on the block (`Shader::sunfile`), resolved to `scripts/<name>.sun` and drawn as the sun disc (§8) |
| deformVertexes | ParseDeform (tr_shader.c) | wave, flap, bulge, move, normal, autosprite, autosprite2 drawn; syncNormal, projectionShadow, text0-7 consumed inertly (§8, "Vertex deforms") |
| fogvars / skyfogvars / waterfogvars / nofog / tesssize / light / entityMergable / clampTime / radialNormals / fogParms / picmip2 / nolightscale / qer* / q3map* | ParseShader :1367 | consumed as known tokens; script fog values stay inert, MP fog rides configstring 12 (§8) |

CoD-only stage keywords read out of CoDMP.exe 1.1's stage parser
(0x4f8390):

- VERIFIED, 0x4f8990..0x4f89f9: `depthfunc` (string 0x547fac) takes
  `lequal` (0x547f20) for GL state 0, `equal` (0x547f18) for 0x20000 and
  `always` (0x547f10) for 0x40000; anything else warns "unknown depthfunc"
  (0x547ee0), and the keyword is refused inside a `nextbundle` (0x547f6c).
  The GL state setter 0x4d57b0 turns 0x20000 into `GL_EQUAL` and 0x40000
  into `GL_ALWAYS` (`cod11-light-grid-and-leaf-lights.md` section 13).
- VERIFIED, 0x4f9838..0x4f9859: `tcGen environment` (0x5478a8) stores
  texgen 4 in the bundle (`+0xa0`) and ORs 2 into the shader word at
  `0x11e67c8`; `lightmap` (0x54789c) stores 2 and `vector` (0x55b358) 6
  with its two basis vectors at `+0xa4` and `+0xb0`, Q3's `texCoordGen_t`
  numbering.
- VERIFIED, 0x514d40 (called from the texgen switch at 0x500520): per
  vertex, `v = normalize(viewOrigin - xyz)` (0x42dd30, the 0x5f3759df
  reciprocal square root with one Newton step), `d = n . v`,
  `r = 2 d n - v`, then `s = (r.y + 1) * 0.5` (constants 1.0 at 0x568e60,
  0.5 at 0x568e70) and `t = 0.5 - r.z * 0.5`: RTCW's
  RB_CalcEnvironmentTexCoords unchanged. `viewOrigin` is read at
  0x16d8dd8, `+0x30` of the orientation at 0x16d8da8 whose model matrix
  sits at `+0x3c` (0x16d8de4, loaded by the surface loop at 0x4d6ad5).
  INFERRED from that layout, which is RTCW's `orientationr_t`: the eye and
  the vertices are in the entity's frame, so the reflection turns with the
  model, not with the world.
- VERIFIED, 0x4f35dc..0x4f3626, the image parse: `only16bit` (0x54a5d4),
  `only32bit` (0x54a5c8) and `noopt` (0x54a5c0) are flags before the image
  name (`noopt` ORs 0x100 into the image flags), so `map noopt
  $texturename` in `viewhands.stype` names `$texturename`.
- VERIFIED, 0x4f34ad..0x4f35ac, the same loop: `nopicmip` (0x54a62c),
  `picmip`, `nomipmaps`, `mipmaps`, `clamp` (0x559bd4, image flags 0x30),
  `clampx` / `clamps` (0x10), `clampy` / `clampt` (0x20) and
  `heightToNormal` (0x54a5e0, flag 0x40), which reads a number after it.
- VERIFIED, 0x4ec490 (the image upload): `GL_TEXTURE_WRAP_S` (0x2802, the
  call at 0x4ec6e9) is `GL_CLAMP_TO_EDGE` (0x812f) when image flag 0x10
  is set and `GL_REPEAT` (0x2901) otherwise; `GL_TEXTURE_WRAP_T` (0x2803,
  0x4ec70f) the same on 0x20. So `clampx` clamps `s` only and `clampy`
  `t` only. vcod keeps them per bundle (`shader::Clamp`) and samples
  through a sampler clamped on that axis (`tex_samplers.wgsl`, stage flags
  `CLAMP_S`/`CLAMP_T` for bundle 0, `CLAMP1_S`/`CLAMP1_T` for bundle 1).

Tokens and lines, CoDMP.exe 1.1 unless noted; CoDMP.exe 1.5 (0x4f8f50)
and CoDUOMP.exe (0x4fea30) parse stages with the same keyword strings:

- VERIFIED, 0x449fc0: every keyword's argument read calls the tokenizer
  0x449c10 with its line-break flag 0, Q3's `COM_ParseExt(text, qfalse)`;
  the keyword reads pass 1 (0x4fa950). 0x44a130 skips to past the next
  `\n`, `SkipRestOfLine`.
- INFERRED from Q3's `COM_ParseExt`, which that call matches: with the
  flag 0 the tokenizer returns an empty token at the end of the line, so
  an argument never comes off the next line and a missing one reads
  empty. The stock corpus agrees: read so, it leaves no token over on
  any install. vcod's tokenizer emits a line-break token
  (`shader::NEWLINE`) and each handler sees only the rest of its line.
  Trap: arguments that run on across lines let stock water's
  three-number `tcMod turb 2.03 0.2 1.03` eat the next line's `rgbGen`,
  and every `rgbGen wave` leave its frequency behind (`unknown token 1.5`,
  49 of them on UO).
- VERIFIED, 0x4f1b40, `tcMod`: each form reads its numbers and returns
  with "missing tcMod turb" (or the form's own message) before counting
  the tcMod when one is empty, so a short `turb` adds no tcMod. vcod drops
  it the same way (`turb` takes four numbers, base first).
- VERIFIED, 0x4f1980, genfuncs: `sin` 1, `square` 2, `triangle` 3,
  `sawtooth` 4, `inversesawtooth` 5, `noise` 6; any other warns "invalid
  genfunc name" and reads as sin (`mov eax,1` at 0x4f1a39). 0x4f1a40 reads
  the four numbers and stops at the first empty one, the rest left 0.
- VERIFIED, 0x4f85a0 and 0x4f9926..0x4f997a, `animMap` and
  `oneshotanimMap`: the rate, then `SkipRestOfLine`. Each `map` after it
  stores its image in the bundle's next frame slot (0x4f84c1, the count
  at bundle `+0x84`), and a second
  `map` in a bundle whose rate is 0 warns "multiple 'map' specifications
  without preceding 'animmap'" and fails the shader. The stock form is
  `animMap 17` over seventeen `map` lines (`fxshaders/pj_fx.shader`,
  `jh_fx.shader`); UO's `bodyFlameAnimating` lists its frames on the
  `animMap` line, which retail skips, so that stage has no image ("bundle
  with no map"). Reading Q3's inline list instead draws the last `map`
  line still.
- VERIFIED, 0x4f946e, `multiplyImage`: up to four arguments off the line,
  each `identityLighting` (the renderer's identityLight, 0x16c55b0) or a
  number; one sets rgb and leaves alpha 1, three set rgb, four rgba. It
  must precede the stage's images, and the factors reach the image loader
  0x4f3410 (its fifth argument), which scales the texels. INFERRED: a
  factor of at most 1 scales the stage's colour the same way, so vcod
  folds it into the gens (`fold_multiply`): the stock use, the three-unit
  window stages of `surfacelight.shader` and UO's `ra_stanjel.shader`
  (`multiplyImage identityLighting` under `rgbGen identity`), is their
  two-unit stage's `rgbGen identityLighting`.
- VERIFIED, 0x4f1890, destination blend names: `GL_ONE`, `GL_ZERO`,
  `GL_SRC_ALPHA`, `GL_ONE_MINUS_SRC_ALPHA`, `GL_DST_ALPHA`,
  `GL_ONE_MINUS_DST_ALPHA`, `GL_SRC_COLOR` (0x30) and
  `GL_ONE_MINUS_SRC_COLOR`; an unknown name warns and reads `GL_ONE`.
  A `blendFunc` with one non-shorthand name warns "missing parm for
  localFunc" and leaves the blend unset (0x4f8b5e); `smokecolumn`'s
  `blendFunc multiply` is that.
- VERIFIED, 0x4f8c21..0x4f8e5b: an unknown `rgbGen` warns and skips the
  line, keeping the stage's gen (`skins/gradient1`'s `rgbGen dot`);
  `alphaGen entity` (0x4f9014) is a form it knows. INFERRED from Q3's
  `AGEN_ENTITY`: the entity's `shaderRGBA` alpha; vcod draws it opaque,
  the world entity's.
- VERIFIED, 0x4fa950, the shader keywords outside stages: `qer` and
  `q3map` prefixes (3 and 5 letters), `radialNormals`, `tesssize`,
  `fogParms` skip their line; `clampTime` and `light` read one token;
  `portal` sets shader flag 2 and sort 1.0 (0x4fae63); `picmip2` flag
  0x200; any other keyword warns "unknown general shader parameter" and
  fails the shader. CoDUOMP.exe 0x5014c4 adds `nolightscale`, shader flag
  0x2000, on the flamethrower's sprites; 1.5's parser (0x4fb510) has no
  such keyword. Neither has `depthFunc less`: UO's two stock uses warn and
  keep `lequal`.

- VERIFIED, 0x4f8a23..0x4f8a46: `perLight` (0x547e98) ORs 0x80 into the
  stage's flag word (`+0x00`; `detail` is 0x2 there) and is refused inside
  a `nextbundle` (0x547e58).
- VERIFIED, 0x500b30, the base stage loop: it walks the shader's stage
  pointers (array at 0x19bffc4, up to eight, stopping at a null) and draws
  a stage (0x5008a0) only when that flag byte's top bit is clear. A
  `perLight` stage never draws in the base pass.
- VERIFIED, 0x500d3a..0x500e19, the stage iterator's tail: after the base
  loop it calls the dlight pass 0x500a10 when the batch's dlight mask
  (0x197ffa0) is non-zero, the shader's sort (`+0x58`) is at most 5.0
  (0x5690f4; `fcomp`, `test ah,0x41`, `jp`) and its word at `+0x5c` has
  neither bit of 0x20004. Otherwise, when the shader's OR of stage flags
  (`+0x54`) has 0x80 and the current light list (pointer 0x16d8e5c) holds
  lights (count `+0xcc`, pairs from `+0xd0`), every `perLight` stage draws
  once per light whose first word is not 8. INFERRED: that list is the
  model light pick's (`cod11-light-grid-and-leaf-lights.md` sections 6
  and 13), so a blended surface (sort above 5) lights per listed light,
  not per dlight.
- VERIFIED, 0x500a10, the dlight pass: for each of the frame's world
  dlights (count 0x16d8b1c, 0x88-byte records at 0x16d8b24) whose bit is
  set in the mask, 0x4ff690 projects the light onto the batch and returns
  the touched triangles. When `+0x54` has 0x80, every `perLight` stage
  draws over those triangles, once per light; otherwise the stage of
  `dlightshader` (registered at 0x4fdb0b, `scripts/gfx.shader`: `$dlight`,
  `blendFunc GL_DST_COLOR GL_ONE`, `depthFunc equal`) draws instead.
- VERIFIED, 0x4fb3c0: a shader left with one stage that passes its checks
  (two bundles, a blend state among 0, 0x12, 0x22, 0x25, 0x65, both
  bundles' `+0x94` at 0x2100) gets a second stage copied from the first,
  bundle 1 bound to the dlight image (0x16c4ddc, built at 0x4f0172 with
  `r_dlightRadius` / `r_dlightShift`), and flag 0x80. INFERRED: so a plain
  texture-times-lightmap surface lights up per dlight through its own
  texture.
- VERIFIED, string search of the stock `maps/MP/*.bsp` against the stock
  blocks: 23 blocks carry `perLight` (`jeff.shader`,
  `surfacelight.shader`); mp_ship references eight (the bridge windows,
  ship doors and the two deck flags), mp_chateau `litwindow1aext`,
  mp_powcamp `barrackswindow3`.

The dlight pass in detail, all CoDMP.exe 1.1:

- VERIFIED, 0x4ff690, per vertex of the batch: `d` is the light's
  origin (`+0x78..+0x80`, carried into the entity's frame) minus the
  vertex; the dlight image's coordinates are `s = d . S / r + 0.5`,
  `t = d . T / r + 0.5` (0.5 at 0x568e70), `r` the intensity at `+0x74`,
  and the depth is `z = d . N`. With `r_dlightQuality` (0x16c3804,
  default "1") `S`, `T` and `N` are the vertex's tangent frame; without
  it `S`, `T`, `N` are the world axes, Q3's projection. The colour is the
  light's raw colour (`+0x04`, times 255.0 at 0x568ec0) times 1 within
  `r / 2` of the plane, `2 (r - |z|) / r` out to `r`, and 0 past it,
  rounded to bytes, alpha 255. It goes into the vertex colour array
  (0x181ff60) and the coordinates into the lightmap's (0x17fff60), the
  call at 0x500a8c passing both in registers.
- VERIFIED, same function: each vertex gets an outcode (s below 0 or
  above 1, t likewise, z above `r` or below `-r`), and a triangle is kept
  unless its three vertices share a bit. A kept triangle draws whole.
  The depth bits test `|z|` only, so a vertex behind the surface's plane
  (`z < 0`, the light on its far side) is kept like one in front.
- VERIFIED, the light's colour: 0x5008a0 fills the colour array GL draws
  from (0x186ff60, `glColorPointer` at 0x500ba0) per stage through
  0x4ffa60, switching on the stage's rgbGen (`+0x664`): 2 (`identity`)
  writes 0xffffffff, 5 (`exactVertex`) copies 0x181ff60, 6 (`vertex`)
  copies it shifted right by 0x16c55b8, and unlisted values (1,
  `identityLighting`) the identityLight byte 0x16c55b4. 0x500a82..0x500a8c
  hands 0x4ff690 0x181ff60 for its colours. 0x4fc440 gives both stages of
  a lightmapped surface rgbGen 2 (0x11e6f84, 0x11e760c), and 0x4fb3c0
  copies stage 0 whole (0x1a2 words) before it swaps bundle 1 for the
  dlight image and the low byte of the state word for 0x22 or 0x25. So the
  copy draws texture times dlight image times 255: on a lightmapped
  surface neither the light's colour nor the depth falloff is drawn.
  `dlightshader`'s `rgbGen exactVertex` draws both.
- VERIFIED, fog: the shader finalize ORs 0x200000 into every stage's state
  word (`+0x684`, 0x4fbc71) unless the shader has flag 0x400 (`nofog`)
  and the stage lacks flag 4. GL_State (0x4d57b0) turns GL fog on for a
  state with that bit (0x4d2a80) and off without it, and after it sets the
  blend (0x4d5b85) calls 0x4d2ad0, which loads a black `GL_FOG_COLOR` when
  the state's destination nibble is 0x20 (`GL_ONE`) and the fog colour
  times identityLight otherwise. The copy keeps stage 0's state word, so a
  fogged surface's copy fogs towards black: its light scales by the fog
  factor. `dlightshader` is `nofog` (`scripts/gfx.shader`), so the blob
  draws unfogged, and a `perLight` stage fogs as its own shader does.
- INFERRED, from 0x4d2ad0 and the per-stage GL_State call: the black fog
  colour holds for every fogged stage whose destination factor is
  `GL_ONE`, world, model or effect, whatever its source factor, so an
  additive stage fades to nothing with distance instead of greying
  towards the fog colour. vcod: `STAGE_FLAG_FOG_BLACK` (`renderer.rs`),
  set by `stage_params` for a stage blended onto `GL_ONE`, makes
  `fs_stage` scale by `1 - fog` (`shader.wgsl`), as `fx.wgsl`'s additive
  draw does.
- VERIFIED, GL_State 0x4d57b0, the blend: a state whose low byte is 0
  disables `GL_BLEND`; otherwise the low nibble picks the source factor
  (1 `GL_ZERO`, 2 `GL_ONE`, 3 `GL_DST_COLOR`, 4 `GL_ONE_MINUS_DST_COLOR`,
  5 `GL_SRC_ALPHA`, 6 `GL_ONE_MINUS_SRC_ALPHA`, 7 `GL_DST_ALPHA`, 8
  `GL_ONE_MINUS_DST_ALPHA`, 9 `GL_SRC_ALPHA_SATURATE`) and the next the
  destination (0x10 `GL_ZERO`, 0x20 `GL_ONE`, 0x30 `GL_SRC_COLOR`, 0x40
  `GL_ONE_MINUS_SRC_COLOR`, 0x50 `GL_SRC_ALPHA`, 0x60
  `GL_ONE_MINUS_SRC_ALPHA`, 0x70 `GL_DST_ALPHA`, 0x80
  `GL_ONE_MINUS_DST_ALPHA`), straight into `glBlendFunc`. 0x4d2ad0 runs
  only when that byte changes. INFERRED: nothing maps or premultiplies the
  script's pair, so an effect sprite blends as its script says, and the
  source colour is texture times vertex colour under `GL_MODULATE`, its
  alpha the texture's alpha times the gen's.
- VERIFIED, the stage parser, 0x4f8d4f..0x4f8d7e: `rgbGen vertex` (gen 6)
  and `exactVertex` (gen 5, joining at 0x4f8d70) set `alphaGen` (`+0x67c`)
  to 4, `vertex` (the code `alphaGen vertex` stores at 0x4f9069), when it
  is still 0; `alphaGen identity` stores 0 (0x4f9009). INFERRED: so every
  stock fx stage, all `rgbGen vertex` or `exactVertex`, takes the sprite's
  vertex alpha, as Q3's parser does.
- vcod: the shared parse applies the rule (`shader::apply_gen`,
  `vertex_alpha_unless_set`), so the world's, the marks' and the fx
  pass's stages agree. An `alphaGen identity` before the rgbGen is
  overridden, one after it stays, and `const` or `wave` either side
  stays. The entity and viewmodel passes read no vertex colour, the static
  props' stage path draws with `vs_prop`'s alpha 1, the cloud dome's
  vertices are white and the HUD draws no script stages, so the rule
  changes none of them.
- VERIFIED, the stock 1.1, 1.5 and UO `scripts/`, `fxshaders/` and
  `shadertypes/` blocks parsed with and without the rule: no stock stage
  writes `alphaGen identity` after a vertex rgbGen, so 0x4ffa60's
  identityLight-1 case below never arises. The world materials whose
  changed stage reads alpha (a `GL_SRC_ALPHA` blend factor or an
  alphaFunc) on a stock `maps/mp/` map, with lump-8 vertex alphas read
  off each map's soups: mp_bocage's
  `textures/normandy/ground/decal_rockface_blades1`..`3` (1064 of 2861
  vertices under 255, down to 0) and `textures/global_use/cratered_ground`
  (62 of 1301, down to 223), mp_rhinevalley's
  `textures/sfx/shoreline_ripple_rv` (111 of 212, down to 0, `GL_SRC_ALPHA
  GL_ONE`) and mp_sicily's `textures/sfx/wave_shorline_a` (all 300, down
  to 0, `GL_SRC_ALPHA GL_ONE`). The others, mp_ship's railings, doors and
  bridge windows, the foliage cards on mp_brecourt, mp_bocage,
  mp_neuville, mp_tigertown, mp_cassino, mp_foy, mp_italy, mp_kursk and
  mp_ponyri, `128windowborder01`, `neuville_window`, mp_berlin's
  `mpberlinwater`, mp_rhinevalley's `truck_single` and mp_uo_stanjel's
  `dove_anim`, carry alpha 255 on every vertex, so the rule changes nothing there.
- VERIFIED, the stock `.efx` `Decal` elements' shader lists: the rule
  reaches every stock mark material but UO's `impact/bullet_sand_a`
  (`rgbgen identity`): 1.1's `gfx/impact/bullethit_*`, `bullethole1`,
  `2`, `cratered*`, `stone_piece1`, `stone_singleshot1`, UO's
  `impact/bullet_*`, `impact/scorch_*` and `misc/lamp_oil`, all `rgbGen
  vertex` under `GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA`. INFERRED, with
  `CG_AddMarks` (events-and-fx 9.3): their alpha fade draws (without the
  rule they stay opaque to the end of their life), and so does the
  corona's `zfadeout` ramp (`flareShader`, section 8).
- vcod, the fx pass (`renderer::fx_stage_draw`, `fx.wgsl`): a stage draws
  straight alpha, additive onto `GL_ONE` (the texture's alpha read only
  under `GL_SRC_ALPHA GL_ONE`), `GL_ONE GL_ONE_MINUS_SRC_COLOR`
  (`fs_screen`) or unblended with depth written, and a `nofog` material
  is never fogged. vcod ignores the stage flag 4 that 0x4fbc6b also
  tests, as the world's `STAGE_FLAG_NOFOG` does.
- VERIFIED, string search of the stock 1.1, 1.5 and UO paks' `scripts/`
  and `fxshaders/` blocks against the `.efx` `shaders` lists and
  `fx::flame`'s names: the fx materials whose draw depends on those
  rules are UO's, `flamethrowerFire1`..`46` in `pakuo01.pk3`
  `scripts/flamethrower.shader` (`GL_ONE GL_ONE_MINUS_SRC_COLOR`, `nofog`;
  `fx::flame` draws 2..44), `flameSmoke1`..`6` there (blend, `nofog`) and
  the nine `.efx` materials `smoke/smk_{p_out,p_top,t}_fire_{a,b,c}`
  (`pakuo00.pk3` `fxshaders/smoke.shader`, `add`, `nofog`). No stock
  `.efx` material has `GL_SRC_ALPHA GL_ONE` on a base stage. The 57 that
  carry it on a second stage over a blended first (`gfx/impact/bullethit_*`,
  `bullethole*`, `cratered*`, `stone_singleshot1`, UO's `impact/bullet_*`
  and `impact/scorch_*`, `misc/lamp_oil`) flag that stage `perLight`
  with `$dlight` in bundle 1, so it is a dlight-pass stage, never the
  base loop's (below).

The fx pass's stage loop, all CoDMP.exe 1.1:

- VERIFIED, the sprite emitter 0x50fe20 (efx-grammar.md R4) writes its
  quad's indices into the batch's index array at 0x17a7f60, flushing
  through 0x5027c0 and restarting through 0x4ff570 when 4 more vertices
  or 6 more indices would pass 0x2000 or 0xc000. 0x500d3a hands that same
  array to the base loop. INFERRED: an effect sprite draws through the
  generic stage iterator 0x500ba0, as a world or model surface does.
- VERIFIED, the base loop 0x500b30: it walks the batch's stage pointers
  (0x19bffc4, at most 8, a null one ends it), skips a stage whose first
  byte has its top bit (`cmp byte [esi],0`, `js`; the `perLight` flag
  0x80 that 0x4fb3c0 sets), and draws every other one through 0x5008a0.
  `r_lightmap` (0x16c36d4) stops it after the first stage that reads the
  lightmap. So every base stage of an effect material draws, in script
  order, once each over the whole batch.
- VERIFIED, 0x5008a0, one stage: colours (0x4ffa60), texture coordinates
  with the bundle's tcGen and tcMods (0x500400), then for a single-bundle
  stage the image (0x4fe0e0) and GL_State on the stage's state word
  (`+0x684`) before the draw (0x4fe070). Each stage has its own blend,
  colour gen, image and tcMods.
- VERIFIED, 0x4ffa60, the colour gens (`+0x664`): 2 (`identity`) fills
  0xffffffff, alpha included; 5 (`exactVertex`) copies the vertex colour;
  6 (`vertex`) copies it with rgb shifted right by the overbright bits
  (0x16c55b8) and alpha kept; 0xc (`const`) the stage's colour word
  (`+0x680`); unlisted values (0, 1 `identityLighting`) the identityLight
  byte (0x16c55b4) in all four bytes. The alpha gens (`+0x67c`): 0 sets
  alpha 255 unless the rgbGen is 2, or 6 with identityLight 1.0; 4
  (`vertex`) copies the vertex alpha unless the rgbGen is 6, which kept
  it already; 9 (`const`) the colour word's alpha byte (`+0x683`).
  INFERRED, with the parser rule above (0x4f8d4f): a `vertex` or
  `exactVertex` stage fades with the sprite, and an `identity` or
  `identityLighting` stage ignores the sprite's colour and fade (UO's
  `impact/bullet_sand_a`, `rgbgen identity`, draws full strength until
  the sprite dies).
- VERIFIED, 0x4f3660..0x4f36a2: `$lightmap` in a material whose
  lightmap index (0x11e67b8) is negative binds the white image
  (0x16c4de8). An effect material registers at index -4 (refexport
  0x4fca80), so the impacts' `nextbundle map $lightmap` modulates by
  white and changes nothing.
- VERIFIED, 0x4fe0e0, the image: an `animMap` bundle with more than one
  frame picks `fistp(shaderTime (0x197ff98) * fps * 1024) >> 10`, at
  least 0, modulo the frame count (or held on the last frame when byte
  `+0xc6` is set).
- vcod: `renderer::fx_stage_draws` lists a material's base stages and
  `set_fx_quads` writes each run's quads once per stage, the stages back
  to back, each with the stage's image (the anim frame at the frame's
  shader time), blend, `alphaFunc` (`FX_VERT_ALPHA_*`), bundle 0's wrap
  (repeat unless `clamp`, so `jh_animfire`'s `tcMod scale -1 1` mirrors;
  a material without a script stays clamped) and bundle 0's tcMods
  folded on the CPU (`bundle_affine`; `turb` is not drawn); the
  colour is the stage's gens over the sprite's colour
  (`fx_stage_rgba`), an alphaGen identity under `rgbGen vertex` or
  `exactVertex` taking the vertex alpha as the parser rule gives it. An
  unblended stage (`FxBlend::Opaque`) writes depth. A stage whose bundle 0
  is not an image file (`$whiteimage`, `$lightmap`) draws nothing, and
  bundle 1 is never drawn. Blended stages keep `LessEqual` with no depth
  write: no stock effect stage has `depthFunc` or a blended `depthWrite`.
  The quad buffers hold `MAX_FX_DRAWN`, twice the sim's quad cap.
- VERIFIED, string search of the 1.1, 1.5 and UO paks' `.efx` materials,
  what the stage loop draws beyond a first-image-stage-only pass: the second stage
  draws on `gfx/effects/smoke/roller` and `smoke_bck` (two blended
  layers) and UO's `impact/bullet_fabric_a` and `_b` (an additive glow,
  `rgbGen wave`); the tcMods draw on those, `smoke_frnt` (`rotate`),
  `gfx/effects/mg42_m`, `gfx/effects/misc/cloudflash_stalingradm`,
  `gfx/misc/rocketplume` (`transform -1 0 0 1 1 0`, mirrored) and
  `gfx/effects/animated/jh_animfire` (`scale -1 1`);
  `gfx/effects/animated/animfire`, `jh_animfire` and `jh_animfire2`
  animate; the debris (`bark_gib`,
  `bark_gib2`, `foliage_stick`, `grass_piece1`, `grass_piece2`,
  `stonegib1`, `c47_singledetail`, `c47formation1`, `parachute1`, all
  `alphaFunc GE128` and `depthWrite`, unblended) cut out at 128 and
  write depth, as `gfx/impact/dusty` (`alphaFunc GE128`) cuts out; and
  `impact/bullet_sand_a` draws white and unfaded.
- Open: whether a blended effect sprite's `perLight` stages draw. They
  take 0x500d3a's second branch (sort above 5), whose light list
  (0x16d8e5c) 0x50fe68 reloads from 0x197ff9c at a flush; what an effect
  batch leaves there is not traced. vcod draws none on fx quads.
- VERIFIED, which surfaces a light reaches: the world walk (0x4e4c10)
  visits the cells the portals show (0x4e44d0), tests each surface against
  the frustum and portal planes (0x4e3070, 0x4e4120) and hands it to
  0x4e1760, which for surface kinds above 0x17 (every soup's vertex
  storage kind, 0x18..0x1c at 0x4da980) keeps a light when the box test
  0x4e16d0 passes: the origin within the intensity of the box on every
  axis (`o + r > min`, `o - r < max`, the record's `+0x74` and `+0x78`).
  The bits go to the surface's `+0x08`, which 0x5122f2 ORs into the batch
  mask. Nothing tests the light's own cell, the surface's plane or which
  way it faces. INFERRED: so retail lights the visible face of any wall
  whose box the light's cube reaches, from behind the wall too, and on a
  lightmapped surface at full strength (the copy drops the falloff).
- VERIFIED, 0x4907f0: the efx `Light` colour lerps from `rgb start` to
  `end` only under the curve's `linear` flag (bit 0x10 of `+0x18`);
  without it the colour holds `start`. `fx/explosions/grenade2.efx` (the
  frag's default `grenade_explode` row) is `rgb { start 0.8392 0.8118
  0.6627 end ... }`, unflagged, and `size { start 800 flags linear }`.
- VERIFIED, 0x515680, called from the soup loader 0x4da2e0: the tangent
  frame is built once per soup at load. Per triangle, 0x5155d0 takes
  `(t1 - t2) P0 + (t2 - t0) P1 + (t0 - t1) P2` (t the base texture's
  second coordinate), the in-plane direction along which t holds,
  normalized with the fast reciprocal square root (0x42dd30); the sum
  over a vertex's triangles is normalized (0x42db90) when more than one
  met there, and `T = N x S`, normalized. The surface keeps `S` at `+0x28`
  and `T` at `+0x2c`; 0x512490 copies them in for the pass. A surface
  without them builds a frame from the normal alone (0x4fde50, called
  only from 0x4ff690).
- VERIFIED, 0x4f853d..0x4f8556: a bundle whose image is the lightmap, the white
  image or the dlight image gets the byte at `+0xc4`, which makes it read
  the lightmap's coordinates. So `$dlight` samples at the projected
  coordinates in the pass.
- VERIFIED, 0x4f0150, the dlight image: 64x64, `r_dlightRadius`
  "0.1124278" and `r_dlightShift` "0.3162278"; a texel `d` half-widths
  from the centre (`(x - 31.5)^2 + (y - 31.5)^2` times 0.0010078106 at
  0x569188, 1/31.5^2) reads `K / (d^2 + s^2) - K / (1 + s^2)`, truncated
  and clamped to 0..255 in all three colour bytes, alpha 255, with `K`
  making `d = r_dlightRadius` read 255. It saturates within about three
  texels of the centre and is black from a half-width out.
- VERIFIED, the surface flags the parser ORs into the shader's `+0x5c`
  (table at 0x5717a0, 16-byte rows, the third word): `nodlight` is
  0x20000 and `sky` 4, the two bits 0x500d3a tests.
- VERIFIED, 0x4fc440, the shader built for a surface with no script: for
  a lightmapped surface, stage 0 is the lightmap (rgbGen identity) and
  stage 1 the texture under `filter` (rgbGen identity), which the
  collapse below turns into one stage, texture times lightmap, opaque, so
  0x4fb3c0 gives it the copy. A vertex-lit one (lightmap index -2) has
  the white image in stage 0 without the lightmap byte, so no copy, and
  the pass draws `dlightshader` there.
- VERIFIED, CollapseMultitexture 0x4fb530: two single-bundle stages fold
  into one when their blend pair is in the table at 0x5404d8 (stage 0,
  stage 1, texture environment, result): (none, `GL_ZERO GL_SRC_COLOR`)
  and (none, `filter`) modulate to none; four `filter`/`GL_ZERO
  GL_SRC_COLOR` pairs modulate to `filter`; (none, `add`) and (`add`,
  `add`) take `GL_ADD`; (none, `blend`) takes `GL_DECAL`. The rgbGen and
  alphaGen must match, or stage 1's be identity under `GL_MODULATE`. A
  lightmap in stage 0 swaps behind the texture. Only the two rows that
  end unblended under `GL_MODULATE` leave something 0x4fb3c0 copies.
- VERIFIED, the nextbundle arm of the stage parser (0x4f8ba3, 0x4f8bce, 0x4f8bf3):
  a `blendFunc` after `nextbundle` sets that bundle's texture
  environment (`+0x90`) and restores the stage's own blend: `add` is
  `GL_ADD` (0x104, refused without a `requires` for
  `GL_ARB_texture_env_add`), `filter` and `GL_ZERO GL_SRC_COLOR`
  `GL_MODULATE` (0x2100), `blend` `GL_DECAL` (0x2101), anything else
  "WARNING: bad blendFunc for nextbundle".
- INFERRED: a blended surface with `perLight` stages takes the second
  branch of 0x500d3a, whose list for a world surface is the one at
  0x16c508c (`0x4d6810`: entity 0x3fe), which nothing found here fills.
  vcod draws nothing for those (mp_ship's bridge windows and see-through
  doors).

vcod: the parser keeps `perLight` stages apart (`Shader::per_light`) and
`Shader::dlight_draw` (`shader.rs`) picks what the pass draws per
material: the `perLight` stages, the copy (after the collapse of the
lightmap-then-`filter` pair), or `dlightshader`, or nothing for sky,
`nodlight` and sorts past opaque. The renderer gives a script-less
surface the copy (texture times the dlight image, rgbGen identity; the
masked overlays alpha-tested at 128) or, vertex-lit, `dlightshader`.
Per frame, each scene light (fx `Light` blocks, then the cgame's lights,
at most 32, as the entity pick sees them) gathers the visible soups
whose bounds it reaches by the cube test of 0x4b59f0, moved brush
models with the light carried into their frame, and draws the
material's dlight stages over them after the last world draw that sorts
up to opaque (§5)
(`vs_dlight`/`fs_dlight` in `shader.wgsl`, `crates/client/src/dlight.rs`).
The vertex shader projects as 0x4ff690 does on the soup's load-time s
axis (`dlight::soup_s_axes`, a second vertex buffer), and the fragment
shader drops what a culled triangle would have covered: an outcode bit
interpolates to 1 only where all three vertices carry it. The pass
writes no depth; positions are `@invariant` so `depthFunc equal`
matches the base pass. A dlight stage whose shader is not `nofog` (the
copy, a fogged material's `perLight` stages) fogs towards black, its
colour times one minus the fog amount (`fs_dlight`); the blob is
`dlightshader`'s own stage under its `nofog` (`dlight::stage_shaders`).
Unlike RTCW-MP's `ProjectDlightTexture`, which draws unfogged and so adds
a light's full brightness in fog. A `nextbundle`'s `blendFunc` is the
bundle's texture environment (`TexEnv`; `fs_stage` adds or decals by the
flags), never the stage's blend, or terrain.shader's trench blocks draw
`filter`.

vcod: `TcGen::Environment` and `DepthFunc` in `shader.rs`. The env UV is
computed in the vertex shader (`env_uv` in `shader.wgsl`,
`dynamic_model.wgsl`, `viewmodel.wgsl`) with the eye and normal turned
into the model's frame (the moved brush model's pose, the entity's axes,
the viewmodel's motion transform; a world-baked static prop dots the
reflection with its model's axes, section 8), before the bundle's tcMods.

VERIFIED, string search of the 12 stock `maps/MP/*.bsp` against the stock
script blocks: one map references a world block with an
ungated `tcGen environment`, mp_ship, six of them
(`textures/battleship/bridgewindowbroken`, `bridgewindow2broken`,
`shipdoor3a`, `shipdoor3b_trans`, `textures/battleship2a/shipdoor2a`,
`shipdoor2a_trans`): an opaque `environmap_1day` stage under the blended
door or window texture with its lightmap. None references a block with
`depthFunc`; its stype users are `objective_masked` and `pickup_masked`
(`depthfunc equal` on their glow stages), so the world pipelines ignore
it. The env stype users are `metal_env` and `wood_env`.

Classification and ordering: `Shader::classify_stage` sorts each stage into
Opaque / Blend / Additive (dst factor One or SrcAlphaSaturate means additive),
which picks the stage's pipeline (`renderer.rs` `stage_draw_facts`). Where a
draw goes in the frame is its shader's sort and sorted index, not its class
(§5); retail sorts nothing by depth, so no per-batch centroid order.
Per-stage state rides to the GPU in 240-byte `StageParams` slots
(`renderer.rs`; size pinned by a const assert), one bind group per stage, with
animated stages re-evaluated per frame.

Each bundle of a stage picks its own sampler from its clamp bits, so a
repeating bundle 0 beside a clamped bundle 1 keeps wrapping.

## 4. Runtime math

- Wave forms index a 1024-entry sin table built over SIZE-1 degrees of arc, so
  entry 256 is sin(90.088 deg), not exactly 1: `tr_init.c` :1150 builds
  `tr.sinTable` with `i * 360 / (FUNCTABLE_SIZE - 1)`, the /1023 divisor;
  vcod mirrors it in a const table (`build_sin_table`, `shader.rs`) and pins it
  against libm over the full range in a unit test.
- Index rounding is nearest-even: WAVEVALUE (`tr_shade_calc.c` :34) feeds
  `myftol`, which is x87 fistp on the Windows client and rounds to nearest, so
  511.9 indexes 512 where a truncating cast would take 511. Under
  linux-i386 non-asm builds myftol compiles to plain truncation, so table
  lookups are config-dependent in retail itself. vcod uses
  `round_ties_even` (`wave_value`, `shader.rs`), choosing the Windows-client
  behaviour.
- tcMod scroll clamps to the fractional part before adding
  (RB_CalcScrollTexCoords, `tr_shade_calc.c` :1041); scale is RB_CalcScale...:
  1028; transform is RB_CalcTransformTexCoords :1062; stretch is
  RB_CalcStretchTexCoords :88. All affine, so vcod composes them into one
  2x3 matrix per bundle (`bundle_affine`, `shader.rs`).
- tcMod turb is not affine: s rides `(x + z)` and t rides y, both divided by
  1024 (= 1/128 * 0.125 in the C), offset by phase + time*freq, scaled by
  amplitude through the sin table (RB_CalcTurbulentTexCoords,
  `tr_shade_calc.c` :1009). vcod ships `[amp0, now0, amp1, now1]` in
  StageParams and does `sin(worldpos_axis/1024 + now)` in the VS
  (`shader.wgsl`), s on `x + z` as in the C.

- VERIFIED, CoDMP.exe 0x514c00 (RB_CalcWaveColor, the `rgbGen wave`
  case 8 of the colour switch at 0x4ffa60): a table wave is `(base + amp *
  table[fistp((rate * shaderTime + phase) * 1024) & 1023]) *
  identityLight` (the float at 0x16c55b0), then clamped to 0 (0x568e64)
  and 1 (0x568e60) and stored as `fistp(v * 255)` (0x568ec0) in r, g and
  b, alpha 0xff. The clamp follows the identityLight scale, so with
  overbright on (identityLight 0.5) a wave of 1.5 draws 0.75 and only
  past 2 does it saturate.
- vcod: `stage_params` clamps a table wave below at 0 only and the
  stage shaders clamp `tint * identityLight` at 1 (`F_TINT_LIGHT`).
  Clamping to 1 before the scale is the trap: 1.5 then draws 0.5.

### The noise wave

- VERIFIED, CoDMP.exe 0x4dd490 (R_NoiseInit): `srand(0x3e9)` (0x526324
  stores the MSVC `_holdrand`), then 256 times one `rand()` for the value
  table at 0x11a3640, `r * c + r * c - 1.0` with `c` the float 1/32767 at
  0x568ed8 and 1.0 the double at 0x568e40, and one for the permutation at
  0x11a3240, `(r * 256) >> 15 & 0xff`. Not Q3's `rand() / RAND_MAX * 255`
  permutation. CoDUOMP.exe 0x50e7d0 is the same loop (its 1/32767 at
  0x5b9b90, VERIFIED).
- VERIFIED, 0x4dd4f0 (R_NoiseGet4f): each coordinate's integer part is a
  `fistp` of `v - 0.49999999` (the double 0x3fdfffffff000000 stored at
  0x4dd4f7), a floor under round-to-nearest; the value is Q3's: trilinear
  in x, y, z over `table[perm[x + perm[y + perm[z + perm[t]]]]]`, then
  linear between `t` and `t + 1`.
- VERIFIED, 0x514c00 (RB_CalcWaveColor): genfunc 6 takes `base + amp *
  noise(0, 0, 0, (shaderTime + phase) * freq)` and skips the identityLight
  factor (0x16c55b0) the table waves take, then clamps to 0..1 like them.
  CoDUOMP.exe 0x522c20 is the same.
- VERIFIED, 0x5133d0 (TableForFunc): genfuncs 1..5 only; 6 errors. The
  `alphaGen wave` (0x513470), `tcMod stretch` and deform paths go through
  it, so noise reaches only `rgbGen wave`.
- Census: UO `ba.shader` (pakuo01) `textures/ardennes/windows/firewindow`
  (`rgbgen wave noise 0.4 0.1 0 10`) and `firewindow1` (`0.4 0.05 0 12`);
  no 1.1 block.
- vcod: `shader::noise_get4f` and `rgb_wave_value` (tables built at
  compile time from the same LCG); `stage_params` leaves a noise glow out
  of `STAGE_FLAG_TINT_LIGHT`. Elsewhere a noise wave holds its base where
  retail errors.

## 5. Sort tokens and the draw order

CoD 1.1 has its own sort enum, not RTCW's. VERIFIED, CoDMP.exe 1.1's
`sort` keyword (0x4fa5d0..0x4fa85a) compares the token case-blind against
a name table and stores a float at the shader's `+0x58` (0x11e67d0 while
parsing); a token that is not a name goes through `atof`, else "WARNING:
bad shader sort" (0x5470c4):

| name | sort | name | sort |
|---|---|---|---|
| portal | 1 | underwater | 10 |
| sky | 2 | water | 11 |
| ocean | 3 | corona | 12 |
| boathull | 4 | innerBlend | 13 |
| opaque | 5 | outerBlend | 14 |
| decal | 6 | blend | 15 |
| seeThrough | 7 | blend2, blend3, blend4 | 16, 17, 18 |
| banner | 8 | additive | 19 |
| | | nearest | 22 |

So `ocean` (mp_ship's two ocean blocks) and `boathull` (the yacht interior
skins) draw before opaque, and inner and outer blends before plain blends.
VERIFIED, FinishShader 0x4fba30 fills an unsorted block: `sky` 2 (flag 8,
0x4fba64 area), polygonOffset 6, then for a blended stage 0 seeThrough 7
while it writes depth (0x4fbcd1), additive 19 when its destination factor
is `GL_ONE` (0x4fbce3), blend 15 otherwise (0x4fbcef), and opaque 5 for
everything left (0x4fbd3f). The stock blocks name `water` 15 times,
`nearest` 9, `innerBlend` and `outerBlend` 5 each, `additive` and `corona`
3 each, `ocean`, `boathull` and `seeThrough` 2 each, `underwater` once
(string search). `map_sort_token` and `Shader::sort_value` (`shader.rs`)
follow both tables, not RTCW's numbers.

The draw list, VERIFIED from CoDMP.exe 1.1:

- Every draw surface, world or entity, is one 8-byte entry `(key,
  surface)` appended at 0x16c57b4 (count 0x16c57b0, at most 0x10000).
  The world's (0x4d4fb0, through 0x4e1760) and an xmodel's (0x50db20,
  from the entity add 0x50e3e0) build the key the same way:
  `((shader+0x48 | vb << 12) << 18) | entity << 8 | low bits`, the
  entity number shifted at 0x16c5348 (`0x3fe00`, entity `0x3fe`, for
  the world: 0x4e4c7d), bit 2 set for xmodel surfaces.
- 0x4d5070 sorts the entries by the whole key, unsigned (the quicksort at
  0x4d4e70, called at 0x4d50a8); the draw loop 0x4d6710 decodes the
  shader from bits 18..29 and the entity from bits 8..17.
- `shader+0x48` is the sorted index R_SortNewShader (0x4fb790) assigns: it
  inserts the new shader after every shader whose sort (the float at
  `+0x58`) is lower, ties broken by the first stage's image numbers
  (`image+0x58`, bundle 0 then bundle 1), and bumps the index of every
  shader it passes.
- `vb` (bits 30-31) is the surface's vertex buffer kind:
  `0x16d8868` for an xmodel, set by 0x4b8420 to 3 and to 1 or 2 only on
  the NV_vertex_array_range path without `GL_ARB_vertex_buffer_object`.
  INFERRED: with VBOs every surface carries 3, so the order is the shader
  sort first, then the shader, then the entity, and within one shader the
  entities (lower numbers) draw before the world (`0x3fe`).
- The entity number is the refEntity's index in the scene. VERIFIED,
  RE_AddRefEntityToScene (0x4e9a00) copies the refEntity to slot
  `0x16dca2c` (stride 0x2b8, at most 0x3fd: "too many visible models")
  and bumps the count; R_AddEntitySurfaces (0x4d5170) walks the scene's
  entities in that order and stores `index << 8` at 0x16c5348 before each
  add (xmodel `reType` 1 goes to 0x50e3e0). Nothing in the key marks a
  depth-hack entity. INFERRED: the same loop skips a `renderfx & 4`
  (first person) entity while the flag at 0x16c5454 is set, Q3's
  `isPortal`.

So a blended entity surface is not depth-sorted: it draws in its
material's sort among the world's surfaces, and an opaque entity draws
before the world's translucent ones. A world surface of one shader draws
all its stages there, whatever each stage's blend: the draw loop
0x4d6710 hands a run of one shader to the stage iterator, so an additive
glow stage over an opaque base draws at sort 5, and an alphaFunc cutout
of `sort blend` among the blends (INFERRED from the key: no stage state
enters it).

R_SortNewShader's tie-break in detail. VERIFIED, 0x4fb790 walks back from
the shader count minus two over the sorted array at 0x16cf84c and puts
the new shader after the first one that either sorts lower (`+0x58`), or
sorts the same with a stage 0 (`+0x154`) whose bundle 0 image (`+4`)
number is lower, or the same bundle 0 number and a bundle 1 image
(`+0xcc`) whose number is no higher; every shader it passes moves up one
(`+0x48` incremented, 0x4fb82c). A shader with the same sort and no stage
0 is passed, and so is one with the same bundle 0 and no bundle 1. A
missing bundle 0 skips straight to the bundle 1 test. The new shader's
own missing images read 0. VERIFIED, 0x4ec310 (image create) sets
`image+0x58` to the image count plus 0x400 and bumps the count (at most
0x800), so the number is the image's load order. GeneratePermanentShader
0x4fb850 calls it once per shader, after FinishShader, so the stages it
reads are already collapsed. INFERRED (Q3 lineage, not traced in
CoDMP.exe): the world's shaders register per (name, lightmap) as the
surfaces load, in the shader lump's first-use order, after the lightmap
images; so shaders of one sort order by their texture's first load, then
by lightmap page.

vcod (`sorted_places`, `ImageNumbers`, `order_draws`, `merge_by_sort`
in `renderer.rs`): at map load each batch (material x lightmap) is one
shader, registered in batch order; image numbers go to white, the
lightmap pages, then every stage's bundle images (animMap frames
included) as each shader registers; `sorted_places` replays 0x4fb790
over the batches' sorts and `Shader::sorting_stage` images. Each frame
the world's draws, across the world and the moved brush models, order by
that place with a batch's stages back to back in stage order, a moved
model before the world on one shader. Still vcod's own: the props and
the script-less overlays (`decal@`, `*_masked@`) keep sort 5 and sort
after every other opaque; images loaded before the map (the UI's, the
console's) are not numbered, so a world shader whose image reuses one of
them sorts later than in retail; `collapse` covers only the
lightmap-then-filter row of CollapseMultitexture's table.

Entity surfaces sort by material sort, then the material's load order
(standing in for the sorted index; INFERRED: the cgame's images load
after the world's), then instance order; each surface draws its stages
back to back.

The viewmodel. VERIFIED, cgame `0x30036cf0` builds the gun's refEntity
with `reType` 1 and `renderfx` 0x8c (`0x30036e27`, `0x30036e2f`) and
adds it through syscall `0x3d`; `0x300371f0` is called at `0x30033e04`,
after the packet entities (`0x30033df5`, `0x30033dfa`). VERIFIED,
CoDMP.exe's draw loop `0x4d6710` switches on the surface entity's
`renderfx & 0x18` (`0x4d69bd`, table `0x4d6c78`/`0x4d6c88`): 0 sets the
depth range 0..1 (`0x4d6a56`), 8 loads the projection at `0x16d8cd0` and
sets 0..0.2 (`0x4d6a5d`-`0x4d6a83`), 0x10 and 0x18 set 0..0.5
(`0x4d6a8a`); leaving 8 restores the projection at `0x16d8c90`
(`0x4d6a98`). INFERRED: the gun has no list of its own; its surfaces sort
by material with the world's and the entities', after the entities within
one shader (a later entity number), and draw in the front fifth of the
depth range with the depth-hack projection (`xmodel-v14-format.md`, "The
view fov"). So an opaque gun draws before the world's translucent
surfaces, which then fail the depth test against it, and a blended gun
surface draws before a world surface of a higher sort. INFERRED: within
one sort, R_SortNewShader's image-number tie puts the gun's materials
after the world's (the cgame registers its weapon models after the world
load), and on a material it shares with a packet entity the gun draws
after it, since the gun is added later (`0x30033e04`). vcod
(`sorted_ent_draws`, `merge_by_sort`, `dlight_slot`): the live entities'
and the gun's surfaces form one list sorted by material sort, load order,
then entity with the gun last; it merges with the world's draws by sort,
a world draw first on a tie. The gun's runs draw in the viewport depth
range 0..0.2 under its own camera, the rest in 0..1. The dlight pass goes
after the last world draw that sorts up to opaque.

Effects. INFERRED: an efx particle reaches the renderer as a polygon
with its material, so it takes the same key and sorts by material sort
among the world's surfaces; between equal sorts, R_SortNewShader's tie on
image numbers puts the effect's material after the world's, which load
first at map load. The stock `gfx/...` blocks (effects and HUD art) sort blend 15
(70 blocks), additive 19 (87), nearest 22 (8) or opaque 5 (15). vcod
(`merge_by_sort`, `set_fx_quads` in `renderer.rs`): the fx quads order by
material sort, keeping the sim's back-to-front order within one, and
merge into the frame's one list with the world's draws and the entity
and viewmodel surfaces by sort; on a tie the world first, then the
models, then the fx (the fx-after-models tie is vcod's guess: the
cgame's registration order of effect and model materials is not
traced). A script-less effect material sorts additive when the fx pass
draws it additive, blend otherwise. Still vcod's own: within one sort an
effect's materials keep the sim's order instead of a sorted index; the
fx test against the gun's depth.

## 6. Sky

Stock skies are always farbox + cloud dome: `skyParms env/<name> 512 -`, no
inner box anywhere in the corpus (§2). The geometry is transcribed from
`tr_sky.c`:

- Cube axis to env-image suffix comes from ParseSkyParms loading
  {rt, bk, lf, ft, up, dn} into outerbox[0..5] while DrawSkyBox renders axis i
  with outerbox[sky_texorder[i]] and sky_texorder is {0,2,1,3,4,5}
  (`tr_sky.c` :374), giving +x rt, -x lf, +y bk, -y ft, +z up, -z dn
  (`AXIS_SUFFIX`, `sky.rs`).
- Box distance is zFar/1.75 ("div sqrt(3)") with a 2*zNear floor so it never
  near-clips (`box_size`, `sky.rs`).
- Cloud UVs come verbatim from R_InitSkyTexCoords (`tr_sky.c` :749,
  radiusWorld 4096): intersect the ray with the sphere of radius
  radiusWorld+height centred at (0,0,-radiusWorld), re-centre, normalise, acos
  x and y (`cloud_uv`, `sky.rs`). The raw radians are the UVs; stage tcMods
  scale them onto the repeating cloud texture.
- The dome is five cube faces (FillCloudBox skips the bottom), grid indices in
  FillCloudySkySide's pattern (`tr_sky.c` :571, index loop :597).
- As in RB_StageIteratorSky (`tr_sky.c` :994), if the farbox's first side
  image fails to load the whole box is skipped; the dome still draws through
  the stage path (`renderer.rs` sky-pass block).
- Which block owns the sky pass: highest soup reference count wins, ties to the
  alphabetically first name (`pick_sky`, `sky.rs`).

noville carries the only prominent scrolling cloud stage in the corpus:
`map textures/skies/noville_clouds.tga`, src_alpha blending, tcMod scroll
0.004 0.004 then scale 3 3, gated `requires cvar scr_gmi_fast == 0`
(VERIFIED: read out of `uo/pakuo01.pk3 : scripts/gmi_sky.shader`,
`textures/skies/noville`).

## 7. CoD-only grammar and requires-gating

Four pieces of the corpus have no RTCW parser counterpart:

- `nextbundle` (two-bundle stages), everywhere: §2 numbers.
- `requires <atom> [|| atom]...` stage gating. Atoms are GL extension
  identifiers, optionally with a relational operator and number, or
  `cvar <name> <op> <value>`; a top-level requires attaches to the following
  stage only. The census hits are NV/ATI hardware paths: GL_NV_texture_shader,
  GL_NV_register_combiners, GL_ATI_fragment_shader, texEnvCombine variants,
  `waterMap`, cubemap nextbundles, tcGen
  environment / nv_dot_product_reflect_cube_map_eye_from_qs_*, sunHalfAngle,
  all confined to requires-gated stages except the ungated `tcGen
  environment` of stalin_1wet and mp_ship's door and window blocks, which
  vcod draws (§3).
- CoD sort names (§5).
- `sunfile <map>` on sky blocks.

vcod evaluates gates against a fixed low-spec profile: the six NV/ATI/cubemap
extensions count absent (`CAPS_ABSENT`, `shader.rs`), GL_MAX_TEXTURE_UNITS_ARB
is 2 (`TEXTURE_UNITS`: a stage draws two bundles, and `nextbundle` opens the
second and last), `sys_cpuMHz >= N` is false and other cvars true. At 4
units six three-bundle stages pass (`ash_embers`, `charred_embers`,
`solidcharred_embers`, `sewercaustic` and mp_ship's two deck flags'
`perLight` stage) and draw their third bundle's blend over the second's; at
2 those surfaces take the two-unit stages the scripts carry for them, and the
deck flags take `dlightshader`. No other stock stage differs between 2 and 4
(diff of every parsed block, 1.1 and UO). The profile drops the
hardware-specific stages and keeps the fallbacks, what retail did on machines
without the extensions: the gated stages exist because the ungated fallback
had to render the surface. A failed gate skips the stage body to its closing
brace, so hw-path fields (register combiner setup and the like) are never
parsed. Unit tests pin each rule (`shader.rs`, `eval_requires`). The profile
is INFERRED from assets, not disassembled; retail's evaluation order is
unread, but the kept and dropped stages on stock assets are deterministic
under it and match the fallback shapes.

## 8. Deliberately not implemented

Each omission with its census justification:

- NV/ATI hardware-path stage contents (`waterMap`, register combiners,
  nv_texshader tcGens, sunHalfAngle). Every occurrence but stalin_1wet sits
  behind a `requires` gate this profile fails, so dropping them reproduces
  retail on fallback hardware. The ungated `tcGen environment` stages
  (stalin_1wet, mp_ship's doors and windows) draw (§3).
- Vertex deforms, model skin scripts and the sun: the three subsections
  at the end of this section.
- Fog: the script keywords stay inert, but stock MP fog does not come from
  them. `fogvars` / `skyfogvars` occur only in SP sky blocks (pak4/pak9
  `sky.shader`), `waterfogvars` nowhere at all. `nofog` sits only on fx,
  gfx and sprite blocks (`flareShader`, `sunFlareShader`, `markShadow`,
  `dlightshader`, the head icons).
- VERIFIED, 0x4faf39: the parser sets shader flag 0x400 for `nofog`.
- INFERRED from RTCW's `noFog`, which drops the fog pass: such a shader
  takes no fog. The read of 0x400 is not traced. vcod's stage path skips
  fog for it.
- The fog every MP map shows is set by gsc
  (`setCullFog` / `setExpFog`, e.g. `mp_ship.gsc`) and reaches the client as
  configstring 12, which vcod parses and applies (wire format in
  docs/protocol-1.1.md, mode rules from RTCW-MP tr_main.c R_SetFog).
  The exp-fog density rides the wire raw: retail's mp_chateau `re` gamestate
  carries `0 1 1e-05 0 0 0 0` for `setExpFog(0.00001, 0, 0, 0, 0)`
  (`crates/server/tests/fixtures/configstrings/mp_chateau-re.txt`, VERIFIED).
  Whether the client adds RTCW's +0.1 before `R_SetFog` is a client question
  this capture does not answer.
- Missing images bind retail's `*default`: 16x16 of RGBA (32, 32, 32, 32)
  inside a one-pixel opaque black border. `R_CreateDefaultImage`, CoDMP.exe
  1.1 @ 0x4f0380 (`mov eax,0x20202020` + `rep stos` over 0x400 bytes, then the
  four border runs written RGB 0 / alpha 0xff, registered 16x16 under the
  `.rdata` string `*default` @ 0x54b57c). `R_FindShader` reaches it through the
  `Couldn't find image for shader %s` branch @ 0x4fc7fc, which flags the
  material and lets FinishShader return the default shader. INFERRED from
  decompilation; a side-by-side with a retail frame is pending (human).
  `assets::default_image` matches those bytes, so a material whose file no
  pak ships reads as a dark surface, not a placeholder pattern (e.g.
  `textures/cod2/egypt_wire_black` on the custom map mp_burgundy, which
  ships it in neither of the server's referenced paks).
- `$dlight` bundle images: the engine-generated dlight image
  (`assets::dlight_blob`, retail's formula from 0x4f0150), never a file
  on disk (corpus whitelist in `shader_corpus.rs`, which cites the pak9
  window.shader occurrence). Every stock one sits in a `perLight` stage,
  drawn by the dlight pass (section 3). The mp_ship flag decks bind 1x1
  white because `textures/battleship/deckflag_np.tga` (flagfore/flagaft's
  second bundle) ships in no stock pak under any extension; retail binds
  its own default image there too, so both stay in the corpus whitelist.
- `heightToNormal` is ignored (a debug note). (`clampy` sits on stock
  foliage and water, `clampx` on seven `shadertypes/world/*_clampx.stype`;
  both clamp their one axis, section 3.)
- `nolightscale` (UO) is consumed; what reads its flag 0x2000 is not
  traced. `portal` sorts but draws no portal view. The stage `detail` flag
  and `fog on|off` are consumed inertly.

### Vertex deforms

The 1.1 stage parser hands `deformVertexes` to 0x4f9d20, which stores up to
three deforms per shader in 0x34-byte records and warns
`MAX_SHADER_DEFORMS` past that (VERIFIED, the string at 0x54742c and the
`cmp 3` before it). The subtype strings it compares, in order, with the
deform number each stores (VERIFIED, the `mov ecx,<string>` before each
compare at 0x4f9dc2..0x4fa24f and the stores after them):

| subtype | args | deform | runtime (switch at 0x5149b0) |
|---|---|---|---|
| projectionShadow | - | 8 | 0x513290 |
| autosprite | - | 9 | 0x514080 |
| autosprite2 | - | 10 | 0x514330 |
| text0..text7 | - | 11..18 | 0x513c20 |
| bulge | width height speed | 6 | 0x513ad0 |
| wave | div func base amp phase rate | 1 | 0x513570 |
| flap | s, x, t or y, then div func base amp phase rate | 2 (s, x) or 3 (t, y) | 0x5136e0 |
| normal | amplitude frequency | 4 | 0x513990 |
| syncNormal | scale | 5 | 0x513870 |
| move | x y z func base amp phase rate | 7 | 0x513b70 |

- VERIFIED, the record (stores in 0x4f9d20): deform at +0x00, move vector
  at +0x04, wave form +0x10, base +0x14, amplitude +0x18, phase +0x1c,
  rate +0x20, spread (`1 / div`) +0x24, bulge +0x28..+0x30. A div of 0
  warns "illegal div value of 0" and stores 100.0 (0x42c80000) as the
  spread.
- VERIFIED: `syncNormal` warns "must follow deformVertexes wave" unless the
  previous record is a wave (deform 1), copies it and scales its amplitude.
- VERIFIED, 0x513570 and 0x5136e0: when the rate is 0 the table index is
  `phase * 1024` for every vertex (no spread term); otherwise
  `(t * rate + (x + y + z) * spread + phase) * 1024`, rounded by `fistp`.
  The vertex moves along its normal by `base + amp * table[index & 1023]`.
- VERIFIED, 0x5136e0 and its two call sites: flap is that wave times the
  vertex's base texture coordinate, `s` for deform 2 (`eax` 0 at 0x514a01)
  and `t` for deform 3 (`eax` 1 at 0x514a0a), indexing the 2-float
  texcoord array at 0x17dff60. A flag's hoist edge (s = 0) holds still.
- VERIFIED, 0x514080 (autosprite): every four tess vertices become one
  quad. The centre is their average (`* 0.25`, 0x568ed0); the half-width
  is the first vertex's distance to it times 0.707 (0x5691cc); the quad is
  stamped through 0x50fe20 along the view's left and up axes (0x16d8b60,
  0x16d8b6c) with texture coordinates 0,0 to 1,1 and the first vertex's
  colour. A vertex count not a multiple of 4 warns "Autosprite shader %s
  had odd vertex count".
- VERIFIED, 0x514330 (autosprite2): per six indices it measures the six
  triangle edges, takes the two shortest as the quad's ends, and moves
  each end's two vertices to its midpoint plus and minus
  `normalize((mid1 - mid0) x forward) * 0.5 * |end|` (forward 0x16d8b54,
  the 0.5 at 0x568f38). INFERRED, from the decompile's pointer shuffling:
  which vertex of an end takes the minus side follows the edge's direction
  in the index list, as Q3's AutospriteDeform2 does.

- VERIFIED, 0x513ad0 (bulge): `now` is the integer at 0x16d8a08 times
  the speed (+0x30) times 0.001 (0x568e6c); per vertex the index is
  `(width (+0x28) * s + now) * 162.97466` (0x568e68, 1024 / 2pi) through
  `_ftol` (0x538be0), masked to 1023 into the sin table at 0x16d3850, and
  the vertex moves along its normal by that times the height (+0x2c).
  INFERRED: 0x16d8a08 is the refdef's millisecond time (0x4d6d98 stores a
  `Milliseconds` call there and 0.001 of it at 0x16d8a10, Q3's
  `backEnd.refdef.time` / `floatTime` pair).
- VERIFIED, 0x513b70 (move): one table wave (0x5133d0 by the form at
  +0x10, index `fistp((rate * shaderTime + phase) * 1024) & 1023`, times
  the amplitude plus the base) scales the vector at +0x04..+0x0c, added to
  every vertex.
- VERIFIED, 0x513990 (normal): per vertex, each normal component adds the
  amplitude (+0x18) times `R_NoiseGet4f(0.98 x + k, 0.98 y, 0.98 z,
  shaderTime * frequency (+0x20))` with `k` 0, 100 (0x568eb0) and 200
  (0x569074) for x, y and z (0.98 at 0x569078), then 0x42dd30
  renormalizes. The positions do not move.
- VERIFIED, 0x4f9d20: bulge with fewer than three numbers warns "missing
  deformVertexes bulge parm" and stores no deform; normal with fewer than
  two warns "missing deformVertexes parm". `move` reads three numbers into
  +0x04.. and then the wave.

Census over the stock 1.1 paks (VERIFIED, `pak[0-9].pk3` script blocks
cross-referenced against BSP materials and the skin names in every
`misc_model` xmodel of the 12 stock MP maps):

- `wave`: `textures/sfx/mp_ship_ocean` and `ship_ocean` (mp_ship's sea),
  `raildisc`, `textures/sfx/stalingradwater_pj` (with `syncNormal`).
- `flap`: `skins/shipflag_np` (mp_ship) and `skins/shipflag2_np`
  (mp_powcamp), `flap s 32 sin 4 8 0 .75`; `skins/russianboatflag` and
  `skins/firetexture` on models no MP map places.
- `autosprite`: `skins/light_sprite`, the glow card of every lit lamp model
  (`light_cagelight_white_on`, `light_hangingbulb_on`,
  `light_ind_ceiling1on`/`2on`, `light_lantern_on`, ...), placed on all 12
  MP maps; `skins/green_corona` (holophane lamps, no MP map) and the four
  `textures/sfx/corona_*` blocks (no stock BSP references them).
- `autosprite2`: `skins/searchlight_beam` (`searchlight_large_on`, no MP
  map) and `textures/germany/foliage/treespritetest` (test.shader, no
  reference).
- `bulge`, `move`, `normal`, `projectionShadow`, `text0-7`: no stock block.

vcod (`vcod_common::deform`, `vs_stage` in `shader.wgsl`):

- wave and flap displace in the vertex shader from their record,
  `sin` in place of the 1024-entry table; flap scales by `uv.x` or `uv.y`.
  A rate of 0 drops the spread term, as retail does.
- autosprite: at load each quad's vertices take the centre as position,
  their stamp corner as uv and the half-width in `lm_uv.x`, and the quad's
  indices become the stamp's (0, 1, 3, 3, 1, 2). The vertex shader offsets
  along the view's right and up, read off `view_proj`'s first two rows.
  Props are encoded in `props::build`, map soups in
  `encode_world_sprites` (renderer).
- autosprite2: at load each vertex takes its end's midpoint, the axis
  between the midpoints as its normal and its signed half end-length; the
  vertex shader crosses the axis with the view forward. vcod picks each
  vertex's side as the one that puts it back where it was seen face on.
- Deformed props draw two-sided like every prop; a stock autosprite stage
  is additive, so winding does not show.
- bulge rides its record (`[width, height, t * speed]`, `sin` in
  place of the table), move its record (the frame's offset,
  evaluated on the CPU through the table), normal
  its record with the noise in `noise.wgsl` (the tables
  `shader::NOISE_TABLE` / `NOISE_PERM`, held equal by a test). The normal
  feeds `tcGen environment` and the dlight projection.
- VERIFIED, 0x5149b0 (RB_DeformTessGeometry): a loop over the shader's
  record count (`+0xac`) from record 0 at `+0xb0`, 0x34 bytes apart,
  switching on each record's deform number; every case works in place on
  the tess arrays, so a record reads the positions and normals the one
  before it wrote. VERIFIED, 0x4f9d20: a fourth `deformVertexes` warns
  `MAX_SHADER_DEFORMS` after reading only its subtype; every stored
  subtype, the sprites, `projectionShadow`, `text0-7` and `syncNormal`
  included, takes a record. A bulge or normal short of numbers, a bad
  flap axis and a `syncNormal` not after a wave store none.
- VERIFIED, the four stage iterators (Generic 0x500ba0, ATI 0x50b9c0, NV
  0x50c870, ARB 0x50d970, each by its `--- RB_StageIterator...( %s )`
  string) call 0x5149b0 first; 0x4b8420 picks one of them into
  0x16d8890 for the GL path, and FinishShader (0x4fba30, the store at
  0x4fbdbc) copies it into every shader's iterator pointer except a
  sky's (shader flag 8, which takes 0x4e84d0, 0x4fbdca). INFERRED: so a model's surfaces, which
  fill the same tess arrays (0x505580 above), run their shader's deforms
  as world surfaces do; the deform reads the tess positions, which for
  a model are its own frame's (the entity's transform rides the GL
  modelview).
- vcod: `Shader::deforms` keeps the records in script order, three at
  most (`MAX_SHADER_DEFORMS`); `StageParams.deform` carries the drawable
  ones with `deform_kinds` (retail's numbers) and `deform.wgsl`'s
  `deform_vertex` runs them in order on the position and normal, in the
  world pass, the dlight pass, the shadow cutouts, the entity pass and
  the viewmodel. The entity pass deforms the posed model-space vertex
  before the instance transform; the viewmodel's bones are view space,
  so there the wave's `x + y + z` and the normal's noise read view-space
  coordinates (no stock viewmodel skin deforms). The sprites stay
  load-time encodings (world soups and props only) and hold their slot
  without a GPU record. Records run in script order on the normal the
  previous one wrote, never one per kind in a fixed order on the
  undeformed normal.
- Inert: syncNormal, projectionShadow, text0-7. None has a stock user.

### Model skin scripts

- VERIFIED: the xmodel loader formats `"skins/%s"` (0x54650c, pushed at
  0x504352 and 0x504838) with the surface's skin name and calls
  `R_FindShader` (0x4fc5c0) with lightmap index -1, the model-skin type.
  `R_FindShader` copies the name up to the first `.` before looking it up,
  so `light_sprite.tga` finds the script block `skins/light_sprite`.
- INFERRED, 0x4fc5c0's branch on `strrchr(name, '@')`: a name with an `@`
  goes to the shader-type path (0x4fc0c0, `.stype`), any other to the
  script lookup and then the implicit model skin (0x4fc440 case -1).
- INFERRED, instruction order in 0x4fc5c0: the `@` test reads the copy cut
  at the first `.`, so `lamp.v2@glow.tga` looks up `skins/lamp` as a plain
  skin. 0x4fc0c0 takes the type from between the cut name's last slash and
  its last `@`, and warns "found '/' or '\' after '@'" and returns no
  shader when a slash follows the `@`. When 0x4fc0c0 finds no type or no
  image it returns 0 and 0x4fc5c0 returns the default shader (0x16c4e08).
- VERIFIED, 0x4fc0c0: after the slash test it looks the type up as a
  shader named `@` plus the type's directory (`model/` for a skin,
  `world/` and `2d/` beside it at 0x546c00) and the type (0x4fbe80); on
  a miss it loads the `.stype` (0x4fbf20) and returns 0 when that fails.
  It then loads the skin's image (0x4eed80, by the cut name) for every
  bundle whose image slot holds the `$texturename` marker (`+0x50 ==
  10`), and returns 0 with "WARNING: shadertype '%s' couldn't load image
  for '%s'" when one fails. 0x4fc5c0 returns the default shader
  (0x16c4e08) for any 0.
- VERIFIED, 0x4fd780 builds that default shader, `<default>`: one stage
  of the default image (0x16c4d58), state bits 0x100 (depth write, no
  blend), its rgbGen word (stage `+0x664`) left 0, which the colour
  switch at 0x4ffa60 takes through its default case: identityLight's
  byte (0x16c55b4) in all four channels.
- vcod: `props::skin_script` and `props::skin_type` cut at the first `.`
  before either test; `skin_type` refuses a slash after the `@`.
  `entity_material::load` draws `shader::default_shader` (the
  `*default` image under `identityLighting`) for a typed skin whose
  template is missing, has no stage, or names `$texturename` without the
  skin's image, and so does a static prop (`prop_skin_draw`). VERIFIED,
  census (`a_typed_skin_retail_cannot_build_draws_the_default_shader`):
  the 5171 typed skin slots of the 1012 stock 1.1 xmodels all build, so
  no stock model draws it. Drawing the implicit skin there would be
  wrong.
- VERIFIED, census: 12 skins placed on stock MP maps have a `skins/` block:
  `light_sprite` (autosprite, additive), `lightbulb_on`, `ceilinglight1on`,
  `ind_lamp1_on`, `lantern_on`, `exteriorlamp2_on` (all `rgbGen
  identityLighting`, so the lit bulbs ignore the grid's shade),
  `lanternglass` and `whiteglobe` (blended glass), `shipflag_np` and
  `shipflag2_np` (flap, `lightingDiffuse`, alpha-tested),
  `yacht_int_floor` and `yacht_int_side` (`sort boathull`, `depthfunc
  always`).
- vcod: prop batches whose skin has such a block draw through the stage
  path (`prop_scripts` in `load_world`), slotted after the map's
  materials and sorted like them. A `lightingDiffuse` stage there takes
  the prop's baked grid colour (`F_LIT` in `stage_colour`, the same
  rescale as `vs_prop`).

#### Static props and `type@` templates

- VERIFIED, pak4 `shadertypes/model/metal_env.stype`: stage 1 maps
  `textures/sfx/environmap_1day.jpg` under `tcgen environment`, stage 2
  maps `$texturename` with `blendFunc GL_ONE_MINUS_SRC_ALPHA
  GL_SRC_ALPHA`, both `rgbgen lightingdiffuse`, no `alphaFunc`. The skin's
  alpha is how much of the environment map shows through, not a mask.
- VERIFIED, asset: mp_chateau's `misc_model` `xmodel/staffcar` (origin
  -281 1099 111, the car before the main building) skins its body
  `metal_env@staffcar.tga` (512x512, 32-bit, alpha under 128 on 260713
  of its 262144 texels) and its hubs `metal_env@staffcarhub.dds` (BC2);
  `vehicle_peugeot_static` (1979 675 144) skins `metal_env@car.dds` (BC3).
- VERIFIED, census of every `misc_model` skin on the stock MP maps (1.1
  paks; UO's `uo/` over 1.5's `main/`): the skins whose image has alpha
  under 128 are the alpha-testing types (`metal_masked`, `foliage_masked`,
  `wood_masked`, `cloth_masked`, `foliage_detail`, UO's `*_cullnone*`),
  the blended ones (`glass_blend_in`, `glass_blend_out`, their `_broken`
  forms, `decal` on mp_ship, UO's `foliage_blend` and `foliage_blend2` on
  mp_cassino and mp_kursk), `metal_env` (the three above; `car.dds` on
  UO's mp_bocage too), UO's `concrete@o_br_prp_potted-plant-0[1245]`
  (one opaque stage; mp_cassino, mp_italy, mp_sicily), and skins with a
  `skins/` block. No skin without a block or an `@` cuts.
- vcod: `fs_prop` drops texels under alpha 0.5, which suits only a
  template with one alpha-testing stage. A prop skin without a `skins/`
  block draws by its template
  (`renderer::prop_skin_draw`): more than one stage or a blend goes
  through the stage path with `$texturename` bound to `skins/<skin>`
  (`entity_material::bind_texture_name`); one alpha-testing stage keeps
  the cut; one opaque stage, or the implicit skin, has its image alpha set
  to 255 at load, as the world types are. Through the cut alone the staff
  car draws only the chrome and the glass blends draw as cutouts.
- VERIFIED, 0x505580 (`RB_SurfaceStaticModel( %s )`, string 0x5463e8):
  the uncached static-model surface copies its vertex positions
  (surface `+0x1c`), texture coordinates (`+0x14`) and, when the shader
  word at `+0x50` has 0x102 set, its normals (`+0x18`) into the tess
  arrays untransformed. 0x4e57e0, the static-model cache builder, takes
  the same `+0x1c` positions through the placement's axis and origin
  (`+0x1c..+0x4c` of its record) into 0x18-byte vertices of coordinates,
  colour and position, with no normal. INFERRED: `+0x1c` is model space,
  so the uncached draw runs under the placement's orientation (the
  surface loop at 0x4d68a2 calls 0x4d3630, Q3's `R_RotateForEntity`, for
  any entity that is not the world), and 0x514d40 reflects in the
  model's frame; the cache, having no normals, cannot carry an
  environment stage.
- vcod: a skin whose script or template takes `tcGen environment`
  (`Shader::reflects`) batches per placement (`props::Batch::model_axes`,
  the model's left and up axes in the world), and that batch's copy of
  the shader carries them (`Shader::model_axes`) into `StageParams`'
  `vec*_s`/`vec*_t`, which `env_uv` dots the world reflection with.
  Reflecting in the world's frame is wrong: the staff car stands at yaw
  93, so its side panels (model normal +/-y, world +/-x) seen straight on
  would reflect `s` 0.5, the middle of `environmap_1day.jpg` (128x128,
  luminance 254 there), where retail's reflect `s` 0 or 1, the tree line
  (luminance 25 and 38). VERIFIED, asset: the body skin's alpha
  averages 43 of 255 and sits near 64..100 on the panels, so the panels
  take 25..40% of the map; the column bands of the map average 69, 168,
  180, 104 and 51 left to right.
- VERIFIED, the rest of the template against the renderer: both stages
  `rgbGen lightingDiffuse` (gen 10), which 0x4ffa60 writes white under
  `GL_LIGHTING` on a map with lump-19 lights and the cache's lit bytes
  otherwise (`cod11-light-grid-and-leaf-lights.md` sections 8, 9; vcod's
  `F_LIT` takes the baked colour); no `alphaGen`, so the alpha switch's
  case 0 writes 0xff and the blend weights by the skin's alpha alone;
  0x514d40's formula as section 3.
- VERIFIED, 0x4f3783..0x4f37b4: a stage image other than `$whiteimage`,
  `*white`, `$dlight` and `$screen` is swapped for the image at
  0x16c4dec while the cvar at 0x16c3788 is non-zero. That cvar is
  `r_graymap` (registered at 0x4b3c49, default "0"), so stock settings
  load the skin and the map themselves.
- Snapshot entities and the viewmodel already took the template's stages
  (`entity_material::load`); their shaders discard only by a stage's
  `alphaFunc`.

#### Entity models

- VERIFIED: the renderer's model registration export (refexport slot
  0xca26d4, stored at 0x4b4a88) is 0x503400 (`RE_RegisterModel`, its
  failure string names it). A name starting `xmodel/` goes to 0x502bf0,
  which copies `"skins/"` (0x546698, loaded at 0x502cf3) in front of each
  surface's skin and calls `R_FindShader` (call at 0x502d7a) with
  lightmap index -1, the same lookup as the map-model path above. The
  third argument is 1 and the image type is the caller's.
- VERIFIED: CoDMP.exe holds `skins/` twice (0x54650c, 0x546698) with
  three references (0x502cf3, 0x504352, 0x504838); cgame_mp_x86.dll holds
  none. So no client model (player, weapon, viewmodel, pickup, script
  model, corpse) has a second resolution path.
- INFERRED: cgame registers every model through the refexport, so the
  viewmodel and its world model share one material per skin; nothing in
  the draw picks a different shader for the first-person view.
- VERIFIED, census over every `xmodel/*` in the stock paks (1012
  models): no player, weapon, viewmodel or pickup skin has a `skins/`
  block. The MP entity user is the S&D bomb: `mp_bomb1` (`setmodel` in
  `gametypes/sd.gsc`) names `timer_face.dds`, whose block draws the face
  lit, then `gfx/misc/timer_hand.dds` clamped, `tcmod rotate -6` and
  `alphafunc ge128`, also lit. Other entity-capable models with blocks are
  single-player or unused in MP: `105` (`skins/105mm`, an env map under
  the blended skin; its `setmodel` in `_utility.gsc` is commented out),
  `vehicle_german_yacht_static` (precached by `mp_ship.gsc`,
  `skins/yacht_window`, alpha-tested), the Stuka and Condor propellers
  (`tcMod rotate 5000`, blended), the parachutes, `wallfire` (flap),
  `holophane_lamp*` (`green_corona`, autosprite), the lit lamp models and
  the flags listed above.
- vcod: `entity_material::load` returns the `skins/` block before the
  implicit skin for any skin without an `@`, for snapshot entities and the
  viewmodel alike. A stage image a block names samples through a
  clamp-to-edge sampler when its bundle is `clamp`. The entity and
  viewmodel shaders run a block's wave, flap, bulge, move and normal
  deforms ("Vertex deforms" above); an autosprite there draws undeformed
  (no stock MP entity carries one).

### World shader types

- VERIFIED, CoDMP.exe: the shader-type path (0x4fc0c0) builds
  `shadertypes/` (0x546cd4, loaded at 0x4fbf3c) + `world/` (0x546c00,
  loaded at 0x4fc182) or `model/` (0x546c0c, at 0x4fc142) + the type +
  `.stype` (0x546ca4), and warns `shader type '%s' is a world shader
  type, but '%s' is %s` (0x546ab0) on a mismatch.
- VERIFIED, pak4 `shadertypes/world/` (71 types): `alphaFunc GE128` in
  `alphashadow`, `autosprite2_masked`, `cloth_masked`, `foliage`,
  `foliage_clamp`, `foliage_clampy`, `foliage_masked`, `foliage_snow`,
  `glass_masked`, `metal_fence`, `metal_fence_nonsolid`, `metal_masked`,
  `metal_masked_solid`, `wood_masked` only. `wood`, `plaster`, `rock`,
  `glass`, `glass_nosight` and the other plain types are one opaque
  `$texturename` stage, `rgbGen exactVertex`, then the lightmap
  `blendFunc filter`: nothing reads the image's alpha.
- VERIFIED, asset: `textures/austria/doors/wood@chateau_exteriordoor.tga`
  (pak1, 256x512, 32-bit) carries alpha 255 on the six panes and 0 on the
  wood; `wood@chateau_extdoorborder.tga` is mostly 0.
- vcod: `fs_main` drops texels under alpha 0.5, which would leave
  mp_chateau's glazed doors only their panes. So the renderer sets a
  script-less, non-overlay material's image alpha to 255 at load when its
  world type draws opaque (no stage alpha-tests, the first does not blend;
  `stype_draws_opaque`, `assets::force_opaque`), which also keeps it out of
  the shadow cut.
  The other alpha-tested types still cut through `fs_main`. VERIFIED, a
  census of every stock `.bsp`'s script-less, non-overlay materials whose
  image has alpha under 128: on the MP maps this turns
  `wood@chateau_exteriordoor`, `wood@chateau_extdoorborder` and
  mp_depot's `glass@window2a` opaque. Blended types (`cloth_blend`,
  `glass_blend_in`) still cut rather than blend.

### The sun: disc, flare, blind and glare

- VERIFIED: worldspawn `sundirection` (string 0x54d1c8) is read with
  `"%f %f %f"` (0x564c44) and passed through AngleVectors (0x42e660, forward
  in `esi` = 0x16c57d0, right and up null) at 0x4dc2c8..0x4dc306. The key
  is pitch, yaw, roll, not a vector. Absent, the direction is (0.45, 0.3,
  0.9) normalised (0x4dd040..0x4dd066). VERIFIED on mp_ship: `-45 110 0`
  read as angles points within 0.999 (dot) of the lump-30 sun light's
  direction (`bsp::mp_ship_sundirection_is_angles_matching_the_sun_light`).
  Read as a vector, it puts mp_ship's moon on the horizon.
- VERIFIED, 0x4e83f0 (called from the sky draw 0x4e8460 when `r_drawSun`
  is set): it returns unless the map has a lump-30 sun light
  (`world+0x114`), and draws the disc (0x4e80c0) and adds the flare
  (0x4e81e0) only while `dot(view forward, 0x16c57d0) > 0`.
- VERIFIED, 0x4e7980: the disc is a quad at unit distance along the sun
  with half-width `r_sunsprite_size * 0.0013110929` (0x569024). vcod scales
  that out to farbox distance; the size is not degrees (read so, the disc
  is several times too large).
- VERIFIED, 0x4e7be0 (run after a `.sun` file loads, 0x4e7f30): it
  registers `r_sunflare_shader` as a shader, halves the flare min and max
  sizes, takes the cosines of the four pairs of angles and turns the six
  fade times from seconds to milliseconds. The cvar defaults are the
  registration strings at 0x4b41b1 on: sprite size 16, flare min size 0,
  max size 2500, min angle 45, max angle 2, max alpha 1, fades 1 and 1;
  blind and glare min angle 30, max angle 5, max 0.75, fade in 0.5, out 3.
- VERIFIED, 0x4e81e0: with `d` the view forward against the sun direction
  and `d > cos(min_angle)`, `f` is 1 past `cos(max_angle)` and linear in
  `d` between; the flare's half-size is `min_size / 2 + f * max_size / 2`
  and its colour (all three channels) `max_alpha * f`.
- VERIFIED, 0x4d9250: the flare's id is -1, its origin the sun light's
  direction with w 0 (a point at infinity, so its depth is 1.0), the
  shader `r_sunflare_shader`, and its pixel radius
  `round(width / 640 * r_sunsprite_size)`; flag 1.
- VERIFIED, 0x4d9310 (the visibility test): the window is `2r + 1` pixels
  square, capped at 65, clipped to the viewport; `glReadPixels` reads its
  depths (GL_DEPTH_COMPONENT, GL_FLOAT) and the fraction is the count of
  depths at or past the flare's depth over the full window area. A window
  entirely off screen reads 1 when the flag is set and 0 otherwise.
- VERIFIED, 0x4d2d90: the visibility eases toward that fraction at
  `dt / fade_in` per millisecond rising and `dt / fade_out` falling.
- INFERRED, 0x4d9770's list walk: a flare not added this frame clears its
  flag, gets depth 2.0 and fades out; it leaves the list at 0.
- VERIFIED, 0x4e8270: blind and glare each ease toward `band * vis` with
  their own fades, where `band` is the 0..1 position of `d` between their
  min and max angle cosines and `vis` the sun flare's raw fraction this
  frame (0x14072fc, written only for id -1); darken is the blind times
  `max_darken`, lighten the glare times `max_lighten`. A max of 0 or less
  reads 0.
- VERIFIED, 0x4d9770 at 0x4d991f..0x4d9a6e: with darken or lighten
  non-zero it draws one viewport quad under GL state 0x10062 (GL_ONE,
  GL_ONE_MINUS_SRC_ALPHA, depth test off; the state setter 0x4d57b0 maps
  bit 0x10000 to `glDisable(GL_DEPTH_TEST)`) with colour (lighten,
  lighten, lighten, darken), then each visible flare's quad (0x4d94b0):
  half-width `size * width / 640`, vertex rgb `identityLight * colour`,
  vertex alpha the faded visibility.
- VERIFIED, `pak4.pk3 scripts/sun.shader`: the three flare blocks
  (`gfx/sun/global_flare`, `lowsun_flare`, `moon_flare`) are one clamped
  stage with `blendFunc gl_src_alpha gl_one`, `rgbgen vertex`,
  `alphaGen vertex`.
- VERIFIED, stock MP sky blocks against the `.sun` files in the stock
  paks: mp_carentan, mp_dawnville and mp_harbor name one that exists
  (`global_flare`), mp_chateau names `chateau.sun` (`moon_flare`);
  mp_brecourt, mp_depot, mp_pavlov, mp_powcamp, mp_railyard, mp_rocket and
  mp_ship name `<map>.sun` files no stock pak ships, and mp_hurtgen's sky
  has no `sunfile`.
- VERIFIED, the world load 0x4dd000 zeroes 0x9c bytes at 0x11dbda0 with
  `memset` (0x4dd035..0x4dd040). Those bytes are the derived sun state:
  the disc shader and quad, the flare shader, the angle cosines, sizes,
  maxima and fade times, and the faded values at 0x11dbe30..0x11dbe38.
  The same function writes 0 to the `sunfile` name at 0x16c4e20
  (0x4dd06b); the shader parser's `sunfile` keyword fills that name
  (`strncpy` at 0x4faeeb).
- INFERRED, 0x4dd000's branch at 0x4dd417: it calls 0x4e7f30 (0x4dd424)
  only when that name is non-empty after the load.
- INFERRED, 0x4e7f30's early return: a file that fails to read prints
  "couldn't load sun file" and returns before 0x4e7be0, the routine that
  refills the state. 0x4e7be0's only other caller, 0x4e840d, runs while
  the cheat cvar `r_suntest` is set.
- VERIFIED: `r_suntest` registers at 0x4b41b1 with default "0".
- INFERRED, the null tests at 0x4e80c5 and 0x4e81e6: the disc returns on
  a null shader at 0x11dbda0 and the flare on a null shader at 0x11dbde8;
  the washes (0x4e8270) ease toward maxima of 0. So a missing `.sun` file
  draws no disc, no flare and no wash, on the first map of a session and
  after any other map alike. The `r_sun*` cvars keep the previous file's
  values, but nothing reads them until a file loads again.
- VERIFIED, census of the 31 `.sun` files in `pak4.pk3`: every one sets
  all twenty keys, so carried-over cvar values could never show anyway.

- VERIFIED, `r_drawSun` registers at 0x4b3965 with default "1" (string
  0x5685e4) and flags 1 (CVAR_ARCHIVE). Its only readers are the sky
  stage iterators 0x4e8460 and 0x4e84d0, each of which calls 0x4e83f0
  (disc and flare) only while its integer is non-zero (0x4e8465,
  0x4e84e4, 0x4e85f9).
- VERIFIED, 0x4e84d0 is the sky shader's stage iterator: the shader
  parser stores it at shader `+0x174` for a block with `skyparms`
  (0x4fb0e3, 0x4fbdca), and the world load collects every soup whose
  shader carries it into the world's sky list (`world+0xa4` count,
  `world+0xa8` list; 0x4db15e..0x4db173), refusing a second sky shader
  ("more than one sky shader", 0x54d474).
- INFERRED, from that call graph: the disc is drawn and the sun's flare
  added only on a frame a sky surface reaches the draw list, between the
  calls at 0x4e8575 (0x4e67e0) and 0x4e8618 (0x4e73f0), the order vcod
  draws box, sun and cloud dome in.
- INFERRED, 0x4e4b70 (called at 0x4e4dfe after the world's cell walk in
  0x4e4c10): with a far plane set it adds every sky-list surface not yet
  added this frame (stamp against 0x16c4d48) whose box reaches nearer
  than the plane, the box corner chosen by the plane's sign bytes; no
  frustum or cell test. The plane is `-forward` at the far distance
  (0x4e4720..0x4e47cf, -1.0 at 0x568e9c); 0x4e1670 returns that distance:
  `r_zfar` (0x16c3710; VERIFIED default "0" at 0x56871c, flags 0x200,
  the cheat flag; vcod does not register it), or
  under linear fog (mode 0x2601) the fog's end, capped at 0x11cc18c. At 0
  there is no far plane (0x4e470f) and the sky comes through the walk
  alone.
- VERIFIED, census over the stock paks alone
  (`sun_census.rs`, `stock_mp_suns`): on 1.1 mp_carentan, mp_dawnville
  and mp_harbor get the `global_flare` flare and no disc (their files
  set `r_sunsprite_shader "0"`), mp_chateau the `moon_full` disc and
  `moon_flare`; the other eight draw nothing. On UO's 35 MP maps,
  sixteen draw some of it: mp_berlin and mp_chateau the moon, mp_foy
  the `global` disc and flare, mp_sicily the `lowsun` pair, and twelve
  `global_flare` only (mp_carentan, mp_cassino, mp_dawnville, mp_harbor,
  mp_neuville, mp_rhinevalley, mp_stalingrad, mp_tigertown,
  mp_uo_carentan, mp_uo_dawnville, mp_uo_harbor and mp_uo_stanjel).
- INFERRED: UO's client draws its sun by 1.1's rules; its client binary
  is unread.

vcod (`sun_flare.rs`, `vcod_common::sun`): the same flare, fades and
washes, in the scene pass after the 3D draws and before the HUD.
`sun::sky_drawn` gates the disc and the flare's add each frame: a world
sky soup in the walk's visible set, or under linear fog one whose box
reaches nearer than the fog's end; `r_drawSun` gates both too. A sky
soup's box spans most of the map, so on every stock map the walk reaches
it from anywhere the sky can be seen (`carentan_spawns_walk_to_the_sky`);
only a sealed interior drops the flare. The visibility test is an occlusion
query over the same window at depth 1.0 (`LessEqual`, no writes), its
passed samples over `n * n * MSAA_SAMPLES`; the result is read back a frame
or more late, so the fades start from a stale fraction. The flare draws
with the stock flare stage hard-coded and the image of the named block's
first stage. A missing `.sun` file draws no sun at all, as retail does,
so the seven stock MP skies above draw no disc, flare or wash.

### Coronas

The world's `corona` entities are sprites the renderer adds itself, not
flares. None of the 1.1 flare machinery draws them.

- INFERRED, the branch in the world load's entity walk 0x4dbf50: a
  `classname` equal to `corona` (`_stricmp`) calls 0x4dbcd0 (0x4dc664), as `misc_model` calls
  0x4dbae0 (0x4dc631).
- VERIFIED, 0x4dbcd0's record (32 bytes, list capped at 512 by the fatal
  `MAX_MAP_CORONAS(%i) exceeded`, 0x4dbcdb): the shader `flareShader`
  (0x4dbd1d); `origin`, default `0 0 0`; radius `scale` times 25.5
  (0x5691ac), scale default 1, and 0 or less is the fatal "corona scale
  must be > 0"; `zcutoff`, default -0.15 (0x5691a8); `zfadeout`, default
  -0.25 (0x56916c); `dl_color`, default `1 1 1`, as bytes, each `c * 255`
  clamped to 0..255 and truncated by `_ftol2`, alpha 255
  (0x4dbe2f..0x4dbf18). The list sits at `world+0xc0`, its count at
  `world+0xc4`.
- VERIFIED, 0x4e4800 is called at 0x4e4ca0 from the world's view setup
  0x4e4c10. No cvar is read in 0x4e4800.
- VERIFIED, the constants 0x4e4800 uses: `sqrt 3` (0x569104) times the
  radius for the cull box's half-size, 16 (0x568ec8) for the move toward
  the eye; the refEntity it fills has type 4 (0x4e4833), the corona's
  shader, radius and colour bytes.
- INFERRED, 0x4e4800's branches: per corona it normalises the vector
  from the corona to the eye (0x42db90) and skips the corona unless that
  vector's z is below `zcutoff` (0x4e48aa), so a corona shows from below.
  The box is then culled against the frustum and the visited cells'
  portal planes (0x4e48b8..0x4e4a41), as a static model's is. The alpha
  is `(zcutoff - z) * 255 / (zcutoff - zfadeout)` truncated while z is
  above `zfadeout`, 255 at or below it (0x4e4acb..0x4e4af4).
- INFERRED, the entity surface switch 0x5128a0: type 4 goes to 0x5101d0,
  which builds `RB_AddQuadStamp`'s quad (0x50fe20) from the view's left
  and up axes (`0x16d8b60`, `0x16d8b6c`) times the radius (rotation 0
  here), s and t from 0 to 1. So type 4 is a camera-facing sprite.
- VERIFIED, `pak4.pk3 scripts/*.shader` `flareShader`: `nofog`, `cull
  none`, `sort corona`, one stage `map gfx/misc/corona_temp.tga`,
  `blendFunc GL_SRC_ALPHA GL_ONE`, `rgbGen vertex` (so `alphaGen vertex`).
- VERIFIED, stock census: mp_brecourt 5, mp_depot 48, mp_harbor 16,
  mp_hurtgen 13, mp_powcamp 60, mp_railyard 33, mp_rocket 53 and mp_ship
  36 coronas (every mp_ship one at scale .3 under a cage light).

vcod (`crate::corona`, `bsp::coronas`): the same sprite each frame through
the marks' stage path (`mark_pass::MarkPass::set_sprites`), so it takes
the shader's sort in the frame's draw list and its alpha from the sprite
(the vertex alphaGen rule of section 3; without it the `zfadeout` ramp
never draws). The cull is the frustum and
the visited cells' bounds (`Vis::prop_visible`), not retail's portal
planes.

### r_flares

- VERIFIED, registrations: `r_flares` "1" CVAR_CHEAT (0x4b386b);
  `r_flareSize` "96", `r_flareFadeIn` and `r_flareFadeOut` "0.2", all
  cheat cvars (0x4b3c73..0x4b3caa).
- INFERRED, 0x4d9770's test of `r_flares`: at 0 the whole flare list is
  skipped: no dlight or world flares added, no visibility test, no fade,
  no wash (0x4e8270 is called inside that block) and no flare quad, so
  the sun's flare, blind and glare go too.
- INFERRED, 0x4d8fd0's test: `r_flares` above 1 adds a flare per scene
  dlight (ids from 0x400, shader at 0x16c4e18, size `r_flareSize`).
- INFERRED, 0x4d9100's test: `r_flares` 1 or 3 adds a flare per refdef
  corona (stride 0x30: origin, colour, scale, id, flags; flag 2 picks the
  shader at 0x16c4e1c), size `r_flareSize * scale`.
- VERIFIED, the refdef corona list's only writer is refexport `+0x54`
  (0x4e9c10, at most 32 a frame, ids below 0x400), reached from the
  cgame's trap 0x43 (dispatch case 67 at 0x402dc3). The 1.1 MP cgame's
  wrapper for that trap (0x30030ff0) has no caller and no stored pointer
  (byte search of `cgame_mp_x86.dll`), so in MP the list is always empty
  (`cod11-light-grid-and-leaf-lights.md` says the same).
- So under default, non-cheat settings 1.1 MP draws one flare: the sun's.
  vcod registers `r_flares` "1" as a cheat cvar; 0 stops the sun flare
  and its washes stepping and drawing. The dlight flares (`r_flares` 2 or
  3, cheat only) are not implemented.

### vc_sunShafts (vcod only)

Not retail: `crate::sun_shafts`. Its sun is the flare's direction (the
lump-30 sun light), so it needs a sun flare: a loaded `.sun` file with a
flare image. Three passes after the scene resolves and before `vc_bloom`
blurs it: the sky pixels (MSAA depth sample 0 at the cleared 1.0) above a
display-value threshold near the sun at half resolution, a 64-tap radial
blur toward the sun's screen position, and an additive composite into the
resolved scene. The HUD draws after, single-sampled. The strength fades
out between the sun's NDC reaching the screen edge and 1.6.

## 9. Verification status

- `stock_shaders_parse_without_warnings` (`shader_corpus.rs`) parses every
  stock `.shader` and `.stype` of the install `COD_DIR` names (1.1, 1.5, or
  UO's `uo/` over `main/`) and fails on any warning: 0 on each (65 unique on
  1.1 and 207 on UO before the line-bound arguments and keyword fixes).
- Corpus tests (`crates/common/tests/shader_corpus.rs`, needs `COD_DIR`):
  >500-block library pin, 250..350 world-reference band, the every-MP-map-has-a-sky
  pin, and every stage image of every authored MP material resolving through
  the renderer's probe chain, which catches paths scripted as .tga but
  shipped as .dds (13 of them; `assets::resolve_bundle_image` handles both).
- Unit tests cover the wave-table ties-even rounding, the sin table against
  libm over the full range, turb axes, requires gating rules, sort mapping and
  sky geometry (zenith UV = pi/2, bottom face omitted, box spans).
- Live fly-throughs on a 1.5/UO install: mp_pavlov (night sky), mp_harbor,
  mp_bocage (overcast hedgerow sky, fence and foliage cutouts, terrain
  detail blends), noville (dusk cloud dome, scrolling cloud stage visible),
  burnville. Zero missing textures on stock main maps; 94-145 fps during the
  passes.
- PENDING HUMAN REVIEW: pixel-truth against the retail renderer. The above
  shows surfaces render as authored; only a side-by-side with the real game
  settles the last percent of blending and sky shading.
- Known divergences, all in §8: NV/ATI hw-path stages dropped (retail
  fallback parity), `deformVertexes` syncNormal, projectionShadow and
  text0-7 inert, script fog keywords inert (no stock MP map sets fog from a
  script; the fog every map shows arrives in configstring 12 and is
  rendered), `$dlight` bundles and the mp_ship deckflag image have
  substitutes rather than files, turb axis follows tr_shade_calc.c.
