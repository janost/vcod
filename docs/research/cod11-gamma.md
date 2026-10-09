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

vcod's frame holds retail's full-screen displayed colour, the framebuffer
doubled by the ramp's one overbright bit; section 5 says where that x2
lands. So its frame at gamma 1 stands for retail's display at gamma 1.
Every pass draws into an `Rgba16Float` scene target
(`renderer::SCENE_FORMAT`), so a doubled colour keeps retail's framebuffer
range up to 2.0 (retail's framebuffer 1.0) instead of clipping at the
display's white, and blends see the unclipped value as retail's
framebuffer does. `gamma.wgsl` then maps every frame onto the swapchain
through a 512-entry table indexed by the frame's sRGB-encoded value
`e = k / 255`, `k` in 0..512 (`crate::gamma::display_table`):

- `overbrightBits` 1 (full screen with device gamma,
  `gamma::overbright_bits`): entry `k` is the shifted ramp's entry `k/2`,
  odd entries the mean of the two around it. Below gamma 1 the values over
  1.0 show as retail's framebuffer bytes 128..255 do, as distinct shades.
- `overbrightBits` 0 (windowed, or `r_ignorehwgamma 1`): entry `k` is the
  unshifted ramp's entry `min(k, 255)`; the framebuffer byte is the display
  byte, and retail's framebuffer clamps at 1.0.

At gamma 1 both tables are the identity up to 1.0 and clamp above it.
`r_gamma` is read every frame and clamped to 0.5..3 with the write-back of
step 4, so the slider is live as in retail; the full-screen test reads the
window's state each frame. The stage tint (`rgbGen`/`alphaGen` wave and
const) is clamped to 0..1, the colour bytes retail writes, since the float
target no longer clamps it.

`r_ignorehwgamma` is read at start-up and on `vid_restart` (latched). While
it is 1 the frame pass runs the gamma 1 table, and each world load reads
`r_gamma` and maps every uncompressed (TGA, JPG) image's rgb through the
unshifted ramp before upload (`gamma::bake_image`): material images,
shader-stage images, prop and model skins and fx sprites. DDS images, the
dlight blob, lightmaps and HUD images keep their bytes, as in retail.

Memory and cost: the 4x MSAA colour target is 8 bytes a sample instead of
4 (about 66 MB at 1920x1080 against 33 MB), plus an 8-byte-a-pixel resolve
target, and the full-screen pass now runs every frame at gamma 1 too.

Divergences:

- vcod's lighting does not follow `overbrightBits`: windowed, it keeps the
  full-screen x2 of section 5 instead of retail's hue-keeping lightmap
  shift at load, and so draws `identity`, `exactVertex` and `const` stages
  at the full-screen brightness, twice retail's windowed one.
- The x2 rides each draw in linear space, and the float target never clamps
  at retail's framebuffer 1.0 (vcod 2.0 encoded): a filter over additive
  stages that passed it multiplies the unclamped sum.
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

vcod does the doubling per draw instead of in a final pass:

- The implicit lightmapped path and a `$lightmap` bundle multiply the
  lightmap by 2 (`shade` and `fs_stage` in `shader.wgsl`). Vertex-lit
  surfaces and props multiply their vertex colour by 2.
- A scripted stage with no `$lightmap` bundle takes the x2 itself
  (`STAGE_FLAG_OVERBRIGHT`, `renderer::overbright`) unless it multiplies
  the framebuffer (source factor zero or dst colour) or a later stage
  multiplies the framebuffer with a `$lightmap` bundle, which carries the
  x2 for the whole chain. Before this, `identityLighting`, `constLighting`,
  `vertex` and `wave` stages on surfaces with no lightmap drew at half
  retail's brightness.
- Doubling per draw no longer clamps at the display's 1.0 (retail's
  framebuffer 0.5): the float scene target of section 4 keeps the value up
  to the final pass, so a blend over a bright background sees what
  retail's framebuffer held.
- Effects (`fx.wgsl`), the sky farbox, entity models and the HUD keep
  their own scales; how retail colours those is not traced here.

