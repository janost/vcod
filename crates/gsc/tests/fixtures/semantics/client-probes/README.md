# Probes that need a connected client

Everything in the directory above measures from a gametype's `main`, which
`tools/run_probe.sh` can drive on its own: it boots the retail server, waits,
and reads `games_mp.log`. `tools/capture_probes.sh` globs
`../probe_*.gsc`, and `semantics_ab.rs` pairs every `.gsc` beside it against a
`retail-captures.txt` section, so a probe that logs nothing without a client
would sit in that set as an empty section indistinguishable from a broken one.

These probes need a client for their own reason -- one measures from
`Callback_PlayerConnect`, one from a notify only a walking player raises, and
one has a half that is only visible on the wire -- so they are here, outside
both. Their output is quoted in whatever
research doc the measurement belongs to, or committed as a fixture a test in
`crates/server` reads; neither is paired against `retail-captures.txt`.

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

## probe_trigger

Every `"trigger"` notify the engine's touch pass raises. It threads a
`waittill("trigger", other)` onto every trigger entity of the six trigger
classnames and logs one line per notify with the trigger's entity number, its
classname, the toucher's entity number and the toucher's origin; a `PROBE
watch` line per entity ahead of them is the census the map ended up with after
the gametype's `_gameobjects` pass deleted what it did not claim.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. The scan waits a second first: the entity lump is
loaded before the gametype's `main()` runs but the map's own `main()` is not,
so a scan without the wait would watch a trigger the map script goes on to
delete or to move.

`tools/run_probe.sh` drives it under its subdirectory name, so both shells are

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=400 \
    tools/run_probe.sh client-probes/probe_trigger mp_pavlov
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-triggers \
    --probe-team allies --probe-secs 380
```

The output is committed as `crates/server/tests/fixtures/triggers/`'s capture
and diffed by `crates/server/tests/triggers_ab.rs`, which runs this same file
as our gametype so both sides log through the same `logPrint`.

## probe_mover

The ten scriptent mover verbs. Two halves of one run, which is why it is here
rather than beside the other probes: the script logs `getorigin()` and
`.angles` once per server frame through each verb, which settles the units and
the motion law, and a `--net-probe` attached to the same server reads the
`pos`/`apos` trajectory groups, which is the only way to see whether retail
ships a mover as per-frame origins or as a trajectory. It needs no client for
its own sake; it spawns one bobbing `script_model` over each player that
connects, because mp_pavlov's two surviving map `script_model`s sit in one
corner and a lone client spawns anywhere.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_mover mp_pavlov
# 18 s later, in the second shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 45
```

Both halves are committed under `crates/server/tests/fixtures/movers/` and
written up in `docs/research/cod11-movers.md`.

One thing it learned the expensive way: a mover verb on anything but a
`script_brushmodel`, `script_model` or `script_origin` is a fatal script
runtime error, so a first version that moved the map's placed weapons for
PVS coverage died on its first frame.
