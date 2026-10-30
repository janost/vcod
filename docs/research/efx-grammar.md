# `.efx` particle-script grammar census

A census of every `.efx` file in the stock 1.5 `main/pak*.pk3` set, then the 1.1 engine's reading of the keys (the R sections). The assets match 1.1's (`cod11-events-and-fx.md` section 8, "the events doc" below). vcod's parser is `crates/client/src/fx/efx.rs`, its simulation `crates/client/src/fx/sim.rs`.

Claims are VERIFIED (occurrence counts over the stock paks, a named file, or a read of the 1.1 binaries) unless labelled otherwise. Addresses without a module prefix are `CoDMP.exe` 1.1 VAs.

## 0. Corpus

```
for p in main/pak{0,1,2,3,4,5,6,8,9,a,b}.pk3; do unzip -o -q "$p" 'fx/*' -d "$S"; done
find "$S/fx" -name '*.efx' | wc -l   # 460
```

- 460 `.efx` files: 459 in `pak5.pk3`, 1 in `pak9.pk3`. No other `main/pak*.pk3` (0,1,2,3,4,6,8,a,b) has `fx/` entries, and the five `localized_english_pak{0,1,2,3,5}.pk3` hold none.
- `fx/iw_impacts.csv` (section 6) comes from the same `pak5.pk3`.
- No case-insensitive duplicate paths, no zero-byte file, and all 460 parse under the grammar below.
- Every file uses CRLF line endings and tab indentation (no bare `\n` line, no space-indented base level). Strip the `\r` before matching: a `grep -E '...$'` matches nothing because `$` does not cross it.

## 1. Block type keywords

A block is a bare identifier at column 0 followed by a line that is just `{`. 10 block types, 1261 blocks over 460 files (2.74 a file):

| Block type | Count |
|---|---|
| `Particle` | 712 |
| `Emitter` | 162 |
| `FxRunner` | 116 |
| `Light` | 71 |
| `Tail` | 66 |
| `OrientedParticle` | 49 |
| `Decal` | 34 |
| `Line` | 31 |
| `Sound` | 12 |
| `Cylinder` | 8 |

A file is a flat sequence of blocks, no nesting, and every block plays when the file is triggered; nothing in a file selects among them. `fx/impacts/default_hit.efx` is three `Particle` blocks and a `Decal`; `fx/explosions/grenade2.efx` is `Particle, Emitter, Particle, Light, Decal, Particle, FxRunner`.

`FxRunner` and `Sound` are pure triggers with no `life`, geometry or render key. `FxRunner` bodies hold only `name`, `count`, `cullrange`, `delay`, `height`, `origin`, `playfx`, `radius`, `spawnFlags`; `FxRunner` chains to another `.efx` through `playfx` (section 3). `Sound` bodies hold only `sounds`, a sound alias list.

## 2. Keys per block type, with arity

A census of keys has to:

- skip list blocks (`shaders [ ... ]`, `models`, `sounds`, `emitfx`, `playfx`, `impactfx`, `deathfx`) by `[`/`]` depth. Their entries are paths or alias names, and counting them reports bogus keys (`gfx`, `fx`, `xmodel`, `textures`, `KABOOM`, `glass`, `explo`, `shell`);
- track the enclosing curve sub-block, so `end` under `rgb` and `end` under `size` stay apart;
- split keys on whitespace as whole tokens. A letters-only regex merges `size2` into `size` and `origin2` into `origin`. Split right, `size` is 1089 (all sub-block openers; `grep -c '^\t+size[ \t]*\r?$'` agrees), `size2` 168 and `origin2` 31.

Two key line shapes:

- Value key: `<key>\t...\t<value>`, a scalar, vector or token list on the same line.
- Sub-block opener: `<key>` alone, the next non-blank line `{`. Five exist: `rgb`, `alpha`, `size`, `size2`, `length`, each holding some of `start`, `end`, `flags`, `parm` (2b). `size2` has the shape of `size`, appears only beside `nonUniformScale 1`, and drives the second scale axis (R4). `fx/atmosphere/rainsplash.efx`'s `Particle` has `size { end 5 7 }`, `size2 { end 12 7 }` and `nonUniformScale 1`.

### 2a. Top-level value keys (23), arity and units

The grammar has 38 key identifiers: these 23, the 5 sub-block openers, the 7 list keys below, and `start`/`end`/`parm` (curve-only, 2b).

Arity is the count of numeric tokens: 1 fixed, 2 min/max (uniform per spawn), 3 a vector, 6 min vec3 / max vec3 (per component). Units are R-backed where marked; the rest are section 5's readings.

| Key | Arities seen (count) | Example | Block types | Unit |
|---|---|---|---|---|
| `life` | 1 (786), 2 (344) | `life 1e+004` | all but `FxRunner`, `Sound` | ms |
| `delay` | 1 (110), 2 (119) | `delay 80 0` | `Particle`, `OrientedParticle`, `Tail`, `Line`, `Cylinder`, `FxRunner` | ms |
| `count` | 1 (416), 2 (288) | `count 2 5` | `Particle`, `OrientedParticle`, `Tail`, `Line`, `Cylinder`, `FxRunner` (never `Emitter`) | instances |
| `cullrange` | 1 (206) | `cullrange 1200` | `Particle`, `Emitter`, `Decal`, `Tail`, `FxRunner` | game units |
| `velocity` | 3 (123), 6 (721) | `velocity 1.19e+004 0 0 1.18e+004 0 0` | `Particle`, `Emitter`, `OrientedParticle`, `Tail` | units/s |
| `acceleration` | 3 (37), 6 (130) | `acceleration -100 21 32 -120 -42 0` | `Particle`, `Emitter`, `OrientedParticle`, `Tail` | units/s² |
| `gravity` | 1 (169), 2 (133) | `gravity -300` | `Particle`, `Emitter`, `OrientedParticle`, `Tail` | units/s², added to accel z (R9) |
| `origin` | 3 (81), 6 (546) | `origin 5 12 0 75 -24 0` | `Particle`, `Emitter`, `OrientedParticle`, `Tail`, `Cylinder`, `FxRunner`, `Light` | game units, offset from the play point |
| `origin2` | 3 (8), 6 (23) | `origin2 245 456 433 345 -456 -433` | `Line` only | game units, second endpoint |
| `radius` | 1 (137), 2 (192) | `radius 1 14` | `Emitter`, `OrientedParticle`, `Tail`, `Cylinder`, `FxRunner` | game units |
| `height` | 1 (63), 2 (90) | `height 50 32` | `Particle`, `OrientedParticle`, `Tail`, `Line`, `Cylinder`, `FxRunner` | game units |
| `rotation` | 1 (5), 2 (665) | `rotation 180 -360` | `Particle`, `Decal`, `OrientedParticle` | degrees, initial roll |
| `rotationDelta` | 1 (27), 2 (487) | `rotationDelta 1 -2` | `Particle`, `OrientedParticle` | degrees/s |
| `angle` | 3 (7), 6 (39) | `angle 122 0 0 2 0 0` | `Emitter` only | degrees, initial model angles |
| `angleDelta` | 6 (65) | `angleDelta 12 0 0 11 0 0` | `Emitter` only | INFERRED: degrees per 100 ms (R9) |
| `bounce` | 1 (21), 2 (3) | `bounce 0.5 0.7` | `Emitter`, `Tail` | elasticity (R9) |
| `density` | 1 (126) | `density 4` | `Emitter` only | game units between `emitfx` plays (R10) |
| `variance` | 1 (28), 2 (41) | `variance 1 5` | `Emitter` only | game units, jitter on that spacing (R10) |
| `wind` | 1 (10), 2 (1) | `wind 100` | `Particle` only | unitless; pairs with `affectedByWind` |
| `nonUniformScale` | 1 (81, always `1`) | `nonUniformScale 1` | `Particle`, `Emitter` | boolean (R4) |
| `name` | free text (127 to 236 words) | `name Copy of Unnamed Particle 0` | all render blocks | editor label |
| `flags` | token list | `flags usePhysics impactKills useAlpha` | all render blocks | section 3 |
| `spawnFlags` | token list | `spawnFlags orgOnSphere axisFromSphere` | `Particle`, `Emitter`, `OrientedParticle`, `Tail`, `Line`, `Cylinder`, `Decal` | section 3 |

List keys, whose value is a `[ ... ]` block of bare strings, one per line:

| Key | Openers | Contents | Block types |
|---|---|---|---|
| `shaders` | 898 | `gfx/...` material paths | `Particle`, `Decal`, `OrientedParticle`, `Tail`, `Line`, `Cylinder` |
| `models` | 69 | `xmodel/...` paths | `Emitter` |
| `emitfx` | 105 | `.efx` paths, played along the emitter's path | `Emitter` |
| `playfx` | 116 | `.efx` paths, one-shot | `FxRunner` |
| `impactfx` | 19 | `.efx` paths, played on a physics hit | `Emitter`, `Line`, `Tail` |
| `deathfx` | 16 | `.efx` paths, played when the element's life runs out | `Particle`, `Line`, `Tail`, `OrientedParticle` |
| `sounds` | 12 | alias names, not paths (`KABOOM`) | `Sound` |

Three chaining lists pair with a `flags` token: `emitfx` with `emitFx`, `impactfx` with `impactFx`, `deathfx` with `deathFx`. The list parsers set the flag themselves (R9, R10), and no stock block carries one without the other. These counts are openers; R10 counts entries.

### 2b. Curve sub-block keys (`rgb`, `alpha`, `size`, `size2`, `length`)

Each sub-block holds up to four keys: `start` (value at spawn), `end` (value at the end of `life`), `flags` (interpolation mode) and `parm` (wave parameters). Counts are `(start/end)`: `rgb`'s `3 (242/264)` means arity-3 `start` 242 times and arity-3 `end` 264.

| Sub-block | `start`/`end` arity | Example | Meaning |
|---|---|---|---|
| `rgb` | 3 (242/264) or 6 (90/133) | `start 1 0.9843 0.9412 0.7529 0.7529 0.7608` | RGB triple, or min/max triples |
| `alpha` | 1 (156/735) or 2 (161/65) | `start 0.4 0.8` | alpha, or min/max |
| `size` | 1 (399/397) or 2 (363/557) | `end 282 374` | size, or min/max (R2: a per-particle range) |
| `size2` | 1 (85/96) or 2 (26/63) | `start 55 5` | as `size`, second axis |
| `length` | 1 (120/182) or 2 (31/23) | `start 4 14` | tail length, or min/max |

A curve's `flags` is a separate vocabulary from the top-level `flags`/`spawnFlags`: `linear` (2985 of 3164 curve-flag tokens), `random` (145), `clamp` (22), `wave` (14), `nonlinear` (7). `parm` (32 in all, arity 1 or 2, `parm 0.1 80`) appears only beside `wave`. INFERRED from the name: it holds the wave's frequency and amplitude.

## 3. `flags` / `spawnFlags` tokens

One tab of indent is a block's own `flags`/`spawnFlags`; two tabs a curve's `flags` (2b). 4143 lines at any depth, matching the key census (3697 `flags` + 446 `spawnFlags`).

vcod's parser keeps the tokens as strings (`efx::Emitter::flags`, `spawn_flags`); `fx::sim` reads the ones the R sections decode.

Top-level `flags`, 11 tokens:

| Token | Count | Meaning |
|---|---|---|
| `useAlpha` | 428 | the `alpha` curve goes to the vertex alpha byte, not into the rgb (R4) |
| `emitFx` | 105 | the `Emitter` plays its `emitfx` list every `density` units along its path (R10) |
| `relative` | 96 | the element rides the tag its effect was played on (R10) |
| `usePhysics` | 88 | traced against the world and bounced (R9) |
| `impactKills` | 81 | dies on its first hit (R9) |
| `useModel` | 69 | the `Emitter` draws its `models` (xmodel debris) (R9) |
| `impactFx` | 19 | plays its `impactfx` list on a hit (R9) |
| `depthHack` | 17 | INFERRED from the name: drawn depth-biased toward the camera |
| `deathFx` | 16 | plays its `deathfx` list where its `life` runs out (R10) |
| `setShaderTime` | 11 | INFERRED from the name: starts the shader's clock at the spawn |
| `expensivePhysics` | 4 | parsed and ignored (R9) |

`spawnFlags`, 10 tokens, a separate namespace. Bits and the engine's reading are in R6.

| Token | Count | Meaning |
|---|---|---|
| `orgOnCylinder` | 214 | spawn point on a cylinder from `radius` and `height` (R6) |
| `axisFromSphere` | 207 | INFERRED from the name: launch direction radial from the sphere or cylinder point |
| `evenDistribution` | 91 | read only on the `Emitter` spawn path (R6). CoD2 spreads delays by it (`fx::sim`) |
| `rgbComponentInterpolation` | 72 | INFERRED from the name: R, G and B interpolate independently |
| `orgOnSphere` | 53 | spawn point on a sphere from `radius` (R6) |
| `absoluteAccel` | 42 | `acceleration` in world space, not the emitter frame (R6) |
| `cheapOrgCalc` | 30 | skips rotating the sampled `origin` into the emitter frame (R6) |
| `absoluteVel` | 22 | `velocity` in world space (R6) |
| `randrotaroundfwd` | 10 | INFERRED from the name: random initial roll about the forward axis |
| `affectedByWind` | 1 | INFERRED from the name: moved by `wind` |

## 4. Defaults: what a missing key means

R7 reads the template initializer; this table states the result per key, with the corpus case that shows it.

| Key / block missing | Default | Evidence |
|---|---|---|
| `alpha` sub-block | 1.0 | VERIFIED (R7). Of the 900 blocks that can carry it (`Particle`, `Decal`, `Cylinder`, `Line`, `OrientedParticle`, `Tail`), 830 set it. `fx/impacts/small_rock.efx`'s 2nd `Particle` has `rgb { flags linear }` with no `start`/`end`, so a curve sub-block need not hold a value. |
| `rgb` sub-block | white (1,1,1) | VERIFIED (R7). 794 of 971 blocks that can carry it (the same set plus `Light`) set it. |
| `start` absent, `end` present | the curve's default: 0 for `size`/`size2`/`length`, 1.0 for `alpha`, white for `rgb` | VERIFIED (R7). `fx/error.efx`'s `size { end 133 parm 4 flags wave }` has no `start`. |
| `end` absent, `start` present | the same per-curve default. A `linear` curve ramps to it: `size { start 210 flags linear }` shrinks to 0 | VERIFIED (R3, R7). 48 `size` curves pair `linear` with no `end`. `fx/impacts/small_glass.efx`'s `Decal` `size { start 3 4 }` has no flags and is constant. |
| curve `flags` absent, `start` and `end` present | holds `start` for the whole life; `end` is ignored | VERIFIED (R3): the fraction stays 1.0 unless `linear` is set. 101 of 1089 `size` blocks carry no `flags`, all with `start`. |
| `velocity` | (0,0,0) | VERIFIED (R7). `default_hit.efx`'s first `Particle` has none; no `Decal` carries one. |
| `gravity` | 0 | VERIFIED (R7). |
| `acceleration` | 0 | VERIFIED (R7). 615 of 712 `Particle` blocks omit it. |
| `rotation` | 0 | VERIFIED (R7). 2 of 34 `Decal` blocks omit it. |
| `rotationDelta` | 0 | VERIFIED (R7). |
| `origin` | (0,0,0), the play point | VERIFIED (R7). Every working-set impact and explosion effect (section 6) omits it. |
| `count` | 1 | VERIFIED (R7). No `Decal` carries the key. |
| `life` | 1 ms | VERIFIED (R7). |
| `nonUniformScale` | 0; `size2` unused | VERIFIED (R4). All 81 occurrences are `1`, and `size2` never appears without it. |
| `flags`/`spawnFlags` | empty | VERIFIED (R7, zeroed template). |
| `cullrange` | 0, never culled | VERIFIED (R7): the runner's `if (cullrange == 0)` branch (0x493a1f) skips the distance check. 206 of 1090 blocks that can carry it set it. |
| `density`/`variance` | 0: an `emitfx` emitter plays on every 12 ms step of its path | VERIFIED (R7, R10). They space `emitfx` plays only; an `Emitter` spawns `count` instances like any element. |
| `radius`/`height` under `orgOnSphere`/`orgOnCylinder` | 1.0, a unit sphere or cylinder | VERIFIED (R7). Not seen in the corpus; every such block sets `radius`. |
| `radius`/`height` without either flag | ignored | VERIFIED (R6). `fx/tagged/tracers.efx`'s `radius 45 55` / `height 100 10` do nothing. |
| `delay` | 0 | VERIFIED (R7). |
| `name` | nothing | INFERRED: an editor label, on about half the `Particle`/`FxRunner`/`Tail` blocks with no visible effect. |

## 5. Units

- `life`, `delay`: ms. INFERRED from scale: `fx/explosions/shock.efx`'s `Cylinder` ring `life 211`, `default_hit.efx`'s `Decal` `life 8000` (a bullet hole fading over 8 s), `grenade2.efx`'s scorch `Decal` `life 4e+004`, `tracers.efx`'s `Tail` `life 3700 4300` and `delay 300 100`. Seconds or ticks are off by orders of magnitude. R8's `ftol(life)` for a decal's mark lifetime agrees.
- `velocity`, `acceleration`, `gravity`: units/s and units/s². VERIFIED (R9): the integrator scales by `frametime * 0.001`. `tracers.efx`'s 11900/11800 is about 990 ft/s at an inch a unit.
- `rotation`, `angle`: degrees; `rotationDelta`: degrees/s. INFERRED from the values: `[-360, 360]` clusters; radians would make `rotation 360 -360` 22,900 degrees. `angleDelta`: INFERRED per 100 ms (R9).
- `radius`, `height`, `origin`, `origin2`, `size`, `size2`, `length`, `cullrange`: game units. INFERRED: the only reading consistent with `origin` in the same blocks.
- `rgb`, `alpha`: `[0, 1]`; every value in the corpus falls there.
- `bounce`: elasticity (R9); values cluster in `[0.5, 2]`.
- `density`, `variance`: game units (R10).
- `wind`, `nonUniformScale`: unitless.

## 6. Working set: effects the event mapping references

vcod needs the `.efx` files the wire's event table reaches (events doc section 5), not all 460.

### 6a. `fx/iw_impacts.csv` format

From `pak5.pk3`, its header comments and its rows:

- 3 columns, no header row: `impact_type,surface,effect_path`. Comment lines start with `#` and are quoted (`"# text",,`); no data row holds a `#`.
- `impact_type` is one of 9: `bullet_small_normal`, `bullet_small_reflect`, `bullet_large_normal`, `bullet_large_reflect`, `grenade_bounce`, `grenade_explode`, `rocket_explode`, `molotov_explode_normal`, `molotov_explode_reflect`.
- `surface` is one of the 23 surface names of events doc section 4 (`default`, `bark`, ..., `asphalt`), lowercase, matching `surfType` by name.
- `effect_path` may be blank, with a trailing comma (`grenade_bounce,gravel,`) or without (`bullet_small_reflect,foliage`). Blank plays nothing.
- A (type, surface) pair can repeat within the file: `bullet_small_normal,bark` appears twice, both `fx/impacts/default_hit.efx`.
- Stock ships one CSV, so the events doc's alphabetical override across files never triggers on retail assets.
- The 50 `molotov_explode_normal`/`_reflect` rows (25 each, the duplicate `bark`/`brick` rows included) are dead in MP: `EV_MOLOTOV_EXPLODE`/`_NOMARKS` have no case in the MP `CG_EntityEvent` jump table (events doc section 1). vcod skips them.

### 6b. The working-set `.efx` list

32 files, all in the corpus.

Bullet impacts (`bullet_small_normal`/`bullet_large_normal`; every `_reflect` column is blank in stock):

```
fx/impacts/default_hit.efx        fx/impacts/small_grass.efx
fx/impacts/small_brick.efx        fx/impacts/small_gravel.efx
fx/impacts/large_brick.efx        fx/impacts/large_gravel.efx
fx/impacts/small_concrete.efx     fx/impacts/snowhit_small.efx
fx/impacts/large_plaster.efx      fx/impacts/snowhit_large.efx
fx/impacts/small_gravel2.efx      fx/impacts/metalhit_small.efx
fx/impacts/large_gravel2.efx      fx/impacts/metalhit_large.efx
fx/impacts/flesh_hit.efx          fx/impacts/small_rock.efx
fx/impacts/flesh_hit_noblood.efx  fx/impacts/large_rock.efx
fx/impacts/small_foliage.efx      fx/impacts/waterhit_small.efx
fx/impacts/small_glass.efx        fx/impacts/waterhit_large.efx
fx/impacts/large_glass.efx        fx/impacts/woodhit_small.efx
```

`flesh_hit_noblood.efx` is the client's one hardcoded path, used for `surfType == 7` when the blood cvar is off (events doc section 4).

Explosions (`grenade_explode`, `rocket_explode`; `grenade_bounce` is blank for every surface):

```
fx/explosions/grenade1.efx
fx/explosions/grenade2.efx
fx/explosions/grenade3.efx
fx/explosions/grenade_water.efx
fx/impacts/snow_mortarlite.efx
fx/impacts/newimps/v_blast1.efx
fx/impacts/newimps/v_blast1dirt.efx
fx/impacts/newimps/v_blast1wood.efx
```

Outside the working set:

- every `molotov_explode_*` row (6a);
- muzzle flashes (`viewFlashEffect`/`worldFlashEffect`), named per weapon file, not in the CSV; they use the same 10 block types;
- script and `ET_FX` effects from configstrings 780 to 843, set per map or mod at run time.

Seven files were checked line by line against the census: `fx/error.efx` (230 bytes, minimal `Emitter`), `fx/impacts/default_hit.efx`, `fx/impacts/n_waterimpact.efx` (`Tail`, `Line`, `Particle`, `FxRunner`), `fx/explosions/grenade2.efx`, `fx/explosions/shock.efx` (`Cylinder`), `fx/atmosphere/rainrings.efx` (`OrientedParticle`), `fx/explosions/mutha2.efx` (`FxRunner` + `Sound`). They span 9 of the 10 block types; no discrepancy.

---

# Renderer semantics (from CoDMP.exe / cgame_mp_x86.dll)

The binary's arithmetic for the keys that decide what is drawn and where.

- The `.efx` parser and the particle system live in `CoDMP.exe`, not `cgame_mp_x86.dll`. The cgame only registers effect paths; every `.efx` keyword string (`cheapOrgCalc`, `orgOnSphere`, `nonUniformScale`, the block names) is in the engine binary.
- Bullet tracers are not an `.efx` effect (R8).

## R1. Block types are an enum, and it has 13 members

0x4931e0 maps the block keyword to an integer at template +0x48:

| id | keyword | id | keyword |
|---|---|---|---|
| 1 | `particle` | 8 | `orientedparticle` |
| 2 | `line` | 9 | `electricity` |
| 3 | `tail` | 0xa | `fxrunner` |
| 4 | `cylinder` | 0xb | `light` |
| 5 | `emitter` | 0xc | (unused, string @ 0x559dc0) |
| 6 | `sound` | 0xd | (unused, string @ 0x559db8) |
| 7 | `decal` | | |

A template is 0x25c bytes. The census's 10 keywords are a subset; `electricity` parses but its renderer is a stub (`"FXPORT RT_ELECTRICITY TDB\n"` at 0x5128a0).

## R2. `size` is really `width`, `length` is really `height`

The curve-group dispatcher in the primitive key parser accepts these aliases:

| group | aliases | template offsets (start.a/start.b, end.a/end.b, parm.a/parm.b) | parser |
|---|---|---|---|
| `size` | `size`, `width` | 0x1fc/0x200, 0x204/0x208, 0x20c/0x210 | FUN_0049d380 @ 0x49d380 |
| `size2` | `size2`, `width2` | 0x214/0x218, 0x21c/0x220, 0x224/0x228 | FUN_0049d530 @ 0x49d530 |
| `length` | `length`, `height` | 0x22c/0x230, 0x234/0x238, 0x23c/0x240 | FUN_0049d6e0 @ 0x49d6e0 |

Each of `start`/`end`/`parm` is read with `sscanf("%f %f")`, and one value fills both slots. So the 2-arity form is a min/max range sampled per particle (sampler FUN_00431440 @ 0x431440), not a two-axis value.

## R3. The size/length curve evaluators, and what "no flags" means

FUN_0049b3e0 @ 0x49b3e0 parses a curve's flag words into a nibble: `linear`=1, `random`=2, `nonlinear`=4, `wave`=8, `clamp`=0xc. Each group shifts it into its slot of the template flag word at +0xb8 (alpha `<<0`, rotation `<<4`, size `<<8`, length `<<0xc`, size2 `<<0x10`).

Per frame FUN_0048e3a0 @ 0x48e3a0 (size), FUN_0048f400 @ 0x48f400 (length) and FUN_0048e4f0 @ 0x48e4f0 (size2) run one shape:

```
f = 1.0
if (linear)  f = 1 - (now - startTime) / (endTime - startTime)
switch (mode bits) { nonlinear / wave / clamp -> replace or average f }
if (random)  f *= rand01
value = f*start + (1 - f)*end
```

Without `linear` or a mode bit, `f` stays 1.0 and the curve holds `start` for the whole life: an unflagged curve is constant, not an implicit ramp.

The results land on the instance: size at +0xb8, size2 at +0xa0, length (tail only) at +0x154.

The corpus carries `length` on non-tail blocks too: the `muzflash2a` `Particle` in `fx/muzzleflashes/heavy.efx` (`length 12 -> 275`; MP40, MP44, BAR), `german_mg.efx`, `thompson.efx` and `russian_mg.efx` (`42 -> 375` and `42 -> 275`). INFERRED from the decompilation: the sprite path (R4) reads only size and size2, so `length` is inert there. vcod drops `length` on sprites; drawn as a tail length it trails a 60-140 unit streak back through the shooter on every MP40 shot.

## R4. `size` is a half-extent; a sprite draws `2 * size` across

The instance embeds a `refEntity_t` at +0x3c (0x9c bytes, copied whole by the add-entity call at 0x4e9a00). Relative to that base: `reType` +0x00, `origin` +0x44, `oldorigin` +0x54, `shaderRGBA` +0x6c, current size2 +0x64, current size +0x7c, `rotation` +0x80.

The backend dispatches on `reType` at FUN_005128a0 @ 0x5128a0 (4 sprite, 0xc oriented sprite, 0xd tail/line, 0xf cylinder).

Sprite path, FUN_005101d0 @ 0x5101d0:

```
w = ent[0x7c]           // current size
h = ent[0x64]           // current size2
left = viewaxis[1] * w
up   = viewaxis[2] * h  // both rotated by ent[0x80] when it is non-zero
FUN_0050fe20(&ent[0x44], left, up, &ent[0x6c], 0, 0, 1.0, 1.0)
```

The quad emitter FUN_0050fe20 @ 0x50fe20 writes the corners:

```
v0 = origin + left + up
v1 = origin - left + up
v2 = origin - left - up
v3 = origin + left - up
```

So `size` is a half-extent and the sprite spans `2 * size`. `size2` is the vertical half-extent; with `nonUniformScale` off the particle constructor FUN_0049f360 @ 0x49f360 copies the `size` curve into the `size2` slots and the sprite is square.

The `Cylinder` primitive (FUN_00511350 @ 0x511350) uses the same two values as ring radii: `size2` at `origin`, `size` at `oldorigin`.

### `nonUniformScale` and `size2`, in detail

- VERIFIED, the key parser 0x49d8b0: `nonUniformScale` stores `atol(value) != 0` as a byte at template +0xc0.
- VERIFIED: the sprite spawn passes that byte as the constructor's fourth argument (`mov dl,[ebp+0xc0]` at 0x495888, pushed at 0x49589f, the call at 0x4958b0).
- VERIFIED, 0x49f360: with the byte 0, the instance's `size2` start and end (+0x100, +0x104) and parm (+0x108) take `size`'s (+0xf4, +0xf8, +0xfc), and the flag word's size nibble (bits 8..11) is copied into the `size2` nibble (bits 16..19). With it set they take the template's `size2` curve.
- VERIFIED, 0x48e4f0: each frame `size2` lands at instance +0xa0 (refEntity +0x64) as `f * start + (1 - f) * end` under its own nibble (`linear` 0x10000), the shape of `size` at 0x48e3a0.
- VERIFIED, 0x5101d0 (`reType` 4) and 0x5103c0 (`reType` 0xc): `size` (+0x7c) scales the view's left axis and `size2` (+0x64) its up axis, both turned by `rotation` (+0x80) when non-zero.
- VERIFIED, string search of the 1.1 paks: 81 blocks in 58 files carry `nonUniformScale`, every one a `Particle`, among them `fx/impacts/blast_flash.efx` and `blast_flash2.efx`, the `FxRunner` flash of every stock `grenade_explode` row.
- INFERRED, from the curves: `blast_flash2`'s four `groundflash1` sprites run `size` 10 to 200 and `size2` 122 to 2 over 100 ms, a flash spreading flat along the ground. Drawn square on `size` they reach 400 units tall and cover about 1.56 times the fade-weighted area.
- vcod: `efx::Emitter::size2` and `non_uniform_scale`. `fx::sim` samples `size2` only under the flag and draws a sprite `size` across the view and `size2` up it (`a_non_uniform_sprite_takes_size2_up_the_view`).

### A sprite's vertex colour: `rgb`, then `alpha`

- VERIFIED, the sprite update 0x48de90: after the move (0x48df20) it evaluates `size` (0x48e3a0), `size2` (0x48e4f0), `rgb` (0x48e640) and `alpha` (0x48e7d0), in that order.
- VERIFIED, 0x48e640: the colour is `f * A + (1 - f) * B` per channel, the start and end triples at instance +0x10c and +0x118, `f` stepped by the nibble at bits 4..7 (`linear` 0x10 is `1 - t`), written as bytes clamped to 0..255 through 0x48d680.
- VERIFIED, 0x48e7d0: the alpha is the same lerp under bits 0..3, clamped to 0..1 (0x48e8d8..0x48e904). With `useAlpha` (instance flag 0x8000000, tested at 0x48e92c) it is written to the alpha byte through `__ftol2` (0x538be0) of `alpha * 255` and the rgb bytes stay. Without it each rgb byte is multiplied by the alpha (0x48e94e..0x48e9a2) and the alpha byte set to 255 (0x48e9a8).
- INFERRED, with the blend: a `GL_ONE GL_ONE` sprite with `useAlpha` (54 stock elements, string search) never fades on its alpha curve, and a `blendfunc blend` sprite without it darkens instead of turning transparent.
- vcod: `fx::sim::vertex_colour` builds the colour this way, and `Renderer::set_fx_quads` gives a `GL_ONE GL_ONE` material vertex alpha 1, so its draw reads none. A sprite material's blend, texture alpha and fog: `cod11-shader-scripts.md` section 3 (`renderer::fx_draw`).

### An element with no shaders

- VERIFIED: the `shaders` handler (0x49c8b0) registers each name and pushes its handle onto the template's list (+0x68..+0x74); a block without the key leaves the list empty. The sprite spawn (case 1 of 0x494050, the constructor call 0x49f360 at 0x495169) takes its shader from 0x492780, which returns a random entry, or 0 for an empty list, and the constructor stores it as the refEntity's `customShader` (+0x68).
- VERIFIED: 0x4d5170 adds a sprite, tail, line or cylinder (`reType` 4..15 but 11) through its `customShader` with only an upper range check, and 0x4fdb30 zeroes the shader count before 0x4fd780 registers `<default>`, so handle 0 is `<default>`: such an element draws the default image.
- VERIFIED, census (`every_stock_element_without_shaders_is_zero_sized` in `fx/efx.rs`): 2 elements of the 1.1 paks (the bare `Particle {}` in `fx/explosions/boom_dirt.efx`, one in `fx/test/modeltest.efx`) and 62 of United Offensive's `uo/` paks have no `shaders`. The UO ones are mostly the `call_impactmark` and `call_shockwave` carriers of `fx/explosions/vehicles/*_complete.efx` and `lofx/`, which only fly to their `impactfx`, plus UO's own `boom_dirt`, `modeltest`, `artillery/pak88_sideorient`, `map_foy/nests/towersoundcall` and `map_trenches/stuka_hitground`. Every one has zero `size` and `length` (R7's default), so retail draws them with no area.
- INFERRED: United Offensive picks the same way (CoDUOMP.exe 0x4a7770, the same rand-or-0 pick off a template list).
- vcod: `fx::sim::push_quads` skips a particle with no shader, which draws nothing, as retail's zero-area `<default>` does. A third-party element with no shaders and a size shows the default image on retail and nothing on vcod.

## R5. A `Tail` is anchored at its head, and `length` is the full length

The tail update at 0x48f360 stashes the pre-move origin at instance +0x13c, moves the particle, then calls the geometry step FUN_0048f550 @ 0x48f550:

```
delta = prevOrigin(+0x13c) - origin(+4)
VectorNormalize(delta)                    // 0x42db90
oldorigin(+0x90) = origin + delta * length(+0x154)
```

The renderer spans that pair verbatim. FUN_00511250 @ 0x511250 normalizes the screen-space side vector and calls FUN_00511010 @ 0x511010, whose four verts are

```
v0 = A + w*side   (st 0,0)      v2 = B + w*side   (st 0,1)
v1 = A - w*side   (st 1,0)      v3 = B - w*side   (st 1,1)
```

with `w = ent[0x7c]` (the current `size`). So:

- the segment runs from the particle (the head) back along its travel, not centred on it;
- its extent is exactly `length`, not `2 * length`;
- its width is `2 * size`, as for a sprite.

## R6. Spawn distribution: `cheapOrgCalc`, `orgOnSphere`/`orgOnCylinder`

From the spawner FUN_00494050 @ 0x494050 (template `origin` ranges at +0xdc..+0xf0, `height` at +0x10c/+0x110, `radius` at +0x114/+0x118, `spawnFlags` at +0xbc):

- The sampled `origin` is rotated into the emitter's axis frame (`origin.x*axis0 + origin.y*axis1 + origin.z*axis2`).
- `cheapOrgCalc` (0x100) skips that rotation and uses the sample as world offsets.
- `orgOnSphere` (0x1) places the point on a sphere from `radius`; `orgOnCylinder` (0x4) from `radius` and `height`. Only these two flags read `radius`/`height`, so a `Tail` that sets them without either (`fx/tagged/tracers.efx`) ignores both.
- `absoluteVel` (0x400) and `absoluteAccel` (0x800) keep the sampled velocity and acceleration in world space instead of the emitter frame.
- `evenDistribution` (0x2000) is tested once, in the `Emitter`-kind (model or chained-fx) spawn path; it does not touch `Particle`/`Tail` geometry.
- Other bits: `org2fromTrace` 0x10, `traceImpactFx` 0x20, `org2isOffset` 0x40, `cheapOrg2Calc` 0x200, `axisFromSphere` 0x2, `randrotaroundfwd` 0x1000, `rgbComponentInterpolation` 0x4000, `affectedByWind` 0x10000, `lessAttenuation` 0x20000.

No per-effect or global size scale exists on these paths, and muzzle-flash world effects get no clamping; they are ordinary `Particle`/`Tail`/`Light` primitives.

## R7. Every "missing key" default, from the template initializer

The allocator FUN_00497640 @ 0x497640 does `memset(t, 0, 0x25c)`, then the default initializer FUN_0049a460 @ 0x49a460 (called at 0x49337a, before the key parser) writes these, all `1.0f`:

| template offsets | key | default |
|---|---|---|
| +0x54/+0x58 | `count` | 1 |
| +0x5c/+0x60 | `life` | 1 |
| +0x10c/+0x110 | `height` | 1 |
| +0x114/+0x118 | `radius` | 1 |
| +0x1ac..+0x1d8 | `rgb` start+end (2 x 3 x 2) | white |
| +0x1e4..+0x1f0 | `alpha` start+end | 1.0 |

VERIFIED: the `life` handler 0x49b5f0 stores its pair at +0x5c/+0x60, and the `bounce`/`intensity` handler 0x49b710 at +0x254/+0x258 (and ORs 0x2000000 into +0xb8). So the first pair is `life`, and an absent `life` is 1 ms, not 0; reading that pair as `bounce` is a past mistake. The `Decal` spawn passes `ftol(life)` as the mark's lifetime (0x496941..0x49696f, `cod11-events-and-fx.md` section 9). vcod: an absent `life` is 1 ms too (`fx::efx::Emitter::default`), so a particle lives until its age passes 1 ms and a decal's mark expires the first frame more than 1 ms later.

Everything else stays 0: `delay`, `origin`, `velocity`, `acceleration`, `gravity`, `rotation`, `rotationDelta`, `cullrange`, and the whole `size`/`size2`/`length` block (+0x1fc..+0x240).

So `start` and `end` share one default per curve, and a curve that sets only one interpolates against the other: `size { start 210 flags linear }` fades 210 -> 0, `alpha { start 0.5 flags linear }` ramps 0.5 -> 1.0.

`cullrange 0`, the default, never culls: the spawner's `if (cullrange == 0)` branch skips the distance check.

## R8. Bullet tracers are hardcoded, not an `.efx`

`fx/tagged/tracers.efx` is the anti-aircraft gun tracer (velocity about 11 900 u/s, 3.7 to 4.3 s life, a `Tail` 120 units long and 10 to 20 wide with `gfx/effects/antiaircraft_tracer`). A rifle bullet does not draw it.

`CG_Tracer` @ `cgame_mp_x86.dll` 0x30039590, called from `CG_BulletHitWall` (173/174) and `CG_BulletHitFlesh` (175/176, always `surfType` 7; events doc section 2), rolls `cg_tracerchance` and hands the muzzle and impact to one of two hardcoded paths by the surface type argument. VERIFIED: `cmp [esp+0x20],7` at 0x300395fb sends `surfType == 7` (flesh) to the segment setup at 0x30039340 with a pushed 0, and every other surface to `CG_SpawnTracer` @ 0x30038f30.

**Every surface but flesh: a moving streak.** `CG_SpawnTracer` takes the impact in `eax` and the muzzle in `ecx` (VERIFIED, 0x30039604 and 0x30039600) and allocates a local entity (`CG_AllocLocalEntity` @ 0x3001ff50). VERIFIED (0x30038f30..0x300390af), its writes:

```
dir       = VectorNormalize(impact - muzzle)          // 0x30039cc0
travel    = |impact - cg_tracerlength*dir - muzzle|   // sqrt @ 0x3005ac90
le+0x08   = 2                                         // leType
le+0x18   = 2                                         // pos.trType, TR_LINEAR
le+0x1c   = cg.time - (rand() % cg.frametime) / 2     // pos.trTime, integer ms
le+0x24   = muzzle                                    // pos.trBase
le+0x30   = cg_tracerSpeed * dir                      // pos.trDelta
le+0x10   = trTime - ftol(travel * -1000 / cg_tracerSpeed)   // endTime
```

INFERRED, from how 0x3001ffe0, `CG_AddLocalEntities` and `BG_EvaluateTrajectory` read those offsets: the field names in the comments. The `-1000.0` is `ds:0x30069594`. VERIFIED: `ftol` @ 0x3005a890 is a `fistp` with a truncation fix-up, so the lifetime is `travel / speed` in whole ms, truncated. INFERRED from use: the globals read at 0x30039029 and 0x3003904d (0x30207144, 0x30207148) are `cg.frametime` and `cg.time`; the second is what `CG_AddLocalEntities` compares `endTime` against.

VERIFIED: `CG_AddLocalEntities` @ 0x300201b0 frees an entity once `0x30207148 >= le+0x10` (`cmp`/`jl` at 0x300201c9) and dispatches `leType` 2 to 0x3001ffe0 (0x3002021a). That evaluates the trajectory at `cg.time` through `BG_EvaluateTrajectory` @ 0x30005470 (its jump table at 0x3000575c sends type 2 to 0x300054a0, `trBase + trDelta * (time - trTime) * 0.001`, the `0.001` at `ds:0x300693c0`), normalizes `trDelta` and draws `CG_DrawTracer(p, p + cg_tracerlength * dir)` (0x30020007..0x30020045). So the trajectory point is the tail:

```
p(t) = muzzle + cg_tracerSpeed * dir * (t - trTime)
draw [p(t), p(t) + cg_tracerlength * dir] while t < endTime
```

The tail starts on the muzzle (or ahead of it by the half-frame back-date) and the head reaches the impact on the last frame. VERIFIED: `CG_SpawnTracer` reads no constant but `-1000.0`, so this path has no minimum distance, and a shot shorter than `cg_tracerlength` draws its whole streak past the impact for `(length - dist) / speed`. United Offensive's spawn adds that gate and widens tracers under `sv_night` (coduo-diff.md 19, "The tracer cvars").

**Flesh: a one-frame segment.** The setup at 0x30039340 (VERIFIED, 0x30039343..0x30039426):

```
len = VectorNormalize(end - start)
if (len < 100.0 && arg == 0)            return          // ds:0x30069524
frac    = 50.0 + (len - 60.0) * rand() * (1/32768)     // ds:0x3006958c / 0x30069590 / 0x300693b4
endDist = min(frac + cg_tracerlength, len)             // ds:0x302999e8
CG_DrawTracer(start + frac*dir, start + endDist*dir)
```

It allocates nothing and only adds the quad to the scene, so it draws on the event's frame only. INFERRED: the syscall 0x40 at the end of `CG_DrawTracer` adds a polygon to this frame's scene, the call the other one-shot quads use.

`CG_DrawTracer` @ 0x300390c0 emits one quad with corners `A ± cg_tracerwidth*side` / `B ± cg_tracerwidth*side`, `side` the normalized screen-space perpendicular, with the `gfx/misc/tracer` shader (registered in `CG_RegisterGraphics` @ 0x30020da0). VERIFIED, from the vertex writes: `u` runs along the segment, 1 at `B` (the head) and 0 at `A` (the tail).

Cvar defaults, from the cgame's cvar table at 0x30074e54..0x30074e8c:

| cvar | struct VA | value VA | default |
|---|---|---|---|
| `cg_tracerchance` | 0x301df440 | 0x301df448 | `0.4` |
| `cg_tracerwidth` | 0x3020d820 | 0x3020d828 | `0.8` (a half-width, 1.6 units across) |
| `cg_tracerSpeed` | 0x301de900 | 0x301de908 | `4500` |
| `cg_tracerlength` | 0x302999e0 | 0x302999e8 | `160` (the full streak length) |

0x30039340 is the flesh tracer's segment setup, not `CG_BloodSpray` as the events doc once named it. Using it for every tracer anchors a moving streak's head 50 units out, putting its tail 110 units behind the muzzle for its first two frames.

vcod: `FxSystem::spawn_tracer` (`fx/sim.rs`), fed by `Resolved::Tracer` (`fx/registry.rs`) from events 173 to 176.

## R9. Physics, `bounce`, and the `Emitter` debris model

The effect system is a class hierarchy with a vtable per primitive; the `Emitter` instance is 0x18c bytes, built by 0x4a02c0 (vtable 0x559f48). Offsets below are that object's; `frametime` is the engine's frame time in ms at 0x1407514. Function names marked "vcod's name" are not the binary's.

- VERIFIED, the flags parser (0x49bb30, the `useModel` / `usePhysics` / `impactKills` / `impactFx` string compares): `useModel` 0x1000000, `useBBox` 0x4000000, `usePhysics` 0x2000000, `impactKills` 0x40000000, `impactFx` 0x80000000, `deathFx` 0x20000000, `emitFx` 0x10000000, `useAlpha` 0x8000000, `relative` 0x200000. `expensivePhysics` is accepted and sets nothing.
- VERIFIED: the `bounce` handler 0x49b710 ORs 0x2000000 (`usePhysics`), the `impactfx` list parser 0x49cbd0 ORs 0x82000000 (`usePhysics` and `impactFx`), the `models` list parser 0x49cae0 ORs 0x1000000 (`useModel`). So `bounce` alone makes a particle physical.
- VERIFIED: an `impactfx` entry goes through 0x492eb0, which copies the name up to its first `.` and lowercases it. INFERRED: the list names an effect without its extension (`fx/shellejects/shelleject_pistolsound`) and `.efx` is put back on.
- VERIFIED, the spawner 0x494050: `gravity` (template +0x194/+0x198, sampled) is added to the acceleration's z.
- VERIFIED, the spawner's `Emitter` arm (case 5): it samples three `angle` pairs (+0x134..+0x148) and adds them to the spawn axis's angles (0x42e470), samples three `angleDelta` pairs (+0x14c..+0x160), and samples `bounce` (+0x254/+0x258). VERIFIED, 0x4a02c0: the instance stores angles at +0x168, angle deltas at +0x174 and its elasticity at +0x138. INFERRED, from the argument order: the sampled `bounce` is the elasticity.
- VERIFIED, `CParticle::UpdateOrigin` (vcod's name, 0x48df20): first `vel += accel * frametime * 0.001` (0x48e340), then `new = org + vel*t + 0.5*accel*t*t` on the new velocity. With `usePhysics` it traces `org` to `new` through 0x424530, model 0, brush mask 1 (`CONTENTS_SOLID`), a point unless `useBBox` passes the instance's box (0x48e0e2..0x48e123).
- INFERRED (branch conditions in 0x48df20): on a hit with neither start- nor all-solid, an `impactFx` particle plays its impact effect at the hit and an `impactKills` particle dies. Otherwise `vel += accel * fraction * t`, `vel -= 2 * (vel . n) * n`, `vel *= elasticity`, and when `n.z > 0` and the new `vel.z < 4` (constant at 0x569080) velocity and acceleration are zeroed and `usePhysics` and `impactFx` cleared (`& 0x7dffffff`). The origin moves to `org + (new - org) * fraction`; without a hit, to `new`.
- INFERRED, from the call sites at 0x48dea5, 0x48ed15, 0x48f394, 0x48fa1c and 0x491272: every moving primitive integrates this way, sprites included.
- VERIFIED, `CEmitter::Update` (vcod's name, 0x48f9e0): when `UpdateOrigin` leaves the origin where it was, the angle deltas are multiplied by 0.7 (0x5691a0); then 0x490300 adds `frametime * 0.01 * angleDelta` to the angles (0x568f00) and turns them into the model's axes. With `useModel` the axes are scaled by the instance's size (+0xb8) and the refEntity goes to the scene (0x48f9c0). INFERRED: `angleDelta` is degrees per 100 ms, a resting model's spin loses 30% a frame, and the model draws with no fade until `life` runs out.
- VERIFIED, the effect runner 0x493870: each element reads its `cullrange` (+0x64) before its spawn count is drawn. INFERRED, with the decal note in the events doc (section 9): an element past its `cullrange` from the view is skipped at spawn.

vcod: `fx::sim::update_origin` and `step_pool` follow the above for every particle. An `Emitter` is a particle with `Debris` (model, angles), drawn through `FxSystem::models`. Every element's `cullrange` is tested once at spawn against `FxSystem::set_view`'s camera. Chaining, `relative` and `useBBox` are R10.

## R10. Chaining (`playfx`, `emitfx`, `impactfx`, `deathfx`), `relative`, `useBBox`

Offsets on a template are the 0x25c template's (R1); on an instance, the primitive's.

### The lists

- VERIFIED, the primitive key parser 0x49d8b0: `impactfx` goes to 0x49cbd0, `deathfx` to 0x49cce0, `emitfx` to 0x49ce00, `playfx` to 0x49cf20. Each registers every entry through 0x492eb0 into a vector: impact at template +0x78, death at +0x88, emit at +0x98, play at +0xa8. 0x49cce0 ORs 0x20000000 (`deathFx`) into +0xb8, 0x49ce00 ORs 0x10000000 (`emitFx`), 0x49cbd0 ORs 0x82000000 (R9); 0x49cf20 sets no flag. Their error strings name them: `CPrimitiveTemplate::ParseDeathFxStrings` / `ParseEmitterFxStrings` / `ParsePlayFxStrings`.
- VERIFIED, 0x492780: a list is drawn from with the engine's LCG (`* 0x343fd + 0x269ec3`, state at 0x57b4b0), one entry per spawned primitive; an empty list yields effect id 0.
- VERIFIED: `density` (0x49c230) stores its pair at template +0x19c/+0x1a0, `variance` (0x49c280) at +0x1a4/+0x1a8.
- VERIFIED, the 1.1 paks (`tools/efx_chains.py`, 459 files): 109 `emitfx` entries in 91 files, all in `Emitter` blocks; 116 `playfx` in 89 files, all in `FxRunner`; 19 `impactfx` in 11 files (`Emitter`, `Tail`); 16 `deathfx` in 12 files (`Particle`, `OrientedParticle`, `Tail`, `Line`). Every target exists. The deepest chain is 5 effects (`fx/explosions/v2_exlosion.efx`); `fx/explosions/moltov_hit.efx` names itself in its `impactfx`. No block sets a chaining flag without its list or the reverse.

### `FxRunner`

- VERIFIED, the effect runner 0x493870 (`CFxScheduler::PlayEffect` by its error string): per element it tests `cullrange` (R9), draws `count`, and per instance draws `delay` and truncates it (0x538be0).
- INFERRED (branch on the truncated delay): under 1 ms it calls the spawner 0x494050 at once; otherwise it queues a 0x44-byte record (effect id, element index, due time `0x140750c + delay`, origin, axis, bolt) that 0x493f40 hands to the spawner once due, with the overshoot as its last argument.
- VERIFIED, the spawner's case 10 (0x494050, template type 0xa): `PlayEffect(pick(+0xa8), origin, axis, bolt)`, with the origin and axis the spawner computed for the element (`origin` rotated unless `cheapOrgCalc`, then `orgOnSphere`/`orgOnCylinder`, R6). INFERRED: the child effect runs its own runner loop inside the call, so an undelayed `FxRunner` chain plays within one `PlayEffect`.
- VERIFIED: nothing on this path counts depth. INFERRED: a self-referencing `playfx` would recurse without end; the stock self-reference is an `impactfx`, which replays only on a later frame's hit.

### `emitfx`

- VERIFIED, the spawner's case 5 hands the `Emitter` constructor 0x4a02c0 `pick(+0x88)`, `pick(+0x78)`, `pick(+0x98)` and the sampled `density` and `variance` (pushed at 0x4967fe/0x4967ff from the slots 0x496620 and 0x4965ed filled). The constructor stores them at instance +0x38 (death), +0x34 (impact), +0x180 (emit), +0x184 (density), +0x188 (variance), the spawn time at +0x160, and `+0x164 = (2 * rand() * (1/32768) - 1) * variance + density`. 0x49e690 and 0x49e6d0 copy its origin and velocity to +0x13c and +0x148.
- INFERRED (the loop in `CEmitter::Update` 0x48f9e0 after the move, with `emitFx` set at 0x48fcd4): from `clock = +0x160` and `s = 0`, while `clock < now`: `s += 12` (0x48fd10); `p = +0x13c + +0x148 * s + 0.5 * accel * s^2` (seconds, `accel` the instance's current +0xe8); when `|p - +0x13c|^2 >= +0x164^2` it takes one Newton step on that squared distance (`3` at 0x5690c8, `0.25` at 0x568ed0, `1000` at 0x569090), floors the result at the previous step, calls `PlayEffect(+0x180, p(s), the instance axis +0x58, bolt)`, moves +0x13c/+0x148 to `p(s)` and its velocity, adds `s` to +0x160 and draws a new spacing as above (0x4902a9); otherwise `clock += 12`. VERIFIED: the only other writes to +0x13c/+0x148 are the save/load pair at 0x4903d0.
- INFERRED from those writes: the plays follow the emitter's launch arc, not its traced path, so a bounce does not move them, and once the emitter rests (R9 zeroes its acceleration) they carry on in a straight line at the launch-arc velocity. The walk samples one 12 ms step past `now`, so a play can land up to 12 ms ahead of the frame.
- INFERRED, from that loop: `density` is a distance, not a rate, and `variance` its jitter. A `variance` above `density` (`grenade2.efx`'s particulates, `density 4 variance 14`) can draw a spacing near zero, and the Newton step then lands about half a step after the last play.

### `impactfx` and `deathfx`

- `impactfx` plays where a physics particle hits (R9), through 0x493730 with no bolt.
- VERIFIED, `Die` (0x48dda0, slot 1 of every primitive vtable, e.g. 0x559f48/0x559f80): with `deathFx` set and `impactKills` (0x40000000) clear it builds a direction from three `rand() / 32768 * 2 - 1` draws, normalizes it (0x42db90) and plays +0x38 at the instance origin +4 through 0x493730, with no bolt.
- VERIFIED, the pool walk 0x49f1e0 (1800 slots, 0x708): a primitive past its end time has 0x40000000 cleared (`& 0xbfffffff`) before `Die`; one whose `Update` (slot 2) returns false gets `Die` with the bit as it was. INFERRED: an expired element always plays its `deathfx`, an `impactKills` hit never does, and any other failed update (a lost bolt) does.
- VERIFIED, 0x49ecb0: a full pool shows `gfx/2d/warning@maxeffects.jpg` (behind a cvar) and recycles slot 0 through `Die`.

### `relative`

- VERIFIED, the flags parser 0x49bb30: `relative` ORs 0x200000.
- VERIFIED, the spawner (0x494050, the test at 0x49547c): with 0x200000 set and a bolt whose entity number is not negative, it takes a shared bolt record (0x48d6c0, a list at 0x1407320 keyed on entity and bone, reference counted, freed by 0x48d730) and stores it at instance +0xd8. When the bolt cannot be posed (0x48d770 returns 0) it creates nothing (the jump to 0x497080 at 0x4954d8).
- VERIFIED, 0x49e8d0: a bolt is posed from the entity's origin and axis (0x460480 with 0xc) and, for a bone index of 0 or more, that bone's matrix (0x460480 with 0xb); 0x48d770 re-poses it when its stamp differs from the counter at 0x163a218. INFERRED: that counter is the frame number, so a bolt is posed once a frame.
- VERIFIED, the `Emitter` constructor's bolted branch: it moves the world origin, velocity and acceleration into the bolt's frame (0x44b890, 0x44b8e0). `UpdateOrigin` (0x48df20) and the draw (0x44b7f0) transform back through the bolt each frame; a bolt that fails to pose has its record marked dead and the primitive's update fails.
- INFERRED, from the callers: only effects played on a tag pass a bolt (syscall 0xe5, `cod11-events-and-fx.md` 5b, 5d), so `relative` matters only for muzzle flashes and brass. The stock users are 26 muzzle flashes and 11 tagged or map effects (`tools/efx_chains.py -v`).

### `useBBox`

- VERIFIED: the `min` / `max` keys (0x49b550 / 0x49b5a0, key strings at 0x55962c / 0x559628) store a box at template +0xc4/+0xd0 and OR 0x6000000 (`useBBox` and `usePhysics`); the flag alone is 0x4000000 (0x49bc80). `UpdateOrigin` traces that box instead of a point when the bit is set (0x48e0e2).
- VERIFIED, the paks: no stock block sets `useBBox`, `min` or `max`.

### vcod

`crates/client/src/fx/sim.rs`:

- `FxRunner` is `Behavior::Runner`: per instance, its origin and spawn frame, then a `Chain` due after its `delay`. `FxSystem::spawn_marked` plays the undelayed ones before it returns; `FxSystem::spawn_chained`, called each frame after `step`, plays the rest with every `impactfx`, `emitfx` and `deathfx` the step raised.
- An `Emitter` with `emitfx` carries the `Emit` cursor and walks it as above, in integer ms. The Newton step is floored at 1 ms past the last play, so a zero spacing cannot spin; retail has no such floor. The child plays along the emitter's forward, not its full axis, and without the `useModel` size scale on that axis.
- `deathfx` plays on expiry and on a lost tag, never on an `impactKills` hit.
- Chains stop at `MAX_CHAIN_DEPTH` (8); retail's only limit is its pool.
- `relative`: `Resolved::Spawn::bolt` names the shooter's `ctx.muzzles` key for a muzzle flash, `FxSystem::set_bolts` takes the muzzles each frame, and a relative element keeps its origin, velocity and acceleration in the frame `tangent_basis` builds on the muzzle's forward (the frame a `Directed` spawn uses), so its roll is not the tag's. It dies when its key leaves the map. A bolted particle does not trace. Brass passes no bolt; its two stock effects have no `relative` element. An `EV_PLAY_FX_ON_TAG` effect is bolted to its tag's full axis instead (`FxSystem::spawn_on_tag`, `set_tag_bolts`, `cod11-events-and-fx.md` 2, "playFxOnTag").
- A delayed element on a bolt spawns at the pose of the play's frame; retail poses the bolt again when it falls due (0x493f40).
- `useBBox` is not implemented; nothing in the paks reaches it.
- `cullrange` is tested once, at spawn, for every element, sprites and decals included (R9).

## R11. The axis an effect plays on: syscalls `0xe3` and `0xe4`

The cgame plays a script effect through one of three syscalls, dispatched at 0x4040b8..0x4040e0 (the `case 0xe3`/`0xe4`/`0xe5` arms of the cgame syscall switch). Each builds a 3x3 axis and calls `PlayEffect` (0x493870); the axis is the frame an element's `origin`, `velocity` and `accel` are written in (R6), forward first.

- VERIFIED, 0xe3 (0x4936c0, position only): the axis is the constant `(0,0,1), (1,0,0), (0,1,0)` built on the stack before the call, so an effect played with no direction points up, its right world +x and its up world +y.
- VERIFIED, 0xe4 (0x4a1800, position and direction): the direction is copied to axis[0] (`[esp+8]`), then 0x42e1b0 is called with `edi` on axis[0], `eax` on axis[1] (`[esp+0x14]`) and `ebx` on axis[2] (`[esp+0x20]`). 0x42e1b0 is Q3's `MakeNormalVectors`: right = `(f.z, -f.x, f.y)` less its projection on `f`, normalized (0x42db90), and up = right x forward.
- 0xe5 plays on a tag's own axis (R10, "relative"): it hands `PlayEffect` a null axis and the bolt, whose pose replaces the origin and axis (`cod11-events-and-fx.md` 2, "playFxOnTag"). vcod: `SpawnAt::Oriented`.

`EV_PLAY_FX` and an `ET_FX` entity with a zero `origin2` take 0xe3; `EV_PLAY_FX_DIR`, an `ET_FX` with a forward, and every impact and explosion event take 0xe4 (`cod11-events-and-fx.md` 2). vcod: `fx::sim::tangent_basis` is `MakeNormalVectors`, and `SpawnAt::Point` plays on `UP_AXIS`. Any other tangent basis turns a directed effect's sideways spread about its forward.

## What vcod implements from this, and what it doesn't

Implemented in `crates/client/src/fx/sim.rs`: R3 (unflagged curves hold `start`; `length` only on `Tail`/`Line`), R4's half-extent, `size2` under `nonUniformScale` and the vertex colour's `useAlpha` split, R5 (tail anchoring and full length), R6's spawn flags, R7's defaults, R8 (`FxSystem::spawn_tracer`, fed by `Resolved::Tracer` from 173 to 176), R9 (physics, `bounce`, `impactfx`, `Emitter` debris models), R10 (chaining, `relative`) and R11 (the play axes).

Not implemented:

- The `nonlinear` / `wave` / `clamp` / `random` curve modes; only `linear` against constant is honoured (`parm` is parsed and dropped). One `size` block and five `alpha`/`rgb` blocks in the corpus use another mode.
- `electricity`, `useBBox`, and CoD1's `evenDistribution` (CoD2 effects honour theirs).
