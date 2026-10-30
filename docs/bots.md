# Bots

vcod-server can fill a server with bots in all four games: CoD 1.1 and
1.5, United Offensive, CoD2 1.3 and CoD4 1.7. Retail has none, so
everything here is vcod's own, faults included. A bot is a real client: it takes a slot,
joins through the stock menus, and shows in the scoreboard and in
`status` like anyone else. Retail clients see bots as ordinary players.

This page covers running bots, [how a bot thinks](#how-a-bot-thinks) and
[what each game's bots do](#per-game). Navigation is in
[research/bot-navigation.md](research/bot-navigation.md), objective play
in [research/bot-objectives.md](research/bot-objectives.md), combat and
the measuring harness in [research/bot-combat.md](research/bot-combat.md).

## Quick start

Six fighting bots:

```
vcod-server mp_carentan --gametype tdm --bots 6 --bots-shoot --set rconPassword=secret
```

The same on the other games (the install picks the game, or pass
`--game`):

```
vcod-server mp_cassino --mod-dir uo --gametype tdm --bots 6 --bots-shoot
vcod-server mp_toujane --game cod2 --gametype tdm --bots 6 --bots-shoot
vcod-server mp_crash --game cod4 --bots 6 --bots-shoot
```

A server that keeps itself busy: with a floor of 10, an empty server runs
a 5v5 of bots, and each human who joins takes a bot's place.
`--max-clients` defaults to 8, so raise it above the floor.

```
vcod-server mp_carentan --gametype tdm --max-clients 16 --set vc_bot_minplayers=10 --set rconPassword=secret
```

Everything below also works on a running server over rcon, from a retail
or vcod console: `rconPassword secret` then `rcon <command>` on CoD1 and
UO, `rcon login secret` then `rcon <command>` on CoD2 and CoD4.

## Command line

| Flag | Does |
|---|---|
| `--bots N` | Start with N bots. Each takes a real slot, so N can't exceed `--max-clients`. |
| `--bots-shoot` | Let those bots fire. Without it they walk the map and play objectives but never shoot (`vc_bot_shoot 0`). |
| `--set NAME=VALUE` | Set any cvar below before the first map loads. |

The bots join once the level is up. The navigation graph builds on its
own thread on the first tick, a few seconds on a big map; bots wander
until it is ready.

## rcon commands

rcon is refused until the server has a password (`rconPassword`;
`rcon_password` on CoD2 and CoD4).

| Command | Does |
|---|---|
| `addbot [allies\|axis\|auto] [count] [skill N]` | Adds `count` bots (default 1). The words can come in any order. `auto`, the default, puts each bot where balance asks. A named team puts the bots there and pins them: balance never moves them. `skill N` (0-3) fixes their [skill](#skill); `vc_bot_skill` then leaves them alone. |
| `removebot [name\|all\|count]` | No argument removes one bot. A name removes that bot (the `[BOT] ` prefix optional, case ignored), `all` every bot, a number that many, lowest score first. CoD2 and CoD4 also take `kickbot`. |
| `listbots` | Every bot's slot, team, score, pin, skill level and name, then the human and bot counts and the `vc_bot_*` values. |

```
rcon addbot 4
rcon addbot axis 2
rcon addbot allies 3 skill 3
rcon removebot NadeSpammer
rcon removebot all
rcon listbots
```

## Cvars

Set them with `rcon set <cvar> <value>` (or `seta`), or `--set` at start.
A change applies within half a second.

| Cvar | Default | Does |
|---|---|---|
| `vc_bot_minplayers` | 0 | [Minimum player count](#minimum-player-count). 0 is off. |
| `vc_bot_autobalance` | 1 | [Keeps the teams even](#team-balance) in team gametypes. |
| `vc_bot_shoot` | 1 | 0 stops bots firing. `--bots` without `--bots-shoot` sets it to 0. |
| `vc_bot_names` | 1 | 0 names new bots `bot<slot>`. |
| `vc_bot_skill` | 1 | The [skill level](#skill), 0 to 3, of every bot `addbot ... skill N` didn't set. |
| `vc_bot_vehicles` | 1 | United Offensive: 0 stops bots getting into vehicles; a bot already in one rides on until it gets out. |

## Skill

Four levels set how fast a bot reacts and turns, how well it aims, how
long it remembers a lost enemy, how far it hears and how wide it sees
(`crates/server/src/bots/skill.rs`):

| Level | Name | Reaction | Turn rate | Walking turn | Aim error at first sight, settled | Hearing | Sight cone |
|---|---|---|---|---|---|---|---|
| 0 | easy | 400-650 ms | 200°/s | 360°/s | 64, 5 units | 0.75x | 100° |
| 1 | normal (default) | 200-350 ms | 300°/s | 480°/s | 36, 4 units | 1x | 120° |
| 2 | hard | 150-300 ms | 440°/s | 600°/s | 32, 2.5 units | 1.25x | 140° |
| 3 | expert | 100-150 ms | 600°/s | 720°/s | 24, 1.5 units | 1.5x | 160° |

- The aim error is how far from the target's chest a shot would land. It
  shrinks every tick the bot stays on the target and grows with range,
  with the target's speed across the view, and while the bot runs.
- A higher level also moves on its sights more often (a slow strafe, a
  bolt rifle's back-off, a lie-down at range) and remembers a lost enemy
  and a heard shot longer.
- The levels change only these numbers. Routes, objective play and
  choices are the same at every level: an expert is faster and sharper,
  not smarter.
- `vc_bot_skill` changes every bot without its own level at once,
  mid-fight included. To mix levels, give each `addbot` its own:

```
rcon addbot axis 4 skill 0
rcon addbot allies 4 skill 3
```

How the levels play against each other: [research/bot-combat.md](research/bot-combat.md) section 3.

## Minimum player count

With `vc_bot_minplayers` at N the server keeps N players in the game,
topping up with bots:

- No humans: N bots, split across the teams.
- A human picks a team: one bot leaves, off the larger team.
- A human leaves: a bot comes back.
- N or more humans: no floor bots.

A human counts only once they pick a team; a spectator or someone in the
team menu does not push a bot out. Bots from `addbot` or `--bots` count
separately: `removebot` lowers that count, but the floor still tops the
server back up to N.

## Team balance

With `vc_bot_autobalance 1`, in every team gametype (all but `dm` and
1.1's `bel`), new bots join the smaller team and bots move so the teams
differ by at most one.

- Humans never move.
- Bots pinned with `addbot allies` or `addbot axis` never move.
- A bot switches only while dead or spectating. In round gametypes like
  `sd` that usually means between rounds.

## Slots

- Bots never take a private slot (`sv_privateClients`).
- `addbot` may fill every public slot; the minimum player floor always
  leaves one free, so a human can still join and push a bot out.
- `sv_maxclients` (up to 64) counts bots. It is latched, as on retail:
  `rcon set sv_maxclients 32` then `rcon map_restart`. Players stay
  connected through the restart.

## Names

A bot is named `[BOT] <name>`, drawn from 120 names in
`crates/server/src/botmgmt.rs` (`[BOT] Sir Camps-a-Lot`, `[BOT]
NadeSpammer`, `[BOT] PronePatrol`). No two connected bots share a name;
past the pool's unused names a number is added (`[BOT] NadeSpammer 2`).
Every name fits retail's 31-byte limit with the prefix and a two-digit
number. The draw has its own random generator, so `vc_bot_names` never
changes how a seeded server plays. `status` prints a bot's address in
retail's form for a test client.

## How a bot thinks

The brain is hand-written rules: no learning, no per-map script. All four
games share one brain (`crates/server/src/bots.rs`).

### The loop

Each server frame (50 ms) the server fills a `BotView` for every bot and
calls `Bot::think`, which returns one usercmd: move keys, view angles,
buttons and the selected weapon. The server runs that cmd through the
game's own `ClientThink` and pmove, the path a human's cmd takes.

`think` reads the view and nothing else. It cannot move the body, see
through a wall or skip a physics rule, so a player could do anything a
bot does with the same keys. If one kills you, it earned it. The view holds:

- the bot's own state (health, ammo, weapons, stance, ground, ladder)
- the enemies a line-of-sight trace reaches inside its sight cone
- gunfire within its hearing range
- the objective's state for the gametype
- probes the brain asks for on some ticks, such as a drop or a lob ahead

### What a tick decides

`think` stops at the first case that applies:

1. Dead: waits 1.5 s, then presses use once a second until the stock
   script respawns it.
2. Spectating or at intermission: presses nothing.
3. In a UO vehicle seat: the vehicle code drives or guns.
4. Stuck: a bot that has stood 10 s where the graph knows no way out (a
   dead end, a pit, a ring of parked vehicles), or has had nothing to
   fire for 10 s, kills itself to respawn. Where a death sits out the
   round (`sd`, `re`) it holds on instead.
5. A grenade throw in progress: switch to the grenade, pull back, cook
   where the game has a cook, throw, switch back.
6. Otherwise it fights if it has an enemy, plays the objective if there
   is one, and roams toward fights if not.

### Fighting

- It spots a new enemy only inside its sight cone; a bot in a vehicle
  seat sees all round. It keeps the enemy it fights while he stays in
  sight; a new one takes over only under 60% of the current one's range
  (`RETARGET_RATIO`).
- Whoever just hit it comes first. Hit by someone it cannot see, it
  turns toward the shot. Gunfire it hears draws it from any direction.
- A new target costs a reaction delay; switching back to a recent enemy
  or onto a fresh attacker costs less.
- It fires before its aim settles: automatics spray early, bolt rifles
  wait for a better line. On CoD1 and CoD2 it aims by where the gun
  points, sway and recoil included.
- Around the shooting it strafes on its sights, backs off a close enemy
  with a bolt rifle, lies down at long range, takes cover and leans,
  retreats when hurt, and peeks for or hunts an enemy it lost.
- It throws frags at an enemy behind cover or one just lost, on a lob
  whose arc comes down by him, and runs from a live frag near it.

### Getting around

- At map load the server builds a navigation graph. It floods the
  walkable space from every spawn point; each link (walk, jump, drop,
  ladder, mantle) goes in only after a simulated body has travelled it
  under the game's movement rules.
- A* plans over the graph. Searches share a per-tick budget and may take
  several ticks, so many bots planning at once do not spike the frame.
- The follower turns the path into keys: it steers at the next node,
  jumps by the rule that proved the jump, waits out the jump cooldown,
  climbs at the rate the ladder was proven at, and takes a risky drop
  only when no other route exists. Out of a fight the view turns at the
  walking rate while the keys keep the path; on open floor it heads
  straight for a node further along.
- A teammate standing still in the way, or a parked UO vehicle, blocks
  the nodes it covers for a while, and the bot plans round it. On CoD2
  and CoD4 a bot held up for a second plans round the teammate for 10 s,
  and the teammate steps aside.

### Objectives

Each gametype has its own goal rules: take and carry the bomb or flag,
plant and defuse, capture and hold a flag or the radio. The objective
decides where the bot walks; an enemy in sight takes over, and the bot
goes back after the fight. A team spreads its bots over different
routes, reshuffles who guards what each round, sends all its S&D
attackers to one site and covers a planter
([research/bot-objectives.md](research/bot-objectives.md) section 9).

### One brain, four games

A thin adapter per game sits between the brain and the server. It
rewrites the cmd into the game's button bits, renumbers the game's
weapons for the brain, answers the game's menus and fills the view from
the game's playerstate. Behaviour differs by game, never by map: the
`BotWorld` trait in `crates/server/src/bots/driver.rs` holds the
differences as constants each game sets (`YIELDS`, `PMOVE_PROBES`,
CoD4's `MANTLE_RESTS` and `PARTS_MATES_FLAT`).

### How a change is judged

Retail has no bots, so for once there's no oracle. A behaviour
change runs `bot_match` on the same seeds before and after and lands
only when stalls drop and kills hold:

```
cargo run --release -p vcod-server --example bot_match [seed...]
```

`MAPS=`, `GAMETYPE=`, `BOTS=`, `TICKS=`, `SET=`, `UO=1`, `GAME=cod2` or
`GAME=cod4`, per-side `AXIS_SKILL=` / `ALLIES_SKILL=` and more;
[research/bot-combat.md](research/bot-combat.md) section 1 explains the
switches and every number it prints. Results go in that doc and
[research/bot-objectives.md](research/bot-objectives.md).

## Per game

### CoD 1.1, 1.5 and United Offensive

- Gametypes: every stock one, `dm`, `tdm`, `sd`, `re` and `bel`, and UO's
  `ctf`, `dom`, `hq` and `bas`.
- Objectives: S&D plants and defuses; in Retrieval the team escorts the
  carrier and holds the goal ahead of him; UO's flags, radio and base
  breaches as in [research/bot-objectives.md](research/bot-objectives.md)
  sections 4 to 7.
- UO grenades are cooked; 1.1 has no cook.
- UO sprint: a bot sprints only on a level, straight run with no jump or
  drop.
- UO vehicles: a bot with no fight and no objective close by walks to a
  vehicle near it and gets in with the use key. A driver drives toward
  its objective or an enemy, waits a moment for a teammate walking up to
  board, holds or backs off for a teammate in its way, and in a tank
  stops to lay the turret on an enemy or enemy vehicle and fire. A gunner
  fires the vehicle's machine gun. A bot on foot holding a rocket
  launcher fires it at an enemy-crewed vehicle in sight. Bots get out
  when they arrive, when stuck, or after 90 s (bot-objectives.md
  section 10).

Cannot:

- Drive anywhere but along the infantry graph over ground the hull fits,
  so a tank can still wedge itself. A driver backs off and gets out after
  four stalls or 8 s without headway; a rider with nobody to shoot gives
  up after the same 8 s; every rider leaves a hull that has scraped
  something for 5 s, before the engine's 7 s stuck kill. A tank sliding
  into a wall with nobody aboard still destroys itself.
- Change seats, use the hatch, or run an enemy over.
- Fetch a rocket launcher outside `bas`.

### CoD2

- Gametypes: every stock one. Objective play in `sd`, `ctf` and `hq`:
  attackers plant in a bombzone and the lead defender defuses, both by
  holding use in the `trigger_use_touch`; a `ctf` bot takes the other
  flag, returns its own and captures at home; an `hq` bot takes the radio
  up and holds it (bot-objectives.md section 11).
- Menus: `serverinfo_<gametype>` is answered `close`; the team menu
  `autoassign` in `dm`, else alternately `allies` and `axis`; the weapon
  menu with a weapon its nationality offers and the `scr_allow_*` cvars
  allow, the one the fewest other bots carry, so a side spreads over the
  menu.
- A new level (`map`, a passed vote, `map_restart`, `fast_restart`,
  `map_rotate`, a level end) keeps the bots; each answers the menus the
  level opens again. A new map builds a new graph. An `sd` round opens no
  menu and the bots play on.
- Bots never vote and are not counted as voters, so one player's
  `callvote` plus his own yes passes on a server full of bots
  ([cod2-protocol.md](research/cod2-protocol.md) 3.4.2). Democracy, of a
  kind.
- Movement: the graph floods CoD2's collision (`SURF_LADDER` brushes as
  ladders) under 1.5's movement rules, which CoD2's starts from. It keeps
  no leap from a standstill. Every node is tried for a mantle in eight
  directions with CoD2's own pmove; each climb that lands is a mantle
  edge (mp_toujane 352, mp_carentan 334 in `dm`).
- Weapons, read off the weapon file: a bolt rifle's cycle is `fireTime`
  plus `rechamberTime` (kar98k 1330 ms), so it fights as a bolt rifle;
  SMGs and automatic rifles fire bursts (2 to 7 ticks close, 1 to 3 past
  800 units), semi-automatics tap at `fireTime`. Fully down a scope with
  an enemy in sight it holds its breath for 3 s, short of the 4.5 s gasp.
- Frags: the offhand frag key; CoD2's stock frag has no cook.
- Smoke: a bot with a smoke grenade (the stock SMG and shotgun kits)
  throws it a third of the way toward an enemy 400 to 1500 units off (at
  least 250 out, 150 short of him), when it heads for a plant or defuse
  within 800 units, covers a planting teammate, or starts a retreat. From
  2 s to 35 s after the burst a cloud 150 units round and 48 up blocks
  every bot's sight. Retail's crosshair smoke test (`SV_FX_GetVisibility`)
  is not ported, so a human's crosshair names are not blocked.
- Cover: it peeks with CoD2's leans (0.5 of the lean standing, as 1.5).
  A hit is read off the playerstate: a new `damageEvent`, `damageYaw` and
  `damagePitch`, the attacker the last client to hurt the bot.
- Mounted guns, in `dm` and `tdm`: a bot that sees an enemy in a free
  gun's arc, past 250 units from the gun, with a clear walk to the spot
  behind it, mans it, fires on enemies in the arc and lets go after 4 s
  with none (bot-combat.md section 14).
- Dropped guns: with every primary dry and no enemy in sight it walks to
  the nearest loaded gun lying within 600 units, the dry one in hand, and
  takes it with use; a gun not taken in 6 s is passed over for 20 s.

Measured (`MAPS=mp_toujane,mp_carentan,mp_burgundy`, seeds 1 and 2, six
bots, 6000 ticks, `dm`, `tdm` and `sd`, 2026-10-23): 725 kills; hits per
round 0.23 for SMGs and automatic rifles, 0.31 semi-automatics, 0.36 to
0.37 bolt and scoped rifles; each skill level kills more than the one
below and an expert side takes 70 to 84% of a `tdm`'s kills. Accuracy
does not rise with the level: on CoD2's sights the gun's spread and kick
are most of a miss. Blame the gun. Full tables and the earlier runs:
[research/bot-measurements-cod2-cod4.md](research/bot-measurements-cod2-cod4.md).

### CoD4

- Gametypes: `war`, `dm`, `sd`, `sab`, `dom` and `koth`. Bots enter a
  fresh level 3 s past its first frame, as a loading client does, since
  `koth`'s spawn script errors on an earlier spawn (as on retail).
- Menus: the team menu alternately `allies` and `axis` (`autoassign` in
  `dm`), then the class menu (`changeclass_marines`, `changeclass_opfor`)
  through `changeclass` with a stock class (assault, spec ops, heavy
  gunner, demolitions, sniper), the one the fewest other bots carry. A
  new level, round or rotation keeps the bots; each answers the menus
  again.
- Weapons: 1.7 numbers every attachment as its own weapon, past the
  brain's 63, so each bot's weapons are renumbered for its brain in the
  order it picked them up (`bots::cod4::Arms`). The cmd's moves are sent
  whole, as 1.7's wire carries only their sign.
- Objectives (bot-objectives.md section 12): in `sd` and `sab` the bot
  nearest a lying bomb takes it, the carrier plants at a zone through use
  and the briefcase while the rest guard it, the nearest enemy defuses a
  planted bomb and the planters guard it. In `dom` they take flags as in
  UO; in `koth` they take the radio by standing in it and guard it. A bot
  in a contested zone walks it until one side is dead or gone. A bot
  that reaches an objective's spot and gets no nearer in 3 s guards round
  it for 15 s before trying again.
- Movement: the graph floods the clip map under 1.5's movement rules and
  adds the mantles 1.7's own pmove climbs, probing again from each
  mantle's top (mp_crash: 8 458 nodes, 1 222 mantle edges, all 177 spawns
  in one component). It keeps a mantle only where a bot running straight
  at its landing is offered the climb, keeps no leap, and clips the
  level's solid brush entities. A bot crouches when a climb forces it
  (`EV_STANCE_FORCE_CROUCH`) and climbs on past a ladder's top.
  `VC_NAV_COD4=1` walks the graph under 1.7's own pmove instead; it kills
  0.4% more and stalls 3% more over six seeds of all maps, so it is off.
- Frags: thrown through the frag key. A live frag under its cursor hint
  with 1.5 s or more on the fuse is picked up and thrown back toward its
  thrower (or the enemy in sight).
- Mounted SAWs, in `war` and `dm` only: a bot takes a free gun whose arc
  covers the enemy it fights, or a lane its team saw an enemy in within
  8 s, past 250 units from the gun, when the spot behind it is within
  400 units, level and a clear walk. With no enemy for 20 s it may walk
  over the graph to a gun on another floor within 800 units (one bot a
  team, at most every 3 minutes). On the gun it turns 12 degrees a tick
  onto the nearest visible enemy in the arc and fires within 2 degrees.
  It lets go when flanked (an enemy outside the arc within 1 200 units)
  or after 3 s with nobody in the arc, then takes no gun for 20 s.
- Sight: an enemy behind a destructible car counts as seen only where a
  round would get through (`Cod4Bots::shot_clear`): it traces the car's
  collision triangles and skips a part `hidePart` removed, so a bot
  shoots through an empty window frame. A round still goes through a
  car's intact panels at reduced damage, which the sight does not count
  on.
- A dead bot that follows a teammate is left alone.

Cannot:

- Call in hardpoints (UAV, airstrike, helicopter) or use special
  grenades.
- Compensate for gun sway; it holds its view on the enemy, which keeps
  the turn sway and movement tilt small.
- Make a few jump edges the graph proved under CoD1's movement rules;
  a bot tries them until it gives up (bot-objectives.md 12.9).

Measured (`war` and `dm` on mp_backlot and mp_carentan, six bots, 6 000
ticks, seeds 1 to 12, 2026-10-27): 2 284 kills, 8 637 hits, accuracy
0.24 to 0.31, no script error. Measure with `GAME=cod4
COD4_DIR=<install> cargo run --release -p vcod-server --example
bot_match` (`OBJ=0` keeps bots off objectives, `GUNS=0` and
`THROWBACK=0` turn those off, `LEAVE=<tick>,...` drops a bot at each
tick, `STALLS=1` prints stalled samples) and `--example nav_build` for
the graph. Full tables:
[research/bot-measurements-cod2-cod4.md](research/bot-measurements-cod2-cod4.md).

## Known limits

- A bot picks cover against the one enemy it fights; another may still
  see it there. It takes no cover while planting, carrying or holding a
  capture zone, nor within 300 units of the enemy.
- In cover it swaps between the spot's two ways out (both lean sides, or
  standing and a crouched lean) from one peek to the next, sometimes
  moves to the next spot while it reloads, and holds the spot for a time
  set by its skill once its enemy is out of sight. To chase a lost enemy
  it heads for a nearby spot that looks onto where he was, and gives up
  sooner the higher its skill.
