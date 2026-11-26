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
| `gsc` | Written for these runs: one probe gsc per capture, packed into a mod's iwd (`+set fs_game mods/vcod<x>`) or loose in its homepath, as each capture's header says | `crates/server/src/cod4/capture_ab.rs`, `hardpoint_ab.rs`, `planefx_ab.rs`, `shot_aim_ab.rs` and the client's CoD4 tests |
| `heli` | `cod4_lnxded` under gdb with the `heli_idle`, `hardpoint_flight` and `heli_yaw` probes (`cod4-gsc.md`, "The flight's float model") | `crates/server/src/game/cod4/vehicle/flight.rs` |
| `net` | `tools/cod4_handshake_probe.py` (`handshake-1.7*.txt`) and `cargo run -p vcod-cod4 --example cod4_probe` (`snapshots-1.7.txt`, `play-1.7-*.txt`): one `<ms> <c2s\|s2c\|auth> <peer> <hex>` line per datagram, raw; `play-1.7-*-hits.txt` are the server's `PROBE` log lines beside a pair; `download-1.7.txt` holds decoded lines from three `cod4_probe` download runs, whose raw datagrams carry a mod's file bytes and are not kept; `writeconfig-1.7.cfg` is the `players/vtest.cfg` the retail server's `writeconfig vtest.cfg` wrote; `handshake-x21.txt` and `two-x21-first.txt` are `cod4x_probe` joins against the CoD4x 21.3 release server, cut by `tools/cod4x_fixture.py` (cod4x-protocol.md 16, 18.1); `auth-x21.txt` is one of a public server, cut the same way, its address replaced (18.4) | `tests/net_decode.rs`, `tests/cod4x_join.rs`, `tests/moves_ab.rs`, `crates/server/src/cod4/` (`download`, `wire_ab`, `capture_ab`, `combat_ab`, `shot_aim_ab`, `hardpoint_ab`, `planefx_ab`, `prematch_ab`, `oob_ab`, `mod_tests`, `console_tests`), `crates/client/src/play/cod4.rs`, `cod4_hud.rs` and the client's other CoD4 tests |
| `pins` | Not retail: the pmove model's own step lines behind each self-pin digest, written by the lib tests under `PIN_DUMP=1` (`src/pmove/pin.rs`); a `#` line of field names, then a line of values a step | `src/pmove/pin.rs`, on a digest miss |
| `tapes` | Not retail: a trace tape, every trace a `moves_ab` replay asked of mp_crash's clip and the answer it got, the weapon list's move scales in its header, written under `RECORD_TAPES=1` at `COD4_DIR` (docs/dev/testing.md, "Trace tapes") | `tests/moves_ab.rs` |
| `skel` | `cod4_lnxded` under gdb, a pose or anim dump per header | `tests/skel_ab.rs`, `crates/common/src/playerpose.rs` |
| `ragdoll` | Not the server: the Windows client `iw3mp.exe` under Wine and gdb (`tools/cod4_wine/run.sh` with `tools/re/cod4_ragdoll_gdb.py`, docs/dev/retail.md, "A ragdoll from spawn to rest"), its own player killed on mp_crash and its corpse's ragdoll dumped step by step to rest; format below | `tools/re/cod4_ragdoll_summary.py`; the oracle for a port of 1.7's solver (`cod4-player-anims.md` 10.5) |

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
- `step N msec M sim T to E seed S next M2`: after world 2's `N`th step
  since the first body (0x5a6e69 in `Phys_RunToTime`): its length, the
  world's clock after it, the time the frame runs it to, the seed after
  the step and the next step's length.
- `slot I state S msec M frames F`: the slot's state (4 simulating, 5 idle),
  the time and frames in that state (+0x24, +0x28, +0x2c).
- `body SLOT BONE ...`: as `start`, after the step. An idle slot has let
  its bodies go and lists none.
- `end`: the dump stopped (the slot idle, or `RAGDOLL_STEPS` passed).
