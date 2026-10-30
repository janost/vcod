# CoD 1.1 gsc script language

The grammar, operator set and engine interface of Activision's own MP
gameplay scripts (`.gsc`), as shipped in the stock paks, for the gsc VM
(`vcod-gsc`) that will run them. Evidence rules as everywhere in this
directory: VERIFIED means I measured it myself, against the shipped assets or
the binary; INFERRED means read off decompilation/disassembly without a live
test, and labelled as such. The reverse-engineered module is `game.mp.i386.so`
(the 1.1d Linux dedicated server's MP game module, full dynamic symbol table).

The census method throughout: this document was drafted while the real
parser did not exist yet, from a small comment/string/devblock-aware
classifier written for the pass, which walks each byte of every `.gsc` file
and tags it code, string, `//` comment, `/* */` comment or `/# #/` developer
block, so a construct is only counted where it appears in live code, not
inside a string or a comment. Counts below are "files that use this
construct at least once," out of 799, unless stated otherwise. The
classifier script itself was never committed.

G1 is now done: `crates/gsc/tests/corpus.rs` compiles all 799 files on every
test run (`every_stock_script_compiles`) and runs `bytecode::stack_depth` on
every resulting function, so parsing and compiling are no longer inferred
from a bespoke classifier — they are proven on every CI run with `COD_DIR`
set. This document has been re-read against the finished parser, compiler
and VM (not just the plan they were built from); every count below that
could be re-derived from the real compiler was, and is marked "re-verified"
where it replaces or confirms the original classifier figure. One classifier
figure did not survive contact with the real parser: the compiler rejected
`maps/redsquare.gsc`'s `mg42_target(nTarget, eMG42, nGunner, nTarget)`
(duplicate parameter name) as a compile error, which retail evidently
accepts since the file shipped; `declare_param` (`crates/gsc/src/compile.rs`)
now allocates a slot per parameter occurrence regardless of a repeated name,
with the last occurrence winning the name lookup, matching the positional
argument binding `Vm::make_frame` already used.

## 1. Corpus

VERIFIED by `unzip -l` and `find`: 799 `.gsc` files, 3,784,878 bytes
(3.78 MB), across three paks:

| pak | files |
|---|---|
| pak0 | 17 |
| pak4 | 162 |
| pak5 | 620 |

pak1, pak2, pak3 and pak6 carry zero `.gsc` entries (checked directly against
each archive's listing). This matches the design doc's number exactly.

VERIFIED by the compile census (`crates/gsc/tests/corpus.rs`,
`every_stock_script_compiles`): all 799 files parse and compile, to 4834
functions total; `the_corpus_builtin_surface_is_stable` counts 339 distinct
names this per-file, no-cross-file-resolution compile classifies as a
builtin call (larger than the engine's own 216 builtins, §7, because an
unresolved call to another script's function looks identical to a call to a
real builtin until the loader follows the reference). Every function that
came out of every file also passed `bytecode::stack_depth`, the
stack-discipline and jump-bounds check described in
`crates/gsc/src/bytecode.rs`; no failures came out of the first run against
the real corpus.

## 2. Grammar surface present

| construct | files (of 799) | representative evidence |
|---|---:|---|
| function definitions at file top level | 799 | every file, e.g. `main()\n{` |
| `if` | 504 | |
| `else` | 124 | |
| `while` | 82 | |
| `for(init;cond;step)` | 108 | |
| `for(;;)` | 48 | subset of the 108 |
| `switch`/`case` | 91 | |
| `default:` | 19 | subset of the 91; most switches have no default |
| `break` | 130 | |
| `continue` | 31 | |
| `return` | 126 | |
| `wait <expr>;` (no parens) | 91 | `dm.gsc:539` `wait delay;`, `:707` `wait getcvarint("scr_forcerespawn");`, `:716` `wait 0;` |
| `thread` (paren-less method form) | 119 | `dm.gsc:545` `self thread killcam(...)`, `us_intro.gsc:22` `player thread skipthebriefing();` |
| `waittill` | 100 | `dm.gsc:240` `self waittill("menuresponse", menu, response);` |
| `endon` | 111 | `dm.gsc:470` `self endon("spawned");` |
| `notify` | 80 | `dm.gsc:573` `self notify("spawned");`, `_utility.gsc:358` `tracker thread death_wait_notify(...)` |
| namespaced calls (`a\b\c::fn(...)`) | 710 | `dm.gsc` calls `maps\mp\gametypes\_callbacksetup::SetupCallbacks()` |
| function pointers as values / `[[expr]]()` deref call | 36 | `_teams.gsc:17` `game["allies_model"] = mptype\american_airborne::main;`; `_callbacksetup.gsc:15` `[[level.callbackStartGameType]]();` |
| localized string refs `&"KEY"` | 54 | `mp_pavlov.gsc` `level.obj["Field Radio"] = (&"RE_OBJ_FIELD_RADIO");` |
| animtree refs `%name` | 102 | `animscripts/init.gsc:332` `self.anim_combatrunanim = %combatrun_forward_1;` |
| `#using_animtree("name");` | 114 | `animscripts/init.gsc:26` `#using_animtree ("generic_human");` (a space before the paren is legal) |
| `/# ... #/` developer blocks | 32 | `_spawner.gsc:555` `/#[[anim.println]]("...");#/` |
| vector literals `(x, y, z)` | 86 | `animscripts/utility.gsc:234` `poseOffset = (0,0,0);` |
| vector component read `vec[0]`..`vec[2]` | 42 | `_utility.gsc:178` `vectorScale`, `_utility.gsc:250` `orientToNormal`; runs on every stock S&D plant. Reads a float. Counted as files with a constant `[0]`, `[1]` or `[2]` on an entity vector field (`.origin`, `.angles`: 24 files, 16 of them `.origin`) or on a local or parameter holding a vector (an assignment from a vector literal, a vector field or a vector-returning builtin, then read by hand), comments stripped; 27 of the 42 are under `maps/`. The out-of-range and non-integer key are a §10 entry |
| empty array init `x = [];` | 157 | `dm.gsc:104` `level.healthqueue = [];` |
| cast `(int)` | 12 | `_window.gsc:64` `xcount = (int)(yendorg[1]-windoworg[1])/spacing;` |
| cast `(float)` | 17 | `_tankdrive.gsc:236` `x = (float) height;` |
| cast `(vector)` | **0** | see note below |
| method-call syntax generally (`self`/var `verb(...)`) | 664 (`self ...(`) + 139 (var `...(` for `playsound`/`thread`/`notify`/`waittill`/`endon`) | |
| `level`/`game` as a call receiver (`level <verb>(...)`, `game <verb>(...)`) | re-verified against the real parser: 1828 sites | `_callbacksetup.gsc` calls `level notify(...)` and similar throughout; both are ordinary AST receivers (`Expr::LevelRef`/`Expr::GameRef`), accepted anywhere `self` or a variable receiver is |

**`(vector)` cast: the design's "Present" list overstates this.** The census
finds exactly one textual hit of `(vector)`, and it is not a cast:
`_tankdrive.gsc:794`, `self setTurretTargetVec(vector);` — a call passing a
variable literally named `vector`. There is no genuine `(vector)` cast
anywhere in the 799-file corpus. `(int)` and `(float)` are real and common
(29 files between them); `(vector)` is either unused-but-legal grammar or not
grammar at all, and the corpus cannot tell which. **Re-verified against the
real parser and compile census:** the grammar accepts `(vector)` and lowers
it to its own op (`Op::CastVector`), for symmetry with `(int)`/`(float)`,
and the compile census confirms zero corpus sites reach it — no fixture in
this repository exercises `Op::CastVector` end to end.

Switch/case fallthrough on empty cases is real and not rare:
`dm.gsc:249-251` stacks `case "allies": case "axis": case "autoassign":`
before a single body. Field access uses `.` with optional surrounding
whitespace (`animscripts/init.gsc:129` `self . ramboChance *= 2;`), so the
lexer needs to tolerate a space either side of the dot.

## 3. What is absent — and two corrections to the design's absence list

Zero hits, all VERIFIED by census: `#include`, `foreach`, ternary `?:`,
`do`/`while` as a *pattern the design predicted zero of* — see below,
`&=`, `^=`, `<<=`, `>>=`.

**`^`, `<<`, `>>`, `~` are absent from live code**, confirmed by classifying
every occurrence in the corpus by context rather than by a naive grep:

| token | code | string | `//` comment | `/* */` comment | `/#…#/` devblock |
|---|---:|---:|---:|---:|---:|
| `^` | 0 | 136 | 59 | 18 | 0 |
| `<<` | 0 | 0 | 10 | 0 | 0 |
| `>>` | 0 | 0 | 12 | 0 | 0 |
| `~` | 0 | 0 | 1 | 0 | 0 |

Every `^` is a colour code inside a string literal or sits in a comment;
`<<`/`>>`/`~` occur only inside `//` comments (ASCII-art dividers and the
odd arrow). None occurs in code. A naive `grep -c` over the raw files would
overcount all four, which is exactly why this needed the classifier rather
than a plain grep.

**Correction 1: `do`/`while` is not absent.** Both design docs state "no
do-while." The corpus has exactly one: `animscripts/predict.gsc:103-114`,
inside `tumbleWall()`:

```
do
{
	thread getNotetrack(notifyName);
	...
} while (bPredictMore);
```

Confirmed with `grep -rn '} while' pak0 pak4 pak5` returning this single
hit. One occurrence in 799 files is easy to miss by reading scripts, which
is presumably how the design's "no do-while" claim was reached. **Re-verified
against the real parser and compile census:** `parse_file` accepts
`do`/`while` (`StmtKind::DoWhile`) and the compile census finds exactly one
site, in exactly one file — `animscripts/predict.gsc` — agreeing with the
classifier count exactly.

**Correction 2: bitwise operators are not absent — they are load-bearing.**
Both design docs list the operator set as `+ - * / %`, assignment forms,
`++ --`, comparisons, `&& || !` and unary `-`, with "no bitwise ops" stated
explicitly. This is wrong for two operators:

- Bare `&` (bitwise AND) tests entity `spawnflags` bits, the standard
  Quake-family entity-flags idiom. VERIFIED in 11 files, e.g.
  `_utility.gsc:65` `if (self.spawnflags & 1)`, `_load.gsc` `if
  (trigger.spawnflags & 8)` (three sites in that file alone),
  `stalingrad.gsc` `if (ai[i].spawnflags & 4)`, `_scripted.gsc`
  `if (bitflags & level.teleport)`. `_load.gsc` alone needs this construct,
  and every map's `maps\mp\_load::main()` tail-call reaches it (per the
  umbrella design's own risk note about `_load.gsc`).
- `|=` (bitwise OR-assign) accumulates player-damage flags in **all five**
  stock gametypes: `dm.gsc`, `tdm.gsc`, `sd.gsc`, `re.gsc`, `bel.gsc` all
  contain the identical line `iDFlags |= level.iDFLAGS_NO_KNOCKBACK;`. The
  flag constants it combines are defined a few lines above in
  `_callbacksetup.gsc`'s `SetupCallbacks()` as the powers of two
  `level.iDFLAGS_RADIUS=1`, `iDFLAGS_NO_ARMOR=2`,
  `iDFLAGS_NO_KNOCKBACK=4`, `iDFLAGS_NO_TEAM_PROTECTION=8`,
  `iDFLAGS_NO_PROTECTION=16`, `iDFLAGS_PASSTHRU=32` — a textbook bitflag set,
  built for `|=`/`&` and nothing else.

Checked and confirmed absent: `&=`, `^=`, `<<=`, `>>=`, and any standalone
`|` outside `||`/`|=`. So the bitwise surface the corpus actually needs is
exactly two operators, not zero: binary `&` and compound `|=`. **Re-verified
against the real parser and compile census:** `parse_file` accepts both
(`BinOp::BitAnd`, and `|=` desugars to `BinOp::BitOr` — see the comment on
`BinOp::BitOr` in `crates/gsc/src/ast.rs`), and the compile census agrees with the classifier
count exactly — 11 files, 24 call sites for binary `&`; 5 files, 5 call
sites for `|=`, one per stock gametype (`dm`, `tdm`, `sd`, `re`, `bel`),
confirming "all five" from live bytecode rather than a source grep. A VM
that implements the design's stated operator set as written cannot compile
`_load.gsc`, `_utility.gsc`, `_scripted.gsc`, `stalingrad.gsc`, or any of the
five stock gametypes — i.e. it fails G1's own milestone.

## 4. The complete operator set (corrected)

`+ - * / %`, `&` (binary AND — see §3), `= += -= *= /= |=`, `++ --`,
`== != < > <= >=`, `&& || !`, unary `-`. No ternary, no `^`, no shifts, no
`~`, no `&=`/`^=`/`<<=`/`>>=`.

Binary `|` and `^` exist too, though the 1.1 corpus uses neither: UO's
`dom.gsc` (3377, 3379) writes `", "^7` inside an `iPrintLn` argument list,
and both the 1.1d and UO 1.51b compilers accept it. VERIFIED
(`probe_bitops`, both servers, identical output): the binary levels are
C's, loosest first `||`, `&&`, `|`, `^`, `&`, `== !=`, `< > <= >=`,
`+ -`, `* / %` (`1 | 2 ^ 3` is 1, `6 ^ 3 & 5` is 7, `2 ^ 3 == 1` is 2,
`1 + 2 ^ 3` is 0, `1 && 2 & 1` is 0). `^` on a string and an int is the
runtime error `pair has unmatching types 'string' and 'int'`, so dom's
line is fatal only when six or more players share a capture.

## 5. The engine/script interface: five callbacks

`maps/MP/gametypes/_callbacksetup.gsc` defines five `CodeCallback_*`
functions that the engine calls directly; each is a one-line dispatch
through a `level.callback*` function-pointer field via the `[[expr]]()`
deref-call syntax:

| engine hook | dispatches through | signature |
|---|---|---|
| `CodeCallback_StartGameType()` | `level.callbackStartGameType` | none |
| `CodeCallback_PlayerConnect()` | `level.callbackPlayerConnect` | none |
| `CodeCallback_PlayerDisconnect()` | `level.callbackPlayerDisconnect` | none |
| `CodeCallback_PlayerDamage(...)` | `level.callbackPlayerDamage` | `eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc` (9 args) |
| `CodeCallback_PlayerKilled(...)` | `level.callbackPlayerKilled` | `eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc` (7 args) |

`CodeCallback_StartGameType` guards against a double run with
`level.gametypestarted`, matching the umbrella design's read.
`_callbacksetup.gsc` also defines `SetupCallbacks()` (calls
`SetDefaultCallbacks()`, then sets the six `iDFLAGS_*` bit constants from
§3) and `SetDefaultCallbacks()` (snapshots the five pointers into
`level.default_Callback*`, so a level script can override one callback and
still reach the gametype's original).

A gametype installs the five pointers itself. `dm.gsc:68-74`, inside
`main()`:

```
level.callbackStartGameType = ::Callback_StartGameType;
level.callbackPlayerConnect = ::Callback_PlayerConnect;
level.callbackPlayerDisconnect = ::Callback_PlayerDisconnect;
level.callbackPlayerDamage = ::Callback_PlayerDamage;
level.callbackPlayerKilled = ::Callback_PlayerKilled;

maps\mp\gametypes\_callbacksetup::SetupCallbacks();
```

`::Callback_StartGameType` is the unqualified-`::` function-pointer-literal
form (current file, no namespace path), confirming that syntax works
alongside the namespaced `a\b\c::fn` form used one line later.

**Function-pointer-value sites, re-verified against the real parser:** a
function-pointer *value* (`Expr::FuncRef`, as opposed to an immediate call)
appears 354 times across 44 files — 212 with no file segment (the
`::Callback_StartGameType` form above) and 142 naming an explicit path
(`x = maps\mp\b::cb;`, the shape `load.rs`'s cross-file scan exists to
follow). Separately, a `[[expr]]()` pointer-deref call — how a stored
callback is actually invoked, e.g. `[[level.callbackStartGameType]]();` —
appears 163 times; an ordinary namespaced call (`a\b\c::fn(...)`, resolved
at compile time, not through a stored pointer) appears 8588 times. These are
compiler-derived exact counts, not the classifier's file-level estimate.

## 6. The `mr` client command and menu-response join path

VERIFIED via `nm -D`: `ClientCommand` at `0x487ec`, `Cmd_MenuResponse_f` at
`0x486d8` — both addresses match the umbrella design exactly.

**`ClientCommand` is a flat `Q_stricmp` chain.** I disassembled it
(`objdump -dr --start-address=0x487ec --stop-address=0x48f8c`, the next
symbol, `Cmd_Score_f`, bounding the function) and read off every string
address pushed immediately before each comparison call, in program order:
`0x73f69, 0x73f6d, 0x73f76, 0x73f7b, 0x73f85, 0x73f8a, 0x73f90, 0x73f93,
0x73f98, 0x73f9d, 0x73fa1, 0x73faa, 0x73fb1, 0x73fb5, 0x73fba, 0x73fc5,
0x73fd0, 0x73fd6, 0x73fdf, 0x73fe4, 0x73fe7, 0x73ff2`. Dumping `.rodata`
across that range (`objdump -s -j .rodata`) gives, in dispatch order:

```
say, say_team, vsay, vsay_team, tell, score, mr, give, take, god,
notarget, noclip, ufo, kill, follownext, followprev, where, callvote,
vote, gc, setviewpos, entitycount
```

21 commands, back to back in one null-delimited string block spanning
`0x73f69..0x73ffe` (the last string, `entitycount`, terminates exactly at
`0x73ffd`, one before the block's end — VERIFIED by reading the raw bytes).
A 22nd, trailing pushed string (`0x73f46`) is not a command name; it is the
format string `entity count = %i\n` that `entitycount`'s handler prints,
sitting in `.rodata` just before the command block. There is no `team`
command in this chain — confirming the design's claim that joining is a menu
response, not a `team` command.

`mr` dispatches straight into `Cmd_MenuResponse_f` (`call 486d8
<Cmd_MenuResponse_f@@Base>` at the `mr` comparison site). Its body, read
with `tools/re/annotate_func.py game.mp.i386.so Cmd_MenuResponse_f`:

- VERIFIED: `trap_Argc() != 4` (0x486e8) stores `""` in the menu buffer and
  the bytes `bad\0` in the response buffer (0x486ed, 0x486f4) and jumps to
  the notify at 0x487b3, so nothing else is read.
- VERIFIED: `argv[1]` goes through `strtol(.., 10)` (0x48730) and is compared
  with `trap_Cvar_VariableIntegerValue("sv_serverId")` (string 0x73f5d,
  0x4874a); a mismatch jumps to the return at 0x487de.
- VERIFIED: `argv[2]` is read into the menu buffer and `strtol`'d
  (0x48772); `cmp eax,0x1f; ja` (0x4877c) skips the
  `trap_GetConfigstring(0x49c + index)` that overwrites the buffer, so an
  index past 31 (unsigned, so a negative one too) leaves the argument's own
  text as the menu name.
- VERIFIED: `argv[3]` is the response (0x487a9). `Scr_AddString(response)`,
  `Scr_AddString(menu)`, then `Scr_Notify(ent, scr_const.menuresponse, 2)`
  (0x487b7..0x487d9). INFERRED, from the stack order: `waittill`'s first
  parameter is the menu, the second the response.

VERIFIED, `client-probes/probe_menuresponse.gsc` on retail 1.1d
(2026-10-13, one `--net-probe` client sending each line through
`--probe-say`, a menu precached in slot 0 only):

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

So a client's `noop` and `bad` both reach the script as menu `""` and
response `"bad"`. VERIFIED from `maps/mp/gametypes/*.gsc` (pak5): no stock
`menuresponse` loop compares either against `""`, so the notify changes
nothing there.

vcod: `parse_menu_response` (`crates/server/src/server.rs`) tokenizes with
quotes (`game::say::tokenize`), reads both numbers with a saturating
`strtol` and hands `ScriptRuntime::menu_response` a `MenuRef`; the probe run
against `vcod-server --gametype-script` logged the same fourteen lines.

**Cross-referenced against the script side.** `dm.gsc:142` builds the menu
name: `game["menu_team"] = "team_" + game["allies"] + game["axis"];`, which
for the stock nationality pairing is `"team_" + "american" + "german"` =
`"team_americangerman"` — the configstring slot 1180 string `configstrings.rs`
already writes. `dm.gsc:240`, inside a `for(;;)` loop, is
`self waittill("menuresponse", menu, response);`, with an early `continue`
for `response == "open"` or `"close"` and a `switch(response)` on
`case "allies": case "axis": case "autoassign":` before checking
`menu == game["menu_team"]`. So the wire join is one `mr <serverId>
<menuIndex> allies` (or `axis`/`autoassign`/`spectator`) landing as a
`notify("menuresponse", menu, response)` on the connecting player's entity,
which wakes this `waittill`.

## 7. The engine's script-function name table

Superseded by `cod11-gsc-object-model.md` section 9, which dumps every builtin
table with `tools/re/dump_builtins.py`. The summary, and what it corrects:

**Count: 216, VERIFIED.** Builtins are five tables, not one. `functions`
(0x7e508, 106 entries) holds the free functions; `Scr_GetMethod` searches
player (0x733dc, 46), scriptent (0x78d40, 12) and hudelem (0x749b4, 14)
methods in that order and falls back to the generic entity methods (0x7ea00,
38). Every walk is a linear `strcmp` over a hardcoded count.

**The record struct, VERIFIED.** `functions` records are 12 bytes
`{char *name; void (*fn)(); int developer;}`; method records are 8 bytes
`{char *name; void (*fn)();}`. `developer` is 1 on `print`, `println` and
`assert` and 0 everywhere else, corroborated by CoDExtended's
`SCRIPTFUNCTION { name, function, developer }` in `src/script.c`.

**Three earlier claims here were wrong**, all from the same cause: a pointer
stored in `.data` reads as 0 in the file, because the module is a shared
object and `.rel.data` supplies the address. Read the raw dwords and every
function pointer looks null. So:

- No record carries a null function pointer. `getent` is `Scr_GetEnt` and
  `getentarray` is `Scr_GetEntArray`.
- The old count of 144 was 106 plus 38 under a uniform 8-byte stride, which
  garbles `functions`. The stride is not uniform across the region.
- The `move*`/`rotate*` builtins are not a `.rodata` whitelist with a constant
  zero second field. They are the scriptent method table in `.data`, with real
  code pointers: `ScriptEntCmd_MoveTo`, `ScriptEntCmd_GravityMove`,
  `ScriptEntCmd_RotateVelocity` and the rest.

`Scr_GetFunction` writes the table's own spelling back through its `char **`
first argument (0x5c199), so the engine canonicalises a builtin name on
lookup, which is consistent with the case folding in section 8.

## 8. Atom identity and case folding

The engine interns every script string (`GScr_AllocString`), and matches
identifiers, field names, file paths and event names (`notify`/`waittill`/
`endon`) case-insensitively, but compares string *values* and array keys
case-sensitively (measured; see below). `vcod-gsc`'s interner
(`crates/gsc/src/atom.rs`) therefore has two entry points over one storage
vector: `intern_folded` dedups on the lowercased text, so two spellings of
an identifier-role key share one `Atom`, and `intern_exact` dedups on the exact
text, so `"ABC"` and `"abc"` are two atoms. Both store the spelling they
were given verbatim, and `Interner::resolve` returns it, so a script-built
display string never gets silently lowercased. Whichever entry point sees a
folded key first owns it, so the two can never hand out different atoms for
one identifier.

**Re-verified against the real parser and compile census:** walking every
string literal in the corpus and grouping by its case-folded form finds 86
distinct keys with more than one spelling in the shipped scripts (an exact
match for the figure `atom.rs`'s own doc comment already cited). All 86 are
weapon, tag, animation-tree-model or asset-path names (`Panzerfaust`/
`panzerfaust`, `TAG_groupA`/`tag_groupA`, `xmodel/head_Elder`/
`xmodel/head_elder`, and so on) — none is user-facing display text. Of the
86, 7 are used at least once as the event-name argument to `notify`/
`waittill`/`endon` (e.g. `"SPAWNED"`/`"spawned"`, `"DIED"`/`"died"`), and 17
are used at least once as a literal `array["key"]` index.

**Settled against retail** (`crates/gsc/tests/fixtures/semantics/retail-captures.txt`,
`# probe_field_case`, `# probe_arraykey_case`, `# probe_cmp`): retail folds
identifiers and field names but treats string values and array keys as
case-sensitive — `level.myField` reads back through `level.myfield`, while
`a["medFire"]` and `a["medfire"]` are two entries with a `.size` of 2, and
`"ABC" == "abc"` is false. vcod originally folded all four through one
identity table; it now routes each call site to the matching entry point,
and the count above splits with it: of the 86 multi-spelling keys, those
used as identifiers, field names, paths or event names still collapse onto
one atom, and those used as array keys or string literals no longer do.

Folding is genuinely required for the event-name role, and that is the one
place the split is dangerous: a `waittill` on one spelling has to see a
`notify` fired with another, and if it stops the thread hangs rather than
erroring. An event name is written as a string literal, so it reaches the VM
case-preserved; `Op::WaitTill`, `Op::Notify`, `Op::EndOn` and the host-facing
`Vm::notify` each map it through `Interner::fold_atom` before matching. That
covers a dynamically built name (`self notify(level.eventName)`) too, which
compile-time routing could not.

**A second, smaller divergence from the same design:** when two spellings of
one identifier-role key collapse onto one atom, `resolve` always returns
whichever spelling was interned first, so which of the two a script observes
(via `print`, string concatenation, etc.) depends on load order, not on which
spelling that particular call site used. Storing every spelling and comparing
case-insensitively at read time would get this exactly right, at the cost of
giving `Value` a custom `PartialEq` that any future `==` on it would need to
know about — judged not worth it for a collision now bounded to the
identifier roles alone.

## 9. Semantics measured against retail

Everything below was measured on the retail 1.1d Linux dedicated server, not
inferred. The probes are `crates/gsc/tests/fixtures/semantics/probe_*.gsc`,
run as gametype scripts by `tools/run_probe.sh`; retail's answers are
committed beside them in `retail-captures.txt`, and
`crates/gsc/tests/semantics_ab.rs` diffs vcod against them on every test run.
It is green on the 25 probes it runs. Five more (`probe_bootstrap`,
`probe_cvar`, `probe_delete`, `probe_ents`, `probe_not_string`) need the
object model, the cvar table or a real map, all of which live in
`crates/server`, so they are measured there by
`crates/server/tests/semantics_ents.rs` against the same capture file, and
so are the seven `probe_stale_*`, which free an object and read its handle
back. A probe retail refuses to compile has a section holding only
`PROBE_ERROR script compile error`, and `semantics_ab.rs` requires vcod's
compiler to refuse it too (`probe_game_dotwrite`, `probe_level_bracket`,
`probe_self_bracket`).

Four facts about the retail side shaped how the measurement had to be taken,
and each is worth knowing before writing another probe:

- **`logPrint` is the only output channel** a dedicated server with no
  clients shows. `print` and `println` produce nothing on the console even
  with `developer 1`; `iPrintLn` needs a client. `logPrint` writes to
  `games_mp.log` with a leading `m:ss ` stamp.
- **A script runtime error terminates the server** at `developer 1`,
  the probes' (below it the thread runs on: "Runtime errors below
  developer 1"), not just the thread:
  the console prints a `******* script runtime error *******` block with the
  file, line and a caret under the offending token, then `ERROR: script
  runtime error` and `----- Server Shutdown -----`. This is why each probe
  group is a separate file.
- **A gametype needs a one-line `.txt` description file** beside its `.gsc`,
  or the engine warns and refuses to load the map.
- **The engine loads only the first 31 loose gametype scripts** it finds and
  runs `dm` instead of any gametype past that (`Too many game type scripts
  found! Only loading the first 31`, then `g_gametype is not a valid
  gametype, defaulting to dm`). Neither line reaches the probe's own
  channel, so the section comes back empty with no `PROBE_FATAL`, which is
  also what a compile error looks like. The probe corpus is at 31 files and
  `tools/run_probe.sh` installs each one into the server's homepath, so it
  now deletes the loose `probe_*` files there before installing its own.
  Two full regenerations were lost to this before the cause was found.

**Boolean reading: numbers and numeric strings, VERIFIED.** `if (x)`
accepts `Int` and `Float`: `0` and `0.0` are false; `1` and `0.5` are
true. A string reads as the number it spells: `if ("1")` takes the
branch and `if ("0")` does not, on 1.1d and UO 1.51b alike
(`probe_truthy_numstr`, 2026-10-14). `""`, `"a"`, a vector and an unset
field each raise `cannot cast X to bool` and kill the server. That is why
the corpus spells most such tests as `isDefined(x)` or `x != ""`. UO's
`tdm.gsc` tests `if (getCvar("scr_teamscorepenalty"))`, which its own
setup leaves at `"1"`, on every suicide and team kill; vcod refused every
string until 2026-10-14, which aborted that callback before its `K` line.

**Unary `!` is the same reading, VERIFIED (`probe_not_string`,
`probe_not_empty_string`).** `!"1"` is `0` and `!"0"` is `1`. The empty
string is fatal under `!` as under `if`, with the same message: `!""` dies
on `cannot cast "" to bool`. `!"a"` was not measured; `if ("a")` is fatal
(`probe_truthy`), and vcod treats both alike. This is not a curiosity.
`_teams::restrictPlacedWeapons` guards on `!getCvar("scr_allow_fg42")`,
stock `scr_allow_fg42` is `"0"`, so the guard is true on a stock server and
the map's placed fg42 weapons are deleted. `not_of`
(`crates/gsc/src/vm/interp.rs`) implements the reading for `Op::Not` and
the conditional jumps, for `Value::String` and `Value::Localized` alike.

**`true` and `false` are ints, and case-sensitive, VERIFIED.** `true` reads
back as `1` and `false` as `0` (`probe_bool`, concatenated onto a string, so
the rendering is the int one). `true == 1` and `false == 0` both hold, and
`if (true)`/`if (false)` take the branches those values imply. `TRUE` is
**not** the literal: reading it back gives `undefined`, which is what an
unassigned local reads as, and concatenating it is the usual fatal `pair has
unmatching types 'string' and 'undefined'`. So these two are the one place
gsc's otherwise case-insensitive identifier and keyword matching does not
apply. The stock corpus depends on this everywhere — `_gameobjects::main`
sets `dodelete = true` and later branches on `if(dodelete)` — so a lexer
that treats `true` as a bare identifier voids those scripts at their first
`if`.

**Arithmetic: VERIFIED, and every earlier inference was right.** `1 / 2` is
`0` (integer division truncates), `4 / 2` is `2`, `3 / 2.0` is `1.5`,
`3 * 3` is `9`, `3 * 1.5` is `4.5`, `1 + 2` is `3`, `1 + 0.5` is `1.5`,
`7 % 3` is `1`, `(0 - 7) % 3` is `-1` (truncating toward zero). `1 / 0` is a
fatal `divide by 0`.

**Number-to-string rendering: C's `%g`, VERIFIED.** Concatenating a number
onto a string gives `5` -> `5`, `-5` -> `-5`, `0.5` -> `0.5`, `2.0` -> `2`,
`0.8` -> `0.8`, `1.0 / 3` -> `0.333333`. Six significant digits, trailing
zeros dropped. The probe's `1000000` case (`probe_concat.gsc:22`) is *not*
evidence for `%g`: it concatenates an int, which never reaches the float
formatter. The exponent form is C's too, VERIFIED (`probe_concat_exp`):
`0.00001` -> `1e-05`, `-0.00002` -> `-2e-05`, `0.000012345678` ->
`1.23457e-05`, `0.0001` -> `0.0001`, `999999.0` -> `999999`, `1000000.0` ->
`1e+06`, `1234567.0` -> `1.23457e+06`, `1e28` -> `1e+28`. INFERRED from those
eight values, which match C's `%g` rule: the switch is
at a decimal exponent below -4 or at 6 and above, the exponent carries a sign
and at least two digits, and the mantissa drops trailing zeros. `setExpFog`
goes through the same formatter: mp_chateau's `setExpFog(0.00001, ...)`
reaches configstring 12 as `0 1 1e-05 0 0 0 0`
(`docs/research/cod11-gametypes-re-bel.md`). A vector
renders by a different
rule: `(1, 2, 3)` -> `(1.00, 2.00, 3.00)`, two decimals per component.
`"str" + undefined` is a fatal `pair has unmatching types 'string' and
'undefined'`.

**Equality: VERIFIED, and it is neither structural nor numeric.**
`1 == 1.0` is true. `"abc" == "abc"` is true. `"ABC" == "abc"` is **false**.
`"5" == 5` is **true**, but `"5.0" == 5`, `"05" == 5` and `"abc" == 0` are
all false — so a number compared against a string is rendered to its `%g`
text and compared textually, not parsed as a number. `undefined == 0` is a
fatal `pair has unmatching types 'undefined' and 'int'`.

**Ordering: numbers only, VERIFIED.** `"a" < "b"` is a fatal `pair has
unmatching types 'string' and 'string'`. There is no string collation.

**Case folding is per role, VERIFIED, and this corrects §8.** Field names
fold: `level.myField` reads back through `level.myfield`. Array keys do
**not**: `a["medFire"]` and `a["medfire"]` are two entries, and the array's
`.size` is 2. Together with `"ABC" == "abc"` being false, this means retail
folds identifiers and field names but treats string *values* and array keys
as case-sensitive. One interner folding everything cannot express that.

**`.size`, VERIFIED.** On an array it counts every key regardless of type: an
array with keys `0`, `1` and `"k"` reports 3, an empty one 0. Strings have it
too: `"abcd".size` is 4.

**A string indexes by character, VERIFIED (`probe_string_index`, 1.1d and
UO 1.51b, 2026-10-14).** `s = "ab c"; s[1]` is `"b"` and `s[2] == " "` is 1
on both servers; `s[9]` is fatal with `string index 9 out of range`. UO's
`_gameobjects::main` splits `script_gameobjectname` this way.

**Assigning to an index of `undefined` auto-vivifies an array, VERIFIED
(`probe_autoviv`).** `a[0] = "x"` on a local that was never set turns it
into a one-element array; a non-zero first index (`b[3] = "x"`) and a string
key (`c["k"] = "v"`) behave the same way, each yielding a `.size` of 1 —
retail's arrays are sparse, so a non-zero first write does not backfill the
lower indices. The same holds through a struct field
(`level.myArray[0] = "y"`), which is exactly what `_load.gsc`'s
`add_to_array(level._script_expoders, ...)` relies on for a field that is
never otherwise initialised. Indexing a value that is defined but *not* an
array is still fatal: `e = 5; e[0] = "x";` dies with `int is not an array`.
vcod raises `BadType("indexing needs an array")` in that case instead,
same reachable outcome (both sides stop, per `semantics_ab.rs`'s error
check) with a different message text, which the harness does not compare.

**Writing `undefined` into an array removes the key, VERIFIED
(`probe_array_undef`, 1.1d 2026-10-22; the same lines from CoD2 1.3's
`cod2_lnxded`).** `a[2] = undefined` on a three-element array leaves
`.size` 2, a hole at `a[0]` leaves 1 with `isdefined(a[0])` 0, and the next
`a[a.size] = "w"` overwrites `a[1]`: `.size` counts keys. Through a field
and under a string key alike. CoD2's `_deathicons::removeDeathIcon` shrinks
its arrays this way and walks them by `.size`.

**Only the literal `undefined` removes a key, and it never vivifies a
local, VERIFIED (`probe_undef_into_unset`, `probe_undef_call_into_unset`,
1.1d and CoD2 1.3's `cod2_lnxded`, 2026-10-22, the same lines from
both).** `c[5] = undefined` on a never-set local is fatal (`undefined is
not an array`) where `b[5] = 1` there reads `.size` 1. `c[5] = nothing()`,
a call that returns undefined, vivifies `c` and reads `.size` 1:
the key stays, holding undefined. INFERRED: the compiler turns an
assignment of the literal into a clear of the element on the array as it
stands, and any other right side into a store. vcod compiles the literal
to `Op::ClearIndex` (an unset local under it is read, not vivified) and
`StoreIndex` keeps an undefined value as a key (`Heap::set_index`,
`Heap::clear_index`). A field or element target under the literal
(`level.unset[5] = undefined`) is unmeasured; vcod still vivifies it.

**Notify wake order: start order, VERIFIED.** Two threads waiting on one
event on `level` wake in the order they were started.

**The thread pick: due time first, then last queued first, VERIFIED
(`probe_notify_frame`, `probe_wait0_yield`, `probe_wait0_loop`, retail
2026-09-27).** Retail resumes the threads of a frame one at a time, the one
due earliest first, and among threads due at the same time the one that was
queued last. Four consequences, each printed by retail:

- Two threads whose `wait 0.1` falls due on the same frame resume newest
  first: `probe_wait0_yield`'s `d` before `c`, both started in that order
  from `main`. The same probe's `a` and `b`, which each `wait 0` from `main`
  at load, resume `b` then `a`. `probe_notify_frame`'s frame counter, bumped
  by a ticker started first, still reads the new frame at the notifier and
  at `main`'s own `wait 1`: the ticker queued its wait later, so it goes
  first.
- A `wait 0` reached inside a frame is due now and queued last, so the thread
  resumes at once, before any other thread due that frame:
  `probe_wait0_yield`'s `d` logs `before` and `after` with nothing between,
  and `probe_wait0_loop`'s thousand turns of `i++; wait 0` all land in one
  frame, ahead of a logger due on the same frame but queued earlier. With no
  bound the loop never lets the frame end: retail printed `WARNING:
  potential infinite loop in script.` 16 times in a 20 s run, at `developer`
  0 and 1 alike, and logged nothing else after the loop started. VERIFIED:
  `cod_lnxded` 0x80a5b8c pushes that string (0x80d7d60) after an `rdtsc` at
  0x80a5b63; INFERRED from the two: the warning is a time budget, not a
  count. The
  `ERROR: ... killing thread` arm next to it was never reached.
- A notify does not run its waiters inside the notifying statement:
  `probe_notify_frame`'s notifier logs past its `notify` first. The waiters
  resume in the same frame, after the notifier's step, in start order
  whether they were started before or after the notifier (`probe_notify` is
  the same order). A waiter that then does `wait 0` resumes at once, before
  the next waiter.
- INFERRED: a killcam therefore lasts exactly its age and its skip respawns with no
  dead frame between (`cod11-spectator-follow.md` 12.7): `waitKillcamTime`'s
  notify resumes `killcam`, which started it, on the same frame, and
  `waitRespawnButton`'s opening `wait 0` resumes on the frame it was reached.

vcod's `Vm::step_runnable` is that pick: a `wait` stamps the thread with a
queue number, a notify stamps its waiters newest first, and the frame's pass
takes the earliest due, highest stamp until nothing is due. Where it is not
retail's: the pass stops after `MAX_THREADS_PER_FRAME` (10,000) steps and
leaves the rest for the next frame, where retail stalls for good; not
measured.

**The packet pass: nothing runs between frames but the callbacks, VERIFIED
(`client-probes/probe_threadorder`, retail 1.1d on mp_pavlov,
2026-10-20).** One client sent its client commands in pairs, both of a
pair in one packet: `mr` alone, `mr` then `kill`, `kill` then `mr`, two
`mr`s. Three threads wait on the player's `menuresponse` (started w1, w2,
w4), one on a level `"killed"` that `Callback_PlayerKilled` notifies (w3,
started between w2 and w4); the callback starts a thread (`kt`) and each
of them logs, takes a `wait 0` and logs again. A ticker queued on each
frame's `wait 0.05` records its last `getTime()`. Retail printed, with T
the frame the packet landed after:

- The callback runs inside the command, on T's `getTime()`, to its first
  suspend, and a thread it starts runs inside it (`killed`, `kt`,
  `killed_after` at T).
- Everything else runs on the next frame, T+50, ahead of the ticker (each
  line reads `tick` T): the waiters the commands woke and the callback's
  `wait 0`s, in one pick, last queued first. `mr` then `kill`: `killedz`,
  `ktz`, `w3`, then w1, w2, w4. `kill` then `mr`: w1, w2, w4, then
  `killedz`, `ktz`, `w3`. One notify's waiters go in start order, and a
  woken thread's own `wait 0` resumes at once, before the next.
- The second of two `mr`s in one packet reaches nobody: no waiter has run
  by then, as with two notifies in one script step (`probe_notify_twice`).
- Two clients' packets in one frame go the same way (a second run, two
  probes each sending `kill` every 3 s, the second one 17 ms later each
  round, so their packets crossed a frame boundary twice): client 1's
  callback ran first at T, then client 0's, and at T+50 client 0's
  `killedz` and `ktz` ran before client 1's, then the two level waiters
  the first kill woke, in start order. The second kill's notify found
  them already woken.

VERIFIED, the same two runs on retail 1.5b and UO 1.51b (mp_pavlov,
2026-10-20, `crates/server/tests/fixtures/threadorder/*-1.5.txt`,
`*-uo.txt`): every line in 1.1d's order, on the same frames past the packet's. On UO's
two-client run the second probe to connect began first, so its level
waiter ran first; the order follows who began, not the slot.

INFERRED, from those lines and the `G_RunFrame` order in "The script
clock": a notify from a command or a callback only wakes its waiters, a
`wait 0` taken there is due on the clock, and both wait for `G_RunFrame`'s
first `Scr_RunCurrentThreads` (0x50653), where the thread pick above takes
them; no `Scr_RunCurrentThreads` runs between a packet's commands.
`SV_ExecuteClientMessage` runs a packet's client commands in order
(`cod11-combat.md` 9.2).

vcod, before 2026-10-20: the menu answers were notified after the move
pass, behind every `kill`, and a packet pass (`Vm::run_runnable`) ran the
woken threads before `level.time` advanced, in start order, so they read
T and ran ahead of the callback's `wait 0`s. Now `mr` is a
`PacketCommand` run in packet order with `kill`, nothing steps between
frames, and the frame's first pass takes the woken threads with the rest.
Gate: `crates/server/tests/threadorder_ab.rs`, both runs at each
protocol. A test harness
that joins two clients in one frame therefore spawns the later packet's
first: `common::join_pair_stamped` hands the second client's packets to the
server first once both are in, so the first spawns first, as a retail
client that joined earlier does. Every gate that joins two or three
clients in one frame goes through that rule (`common::step_pair_joining`,
`join_trio`, `step_trio_ordered`), or joins its second client as late as
the retail recipe did (`vehicle_runover_ab`, `vehicle_armor_ab`); a
`gametypes_ab` or `votes_ab` restart that sends both clients back through
the menus in one frame reads the same in either packet order.

**The script clock.** The VM keeps its own clock, separate from
`level.time`, and a frame's threads run on it a frame behind.

- VERIFIED: `getTime` (`game.mp.i386.so` 0x5d088, entry 17 of the
  `functions` table) pushes `level.time` (`level+0x1e8`) through
  `Scr_AddInt` and nothing else, so it reads milliseconds of `level.time`.
- VERIFIED: `G_InitGame` writes its first argument to `level.time` and
  `level.startTime` (`level+0x1e8`, `+0x1f4`, at 0x4fc6d and 0x4fc72) and
  later passes `level.time` to `Scr_InitSystem` (0x4fe4a..0x4fe58), ahead of
  `Scr_LoadGameType`, `Scr_LoadLevel` and `Scr_StartupGameType`.
- VERIFIED: `cod_lnxded` 0x80a97ec stores its second argument, masked to 24
  bits, into 0x82f594c, the word the `wait` opcode below adds to. INFERRED
  (the game module reaches the engine through a pointer table, and this is
  the only two-argument writer of that word): 0x80a97ec is `Scr_InitSystem`
  and 0x82f594c is the VM clock.
- VERIFIED: `G_RunFrame` (0x50478) writes `level.time` first, calls
  `Scr_RunCurrentThreads` at 0x50653 (reached whether or not its touch
  notify list at `level+0x29ec` is empty), again at 0x506d0 inside the DObj
  loop, then `Scr_SetTime(level.time)` at 0x5070c, ahead of the entity loop.
- VERIFIED: in `cod_lnxded`, 0x80aa3d0 walks the VM clock up to its
  argument one millisecond at a time, running the threads queued on each
  millisecond before stepping past it, and stops on the argument without
  running its queue; 0x80aa470 runs the queue of the current millisecond
  only. INFERRED (call order against `G_RunFrame`'s): those are
  `Scr_SetTime` and `Scr_RunCurrentThreads`.
- VERIFIED: the `wait` opcode (`cod_lnxded` 0x80a4dbb..0x80a4e3e) queues the
  thread on `(int)(seconds * 1000 + 0.5) + clock`, masked to 24 bits, and
  errors on a negative wait and on one of 16777 s or more (`0x80d7fe4`).

INFERRED from those: a level's script clock starts on its own `level.time`;
in `G_RunFrame(L)` the threads due on the clock the last frame left run
first, then every thread due before `L` runs on its own due time, and one
due at `L` itself waits for the next frame. A `wait` taken by a thread the
frame resumed counts from that thread's due time. Between frames the clock
reads the last frame's `level.time`, so a client callback's `wait` counts
from there.

VERIFIED (`client-probes/probe_startclock`, retail on `mp_carentan`,
2026-10-09): `main` and `Callback_StartGameType` both read `getTime` 0, a
`wait 1` taken in the callback wakes at 1050 and the next `wait 1` at 2050,
a `wait 0` at 100, and a `wait 0.05` loop logs 100, 200, 200, 300, 300,
350, 400 and every 50 ms after: the three 100 ms settle frames
(`cod11-map-cycle.md` 3, step 20) each run two 50 ms steps of the loop. A
`wait 0.1` then `wait 0.2` logs 200 and 350.

vcod's `Vm::run_frame` walks the clock the same way and `Vm::set_time` is
`Scr_InitSystem`; `ScriptRuntime::load_from` sets it to the load's
`level.time` before the gametype's `main`.

**The clock is 24-bit.** Every store to it is masked:

- VERIFIED: `Scr_InitSystem` (`cod_lnxded` 0x80a97ec) stores
  `arg & 0xffffff` (`and` at 0x80a9822). `Scr_SetTime` (0x80aa3d0) masks
  its argument first (0x80aa3da), and its walk masks each step
  (`inc`/`and $0xffffff` at 0x80aa451..0x80aa457).
- VERIFIED: the `wait` opcode adds the clock to the rounded milliseconds at
  0x80a4e15..0x80a4e1b and clears the sum's high byte (`movb $0x0,
  -0x39(%ebp)` at 0x80a4e1e, the top byte of the int at `-0x3c`), so a
  deadline is 24-bit too. The rounding is `(seconds * 1000.0f + 0.5f)`
  (0x80a4ded, 0x80a4df3; floats at 0x80d7fe8 and 0x80d7fec) stored by
  `fistpl` under a truncating control word (`or $0xc00` at 0x80a4e03).
- VERIFIED: `Scr_SetTime` subtracts the clock from the masked target and
  takes the `jle` at 0x80aa3ea when the difference is not positive: it
  then stores the target as the clock (0x80aa462) and runs nothing.
- VERIFIED: the two refusals come before the rounding. A wait that is
  negative or unordered against 0 (`fcom`, `and $0x5,%ah`, `jne` at
  0x80a4dc6..0x80a4dd3) errors with `negative wait of %g is not allowed`
  (0x80d7c80); otherwise one not below 16777.0 (`fcoms 0x80d7fe4`,
  `and $0x45,%ah; cmp $0x1,%ah` at 0x80a4dd9..0x80a4de7) errors with `wait
  of %.0f seconds is too long` (0x80d7c40). Both go through the
  runtime-error path at 0x80aa158, which on retail ends the server.
- VERIFIED: `svs.time` (0x83b67a4) has no store in `cod_lnxded` other than
  the `addl $0x64` settle steps (0x8083f40, 0x808a650) and the frame loop's
  `add` (0x808d1ab), and a load hands it to `G_InitGame` as `level.time`
  (`cod11-map-cycle.md` 3, step 19). INFERRED from those: `level.time`
  carries across map changes for the life of the server process, so the
  wrap comes 16,777,216 ms (about 4.7 hours) into a server's uptime, not
  into one level. The engine's own time-wrap restart
  (`EXE_SERVERRESTARTTIMEWRAP`, 0x80d5c8d) tests `svs.time > 0x70000000`
  (0x808ced4), far later.

INFERRED from those: on the frame whose `level.time` crosses a multiple of
2^24, the masked target is behind the clock, so `Scr_SetTime` jumps the
clock there and runs only what `Scr_RunCurrentThreads` ran, the queue of
the clock's own millisecond. A thread due between the old clock and 2^24,
or below the new masked target, is not run until a later walk crosses its
due time again, about 4.7 hours on. A thread due on the target itself runs
the next frame as usual, and every later deadline counts on the wrapped
clock. `getTime` is unaffected: it reads `level.time`, not the clock.

vcod's `Vm` keeps the clock and every deadline under `CLOCK_MASK`, and
`run_frame` picks a waiting thread only when it is due on the clock or
strictly between the clock and the masked target, which is the jump above.
The `Wait` opcode refuses a negative (or NaN) wait and one of 16777 s or
more with retail's two messages; as with every runtime error (section 10),
ours aborts the thread where retail ends the server.

**`getTime` in a callback a cmd raises reads the last frame's
`level.time`.** VERIFIED (`client-probes/probe_fall`, retail on
`mp_carentan`, 2026-10-06, `crates/server/tests/fixtures/playerstate/
mp_carentan-dm-fall-damage.txt`): every `PROBE damage <t>` line the wrapped
damage callback logged names a `t` whose snapshot still carries the health
the callback found (`t=19650 ... health=100` against `PROBE damage 19650
... health 100`), and the snapshot 50 ms later is the first with the health
it left (`t=19700 ... health=77`, `ct=19683`). The same holds for all five
landings in that capture and in the `-cvars`, `-corpse` and
`-walk-corpse` captures. INFERRED, off that and the packet order in
`cod11-combat.md` 16.1: `ClientThink_real` runs the landing cmd between
`G_RunFrame`s, and `getTime` (`level.time`) still reads the frame before.
A shot's, a `trigger_hurt`'s and a `kill` command's callbacks run from the
same place, so they read it too; a blast's (entity pass) and a turret
round's (`ClientEndFrame`) run inside the frame and read that frame's.

vcod runs a cmd's callbacks inside `Server::replay_moves`, after the tick
has advanced its own clock, so `ScriptRuntime` leaves `host.level_time_ms`
on the last frame's value until `run_threads` writes the new one; nothing
on the callback path sets it. `fall_ab`'s `check_damage_clock` holds both
sides to the snapshot rule above.

**A receiver-less call keeps the caller's `self`, VERIFIED (`probe_self`).**
A plain `f()`, a `[[ptr]]()` and a `thread f()` all inherit the calling
frame's `self`, and the callee reads the caller's fields off it. This is what
makes §5's five callbacks work at all — `_callbacksetup.gsc` reaches each one
as `[[level.callbackPlayerConnect]]()` with no receiver, and `dm.gsc:185`
opens on `self.statusicon`. The probe never calls *with* a receiver, so what
an explicit one does to the frames around it is not measured: vcod binds it to
the callee's frame alone, so `a f()` rebinds for `f` only and the caller's
`self` is back on return. Whether a *builtin* called without a receiver
inherits it too is not measured either; vcod inherits for script calls only.

**A thread survives notifying an event it has itself endon'd, VERIFIED
(`probe_endon_self`).** A thread that registers `level endon("e")`, then
executes `level notify("e")` itself, keeps running past the notify *and*
resumes past its next `wait`: retail printed all four of the probe's lines,
the last of them after a one-second wait. Stock `dm.gsc` depends on it. Its
`endMap` runs inside a `Callback_PlayerKilled` thread whose first statement
is `self endon("spawned")`, calls `spawnIntermission` (which notifies
`"spawned"`) on every player including that one, and only then waits ten
seconds and calls `exitLevel` — so a VM that let the notify kill the
notifier would leave a score-limit map stuck at the intermission camera
forever. `Vm::notify_from` (`crates/gsc/src/vm/sched.rs`) is the half that
skips the issuing thread; an endon fired by any *other* thread still kills,
which §10 covers.

**A notify acts where it executes, VERIFIED (`probe_endon_late`, retail
1.1d, 2026-10-14).** A thread that threads `early()` (which registers
`level endon("probe_kill")` and waits), then executes `level notify("probe_kill")`,
then threads `late()` (the same endon) and `late_wait()` (a `level
waittill("probe_kill")`), all in one step: retail logged `late_armed`,
`notified`, `late_survived` and `done`, and neither `early_survived` nor
`late_wait_woken`. So the notify killed the endon registered before it and
spared the one registered after it in the same step, and a `waittill`
registered after it did not wake to it. vcod resolves `Op::Notify` on the
spot (`vm/interp.rs`, `Vm::notify_from`); it used to queue it to the end of
the step, which killed `late`. United Offensive's stock `sd`, `re`, `bas`,
`ctf` and `dom` need it: `checkMatchStart` does `level
notify("kill_endround")` and then `level thread endRound(...)`, whose first
line is `level endon("kill_endround")`, and the queued notify killed the
match-start round reset, so no round past the first ever started and the
bombzones' threads stayed dead.

UO 1.51b agrees, and so does a `waittill` (VERIFIED,
`probe_endon_after_notify`, 1.1d and UO, identical): a thread started right
after a `level notify("x")` with `level endon("x")` first runs on (14 ticks
of `wait 0.05` against 4 had it died), and one that opens with `level
waittill("x")` is not woken (0). UO's `hq.gsc` restarts its wave timer this
way (`level notify("Timer Changed"); level thread hq_reinforcement_timer()`),
and the queued notify had frozen `level.counter`.

**Two notifies of one event in one step: the second is lost, VERIFIED
(`probe_notify_twice`, retail 1.1d and UO 1.51b, 2026-10-17, identical).**
A pump that runs `level notify("tick", 1); level notify("tick", 2); level
notify("first"); level notify("second")` in one step, against a thread that
waits on `"tick"` twice, one that waits on it once and one that waits on
`"first"` and then `"second"`: retail logged `pumped`, `chain_first`,
`twice_1 1`, `once 1`, `done`, and neither `twice_2` nor `chain_second`.
INFERRED: the first notify wakes a waiter and takes it off the event, the waiter does
not run before the notifier's step ends, and the second notify finds nobody;
a woken waiter reads the first notify's arguments. The threads run newest
wake first (`chain_first` ahead of the `"tick"` waiters), and one notify's
waiters in start order, as the thread pick paragraph has it. vcod's
`Vm::notify` does the same: it only flips the waiter to runnable
(`sched::tests::two_notifies_of_the_same_event_in_one_step_lose_the_second`).

**Engine notifies and the trigger queue (`game.mp.i386.so` 1.1d, 1.5b, UO).**
INFERRED, from the bullets below: what the game module notifies outside a
script step only wakes waiters too; they run at the next `Scr_RunCurrentThreads`, so a notify
raised in every cmd reaches a waiter once a frame.

- VERIFIED: `G_TouchTriggers` notifies `"touch"` on the spot (0x3fa4d,
  0x3fa6f; `cod11-combat.md` 14.7), and so does `Cmd_Activate_f` its
  `trigger_use`'s `"trigger"` (0x48547; `cod11-gametypes-re-bel.md` 4).
- VERIFIED, `client-probes/probe_useby` on 1.1d, 1.5b and UO 1.51b
  (2026-10-21, `crates/server/tests/fixtures/triggers/mp_carentan-dm-useby*.txt`,
  `cod11-combat.md` 5.9): `useBy` notifies `"trigger"` with its argument
  on the spot, its waiter logging in the frame of the call, on a
  `trigger_once`, a `trigger_multiple`, a `trigger_damage` and a
  `func_static` alike; the use function it then calls queues nothing
  (`Use_Multi` has no notify, `Use_trigger_damage` passes mod -1).
- VERIFIED, `client-probes/probe_bel` on retail 1.1d (2026-10-20,
  `cod11-gametypes-re-bel.md` 8.7): the menu thread `Cmd_MenuResponse_f`'s
  `menuresponse` wakes reads the new frame's `getTime()` and runs ahead
  of a `wait 0.05` due on that frame, so it too waits for the frame's
  first round. vcod does the same since the packet pass change ("The
  packet pass"); `gametypes_ab`'s `spawn clock` rows gate it.
- VERIFIED: `Touch_Multi` (0x65a18), `hurt_touch` (0x64dc4),
  `Activate_trigger_damage` (0x650c0) and `G_Trigger` (0x656f0, the
  `trigger_lookat` path) do not notify `"trigger"`. Each appends (entity
  number, activator number, both entities' `+0x300` use counts) to a
  12-byte queue at `level+0x1dec`, its count at `level+0x29ec`, and only a
  full queue (0x100 entries) notifies on the spot.
- VERIFIED, `G_RunFrame` 0x50529..0x50665, its first work after the cvar
  update: a 0x400-byte seen table is zeroed once, then each round bumps a
  byte marker and walks the queue in order. An entry whose two use counts
  no longer match is dropped; one whose entity was already sent this round
  stays and marks the round for another; the rest get the activator pushed
  and `Scr_Notify(ent, "trigger", 1)` (0x505f6). A sent or dropped entry is
  replaced by the last one, which is examined next (0x505fe..0x50635). Each
  round ends in `Scr_RunCurrentThreads` (0x50653); the rounds repeat while a
  round kept an entry, and the count is zeroed after the last (0x50665), so
  what a round's threads queued is dropped unless a later round runs.
- VERIFIED, `client-probes/probe_touchorder` (`cod11-combat.md` 14.7):
  every frame a player stands in two minefields logs one `"trigger"` pair,
  then the `"touch"` lines, then one more `"trigger"` pair per later cmd of
  the frame. INFERRED, from that log and the code above: the first round
  runs the waiters its triggers woke ahead of those the cmds' touches woke,
  last woken first, and each later round runs the next cmd's triggers.
- VERIFIED: 1.5b's `G_RunFrame` (0x506c9..0x5082a, count at
  `level+0x2e10`) and UO 1.51b's (0x59cf5..0x59e56, `level+0x2e60`) run the
  same rounds. 1.5b's `G_Trigger` (0x69f68, queue at `level+0x2210`) and
  UO's (0x76218, `level+0x2260`) are the queue append, with the same
  0x100 check; their `Touch_Multi` calls it.
- VERIFIED, `client-probes/probe_touchorder` on retail 1.5b and UO 1.51b,
  2026-10-20 (`tests/fixtures/touchorder/mp_pavlov-1.5.txt`, `-uo.txt`):
  every phase's first frame logs the same lines in the same order as
  1.1d's, and each later cmd of a frame one more `f` pair.
- VERIFIED, `client-probes/probe_trigqueue` on retail 1.1d, 1.5b and UO
  1.51b, 2026-10-20, three byte-identical logs
  (`tests/fixtures/touchorder/trigqueue.txt`). A script-spawned
  `trigger_damage` with `enableLinkTo` (no wait armed) takes N
  `radiusDamage`s in one step from a thread on the frame grid, which runs
  inside the first round. N = 1 or 256 log no `"trigger"`: every entry is
  dropped with the round's queue. N = 257 or 259 log one, at the step's
  own time: the 257th activation notifies on the spot, its waiter runs
  after the step, and the later spot notifies reach no waiter. From a
  thread woken off the grid (`wait 0.06`), which runs after the rounds,
  N = 1 logs one `"trigger"` the next frame, and N = 258 logs one at the
  step's time and 256 the next frame, one per round. INFERRED: on 1.5b
  and UO too a thread due past the rounds' clock runs after the count is
  zeroed, so its entries wait for the next frame's rounds.

vcod: `engine_notifies` holds what retail notifies on the spot and
`trigger_fires` the queue; `ScriptRuntime::run_threads` raises the first at
the top of the script frame and `ScriptRuntime::trigger_rounds` hands out
the second. `GameHost::queue_trigger` refuses an entry past 0x100
(`TRIGGER_QUEUE_LEN`); a cmd's touch and the lookat path then push the
notify to `engine_notifies`, and a `trigger_damage` activation returns it
with `"damage"` from `damage_nonclient`, so a builtin's blast notifies it
inside the step. Gate: `crates/server/tests/touchorder_ab.rs`, both probes
at each patch. A notify raised between frames (`ClientDisconnect`'s
`menuresponse`, `ClientBegin`'s `begin`, a client command's) wakes its
waiters at once, and they run in the next frame's first pass ("The packet
pass" above; `cod11-server-handshake.md`, "When the game hears a drop" and
"When the client's `begin` is heard").

**`getentarray` order: map entities first, then spawn order, VERIFIED.** The
map's own four `script_origin` entities come back before three the probe
spawned, and those three in spawn order. The probe prints only `targetname`
(`probe_ents.gsc:32`), so the ordering *within* the map's own four is not
measured and the underlying key — entity number or otherwise — is not
established.

**`i%count` compiles on retail, VERIFIED.** The tight spelling of modulo
evaluates to `1` for `7 % 3`, so retail's lexer does not read `%` before an
identifier as an animation reference in that position. vcod's lexer gives the
`%` scan a one-token lookback: it is an animation reference only where the
previous token cannot already have ended an operand (not an identifier,
number, `)` or `]`). Across the 799-file corpus that reclassifies nothing —
3432 `Anim` tokens before and after.

**`-2147483648` is not reachable on retail, VERIFIED.** Both the direct
literal and `(int)(-2147483648)` yield `-2147483647`, so the magnitude
saturates at `i32::MAX` before the unary minus applies: an overflowing
integer literal clamps rather than widening to a float. vcod's `read_number`
does the same, which is also what the two stock scripts that ship a literal
past `i32::MAX` as an effectively-infinite sentinel
(`maps/carride.gsc:1606`, `maps/redsquare.gsc:1547`) mean by it.

**`game` is array-typed, not a struct, VERIFIED (`probe_game`).**
`game["allies"] = "russian"` then reading `game["allies"]` back round-trips,
`game.size` is 1 after one bracket write and 2 after two, and the value
survives a `wait` (`game["allies"]` still reads `"russian"` afterward) —
`game` is the cross-map persistent global retail keeps it as. `game.foo = 1`
is not merely fatal at runtime the way an out-of-place index assignment is
(§9's `probe_autoviv` case): it is a *compile*-time rejection, `not an
object`, that voids the whole script before `main()` runs at all
(`probe_game_dotwrite`). `level["k"] = "v"` gets the same compile-time
treatment the other way, `not an array, string, or vector`
(`probe_level_bracket`), and so does `self["k"] = 1`
(`probe_self_bracket`). VERIFIED on 1.1d and UO 1.51b, each console naming
the line and the message. `game` is genuinely array-typed, not a struct
that also answers to brackets, and vcod models it as one (`Vm::game`,
`ArrayId`, `Op::LoadGame` pushes `Value::Array`).

**Why: the compiler types the bare globals.** cod_lnxded 1.1d's object
emitter (0x08096d34) switches on the operand's node kind: four kinds
compile (among them three keyword nodes, 0x1c, 0x1d and 0x1f, emitting
opcodes 7, 8 and 9) and any other is `not an object` (VERIFIED, the switch
and the string). The array emitter (0x08096bb8) does the same with its own
set, which holds the keyword node 0x1e (opcode 0xf) and none of the other
three, and fails the rest with `not an array, string, or vector`
(VERIFIED). `game` is 0x1e and `level` and `self` are object nodes
(VERIFIED by the probes); that the third object node is `anim` is INFERRED
from the keyword set, as no probe reaches `anim`. Three emitters call the
object emitter (0x08093e44, 0x08093f98, 0x080940d8) and several the array
emitter; that field and index reads go through them as the writes do is
INFERRED from that, not probed. vcod refuses `game.x`, `level[k]`,
`self[k]` and `anim[k]` read or written (`Compiler::check_object`,
`check_array`, `crates/gsc/src/compile.rs`). `.size` is not a field: the
interpreter runs it as opcode 0x4e (VERIFIED, cod_lnxded's dispatch calls
0x080a2f28 there), and `game.size` and `level.size` both compile (VERIFIED,
`probe_game`, `probe_level_size`), so vcod skips the object check for it.

**`.size` of an object that is not an array is 1 (`probe_level_size`).**
Set one dot-field on `level` or two, `level.size` reads `1` either way, on
1.1d and on UO 1.51b (VERIFIED). The size opcode's handler (cod_lnxded
0x080a2f28, VERIFIED) reads a string's length; for an object (type 7) it
answers the element count when the object is an array (type 0xf) and `1`
for any other kind, struct, entity or hudelem alike; any other operand is
`size cannot be applied to %s`. vcod's `Op::LoadField` answers 1 for a
`Value::Struct` or `Value::Entity` under `size`.

Three of the five that run in `crates/server` were measured for the gametype
bootstrap, and each is there because it needs the cvar table or a real map
load:

**Bootstrap order, VERIFIED (`probe_bootstrap`).** The gametype script's
`main()` runs first, then the map script's, then the gametype's
`Callback_StartGameType`. `game["allies"]` reads `undefined` inside the
gametype's own `main()` and `russian` by `Callback_StartGameType` on
`mp_pavlov`, whose map script is what sets it, which places the map's
`main()` between the two. The plan for this stage had the first two the
other way round. Measured in the same run: a bare `thread f()` runs `f` to
its first suspend before the caller's next line, so a thread that never
waits has finished by the time the call returns.

**Cvar coercion, VERIFIED (`probe_cvar`).** `getCvarInt` and `getCvarFloat`
read an unset or non-numeric cvar as `0`, and take a numeric prefix where
there is one: `getCvarInt("12abc")` is `12`, which Rust's own `parse`
rejects. Cvar names are case-insensitive, `probe_MixedCase` and
`probe_mixedcase` reading back one value. `randomInt(1)` never returns `1`,
so the bound is exclusive. `getTime` is non-negative; its units are
milliseconds of `level.time` ("The script clock" below).

**`delete()` defers the free, VERIFIED (`probe_delete`).** The entity stays
in `getEntArray` and in its count immediately after `delete()`, a spawn
right after takes a fresh number rather than the deleted one, and after a
150 ms wait the number is back in circulation. The mechanism and the 100 ms
the engine actually arms are section 14 of
`docs/research/cod11-gsc-object-model.md`; what the probe pins is the bound,
not the constant, because its frames step 50 ms at a time.

**A handle outlives its object as a dead entity (`probe_stale_handle` and
six `probe_stale_*` probes, one per fatal case).** What a script holding a
handle to a freed entity or hudelem reads, whether the handle sits in a local,
in a field of `level` or in a field of another entity:

- VERIFIED: inside `delete()`'s 100 ms window the entity is live.
  `isDefined` reads 1, a field reads back and takes a write, and
  `getEntityNumber` answers the entity's own number.
- VERIFIED: past the free, `isDefined` reads 0, and still reads 0 once a
  later spawn has taken the same entity number (`stale_ent_slot_reused 1`).
- VERIFIED: `==` between the dead handle and the slot's new occupant is 0,
  and between the dead handle and its own copy in `level` is 1.
- VERIFIED: a hudelem is dead the moment `destroy()` returns: `isDefined` 0
  at once, through a local, `level` and an entity field (`sd.gsc`'s
  `other.progressbackground` shape), and `==` against the next `newHudElem`
  is 0. Whether that next element took the destroyed record the probe cannot
  say, since script has no number for a hudelem.
- VERIFIED: a field read or a field write through a dead handle, entity or
  hudelem, is fatal with `dead entity is not an object`, and a method call
  (`getEntityNumber`, a second `destroy`) is fatal with `dead entity is not
  an entity`.
- VERIFIED: `dead entity` is an entry of the engine VM's value type-name
  table in `cod_lnxded` (the string at 0x80d758c, the table at 0x80e3180,
  beside `entity`, `dead thread` and `dead object`), and the two messages are
  the format strings `%s is not an object` (0x80d773d, 0x80d7d16) and `%s is
  not an entity` (0x80d7be9). The free reaches the VM through the game
  module: `G_FreeEntity` calls `Scr_FreeEntity` at 0x66b7d, which calls
  `Scr_FreeEntityNum` at 0x62066; `HudElem_Free` calls `Scr_FreeHudElem` at
  0x4c57e; `Scr_FreeEntityNum` (0x6c0f8) calls through the engine import slot
  at 0xc1088 (call targets read off the relocations). INFERRED, from the
  message and the chain: the free turns the script's object into the dead
  type in place, which is why every copy of the handle goes dead at once and
  a new spawn gets an object of its own.

vcod reproduces this with a generation. `EntId` carries the slot's
generation beside its number, `ObjectTable` bumps it on every free (entity,
hudelem and client alike), a handle whose generation is not the slot's
reaches nothing, `Host::is_live` says so to the interpreter, which raises
retail's two texts on a field access or a method call through it, and `==`
compares both halves, which is retail's equal-only-to-its-copies. Not
measured: a dead handle passed to a builtin other than `isDefined`
(`linkTo(dead)`), or used with `notify`, `waittill`, `endon` or `thread`;
ours leaves each to the builtin or the scheduler.

**Freeing an object ends every thread whose `self` it is
(`probe_stale_self`, `probe_stale_self_hud`).** VERIFIED on 1.1d and on
UO 1.51b (`probe_stale_self` on both, the hudelem case on 1.1d): a
thread started on an entity asleep in `wait` over the entity's free, and
one started on it waiting on a `level` notify sent past the free, never
log another line and raise no error; a thread on `level` holding the
entity in a local wakes and reads `isDefined` 0. A thread on a hudelem
that `destroy()` frees ends the same way. INFERRED, from the free chain
above: the end rides `Scr_FreeEntityNum`'s engine call, which frees the
object a thread's `self` refers to. vcod drops such a thread when the
scheduler picks it to run (`Vm::step_runnable`, through `Host::is_live`),
which no script can tell apart from an end at the free: the thread runs
no step in between. The stock jeep self-destruct timer depends on it
(`_jeepdrive_gmi::process_deactivate`, `client-probes/probe_jeepdeact`,
`crates/server/tests/fixtures/vehicles/mp_foy-jeep-deactivate-uo.txt`):
a jeep destroyed while it waits is deleted, and retail's server runs past
the timer's `dodamage` line with nothing in its console.

### Runtime errors below developer 1

Every probe capture in this section was taken at `developer 1`
(`tools/run_probe.sh`), where a runtime error is fatal (section 9's
opening list). A server below `developer 1`, which is retail's default
and vcod-server's, runs on past one.

- `Scr_Init` takes its two flags from cvars at each level's script init:
  coduo_lnxded 0x80714f1 passes `developer || logfile`, `developer_script`
  and `developer` to 0x80aa818, which stores them at 0x829fa00, 0x829fa04
  and 0x829fa08 (VERIFIED decompile). `RuntimeError` (0x80a3f4a) prints
  only when the first or the terminal flag (0x8433eac) is set, and calls
  `Com_Error` only when `developer` (0x829fa08) or the terminal flag is
  (VERIFIED decompile). `Scr_TerminalError` (0x80b055c) sets the terminal
  flag; its callers are the variable-pool exhaustion
  (`exceeded maximum number of script variables`) and `potential infinite
  loop in script` (VERIFIED decompile).
- Below that, `VM_Execute`'s error arm (the switch on the failed opcode
  after 0x80ab5ac's dispatch) frees the instruction's operands, leaves its
  result undefined, steps past its operand bytes, and the loop runs on: a
  conditional jump pops its condition and is not taken (VERIFIED
  decompile; INFERRED, read off the arms, that every arm has this shape).
- `client-probes/probe_undef_if` at `developer 0` (VERIFIED, 1.1d on
  mp_carentan and UO 1.51b on mp_cassino, port 34631/34632, 2026-10-21):
  `if (level.unset)` and `if (!level.unset)` both take their branch, the
  statements after them run, `level.unset + 1`, `!level.unset` and
  `level.unset_struct.field` read undefined, `wait level.unset` returns at
  once (0 ms), and the thread and another thread both run on. A builtin's
  own error (`distance(undefined, ...)`) reads undefined on 1.1d; on UO
  1.51b it aborts the server process (SIGABRT) instead (VERIFIED, both
  runs). At `developer 1` the same probe dies on the first `if` with
  `cannot cast undefined to bool` and `ERROR: script runtime error`
  (VERIFIED, UO).

vcod's VM takes the mode from `developer` at the load
(`Vm::set_recover_errors`, `ScriptRuntime::load_from`): on, a failed
instruction leaves `stack_effect`'s results undefined and the thread runs
on (`Vm::step_frames`); the error is still recorded in `Vm::aborts`.
vcod's own errors (a builtin it lacks, its instruction cap, a wait in
`call_now`) still end the thread, and a builtin's error recovers on UO
as on 1.1d rather than ending the process. A gate that replays a
`run_probe.sh` capture runs at `developer 1` (`semantics_ents.rs`'s
`probe_cvars`, `linkto_ab.rs`). Stock UO `tdm.gsc` 815 is the case that
shows in play: a death to no player (a tank its own attacker,
coduo-vehicles.md 11.6) reads the unset `level.teamkill_penalty`, takes
the branch, and costs the victim's team a point.

### The map-cycle builtins, and the `sd` connect race

Five builtins end or restart a level or move the scores it ends on. The
addresses each rests on are in `docs/research/cod11-map-cycle.md` sections 1,
2 and 6, and in `docs/research/cod11-sound-system.md` section 9; what a script
author has to know about them is here.

**`map_restart(savePersist)` and `exitLevel(savePersist)` both queue a console
line and return, VERIFIED (sections 1 and 2).** Neither reloads anything
itself: `map_restart` queues `map_restart`, `exitLevel` clears the team scores
and the connected clients' scores and queues `map_rotate`, and the engine runs
the line at the top of a later frame, so the statements after the call in the
calling thread still run. The pair shares one per-level latch and a second
call is a fatal script error naming whichever of the two got there first.
The argument is the whole of the persistence gate: `game[]` and every client's
`pers[]` are carried into the next level when it is non-zero and freed when it
is zero, on both paths alike, while `level` is always new and every entity
handle stored in `game[]` is dropped whatever the flag says. VERIFIED by
`probe_persist_restart`, `probe_persist_exit` and `probe_persist_exit_save`
against retail: `map_restart(true)` keeps `game[]`, `exitLevel(false)` frees
it, `exitLevel(true)` keeps it, and in all three the entity handle is gone.
`dm.gsc` passes 0 at its map end and `sd.gsc` passes 1 at its round end, which
is why a `dm` client is put back through the team menu after a restart and an
`sd` client is not.

**`getTeamScore(team)` and `setTeamScore(team, n)` take `"allies"` or
`"axis"` and nothing else, VERIFIED (section 6.3).** Any other string is a
fatal script error. The setter writes configstring 5 for axis and 6 for
allies, formatted `%i`, and dirties the flag whose drain pushes the `b`
scoreboard to every client in intermission. It and `CalculateRanks` are the
only two writers of that flag. The one other scoreboard that goes out unasked
is `player_die`'s, to the victim's followers
(`docs/research/cod11-spectator-follow.md` 4).

**`playLocalSound(alias)` reaches one client only.** VERIFIED
(`docs/research/cod11-sound-system.md`, section 9): it allocates the alias the
first empty configstring in the `CS_SOUNDS` block, at first use rather than at
load, and sends that client the reliable command `s <index>`, non-positional,
with no entity and no origin. The receiver must be a player, and a spectator
counts as one, which is how a headless probe hears the announcer at all. It is
what the stock gametypes' round-end lines travel on.

**`sd.gsc`'s `Callback_PlayerConnect` opens the team menu before it suspends,
VERIFIED from the stock script.** The `else` arm calls `openMenu(game
["menu_team"])` and only then `spawnSpectator()`, which after its opening
`notify`, its `resettimeout()`, its session-state assignments and the
`self spawn(...)` that places the camera calls that file's own
`updateTeamStatus()`, and that function opens with `wait 0` and a comment
saying the wait is there so `Callback_PlayerDisconnect` can finish first.
INFERRED, from that order against the scheduler: the `t` command is on
the wire while the connect thread is suspended and before it reaches the
`waittill("menuresponse")` loop, so a menu answer that arrives inside that one
frame notifies an event nothing is parked on and is dropped. `dm.gsc` has no
such wait between its `openMenu` and its loop.

### Precache builtins

The engine's script-function table (`tools/re/dump_builtins.py`) holds nine
precache builtins: `precacheModel` (0x5ecb8), `precacheShellShock`
(0x5ecf0), `precacheItem` (0x5ed28), `precacheShader` (0x5eda8),
`precacheString` (0x5ee00), `loadFx` (0x5ef88), `precacheMenu` (0x5a92c),
`precacheStatusIcon` (0x5aa08) and `precacheHeadIcon` (0x5aae4). There is no
`precacheTurret` or `precacheFx` in MP.

- VERIFIED, disassembly: the first five test `level.initializing`
  (`level+0x1c`, set by `G_InitGame` while the gametype's `main`, the map's
  `main` and `Callback_StartGameType` run to their first wait,
  `cod11-events-and-fx.md` 2) before they read their argument, and on 0
  raise `Scr_Error("<builtin> must be called before any wait statements in
  the gametype or level script\n")` (strings 0x77580, 0x775e0, 0x77640,
  0x776c0, 0x77740). The test does not look at the name, so a name already
  precached is refused too.
- VERIFIED, disassembly: the menu and icon builtins have no such test. Each
  scans its whole range with `Q_stricmp`; a match is
  `Com_DPrintf("Script tried to precache the <menu|player status icon|player
  head icon> '%s' more than once\n")` and nothing else, a full range is
  `Scr_Error("Too many menus precached. Max allowed menus is %i")` (32),
  `"Too many player status icons precached. Max allowed is %i"` (8) or
  `"Too many player head icons precached. Max allowed is %i"` (15)
  (0x77180..0x77360).
- VERIFIED, disassembly: none of the eight but `loadFx` calls `Scr_AddInt`,
  so each answers `undefined`. `precacheShader` raises
  `Scr_ParamError(0, "Shader name string is empty")` on `""`;
  `precacheString` skips an empty key; `precacheItem` raises `unknown item
  '%s'` when `BG_FindItem` misses.
- VERIFIED: every indexer behind them (`G_ModelIndex` 0x66ed8,
  `G_ShaderIndex` 0x65ee8, `G_LocalizedStringIndex` 0x65e30,
  `G_ShellShockIndex` 0x67540, `G_EffectIndex` 0x65fa4,
  `G_FindConfigstringIndex` 0x66e30, `G_TagIndex` 0x66fa8,
  `G_SoundAliasIndex` 0x675c0) matches with `strcasecmp`. The hint-string
  indexer (0x5a238) is the one `strcmp`.
- VERIFIED, `crates/gsc/tests/fixtures/semantics/probe_precache_*.gsc` on
  retail 1.1d (2026-10-13), `retail-captures.txt`: every precache returns
  `undefined` (`isDefined` 0); past one `wait 0.05` a new menu, head icon and
  status icon are taken while `precacheModel`, `precacheShader`,
  `precacheString`, `precacheItem` and `precacheShellShock` of a name the
  level already precached each die with their message; a status icon in
  another case is no new slot; the ninth status icon dies with `Too many
  player status icons precached. Max allowed is 8`; an empty model or string
  passes and an empty shader dies; an unknown item dies.
What the indexers do past initializing, for the builtins and fields that
reach them at run time, is the next subsection.

vcod: `crates/server/src/game/builtins/precache.rs` and
`Allocators::index`'s case-folded scan; the A/B is
`probe_precache_wait_matches_retail` in
`crates/server/tests/semantics_ents.rs`.

### Arrays are values

`probe_array_copy` (VERIFIED, 1.1d on mp_pavlov, port 28991,
2026-10-25): after `b = a`, a write through either leaves the other's
size alone (`local_copy_write 1 2`, `local_orig_write 2 1`), and so does
a string key written into a copy of a `level` field
(`field_copy_strkey 1 2`), a write through an argument (`arg_write 1`),
a write into a copy of an outer array's nested one (`nested_write 1 2`),
into a copy of an element (`element_copy_write 1 2`) and into a field
assigned from another field (`field_to_field 1 2`). CoD4 1.7 prints the
same lines, and a struct an array holds is shared there (cod4-gsc.md
10). vcod copies at the write (`Heap::own`); before that its arrays were
shared, which no 1.1 stock script was seen to depend on. INFERRED.

### Indexing past the first wait

Which builtins index a configstring at run time comes from the relocations
against each indexer in `game.mp.i386.so` (`readelf -r`, mapped onto the
builtin tables `tools/re/dump_builtins.py` prints). Addresses are `.so`
file addresses.

- INFERRED, from the branch on `level+0x1c` in each: `G_ModelIndex`
  (0x66ed8), `G_ShaderIndex` (0x65ee8), `G_LocalizedStringIndex` (0x65e30)
  and `G_EffectIndex`
  (0x65fa4) scan their range with `strcasecmp` and return a match at any
  time. A miss allocates only while `level.initializing` (`level+0x1c`,
  loaded at 0x65ef4 in `G_ShaderIndex`) is set; on 0 it is
  `Scr_Error(va("model '%s' not precached"))` (0x79b3a) for a model and
  `Scr_Error(va("%s \"%s\" not precached", "shader" | "localized string" |
  "effect"))` (0x79ae0) for the rest. `""` is index 0 in all four.
- VERIFIED, relocations and disassembly: `G_SetModel` (0x67020, behind
  `setModel`) and
  `G_EntAttach` (0x66190, behind `attach`) inline `G_ModelIndex` with the
  same gate and the same message (0x67088, 0x66247). `setViewModel`
  (0x4512c) calls `G_ModelIndex`. `setShader` (0x4b5b0), `objective_add`
  (0x5a2d0) and `objective_icon` (0x5a5d8) call `G_ShaderIndex`; `setText`
  (0x4c590) and the hud `label` setter (0x4c400) call
  `G_LocalizedStringIndex`.
- INFERRED, from the parameter-count compares at 0x4b5c3: `setShader`
  takes one parameter or three; with one the width and height are 0
  (0x4b609).
- INFERRED, from the absence of a `level+0x1c` load in either:
  `G_ShellShockIndex` (0x67540, only `precacheShellShock` calls it) and
  `G_SoundAliasIndex` (0x675c0, behind `playSound`, `playLoopSound`,
  `playLocalSound` and the spawn functions) have no gate: a new name
  allocates at any time.
- INFERRED, disassembly: the `statusicon` setter (0x41b50) goes through
  `GScr_GetStatusIconIndex` (0x5c7c8) and the `headicon` setter (0x41bd4)
  through `GScr_GetHeadIconIndex` (0x5c840). Neither allocates or looks at
  `level.initializing`: `""` is 0, a `Q_stricmp` match over the 8 (15)
  slots is its 1-based slot, and anything else is
  `Scr_Error("Status icon '%s' was not precached\n")` (0x772c0) or
  `"Head icon '%s' was not precached\n"` (0x773a0), during
  initialization too. The getter reads the slot back, so an icon set in
  another case reads in its precached spelling.
- VERIFIED, `probe_late_*.gsc` on retail 1.1d (2026-10-13),
  `retail-captures.txt`: past one `wait 0.05`, names the gametype
  precached in lower case and set in upper case pass `setModel`,
  `attach`, `setShader` (three arguments and one), `setText`, `label`,
  `objective_add`, `objective_icon`, `statusicon`, `headicon`,
  `shellshock` and `setViewModel`; `.model`, `getAttachModelName`,
  `.statusicon`, `.headicon` and `getViewModel` read back the precached
  spelling. `playSound` of an alias nothing indexed passes. A name nothing
  precached dies in each of `setModel`, `attach` and `setViewModel`
  (`model 'xmodel/probe_never' not precached`), `setShader` and
  `objective_add` (`shader "probe_never" not precached`), `setText` and
  `label` (`localized string "PROBE_NEVER" not precached`), `statusicon`
  (`Status icon 'probe_never' was not precached`) and `shellshock`
  (`shellshock 'probe_never' was not precached`). The player-scoped probes
  run on a test client from `addTestClient`, whose first bot retail refuses
  within `sv_reconnectlimit` of the start (`reconnect rejected : too soon`;
  `cod11-gsc-object-model.md` 25), hence their `wait 5`.

- VERIFIED, `dump_builtins.py` and disassembly: `shellshock` and
  `stopShellShock` are generic entity methods 28 and 29 (0x5bb44,
  0x5f468) that refuse a non-client (`only valid on players; called on
  entity %i at ...`).
- INFERRED, disassembly: `shellshock(name, duration)` wants exactly two
  parameters (`USAGE: <player> shellshock(<shellshockname>, <duration>)\n`).
  It scans configstrings 1101..1115 (`0x44c + i`, `i` 1..15) with
  `strcasecmp`, empty slots included, so `""` finds the first free slot;
  a miss is `shellshock '%s' was not precached\n` (0x780e0). The duration
  is `(int)(duration * 1000 + 0.5)` (0x5bc7a, constants 0x78104 1000.0 and
  0x78108 0.5, truncating); above 60000 as an unsigned compare, so a
  negative one too, it is `Scr_ParamError(1, "duration %g should be >= 0
  and <= 60")` (0x780a0) of that value times 0.001. Then
  `ps.shellshockIndex` (gclient +0x3dc) gets `i`, `shellshockTime`
  (+0x3e0) `level.time` and `shellshockDuration` (+0x3e4) the
  milliseconds (0x5bcf1..0x5bd14).
- VERIFIED, `probe_late_shockspan`: `shellshock("default", 61)` dies with
  `duration 61 should be >= 0 and <= 60`; 0, 1.5 and 60 pass.
- INFERRED, disassembly: `stopShellShock()` takes no parameters
  (`USAGE: <player> stopshellshock()\n`) and zeroes the three fields.
- INFERRED, from `ClientSpawn`'s `bzero` of the `gclient_t` (0x42804, 0x22c4
  bytes, the session block at +0x20d0 saved around it): a spawn clears the
  shellshock.
- The client effect lives in cgame: `cod11-shellshock.md` has the
  `.shock` files, the 27 `cg_shock_*` cvars, the screen blend, view kick,
  mouse damping, sound mix and the `wbuttons` 0x4 slowdown, and that no
  stock MP script calls `shellshock`.

vcod: `Allocators::index_precached` (`crates/server/src/configstrings.rs`)
is the gate for `setModel`, `attach`, `setViewModel`, `setShader`,
`objective_add`, `objective_icon`, `setText`, `label` and `loadFx`;
`setModel` and `attach` keep the precached spelling. The `statusicon`
setter refuses an icon not precached and keeps its precached spelling.
`shellShock` and `stopShellShock` (`builtins/client.rs`) write
`GameHost::client_shellshock`, which the sim sends as the three playerstate
fields; the client's half is `play::shock` (`cod11-shellshock.md`). The A/B is
`probe_late_index_matches_retail` in `crates/server/tests/semantics_ents.rs`.

### `vectorToAngles`

VERIFIED, `vectoangles` (1.1d 0x3de94, its `.rodata` 0x72ae8..0x72b08):
yaw is `atan2(y, x) * 180 / PI` and pitch `atan2(z, sqrt(x² + y²)) *
-180 / PI`, each stored as a float and given 360 while negative, roll 0;
a vector with no horizontal part reads pitch 270 when `z > 0` and 90
otherwise. Unlike Q3's, which negates a pitch already wrapped, both
angles come back in [0, 360): 16 degrees down reads 15.99, not -344.01.
VERIFIED, retail UO's log of `probe_dmgtouch` and `probe_belltower`
(cod11-combat.md 14.8): `(15.99, 180.00, 0.00)`, `(34.99, 0.00, 0.00)`
and `(51.34, 90.00, 0.00)` for three downward aims. vcod:
`builtins::math::vector_to_angles`; before 2026-10-18 it returned Q3's
negative pitch.

### Casting a string

VERIFIED, cod_lnxded 1.1d: the casts to bool (0x080a3c30), int
(0x080a3de0, the VM's case 0x31) and float (0x080a3fa0) read a string
through `strtol(s, 0, 10)` and `strtod`, so the string's numeric prefix
is the value, blanks and a sign included and anything after ignored. A
result of 0 stands only when the first character past those under `!`
and a sign is `0`, or `.` then `0`; otherwise the cast raises `cannot
cast "%s" to int` (to bool, to float). UO's engine has the same three
(coduo_lnxded 0x080aa9a0, 0x080aaae2, 0x080aac34). VERIFIED, retail
1.1d's log of `probe_cast_string`: `(int) "5000"` is 5000, `(int) " -7"`
-7, `(int) "12abc"` 12, `(int) "3.9"` 3, `(int) "0.9"` and `(int) ".05"`
0, `(float) "1.5e1x"` 15, and `(int) "abc"` dies with `cannot cast "abc"
to int`. A condition and unary `!` take the bool cast, so `!"0.5"` is 1:
`strtol` stops at the point.

UO's stock `maps/mp/_birdbush.gsc` (pakuo01) adds `(int)
self.script_noteworthy`, a BSP key and so a string, to `(int) gettime()`
on every trigger of a `birdbush` brush (mp_uo_stanjel's, in the UO soak,
`cod11-client-soak.md`). vcod's `(int)` and `(float)`
refused any string before 2026-10-28: the sum came back undefined, the
next trigger compared undefined, and the birds never flew; vcod-server
logged four script errors per trigger. vcod: `vm::interp`'s
`string_int` and `string_float`, gated by `semantics_ab.rs` on the probe.

### Still unestablished

- **Unary `!` on a string that will not parse.** `!""` is fatal and `!"1"`
  and `!"0"` are numeric (§9), but `!"a"` was never put to a retail server.
  `not_of` (`crates/gsc/src/vm/interp.rs`) treats any unparseable string as
  the same fatal cast `!""` measured, which is the reading that follows from
  the empty case, and nothing on the bootstrap path reaches it: every
  `scr_allow_*` value is `"0"` or `"1"`.
- **`undefined == undefined`.** Not probed. Retail's error message for the
  mixed case names a "pair has unmatching types", which two `undefined`s are
  not, so equal is the reading that message supports — but that is inference,
  not measurement. vcod answers true on that inference (`values_equal`,
  `crates/gsc/src/vm/interp.rs`); every *mixed* pair with `undefined` errors,
  which is the measured half.
- **String comparison against a localized string** (`&"KEY"`). No stock
  script does it. How one renders is measured on two paths: a message
  argument renders as `KEY\x15` (`crates/server/src/game/builtins/message.rs`),
  and so does a `setCvar` value, since `_teams::scoreboard`'s
  `setcvar("g_TeamName_Allies", &"MPSCRIPT_AMERICAN")` reaches the retail
  gamestate as `MPSCRIPT_AMERICAN\x15`
  (`crates/server/tests/fixtures/configstrings/mp_carentan-sd.txt`, slots
  223 and 224). Plain concatenation is still unmeasured.
- **Equality outside the pairs above.** `values_equal`'s catch-all
  (`crates/gsc/src/vm/interp.rs`) is `a == b` on `Value`'s derived
  `PartialEq`, so a genuinely mixed pair (`vector == "a"`, `entity == "a"`)
  answers `false`, but two vectors, entities, arrays or function pointers
  compare by value and can answer **true**. Retail's "pair has unmatching
  types" message suggests the mixed case is fatal there. Of the same-type
  cases only entities are measured (`probe_stale_handle`, above: a handle
  equals its copies, dead or not, and nothing else); vectors, arrays and
  function pointers are plausible but unmeasured.

## 10. Divergences kept as documentation, not code

Ten places where the implementation made a deliberate call the corpus
cannot settle, recorded here rather than silently baked into behaviour that
looks authoritative. Four more are gone. `delete()` used to free the entity
on the spot where retail defers it, and the entity think scheduler stage 3
added closes that (section 14 of
`docs/research/cod11-gsc-object-model.md`, and `probe_delete` in §9). A
script handle used to be a bare entity number, so a stale one read whatever
spawned into its slot next; it carries a generation now and reads as
retail's dead entity (`probe_stale_handle` in §9). `game.foo` and
`level["k"]` used to compile and fail at the line; the compiler refuses
them now, as retail's does (§9, `probe_game_dotwrite`). Two notifies of one
event in one step losing the second was listed here and is retail's
behaviour (`probe_notify_twice`, §9).

- **A thread killed by *another* thread's `endon` notify mid-step keeps
  executing to its next suspend.** The self-notify half of this is no longer
  a divergence: a thread's own `notify` does not kill it, measured against
  retail by `probe_endon_self` (§9) and implemented by `Vm::notify_from`.
  What remains is a nested spawn's notify.
  `main`'s `self thread killer();` runs `killer` synchronously
  (`Vm::spawn`), and if `killer` notifies the event `main` just registered
  via `endon`, that notify (`Vm::notify`) removes `main`'s entry from
  `self.threads` immediately — while `main`'s own `step_frames` call is
  still running further up the native call stack, oblivious, since it only
  consults `self.threads` for endon registration and to kill/wake others,
  never to check whether its own thread id still exists. `main` runs on to
  its next `wait`/`waittill`/return; only then does the outer `step_thread`
  look for `main`'s entry to write the suspended frames back, find it
  already gone, and drop them — matching `step_thread`'s own doc comment
  ("simply gone by the time we look again"). Measured: `main() { self
  endon("die"); self thread killer(); sideEffectA(); sideEffectB(); wait 5;
  done(); }` ends with `threads=0`, but both side effects having run for
  real; `done()` never does, since the thread that would have called it was
  already discarded by the time its `wait 5` resolved. The alternative —
  `step_frames` re-checking `self.threads` for its own id after every
  instruction — is real reentrancy-safety cost for a pattern (a thread
  killing itself via a nested spawn's notify, mid-step) no corpus script
  seems to rely on either way; deferred to the next project. Pinned by
  `sched::tests::a_thread_killed_by_its_own_endon_mid_step_still_runs_to_its_next_suspend`.
  CoD2's stock tdm does rely on it, and retail 1.3 stops the thread at
  once (cod2-protocol.md 16.8); a CoD2 level turns that on
  (`Vm::set_kill_mid_step`, a list of the threads a notify ended, checked
  before each instruction). 1.1 is not measured and keeps the old rule.
- **A bare local is accepted as a statement.** vcod compiles `x;` (and
  so `waittillframeend;`, which 1.1 has no keyword for) as a local read
  and a pop. VERIFIED, a 1.1d probe: retail's compiler refuses both lines
  as `bad syntax` (cod2-gsc-diff.md 3.2). No stock 1.1 script has one, so
  the looser grammar stays; CoD2's `waittillframeend` lives behind
  `Dialect::Cod2`.
- **A runtime error aborts the thread, where retail kills the server.**
  Every fatal in §9 — a non-numeric condition, `"str" + undefined`,
  `undefined == 0`, `"a" < "b"`, `1 / 0` — raises the equivalent
  `ErrorKind` in vcod, which unwinds that thread's frames and logs; the
  other threads and the server keep running. Retail prints a `script
  runtime error` block and shuts the server down. This is deliberate and
  predates the measurement: a third-party map script must not be able to
  take the server down. `crates/gsc/tests/semantics_ab.rs` therefore checks
  only that both sides *stopped* at the same expression, which the compared
  output lines pin, not that they stopped the same way.
- **Multiple `#using_animtree` directives in one file resolve to the last
  one.** `compile_file` (`crates/gsc/src/compile.rs`) takes
  `file.animtrees.last()` because the AST does not track which directive was
  lexically active at each function's position. This is inert today —
  nothing in the VM resolves a `Value::Anim` to anything a builtin can act
  on — but wrong in general: of the 38 files in the corpus that read
  `#animtree` back in expression position, 35 carry more than one
  `#using_animtree` directive, so "last wins" is the common case, not the
  exception. The eventual fix belongs in the parser, which is the only layer
  that still knows each function's source position relative to the
  directives around it.
- **Array iteration order is interning order, not retail's.** `ArrayKey`'s
  derived `Ord` (`crates/gsc/src/heap.rs`) puts every `Int` before every
  `Str` and orders `Str(Atom)` by the order the atoms were interned, not by
  text. Deterministic, which is all iteration needs while nothing lists
  keys, and knowingly not retail's enumeration order — a `getarraykeys`-
  style builtin will have to settle that against a probe first.
- **A float array subscript truncates toward zero.** `array_key`
  (`crates/gsc/src/vm/interp.rs`) turns `a[1.9]` into `a[1]`, matching the
  `(int)` cast rather than rejecting it, on the reasoning that a loop
  counter that drifted to a float is likelier than a script meaning to key
  by a fractional value. Retail's behaviour here is unmeasured; it may well
  be fatal.
- **A vector refuses every key but the integers 0, 1 and 2.** `LoadIndex`
  (`crates/gsc/src/vm/interp.rs`) reads `v[0]`..`v[2]` as a float and
  answers any other key on a vector, an out-of-range integer or a float
  among them, with a `BadType`; a store into a vector component is refused
  the same way. So a float key on a vector is refused where a float key on
  an array truncates (the entry above). Retail's answer to both is
  unmeasured; the corpus pins only the in-range integer read (§2).
- **Every endon kill lands before any notify wake.** `Vm::notify`
  (`crates/gsc/src/vm/sched.rs`) makes two passes over `threads` — kills
  first, then wakes — so a thread that has both an `endon` and a `waittill`
  on one event is killed rather than woken, regardless of which it
  registered first. The wake order *within* each pass is measured (start
  order, `# probe_notify`); the ordering *between* the two passes is not.
- **`radiusDamage` walks clients and turrets and nothing else.** The falloff, the
  line of sight, the direction and the callback timing are all settled and no
  longer divergences. VERIFIED: the builtin at `.so` 0x5eef4 and
  `G_RadiusDamage` (`.so` 0x4a3f4) have been read out, and both the linear
  curve from `maxDamage` at the blast to `minDamage` at the range and
  `CanDamage`'s five-trace fraction are what `crate::game::combat`'s
  `Blast::hit` and `can_damage` compute, second-chance arm included
  (`docs/research/cod11-combat.md` section 14). VERIFIED: each of those five
  probes is a `trap_LocationalTrace(&tr, point, origin, targ->s.number,
  0x02802091, bulletPriorityMap)` (14.3). INFERRED, off that mask and pass
  entity: another live player's body stops a probe and the victim's own and a
  corpse do not, which the retail runs of `client-probes/probe_blastbody`
  measure (14.4: 13 of a flat 20 behind a standing player, 6 or none at other
  yaws, 20 behind its corpse); vcod's `can_damage` traces every other live
  player's posed body and reads the same numbers. The callback half is
  retail's walk (combat doc 14.5): each victim's
  `CodeCallback_PlayerDamage` runs to its first `wait` before the next
  victim is measured, as a `Cx::spawn_then` whose `Host::spawn_returned`
  takes the next turn, and the whole walk ends before the calling thread's
  next instruction, so a script that damages and then reads `self.health`
  sees what the callbacks left. Two things around it are still divergences,
  and every retail half below is `docs/research/cod11-combat.md` section
  14's, read out of the two functions there:
  - **The victim walk.** VERIFIED: retail walks `trap_EntitiesInBox` over a
    `radius * sqrt(2)` box, and the loop body reads `takedamage` and the
    entity's own bounds (14.1). INFERRED, since the skip is a branch: it takes
    anything with `takedamage` set and measures a brush model to the nearest
    point of those bounds. vcod walks the client entities in the box, each
    tested for `takedamage` on its turn and measured at the script `origin`
    field, then the turrets in the box, which take `G_Damage`'s entity arm
    (14.6). Doors have `takedamage` on retail and no spawn here; no stock MP
    map places one. `func_static` and `trigger_damage` are walked
    (`cod11-combat.md` 14.6, 14.8). It walks in entity
    order, where retail's order is the area tree's (14.7), which depends on
    every link since the map load.
  - **The victim's box and eye are the standing ones.** VERIFIED: retail's
    probe points come off the entity's own bounds and its `client+0xD0` eye
    height (14.3). The host carries no stance, so a crouched or prone player
    is measured as if he stood. That moves the five probe points, not the
    distance, which is origin to origin either way. The second chance's box
    midpoint is taken at the origin of the client's last link, mirrored from
    the sim: its last cmd's truncated origin, or the unsnapped one after a
    `setOrigin` with no cmd since or on a linked client (14.3).
  - **Not a divergence: a fifth argument is ignored.** VERIFIED: retail
    reads its four arguments by index, `Scr_GetVector(0)` and `Scr_GetFloat(1)` to `(3)`
    (14.2). INFERRED, from the absence of any arity check in that body: a
    fifth argument is ignored rather than refused, which is what vcod does
    with one. No call in the shipped corpus passes one.
- **Of the `SP_` layer, only what the wire can see runs.**
  `spawn_entities_from_string` (`crates/server/src/game/spawn.rs`) reproduces
  `G_CallSpawn`'s third case for the five classnames whose `SP_` function is
  an unconditional `G_FreeEntity(self)`, because that half decides entity
  numbering (section 13 of docs/research/cod11-gsc-object-model.md), and the
  spawn-time writes that take a configstring slot: `SP_worldspawn`'s
  `northyaw`, `SP_trigger_hurt`'s sound alias, and the item and two aliases a
  mounted mg42 registers through `SP_turret` (sections 17 and 19 there).
  Everything else those 22 spawn functions do is still absent: no brush model
  is bound, no trigger is linked, no turret entity is built. Those entities
  are live and script-visible
  with every Radiant key applied, which is what the corpus reads them for,
  but their engine-side setup is absent. Nothing measured so far depends on
  it; a script that asks a `func_door` to move is the case that would.
