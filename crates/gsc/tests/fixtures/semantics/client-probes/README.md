# Probes that need a connected client

Everything in the directory above measures from a gametype's `main`, which
`tools/run_probe.sh` can drive on its own: it boots the retail server, waits,
and reads `games_mp.log`. `tools/capture_probes.sh` globs
`../probe_*.gsc`, and `semantics_ab.rs` pairs every `.gsc` beside it against a
`retail-captures.txt` section, so a probe that logs nothing without a client
would sit in that set as an empty section indistinguishable from a broken one.

These probes measure from `Callback_PlayerConnect` instead, so they are here,
outside both. They are run by hand and their output is quoted in whatever
research doc the measurement belongs to, rather than diffed by a test.

## Running one

Two shells. The first boots the retail server with the probe installed as a
loose gametype (the `.txt` is the description file the engine refuses to load
a map without), on a port clear of `run_server.sh`'s 28960 and
`run_probe.sh`'s 28970:

```
cp client-probes/probe_pers.gsc "$COD_LNXDED_HOME/main/maps/mp/gametypes/"
printf '"PROBE_PERS"\r\n' > "$COD_LNXDED_HOME/main/maps/mp/gametypes/probe_pers.txt"
rm -f "$COD_LNXDED_HOME/main/games_mp.log"
private/reference/cod-lnxded-1.1d/cod_lnxded \
    +set dedicated 1 +set developer 1 +set logfile 2 \
    +set g_log games_mp.log +set g_logSync 1 \
    +set fs_basepath "$COD_DIR" +set fs_homepath "$COD_LNXDED_HOME" \
    +set net_port 28971 +set sv_maxclients 8 +set sv_pure 0 \
    +set g_gametype probe_pers +map mp_pavlov
```

The second connects a client, which is what makes the callback run:

```
cargo run -p vcod -- --net-probe 127.0.0.1:28971 --probe-secs 12
```

Then read `$COD_LNXDED_HOME/main/games_mp.log` for the `PROBE ` lines and the
server's console for a `script runtime error` block, exactly as
`run_probe.sh` does. Delete the loose `probe_*` files from the homepath
afterwards: the engine keeps only the first 31 loose gametype scripts and
silently falls back to `dm` past that.

`COD_LNXDED_HOME` must be an absolute path with no `+` in it — the engine
splits its own command line on `+`.

## probe_pers

Run 2026-08-31 against the retail 1.1d dedicated server, `dm`-shaped probe
gametype on mp_pavlov, one client. `games_mp.log`:

```
  0:00 PROBE at main
  0:00 PROBE at startgametype
  0:05 PROBE at connect_before_begin
  0:05 PROBE pers_before_begin defined
  0:05 PROBE at connect_after_begin
  0:05 PROBE pers_after_begin defined
  0:05 PROBE at read_pers_key
  0:05 PROBE pers_team undefined
  0:05 PROBE at write_pers_key
  0:05 PROBE pers_team_written spectator
  0:05 PROBE at read_name
  0:05 PROBE name vcod
  0:05 PROBE at read_undef_field_index
```

and the console, on the line after the last one logged:

```
******* script runtime error *******
undefined is not an array, string, or vector: (file 'maps\mp\gametypes\probe_pers.gsc', line 48)
 if(isdefined(self.nosuchfield["team"]))
```

The line number is from the copy that ran, which had no header comment; the
committed file carries one, so a rerun names a later line.

Three measurements out of it, all used in
`docs/research/cod11-gsc-object-model.md`: `.pers` is an indexable object
before `begin` and holds nothing, `.name` already carries the client's
userinfo name, and reading an index off a genuinely undefined field is fatal.
