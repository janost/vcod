# vcod-cod4 test fixtures

Recordings of the retail CoD4 1.7 Linux server (`cod4_lnxded`) on loopback,
inside the namespace of `docs/research/cod4-handshake.md` 7, plus the gsc
probes that drove them. Each capture opens with a `#` header holding its
recipe; the parsers skip `#` lines (`vcod_cod4::net::capture::parse` for
`net/`, the gdb dump readers for `heli/` and `skel/`).

| Subdirectory | Recorded by | Read by |
|---|---|---|
| `gsc` | Written for these runs: one probe gsc per capture, packed into a mod's iwd (`+set fs_game mods/vcod<x>`) or loose in its homepath, as each capture's header says | `crates/server/src/cod4/capture_ab.rs`, `hardpoint_ab.rs`, `planefx_ab.rs`, `shot_aim_ab.rs` and the client's CoD4 tests |
| `heli` | `cod4_lnxded` under gdb with the `heli_idle`, `hardpoint_flight` and `heli_yaw` probes (`cod4-gsc.md`, "The flight's float model") | `crates/server/src/game/cod4/vehicle/flight.rs` |
| `net` | `tools/cod4_handshake_probe.py` (`handshake-1.7*.txt`) and `cargo run -p vcod-cod4 --example cod4_probe` (`snapshots-1.7.txt`, `play-1.7-*.txt`): one `<ms> <c2s\|s2c\|auth> <peer> <hex>` line per datagram, raw; `play-1.7-*-hits.txt` are the server's `PROBE` log lines beside a pair; `download-1.7.txt` holds decoded lines from three `cod4_probe` download runs, whose raw datagrams carry a mod's file bytes and are not kept; `writeconfig-1.7.cfg` is the `players/vtest.cfg` the retail server's `writeconfig vtest.cfg` wrote | `tests/net_decode.rs`, `tests/moves_ab.rs`, `crates/server/src/cod4/` (`download`, `wire_ab`, `capture_ab`, `combat_ab`, `shot_aim_ab`, `hardpoint_ab`, `planefx_ab`, `prematch_ab`, `oob_ab`, `mod_tests`, `console_tests`), `crates/client/src/play/cod4.rs`, `cod4_hud.rs` and the client's other CoD4 tests |
| `skel` | `cod4_lnxded` under gdb, a pose or anim dump per header | `tests/skel_ab.rs`, `crates/common/src/playerpose.rs` |
