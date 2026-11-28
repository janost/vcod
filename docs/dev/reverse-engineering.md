# Reverse engineering

Which retail binary answers which question, the scripts in `tools/re/`, and
the disassembly traps. What a binary says goes into the subsystem's research
doc with its address and an evidence label (`docs/research/README.md`,
"Evidence labels"). A branch or an instruction order read at a named address
range is VERIFIED (the instructions); which function it is, unless a string
names it, is INFERRED.

## The binaries

| Game | Module | Role |
|---|---|---|
| CoD1 1.1 | `main/cgame_mp_x86.dll` | MP client game: event dispatch, effects. The authority for the client. Image base `0x30000000`. |
| CoD1 1.1 | `main/game_mp_x86.dll` | MP server game. Image base `0x20000000`. |
| CoD1 1.1 | `game.mp.i386.so` (1.1d Linux) | the same as `game_mp`, with full symbols |
| CoD1 1.1 | `cod_lnxded` (1.1d Linux) | the dedicated server engine (`tools/re/net-notes.md`) |
| CoD1 1.1 | `CoDMP.exe` | client netfield tables, sound system, efx renderer. Image base `0x00400000`. |
| CoD1 1.1 | `cgamex86.dll` | single-player. Its `EV_*` ids diverge from 173 up: wrong module for MP. |
| CoD1 1.5, UO | `game.mp.i386.so` (1.5b), `game.mp.uo.i386.so` (UO 1.51b) | read by the same `dump_*` scripts as 1.1d |
| CoD2 1.3 | `cod2_lnxded` | stripped, links the game in |
| CoD2 1.3 | Mac 1.3 MP binary (i386 slice) | keeps its symbols: the names for `cod2_lnxded` |
| CoD4 1.7 | `cod4_lnxded` (md5 `bf8448740b3f4cb74c2254f01af14cdd`) | the server; stripped, read with `objdump` |
| CoD4 1.7 | `cod4_lnxded-bin` (md5 `6b9995fa2e800664644da1c067779414`) | an earlier build beside it (`May 20 2008`); only `cod4-fastfile.md`'s `Load_*` addresses are in it |
| CoD4 1.7 | `iw3mp.exe` | the client: menu commands, vision fields, default stats |
| CoD4x 21.3 | `cod4x18_dedrun` (md5 `543d59bdc5109556460d4562a7481681`) | the CoD4x server release, unstripped PIE; the binary only, never its source (`cod4x-protocol.md`) |

The md5s of the CoD1 modules are in `docs/research/cod11-events-and-fx.md`.
CoD4 1.7 is the only CoD4 target. Its sources are the 1.7 binaries
(`objdump`, `tools/re`), live captures, the research docs and the A/B gates.

## tools/re

CoD1 (1.1d, 1.5b, UO):

- `evtab.py`: the `EV_*` name table of a cgame DLL, in index order.
- `netfields.py`: finds `{name, offset, bits}` netfield tables in a PE or a
  32-bit ELF. `netfield_diff.py` diffs two by field name.
- `dump_field_table.py`: one netField table out of a dedicated-server ELF.
- `xref.py`: raw 4-byte immediates equal to a VA in a PE.
- `gdb_gunner_tree.py`: a gdb script for the 1.1d, 1.5b and UO servers
  that logs a player's anim info records and first traced skeleton each
  frame, or both gunners' at the end-frame traces (its header has the
  recipe; cod11-turrets.md 13.9, 13.10).
- `gdb_radius_damage.py`: a gdb script for the 1.1d server that logs each
  `G_RadiusDamage` candidate, its link box and `CanDamage`'s result, and
  every `G_Damage` (cod11-movers.md 26.9).
- `gdb_bullet.py`: a gdb script for the 1.1d server that logs every
  `Bullet_Fire_Extended` leg's trace and glass test and every `G_Damage`.
- `gdb_missile.py`: a gdb script for the 1.1d server, read only, that logs
  every `G_RunMissile` move and ground-snap trace, its ends and its trace
  result, every float as its bits (cod11-combat.md 12.4).
- `gdb_muzzle.py`: a gdb script for the UO server, read only, that logs
  every `CalcMuzzlePoint`'s playerstate inputs (`bobCycle`, origin,
  velocity, view), its bob factors and its muzzle, every float as its bits
  (cod11-combat.md 14.8).
- `gdb_corpse.py`: a gdb script for the UO server, read only, that logs
  each cmd's vehicle overlap test in `ClientThink_real`, what it unlinked,
  the `Pmove` after it, and a dead body's ps and box at eight points of
  `PmoveSingle` (coduo-vehicles.md 11.6.1).
- `gdb_trigger_walk.py`: a gdb script for the UO server, read only, that
  logs every `G_CheckHitTriggerDamage` walk: its attacker, start, end,
  damage and means of death, every float as its bits (cod11-combat.md
  14.8.3).
- `gdb_teams.py`: a gdb script for the 1.1d, 1.5b and UO servers, read
  only, that logs the team pick of `Use_Item` and `RespawnItem`
  (cod11-items.md 14.8).
- `gdb_vehicle.py`: a gdb script for the UO server, read only, that logs
  every vehicle record's physics state, sway and jolt at each think's track
  clock, and its velocity and committed copy at the think's entry, every
  float as its bits (coduo-vehicles.md 17.5).
- `gdb_fault.py`: a gdb script for the CoD1 servers that writes the pc,
  registers and backtrace of a fatal signal, each frame also as a module
  offset (coduo-vehicles.md 17.4.1).
- `annotate_func.py`: one function of a PIC ELF (or an unstripped
  executable, by its full symbol table) with its relocations resolved. Run it first: a plain `objdump` of `game.mp.i386.so` hides every
  call and cvar behind PIC relocations.
- `dump_script_fields.py`, `dump_builtins.py`, `dump_cvars.py`,
  `dump_itemlist.py`: the game module's script fields, builtins and spawn
  classnames, cvars and `bg_itemlist`. They read 1.1d, 1.5b and UO alike
  through `gameso.py`, which finds each table by symbol or off its lookup
  function and resolves the relocations. A pointer stored in `.data` reads as
  0 in the file.
- `net-notes.md`: the 1.1d server disassembly log.

CoD2 1.3:

- `ExportFuncGraph.java` dumps each analyzed program's functions
  (`*.funcs.tsv`). `symmap.py` pairs them, transferring names from the Mac
  build to `cod2_lnxded` (its header lists the passes and why the size filter
  stays). `merge_zk.py` folds in zk_libcod's `functions.hpp` addresses for
  the rest, and `check_zk.py` scores a map against them. `ApplySyms.java`
  applies the result to a Ghidra program before `ExportDecomp.java`.
- `dump_cod2_gsc.py` (builtin, method, field and spawn tables),
  `dump_cod2_weapon_fields.py` (weapon file fields and enum tables).
- gdb scripts for `cod2_lnxded`, each writing the file its header names:
  `cod2_trace_gdb.py` (each locational `DObjTraceline` on a player, in the
  `crates/cod2/tests/fixtures/trace/` format; `cod2_trace_trim.py` trims a
  dump to a fixture), `cod2_skel_gdb.py` (each player DObj's pose and
  skeleton, the `skel/` fixtures' format), `cod2_anim_gdb.py` (the anim
  tree at the end frame), and `cod2_gdb_all.py` (the three in one
  session). `GDB_PY` in `tools/run_probe_cod2.sh` runs one.
- `cod2_rand_trace.c`: an `LD_PRELOAD` shim logging each `srand()` and
  `rand()` with its call site and `level.time` (`RAND_TRACE=1` in
  `tools/run_probe_cod2.sh`).
- `d3dbsp_census.py` (the evidence behind `cod2-d3dbsp-format.md`),
  `cod2_matparse.py`, `cod2_techparse.py`, `cod2_mapcensus.py` (materials,
  techsets, their use on stock maps), `cod2_macarb.py` (the Mac build's ARB
  shader programs).

CoD4 1.7:

- `cod4_netfields.py` (generates `vcod_cod4::net` field tables),
  `cod4_events.py` (generates the event names), `cod4_builtins.py` (gsc
  builtin tables), `cod4_huffman.py` (the static Huffman tree),
  `cod4_dvars.py` (dvar registrations), `cod4_weapon_fields.py`
  (`weaponDefFields`).
- Off `iw3mp.exe`: `cod4_menu_commands.py`, `cod4_vision_fields.py`,
  `cod4_default_stats.py`, `cod4_cac_items.py` (the create-a-class item
  table the stat writes check).
- `d3d9dis.py`: a small D3D9 shader bytecode disassembler for a technique's
  pixel shader.
- `cod4_tagread_gdb.py`: a gdb script for `cod4_lnxded` that logs each
  player body's end-frame swings, controller runs and script tag reads,
  and each `srand` (its header has the line format and the run;
  cod4-player-anims.md 8.1).

Ghidra helpers: `ExportDecomp.java` decompiles every function of a program
into one `.c` file. `DecompAt.java <hex addr>...` decompiles just those,
making a function where analysis left a label. Run Ghidra scripts with
`-scriptPath` on a real directory: Ghidra refuses a script found through a
symlink.

## Disassembly traps

- Ghidra prints a `fistp` under a `| 0xc00` control word as `ROUND`, and that
  store truncates toward zero. Read the `fldcw` before porting one: the bob
  cycle and every `delta_angles` push are such stores.
- A float times a float constant feeding a `fistp` stays unrounded on the x87
  stack, so an `f32` product in Rust can land a step off. Widen it to `f64`.
- Find CoD1 dispatch code by string: `CG_EntityEvent:%s`,
  `CG_EntityPreEvent:%s` (bullet impacts live in the pre-event),
  `fx/impacts/`. Sound-system entry points are catalogued in
  `docs/research/cod11-sound-system.md`.
- 1.5b and UO are an unoptimized gcc build that stores every float; 1.1d keeps
  them on the stack. A port takes the version's `Arith`
  ([traps.md](traps.md#movement-and-collision)).
