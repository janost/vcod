# Semantics A/B fixtures

`probe_*.gsc` are gametype scripts run on the retail 1.1d Linux dedicated
server. `retail-captures.txt` is what retail printed, one `# <probe>` section
per file. `semantics_ab.rs` runs the same files through `vcod-gsc` and diffs.
When the two disagree, retail is right.

Re-capture with:

```
COD_DIR=... tools/capture_probes.sh > crates/gsc/tests/fixtures/semantics/retail-captures.txt
```

Four things about the retail side shape these files, all learned the hard
way:

- **`logPrint` is the only output channel.** A dedicated server with no
  clients shows nothing from `print` or `println`, even with `developer 1`;
  `iPrintLn` needs a client to send to. `logPrint` goes to `games_mp.log`,
  which is what `tools/run_probe.sh` reads, stripping the engine's leading
  `m:ss` stamp.
- **A script runtime error kills the whole server.** That is why each group
  is its own file: one fatal expression would cost every measurement after
  it. A probe that dies is still a measurement, recorded as
  `PROBE_FATAL <message>`.
- **A gametype needs a one-line `.txt` description file** beside the `.gsc`
  or the engine refuses to load the map. `run_probe.sh` writes one.
- **A script *compile* error is not a script *runtime* error, and it is
  worse: nothing before it in the same file runs either.** `game.foo = 1`
  and `level["k"] = "v"` both die with a `******* script compile error
  *******` block, not the runtime-error block above, because retail's
  compiler statically knows `level`/`game`'s access mode and rejects the
  wrong one before `main()` starts. `run_probe.sh` only greps the console
  for `script runtime error`, so a compile error shows up as an empty
  capture section with no `PROBE_FATAL` line -- indistinguishable, without
  reading the raw console, from a probe that simply logged nothing. Keep a
  construct that might be a *compile*-time rejection alone in its own file,
  same as a fatal runtime one, or it costs every measurement in the file,
  not just the ones after it (`probe_game_dotwrite.gsc`,
  `probe_level_bracket.gsc`, both skipped in `semantics_ab.rs`'s
  `KNOWN_GAPS_OUT_OF_SCOPE` since vcod's compiler has no equivalent
  static check to reproduce the empty-capture result).

Probes emit `PROBE at <name>` before an expression that might be fatal, so a
run that dies names what killed it.

`probe_ents` measures `getentarray`'s return order and needs real map
entities, so it does not run here. It runs in `crates/server`, against the
object model and a real `mp_pavlov` load, and diffs this same file.
`every_capture_section_has_a_runner` keeps the split honest: a capture
section belongs to exactly one of the two runners.

Its order is settled: ascending entity number, which is BSP entity lump
order. `mp_pavlov`'s four `script_origin` blocks sit at lump indices 2, 3, 4
and 344 as auto5, auto4, auto3, auto6, exactly what retail returned
(`docs/research/cod11-gsc-object-model.md` section 10).
