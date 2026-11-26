# vcod-server test fixtures

Recordings of retail dedicated servers on loopback that the A/B gates replay
against vcod-server. Each text file opens with a `#` header naming the
server, the map, the flags and the commands that made it; the general
recipes (`tools/run_probe.sh`, the probe flags, which mode writes where) are
in `docs/dev/retail.md`, and the gsc probes named below (`client-probes/...`)
are in `crates/gsc/tests/fixtures/semantics/client-probes/`, whose README
holds each probe's recipe. "1.1d", "1.5b" and "UO" are the retail CoD 1.1d,
1.5b and United Offensive 1.51b Linux servers; "CoD2" is retail
`cod2_lnxded` 1.3. Gates live in `crates/server/tests/<name>.rs` unless a
path says otherwise.

| Subdirectory | Recorded by | Read by |
|---|---|---|
| `chat` | 1.5b, `client-probes/probe_squad` with `--probe-say` clients | `squad_ab` |
| `cod2` | CoD2 out-of-band answers and connects, in `unshare -rn` (`cod2-protocol.md` 17.1); `probe_math`, the math builtins' rounding, `tools/run_probe_cod2_gsc.sh` (`cod2-gsc-diff.md` 7) | `src/cod2/tests.rs`, `src/cod2/house_tests.rs`, `src/game/cod2_combat_ab.rs`, `src/game/cod2_turret_ab.rs`, `semantics_ents.rs` |
| `cod2-wire` | CoD2: a spectator and a joiner per map and gametype, `crates/cod2/examples/spectate.rs --save`; binary, recipe in the gate's module comment | `src/cod2/wire_ab.rs` |
| `configstrings` | 1.1d, 1.5b and UO configstring tables at gamestate, `--save-configstrings` | `configstrings_ab`, `configstrings_15_ab`, `configstrings_uo_ab` and the pmove gates that read the tables |
| `doors` | 1.1d, 1.5b and UO, `client-probes/probe_doors`; `mp_carentan-doors.ent` is a hand-written entity block the recipe splices into mp_carentan | `doors_ab` |
| `dyingpose` | 1.1d, 1.5b and UO, `client-probes/probe_dyingpose` and `probe_corpseblast` | `dyingpose_ab` |
| `entities` | 1.1d, 1.5b and UO entity traces along walked routes and cull probes (`--save-entities`, `--probe-edge`) | `entities_ab`, `entity_vis_ab`, `cull15_ab`, `uocull_ab`, `two_clients` |
| `gametypes` | 1.1d and UO, gsc gametype probes under `--save-scripted` and the servers' `games_mp.log` | `gametypes_ab`, `fx_ab` |
| `gunduel` | 1.1d (under gdb), 1.5b and UO, `client-probes/probe_gunduel` | `gunduel_ab` |
| `handshake` | 1.1d, 1.5b and UO, `client-probes/probe_primedprint`, `probe_svinfoquote`, `--probe-download` | `connect`, `www_download`, `src/download.rs` |
| `istouching` | 1.1d, 1.5b and UO, `client-probes/probe_istouching` | `istouching_ab` |
| `items` | 1.1d, 1.5b and UO, item probes (`probe_itemdrop`, `probe_respawnsign`, `probe_startsolid`, `probe_class`, `probe_fragcount`) and `--probe-items` | `pickup_ab`, `itemdrop_ab`, `src/game/teams_ab.rs` |
| `leanhit` | 1.1d, 1.5b and UO, `client-probes/probe_leanhit` with a `--probe-uo-weapon direct` client | `leanhit_ab` |
| `leanpvs` | 1.1d, `client-probes/probe_leanpvs` | `leanpvs_ab` |
| `missiles` | 1.1d, 1.5b and UO, `client-probes/probe_skyshot`, `probe_satchelshot` and kin | `missile_edge_ab`, `uo_weapons_ab` |
| `movers` | 1.1d, 1.5b and UO mover and ride probes (`--probe-ride`, `probe_legs`) | `movers_ab`, `push_ab`, `push_order_ab`, `push_candidate_ab`, `ride_ab`, `linkto_ab`, `legs_ab`, `sine_ab`, `spawnclip_ab`, `worldcopy_ab`, `predict_ab` |
| `netchan` | 1.1d across a map end and an S&D round restart (`--save-roundrestart`) | `mapchange_ab`, `roundrestart_ab`, `two_clients` |
| `noclass` | 1.1d, 1.5b and UO, `client-probes/probe_noclass` | `noclass_ab` |
| `ownline` | 1.1d, `client-probes/probe_ownline` | `ownline_ab`, `crates/client/src/entities_gunner_ab.rs` |
| `playerstate` | 1.1d, 1.5b and UO playerstates along walked routes and probes (`--save-slope`, `--probe-cmd-ms`, `probe_teleport`, `probe_ladder`, `probe_prone`) | `playerstate_ab` and its siblings, `predict_ab`, `combat`, `fall_ab`, `ladder_ab`, `lean_ab`, `vehicle_ab` and about twenty more |
| `pronebones` | 1.1d, 1.5b and UO, `client-probes/probe_pronebones` | `pronepose_ab` |
| `pronecrouch` | 1.1d, 1.5b and UO, `client-probes/probe_pronecrouch` | `pronecrouch_ab` |
| `scriptmodel` | 1.1d, 1.5b and UO, `client-probes/probe_scriptmodel` and `probe_modelbounds` | `scriptmodel_ab`, `model_bounds_ab` |
| `seatspawn` | 1.1d and UO, `client-probes/probe_respawn`, `probe_turret` and `probe_vehicle`, some with `--save-turret` | `turret_ab`, `vehicle_ab` |
| `setoriginlink` | 1.1d, `client-probes/probe_setoriginlink` | `setoriginlink_ab` |
| `shellshock` | 1.1d, `client-probes/probe_shock` with `--save-shock` | `shellshock_ab` |
| `spawnpoints` | 1.1d and 1.5b, `client-probes/probe_placespawn` on every stock map | `placespawn_ab`, `legs_ab` |
| `spawnview` | 1.1d, 1.5b and UO, `client-probes/probe_spawnview` | `spawnview_ab` |
| `specspawn` | 1.1d, 1.5b and UO, `client-probes/probe_specspawn` | `specspawn_ab` |
| `tapes` | Not retail: trace tapes, every trace and point contents a 1.1 replay asked of the retail map and the answer it got, plus the numbers of the weapon files it read, written under `RECORD_TAPES=1` at the 1.1 install (docs/dev/testing.md, "Trace tapes") | `moves_ab`, `ladder_ab` |
| `teams` | 1.1d, 1.5b and UO under gdb, no client; `mp_carentan-teams.ent` is a hand-written entity block the recipe appends to mp_carentan | `src/game/teams_ab.rs` |
| `threadorder` | 1.1d, 1.5b and UO, `client-probes/probe_threadorder` (`mp_pavlov*.txt`, the servers' `PROBE` lines) and `probe_menuclock` (`menuclock-first*.txt` with a header, `menuclock-log*.txt` the servers' `PROBE` lines). The gates compare these files line for line, so the headerless ones carry no header; their recipes are the two probes' sections of the client-probes README | `threadorder_ab`, `menuclock_ab` |
| `timeout` | 1.5b, `--probe-say` clients | `timeout_15_ab` |
| `touchorder` | 1.1d, 1.5b and UO, `client-probes/probe_touchorder` and `probe_trigqueue` | `touchorder_ab` |
| `triggers` | 1.1d, 1.5b and UO trigger probes (`probe_hurtfall`, `probe_useby`, `probe_minefield`, `--probe-ride`) | `triggers_ab`, `hurtfall_ab`, `minefield_uo_ab`, `sd_plant_ab` |
| `turret` | 1.1d, 1.5b and UO, `client-probes/probe_turret` with `--save-turret` | `turret_ab` |
| `usage` | 1.1d, `client-probes/probe_usage`, `scriptUsage` over rcon | `src/server.rs` tests |
| `vehicles` | UO vehicle probes (`probe_vehicle_armor`, `probe_vehicle_hits`, `probe_vehblast`, `--probe-vehicle`) | `vehicle_ab`, `vehicle_armor_ab`, `vehicle_blast_ab`, `vehicle_hits_ab`, `vehicle_shell_ab`, `vehicle_fuse_ab` and the other `vehicle_*` gates |
| `votes` | 1.1d, 1.5b and UO, `--probe-say` callvotes | `votes_ab` |
| `cod2-mp_toujane-mapfx.bin` | CoD2 on mp_toujane with one `crates/cod2/examples/spectate.rs` client; binary, recipe on `MAPFX_FIXTURE` | `src/cod2/tests.rs` |
