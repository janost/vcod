# CoD4 1.7: effects (`FxEffectDef`), the impact table, the event effects

How a 1.7 effect is laid out in a zone, how its sampled graphs read back,
how vcod plays it on its CoD2 effect path, and which effect each entity
event plays. Code: `crates/cod4/src/fx.rs` (the element decode),
`crates/client/src/cod4_fx.rs` (the conversion, the event arms, the
looping elements, the createfx ambience and the client-side bullets),
`crates/client/src/fx/cod4_trail.rs` (the trails);
gates: `crates/cod4/tests/fx_census.rs` and the replay gate in
`crates/client/src/play/cod4.rs`.

## Sources

- The zones of a 1.7 install (`COD4_DIR`), walked with their bulk data;
  `cod4-fastfile.md` 10 has the `FxEffectDef` (0x20), `FxElemDef` (0xFC)
  and `FxImpactTable` load order.
- The captures of `cod4-netfields.md` 8 and 10.

A claim labelled INFERRED that names no 1.7 address is not yet confirmed
on 1.7.

## 1. `FxEffectDef`

Name 0, flags 4, total size 8, `msecLoopingLife` 12, then the looping,
one-shot and emission element counts at 16, 20 and 24 and the elements
at 28: the looping ones first, then the one-shot, then copies of the
one-shot elements of the effects its elements emit (section 15).
VERIFIED for the slots the walk follows (the
counts summed size the element array); the three roles' order is
INFERRED. mp_crash's and `common_mp`'s effects hold 1398
elements: 215 looping, 1173 one-shot, 10 emitted. VERIFIED (the census).

## 2. `FxElemDef` (0xFC)

| Offset | Field |
|---|---|
| 0 | flags |
| 4 | spawn: looping `(intervalMsec, count)` or one-shot count `(base, amplitude)` |
| 12, 20, 28 | `spawnRange`, `fadeInRange`, `fadeOutRange` (float base, amplitude) |
| 36 | `spawnFrustumCullRadius` |
| 40, 48 | `spawnDelayMsec`, `lifeSpanMsec` (int base, amplitude) |
| 56 | `spawnOrigin[3]` |
| 80, 88 | `spawnOffsetRadius`, `spawnOffsetHeight` |
| 96, 120 | `spawnAngles[3]` (radians), `angularVelocity[3]` (radians per ms) |
| 144, 152, 160 | `initialRotation` (radians), `gravity`, `reflectionFactor` |
| 168 | `FxElemAtlas`: behavior, index, fps, loopCount, column bits, row bits, entry count |
| 176..179 | `elemType`, `visualCount`, `velIntervalCount`, `visStateIntervalCount` |
| 180, 184 | velocity samples (0x60 each), visual samples (0x30 each), count + 1 of each |
| 188 | visuals |
| 192, 204 | collision mins, maxs |
| 216..224 | effect on impact, on death, emitted (names) |
| 228, 236 | `emitDist`, `emitDistVariance` |
| 244 | trail definition |
| 248, 249 | sort order, lighting fraction |

The slots at 176..188 and 216..244 are VERIFIED (the walk follows them
block-exactly, `cod4-fastfile.md` 10); the rest of the layout is
INFERRED. The sample slots hold `count + 1` samples whenever
they are not null; a runner's visual slot is null. VERIFIED (the
census).

Element types: 0 billboard sprite, 1 oriented sprite, 2 tail, 3 trail,
4 cloud, 5 model, 6 omni light, 7 spot light, 8 sound, 9 decal, 10
runner. The census meets every type but 7 and 8: 733 billboards, 23
oriented sprites, 204 tails, 5 trails, 16 clouds, 116 models, 82 omni
lights, 64 decals, 155 runners. VERIFIED (the counts); the names of 7
and 8 are INFERRED.

Flag 0x2 puts `spawnOrigin` in the effect's axes: `FX_GetSpawnOrigin`
(iw3mp.exe 0x4a5cb0) draws the offset off `fx_randomTable` (0x6bce70,
entries 6..8) over the element's ranges at +0x38..+0x4c, then `test
[elemDef], 2` either turns it through the effect's frame (0x4a5a00) or
adds it to the frame's origin (+0x10) as it stands, in the world's axes.
VERIFIED (the instructions). vcod spawns an element without 0x2 as
CoD2's `cheapOrgCalc` does (`cod4_fx::primitive`). Trap: turning every
offset through the effect's axes wheels mp_backlot's `weather/hawk`
(flags 0x11000040, an oriented sprite of `gfx_hawk`, offset (0,
-500..1000, 1000..1500), placed at (496.7, 63.3, 275.5) on angles (270,
0, 0)) at z 275.45 through a building, the offset's z on world -x; read
right it flies 1275 to 1710 units up. VERIFIED (the zone; the gate
`cod4_fx::tests::backlot_hawks_circle_over_the_town`, which fails with
the turned frame). That it looks as retail's is pending a human's look.

Other flags, INFERRED: 0x4 frustum-culled spawn; 0x30 the spawn
offset (0x10 sphere, 0x20 cylinder); 0xc0 what the element moves with
(0 world, 0x40 its spawn, 0x80 the effect, 0xc0 the effect's offset);
0x100 collides; 0x200 dies on touch; 0x1000000 / 0x2000000 the local /
world velocity graph moves it; 0x4000000 gravity; 0x10000000 a second
size of its own.

## 3. The velocity graph

A velocity sample is the local frame's `FxElemVelStateInFrame`, then the
world's, each a velocity and a summed displacement (`totalDelta`), each
per axis as a base and an amplitude. In every element of the census each
sample's `totalDelta` is the last one's plus the trapezoid of the two
velocities, exactly, for bases and amplitudes both. VERIFIED (the
census, gap 0).

The runtime reads a position back as `life_ms * (totalDelta[i] + v[i] *
(u - u^2 / 2) + v[i + 1] * u^2 / 2)` at the fraction `u` into interval
`i`, the random draw scaling each axis's amplitude
(`FX_IntegrateVelocity*`; INFERRED), and the samples are the
editor's units per second over `intervalCount * 1000`
(`FX_SampleVelocity`; INFERRED).
So a velocity sample times `intervalCount * 1000` is units per second
over the element's life, and its piecewise-linear curve integrates to
the same path CoD2's `Particle::IntegrateVelocity` gives (cod2-efx.md
2.3). Gravity pulls `0.5 * gravity * 800 * t^2` down
(FX_NextElementPosition; INFERRED).

## 4. The visual graph and atlas

A visual sample is a base and an amplitude `FxElemVisualState`: colour
bytes, rotation delta, rotation total, two sizes, scale (0x18).
INFERRED, as is how the runtime samples it: the sample pair by the
life fraction times the interval count, lerped; one random draw for all
four colour bytes, its own for each size; rotation integrated like the
velocity, in radians per ms over `intervalCount * 1000`.

- The colour bytes are a `D3DCOLOR`, B, G, R, A in memory: the converter
  packs both ends with `Byte4PackVertexColor` (red to byte 2, blue to
  byte 0) (`FX_ConvertVisualSamples`; INFERRED). The m16's
  world flash light (`muzzleflashes/m16_flash_wv_a` element 0) carries
  bytes (158, 182, 255, 255), a warm light read that way. VERIFIED
  (`common_mp`).
- The amplitude's colour is the random lerp's far end, not an offset:
  `FX_InterpolateColor` takes `base * (1 - r) + amplitude * r`, where
  sizes, rotation and scale take `base + amplitude * r`.
  INFERRED. An element whose colour does not vary carries the same
  bytes in both (`grenadeexp_concrete_1`'s white smoke), and the
  concrete debris (element 3) carries base (40, 40, 40) and amplitude
  (77, 77, 77). VERIFIED (`common_mp`).

The atlas behavior's low two bits pick the first frame (0 `index`, 1 a
random frame, 2 the spawn's sequence number); 0x4 plays the whole atlas
over the life, else `fps`; 0x8 loops `loopCount` times only; the grid is
`1 << rowBits` by `1 << columnBits`. INFERRED.

`spawnRange` culls a spawn whose camera distance minus the base is
negative or past the amplitude, only when the amplitude is set
(`FX_CullElemForSpawn`, sections 5 and 18).

## 5. On vcod's effect path

`cod4_fx::primitive` turns each element into a CoD2 primitive
(`vcod_cod2::efx::Primitive`) and the fx system plays it as CoD2's:

- Sprites, oriented sprites, tails, clouds, models (as CoD2's
  model-debris `Emitter`), lights, decals and runners map to their CoD2
  kinds.
- The visual samples become the colour, alpha, size, size2, length and
  rotation-delta channels, the base as the channel curve and base plus
  amplitude as its rand curve (the amplitude's own colour for the colour
  and alpha, section 4); the velocity samples become the
  `velocity` (local) and `velocity2` (world, `absoluteVel2`) channels,
  times `intervalCount * 1000`. With the random weight drawn per
  channel, this is CoD4's `base + amplitude * random` per channel.
- A cloud (type 4) is not CoD2's: its cube is the visual state's
  `scale` and each flake `size[0]` wide and `size[1]` long, streaked
  along the element's velocity (`FX_DrawElem_Cloud` puts the scale in the
  placement and the two sizes in `GfxParticleCloud.radius`, its end point
  the position less the velocity's direction; `R_SetParticleCloudConstants`
  turns the long side onto the view's image of that direction).
  INFERRED. So a CoD4 cloud
  converts with `size` from the scale, `size2` and `length` from the two
  sizes, `nonUniformScale` and `useLength` on. mp_cargoship's
  `weather/rain_mp_cargoship` element 0 is a cloud of
  `cloud_snowflake01` at size (1.5, 150) and scale 1000 (VERIFIED, the
  zone); read as CoD2's sizes it draws as a 1.5-unit cube of 150-unit
  flakes, giant white discs. The gate
  `cod4_fx::tests::cargoship_rain_clouds_are_thin_streaks_in_a_wide_cube`
  holds the conversion; how the rain looks beside retail's is pending a
  human's look.
- Gravity becomes CoD2's `gravity` range at -400 times CoD4's.
- An element running relative to the effect (flags 0xc0 at 0x80 or
  above) rides the effect's tag (`relative`).
- A looping element's `(intervalMsec, count)` is `count` spawns in all,
  0x7fffffff for ever: every effect's `msecLoopingLife` is its looping
  elements' longest `interval * (count - 1)`, or 0x7fffffff when one
  loops for ever; mp_crash's and `common_mp`'s looping elements are 134
  finite and 81 for ever. VERIFIED (the census,
  `fx_census::looping_counts_are_spawns_in_all`). That each spawn is
  one element, the `n`th at `n * intervalMsec` past the effect's start,
  is INFERRED (`FX_SpawnLoopingElems`), as is that an update
  longer than 128 ms starts its spawns late (its
  `FX_LimitStabilizeTimeForElemDef_Recurse` arm).
- vcod splits an effect with looping elements into its one-shot
  elements and one effect a looping element (`cod4_fx::parts`):
  `Cod4Effects::start` plays the first and keeps a looper per looping
  element, which `run_loops` spawns once an interval for its whole
  count, skipping on a catch-up past 128 ms the spawns whose delay and
  life are already over. The gate
  `cod4_fx::tests::a_looping_element_spawns_once_an_interval` holds the
  schedule. An effect another effect's runner starts goes through the
  fx system's chains, where a looping element still spawns its first
  eight intervals at once.

- An element spawns only from a camera inside its `spawnRange`:
  `FX_CullElemForSpawn` (iw3mp.exe 0x4a0b90) culls it when the camera's
  distance less `spawnRange.base` (elem def +0xc) is under 0 or at least
  `spawnRange.amplitude` (+0x10), and with flag 0x4 against the frustum
  at `spawnFrustumCullRadius` (+0x24); the spawn at 0x4a25b6 tests it
  while `fx_cull_elem_spawn` (registered at 0x49947d, stored at
  0x110ceb4) is on. VERIFIED (the disassembly). The stock effects use it
  as their LOD: `smoke/car_damage_blacksmoke_fire` draws its lens flare
  inside 550 and another past it, its fire sprites inside 1300 or 2500
  and a far fire from 1300 to 4000. VERIFIED (mp_crash's zone). vcod
  culls by `cod4_fx::primitive`'s `spawn_range` against the camera
  `Cod4Client::pump` hands the fx system each frame
  (`FxSystem::set_view`); before it handed none and every element of
  every effect spawned at any distance (cod4-client-perf.md 15).

Effects are keyed as the fx system's chains name them (`efx::chained_path`,
`<name>.efx`) and handed to it on the first frame
(`FxSystem::insert`); their materials join the map's material list so
the fx draw finds their images.

## 6. The event effects

What each event plays follows. The arms are `CG_EntityEvent`'s,
`CG_FireWeapon`'s and `CG_ImpactEffectForWeapon`'s, INFERRED.
That every name the replay gate meets is a zone effect is VERIFIED.

- `EV_FIRE_WEAPON` / `_LASTSHOT` / `_MG42`: the weapon's
  `worldFlashEffect` (`muzzleflashes/m16_flash_wv` for the M16) on the
  held weapon's `tag_flash`, and its `worldShellEjectEffect` (the last
  shot's `worldLastShotEjectEffect` when it has one) on `tag_brass`
  (section 10).
- `EV_BULLET_HIT`: the impact table's row by the weapon's impact type
  (0 small, 1 small exit, 2 large, 3 large exit, 4 shotgun, 5 its exit,
  6 AP, 7 AP exit, 8 grenade bounce, 9 grenade explode, 10 rocket
  explode, 11 dud), the surface's column, or for flesh (surface 7) the
  flesh column by impact flags 2 and 1; along `ByteToDir(eventParm)`.
- `cg_blood` 0 swaps a flesh hit (surface 7) for
  `impacts/flesh_hit_noblood`: iw3mp.exe registers that effect at
  0x43efd1 (stored at 0x84f288), and at 0x458ad9 the impact pick, its
  effect non-null, reads `cg_blood`'s byte (0x8c63bc + 12) and, it clear
  and the surface 7, takes the stored effect. VERIFIED (the disassembly).
  The knife's flesh hit swaps `impacts/flesh_hit_knife` (0x84f28c) for
  `impacts/flesh_hit_knife_noblood` (0x84f290) the same way at 0x458dee.
  VERIFIED (the disassembly). vcod plays no knife hit effect, so only the
  bullet's swap applies (`cod4_fx::FLESH_NOBLOOD`). That `esi` at 0x458ae5
  is the surface type is INFERRED (the compare with 7 against vcod's
  flesh column).
- `EV_GRENADE_BOUNCE`: row 8's surface column (`impacts/footstep_dust`
  for the frag's four bounces). VERIFIED (the replay gate).
- `EV_GRENADE_EXPLODE` and the rocket and custom explodes: the impact
  type's row (`explosions/grenadeexp_concrete_1`) and the weapon's
  `projExplosionEffect`. VERIFIED (the replay gate).
- `EV_PLAY_FX`: configstring 1598 + parm, along the entity's angles.
- The impact table is the map zone's one `FxImpactTable`
  (`cod4-fastfile.md` 10).

## 7. The earthquake

`EV_EARTHQUAKE`'s event entity carries `lerp.u.anonymous.data[0..2]` as
float scale, float radius and int duration ms: the frag's reads 0.3, 400
and 500. VERIFIED (`play-1.7-spec.txt`; the words are 0x3e99999a,
0x43c80000 and 500). vcod shakes the drawn view with CoD 1's
`CG_ShakeCamera` (`play::quake`); that CoD4's shake is the same formula
is INFERRED.

## 8. The entering ring

The frag's explode does not ride the missile: entity 317 enters as an
`ET_GENERAL` at the missile's last position with `eventSequence` 1 and
`EV_GRENADE_EXPLODE` (parm 5) in its ring's first slot. VERIFIED
(`play-1.7-spec.txt`, 69350 ms). `CG_ResetEntity` replays an entering
general entity's or missile's ring from 0, unless a missile launched
over 200 ms ago (`eFlags` 0x10000), INFERRED; vcod's walk
does the same, so the explode plays.

## 9. Gaps

- Elements drawing `distortion_*` materials were left out until they
  drew through the scene copy (section 17). Such a material's colour map
  is an offset map, not a colour: `distortion_heat.iwi`
  (`gfx_distortion_heat_far`, element 0 of
  `explosions/grenadeexp_concrete_1`, a sprite growing to 397 units over
  500 ms) holds DXT5 blocks whose colour endpoints span red 24..255 and
  green 4..255 with blue 0 everywhere. VERIFIED (the iwi decoded). Drawn
  as a colour sprite it painted the frag's explosion in reds, greens and
  yellows.

- Trails (type 3) draw (section 16) and emitting elements emit
  (section 15). Sound elements (type 8) are left out, none in these
  zones.
- Spot lights (type 7): no stock MP zone holds one. Of `common_mp` and
  the 21 stock MP map zones (`*_load` aside) no element is type 7 or 8.
  VERIFIED (a scratch walk of every element of those zones). vcod still
  converts one as an omni light (`cod4_fx::primitive`): the fx system's
  lights carry a position, radius and colour and no cone, and none of
  the stock effects needs one.
- Model elements need the zone's xmodel uploaded; the fx draw looks
  models up on the search path, where CoD4's are not.
- A material draws by its zone material's stages as the map's material
  list compiles them; the lighting fraction, the sort order and
  collision against the world are not applied. The distance fades are
  (section 18).
- Sizes are taken as CoD2's sizes; whether CoD4's are radii or diameters
  is not settled (pending a look).

## 10. The weapon's tags

- `CG_FireWeapon` plays the flash bolted to the firing entity's
  `tag_flash`, and `CG_EjectWeaponBrass` the shell eject bolted to its
  `tag_brass`: the view model's for the drawn player's own entity, the
  world weapon's for anyone else's. INFERRED.
- vcod takes the tags off the posed body (`play::cod4_players`,
  `WeaponTags`): the held weapon hangs off `tag_weapon_right`, and its
  `tag_flash` and `tag_brass` are the rig's bones, in the world through
  the body's transform. The fire event plays on this frame's pose
  (`Cod4Client::pump` plays the frame's events after it poses the bodies
  and the view weapon), on the tag's own axes (`SpawnAt::Oriented`). A body the frame did not pose (the own view, no
  rig) flashes 56 units over its origin and ejects nothing.
- The drawn first-person view's own shots (its playerstate's ring, or
  its own entity's) play the weapon's `viewFlashEffect` on the view
  model's `tag_flash` and `viewShellEjectEffect` (or
  `viewLastShotEjectEffect` on the last round) on its `tag_brass`, the
  tags of this frame's view pose in the world (`cod4_viewmodel`,
  cod4-client-view.md 4). INFERRED (the flash step's view
  arm, `CG_EjectWeaponBrass`). Each of the kill-shooter
  capture's own shots flashes 10 to 40 units ahead of the eye pointing
  down the view and ejects once. VERIFIED (the gate
  `play::cod4::tests::shooter_draws_its_view_weapon_and_flashes_on_its_tag`).
- The spectator capture's 18 M16 shots each flash about 37 units ahead of
  the walker's origin and 50 up, pointing along its yaw, and eject 18
  shells. VERIFIED (the gate
  `play::cod4::tests::spectator_capture_flashes_on_the_weapon_tags`).
- A flash or eject rides its tag while it plays: `cod4_fx` keys it in
  the fx system's `TagBolt` by the entity and a slot (`cod4_fx::slot`,
  the view's on the drawn client), `Cod4Effects::set_tags` hands the
  frame's tag poses on each frame, and the elements that move with the
  effect (flags 0xc0 at 0x80 or above, section 2) follow the pose; a
  looping element on a tag that stops posing ends. That retail bolts
  the effect to the tag is INFERRED (the flash step's
  `FX_PlayBoltedEffect`). In `muzzleflashes/m16_flash_wv_a` the omni
  light (18 units forward, flags 0x1000042) stays where it spawned and
  the short sprites (flags 0x82) ride the effect. VERIFIED
  (`common_mp`). Each of the spectator capture's flashes rides the
  walker's `tag_flash`, 36 frames in all, the nearest sprite within 2
  units of the tag's pose in each. VERIFIED (the gate
  `play::cod4::tests::spectator_capture_flashes_follow_the_tag`).

## 11. The map's createfx ambience and the effect entities

- `iw3mp.exe` holds `clientSideEffects` (0x6c6870, referenced from
  0x424d2f and 0x530c8e), `maps/mp/%s_fx.gsc` (0x6c6818, from 0x424c72),
  `maps/createfx/%s_fx.gsc` (0x6c682c, from 0x424ca5), the createfx
  header `//_createfx generated. Do not touch!!` (0x6c665c, from
  0x424581), `ent = maps\mp\_createfx::createLoopSound();` (0x6c6540,
  from 0x4241b9 and 0x424629), `ent = maps\mp\_utility::createOneshotEffect( `
  (0x6c6604, from 0x4243b9 and 0x424659) and `level._effect[`
  (0x6c67f8, from 0x4248df and 0x424922). VERIFIED (the strings and
  their references).
- So a 1.7 client starts a map's ambience itself at load
  (`CG_StartClientSideEffects`): the `level._effect[ "name" ] = loadfx(
  "file" )` lines of `maps/mp/<map>_fx.gsc` name the effects (32 at
  most), and `maps/createfx/<map>_fx.gsc` places them, each entry's keys
  in a fixed order (origin, angles, fxid, delay, an optional
  soundalias; a loop sound's origin, angles, soundalias), the first line
  out of order ending the read. Each effect plays on `AnglesToAxis` of
  its angles from `delay * 1000` ms of the cgame's clock; each sound
  (128 at most) plays on sound entity 1024 on. INFERRED.
  `vcod_cod4::fx::createfx` reads
  both files.
- mp_crash's server never places them: `maps/mp/mp_crash_fx.gsc` runs
  `maps\createfx\mp_crash_fx::main()` only inside a `/# #/` block when
  `clientSideEffects` is not `"1"`. VERIFIED (the rawfile). Its
  createfx file places 36 one-shot effects, every one at delay -15 (8
  `dust_wind_spiral`, 13 `hallway_smoke_light`, fires, smoke banks and
  dust), each of the eight names an effect that loops for ever, and no
  sounds. VERIFIED (the gate `fx_census::mp_crash_places_its_createfx_ambience`).
- vcod starts them on the first frame at their delay on the snapshot's
  clock, so they come up settled: mp_crash's 36 keep 80 looping
  elements running, about 535 particles and 6 fire lights alive, the
  drawn sprites within 400 units of the placed origins. VERIFIED (the
  gate `cod4_fx::tests::mp_crash_ambience_runs_for_ever`). The fire
  lights respawn every millisecond (interval 1, life 1 ms), as the zone
  says. VERIFIED (`common_mp`'s `fire/firelp_small_pm`).
- Fly mode (`vcod <map> --game cod4`, no server) runs the same
  ambience through `cod4_fly::FlyAv`: started on the first frame as on a
  level time of 60 s (vcod's choice, so it stands settled as a joining
  client sees it), and the map script's `ambientPlay("ambient_crash")`
  (`maps/mp/mp_crash.gsc`, a rawfile of the map's zone) playing as the
  ambient, the alias a server sends in configstring 821. VERIFIED (the
  rawfile; the gate
  `cod4_fly::tests::cod4_fly_mode_runs_the_maps_effects_and_ambient`:
  `ambient_crash` reaches the mixer, over 50 loopers and 535 to 584 particles
  over the first 3 s).
- `ET_FX` is `eType` 8 and `ET_LOOP_FX` 9, the loop's `cullDist` and
  `period` sit at offsets 84 and 88. VERIFIED (the netfield tables'
  order and names, cod4-netfields.md). An `ET_FX` (`spawnFx`) starts
  configstring 1598 + `un1`'s effect at `time2` and again whenever
  `time2` changes, ending the last one's loops; an `ET_LOOP_FX`
  (`playLoopedFx`) starts it on first sight within `cullDist` of the
  view (0 never culls) and restarts it every `period` ms. INFERRED
  (`CG_Fx`, `CG_LoopFx`). vcod ends an entity's loops when it
  leaves the snapshot. The captures carry neither type, so only the
  gate `cod4_fx::tests::effect_entities_start_and_retrigger` runs them.

## 12. Client-side bullets

- `iw3mp.exe` holds `sv_clientSideBullets` (0x6e7070, referenced from
  0x530106) and `whizby` (0x6cba90, referenced from 0x43e54b).
  VERIFIED.
- The server sends no impact for a shot into the world: the spectator
  capture's 18 M16 shots into walls carry no `EV_BULLET_HIT`. VERIFIED
  (the replay gate's 23 effects: 18 flashes, 4 bounces, 1 explode). A
  client traces each player's shot itself (`DrawBulletImpacts`): from
  the shooter's `tag_flash` along its view angles 8192 units (a
  shotgun's pellets to `minDamageRange`), the impact table's effect
  and the weapon's impact sound at the hit (`CG_BulletHitEvent`), and
  `whizby` at 16 units back along the shot from the point nearest the
  eye. INFERRED. The whizby test is `iw3mp.exe` 0x457f40 (the only read
  of the handle 0x43e557 stores at 0x84db64): the eye at least 64 units
  down the shot (the float 64 at 0x70b128), at least 64 short of its end
  (the double -64 at 0x70bd70, 0x457fe4..0x458011) and within 140 of its
  line (0x70bbe8), the sound 16 back (0x70b1f8). The constants are
  VERIFIED (the bytes). The three tests are INFERRED (the branches at
  0x457fde, 0x458011 and 0x4580a2). vcod tests the same; a test of
  `end + 64` plays a whizby to an eye just past the hit, which retail
  never does.
- vcod traces each bullet shot (`impactType` 1 to 4) against the clip
  map's brushes, terrain and static models (`cod4_fx::client_bullet`):
  the drawn view's own (its playerstate's ring or its client's entity)
  from the drawn eye along the view, another player's from its posed
  `tag_flash` along its angles. The spectator capture's 18
  shots strike at about 660 units, 15 on plaster and 3 on dirt
  (`impacts/large_plaster_1`, `bullet_large_plaster`); the kill target
  hears two of the shooter's rounds go by. VERIFIED (the gate
  `play::cod4::tests::client_side_bullets_strike_the_world`).
- The tracer: `cg_tracerchance` 0.2, `cg_firstPersonTracerChance` 0.5,
  `cg_tracerwidth` 4, `cg_tracerSpeed` 7500, `cg_tracerlength` 160,
  `cg_tracerScale` 1, `cg_tracerScaleMinDist` 5000,
  `cg_tracerScaleDistRange` 25000, `cg_tracerScrewDist` 100,
  `cg_tracerScrewRadius` 0.5: the defaults `iw3mp.exe` loads before
  each name's registration (0x43c4f1 and 0x43c99d to 0x43cca8).
  VERIFIED (the `flds` constants ahead of each `mov eax, name`). The
  beam's material is `gfx_tracer` (string in `iw3mp.exe`; the material
  in `localized_common_mp`). VERIFIED. The own shot draws one from the
  view weapon's `tag_flash` to the hit at the first-person chance
  (the penetration walk), the others' from their `tag_flash` at
  `cg_tracerchance` (the trajectory effects, the tracer test),
  back-dated half a draw of the frame's ms. INFERRED. vcod
  moves it as CoD2's moving tracer (`fx::cod2_tracer::COD4`) and draws
  it as 1.7's beam, below.
- The corkscrew. `cg_tracerScrewDist` and `cg_tracerScrewRadius`
  (100 and 0.5, above) register at 0x43cc45 and 0x43cca7 and land at
  0x8c9418 and 0x8c93e8. VERIFIED (the stores after each call). Their
  one reader, `CG_DrawTracer` (0x458390), calls the tracer scale
  (0x458290) for the two ends' widths and stores a
  13-dword beam into the pool at 0x110bb00 (96 entries, count at
  0x110ce80): the ends, two white colours, the widths, the material,
  `(int)(|end - start| * 8.0 / cg_tracerScrewDist)` segments (8.0 the
  double at 0x70ae88, the `fdiv` at 0x458470) and `cg_tracerScrewRadius`
  as the wiggle. VERIFIED (0x458390-0x4584b5). `FX_Beam_GenerateVerts`
  (0x492ab0) puts segment end `i` at `i / n` along the beam (the `fild`,
  `fdiv` at 0x4931fe) moved by the wiggle times entry `i & 7` of the
  eight-point circle at 0x6d40c8 ((0, 1), (0.71, 0.71), (1, 0), ... in
  steps of 45 degrees; `and eax, 7` at 0x493204), each end's two
  vertices at uv (1, 0) and (0, 1) (the half pairs 0x3c00 and
  0x3c000000, 0x4933dd), behind the first a cap at uv (0, 0). VERIFIED
  (the circle's floats, the stores). That the circle's x runs across
  the beam on screen and its y along it, the beam's screen direction
  taken back into the world with the clip matrix's inverse, the side
  `direction x view forward`, and the caps a radius beyond each end
  (the last at uv (1, 1)) is INFERRED
  (`FX_GenerateBeam_GetFlatDelta`, `FX_Beam_GenerateVerts`). So a
  160-unit streak is 12 segments, a turn of the circle every 100 units,
  half a unit off the line, and the body samples the image's
  anti-diagonal. vcod: `cod2_tracer::beam` builds those quads off the
  drawn eye; retail divides by a zero count on the last 12 units, vcod
  draws one segment there. `cod4_live_tracer_draws_the_corkscrew_beam`
  fires the M16 on the live path, checks one tracer comes out a cap,
  12 segments and a cap, and that it lights the frame against the same
  frame without it. VERIFIED (the test, ours).
- An impact effect's decal element marks the wall. The kill shooter's
  30 own shots each place a mark (`mc/gfx_impact_concrete01..03`,
  `mc/gfx_impact_plaster01`) that lands on the map's mark world, its
  material in the map's material list, and 14 draw a tracer. VERIFIED
  (the gate `play::cod4::tests::own_shots_strike_mark_and_trace`).
- The impact table's surface-0 (`default`) column is empty for every
  bullet row, so a round into `caulk` (mp_crash's edge,
  `surfaceFlags` 0x400a0) plays no effect, only `bullet_large_default`.
  VERIFIED (the zone's table).
- Live on vcod-server (`war`, mp_crash), once the prematch's
  `PMF_FROZEN` (0x800) clears, `+attack` sends the server's fire events
  back in the playerstate ring and each is traced from the drawn eye:
  six rounds into the rock 85 units ahead, six impact effects and
  sounds, three tracers. VERIFIED (the gate
  `play::cod4::tests::cod4_live_own_shots_strike_the_world`).
- The own shots fire off the prediction (cod4-client-view.md 5.4): the
  frame `+attack` goes down the predicted weapon half raises
  `EV_FIRE_WEAPON` and its flash, fire sound, view clip, tracer roll and
  client-side impact play; the snapshot that carries the shot a round
  trip later fires nothing. Live on vcod-server (two half-second bursts
  of the M16's three-round burst, loopback, 60 frames a second): each
  burst's first fire event on the press's frame, 0 ms after it, where
  the server's ring carried it 1 to 3 frames (17 to 54 ms) later; six
  fire events, six in the server's ring, six flashes, six impacts.
  VERIFIED (the gate
  `play::cod4::tests::cod4_live_own_shots_fire_the_frame_they_press`).
  Off a real link the gap is the round trip plus up to a snapshot.
- The spread, the bodies in the way and a shotgun's pellets are 12.2,
  the turrets' rounds, the tracer rolls and the own shots' third-person
  tracer start 12.3.

### 12.1 Penetration

- 1.7's client-side round penetrates (the penetration walk, `iw3mp.exe`
  0x455a20..0x4561a6): when the weapon's `penetrateType` (`WeaponDef`
  +0x134, read at 0x455c1c) is set, each contact's depth is
  `penetrationDepthTable[penetrateType][surface]` (0x7373b8, 29 floats a
  type, read at 0x455c65; surface 0 reads 0), times
  `perk_bulletPenetrationMultiplier` when the shooter's
  `clientinfo.perks` carries bit 5 (Deep Impact, 0x455c79..0x455ca0;
  the dvar registers 2.0 at 0x40de65); `bullet_penetrationMinFxDist`
  (30, 0x40b993) is read at 0x455e8b. VERIFIED (the code).
- The table is `info/bullet_penetration_mp`, a `BULLET_PEN_TABLE` info
  string of `small_`, `medium_` and `large_` depths by surface name in
  `common_mp`: concrete 6, 12, 12; plaster 12, 20, 20; wood 12, 16, 16;
  cloth, glass and foliage 72; dirt, sand, water and painted metal 0.
  VERIFIED (the rawfile). The M16 is medium (its row in the client's
  table). VERIFIED (`cod4_audio::weapons_from`).
- The loop, up to five times: the depth of the surface hit (0 ends it),
  the start advanced 0.135 past it over the cosine to the surface (a
  graze under 0.125 ends it), the next contact, then a trace back from
  that contact (or the round's end) to 0.01 short of the entry: its hit
  is the far side. The thickness (at least 1) over the lesser of the two
  sides' depths comes off a damage multiplier from 1, which ends the
  round at 0. A far side more than 30 units from the next contact plays
  the impact with flag 4 (the `_exit` effect row and sound) along the
  round, unless the next contact is sky, and the next contact its plain
  one; with no far side found the next contact plays when it is 30 units
  on. INFERRED (the penetration walk, `BulletTrace`,
  `BG_AdvanceTrace`).
- vcod runs it on the world alone, as its first trace
  (`cod4_fx::client_bullet`, `BulletHit::pierced`); the effects and
  sounds follow each contact (`bullet_plays`, `cod4_audio::bullet_cues`).
  The terrain's triangles meet a trace front to back only
  (cod4-collision.md 3), so a round into the ground passes it and meets
  the caulk floor under the map, which plays `bullet_large_default` and
  no effect.
- The spectator capture's 18 M16 rounds: one passes a plaster wall and
  plays `impacts/large_plaster_1_exit` and `bullet_large_exit_plaster`
  on its far side, then the wood beyond. Live on vcod-server, our M16
  turned east at the mp_crash spawn: every round's first contact is the
  8-unit concrete wall ahead, and each plays its far side's exit
  (`impacts/large_woodhit_exit`, `bullet_large_exit_wood`). VERIFIED
  (the gates `client_side_bullets_strike_the_world`,
  `cod4_live_own_rounds_pierce_the_wall_and_play_its_far_side`); no
  retail capture shows a client-side impact, so the counts rest on the
  code above.
- The hit marker on a penetrating hit is the server script's
  (`_damagefeedback.gsc`'s hudelem), drawn as any other hudelem.

### 12.2 Spread and bodies

- The own shot's rounds (`DrawBulletImpacts`, `iw3mp.exe` 0x456620):
  the own-client test is `otherFlags & 6` (0x45662b) and the entity's
  number against `ps.clientNum` (0x456647); the spread scale is a float
  at 0x84caa0 over 255.0 (0x456674..0x45667f); the sight test is
  `fWeaponPosFrac == 1.0` (`fld1; fcomp [ps+0xf4]`, 0x456695); the two
  ends come from the call at 0x4566bb and the eye from the one at
  0x4566cc. VERIFIED (the code). That the float is last frame's
  `aimSpreadScale` (`cg.lastFrame`), the callees
  `BG_GetSpreadForWeapon` and `CG_GetPlayerViewOrigin`, and the hip
  direction off the drawn view (`gunPitch`, `gunYaw`) are INFERRED.
- The amount is `(max - adsSpread) * scale + adsSpread` in the sight,
  else `(max - min) * scale + min` (0x456b6f..0x456b81 and
  0x45682d..0x456842). A spread weapon (`weapClass` 3, 0x456bca) fires
  `shotCount` rounds to `minDamageRange`, anything else one to 8192
  (0x456bec). Round `k`'s seed is the playerstate's `commandTime` plus
  `k` (`mov ecx,[ps]; add ecx,esi`, 0x456ca2..0x456ca9), its end off
  0x456470, which draws the disc at 0x4a66e0 and adds `range` along the
  forward axis, then the disc's point times `tan(spread) * range` along
  right and up. VERIFIED (the code). The disc is the spread disc draw
  (`G_GoodRandomFloat` twice, angle then radius), INFERRED.
- The playerstate handed in is the frame's predicted one after every
  cmd (`CG_TransitionPlayerState` after `CG_PredictPlayerState`'s loop),
  so its `commandTime` is the fire cmd's time and the server's
  `Bullet_Fire` (seeded by the cmd's time under `g_antilag`) draws the
  same disc point. INFERRED.
- Another player's shot: the stance's hip ends
  (`CG_GuessSpreadForWeapon`, 0x456874), his sight (0x456890), and the
  scale his speed between the two snapshots, `trBase` difference times
  1000 over the snapshots' time difference, capped at 255, over 255
  (0x45689b..0x456949). VERIFIED (the code).
- His stance and sight are read off his legs anim (entity +0x198:
  `nextState` at +0xcc, `legsAnim` at its +0xcc, the netfield table's
  offset 204), toggle bit 0x200 off, in the anim records at
  `[0xd5ec414]` (0x68 bytes each, a u64 `movetype` at +0x58): prone is
  `movetype & 0x308` (0x4561b0, 0x4561e7), crouched `& 0xc4` or the
  high word's `& 0x300` (0x4059b0, 0x4059e7), each picking its hip ends
  at 0x456250 (prone first); sighted is `& 0x3f0`, the six walks
  (0x456200, 0x456237), stored at 0x4568bc and tested at 0x45681b, the
  near end then `adsSpread` (`WeaponDef` +0x71c, 0x456838). VERIFIED
  (the code). So a crouched run reads standing and any walk sighted.
  The movetype bits are the state blocks whose legs or both lines name
  the anim (`BG_ParseCommands`), INFERRED.
- The only legs anims the `fireweapon` event puts on are the throws'
  `both pb_stand_grenade_throw` (`movetype idle`) and `both
  pb_crouch_grenade_throw` (`movetype idlecr`), and no state block names
  either. VERIFIED (`mp/playeranim.script`, `common_mp`). So rounds fired
  while a throw's legs play read standing, whatever the stance. INFERRED.
  The script's events hold nothing between standing and crouched (only
  `prone_to_crouch` and `crouch_to_prone`), VERIFIED (the rawfile), so a
  body that crouches inside the throw's legs timer keeps
  `pb_stand_grenade_throw` under `eFlags` 4 (the timer rule,
  cod4-player-anims.md 7). INFERRED.
- A turret's (`eType` 11, 0x456a12) rounds take its def's +0x7b0 at
  both ends with scale 1 (0x456a22..0x456a4e); a vehicle's or
  helicopter's (12, 14) `adsSpread` (0x456ad9..0x456b63). VERIFIED (the
  code). The seed is still our own
  playerstate's `commandTime`, so his rounds' disc points are not the
  server's. INFERRED.
- The round's trace is `CG_LocationalTrace` under 0x2806831
  (`BulletTrace`, 0x4558fe, call 0x459ef0); a contact on a player or a
  corpse with no surface flags reads flesh, 0x700000 (0x4559ae..0x4559c4).
  VERIFIED (the code).
- A player is traced as his packed `solid` box: 1.7's
  `CG_LocationalTraceDObj` (0x459250) returns no DObj for an entity
  whose `solid` (entity +0x98) is set, and a DObj only for types 6, 14
  and 12 (script mover, helicopter, vehicle). VERIFIED (the code). The
  box is the packed `solid` a unit inside on every side
  (`CG_GetEntityBModelBounds`), INFERRED.
- A script mover with no `solid` (every script model, a destructible
  car among them) is traced by its DObj's collision, as the server's
  round is (cod4-combat.md 13.1). INFERRED (0x459250's type test above,
  `CG_LocationalTrace`). vcod's client traces its
  model's collision surfaces (`SolidKind::Model`, `Cod4World::model_coll`)
  at its wire origin and angles, the parts its `partBits` hide skipped;
  its attachments and the helicopter's and vehicles' DObjs are left out.
  `cod4_live_own_rounds_stop_on_a_car_where_the_servers_do` fires our
  M16 at an mp_crash sedan and holds each own round's stop within 0.5
  units of the server's trace of the same line (the wire's whole-unit
  origin, as retail's, is the rest); without it the rounds went through
  the car. VERIFIED (the test).
- A contact on a player plays nothing on the client: the call to
  `CG_BulletHitEvent` goes through 0x455a20, which returns when
  `sv_clientSideBullets` is on (`cmp byte [dvar+0xc], 0`, 0x455a26) and
  the hit entity's type is 1 (`cmp [cent+0xd0], 1`, 0x455a3c). VERIFIED
  (the code). The flesh hit and the blood are the server's
  `EV_BULLET_HIT` with `surfType` 7 off `finishPlayerDamage`
  (`cod4-combat.md`), sent to all but the victim.
- vcod: `vcod_cod4::bullet` holds the spread and the disc for both
  sides (`Bullet_Fire` on vcod-server, `cod4_fx::client_bullets` on the
  client). The own fire event carries `cod4_fx::Aim::own` off the
  predicted playerstate and last frame's `aimSpreadScale`; another's
  `Aim::other` off his legs anim's movetype bits
  (`vcod_cod4::pmove::anim::movetype_mask`, `LegsPose`) and speed. A
  session with no anim set (no `BodySource`) reads his `eFlags` stance
  instead, never sighted. The rounds meet
  the last scene's solid entities but our own (`Cod4Session::shot_solids`,
  the prediction's boxes and brush models); a round stopped by a player
  draws no impact, sound or exit there, another's no tracer or whizby.
- The seed is the time of the cmd whose step raised the fire event
  (`Predicted::event_times`), not the frame's last cmd: vcod steps a
  provisional frame cmd past the sent ones, which the server never runs.
- Live on vcod-server (`war`, mp_crash): our M16's bursts at an axis bot
  held still 200 units ahead (the only bot; his cmds carry no move and no
  button, and he is healed every frame, since two head hits kill a body
  standing still) stop on his box and play nothing there; every round's
  seed is the time of a server round, and it leaves within 0.05 degrees
  of that round's disc point scaled to the client's own spread, along
  the server's round itself where the client's last-frame
  `aimSpreadScale` and the server's last end frame's agree. They part
  when no server frame runs between two rounds' cmds: the second round
  of an M16 burst read 3 degrees of spread on the server against the
  client's 4.6. The own round leaves along the view of the frame whose
  prediction fired it; paired with the next frame's view it read 0.1
  degrees off by the recoil. The server's `EV_BULLET_HIT` flesh events land
  within 24 units of where our rounds stopped and play the flesh hit.
  VERIFIED (the gate
  `play::cod4::tests::cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds`).
- That gate's missing first round (server 11016, 11082, 11150; client
  11082, 11150) was the harness, not the prediction: its spawn and
  settle loops ran no scene, so no ring was kept (`ps_ring` `None`), and
  the first scene, on the burst's first frame, took the predicted ring
  with that round in it as its baseline. `walk_snapshot` seeds a
  predicted view's missing ring from the snapshot's, as
  `CG_PredictPlayerState` compares against the `predictedPlayerState`
  it kept, which a client that predicts every frame never lacks.
  INFERRED. The window runs a
  scene every frame, so live play lost no round to this. VERIFIED (gate
  `play::cod4::tests::cod4_live_every_server_round_fires_once`: in the
  window's frame order, the M16's bursts, the MP5 held and the M40A3's
  single shots, hip and sighted, fire one predicted event per round the
  server fires, seeded with its time: 12, 16 and 4 rounds, no miss and
  no double).
- Live on vcod-server, a shooting axis bot pinned 200 units ahead:
  every legs anim he fires in names a movetype, a standing one reads
  neither crouched nor prone, and each round's spread lies between that
  stance's ends. In one run, of 12 rounds 4 in `idlecr` read crouched, 2 in
  `runcr` on a crouched body (`eFlags` 4) read standing, as 1.7's mask
  reads them. VERIFIED (the gate
  `play::cod4::tests::cod4_live_a_bots_rounds_spread_by_his_legs_anim`).
- Left out: vehicles' client-side rounds and the exit effect 1.7's
  `CG_BulletHitEvent` adds at a player target (flag 4 along the muzzle
  line) on the server's event.

### 12.3 Turrets' rounds, tracer starts and rolls

Addresses are `iw3mp.exe` 1.7.

- A gun's round (`DrawBulletImpacts`' `eType` 11 arm, 0x456a12..0x456aaf):
  `playerSpread` (`WeaponDef` +0x7b0, the file's +1968 that vcod-server
  reads) at both ends of the spread and a scale of 1, the gun's DObj
  (0x435a50) and its `tag_flash` (0x4a4be0, an origin and three axes):
  the round starts at the tag and leaves along the tag's own forward
  axis, no `AngleVectors` on that path. VERIFIED (the code). It shares
  the loop's seed, our playerstate's `commandTime` plus the round
  (0x456ca2), so two gun rounds of one snapshot leave along one disc
  point. VERIFIED (the code).
- The tracer byte the penetration walk (0x455a80) is handed is set only
  on the own arm: `rand() < cg_firstPersonTracerChance * 32768`
  (0x4567f2..0x456816, the dvar at 0x8c6358), rolled once before the
  rounds' loop (0x456c2b..0x456cee), so a shotgun's pellets all draw one
  or none; the other player's, the gun's and the vehicles' arms clear it
  (0x4569f2, 0x456a88, 0x456b3c). With it set each round spawns one
  (`CG_SpawnTracer`, 0x458110, its back-date `rand() % frametime / 2`
  per call at 0x45818e) from the muzzle to its first contact, or to its
  end on a miss (0x455b08, 0x455b63). VERIFIED (the code).
- Everyone else's tracer comes from `CG_BulletHitEvent` (0x458a90), which
  ends in the trajectory effects (0x458790, called at 0x458bcd): the
  start off 0x4585f0 (the shooter's DObj `tag_flash`, else his origin
  plus 11 prone, 40 crouched or 60 standing by `eFlags` 8 and 4, with a
  console error for a client), then the tracer test (0x458720):
  `cg_tracerchance` (0x8c62dc) above 0, not our own shot, not the gun
  we man (0x459040: our `eFlags & 0x300`, `viewlocked`, and
  `viewlocked_entNum` the shooter, 0x458752), then
  `rand() < cg_tracerchance * 32768`. VERIFIED (the code).
  The penetration walk calls the hit event (through 0x455a20, which
  returns on a player target under `sv_clientSideBullets`) for the
  first contact (0x455c0d) and for each penetration exit and entry
  (0x4560f8, 0x456154), so another's round rolls a tracer on each
  contact it plays, to that contact, and none on a miss. VERIFIED (the
  code).
- The own arm's tracer start is the view weapon's `tag_flash` (0x4a4be0)
  unless `cg.renderingThirdPerson` (0x794470) is set, which 0x451d70
  sets from `cg_thirdPerson` (the dvar at 0x748638, registered at
  0x43cdbf), a `pm_type` of 7 and up, and two client-info and entity
  tests (0x451da7..0x451deb): then the eye plus 30 (the double at
  0x70b438) along the client info's angles (+0x3e4, the same angles the
  rounds then leave along) (0x456799..0x4567eb). VERIFIED (the code).
  The flag's tests are cod4-client-view.md 16.
- vcod: a gun's fire event takes `cod4_fx::Aim::turret` (the weapon's
  `player_spread`, our `commandTime`); the effects skip the tracers of
  the gun we man; `Cod4Effects::tracers` rolls the own shot once and
  another's round on each contact. Live on vcod-server (`war`,
  mp_backlot, the crouch SAW 393 manned, view held): 20 client rounds
  for the server's 21 (the last still on the wire), two of one snapshot
  sharing a seed, each 0.5 degrees of `playerSpread` off `tag_flash`'s
  axis by its seed's disc point within 0.01 degrees, the axis 0.006
  degrees off the server's view, no tracer. VERIFIED (the gate
  `cod4_client::saw_tests::cod4_live_saw_gunner_view`). Our
  demolitions shotgun on mp_crash: every pellet within 0.05 degrees of
  the server's, 8 tracers or none a shot. VERIFIED (the gate
  `play::cod4::tests::cod4_live_own_shotgun_pellets_share_one_tracer_roll`).
- vcod: under `cg.renderingThirdPerson` (`cg_thirdPerson` or a dead
  playerstate, cod4-client-view.md 16) the own shot's tracer starts 30
  along the rounds' axis from the shot's eye (`cod4_fx::client_bullets`,
  `OwnView::third_person`), else at the view weapon's `tag_flash`. Every
  own round of the kill capture's shooter on mp_crash, drawn again in
  third person, keeps its start and end and starts its tracer 30 from
  the eye within 15 degrees of its way. VERIFIED (the gate
  `play::cod4::tests::own_shots_strike_mark_and_trace`).

## 13. The effect and weapon sets' colour

Which pixel shaders write their colour times their alpha, read off the
draw technique's bytecode (`--example techset_shaders common_mp <set>`,
`tools/re/d3d9dis.py`), each the first of techniques 7, 4 and 5 the
material names a state for. VERIFIED (bytecode):

| shader | sets (`common_mp`) | last colour write |
|---|---|---|
| `vertcol_simple_add_fog.hlsl`, `vertcol_simple_add.hlsl` | `effect_add*`, `effect_falloff_add_nofog`, `mc_unlit_add` | `oC0.xyz = r0.w * r0` |
| `zfeather_add.hlsl`, `zfeather_add_nf.hlsl` | `effect_zfeather[_falloff]_add*` | colour times the feathered alpha it writes |
| `particle_cloud_add.hlsl`, `particle_cloud_outdoor_add.hlsl` | `particle_cloud[_outdoor]_add` | colour times the alpha it writes |
| `lp_b0c0*_sm3.hlsl`, `lm_b0c0*_sm3.hlsl` | the marks' `mc_l_sm_b0*`, `wc_l_sm_b0*` | colour map times vertex colour scaled by its alpha first |
| `vertcol_simple_fog.hlsl` | `effect`, `effect_falloff`, `mc_unlit`, `mc_effect` | the fog lerp, alpha apart |
| `vertcol_simple.hlsl` | `effect_nofog` (a stub in `common_mp`, its techniques in `code_post_gfx_mp`) | `oC0 = tex * vertex` |
| `zfeather.hlsl`, `zfeather_nf.hlsl` | `effect_zfeather[_nofog]` | the colour as is, the feather in the alpha |
| `particle_cloud.hlsl`, `particle_cloud_outdoor.hlsl` | `particle_cloud[_outdoor]` | the colour as is |

- The effects' own sets carry no `mc_` prefix (`effect_zfeather_add_nofog`)
  and spell `mc_effect`'s grammar: `vcod_cod4::techset::classify` reads
  them so, and a premultiplying set's One draws as SrcAlpha, as the
  world's do (cod4-world.md 8.1). Read as unclassified and not
  premultiplied, the fx pass draws every additive effect's (One, One),
  which reads no source alpha (`fx_stage_rgba`), and fogs the `_nofog`
  ones: an element's alpha curve never fades its sparks, glows or
  flames. INFERRED (vcod's fx pass). VERIFIED (the gate
  `weapon_view::weapon_and_effect_materials_premultiply_as_their_shaders`
  holds every blended weapon, effect and mark material's class to the
  table; a shader not in it fails).
- The ACOG reticle (`mc/mtl_weapon_acog_reticle`, set `mc_effect`) runs
  `vertcol_simple_fog.hlsl` under (One, InvSrcAlpha) with no depth
  write: the shader does not premultiply, its colour map does.
  `weapon_acog_reticle_col` (512x512 DXT5, `iw_04.iwd`) holds 262144
  texels of which 209 carry a colour more than 8 over their alpha (the
  DXT5 blocks' rounding) and none above 5 at alpha 0. VERIFIED (the
  texels decoded). vcod's view model pipelines draw a stage's own blend
  pair (`entity_material`), so it draws (One, OneMinusSrcAlpha) as
  retail. VERIFIED (the gate
  `renderer::cod4_census::cod4_weapon_and_effect_materials_draw_as_retail`).
- That gate holds `common_mp`'s 192 weapon model materials, through the
  entity pipelines, and its 101 effect element materials, through the
  fx pass, to their state bits and the table: the same but the lit
  model sets' light (cod4-world.md 10), the zfeather sets' feather, the
  three (InvDestColor, One) materials the fx pass draws as a plain add
  (`gfx_fire_ball_atlas_sa`, `gfx_fire_ball_atlas_z60`,
  `gfx_spotlight_beam`); the distortion elements draw through the scene
  copy (17). VERIFIED (the gate).

## 14. The depth feather (`zfeather`)

- The `effect_zfeather*` sets (`common_mp`'s smoke, dust, mist, fire
  and water sprites: `gfx_smk_white_atlas`, `gfx_smokepuff_atlas`,
  `gfx_fog_wisp_afm` and kin under `effect_zfeather`, all drawn
  (SrcAlpha, InvSrcAlpha) with alpha test `gt0`) draw technique 4
  through `zfeather_dtex.hlsl` and `zfeather.hlsl`. VERIFIED (`--example
  techset_shaders common_mp effect_zfeather`, bytecode).
  - The vertex shader writes `clip.w * featherParms.x` and folds
    `saturate(clip.w * featherParms.x)` into the vertex alpha: the near
    fade. The pixel shader reads `floatZ` at the screen position and
    multiplies the alpha by `saturate(|abs(floatZ) * featherParms.x -
    clip.w * featherParms.x|)`. The depths are the view depth itself,
    not CoD2's `0.9995 (w - 4)` clip z (cod2-materials.md 9.5).
  - `featherParms` is a material constant (the argument's name hash
    1300144692 on both shaders), so each material carries its own scale.
- `r_zFeather` (iw3mp.exe 0x62b970, default 1, cod4-front-end.md 9.2).
  VERIFIED. Off, the technique set is remapped to the name without its
  `zfeather` token (`Material_GetRemappedFeatures_RunTime`, the feature
  mask 1), the plain `effect*` set, with neither fade.
  INFERRED.
- vcod: `vcod_cod4::materials::compile` gives a `zfeather` set
  `Material::feather` (`featherParms.x`) and `feather_view_depth`; the fx
  pass draws its quads with `FX_VERT_FEATHER_W`, reading the scene's
  linear depth copy, taken at the frame's split after the opaque
  entities and the gun, which 1.7's `floatZ` holds
  (cod4-client-perf.md 20); CoD2's copy goes before the first model. With
  `r_zFeather` 0 the quads carry no feather. The world and static
  model `zfeather` sets draw through `cod4_worldfx` (cod4-world.md 9.5).
  `cod4_live_sight_blurs_and_smoke_feathers` stands
  `gfx_smk_white_atlas` half in mp_crash's ground on the live path and
  checks the feathered sprite sits nearer the frame without it than the
  hard one. VERIFIED (the test, ours).

## 15. Emitted effects

- An element whose `effectEmitted` (+224) names an effect plays it along
  its own move: each update `FX_ProcessEmitting` takes the element's
  move from where it stood to where it stands, draws `emitDist` once an
  element (`base + amplitude * fx_randomTable[seed + 20]`), adds
  `emitDistVariance.base`, and walks the move a spacing at a time, each
  spacing adding a fresh `rand()` share of `emitDistVariance.amplitude`;
  the effect plays at each point along the move's direction, and the
  units left over are kept as a byte, 256ths of the largest spacing
  (`emitResidual`). The element lives to its life's end even when it
  draws nothing. INFERRED (`FX_ProcessEmitting`,
  `FX_UpdateElement_HandleEmitting`).
- Any element type emits, not only a model: `common_mp`'s emitters are
  the large bullet impacts' flying chunks (`impacts/large_brick`'s
  one-shot model, `impacts/brickimpact_em2`, `emitDist` 4, variance
  1 + 0..11; `large_rock`, `large_plaster`, `large_dirt`,
  `large_gravel`, `small_brick` alike), the helicopter crash's debris
  (`explosions/aerial_explosion_large`'s looping model,
  `fire/fire_smoke_trail_m` every 40 units), `explosions/tanker_explosion`'s
  sprites (`tanker_explosion_child`, 50) and `muzzleflashes/at4_flash`'s
  (`at4_flash_fire_child`, 5); mp_citystreets' `props/barrel_ignite`
  emits `fire/flame_small_em`. Every emitted name is an effect of the
  same zone. VERIFIED (the scratch walk).
- The emission elements an effect counts last (section 1) are copies of
  its emitted effects' one-shot elements, made when the effect was
  converted; `FX_TriggerOneShot` and `FX_BeginLooping` cover the looping
  and one-shot ranges only, so no play of the effect spawns them, and
  the emitted effect plays its own. INFERRED (`FX_CopyEmittedElemDefs`). Each
  copy matches its emitted effect's one-shot count here
  (`impacts/brickimpact_em2`: 1 one-shot, the copy 1). VERIFIED (the
  scratch walk).
- vcod: an element carrying `effectEmitted` gets
  `efx::Emitter::cod4_emit`, and `fx::sim`'s `Emit4` walks each step's
  move as above (world space, a bolted element through its tag's frame),
  pushing the plays on the chains, which `main`'s frame starts after the
  step. The CoD2 `Emitter` cursor (`emitfx`, `density`) stays off for a
  CoD4 element. The emission copies are skipped and no longer count as a
  gap. Gates: `fx::sim::tests::a_cod4_element_emits_every_emit_distance_it_moves`,
  `cod4_fx::tests::brick_hit_chunks_emit_their_dust_along_their_flight`,
  and the live `cod4_live_hardpoints_draw_smoke_and_sound`, where
  vcod-server's helicopter crash debris plays `fire/fire_smoke_trail_m`
  (241 plays in one run). VERIFIED (the tests, ours). Whether the dust
  and smoke look as retail's is pending a human's look.

## 16. Trails

- `FxTrailDef` (+244, 0x1C): `scrollTimeMsec` 0, `repeatDist` 4,
  `splitDist` 8, the vertex count and vertices (0x14: `pos[2]`,
  `normal[2]`, `texCoord`) 12 and 16, the index count and indices
  (`u16`) 20 and 24. VERIFIED (the walk follows the slots
  block-exactly, `cod4-fastfile.md` 10); the field names INFERRED.
  `vcod_cod4::assets::fx::FxTrail` keeps them.
- `common_mp` holds five trails, all looping elements, none elsewhere:
  `smoke/smoke_geotrail_rpg` (the RPG's `projTrailEffect`; scroll
  -100000 ms, repeat 128, split 100, life 8000 ms, `gfx_smoke_trail02_godray`,
  four cuts: two crossed, two diagonal), `smoke/smoke_geotrail_m203`
  (every `gl_*_mp`'s; -25000, 256, 250, 2000 ms, two diagonal cuts),
  `smoke/smoke_geotrail_hellfire` (`cobra_ffar_mp`, `hind_ffar_mp`; 100
  split, four cuts), `fire/jet_afterburner` and `smoke/jet_contrail`
  (the airstrike's `playPlaneFx`, split 1000, two crossed cuts), the last
  four on `gfx_smoke_trail01`. Each cut is a pair of vertices, one index
  pair. VERIFIED (the scratch walk).
- The runtime: the effect's `distanceTraveled` grows by its frame's move
  each update, and a cross-section (`FxTrailElem`) drops at every
  multiple of `splitDist` it passes, at the frame lerped there and the
  time lerped likewise, with the element's delay and its own life;
  `FX_BeginLooping` drops two at distance 0 when the effect starts. A
  section's right and up are the effect frame's second and third axes
  then, and it moves by the element's velocity graph from there. The
  draw (`FX_DrawTrail`) turns each live section into the cut's vertices
  at `pos + pos[0] * size[0] * right + pos[1] * size[1] * up` (rotation
  aside) and each index pair into a quad between neighbouring sections,
  `u` the section's distance over `repeatDist`, less the first section's
  whole repeats, plus the scroll's fraction of `msecDraw` over
  `scrollTimeMsec`; the first section, when it is the trail's very first,
  has alpha 0, and a spent section ahead of the first live one gives the
  tail a point lerped to where its neighbour's life would end. INFERRED
  (`FX_SpawnTrailLoopingElems`, `FX_SpawnTrailElem_NoCull`,
  `FX_GetOriginForTrailElem`, `FX_DrawTrail`,
  `FX_GenTrail_VertsForSegment`, `FX_GenTrail_IndsForSegment`).
- The vertex's `texCoord` is packed in the low half and `u` in the high
  one (`FX_GenTrail_VertsForSegment`). INFERRED. The packed
  vertex's shaders read the high half as the first coordinate:
  `vertcol_simple_fog_fo_dtex.hlsl` (`effect_falloff`'s techniques 4-6,
  24, 25) unpacks `o2.x` from bytes 2 and 3 and `o2.y` from bytes 0 and
  1, as iw3mp.exe 0x5648e0 takes the first coordinate from the high
  word (cod4-world.md 9.1).
  VERIFIED (bytecode, `--example techset_shaders common_mp
  effect_falloff`, `tools/re/d3d9dis.py`). So a trail's texture runs
  its first coordinate along the trail and its second across the cut;
  vcod had them the other way round.
- `smoke_geotrail_rpg`'s material, `gfx_smoke_trail02_godray`, is an
  `effect_falloff` set. Its vertex shader turns the vertex normal into
  the world, takes `d = dot(normalize(pos - eye), n)` and
  `f = saturate(d * d * falloffParms.z + falloffParms.w)`, writes the
  alpha `f * vertex.a` and the colour `lerp(falloffEndColor,
  falloffBeginColor, f) * vertex.rgb`; the pixel shader is `effect`'s
  `vertcol_simple_fog`. VERIFIED (bytecode). The material's constants:
  `falloffParms` (0.005263158, -0.05263158, 11.517542, -0.34729624),
  `falloffBeginColor` (1, 1, 1, 1), `falloffEndColor` (1, 1, 1, 0.5).
  VERIFIED (the zone). A cut is gone below 10 degrees off edge-on and
  whole past 20, so the crossed sheets read as a tube, never a flat
  sheet. A trail vertex's normal is `normal[0] * left + normal[1] * up`
  of its section. INFERRED (`FX_GenTrail_VertsForSegment`).
- vcod: `cod4_fx::trail_falloff` reads a falloff set's three constants
  into `TrailDef::falloff`, and `Trail::quads` applies the shader's
  factor to each quad at its centre, off the mean of its four corners'
  normals (an `FxQuad` has one colour). Without it the RPG's trail draws
  its four cuts whole from every side, as wide flat sheets.
  Gates: `fx::cod4_trail::tests::a_falloff_ribbon_fades_edge_on`, and
  `rpg_trail_drops_a_section_every_split_distance_on_its_tag`, whose
  side view has its flat cut at alpha 0 and its upright one drawn.
  VERIFIED (the tests, ours). The look is pending a human's eye.
- `CG_Missile` plays the weapon's `projTrailEffect` bolted to the
  missile's `tag_fx` once an entity (the trail flag, cleared by
  `CG_ResetEntity`). INFERRED.
- vcod: a trail element converts as one play (count 1, its delay moved
  to the sections) whose `efx::Emitter::cod4_trail` makes the fx system
  run an `fx::cod4_trail::Trail` on the effect's tag; an effect on no tag
  never moves and draws no trail. `Trail::advance` drops the sections as
  above off the tag's pose each step, and `Trail::quads` adds the quads,
  one colour each (the two sections' mean: `FxQuad` carries no vertex
  colours), to the frame's fx quads. A looping trail no longer gets a
  `Looper`. `Cod4Client::effects_frame` plays a drawn missile's
  `projTrailEffect` on its model's `tag_fx` once a missile entity.
  Gates: `fx::cod4_trail::tests::a_moving_trail_drops_a_section_every_split_distance`,
  `cod4_fx::tests::rpg_trail_drops_a_section_every_split_distance_on_its_tag`
  (the RPG's trail on a tag flying 1003 units a second: 12 sections,
  eight quads a segment, `u` 100/128 a section, gone 8 s after the tag),
  and the live `cod4_live_hardpoints_draw_smoke_and_sound`, where our
  M16's launcher grenade, fired on vcod-server, draws
  `smoke/smoke_geotrail_m203`'s quads (8 at most in a frame: the grenade
  bursts some 400 units out, a section or two at a 250 split). VERIFIED
  (the tests, ours). The look is pending a human's eye.
- The airstrike's jets draw no trail in vcod: `playPlaneFx` runs
  `playFxOnTag` in the frame `spawnPlane` made the plane, so the events
  ride the plane's entering snapshot, and `CG_ResetEntity`'s
  `ET_PLANE` arm sets `previousEventSequence` to the entity's
  `eventSequence`, which skips them. INFERRED (`_hardpoints.gsc`
  `doPlaneStrike`). In the live
  run no `fire/jet_afterburner` or `smoke/jet_contrail` played. VERIFIED
  (the test's effect list, ours). Whether 1.7's arm is the same is not
  measured; a retail airstrike capture would settle it. `iw3mp.exe`
  registers `fire/jet_afterburner` (string 0x6cc0ac, `FX_Register` at
  0x43f11b, stored at 0x84f2a8) and nothing reads that slot.
  VERIFIED (the disassembly: the store is the address's only
  reference).

## 17. Distortion (`distortion_scale_zfeather`)

What the heat haze of explosions, fires and exhausts draws: a sprite
that bends a copy of the frame behind it.

- `common_mp`'s distortion materials, all on `distortion_scale_zfeather`:
  `gfx_distortion_heat` (`distortionScale` (5, 5)),
  `gfx_distortion_heat_far` (10, 10), `gfx_distortion_ring_light`
  (15, 15) and `gfx_distortion_ring_heavy` (75, 75); techniques 4 and 5
  carry one state, (SrcAlpha, InvSrcAlpha), alpha test `gt0`, no depth
  write, depth test less-equal. VERIFIED (`--example techset_shaders
  common_mp distortion_scale_zfeather`, which prints both techniques'
  states and the constants). The constant's hash is `R_HashString`'s
  0xF37B6913. VERIFIED (the zone's argument; the materials' unit test).
- Technique 5 (emissive, `distortion_scale_zfeather_dtex.hlsl` and
  `distortion_zfeather.hlsl`) binds the material's colour map to the
  pixel shader's `normalMapSampler` (s4), code sampler 10 to
  `colorMapSampler` (s0) and 18 to `floatZSampler` (s5), code constants
  0x33 and 0x34 (`clipSpaceLookupScale`, `clipSpaceLookupOffset`) and the
  world view projection. VERIFIED (the pass's arguments). The names sit
  in the shaders' constant tables. VERIFIED. That code sampler 10 is the
  resolved scene (`resolvedPostSun`) is INFERRED from the
  shader's screen lookup into it (cod4-world.md 3.5 shows a sampler
  number whose name does not hold on 1.7, the spot's shadow map).
- The vertex shader: the screen lookup `o5 = (clip.x lsx + clip.w lox,
  clip.y lsy' + clip.w loy, 0, clip.w)` with `lsy' = -lsy`, `lookupScale`
  carrying the minus as its second component; the tangent (texcoord 1)
  and the binormal `tangent x normal`, each turned by the matrix without
  its translation, give `o3 = ds.x * v.r * (t.x lsx, t.y lsy, 0, t.w)`
  and `o4 = -ds.y * v.g * (b.x lsx, b.y lsy, 0, b.w)` (`ds` the
  `distortionScale`, `v` the vertex colour, both y terms with `+lsy`);
  the colour out is `v.bbba`, and the clip z rides out on its own.
  VERIFIED (bytecode).
- The pixel shader: `n = 2 * map.rg - 1` at the texcoord, the bent
  lookup `o5 + n.x o3 + n.y o4` and the plain one `o5`, both `texldp`
  (projected: control bits 0x01 on the opcode token); the copy's colour
  at the bent point where `|floatZ| - clip z >= 0` there, else the copy
  at the plain point; out the colour times `v.b`, the alpha the map's
  times `v.a`. The plain `distortion.hlsl` (`distortion_scale`, no
  material of `common_mp` on it) takes the map unscaled and no depth
  test. VERIFIED (bytecode).
- `r_distortion` (name 0x706cd0, description "Enable distortion" at
  0x706cbc) registers at 0x62c59f through `Dvar_RegisterBool` (0x56c350)
  with `ebx` as both its default and flags, as `r_zFeather`'s
  registration (0x62b96d) does: default 1, archived. VERIFIED (the
  disassembly; `r_zFeather`'s default in cod4-front-end.md 9.2).
  Retail resolves the scene into `resolvedPostSun` after the lit pass
  (`R_DrawLights`) and before the emissive one when it is on and one
  local client plays, and skips a technique that needs the copy when it
  is off. INFERRED (`RB_Draw3DInternal`,
  `R_SetupMaterial`).

vcod: `vcod_cod4::materials::compile` keeps an effect set's stage and
gives it `Material::distortion`; `cod4_fx` no longer leaves such an
element out. The fx pass flags its quads `FX_VERT_DISTORT` (the scale in
the vertex's feather and size) and draws them through
`fs_distort` over `crate::scene_copy`, the colour resolved at the
frame's split (after the opaque and decal draws), and the soft
particles' linear depth as `floatZ`. The tangent is the quad's u axis
off the screen derivatives, the normal faces the eye, and the clip z is
`FEATHER_K (w - 4)`; D3D9's half texel offset in `lookupOffset` is left
out. `r_distortion` 0 draws none. The world set
`wc_distortion_scale_zfeather` (mp_overgrown's `wc/office_brickglass`)
draws over the same copy (cod4-world.md 9.5). To see it, `sv_cheats 1`
and `setviewpos -525 77 -116 -90 -5` on mp_overgrown face the window of
glass bricks from inside the building, on the floor (eye 60 units over
it), `r_distortion 0` against 1 moving about 50000 of 245760 pixels.
`setviewpos -406 -95 -108 163 -53`, the spot given before (and section
13 of cod4-client-perf.md's), is 68 units over the floor and looks 53
degrees up: the player drops 8 units and sees a sliver of the same
window from below (4556 pixels). VERIFIED (our lavapipe frames;
`cod4_live_overgrown_glass_spot_shows_the_bricks` sends the command on
the live session). Not done: vcod's split
copies the unlit opaque surfaces too, which retail draws in the emissive pass
after its copy; the fx tangent is INFERRED (the sprite's own frame is
not read). `cod4_live_grenade_haze_bends_the_scene` throws a frag on
the live path and checks its explode's haze bends the frame against
`r_distortion` 0. VERIFIED (the test, ours). Cost: `tools/cod4_perf.sh`
on mp_crash's `war` (the spectator spot, 30 s, lavapipe, 1280x772, 4x
MSAA) read GPU scene 227 ms with `r_distortion` 1 and 249 ms with 0, the
same triangles: the copy and its draws sit inside the run-to-run noise
there (the bots move). VERIFIED (our runs); a real GPU is not measured.

## 18. What 1.7 culls and fades, from one view

At mp_crash's intact red sedan (`setviewpos -279 1396 233 -15 28`)
retail draws no smoke, and vcod drew mp_crash's createfx smoke, mist and
fire over the town (cod4-client-perf.md 20).
What 1.7 does with an effect element between its spawn and its pixels,
read off iw3mp.exe:

- Dvars, registered in one function (0x499390..0x4997ef, `Dvar_RegisterNew`
  0x56c350): `fx_enable` (pointer at 0x110ce88, default 1), `fx_draw`
  (0x110ceac, 1), `fx_cull_elem_spawn` (0x110ceb4, 1),
  `fx_cull_elem_draw` (0x110ce90, 1), `fx_cull_effect_spawn` (0x110ce8c,
  **0**), `fx_visMinTraceDist` (0x110cea0, 80, the float at 0x70b568)
  and `fx_drawClouds` (0x110cea4, 1). VERIFIED (the pushes and stores).
- The spawn. `FX_SpawnElement` (0x4a25b0) asks `FX_CullElemForSpawn`
  (0x4a0b90) while `fx_cull_elem_spawn` is on, with the effect frame's
  origin (frame + 0x10) and the fx camera (system + 0xb0). VERIFIED (the
  arguments). A cull drops that spawn: the range test of section 5,
  then, with flag 0x4, `FX_CullSphere` (0x4a6290) of that origin at
  `spawnFrustumCullRadius` (+0x24) against the camera's
  `frustumPlaneCount` (camera + 0x94) planes. VERIFIED (the
  instructions). `FX_CullSphere` culls when `dot(normal, p) - dist <=
  -radius` for any plane, the planes 16 bytes apart from camera + 0x10.
  VERIFIED. The planes: the view's forward through the eye, then the
  four sides off `tanHalfFovX` and `tanHalfFovY`, and a sixth at the far
  plane when `R_GetFarPlaneDist` is positive (`FX_SetNextUpdateCamera`).
  INFERRED.
- `fx_cull_effect_spawn` (0x4a0c10, read at 0x4a115c) culls a whole
  effect only when every looping and one-shot element culls; off by
  default. VERIFIED (the loop and the default).
- The draw. Each element's setup calls `FX_EvaluateDistanceFade`
  (0x498010, from 0x49934f): with `fadeInRange` (+0x14) or `fadeOutRange`
  (+0x1c) amplitude set, the camera distance through `FX_ClampRangeLerp`
  (0x497fb0: 1 inside the base, 0 past base plus amplitude, linear
  between), the fade in's value and one less the fade out's, the smaller
  of the two times 255 plus 0.5 (the doubles at 0x70aee0 and 0x70ace8)
  stored truncated at draw state + 0x9c. VERIFIED (the instructions).
  `FX_EvaluateVisualState` scales the alpha byte by it, `alpha * fade
  >> 8` (0x496d77). VERIFIED. Unset, the fade stays 255
  (`FX_SetupVisualState`), so every element's alpha loses its last
  1/256. INFERRED.
- The sprite's draw cull (0x497af0, `fx_cull_elem_draw`): a sphere of
  the larger of its two sizes at its position against the camera's
  planes, five only with flag 0x400. VERIFIED. The GPU clips what it
  culls, bar the far plane's sixth. Neither it nor the spawn reads the
  world's cells or portals, and the sprites go to the emissive code mesh
  list (`R_AddCodeMeshDrawSurf`) with no cell test: an effect
  behind a wall draws, depth tested. INFERRED.
- `fx_visMinTraceDist` is read only at 0x4a2b65, in a query (0x4a2b20)
  that returns 1 for a trace under 80 units and otherwise walks a list
  of vis blockers, which flag 0x1000 elements add at their draw
  (0x499357). VERIFIED (the reads and the early return). Its three
  callers (0x4038af, 0x42de13, 0x42e09d) lie outside the effect spawn
  and draw code, so it fades nothing an effect draws. INFERRED.

vcod runs the range test, the frustum test
(`fx::sim::Frustum`, set each frame by `Cod4Client::pump` from the eye
and the last drawn view's fov and aspect; CoD2's frame sets none) and
the distance fade (`fx::cod2::Live::distance_fade` on the quad's and
the cloud's alpha, from `vcod_cod2::efx::Primitive::fade_ranges`); not
the sixth plane, nor the 1/256 every unfaded element loses.

At the sedan spot (`--example fx_ranges -- mp_crash -279 1396 233 -15
28`, cg_fov 65 at 16:9; VERIFIED, the zone and the example):

| effect, distance | element | 1.7 |
|---|---|---|
| `fire/firelp_small_pm`, 1182 | 2, `gfx_smk_white_atlas`, fade in 1000 + 200 | spawns, alpha 23/255 |
| `fire/firelp_small_pm`, 1330 and 1398 | 2 | past its 1250 range |
| `dust/dust_wind_slow_yel_loop`, 1600 | 1, `gfx_smk_white_atlas`, flag 0x4, radius 250 | outside the frustum |
| `smoke/battlefield_smokebank_S`, 1253 and 1438 | 0, fog wisps, fade in 2000 + 500 | spawns, alpha whole |

So the smoke column over the town is the nearest fire's element 2,
which 1.7 draws at 9% and vcod, with the range test alone, drew whole. Everything else vcod draws
there 1.7 draws too: the dust spirals and hallway smoke (no fades, no
0x4), the fires' flares and flames. `cod4_fx::tests::
crash_fire_smoke_fades_with_distance_and_spawns_in_view` runs mp_crash's
ambience from that view for 10 s: the fire smoke draws under a fifth of
its alpha (at most 0.036 over 5 sprites), and none with the view turned
180 degrees. VERIFIED (the test, ours; it fails with the fade left
out). `renderer::cod4_worldfx_live::cod4_live_crash_sedan_spot_fades_the_fire_smoke`
holds the same on the live session, a `setviewpos` to the spot turned to
yaw -56 (the fire 1177 units off; five smoke sprites at alpha 0.001 to
0.040). VERIFIED (the test, ours). At the first view's yaw -15 that fire
sits on the frustum's edge: inside at 16:9 and cg_fov 65 (41 degrees
off the view against a half fov of 40.4 and the radius's 3.9), outside
at the live harness's 5:3. The setviewpos drops the player to the
ground there, the eye at z 234, not 293. VERIFIED (ours).

