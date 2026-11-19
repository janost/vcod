# vcod-common test fixtures

Recordings of the retail CoD 1.1d, 1.5b and United Offensive 1.51b servers.
The `.txt` files open with a `#` header holding their recipe; the `.bin`
and `.dm_<n>` files cannot carry one, so their recipes are listed here.

| Subdirectory | Recorded by | Read by |
|---|---|---|
| `net` | `vcod --net-probe` against a retail server: `gamestate-<tag>.bin` and `snapshots-<tag>.bin` pairs, most from `--save-snapshots --capture-tag <tag>`, `demo*.dm_<n>` from `--save-demo`, `www-redirect-*.bin` from `--probe-download` (`cod15-protocol-diff.md`, "HTTP downloads"). The recipe per tag is the table under "Fixtures are retail evidence" in `docs/dev/retail.md`. The untagged `gamestate.bin` / `snapshots.bin` pair came from `--save-fixture`: the gamestate off a populated 1.1 server running the CoDaM mod (`cod11-server-handshake.md`), the snapshots a lone spectator on mp_carentan (`docs/protocol-1.1.md`); recipe not recorded | `src/net/` tests, `vcod_common::testing::fixture_frames` users across the server and client tests |
| `patch` | 1.1d, 1.5b and UO: `bulletTrace` and straight-down traces over patches and terrain, two runs under gdb at the point arm | `src/collision/tests.rs`, `src/terrain.rs` tests |
| `skel11` | 1.1d, 1.5b and UO under gdb: a player's and a turret gun's DObj skeleton | `tests/skel11_ab.rs`, `tests/gunskel_ab.rs`, `crates/server/src/game/turret.rs` |
