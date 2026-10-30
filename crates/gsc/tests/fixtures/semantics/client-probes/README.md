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

## probe_lookat

The `trigger_lookat` half of S&D: every `"trigger"` notify the aim trace
raises, and every player `isLookingAt` answers true for, polled once a server
frame. It threads both onto each `trigger_lookat` the map spawned, logs a
`PROBE watch` census line per entity and a `PROBE lookats` count, and then one
`PROBE fire` per notify and one `PROBE looking` per frame a player is aimed at
one. Each carries `getTime()`, which is what pairs a line with the `serverTime`
on a client probe's own `!trace`.

It calls `maps\mp\gametypes\sd::main()` itself, so the bombzones, the bomb and
the lookat triggers are the stock gametype's. The scan waits a second first,
for the same reason `probe_trigger` does.

Three shells, the gsc probe first, then the defender, then the attacker:

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=420 \
    tools/run_probe.sh client-probes/probe_lookat mp_carentan +set probe_teleport 1
# second and third shells, the defender first:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-defuse --probe-secs 400
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-plant --probe-secs 380
```

`probe_teleport 1` is why the walk arrives at all. mp_carentan's allied S&D
spawns sit at y around -1064 and bombzone_A at (-146, 2490), some 3500 units
through the town, and a first retail run spent its whole 380 s oscillating
around (300, 1200) without ever reaching the zone. Under the cvar the probe
puts each player once per level on a teamdeathmatch spawn in the courtyard,
416 units from the zone for the attacker and 541 for the defender, so the walk
is a courtyard crossing. The same cvar also moves every
defender to 20 units from the charge, toward where the planter stood, once the plant has spawned
`level.bombmodel`, because the flak88 and the cart ring the zone and a walk
that has to round them reached the charge 27 s after its 60 s fuse had blown
it, and sends the planter back to the attackers' spawn at the same moment,
since a planter left standing on the defender-to-bomb line is what the
lookat's body trace stops at and no station fires. A run without the cvar walks from the stock spawns and, on mp_carentan,
does not get there. Anything after the map name is passed
to the engine verbatim, which takes its `+set` arguments in any order. Only
mp_carentan has the two origins; on any other map the spawn thread logs `PROBE
teleport unsupported <map>` once and does nothing, and the plant-time thread
returns without a line. `crates/server/tests/sd_plant_ab.rs` sets the cvar
too, for the post-plant planter teleport; it stands its clients where it
wants them itself after the spawn teleport and the defender's landing.

What the three halves measure between them: how often a lookat fires while a
player is aimed at it (the server's log says every frame, with no wait gate in
`G_Trigger`), what `isLookingAt` answers on the frames around a fire, the
`pm_type` a planting player sits at while retail has it linked to the bombzone
and what its velocity does under a forward cmd, the objective slots the plant
and the defuse write and delete, and the HUD element the progress bar rides
with its `scaleStartTime` / `scaleTime` / `fromWidth` / `fromHeight` tween
fields.

The server's log goes to
`crates/server/tests/fixtures/triggers/<map>-sd-lookat.txt`; the two client
halves write `crates/server/tests/fixtures/playerstate/<map>-sd-plant-attacker.txt`
and `<map>-sd-defuse-defender.txt`. All three are retail evidence, and a run
against `vcod-server` overwrites the two client ones: move them to `tmp/` and
`git checkout` the fixture directory after.

## probe_pickup

Every `"touch"` and `"trigger"` notify an item entity or a player takes, for
the A/B in `crates/server/tests/pickup_ab.rs`. It logs one `PROBE item` line
per watched entity at first sight (its number, classname, `getTime()` and
origin), one `PROBE other` line for any other non-player entity a scan first
sees after the startup census (so a dropped weapon whose classname is not in
the watched list still shows up), one `PROBE touch` per item's own `"touch"`
notify and one `PROBE ptouch` per player's, one `PROBE trigger` per item's
`"trigger"` notify with the toucher and, if the pickup swapped out a held
weapon, that weapon's number and classname (`undefined` otherwise), and one
`PROBE teleport` per `probe_teleport` move.

The watched classnames are mp_carentan's two placed weapon kinds
(`mpweapon_fg42`, `mpweapon_panzerfaust`, `docs/research/cod11-items.md`
§4.1's stock numbers), the allied loadout a weapon swap drops
(`mpweapon_m1carbine`, `mpweapon_colt`, `mpweapon_fraggrenade`) and the
health `dm` drops on a death (`item_health`); an unlisted classname is a
`PROBE other` line instead. The census's `PROBE item` lines are what
measured the two fg42s at entities 252 and 258
(`docs/research/cod11-items.md` §12.1).

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. mp_carentan's two fg42s sit a town apart, so
under `probe_teleport 1` each live player is put on the first one on its
first spawn and, once that fg42's own `"trigger"` notify has fired (i.e. it
has been taken), on the second one two seconds later; both moves are onto
the item's own `origin`, not a nearby spot, so Task 3's `--save-pickup`
finds "on the first fg42" by the stepped snapshot's position (within 48
units xy of the fg42's origin, or a jump, ledger ruling R1) without having
to see the teleport as an origin jump. A run without the cvar never
teleports and never logs a `PROBE teleport` line, since an unset cvar reads
`""`. `crates/server/tests/pickup_ab.rs` sets it, so ours teleports the way
retail did.

```
COD_LNXDED_HOME=<absolute, no '+'> SECS=150 \
    tools/run_probe.sh client-probes/probe_pickup mp_carentan \
        +set probe_teleport 1 +set scr_allow_fg42 1
# about 15 s later, in the second shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-pickup --probe-secs 120
```

`scr_allow_fg42 1` is needed because stock `default_mp.cfg` sets it 0, and
`_teams::restrictPlacedWeapons` then deletes both fg42s at map load.

The `PROBE` lines plus the server's own `Weapon:` and `Item:` lines (read
straight out of `games_mp.log`, which `run_probe.sh` does not print) go to
`crates/server/tests/fixtures/items/mp_carentan-dm-pickup-script.txt`; the
client half writes `mp_carentan-dm-pickup.txt`. Both are retail evidence,
and a run against `vcod-server` overwrites the client one: move it to
`tmp/` and `git checkout` the fixture directory after.

## probe_turret

Mounted MG's server half. It logs every `misc_mg42` once (`PROBE turret
<num> <origin> <angles>`), and under `probe_teleport 1` puts each spawning
player at a fixed spot around the gun nearest (1712 1830 8), mp_carentan's
one MG: an allied player 40 units behind it facing along its yaw, an axis
player 300 units in front facing back down it, both inside the gun's
128-unit activate range and its ±45 degree arc. It logs each move as `PROBE
place <clientnum> <team> <origin> <yaw>`. The hits themselves never need a
script line: the engine's own `D;`/`K;` records in `games_mp.log` carry the
weapon and MOD of every one, the same evidence the hit-capture probes read.

`watch_delete`, gated on `probe_delete_after N` (retail runs leave it
unset), deletes the same gun N seconds in; only the A/B rig sets the cvar,
for the mount's release-on-delete case.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1
# second shell, about 15 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team axis --probe-secs 180
# third shell:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-turret --probe-secs 170
```

The server's `PROBE` lines and `games_mp.log`'s `D;`/`K;` records are the
retail evidence a later capture reads; this probe itself writes no fixture.

On United Offensive the same placement feeds `predict_ab`'s turret
capture: one allied `--save-slope` client mounts, sweeps, fires and lets go
(`docs/research/coduo-movement.md` 15):

```
COD_VERSION=uo COD_DIR=<UO install> COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --game-dir <UO install> --mod-dir uo \
    --save-slope --slope-every-state --probe-team allies --probe-weapon m1carbine_mp \
    --probe-uoturret 229 --probe-cmd-ms 8 --capture-tag uoturret-uo --probe-secs 60
```

`probe_pose 1` is the pose measurement of `docs/research/cod11-combat.md`
16.2: every hit logs `PROBE hit <time> <clientnum> <sHitLoc> <iDamage>
<point>` and none lands. The target flips its own view pitch and logs the
snapshot each flip reached; the gunner holds the trigger at it for 8 s. The
connect order picks the slots: gunner first puts the target in slot 1,
target first puts it in slot 0. A tagged turret fixture lands in
`crates/server/tests/fixtures/turret/`; move it out after the run.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=120 \
    tools/run_probe.sh client-probes/probe_turret mp_carentan +set probe_teleport 1 +set probe_pose 1
# 15 s later, the two in the order that picks the slots, 3 s apart:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-turret --probe-turret-target-ms 8000 --capture-tag pose --probe-secs 90
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team axis --probe-pitch-flip 85 --probe-secs 95
```

Pair each `PITCH serverTime=<t>` line of the target with the `PROBE hit`
lines at `t - 50`, `t` and `t + 50`. Retail, 2026-10-08, one change each way
(hit x of the frames k-1 to k+3, `k0` the frame at `t`):

```
target slot 1:  0 -> 85  1518.34 1518.34 1515.40 1512.97 1512.08
target slot 0:  0 -> 85  1518.11 1515.98 1513.45 1512.48 1512.03
```

`probe_respawn 1` (with `probe_teleport 1`) spawns the allied gunner as a
player 40 units behind his placement a second after he is on the gun, and
logs `PROBE seat` / `PROBE seatnext <time> <num> <origin> <angles>` straight
after the `spawn` and a frame later. `probe_vehicle`'s `probe_respawn N`
does the same for a seated player N seconds after the seat, 200 units off
the vehicle's starting left. Recipes in the headers of
`crates/server/tests/fixtures/seatspawn/`. Its `probe_suicide S` kills a
seated player with `suicide()` S seconds after his first seat and logs
`PROBE dead` lines for 3 s; `mp_peaks-tdm-slope-8ms-jeep-die-uo.txt`'s
header has the recipe (`coduo-vehicles.md` 11.7).

## probe_prone

The prone slope capture's server half. Under `probe_teleport 1` it puts each
spawning allied player on a mp_carentan grade once per spawn, and logs
`PROBE place <time> <clientnum> <origin> <yaw>`: `probe_spot street` is the
4-degree street at (900 1930), `probe_spot mound` the 19-degree terrain mound
at (-224 60). The spots were picked with a throwaway ground-normal scan over
our collision world; on any other map the thread logs `PROBE teleport
unsupported <map>`. The client holds its own heading, so the yaw the gsc sets
does not matter to the capture.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=118 \
    tools/run_probe.sh client-probes/probe_prone mp_carentan +set probe_teleport 1 +set probe_spot street
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-slope --probe-prone 90 \
    --probe-cmd-ms 8 --capture-tag prone-street --probe-secs 105
```

The mound run is the same with `probe_spot mound`, `--probe-prone 270` and
`--capture-tag prone-mound`. It writes
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-prone-<spot>.txt`,
named `dm` because retail runs the probe as gametype `probe_prone`.

## probe_speedgrav

probe_prone's street placement (always on, no `probe_teleport`) with the
speed and gravity changed: `probe_gravity N` is `setCvar("g_gravity", N)`
a frame after the load (`SP_worldspawn` writes 800 over any `+set`),
`probe_maxspeed N` sets each placed allied player's `maxspeed` (1.5 and UO;
1.1 reads `g_speed`, set with `+set`). It logs `PROBE gravity <g_gravity>
speed <g_speed>` and probe_prone's `PROBE place` line.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=110 \
    tools/run_probe.sh client-probes/probe_speedgrav mp_carentan +set g_speed 250 +set probe_gravity 400
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-slope --probe-moves \
    --probe-cmd-ms 8 --capture-tag speedgrav-1.1 --probe-secs 95
```

On 1.5b and UO it is `COD_VERSION=1.5` / `COD_VERSION=uo` with `+set
probe_maxspeed 250 +set probe_gravity 400` and the tag `speedgrav-1.5` /
`speedgrav-uo` (UO's probe also `--mod-dir uo`). It writes
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-speedgrav-<v>.txt`.

## probe_pronebones

A still prone body's bones. Each live player is put once per spawn on the
spot `probe_spot` names (`mound`: mp_carentan's 19-degree mound at (-224 60),
yaw 270; `flat`: probe_scriptmodel's carentan prone target, yaw 225;
`brecourt`: its mp_brecourt one, yaw 270). His origin is logged every frame
for 3 s as `PROBE pos`, then a `script_origin` linked to each of eight bones
for 2 s as `PROBE body` and `PROBE bone <time> <client> <tag> <origin>`, and
`PROBE done`. The bones are linked only after the wait: linking before the
spawn has set the body's model is a script error on retail. `probe_hold 1`
puts him back every frame he is more than half a unit off the spot, z
included, the way probe_scriptmodel's prone part used to.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=25 \
    tools/run_probe.sh client-probes/probe_pronebones mp_carentan +set probe_spot mound +set probe_hold 0
# second shell, about 6 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team axis --probe-fall \
    --probe-fall-prone 270 --probe-secs 15
```

The fixtures are `crates/server/tests/fixtures/pronebones/<map>-<spot>.txt`,
gated by `crates/server/tests/pronepose_ab.rs`. Against ours:
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_pronebones.gsc --set
probe_spot=mound`.

## probe_specspawn

A spectator's spawn think. From 3 s in, every 2 s, each spectator is
`spawn`ed again as a spectator at (-224 60 200) over mp_carentan's mound,
facing 0, and his origin is logged straight after the builtin returns
(`PROBE spec`) and a frame later (`PROBE next`). The client half is
`--probe-edge spec-fly`, which holds forward from the join on and prints
every snapshot's origin as `EDGE origin`.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=18 \
    tools/run_probe.sh client-probes/probe_specspawn mp_carentan
# second shell, about 4 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge spec-fly --probe-secs 10
```

The fixtures are `crates/server/tests/fixtures/specspawn/mp_carentan*.txt`,
the `PROBE` lines and the probe's `EDGE origin` lines on the spawn frames,
gated by `crates/server/tests/specspawn_ab.rs`.

## probe_pronedrop

The airborne prone refusal's server half. Under `probe_teleport 1` it lifts
each spawned allied player 100 units over the mp_carentan street, facing
north, four times 6 s apart: at (900 1680), where the wall to the south
leaves the body no room, and at (900 1930), in the open. It logs `PROBE lift
<time> <origin>`. The client half is `--probe-fall` with `--probe-fall-prone
90`, which holds prone at world yaw 90 on every cmd and prints the `FALL`
line per airborne snapshot with `pm_flags` and the event ring. Writes no
fixture; `docs/research/cod11-mantle.md`, "The forced-stance events", has
the readings.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=70 \
    tools/run_probe.sh client-probes/probe_pronedrop mp_carentan +set probe_teleport 1
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team allies --probe-fall \
    --probe-fall-prone 90 --probe-secs 50
```

Against ours: `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_pronedrop.gsc --set
probe_teleport=1`.

## probe_rise

The stance refusals' server half, on 1.1d, 1.5b and UO, any map it knows
(mp_carentan, mp_ship). It drives one `--probe-uo-weapon direct` client through `PROBE_IN`
lines: on mp_carentan it lies him down in the street, moves him prone under
two attic ceilings (one a crouch does not fit, one a stand does not) and
tries each rise with and without `wbuttons` 0x2, then kills him twice with
the trigger held, through dm's killed callback and through one that keeps
his weapon; on mp_ship it hangs him on the hold ladder and holds each stance
key. It logs `PROBE phase <time> <name>`, `PROBE place`, `PROBE killed` and
`PROBE done`. The client's `--save-slope --slope-every-state` capture, with
the `PROBE` lines appended, is the fixture `crates/server/tests/rise_ab.rs`
replays; `docs/research/cod11-mantle.md`, "The forced-stance events", and
`cod11-combat.md` 16.5 have the readings.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=55 \
    tools/run_probe.sh client-probes/probe_rise mp_carentan
# second shell, about 6 s later (mp_ship: --probe-weapon enfield_mp, --probe-secs 25):
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team allies \
    --probe-weapon m1carbine_mp --probe-uo-weapon direct --save-slope \
    --slope-every-state --capture-tag rise-1.1 --probe-cmd-ms 50 --probe-secs 45
```

Then append the server's `PROBE` lines to the fixture. The 1.5b and UO
captures run the same with `COD_VERSION=1.5` or `uo` on the server, the
client given that install (`--game-dir`, plus `--mod-dir uo` for UO) and
`--capture-tag rise-1.5` or `rise-uo`.

## probe_freeze

`freezeControls`' server half. It calls `freezecontrols(true)` on each
player on its first frame alive and logs `PROBE freeze <time> <name>`. The
client half is `--save-motion`, which holds the lean, stance, run and jump
poses in turn and prints a `MOTION` line per pose with `pm_flags` and
`leanf`. Its fixture lands under the gametype's name
(`mp_carentan-probe_freeze-motion.txt`) and is not evidence anyone keeps;
delete it after the run. `docs/research/cod11-mantle.md`, "`pm_flags`
0x4000", has the readings: retail freezes nothing.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_freeze mp_carentan
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --save-motion
```

## probe_quake

`earthquake`'s wire fields, for `docs/research/cod11-combat.md` 17.2. Every
player alive gets `earthquake(0.3, 2.5, origin, 850)` at its own origin
every 2 s and `earthquake(0.05, 0.0026, origin, 100)` a frame later, logged
as `PROBE quake`. A plain `--net-probe` prints an `EV_EARTHQUAKE` line per
event with the temp entity's `angles2` and `time`.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=35 \
    tools/run_probe.sh client-probes/probe_quake mp_carentan
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team allies --probe-secs 20
```

Retail, 2026-10-10: `angles2 [0.3, 850.0, 0.0] time 2500` and `angles2
[0.05, 100.0, 0.0] time 3`, the origin truncated. `vcod-server mp_carentan
--gametype-script .../probe_quake.gsc` reads the same.

## probe_fall

The landing stun and fall damage measurement's server half. Under
`probe_teleport 1` it drops each spawned allied player onto the mp_carentan
street at (900 1930) from 100, 300, 340, 420, 340 again at `maxhealth` 200,
and 520 units above it, 8 s apart, with `self.health` reset to the max
before each, and logs `PROBE drop <time> <height> <origin> maxhealth <n>`
and, 4 s on, `PROBE after <time> <height> health <health> <origin>`. It
wraps `level.callbackPlayerDamage` and `level.callbackPlayerKilled` to log
`PROBE damage`, `PROBE damaged` and `PROBE killed` lines with every argument
the engine handed them. The client half prints a `FALL` line per snapshot
whose ground entity, `pm_flags`, `pm_time`, event ring or health moved, and
every airborne snapshot, and a `CMDS` line per 60 cmds with the `serverTime`
of each. Neither half writes a file; each run's lines were pasted by hand
into a fixture in `crates/server/tests/fixtures/playerstate/`, the server's
as comments: `mp_carentan-dm-fall.txt` (the first run, five drops, no
callback or `CMDS` lines), `mp_carentan-dm-fall-damage.txt` and
`mp_carentan-dm-fall-damage-cvars.txt` (2026-10-06, the second with `+set
bg_fallDamageMinHeight 200 +set bg_fallDamageMaxHeight 1000` after the
`probe_teleport` set). `fall_ab` replays the `CMDS` timeline, so a new
capture of either needs those lines. `mp_carentan-dm-fall-walk.txt`
(2026-10-06) is the stock-bounds run with `--probe-fall-walk 315` on the
client, which holds forward at that world yaw, so each stun walks the player
into the street's south wall; its `CMDS` lines carry each cmd's yaw word. `docs/research/cod11-player-clip.md` 8.9 and 8.10 read
them, and `crates/server/tests/fall_ab.rs` gates the last two.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=90 \
    tools/run_probe.sh client-probes/probe_fall mp_carentan +set probe_teleport 1
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-fall --probe-secs 78
```

## probe_ladder

The ladder measurements' server half. Under `probe_teleport 1` it
sets each spawned allied player at the foot of mp_ship's hold ladder
(4332 -94 57) facing the rungs at yaw 90, or at `probe_spot`'s, once per spawn, and logs `PROBE
place <time> <entnum> <origin>`. The client half, `--probe-ladder 90`,
builds every cmd through the play client's own `PlayInput`, so the weapon
byte is the one the play client sends; it holds forward for 14 s from the
settled join, presses weapon slot 1 3 s later and `weapnext` 3 s after
that, and prints a `LADDER` line per snapshot whose weapon fields,
`pm_flags`, sent byte or 64-unit height band moved. Without
`--save-ladder` neither half writes a file; `docs/research/cod11-combat.md` 9.7 reads the 2026-10-12 runs before
and after the play client's fix.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=60 \
    tools/run_probe.sh client-probes/probe_ladder mp_ship +set probe_teleport 1
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ladder 90 --probe-secs 45
```

With `--save-ladder` the client runs a climb script instead of the binds,
starting 1.5 s after the first snapshot on the rungs: up at a level view,
a hang, up looking 45 up, backing down, a strafe and its release, up and
across, down looking 60 down, up looking 10 down, then up and off the top.
`--save-ladder climb` runs the short one, straight up looking 45 up and off
the top. Either writes every cmd and snapshot to
`crates/server/tests/fixtures/playerstate/mp_ship-dm-ladder[-<tag>].txt`,
which `crates/server/tests/ladder_ab.rs` replays
(`docs/research/cod11-mantle.md`, "Ladders"). `+set probe_spot 1` with
`probe_spot_x`, `_y`, `_z` and `_yaw` puts the player at another ladder's
foot; pass the same yaw to `--probe-ladder`. Same server line; the client
takes about 25 s:

```
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ladder 90 --save-ladder --probe-secs 60
# the crow's nest, with +set probe_spot 1 +set probe_spot_x 4959 +set probe_spot_y 64
# +set probe_spot_z 693 +set probe_spot_yaw 0 on the server line:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ladder 0 --save-ladder climb --capture-tag crowsnest-climb --probe-secs 35
```

`--save-ladder turn --capture-tag turn` climbs a little, then sweeps the
view past `bg_ladder_yawcap` both ways while hanging, climbs and flicks
while turned, climbs down and sweeps again at the foot, sending the yaw
as a mouse does (rebased on the placement's `delta_angles`); it writes
`mp_ship-dm-ladder-turn.txt`, which `crates/server/tests/ladder_yawcap_ab.rs`
replays (`docs/research/cod11-mantle.md`, "The ladder yaw cap"). The
1.5 and UO captures took the tags `turn-1.5` and `turn-uo`.

On 1.5 and UO the same lines run with `COD_VERSION=1.5` or `uo` on the
server's (`COD_DIR` holding the stock 1.5 `main/`, and UO's `uo/`) and
`--capture-tag 1.5` or `--capture-tag uo` on the client's, which for UO
also takes `--game-dir <UO install> --mod-dir uo`.

## probe_weapsel

The client weapon selection's server half. On each allied player's first
spawn it logs the primary and the pistol, then 4 s apart: `switchToWeapon`
to the pistol, `switchToWeapon` back to the primary, the primary's clip and
reserve emptied, and 8 s later the weapon held, each as `PROBE <step>
<time> current <weapon>`. It then puts a loaded `thompson_mp` in the primary
slot, switches to it, and logs `PROBE mode_<n> slot <primary> <time>
current <weapon>` every 4 s five times. The client half, `--probe-weapsel`,
builds every cmd through `PlayInput` with `a` and its own events routed in,
pulls the trigger every 700 ms from 15 s to 19 s after its join settles,
runs `weapalt` at 26 and 30 s, `weapon colt_mp` at 34 s and `weapalt` at
38 s, prints a `WEAPSEL` line per snapshot whose weapon fields or sent byte
moved, per `a`, press and own weapon event, and writes
`crates/server/tests/fixtures/playerstate/<map>-dm-weapsel.txt` for
`weapsel_ab`. `docs/research/cod11-combat.md` 9.8 and 9.9 read the runs.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30721 SECS=100 \
    tools/run_probe.sh client-probes/probe_weapsel mp_carentan
# second shell, about 9 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:30721 --probe-team allies --probe-weapsel --probe-secs 75
```

## probe_slide

A hit's knockback timer while the player slides along a wall, the server
half. Under `probe_teleport 1` it sets each spawned allied player down at
(960 1830) on the mp_carentan street, 15 units north of the south wall, six
times 3 s apart, and 0.3 s after each sets off a `radiusDamage(at, 100, d,
d)` beside it: from the north (200, then 100), the north-west, the east,
and twice from the north one frame apart. It logs `PROBE drop`, `PROBE
blast <time> <trial> <origin> at <blast> damage <d>`, the damage callback's
`PROBE damage` / `PROBE damaged` and `PROBE after`, with health held at
1000. The client half is `probe_fall`'s with `--probe-fall-walk 315`. The
2026-10-09 run is `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slide-hit.txt`,
read in `docs/research/cod11-player-clip.md` 8.5 and gated by `fall_ab.rs`.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29701 PROBE_SECS=50 \
    tools/run_probe.sh client-probes/probe_slide mp_carentan +set probe_teleport 1
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29701 --probe-team allies --probe-fall \
    --probe-fall-walk 315 --probe-secs 40
```

## probe_ride

The mover push and ride capture's server half. It renames mp_carentan's two
bombzone brush models' `script_gameobjectname` to `dm` so `_gameobjects`
keeps them, and under `probe_teleport 1` stands each spawned allied player on
model `*5`'s plank slab and moves it one verb per phase (up, down, across, a
2-degree yaw, a push, the slab lowered onto the player's head), then links
the player to a `script_origin` and moves and turns that. Every server frame
of a phase logs `PROBE f <phase> <time> <player origin> <mover origin> <mover
angles>`. The client half prints a `RIDE` line per snapshot and `--probe-fall`'s
`CMDS` lines. Neither half writes a file; the 2026-10-20 run is in
`crates/server/tests/fixtures/movers/mp_carentan-dm-ride.txt` (the server's
`PROBE` lines, then the client's `CMDS` lines) and `-ride-wire.txt` (the
client's `RIDE*` and trajectory lines), which `crates/server/tests/ride_ab.rs`
replays on the client's cmds and `docs/research/cod11-movers.md` 11 to 14
reads.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33801 SECS=135 \
    tools/run_probe.sh client-probes/probe_ride mp_carentan +set probe_teleport 1
# second shell, about 6 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33801 --probe-team allies --probe-ride --probe-secs 122
```

## probe_cull

When a moving brush model leaves and re-enters the snapshot. It keeps
mp_carentan's bombzone brush models as probe_ride does, stands each spawned
allied player on the attackers' spawn beside model `*5`, lifts the slab 20000
units over 8 s and lowers it back over 8 s, logging `PROBE f <phase> <time>
<mover origin>` every frame. The client half's `RIDE_GONE` and `RIDE_ENT`
lines say which snapshot dropped it and which brought it back. The
2026-10-06 run is `crates/server/tests/fixtures/movers/mp_carentan-dm-cull.txt`;
`docs/research/cod11-movers.md` 14 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_cull mp_carentan
# second shell, about 12 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-ride --probe-secs 55
```

## probe_bump

Player-vs-player clipping's server half. Under `probe_teleport 1` it puts
each spawning axis player on a flat brush floor on mp_carentan at
(1132 -376 -151.875) and each allied player 200 units behind it along -x,
both facing +x, once per spawn, and logs each move as `PROBE place <time>
<clientnum> <team> <origin> <yaw>`. The floor is flat from 280 units behind the
spot to 360 in front and 70 either side, which a throwaway `test_ground_under` /
`test_clear_line` search in `crates/server` found; on any other map the thread
logs `PROBE teleport unsupported <map>` and does nothing.

Under `probe_overlap 1` it also waits for both players to be placed (`PROBE
both_placed <time>`), then `setorigin`s the axis player onto the allied one's
origin 12 s and 30 s later (`PROBE overlap <time> <target> <walker> <origin>`)
and logs `PROBE state <time> <clientnum> <team> <origin> <isOnGround>` for every
playing player every frame from 1 s before to 3 s after each. 1.1 gsc has no
`getvelocity` (`tools/re/dump_builtins.py` lists none), so the per-frame
origins stand in for it.

It calls `maps\mp\gametypes\dm::main()` itself, so a client can answer the
stock team menu and spawn. Three shells, the gsc probe first, then the target,
then the walker:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_bump mp_carentan +set probe_teleport 1
# second shell, about 12 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-bump-target --probe-team axis --probe-secs 185
# third shell, about 10 s after that:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-bump --probe-team allies --probe-secs 170
```

`--probe-bump-target` starts its schedule once it sees the walker on its mark:
standing 35 s, crouched 25 s, standing 1.5 s (a prone straight out of a
crouch is sometimes refused), prone 25 s, standing. `--save-bump` runs its
script once per stance, each started once the target's `solid` top byte reads
that stance: a head-on walk, a walk back, a glance 20 units right of the line,
a walk back, a jump from rest 10 units short of contact, a walk back. It writes
`crates/server/tests/fixtures/playerstate/<map>-dm-bump-walker.txt`.

The overlap capture is the same three shells with `+set probe_overlap 1` on
the server and `--capture-tag overlap` on the walker, which picks its overlap
script: still through the first setorigin, then walking +x through the second,
clocked off the placement's server time. The spectator-slot capture adds a
plain `--net-probe 127.0.0.1:28970 --probe-secs 185` started before the target,
so it holds slot 0 and never joins, and tags the walker `overlap-spectator`.
The gsc's `PROBE` lines of each run go to
`<map>-dm-bump-script.txt`, `-bump-overlap-script.txt` and
`-bump-overlap-spectator-script.txt` beside the walker fixtures, with the
target's own push lines (`BUMPT`, its stdout) appended as comments.

The wall capture adds `+set probe_bump_spot wall` to the overlap run and
tags the walker `overlap-wall`: the gsc puts the target at (144 448 0.25)
and the walker 200 behind, 8.875 short of a +y wall, so the first push
drives the walker into it and the second, once it has walked into the
corner with a +x wall, into both (`docs/research/cod11-player-clip.md`
10.4). Its files are `-bump-overlap-wall-walker.txt` and
`-bump-overlap-wall-script.txt`.

All eight files are retail evidence. A walker run against `vcod-server`
overwrites the untagged one and refuses the tagged ones without
`--overwrite-fixture`: move them to `tmp/` and `git checkout` the fixture
directory after.

## probe_spawnbody

A spawn inside another player and `ClientImpacts`' `"touch"`
(`docs/research/cod11-player-clip.md` 13). The first player to play is the
host, the second the spawner; the script holds both clients' keys through
`PROBE_IN <buttons> <wbuttons> <up> <forward> <right>` lines, so both run
`--probe-uo-weapon direct`. Each trial puts the host on probe_bump's
mp_carentan spot facing +x, sets the spawner's forward key and `spawn`s
him 8 inside the host or 31 off, logging the origin the spawn's think
left (exact, integer part and remainder); two walks then put the spawner
60 behind the host and hold forward 0.6 s. Every `"touch"` a player gets
from another is logged. Two shells after the server, the host first:

```
COD_VERSION=<1.1|1.5|uo> COD_DIR=<install> PORT=<p> PROBE_SECS=85 \
  tools/run_probe.sh client-probes/probe_spawnbody mp_carentan
# about 8 s later:
cargo run -p vcod -- --game-dir <install> --net-probe 127.0.0.1:<p> \
  --probe-uo-weapon direct --probe-team allies --probe-secs 74
# 4 s after that:
cargo run -p vcod -- --game-dir <install> --net-probe 127.0.0.1:<p> \
  --probe-uo-weapon direct --probe-team allies --probe-secs 70
```

(`--mod-dir uo` on both clients for UO.) The server's `PROBE` lines are
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-spawnbody-<1.1|1.5|uo>.txt`,
which `spawnbody_ab.rs` replays.

The 1.5b overlap capture `-bump-overlap-spectator-15-walker.txt` and
`-script.txt` is probe_bump's spectator-slot recipe on `COD_VERSION=1.5`,
the walker tagged `overlap-spectator-15`.

## probe_bomb and probe_blastbody

Who a scripted blast reaches, for `docs/research/cod11-combat.md` 14.4. Both
call `maps\mp\gametypes\sd::main()` and wait for four playing clients after
the match-start restart, so the recipe is four `--probe-team` clients, two per
team, started a couple of seconds apart:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=110 \
    tools/run_probe.sh client-probes/probe_bomb mp_carentan
# second shell, about 6 s later:
for t in allies axis allies axis; do
    cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team $t --probe-secs 100 &
    sleep 2
done
```

`probe_blastbody` takes the same recipe with 120 s on the server and
`--probe-secs 110` on the clients. Against ours
the server half is
`vcod-server mp_carentan --gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_bomb.gsc`,
whose log carries the same lines as `script: …`. Nothing here writes a
fixture.

`probe_bomb` stands the four on fixed stations round the committed plant
capture's charge, `radiusDamage`s at and near it four times at a flat 20, then
replays the tail of `sd.gsc`'s plant and lets the stock `bomb_countdown` run
out. Retail, 2026-09-26, the first round of the run:

```
0:16 PROBE place 0 (-196.00, 2455.00, -22.00)
0:16 PROBE place 1 (-84.00, 2511.00, -22.00)
0:16 PROBE place 2 (-112.00, 2446.00, -22.00)
0:16 PROBE place 3 (-77.00, 2473.00, -32.00)
0:16 PROBE bomb (-176.80, 2473.10, -22.96)
0:16 PROBE trigger getorigin (-176.80, 2473.10, -22.96) origin (-176.80, 2473.10, -22.96)
0:17 PROBE blast at_bomb
0:18 PROBE blast above_bomb
0:18 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:19 PROBE blast trigger_getorigin
0:20 PROBE blast above_player0
0:20 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:21 PROBE before 0 100 playing (-197.05, 2454.00, -21.88)
0:21 PROBE before 1 100 playing (-84.00, 2511.00, -21.88)
0:21 PROBE before 2 100 playing (-112.00, 2446.00, -21.88)
0:21 PROBE before 3 100 playing (-77.00, 2473.00, -31.87)
1:21 W;allies;vcod;vcod
1:21 L;axis;vcod;vcod
1:22 PROBE after 0 100 playing (-197.05, 2454.00, -21.88)
1:22 PROBE after 1 100 playing (-84.00, 2511.00, -21.88)
1:22 PROBE after 2 100 playing (-112.00, 2446.00, -21.88)
1:22 PROBE after 3 100 playing (-77.00, 2473.00, -31.87)
```

`probe_blastbody` puts two clients on one line from a blast in the open and
blasts at a flat 20: once, once with the front one moved off the line, once
per yaw 0 to 315 in steps of 45 with the front one back on its station and
turned by `setPlayerAngles`, and once over its corpse. Each yaw row logs the front client's `angles` as they read back: the call did not
land on every row, and the rows are read by the logged yaw. Retail,
2026-09-27, the first run from the yaw rows on (the rows above them read as
the second run's):

```
0:23 PROBE blast yaw 0 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-271.00, 2379.00, -31.99)
0:23 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:23 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:25 PROBE blast yaw 45 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.99) (-271.77, 2378.23, -31.99)
0:25 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:25 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:28 PROBE blast yaw 90 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.53, 2377.47, -31.99)
0:28 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:28 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:30 PROBE blast yaw 135 (0.00, 135.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:30 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:33 PROBE blast yaw 180 (0.00, 180.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:33 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:33 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:35 PROBE blast yaw 225 (0.00, 225.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:35 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:38 PROBE blast yaw 270 (0.00, 270.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:38 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:40 PROBE blast yaw 315 (0.00, 315.00, 0.00) (-226.00, 2424.00, -31.00) (-273.30, 2376.70, -31.99)
0:40 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:42 K;0;allies;vcod;0;allies;vcod;none;100000;MOD_SUICIDE;none
0:44 PROBE blast corpse dead (-273.30, 2376.70, -31.99)
0:44 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:45 PROBE done
```

The second run, whole:

```
0:18 PROBE place 0 (-226.00, 2424.00, -32.00) (0.00, 0.00, 0.00)
0:18 PROBE place 1 (-269.00, 2381.00, -32.00) (0.00, 0.00, 0.00)
0:18 PROBE place 2 (400.00, 3272.00, -23.88) (0.00, 0.00, 0.00)
0:18 PROBE place 3 (224.00, -1280.00, 1.86) (0.00, 0.00, 0.00)
0:19 PROBE blast shielded
0:19 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:19 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:21 PROBE blast unshielded (-300.00, 2473.00, -31.87) (-269.77, 2380.23, -31.87)
0:21 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:21 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:24 PROBE blast yaw 0 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-270.94, 2379.06, -31.87)
0:24 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:24 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:26 PROBE blast yaw 45 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-271.71, 2378.29, -31.87)
0:26 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:26 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:29 PROBE blast yaw 90 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.71, 2377.30, -31.87)
0:29 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:29 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:31 PROBE blast yaw 135 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-272.71, 2377.30, -31.87)
0:31 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:31 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:34 PROBE blast yaw 180 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-273.80, 2376.20, -31.87)
0:34 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:34 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:36 PROBE blast yaw 225 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.78) (-274.66, 2375.34, -31.87)
0:36 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:36 D;1;axis;vcod;-1;world;;none;13;MOD_EXPLOSIVE;none
0:39 PROBE blast yaw 270 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-275.36, 2374.64, -31.87)
0:39 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:39 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:41 PROBE blast yaw 315 (0.00, 0.00, 0.00) (-226.00, 2424.00, -31.87) (-275.80, 2374.21, -31.87)
0:41 D;0;allies;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:41 D;1;axis;vcod;-1;world;;none;6;MOD_EXPLOSIVE;none
0:43 K;0;allies;vcod;0;allies;vcod;none;100000;MOD_SUICIDE;none
0:45 PROBE blast corpse dead (-276.51, 2373.49, -31.87)
0:45 D;1;axis;vcod;-1;world;;none;20;MOD_EXPLOSIVE;none
0:46 PROBE done
```

What the rows measure is `docs/research/cod11-combat.md` 14.4.

## probe_passthru

Whether a rifle round goes on through the player it hits, for
`docs/research/cod11-combat.md` 2.4. Under `probe_teleport 1` on mp_carentan
it stands the lower-numbered axis player on `probe_bump`'s spot, the other
axis player 100 units behind it along +x and the allied player 200 units in
front, all facing +x, once per spawn. It wraps `dm.gsc`'s damage callback to
log `PROBE damage <time> <victim> <iDamage> <iDFlags> <mod> <hitloc> <vPoint>
<origin>` first, so two hits in one frame share a time. Four shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_passthru mp_carentan +set probe_teleport 1
# second and third shell, about 8 s and 14 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-target --probe-team axis --probe-secs 180
# fourth shell, about 18 s after that, once both targets have killed
# themselves once and respawned:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-hit --probe-sweep --probe-team allies --probe-secs 150
```

Against ours the server half is `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_passthru.gsc --set
probe_teleport=1`. The two targets write
`crates/server/tests/fixtures/playerstate/mp_carentan-probe_passthru-hit-target.txt`,
which is nobody's evidence: delete it after. Retail, 2026-09-27, the first
round:

```
0:24 PROBE place 24600 0 axis (1132.00, -376.00, -151.88)
0:28 PROBE place 28100 1 axis (1232.00, -376.00, -151.88)
0:30 PROBE place 30650 2 allies (932.00, -376.00, -151.88)
0:33 PROBE damage 33600 0 40 32 MOD_RIFLE_BULLET torso_upper (1122.40, -376.23, -90.13) (1132.00, -375.95, -151.87)
0:33 PROBE damage 33600 1 33 32 MOD_RIFLE_BULLET head (1229.84, -377.40, -85.77) (1232.00, -376.00, -151.88)
0:33 PROBE damage 33950 0 67 32 MOD_RIFLE_BULLET head (1134.93, -375.65, -82.77) (1135.53, -375.95, -151.88)
0:34 PROBE damage 34350 0 67 32 MOD_RIFLE_BULLET head (1143.72, -374.13, -86.55) (1143.27, -375.55, -151.88)
0:34 PROBE damage 34700 1 67 32 MOD_RIFLE_BULLET head (1231.67, -376.03, -84.59) (1234.30, -376.00, -151.88)
```

The sweep spends its taps within about 20 s, so the back player is in line
behind a live front one for only a few of them: this run caught one pass,
at 33600.

## probe_glass

Whether a round goes on through a window pane, for
`docs/research/cod11-combat.md` 2.4, step 3. Under `probe_teleport 1` on
mp_depot it stands every axis player at (-848 -3656), 76 units behind the
glass brush at x -772..-764, and every allied player at (-748 -3656), 16
units in front of it, facing each other, once per spawn; the sweep's
120-unit range is met from the spot, so the shooter never has to walk round
the pane. The damage log is `probe_passthru`'s. Three shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=200 \
    tools/run_probe.sh client-probes/probe_glass mp_depot +set probe_teleport 1
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-target --probe-team axis --probe-secs 180
# third shell, about 14 s after that:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --save-hit --probe-sweep --probe-team allies --probe-secs 150
```

Against ours the server half is `vcod-server mp_depot --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_glass.gsc --set
probe_teleport=1`. The target writes
`crates/server/tests/fixtures/playerstate/mp_depot-probe_glass-hit-target.txt`,
which is nobody's evidence: delete it after. The shooter's `bullethit` lines
carry the pane's impact and the flesh one behind it. Retail, 2026-09-27, the
first hits:

```
1:32 PROBE damage 92600 0 180 0 MOD_PISTOL_BULLET head (-840.35, -3655.53, 27.39) (-848.00, -3656.00, -39.88)
1:48 PROBE damage 108850 0 180 0 MOD_PISTOL_BULLET head (-840.68, -3655.10, 24.69) (-848.00, -3656.00, -39.88)
2:18 PROBE damage 138350 0 107 0 MOD_PISTOL_BULLET torso_upper (-839.14, -3656.68, 18.99) (-848.00, -3656.00, -38.99)
```

## probe_blastloop

What one `radiusDamage` does between its victims, for
`docs/research/cod11-combat.md` 14.5. It wraps `level.callbackPlayerDamage`
so every victim logs `PROBE cb <slot> <iDamage> <sessionstate> <health>` as
the engine calls it and `PROBE cbafter <slot> <sessionstate> <health>` once
sd's own callback returns. `probe_blastbody`'s recipe, 75 s on the server
and `--probe-secs 65` on the clients. Against ours the server half is
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_blastloop.gsc`.
Nothing here writes a fixture.

Rows: three flat-20 blasts across `setPlayerIgnoreRadiusDamage(true)`, a
second blast with the flag still set, and `(false)`; then a lethal flat 200
down `probe_blastbody`'s line with 100-health slot 0 in front and
1000-health slot 1 behind, a flat 20 in the same frame and one a frame
later; then the same three on a second line with slot 3 in front and slot 2
behind. Retail, 2026-10-05, from the first blast on (sd's `D;` and `K;`
records dropped):

```
PROBE blast ignore_on
PROBE blast ignore_still
PROBE blast ignore_off
PROBE cb 0 20 playing 100
PROBE cbafter 0 playing 80
PROBE cb 1 13 playing 100
PROBE cbafter 1 playing 87
PROBE before_low 0 playing 100 (-226.00, 2424.00, -31.87)
PROBE before_low 1 playing 1000 (-269.00, 2381.00, -31.87)
PROBE blast lethal_low
PROBE cb 0 200 playing 100
PROBE cbafter 0 dead 0
PROBE cb 1 200 playing 1000
PROBE cbafter 1 playing 800
PROBE blast same_frame_low
PROBE cb 0 20 dead 0
PROBE cbafter 0 dead -20
PROBE cb 1 20 playing 800
PROBE cbafter 1 playing 780
PROBE blast next_frame_low
PROBE cb 1 20 playing 780
PROBE cbafter 1 playing 760
PROBE before_high 0 spectator -20 (-245.37, 2404.63, 28.01)
PROBE before_high 1 playing 760 (224.43, -1280.81, 1.97)
PROBE before_high 2 playing 1000 (-306.80, 2473.10, -31.87)
PROBE before_high 3 playing 100 (-246.80, 2473.10, -31.00)
PROBE blast lethal_high
PROBE cb 3 200 playing 100
PROBE cbafter 3 dead 0
PROBE blast same_frame_high
PROBE cb 3 20 dead 0
PROBE cbafter 3 dead -20
PROBE cb 2 20 playing 1000
PROBE cbafter 2 playing 980
PROBE blast next_frame_high
PROBE cb 2 20 playing 980
PROBE cbafter 2 playing 960
```

Ours, the same day, after the walk was made retail's: every row reads the
same down to `next_frame_low`. On the second line slot 3 stands at z -21.88
where retail's stands at -31.00, its body leaves two of slot 2's five
probes clear where retail's left none, and slot 2 takes 133 on
`lethal_high`; the
`same_frame_high` callbacks run 2 then 3 where retail's ran 3 then 2.

Ours on 2026-10-06, after the walk took the area tree's order (combat doc
14.7): `same_frame_high` runs 3 then 2 as retail's did. Then, with
`PM_CorrectAllSolid` and the step gate for a start in solid ported (mantle
doc, "A start inside a solid"), slot 3 stands at -31.00 inside the flak88
clip as retail's does, and every row from the first blast on reads as
retail's.

The `after_low` and `before_high` rows this section's block drops, retail
then ours on 2026-10-06 after the `PM_DeadMove` and spawn-health fixes
(combat doc 5.6 and 5.7): dead slot 0 came to rest at (-245.37, 2404.63)
and (-244.78, 2405.35), and read `spectator -20` after sd's
`spawnSpectator` on both. Before the fixes ours slid it to (-273.20,
2376.80) and its spectator read 100.

## probe_blastorder

The order one `radiusDamage` walks its victims in, for
`docs/research/cod11-combat.md` 14.7. Four players at 1000 health are set
down round mp_carentan's second area-tree split (y 2464), then moved one at
a time, a flat-20 blast from `(-176.8, 2473.1, 7)` after each; the wrapped
damage callback logs `PROBE cb <entity> <iDamage>` and puts the health back.
`probe_blastbody`'s recipe, 100 s on the server and `--probe-secs 85` on the
clients. Against ours the server half is `vcod-server mp_carentan
--gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_blastorder.gsc`.
Nothing here writes a fixture.

Retail, 2026-10-06 (the per-blast `state` lines cut to the moved player):

```
PROBE placed 0 playing (-290.00, 2430.00, -31.94)
PROBE placed 1 playing (-260.00, 2480.00, -31.83)
PROBE placed 2 playing (-230.00, 2380.00, -31.87)
PROBE placed 3 playing (-230.00, 2540.00, -31.97)
PROBE blast placed
PROBE cb 1 20
PROBE cb 0 20
PROBE cb 3 20
PROBE cb 2 20
PROBE moved_within 0 playing (-290.00, 2440.00, -31.87)
PROBE blast moved_within
PROBE cb 0 20
PROBE cb 1 20
PROBE cb 3 20
PROBE cb 2 20
PROBE moved_into 2 playing (-200.00, 2430.00, -31.77)
PROBE blast moved_into
PROBE cb 2 20
PROBE cb 0 20
PROBE cb 1 20
PROBE cb 3 20
PROBE moved_out 1 playing (-290.00, 2380.00, -31.87)
PROBE blast moved_out
PROBE cb 2 20
PROBE cb 0 20
PROBE cb 3 20
PROBE cb 1 20
```

Ours, the same day, after the walk took the area tree's order: every `cb`
line the same and in the same order. The rest heights differ by up to 0.12
(-31.87 on all four, -31.75 for slot 1's last move).

## probe_itemdrop

The item flight, landing and respawn capture's server half, on mp_carentan
with one allied player. With the full loadout it first spawns an
`item_health` at the feet of a player at 40 health with `random` 0.5, then a
colt with spawnflags 8 after emptying the pistol's reserve, each taken by
walking onto it, and moves the player off each so the return stays on the
ground. Then four `dropItem` drops (the held carbine onto flat ground, the
colt into a wall 60 units ahead, the frag down a slope, and `item_health`)
and three script spawns (a carbine 72 units in the air, a `dropHealth`-style
pack at the feet, a colt with spawnflags 1). It logs `PROBE spawned`, `PROBE
drop` (with the player's and the item's origin and angles), `PROBE trigger`,
one `PROBE at` per frame a tracked item's `origin` or `angles` changed and a
`PROBE rest` once they have held for 3 s. `wait` is a gsc keyword, so the
pack's respawn goes through `random`. The client half, `--probe-items`,
prints an `ITEM` line per snapshot an item entity appeared or changed in and
an `ITEM_GONE` when it left. Neither half writes a file; the two runs' lines
were concatenated into
`crates/server/tests/fixtures/items/mp_carentan-dm-itemdrop.txt`, read in
`docs/research/cod11-items.md` 14 and gated by
`crates/server/tests/itemdrop_ab.rs`.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=115 \
    tools/run_probe.sh client-probes/probe_itemdrop mp_carentan
# second shell, about 15 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-items --probe-secs 90
```

## probe_nodrop

`CONTENTS_NODROP` (0x80000000) on four spawned carbines. No stock material
carries the bit, so it runs on `mp_itemtest`: mp_carentan's BSP with 0x80000000
ORed into material 0's contents (`textures/common/clipmonster`, the
`u32` at lump 0's offset + 68), saved as `maps/mp/mp_itemtest.bsp` beside a
copy of `maps/MP/mp_carentan.gsc` renamed to `maps/mp/mp_itemtest.gsc`, in a
`zzz_` pak in the homepath's `main/`. The pak is built from game data and is
never committed. It needs no client, so `tools/run_probe.sh` alone prints
everything; run it on stock mp_carentan too for the control.
`docs/research/cod11-items.md` 14.5 reads both.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=30 \
    tools/run_probe.sh client-probes/probe_nodrop mp_itemtest
```

Ours takes the same pak through a scratch `--game-dir` whose `main/` links
the stock paks beside it: `vcod-server mp_itemtest --game-dir <dir>
--gametype-script .../probe_nodrop.gsc`.

## probe_victims

Four leftovers of the damage path, for `docs/research/cod11-combat.md` 4.4,
5.6, 5.7 and 14.6. `probe_blastbody`'s recipe, 75 s on the server and
`--probe-secs 65` on the clients; against ours the server half is
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_victims.gsc`.
Nothing here writes a fixture.

Rows: three lethal `finishPlayerDamage` calls whose killed callback logs
the inflictor and attacker it was handed; a `spawnSpectator` and a bare
`self spawn` on a client whose health the probe set first; a flat 200 down
`probe_blastbody`'s line, killing slot 0 in front and not 1000-health slot 1
behind, with both origins logged every frame for 1.5 s; three flat-60 blasts
beside carentan's `misc_mg42` at (1712, 1830, 8) with slot 1 in range too,
the turret's `"damage"` and `"death"` waiters logging. Retail, 2026-10-06
(sd's `D;` and `K;` records and slide frames 10 to 29, all equal to 9,
dropped):

```
PROBE org 179
PROBE fpd ent_player
PROBE killed 0 inflictor 1 attacker 1 MOD_GRENADE_SPLASH
PROBE fpd player_ent
PROBE killed 3 inflictor 179 attacker 179 MOD_RIFLE_BULLET
PROBE fpd undefined_player
PROBE killed 0 inflictor 1022 attacker 1 MOD_RIFLE_BULLET
PROBE spec health 37 spectator
PROBE spec_later health 37
PROBE play health 41 maxhealth 100
PROBE play_later health 41
PROBE slide_before 0 (-226.00, 2424.00, -31.79) 1 (-269.00, 2381.00, -31.99)
PROBE cb 0 200 playing 100
PROBE killed 0 inflictor 1022 attacker 1022 MOD_EXPLOSIVE
PROBE cb 1 200 playing 1000
PROBE slide 0 0 dead (-226.00, 2424.00, -31.79) 1 (-269.00, 2381.00, -31.99)
PROBE slide 1 0 dead (-231.11, 2418.92, -31.87) 1 (-274.65, 2375.35, -31.99)
PROBE slide 2 0 dead (-238.91, 2411.17, -31.87) 1 (-286.18, 2363.82, -31.99)
PROBE slide 3 0 dead (-242.41, 2407.71, -31.99) 1 (-293.87, 2356.14, -31.99)
PROBE slide 4 0 dead (-243.68, 2406.47, -31.99) 1 (-299.84, 2350.16, -31.99)
PROBE slide 5 0 dead (-243.68, 2406.47, -31.99) 1 (-304.31, 2345.70, -31.99)
PROBE slide 6 0 dead (-243.68, 2406.47, -31.99) 1 (-307.60, 2342.40, -31.99)
PROBE slide 7 0 dead (-243.68, 2406.47, -31.99) 1 (-309.86, 2340.14, -31.99)
PROBE slide 8 0 dead (-243.68, 2406.47, -31.99) 1 (-310.80, 2339.21, -31.99)
PROBE slide 9 0 dead (-243.68, 2406.47, -31.99) 1 (-311.49, 2338.51, -31.99)
PROBE turret 297 (-500.00, 1896.00, 175.00) health 100
PROBE turret 298 (1712.00, 1830.00, 8.00) health 100
PROBE tplace 1 (1712.00, 1990.00, -23.87)
PROBE tblast 0
PROBE cb 1 60 playing 1000
PROBE tblast_after 0 health 40
PROBE tdamage 60 1022 health 40
PROBE tblast 1
PROBE cb 1 60 playing 940
PROBE tblast_after 1 health -20
PROBE tdeath 1022 health -20
PROBE tdamage 60 1022 health -20
PROBE tblast 2
PROBE cb 1 60 playing 880
PROBE tblast_after 2 health -80
PROBE tdeath 1022 health -80
PROBE tdamage 60 1022 health -80
PROBE done
```

Ours, the same day, after the fixes: every row the same but the slide. Ours
stands both clients at z -31.87, and the frames move in bursts, since a
debug build's tick overruns and catches up; slot 0 came to rest at
(-241.43, 2408.58), 21.8 units out where retail's slid 24.9, and slot 1 at
(-307.67, 2342.33). Before the `PM_DeadMove` fix slot 0 slid 56.4.

## probe_hud_disconnect

Whether a client's `newClientHudElem` record outlives it, for
`docs/research/cod11-hud-protocol.md`, "A client's elements die with it".
One client is enough: it connects, `begin` makes one owned and one shared
element, and leaving runs the disconnect callback. Two shells:

```
COD_LNXDED_HOME=<absolute, no '+'> tools/run_probe.sh client-probes/probe_hud_disconnect mp_pavlov
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 8
```

Against ours: `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_hud_disconnect.gsc`.
Plain `--net-probe` writes no fixture. Retail and ours, 2026-10-06, print
the same three lines; the doc section quotes them.

## probe_dropclient

When the game hears a client leave: the disconnect callback's
`getTime()`, the `menuresponse` (`-1`, `disconnect`) that
`ClientDisconnect` raises on the player as its own thread and a level
thread hear it, and which message carries the callback's `iPrintLn`
against the drop's `e` notice. `docs/research/cod11-server-handshake.md`,
"When the game hears a drop". Four shells (or one script): a watcher, a
client that goes silent and times out, and one that sends `disconnect`.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=45 tools/run_probe.sh client-probes/probe_dropclient mp_pavlov +set sv_timeout 3
# 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge watch --probe-secs 34
# 2 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge silence --probe-secs 25
# 12 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge zombie-active --probe-secs 12
```

`COD_VERSION=1.5` and `COD_VERSION=uo` (map `mp_uo_carentan`: mp_cassino's
tank script needs `level.ceasefire`) take the same recipe. Against ours:
`vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_dropclient.gsc
--set sv_timeout=3`. The watcher's `EDGE cmd` lines carry the order.
Retail on all three and ours, 2026-10-18, print the same `PROBE` lines
(clocks aside) and the same layout.

## probe_reconnect

A client that connects again into the slot it still holds, and what the
disconnect callback sends the leaver alone: whether the game hears the old
client go, whether a thread on the old player lives on, whether a script
field and `pers` survive, and which of a print and a `setClientCvar` reach
the zombie. `docs/research/cod11-server-handshake.md`, "A reconnect into a
held slot" and "When the game hears a drop". Four shells (or one script):

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=40 tools/run_probe.sh client-probes/probe_reconnect mp_pavlov
# 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge watch --probe-secs 28
# 2 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge reconnect --probe-secs 20
# 2 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-edge zombie-active --probe-secs 12
```

`COD_VERSION=1.5` and `COD_VERSION=uo` (map `mp_uo_carentan`) take the
same recipe. Against ours: `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_reconnect.gsc`.
The zombie's `EDGE cmd` lines carry what reached it. Retail on all three
and ours, 2026-10-18, print the same `PROBE` lines (clocks aside); ours
takes the reconnect a few seconds later, since it refuses a connect with a
new challenge into a slot heard from in the last 3 s.

## probe_uostates

The wire round trip's state changes on United Offensive: a placed allied
player dies, respawns, spectates, respawns and rides a `linkTo` on a moving
script_origin, under a `--save-slope --slope-every-state` client.
`docs/research/coduo-movement.md` 15. Two shells:

```
COD_VERSION=uo COD_DIR=<UO install> COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=90 tools/run_probe.sh client-probes/probe_uostates mp_carentan
# 10 s later:
cargo run -p vcod -- --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:<p> --save-slope --slope-every-state --probe-moves --probe-cmd-ms 8 --probe-team allies --probe-weapon m1carbine_mp --capture-tag uostates-uo --probe-secs 80
```

The capture is `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-uostates-uo.txt`
(its header names this recipe), which `predict_ab`'s UO gate replays.

## probe_states

The states the client predicts besides a live body, on 1.1d: the placed
allied player spends 22 s as a free spectator under the moves script (its
runs and jumps fly him into the roofs and walls, the crouch dives him onto
the street), respawns, kills himself inside the diagonal runs and goes to
the intermission. `docs/research/cod11-player-clip.md` 1.1. Two shells:

```
COD_DIR=<1.1 install> COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=100 tools/run_probe.sh client-probes/probe_states mp_carentan
# 10 s later:
cargo run -p vcod -- --game-dir <1.1 install> --net-probe 127.0.0.1:<p> --save-slope --slope-every-state --probe-moves --probe-cmd-ms 8 --probe-team allies --probe-weapon m1carbine_mp --capture-tag states-1.1 --probe-secs 80
```

The capture is `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-states-1.1.txt`,
which `predict_ab` replays.

On 1.5b (`COD_VERSION=1.5`, `COD_DIR` at a stock 1.5 `main/`, the client
with `--game-dir` there and `--capture-tag states-1.5`) the run takes
`+set probe_suicide_wait 10.5` after the map, which kills the body inside
a stance change's `pm_time`, the timer 1.5's corpse ladder test reads
(`cod11-player-clip.md` 1.1). That capture is
`mp_carentan-dm-slope-8ms-states-1.5.txt`.

## probe_deadlink

A linked corpse (`pm_type` 7) on 1.1d, 1.5b and UO: the placed allied player is linked
to a `script_origin` inside the moves script's diagonal runs, killed half a
second later, and stays a linked corpse to the end of the run.
`docs/research/cod11-player-clip.md` 1.1. The link wait is timed against
the capture's join; check the capture's velocity at the link and move
`probe_link_wait` if it reads 0. Two shells:

```
COD_DIR=<1.1 install> COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> PROBE_SECS=100 tools/run_probe.sh client-probes/probe_deadlink mp_carentan +set probe_link_wait 49.95
# 10 s later:
cargo run -p vcod -- --game-dir <1.1 install> --net-probe 127.0.0.1:<p> --save-slope --slope-every-state --probe-moves --probe-cmd-ms 8 --probe-team allies --probe-weapon m1carbine_mp --capture-tag deadlink-1.1 --probe-secs 75
```

The capture is `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-deadlink-1.1.txt`,
which `predict_ab` replays. The same two shells with `COD_VERSION=1.5`
(`COD_DIR` and the client's `--game-dir` at a stock 1.5 `main/`,
`--capture-tag deadlink-1.5`) and `COD_VERSION=uo` (`--game-dir <UO
install> --mod-dir uo`, `--capture-tag deadlink-uo`) took the `-1.5.txt`
and `-uo.txt` captures with the same link wait.

## probe_menuresponse

What `Cmd_MenuResponse_f` hands the script for each shape of `mr`, for
`docs/research/cod11-gsc-language.md` 6. One plain probe sends the lines;
the script logs every `menuresponse` its entity hears.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=50 tools/run_probe.sh client-probes/probe_menuresponse mp_pavlov
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 25 \
    --probe-say "1:mr 0 noop" --probe-say "2:mr 9 bad" --probe-say "3:mr {sid} 0 hello" \
    --probe-say "4:mr {sid} 40 past" --probe-say "5:mr {sid} -1 neg" --probe-say "6:mr {sid} 0 a b" \
    --probe-say "7:mr" --probe-say "8:mr {sid} 0" --probe-say "9:mr 12345 0 stale" \
    --probe-say '10:mr {sid} 0 "quoted word"' --probe-say "11:mr {sid} 31 last" \
    --probe-say "12:mr {sid} 0x1 hex" --probe-say "13:mr {sid} 1abc junk" --probe-say "14:mr {sid} 0 Mixed"
```

Against ours: `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_menuresponse.gsc`.
Retail and ours, 2026-10-13, log the same fourteen `PROBE` lines; the doc's
table quotes them. `server::tests::mr_answers_each_shape_as_retail` pins
the parse.


## probe_say

What `sayAll`, `sayTeam` and `pingPlayer` send and who `G_Say` lets hear
it, for `docs/research/cod11-chat.md` 2.1. Two plain probes: the first to
begin is slot 0, the allied speaker, the second the axis listener. Each
`--probe-say` line prints the command it sent as `SAY`; every `h`/`i` line
that arrives prints as `CHAT`, control bytes escaped.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=60 tools/run_probe.sh client-probes/probe_say mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 45 --probe-say "0.5:say first hello"
# third shell, two seconds later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 43 --probe-say "0.5:say second hello"
```

Against ours: `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_say.gsc`; a debug
build's `wait 2` runs long, so give its probes 90 s. Retail and ours,
2026-10-06, print the same `CHAT` lines in both probes; the doc's table
quotes them. The client-command half of that table needs no probe script:
`tools/run_server.sh mp_carentan +set g_gametype tdm` and three probes,
`--probe-team axis`, `--probe-team spectator` and `--probe-team allies`
with the `--probe-say` list (`say`, `say_team`, `kill`, `say`, `tell 0 ...`).


## probe_touchorder

The order one touch pass meets what it touches, for
`docs/research/cod11-combat.md` 14.7, "The touch pass". One client under dm
on mp_pavlov, set down where the minefield triggers 101 and 102 overlap,
with three `item_health` spawned on the spot; damage is swallowed, so the
mines kill nobody and no health is taken. Each phase logs every `"trigger"`
(`f`), item `"touch"` (`t`), mine `"touch"` (`mt`) and player `"touch"`
(`pt`) for a fifth of a second. `+set probe_mode ammo` runs the other half:
three mosin items on the spot and a reserve one round short, so only the
first item the walk meets is taken. Two shells:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=60 \
    tools/run_probe.sh client-probes/probe_touchorder mp_pavlov [+set probe_mode ammo]
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-secs 45
```

Against ours the server half is `vcod-server mp_pavlov --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_touchorder.gsc
[--set probe_mode=ammo]`; `RUST_LOG=info,touch_pass=trace` prints each
pass's walk. Nothing here writes a fixture.

Retail, 2026-10-06, the first frame of each phase (each later cmd of a frame
adds one more `f 101`, `f 102` pair and nothing else):

```
PROBE phase spawned
PROBE f 101 11250
PROBE f 102 11250
PROBE t c 11250
PROBE t a 11250
PROBE t b 11250
PROBE mt 101 11250
PROBE pt 102 11250
PROBE mt 102 11250
PROBE phase kept
PROBE f 101 12550
PROBE f 102 12550
PROBE t c 12550
PROBE t a 12550
PROBE t b 12550
PROBE mt 101 12550
PROBE pt 102 12550
PROBE mt 102 12550
PROBE phase moved
PROBE f 101 13950
PROBE f 102 13950
PROBE t c 13950
PROBE t b 13950
PROBE t a 13950
PROBE mt 101 13950
PROBE pt 102 13950
PROBE mt 102 13950
```

and with `probe_mode ammo`:

```
PROBE weapon mosin_nagant_mp
PROBE phase ammo
PROBE take e 9650
PROBE done
```

An earlier run without the `mt` and `pt` watchers printed the same `f` and
`t` lines. Ours, the same day: the
`touch_pass` trace walks 102, 101, b, a, c after the spawn and after `kept`,
102, 101, a, b, c after `moved`, and the ammo run takes e. Its log lines
came out in another order then (`mt 101`, `mt 102`, `pt 102`, `t a`, `t b`,
`t c`, `f 101`, `f 102`, once a frame); since 2026-10-17 they match
retail's, `G_RunFrame`'s trigger queue included. A full retail log of the
first mode, rerun that day, is
`crates/server/tests/fixtures/touchorder/mp_pavlov.txt`, which
`touchorder_ab.rs` replays. On 2026-10-20 the same mode ran on 1.5b
(`COD_VERSION=1.5`, the probe client with the 1.5 install as `--game-dir`)
and UO 1.51b (`COD_VERSION=uo`, the UO install and `--mod-dir uo`): every
first frame logs 1.1d's lines in 1.1d's order (`mp_pavlov-1.5.txt`,
`mp_pavlov-uo.txt` beside it, both replayed). The items rest at other
heights there (-29.08 for a on both), which the gate does not read.

## probe_trigqueue

`G_RunFrame`'s trigger queue when it fills, for
`docs/research/cod11-gsc-language.md`, "Engine notifies and the trigger
queue". No client; one script-spawned `trigger_damage` opened by
`enableLinkTo` takes N `radiusDamage`s in one step. One shell per patch:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=20 \
    [COD_VERSION=1.5|uo] tools/run_probe.sh client-probes/probe_trigqueue mp_carentan
```

Retail 1.1d, 1.5b and UO 1.51b printed byte-identical logs on 2026-10-20,
`crates/server/tests/fixtures/touchorder/trigqueue.txt`, which
`touchorder_ab.rs` replays at each patch.

## probe_squash

Which queued server commands `SV_AddServerCommand` replaces or drops, for
`docs/protocol-1.1.md`, "The server command queue". One plain probe; its
`serverCommand:` lines are the measurement.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=40 tools/run_probe.sh client-probes/probe_squash mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-secs 25
```

Retail, 2026-10-07: `v sq_early "1"` arrives and the early print does not;
the one-frame burst arrives as `v sq_a "2"`, `v sq_b "1"`, `v sq_c "1"`,
`f "sq dup"` twice, `u`, `f "sq between"`, `v sq_d "2"`; `v sq_a "3"` and
`v sq_a "4"` both arrive. `crates/server/src/client.rs`'s
`one_frame_squashes_as_the_retail_probe_did` replays the burst.

## probe_compass

What `G_GetNonPVSFriendlyInfo` packs into `iCompassFriendInfo`, for
`docs/research/cod11-hud-protocol.md` section 9, "Compass friendlies". Both
clients join allies; slot 0 stands at a spawn, slot 1 is set down at spawns 2
to 13 of mp_harbor in turn, pings, changes team and back, suicides, takes the
dead session and ends a spectator. Two probes with `--probe-compass`, whose
`COMPASS` lines print the field decoded beside the eye and the players the
snapshot carries:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=75 tools/run_probe.sh client-probes/probe_compass mp_harbor
# second and third shells, once the map is up, two seconds apart:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 62
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 60
```

The `PROBE step` lines carry slot 1's origin; slot 0's `COMPASS` lines carry
what it was sent. Against ours: `vcod-server mp_harbor --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_compass.gsc`.

## probe_suicide

What `ClientEndFrame` does with a `suicide()` whose killed callback leaves
`sessionstate` at `playing`, for `docs/research/cod11-combat.md` 5.1, "A
suicide that stays playing". Both clients join allies at mp_harbor's first
two tdm spawns; slot 1 suicides, stays `playing` 3 s, takes the dead session,
then spawns again. Two probes with `--probe-compass`: slot 1's `COMPASS`
lines carry its own `pm_type` and eye, slot 0's the players its snapshot
holds:

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=40 tools/run_probe.sh client-probes/probe_suicide mp_harbor
# second and third shells, once the map is up, two seconds apart:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 30
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-compass --probe-secs 28
```

Against ours: `vcod-server mp_harbor --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_suicide.gsc`.

## probe_tankcompass

What United Offensive's `G_GetNonPVSTankInfo` packs into
`iCompassTankInfo`, for `docs/research/coduo-diff.md` 9, "Compass". UO
only. The script puts the lowest-numbered `probe_vehicletype` vehicle on the
compass (`addVehicleToCompass`), sets client 0 down beside it, and once
client 0 sits in it moves client 1 to its team and round the tdm spawns,
takes the tank off the compass and back, goes round six spawns again while
the tank drives, and changes client 1's team. Client 0 runs `--probe-vehicle
driver`, client 1 `--probe-compass`, whose `COMPASS` lines add the field and
the vehicles in the snapshot on UO:

```
COD_VERSION=uo COD_DIR=<UO install> COD_LNXDED_HOME=<absolute, no '+'> SECS=95 \
    tools/run_probe.sh client-probes/probe_tankcompass mp_berlin +set probe_vehicletype panzeriv_mp
# second shell, once the map is up:
cargo run -p vcod -- --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:28970 \
    --probe-team axis --probe-vehicle driver --probe-secs 80
# third shell, three seconds later:
cargo run -p vcod -- --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:28970 \
    --probe-team axis --probe-compass --probe-secs 78
```

Against ours: `vcod-server mp_berlin --game-dir <UO install> --mod-dir uo
--gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_tankcompass.gsc --set
probe_vehicletype=panzeriv_mp`. Retail and ours, 2026-10-16: the field reads 0
on every line, and the out-of-view teammate (the hidden driver) is the same
step for step.

## probe_uocull

What 1.5's and United Offensive's snapshot builders send past the PVS, for
`docs/research/cod15-protocol-diff.md` 15 and `coduo-diff.md` 9. UO only,
mp_berlin, its own gametype (no tdm). The one client spawns at the tdm spawn
(1232, 2316), out of the parked Panzer IV's PVS; the script sounds the
Panzer, then a `script_origin` beside it, sets a 500-unit cull fog, then
exponential fog, then the map's fog. The client runs `--probe-edge watch`:

```
COD_VERSION=uo COD_DIR=<UO install> SECS=45 \
    tools/run_probe.sh client-probes/probe_uocull mp_berlin
# second shell, once the map is up:
cargo run -p vcod -- --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:28970 \
    --probe-edge watch --probe-secs 30
```

The retail run of 2026-10-16 is `crates/server/tests/fixtures/vehicles/
uocull-berlin.txt`; `crates/server/tests/uocull_ab.rs` runs the probe on ours
and compares each step's lists.

## probe_snapcull

What the snapshot builder sends an eye in a solid leaf, and what the
compass's `SV_inSnapshot` makes of a teammate in no cluster, of a sounded
one and of one in the PVS past a cull fog (`docs/protocol-1.1.md`, "Which
entities a client is sent"; `docs/research/cod11-hud-protocol.md` 9). 1.1, 1.5 and UO, mp_carentan, own
gametype. Slot 0, the compass viewer, at (976, -864); slot 1, the subject,
at (-992, 2328), out of its PVS, both allies. The script holds the subject
1000 units under the map for three seconds, puts him back and sounds him,
then sets him in slot 0's PVS 1300 units off, sets a 1000-unit cull fog,
sounds him inside it and clears it with exponential fog:

```
[COD_VERSION=1.5|uo] COD_DIR=<install> SECS=60 \
  tools/run_probe.sh client-probes/probe_snapcull mp_carentan
# second shell, once the map is up (UO: add --mod-dir uo):
cargo run -p vcod -- --game-dir <install> --net-probe 127.0.0.1:28970 \
  --probe-compass --probe-team allies --probe-secs 45
# third shell, two seconds later:
cargo run -p vcod -- --game-dir <install> --net-probe 127.0.0.1:28970 \
  --probe-edge watch --probe-secs 43
```

The retail runs of 2026-10-20 are `crates/server/tests/fixtures/entities/
snapcull-carentan-11.txt`, `-15.txt` and `-uo.txt`;
`crates/server/tests/uocull_ab.rs` (1.1, UO) and `cull15_ab.rs` (1.5) run
the probe on ours and compare the compass word and the subject's lists from
the `quiet` step on.

## probe_cull15

`probe_uocull`'s 1.5 twin, on script models instead of a vehicle, for
`cod15-protocol-diff.md` 15. mp_carentan, its own gametype. The client
spawns on the dm spawn (-584, 1872, 8); the script sounds models in and out
of its PVS, sounds two `script_origin`s, loops and stops a sound, sets a
1000-unit cull fog, moves models across it, sounds and loops through it,
then exponential fog and the map's fog:

```
COD_VERSION=1.5 COD_DIR=<stock 1.5 install> SECS=75 \
    tools/run_probe.sh client-probes/probe_cull15 mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --game-dir <stock 1.5 install> --net-probe 127.0.0.1:28970 \
    --probe-edge watch --probe-secs 62
```

The retail run of 2026-10-16 is `crates/server/tests/fixtures/entities/
cull15-carentan.txt`; `crates/server/tests/cull15_ab.rs` runs the probe on
ours and compares each step's lists and when they change.

## probe_re and probe_bel

Stock `re.gsc` and `bel.gsc` on mp_brecourt with the probe driving: the
Retrieval pickup, hold-use drop, timeout return, drop on death, refused
defender pickup and capture, and the Behind Enemy Lines swap on three
scripted kills. The clients are two `--save-scripted <role>` probes, which
press use when the probe's `setClientCvar("probe_use", ...)` says and record
into `crates/server/tests/fixtures/gametypes/`; the server's `games_mp.log`
goes there too, as `mp_brecourt-<gametype>-log.txt`.
`crates/server/tests/gametypes_ab.rs` runs both probes on ours and compares.
Recipe, timeline and findings: `docs/research/cod11-gametypes-re-bel.md`,
sections 7 (re: allies `attacker` first, axis `defender` 3 s later, 150 s)
and 8 (bel: both `--probe-team axis`, `first` then `second`, 130 s).
`probe_bel` logs every spawn into play's origin (`PROBE spawned`) before it
moves the player, which is how the gate replays `bel.gsc`'s random picks
(section 8.7).

## probe_linkto

`linkTo` on script entities. It keeps mp_carentan's bombzone_A brush model
`*5` as probe_ride does and, once an allied player has spawned, links a
script_origin and a script_model to it and moves and turns it, verbs and
unlinks a linked child, runs a link chain in each entity order, deletes a
moving parent, links four script_origins to the player (no tag, a bone, a
tag with zero offsets, the four-argument form with no tag), and ends on a
link cycle whose fatal is the last measurement. Every server frame of a
phase logs `PROBE f`/`g`/`h` lines (the header of the fixture spells them
out). The plain `--net-probe` client's trajectory lines are the wire half.
The 2026-10-08 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkto.txt` and
`-linkto-wire.txt`, which `crates/server/tests/linkto_ab.rs` replays and
`docs/research/cod11-movers.md` 15 reads.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=95 \
    tools/run_probe.sh client-probes/probe_linkto mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-team allies --probe-secs 85
```

## probe_blastmove

What a blast victim's damage callback does to the rest of its walk, for
`docs/research/cod11-combat.md` 14.5. dm on mp_carentan with
`+set scr_forcerespawn 1`; three `--probe-team` clients 2 s apart, then a
fourth `--save-grenade --capture-tag <tag>` client 35 s later (move its
tagged fixture out of `crates/server/tests/fixtures/playerstate/` after the
run). 120 s on the server, `--probe-secs 100` on the three and 60 on the
thrower. Against ours the server half is `vcod-server mp_carentan
--gametype-script crates/gsc/tests/fixtures/semantics/client-probes/probe_blastmove.gsc
--set scr_forcerespawn=1`.

Each row logs `PROBE cb <row> <entity> <iDamage> <sessionstate> <origin>`
per victim and `PROBE cbdone <row> <entity>` once the first victim's action
is over. Retail, 2026-10-08, the walk per row (state lines dropped):

```
plain         cb 1, cb 0, cb 2
move_out      cb 1 (parks 0 and 2)
move_in       cb 1 (pulls 2 in from outside the box), cb 0
ignore_mid    cb 1 (setPlayerIgnoreRadiusDamage(true)), cb 0, cb 2
ignore_after  (none)
nested        cb 1, cb nested_inner 0 5, cbdone 1, cb 0, cb 2
kill          cb 1, cb kill_inner 0 50, cb kill_inner 2 50, cbdone 1,
              cb 0 dead, cb 2 dead
```

The grenade half logs `PROBE gcb first <time> <entity> <iDamage> <origin>`
for the first victim of each grenade walk, which parks the other two, and
`PROBE gcb later ...` for any victim after it. While a grenade is live the
thrower waits out of reach and the three float side by side 60 units over
it, linked in slot order on a frame whose time is a multiple of 100 and in
reverse on the others, so the first victim names the frame whose links the
walk met. Retail, 2026-10-09: `first 62350 0`, `first 72400 2`,
`first 81050 0`, no `later`: this frame's last link each time (combat doc
14.7).

## probe_blastlink

Which links a grenade's blast walk meets, for `docs/research/cod11-combat.md`
14.7 ("One entity pass"). dm on mp_carentan with `+set scr_forcerespawn 1`;
a `--probe-team allies` victim, then 4 s later a `--save-grenade
--capture-tag <tag>` thrower on allies too (the script throws the frag; move
its tagged fixture out of `crates/server/tests/fixtures/playerstate/` after
the run). 110 s on the server, `--probe-secs 95` on the victim and 80 on the
thrower.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29821 SECS=110 \
    tools/run_probe.sh client-probes/probe_blastlink mp_carentan +set scr_forcerespawn 1
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29821 --probe-team allies --probe-secs 95
# third shell, 4 s after that:
cargo run -p vcod -- --net-probe 127.0.0.1:29821 --probe-team allies --save-grenade --capture-tag blastlink --probe-secs 80
```

Against ours the server half is `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_blastlink.gsc --set
scr_forcerespawn=1`. Each damage callback logs `PROBE link <time> <mod>
<inflictor> <child> <off>` per child, `off` 0 when the child was re-anchored
this frame and +/-8 when it still sits on the last frame's. Retail,
2026-10-09: every `MOD_GRENADE_SPLASH` walk read 0 for the children numbered
below the grenade and 8 for the ones above it (grenades 170, 180, 243), and
every `MOD_FALLING` callback 0 for all.

## probe_entnum

Entity numbers and where the entity pass sits against the threads, for
`docs/research/cod11-combat.md` 14.7 ("The pass runs after the threads",
"Entity numbers"). No client needed:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29921 SECS=12 \
    tools/run_probe.sh client-probes/probe_entnum mp_carentan
```

Against ours: `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_entnum.gsc`. Retail,
2026-10-09: `a 1050 299`, `b 1050 301` (the `playFx` took 300), `c_300 302`,
`d_350 303`, `e_400 300`, `f_450 304`; the deleted entity read `isdefined` 1
at +0, +50 and +100 and 0 at +150; `movedone 3650 3800`.

## probe_concnum

`grenadeExplosionEffect`'s two entity numbers, for
`docs/research/cod11-combat.md` 13.5. No client needed:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29941 SECS=12 \
    tools/run_probe.sh client-probes/probe_concnum mp_carentan
```

Retail, 2026-10-09: `a 1050 299`, `b 1050 302`, then one spawn a frame
303..309, 300 at +400, 310..315, 301 at +750, 316 on. Against ours the
table is `grenade_explosion_effect_takes_a_temp_and_a_concussion_number`
(`crates/server/src/game/script.rs`).

## probe_pushorder

The area-tree order a mover's push leaves two players in, for
`docs/research/cod11-movers.md` 12. mp_carentan under dm rules with two
`--probe-team allies` clients; the recipe is in the header of
`crates/server/tests/fixtures/movers/mp_carentan-dm-pushorder.txt`, the
2026-10-09 run, which `crates/server/tests/push_order_ab.rs` replays. Both
players go down in the slab's path, slot 0 then slot 1, and a flat
`radiusDamage` after each `movey` reads the walk off the `cb` lines.

## probe_pushcand

What `G_MoverPush`'s candidate trace lets through besides the pusher, for
`docs/research/cod11-movers.md` 12 ("The candidate trace"). mp_carentan
under dm rules with a `--probe-team spectator` client first, then two
`--probe-team allies --probe-ride` ones; the spectator keeps
`StuckInClient` off so a stacked pair stays stacked, and the allies log
their `CMDS` lines with each send's wall clock (`w=`). The recipe is in the
header of `crates/server/tests/fixtures/movers/mp_carentan-dm-pushcand.txt`,
the 2026-10-20 run, which `crates/server/tests/push_candidate_ab.rs`
replays on those cmds in that packet order.
The slab runs a one-frame `movey` into one player, two stacked, two half
overlapped and one sunk into the street, logging its origin and both
players' every frame.
The same recipe ran on 1.5b and UO (`-1.5.txt`, `-uo.txt`, the headers
carry the `COD_VERSION` and `--game-dir` lines). There the spectator does
not keep `StuckInClient` off and the stacked and half pairs are shoved
apart; those fixtures add each push frame's `RIDE` lines, which the gate
pins in place of `rand()`.

## probe_sine

Whether script reaches a `TR_SINE` mover, for
`docs/research/cod11-movers.md` 12 ("`TR_SINE` and the crush arm"). No
client needed:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30051 SECS=12 \
    tools/run_probe.sh client-probes/probe_sine mp_carentan
```

Retail, 2026-10-10: `spawn("func_bobbing", ...)` and `spawn("func_pendulum",
...)` both return live entities (299 and 300) that move, the bobbing's z
reading 31.90 at 1100; the PROBE lines end at `f 4000` and the console's tail
reads `ERROR: Reached_BinaryMover: bad moverState`. A run that spawned them
at 2050 ended on the same frame. `probe_bobbing` below measures the same
movers further and is the one ours replays.

## probe_bobbing

`SP_func_bobbing` and `SP_func_pendulum` for `docs/research/cod11-movers.md`
18, in three modes the header describes. Five script-spawned movers, no
client, up to the drop:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30151 SECS=10 \
    tools/run_probe.sh client-probes/probe_bobbing mp_carentan
```

A spawned pair on the wire: the client first, since the bobbing drops the
level 4 s into the map and a map change does not restart the clock.

```
cargo run -p vcod -- --net-probe 127.0.0.1:30151 --probe-ride --probe-secs 25
# second shell, about a second later:
COD_LNXDED_HOME=<absolute, no '+'> PORT=30151 SECS=20 \
    tools/run_probe.sh client-probes/probe_bobbing mp_carentan +set probe_wire 1
```

A map-placed pair: `*5` and `*6` rewritten (the keys are in the header of
`crates/server/tests/fixtures/movers/mp_carentan-dm-sine-map.txt`;
`sine_ab.rs`'s `sine_bsp` builds the same bsp), the new lump appended and
lump 29's directory entry pointed at it, the bsp in a `zzz_sine.pk3` in the
server's homepath as `maps/mp/mp_carentan.bsp`, then the first recipe with
SECS=40 and the client about 5 s after. The 2026-10-11 runs are
`crates/server/tests/fixtures/movers/mp_carentan-dm-sine.txt`,
`-sine-map.txt` and `-sine-wire.txt`; `crates/server/tests/sine_ab.rs`
replays all three. `run_probe.sh` prints the console's `ERROR:` line as
`PROBE_ERROR`.

## probe_spawnclip

Whether a script-spawned brush entity clips, for
`docs/research/cod11-movers.md` 19: `bulletTrace`s onto the first dm spawn
point's floor around a `script_brushmodel` spawned at (0, 0, 64), moved,
made non-solid and solid and deleted, then under a spawned `func_bobbing`
until the level drops. No client:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30241 SECS=14 \
    tools/run_probe.sh client-probes/probe_spawnclip mp_carentan
```

The 2026-10-12 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-spawnclip.txt`;
`crates/server/tests/spawnclip_ab.rs` replays it.

## probe_worldcopy

A script-spawned `script_brushmodel`, a copy of the world's clip, on the
wire and as a pusher, for `docs/research/cod11-movers.md` 20. Once a
player is playing it stands it on the street at (-512, 2688), drops a
health pack 40 units east, spawns the brush model at (0, 0, -64) and lifts
it 96, slides it 32 east, sinks it 96 and deletes it, then spawns and
deletes a `func_static`. Every frame of a phase logs `PROBE f <phase>
<time> <player origin> <pack origin> <copy origin>`. The `--probe-ride`
client's `RIDE_NEW` lines carry each entity as it entered the snapshot.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33631 SECS=45 \
    tools/run_probe.sh client-probes/probe_worldcopy mp_carentan
# second shell, about 6 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33631 --probe-team allies --probe-ride --probe-secs 38
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-worldcopy.txt` (the
`PROBE` lines, then the client's `CMDS` lines) and `-worldcopy-wire.txt`
(the client's `RIDE` and trajectory lines); `crates/server/tests/worldcopy_ab.rs` and
`predict_ab.rs`'s `predictor_rides_a_retail_world_copy` replay them.

## probe_worldcopy2

A turned world copy and a damaged `func_static`, for
`docs/research/cod11-movers.md` 21. The player and a health pack on the
street as in `probe_worldcopy`; a `script_brushmodel` at
(-576, 2688, -75.875) lifts them on its terrain hill, turns 45 and 45 in
yaw, back 90, pitches 5 and back, sinks and is deleted, logging `PROBE f`
per frame and a 3 x 3 grid of `PROBE tr` traces at each rest. Then three
`func_static`s (spawnflags 0, 2, 4) and three `radiusDamage`s, with every
`damage`, `death` and `trigger` notify logged.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33632 SECS=60 \
    tools/run_probe.sh client-probes/probe_worldcopy2 mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33632 --probe-team allies --probe-ride --probe-secs 50
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-worldcopy2.txt`, the `PROBE` lines and then the
client's `CMDS` lines (`--probe-ride` logs them);
`crates/server/tests/worldcopy_ab.rs` replays it.

## probe_worldcopy3

`probe_worldcopy2`'s copy and verbs to the end of the unpitch, without the
grids and the `func_static`s, for `docs/research/cod11-movers.md` 21 ("The
turned centre"). Besides each `PROBE f` it logs a `PROBE g` line per frame:
the player's offset from the copy's spawn origin, three floats the engine
prints `%g`, so to the ten-thousandth where `PROBE f`'s vector stops at the
hundredth.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33633 SECS=55 \
    tools/run_probe.sh client-probes/probe_worldcopy3 mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33633 --probe-team allies --probe-ride --probe-secs 45
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-worldcopy3.txt`, the `PROBE` lines and then the
client's `CMDS` lines (`--probe-ride` logs them);
`crates/server/tests/worldcopy_ab.rs` replays it.

The same recipe on 1.5b and UO (2026-10-21, ports 34591 and 34592):
`COD_VERSION=1.5` or `COD_VERSION=uo` before `tools/run_probe.sh`, and
`vcod --game-dir <install>` (`--mod-dir uo` for UO) for the client, gave
`mp_carentan-dm-worldcopy3-1.5.txt` and `-worldcopy3-uo.txt`, the carry's
rounding on those builds (`docs/research/cod11-movers.md` 21).

## probe_turnedslab

A turned submodel against `G_MoverPush`'s box tests, for
`docs/research/cod11-movers.md` 22. mp_harbor's `bel` slab `*9` is kept
from `_gameobjects`, turned 45 degrees and moved onto an open floor in one
frame; the player is stood 25, 22, 20 and 18 from its centre along the
turned face's normal, and at each the slab lifts 2 and comes back. A `PROBE
f` and a `PROBE g` line per frame.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33634 SECS=40 \
    tools/run_probe.sh client-probes/probe_turnedslab mp_harbor
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33634 --probe-team allies --probe-ride --probe-secs 30
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_harbor-dm-turnedslab.txt`, the `PROBE` lines and then the
client's `CMDS` lines (`--probe-ride` logs them);
`crates/server/tests/worldcopy_ab.rs` replays it.

## probe_tiltslab

A player's capsule against a pitched and a rolled submodel, for
`docs/research/cod11-movers.md` 22. The same `bel` slab is laid in four
poses over the same floor: pitched 90 (lying flat), rolled 90 (on its
side), pitched 60 and rolled -60 (a 30-degree face). In each the player is
stood at z 110 over it and falls onto its upward face. A `PROBE r` line
per pose (the slab's origin and angles less their targets, in
ten-thousandths and millionths of a degree), then a `PROBE f` and a
`PROBE g` line per frame.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33882 SECS=45 \
    tools/run_probe.sh client-probes/probe_tiltslab mp_harbor
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33882 --probe-team allies --probe-ride --probe-secs 32
```

The 2026-10-20 runs are
`crates/server/tests/fixtures/movers/mp_harbor-dm-tiltslab.txt` (before the
`PROBE r` line) and `-tiltslab-ride.txt`, each the `PROBE` lines and then the
client's `CMDS` lines (`--probe-ride` logs them);
`crates/server/tests/worldcopy_ab.rs` replays both. The same recipe under
`COD_VERSION=1.5` and `COD_VERSION=uo`, the client given the install as
`--game-dir` (UO's with `--mod-dir uo`), wrote `-tiltslab-1.5.txt` and
`-tiltslab-uo.txt` (2026-10-21), replayed by the same file.

## probe_moverreads

Mover script reads, for `docs/research/cod11-movers.md` 23, no client:
`.speed` and `.dmg` on a script-spawned `func_bobbing` and
`func_pendulum` (deleted in the frame they spawn, before their period
drops the level) and on a `script_model`; `.origin` around a
`movegravity` started on a running `movez`, on a running arc and at rest;
`.angles` through rotates that cross 0 and 360, a `rotatevelocity` and a
`rotateto` the near way round.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=33881 SECS=20 \
    tools/run_probe.sh client-probes/probe_moverreads mp_carentan
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-moverreads.txt`;
`crates/server/tests/ride_ab.rs` replays it. The same recipe under
`COD_VERSION=1.5` and `COD_VERSION=uo` wrote `-moverreads-1.5.txt` and
`-moverreads-uo.txt` (2026-10-20), replayed by the same file.

## probe_sinecrush

`G_MoverPush`'s `TR_SINE` crush, for `docs/research/cod11-movers.md` 21. A
`func_bobbing` spawned on the first frame swings its copy of the world, and
the player is stood under a sloped ceiling at (752, -40) at 2400, where the
sinking copy's ceiling crushes it. The level drops at 4000, so the client has
to be playing by 2.4 s of level time: start it straight after the server.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=34211 SECS=20 \
    tools/run_probe.sh client-probes/probe_sinecrush mp_carentan &
sleep 0.3; cargo run -p vcod -- --net-probe 127.0.0.1:34211 --probe-team allies --probe-ride --probe-secs 15
```

The 2026-10-20 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-sinecrush.txt`, `PROBE`
lines and then the client's `CMDS` lines (`--probe-ride` logs them);
`crates/server/tests/worldcopy_ab.rs` replays it.

## probe_hurtfall

A `trigger_hurt` kill and the corpse's fall, for
`docs/research/cod11-combat.md` 5.8. It lifts mp_carentan's kill slab
(entity 72) 400 by an origin write on the first frame (`probe_lift` sets
another lift; `probe_late_lift` moves it again at the drop), stands the
first playing client 400 over the street at (900 1930) at 2400, and logs
`PROBE f <time> <origin> <health> <sessionstate>` every frame, the damage
and killed callbacks' arguments, and `PROBE done` 3 s after the kill. As
with `probe_sinecrush`, the client has to be playing by 2.4 s of level time.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=34331 SECS=20 \
    tools/run_probe.sh client-probes/probe_hurtfall mp_carentan &
sleep 0.3; cargo run -p vcod -- --net-probe 127.0.0.1:34331 --probe-team allies --probe-ride --probe-secs 15
```

The 2026-10-20 runs are `crates/server/tests/fixtures/triggers/mp_carentan-dm-hurtfall.txt`
(no `+set`), `-hurtlow.txt` (`+set probe_late_lift 203`) and `-hurtland.txt`
(`+set probe_lift 120`); `crates/server/tests/hurtfall_ab.rs` replays them.

`probe_drop` drops from another z, `probe_spawn <spawnflags>` spawns a
second `trigger_hurt` by script at the drop, and `probe_dmg` writes the
`dmg` of that one or of the slab; under either the damage callback logs the
number a fresh `script_origin` takes (`PROBE spawned`). The 2026-10-21 runs,
on 1.1d, 1.5b (`COD_VERSION=1.5`, a `-1.5` suffix) and UO (`COD_VERSION=uo`,
`-uo`), are `mp_carentan-dm-hurtrate*.txt` (`+set probe_lift 120 +set
probe_dmg 30 +set probe_drop -30`), `-hurtslow*.txt` (`+set probe_spawn 16
+set probe_dmg 25 +set probe_drop -30`) and `-hurtonce*.txt` (`+set
probe_spawn 44 +set probe_dmg 10 +set probe_drop -30`), each with a
`--probe-secs 12` client; `docs/research/cod11-combat.md` 5.9 reads them.

`probe_use <ms>` logs every `"trigger"` on the spawned hurt and calls its
`useBy` with the player `<ms>` and twice that after the drop; `probe_write 1`
writes the spawned hurt's `spawnflags`, `classname` and `model`, one thread
each. A read-only write is fatal at `developer 1`, so that run takes
`PRE_MAP_ARGS='+set developer 0'`. The 2026-10-21 runs on all three servers
are `mp_carentan-dm-hurtuse*.txt` (`+set probe_spawn 17 +set probe_dmg 25
+set probe_drop -30 +set probe_use 1000`) and `-hurtwrite*.txt` (`+set
probe_spawn 17 +set probe_drop -30 +set probe_write 1`); UO's write run
ends at the first write, where the server aborts. The 1.5b write run was
taken again on port 34770 the same day, its first take's body having
drifted sideways.

`probe_spawn_x`, `probe_spawn_y` and `probe_spawn_z` put the spawned hurt
at that point instead of (900 1930 -20). The 2026-10-21 runs on all three
servers, `mp_carentan-dm-hurtfar*.txt` (`+set probe_spawn 16 +set
probe_dmg 25 +set probe_drop -30 +set probe_spawn_x 0 +set probe_spawn_y 0
+set probe_spawn_z 8`), spawn it 1900 units from the body; its inline
model 0, the world lifted 8, still hurts the body on the street.

## probe_useby

`useBy` on the entities whose `use` is not `hurt_use`
(`docs/research/cod11-combat.md` 5.9). No client. It spawns a
`trigger_once`, a `trigger_multiple`, a `trigger_damage` and a
`func_static` at the world origin in turn, lifts the static 8 by an origin
write, calls `useBy` with a `script_origin` and logs every `"trigger"`
notify, `isdefined` for a few frames, and for the static a `bulletTrace`
down on to the street at (900 1930). Under `+set probe_life 1` it runs only
the `life` phase: `useBy` on mp_carentan's `trigger_hurt` and `isdefined`
for 3 s, which needs the slab's `"dmg" "1000"` replaced by `"life" "2.5"`
(same length) in a `zzz_hurtlife.pk3` in the homepath's game dir
(`uo/` on UO); `hurtlife_bsp` in `crates/server/tests/hurtfall_ab.rs`
applies the same edit. The 2026-10-21 runs on 1.1d, 1.5b and UO are
`crates/server/tests/fixtures/triggers/mp_carentan-dm-useby*.txt` and
`-hurtlife*.txt`; `hurtfall_ab.rs` replays them.

Three more runs, each alone: `+set probe_sine 1` uses a script-spawned
`func_bobbing` and `func_pendulum` twice each and logs their `origin` and
`angles` every frame (`-usesine*.txt`); `+set probe_spawner 1` uses a
script-spawned `misc_spawner` and the map's, which needs mp_carentan's
`mp_retrieval_spawn_axis` at (904 2896 100) made a `misc_spawner` of
`item_health` in a `zzz_spawner.pk3` (`spawner_bsp` in `hurtfall_ab.rs`),
twice each, and logs every `item_health` every frame (`-usespawner*.txt`);
`+set probe_turret 1` waits for a `--probe-ride` client, puts him behind
the gun at (1712 1830 8) and uses the gun with him (`-useturret*.txt`, the
client's `CMDS` lines after the `PROBE` ones). `+set probe_turret 2` uses
the gun with the `script_origin`, which segfaults all three servers.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=34761 SECS=15 \
    tools/run_probe.sh client-probes/probe_useby mp_carentan [+set probe_life 1]
```

## probe_linkto2

`linkTo`'s second round on mp_carentan. Once an allied player has spawned it
moves the bombzone_A `trigger_multiple` onto the player by an origin write,
then calls `enableLinkTo` on it and links it to a script_origin that carries it
there and back, logging every `trigger` notify. It links the `misc_mg42` 297
to a script_origin and lifts and turns it, links two script_origins to a
script_model (one on `bip01 head`, one on the model's own frame) and changes
the model four times, links an item the frame it spawns in mid-air and one
that has landed, lifts both and unlinks them, and ends on `enableLinkTo` on a
script_origin, whose fatal is the last measurement. The 2026-10-08 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-linkto2.txt` and
`-linkto2-wire.txt`; `crates/server/tests/linkto_ab.rs` replays it and
`docs/research/cod11-movers.md` 16 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29581 PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_linkto2 mp_carentan
# second shell, about 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29581 --probe-team allies --probe-secs 64
```

## probe_trigwait

`Touch_Multi`'s `wait` arm on mp_carentan with four same-length entity lump
edits (`wait` keys on three `trigger_multiple`s, auto2 turned into a
`trigger_once`; the fixture header lists them, and `trigwait_bsp` in
`crates/server/tests/linkto_ab.rs` applies them). `wait` is a gsc keyword, so
the field cannot be written from script; the patched bsp goes into a
`zzz_trigwait.pk3` in the server's homepath, which overrides pak4. The probe
moves one trigger at a time onto the parked player for 20 frames and logs
`isdefined`, the trigger count and every `trigger` notify, then ends on
`enableLinkTo` on a script_origin. The 2026-10-09 run is
`crates/server/tests/fixtures/movers/mp_carentan-dm-trigwait.txt`;
`linkto_ab.rs` replays it and `docs/research/cod11-movers.md` 17 reads it.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29671 PROBE_SECS=55 \
    tools/run_probe.sh client-probes/probe_trigwait mp_carentan
# second shell, about 10 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:29671 --probe-team allies --probe-secs 40
```

## probe_startclock

The script clock from load (`docs/research/cod11-gsc-language.md`, "The
script clock"). No client needed. Logs `getTime` at the gametype's `main`
and at `Callback_StartGameType`, then a `wait 0.05` loop, a `wait 1` pair, a
`wait 0` and a `wait 0.1` / `wait 0.2` pair started from the callback. The
2026-10-09 retail run on mp_carentan:

```
PROBE main 0
PROBE start 0
PROBE wait0 100
PROBE tick 0 100
PROBE tick 1 200
PROBE wait01 200
PROBE tick 2 200
PROBE tick 3 300
PROBE tick 4 300
PROBE tick 5 350
PROBE wait02 350
PROBE tick 6 400
...
PROBE tick 11 650
PROBE wait1 1050
PROBE wait1b 2050
```

`crates/server/src/server.rs`'s `the_script_clock_matches_probe_startclock`
replays it on ours.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=29931 SECS=12 \
    tools/run_probe.sh client-probes/probe_startclock mp_carentan
```

## probe_wedge

Where a body comes to rest in a terrain crease on mp_hurtgen. Under
`probe_teleport 1` it respawns each allied player (`spawn`, so at rest; a
`setOrigin` keeps the walk's velocity) at five spots on and above the crease
at (436.8, 2824.3, -97.2), 3 s apart, and logs `PROBE f <time> <trial>
<origin>` every frame for 2 s. The client half is `probe_fall`'s, standing
still or with `--probe-fall-walk <yaw>`. The 2026-10-12 runs are read in
`docs/research/bot-navigation.md` section 3 ("mp_hurtgen's crease").

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30411 PROBE_SECS=55 \
    tools/run_probe.sh client-probes/probe_wedge mp_hurtgen +set probe_teleport 1
# second shell, about 9 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:30411 --probe-team allies --probe-fall \
    [--probe-fall-walk 45] --probe-secs 40
```

## probe_shipcrease

Whether a body at rest in mp_ship's shelf corners at (5375.1, 321.1, 352.1)
and (5375.1, -193.1, 352.1) runs free. probe_wedge's shape on mp_ship: under
`probe_teleport 1` it respawns each allied player at the two corners, 3 s
apart, and logs `PROBE f <time> <trial> <origin>` every frame for 2 s. The
2026-10-18 runs at yaw 140, 200 and 225 are read in
`docs/research/bot-navigation.md` section 3 ("The corners at x 5375.1").

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30411 PROBE_SECS=32 \
    tools/run_probe.sh client-probes/probe_shipcrease mp_ship +set probe_teleport 1
# second shell, about 9 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:30411 --probe-team allies --probe-fall \
    --probe-fall-walk 200 --probe-secs 22
```

## probe_restart

When the first snapshot after a `map_restart` reaches a client that was in
the world. dm, plus a thread that waits for a player, waits `probe_wait`
seconds (default 3), logs `PROBE restart <n> <time> <persist>` and calls
`map_restart(probe_persist)`, `probe_count` times (default 3). `probe_play
1` puts the client in play rather than leaving it a spectator. The client
half is `--probe-edge watch`; the snapshot whose `serverTime` jumps past the
logged restart time is the measurement. The 2026-10-12 runs are read in
`docs/research/cod11-map-cycle.md` 4.7. Against ours, `vcod-server
mp_carentan --gametype-script <this file> --set probe_persist=1`.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30541 SECS=28 \
    tools/run_probe.sh client-probes/probe_restart mp_carentan +set probe_persist 1
# second shell, about 5 s later:
cargo run --release -p vcod -- --net-probe 127.0.0.1:30541 --probe-edge watch --probe-secs 20
```

## probe_legs

The bones of a walking player. Once an allied player plays and the level
clock reads 25000 it puts the player on mp_carentan's flat bottom floor at
`(932, -376, -151.875)`, 145 units under the garden terrain there (the
`probe_bump` and `probe_passthru` floor; a view from it shows the town from
below), links script_origins to `bip01 l foot`,
`bip01 r foot`, `tag_weapon_right` and `bip01 head` with zero offsets, and
logs the player's origin and the four every server frame (`PROBE f`/`h`).
The `--save-legs` client waits for the placement and walks a fixed script at
world yaw 0 (run, stop, backpedal, strafes, diagonals, crouch walk), writing
every cmd and snapshot. The mark is an absolute time so a replay starts the
walk on the same level time: a movement clip's first phase is read off it.
The 2026-10-12 run is `crates/server/tests/fixtures/movers/mp_carentan-dm-legs.txt`
(server half, header added by hand) and `-legs-client.txt`, which
`crates/server/tests/legs_ab.rs` replays and `docs/research/cod11-movers.md`
15, "A moving body's clip", reads.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30701 PROBE_SECS=75 \
    tools/run_probe.sh client-probes/probe_legs mp_carentan > legs.txt
# second shell, about 6 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:30701 --probe-team allies --save-legs --probe-secs 70
```

## probe_playfx

Map effects on the wire. It loads two effects, puts the first allied player
on mp_carentan's street at `(900, 1930, -38)` and a second later plays
`playFx` plain, with a forward and out of view, three `playLoopedFx`
(no cull, cull and forward, far off), a `playFxOnTag` on the player's
`tag_helmet`, then deletes the looped ones, logging each step's time
(`PROBE <step> <time> [<entity>]`). The `--save-scripted fx` client writes an
`!fx` line per effect event and per `ET_FX` entity as it comes, changes and
goes. The 2026-10-13 run is
`crates/server/tests/fixtures/gametypes/mp_carentan-playfx-fx.txt` and the
log's `PROBE` lines `-playfx-log.txt`, which `crates/server/tests/fx_ab.rs`
compares; `docs/research/cod11-events-and-fx.md` 2, "On the wire, measured",
reads them.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30751 SECS=40 \
    tools/run_probe.sh client-probes/probe_playfx mp_carentan > playfx.txt
# second shell, about 5 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:30751 --probe-team allies --save-scripted fx --probe-secs 25
```

## probe_soundfade

`musicPlay`, `musicStop` and `soundFade` on the wire, paced for a listener
too: the steps are in the probe's header. Run 2026-10-13 against the retail
1.1d server on mp_carentan with one `--probe-team allies` client, whose
`JOIN cmd:` lines read

```
o codtheme
q 0.200000 3000
q 1.000000 2000
p 10000
q 0.000000 2000
q 1.000000 0
```

with a newline closing each `q` (the `.rodata` format `0x77568` carries it)
and none after `o` or `p`. vcod's server, run on the same probe through
`--gametype-script`, sends the same bytes.
`docs/research/cod11-sound-system.md` 9 ("Live verification") and 10
("`soundFade`") read it.

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=30761 tools/run_server.sh mp_carentan \
    +set g_gametype probe_soundfade +set g_log games_mp.log +set g_logSync 1
# with the probe and a probe_soundfade.txt installed under the homepath's
# main/maps/mp/gametypes, as in "Running one"; second shell:
cargo run -p vcod -- --net-probe 127.0.0.1:30761 --probe-team allies --probe-secs 50
```


## probe_complaint

The teamkill complaint `allowComplaint` opens and `vote` answers, for
`docs/research/cod11-votes.md` 3.1. Two plain probes, both axis: the first
to begin is slot 0, the offender, the second the victim, whose window on
slot 0 the script opens 3 s after both are in and every 5 s after; it calls
`allowComplaint` on the victim itself first, which sends nothing. Every
`m` and `e` line prints as `serverCommand`.

```
COD_LNXDED_HOME=<absolute, no '+'> PROBE_SECS=50 tools/run_probe.sh client-probes/probe_complaint mp_carentan
# second shell, once the map is up:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-name Able --probe-secs 38
# third shell, two seconds later:
cargo run -p vcod -- --net-probe 127.0.0.1:28970 --probe-name Baker --probe-secs 36 \
    --probe-say "2.5:vote yes" --probe-say "5.0:vote no" --probe-say "6.0:vote yes" \
    --probe-say "10.0:vote yes" --probe-say "15.0:vote yes" --probe-say "20.0:vote yes" \
    --probe-say "26.0:vote yes"
```

The second run gives Able `--probe-secs 17` and Baker only `10.0:vote yes`
and `18.0:vote yes`, so Able leaves with a window open on it. Against ours:
`vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_complaint.gsc`.
Retail and ours, 2026-10-14, print the same lines in both probes.

## probe_patchshot and probe_patchseams

Server-only probes: `bulletTrace(from, to, ...)` along fixed segments on
mp_carentan, one `PROBE` line each, no client needed. `probe_patchshot`
traces the 1.5b turret round `turret_ab` replays at 30000 and 24 segments
around it, plus a few drops onto the same patches. `probe_patchseams`
traces the 790 segments of `crates/common/tests/fixtures/patch/mp_carentan-seams.txt`,
aimed at the facet seams of mp_carentan's patches, and prints each fraction
and `(fraction - base) * 1000000` against the line's base, which tells two
floats a bit apart. The script file is generated with the fixture: the
segments are literals, fifty to a function, a `wait 0.05` between
functions.

```
PORT=<p> SECS=20 COD_DIR=<1.1 install> tools/run_probe.sh client-probes/probe_patchseams mp_carentan
PORT=<p> SECS=20 COD_VERSION=1.5 COD_DIR=<1.5 install> tools/run_probe.sh client-probes/probe_patchseams mp_carentan
```

`crates/common/src/collision.rs`'s `patch_seams_clip_as_each_retail_build`
gates ours against both columns (`bsp-ibsp59-format.md`, "Patch collision
on 1.5 and UO").

## probe_noclass

What a script reads off a client between its connect and its first
spawn: the fields at the connect callback, 3 s later, and straight after
a `spawn` at the first `mp_deathmatch_spawn`. One client:

```
PORT=<p> COD_DIR=<install> [COD_VERSION=1.5|uo] tools/run_probe.sh client-probes/probe_noclass mp_carentan
vcod [--game-dir <install> [--mod-dir uo]] --net-probe 127.0.0.1:<p> --probe-secs 8
```

The fixtures are `crates/server/tests/fixtures/noclass/`, gated by
`crates/server/tests/noclass_ab.rs` (cod11-gametypes-re-bel.md 8.5).
Against ours: `vcod-server mp_carentan --gametype-script
crates/gsc/tests/fixtures/semantics/client-probes/probe_noclass.gsc`.

## probe_spawnview

The `origin` and `angles` a script reads straight after `spawn`, times
1000: mp_carentan's first deathmatch spot at several heights and angles,
then a spectator's and an intermission camera's spawn straight after a
player's. One client:

```
PORT=<p> COD_DIR=<install> [COD_VERSION=1.5|uo] tools/run_probe.sh client-probes/probe_spawnview mp_carentan
vcod [--game-dir <install> [--mod-dir uo]] --net-probe 127.0.0.1:<p> --probe-secs 14
```

The fixtures are `crates/server/tests/fixtures/spawnview/`, gated by
`crates/server/tests/spawnview_ab.rs` (cod11-gametypes-re-bel.md 8.6). The
`next` lines ride the probe client's cmds and the gate skips them.

## probe_yawres

`.angles` of mp_carentan's bombzone_A brush model `*5` times 1e9, every
frame, through a `rotateyaw` pair of 2 and of 10 degrees and a `movey`
after each. It needs no client; it lives here because it needs
mp_carentan's brush model and a `crates/server` gate, not
`retail-captures.txt`:

```
PORT=<p> COD_DIR=<install> SECS=22 tools/run_probe.sh client-probes/probe_yawres mp_carentan
```

The fixture is `crates/server/tests/fixtures/movers/mp_carentan-dm-yawres.txt`,
gated by `ride_ab.rs`'s `a_rotated_brush_model_keeps_retails_quantized_angles`
(cod11-movers.md 12, "The residual yaw").

## probe_crush

probe_ride's crush alone, held for 8 s: mp_carentan's bombzone_A slab `*5`
lowered onto the first allied player, then a teleport and a `moveto` in the
same frame. Every frame logs `PROBE f <phase> <time> <player origin> <mover
origin>`. Run the client on a jittered link so some frames go without a cmd:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=45 \
    tools/run_probe.sh client-probes/probe_crush mp_carentan
# second shell, about 6 s later:
VCOD_NETSIM="ping=40,jitter=70" cargo run -p vcod -- --net-probe 127.0.0.1:<p> \
    --probe-ride --probe-team allies --probe-secs 34
```

The 2026-10-20 run (port 34213) is
`crates/server/tests/fixtures/movers/mp_carentan-dm-crush.txt`: the `PROBE`
lines, then the client's `RIDE` and trajectory lines and its `CMDS` lines,
which carry the `commandTime` of every snapshot it parsed. `ride_ab.rs`'s
`a_blocked_slab_moves_on_a_frame_without_a_cmd_like_retail` replays the
cmds (cod11-movers.md 12, "A blocked mover").

## probe_anglesnap

`.angles` of a script model, a script origin, a suspended `item_health` and
mp_carentan's bombzone_A brush model `*5`, times 1e9, after an `.angles`
write, after the `.origin` write that links them and a frame later. No
client; it lives here for the same reason as probe_yawres:

```
PORT=<p> COD_DIR=<install> SECS=12 tools/run_probe.sh client-probes/probe_anglesnap mp_carentan
```

The fixture is `crates/server/tests/fixtures/movers/mp_carentan-dm-anglesnap.txt`
(1.1d; 1.5b and UO 1.51b print the same lines), gated by `ride_ab.rs`'s
`every_link_snaps_angles_like_retail` (cod11-movers.md 12, "The snap on
every link").

## probe_leanhit

A leaning target's hit pose and a leaning shooter's rounds, for
`docs/research/cod11-combat.md` 3.6. Two `--probe-uo-weapon direct`
clients, an axis target and an allies `springfield_mp` shooter 256 units
apart; the gsc sends each its buttons, lean and stance through
`iPrintLn("PROBE_IN ...")` and runs 13 phases from level time 20000: the
target standing, crouched and prone, each level and leaning both ways, then
the shooter leaning both ways. Each phase logs a grid of `bulletTrace`
cells across the target and the damage of one sighted round. The recipe is
in the header of `crates/server/tests/fixtures/leanhit/mp_carentan-leanhit.txt`,
gated by `crates/server/tests/leanhit_ab.rs`.

Two things its first runs taught: the target's torso yaw stays where his
spawn point left it until something swings it (16.4), so the probe lays him
prone and stands him up before the phases; and a stance change needs the
ground behind him clear, so the site asks for 80 level units past his spot.

## probe_crouchjump

United Offensive's spawn rooms whose only way out is a window too low for a
standing body (`docs/research/bot-navigation.md`, "Walled-in spawns"). Under
`probe_teleport 1` it puts every playing client facing the window:
`probe_spot cassino` (mp_cassino's axis room), `streets` (mp_streets' axis
room at z 101) or `low` (mp_streets' room at z -24). The client runs
`--save-slope --probe-crouchjump <yaw>`: a run at the wall, a jump, and
crouch held in the air for a second; `--probe-crouchjump-stand` is the same
without the crouch.

```
COD_VERSION=uo PORT=<p> COD_DIR=<uo install> SECS=45 tools/run_probe.sh client-probes/probe_crouchjump mp_cassino +set probe_teleport 1 +set probe_spot cassino
# second shell, about 12 s later (yaw -128 cassino, 180 streets, -90 low):
vcod --game-dir <uo install> --mod-dir uo --net-probe 127.0.0.1:<p> --save-slope --probe-crouchjump=-128 --probe-cmd-ms 50 --capture-tag crouchjump-cassino --probe-secs 28
```

The twelve fixtures (8 and 50 ms cmds, crouched and standing, three rooms)
are `crates/server/tests/fixtures/playerstate/*-crouchjump-*.txt`, gated by
`moves_ab.rs`'s `the_predictor_replays_uo_jumps_through_low_windows`.

## probe_moveshot

Shots whose 1.5 or United Offensive muzzle moves (`docs/research/cod11-combat.md`
2.1, "1.5 and United Offensive"). Stock dm on mp_carentan; under
`probe_teleport 1` it puts every allied player facing world yaw 0:
`probe_spot street` is probe_prone's street at (900 1930), the wall 1090
units ahead; `wall` is the ground 20 units (`probe_walldist`) short of that
wall (face x 1991.875, floor z -23.875). The client runs `--save-slope
--probe-moveshot walk` (a thompson: bursts strafing, running, crouched,
sighted and leaning) or `bipod` (UO, an mg30cal: bursts from the bipod with
the view along it and 30 degrees off each side, then the trigger held prone
with the bipod up, which fires nothing on retail). Each server's homepath
holds the weapon file with `crates/server/tests/common/mod.rs`
`STEADY_WEAPON` applied, so a round leaves along the view.

```
COD_VERSION=uo COD_LNXDED_HOME=<home with uo/weapons/mp/thompson_mp> PORT=<p> COD_DIR=<uo install> SECS=80 tools/run_probe.sh client-probes/probe_moveshot mp_carentan +set probe_teleport 1 +set probe_spot street
# second shell, about 6 s later:
vcod --game-dir <uo install> --mod-dir uo --net-probe 127.0.0.1:<p> --probe-team allies --probe-weapon thompson_mp --save-slope --probe-moveshot walk --probe-cmd-ms 8 --capture-tag moveshot-walk-uo --probe-secs 65
```

The fixtures are `crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-moveshot-*.txt`
(walk on 1.5b and UO, bipod on UO), gated by `crates/server/tests/moveshot_ab.rs`.
## probe_patchwalk

Player movement against mp_carentan's lump 24 patches
(`docs/research/bsp-ibsp59-format.md`, "Patch collision on 1.5 and UO").
Under `probe_teleport 1` puts every playing client beside a patch, facing
it: `probe_spot pole` (patch 4, yaw 45), `pillar` (patch 11 on terrain,
yaw 0) or `kerb` (the posts round the square, patch 98, yaw 180). The
client runs `--save-slope --probe-patchwalk <yaw>`: into the patch, along
it both ways, at it turned 30 degrees, a running jump, crouched along it,
a crawl into and along it and a stand. Run on each server, one at a time:

```
[COD_VERSION=1.5|uo] PORT=<p> COD_DIR=<install> SECS=52 tools/run_probe.sh client-probes/probe_patchwalk mp_carentan +set probe_teleport 1 +set probe_spot pole
# second shell, about 12 s later (45 pole, 0 pillar, 180 kerb; on UO add --game-dir <uo install> --mod-dir uo):
vcod --net-probe 127.0.0.1:<p> --save-slope --probe-patchwalk=45 --probe-cmd-ms 8 --capture-tag patchwalk-pole-1.1 --probe-secs 34
```

The nine captures are
`crates/server/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-patchwalk-<spot>-<1.1|1.5|uo>.txt`,
which `moves_ab.rs`'s `the_predictor_replays_walks_against_patches`
replays under each build's rules.

## probe_dyingpose

A dying body's bones and its corpse's ring, `docs/research/cod11-combat.md`
16.4 ("The dying body") and 5.2 ("The corpse as a script entity"). dm with
its killed callback replaced: the first player to spawn is put on
mp_carentan's flat prone spot, eight bones get a linked script_origin, and
every frame for 0.5 s alive and 2.5 s after `suicide()` the script logs
them. The killed callback takes the dead session, clones the corpse and
plays `MP_bomb_plant` on it. The client holds world yaw 90 with
`--probe-yaw` (the plain probe holds 0, which hides the dead body's turn)
and writes the snapshot pair the corpse rides:

```
COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=30 \
  tools/run_probe.sh client-probes/probe_dyingpose mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team axis --probe-yaw 90 \
  --save-snapshots --capture-tag dyingpose --probe-secs 18
```

The fixtures `crates/server/tests/fixtures/dyingpose/mp_carentan-dm.txt`
and `crates/common/tests/fixtures/net/{gamestate,snapshots}-dyingpose.bin`,
gated by `crates/server/tests/dyingpose_ab.rs`. The death clip is a
`rand()` draw: the gate seeds ours to draw retail's `legsAnim` 20.
`COD_VERSION=1.5` and `COD_VERSION=uo` ran it too (the client with
`--game-dir`, plus `--mod-dir uo` on UO, and `--capture-tag
dyingpose-1.5` / `dyingpose-uo`): `mp_carentan-dm-1.5.txt` and `-uo.txt`,
`legsAnim` 16 and 18.

## probe_corpseblast

A corpse born in the entity pass, `docs/research/cod11-combat.md` 5.2. dm
with its killed callback wrapped to log its `getTime()`: the first player to
spawn is put on mp_carentan's flat prone spot looking down at 70, handed a
frag and left on 1 health, and a `--probe-uo-weapon throw` client drops it
at his feet. Any patch:

```
COD_LNXDED_HOME=<abs> PORT=<p> SECS=35 tools/run_probe.sh client-probes/probe_corpseblast mp_carentan
# second shell, about 7 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team axis --probe-uo-weapon throw \
  --save-snapshots --capture-tag corpseblast --probe-secs 22
```

The fixtures `crates/server/tests/fixtures/dyingpose/mp_carentan-dm-corpseblast*.txt`
and `net/{gamestate,snapshots}-corpseblast*.bin`, gated by
`dyingpose_ab::a_blast_corpse_keeps_its_fresh_flag_like_*`.

## probe_dmgtrig, probe_dmgshot, probe_dmgtouch, probe_belltower, probe_dmgspawn

`trigger_damage` on United Offensive's mp_uo_stanjel
(docs/research/cod11-combat.md 14.8). `probe_dmgtrig` needs no client: it
watches all seventeen bushes and sets off script blasts on two of them.

```
COD_VERSION=uo COD_DIR=<UO install> COD_LNXDED_HOME=<absolute, no '+'> \
    PORT=33001 SECS=30 tools/run_probe.sh client-probes/probe_dmgtrig mp_uo_stanjel
```

`probe_dmgshot` holds one client off the first bush and has him shoot,
miss, swing and throw a frag at it:

```
COD_VERSION=uo ... PORT=33003 SECS=60 \
    tools/run_probe.sh client-probes/probe_dmgshot mp_uo_stanjel
# 6 s later:
cargo run -p vcod -- --game-dir <UO install> --mod-dir uo \
    --net-probe 127.0.0.1:33003 --probe-team axis --probe-weapon kar98k_mp \
    --probe-uo-weapon direct --probe-secs 48
```

`probe_dmgtouch` runs the same client on mp_uo_stanjel through a frag's
flight with `enableGrenadeTouchDamage` on and off and two panzerschreck
shells; `probe_belltower` runs it on mp_foy against the belltower, the
last shell from inside the belfry. Each takes `SECS=75` and
`--probe-secs 62` with the same command lines (map `mp_foy` for the
second). A gdb breakpoint on `G_CheckHitTriggerDamage` (run the server
binary under `gdb -batch` through `COD_LNXDED`, and read the arguments
off `$ebp+8` on) prints each walk's segment.

`probe_dmgspawn` needs no client: it spawns a `trigger_damage` from
script on any 1.1d map and blasts at it twice.

Retail's lines are in `crates/server/tests/dmgtrig_ab.rs` (the
belltower's, the touch walks') and `cod11-combat.md` 14.8 (the spawn).

## probe_istouching

`isTouching` against a standing, crouched, prone and dead player
(`docs/research/cod11-gsc-object-model.md` 22.3): one direct-mode client
on mp_carentan, the map's trigger_hurt swept past his box in one frame
per face. `COD_VERSION=1.5` or `uo` (with that install as `COD_DIR` and
the client's `--game-dir`, `--mod-dir uo` for UO) runs the same recipe.

```
COD_DIR=<1.1 install> COD_LNXDED_HOME=<absolute, no '+'> PORT=34671 SECS=40 \
    tools/run_probe.sh client-probes/probe_istouching mp_carentan
# 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:34671 --probe-team axis \
    --probe-weapon kar98k_mp --probe-uo-weapon direct --probe-secs 30
```

Retail's logs are `crates/server/tests/fixtures/istouching/`, replayed by
`crates/server/tests/istouching_ab.rs`.

## probe_threadorder

Which order a frame runs the threads one client packet woke
(`docs/research/cod11-gsc-language.md`, "The packet pass"). One plain
probe spawns from the script and sends its client commands in pairs; two
commands with one `--probe-say` time ride one packet.

```
COD_DIR=<1.1 install> COD_LNXDED_HOME=<absolute, no '+'> PORT=33681 PROBE_SECS=45 \
    tools/run_probe.sh client-probes/probe_threadorder mp_pavlov
# second shell, 8 s later:
cargo run -p vcod -- --net-probe 127.0.0.1:33681 --probe-secs 24 \
    --probe-say "6:mr {sid} 0 a" --probe-say "9:mr {sid} 0 b" --probe-say "9:kill" \
    --probe-say "12:kill" --probe-say "12:mr {sid} 0 c" \
    --probe-say "15:mr {sid} 0 d" --probe-say "15:mr {sid} 0 e"
```

The two-client run (server as above with `PORT=33683 PROBE_SECS=70`)
starts two probes at the same moment with only `kill`s, at 6, 9, ... 48 s on one and the same plus 17 ms more each round
on the other, so some rounds put both packets in one frame:

```
for k in $(seq 0 14); do A="$A --probe-say $((6+3*k)):kill"; \
    B="$B --probe-say $(python3 -c "print(f'{6+3.017*$k:.3f}')"):kill"; done
cargo run -p vcod -- --net-probe 127.0.0.1:33683 --probe-secs 53 $A &
cargo run -p vcod -- --net-probe 127.0.0.1:33683 --probe-secs 53 $B
```

Retail's lines, 2026-10-20, are
`crates/server/tests/fixtures/threadorder/mp_pavlov.txt` and
`mp_pavlov-pair.txt`, which `threadorder_ab.rs` replays on ours. It needs a client, so
`capture_probes.sh` and `semantics_ab.rs` cannot run it. The same two runs
on 1.5b (`COD_VERSION=1.5`, `COD_DIR` and the probes' `--game-dir` at a
1.5 install) and UO 1.51b (`COD_VERSION=uo`, the UO install, the probes
with `--mod-dir uo`), 2026-10-20, are `mp_pavlov-1.5.txt`,
`mp_pavlov-pair-1.5.txt`, `mp_pavlov-uo.txt` and `mp_pavlov-pair-uo.txt`
beside them: 1.1d's lines in 1.1d's order. On UO's pair run the second
probe to connect began first; the gate renumbers each log's clients in
the order they began.

## probe_menuclock

The clock a spawn woken by a menu answer runs on, against the client's
first playing snapshot (`docs/research/cod11-gametypes-re-bel.md` 8.7).
The player sits as a spectator until his `mr`; each `kill` respawns him
from the callback 0.5 s later. `sv_floodProtect 0` keeps the scripted
capture's `score`s from dropping a `kill`.

```
[COD_VERSION=1.5|uo] COD_DIR=<install> COD_LNXDED_HOME=<absolute, no '+'> PORT=34025 PROBE_SECS=40 \
    tools/run_probe.sh client-probes/probe_menuclock mp_pavlov +set sv_floodProtect 0
# second shell, 10 s later (1.5: --capture-tag 1.5; UO: --mod-dir uo --capture-tag uo):
vcod --game-dir <install> --net-probe 127.0.0.1:34025 --probe-secs 18 --save-scripted first \
    --probe-say "6:mr {sid} 0 a" --probe-say "9:kill" --probe-say "12:kill"
```

The capture lands in `crates/server/tests/fixtures/gametypes/`; the
committed ones (2026-10-20, 1.1d, 1.5b, UO 1.51b) moved to
`crates/server/tests/fixtures/threadorder/menuclock-first*.txt`, the
server's `PROBE` lines beside them as `menuclock-log*.txt`, and
`menuclock_ab.rs` replays them. All three print the same offsets: each
pick on its first playing snapshot's time, each placing 50 later.

## probe_satchelshot

United Offensive only. Two satchel charges thrown on mp_cassino, then a
sighted kar98k round into the second; with `probe_shoot 0` no round, and
the first's fuse sets the second off (`docs/research/coduo-diff.md` 21,
"Satchel"). The script presses the buttons through `PROBE_IN` lines.

```
COD_VERSION=uo COD_DIR=<UO install> COD_LNXDED_HOME=<absolute, no '+'> PORT=33931 PROBE_SECS=50 \
    tools/run_probe.sh client-probes/probe_satchelshot mp_cassino [+set probe_shoot 0 +set probe_turn 10]
# second shell, 18 s later:
vcod --game-dir <UO install> --mod-dir uo --net-probe 127.0.0.1:33931 --probe-team allies \
    --probe-weapon thompson_mp --probe-uo-weapon direct --probe-secs 28 \
    --save-snapshots --capture-tag uo-satchelshot
```

Retail's lines, 2026-10-20, are
`crates/server/tests/fixtures/missiles/mp_cassino-satchel{shot,chain}-uo.txt`
with the snapshot pairs `*-uo-satchel{shot,chain}.bin`, which
`uo_weapons_ab.rs` replays on ours.

## probe_pronecrouch

A crouch then prone press on probe_legs' flat floor, the same keys under a
`setOrigin` hold at mp_carentan's tdm spawn (844 3480) facing 180 and 135,
and a body let go there alive and dead (`docs/research/cod11-mantle.md`,
"Prone under a setOrigin hold"; `cod11-combat.md` 5.7). Run it at the net
probe's default 60 Hz cmds: the hold's outcome and the corpse's rest depend
on the cmd split.

```
[COD_VERSION=1.5|uo] COD_DIR=<install> PORT=34715 SECS=95 \
    tools/run_probe.sh client-probes/probe_pronecrouch mp_carentan
# second shell, 6 s later:
vcod --net-probe 127.0.0.1:34715 --game-dir <install> [--mod-dir uo] --probe-team allies \
    --probe-weapon m1carbine_mp --probe-uo-weapon direct --save-slope --slope-every-state \
    --capture-tag pronecrouch-<server> --probe-secs 82
```

`crates/server/tests/fixtures/pronecrouch/mp_carentan-{1.1,1.5,uo}.txt` are
the 2026-10-21 captures, cut to the fields `pronecrouch_ab.rs` reads, with
the server's `PROBE` lines.
