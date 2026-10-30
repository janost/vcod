# CoD 1.1 gamma, overbright and intensity

How retail applies the Brightness slider (`r_gamma`), `r_overBrightBits`,
`r_ignorehwgamma` and `r_intensity`, and how vcod reproduces it. Module:
`CoDMP.exe` 1.1 (image base `0x00400000`, md5 in
`cod11-events-and-fx.md`); the renderer is linked into the exe. The code
is Q3's `R_SetColorMappings` / `GLimp_SetGamma` / `R_LightScaleTexture`
with CoD's own clamps.

## 1. Cvars

Registered in the renderer's cvar block (`0x4b3a09` stores `r_gamma`).
VERIFIED (registration calls and their default strings):

| Cvar | Default | Flags | Pointer |
|---|---|---|---|
| `r_gamma` | "1.0" (`0x557ef4`) | 1 (archive) | `0x16c3718` |
| `r_overBrightBits` | "1" (`0x5685e4`) | 0x21 (archive, latch) | `0x16c3884` |
| `r_ignorehwgamma` | "0" (`0x56871c`) | 0x21 (archive, latch) | `0x16c3a14` |
| `r_intensity` | "1" (`0x5685e4`) | 0x20 (latch) | `0x16c36dc` |

The stock options touch only `r_gamma`. VERIFIED (the file):
`ui/options_graphics.menu` (`localized_english_pak0.pk3`) has two
Brightness sliders on `r_gamma`, range 0.5 to 3. One sits in the Options
group, shown while `r_ignorehwgamma` is 0. The other sits in the System
group, shown while it is 1; its comment says that case needs a
`vid_restart`.

## 2. The ramp rebuild (`0x4f0780`)

`0x4f0780` (Q3's `R_SetColorMappings`) runs at renderer init (call at
`0x4b4766`, and from `0x4f098f`) and from the per-frame begin
(`0x4de38b`). There, when `r_gamma`'s modified flag (`+0x14`) is set, it
clears it and calls `0x4ddbf0`, then `0x4f0780`. A slider move takes
effect on the next frame, no restart. VERIFIED (instructions at
`0x4de38b`-`0x4de39d`).

The steps in order. VERIFIED (disassembly `0x4f0780`-`0x4f0970`,
constants read from `.rdata`) unless labelled:

1. `overbrightBits` (`0x16c55b8`) = `r_overBrightBits`, then 0 when
   device gamma is unsupported (`0x16c3a8c` is 0) or the window is not
   full screen (`0x16c3af4` is 0). Then clamped to 0..1 at colour depth
   (`0x16c3a80`) 16 or less, else 0..2. INFERRED (from its writes):
   `0x16c3af4` is the full-screen flag. GLW_SetMode (`0x508630`) stores
   its full-screen argument there and clears it when
   `ChangeDisplaySettingsA` fails and it falls back.
2. `identityLight` (`0x16c55b0`) = `1.0 / (1 << overbrightBits)`;
   `identityLightByte` (`0x16c55b4`) = `ftol(identityLight * 255.0)`
   (constants `0x568e60` 1.0, `0x568ec0` 255.0).
3. `r_intensity` <= 1.0 is set to "1".
4. `r_gamma` < 0.5 (`0x568e70`) is set to "0.5" (`0x55797c`); > 3.0
   (`0x5690c8`) to "3.0" (`0x54b554`).
5. The gamma table at `0x11de160`, for i in 0..256:
   `v = (g == 1.0) ? i : ftol(255.0 * pow(i * (1/255), 1/g) + 0.5)`,
   then `v << overbrightBits`, clamped to 0..255. `1/255` is the float at
   `0x568ec4` (0.0039215689); 255.0 and 0.5 are doubles at `0x5690c0` and
   `0x568f38`. INFERRED (call shape, two x87 operands in, one out; MSVC
   `_CIpow` and `_ftol`): `0x538ca0` is `pow` and `0x538be0` a truncating
   `ftol`.
6. The intensity table at `0x11de268`: `min(255, ftol(i * r_intensity))`.
7. With device gamma, `0x515d20` (GLimp_SetGamma) loads the table as all
   three channels of the hardware ramp (each byte widened to
   `b << 8 | b`) through `SetDeviceGammaRamp`, with a "performing W2K
   gamma clamp." path on Windows 2000. VERIFIED (strings and the call at
   `0x4f0965`).

VERIFIED (strings and calls): `0x515c30` probes device gamma
(`0x16c3a8c`) at GL init, only while `r_ignorehwgamma` is 0, through
`GetDeviceGammaRamp` on the desktop DC. It drops it again ("WARNING:
device has broken gamma support") when the saved ramp is not increasing.
Shutdown restores the saved ramp.

## 3. Where the tables reach pixels

- Device gamma on (the default): the ramp scales the whole display, menus
  and HUD included. Textures load through `r_intensity` only. VERIFIED:
  `0x4eaa90` (Q3's `R_LightScaleTexture`) maps each RGBA texel's rgb
  through the intensity table with device gamma on, through
  gamma-of-intensity with it off, and an `only_gamma` image through the
  gamma table only with it off.
- Captured frames. VERIFIED: the TGA screenshot writer `0x4b0f90` (18-byte
  header, type 2, 24 bpp), `0x4b10a0` (reads RGBA, then `0x4ea4b0`, a
  byte-wise pass through the gamma table) and the downsampling captures
  `0x4b11c0`, `0x4b1520`, `0x4b17f0` run the read-back pixels through the
  gamma table when `overbrightBits > 0` and device gamma is on. INFERRED:
  that makes a capture match the screen.
- Lighting. VERIFIED (the multiplies): vertex and grid light terms
  multiply by `1 << overbrightBits` (`0x4b5e30`, `0x4b69f0`), and
  `identityLight` scales the light contributions near `0x4b5ed0`. Their
  full effect on the frame is not traced.
- Which uploads the texture gamma reaches. VERIFIED: `0x4eaa90` starts
  with `cmp [esp+0xc], 0x1908` (`0x4eaa91`) and returns unless its format
  argument is `GL_RGBA`. VERIFIED: the image loader passes the image's own
  format to `0x4ec730` (`0x4eecd1`, `0x4eed00`), either `GL_RGBA` or one
  of `0x83f0`..`0x83f3` (checks at `0x4eec7d`-`0x4eec94`, next to
  "heightToNormal not valid for DDS textures" at `0x54b77c`). INFERRED
  (the S3TC names are GL's enum values): DDS images upload compressed and
  the texture gamma reaches TGA and JPG images only.
- VERIFIED: the upload routine `0x4ebf20` calls `0x4eaa90` at `0x4ec124`
  only on its resample or mipmap path. When the picmipped size equals the
  image's size (`0x4ec004`, `0x4ec00e`) and flag bit 0 is clear
  (`0x4ec018`-`0x4ec023`), it uploads straight through `0x4ebdc0` at
  `0x4ec03c` and skips the gamma. INFERRED: flag bit 0 is "mipmap", from
  the "image '%s' requested mipmaps, but they aren't available" string
  (`0x54b7b4`) pushed when `bl & 1` is set at `0x4eec5c`. So 2D images at
  native size keep their bytes.
- Lightmaps. VERIFIED: the lightmap loader `0x4d9fa0` (it names each page
  `*lightmap%d`) runs every texel of each 512x512 page through `0x4d9f30`,
  which calls `0x4d9af0` per texel and sets alpha 255. `0x4d9af0` shifts r,
  g and b left by `1 - overbrightBits`; when any passes 255 it scales all
  three by `255 / max` (integer divides), keeping the hue. INFERRED: this
  is Q3's `R_ColorShiftLightingBytes` with `r_mapOverBrightBits` fixed at
  1, so full screen loads lightmaps as they are and windowed doubles them.
  VERIFIED: with no lightmap data the pages are filled with
  `identityLightByte` (`0x16c55b4`). VERIFIED: each page is created by
  `0x4ec310(0xde1, w, h, 0x38, 3)` (pushes at `0x4da19d`-`0x4da1a3`) and
  uploaded by `0x4ec490(data, 0xde1, 0x1908)` (`0x4da1b3`-`0x4da1c4`).
  INFERRED: flags `0x38` have bit 0 clear, so a page at native size takes
  the fast path and the texture gamma never reaches lightmaps.

INFERRED (steps 1, 5 and 7 and `0x4eaa90`): with defaults in full screen
(`r_overBrightBits 1`) the framebuffer holds half-bright lighting and the
ramp doubles it after gamma:
`display = min(255, round(255 * (fb/255)^(1/g)) << 1)`. Windowed, or with
`r_ignorehwgamma 1`, `overbrightBits` is 0 and nothing doubles. With
`r_ignorehwgamma 1` the gamma is baked into TGA and JPG textures at load
and picked up again whenever images reload (every map load and
`vid_restart`), which is why the menu moves the slider to the System
group. That a map load reloads every image is Q3's `RE_Shutdown(qfalse)`
on map change, not traced in CoDMP.exe.

INFERRED (from the above): windowed with device gamma, the ramp still
loads (step 7 tests only device gamma) but unshifted, so `r_gamma` stays
live and the framebuffer byte is the display byte. A lightmapped texel
then shows `texture * shifted lightmap`, where full screen shows
`min(255, 2 * texture * lightmap)`. The two agree until a lightmap channel
passes 127. Past that, windowed keeps the lightmap's hue and full screen
clamps per channel after the multiply. `identityLighting`, `rgbGen vertex`
and the other `identityLight`-scaled terms are halved in the full-screen
framebuffer and doubled back by the ramp, so they look the same in both.
Stages with no `identityLight` term (`identity`, `exactVertex`, `const`)
show twice as bright full screen as windowed, up to the clamp.

## 4. What vcod does

vcod's frame holds retail's framebuffer bytes. Every pass draws into an
`Rgba8Unorm` scene target (`renderer::SCENE_FORMAT`). Textures upload as
`Rgba8Unorm` and `Bc1`..`Bc3RgbaUnorm` with no sRGB decode, and vertex
colours are raw bytes, so products, blends, fog and the clamp at 1.0 work
in byte space as in retail's GL 1.x framebuffer. `gamma.wgsl` maps every
frame onto the swapchain through retail's 256-entry table
(`gamma::ramp(r_gamma, overbrightBits)`), indexed by the framebuffer byte,
and linearises the result where the swapchain is sRGB. The display
doubling happens once, at the end, as retail's hardware ramp does it:

- `overbrightBits` 1 (full screen with device gamma,
  `gamma::overbright_bits`): the shifted ramp. Below gamma 1, framebuffer
  bytes 128..255 show as distinct shades, as in retail.
- `overbrightBits` 0 (windowed, or `r_ignorehwgamma 1`): the unshifted
  ramp; the framebuffer byte is the display byte.

That last case at `r_gamma` 1 makes the table the identity and the pass a
copy. A frame that runs the post pass then draws it and the 2D straight
into the swapchain and skips the gamma pass, when the swapchain is
`Rgba8Unorm` (the window asks for it where the surface lists it) and
opaque, and nothing that frame samples or copies the gamma scene (a
shellshock's capture; without the post pass, glow, blur, bloom and shafts
work in it). The bytes are the same either way (cod4-client-perf.md 30).

`identityLight` (`gamma::identity_light`, 0.5 or 1) follows the same bits
every frame and rides the camera uniform (`time_pad.y`), so the lighting
switches with the window as retail's does at its `vid_restart`:

- Lightmaps: raw full screen; windowed, doubled and scaled back by the
  brightest channel (`lightmap_shift` in `shader.wgsl`, section 3's
  `0x4d9af0`). vcod shifts the filtered sample in the shader; retail
  shifts texels at load.
- Stage gens (section 5): `vertex` scales the vertex colour by
  `identityLight`; `identityLighting`, `constLighting` and `wave` scale
  the tint (`STAGE_FLAG_VERTEX_RGB_HALF`, `STAGE_FLAG_TINT_LIGHT`).
  `identity`, `exactVertex` and `const` draw raw, so windowed shows them
  at half the full-screen brightness, as in retail.
- Props: baked colours are computed at `identityLight` 0.5
  (`static_light.rs`); the vertex shader rescales them by
  `2 * identityLight`, clamped at 1. Entity and viewmodel light sets are
  rescaled the same way before GL's clamp (`entity_light::pack`).
- Fog colour: `identityLight * rgb` (`0x4d2ad0`, section 5); the clear
  colour stays raw.
- HUD and fx quads: the vertex colour is scaled by the material's first
  stage gen (`renderer::colour_scale`). A material with no script is
  implicit 2D or fx and takes `rgbGen vertex`, so `identityLight`.
- Sky farbox and sun: `identityLight` (section 5).

`r_gamma` is read every frame and clamped to 0.5..3 with step 4's
write-back, so the slider is live as in retail. The stage tint
(`rgbGen`/`alphaGen` wave and const) is clamped to 0..1, the colour bytes
retail writes.

`r_ignorehwgamma` is read at start-up and on `vid_restart` (latched).
While it is 1 the frame pass runs the gamma 1 table. Each world load reads
`r_gamma` and maps every uncompressed (TGA, JPG) image's rgb through the
unshifted ramp before upload (`gamma::bake_image`, from
`texture_settings::prepare`) when the image is mipmapped or picmip shrank
it (section 3's rule): material images, shader-stage images, prop and
model skins, fx sprites, HUD and menu images. DDS images, the dlight blob,
lightmaps and unmipped images at native size keep their bytes, as in
retail. HUD images reload when the table changes.

Divergences:

- `r_aaSamples` above 1 resolves MSAA to the 8-bit target. Retail ran
  without it; the default is retail's single sample
  (`cod11-client-perf.md` 4).
- vcod's implicit vertex-lit world surfaces multiply the raw vertex colour
  in both modes. INFERRED: retail does the same, since `0x4d9af0` is called
  only for lightmap pages (section 3); the BSP vertex-colour load is not
  traced.
- The HUD scale comes from the material's first stage; a scripted material
  whose later stages use other gens draws with the first one's. The fx
  pass takes each stage's own gens (`cod11-shader-scripts.md` section 3).
  Debug text (F3) draws raw.
- vcod's `vid_restart` rebuilds the renderer (and reloads the world) only
  when the scene format, the MSAA or an image setting moved, so a baked
  gamma change otherwise waits for the next map load. Retail's
  `vid_restart` reloads every image.
- `r_overBrightBits` is not registered; vcod uses its default 1.
- `r_intensity` is not applied (no stock menu sets it).

Bloom (`vc_bloom`, `crate::bloom`) is vcod-only. While `vc_bloom` is above
0 the map pass resolves the world into the scene target, a bright-pass
reads it, a dual-filter chain blurs it from half resolution down to an
8-pixel side, and a full-screen draw adds it into the resolved scene. The
HUD and the F3 text then draw into the resolved scene single-sampled, so
they neither glow nor take the glow. The threshold (`vc_bloomThreshold`)
is on the display value at gamma 1, `framebuffer / identityLight` capped
at 1, and the composite adds back in framebuffer units
(`x identityLight`). A pixel therefore blooms by the same display amount
windowed and full screen (full screen, the ramp doubles a framebuffer
value half the windowed one). `r_gamma` curves the added glow with the
rest of the frame. Both cvars are read every frame. With `vc_hdr 1` the
chain reads the float world target (section 9).

## 5. Stage colours and the display doubling

What retail writes to the framebuffer per `rgbGen`, before the ramp
doubles it:

- Lightmaps: the shift at load in section 3 (`0x4d9af0`, called only from
  `0x4d9f30`, a 512 x 512 loop over a page). INFERRED (from that
  section): with one overbright bit the shift is 0, so lightmaps reach the
  framebuffer raw.
- VERIFIED: the colour switch at `0x4ffa60` (Q3's `RB_CalcColors`, keyed
  on the stage's gen at `+0x664`; gen numbers in
  `cod11-light-grid-and-leaf-lights.md` section 9):

  | Gen | Writes |
  |---|---|
  | identity (2) | 0xffffffff |
  | exactVertex (5) | the vertex colour |
  | vertex (6) | each channel right-shifted by `overbrightBits` |
  | oneMinusVertex (7) | the inverse, through `__ftol2` when `identityLight` is not 1 |
  | const, constLighting (0xc) | the stage constant |
  | lightingAmbient, lightingDiffuse (9, 10) | `0x515ad0` on a map with no lump-19 lights, white otherwise |
  | identityLighting (1) and every unlisted gen | `identityLightByte` |

- VERIFIED: in the GL state setter `0x4d57b0`, state bit 0x100000 enables
  `GL_LIGHTING` (0xb50) for a map with lump-19 lights while the cvar at
  `0x16c39e4` is 0. Otherwise it disables it and, when the new state
  carries the bit, sets `glColor3f(identityLight, identityLight,
  identityLight)`. INFERRED: lit models draw through fixed-function
  lighting with the picked lights, which is why gens 9 and 10 write white.

INFERRED (the switch and section 3): a framebuffer colour is either a
texture times a raw lightmap, or a texture times a gen `identityLight`
already halved, and the display shows each doubled.

- VERIFIED: the 2D shader registrations (refexport `0x4fca80`, mipmapped,
  and `0x4fcae0`, no mipmaps) call `0x4fc5c0` with lightmap index -4 (the
  push at `0x4fcabc`); the console and font atlases (`0x4de850`,
  `"font/%s_%d_1024_%d.tga"`) register with -4 too. The implicit-shader
  builder `0x4fc440`, case -4, gives stage 0 `rgbGen vertex` (6),
  `alphaGen vertex` and state 0x10065. Its other cases: -1 (model skin)
  gen 10, state 0x100100; -3 gen 5 (exactVertex); -2 gen 1 on the white
  image; a lightmap index gen 2.
- VERIFIED: `RE_SetColor` (`0x4ddcf0`) stores `colour * 255` as bytes with
  no `identityLight` (`0x568ec0` is 255.0f); `0x4d7390` copies them to
  `backEnd.color2D` (`0x16d8e74`), which the stretch-pic path (push at
  `0x4d75ad`) writes into the vertex colours (`0x4d73a0`,
  `0x4d74ac`-`0x4d74c1`). So an implicit 2D quad lands at
  `colour >> overbrightBits` and shows at its nominal colour. INFERRED:
  text draws through the same stretch-pic path, as in Q3TA.
- VERIFIED, `hud.shader` (`pak0.pk3`): 43 of 44 stages say `rgbGen
  vertex`; `ui/assets/hudbar*` say `identity`, so they show doubled;
  `black` is `const`, `console` is `constLighting`. The `fonts/` entries
  sit inside a `/* */` comment, so fonts are implicit.
- VERIFIED: `constLighting` multiplies its constant by `identityLight` at
  parse (`0x4f8e96`-`0x4f8eb2`, the keyword at `0x547c34`); plain `const`
  does not.
- VERIFIED: a stage with no `rgbGen` defaults to `identityLighting` (1)
  when its source blend is 0, 2 or 5 and to `identity` (2) otherwise
  (`0x4f9a0a`-`0x4f9a32`). INFERRED: that is Q3's `ParseStage` rule (no
  blend, `GL_ONE`, `GL_SRC_ALPHA`).
- VERIFIED: the farbox drawer `0x4e68e0` binds the six skyParms images
  (`0x11e67f4`) and sets `glColor3f(identityLight x3)`
  (`0x4e6ad3`-`0x4e6ade`) before its quads. INFERRED: with the modulate
  texture env the sky lands halved and shows at its texture colour.
  VERIFIED, `sun.shader`: the sun discs are `rgbGen identityLighting`, the
  flares `rgbGen vertex`.
- VERIFIED, `fxshaders/*.shader` in `pak5.pk3`: 185 stages, each with an
  explicit `rgbGen`: 166 `vertex`, 19 `exactVertex`. The 19 are all
  `blendFunc GL_ONE GL_ONE` (`gfx/effects/explosion/*`,
  `gfx/effects/fire/*`, `gfx/effects/misc/signal_flash`) and show doubled.
  Where the efx renderer's vertex colours come from is not traced.
- VERIFIED: `0x4d2ad0`, called by the fog setter `0x4d28e0`, sets
  `GL_FOG_COLOR` to `identityLight * rgb` with alpha raw, and to black when
  `(glState & 0xf0) == 0x20`. INFERRED: that is destination blend
  `GL_ONE`, so additive stages fog to black. VERIFIED: `glClearColor`
  takes the raw fog colour (`0x16c4c50`), and `identityLight * 0.5` with no
  fog and fast sky (`0x568e70` is 0.5f).

vcod applies this in its final pass and per gen (section 4). Trap: doubling
each draw in linear space instead draws lightmapped surfaces at about
`2^(1/2.2)`, 1.37 times, rather than twice the framebuffer. An earlier vcod
did that.

## 6. `vc_ssao`, a vcod-only multiply in framebuffer bytes

Retail has no ambient occlusion. `vc_ssao` (default 0, archived, read
every frame; `crates/client/src/ssao.rs`) works on section 4's bytes,
before the gamma pass:

- The frame's merged draw list splits into two render passes after its
  last draw sorting up to `decal` (6, shader doc section 5), and before the
  dlights when they fall there. The first pass stores its MSAA colour and
  depth.
- Half-resolution GTAO reads sample 0 of the MSAA depth: a resolve would
  average a silhouette's two surfaces into a depth neither has. A
  separable blur skips taps off the centre's depth.
- The multiply runs per pixel into the MSAA colour (blend `dst * src`) at
  `visibility ^ vc_ssao`, faded by the fog factor at that depth. A
  fullscreen triangle at depth 0.2 with `Less` keeps the viewmodel's
  samples (its 0..0.2 depth range) out; the shader skips the sky (cleared
  depth 1).
- So it scales `texture * lightmap`, vertex-lit and grid-lit surfaces and
  the decal layers blended onto them. Blends, additives, effects,
  viewmodel and HUD draw afterwards, untouched. Additive glow stages of an
  opaque shader draw at sort 5 and darken with their base.

## 7. `vc_bicubic`: vcod's lightmap filter (not retail)

Retail samples lightmap pages with the page's own GL filter. INFERRED:
`GL_LINEAR`, as Q3's `R_CreateImage` sets for an image without mipmaps
(the flags at `0x4da19d`, section 3, have bit 0 clear); the filter set-up
is not traced in CoDMP.exe. vcod draws them the same way by default.

`vc_bicubic 1` (vcod only, archived, read every frame) replaces that
sample in `shader.wgsl`'s `lightmap()` with a uniform cubic B-spline over
the 4x4 texels around it, then applies section 3's shift. Facts about the
stock pages it rests on, VERIFIED by reading the BSPs
(`lightmap_blocks.rs` and its tests):

- A page packs charts flush, with no gutter texels, so a 4x4 footprint at
  a chart's edge reads its neighbours. vcod keeps, per texel, how far its
  chart runs along its row and column (`Rgba8Uint`, material binding 4)
  and clamps every tap to that run, the chart's own ClampToEdge.
  mp_carentan's 5 pages hold 5898 charts, mp_ship's 8 hold 10066.
- A chart's lightmap coordinates sit on texel centres or texel borders,
  both common: of mp_carentan's lightmapped vertex coordinates, 41% are on
  a centre and most of the rest on a border or a quarter. On a border, two
  triangles of one chart cover texel ranges that meet without overlapping
  (mp_carentan's papered wall on page 3 splits at texel x 25), like two
  unrelated charts. So triangles that share a vertex (same position, same
  lightmap coordinate) are one chart, as are any whose texel ranges
  overlap. Split there, the wall drew a seam.
- Terrain charts are not rectangles (the largest, mp_carentan 102960
  texels and mp_ship 138869, wrap round smaller charts), so the runs follow
  the chart's outline, not its bounds. A tap off both the centre's row and
  column can still leave an L-shaped chart at a concave corner.

Why a B-spline, not Catmull-Rom: Catmull-Rom passes through its texels
but rings. Over a 40x30 texel stretch of mp_carentan page 3 (a diagonal
shadow, 12x magnified) it read 7.85% of its samples below the stretch's
darkest texel and peaked at 117.5 over a 99.8 texel maximum: dark and
bright halos on every shadow edge. The B-spline stays inside the texel
range and smooths bilinear's staircase, at the cost of softening the edge.
A B-spline does not pass through its texels, so across a chart's outer
texel the weights fade back to bilinear's (`smoothstep` on the distance to
the chart's edge), and a seam between two charts reads the edge texels as
retail does. The 16 taps run as four bilinear samples placed by the
weights. Clamping two neighbouring taps keeps them neighbours or folds
them onto one texel, so the trick holds at the edges.

Cost, VERIFIED by vcod measurement (release, 1600x900, 4x MSAA, an
integrated GPU, machine loaded by other builds, so medians of alternating
120-frame windows): mp_carentan indoors +0.5..0.9 ms on a 1.7..2.4 ms
frame, its street +0.6 ms on 3.9 ms, mp_ship +0.3 ms on 4.5 ms. With
`vc_bicubic 0` the frame is within noise of a build without the code
(+0.04 ms on 1.65 ms). The run texture adds 1 MB per page.

## 8. `vc_softParticles`: effect sprites fade into the scene (not retail)

vcod draws effect sprites with a plain depth test, so a smoke card that
crosses the ground or a wall shows a hard line there. INFERRED: retail
does the same; its renderer is fixed-function GL 1 (section 3), which
cannot read depth in a fragment. `vc_softParticles` (vcod only, default 0,
archived, read every frame; `crates/client/src/soft_particles.rs`) fades
them by the gap to the opaque scene behind:

- It uses section 6's split (the draw list cut after its last draw sorting
  up to `decal`), shared with `vc_ssao` and `vc_shadows`. It asks for the
  split only when an fx run sorts past `decal`, so a frame with no effects
  pays nothing.
- At the split a fullscreen pass copies sample 0 of the MSAA depth into a
  single-sample `R32Float` target as linear view depth. The fx pipelines
  cannot bind the MSAA depth itself: the second half keeps it as a
  writable attachment for blended draws that still write depth. Sample 0,
  not an average, for section 6's reason.
- Fx runs after the split use a second pipeline pair (straight alpha and
  `GL_ONE GL_ONE`) with that target in group 2. A fragment scales alpha
  (additive: colour) by `clamp((scene_z - z) / d, 0, 1)` with
  `d = vc_softParticles * clamp(size / 2, 1, 32)` world units, `size` the
  quad's smaller half-extent. Sparks born at a wall stay visible; an
  explosion card at the ground keeps most of its body. Runs sorting at or
  below `decal` draw before the copy exists and stay hard.
- Not covered: impact marks (they lie on the surface by design), world
  shader stages with blended or additive sprites, and anything drawn after
  the split that writes depth (the copy holds only the opaque and decal
  layers).

Cost, VERIFIED by vcod measurement (release, 1280x765 window in a nested
kwin, 4x MSAA, an integrated GPU, machine loaded by other builds): the
copy pass alone reads 0.26..0.37 ms on GPU timestamps. Whole-scene GPU
time on a view filled with `fx/smoke/cratersmoke.efx` (about 70 large
cards) ran 4.7..13.0 ms off (median 8.1 over five runs) and 9.8..11.7 ms on
(median 10.9). The noise is wider than the gap: treat +1..3 ms as an upper
bound until re-measured on an idle machine.


## 9. `vc_hdr`, `vc_grade`, `vc_vignette`, `vc_grain` (vcod only)

Retail has none of these. Each defaults to 0 (`crate::post`). With all
four at 0 the post pass never runs and the frame is section 4's: the map
pass resolves straight into the gamma pass's 8-bit scene. vcod
measurement: a `mp_carentan` fly-spawn frame with and without this code,
at defaults, compared equal on every pixel.

Order while any is on: the map pass resolves into the post pass's world
target, `vc_sunShafts` adds into it, `vc_bloom` blurs and composites into
it, the post pass writes framebuffer bytes into the gamma scene, the HUD
and F3 text draw over them single-sampled, then the gamma pass runs the
ramp. So the HUD is never tonemapped, graded, vignetted or grained, and
`r_gamma` and the overbright doubling still apply to everything once, at
the end.

`vc_hdr` (archived, latched; a `vid_restart` that changes it rebuilds the
renderer and reloads the map into it) makes the world's MSAA target and
the post world target `Rgba16Float` (`renderer::HDR_FORMAT`). Section 4's
model holds: the target holds framebuffer units, not linear light,
textures upload without sRGB decode, the x2 stays in the ramp and nothing
doubles per draw. The difference is that a blend no longer clamps at 1.0:
additive stages, the sun disc and flare, fx and `GL_ONE` stacks keep their
sum. Trap: a float target that draws linear light and doubles each draw
lands about 1.37x where retail shows 2x; an earlier `vc_hdr` did that and
was reverted.

The post pass reads the display value at gamma 1,
`d = framebuffer / identityLight`, and writes `t * identityLight` back for
the ramp:

- Tonemap, per channel: `t = d` up to a knee of 0.9. Above it, a
  Reinhard-extended shoulder `x (1 + x / w^2) / (1 + x)`, `x` the distance
  past the knee in units of the headroom (0.1), `w` the white point 2.0 in
  those units, C1 at the knee, so display 2.0 lands on white. Below the
  knee the ramp sees retail's bytes, so ordinary surfaces keep retail's
  look at any `r_gamma`. Not ACES: it tone-curves the whole range
  (mid-tone contrast and saturation change) and would move every surface
  off retail's look; plain Reinhard fails the same way. Per channel, like
  retail's clip, so a bright colour drifts toward white as clipped colours
  do.
- The shoulder steals from display values 0.9 to 1.0. vcod measurement,
  `mp_carentan` sun view windowed: the sun's core read (242, 255, 255) at
  `vc_hdr 0` and (238, 244, 249) at 1. A knee of 0.8 and white of 4.0 read
  (226, 231, 238), a grey sun, since most of what stacks over white lands
  between 1 and 2. Hence 0.9 / 2.0.
- One opaque draw reaches display 1.0 windowed and 2.0 full screen
  (lightmaps raw under the shifted ramp, section 3), so full screen the
  overbright part of sunlit surfaces rolls off where retail clipped it.
- Negative values (a `GL_ONE_MINUS_DST_COLOR` blend over a value past 1)
  clamp to 0 when read.
- Every pipeline that draws into the world target takes its format
  (`Renderer::world_format`): world stages and marks (`scene_pipeline`),
  sky, sun and flare, dlights, entities, fx (with `vc_softParticles`'
  depth read, whose own copy target stays `R32Float`), the SSAO multiply,
  the `vc_shadows` mask and the bloom and shaft composites.

`vc_bloom` with `vc_hdr`: the bright-pass reads the float world target
uncapped (to 16 framebuffer units), and its knee sits at
`vc_bloomThreshold + 0.2` on the display value (white at the default
0.8), with a quadratic soft knee that keeps the hue. So only what retail
would clip glows. vcod measurement, `mp_carentan` fly spawn (sunlit grass,
wall, sky) windowed: `vc_bloom 1` changed 14% of pixels by more than 8
levels at `vc_hdr 0` and 0.1% at `vc_hdr 1`; full screen, 21% and none.
Looking into the sun, the glow stays on the sun's halo in both modes.

- `vc_grade` (strength, live, 0..2), on `t`: a third of the saturation
  gone toward Rec. 601 luma, highlights warmed by
  `(0.05, 0.02, -0.04) l^2`, shadows cooled by
  `(-0.03, 0, 0.04) 4 l (1 - l)^2`, so black and white stay put; strength
  mixes it in.
- `vc_vignette` (live, 0..2): darkens toward the corners by up to
  40% x strength on the squared distance from the centre, the frame's
  aspect kept.
- `vc_grain` (live, 0..2): monochrome noise of up to 6% x strength,
  strongest in the mid-tones, reseeded 24 times a second.

Cost, vcod measurement (release, nested compositor at about 1280 x 765,
4x MSAA, an integrated GPU, load average 11 to 33 from sibling builds,
vsync off, mean frame time of 300-frame windows): `vc_hdr 0` held 6.8 ms
at its steadiest, `vc_hdr 1` 7.6 ms in the same run, about +0.8 ms from
the doubled MSAA colour bandwidth and the post pass. The grade alone stayed
inside the noise. Memory: the float MSAA target doubles to 8 bytes a
sample, and the post world target adds 4 (8-bit) or 8 (float) bytes a
pixel while allocated (always).
