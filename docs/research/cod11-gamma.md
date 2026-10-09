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

So with defaults in full screen (`r_overBrightBits 1`), the framebuffer
holds half-bright lighting and the ramp doubles it after gamma:
`display = min(255, round(255 * (fb/255)^(1/g)) << 1)`. Windowed, or with
`r_ignorehwgamma 1`, `overbrightBits` is 0: no doubling, and with
`r_ignorehwgamma 1` the gamma is baked into textures at load (latched until
`vid_restart`, which is why the menu moves the slider). INFERRED (from
steps 1, 5 and 7 and `0x4eaa90`).

## 4. What vcod does

vcod's world shading already carries the one overbright bit (the x2 on
lightmaps in `shader.wgsl`), so its frame at gamma 1 stands for retail's
full-screen display at gamma 1. `crates/client/src/gamma.rs` builds
retail's table (`ramp`, step 5, shift 1) into a 256-entry texture; when
`r_gamma` is not 1 the frame renders offscreen and `gamma.wgsl` maps each
channel's display byte `e` through table entry `e/2` (linear between
entries) onto the swapchain, in sRGB-encoded values. At gamma 1 the pass is
skipped, since the table is then the identity on vcod's frame. `r_gamma`
is read every frame and clamped to 0.5..3 with the write-back of step 4,
so the slider is live as in retail.

Divergences:

- vcod always models full screen with `r_overBrightBits 1`. Retail
  windowed has no overbright doubling; vcod does not change its lighting
  for windowed play, `r_overBrightBits` or `r_ignorehwgamma`.
- `r_ignorehwgamma 1` does not move gamma into texture load; the slider
  stays live through the same pass.
- vcod's frame clamps at retail framebuffer byte 127.5 (its display
  white). Below gamma 1 retail shows framebuffer bytes above that as
  distinct brighter shades; vcod caps them at the table entry for 127.5
  (about 128 at gamma 0.5).
- `r_intensity` is not applied (no stock menu sets it).
