# vcod-cod4 test fixtures

Recordings of the retail CoD4 1.7 Linux server (`cod4_lnxded`) on loopback,
inside the namespace of `docs/research/cod4-handshake.md` 7, plus the gsc
probes that drove them; the `*-x21*` net captures are of the CoD4x 21.3
release server in `tools/cod4x_capture.sh`'s namespace, but for
`auth-x21.txt`, a public CoD4x server's side channel up to its stats
request (cod4x-protocol.md 19, 18.4). Each capture opens with a `#` header holding its
recipe; the parsers skip `#` lines (`vcod_cod4::net::capture::parse` for
`net/`, the gdb dump readers for `heli/` and `skel/`).

| Subdirectory | Recorded by | Read by |
|---|---|---|
| `clip` | Not a capture: `iw3mp.exe` 1.7's position test kernels run on generated inputs by `tools/re/x87emu.py`, written by `tools/re/cod4_postest_vectors.py` (cod4-collision.md 3.1); its header has the line format | `vcod_common::clipmap::pos4`'s test |
| `gsc` | Written for these runs: one probe gsc per capture, packed into a mod's iwd (`+set fs_game mods/vcod<x>`) or loose in its homepath, as each capture's header says | `crates/server/src/cod4/capture_ab.rs`, `hardpoint_ab.rs`, `planefx_ab.rs`, `shot_aim_ab.rs` and the client's CoD4 tests |
| `heli` | `cod4_lnxded` under gdb with the `heli_idle`, `hardpoint_flight` and `heli_yaw` probes (`cod4-gsc.md`, "The flight's float model") | `crates/server/src/game/cod4/vehicle/flight.rs` |
| `net` | `tools/cod4_handshake_probe.py` (`handshake-1.7*.txt`) and `cargo run -p vcod-cod4 --example cod4_probe` (`snapshots-1.7.txt`, `play-1.7-*.txt`): one `<ms> <c2s\|s2c\|auth> <peer> <hex>` line per datagram, raw; `play-1.7-*-hits.txt` are the server's `PROBE` log lines beside a pair; `download-1.7.txt` holds decoded lines from three `cod4_probe` download runs, whose raw datagrams carry a mod's file bytes and are not kept; `writeconfig-1.7.cfg` is the `players/vtest.cfg` the retail server's `writeconfig vtest.cfg` wrote; `handshake-x21.txt` and `two-x21-first.txt` are `cod4x_probe` joins against the CoD4x 21.3 release server, cut by `tools/cod4x_fixture.py` (cod4x-protocol.md 16, 18.1); `auth-x21.txt` is one of a public server, cut the same way, its address replaced (18.4) | `tests/net_decode.rs`, `tests/cod4x_join.rs`, `tests/moves_ab.rs`, `crates/server/src/cod4/` (`download`, `wire_ab`, `capture_ab`, `combat_ab`, `shot_aim_ab`, `hardpoint_ab`, `planefx_ab`, `prematch_ab`, `oob_ab`, `mod_tests`, `console_tests`), `crates/client/src/play/cod4.rs`, `cod4_hud.rs` and the client's other CoD4 tests |
| `pins` | Not retail: the pmove model's own step lines behind each self-pin digest, written by the lib tests under `PIN_DUMP=1` (`src/pmove/pin.rs`); a `#` line of field names, then a line of values a step | `src/pmove/pin.rs`, on a digest miss |
| `tapes` | Not retail: a trace tape, every trace a `moves_ab` replay asked of mp_crash's clip and the answer it got, the weapon list's move scales in its header, written under `RECORD_TAPES=1` at `COD4_DIR` (docs/dev/testing.md, "Trace tapes") | `tests/moves_ab.rs` |
| `skel` | `cod4_lnxded` under gdb, a pose or anim dump per header | `tests/skel_ab.rs`, `crates/common/src/playerpose.rs` |
| `phys` | Not the server: the Windows client under Wine and gdb (`tools/cod4_wine/run.sh` with `tools/re/cod4_phys_gdb.py`, docs/dev/retail.md, "Worlds 0 and 1 from the first push to rest"), ODE worlds 0 and 1 from the first body after a map load to rest; format and recipes in `phys/README.md` | `tools/re/cod4_phys_summary.py`; `tests/phys_fixtures.rs` (that they parse) |
| `ragdoll` | Not the server: the Windows client `iw3mp.exe` under Wine and gdb (`tools/cod4_wine/run.sh` with `tools/re/cod4_ragdoll_gdb.py`, docs/dev/retail.md, "A ragdoll from spawn to rest"), its own player killed on mp_crash and its corpse's ragdoll dumped step by step to rest; format below | `tools/re/cod4_ragdoll_summary.py`; `ragdoll_ab`, the gate of the ODE port (`cod4-player-anims.md` 10.5, 10.7) |

## `ragdoll/` lines

One ragdoll slot of world 2 (the ragdolls'), from the frame its bodies are
built to the step it goes idle. Every float is its IEEE bits in hex
(`f32::from_bits`); units are the game's, seconds and radians.

- `# frame F ragdoll_time T: CMD`: the console commands the driver sent,
  `F` frames after `cl_ingame` first read 1, `T` the ragdoll clock
  (0xd5ec534) then.
- `world W seed S msec_step M`: world 2's `dxWorld` address, its reorder
  seed (world +0x30) and its step length when the first body appears.
- `start SLOT BONE FLAGS IDLE | px py pz | qw qx qy qz | vx vy vz | wx wy wz`:
  each body as the slot built it, before its first step: ODE's flags
  (+0x18; 0x4 disabled, 0x10 auto-disable), the idle time left (+0xd8), the
  position (+0xe0, the centre of mass), the quaternion (+0xf0, ODE's w
  first), the linear (+0x130) and angular (+0x140) velocity. `BONE` is the
  def's bone index, `ragdoll.cfg`'s `ragdoll_bone 1` lines in order (0 the
  root, 1 and 2 the spine, 3 the neck, 4 and 5 the left upper and lower
  arm, 6 and 7 the right's, 8 and 9 the left thigh and shin, 10 and 11
  the right's).
- `bone SLOT BONE PARENT LEN DT | MASS | CUR | PREV` (the max-life take
  only, after each `start`): the slot bone's parent (+0) and length (+0xc),
  the time between the two pose snapshots (slot +0x30), the body's `dMass`
  (+0x20: mass, `c[4]`, `I[12]`), and the two snapshots the build read
  (0x2e0 each from slot +0x2f4, +0x2f0 the latest): each bone's origin and
  IW quaternion (`x y z w`). The latest is read after the build frame's
  state 4 saved its pose over it (0x5bc4f0).
- `step N msec M sim T to E seed S next M2`: after world 2's `N`th step
  since the first body (0x5a6e69 in `Phys_RunToTime`): its length, the
  world's clock after it, the time the frame runs it to, the seed after
  the step and the next step's length.
- `slot I state S msec M frames F`: the slot's state (4 simulating, 5 idle),
  the time and frames in that state (+0x24, +0x28, +0x2c).
- `body SLOT BONE ...`: as `start`, after the step. An idle slot has let
  its bodies go and lists none.
- The max-life take only, from `RAGDOLL_SKEL=1` (cod4-player-anims.md
  10.7.2):
  - `models SLOT NAME:BONE,BONE,... ...`: the corpse's DObj's models in
    order, each with its bones' names in its `boneNames` order; the DObj's
    bones are these, model by model.
  - `ent SLOT | ORIGIN | ANGLES | AXES`: the corpse's entity as the build
    read it (0x5bc930): its origin (+0x1c), its angles (+0x28) and the
    nine words of the axes made of them, forward, left, up.
  - `skel F T SLOT STATE PC | ORIGIN | ANGLES | SHIFT | BITS | MATS`: the
    corpse's skeleton after frame `F`'s last pose (`PC` the hook it was
    read at: 642a35 the renderer's), `T` the ragdoll clock, the slot's
    state, the entity's origin and angles then, the vector the
    skeleton's positions are relative to (0x797648), the four words of
    the DObj's bits of the bones computed (+0x34; bone `b` is `0x80000000
    >> (b & 31)` of word `b >> 5`) and each bone's eight words: its
    quaternion `x y z w`, its position less `SHIFT`, its weight. A bone
    whose bit is clear holds nothing. The committed take keeps the lines
    to 30 frames after the build.
- `end`: the dump stopped (the slot idle, or `RAGDOLL_STEPS` passed).
- `trace SITE N | START | END | A | B | MASK | TRACE` (`RAGDOLL_TRACES=1`;
  `traces-mp_crash-1.7-fall.txt` keeps only these, steps 85 to 95, of a
  corpse dropped from 1200 up): a `CM_Trace` (0x4f4d30) the ragdolls or
  the after-frame sweep ran during step `N`, `SITE` its call (5a6aea the
  sweep, 5be30c, 5be33c, 5be37c `Ragdoll_BoneTrace`), its start, its end,
  the box's two corners (eax and `[esp+0xc]` at the call; their order is
  not read), its mask, and the `trace_t`'s eleven words after it: the
  fraction, the normal, then the surface flags, contents and hit words;
  the last word's bytes are +0x28 to +0x2b, `startsolid` at +0x29 and
  `walkable` at +0x2a.
