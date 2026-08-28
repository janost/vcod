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
builtin call (larger than the engine's own 144-entry table, §7, because an
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
(`BinOp::BitAnd`, and `|=` desugars to `BinOp::BitOr` — see the comment in
`crates/gsc/src/ast.rs`), and the compile census agrees with the classifier
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

**Location, VERIFIED.** `nm -D` shows a data symbol `functions` at `0x7e508`
in `.data`, immediately followed (no other symbol between) by a second
symbol `spawns` at `0x7eb30`; `spawns` turns out to be a *different* table —
valid entity classnames for the `spawn()`/map-load path (`func_door`,
`trigger_multiple`, `misc_model`, `info_null`, ... — spot-checked, these are
plainly classnames, not callables), not more builtin functions. So the
builtin-name table lives in the closed range `[0x7e508, 0x7eb30)`, 1576
bytes. `getcvar` sits inside it (`.rodata` offset `0x7870c`, part of a
cluster with `getcvarfloat`/`getcvarint`/`isalive`/`isdefined` right next to
it), matching the design's "near the `getcvar` string."

**Recipe (reproducible in seconds):** walk every 4-byte-aligned dword `v` in
`[0x7e508, 0x7eb30)`. Read the dword at `v`'s value as a candidate pointer;
if it falls inside `.rodata`'s address range (`0x6e260..0x7a394`, read off
`objdump -h`) and the bytes there decode as a printable, NUL-terminated,
identifier-shaped ASCII string, record it. `.data`'s file offset is `vaddr -
0x1000` for this binary (non-PIE, confirmed from `objdump -h`'s VMA/file-offset
columns); `.rodata` and `.text` file offsets equal their VAs.

**Count: 144, VERIFIED — not "roughly 200."** The design doc's "roughly 200"
does not hold up against a direct table walk; my own count, from the
`functions` symbol to where the entries stop looking like callables and
start looking like entity classnames (`info_null`, exactly at the `spawns`
symbol boundary), is 144. Individual records look like `{name_ptr, func_ptr,
flags}` for most entries (e.g. `getcvar` -> `0x5cff4`, flag `0`; `print` ->
flag `1`) but the record stride is not a uniform 12 bytes for the whole
table — a handful of entries (`getent`, `getentarray` among them) carry a
null function pointer in the second field, which is why a strict fixed-stride
walk undercounts; the pointer-scan recipe above sidesteps that by not
assuming a struct shape. I did not fully resolve the struct, which is a gap
worth closing before G2 needs a hard "does the host implement every name"
check — `tools/re/` gains a proper dumper for it there, per the design's own
note.

Representative sample relevant to a deathmatch server (of the 144, not all
listed): `getcvar`, `getcvarint`, `getcvarfloat`, `setcvar`,
`spawn`, `spawnstruct`, `getent`, `getentarray`, `getentbynum`, `isdefined`,
`isalive`, `bullettrace`, `radiusdamage`, `ambientplay`, `setcullfog`,
`setexpfog`, `precachemodel`, `precacheitem`, `precacheshader`,
`precachestring`, `precacheshellshock`, `precachemenu`, `precachestatusicon`,
`precacheheadicon`, `attach`, `detach`, `linkto`, `getorigin`, `playsound`,
`playloopsound`, `delete`, `setmodel`, `print`, `println`, `iprintln`,
`iprintlnbold`, `distance`, `vectordot`, `vectornormalize`,
`vectortoangles`, `anglestoforward`, `randomint`, `randomfloat`,
`obituary`, `logprint`, `map_restart`, `exitlevel`.

**Open question, not settled here:** the design also names `move*`/`rotate*`
mover builtins (`moveto`, `movex`, `movey`, `movez` exist as `.rodata`
strings, VERIFIED). Pointers to them do **not** sit in the `[0x7e508,
0x7eb30)` table; they sit in a different, smaller table embedded directly in
`.rodata` (not `.data`) with a constant zero second field per entry, which
reads like a diagnostic/whitelist list (plausibly "methods valid only on an
`ET_MOVER` entity," used for an error message) rather than a call-dispatch
table. Where the movers are actually dispatched from is unresolved; flagged
for whoever picks up the mover-entity builtins in a later sub-project.

## 8. Atom identity and case folding

The engine interns every script string (`GScr_AllocString`), and matches
identifiers, field names, file paths and event names (`notify`/`waittill`/
`endon`) case-insensitively — `vcod-gsc`'s interner (`crates/gsc/src/atom.rs`)
folds every string to its lowercase form for identity, so two spellings of
one key share one `Atom`, but stores and returns the *first* spelling it
saw, verbatim (`Interner::resolve`), so a script-built display string never
gets silently lowercased. `Value::String`/`Localized`/`Anim` all resolve
through the same table as identifiers, since the corpus gives no reason to
maintain two interners.

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

**Divergence, kept as documentation rather than fixed:** whether gsc string
comparison (`==`) is case-insensitive at all is not established against
retail — no corpus script relies on it either way, the same way no script's
`waittill` and `notify` pairing in this corpus depends on their event-name
spellings matching by case rather than being identical outright. As
implemented, it *is* case-insensitive, but only as an unintended consequence
of identifiers and string content sharing one identity table for a reason
that has nothing to do with `==`: folding is required so a `waittill`
waiting on one spelling of an event name sees a `notify` fired with another.
A future host that needs case-sensitive string equality would need a second,
unfolded table for `Value::String` distinct from the one identifiers use.

**A second, smaller divergence from the same design:** when two spellings
of one key collapse onto one atom, `resolve` always returns whichever
spelling was interned first, so which of the two a script observes (via
`print`, string concatenation, etc.) depends on load order, not on which
spelling that particular call site used. Storing every spelling and
comparing case-insensitively at read time would get this exactly right, at
the cost of giving `Value` a custom `PartialEq` that any future `==` on it
would need to know about — judged not worth it for a collision bounded at
86 keys, none of them display text.

## 9. Semantics not yet established

Each of these was read from scripts or inferred from decompilation, never
observed live. Each is a candidate for a retail A/B (`tools/run_server.sh`
plus a small custom test script logged through `iPrintLn`/`logPrint`,
diffed against vcod's VM on the same script) before G1's semantics tests are
trusted as ground truth rather than a best guess.

- **Is an empty string truthy?** `if (getcvar("scr_allies") != "")` (`dm.gsc`)
  is the corpus's actual idiom for "cvar not set," always spelled as an
  explicit `!= ""` comparison rather than `if (getcvar(...))`. That means the
  corpus itself never settles what `Value::is_truthy` should do with `""` —
  no stock script relies on it either way. Settle it by writing a one-line
  test script that does `if ("") iprintln("truthy"); else
  iprintln("falsy");` and running it on the retail server.
- **Int/float promotion in arithmetic.** Whether `1 / 2` truncates to `0`
  (both operands int) versus promotes, and whether a mixed `int op float`
  always yields float, is nowhere pinned by reading scripts — arithmetic in
  the corpus is uniformly written with float-looking literals or cvar-derived
  floats where precision would matter. Settle by comparing `iprintln(1/2)`
  and `iprintln(3/2.0)` against retail's printed values.
- **Comparison across incompatible types.** What `"5" == 5` or
  `undefined == 0` evaluate to is not exercised anywhere in the corpus in a
  way that pins the rule (every comparison in the stock scripts compares
  same-typed operands, e.g. string-vs-string constants or int-vs-int
  counters). Settle by a small matrix of cross-type `==`/`<` comparisons
  logged against retail.
- **Notify wake order.** When `notify(event, ...)` has multiple threads
  waiting on the same event via `waittill`, the corpus never depends on a
  specific wake order (each `waittill` site in the stock scripts is the only
  waiter on its event within its own thread lineage, as far as a source read
  can show — proving the negative would need a cross-file call-graph, which
  this pass did not build). Settle by spawning two threads that both
  `self waittill("x")` on the same entity, firing one `notify("x")`, and
  observing which runs first on retail.
- **`getentarray` ordering.** Whether it returns entities in spawn order,
  entity-number order, or something else is unverified; scripts that consume
  it (e.g. spawnpoint selection in `_spawnlogic.gsc`) either iterate the
  whole array or pick randomly, never assuming a specific position matters.
  Settle by spawning several `mp_deathmatch_spawn` entities in a known
  input order on a test map and diffing `getentarray`'s returned order
  against retail's.
- **`i%count` is a parse error.** `lex.rs`'s number/operator scan resolves a
  `%` immediately followed by an identifier-start character unconditionally
  as an anim reference (`%count` lexes as `Tok::Anim("count")`), with no
  lookback at what precedes it — so `i % count` (spaced) lexes as modulo,
  but `i%count` (tight) lexes as `Ident("i")` followed by `Anim("count")`
  with no operator between them, which the parser rejects. No stock script
  spells modulo this way (the corpus always spaces it), but this crate's
  stated posture is that it also runs third-party map scripts, where a
  tightly-spelled modulo is plausible. Whether retail's lexer has the same
  ambiguity — it would need the same kind of one-character lookahead choice
  — is unverified.
- **`-2147483648` is unreachable as a literal, but reachable via a cast.**
  No lexed integer literal can spell `i32::MIN`: `read_number` (`lex.rs`)
  parses only the unsigned digit run, and `2147483648` (one past
  `i32::MAX`) overflows `i32::from_str`, so it falls back to `Tok::Float`
  the same way any oversized literal does (see the "sentinel" comment next
  to that fallback). `-2147483648` therefore compiles as `Neg` applied to
  `Float(2147483648.0)`, evaluated through `eval_neg`'s float arm to
  `Float(-2147483648.0)`, exactly representable since `2^31` fits an f32
  mantissa exactly. `(int)` of that goes through `CastInt`'s `f as i32`,
  a saturating cast since Rust 1.45 — and lands exactly on `i32::MIN`,
  since the float value is exact and already in range, not clamped. So the
  value is reachable, just never as a direct `Op::Const(Value::Int(...))`.
  Pinned by `vm::tests::int_min_is_reachable_only_through_a_float_cast_not_a_direct_literal`.

## 10. Divergences kept as documentation, not code

Four places where the implementation made a deliberate call the corpus
cannot settle, recorded here rather than silently baked into behaviour that
looks authoritative:

- **Two `notify`s of the same event on the same target queued within one
  scheduling step coalesce; the second is lost.** `Op::Notify` queues rather
  than resolving inline (`vm.rs`, `step_frames`'s `Notify` arm), and
  `step_thread` flushes the queue only after the whole step
  (`vm.rs:step_thread`) — so if a step queues two notifies of the same event
  at the same target, the first flush wakes the waiter (flips it out of
  `WaitingNotify`), and the second flush finds no matching waiter and drops
  it silently. Measured: a pump firing `self notify("tick", 1); self
  notify("tick", 2);` in one step against a loop doing `self
  waittill("tick", v)` twice receives only `1`, then hangs forever on the
  second `waittill` — one frame apart (each notify in its own step) both
  arrive. Retail runs the waiter synchronously off `notify`, so it sees
  both. This is a direct consequence of vcod's choice that a notify can
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
- **Ordering comparisons against `undefined` read false, where retail is
  believed to error.** `numeric_cmp` (`crates/gsc/src/vm.rs`) returns
  `Value::Int(0)` for `<`/`>`/`<=`/`>=` whenever either operand does not
  convert to a number, `undefined` included — a deliberate choice per the
  implementation brief, not something the corpus exercises either way (same
  gap as "comparison across incompatible types" above), and unverified
  against the retail server.
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
