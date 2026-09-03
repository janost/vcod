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
<Cmd_MenuResponse_f@@Base>` at the `mr` comparison site). I disassembled
`Cmd_MenuResponse_f` itself
(`objdump -dr --start-address=0x486d8 --stop-address=0x487ec`). Everything
in this paragraph is **INFERRED from disassembly, not live-tested**:

- It requires `argc == 4`; anything else sets a default `"bad"` response and
  skips straight to the reply path.
- `argv[1]` is read into a buffer and parsed as a base-10 integer, then
  compared against the return of a call fed the string `sv_serverId`
  (address `0x73f5d`, sitting immediately before the `ClientCommand` string
  block) — almost certainly `Cvar_VariableIntegerValue("sv_serverId")`. A
  mismatch jumps straight to cleanup, so a stale `serverId` silently drops
  the response.
- `argv[2]` is read and parsed the same way, then compared unsigned against
  `0x1f` (31); a value above 31 skips one block (likely a bounds-checked
  table lookup keyed by menu index) but does not abort the command. This is
  the "menu index bounded at 31" the design claims.
- `argv[3]` is read into a third buffer and carried to the end unparsed —
  the response string.
- The tail of the function (after the argv[2]/argv[3] handling) makes two
  more calls carrying the menu-name and response buffers, then an
  unconditional call carrying the client's edict pointer and a small
  constant — plausibly a generic client-event log distinct from the
  menu-response delivery itself; I did not chase this further since it is
  outside what G1/G2 need.

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
It is green on the 24 probes it runs. Five more (`probe_bootstrap`,
`probe_cvar`, `probe_delete`, `probe_ents`, `probe_not_string`) need the
object model, the cvar table or a real map, all of which live in
`crates/server`, so they are measured there by
`crates/server/tests/semantics_ents.rs` against the same capture file. Three
(`probe_game_dotwrite`, `probe_level_bracket`, `probe_level_size`) are
captured but skipped for the reasons `§10` and `semantics_ab.rs`'s
`KNOWN_GAPS_OUT_OF_SCOPE` give.

Four facts about the retail side shaped how the measurement had to be taken,
and each is worth knowing before writing another probe:

- **`logPrint` is the only output channel** a dedicated server with no
  clients shows. `print` and `println` produce nothing on the console even
  with `developer 1`; `iPrintLn` needs a client. `logPrint` writes to
  `games_mp.log` with a leading `m:ss ` stamp.
- **A script runtime error terminates the server**, not just the thread:
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

**Boolean reading: numbers only, VERIFIED.** `if (x)` accepts `Int` and
`Float` and nothing else. `0` and `0.0` are false; `1` and `0.5` are true.
`""`, `"a"`, a vector and an unset field each raise `cannot cast X to bool`
and kill the server. So the design's old question "is an empty string
truthy?" was the wrong question: no string has a boolean reading at all,
empty or not. That is why the corpus spells every such test as `isDefined(x)`
or `x != ""` rather than `if (x)`.

**Unary `!` is not that cast, VERIFIED (`probe_not_string`,
`probe_not_empty_string`).** It reads a string numerically instead of
refusing it: `!"1"` is `0` and `!"0"` is `1`, where the `if` cast takes no
string at all (`probe_truthy`'s `if ("a")` line is
`cannot cast "a" to bool`). The empty string is still fatal under `!`, with
that same message: `!""` dies on `cannot cast "" to bool`. Whether any other
unparseable string is fatal was not measured; see below. This is not a
curiosity. `_teams::restrictPlacedWeapons` guards on
`!getCvar("scr_allow_fg42")`, stock `scr_allow_fg42` is `"0"`, so the guard
is true on a stock server and the map's placed fg42 weapons are deleted;
under the `if` cast that line would have taken the server down at map load.
`Op::Not`'s `not_of` (`crates/gsc/src/vm/interp.rs`) implements the numeric
reading for `Value::String` and `Value::Localized` alike.

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
formatter. So the exponent boundary is untested — vcod's `format_g` switches
to Rust's `1e6` there, and it is the formatter `set_cull_fog` uses, so a fog
distance past six digits would go out in that spelling unchecked. A vector
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

**Notify wake order: start order, VERIFIED.** Two threads waiting on one
event on `level` wake in the order they were started.

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
(`probe_level_bracket`) — retail's compiler statically knows each global's
access mode and rejects the wrong one before any code runs, not just the
line that reaches it. This settles the "still unestablished" entry the
previous task left here: `game` is genuinely array-typed, not a struct that
also answers to brackets, and vcod now models it as one (`Vm::game`,
`ArrayId`, `Op::LoadGame` pushes `Value::Array`). Section 10 covers why
vcod does not reproduce the compile-time half.

Also measured, though out of the scope that motivated the above:
**`level.size` reads a constant `1`, not a field count (`probe_level_size`).**
Set one dot-field on `level` or two, `level.size` reads `1` either way —
unlike `game.size`/an array's `.size`, which do count keys. vcod's
`LoadField` only special-cases `.size` for `Value::Array`/`Value::String`,
so `level.size` in vcod reads `Undefined` and then fails to concatenate.
Not fixed here; see the "Still unestablished" entry below.

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
so the bound is exclusive. `getTime` is non-negative and its units are
**not** measured, only its sign.

**`delete()` defers the free, VERIFIED (`probe_delete`).** The entity stays
in `getEntArray` and in its count immediately after `delete()`, a spawn
right after takes a fresh number rather than the deleted one, and after a
150 ms wait the number is back in circulation. The mechanism and the 100 ms
the engine actually arms are section 14 of
`docs/research/cod11-gsc-object-model.md`; what the probe pins is the bound,
not the constant, because its frames step 50 ms at a time.

### Still unestablished

- **Unary `!` on a string that will not parse.** `!""` is fatal and `!"1"`
  and `!"0"` are numeric (§9), but `!"a"` was never put to a retail server.
  `not_of` (`crates/gsc/src/vm/interp.rs`) treats any unparseable string as
  the same fatal cast `!""` measured, which is the reading that follows from
  the empty case, and nothing on the bootstrap path reaches it: every
  `scr_allow_*` value is `"0"` or `"1"`.
- **What `level.size` actually is.** `probe_level_size` (above) shows retail
  answers `1` regardless of field count, but not why — whether it is a
  generic non-array-non-string `.size` default, an engine-native property
  unrelated to field count, or something else. `level.foo`-style field reads
  are otherwise ordinary (§9's `field_read_*` cases), so this looks specific
  to the name `size`. No probe has isolated the cause.
- **`undefined == undefined`.** Not probed. Retail's error message for the
  mixed case names a "pair has unmatching types", which two `undefined`s are
  not, so equal is the reading that message supports — but that is inference,
  not measurement. vcod answers true on that inference (`values_equal`,
  `crates/gsc/src/vm/interp.rs`); every *mixed* pair with `undefined` errors,
  which is the measured half.
- **String comparison against a localized string** (`&"KEY"`), and whether a
  localized string renders as its key or its resolved text when
  concatenated. No stock script does either.
- **`format_g`'s exponent form.** No probe has driven a float outside
  roughly `1e-4 .. 1e6`, so whether retail's `%g` prints `1e+06` or
  something else is unmeasured; `format_g` (`crates/gsc/src/value.rs`)
  spells it Rust's way (`1e6`) and, being unmeasured either way,
  `format_g(999999.5)` currently rounds to `"1000000"` rather than
  switching to exponent form.
- **Equality outside the pairs above.** `values_equal`'s catch-all
  (`crates/gsc/src/vm/interp.rs`) is `a == b` on `Value`'s derived
  `PartialEq`, so a genuinely mixed pair (`vector == "a"`, `entity == "a"`)
  answers `false`, but two vectors, entities, arrays or function pointers
  compare by value and can answer **true**. Retail's "pair has unmatching
  types" message suggests the mixed case is fatal there; the same-type cases
  are plausible but unmeasured. No probe has driven either.

## 10. Divergences kept as documentation, not code

Eleven places where the implementation made a deliberate call the corpus
cannot settle, recorded here rather than silently baked into behaviour that
looks authoritative. A twelfth is gone: `delete()` used to free the entity
on the spot where retail defers it, and the entity think scheduler stage 3
added closes that (section 14 of
`docs/research/cod11-gsc-object-model.md`, and `probe_delete` in §9).

- **Two `notify`s of the same event on the same target queued within one
  scheduling step coalesce; the second is lost.** `Op::Notify` queues rather
  than resolving inline (`vm/interp.rs`, `step_frames`'s `Notify` arm), and
  `step_thread` flushes the queue only after the whole step
  (`vm/sched.rs:step_thread`) — so if a step queues two notifies of the same
  event at the same target, the first flush wakes the waiter (flips it out of
  `WaitingNotify`), and the second flush finds no matching waiter and drops
  it silently. Measured: a pump firing `self notify("tick", 1); self
  notify("tick", 2);` in one step against a loop doing `self
  waittill("tick", v)` twice receives only `1`, then hangs forever on the
  second `waittill` — one frame apart (each notify in its own step) both
  arrive. Retail runs the waiter synchronously off `notify`, so it sees both
  (INFERRED from the engine's dispatch shape; the double-notify case has not
  been put to a retail server). This is a direct consequence of vcod's choice that a notify can
  never reenter the VM (`step_thread`'s own doc comment); fixing it without
  reopening reentrancy would mean re-checking, at flush time, which of a
  step's queued notifies still have a live waiter, in queue order, deferred
  to the next project. Pinned by
  `sched::tests::two_notifies_of_the_same_event_in_one_step_coalesce_and_the_second_is_lost`.
- **A thread killed by its own `endon` mid-step keeps executing to its next
  suspend.** `main`'s `self thread killer();` runs `killer` synchronously
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
- **`game.foo = 1` and `level["k"] = "v"` compile in vcod; retail rejects
  each at compile time.** Measured (`probe_game_dotwrite`,
  `probe_level_bracket`): retail's compiler statically knows `game` is
  array-typed and `level` is a struct, so the wrong access mode on either
  bare identifier is a `script compile error` that voids the whole script
  before `main()` runs, not a fault at the line that reaches it. vcod's
  compiler (`crates/gsc/src/compile.rs`) does no such static typing of
  `level`/`game`; both constructs compile and only fail once the
  instruction loop reaches them (`Op::StoreField`/`Op::StoreIndex`
  rejecting an array/struct operand), by which point any `logPrint` before
  the failing line has already run — a real, observable divergence from
  retail's all-or-nothing compile failure, not just message-text. Adding
  static typing for these two identifiers to the compiler would fix it;
  out of scope for the task that made `game` array-typed, since neither
  construct appears anywhere in the corpus (`game.` and `level["` are both
  zero-hit). `crates/gsc/tests/semantics_ab.rs`'s
  `KNOWN_GAPS_OUT_OF_SCOPE` skips both probes rather than asserting a false
  pass.
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
- **Every endon kill lands before any notify wake.** `Vm::notify`
  (`crates/gsc/src/vm/sched.rs`) makes two passes over `threads` — kills
  first, then wakes — so a thread that has both an `endon` and a `waittill`
  on one event is killed rather than woken, regardless of which it
  registered first. The wake order *within* each pass is measured (start
  order, `# probe_notify`); the ordering *between* the two passes is not.
- **`radiusDamage`'s falloff curve is RTCW's, not retail's.** The callback
  itself is no longer a divergence: `radius_damage`
  (`crates/server/src/game/builtins/combat.rs`) hands
  `CodeCallback_PlayerDamage` to `Cx::spawn`, which the interpreter starts as
  soon as the builtin returns and before the calling thread's next
  instruction, so a script that damages and then reads `self.health` sees
  what the callback left, the way retail's synchronous call does. What each
  victim takes is the open half: the damage falls off linearly from
  `maxDamage` at the blast to `minDamage` at the radius, which is RTCW's
  `G_RadiusDamage`. INFERRED — the curve at `.so` 0x5eef4 was not read.
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
- **A script entity handle carries no generation, so a stale one silently
  aliases whatever spawns into the freed slot next.** Retail bumps a
  per-entity generation counter at `gentity+0x300` on every free and checks
  a script handle against it, which is what a stale handle is caught by
  (section 14 of docs/research/cod11-gsc-object-model.md, `G_FreeEntity`
  0x66948). `Value::Entity(EntId)` (`crates/gsc/src/value.rs:7`) is a bare
  entity number with no such tag, and now that `ObjectTable`'s free list
  (`crates/server/src/game/entity.rs`) hands a freed slot straight back out
  to the next spawn, a script holding a handle to the deleted entity reads
  the new occupant's fields under the old handle instead of hitting an
  error. Before slot reuse landed, the same stale handle pointed at a dead
  slot and failed cleanly instead; this branch made an existing gap
  reachable rather than opening a new one. Not reachable by any script in
  the corpus today, since nothing in it holds an entity handle across a
  delete and a respawn, but a real hazard for a third-party script that
  does. Closing it needs a generation counter stored beside the slot and
  checked on every handle dereference, not a fix to the free list itself.
