# vcod-cod2 test fixtures

Recordings of the retail CoD2 1.3 Linux server (`cod2_lnxded`) on loopback,
plus the gsc probes that drove them. Each text file opens with a `#`
header: the map, and a `made by:` line with the `tools/run_probe_cod2.sh`
command and its flags, or the gdb script and breakpoints. The script's
header and `docs/dev/retail.md` cover the harness. The `.gsc` files are the
probe gametypes written for these runs.

| Subdirectory | Recorded by | Read by |
|---|---|---|
| `antilag` | `tools/run_probe_cod2.sh` with `probe_antilag.gsc`, some runs under gdb | `crates/server/src/game/cod2_antilag_ab.rs` |
| `combat` | `tools/run_probe_cod2.sh`, `GT=combat`, one file per weapon or case | `crates/server/src/game/cod2_combat_ab.rs`, `cod2_hitpose_ab.rs`, `tests/moves_ab.rs` |
| `demos` | `mp_toujane-retail.dm_1`: retail `CoD2MP_s.exe` 1.3 under Wine, `record` then `devmap mp_toujane`; binary, recipe on `a_retail_demo_parses_every_record` in `tests/demo.rs` (`cod2-demos.md` 6) | `tests/demo.rs`, `crates/client/src/play/cod2.rs` |
| `hitpose` | `tools/run_probe_cod2.sh` with `probe_hitpose.gsc` under `tools/re/cod2_trace_gdb.py` | `crates/server/src/game/cod2_hitpose_ab.rs` |
| `killcam` | `tools/run_probe_cod2.sh` with `probe_killcam.gsc` | `crates/server/src/game/cod2_killcam_ab.rs` |
| `moves` | `tools/run_probe_cod2.sh`, one move script per file | `tests/moves_ab.rs`, `tests/anim_ab.rs`, `crates/server/src/game/cod2_ladder.rs`, `crates/client/src/play/cod2_predict.rs` |
| `pickup` | `tools/run_probe_cod2.sh` with `probe_pickup.gsc` | `crates/server/src/game/cod2_pickup_ab.rs` |
| `skel` | `cod2_lnxded` under gdb (`tools/re/cod2_skel_gdb.py`), the pose build per player | `tests/skel_ab.rs` |
| `trace` | `cod2_lnxded` under gdb (`tools/re/cod2_trace_gdb.py`), the `DObjTraceline` calls of the capture beside each | `tests/dobj_trace_ab.rs`, `crates/server/src/game/cod2_antilag_ab.rs`, `cod2_hitpose_ab.rs` |
| `traces` | `cod2_lnxded` running the gametype `examples/trace_probe.rs` writes: `bulletTrace`, `physicsTrace` and `placeSpawnPoint` per stock map | `tests/trace_ab.rs` |
| `turret` | `tools/run_probe_cod2.sh` with `probe_turret.gsc` | `crates/server/src/game/cod2_turret_ab.rs` |
| `mp_toujane-spectate.bin` | the first 40 server messages of `examples/spectate.rs --fly --save ... --save-messages 40` beside a `--join --fly` client; binary, recipe in `tests/replay.rs`'s module comment and `cod2-protocol.md` 10 | `tests/replay.rs`, `tests/demo.rs`, `src/net/write.rs`, the client's CoD2 scene, effect and sound tests |
