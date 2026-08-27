# CoD 1.1 shader scripts

What CoD's material scripts contain, what vcod implements of them, and where
the implementation deliberately diverges. Evidence rules as everywhere in this
directory: VERIFIED means I measured it against the stock assets or saw it in
the running client; INFERRED means read off the RTCW sources or asset shape and
labelled so. RTCW references are `private/reference/RTCW-MP/src/renderer/*.c`
(the closest public ancestor of the CoD 1.1 renderer); vcod references name the
file and line on `feature/shader-scripts`. The vcod-side parser and runtime
math live in `crates/common/src/shader.rs`, application in
`crates/client/src/renderer.rs` and `crates/client/src/sky.rs`.

## 1. What shader scripts are and where they live

CoD 1.1 keeps the Q3/RTCW material-script system: a BSP soup material name is
looked up in a shader library parsed from `scripts/*.shader` blocks across all
mounted paks; a name with no block falls back to being used as a plain image
path. Block syntax is Q3's `{ keyword args ... { stage } }` grammar. Two
CoD-specific placements matter:

- World materials resolve from `scripts/*.shader` in `main/` paks; every
  authored stock block sits in pak4, pak5, pak8 or pak9 of the Deluxe install
  (VERIFIED by census; see §2).
- Effect shaders for the fx system live in `fxshaders/` inside `pak5.pk3`, not
  `scripts/`, and are almost all additive (`blendfunc gl_one gl_one`). This was
  already noted in AGENTS.md; the fx path consumes them as texture sources.
- United Offensive adds its own scripts under `uo/`, e.g. the MP sky blocks in
  `uo/pakuo01.pk3 : scripts/gmi_sky.shader` (VERIFIED: I read the
  `textures/skies/noville` block straight out of that archive).

vcod parses every script entry into `ShaderLib` at load
(`shader.rs` `ShaderLib::load`). A block whose name matches nothing drawn never
costs more than memory.

## 2. Census

Method: parse every `.shader` entry from the stock `pak[0-9].pk3` of my Deluxe
1.5 install (assets identical between 1.1 and 1.5), then cross-reference block
names against the soup-material set of every map in those paks. The throwaway
script I used for the first pass is gone on purpose; the durable form of this
census is `crates/common/tests/shader_corpus.rs`, which re-measures the pins on
every run (needs `COD_DIR`) and fails when parser changes move them. Like the
other census tests it scopes to stock paks only, because live-server downloads
drop third-party `zzz_*.pk3` files into `main/`.

Headline numbers, measured 2026-08-26 on the Deluxe install (all VERIFIED):

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
| rgbGen | 92.9% — exactVertex 180, vertex 88, identity 21, identityLighting 15, const 12, constLighting 11, wave 2 |
| blendFunc | 65.3% |
| alphaGen | 37.3% — vertex 111, const 19 |
| tcMod | 40.7% — scroll 166, scale 93, turb 57, transform 9, rotate 1, stretch 0 |
| tcGen | 26% — vector 90; environment/nv_* only behind `requires` gates except stalin_1wet |
| depthWrite | 18.4% |
| alphaFunc | 18.6% — every occurrence GE128 |
| animMap | one block |

blendFunc pairs in world blocks: src_alpha/one_minus_src_alpha 141,
one/one 51, src_alpha/one 18, dst_color/zero 17, plus the shorthands
(`add`, `filter`, `blend`). `nextbundle` appears in roughly 140 world blocks —
water `$lightmap` bundles, terrain blends, foliage, facades, windows, flags —
so two-bundle stages are core CoD grammar, not an exotic corner.

A `$lightmap` bundle must carry the same overbright as the implicit
texture-times-lightmap path, or coplanar surfaces split in brightness:
`scripts/terrain.shader` overlays (`rgbGen exactVertex` + `alphagen vertex`
over `map $lightmap` `blendFunc filter`, polygonOffset) sit on base ground
soups whose material is the raw image path with no script at all, and the
pair must shade identically for the vertex-alpha cross-fade to read as one
surface (mp_brecourt: 19 staged overlay soups over 65 implicit base soups).
vcod applies its x2 overbright and fx-light term to `$lightmap` bundles in
`fs_stage` for this reason (`shader.wgsl`).

Sky: every stock MP map carries at least one sky-parms material among its soup
materials (pinned by the corpus test's skyless assertion), and every stock
skyParms line is `skyParms env/<name> 512 -`: farbox plus cloud dome at height
512, never an inner box.

## 3. Grammar subset implemented

The parser accepts the keywords below; anything else inside a stage warns once
per (block, message) via `WarnSet` (`shader.rs`) and is skipped. RTCW line
anchors from `tr_shader.c` unless noted.

| token | RTCW anchor | vcod notes |
|---|---|---|
| map / clampmap | ParseStage :529 | image resolved through `assets::resolve_bundle_image` (`assets.rs`) |
| animMap | ParseStage :529 | frame list per bundle |
| nextbundle | no RTCW keyword (CoD-only) | opens bundle 1 of the same stage (`shader.rs` nextbundle arm) |
| blendFunc | ParseStage :529 | full factor pair plus shorthands add/filter/blend |
| alphaFunc GE128 | ParseStage :529 | cutout band; GE128 is the only form the corpus uses |
| rgbGen / alphaGen forms | ParseStage :529 | exactVertex, vertex, identity, identityLighting, const(Lighting), wave; alphaGen vertex/const |
| tcMod scroll/scale/turb/transform/rotate/stretch | ParseTexMod :321 | runtime math in §4 |
| tcGen vector / lightmap | ParseStage :529 | vector basis dots world position in the VS; environment unsupported → stage dropped with a warning |
| depthWrite | ParseStage :529 | explicit keyword beats the blend default (`depth_write()` in `shader.rs`) |
| cull / polygonOffset / nopicmip / nomipmaps / surfaceParm | ParseShader :1367 | surfaceParm sets water/sky flags among others |
| sort | sort-name table tr_shader.c :1224-1242 | mapping in §5 |
| skyParms | ParseSkyParms :1152 | farbox + cloud dome; see §6 |
| sunfile | no RTCW anchor (CoD-only) | argument stored on the block (`Shader::sunfile`), resolved to `scripts/<name>.sun` and drawn as the sun disc (§8) |
| fogvars / skyfogvars / waterfogvars / nofog / tesssize / light / entityMergable / qer_* / q3map_* | ParseShader :1367 | consumed as known tokens; script fog values stay inert, MP fog rides configstring 12 (§8) |

Classification and ordering: `Shader::classify_stage` sorts each stage into
Opaque / Blend / Additive (dst factor One or SrcAlphaSaturate means additive),
and the renderer buckets draws into five emission bands - opaque (incl.
alpha-test cutouts), coplanar decal, see-through, back-to-front blends sorted
by per-batch centroid depth, additives last
(`renderer.rs` `stage_draw_facts`, bands comment and the draw-loop band walk).
Per-stage state rides to the GPU in 176-byte `StageParams` slots
(`renderer.rs`; size pinned by a const assert), one bind group per stage, with
animated stages re-evaluated per frame.

Ruling on blend-sort granularity, recorded so nobody re-litigates it: sorting
keys on per-batch centroids (material x lightmap), not per-soup, so translucent
soups sharing a batch never reorder against each other. The numeric sorts
between SEE_THROUGH and BLEND0 (water 8.75, banner 6, underwater 8) collapse
into one unsorted seethrough band. Fly-throughs on mp_harbor and noville showed
no artifacts from that collapse; I revisit only if overlapping same-material
translucency ever does.

One sampler pair serves every stage (`stage_bind_groups`, `renderer.rs`), so
when either bundle of a stage is clamped both currently sample with
ClampToEdge and a repeat bundle-0 + clamp bundle-1 stage would wrap its
bundle-0 incorrectly; zero known corpus hits.

## 4. Runtime math

- Wave forms index a 1024-entry sin table built over SIZE-1 degrees of arc, so
  entry 256 is sin(90.088 deg), not exactly 1: `tr_init.c` :1150 builds
  `tr.sinTable` with `i * 360 / (FUNCTABLE_SIZE - 1)`, i.e. the /1023 divisor;
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
  (`shader.wgsl`). An earlier draft had the s-axis wrong; the corpus-era
  screenshots caught it and it now matches the C.

## 5. Sort tokens

`map_sort_token` (`shader.rs`) mirrors the RTCW numeric enum
(`tr_local.h` :110-135): portal 1, sky 2, opaque 3, decal 4, seethrough 5,
banner 6, underwater 8, blend0 9, blend1/additive 10, almostnearest 14,
nearest 15. Numeric tokens parse directly and win, as in retail.

CoD-only names found in world blocks and their mapping (counts from the §2
census): water 27 and ocean 2 → 8.75 (between UNDERWATER and BLEND0, where
retail's water plane belongs); outer 9 (+outerblend) → BLEND0; inner 4
(+innerblend, additive 4) → BLEND1; seethrough 2 → SEE_THROUGH. Blocks without
an explicit sort get the FinishShader defaults (`tr_shader.c` :2180): sky →
SS_ENVIRONMENT, then polygonOffset → SS_DECAL, then stage-0 blend with depth
write → SS_SEE_THROUGH, without → SS_BLEND0, no active stage → SS_OPAQUE;
`Shader::sort_value` copies that order exactly.

## 6. Sky

Stock skies are always farbox + cloud dome: `skyParms env/<name> 512 -`, no
inner box anywhere in the corpus (§2). The geometry is transcribed from
`tr_sky.c`:

- Cube axis → env-image suffix comes from ParseSkyParms loading
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
  environment / nv_dot_product_reflect_cube_map_eye_from_qs_*, sunHalfAngle -
  all confined to requires-gated stages except stalin_1wet's ungated
  `tcGen environment`.
- CoD sort names (§5).
- `sunfile <map>` on sky blocks.

vcod evaluates gates against a fixed low-spec profile: the six NV/ATI/cubemap
extensions count absent (`CAPS_ABSENT`, `shader.rs`), GL_MAX_TEXTURE_UNITS_ARB
is 4, `sys_cpuMHz >= N` is false and other cvars true. That drops the
hardware-specific stages and keeps the fallback ones, which is what retail did
on machines without the extensions - the gated stages exist precisely because
the ungated fallback had to render the surface. A failed gate skips the stage
body straight to its closing brace, so hw-path fields (register combiner setup
and friends) are never parsed at all. Unit tests pin each rule
(`shader.rs` tests around the `eval_requires` machinery). The profile choice is
INFERRED-from-assets rather than disassembled; retail's exact evaluation order
was never dumped, but the kept/dropped outcome on stock assets is deterministic
under it and matches the fallback shapes.

## 8. Deliberately not implemented

Each omission with its census justification:

- NV/ATI hardware-path stage contents (`waterMap`, register combiners,
  nv_texshader tcGens, sunHalfAngle). Every occurrence but stalin_1wet sits
  behind a `requires` gate this profile fails, so dropping them reproduces
  retail-on-fallback-hardware. stalin_1wet's single ungated `tcGen environment`
  stage drops too (unsupported tcGen warning); it is one window effect.
- `sunfile` sun disc: rendered since 2026-08-26. The token names
  `scripts/<name>.sun` — a cvar dump (CoDMP.exe loads it via
  `"scripts/%s.sun"`; cvars `r_sunsprite_*` registered at CoDMP.exe
  `0x54cc10`/`0x54cbfc`), of which vcod consumes `r_sunsprite_shader`
  ("0" disables the disc, as mp_harbor.sun does) and `r_sunsprite_size`;
  the flare/glare/blind keys are ignored. The disc direction is not in the
  `.sun` file: it is the worldspawn `sundirection` key (mp_ship.bsp:
  `-45 110 0`, alongside `suncolor`/`sunlight`). vcod draws an additive
  billboard at farbox distance when both a sprite image and the direction
  exist; maps whose active sky block has no sunfile get no disc, matching
  retail. Flare/glare/blind screen effects remain unimplemented.
- `deformVertexes wave`: implemented 2026-08-26 after the ocean blocks made
  mp_ship's water read as a flat sheet next to retail. Only the `wave` form
  exists in a world block (mp_ship-style ocean planes); the BSP already
  ships those surfaces subdivided (one 1052-vertex soup on mp_ship), so the
  displacement runs in the vertex shader with no CPU tessellation:
  `pos += normal * (base + amp * sin(2pi * (phase + t*rate + (x+y+z)/div)))`
  (RTCW-MP tr_shade_calc.c RB_CalcDeformVertexes). Other deform kinds
  consume their args and stay inert.
- Fog: the script keywords stay inert, but stock MP fog does not come from
  them. `fogvars` / `skyfogvars` occur only in SP sky blocks (pak4/pak9
  `sky.shader`), `waterfogvars` nowhere at all; `nofog` sits only on fx/gfx
  blocks, which vcod does not fog. The fog every MP map shows is set by gsc
  (`setCullFog` / `setExpFog`, e.g. `mp_ship.gsc`) and reaches the client as
  configstring 12; vcod parses and applies it since 2026-08-26 — wire format
  in docs/protocol-1.1.md, mode rules from RTCW-MP tr_main.c R_SetFog.
  Open question: whether an exp-fog map's density rides the wire raw or with
  RTCW's +0.1 client-side offset; settle on the first exp-fog server capture
  (neuville/bocage would show a black wall if we over-thicken).
- Missing images bind retail's `*default`: 16x16 of RGBA (32, 32, 32, 32)
  inside a one-pixel opaque black border. `R_CreateDefaultImage`, CoDMP.exe
  1.1 @ 0x4f0380 (`mov eax,0x20202020` + `rep stos` over 0x400 bytes, then the
  four border runs written RGB 0 / alpha 0xff, registered 16x16 under the
  `.rdata` string `*default` @ 0x54b57c). `R_FindShader` reaches it through the
  `Couldn't find image for shader %s` branch @ 0x4fc7fc, which flags the
  material and lets FinishShader return the default shader. INFERRED from
  decompilation; not yet compared against a retail frame. `assets::default_image`
  matches those bytes, so a material whose file no pak ships reads as a dark
  surface rather than a placeholder pattern (`textures/cod2/egypt_wire_black`
  on the custom map mp_burgundy is the case that motivated it: the map
  references it and ships it in neither of the server's referenced paks).
- `$dlight` bundle images: engine-generated light blobs, never files on disk
  (corpus whitelist in `shader_corpus.rs`, which cites the pak9
  window.shader occurrence). vcod binds a generated radial falloff blob
  (`assets::dlight_blob`) since 2026-08-26, so the neuville window glass
  shows a soft glow instead of the default-image placeholder. The mp_ship
  flag decks bind 1x1 white for the same reason that
  `textures/battleship/deckflag_np.tga` (flagfore/flagaft's second bundle)
  ships in no stock pak under any extension; retail binds its own default
  image there too, so both stay in the corpus whitelist.
- `clampY` is approximated (warns once), `heightToNormal` ignored: neither
  occurs in a world block of the stock corpus.

## 9. Verification status

- Corpus tests (`crates/common/tests/shader_corpus.rs`, needs `COD_DIR`):
  >500-block library pin, 250..350 world-reference band, the every-MP-map-has-a-sky
  pin, and the assertion that every stage image of every authored MP material
  resolves through the renderer's probe chain. That last one earned its keep
  during development: it caught 13 paths scripted as .tga but shipped as .dds,
  fixed by routing resolution through the shared `assets::resolve_bundle_image`.
- Unit tests cover the wave-table ties-even rounding, the sin table against
  libm over the full range, turb axes, requires gating rules, sort mapping and
  sky geometry (zenith UV = pi/2, bottom face omitted, box spans).
- Live fly-throughs, VERIFIED by me on the Deluxe install: mp_pavlov (night
  sky), mp_harbor, mp_bocage (overcast hedgerow sky, fence and foliage
  cutouts, terrain detail blends), noville (dusk cloud dome, scrolling cloud
  stage visible), burnville. Zero missing textures on stock main maps; 94-145 fps
  during the passes.
- PENDING HUMAN REVIEW: pixel-truth against the retail renderer. Everything
  above proves the surfaces render as authored; only side-by-side comparison
  with the real game can prove the last percent of blending and sky shading.
- Known divergences, all logged in §8: NV/ATI hw-path stages dropped (retail
  fallback parity), `deformVertexes` forms other than `wave` inert, script fog
  keywords inert (no stock MP map sets fog from a script; the fog every map
  shows arrives in configstring 12 and is rendered), `$dlight` bundles and the
  mp_ship deckflag image have substitutes rather than files, noville's missing textures under `--mod-dir uo` are the
  documented single-mod-dir mounting limitation (its textures ship in main/
  pak0/pak1), turb axis follows tr_shade_calc.c.
