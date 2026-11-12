# CoD2 1.3 effects (`.efx`)

CoD2 keeps CoD1's effect elements (`efx-grammar.md`) and adds `Cloud`,
`CameraShake` and `Flash`. The loader is new: Raven's `GenericParser2`
text into `PrimitiveTemplate`s. Every graph is a channel, a
piecewise-linear curve over the element's life times a random scale.
CoD1-style `start`/`end` graphs still load, migrated into curves.

vcod: the parser is `vcod_cod2::efx`; the client runs the result through
its CoD1 fx system (`crates/client/src/fx/cod2.rs`) and plays the
spectator's effects in `crates/client/src/play/cod2_fx.rs`.

## 0. Modules

| Module | md5 | Role |
|---|---|---|
| `cod2mp_mac13_i386` (Mac 1.3 MP, i386 slice, Mach-O, STABS) | `b30966627b680de20bf35603c562a3f6` | every address below; image base 0 |
| `cod2_lnxded` (Linux 1.3 dedicated server) | `fefed4c4909fb7c5c13b13a003f5ccf7` | the event emitters, marked "lnxded" |
| `main/iw_07.iwd` | | all 375 stock `fx/**/*.efx` and `fx/iw_impacts.csv` (VERIFIED: no other stock iwd holds an `fx/` entry) |

Function names are the Mac build's symbols. Data tables come from the
Mach-O's `__const` and `__data` sections, by symbol (`llvm-nm`).

## 1. Grammar

### 1.1 Tokens and groups

`FX_ParseEffect` (0x5d218) reads `fx/<name>.efx` whole and hands it to
`GenericParser2::Parse` (0xac244), then `GPGroup::Parse` (0xac080) over
`GetToken` (0xab80c). VERIFIED.

- A key is a word up to the next byte below `!`. `GetToken` compares the
  byte as a signed `char`, so every byte from 0x80 up is blank. VERIFIED
  (the `cVar3 < '!'` tests on a `char`).
- After a key, the next token is read in rest-of-line mode: from the next
  non-blank byte (newlines skipped) up to `\n`, `\r`, `//` or `/*`,
  trailing bytes below space trimmed, one pair of quotes stripped.
  VERIFIED.
- A value of `{` opens a subgroup named by the key; `[` opens a list whose
  items are rest-of-line tokens up to `]`; anything else is a pair
  `(key, value)`. VERIFIED. A group ends at a `}` key or at the end of the
  text; a list that never closes fails the parse. VERIFIED.
- `//` runs to the end of the line; `/* */` is skipped between tokens.
  VERIFIED.
- Pairs and subgroups are two separate lists on a group (`+0x10` and
  `+0x1c`), each in file order. VERIFIED.

`fx/muzzleflashes/panzerschreck_flash.efx` closes its first block with `}`
then byte 0xf6. Under the signed compare that byte is blank, so the file
parses with every block in place. VERIFIED (the bytes, and the compare).

### 1.2 Blocks

A top-level group becomes a primitive when its name matches, without case,
one of the kinds below (the number is `PrimitiveTemplate +0x40`). VERIFIED
(the `strcasecmp` chain in 0x5d218).

| # | kind | # | kind |
|---|---|---|---|
| 1 | `particle` | 7 | `orientedparticle` |
| 2 | `line` | 8 | `fxrunner` |
| 3 | `tail` | 9 | `light` |
| 4 | `cylinder` | 10 | `cameraShake` |
| 5 | `emitter` | 11 | `flash` |
| 6 | `decal` | 12 | `cloud` |

- A group with any other name is skipped silently. VERIFIED.
- A primitive that fails to parse fails the whole effect, as does a
  Particle, Tail or OrientedParticle with no material while a renderer
  exists. VERIFIED.
- At most 24 primitives are kept; from the 25th on each prints `too many
  primitives` and is dropped. VERIFIED.

A refused effect: `FX_RegisterEffect` (0x5d5fa) returns `_defaultEffect`,
which `FX_CreateDefaultEffect` (0x5d116) registers from `misc/missing_fx`.
VERIFIED. Registration drops one leading `/` or `\`, requires an `fx/`
prefix, strips the extension and lowercases the name. VERIFIED.

Stock census (`vcod_cod2::efx` over `iw_07.iwd`, VERIFIED by
`crates/cod2/tests/efx_stock.rs`): Particle 858, Tail 143, Emitter 111,
FxRunner 98, Decal 79, Light 50, OrientedParticle 20, Cloud 9,
CameraShake 1, Line 1. No `Cylinder`, `Flash` or CoD1 `Sound` block ships.

### 1.3 Primitive keys

`ParsePrimitiveInternal` (0x5f778) reads the pairs first, then the
subgroups (the channels, section 2). VERIFIED. An unknown pair key prints
`Unknown key parsing an effect primitive` and fails the primitive.
VERIFIED. A one-number value of a two-number key is doubled (`sscanf`
returning 1). VERIFIED. The pair keys (aliases after `/`), each VERIFIED
from its `stricmp` arm:

| Key | Values | `PrimitiveTemplate` | Note |
|---|---|---|---|
| `count` | 1-2 | +0x50 | |
| `shaders` / `shader` | list or one | +0x68 handles | `Material_RegisterHandle(name, 3, 6)` |
| `models` / `model` | list or one | | |
| `impactfx` | list or one | | sets `impactFx | usePhysics` (0x820) |
| `deathfx` | list or one | | sets `deathFx` (0x200) |
| `emitfx` | list or one | | sets `emitFx` (0x100) |
| `playfx` | list or one | | |
| `life` | 1-2 | +0x58 | ms |
| `cullrange` | 1 | +0x64 | the far end of `spawnRange` alone |
| `spawnRange` | 1-2 | +0x60, +0x64 | fails when max < min |
| `delay` | 1-2 | +0x48 | ms |
| `bounce` / `intensity` | 1-2 | +0x280 | each end in 0..1, max >= min; sets `usePhysics` |
| `min`, `max` | 3 | +0xa0, +0xac | set `usePhysics | useBBox` (0x60) |
| `angle` / `angles`, `angleDelta` | 3 or 6 | +0x228, +0x240 | |
| `velocity` / `vel` | 3 or 6 | channels 12..17 | section 3.2 |
| `acceleration` / `accel` | 3 or 6 | channels 18..23 | section 3.3 |
| `gravity` | 1-2 | +0x258 | |
| `density`, `variance` | 1-2 | +0x260, +0x268 | |
| `origin`, `origin2` | 3 or 6 | +0xb8, +0xd0 | |
| `radius`, `height`, `wind` | 1-2 | +0xe8, +0xf0, +0xf8 | |
| `rotation` | 1-2 | +0x220 | degrees |
| `rotationDelta` | 1-2 | channels 10, 11 | section 3.4; as a group, a channel |
| `flags` / `flag` | words | +0x90 | table 1.4 |
| `spawnFlags` / `spawnFlag` | words | +0x94 | table 1.4 |
| `nonUniformScale`, `useLength` | `atoi` | +0x9c, +0x9d | |
| `name`, `shaderImpact` | text | +0x0, +0x20 | 32 bytes |
| `sequenceStartFrameMode` | 0..2 | +0x288 | |
| `sequenceFixedFrameValue` | >= 1 | +0x28c | |
| `sequencePlayRateMode` | 0..1 | +0x290 | |
| `sequenceFixedFpsValue` | >= 0 | +0x294 | |
| `sequenceLoopMode` | 0..1 | +0x298 | |
| `sequenceLoopTimes` | >= 0 | +0x29c | |
| `spawnFrustumCullRadius` | >= 0 | +0x2a0 | |

A 3-or-6 key fails on 4 or 5 numbers. VERIFIED. After the pairs, a
`min`/`max` box with any min above its max, or wider or deeper than it is
tall, fails the primitive. VERIFIED.

`PrimitiveTemplate::Init` (0x5d9b4) defaults: `life` 1, `count` 1,
`radius` 1, `height` 1, `density` 10, `variance` 1, fixed frame 1, fps 1,
loop times 1, every channel's scale 1. VERIFIED.

### 1.4 Flags

`ParseFlags` (0x5e5c4) fails the primitive unless every word names an
entry of its table. VERIFIED.

`_fxAttributeFlags` (0x36a800, 24 rows of name, `+0x90` bits, `+0x94`
bits). VERIFIED.

| flag | bit | flag | bit |
|---|---|---|---|
| `depthHack` | 0x1 | `useRandomColors` | 0x2000 |
| `relative` | 0x2 | `useRandomAlpha` | 0x4000 |
| `setShaderTime` | 0x4 | `useRandomSize` | 0x8000 |
| `useModel` | 0x10 | `useRandomSize2` | 0x10000 |
| `usePhysics` | 0x20 | `useRandomLength` | 0x20000 |
| `useBBox` | 0x40 | `useRandomRotationDelta` | 0x40000 |
| `useAlpha` | 0x80 | `useRandomVelocity` | 0x80000 |
| `emitFx` | 0x100 | `useRandomVelocity2` | 0x100000 |
| `deathFx` | 0x200 | `absoluteVel` | 0x200000 |
| `impactKills` | 0x400 | `absoluteVel2` | 0x400000 |
| `impactFx` | 0x800 | `affectedByWind` | 0x800000 |
| `blocksSight` | 0x1000 | `disableFarPlaneCulling` | 0x2000000 |

`_fxSpawnFlags` (0x36a760, 13 rows). VERIFIED.

| flag | bit | flag | bit |
|---|---|---|---|
| `orgOnSphere` | 0x1 | `cheapOrg2Calc` | 0x80 |
| `axisFromSphere` | 0x2 | `randrotaroundfwd` | 0x100 |
| `orgOnCylinder` | 0x4 | `evenDistribution` | 0x200 |
| `org2fromTrace` | 0x8 | `frustumCull` | 0x400 |
| `traceImpactFx` | 0x10 | `absoluteVel` | attribute 0x200000 |
| `org2isOffset` | 0x20 | `absoluteAccel` | attribute 0x400000 (`absoluteVel2`) |
| `cheapOrgCalc` | 0x40 | | |

### 1.5 The one refused stock file

`fx/impacts/wall_impact_small_dirt.efx` writes `rgbComponentInterpolation`
in three `spawnFlags` lines. That name is in neither table, so the engine
refuses the file and plays `misc/missing_fx` for it. VERIFIED (the asset
and the tables). No impact CSV row names it, so no stock event reaches the
fallback. INFERRED. vcod refuses it the same way.

## 2. Channels and curves

### 2.1 The 24 channels

A channel is `PrimitiveTemplate +0x100 + 12 * id`: an `FxCurve *` and a
scale range. VERIFIED. Group names (aliases after `/`), VERIFIED from
`ParsePrimitiveInternal`'s second loop:

| id | group | id | group |
|---|---|---|---|
| 0 | `rgb` | 7 | `size2Rand` |
| 1 | `rgb2` / `rgbRand` | 8 | `length` / `height` |
| 2 | `alpha` | 9 | `lengthRand` |
| 3 | `alphaRand` | 10 | `rotationDelta` |
| 4 | `size` / `width` | 11 | `rotationDeltaRand` |
| 5 | `sizeRand` | 12..14 / 15..17 | `velocityX/Y/Z` / `velocityXRand..ZRand` |
| 6 | `size2` / `width2` | 18..20 / 21..23 | `velocity2X/Y/Z` / `velocity2XRand..ZRand` |

Any other group name fails the primitive. VERIFIED.

Inside a channel group (`ParseChannel`, 0x5e8d0): `curve [ ... ]`, `scale`
(1-2 numbers), and CoD1's `start`, `end`, `parm` / `parms`, `flags` /
`flag` (section 3). VERIFIED. An unknown key prints `Unknown key parsing a
channel` and is skipped, not fatal. VERIFIED.

`Init` makes every channel a two-key line (`FxChannel_CreateDefault`,
0xac458), scale 1: rgb, rgbRand, alpha .. lengthRand run from 1 at t=0 to
0 at t=1; rotation delta and the velocities are flat 0. VERIFIED.

### 2.2 Curve rows

A colour channel (ids 0, 1) reads each row with `%f %f %f %f` as
`t r g b`; every other channel reads `%f %f` as `t v` and ignores the rest
of the row (`ParseChannelRgbCurve` 0x5e0a8, `ParseChannelCurve` 0x5dd36).
VERIFIED. The stock files write four numbers on every row of every
channel; on a 1-wide channel the last two are editor leftovers. VERIFIED
(the corpus).

`FxCurve` (`FxCurve_AllocAndCreateWithKeys`, 0xac320) is `{dims, count,
keys[(dims + 1) * count]}`. VERIFIED. When the first key's `t` is not 0, a
key at 0 with the first key's values goes in front; when the last `t` is
not 1, a key at 1 is appended, which as decompiled also copies the first
key's values. VERIFIED (decompile). All 30 867 stock curves start at 0 and
end at 1 with ascending `t`, so neither pad is taken. VERIFIED (the
corpus).

### 2.3 Sampling

Every reader (`Particle::UpdateRGB` 0xa1c6a, `UpdateAlpha` 0xa5178,
`CalcVelocityValue` 0xa2f16, `Update` 0xa7970) walks a cached segment
index to the first key pair whose second `t` is at or past the sample's,
then interpolates linearly between the two keys. VERIFIED. The sample is
the particle's life fraction, `(now - start) / (end - start)` clamped to
0..1 (`Particle::Update` +0x3c). VERIFIED.

A channel's value is `lerp(curve(t), randCurve(t), w) * scale` when the
primitive's `useRandom*` flag for it is set, else `curve(t) * scale`; `w`
and `scale` are drawn once per particle. VERIFIED (the two arms of each
reader). The scale is the channel's own (`+0x8` of its instance,
`FxRange::GetVal` of the template's range in `FxChannelInstance_Create`
0x9ff88); the rand curve's scale is never read. VERIFIED.

Colour is clamped per component to 0..255 after `* 255`. VERIFIED. Alpha
is clamped to 0..1; under `useAlpha` it becomes the alpha byte, without it
it scales the three colour bytes and alpha is 255. VERIFIED.

`FxCurve_Integrate` (0xb9fde) is the area under a 1-wide curve from 0 to
`t`: whole segments by the trapezoid rule, the last one up to `t`.
VERIFIED. Its multiplier at 0x326190 reads 0.5. VERIFIED.

## 3. CoD1 graphs migrated

Seven stock files still use `start`/`end` graphs (`fireheavysmoke`,
`grenade_flash`, `coolaidmanbrick`, `minefield`, `flare`,
`elalamein_trenchdust`, `large_smoke_plume`). VERIFIED (the corpus).

### 3.1 Channel graphs

`ParseChannel` stores CoD1 keys in a 0x40-byte `BackCompatibleParameters`
record per channel: per component a `start` and an `end` range (default
1, 1), a `parm` range (default 1), the group flags, a present byte.
VERIFIED. Group flag words (`ParseGroupFlags`, 0x5d79c): `linear` 1,
`random` 2, `nonlinear` 4, `wave` 8, `clamp` 0xc. VERIFIED. Once every
group is read, each present record replaces its channel through
`FxChannel_CreateViaMigration` (0xac4e4), alpha with normalisation off.
VERIFIED.

What the migration builds, VERIFIED off the decompile's arithmetic:

- Values are the ranges' midpoints. Except for alpha, the larger of the
  start and end midpoints of the first component, times 4/3 (0x3263cc),
  becomes the scale when it is at least 1, and the values are divided by
  it; the scale range is that +- half the first start range.
- `parm`'s midpoint (divided likewise) times 0.01 (0x3262f0) is a life
  fraction `p`; times pi / 1000 (0x3263d0) it is a wave rate.

The curve shapes, INFERRED from the branch structure:

- `linear` alone: a line from start to end.
- `nonlinear` alone: start held to `p`, then a line to end.
- No shape flag without `linear`: start held (sixteen equal keys).
- Any other mix: sixteen keys at `t = i / 15` of
  `w * start + (1 - w) * end`, clamped at 0, with `w` the start weight:
  `1 - t` under `linear` (1 without), times `cos(rate * life * t)` under
  `wave`, `(p - t) / p` before `p` under `clamp` (averaged with `1 - t`
  under `linear`), and under `linear | nonlinear` the average of `1 - t`
  and 1 up to `p`, falling to 0 at 1.
- `random`: each key a uniform draw below that value, made once at load.

vcod builds every case but `random`, which it takes as the value itself.

### 3.2 `velocity`

`ParseVelocity` (0x5ed92) sorts each axis's pair, takes the largest
magnitude as the scale of channels 12..17, and writes flat curves at
`min / scale` (12..14) and `max / scale` (15..17); any axis with
min != max sets `useRandomVelocity`. VERIFIED. A CoD1 velocity range thus
becomes the same per-axis uniform draw as before. INFERRED.

### 3.3 `acceleration`

`ParseAcceleration` (0x5f244) writes channels 18..23 as lines from 0 at
`t = 0` to `accel * life * 0.001` at `t = 1` over a common scale, with
`useRandomVelocity2` for a range; `life` is the longer end of the `life`
parsed so far. VERIFIED. The second velocity grows as the acceleration
would grow a CoD1 velocity over the particle's longest life. INFERRED.

### 3.4 `rotationDelta` as a pair

`ParseRotationDelta` (0x5e79e) sorts the pair and takes
`max(|(int)a|, b)` as the scale. `CreateBackCompatibleRotationDeltaCurve`
(0x5e16e) writes channel 10 from `a` and channel 11 from `b`: twenty keys
at `t = i / 19`, the first `a / scale`, each next one the last times
`1 - life / 20 * 0.00065` (clamped to 0..1), `life` the shorter end for
channel 10 and the longer for 11. `a != b` sets `useRandomRotationDelta`.
VERIFIED (0x32624c 20, 0x326250 0.00065, 0x326254 19).

## 4. Runtime

### 4.1 Playing an effect

`FxScheduler::PlayEffect` (0x61322), per primitive. VERIFIED.

- `spawnRange`: skipped when the effect's origin is nearer the camera than
  the near end, or (far end non-zero) farther than the far end, both
  scaled by `theFxHelper +0xf8`.
- `frustumCull`: skipped when a sphere of `spawnFrustumCullRadius` was
  outside last frame's frustum.
- `count` is `(int)(draw + 0.5)`.
- Without `evenDistribution` each spawn draws its delay; under 1 ms it
  spawns at once, else it is scheduled. With it, spawn `i` is delayed
  `(int)(i * |delayMax - delayMin| / count)` ms, the near end not added.

### 4.2 A particle's spawn

`FX_InitParticle` (0x59d78) draws, in order: one `flrand(0, 1)` weight for
each set flag of `useRandomColors`, `useRandomAlpha`, `useRandomSize`,
`useRandomSize2`, `useRandomRotationDelta`; three for `useRandomVelocity`
and three for `useRandomVelocity2`; then gravity, wind, origin and axis,
`bounce`, and `rotation`. No weight is drawn for `useRandomLength` there.
VERIFIED.

### 4.3 Motion

`Particle::IntegrateTotalVelocity` (0xa25fc) places a particle at its
spawn origin plus `(I1(t) + I2(t)) * life_s` plus `gravity * s^2` on world
z. `I1` and `I2` are the velocity and velocity2 channels' integrals to the
life fraction `t`, blended and scaled as in 2.3, turned onto the effect
axis unless `absoluteVel` / `absoluteVel2` keeps them in world space; `s`
is the age in seconds. VERIFIED (`IntegrateVelocity` 0xa20b4 multiplies by
`life_ms * 0.001`; the gravity term has no half).

Rotation: each update adds `rotationDelta(t) * frame_ms * 0.01` degrees,
so the channel is degrees per 100 ms. VERIFIED (0xa7970,
`theFxHelper +0xc` times 0x3262f0).

### 4.4 Atlas sequences

`FX_SetMaterialAndSequenceParams` (0x5a4fc), for a material with more
than one sub-image (the material file's atlas rows times columns,
cod2-materials.md 2). VERIFIED.

| mode | start frame (`sequenceStartFrameMode`) | rate (`sequencePlayRateMode`) |
|---|---|---|
| 0 | `sequenceFixedFrameValue - 1` | `fps / 1000` frames per ms |
| 1 | random | `frames / life` |
| 2 | the spawn index | |

`Particle::Update` then draws frame `start + (int)(age_ms * rate)`. Past
the last frame, loop mode 0 wraps; loop mode 1 holds the last frame once
`sequenceLoopTimes` loops have run (at once when it is 0). VERIFIED.

### 4.5 The draw budget

- Pool: `FX_AddPrimitive` (0x5a62a) refuses a primitive once
  `_effectActiveCount + 1` passes 0x708 (1800 live elements, bolted and
  unbolted together). VERIFIED. vcod's pool is `fx::sim::MAX_PARTICLES`,
  2048.
- Cull: `FX_DrawAll` (0x5a154) keeps an element only when `fx_cull` (bool,
  default 1, registered in `CL_InitOnceForAllClients` 0x1485e2) is off or
  its `Cull` returns 0, then sorts the kept ones by camera distance under
  `fx_sort` (default 1) and draws them. VERIFIED. `Particle::Cull` and
  `OrientedParticle::Cull` (0xa0758, 0xa07e4) test a sphere at the
  element's origin (+0x7c) of its radius (+0x88) against the view's planes
  (`FxHelper::CullSphere` 0xab0d0, plane count `theFxHelper +0x80`, at most
  5 under flag `+0xab & 2`); `Effect::Cull` and `Emitter::Cull` return 0.
  VERIFIED. There is no distance cull at draw time; distance enters only
  through `spawnRange` at spawn (4.1). VERIFIED (no other test in
  `FX_DrawAll`). vcod draws every live sprite and lets the GPU clip those
  off screen, which draws what retail draws.
- Samples: `r_aaSamples` registers 1 in 1..4 (`R_RegisterDvars` 0xc8f98),
  so a stock CoD2 client blends its smoke single-sampled. VERIFIED. vcod
  registers the same default for CoD2 (`Shell::register_aa_samples`).

A stock smoke grenade (`fx/props/american_smoke_grenade.efx` running
`fx/explosions/smoke_grenade.efx`) holds 261 live elements at its first
frame and 95 sprites from 10 s to 35 s, gone by 45 s. VERIFIED (vcod's sim
on the stock file, 60 Hz). Their projected boxes cover about 35 screens at
1920x1080 and fov 80 from 400 units off at 30 s, and 26-34 from inside the
cloud. INFERRED (bounding boxes of the sprites, an overestimate of their
pixels).

## 5. Element kinds

- `Cloud` (`FX_AddCloud` 0x5a906): a Particle subclass placed by the same
  integral, with one extra `flrand(0, 1)` (the `length` channel's random
  weight) and `useLength` stored; drawn by the renderer's particle-cloud
  path (`R_CreateParticleCloudBuffer`, `RB_TessParticleCloud`). VERIFIED.
  One particle is a cube of 1024 flakes, `size` its half-extent, `size2`
  (under `nonUniformScale`) the flakes' width, spun by `rotationDelta`
  about an axis drawn at spawn. VERIFIED (cod2-materials.md 9.14, which
  has the draw and vcod's port: `fx::cod2_cloud`, `renderer::fx_cloud`).
- `CameraShake` (`FX_AddCameraShake` 0x59fec): hands
  `FxHelper::CameraShake` the element's origin, its `bounce`/`intensity`
  draw and its `radius` draw; the `life` draw is made and not passed.
  VERIFIED. The one stock block (in `fx/explosions/`) has no `intensity`.
  VERIFIED. `FxHelper::CameraShake` (0xab0b0, 5 bytes) is a bare `return`
  in the 1.3 Mac client, so the element shakes nothing on retail. VERIFIED.
  vcod logs it at debug and shakes nothing. A CoD2 view's shakes come off
  `EV_EARTHQUAKE` and `EV_FIRE_WEAPON_MG42` (cod2-hud.md 17.2).
- `Flash` (`FX_AddFlash` 0x5aa6c): a `Light` subclass with a material. No
  stock file has one. VERIFIED. vcod parses the kind and draws nothing for
  it.
- Particle, OrientedParticle, Tail, Line, Decal, Light, Emitter and
  FxRunner map onto the CoD1 sim's behaviours of the same name
  (`fx::sim::classify`).

## 6. Effects a spectator sees

### 6.1 Finding events

`CG_CheckEvents` (0x1e614a). VERIFIED.

- An entity with `eType > 10` raises event `eType - 10` once and sets its
  `previousEvent` (`cent +0x1e4`).
- Any other runs its ring from `previousEvent` to `es.eventSequence` (a
  lower sequence wraps by 256; more than 4 behind skips to the last 4),
  each slot `i & 3` raising `events[i & 3]` with `eventParms[i & 3]` as
  the parm.

`CG_ResetEntity` (0x1deb10), run on an entity entering the snapshot,
starts `previousEvent` at 0 for `ET_GENERAL` and `ET_MISSILE` and at the
current sequence for a player and the other types. A corpse starts at 0
when its `eFlags` carry 0x80000 (the `cent[0xfa] & 8` test), else at its
sequence. VERIFIED. `CG_SetNextSnap` (0x1dee08) runs it on an entity not
in the last snapshot and on one whose `eFlags` 0x2 (the teleport bit)
flipped. VERIFIED. vcod's one tracker for sounds and effects is
`play::cod2_events::EventTracker`.

### 6.2 The bullet hits

The event name table (lnxded 0x8184da0, Mac `_eventnames` 0x3435e0,
identical) names 181 `EV_BULLET_HIT_SMALL`, 182 `EV_BULLET_HIT_LARGE`, 183
`EV_SHOTGUN_HIT`, 184 `EV_BULLET_HIT_AP`. VERIFIED. The binaries do not use
the numbers as named. VERIFIED:

- The server's bullet code (lnxded `FUN_0811fe90`) sets a local to 181 for
  a weapon of type 3 and to 182 or 183 by the weapon def's `+0x31c`, but
  the world-hit temp entity takes 182 or 183 by `+0x31c` alone; the
  decompile shows no read of the 181. The script's damage path (lnxded
  `PlayerCmd_finishPlayerDamage` 0x80fb170) writes 182 or 183, then 185 or
  186, the same way.
- `CG_BulletHitEvent` (0x1db1f4) plays impact rows 0/1 for 182, rows 2/3
  for 183, rows 4/5 for anything else. `CG_EntityEvent` sends 181..183
  there and has no arm for 184.

Rows 0..5 are named `bullet_small_*`, `bullet_large_*`, `shotgun_*`
(`g_TypeName`, 0x36e7e0). VERIFIED. So 181 is a shotgun hit, 182 a small
round and 183 a large one. INFERRED. The sounds agree: `CG_BulletHitEvent`
plays `cgs + 0x27f24` for 182, `+0x27fdc` for 183 and `+0x27f80`
otherwise, which `CG_RegisterSounds` fills with `bullet_small_<surf>`,
`bullet_large_<surf>` and `bulletspray_small_<surf>`. VERIFIED.
`vcod_cod2::net::events` names 181 `EV_SHOTGUN_HIT`, 182
`EV_BULLET_HIT_SMALL` and 183 `EV_BULLET_HIT_LARGE` by this reading, not
the table's strings.

A world hit plays the row's `*_normal` effect at the hit along
`ByteToDir(eventParm)` (the surface normal) and its `*_reflect` effect
along `ByteToDir(es.scale)`, on surface `es.surfType`. VERIFIED (both
`ByteToDir`s in `CG_EntityEvent`; the server writes the trace normal and
the reflected shot into those fields). With `cg_blood 0` a flesh hit plays
`cgs +0x28384` instead. VERIFIED.

### 6.3 The impact table

`CG_RegisterImpactEffects` (0x1e7dde) reads every `.csv` in `fx/`, then in
`fx/maps/<mapname>/`, each directory's files sorted by
`compare_impact_files`, a later row overwriting a cell. VERIFIED. A row is
`kind,surface,effect`: an unknown kind or surface is an error, a blank
effect plays nothing. VERIFIED. Rows are the eleven `g_TypeName` kinds,
columns the 23 surface types. VERIFIED. The stock `fx/iw_impacts.csv`
fills 138 cells (139 rows with an effect; `bullet_ap_normal` names one
surface twice). VERIFIED (asset; `play::cod2_fx` test).

### 6.4 The other effect arms

From `CG_EntityEvent` (0x1e4650). VERIFIED.

| Event | Effect |
|---|---|
| 187 `EV_GRENADE_BOUNCE` | impact row 8 for `surfType`, along `ByteToDir(eventParm)` |
| 188 `EV_GRENADE_EXPLODE` | row 9, then the weapon's explosion effect (`cg_weapons +0x164`) |
| 189 `EV_ROCKET_EXPLODE`, 190 `_NOMARKS` | row 10, then the weapon's explosion |
| 191 `EV_CUSTOM_EXPLODE` | the weapon's explosion alone, played at `es.time` (`FX_WarpTime`) |
| 194 `EV_PLAY_FX` | effect `eventParm` (1..63, configstring 846 + id) on `AngleVectors(apos)`'s forward and up |
| 195 `EV_PLAY_FX_ON_TAG` | configstring 910 + parm names the effect and tag, played on the entity's bone |
| 196 `EV_EARTHQUAKE` | a camera shake (cod2-hud.md 17.2), no effect |
| 197 `EV_GRENADE_SUICIDE` | row 9 of surface 0, straight up |
| 158..160, 175..177 (fire) | `CG_FireWeapon`: a turret's flash on its barrel tag, a player's flash flagged for his gun's next draw, the brass (6.8) |
| 162 `EV_EJECT_BRASS` | `CG_EjectWeaponBrass` (6.8) |

The weapon's explosion is the weapon file's `projExplosionEffect`:
`CG_RegisterWeapon` (0x1d85cc) registers def +0x394 into `cg_weapons
+0x164`. VERIFIED (the store; the key's offset is `weapon::FIELDS`'s).
191 plays without its time warp; 195 is not played.

### 6.5 Looping effects (`ET_FX`, eType 8)

`Scr_PlayLoopedFX` (0x19c6d0) spawns an entity with `eType` 8, the effect
id in `es.scale`, the forward in `origin2`, `DirToByte(up)` in
`eventParm`, the cull distance in `angles2[0]` and the period in ms
(`floor(seconds * 1000 + 0.5)`) in `angles2[1]`. VERIFIED.

`CG_ProcessEntity`'s arm 8 (0x164bd4). VERIFIED.

- If `cg.time` is behind the last play (`cent +0x210`) the last play
  resets to now.
- Otherwise it returns while less than a period has passed, then advances
  the last play by whole periods.
- It plays only inside the cull distance of the view (0 never culls):
  with no axis for a zero forward, the forward alone for a zero up, else
  the forward with the up made square to it.
- The id must be 1..63.

`crates/cod2/tests/fixtures/mp_toujane-spectate.bin` carries 19 such
entities (17 `fx/dust/dust_wind_brown.efx`, 2
`fx/smoke/smoke_plumeBG_toujane.efx`); replayed through `play::cod2_fx`
each plays on first sight and again on its period. VERIFIED (capture).

### 6.6 Script effects on the server

What `cod2_lnxded`'s builtins put on the wire, read in the Linux build:

- `loadFx` fills the effect block from configstring 847
  (`G_EffectIndex` 0x811d600, base 846); the id is the offset from 846.
  VERIFIED live: mp_toujane's `_fx` script's four paths sit at 847..850 in
  load order.
- `playLoopedFx(id, delay, origin[, cull[, forward[, up]]])`
  (`Scr_PlayLoopedFX` 0x8115466) takes three to six arguments; a zero
  forward or up is a script error. It runs `G_Spawn`, sets `eType` 8,
  `r.svFlags |= 8` (broadcast), `scale` (es+0xd8) the id's low byte,
  `G_SetOrigin`, `origin2` (es+0x5c) the forward, `eventParm` (es+0xa0)
  `DirToByte(up)`, `angles2[0]` the cull, `angles2[1]` the period as a
  float, links it and returns the entity. The period is
  `floor(delay * 1000.0 + 0.5)` (0x81185e6, the 1000.0 at 0x815c9e0, the
  0.5 at 0x815e188) and must be at least 1. VERIFIED.
- `playFx(id, origin[, forward[, up]])` (`Scr_PlayFX` 0x81150a4) takes two
  to four arguments: `G_TempEntity(origin, 194)` (0x811efc4: `eType` 10 +
  event, origin rounded to whole units, freed 300 ms on as CoD1's), the
  id's low byte in `eventParm` and the effect's angles in `apos.trBase`
  (es+0x3c). The angles are (-90, 0, 0) (the 0xc2b40000) without a
  forward, `vectoangles(forward)` with one, and with an up `AxisToAngles`
  of (forward, up x forward, up), the up made square to the forward when
  `|up x forward|` is under 0.9999 (0x815c700) and refused under 0.001
  (0x815c6fc). VERIFIED.
- `AxisToAngles` (0x80a9558) takes pitch and yaw from
  `vectoangles(axis[0])` and the roll from `axis[1]` turned by minus the
  yaw and minus the pitch: its pitch-style angle (0x80a4f2c, the -180.0 at
  0x814d0a0) negated, or pushed 180 round when the turned vector's y is
  negative (0x814d210). VERIFIED.

The stock map scripts reach these through `maps/mp/_fx.gsc`. VERIFIED (iwd
scripts).

- `loopfx` waits a frame and calls `playLoopedFx` with the forward
  `vectorNormalize(org2 - org)` and a cull of 0.
- `oneShotfx` and the exploders call `playFx`.
- `soundfx` spawns a `script_origin` and `playLoopSound`s on it, then
  assigns fields on `level thread createfx_showOrigin(...)`'s value, the
  struct the thread returned before it suspended (cod2-gsc-diff.md 3.5),
  so no error follows. VERIFIED live: retail's console printed none for it
  on mp_breakout, and mp_burgundy's `main` ran on past its `soundfx` line
  on retail at `developer 1` (cod2-protocol.md 15.7).
- The `script_origin` goes out to every client while it loops
  (`playLoopSound` sets `r.broadcastTime` -1, lnxded 0x8111e0c, which the
  snapshot builder takes before `SVF_NOCLIENT`). VERIFIED.

Retail's mp_toujane
(`crates/server/tests/fixtures/cod2-mp_toujane-mapfx.bin`, a lone
spectator; recipe on `cod2::tests::MAPFX_FIXTURE`) carries the 19 `ET_FX`
entities in its baselines and in every snapshot, all `origin2` (0, 0, 1),
`eventParm` 0, `angles2[0]` 0; `scale` 2 and period 300 for the 17 dust
loops, `scale` 4 and period 1000 for the two smoke plumes. VERIFIED
(capture). vcod-server sends the same states field for field and in the
same order (`mp_toujane_ambient_fx_match_retail`), at lower numbers
(258..276 against retail's 619..637), as it numbers every map entity
lower.

Trap: vcod's 1.1 entity builder names the word `_union.scale`, CoD2's
table plain `scale`. A write under the 1.1 name was once dropped, sending
`scale` 0, and the client drew effect 0 (no smoke).
`EntityState::field_index` falls back from a `_union.` name to its bare
one. `cod2_stock_map_fx_scripts_run` runs every stock map's script for
three seconds: each `loopfx` line of its `_fx` script gives an `ET_FX`
entity and no builtin is unanswered.

### 6.7 Bullet tracers

Read in the Mac cgame (`cod2mp_mac13_i386`).

- `CG_EntityEvent` (0x1e4650) sends 181..183 to `CG_BulletHitEvent`
  (0x1db1f4) and 185, 186 to `CG_BulletHitClientEvent` (0x1d9b78), the
  shooter in both as `es.otherEntityNum` and the hit as the event's
  position. VERIFIED. 184 has no arm (6.2). The server writes the shooter
  there on a world hit and on both flesh hits (cod2-combat.md, lnxded
  `Bullet_Fire_Extended`). VERIFIED.
- After the sound and the impact effects, both run the same tail, only
  when `cg_tracerchance` is above 0. VERIFIED. It calls
  `CG_CalcMuzzlePoint` (0x1d7f72) on `_s_barrelTags[0]`, the first of
  `tag_flash`, `tag_flash_11`, `tag_flash_2`, `tag_flash_22`, which
  `GScr_LoadConsts` (0x1cb694) allocates in that order into
  0x1ac8f0c..0x1ac8f12. VERIFIED. That `_s_barrelTags` is 0x1ac8f0c is
  INFERRED. When `CG_CalcMuzzlePoint` returns 0 nothing follows. Then,
  unless `ps.pm_flags & 0xc00000` is set and the shooter is
  `ps.clientNum`, `CG_SpawnTracer` runs when
  `rand() < 2147483648.0 * cg_tracerchance` (`FLOAT_00326634`
  0x4f000000), a 0.4 chance at the default. VERIFIED. The whizby follows
  on any shooter (cod2-sound.md 6).
- The first-person body's own rounds never draw a tracer: its muzzle
  point is the view origin (`CG_CalcMuzzlePoint`'s first arm, `cg +
  0x28588`) and the spawn is skipped. VERIFIED (the two tests).
  `CG_SpawnTracer` has no other caller (two calls in the decompile, one
  per function). VERIFIED. A player sees other players' tracers only, and
  his own in a killcam or while following another, when 0xc00000 or the
  client number no longer match. INFERRED.
- `CG_CalcMuzzlePoint` returns 0 with no `DObj` for the shooter; with one,
  the tag's world position, else, for a client (number under 64), the
  entity's origin raised 11 under `eFlags` 0x8, 40 under 0x4, else 60.
  VERIFIED.
- `CG_SpawnTracer` (0x1d9886): a local entity with `trType` 2
  (`TR_LINEAR`), `trBase` the muzzle, `trDelta` the unit shot times
  `cg_tracerSpeed`, `trTime` `cg.time - (rand() % cg.frametime) / 2` (0
  when `cg.frametime` is 0), `endTime` `trTime + (int)(dist * 1000.0 /
  speed)` (`FLOAT_00326180` 1000.0), and the shot's length at `le +
  0x44`. VERIFIED. The pool is 128 entries (`CG_InitLocalEntities`
  0x1dfe94: 0x5e00 bytes of 0xbc); a full pool reuses the oldest
  (`CG_AllocLocalEntity` 0x1e0010). VERIFIED. No weapon file key reaches
  the tracer: `CG_SpawnTracer` takes only the two points. VERIFIED.
- `CG_AddLocalEntities` (0x1e00b4) draws an entity while `trTime <=
  cg.time < endTime` and frees it otherwise. VERIFIED.
- `CG_AddMovingTracer` (0x1dfef8): the tail is the trajectory at
  `cg.time`, the head `min(length - travelled, cg_tracerlength)` ahead of
  it along the shot. The streak leaves the muzzle at full length, ends at
  the hit and shrinks into it as the tail arrives. VERIFIED. Unlike CoD1
  (efx-grammar.md R8) there is no flesh branch and no length gate.
- `CG_DrawTracer` (0x1db3a4): the quad's side is `-(d . left) up + (d .
  up) left`, normalized (the view's axes at `cg + 0x285a0` and
  `+ 0x285ac`), each end offset `width` either way, so `cg_tracerwidth` is
  a half-width. With `cg_tracerScale` not 1, an end whose distance from
  the eye, less `cg_tracerScaleMinDist` when that is not 0, is above 0
  takes the width times `(d / range) * scale` held to `[1, scale]`
  (`scale` when the range is not above 0). VERIFIED. Vertex colour
  0xffffffff; s 1 at the head, 0 at the tail; t 1 on the positive side.
  VERIFIED. It goes out through `CL_AddPolyToScene` with
  `gfx/misc/tracer` (`cgs + 0x27bbc`, `CG_RegisterGraphics`). VERIFIED.
  That material (`iw_13.iwd`) is technique set `effect_add` on image
  `tracer`, one additive stage. VERIFIED (asset;
  `fx::cod2_tracer::tests::the_stock_material_loads`).

The dvars (`CG_Init`, the `Dvar_RegisterFloat` calls). VERIFIED.

| dvar | default |
|---|---|
| `cg_tracerchance` | 0.4 |
| `cg_tracerwidth` | 0.8 |
| `cg_tracerSpeed` | 4500 |
| `cg_tracerlength` | 160 |
| `cg_tracerScale` | 50 (minimum 1) |
| `cg_tracerScaleMinDist` | 5000 |
| `cg_tracerScaleDistRange` | 25000 |

vcod: `play::cod2_fx` rolls the chance and the back-date per hit event and
starts the tracer at the shooter's posed world `tag_flash` from the last
drawn frame (`Cod2Players::muzzles`); at the tagless stance height for a
shooter in the snapshot that vcod did not pose; and not at all for one not
in the snapshot. `fx::cod2_tracer` flies and draws it. The rate, the skip
and the geometry are gated in `bullet_hits_spawn_tracers_at_retails_rate`
and the `fx::cod2_tracer` tests. Pending: a by-eye check of the tracer on
screen.

### 6.8 Muzzle flashes and brass

`CG_RegisterWeapon` (0x1d85cc) registers four of the weapon file's
effects: `viewFlashEffect` (def +0x90) into `cg_weapons +0xc0`,
`worldFlashEffect` (+0x94) into `+0xc4`, `shellEjectEffect` (+0x110)
into `+0x15c`, and `lastShotEjectEffect` (+0x114) into `+0x160`.
VERIFIED (the stores). An empty `lastShotEjectEffect` leaves `+0x160`
the shell eject's handle. INFERRED (the branch). The stock files: every
gun names both flashes; every gun but the panzerfaust, the panzerschreck,
the webley and the mounted guns names a shell eject; none names a
last-shot eject; `boltAction` is 1 on the kar98k, the enfield, the mosin
nagant (their scoped ones too), the springfield and the shotgun.
VERIFIED (the `weapons/mp` files).

`CG_FireWeapon` (0x1d823c):

- It returns at once for `es.weapon` 0, else sets the entity's flash
  flag (`cent + 0x1e2`). INFERRED (the test, then the store).
- On a turret (`eType` 9) it plays the flash itself through
  `FX_PlayEntityEffect`, bolted to the turret's bone
  `_s_barrelTags[n]`. VERIFIED (the call). The effect is the view flash
  (`+0xc0`) when `cg + 0x25bc0` is 0, the snapshot's playerstate has
  `eFlags & 0x300` and its `viewlocked_entNum` (ps 0x594) is the
  turret, else the world flash (`+0xc4`). INFERRED (the tests).
- `CG_EntityEvent` (0x1e4650) passes barrel 0 for 158..160 and 175,
  calls it twice with barrels 0 and 1 for 176, 2 and 3 for 177.
  VERIFIED (the calls' immediates).
- After the fire sound it calls `CG_EjectWeaponBrass(es, event)` unless
  the weapon's `boltAction` (def +0x328) is set. INFERRED (the order and
  the test).

`CG_AddPlayerWeapon` (0x1d9c8e) plays a player's flag:

- `CG_AddViewWeapon` (0x1da2f8) calls it with the predicted playerstate,
  the predicted player entity (`cg + 0x2826c`) and its draw flag;
  `CG_Player` (0x1e6834) with no playerstate and 1. VERIFIED (the
  calls). `CG_Player` skips it, and so holds the flag, under `eFlags &
  0x20` (no draw) or `0x20000` (dead), for the first-person body's own
  entity and for a body with no DObj; it culls nothing before the call.
  INFERRED (the tests).
- For a weapon above 0 and the entity's `eFlags & 0x300` clear, a set
  flag is cleared and, when the call draws, played: with a playerstate
  the view flash on bone `_s_barrelTags[0]` (`tag_flash`) of the view
  weapon's DObj, `0x400 + BG_GetViewmodelWeaponIndex(ps)`; else the
  world flash on the entity's own `tag_flash`. INFERRED (the tests and
  the two arms). The world call leaves the first-person body's own
  entity (`pm_flags & 0xc00000`, number `clientNum`) set, so its flag
  never plays. INFERRED (the test).
- `CG_AddViewWeapon`'s draw flag is 1 when `cg + 0xc` is 0, `cg_drawGun`
  is on and `CG_GetWeapReticleZoom` (0x1cdb06) returns 0. INFERRED (the
  tests). The flag is cleared before the draw test, so a shot fired under
  a scope's overlay plays no view flash, then or later. INFERRED (the
  order).
- `cg_drawGun` is `Dvar_RegisterBool("cg_drawGun", 1, 0x1080)` in
  `CG_Init` (0x145214). VERIFIED. Bit 0x80 is the cheat bit: the dvar
  set path (0x5313e) prints `%s is cheat protected.` for a console set of
  a dvar whose flags' low byte is negative while `sv_cheats` is 0.
  VERIFIED (the test and the string). The archive bit 0x1 is clear, so
  unlike CoD1's (flags 0x201) it is not saved. INFERRED.
- A draw flag of 0 skips only `CG_AddPlayerWeapon`'s
  `R_AddRefEntityToScene` of the gun and the flash's play: the placed
  gun's origin and axis, the `tag_brass` read into
  `_ejectBrassCasingOrigin` and the breath sounds run either way.
  INFERRED (the tests round the two blocks). The brass still plays on
  the hidden gun: `FX_GetBoneOrientation` runs `CG_DObjCalcBoneGeneric`
  (VERIFIED, the call) on the bolt's DObj itself, drawn or not. INFERRED.
- `CG_DrawCrosshair` reads it too (cod2-hud.md 10).
- Events run before the frame adds its entities and the view weapon, so
  a flash plays in its event's frame. INFERRED (the frame's order).

`CG_EjectWeaponBrass` (0x1d80ba):

- Nothing unless `cg_brass` is on, `es.eType` is under 10 (no temp
  event) and `es.weapon` is not 0. INFERRED (the tests). `cg_brass`
  defaults to 1. VERIFIED (`CG_Init`'s `Dvar_RegisterBool`).
- Event 160 takes `+0x160` when it is set, any other `+0x15c`. INFERRED
  (the test).
- The bolt is the view weapon's DObj `0x400 + es.weapon` when the
  playerstate's `pm_flags & 0xc00000` is set and `es.number` is its
  `clientNum`, else `es.number`. INFERRED (the tests). The bone is
  `scr_const + 0x94`, `tag_brass` (0x1ac8f14, `GScr_LoadConsts`
  0x1cb694). VERIFIED (the operand and the string). A missing bone plays
  nothing. INFERRED (the test).
- `CG_EntityEvent`'s 162 (`EV_EJECT_BRASS`) calls it with 162. VERIFIED
  (the call). A bolt action's brass therefore leaves at
  `rechamberBoltTime` (cod2-combat.md 1.2). INFERRED.

`FX_PlayEntityEffect` (0x60d14) is `FxScheduler::PlayEffect` (0x61322)
with a bolt (entity, bone). VERIFIED (the call). With one, the spawn's
origin and axis are `FX_GetBoneOrientation`'s (0x59230), the origin
passed in feeds only the `spawnRange` and `frustumCull` tests, and a
negative bone or a failed orientation plays nothing. INFERRED (the
arms). Every spawn record keeps the bolt. VERIFIED (the stores at +0xc
and +0x10). `CG_AddPlayerWeapon` stores the placed view weapon's origin
and axis at `cg + 0x2c0bc` and `+0x2c0c8`. VERIFIED (the stores). That
`CG_GetDObjOrientation` reads them for the `0x400` DObjs is INFERRED.

vcod: `play::cod2_fx::Cod2Fx::weapon_fx` and `brass` raise both per
event, played on this frame's poses: a body's `tag_flash` and
`tag_brass` (`cod2_players`, `BodyTags`) and a turret's `tag_flash`
(`cod2_scene`) as `Play::OnTag`; the view weapon's as `Play::View`,
which `cod2_client::Cod2Client::view` spawns once the gun is placed
(the gun's offset and angles, the view's angles with roll) on
`cod2_viewmodel::ViewTags`. Each frame hands the effects every pose as a
tag bolt (`FxSystem::set_tag_bolts`), so a `relative` element rides the
gun. The flags are held as retail holds them: a body's in
`Cod2Fx::held_flashes`, played at the next frame `cod2_scene` poses the
body armed and off a gun, the playerstate's (`Play::ViewFlash`) in
`Cod2Client`, cleared at the next placed gun and played there under
`cg_drawGun` with no overlay; each plays the weapon held at that draw.
`cg_drawGun 0` hides the gun (`Cod2Client::gun_drawn`) and its flash;
its brass plays. vcod's own: the view brass plays only off the weapon
placed; the view's tags are this frame's pose; barrels 1..3
(`tag_flash_11` .. `tag_flash_22`, no stock weapon) are not used. Gates:
`play::cod2_fx::tests::shots_flash_and_eject_brass_on_their_tags`,
`play::cod2_fx::tests::a_shots_flash_waits_for_the_bodys_next_draw`,
`cod2_players::tests::an_armed_body_poses_its_flash_and_brass_tags`,
`cod2_scene`'s gun test,
`cod2_viewmodel::tests::the_view_weapon_poses_its_tags_and_sounds_its_notetracks`
and, live on vcod-server with bots,
`play::cod2_soak::cod2_live_shots_flash_and_eject_on_their_guns` (its
last phase under `cg_drawGun 0`). Pending: a by-eye check of the flashes
and the brass on screen.

## 7. What vcod draws

`fx::cod2::to_emitter` fills the CoD1 emitter fields from the primitive and
keeps the primitive on it. `fx::cod2::Live` holds a particle's draws (2.3,
4.2) and evaluates colour, alpha, size, size2, length, rotation and
position as 2.3 and 4.3 give them, and the atlas cell as 4.4.

vcod's own approximations:

- A bounced physics particle leaves the integral and falls under twice its
  gravity.
- `wind`, `org2*`, `frustumCull` and the far-plane flags are not read.
- `useRandomLength` draws a weight though retail draws none.
- `CameraShake` is a debug log, as retail's empty `FxHelper::CameraShake`
  leaves it.

An effect's material draws through its set's "emissive feathered"
technique, the PC renderer's default. The 33 `effect_zfeather*` materials
(among them `gfx_mist`, the street dust of `fx/dust/dust_wind_eldaba.efx`
on mp_carentan) fade their alpha over the depth gap to the world behind
them instead of cutting it on a hard line (cod2-materials.md 9.5, VERIFIED
there).
