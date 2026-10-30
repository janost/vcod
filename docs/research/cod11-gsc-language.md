# CoD 1.1 gsc script language

The grammar, operator set, engine interface and measured semantics of Activision's MP gameplay scripts (`.gsc`) in the stock paks, as `vcod-gsc` runs them. VERIFIED: measured against the shipped assets, the binary or a retail server. INFERRED: read off decompilation or disassembly without a live test. The module is `game.mp.i386.so` (1.1d Linux dedicated server, MP game module, full dynamic symbol table); VM facts come from `cod_lnxded`.

Census method: sections 2 and 3 started from a comment-, string- and devblock-aware byte classifier (each byte tagged code, string, `//` comment, `/* */` comment or `/# #/` developer block), so a construct counts only in live code. Counts are files using the construct at least once, out of 799, unless stated. Where the compiler could re-derive a count it did ("re-verified"). `crates/gsc/tests/corpus.rs` compiles all 799 files on every test run with `COD_DIR` set (`every_stock_script_compiles`) and runs `bytecode::stack_depth` on every function.

The compiler accepts a repeated parameter name (`maps/redsquare.gsc`'s `mg42_target(nTarget, eMG42, nGunner, nTarget)` ships, so retail accepts it): `declare_param` (`crates/gsc/src/compile.rs`) gives each occurrence a slot, the last one winning the name lookup, as `Vm::make_frame`'s positional binding does.

Contents: 1 corpus, 2 grammar present, 3 absent constructs, 4 operators, 5 callbacks, 6 `mr`, 7 builtin table, 8 case folding, 9 semantics measured against retail, 10 divergences.

## 1. Corpus

VERIFIED (`unzip -l`, `find`): 799 `.gsc` files, 3,784,878 bytes (3.78 MB):

| pak | files |
|---|---|
| pak0 | 17 |
| pak4 | 162 |
| pak5 | 620 |

pak1, pak2, pak3 and pak6 hold no `.gsc`.

VERIFIED (compile census, `every_stock_script_compiles`): all 799 parse and compile, to 4834 functions, and every function passes `bytecode::stack_depth`, the stack-discipline and jump-bounds check in `crates/gsc/src/bytecode.rs`. `the_corpus_builtin_surface_is_stable` counts 339 distinct names this per-file compile classifies as builtin calls. That is more than the engine's 216 builtins (section 7) because an unresolved call into another script looks like a builtin call until the loader follows it.

## 2. Grammar surface present

| construct | files (of 799) | evidence |
|---|---:|---|
| function definitions at file top level | 799 | every file, e.g. `main()\n{` |
| `if` | 504 | |
| `else` | 124 | |
| `while` | 82 | |
| `for(init;cond;step)` | 108 | |
| `for(;;)` | 48 | subset of the 108 |
| `switch`/`case` | 91 | |
| `default:` | 19 | subset of the 91 |
| `break` | 130 | |
| `continue` | 31 | |
| `return` | 126 | |
| `wait <expr>;` (no parens) | 91 | `dm.gsc:539` `wait delay;`, `:707` `wait getcvarint("scr_forcerespawn");`, `:716` `wait 0;` |
| `thread` (paren-less method form) | 119 | `dm.gsc:545` `self thread killcam(...)`, `us_intro.gsc:22` `player thread skipthebriefing();` |
| `waittill` | 100 | `dm.gsc:240` `self waittill("menuresponse", menu, response);` |
| `endon` | 111 | `dm.gsc:470` `self endon("spawned");` |
| `notify` | 80 | `dm.gsc:573` `self notify("spawned");`, `_utility.gsc:358` `tracker thread death_wait_notify(...)` |
| namespaced calls (`a\b\c::fn(...)`) | 710 | `dm.gsc` calls `maps\mp\gametypes\_callbacksetup::SetupCallbacks()` |
| function pointers as values / `[[expr]]()` | 36 | `_teams.gsc:17` `game["allies_model"] = mptype\american_airborne::main;`; `_callbacksetup.gsc:15` `[[level.callbackStartGameType]]();` |
| localized string refs `&"KEY"` | 54 | `mp_pavlov.gsc` `level.obj["Field Radio"] = (&"RE_OBJ_FIELD_RADIO");` |
| animtree refs `%name` | 102 | `animscripts/init.gsc:332` `self.anim_combatrunanim = %combatrun_forward_1;` |
| `#using_animtree("name");` | 114 | `animscripts/init.gsc:26` `#using_animtree ("generic_human");` (a space before the paren is legal) |
| `/# ... #/` developer blocks | 32 | `_spawner.gsc:555` `/#[[anim.println]]("...");#/` |
| vector literals `(x, y, z)` | 86 | `animscripts/utility.gsc:234` `poseOffset = (0,0,0);` |
| vector component read `vec[0]`..`vec[2]` | 42 | see below |
| empty array init `x = [];` | 157 | `dm.gsc:104` `level.healthqueue = [];` |
| cast `(int)` | 12 | `_window.gsc:64` `xcount = (int)(yendorg[1]-windoworg[1])/spacing;` |
| cast `(float)` | 17 | `_tankdrive.gsc:236` `x = (float) height;` |
| cast `(vector)` | 0 | see below |
| method-call syntax (`self`/var `verb(...)`) | 664 (`self ...(`) + 139 (var `...(` for `playsound`/`thread`/`notify`/`waittill`/`endon`) | |
| `level`/`game` as a call receiver | re-verified: 1828 sites | `_callbacksetup.gsc` `level notify(...)`; ordinary AST receivers (`Expr::LevelRef`/`Expr::GameRef`), accepted wherever `self` or a variable is |

Vector component reads: `_utility.gsc:178` `vectorScale`, `_utility.gsc:250` `orientToNormal` (runs on every stock S&D plant). The read is a float. Counted as files with a constant `[0]`, `[1]` or `[2]` on an entity vector field (`.origin`, `.angles`: 24 files, 16 of them `.origin`) or on a local or parameter holding a vector (assigned from a vector literal, a vector field or a vector-returning builtin), comments stripped; 27 of the 42 are under `maps/`. Out-of-range and non-integer keys: section 10.

`(vector)` cast: the one textual hit is `_tankdrive.gsc:794`, `self setTurretTargetVec(vector);`, a variable named `vector`. No real `(vector)` cast exists in the corpus. `(int)` and `(float)` are in 29 files between them. Re-verified: the grammar accepts `(vector)` and lowers it to `Op::CastVector`, alongside `(int)`/`(float)`; no corpus site and no fixture reaches it.

Switch fallthrough on empty cases is common: `dm.gsc:249-251` stacks `case "allies": case "axis": case "autoassign":` before one body. Field access tolerates spaces around the dot (`animscripts/init.gsc:129` `self . ramboChance *= 2;`).

## 3. What is absent — and two corrections to the design's absence list

VERIFIED by census, zero hits: `#include`, `foreach`, ternary `?:`, `&=`, `^=`, `<<=`, `>>=`.

`^`, `<<`, `>>` and `~` never occur in live code. By context:

| token | code | string | `//` comment | `/* */` comment | `/#…#/` devblock |
|---|---:|---:|---:|---:|---:|
| `^` | 0 | 136 | 59 | 18 | 0 |
| `<<` | 0 | 0 | 10 | 0 | 0 |
| `>>` | 0 | 0 | 12 | 0 | 0 |
| `~` | 0 | 0 | 1 | 0 | 0 |

Every `^` is a colour code in a string or sits in a comment; `<<`/`>>`/`~` appear only in `//` comments (dividers, arrows). A plain `grep -c` overcounts all four.

Correction 1: `do`/`while` exists, once, in `animscripts/predict.gsc:103-114`, inside `tumbleWall()`:

```
do
{
	thread getNotetrack(notifyName);
	...
} while (bPredictMore);
```

`grep -rn '} while' pak0 pak4 pak5` returns only this. Re-verified: `parse_file` accepts it (`StmtKind::DoWhile`), and the compile census finds that one site.

Correction 2: two bitwise operators are required.

- Binary `&` tests `spawnflags` bits. VERIFIED in 11 files, e.g. `_utility.gsc:65` `if (self.spawnflags & 1)`, `_load.gsc` `if (trigger.spawnflags & 8)` (three sites), `stalingrad.gsc` `if (ai[i].spawnflags & 4)`, `_scripted.gsc` `if (bitflags & level.teleport)`. Every map's `maps\mp\_load::main()` reaches `_load.gsc`'s.
- `|=` accumulates damage flags in all five stock gametypes: `dm.gsc`, `tdm.gsc`, `sd.gsc`, `re.gsc` and `bel.gsc` each have `iDFlags |= level.iDFLAGS_NO_KNOCKBACK;`. The constants come from `_callbacksetup.gsc`'s `SetupCallbacks()`: `level.iDFLAGS_RADIUS=1`, `iDFLAGS_NO_ARMOR=2`, `iDFLAGS_NO_KNOCKBACK=4`, `iDFLAGS_NO_TEAM_PROTECTION=8`, `iDFLAGS_NO_PROTECTION=16`, `iDFLAGS_PASSTHRU=32`.

Absent: `&=`, `^=`, `<<=`, `>>=`, and any standalone `|` outside `||`/`|=`. Re-verified: `parse_file` accepts both (`BinOp::BitAnd`; `|=` desugars to `BinOp::BitOr`, see its comment in `crates/gsc/src/ast.rs`). Compile census: 11 files, 24 sites for binary `&`; 5 files, 5 sites for `|=`, one per stock gametype. A VM with no bitwise operators cannot compile `_load.gsc`, `_utility.gsc`, `_scripted.gsc`, `stalingrad.gsc` or any stock gametype.

## 4. The complete operator set (corrected)

`+ - * / %`, binary `&` (section 3), `= += -= *= /= |=`, `++ --`, `== != < > <= >=`, `&& || !`, unary `-`. No ternary, no shifts, no `~`, no `&=`/`^=`/`<<=`/`>>=`.

Binary `|` and `^` exist too, unused by the 1.1 corpus. UO's `dom.gsc` (3377, 3379) writes `", "^7` inside an `iPrintLn` argument list, and both the 1.1d and UO 1.51b compilers accept it. VERIFIED (`probe_bitops`, both servers, identical output): the binary levels are C's, loosest first `||`, `&&`, `|`, `^`, `&`, `== !=`, `< > <= >=`, `+ -`, `* / %`.

| Expression | Value |
|---|---|
| `1 \| 2 ^ 3` | 1 |
| `6 ^ 3 & 5` | 7 |
| `2 ^ 3 == 1` | 2 |
| `1 + 2 ^ 3` | 0 |
| `1 && 2 & 1` | 0 |

`^` on a string and an int is the runtime error `pair has unmatching types 'string' and 'int'`, so dom's line is fatal only when six or more players share a capture.

## 5. The engine/script interface: five callbacks

`maps/MP/gametypes/_callbacksetup.gsc` defines five `CodeCallback_*` functions the engine calls; each is a one-line `[[expr]]()` dispatch through a `level.callback*` field:

| engine hook | dispatches through | signature |
|---|---|---|
| `CodeCallback_StartGameType()` | `level.callbackStartGameType` | none |
| `CodeCallback_PlayerConnect()` | `level.callbackPlayerConnect` | none |
| `CodeCallback_PlayerDisconnect()` | `level.callbackPlayerDisconnect` | none |
| `CodeCallback_PlayerDamage(...)` | `level.callbackPlayerDamage` | `eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc` (9 args) |
| `CodeCallback_PlayerKilled(...)` | `level.callbackPlayerKilled` | `eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc` (7 args) |

`CodeCallback_StartGameType` guards a double run with `level.gametypestarted`. `SetupCallbacks()` calls `SetDefaultCallbacks()`, then sets the six `iDFLAGS_*` constants (section 3). `SetDefaultCallbacks()` copies the five pointers into `level.default_Callback*`, so a level script can override one callback and still reach the gametype's.

A gametype installs the pointers itself, `dm.gsc:68-74` in `main()`:

```
level.callbackStartGameType = ::Callback_StartGameType;
level.callbackPlayerConnect = ::Callback_PlayerConnect;
level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
level.callbackPlayerDamage = ::Callback_PlayerDamage;
level.callbackPlayerKilled = ::Callback_PlayerKilled;

maps\mp\gametypes\_callbacksetup::SetupCallbacks();
```

`::Callback_StartGameType` is the bare `::` pointer literal (current file), beside the namespaced `a\b\c::fn` form.

Re-verified, compiler counts: a function-pointer value (`Expr::FuncRef`) appears 354 times in 44 files, 212 with no file segment and 142 naming a path (`x = maps\mp\b::cb;`, what `load.rs`'s cross-file scan follows). A `[[expr]]()` call appears 163 times. An ordinary namespaced call, resolved at compile time, appears 8588 times.

## 6. The `mr` client command and menu-response join path

VERIFIED (`nm -D`): `ClientCommand` at `0x487ec`, `Cmd_MenuResponse_f` at `0x486d8`.

VERIFIED: `ClientCommand` is a flat `Q_stricmp` chain (`objdump -dr --start-address=0x487ec --stop-address=0x48f8c`, the next symbol `Cmd_Score_f` bounding it). The string addresses pushed before each compare, in order: `0x73f69, 0x73f6d, 0x73f76, 0x73f7b, 0x73f85, 0x73f8a, 0x73f90, 0x73f93, 0x73f98, 0x73f9d, 0x73fa1, 0x73faa, 0x73fb1, 0x73fb5, 0x73fba, 0x73fc5, 0x73fd0, 0x73fd6, 0x73fdf, 0x73fe4, 0x73fe7, 0x73ff2`. In `.rodata`, in dispatch order:

```
say, say_team, vsay, vsay_team, tell, score, mr, give, take, god,
notarget, noclip, ufo, kill, follownext, followprev, where, callvote,
vote, gc, setviewpos, entitycount
```

22 commands in one null-delimited block `0x73f69..0x73ffe` (`entitycount` ends at `0x73ffd`; VERIFIED, raw bytes). One more pushed string, `0x73f46`, is `entitycount`'s format `entity count = %i\n`, just before the block. No `team` command: joining is a menu response.

`mr` calls `Cmd_MenuResponse_f` (`call 486d8 <Cmd_MenuResponse_f@@Base>`). Its body, read with `tools/re/annotate_func.py game.mp.i386.so Cmd_MenuResponse_f`:

- VERIFIED: `trap_Argc() != 4` (0x486e8) stores `""` in the menu buffer and `bad\0` in the response buffer (0x486ed, 0x486f4) and jumps to the notify at 0x487b3.
- VERIFIED: `argv[1]` goes through `strtol(.., 10)` (0x48730) and is compared with `trap_Cvar_VariableIntegerValue("sv_serverId")` (string 0x73f5d, 0x4874a); a mismatch jumps to the return at 0x487de.
- VERIFIED: `argv[2]` is read into the menu buffer and `strtol`'d (0x48772). `cmp eax,0x1f; ja` (0x4877c) skips the `trap_GetConfigstring(0x49c + index)` that overwrites the buffer, so an index past 31 (unsigned, so a negative one too) leaves the argument's own text as the menu name.
- VERIFIED: `argv[3]` is the response (0x487a9). `Scr_AddString(response)`, `Scr_AddString(menu)`, then `Scr_Notify(ent, scr_const.menuresponse, 2)` (0x487b7..0x487d9). INFERRED, from the stack order: `waittill`'s first parameter is the menu, the second the response.

VERIFIED, `client-probes/probe_menuresponse.gsc` on retail 1.1d (one `--net-probe` client sending each line through `--probe-say`, a menu precached in slot 0 only):

| client sent | `menuresponse` menu, response |
|---|---|
| `mr 0 noop`, `mr 9 bad` (the client's own refusals) | `""`, `bad` |
| `mr`, `mr <sid> 0`, `mr <sid> 0 a b` | `""`, `bad` |
| `mr <sid> 0 hello` | `probe_menu_zero`, `hello` |
| `mr <sid> 0 "quoted word"` | `probe_menu_zero`, `quoted word` |
| `mr <sid> 0x1 hex` | `probe_menu_zero`, `hex` |
| `mr <sid> 1abc junk` | `""` (slot 1 is empty), `junk` |
| `mr <sid> 31 last` | `""`, `last` |
| `mr <sid> 40 past` | `40`, `past` |
| `mr <sid> -1 neg` | `-1`, `neg` |
| `mr 12345 0 stale` | nothing |
| `mr <sid> 0 Mixed` | `probe_menu_zero`, `Mixed` |

A client's `noop` and `bad` both reach the script as menu `""`, response `"bad"`. VERIFIED (`maps/mp/gametypes/*.gsc`, pak5): no stock `menuresponse` loop compares either against `""`.

vcod: `parse_menu_response` (`crates/server/src/server.rs`) tokenizes with quotes (`game::say::tokenize`), reads both numbers with a saturating `strtol` and hands `ScriptRuntime::menu_response` a `MenuRef`. VERIFIED: the probe against `vcod-server --gametype-script` logged the same fourteen lines.

The script side: `dm.gsc:142` builds `game["menu_team"] = "team_" + game["allies"] + game["axis"];`, `"team_americangerman"` for the stock pairing, the configstring slot 1180 string `configstrings.rs` writes. `dm.gsc:240`, in a `for(;;)`, is `self waittill("menuresponse", menu, response);`, with a `continue` for `response == "open"` or `"close"` and a `switch(response)` on `case "allies": case "axis": case "autoassign":` before checking `menu == game["menu_team"]`. The join on the wire is one `mr <serverId> <menuIndex> allies` (or `axis`/`autoassign`/`spectator`), landing as `notify("menuresponse", menu, response)` on the player's entity.

## 7. The engine's script-function name table

Superseded by `cod11-gsc-object-model.md` section 9, which dumps every builtin table with `tools/re/dump_builtins.py`. Summary:

- Count 216, VERIFIED, in five tables. `functions` (0x7e508, 106 entries) holds the free functions. `Scr_GetMethod` searches player (0x733dc, 46), scriptent (0x78d40, 12) and hudelem (0x749b4, 14) methods in that order and falls back to the generic entity methods (0x7ea00, 38). Every walk is a linear `strcmp` over a hardcoded count.
- Records, VERIFIED: `functions` rows are 12 bytes `{char *name; void (*fn)(); int developer;}`, method rows 8 bytes `{char *name; void (*fn)();}`. `developer` is 1 on `print`, `println` and `assert`, 0 elsewhere, as in CoDExtended's `SCRIPTFUNCTION { name, function, developer }` (`src/script.c`).
- Trap: a pointer stored in `.data` reads as 0 in the file, since the module is a shared object and `.rel.data` supplies the address. Read raw, every function pointer looks null. So no record has a null function (`getent` is `Scr_GetEnt`, `getentarray` `Scr_GetEntArray`); an earlier count of 144 was 106 plus 38 under a uniform 8-byte stride, which does not hold; and the `move*`/`rotate*` builtins are the scriptent method table in `.data` with real code pointers (`ScriptEntCmd_MoveTo`, `ScriptEntCmd_GravityMove`, `ScriptEntCmd_RotateVelocity` and the rest), not a `.rodata` whitelist.

`Scr_GetFunction` writes the table's spelling back through its `char **` first argument (0x5c199), so the engine canonicalises a builtin name on lookup, consistent with section 8.

## 8. Atom identity and case folding

The engine interns every script string (`GScr_AllocString`). It matches identifiers, field names, file paths and event names (`notify`/`waittill`/`endon`) case-insensitively, and compares string values and array keys case-sensitively (VERIFIED, below).

`vcod-gsc`'s interner (`crates/gsc/src/atom.rs`) has two entry points over one storage vector. `intern_folded` dedups on lowercased text, so two spellings of an identifier share one `Atom`. `intern_exact` dedups on exact text, so `"ABC"` and `"abc"` are two atoms. Both store the spelling given, and `Interner::resolve` returns it, so a display string is never lowercased. Whichever entry point sees a folded key first owns it, so the two never hand out different atoms for one identifier.

Re-verified: grouping every string literal in the corpus by case-folded form finds 86 keys with more than one spelling, all weapon, tag, animtree-model or asset-path names (`Panzerfaust`/`panzerfaust`, `TAG_groupA`/`tag_groupA`, `xmodel/head_Elder`/`xmodel/head_elder`, ...), none display text. 7 of the 86 are used as event names (`"SPAWNED"`/`"spawned"`, `"DIED"`/`"died"`), 17 as a literal `array["key"]` index.

VERIFIED against retail (`crates/gsc/tests/fixtures/semantics/retail-captures.txt`, `# probe_field_case`, `# probe_arraykey_case`, `# probe_cmp`): `level.myField` reads back through `level.myfield`; `a["medFire"]` and `a["medfire"]` are two entries (`.size` 2); `"ABC" == "abc"` is false. vcod routes each call site to the matching entry point: of the 86, the identifier, field, path and event-name uses collapse onto one atom, the array-key and string-literal uses do not.

Event names are where the split bites: a `waittill` on one spelling must see a `notify` with another, or the thread hangs silently. An event name is a string literal, so it reaches the VM case-preserved; `Op::WaitTill`, `Op::Notify`, `Op::EndOn` and `Vm::notify` each map it through `Interner::fold_atom` before matching, which also covers a built name (`self notify(level.eventName)`).

Known divergence: when two spellings of an identifier collapse onto one atom, `resolve` returns the first spelling interned, so what a script prints depends on load order, not on the call site's spelling. Fixing it would need a custom `PartialEq` on `Value`; not done.

## 9. Semantics measured against retail

All measured on the retail 1.1d Linux dedicated server unless stated. Probes are `crates/gsc/tests/fixtures/semantics/probe_*.gsc`, run as gametypes by `tools/run_probe.sh`; retail's answers are in `retail-captures.txt` beside them. `crates/gsc/tests/semantics_ab.rs` diffs vcod against them on every test run. Probes that need the object model, the cvar table or a map (the `RUN_IN_SERVER_CRATE` list in `semantics_ab.rs`: `probe_bootstrap`, `probe_cvar`, `probe_delete`, `probe_ents`, `probe_not_string`, the `probe_stale_*`, `probe_late_*`, `probe_precache_*`, `probe_persist_*` and others) run in `crates/server/tests/semantics_ents.rs` against the same file. A probe retail refuses to compile has a section holding only `PROBE_ERROR script compile error`, and `semantics_ab.rs` requires vcod to refuse it too (`probe_game_dotwrite`, `probe_level_bracket`, `probe_self_bracket`).

Four retail facts shape every probe:

- `logPrint` is the only output a client-less dedicated server shows. `print` and `println` print nothing even at `developer 1`; `iPrintLn` needs a client. `logPrint` writes `games_mp.log` with a leading `m:ss ` stamp.
- At `developer 1`, the probes' setting, a script runtime error ends the server, not just the thread: the console prints a `******* script runtime error *******` block with file, line and a caret, then `ERROR: script runtime error` and `----- Server Shutdown -----`. Hence one probe group per file. Below `developer 1` the thread runs on ("Runtime errors below developer 1").
- A gametype needs a one-line `.txt` description beside its `.gsc`, or the engine warns and refuses the map.
- The engine loads only the first 31 loose gametype scripts and runs `dm` for any past that (`Too many game type scripts found! Only loading the first 31`, then `g_gametype is not a valid gametype, defaulting to dm`). Neither line reaches the probe's channel, so the section comes back empty with no `PROBE_FATAL`, as a compile error would. `tools/run_probe.sh` deletes the loose `probe_*` files in the homepath before installing its own.

**Boolean reading: numbers and numeric strings, VERIFIED.** `if (x)` takes `Int` and `Float`: `0` and `0.0` are false, `1` and `0.5` true. A string reads as the number it spells: `if ("1")` branches and `if ("0")` does not, on 1.1d and UO 1.51b (`probe_truthy_numstr`). `""`, `"a"`, a vector and an unset field each raise `cannot cast X to bool`. The corpus therefore tests `isDefined(x)` or `x != ""`. UO's `tdm.gsc` tests `if (getCvar("scr_teamscorepenalty"))`, `"1"` from its own setup, on every suicide and team kill; a VM that refuses strings aborts that callback before its `K` line.

**Unary `!` is the same reading, VERIFIED (`probe_not_string`, `probe_not_empty_string`).** `!"1"` is `0`, `!"0"` is `1`, and `!""` dies on `cannot cast "" to bool`. `_teams::restrictPlacedWeapons` guards on `!getCvar("scr_allow_fg42")`; stock `scr_allow_fg42` is `"0"`, so on a stock server the guard is true and the map's placed fg42s are deleted. `not_of` (`crates/gsc/src/vm/interp.rs`) implements the reading for `Op::Not` and the conditional jumps, for `Value::String` and `Value::Localized`.

**`true` and `false` are ints, and case-sensitive, VERIFIED (`probe_bool`).** `true` concatenates as `1`, `false` as `0`; `true == 1` and `false == 0` hold. `TRUE` is not the literal: it reads `undefined`, as an unassigned local, and concatenating it is `pair has unmatching types 'string' and 'undefined'`. This is the one exception to case-insensitive identifier matching. The corpus depends on it (`_gameobjects::main` sets `dodelete = true` and branches on `if(dodelete)`).

**Arithmetic, VERIFIED.**

| Expression | Value |
|---|---|
| `1 / 2` | `0` (truncates) |
| `4 / 2` | `2` |
| `3 / 2.0` | `1.5` |
| `3 * 3` | `9` |
| `3 * 1.5` | `4.5` |
| `1 + 2` | `3` |
| `1 + 0.5` | `1.5` |
| `7 % 3` | `1` |
| `(0 - 7) % 3` | `-1` (toward zero) |
| `1 / 0` | fatal `divide by 0` |

**Number-to-string rendering: C's `%g`, VERIFIED.** Concatenated: `5` -> `5`, `-5` -> `-5`, `0.5` -> `0.5`, `2.0` -> `2`, `0.8` -> `0.8`, `1.0 / 3` -> `0.333333`. Six significant digits, trailing zeros dropped. `probe_concat.gsc:22`'s `1000000` is an int and never reaches the float formatter, so it says nothing about `%g`. VERIFIED (`probe_concat_exp`), the exponent form: `0.00001` -> `1e-05`, `-0.00002` -> `-2e-05`, `0.000012345678` -> `1.23457e-05`, `0.0001` -> `0.0001`, `999999.0` -> `999999`, `1000000.0` -> `1e+06`, `1234567.0` -> `1.23457e+06`, `1e28` -> `1e+28`. INFERRED from those eight, which match C's rule: the exponent form starts below 10^-4 or at 10^6, the exponent has a sign and at least two digits, the mantissa drops trailing zeros. `setExpFog` uses the same formatter: mp_chateau's `setExpFog(0.00001, ...)` reaches configstring 12 as `0 1 1e-05 0 0 0 0` (`cod11-gametypes-re-bel.md`). A vector renders with two decimals per component: `(1, 2, 3)` -> `(1.00, 2.00, 3.00)`. `"str" + undefined` is fatal: `pair has unmatching types 'string' and 'undefined'`.

**Equality, VERIFIED.** `1 == 1.0` true; `"abc" == "abc"` true; `"ABC" == "abc"` false; `"5" == 5` true, but `"5.0" == 5`, `"05" == 5` and `"abc" == 0` false. A number against a string is rendered with `%g` and compared as text. `undefined == 0` is fatal: `pair has unmatching types 'undefined' and 'int'`.

**Ordering: numbers only, VERIFIED.** `"a" < "b"` is fatal: `pair has unmatching types 'string' and 'string'`. No string collation.

**Case folding is per role, VERIFIED.** Field names fold; array keys and string values do not (section 8).

**`.size`, VERIFIED.** On an array it counts every key whatever its type: keys `0`, `1` and `"k"` give 3, an empty array 0. `"abcd".size` is 4.

**A string indexes by character, VERIFIED (`probe_string_index`, 1.1d and UO 1.51b).** `s = "ab c"; s[1]` is `"b"`, `s[2] == " "` is 1, `s[9]` is fatal (`string index 9 out of range`). UO's `_gameobjects::main` splits `script_gameobjectname` this way.

**Assigning to an index of `undefined` auto-vivifies an array, VERIFIED (`probe_autoviv`).** `a[0] = "x"`, `b[3] = "x"` and `c["k"] = "v"` on never-set locals each give a one-element array (`.size` 1); arrays are sparse, so a non-zero first index does not backfill. The same through a field (`level.myArray[0] = "y"`), which `_load.gsc`'s `add_to_array(level._script_expoders, ...)` relies on. Indexing a defined non-array is fatal: `e = 5; e[0] = "x";` dies with `int is not an array`. vcod raises `BadType("indexing needs an array")`, a different message the harness does not compare.

**Writing `undefined` into an array removes the key, VERIFIED (`probe_array_undef`, 1.1d; the same lines from CoD2 1.3's `cod2_lnxded`).** `a[2] = undefined` on a three-element array leaves `.size` 2; a hole at `a[0]` leaves 1 with `isdefined(a[0])` 0, and the next `a[a.size] = "w"` overwrites `a[1]`, since `.size` counts keys. Same through a field and under a string key. CoD2's `_deathicons::removeDeathIcon` shrinks its arrays this way and walks them by `.size`.

**Only the literal `undefined` removes a key, and it never vivifies a local, VERIFIED (`probe_undef_into_unset`, `probe_undef_call_into_unset`, 1.1d and `cod2_lnxded` 1.3, identical).** `c[5] = undefined` on a never-set local is fatal (`undefined is not an array`), where `b[5] = 1` gives `.size` 1. `c[5] = nothing()`, a call returning undefined, vivifies `c` with `.size` 1: the key stays, holding undefined. INFERRED: the compiler turns an assignment of the literal into a clear of the element and any other right side into a store. vcod compiles the literal to `Op::ClearIndex` (an unset local under it is read, not vivified), and `StoreIndex` keeps an undefined value as a key (`Heap::set_index`, `Heap::clear_index`). A field or element target under the literal (`level.unset[5] = undefined`) is unmeasured; vcod vivifies it.

**Notify wake order: start order, VERIFIED.** Two threads waiting on one `level` event wake in the order they were started.

**The thread pick: due time first, then last queued first, VERIFIED (`probe_notify_frame`, `probe_wait0_yield`, `probe_wait0_loop`).** Retail resumes a frame's threads one at a time: earliest due first, and among threads due together the one queued last. Each consequence below is printed by retail:

- Two threads whose `wait 0.1` falls due on one frame resume newest first: `probe_wait0_yield`'s `d` before `c`, started in that order from `main`. Its `a` and `b`, each `wait 0` from `main` at load, resume `b` then `a`. `probe_notify_frame`'s frame counter, bumped by a ticker started first, already reads the new frame at the notifier and at `main`'s `wait 1`: the ticker queued its wait later, so it goes first.
- A `wait 0` reached inside a frame is due now and queued last, so the thread resumes at once, before any other thread due that frame. `probe_wait0_yield`'s `d` logs `before` and `after` with nothing between. `probe_wait0_loop`'s thousand turns of `i++; wait 0` all land in one frame, ahead of a logger due that frame but queued earlier. Unbounded, the loop never lets the frame end: retail printed `WARNING: potential infinite loop in script.` 16 times in 20 s, at `developer` 0 and 1, and logged nothing else. VERIFIED: `cod_lnxded` 0x80a5b8c pushes that string (0x80d7d60) after an `rdtsc` at 0x80a5b63. INFERRED from the two: the warning is a time budget, not a count. The `ERROR: ... killing thread` arm next to it was never reached.
- A notify does not run its waiters inside the notifying statement: `probe_notify_frame`'s notifier logs past its `notify` first. The waiters resume in the same frame after the notifier's step, in start order, whether started before or after the notifier (`probe_notify` agrees). A waiter that then does `wait 0` resumes at once, before the next waiter.
- INFERRED: a killcam therefore lasts exactly its age, and its skip respawns with no dead frame between (`cod11-spectator-follow.md` 12.7). `waitKillcamTime`'s notify resumes `killcam`, which started it, on the same frame, and `waitRespawnButton`'s opening `wait 0` resumes on the frame it was reached.

vcod's `Vm::step_runnable` is that pick: a `wait` stamps the thread with a queue number, a notify stamps its waiters newest first, and the pass takes earliest due, highest stamp, until nothing is due. Divergence, not measured: the pass stops after `MAX_THREADS_PER_FRAME` (10,000) steps and leaves the rest for the next frame, where retail stalls for good.

**The packet pass: nothing runs between frames but the callbacks, VERIFIED (`client-probes/probe_threadorder`, retail 1.1d on mp_pavlov).** One client sent client commands in pairs, both in one packet: `mr` alone, `mr` then `kill`, `kill` then `mr`, two `mr`s. Three threads wait on the player's `menuresponse` (started w1, w2, w4), one on a level `"killed"` that `Callback_PlayerKilled` notifies (w3, started between w2 and w4). The callback starts a thread (`kt`), and each logs, takes a `wait 0` and logs again. A ticker on each frame's `wait 0.05` records its last `getTime()`. With T the frame the packet landed after, retail printed:

- The callback runs inside the command, on T's `getTime()`, to its first suspend, and a thread it starts runs inside it (`killed`, `kt`, `killed_after` at T).
- Everything else runs on the next frame, T+50, ahead of the ticker (each line reads `tick` T): the waiters the commands woke and the callback's `wait 0`s, in one pick, last queued first. `mr` then `kill`: `killedz`, `ktz`, `w3`, then w1, w2, w4. `kill` then `mr`: w1, w2, w4, then `killedz`, `ktz`, `w3`. One notify's waiters go in start order, and a woken thread's own `wait 0` resumes at once, before the next.
- The second of two `mr`s in one packet reaches nobody: no waiter has run by then, as with two notifies in one step (`probe_notify_twice`).
- Two clients' packets in one frame behave the same (a second run: two probes each sending `kill` every 3 s, the second 17 ms later, so their packets crossed a frame boundary twice). Client 1's callback ran first at T, then client 0's; at T+50 client 0's `killedz` and `ktz` ran before client 1's, then the two level waiters the first kill woke, in start order. The second kill's notify found them already woken.

VERIFIED, the same two runs on retail 1.5b and UO 1.51b (mp_pavlov, `crates/server/tests/fixtures/threadorder/*-1.5.txt`, `*-uo.txt`): every line in 1.1d's order, on the same frames relative to the packet. On UO's two-client run the second probe to connect began first, so its level waiter ran first: the order follows who began, not the slot.

INFERRED, from those lines and `G_RunFrame`'s order ("The script clock"): a notify from a command or a callback only wakes its waiters, a `wait 0` taken there is due on the clock, and both wait for `G_RunFrame`'s first `Scr_RunCurrentThreads` (0x50653), where the pick takes them. No `Scr_RunCurrentThreads` runs between a packet's commands. `SV_ExecuteClientMessage` runs a packet's client commands in order (`cod11-combat.md` 9.2).

vcod: `mr` is a `PacketCommand` run in packet order with `kill`, nothing steps between frames, and the frame's first pass takes the woken threads with the rest. Trap: notifying the menu answers after the move pass, or running woken threads before `level.time` advances, puts them ahead of the callback's `wait 0`s on T. Gate: `crates/server/tests/threadorder_ab.rs`, both runs at each protocol.

A test harness that joins two clients in one frame spawns the later packet's client first, so `common::join_pair_stamped` hands the second client's packets to the server first once both are in, and the first client spawns first, as a retail client that joined earlier does. Every gate that joins two or three clients in one frame goes through that rule (`common::step_pair_joining`, `join_trio`, `step_trio_ordered`), or joins its second client as late as the retail recipe did (`vehicle_runover_ab`, `vehicle_armor_ab`). A `gametypes_ab` or `votes_ab` restart that sends both clients back through the menus in one frame reads the same in either packet order.

**The script clock.** The VM keeps its own clock, separate from `level.time`, and a frame's threads run on it a frame behind.

- VERIFIED: `getTime` (`game.mp.i386.so` 0x5d088, entry 17 of `functions`) pushes `level.time` (`level+0x1e8`) through `Scr_AddInt`, nothing else: milliseconds of `level.time`.
- VERIFIED: `G_InitGame` writes its first argument to `level.time` and `level.startTime` (`level+0x1e8`, `+0x1f4`, at 0x4fc6d and 0x4fc72), then passes `level.time` to `Scr_InitSystem` (0x4fe4a..0x4fe58), ahead of `Scr_LoadGameType`, `Scr_LoadLevel` and `Scr_StartupGameType`.
- VERIFIED: `cod_lnxded` 0x80a97ec stores its second argument, masked to 24 bits, into 0x82f594c, the word the `wait` opcode adds to. INFERRED (the game module reaches the engine through a pointer table, and this is the only two-argument writer of that word): 0x80a97ec is `Scr_InitSystem`, 0x82f594c the VM clock.
- VERIFIED: `G_RunFrame` (0x50478) writes `level.time` first, calls `Scr_RunCurrentThreads` at 0x50653 (reached whether or not the touch notify list at `level+0x29ec` is empty) and again at 0x506d0 inside the DObj loop, then `Scr_SetTime(level.time)` at 0x5070c, ahead of the entity loop.
- VERIFIED: in `cod_lnxded`, 0x80aa3d0 walks the VM clock up to its argument one millisecond at a time, running each millisecond's queue before stepping past it, and stops on the argument without running its queue. 0x80aa470 runs the current millisecond's queue only. INFERRED (call order against `G_RunFrame`'s): those are `Scr_SetTime` and `Scr_RunCurrentThreads`.
- VERIFIED: the `wait` opcode (`cod_lnxded` 0x80a4dbb..0x80a4e3e) queues the thread on `(int)(seconds * 1000 + 0.5) + clock`, masked to 24 bits, and errors on a negative wait and on one of 16777 s or more (`0x80d7fe4`).

INFERRED from those: a level's script clock starts on its own `level.time`. In `G_RunFrame(L)` the threads due on the clock the last frame left run first, then every thread due before `L` runs on its own due time, and one due at `L` waits for the next frame. A `wait` taken by a resumed thread counts from that thread's due time. Between frames the clock reads the last frame's `level.time`, so a client callback's `wait` counts from there.

VERIFIED (`client-probes/probe_startclock`, retail on `mp_carentan`): `main` and `Callback_StartGameType` both read `getTime` 0. A `wait 1` in the callback wakes at 1050 and the next `wait 1` at 2050; a `wait 0` at 100. A `wait 0.05` loop logs 100, 200, 200, 300, 300, 350, 400 and every 50 ms after: each of the three 100 ms settle frames (`cod11-map-cycle.md` 3, step 20) runs two 50 ms steps. A `wait 0.1` then `wait 0.2` logs 200 and 350.

vcod: `Vm::run_frame` walks the clock the same way; `Vm::set_time` is `Scr_InitSystem`, set by `ScriptRuntime::load_from` to the load's `level.time` before the gametype's `main`.

**The clock is 24-bit.** Every store to it is masked:

- VERIFIED: `Scr_InitSystem` (`cod_lnxded` 0x80a97ec) stores `arg & 0xffffff` (`and` at 0x80a9822). `Scr_SetTime` (0x80aa3d0) masks its argument first (0x80aa3da) and each step of its walk (`inc`/`and $0xffffff` at 0x80aa451..0x80aa457).
- VERIFIED: the `wait` opcode adds the clock to the rounded milliseconds at 0x80a4e15..0x80a4e1b and clears the sum's high byte (`movb $0x0, -0x39(%ebp)` at 0x80a4e1e, the top byte of the int at `-0x3c`), so a deadline is 24-bit too. The rounding is `(seconds * 1000.0f + 0.5f)` (0x80a4ded, 0x80a4df3; floats at 0x80d7fe8 and 0x80d7fec), stored by `fistpl` under a truncating control word (`or $0xc00` at 0x80a4e03).
- VERIFIED: `Scr_SetTime` subtracts the clock from the masked target and takes the `jle` at 0x80aa3ea when the difference is not positive; it then stores the target as the clock (0x80aa462) and runs nothing.
- VERIFIED: the two refusals come before the rounding. A wait negative or unordered against 0 (`fcom`, `and $0x5,%ah`, `jne` at 0x80a4dc6..0x80a4dd3) errors with `negative wait of %g is not allowed` (0x80d7c80). Otherwise one not below 16777.0 (`fcoms 0x80d7fe4`, `and $0x45,%ah; cmp $0x1,%ah` at 0x80a4dd9..0x80a4de7) errors with `wait of %.0f seconds is too long` (0x80d7c40). Both go through the runtime-error path at 0x80aa158.
- VERIFIED: `svs.time` (0x83b67a4) has no store in `cod_lnxded` other than the `addl $0x64` settle steps (0x8083f40, 0x808a650) and the frame loop's `add` (0x808d1ab), and a load hands it to `G_InitGame` as `level.time` (`cod11-map-cycle.md` 3, step 19). INFERRED from those: `level.time` carries across map changes for the life of the process, so the wrap comes 16,777,216 ms (about 4.7 hours) into server uptime, not into one level. The engine's own time-wrap restart (`EXE_SERVERRESTARTTIMEWRAP`, 0x80d5c8d) tests `svs.time > 0x70000000` (0x808ced4), far later.

INFERRED from those: on the frame whose `level.time` crosses a multiple of 2^24, the masked target is behind the clock, so `Scr_SetTime` jumps the clock there and runs only what `Scr_RunCurrentThreads` ran, the clock's own millisecond. A thread due between the old clock and 2^24, or below the new masked target, does not run until a later walk crosses its due time again, about 4.7 hours on. A thread due on the target itself runs the next frame as usual, and later deadlines count on the wrapped clock. `getTime` reads `level.time`, not the clock, so it is unaffected.

vcod's `Vm` keeps the clock and every deadline under `CLOCK_MASK`, and `run_frame` picks a waiting thread only when it is due on the clock or strictly between the clock and the masked target, which is the jump above. The `Wait` opcode refuses a negative (or NaN) wait and one of 16777 s or more with retail's two messages, handled as any runtime error ("Runtime errors below developer 1").

**`getTime` in a callback a cmd raises reads the last frame's `level.time`.** VERIFIED (`client-probes/probe_fall`, retail on `mp_carentan`, `crates/server/tests/fixtures/playerstate/mp_carentan-dm-fall-damage.txt`): every `PROBE damage <t>` line the wrapped damage callback logged names a `t` whose snapshot still carries the health the callback found (`t=19650 ... health=100` against `PROBE damage 19650 ... health 100`), and the snapshot 50 ms later is the first with the health it left (`t=19700 ... health=77`, `ct=19683`). The same holds for all five landings there and in the `-cvars`, `-corpse` and `-walk-corpse` captures. INFERRED, off that and the packet order in `cod11-combat.md` 16.1: `ClientThink_real` runs the landing cmd between `G_RunFrame`s, and `getTime` still reads the frame before. A shot's, a `trigger_hurt`'s and a `kill` command's callbacks run from the same place and read it too; a blast's (entity pass) and a turret round's (`ClientEndFrame`) run inside the frame and read that frame's.

vcod runs a cmd's callbacks inside `Server::replay_moves`, after the tick advanced its own clock, so `ScriptRuntime` leaves `host.level_time_ms` on the last frame's value until `run_threads` writes the new one; nothing on the callback path sets it. `fall_ab`'s `check_damage_clock` holds both sides to the snapshot rule.

**A receiver-less call keeps the caller's `self`, VERIFIED (`probe_self`).** A plain `f()`, a `[[ptr]]()` and a `thread f()` inherit the caller's `self`. Section 5's callbacks depend on it: `_callbacksetup.gsc` calls `[[level.callbackPlayerConnect]]()` with no receiver, and `dm.gsc:185` opens on `self.statusicon`. Unmeasured: what an explicit receiver does to the surrounding frames (vcod binds it to the callee's frame only, so `a f()` rebinds for `f` and the caller's `self` is back on return), and whether a builtin called without a receiver inherits it (vcod: script calls only).

**A thread survives notifying an event it has itself endon'd, VERIFIED (`probe_endon_self`).** A thread that registers `level endon("e")` and then executes `level notify("e")` runs past the notify and past its next `wait`: retail printed all four lines, the last after a one-second wait. Stock `dm.gsc` depends on it: `endMap` runs in a `Callback_PlayerKilled` thread that opened with `self endon("spawned")`, calls `spawnIntermission` (which notifies `"spawned"`) on every player including that one, then waits ten seconds and calls `exitLevel`. A VM that let the notify kill the notifier leaves a score-limit map stuck at the intermission camera. `Vm::notify_from` (`crates/gsc/src/vm/sched.rs`) skips the issuing thread; an endon fired by any other thread still kills (section 10).

**A notify acts where it executes, VERIFIED (`probe_endon_late`, retail 1.1d).** In one step a thread threads `early()` (registers `level endon("probe_kill")` and waits), executes `level notify("probe_kill")`, then threads `late()` (the same endon) and `late_wait()` (a `level waittill("probe_kill")`). Retail logged `late_armed`, `notified`, `late_survived` and `done`, and neither `early_survived` nor `late_wait_woken`: the notify killed the endon registered before it, spared the one after it, and did not wake a `waittill` registered after it. vcod resolves `Op::Notify` on the spot (`vm/interp.rs`, `Vm::notify_from`). Trap: queuing the notify to the end of the step kills `late`. UO's stock `sd`, `re`, `bas`, `ctf` and `dom` need the on-the-spot rule: `checkMatchStart` does `level notify("kill_endround")` then `level thread endRound(...)`, whose first line is `level endon("kill_endround")`; a queued notify kills the match-start round reset, so no round past the first starts.

VERIFIED (`probe_endon_after_notify`, 1.1d and UO, identical): a thread started right after a `level notify("x")` with `level endon("x")` first runs on (14 ticks of `wait 0.05`, against 4 had it died), and one that opens with `level waittill("x")` is not woken (0). UO's `hq.gsc` restarts its wave timer this way (`level notify("Timer Changed"); level thread hq_reinforcement_timer()`); a queued notify freezes `level.counter`.

**Two notifies of one event in one step: the second is lost, VERIFIED (`probe_notify_twice`, retail 1.1d and UO 1.51b, identical).** A pump runs `level notify("tick", 1); level notify("tick", 2); level notify("first"); level notify("second")` in one step, against a thread that waits on `"tick"` twice, one that waits once, and one that waits on `"first"` then `"second"`. Retail logged `pumped`, `chain_first`, `twice_1 1`, `once 1`, `done`, and neither `twice_2` nor `chain_second`. INFERRED: the first notify wakes a waiter and takes it off the event; the waiter does not run before the notifier's step ends, so the second notify finds nobody, and a woken waiter reads the first notify's arguments. The threads run newest wake first (`chain_first` ahead of the `"tick"` waiters), one notify's waiters in start order, as in the thread pick. vcod's `Vm::notify` only flips the waiter to runnable (`sched::tests::two_notifies_of_the_same_event_in_one_step_lose_the_second`).

**Engine notifies and the trigger queue (`game.mp.i386.so` 1.1d, 1.5b, UO).** INFERRED, from the bullets below: what the game module notifies outside a script step only wakes waiters, which run at the next `Scr_RunCurrentThreads`, so a notify raised in every cmd reaches a waiter once a frame.

- VERIFIED: `G_TouchTriggers` notifies `"touch"` on the spot (0x3fa4d, 0x3fa6f; `cod11-combat.md` 14.7), and `Cmd_Activate_f` its `trigger_use`'s `"trigger"` (0x48547; `cod11-gametypes-re-bel.md` 4).
- VERIFIED (`client-probes/probe_useby` on 1.1d, 1.5b and UO 1.51b, `crates/server/tests/fixtures/triggers/mp_carentan-dm-useby*.txt`, `cod11-combat.md` 5.9): `useBy` notifies `"trigger"` with its argument on the spot, its waiter logging in the frame of the call, on a `trigger_once`, a `trigger_multiple`, a `trigger_damage` and a `func_static` alike. The use function it then calls queues nothing (`Use_Multi` has no notify, `Use_trigger_damage` passes mod -1).
- VERIFIED (`client-probes/probe_bel` on retail 1.1d, `cod11-gametypes-re-bel.md` 8.7): the menu thread `Cmd_MenuResponse_f`'s `menuresponse` wakes reads the new frame's `getTime()` and runs ahead of a `wait 0.05` due that frame, so it too waits for the frame's first round. vcod matches ("The packet pass"); `gametypes_ab`'s `spawn clock` rows gate it.
- VERIFIED: `Touch_Multi` (0x65a18), `hurt_touch` (0x64dc4), `Activate_trigger_damage` (0x650c0) and `G_Trigger` (0x656f0, the `trigger_lookat` path) do not notify `"trigger"`. Each appends (entity number, activator number, both entities' `+0x300` use counts) to a 12-byte-entry queue at `level+0x1dec`, its count at `level+0x29ec`. Only a full queue (0x100 entries) notifies on the spot.
- VERIFIED, `G_RunFrame` 0x50529..0x50665, its first work after the cvar update: a 0x400-byte seen table is zeroed once; each round bumps a byte marker and walks the queue in order. An entry whose two use counts no longer match is dropped. One whose entity was already sent this round stays and marks the round for another. The rest get the activator pushed and `Scr_Notify(ent, "trigger", 1)` (0x505f6). A sent or dropped entry is replaced by the last, which is examined next (0x505fe..0x50635). Each round ends in `Scr_RunCurrentThreads` (0x50653); rounds repeat while a round kept an entry, and the count is zeroed after the last (0x50665), so what a round's threads queued is dropped unless a later round runs.
- VERIFIED (`client-probes/probe_touchorder`, `cod11-combat.md` 14.7): every frame a player stands in two minefields logs one `"trigger"` pair, then the `"touch"` lines, then one more `"trigger"` pair per later cmd of the frame. INFERRED, from that log and the code above: the first round runs the waiters its triggers woke ahead of those the cmds' touches woke, last woken first, and each later round runs the next cmd's triggers.
- VERIFIED: 1.5b's `G_RunFrame` (0x506c9..0x5082a, count at `level+0x2e10`) and UO 1.51b's (0x59cf5..0x59e56, `level+0x2e60`) run the same rounds. 1.5b's `G_Trigger` (0x69f68, queue at `level+0x2210`) and UO's (0x76218, `level+0x2260`) are the append, with the same 0x100 check; their `Touch_Multi` calls it.
- VERIFIED (`client-probes/probe_touchorder` on retail 1.5b and UO 1.51b, `tests/fixtures/touchorder/mp_pavlov-1.5.txt`, `-uo.txt`): every phase's first frame logs the same lines in the same order as 1.1d's, and each later cmd of a frame one more `f` pair.
- VERIFIED (`client-probes/probe_trigqueue` on retail 1.1d, 1.5b and UO 1.51b, three byte-identical logs, `tests/fixtures/touchorder/trigqueue.txt`): a script-spawned `trigger_damage` with `enableLinkTo` (no wait armed) takes N `radiusDamage`s in one step from a thread on the frame grid, which runs inside the first round. N = 1 or 256 log no `"trigger"`: every entry is dropped with the round's queue. N = 257 or 259 log one, at the step's own time: the 257th activation notifies on the spot, its waiter runs after the step, and later spot notifies reach no waiter. From a thread woken off the grid (`wait 0.06`), which runs after the rounds, N = 1 logs one `"trigger"` the next frame, and N = 258 logs one at the step's time and 256 the next frame, one per round. INFERRED: on 1.5b and UO too, a thread due past the rounds' clock runs after the count is zeroed, so its entries wait for the next frame's rounds.

vcod: `engine_notifies` holds what retail notifies on the spot, `trigger_fires` the queue. `ScriptRuntime::run_threads` raises the first at the top of the script frame; `ScriptRuntime::trigger_rounds` hands out the second. `GameHost::queue_trigger` refuses an entry past 0x100 (`TRIGGER_QUEUE_LEN`); a cmd's touch and the lookat path then push the notify to `engine_notifies`, and a `trigger_damage` activation returns it with `"damage"` from `damage_nonclient`, so a builtin's blast notifies it inside the step. Gate: `crates/server/tests/touchorder_ab.rs`, both probes at each patch. A notify raised between frames (`ClientDisconnect`'s `menuresponse`, `ClientBegin`'s `begin`, a client command's) wakes its waiters at once, and they run in the next frame's first pass ("The packet pass"; `cod11-server-handshake.md`, "When the game hears a drop" and "When the client's `begin` is heard").

**`getentarray` order: map entities first, then spawn order, VERIFIED.** The map's four `script_origin` entities come back before three the probe spawned, those in spawn order. The probe prints only `targetname` (`probe_ents.gsc:32`), so the order within the map's four, and its key, are unmeasured.

**`i%count` compiles on retail, VERIFIED.** `7 % 3` written tight gives `1`, so retail does not lex `%` before an identifier as an anim reference there. vcod's lexer looks back one token: `%` starts an anim reference only where the previous token cannot end an operand (not an identifier, number, `)` or `]`). Over the corpus that reclassifies nothing: 3432 `Anim` tokens before and after.

**`-2147483648` is not reachable on retail, VERIFIED.** The literal and `(int)(-2147483648)` both give `-2147483647`: the magnitude saturates at `i32::MAX` before the minus applies. vcod's `read_number` does the same, which is what the two stock scripts with a literal past `i32::MAX` as a sentinel mean (`maps/carride.gsc:1606`, `maps/redsquare.gsc:1547`).

**`game` is array-typed, not a struct, VERIFIED (`probe_game`).** `game["allies"] = "russian"` round-trips, `game.size` is 1 after one bracket write and 2 after two, and the value survives a `wait`. `game.foo = 1` is a compile-time `not an object` that voids the script before `main()` (`probe_game_dotwrite`). `level["k"] = "v"` is the compile-time `not an array, string, or vector` (`probe_level_bracket`), and so is `self["k"] = 1` (`probe_self_bracket`). VERIFIED on 1.1d and UO 1.51b, each console naming the line and the message. vcod models `game` as an array (`Vm::game`, `ArrayId`; `Op::LoadGame` pushes `Value::Array`).

**Why: the compiler types the bare globals.** VERIFIED (the switch and the string): cod_lnxded 1.1d's object emitter (0x08096d34) switches on the operand's node kind; four kinds compile (three of them keyword nodes 0x1c, 0x1d and 0x1f, emitting opcodes 7, 8 and 9) and any other is `not an object`. VERIFIED: the array emitter (0x08096bb8) has its own set, holding keyword node 0x1e (opcode 0xf) and none of the other three, and fails the rest with `not an array, string, or vector`. VERIFIED (the probes): `game` is 0x1e, `level` and `self` are object nodes. INFERRED from the keyword set (no probe reaches it): the third object node is `anim`. Three emitters call the object emitter (0x08093e44, 0x08093f98, 0x080940d8) and several the array emitter. INFERRED from that, not probed: field and index reads go through them as writes do. vcod refuses `game.x`, `level[k]`, `self[k]` and `anim[k]`, read or written (`Compiler::check_object`, `check_array`, `crates/gsc/src/compile.rs`). `.size` is not a field: the interpreter runs it as opcode 0x4e (VERIFIED, cod_lnxded's dispatch calls 0x080a2f28 there), and `game.size` and `level.size` both compile (VERIFIED, `probe_game`, `probe_level_size`), so vcod skips the object check for it.

**`.size` of an object that is not an array is 1 (`probe_level_size`).** VERIFIED on 1.1d and UO 1.51b: with one or two dot-fields set, `level.size` reads `1`. VERIFIED, the size handler (cod_lnxded 0x080a2f28): a string gives its length; an object (type 7) gives the element count when it is an array (type 0xf) and `1` for any other kind (struct, entity, hudelem); any other operand is `size cannot be applied to %s`. vcod's `Op::LoadField` answers 1 for a `Value::Struct` or `Value::Entity` under `size`.

Three of the server-crate probes measure the gametype bootstrap; each needs the cvar table or a real map load:

**Bootstrap order, VERIFIED (`probe_bootstrap`).** The gametype's `main()` runs first, then the map script's, then the gametype's `Callback_StartGameType`. On `mp_pavlov`, whose map script sets `game["allies"]`, it reads `undefined` in the gametype's `main()` and `russian` by `Callback_StartGameType`. Same run: a bare `thread f()` runs `f` to its first suspend before the caller's next line, so a thread that never waits has finished when the call returns.

**Cvar coercion, VERIFIED (`probe_cvar`).** `getCvarInt` and `getCvarFloat` read an unset or non-numeric cvar as `0` and take a numeric prefix: `getCvarInt("12abc")` is `12` (Rust's `parse` rejects it). Cvar names are case-insensitive: `probe_MixedCase` and `probe_mixedcase` read one value. `randomInt(1)` never returns `1`: the bound is exclusive. `getTime` is non-negative, in milliseconds of `level.time` ("The script clock").

**`delete()` defers the free, VERIFIED (`probe_delete`).** Right after `delete()` the entity stays in `getEntArray` and its count, and a spawn takes a fresh number. After a 150 ms wait the number is back in circulation. The mechanism and the 100 ms the engine arms are `cod11-gsc-object-model.md` section 14; the probe pins the bound, not the constant, since its frames step 50 ms.

**A handle outlives its object as a dead entity (`probe_stale_handle` and six `probe_stale_*` probes, one per fatal case).** What a script reads through a handle to a freed entity or hudelem, held in a local, a `level` field or another entity's field:

- VERIFIED: inside `delete()`'s 100 ms window the entity is live: `isDefined` 1, a field reads and takes a write, `getEntityNumber` answers its number.
- VERIFIED: past the free, `isDefined` reads 0, also once a later spawn takes the same number (`stale_ent_slot_reused 1`).
- VERIFIED: `==` between the dead handle and the slot's new occupant is 0; between the dead handle and its copy in `level`, 1.
- VERIFIED: a hudelem is dead the moment `destroy()` returns: `isDefined` 0 at once, through a local, `level` and an entity field (`sd.gsc`'s `other.progressbackground` shape), and `==` against the next `newHudElem` is 0. Script has no number for a hudelem, so whether that element reused the record is unknown.
- VERIFIED: a field read or write through a dead handle, entity or hudelem, is fatal with `dead entity is not an object`; a method call (`getEntityNumber`, a second `destroy`) with `dead entity is not an entity`.
- VERIFIED: `dead entity` is an entry of the VM's value type-name table in `cod_lnxded` (string 0x80d758c, table 0x80e3180, beside `entity`, `dead thread` and `dead object`). The messages are `%s is not an object` (0x80d773d, 0x80d7d16) and `%s is not an entity` (0x80d7be9). The free reaches the VM through the game module: `G_FreeEntity` calls `Scr_FreeEntity` at 0x66b7d, which calls `Scr_FreeEntityNum` at 0x62066; `HudElem_Free` calls `Scr_FreeHudElem` at 0x4c57e; `Scr_FreeEntityNum` (0x6c0f8) calls through the engine import slot at 0xc1088 (call targets off the relocations).
- INFERRED, from the message and the chain: the free turns the script object into the dead type in place, so every copy of the handle dies at once and a new spawn gets its own object.

vcod uses a generation. `EntId` carries the slot's generation beside its number; `ObjectTable` bumps it on every free (entity, hudelem, client). A handle whose generation is stale reaches nothing; `Host::is_live` tells the interpreter, which raises retail's two messages on a field access or method call; `==` compares both halves, so a handle equals only its copies. Unmeasured: a dead handle passed to a builtin other than `isDefined` (`linkTo(dead)`), or used with `notify`, `waittill`, `endon` or `thread`; vcod leaves each to the builtin or the scheduler.

**Freeing an object ends every thread whose `self` it is (`probe_stale_self`, `probe_stale_self_hud`).** VERIFIED on 1.1d and UO 1.51b (`probe_stale_self` on both, the hudelem case on 1.1d): a thread on an entity asleep in `wait` over the free, and one waiting on a `level` notify sent after the free, never log again and raise no error. A thread on `level` holding the entity in a local wakes and reads `isDefined` 0. A thread on a hudelem that `destroy()` frees ends the same way. INFERRED, from the free chain: the end rides `Scr_FreeEntityNum`'s engine call. vcod drops such a thread when the scheduler picks it (`Vm::step_runnable`, through `Host::is_live`); no script can tell this from an end at the free, since the thread runs no step in between. The stock jeep self-destruct timer depends on it (`_jeepdrive_gmi::process_deactivate`, `client-probes/probe_jeepdeact`, `crates/server/tests/fixtures/vehicles/mp_foy-jeep-deactivate-uo.txt`): a jeep destroyed while it waits is deleted, and retail runs past the timer's `dodamage` line with nothing in its console.

### Runtime errors below developer 1

Every probe capture in this section was taken at `developer 1` (`tools/run_probe.sh`), where a runtime error is fatal. Below `developer 1`, retail's default and vcod-server's, the server runs on.

- VERIFIED (decompile): `Scr_Init` takes its two flags from cvars at each level's script init. coduo_lnxded 0x80714f1 passes `developer || logfile`, `developer_script` and `developer` to 0x80aa818, which stores them at 0x829fa00, 0x829fa04 and 0x829fa08.
- VERIFIED (decompile): `RuntimeError` (0x80a3f4a) prints only when the first flag or the terminal flag (0x8433eac) is set, and calls `Com_Error` only when `developer` (0x829fa08) or the terminal flag is.
- VERIFIED (decompile): `Scr_TerminalError` (0x80b055c) sets the terminal flag. Its callers are variable-pool exhaustion (`exceeded maximum number of script variables`) and `potential infinite loop in script`.
- VERIFIED (decompile): below that, `VM_Execute`'s error arm (the switch on the failed opcode after 0x80ab5ac's dispatch) frees the instruction's operands, leaves its result undefined, steps past its operand bytes, and the loop runs on; a conditional jump pops its condition and is not taken. INFERRED, read off the arms: every arm has this shape.
- VERIFIED (`client-probes/probe_undef_if` at `developer 0`, 1.1d on mp_carentan and UO 1.51b on mp_cassino): `if (level.unset)` and `if (!level.unset)` both take their branch, the statements after run, `level.unset + 1`, `!level.unset` and `level.unset_struct.field` read undefined, `wait level.unset` returns at once (0 ms), and that thread and another run on. A builtin's own error (`distance(undefined, ...)`) reads undefined on 1.1d and aborts the process (SIGABRT) on UO 1.51b. At `developer 1` the same probe dies on the first `if` with `cannot cast undefined to bool` and `ERROR: script runtime error` (VERIFIED, UO).

vcod takes the mode from `developer` at the load (`Vm::set_recover_errors`, `ScriptRuntime::load_from`). On, a failed instruction leaves `stack_effect`'s results undefined and the thread runs on (`Vm::step_frames`); the error is still recorded in `Vm::aborts`. vcod's own errors (a builtin it lacks, its instruction cap, a wait in `call_now`) still end the thread, and a builtin's error recovers on UO as on 1.1d instead of ending the process. A gate replaying a `run_probe.sh` capture runs at `developer 1` (`semantics_ents.rs`'s `probe_cvars`, `linkto_ab.rs`). Stock UO `tdm.gsc` 815 shows it in play: a death to no player (a tank its own attacker, coduo-vehicles.md 11.6) reads the unset `level.teamkill_penalty`, takes the branch, and costs the victim's team a point.

### The map-cycle builtins, and the `sd` connect race

Five builtins end or restart a level or set the scores it ends on. Addresses: `cod11-map-cycle.md` sections 1, 2 and 6, and `cod11-sound-system.md` section 9.

**`map_restart(savePersist)` and `exitLevel(savePersist)` both queue a console line and return, VERIFIED (map-cycle sections 1 and 2).** `map_restart` queues `map_restart`; `exitLevel` clears the team scores and connected clients' scores and queues `map_rotate`. The engine runs the line at the top of a later frame, so the statements after the call still run. The pair shares one per-level latch; a second call is a fatal script error naming whichever ran first. The argument is the whole persistence gate: `game[]` and every client's `pers[]` carry into the next level when it is non-zero and are freed when it is zero, on both paths. `level` is always new, and every entity handle in `game[]` is dropped whatever the flag. VERIFIED (`probe_persist_restart`, `probe_persist_exit`, `probe_persist_exit_save`): `map_restart(true)` keeps `game[]`, `exitLevel(false)` frees it, `exitLevel(true)` keeps it, and in all three the entity handle is gone. `dm.gsc` passes 0 at its map end and `sd.gsc` 1 at its round end, so a `dm` client goes back through the team menu after a restart and an `sd` client does not.

**`getTeamScore(team)` and `setTeamScore(team, n)` take `"allies"` or `"axis"` and nothing else, VERIFIED (map-cycle 6.3).** Any other string is fatal. The setter writes configstring 5 for axis and 6 for allies, `%i`, and dirties the flag whose drain pushes the `b` scoreboard to every client in intermission. It and `CalculateRanks` are that flag's only writers. The one other unrequested scoreboard is `player_die`'s, to the victim's followers (`cod11-spectator-follow.md` 4).

**`playLocalSound(alias)` reaches one client only.** VERIFIED (`cod11-sound-system.md` section 9): it gives the alias the first empty configstring in the `CS_SOUNDS` block, at first use, and sends that client the reliable `s <index>`, non-positional, no entity, no origin. The receiver must be a player; a spectator counts, which is how a headless probe hears the announcer. The stock gametypes' round-end lines use it.

**`sd.gsc`'s `Callback_PlayerConnect` opens the team menu before it suspends, VERIFIED from the stock script.** The `else` arm calls `openMenu(game["menu_team"])`, then `spawnSpectator()`. After its `notify`, `resettimeout()`, session-state assignments and the `self spawn(...)` that places the camera, that calls the file's own `updateTeamStatus()`, which opens with `wait 0` and a comment saying the wait lets `Callback_PlayerDisconnect` finish first. INFERRED, from that order against the scheduler: the `t` command is on the wire while the connect thread is suspended, before it reaches the `waittill("menuresponse")` loop, so a menu answer arriving in that one frame notifies an event nothing waits on and is lost. `dm.gsc` has no wait between its `openMenu` and its loop.

### Precache builtins

The builtin table (`tools/re/dump_builtins.py`) has nine precache builtins: `precacheModel` (0x5ecb8), `precacheShellShock` (0x5ecf0), `precacheItem` (0x5ed28), `precacheShader` (0x5eda8), `precacheString` (0x5ee00), `loadFx` (0x5ef88), `precacheMenu` (0x5a92c), `precacheStatusIcon` (0x5aa08) and `precacheHeadIcon` (0x5aae4). MP has no `precacheTurret` or `precacheFx`.

- VERIFIED, disassembly: the first five test `level.initializing` (`level+0x1c`, set by `G_InitGame` while the gametype's `main`, the map's `main` and `Callback_StartGameType` run to their first wait, `cod11-events-and-fx.md` 2) before reading their argument, and on 0 raise `Scr_Error("<builtin> must be called before any wait statements in the gametype or level script\n")` (strings 0x77580, 0x775e0, 0x77640, 0x776c0, 0x77740). The test ignores the name, so a name already precached is refused too.
- VERIFIED, disassembly: the menu and icon builtins have no such test. Each scans its range with `Q_stricmp`. A match is `Com_DPrintf("Script tried to precache the <menu|player status icon|player head icon> '%s' more than once\n")` and nothing else. A full range is `Scr_Error("Too many menus precached. Max allowed menus is %i")` (32), `"Too many player status icons precached. Max allowed is %i"` (8) or `"Too many player head icons precached. Max allowed is %i"` (15) (0x77180..0x77360).
- VERIFIED, disassembly: of the eight others none but `loadFx` calls `Scr_AddInt`, so each returns `undefined`. `precacheShader` raises `Scr_ParamError(0, "Shader name string is empty")` on `""`; `precacheString` skips an empty key; `precacheItem` raises `unknown item '%s'` when `BG_FindItem` misses.
- VERIFIED: every indexer behind them (`G_ModelIndex` 0x66ed8, `G_ShaderIndex` 0x65ee8, `G_LocalizedStringIndex` 0x65e30, `G_ShellShockIndex` 0x67540, `G_EffectIndex` 0x65fa4, `G_FindConfigstringIndex` 0x66e30, `G_TagIndex` 0x66fa8, `G_SoundAliasIndex` 0x675c0) matches with `strcasecmp`. The hint-string indexer (0x5a238) is the one `strcmp`.
- VERIFIED, `crates/gsc/tests/fixtures/semantics/probe_precache_*.gsc` on retail 1.1d, `retail-captures.txt`: every precache returns `undefined` (`isDefined` 0). Past one `wait 0.05`, a new menu, head icon and status icon are taken, while `precacheModel`, `precacheShader`, `precacheString`, `precacheItem` and `precacheShellShock` of a name the level already precached each die with their message. A status icon in another case takes no new slot. The ninth status icon dies with `Too many player status icons precached. Max allowed is 8`. An empty model or string passes; an empty shader dies; an unknown item dies.

What the indexers do past initializing is the next subsection but one.

vcod: `crates/server/src/game/builtins/precache.rs` and `Allocators::index`'s case-folded scan; A/B `probe_precache_wait_matches_retail` in `crates/server/tests/semantics_ents.rs`.

### Arrays are values

VERIFIED (`probe_array_copy`, 1.1d on mp_pavlov): after `b = a`, a write through either leaves the other's size alone (`local_copy_write 1 2`, `local_orig_write 2 1`). Likewise a string key written into a copy of a `level` field (`field_copy_strkey 1 2`), a write through an argument (`arg_write 1`), into a copy of an outer array's nested array (`nested_write 1 2`), into a copy of an element (`element_copy_write 1 2`) and into a field assigned from another field (`field_to_field 1 2`). CoD4 1.7 prints the same lines, and there a struct an array holds is shared (cod4-gsc.md 10). vcod flags an array stored a second time and copies it at the next write (`Heap::share`, `Heap::own`). INFERRED: no 1.1 stock script depends on shared arrays, since vcod ran the corpus with them shared before this.

### Indexing past the first wait

Which builtins index a configstring at run time comes from the relocations against each indexer in `game.mp.i386.so` (`readelf -r`, mapped onto the tables `tools/re/dump_builtins.py` prints). Addresses are `.so` file addresses.

- INFERRED, from the branch on `level+0x1c` in each: `G_ModelIndex` (0x66ed8), `G_ShaderIndex` (0x65ee8), `G_LocalizedStringIndex` (0x65e30) and `G_EffectIndex` (0x65fa4) scan their range with `strcasecmp` and return a match at any time. A miss allocates only while `level.initializing` (`level+0x1c`, loaded at 0x65ef4 in `G_ShaderIndex`) is set; on 0 it is `Scr_Error(va("model '%s' not precached"))` (0x79b3a) for a model and `Scr_Error(va("%s \"%s\" not precached", "shader" | "localized string" | "effect"))` (0x79ae0) for the rest. `""` is index 0 in all four.
- VERIFIED, relocations and disassembly: `G_SetModel` (0x67020, behind `setModel`) and `G_EntAttach` (0x66190, behind `attach`) inline `G_ModelIndex` with the same gate and message (0x67088, 0x66247). `setViewModel` (0x4512c) calls `G_ModelIndex`. `setShader` (0x4b5b0), `objective_add` (0x5a2d0) and `objective_icon` (0x5a5d8) call `G_ShaderIndex`; `setText` (0x4c590) and the hud `label` setter (0x4c400) call `G_LocalizedStringIndex`.
- INFERRED, from the parameter-count compares at 0x4b5c3: `setShader` takes one parameter or three; with one, width and height are 0 (0x4b609).
- INFERRED, from the absence of a `level+0x1c` load in either: `G_ShellShockIndex` (0x67540, called only by `precacheShellShock`) and `G_SoundAliasIndex` (0x675c0, behind `playSound`, `playLoopSound`, `playLocalSound` and the spawn functions) have no gate; a new name allocates at any time.
- INFERRED, disassembly: the `statusicon` setter (0x41b50) goes through `GScr_GetStatusIconIndex` (0x5c7c8), the `headicon` setter (0x41bd4) through `GScr_GetHeadIconIndex` (0x5c840). Neither allocates or reads `level.initializing`: `""` is 0, a `Q_stricmp` match over the 8 (15) slots is its 1-based slot, and anything else is `Scr_Error("Status icon '%s' was not precached\n")` (0x772c0) or `"Head icon '%s' was not precached\n"` (0x773a0), during initialization too. The getter reads the slot back, so an icon set in another case reads in its precached spelling.
- VERIFIED (`probe_late_*.gsc` on retail 1.1d, `retail-captures.txt`): past one `wait 0.05`, names precached in lower case and set in upper case pass `setModel`, `attach`, `setShader` (three arguments and one), `setText`, `label`, `objective_add`, `objective_icon`, `statusicon`, `headicon`, `shellshock` and `setViewModel`; `.model`, `getAttachModelName`, `.statusicon`, `.headicon` and `getViewModel` read back the precached spelling. `playSound` of an alias nothing indexed passes. A name nothing precached dies in `setModel`, `attach` and `setViewModel` (`model 'xmodel/probe_never' not precached`), `setShader` and `objective_add` (`shader "probe_never" not precached`), `setText` and `label` (`localized string "PROBE_NEVER" not precached`), `statusicon` (`Status icon 'probe_never' was not precached`) and `shellshock` (`shellshock 'probe_never' was not precached`). The player-scoped probes run on an `addTestClient` client, whose first bot retail refuses within `sv_reconnectlimit` of the start (`reconnect rejected : too soon`; `cod11-gsc-object-model.md` 25), hence their `wait 5`.
- VERIFIED, `dump_builtins.py` and disassembly: `shellshock` and `stopShellShock` are generic entity methods 28 and 29 (0x5bb44, 0x5f468) that refuse a non-client (`only valid on players; called on entity %i at ...`).
- INFERRED, disassembly: `shellshock(name, duration)` wants exactly two parameters (`USAGE: <player> shellshock(<shellshockname>, <duration>)\n`). It scans configstrings 1101..1115 (`0x44c + i`, `i` 1..15) with `strcasecmp`, empty slots included, so `""` finds the first free slot; a miss is `shellshock '%s' was not precached\n` (0x780e0). The duration is `(int)(duration * 1000 + 0.5)` (0x5bc7a, constants 0x78104 1000.0 and 0x78108 0.5, truncating). Above 60000 as an unsigned compare (so a negative one too) it is `Scr_ParamError(1, "duration %g should be >= 0 and <= 60")` (0x780a0) of that value times 0.001. Then `ps.shellshockIndex` (gclient +0x3dc) gets `i`, `shellshockTime` (+0x3e0) `level.time`, `shellshockDuration` (+0x3e4) the milliseconds (0x5bcf1..0x5bd14).
- VERIFIED (`probe_late_shockspan`): `shellshock("default", 61)` dies with `duration 61 should be >= 0 and <= 60`; 0, 1.5 and 60 pass.
- INFERRED, disassembly: `stopShellShock()` takes no parameters (`USAGE: <player> stopshellshock()\n`) and zeroes the three fields.
- INFERRED, from `ClientSpawn`'s `bzero` of the `gclient_t` (0x42804, 0x22c4 bytes, the session block at +0x20d0 saved around it): a spawn clears the shellshock.
- The client effect is cgame's: `cod11-shellshock.md` has the `.shock` files, the 27 `cg_shock_*` cvars, the screen blend, view kick, mouse damping, sound mix and the `wbuttons` 0x4 slowdown, and notes no stock MP script calls `shellshock`.

vcod: `Allocators::index_precached` (`crates/server/src/configstrings.rs`) gates `setModel`, `attach`, `setViewModel`, `setShader`, `objective_add`, `objective_icon`, `setText`, `label` and `loadFx`; `setModel` and `attach` keep the precached spelling. The `statusicon` setter refuses an icon not precached and keeps the precached spelling. `shellShock` and `stopShellShock` (`builtins/client.rs`) write `GameHost::client_shellshock`, which the sim sends as the three playerstate fields; the client half is `play::shock` (`cod11-shellshock.md`). A/B: `probe_late_index_matches_retail` in `crates/server/tests/semantics_ents.rs`.

### `vectorToAngles`

VERIFIED, `vectoangles` (1.1d 0x3de94, `.rodata` 0x72ae8..0x72b08): yaw is `atan2(y, x) * 180 / PI`, pitch `atan2(z, sqrt(x² + y²)) * -180 / PI`, each stored as a float and given 360 while negative; roll 0. A vector with no horizontal part reads pitch 270 when `z > 0`, else 90. Unlike Q3's, which negates a pitch already wrapped, both angles come back in [0, 360): 16 degrees down reads 15.99, not -344.01. VERIFIED (retail UO's log of `probe_dmgtouch` and `probe_belltower`, cod11-combat.md 14.8): `(15.99, 180.00, 0.00)`, `(34.99, 0.00, 0.00)` and `(51.34, 90.00, 0.00)` for three downward aims. vcod: `builtins::math::vector_to_angles` (Q3's negative pitch is the trap).

### Casting a string

VERIFIED, cod_lnxded 1.1d: the casts to bool (0x080a3c30), int (0x080a3de0, the VM's case 0x31) and float (0x080a3fa0) read a string through `strtol(s, 0, 10)` and `strtod`, so the numeric prefix is the value, blanks and a sign included, the rest ignored. A result of 0 stands only when the first character past those (under `!`) and a sign is `0`, or `.` then `0`; otherwise the cast raises `cannot cast "%s" to int` (to bool, to float). UO's engine has the same three (coduo_lnxded 0x080aa9a0, 0x080aaae2, 0x080aac34).

VERIFIED (retail 1.1d's log of `probe_cast_string`):

| Cast | Result |
|---|---|
| `(int) "5000"` | 5000 |
| `(int) " -7"` | -7 |
| `(int) "12abc"` | 12 |
| `(int) "3.9"` | 3 |
| `(int) "0.9"`, `(int) ".05"` | 0 |
| `(float) "1.5e1x"` | 15 |
| `(int) "abc"` | fatal `cannot cast "abc" to int` |

A condition and unary `!` take the bool cast, so `!"0.5"` is 1: `strtol` stops at the point.

UO's stock `maps/mp/_birdbush.gsc` (pakuo01) adds `(int) self.script_noteworthy`, a BSP key and so a string, to `(int) gettime()` on every trigger of a `birdbush` brush (mp_uo_stanjel's, seen in the UO soak, `cod11-client-soak.md`). A VM whose `(int)` refuses strings returns undefined there, the next trigger compares undefined, and the birds never fly. vcod: `vm::interp`'s `string_int` and `string_float`, gated by `semantics_ab.rs` on the probe.

### Still unestablished

- `!"a"`: not put to a retail server. INFERRED from the bool cast above (0x080a3c30): `"a"` has no numeric prefix and is not `0`, so the cast raises, the same fatal as `!""`; `if ("a")` is fatal (`probe_truthy`). `not_of` (`crates/gsc/src/vm/interp.rs`) treats any unparseable string that way. Nothing on the bootstrap path reaches it: every `scr_allow_*` value is `"0"` or `"1"`.
- `undefined == undefined`: not probed. INFERRED: retail's mixed-case message names a "pair has unmatching types", which two `undefined`s are not, so equal. vcod answers true (`values_equal`, `crates/gsc/src/vm/interp.rs`); every mixed pair with `undefined` errors, which is measured.
- String comparison against a localized string (`&"KEY"`): no stock script does it. A message argument renders as `KEY\x15` (`crates/server/src/game/builtins/message.rs`), and so does a `setCvar` value: `_teams::scoreboard`'s `setcvar("g_TeamName_Allies", &"MPSCRIPT_AMERICAN")` reaches the retail gamestate as `MPSCRIPT_AMERICAN\x15` (VERIFIED, `crates/server/tests/fixtures/configstrings/mp_carentan-sd.txt`, slots 223 and 224). Plain concatenation is unmeasured.
- Equality outside the measured pairs: `values_equal`'s catch-all is `a == b` on `Value`'s derived `PartialEq`, so a mixed pair (`vector == "a"`, `entity == "a"`) answers `false`, while two vectors, entities, arrays or function pointers compare by value and can answer true. INFERRED from retail's "pair has unmatching types" message: the mixed case is fatal there. Of same-type cases only entities are measured (`probe_stale_handle`: a handle equals its copies, dead or not, and nothing else).

## 10. Divergences kept as documentation, not code

Deliberate choices the corpus cannot settle. Closed since: `delete()` deferring the free (`cod11-gsc-object-model.md` section 14, `probe_delete`), stale handles reading as retail's dead entity (`probe_stale_handle`), `game.foo` and `level["k"]` refused at compile time (`probe_game_dotwrite`), the second of two notifies in one step being lost (`probe_notify_twice`), and the `SP_` spawn layer (last entry).

- **A thread killed by *another* thread's `endon` notify mid-step keeps executing to its next suspend.** A thread's own `notify` never kills it (`probe_endon_self`, `Vm::notify_from`). The remaining case is a nested spawn's notify: `main`'s `self thread killer();` runs `killer` synchronously (`Vm::spawn`); if `killer` notifies the event `main` registered with `endon`, `Vm::notify` removes `main` from `self.threads` while `main`'s own `step_frames` is still running up the call stack. `main` runs on to its next `wait`/`waittill`/return; only then does `step_thread` find its entry gone and drop the frames. Example: `main() { self endon("die"); self thread killer(); sideEffectA(); sideEffectB(); wait 5; done(); }` ends with `threads=0`, both side effects run, `done()` never does. Pinned by `sched::tests::a_thread_killed_by_its_own_endon_mid_step_still_runs_to_its_next_suspend`. CoD2's stock tdm relies on the other rule, and retail 1.3 stops the thread at once (cod2-protocol.md 16.8): a CoD2 level turns on `Vm::set_kill_mid_step` (a list of threads a notify ended, checked before each instruction). 1.1 is unmeasured and keeps the old rule.
- **A bare local is accepted as a statement.** vcod compiles `x;` (and so `waittillframeend;`, which 1.1 has no keyword for) as a local read and a pop. VERIFIED, a 1.1d probe: retail refuses both as `bad syntax` (cod2-gsc-diff.md 3.2). No stock 1.1 script has one; CoD2's `waittillframeend` lives behind `Dialect::Cod2`.
- **A runtime error aborts the thread, where retail kills the server.** At `developer 1` every fatal in section 9 (a non-numeric condition, `"str" + undefined`, `undefined == 0`, `"a" < "b"`, `1 / 0`) raises an `ErrorKind` that unwinds that thread and logs; other threads and the server keep running, where retail prints a `script runtime error` block and shuts down. A third-party map script must not take the server down. Below `developer 1` vcod runs on as retail does ("Runtime errors below developer 1"). `crates/gsc/tests/semantics_ab.rs` checks only that both sides stopped at the same expression.
- **Multiple `#using_animtree` directives in one file resolve to the last one.** `compile_file` (`crates/gsc/src/compile.rs`) takes `file.animtrees.last()` because the AST does not track which directive was active at each function. No stock MP script reads a bare `#animtree` back. Across the whole corpus, of the 38 files that read `#animtree` in expression position, 35 carry more than one `#using_animtree`, so "last wins" would be wrong there. The fix belongs in the parser, the only layer that knows each function's position relative to the directives.
- **Array iteration order is interning order, not retail's.** `ArrayKey`'s derived `Ord` (`crates/gsc/src/heap.rs`) puts every `Int` before every `Str` and orders `Str(Atom)` by interning order. 1.1 has no key-listing builtin, so no script sees it. CoD4's `getArrayKeys` lists through `array_keys_newest_first` instead.
- **A float array subscript truncates toward zero.** `array_key` (`crates/gsc/src/vm/interp.rs`) turns `a[1.9]` into `a[1]`, as the `(int)` cast would. Retail is unmeasured and may be fatal.
- **A vector refuses every key but the integers 0, 1 and 2.** `LoadIndex` reads `v[0]`..`v[2]` as a float and answers any other key (out of range, or a float) with `BadType`; a store into a component is refused the same way. So a float key on a vector is refused where one on an array truncates. Retail is unmeasured for both; the corpus pins only the in-range integer read (section 2).
- **Every endon kill lands before any notify wake.** `Vm::notify` (`crates/gsc/src/vm/sched.rs`) makes two passes over `threads`, kills then wakes, so a thread with both an `endon` and a `waittill` on one event is killed, whichever it registered first. The order within each pass is measured (start order, `# probe_notify`); the order between the passes is not.
- **`radiusDamage` walks clients, turrets and damageable entities, measured standing.** The falloff, line of sight, direction and callback timing are settled. VERIFIED: the builtin (`.so` 0x5eef4) and `G_RadiusDamage` (`.so` 0x4a3f4) give the linear curve from `maxDamage` at the blast to `minDamage` at the range and `CanDamage`'s five-trace fraction, which `crate::game::combat`'s `Blast::hit` and `can_damage` compute, second-chance arm included (`cod11-combat.md` section 14). VERIFIED: each probe is `trap_LocationalTrace(&tr, point, origin, targ->s.number, 0x02802091, bulletPriorityMap)` (14.3). INFERRED, off that mask and pass entity: another live player's body stops a probe, the victim's own and a corpse do not. The retail runs of `client-probes/probe_blastbody` measure it (14.4: 13 of a flat 20 behind a standing player, 6 or none at other yaws, 20 behind its corpse), and vcod's `can_damage`, tracing every other live player's posed body, reads the same numbers. Callbacks follow retail's walk (14.5): each victim's `CodeCallback_PlayerDamage` runs to its first `wait` before the next victim is measured, as a `Cx::spawn_then` whose `Host::spawn_returned` takes the next turn, and the walk ends before the caller's next instruction, so a script that damages and then reads `self.health` sees what the callbacks left. What remains:
  - **The victim walk.** VERIFIED: retail walks `trap_EntitiesInBox` over a `radius * sqrt(2)` box, and the loop body reads `takedamage` and the entity's own bounds (14.1). INFERRED, the skip being a branch: it takes anything with `takedamage` and measures a brush model to the nearest point of its bounds. vcod (`builtins::combat::blast_candidates`) walks `entities_in_box` in the area tree's order (14.7), keeping clients and turrets; a client is tested for `takedamage` on its turn and measured at its script `origin` field, a turret takes `G_Damage`'s entity arm (14.6). `func_static` and `trigger_damage` are walked (`cod11-combat.md` 14.6, 14.8). Doors have `takedamage` on retail and no spawn here; no stock MP map places one.
  - **The victim's box and eye are the standing ones.** VERIFIED: retail's probe points come off the entity's bounds and its `client+0xD0` eye height (14.3). vcod's host carries no stance for the builtin, so a crouched or prone player is measured standing. That moves the five probe points, not the distance (origin to origin either way). The second chance's box midpoint is taken at the origin of the client's last link, mirrored from the sim: its last cmd's truncated origin, or the unsnapped one after a `setOrigin` with no cmd since, or on a linked client (14.3).
  - **Not a divergence: a fifth argument is ignored.** VERIFIED: retail reads its four arguments by index, `Scr_GetVector(0)` and `Scr_GetFloat(1)` to `(3)` (14.2). INFERRED, from the absence of an arity check: a fifth argument is ignored, not refused, as in vcod. No 1.1 call passes one. UO's builtin takes an attacker and an inflictor after the four (`coduo-vehicles.md` 11.2).
- **Of the `SP_` layer, only what the wire can see runs.** Mostly closed. `spawn_entities_from_string` (`crates/server/src/game/spawn.rs`) reproduces `G_CallSpawn`'s third case for the five classnames whose `SP_` is an unconditional `G_FreeEntity(self)` (`SPAWN_FREES`; entity numbering, `cod11-gsc-object-model.md` section 13), the spawn-time configstring writes (`SP_worldspawn`'s `northyaw`, `SP_trigger_hurt`'s alias, the item and two aliases a mounted mg42 registers through `SP_turret`; object-model sections 17 and 19), and the trigger, script entity, item and turret `SP_` functions, plus `func_bobbing`/`func_pendulum` (`mover::spawn_sine`). Still absent: the engine setup of `func_door`, `func_door_rotating` and `func_rotating`. They are live and script-visible with every Radiant key applied, but a script that asks a `func_door` to move is the case that would show it.
