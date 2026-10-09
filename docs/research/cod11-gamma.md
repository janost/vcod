# CoD 1.1 gamma, overbright and intensity

How retail applies the options screen's Brightness slider (`r_gamma`) and
the related `r_overBrightBits`, `r_ignorehwgamma` and `r_intensity`, and
how vcod reproduces it. Module: `CoDMP.exe` 1.1 (image base `0x00400000`,
md5 in `cod11-events-and-fx.md`); the renderer is linked into the exe. The
code is Q3's `R_SetColorMappings` / `GLimp_SetGamma` /
`R_LightScaleTexture` with CoD's own clamps.

## 1. Cvars

Registered in the renderer's cvar block (`0x4b3a09` stores `r_gamma`).
VERIFIED (registration calls and their default strings):

| Cvar | Default | Flags | Pointer |
|---|---|---|---|
| `r_gamma` | "1.0" (`0x557ef4`) | 1 (archive) | `0x16c3718` |
| `r_overBrightBits` | "1" (`0x5685e4`) | 0x21 (archive, latch) | `0x16c3884` |
| `r_ignorehwgamma` | "0" (`0x56871c`) | 0x21 (archive, latch) | `0x16c3a14` |
| `r_intensity` | "1" (`0x5685e4`) | 0x20 (latch) | `0x16c36dc` |

`r_gamma` is the only one the stock options touch.
`ui/options_graphics.menu` (`localized_english_pak0.pk3`) carries two
Brightness sliders on `r_gamma`, range 0.5 to 3: one in the Options group
shown while `r_ignorehwgamma` is 0, one in the System group shown while it
is 1, whose comment says that case needs a `vid_restart`. VERIFIED (the
file).

## 2. The ramp rebuild (`0x4f0780`)

`0x4f0780` (Q3's `R_SetColorMappings`) runs at renderer init (call at
`0x4b4766`, and from `0x4f098f`) and from the per-frame begin
(`0x4de38b`): when `r_gamma`'s modified flag (`+0x14`) is set it clears
it, calls `0x4ddbf0` and then `0x4f0780`. So a slider move takes effect
on the next frame, without a restart. VERIFIED (the instructions at
`0x4de38b`-`0x4de39d`).

The steps, in order. VERIFIED (disassembly `0x4f0780`-`0x4f0970`,
constants read from `.rdata`) unless labelled:

1. `overbrightBits` (`0x16c55b8`) = `r_overBrightBits`, then 0 when
   device gamma is unsupported (`0x16c3a8c` is 0) or the window is not
   full screen (`0x16c3af4` is 0). Then clamped to 0..1 when the colour
   depth (`0x16c3a80`) is 16 or less, else to 0..2.
   `0x16c3af4` is the full-screen flag: GLW_SetMode (`0x508630`) stores
   its full-screen argument there and clears it when
   `ChangeDisplaySettingsA` fails and it falls back. INFERRED (from those
   writes).
2. `identityLight` (`0x16c55b0`) = `1.0 / (1 << overbrightBits)`, and
   `identityLightByte` (`0x16c55b4`) = `ftol(identityLight * 255.0)`
   (constants `0x568e60` 1.0, `0x568ec0` 255.0).
3. `r_intensity` <= 1.0 is set to "1".
4. `r_gamma` < 0.5 (`0x568e70`) is set to "0.5" (`0x55797c`); > 3.0
   (`0x5690c8`) is set to "3.0" (`0x54b554`).
5. The gamma table at `0x11de160`, for i in 0..256:
   `v = (g == 1.0) ? i : ftol(255.0 * pow(i * (1/255), 1/g) + 0.5)`,
   then `v << overbrightBits`, clamped to 0..255. `1/255` is the float at
   `0x568ec4` (0.0039215689), 255.0 and 0.5 are doubles at `0x5690c0` and
   `0x568f38`. That `0x538ca0` is `pow` and `0x538be0` is a truncating
   `ftol` is INFERRED from the call shape (two x87 operands in, one out;
   MSVC `_CIpow` and `_ftol`).
6. The intensity table at `0x11de268`: `min(255, ftol(i * r_intensity))`.
7. With device gamma, `0x515d20` (GLimp_SetGamma) loads the table as all
   three channels of the hardware ramp (each byte widened to `b << 8 | b`)
   through `SetDeviceGammaRamp`, with a "performing W2K gamma clamp."
   path on Windows 2000. VERIFIED (strings and the call at `0x4f0965`).

Device gamma (`0x16c3a8c`) is probed at GL init by `0x515c30`: only while
`r_ignorehwgamma` is 0, through `GetDeviceGammaRamp` on the desktop DC,
and dropped again ("WARNING: device has broken gamma support") when the
saved ramp is not increasing. The shutdown path restores the saved ramp.
VERIFIED (strings and calls).

## 3. Where the tables reach pixels

- Device gamma on (the default): the ramp scales the whole display,
  menus and HUD included. Textures load through `r_intensity` only:
  `0x4eaa90` (Q3's `R_LightScaleTexture`) maps each RGBA texel's rgb
  through the intensity table when device gamma is on, through
  gamma-of-intensity when it is off, and an `only_gamma` image through
  the gamma table only when device gamma is off. VERIFIED.
- Captured frames: the TGA screenshot writer `0x4b0f90` (18-byte header,
  type 2, 24 bpp), `0x4b10a0` (reads RGBA, then `0x4ea4b0`, a byte-wise
  pass through the gamma table) and the downsampling captures `0x4b11c0`,
  `0x4b1520`, `0x4b17f0` run the read-back pixels through the gamma table
  when `overbrightBits > 0` and device gamma is on. VERIFIED. That this
  makes a capture match the screen is INFERRED.
- `overbrightBits` also scales lighting: vertex and grid light terms
  multiply by `1 << overbrightBits` (`0x4b5e30`, `0x4b69f0`) and
  `identityLight` scales the light contributions near `0x4b5ed0`.
  VERIFIED (the multiplies). Their full effect on the frame is not
  traced here.

- Which uploads the texture gamma reaches. VERIFIED: `0x4eaa90` starts
  with `cmp [esp+0xc], 0x1908` (`0x4eaa91`) and returns unless its format
  argument is `GL_RGBA`. VERIFIED: the image loader passes the image's own
  format to `0x4ec730` (`0x4eecd1`, `0x4eed00`), and that format is
  `GL_RGBA` or one of `0x83f0`..`0x83f3` (the S3TC formats; the checks at
  `0x4eec7d`-`0x4eec94`, next to "heightToNormal not valid for DDS
  textures" at `0x54b77c`). So DDS images upload compressed and the texture
  gamma never touches them; it reaches TGA and JPG images only. INFERRED
  (the S3TC names are GL's enum values).
- VERIFIED: the upload routine `0x4ebf20` calls `0x4eaa90` at `0x4ec124`
  only on its resample or mipmap path. When the picmipped size equals the
  image's size (`0x4ec004`, `0x4ec00e`) and flag bit 0 is clear
  (`0x4ec018`-`0x4ec023`), it uploads straight through `0x4ebdc0` at
  `0x4ec03c` and skips the gamma. INFERRED: flag bit 0 is "mipmap", from
  the "image '%s' requested mipmaps, but they aren't available" string
  (`0x54b7b4`) pushed when `bl & 1` is set at `0x4eec5c`. So 2D images at
  their native size keep their bytes.
- Lightmaps: VERIFIED, the lightmap loader `0x4d9fa0` (it names each page
  `*lightmap%d`) runs every texel of each 512x512 page through `0x4d9f30`,
  which calls `0x4d9af0` per texel and sets alpha 255. `0x4d9af0` shifts
  each of r, g and b left by `1 - overbrightBits` and, when any of them
  passes 255, scales all three by `255 / max` (integer divides), keeping the
  hue. INFERRED: it is Q3's `R_ColorShiftLightingBytes` with
  `r_mapOverBrightBits` fixed at 1, so full screen loads lightmaps as they
  are and windowed doubles them at load. VERIFIED: with no lightmap data the
  pages are filled with `identityLightByte` (`0x16c55b4`). VERIFIED: each
  page is created by `0x4ec310(0xde1, w, h, 0x38, 3)` (pushes at
  `0x4da19d`-`0x4da1a3`) and uploaded by `0x4ec490(data, 0xde1, 0x1908)`
  (`0x4da1b3`-`0x4da1c4`). INFERRED: flags `0x38` have bit 0 clear, so a
  page at its native size takes the fast path above and the texture gamma
  never reaches lightmaps.

So with defaults in full screen (`r_overBrightBits 1`), the framebuffer
holds half-bright lighting and the ramp doubles it after gamma:
`display = min(255, round(255 * (fb/255)^(1/g)) << 1)`. Windowed, or with
`r_ignorehwgamma 1`, `overbrightBits` is 0: no doubling, and with
`r_ignorehwgamma 1` the gamma is baked into TGA and JPG textures at load
(picked up again whenever images reload, which is every map load and
`vid_restart`; the menu moves the slider to the System group for that
reason). INFERRED (from steps 1, 5 and 7 and `0x4eaa90`; that a map load
reloads every image is Q3's `RE_Shutdown(qfalse)` on map change and not
traced in CoDMP.exe).

INFERRED, from the above: windowed with device gamma, the ramp still loads
(step 7 tests only device gamma) but unshifted, so `r_gamma` stays live and
the framebuffer byte is the display byte. A lightmapped texel then shows
`texture * shifted lightmap`, where full screen shows
`min(255, 2 * texture * lightmap)`: the two agree until a lightmap channel
passes 127, past which windowed keeps the lightmap's hue and full screen
clamps per channel after the multiply. `identityLighting`,
`rgbGen vertex` and the other `identityLight`-scaled terms are halved in
the full-screen framebuffer and doubled back by the ramp, so they show the
same in both; stages with no `identityLight` term (`identity`,
`exactVertex`, `const`) show twice as bright full screen as windowed, up to
the clamp.

## 4. What vcod does

vcod's frame holds retail's framebuffer bytes. Every pass draws into an
`Rgba8Unorm` scene target (`renderer::SCENE_FORMAT`); textures upload as
`Rgba8Unorm` and `Bc1`..`Bc3RgbaUnorm`, with no sRGB decode, and vertex
colours are raw bytes, so products, blends, fog and the clamp at 1.0 all
happen in byte space as in retail's GL 1.x framebuffer. `gamma.wgsl` then
maps every frame onto the swapchain through retail's 256-entry table
(`gamma::ramp(r_gamma, overbrightBits)`), indexed by the framebuffer byte,
and linearises the result for the sRGB swapchain. So the display doubling
happens once, at the end, as retail's hardware ramp does it:

- `overbrightBits` 1 (full screen with device gamma,
  `gamma::overbright_bits`): the shifted ramp. Below gamma 1 framebuffer
  bytes 128..255 show as distinct shades, as in retail.
- `overbrightBits` 0 (windowed, or `r_ignorehwgamma 1`): the unshifted
  ramp; the framebuffer byte is the display byte.

`identityLight` (`gamma::identity_light`, 0.5 or 1) follows the same bits
every frame and rides the camera uniform (`time_pad.y`), so the lighting
switches with the window as retail's does at its `vid_restart`:

- Lightmaps: raw full screen; windowed, doubled and scaled back by the
  brightest channel (`lightmap_shift` in `shader.wgsl`, section 3's
  `0x4d9af0`). vcod shifts the filtered sample in the shader; retail
  shifts texels at load.
- Stage gens (section 5): `vertex` scales the vertex colour by
  `identityLight`, `identityLighting`, `constLighting` and `wave` scale the
  tint (`STAGE_FLAG_VERTEX_RGB_HALF`, `STAGE_FLAG_TINT_LIGHT`); `identity`,
  `exactVertex` and `const` draw raw, so windowed shows them at half the
  full-screen brightness, as in retail.
- Props: the baked colours are computed at `identityLight` 0.5
  (`static_light.rs`) and the vertex shader rescales them by
  `2 * identityLight`, clamped at 1. Entity and viewmodel light sets are
  rescaled the same way before GL's clamp (`entity_light::pack`).
- Fog colour: `identityLight * rgb` (`0x4d2ad0`, section 5); the clear
  colour stays raw.
- HUD and fx quads: the vertex colour is scaled by the material's first
  stage gen (`renderer::colour_scale`): a material with no script is
  implicit 2D or fx and takes `rgbGen vertex`, so `identityLight`.
- Sky farbox and sun: `identityLight` (section 5).

`r_gamma` is read every frame and clamped to 0.5..3 with the write-back of
step 4, so the slider is live as in retail. The stage tint
(`rgbGen`/`alphaGen` wave and const) is clamped to 0..1, the colour bytes
retail writes.

`r_ignorehwgamma` is read at start-up and on `vid_restart` (latched). While
it is 1 the frame pass runs the gamma 1 table, and each world load reads
`r_gamma` and maps every uncompressed (TGA, JPG) image's rgb through the
unshifted ramp before upload (`gamma::bake_image`): material images,
shader-stage images, prop and model skins and fx sprites. DDS images, the
dlight blob, lightmaps and HUD images keep their bytes, as in retail.

Divergences:

- 4x MSAA resolves to the 8-bit target; retail ran without it.
- vcod's implicit vertex-lit world surfaces multiply the raw vertex colour
  in both modes. INFERRED: retail does the same, since `0x4d9af0` is called
  only for lightmap pages (section 3); the BSP vertex-colour load is not
  traced.
- The HUD and fx scale is picked from the material's first stage; a
  scripted material whose later stages use other gens is drawn with the
  first one's. Debug text (F3) draws raw.
- `r_ignorehwgamma 1` bakes on every RGBA image the paths above load; retail
  skips images uploaded at their native size without mipmaps (section 3),
  which covers shaders marked `nomipmaps`. A `vid_restart` does not reload
  the world, so a gamma change waits for the next map load, where retail's
  `vid_restart` reloads every image.
- `r_overBrightBits` is not registered; vcod uses its default 1.
- `r_intensity` is not applied (no stock menu sets it).

## 5. Stage colours and the display doubling

What retail writes to the framebuffer per `rgbGen`, before the ramp
doubles it:

- Lightmaps: the shift at load in section 3 (`0x4d9af0`, called only from
  `0x4d9f30`, a 512 x 512 loop over a page). With one overbright bit the
  shift is 0, so lightmaps reach the framebuffer raw. INFERRED (from that
  section).
- VERIFIED, the colour switch at `0x4ffa60` (Q3's `RB_CalcColors`, keyed
  on the stage's gen at `+0x664`; the gen numbers are in
  `cod11-light-grid-and-leaf-lights.md` section 9): identity (2) writes
  0xffffffff; exactVertex (5) copies the vertex colour; vertex (6) shifts
  each channel right by `overbrightBits`; oneMinusVertex (7) inverts, through
  `__ftol2` when `identityLight` is not 1; const and constLighting (0xc)
  copy the stage constant; identityLighting (1) and every unlisted gen take
  `identityLightByte`. lightingAmbient and lightingDiffuse (9, 10) call
  `0x515ad0` on a map with no lump-19 lights and write white otherwise.
- VERIFIED: in the GL state setter `0x4d57b0`, state bit 0x100000 enables
  `GL_LIGHTING` (0xb50) for a map with lump-19 lights while the cvar at
  `0x16c39e4` is 0; otherwise it disables it and, when the new state carries
  the bit, sets `glColor3f(identityLight, identityLight, identityLight)`.
  INFERRED: lit models draw through fixed-function lighting with the picked
  lights, which is why gens 9 and 10 write white.

So a framebuffer colour is either a texture times a raw lightmap, or a
texture times a gen that `identityLight` already halved, and the display
shows each of them doubled. INFERRED from the switch and section 3.

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
  explicit `rgbGen`; 166 `vertex`, 19 `exactVertex`, the latter all
  `blendFunc GL_ONE GL_ONE` (`gfx/effects/explosion/*`,
  `gfx/effects/fire/*`, `gfx/effects/misc/signal_flash`), which show
  doubled. Where the efx renderer's vertex colours come from is not traced.
- VERIFIED: `0x4d2ad0`, called by the fog setter `0x4d28e0`, sets
  `GL_FOG_COLOR` to `identityLight * rgb` with alpha raw, and to black when
  `(glState & 0xf0) == 0x20`. INFERRED: that is destination blend `GL_ONE`,
  so additive stages fog to black. VERIFIED: `glClearColor` takes the raw
  fog colour (`0x16c4c50`), and `identityLight * 0.5` with no fog and fast
  sky (`0x568e70` is 0.5f).

vcod applies this in its final pass and per gen (section 4). Before
2026-10-09 it doubled each draw instead, in linear space, which drew
lightmapped surfaces at about `2^(1/2.2)`, 1.37 times, rather than twice
the framebuffer; that is gone.
