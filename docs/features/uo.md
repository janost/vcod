# Call of Duty: United Offensive 1.51

United Offensive patch 1.51, protocol 22: CoD1 with tanks, jeeps, sprint
and a flamethrower, and vcod does all four. It runs on the CoD1 code path
with 1.5's movement and wire as its base, so everything in
[cod1.md](cod1.md) applies unless this page says otherwise. The other
games and what they all share are in [../features.md](../features.md).

Run both binaries from the UO install with `--mod-dir uo`: vcod reads
`uo/` over the 1.5 `main/` beside it.

## What works

### Client

Joining and watching:

- Connects speaking UO's protocol. A UO install refuses a 1.1 or 1.5
  server, and the others refuse a UO one.
- The browser's Internet source asks `master.cod.pm` on port 20610 for
  protocol 22.
- Loads UO's maps and models from `uo/` over `main/` and draws players,
  items, script models, the stock UO script menus, the kill feed and the
  events UO shares with 1.1 ([coduo-diff.md](../research/coduo-diff.md)
  18).
- UO's own events play their effects and sounds: bullet impacts by ammo
  type, artillery, smoke and tank explodes, deploy, fatigue, overheating,
  the gunner's fire, the MG fire loop.
- Vehicle engines and MG loops crossfade as UO's sound blend entities do;
  tank shells whizz in flight.
- A vehicle MG's rounds draw the impacts and tracers UO's cgame traces
  itself and stop on vehicles. Tracers follow UO's length gate and
  `sv_night` width.
- The flamethrower streams flame that bounces off bodies, leaves burning
  residue, and reports its burns on you to the server (`fdc`).
- efx camera shakes shake the view. The HUD adds the sprint and fatigue
  icon, vehicle status icons and crosshair, and vehicles on the compass
  (coduo-diff.md 19).
- Every body's spine and lean ease toward the view as UO's do.
- `record` writes `.dm_3`; `cg_autodemo 1` records on the server's
  `autoDemoStart`.

On foot:

- `+sprint` (Alt, rebindable) sprints with UO's gun pose, breaths and
  footsteps.
- An LMG's sight deploys its bipod on sandbags, at a `trigger_mount` or
  prone.
- The sight cooks a pulled grenade.
- Prediction runs UO's movement: 1.5's plus sprint, the fatigue bar, the
  bipod's locks and yaw cap, the cook and automatic fire, replayed against
  retail captures ([coduo-movement.md](../research/coduo-movement.md)).
- The weapon-slot strip has UO's satchel and binocular slots.
- On a mounted MG the view stays in the gun's arcs, the eye sways and
  shakes, and a hit doesn't kick it.

Vehicles:

- Tanks and jeeps draw posed as UO's cgame poses them: hull tilt, turret
  and barrel, the gunner's gun, the jeep's steering wheel (turned four
  times as far under your own hands) and front wheels.
- Treads and tyres scroll with travel; wheels kick up the ground's dust,
  never over another vehicle (`cg_vehicletrails`).
- A seated player rides UO's seat blend, the jeep driver's steer included,
  head turned to the view ([coduo-vehicles.md](../research/coduo-vehicles.md)
  9).
- The use key gets in and out, the weapon wheel changes seats, the stock
  keys drive, Space turns a tank's hull to the view, and the main and
  alternate guns fire.
- The camera sits where UO's does: the tank driver's chase camera and
  gunsight, the gunner on his gun with sway and firing shake, a jeep's
  riders turning with it. Your own body draws in the seat.
- Prediction holds a seat's view clamps and sight, replayed against retail
  captures.

### Server

- Hosts UO from a UO install with `--mod-dir uo`: UO's queries, cvars,
  configstring layout, 63 weapons, means of death and event numbering,
  and the stock `dm`, `tdm`, `sd`, `ctf`, `dom`, `hq` and `bas` scripts.
- The join, a spawned player's playerstate and a kar98k's shots and
  reload match the retail 1.51b server (coduo-diff.md 20).
- Players sprint, tire, deploy the bipod and cook grenades by UO's rules.
  An LMG shows the mount hint and deploys on sandbag brushes and on a
  script's `spawnTriggerMount` (coduo-movement.md 11).
- Artillery strikes, smoke grenades, satchel charges, the flamethrower's
  reported damage and mounted MG overheat run UO's rules. The strike, the
  smoke, the bipod MG, the satchel and the overheat match retail captures,
  and so does a satchel shot off or set off by another's blast
  (coduo-diff.md 21).
- The bazooka and panzerschreck fire rockets like the panzerfaust's. A
  missile at the sky flies on through it on UO. UO frees a smoke grenade
  dropped in water; a grenade that goes off under water goes off on the
  surface. A rocket or a swing hurts a `setTakeDamage` model directly.
- The four UO modes' configstrings match retail's, and so do: a `ctf` flag
  pickup and capture, a `dom` flag taken and retaken, an `hq` radio
  captured and neutralized, a `bas` breach, with HUD, compass and
  scoreboard.
- Endings match too: `ctf`'s flag drop and 20 s return, `dom`'s domination
  bonus, `hq`'s wave scoring, `bas`'s plant, defuse and blast; round end,
  score and round limits, intermission and the restart `exitLevel` falls
  back to; a defender's touch returning the flag; the round clock and time
  limit in `ctf`, `dom`, `bas`, `hq` and `tdm`; `tdm`'s killcam; a
  rotation a script sets (coduo-diff.md 22).
- `sv_wwwDownload` redirects again after a failed www transfer, as UO's
  server does.

Vehicles ([coduo-vehicles.md](../research/coduo-vehicles.md) 10-14):

- Tanks and jeeps spawn with retail's entity numbers and configstrings.
  The use key boards the one you look at; `nextvehslot` / `prevvehslot`
  change seats.
- They drive, aim, fire shells and MGs, overheat, sound, take damage and
  run players over by the 1.51 game module's rules. A tank's driver is
  shut against every round but his own tank's.
- Hulls posed per bone stop rounds, grenades, shells and blasts. A jeep
  takes bullets and a struck vehicle takes a shell's whole damage. One
  driven into another shoves it; one wedged seven seconds destroys
  itself (retail's rule, not mine), and the wreck respawns.
- A vehicle's death puts its riders out: each takes the 5-point probe hit
  through his damage callback, a survivor looks for a dismount spot, and
  the corpses slide out of the hull.
- A body inside a vehicle is pushed out before its move. A moving vehicle
  runs him over (`vehicle_hit_person`); a prone one under it takes 10 a
  cmd; a driverless one is itself the attacker.
- A seated driver's and gunner's body meets a level ray where retail's
  does, to a quarter unit. A Panzer IV's and a Willys's drives replay on
  vcod-server as on retail to the field, seat changes too. A vehicle and
  its sound blends leave a client's snapshots where retail's do.
- Map triggers include UO shells: a `trigger_damage` fires off them,
  measured on mp_uo_stanjel and mp_foy's belltower.

### Bots

Everything from [cod1.md](cod1.md#bots), plus ([bots.md](../bots.md)):

- The nav graph is proven under UO's movement and rebuilt when the script
  deletes walls, as `bas` does to a breached base.
- They sprint long straight legs while they have the breath, pick from
  UO's weapon menus and deploy an LMG's bipod to fire it.
- They cook frags to burst near landing, throw only along a flight clear
  of bodies, and run from live ones.
- Objectives: `sd`; `ctf` (take, carry, return); `dom` (stand in the flag
  zones); `hq` (take and neutralize the radio); `bas` (fetch a bazooka or
  panzerschreck, breach a base with rockets from a spot with a shot at
  it, plant on a breached base, defuse).
- Vehicles (`vc_bot_vehicles`): a driver steers along the graph over
  ground its hull fits and waits for a teammate in its way; a tank driver
  stops to lay the turret on an enemy and fire; a gunner fires the MG; a
  bot on foot with a launcher fires at an enemy-crewed vehicle.
- A bot jumps through a window too low for a standing body with crouch
  held in the air (mp_cassino's axis room, two rooms on mp_streets).

### Tools

- `COD_VERSION=uo tools/run_server.sh` runs the retail 1.51b Linux
  server; the net probe joins it with `--mod-dir uo`. `--probe-vehicle`
  and `--probe-uo-weapon` capture vehicles and UO weapons.
- `UO=1 tools/cod1_soak.sh` soaks every UO map.

## Missing compared to retail

Everything under [cod1.md](cod1.md#missing-compared-to-retail) applies:
no listen server, no PunkBuster, no screenshots, the UI DLL ignored, no
authorize server, rcon's subset.

- Audio has had little checking by ear, tank engines included.

## Beyond retail

The renderer extras, widescreen, F3 overlay, `--vanilla` and the asphalt
footsteps from [cod1.md](cod1.md#beyond-retail) all apply. Also:

- Bots, vehicle crews included. Retail has none. They wedge a tank into a
  wall now and then, which is period-accurate.
