# `phys/`: retail 1.7's ODE worlds 0 and 1

Each file is one world of the Windows client `iw3mp.exe` 1.7 under Wine and
gdb, dumped by `tools/re/cod4_phys_gdb.py` from the first body of the world
after a map load (so the reorder seed starts at 0x89abcdef) to the step
the last body slept. Recipe: docs/dev/retail.md, "Worlds 0 and 1 from the
first push to rest"; the facts read off them: `cod4-gsc.md` 3.6.2; the
port run against them step by step: `phys_ab` (3.6.7).

Every take ran under `tools/cod4_wine/run.sh` with

```
+set sv_timeout 600 +set cl_timeout 600 +set cl_connectTimeout 600
+set sv_connectTimeout 600 +set scr_game_playerwaittime 1
+set scr_game_matchstarttime 1 +devmap <map> +set fixedtime 17
```

and the driver's defaults (class `specops_mp,0`, `god`, the action at
frame 700, contacts on steps 1-40), and was cut by
`python3 -I tools/re/cod4_phys_summary.py DUMP --trim OUT [--steps N]
[--every K]`.

| File | Map | World | `PHYS_*` | Scenario |
|---|---|---|---|---|
| `round-mp_crash-1.7.txt` | mp_crash | 0 | `SCENARIO=round POS="1035 816.1 198.7 90" PITCH=46.4` | one MP5 round into the cinderblock `bog_b_cinderblock_clutter` at (1035, 876.1, 131.7), 60 units off |
| `frag-mp_crash-1.7.txt` | mp_crash | 0 | `SCENARIO=frag POS="460 125 196 0" PITCH=85` | a frag held 30 frames and dropped at the feet beside the plastic crate at (496.6, 125.1, 138.6) |
| `car-mp_backlot-1.7.txt` | mp_backlot | 1 | `SCENARIO=burst FIRE=120 POS="466.4 -425.4 128.1 169" PITCH=15 QUIET=600 STEPS=2500 CONTACTS_FROM=launch` | 120 frames of MP5 fire into the side of the first sedan, `vehicle_80s_sedan1_silv` at (368.2, -406.3, 68), until `_destructible.gsc` blows it and launches its parts |
| `watermelon-mp_bog-1.7.txt` | mp_bog | 1 | `SCENARIO=round POS="3624.6 -32.2 52 0" PITCH=10` | one round into the watermelon `me_fruit_watermelon_round` at (3672.6, -32.2, 43.5); its `props/watermelon` effect's chunks. Cut at step 170 (`--steps 170`): one chunk was still rolling at 4 u/s and 2.2 rad/s at step 600, where the take stopped |
| `shards-mp_crash-1.7.txt` | mp_crash | 0 | the frag's, with `CONTACTS=1-200 CONTACT_IDS="4 5" PLACE="4=W;5=W"` | the frag take's throw (2026-11-27); its two barrel shards (bodies 4 and 5) put 24 units over mp_crash's step edge at (308, -1458, 101.9) before their first step, at rest, turned as `part_edges`' 45 degree drop there (`W` the 19 words of the `# place` lines). Cut at step 110 (`--steps 110`): the can slept on step 462, the bottle not by step 600 |
| `frag-mp_shipment-1.7.txt` | mp_shipment | 0 | `SCENARIO=frag POS="-576 -672 256.125 90" PITCH=85 CONTACTS=0-0 STEPS=2400` | a frag dropped at the feet beside the bottles and fruit on mp_shipment's floor (2026-11-27). Body lines every tenth step and on each body's first and first asleep step (`--every 10`) |

The summary of each, as `cod4_phys_summary.py` reads it:

| File | Bodies (creator, mass, geom class) | Steps | `msecStep` | Asleep at step | Raw contacts (steps 1-40) | Impact sounds |
|---|---|---|---|---|---|---|
| round | 1 dynent, 20, box (1) | 61 | 17 every step | 61 | 40 calls, 8 each; the pass got 6 | 0 |
| frag | 8 dynents: two crates 1 (1), a bottle and a can 0.1 (6 over 13), two barrel shards 6 (1), two tyres 18 (6 over 13) | 140 | 17 | 74 to 140 | 320 calls, 642 contacts, at most 14 | 23 |
| car | 9 launched: four bounds boxes 6 (1), two doors 25 and one part 6 as brushes (12), two tyres 18 (6 over 13) | 132 | 17 | 89 to 132 | 360 calls, 1017 contacts, at most 24 | 37 |
| watermelon | 6 FX models, 1 each (1) | 170 (cut) | 17 | 95 to 165, one never | 240 calls, 682 contacts, at most 12 | 6 |
| shards | the frag's eight | 110 (cut) | 17 | 78 to 108, the bottle and the can past the cut | the shards' alone, steps 1-200: 168 calls, 911 contacts, at most 12 | 16 |
| shipment | 11 dynents: five bottles and a can 0.1, five fruit 0.5 (6 over 13) | 1212 | 17 | 103 to 1212 | none dumped | 0 |

## Lines

Every float or pointer is its 32 bits in hex (`f32::from_bits`); units are
the game's, seconds and radians. Body ids count up from 1 in the order the
dump first met each body.

- `# ...`: the driver's header, the console commands it sent (`# frame F
  time T: CMD`, `T` the dumped world's clock), `# pitch P`, `# frame F
  round leg` (a round reached `DynEntCl_EntityImpactEvent`, 0x48e0f0) and
  `# place ID W0..W18` (body ID put at pos, q and R before its `new`
  line, its velocities and accumulators zeroed, `PHYS_PLACE`).
- `made ID ADDR CREATOR`: a body of this world from `Phys_ObjCreate`
  (0x5a4fb0), by its call's return address: `launch` 0x434d4e, `dynent`
  0x48dcc8, `jitter` 0x48ed9f, `pieces` 0x490fda, `fxmodel` 0x4a21be.
- `push STEP ID CALLER px py pz | fx fy fz | MSEC`: `Phys_ObjAddForce`
  (0x5a5900) on this world: the step count so far, the body, the return
  address (0x48eaa6 a blast, 0x5a5db4 `Phys_ObjBulletImpact`), the point,
  the force before the `1000 / msecStep` scale and the world's `msecStep`.
- `world W ADDR seed S msec_step M time T nb N nj J gravity gx gy gz`: at
  `Phys_RunToTime` (0x5a6d50) entry with the first body in the list: the
  `dxWorld`, its seed (+0x30), `msecStep` (0x71e798 + 4 W), the time the
  call runs to, the body and joint counts (+8, +0xc) and gravity (+0x10).
- `frame F time T bodies N nb N`: a `Phys_RunToTime` entry that found new
  bodies in the world's list (+0 first body, body +4 next).
- `new ID ADDR CREATOR world_ok K flags F | mass 17 | inv 13 | force 3 |
  torque 3 | idle 4 | pos 3 | q 4 | R 12 | lvel 3 | avel 3`: a body before
  its first step. `world_ok` 1 when the body's +0 is the world. Its flags
  (+0x18), `dMass` (+0x20: mass, `c[4]`, `I[12]`), +0x64..+0x97 (the
  inverse inertia and inverse mass), the force and torque accumulators
  (+0x98, +0xa8), +0xd0..+0xdc (the auto-disable time and steps, then
  what is left of them), position (+0xe0), quaternion (+0xf0, w first),
  rotation (+0x100, 3x4), linear (+0x130) and angular (+0x140) velocity.
- `user ID W0..W19`: the body's userdata (+0xc), 0x50 bytes: +0 the
  body's offset from the model's origin, +0xc the body, +0x10 the
  position and +0x1c the rotation copied each frame, +0x40 the impact
  sound row, +0x44 friction, +0x48 bounce, +0x4c the contact pass's state.
- `geom ID ADDR | W0..`: each geom on the body (body +0x1c, then geom
  +0x14), 0xc0 bytes (0x70 in the round take): +0 vtable, +4 class, +8
  flags, +0x10 body, +0x14 next, +0x18 pos*, +0x1c R*, +0x2c bounds,
  +0x44 and +0x48 category and collide bits, +0x4c on the class's fields
  (box: the side lengths; brush: its `BrushWrapper`; class 13: axis,
  radius, half height; class 6: the inner geom, then its own frame).
- `inner ADDR | W0..` and `inner_pose pos 3 | R 12`: a class 6 geom's
  inner geom and what its pos and R pointers hold.
- `brush W0..W19`: a class 12 geom's `BrushWrapper`, 0x50 bytes.
- `step N msec M sim S to E seed X next M2 awake A of B`: after world W's
  `N`th step since the first body (0x5a6e69): its length, the world's clock
  after it, the time the frame runs it to, the seed after it, the next
  step's length and the bodies awake and in the list.
- `body ID FLAGS IDLE | px py pz | qw qx qy qz | vx vy vz | wx wy wz`: each
  body in the list after the step, as the ragdoll dumps have it (flags
  0x4 disabled, 0x10 auto-disable; +0xd8 the idle time left). A world 0
  body is destroyed the frame after it sleeps and leaves the list.
- `collide STEP ID1 ID2 CLASS1 CLASS2 N` then `N` lines `c W0..W11`: the
  near callback's `dCollide` (0x5a5f20, after the call at 0x5a5f96) on
  step `STEP`: the geoms' bodies (0 none: the world), their classes (15
  the world geom) and each `dContactGeom` (0x30: pos +0, normal +0x10,
  depth +0x20, the geoms +0x24 and +0x28, surface flags +0x2c).
- `cut STEP ID1 ID2 N | surface 11` then `N` `c` lines: the contact pass
  (0x5a2760) entry: the contacts it gets, after 0x5a2320's cut when the
  raw count is 5 or more, and the `dSurfaceParameters` it gets.
- `sound STEP HANDLE px py pz`: the impact sound (0x5a5f0e) with the
  contacts' mean point.
- `end`: the dump stopped (or was cut there).
