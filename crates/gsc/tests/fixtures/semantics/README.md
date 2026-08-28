# Semantics A/B fixtures

`probe_*.gsc` are gametype scripts run on the retail 1.1d Linux dedicated
server. `retail-captures.txt` is what retail printed, one `# <probe>` section
per file. `semantics_ab.rs` runs the same files through `vcod-gsc` and diffs.
When the two disagree, retail is right.

Re-capture with:

```
COD_DIR=... tools/capture_probes.sh > crates/gsc/tests/fixtures/semantics/retail-captures.txt
```

Three things about the retail side shape these files, all learned the hard
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

Probes emit `PROBE at <name>` before an expression that might be fatal, so a
run that dies names what killed it.

`probe_ents` measures `getentarray`'s return order and needs real map
entities, so it is captured but not run through `vcod-gsc`: there is no
entity object model yet. Its recorded order — the map's own entities first,
then spawned ones in spawn order — is what that implementation has to
reproduce.
